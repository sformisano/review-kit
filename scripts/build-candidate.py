#!/usr/bin/env python3
"""Build a binary archive with its exact source and dependency notices."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile


ROOT = Path(__file__).resolve().parent.parent
TOOLCHAIN = "1.98.1"
TARGET = "x86_64-unknown-linux-gnu"
FALLBACKS = {
    "jsonschema-regex": ("0.58.6", "jsonschema-LICENSE",
        "https://raw.githubusercontent.com/Stranger6667/jsonschema/55ac1664384793c5d3b20745d3f9c169dfe2c06f/LICENSE",
        "117829c3ca21efb132d81a44b55363d395ab8eea18526873bc828da4c0e5f038"),
    "jsonschema-value": ("0.58.6", "jsonschema-LICENSE",
        "https://raw.githubusercontent.com/Stranger6667/jsonschema/55ac1664384793c5d3b20745d3f9c169dfe2c06f/LICENSE",
        "117829c3ca21efb132d81a44b55363d395ab8eea18526873bc828da4c0e5f038"),
    "uuid-simd": ("0.8.0", "simd-LICENSE",
        "https://raw.githubusercontent.com/Nugine/simd/d74c030d9dc4f3cae02146d1f497ff62726ef09a/LICENSE",
        "71674605ec4c087fe9eb534e3e4f9e26eb2e4aabcd76a29fd156c6a844d44b3d"),
    "vsimd": ("0.8.0", "simd-LICENSE",
        "https://raw.githubusercontent.com/Nugine/simd/d74c030d9dc4f3cae02146d1f497ff62726ef09a/LICENSE",
        "71674605ec4c087fe9eb534e3e4f9e26eb2e4aabcd76a29fd156c6a844d44b3d"),
}


def run(args, *, env=None):
    return subprocess.check_output(args, cwd=ROOT, env=env, text=True).strip()


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def require_clean(commit=None):
    if run(["git", "status", "--porcelain", "--untracked-files=all"]):
        raise RuntimeError("candidate requires a clean committed checkout")
    current = run(["git", "rev-parse", "HEAD"])
    if commit is not None and current != commit:
        raise RuntimeError("source commit changed during candidate assembly")
    return current


def collect_notices(metadata, package):
    packages = {item["id"]: item for item in metadata["packages"]}
    nodes = {item["id"]: item for item in metadata["resolve"]["nodes"]}
    todo = [next(key for key, item in packages.items() if item["name"] == "review-kit-cli")]
    seen = set()
    while todo:
        key = todo.pop()
        if key in seen:
            continue
        seen.add(key)
        todo.extend(dep["pkg"] for dep in nodes[key]["deps"]
                    if any(kind["kind"] != "dev" for kind in dep["dep_kinds"]))
    notices = []
    for item in sorted((packages[key] for key in seen), key=lambda p: (p["name"], p["version"])):
        if item["source"] is None:
            continue
        source = Path(item["manifest_path"]).parent
        destination = package / "licenses" / f'{item["name"]}-{item["version"]}'
        destination.mkdir(parents=True)
        files = sorted(path for path in source.rglob("*") if path.is_file()
                       and path.name.upper().startswith(
                           ("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT", "AUTHORS")))
        license_files = [path for path in files if path.name.upper().startswith(
            ("LICENSE", "LICENCE", "COPYING"))]
        record = {"name": item["name"], "version": item["version"],
                  "license": item["license"], "repository": item["repository"],
                  "source": item["source"], "files": []}
        if not item["license"] and not item["license_file"]:
            raise RuntimeError(f'missing license declaration: {item["name"]}')
        if item["license_file"]:
            declared = source / item["license_file"]
            if not declared.is_file():
                raise RuntimeError(f'missing declared license: {declared}')
            if declared not in files:
                files.append(declared)
            license_files.append(declared)
        if not license_files:
            fallback = FALLBACKS.get(item["name"])
            if fallback is None or fallback[0] != item["version"]:
                raise RuntimeError(f'no license text for {item["name"]} {item["version"]}')
            path = ROOT / "scripts" / "license-fallbacks" / fallback[1]
            if sha256(path) != fallback[3]:
                raise RuntimeError(f"license fallback checksum mismatch: {path}")
            shutil.copyfile(path, destination / "LICENSE")
            record["fallback_source"] = fallback[2]
            record["files"].append({"path": "LICENSE", "sha256": fallback[3]})
        for path in files:
            relative = path.relative_to(source)
            if path.is_symlink() or not path.resolve().is_relative_to(source.resolve()):
                raise RuntimeError(f"license path escapes dependency: {path}")
            output = destination / relative
            output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, output)
            record["files"].append({"path": relative.as_posix(), "sha256": sha256(output)})
        notices.append(record)
    write_json(package / "THIRD-PARTY.json", notices)
    text = ["# Third-party notices", "",
            "This package retains license and notice files for the Linux dependency graph,",
            "including build dependencies. See THIRD-PARTY.json for versions and file checksums.", ""]
    for item in notices:
        text.extend([f'## {item["name"]} {item["version"]}', "",
                     f'License declaration: `{item["license"]}`.', "",
                     f'License files: `licenses/{item["name"]}-{item["version"]}/`.', ""])
    (package / "THIRD-PARTY-NOTICES.md").write_text("\n".join(text))


def archive(directory, output, epoch):
    with output.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as tar:
                for path in sorted([directory, *directory.rglob("*")]):
                    info = tar.gettarinfo(str(path), arcname=str(path.relative_to(directory.parent)))
                    if not info.isdir() and not info.isfile():
                        raise RuntimeError(f"unsupported package member: {path}")
                    info.uid = info.gid = 0
                    info.uname = info.gname = ""
                    info.mtime = epoch
                    info.mode = 0o755 if info.isdir() or path.name == "rkc" else 0o644
                    if info.isfile():
                        with path.open("rb") as content:
                            tar.addfile(info, content)
                    else:
                        tar.addfile(info)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path,
                        help="new directory outside the checkout for the local candidate")
    args = parser.parse_args()
    output = args.output.resolve()
    if output.is_relative_to(ROOT) or output.exists():
        raise RuntimeError("output must be a new directory outside the checkout")
    if run(["uname", "-s"]) != "Linux" or run(["uname", "-m"]) != "x86_64":
        raise RuntimeError("candidate build supports Linux x86_64 only")
    overrides = [key for key in os.environ if key in {
        "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC", "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_RUSTFLAGS", "CARGO_TARGET_DIR",
        "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER",
        "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS"}
        or key.startswith("CARGO_PROFILE_RELEASE_")]
    if overrides:
        raise RuntimeError("unset build overrides: " + ", ".join(sorted(overrides)))
    commit = require_clean()
    epoch = int(run(["git", "show", "-s", "--format=%ct", commit]))
    cargo = ["cargo", f"+{TOOLCHAIN}"]
    workspace = json.loads(run(cargo + ["metadata", "--locked", "--no-deps",
                                       "--format-version", "1"]))
    version = next(item["version"] for item in workspace["packages"]
                   if item["name"] == "review-kit-cli")
    name = f"review-kit-{version}-{TARGET}"
    output.mkdir(parents=True)
    package = output / name
    package.mkdir()
    command = cargo + ["build", "--release", "--locked", "--package", "review-kit-cli",
                       "--bin", "rkc", "--target", TARGET]
    env = dict(os.environ, CARGO_TARGET_DIR=str(ROOT / "target"))
    subprocess.run(command, cwd=ROOT, env=env, check=True)
    binary = ROOT / "target" / TARGET / "release" / "rkc"
    shutil.copyfile(binary, package / "rkc")
    (package / "rkc").chmod(0o755)
    metadata = json.loads(run(cargo + ["metadata", "--locked", "--format-version", "1",
                                      "--filter-platform", TARGET], env=env))
    collect_notices(metadata, package)
    tracked = run(["git", "ls-files"]).splitlines()
    for relative in tracked:
        path = Path(relative)
        source = ROOT / path
        if not source.is_file() or not source.resolve().is_relative_to(ROOT):
            raise RuntimeError(f"tracked package source is not an internal file: {path}")
        destination = package / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, destination)
    sysroot = Path(run(["rustc", f"+{TOOLCHAIN}", "--print", "sysroot"]))
    rust_notices = package / "licenses" / "rust-standard-library"
    rust_notices.mkdir()
    rust_docs = sysroot / "share" / "doc" / "rust"
    library_copyright = rust_docs / "COPYRIGHT-library.html"
    if not library_copyright.is_file():
        raise RuntimeError("install the toolchain rust-docs component for standard library notices")
    shutil.copyfile(library_copyright, rust_notices / library_copyright.name)
    for path in (rust_docs / "html" / "static.files").glob("LICENSE-*.txt"):
        shutil.copyfile(path, rust_notices / path.name)
    subprocess.run(["git", "archive", "--format=tar.gz", f"--output={package / 'SOURCE.tar.gz'}", commit],
                   cwd=ROOT, check=True)
    dynamic = run(["readelf", "-d", str(package / "rkc")])
    versions = run(["readelf", "--version-info", str(package / "rkc")])
    operating_system = {key: value.strip('"') for line in Path("/etc/os-release").read_text().splitlines()
                        if "=" in line for key, value in [line.split("=", 1)]
                        if key in {"ID", "VERSION_ID", "PRETTY_NAME"}}
    manifest = {"kind": "binary_archive", "version": version,
                "target": TARGET, "source_commit": commit, "source_tree": run(["git", "rev-parse", "HEAD^{tree}"]),
                "source_archive_sha256": sha256(package / "SOURCE.tar.gz"),
                "cargo_lock_sha256": sha256(ROOT / "Cargo.lock"),
                "binary_sha256": sha256(package / "rkc"),
                "binary_version": run([str(package / "rkc"), "--version"]),
                "build_command": command, "rustc": run(["rustc", f"+{TOOLCHAIN}", "--version", "--verbose"]),
                "cargo": run(cargo + ["--version"]), "cc": run(["cc", "--version"]).splitlines()[0],
                "operating_system": operating_system, "build_glibc": run(["getconf", "GNU_LIBC_VERSION"]),
                "dynamic_libraries": sorted(re.findall(r"\(NEEDED\).*\[(.*?)\]", dynamic)),
                "glibc_symbol_versions": sorted(set(re.findall(r"GLIBC_[0-9.]+", versions))),
                "build_image": os.environ.get("REVIEW_KIT_BUILD_IMAGE")}
    if manifest["binary_version"] != f"rkc {version}":
        raise RuntimeError("binary version differs from workspace version")
    write_json(package / "BUILD.json", manifest)
    files = {path.relative_to(package).as_posix(): sha256(path)
             for path in sorted(package.rglob("*")) if path.is_file()}
    write_json(package / "FILES.json", files)
    require_clean(commit)
    artifact = output / f"{name}.tar.gz"
    archive(package, artifact, epoch)
    (output / "SHA256SUMS").write_text(f"{sha256(artifact)}  {artifact.name}\n")
    print(artifact)
    print(output / "SHA256SUMS")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError, OSError) as error:
        raise SystemExit(f"candidate build failed: {error}") from error
