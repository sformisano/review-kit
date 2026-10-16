use std::{
    env,
    error::Error,
    fs,
    io::Write,
    net::TcpListener,
    num::NonZeroU16,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use reqwest::blocking::Client;
use testcontainers::{
    Container, GenericImage, ImageExt,
    bollard::models::PortBinding,
    core::{ExecCommand, IntoContainerPort, WaitFor},
    runners::SyncRunner,
};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

const IMAGE_TAG: &str =
    "19.3.3-ce.0@sha256:8c6ede6b1334738123feba0d248e81a14fc3db7065dc6e1f0a753c0d0332fee8";
const HTTP_PORT: u16 = 8929;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(900);

/// Own this service until every scenario in the suite has finished.
pub struct GitLabService {
    endpoint: String,
    admin_token: String,
    _container: Option<Container<GenericImage>>,
}

impl GitLabService {
    pub fn start() -> Result<Self> {
        match (
            env::var_os("REVIEW_KIT_GITLAB_URL"),
            env::var_os("REVIEW_KIT_GITLAB_ADMIN_TOKEN_FILE"),
        ) {
            (Some(endpoint), Some(token_file)) => {
                let endpoint = endpoint
                    .into_string()
                    .map_err(|_| "REVIEW_KIT_GITLAB_URL is not valid UTF-8")?;
                validate_endpoint(&endpoint)?;
                let endpoint = endpoint.trim_end_matches('/').to_owned();
                let admin_token = read_token(fs::read_to_string(token_file)?.as_bytes())?;
                wait_for_external_server(&endpoint, &admin_token)?;
                Ok(Self {
                    endpoint,
                    admin_token,
                    _container: None,
                })
            }
            (None, None) => Self::managed(),
            _ => Err(
                "set both REVIEW_KIT_GITLAB_URL and REVIEW_KIT_GITLAB_ADMIN_TOKEN_FILE, or neither"
                    .into(),
            ),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Only fixture setup uses this credential; reviewers receive separate tokens.
    pub fn admin_token(&self) -> &str {
        &self.admin_token
    }

    fn managed() -> Result<Self> {
        if let Some(host) = env::var_os("DOCKER_HOST")
            && !host.to_string_lossy().starts_with("unix://")
        {
            return Err("managed GitLab requires a local Unix Docker socket".into());
        }
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let host_port = listener.local_addr()?.port();
        let endpoint = format!("http://127.0.0.1:{host_port}");
        let config = format!(
            "external_url '{endpoint}'\n\
             nginx['listen_port'] = {HTTP_PORT}\n\
             puma['worker_processes'] = 2\n\
             gitlab_rails['smtp_enable'] = false\n\
             gitlab_rails['usage_ping_enabled'] = false\n\
             letsencrypt['enable'] = false"
        );
        drop(listener);
        println!(
            "Starting disposable GitLab CE; reconfiguration and readiness each allow 15 minutes."
        );
        let container = GenericImage::new("gitlab/gitlab-ce", IMAGE_TAG)
            .with_exposed_port(HTTP_PORT.tcp())
            .with_wait_for(WaitFor::message_on_stdout("gitlab Reconfigured!"))
            .with_hostname("review-kit-gitlab.local")
            .with_env_var("GITLAB_OMNIBUS_CONFIG", config)
            .with_mapped_port(host_port, HTTP_PORT.tcp())
            .with_shm_size(256 * 1024 * 1024)
            .with_startup_timeout(STARTUP_TIMEOUT)
            .with_host_config_modifier(move |host| {
                host.memory = Some(16 * 1024 * 1024 * 1024);
                host.nano_cpus = Some(8 * 1_000_000_000);
                host.publish_all_ports = Some(false);
                host.port_bindings = Some(std::collections::HashMap::from([(
                    format!("{HTTP_PORT}/tcp"),
                    Some(vec![PortBinding {
                        host_ip: Some("127.0.0.1".into()),
                        host_port: Some(host_port.to_string()),
                    }]),
                )]));
            })
            .start()?;
        println!("GitLab container {} at {endpoint}", container.id());
        let mut bootstrap_stderr = Vec::new();
        let admin_token = (|| -> Result<String> {
            wait_for_container_readiness(&container)?;
            let mut output = container.exec(ExecCommand::new([
                "timeout",
                "--signal=KILL",
                "180",
                "gitlab-rails",
                "runner",
                include_str!("../bootstrap.rb"),
            ]))?;
            let token = output.stdout_to_vec()?;
            bootstrap_stderr = output.stderr_to_vec()?;
            if output.exit_code()? != Some(0) {
                return Err(
                    "GitLab fixture credential bootstrap failed or exceeded 180 seconds".into(),
                );
            }
            read_token(&token)
        })()
        .inspect_err(|_| save_failure_logs(&container, &bootstrap_stderr))?;
        Ok(Self {
            endpoint,
            admin_token,
            _container: Some(container),
        })
    }
}

fn save_failure_logs(container: &Container<GenericImage>, bootstrap_stderr: &[u8]) {
    let result = (|| -> Result<_> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .find(|directory| directory.join("tests/gitlab").is_dir())
            .ok_or("cannot locate GitLab test directory")?;
        let directory = root.join("tests/gitlab/.local");
        fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{}.log", container.id()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        file.write_all(&container.stdout_to_vec()?)?;
        file.write_all(&container.stderr_to_vec()?)?;
        file.write_all(bootstrap_stderr)?;
        Ok(path)
    })();
    match result {
        Ok(path) => eprintln!("Private GitLab startup log: {}", path.display()),
        Err(error) => eprintln!("Could not save private GitLab startup logs: {error}"),
    }
}

fn validate_endpoint(endpoint: &str) -> Result<()> {
    endpoint
        .strip_prefix("http://127.0.0.1:")
        .and_then(|port| {
            port.strip_suffix('/')
                .unwrap_or(port)
                .parse::<NonZeroU16>()
                .ok()
        })
        .map(|_| ())
        .ok_or_else(|| {
            "GitLab fixture writes require http://127.0.0.1:<port> without credentials or a path"
                .into()
        })
}

fn read_token(bytes: &[u8]) -> Result<String> {
    let token = std::str::from_utf8(bytes)?.trim();
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        return Err("GitLab fixture credential is empty or contains whitespace".into());
    }
    Ok(token.into())
}

fn wait_for_container_readiness(container: &Container<GenericImage>) -> Result<()> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        let mut probe = container.exec(ExecCommand::new([
            "curl",
            "--fail",
            "--silent",
            "--max-time",
            "5",
            "--output",
            "/dev/null",
            "http://127.0.0.1:8929/-/readiness?all=1",
        ]))?;
        probe.stdout_to_vec()?;
        if probe.exit_code()? == Some(0) {
            return Ok(());
        }
        thread::sleep(Duration::from_secs(5));
    }
    Err("GitLab did not become ready within 15 minutes".into())
}

fn wait_for_external_server(endpoint: &str, token: &str) -> Result<()> {
    let mut credential = reqwest::header::HeaderValue::from_str(token)?;
    credential.set_sensitive(true);
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()?;
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(response) = client
            .get(format!("{endpoint}/api/v4/version"))
            .header("PRIVATE-TOKEN", &credential)
            .send()
        {
            let status = response.status();
            if status.is_success() {
                return Ok(());
            }
            if status.is_redirection()
                || (status.is_client_error() && status != reqwest::StatusCode::TOO_MANY_REQUESTS)
            {
                return Err(format!(
                    "external GitLab readiness API rejected the request: {status}"
                )
                .into());
            }
        }
        thread::sleep(Duration::from_secs(5));
    }
    Err("GitLab did not become ready within 15 minutes".into())
}

#[cfg(test)]
mod tests {
    use super::validate_endpoint;

    #[test]
    fn fixture_endpoint_accepts_loopback_ports() {
        assert!(validate_endpoint("http://127.0.0.1:8929").is_ok());
        assert!(validate_endpoint("http://127.0.0.1:80").is_ok());
        assert!(validate_endpoint("http://127.0.0.1:43827/").is_ok());
    }

    #[test]
    fn fixture_endpoint_rejects_remote_and_ambiguous_targets() {
        for endpoint in [
            "https://gitlab.com:443",
            "http://localhost:8929",
            "http://127.0.0.1",
            "http://127.0.0.1:0",
            "http://2130706433:8929",
            "http://user:secret@127.0.0.1:8929",
            "http://127.0.0.1:8929/path",
            "http://127.0.0.1:8929?target=remote",
            "http://127.0.0.1:8929#fragment",
        ] {
            assert!(validate_endpoint(endpoint).is_err());
        }
    }
}
