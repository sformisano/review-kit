use anyhow::{Context, Result, ensure};
use assert_cmd::Command;
use process_wrap::tokio::{CommandWrap, KillOnDrop, ProcessGroup};
use serde_json::json;
use std::{fs, os::unix::process::ExitStatusExt, path::Path, time::Duration};

use super::{private_file, root, write_json};

pub fn execute(
    directory: &Path,
    tokens: &[String],
    label: &str,
    command: &mut Command,
    expected: Option<i32>,
) -> Result<std::process::Output> {
    execute_in(directory, tokens, label, command, expected, &root())
}

/// Runs `command` from `working_directory` with the fixture's private home,
/// retains its output as evidence and rejects output containing a credential.
pub fn execute_in(
    directory: &Path,
    tokens: &[String],
    label: &str,
    command: &mut Command,
    expected: Option<i32>,
    working_directory: &Path,
) -> Result<std::process::Output> {
    let path = evidence_path(directory, label)?;
    command
        .current_dir(working_directory)
        .env("HOME", directory)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("RKC_CONFIG")
        .timeout(Duration::from_secs(180));
    let arguments: Vec<_> = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let result = command.output();
    let output = match result {
        Ok(output) => output,
        Err(error) => {
            write_json(
                path.with_extension("command.json"),
                &json!({"arguments": arguments, "exit": null, "execution_error": error.to_string(), "execution_mode": "sequential"}),
            )?;
            return Err(error).context(format!(
                "child execution failed; inspect {}",
                path.display()
            ));
        }
    };
    private_file(path.with_extension("stdout"), &output.stdout)?;
    private_file(path.with_extension("stderr"), &output.stderr)?;
    write_json(
        path.with_extension("command.json"),
        &json!({"arguments": arguments, "exit": output.status.code(), "signal": output.status.signal(), "time_limit_seconds": 180, "execution_mode": "sequential"}),
    )?;
    for token in tokens {
        ensure!(
            !String::from_utf8_lossy(&output.stdout).contains(token)
                && !String::from_utf8_lossy(&output.stderr).contains(token),
            "credential appears in child output; inspect {}",
            path.display()
        );
    }
    if let Some(expected) = expected {
        ensure!(
            output.status.code() == Some(expected),
            "{label} exited {:?}, expected {expected}; inspect {}",
            output.status.code(),
            path.display()
        );
    }
    Ok(output)
}

pub fn execute_git(
    directory: &Path,
    tokens: &[String],
    label: &str,
    mut command: std::process::Command,
    time_limit: Duration,
) -> Result<std::process::Output> {
    let path = evidence_path(directory, label)?;
    let arguments: Vec<_> = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let stdout = path.with_extension("stdout");
    let stderr = path.with_extension("stderr");
    private_file(&stdout, [])?;
    private_file(&stderr, [])?;
    command
        .current_dir(root())
        .stdout(fs::OpenOptions::new().write(true).open(&stdout)?)
        .stderr(fs::OpenOptions::new().write(true).open(&stderr)?);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let waited = runtime.block_on(async {
        let mut command = CommandWrap::from(tokio::process::Command::from(command));
        command.wrap(KillOnDrop).wrap(ProcessGroup::leader());
        let mut child = command.spawn()?;
        let pid = child.id().context("Git process id")?;
        let outcome = tokio::time::timeout(time_limit, child.wait()).await;
        if let Ok(Ok(status)) = &outcome {
            return Ok((*status, false, pid));
        }
        child.start_kill()?;
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .context("Git process group did not exit after termination")??;
        match outcome {
            Ok(result) => result
                .map(|_| (status, false, pid))
                .map_err(anyhow::Error::from),
            Err(_) => Ok((status, true, pid)),
        }
    });
    runtime.shutdown_timeout(Duration::from_secs(10));
    let mut record = json!({"arguments": arguments, "time_limit_seconds": time_limit.as_secs_f64(), "execution_mode": "sequential"});
    match waited {
        Ok((status, timed_out, pid)) => {
            record["pid"] = json!(pid);
            record["exit"] = json!(status.code());
            record["signal"] = json!(status.signal());
            record["timed_out"] = json!(timed_out);
            write_json(path.with_extension("command.json"), &record)?;
            let output = std::process::Output {
                status,
                stdout: fs::read(stdout)?,
                stderr: fs::read(stderr)?,
            };
            for body in [&output.stdout, &output.stderr] {
                let body = String::from_utf8_lossy(body);
                ensure!(
                    !tokens.iter().any(|token| body.contains(token)),
                    "credential appears in Git output; inspect {}",
                    path.display()
                );
            }
            ensure!(
                !timed_out && status.success(),
                "Git {label} failed or timed out; inspect {}",
                path.display()
            );
            Ok(output)
        }
        Err(error) => {
            record["execution_error"] = json!(error.to_string());
            write_json(path.with_extension("command.json"), &record)?;
            Err(error).context(format!("Git execution failed; inspect {}", path.display()))
        }
    }
}

fn evidence_path(directory: &Path, label: &str) -> Result<std::path::PathBuf> {
    let evidence = directory.join("evidence/e2e");
    std::fs::create_dir_all(&evidence)?;
    let mut sequence = 0_u64;
    for entry in std::fs::read_dir(&evidence)? {
        let name = entry?.file_name();
        if let Some((prefix, _)) = name.to_string_lossy().split_once('-')
            && let Ok(previous) = prefix.parse::<u64>()
        {
            sequence = sequence.max(previous);
        }
    }
    let sequence = sequence
        .checked_add(1)
        .context("process evidence sequence exhausted")?;
    Ok(evidence.join(format!("{sequence:03}-{label}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::read_json;
    use std::{io, os::unix::fs::PermissionsExt, time::Instant};

    fn directory() -> Result<tempfile::TempDir> {
        let local = root().join("tests/cli/.local");
        fs::create_dir_all(&local)?;
        Ok(tempfile::Builder::new()
            .prefix("process-proof-")
            .tempdir_in(local)?)
    }

    #[test]
    fn repeated_command_retains_earlier_evidence() -> Result<()> {
        let directory = directory()?;
        let binary = option_env!("CARGO_BIN_EXE_rkc")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| root().join("target/debug/rkc"));
        let mut command = Command::new(&binary);
        command.arg("--version");
        let first = execute(
            directory.path(),
            &[],
            "agent-command",
            &mut command,
            Some(0),
        )?;
        let first_path = fs::read_dir(directory.path().join("evidence/e2e"))?
            .collect::<io::Result<Vec<_>>>()?
            .into_iter()
            .find(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "stdout")
            })
            .context("first process output")?
            .path();
        let mut command = Command::new(binary);
        command.arg("--help");
        let second = execute(
            directory.path(),
            &[],
            "agent-command",
            &mut command,
            Some(0),
        )?;
        assert_ne!(first.stdout, second.stdout);
        assert_eq!(fs::read(&first_path)?, first.stdout);
        let captures = fs::read_dir(directory.path().join("evidence/e2e"))?
            .collect::<io::Result<Vec<_>>>()?
            .into_iter()
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "stdout")
            })
            .map(|entry| fs::read(entry.path()))
            .collect::<io::Result<Vec<_>>>()?;
        assert_eq!(captures.len(), 2);
        assert!(captures.contains(&first.stdout) && captures.contains(&second.stdout));
        Ok(())
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn git_timeout_terminates_credential_helper_descendants() -> Result<()> {
        let directory = directory()?;
        let askpass = directory.path().join("askpass.sh");
        private_file(
            &askpass,
            "#!/bin/sh\n/bin/sh -c 'printf \"%s\\n\" \"$$\" > \"$RKC_GIT_HELPER_PID\"; sleep 60' &\nwait\n",
        )?;
        fs::set_permissions(&askpass, fs::Permissions::from_mode(0o700))?;
        let input = directory.path().join("credential-input");
        private_file(&input, "protocol=http\nhost=example.test\n\n")?;
        let pid_file = directory.path().join("helper.pid");
        let mut command = std::process::Command::new("git");
        command
            .args(["-c", "credential.helper=", "credential", "fill"])
            .stdin(fs::File::open(input)?)
            .env("GIT_ASKPASS", askpass)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("RKC_GIT_HELPER_PID", &pid_file);
        let started = Instant::now();
        let result = execute_git(
            directory.path(),
            &[],
            "credential-timeout",
            command,
            Duration::from_secs(2),
        );
        assert!(result.is_err());
        let helper = fs::read_to_string(pid_file)?.trim().parse::<u32>()?;
        let records = fs::read_dir(directory.path().join("evidence/e2e"))?
            .collect::<io::Result<Vec<_>>>()?
            .into_iter()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".command.json")
            })
            .map(|entry| read_json(entry.path()))
            .collect::<Result<Vec<_>>>()?;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["timed_out"], true);
        let mut helper_running = true;
        let deadline = Instant::now() + Duration::from_secs(1);
        while helper_running && Instant::now() < deadline {
            helper_running = match fs::read_to_string(format!("/proc/{helper}/stat")) {
                Ok(state) => !state
                    .rsplit_once(") ")
                    .context("helper process state")?
                    .1
                    .starts_with('Z'),
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(error) => return Err(error.into()),
            };
            if helper_running {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if helper_running {
            let group = format!("-{}", records[0]["pid"]);
            Command::new("/bin/kill")
                .args(["-KILL", "--", &group])
                .timeout(Duration::from_secs(2))
                .output()?;
        }
        assert!(
            !helper_running,
            "Git credential helper survived the timeout"
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "Git timeout waited for inherited output handles"
        );
        Ok(())
    }
}
