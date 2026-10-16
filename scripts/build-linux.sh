#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 NEW_OUTPUT_DIRECTORY" >&2
  exit 2
fi

root=$(cd "$(dirname "$0")/.." && pwd)
output=$(realpath -m "$1")
if [[ -e "$output" || "$output" == "$root" || "$output" == "$root/"* ]]; then
  echo "Output must be a new directory outside the checkout" >&2
  exit 2
fi
if [[ -n "$(git -C "$root" status --porcelain --untracked-files=all)" ]]; then
  echo "Build requires a clean committed checkout" >&2
  exit 2
fi

git_common=$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)
build_dir="$root/target/linux-release"
mkdir -p "$(dirname "$output")" "$build_dir/tmp" "$build_dir/cargo"
# Attestation timestamps would change the image ID recorded in each archive.
docker build --platform linux/amd64 --provenance=false --iidfile "$build_dir/image.id" \
  --file "$root/scripts/linux-release.Dockerfile" "$root/scripts"
image=$(cat "$build_dir/image.id")

# Worktrees keep their Git metadata outside the source mount.
docker run --rm --platform linux/amd64 --user "$(id -u):$(id -g)" \
  --mount "type=bind,source=$root,target=/work,readonly" \
  --mount "type=bind,source=$git_common,target=$git_common,readonly" \
  --mount "type=bind,source=$build_dir,target=/work/target" \
  --mount "type=bind,source=$(dirname "$output"),target=/output" \
  --env CARGO_HOME=/work/target/cargo --env TMPDIR=/work/target/tmp \
  --env "REVIEW_KIT_BUILD_IMAGE=$image" --workdir /work \
  "$image" python3 scripts/build-candidate.py --output "/output/$(basename "$output")"
