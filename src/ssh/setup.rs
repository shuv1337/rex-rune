//! A one-shot, explicitly authorized native Rex SSH auto-start. Never used by attachment.
use super::{Target, last_line};
use anyhow::{Context, Result, ensure};
use std::{ffi::OsStr, path::PathBuf, process::Stdio, time::Duration};

/// Whether the one-shot command could already have started a server.
#[derive(Debug)]
pub struct Failure {
    pub message: String,
    pub may_have_started: bool,
}

impl Failure {
    fn after_start(error: impl std::fmt::Display) -> Self {
        Self {
            message: format!(
                "The personal server may have started, but setup did not confirm its status: {error}"
            ),
            may_have_started: true,
        }
    }
}

impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        Self {
            message: format!("{error:#}"),
            may_have_started: false,
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

/// How the reported server can be attached to without another setup attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attachment {
    Automatic,
    Review {
        reason: String,
        endpoint: Option<String>,
    },
}

fn attachment(target: &Target, status: &rexrs::api::server::StatusResponse) -> Attachment {
    let primary = status
        .listeners
        .iter()
        .find(|l| l.primary == Some(true))
        .or_else(|| status.listeners.first());
    let unix = primary
        .filter(|l| l.transport == "unix")
        .or_else(|| status.listeners.iter().find(|l| l.transport == "unix"));
    let path = unix
        .and_then(|l| {
            l.addresses
                .first()
                .map(String::as_str)
                .or_else(|| l.endpoint.strip_prefix("unix://"))
        })
        .map(|p| p.strip_prefix("unix://").unwrap_or(p));
    if let Some(path) = path {
        let explicit = Target {
            socket: Some(path.into()),
            ..target.clone()
        }
        .endpoint();
        // The ordinary lookup extracts a .sock path from the primary Unix endpoint.
        if primary.is_some_and(|l| l.transport == "unix")
            && path.ends_with(".sock")
            && !path.contains(char::is_whitespace)
        {
            return Attachment::Automatic;
        }
        let reason = "The server answered, but automatic SSH socket lookup cannot find this listener. Save its reported Unix socket endpoint to connect without starting it again.".into();
        if path.starts_with('/') && Target::parse(&explicit).is_ok() {
            return Attachment::Review {
                reason,
                endpoint: Some(explicit),
            };
        }
    }
    Attachment::Review {
        reason: "The server answered, but it reported no Unix socket this client can forward. Automatic SSH socket lookup needs a Unix listener, not a TCP endpoint. Edit the host to use a reachable network endpoint, or ask the server owner for a Unix socket; do not start it again.".into(),
        endpoint: None,
    }
}

/// The exact invocation the confirmation previews. No shell and no session creation.
pub fn start_args(target: &Target) -> Result<Vec<String>> {
    ensure!(
        target.socket.is_none(),
        "A custom socket must be started by its owner; edit the endpoint to use a personal server instead"
    );
    Ok([
        "--autostart=true",
        "--server",
        &target.endpoint(),
        "--timeout",
        "20s",
        "api",
        "call",
        "server.status",
        "{}",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect())
}

pub fn command_preview(target: &Target) -> Result<String> {
    Ok(format!(
        "rex {}",
        start_args(target)?
            .iter()
            .map(|arg| quote_arg(arg))
            .collect::<Vec<_>>()
            .join(" ")
    ))
}

fn quote_arg(arg: &str) -> String {
    if arg == "{}"
        || (!arg.is_empty()
            && arg
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_./:@%+=,-".contains(&c)))
    {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

fn local_rex() -> PathBuf {
    if std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("rex").is_file()))
    {
        return "rex".into();
    }
    std::env::var_os("XDG_BIN_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/bin")))
        .map(|dir| dir.join("rex"))
        .filter(|p| p.is_file())
        // On macOS the Rex app carries the CLI.
        .or_else(|| crate::platform::tools::bundled("rex"))
        .unwrap_or_else(|| "rex".into())
}

async fn output(program: &OsStr, args: &[String]) -> Result<String> {
    output_with_timeout(program, args, Duration::from_secs(45)).await
}

async fn output_with_timeout(
    program: &OsStr,
    args: &[String],
    timeout: Duration,
) -> Result<String> {
    let output = tokio::time::timeout(timeout, tokio::process::Command::new(program)
        .args(args).stdin(Stdio::null()).kill_on_drop(true).output()).await
        .context("Command timed out")?
        .with_context(|| format!("Couldn't run {}. Install Rex preview.14 or newer locally and on the SSH host, then try again", program.to_string_lossy()))?;
    ensure!(
        output.status.success(),
        "{}",
        last_line(&String::from_utf8_lossy(&output.stderr))
            .unwrap_or_else(|| format!("Command refused setup ({})", output.status))
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn require_api12(version: &str, location: &str) -> Result<()> {
    // Local Rex can pretty-print JSON; SSH login banners may precede a compact reply.
    let value: serde_json::Value = serde_json::from_str(version)
        .or_else(|_| {
            serde_json::from_str(
                version
                    .lines()
                    .rfind(|l| !l.trim().is_empty())
                    .unwrap_or(""),
            )
        })
        .with_context(|| {
            format!(
                "{location} Rex didn't report its version; preview.14 (API 12) or newer is required"
            )
        })?;
    ensure!(
        value["api_revision"].as_u64().is_some_and(|n| n >= 12),
        "{location} Rex is too old for this setup. Update to preview.14 (API 12) or newer; nothing was started"
    );
    Ok(())
}

/// Call only after the person confirms. Every invocation authorizes one attempt, never a
/// persistent auto-start setting. SSH config/aliases remain OpenSSH's responsibility.
pub async fn start_personal(endpoint: &str) -> Result<Attachment, Failure> {
    let target = Target::parse(endpoint)?.context("Setup requires an ssh:// host")?;
    start_with(&target, local_rex().as_os_str(), OsStr::new("ssh")).await
}

async fn start_with(target: &Target, rex: &OsStr, ssh: &OsStr) -> Result<Attachment, Failure> {
    let args = start_args(target)?;
    let local = output(rex, &["version".into(), "--json".into()]).await?;
    require_api12(&local, "Local")?;
    let mut remote = target.options();
    remote.extend([
        "--".into(),
        target.destination(),
        "rex version --json".into(),
    ]);
    let version = output(ssh, &remote).await.context("Couldn't check remote Rex. Install preview.14 or newer there, or check the SSH login; nothing was started")?;
    require_api12(&version, "Remote")?;
    start_after_preflight(target, rex, &args, Duration::from_secs(45)).await
}

async fn start_after_preflight(
    target: &Target,
    rex: &OsStr,
    args: &[String],
    timeout: Duration,
) -> Result<Attachment, Failure> {
    let reply = output_with_timeout(rex, args, timeout)
        .await
        .map_err(|error| Failure::after_start(format!("{error:#}")))?;
    let status: rexrs::api::server::StatusResponse =
        serde_json::from_str(&reply).map_err(Failure::after_start)?;
    Ok(attachment(target, &status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn old_missing_and_refusing_rex_never_start_or_retry() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("rex-setup-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let rex = root.join("rex");
        let ssh = root.join("ssh");
        let log = root.join("attempts");
        let script = |path: &std::path::Path, body: &str| {
            std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        };
        script(
            &rex,
            &format!(
                "if [ \"$1\" = version ]; then echo '{{\"api_revision\":12}}'; exit 0; fi\nprintf '%s\\n' \"$@\" >> '{}'; echo 'remote refused to start' >&2; exit 1",
                log.display()
            ),
        );
        let target = Target::parse("ssh://work-alias").unwrap().unwrap();
        for remote in [
            "echo '{\"api_revision\":11}'",
            "echo 'rex: command not found' >&2; exit 127",
            "echo 'SSH login refused' >&2; exit 255",
        ] {
            script(&ssh, remote);
            assert!(
                start_with(&target, rex.as_os_str(), ssh.as_os_str())
                    .await
                    .is_err()
            );
            assert!(!log.exists(), "preflight failed, no native auto-start");
        }
        script(&ssh, "echo '{\"api_revision\":12}'");
        let error = start_with(&target, rex.as_os_str(), ssh.as_os_str())
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("remote refused to start"));
        assert!(error.may_have_started);
        assert!(error.message.contains("may have started"));
        let attempts = std::fs::read_to_string(&log).unwrap();
        assert_eq!(attempts.matches("--autostart=true").count(), 1);
        assert!(attempts.contains("ssh://work-alias"));
        std::fs::remove_file(&log).unwrap();
        script(&rex, "echo '{\"api_revision\":11}'");
        assert!(
            start_with(&target, rex.as_os_str(), ssh.as_os_str())
                .await
                .unwrap_err()
                .to_string()
                .contains("Local Rex is too old")
        );
        assert!(
            start_with(&target, root.join("missing").as_os_str(), ssh.as_os_str())
                .await
                .is_err()
        );
        assert!(!log.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn every_post_preflight_failure_is_uncertain_including_timeouts() {
        let target = Target::parse("ssh://box").unwrap().unwrap();
        for command in [
            "echo invalid-json",
            "echo refusal >&2; exit 1",
            "exec sleep 60",
        ] {
            let args = ["-c".into(), command.into()];
            let error = start_after_preflight(
                &target,
                OsStr::new("/bin/sh"),
                &args,
                Duration::from_millis(100),
            )
            .await
            .unwrap_err();
            assert!(error.may_have_started, "{command}");
            assert!(error.message.contains("may have started"));
            if command.contains("sleep") {
                assert!(error.message.contains("timed out"));
            }
        }
        let error = start_after_preflight(
            &target,
            OsStr::new("/nonexistent/rex"),
            &[],
            Duration::from_millis(100),
        )
        .await
        .unwrap_err();
        assert!(error.may_have_started);
    }

    #[test]
    fn attachment_uses_reported_unix_addresses_and_handles_tcp_primaries() {
        use rexrs::api::server::{Listener, StatusResponse};
        let target = Target::parse("ssh://ops@box:2222").unwrap().unwrap();
        let unix = Listener {
            endpoint: "unix:///configured".into(),
            addresses: vec!["/actual/socket".into()],
            transport: "unix".into(),
            primary: Some(true),
            ..Default::default()
        };
        let mut status = StatusResponse {
            listeners: vec![unix],
            ..Default::default()
        };
        let Attachment::Review { endpoint, .. } = attachment(&target, &status) else {
            panic!("explicit path")
        };
        assert_eq!(
            endpoint.as_deref(),
            Some("ssh://ops@box:2222/actual/socket")
        );
        status.listeners[0].addresses = vec!["/actual/server.sock".into()];
        assert_eq!(attachment(&target, &status), Attachment::Automatic);
        status.listeners[0].primary = None;
        status.listeners.insert(
            0,
            Listener {
                endpoint: "127.0.0.1:7297".into(),
                transport: "tcp".into(),
                primary: Some(true),
                ..Default::default()
            },
        );
        let Attachment::Review { endpoint, .. } = attachment(&target, &status) else {
            panic!("TCP primary")
        };
        assert_eq!(
            endpoint.as_deref(),
            Some("ssh://ops@box:2222/actual/server.sock")
        );
        status.listeners.pop();
        assert!(matches!(
            attachment(&target, &status),
            Attachment::Review { endpoint: None, .. }
        ));
        status.listeners.clear();
        assert!(matches!(
            attachment(&target, &status),
            Attachment::Review { endpoint: None, .. }
        ));
    }

    #[test]
    fn command_preview_quotes_only_shell_sensitive_arguments() {
        let target = Target::parse("ssh://ops@box:2222").unwrap().unwrap();
        assert_eq!(
            command_preview(&target).unwrap(),
            "rex --autostart=true --server ssh://ops@box:2222 --timeout 20s api call server.status {}"
        );
        assert_eq!(quote_arg("ssh://me@[fd7a::1]"), "'ssh://me@[fd7a::1]'");
        assert_eq!(quote_arg("a'b"), "'a'\\''b'");
        assert_eq!(quote_arg("two words"), "'two words'");
        assert_eq!(quote_arg("ssh://{a,b}"), "'ssh://{a,b}'");
    }

    #[test]
    fn setup_keeps_alias_user_port_and_ipv6_and_rejects_custom_sockets() {
        for endpoint in [
            "ssh://work-alias",
            "ssh://ops@box:2222",
            "ssh://me@[fd7a::1]:22",
        ] {
            let target = Target::parse(endpoint).unwrap().unwrap();
            let args = start_args(&target).unwrap();
            assert_eq!(
                args,
                [
                    "--autostart=true",
                    "--server",
                    endpoint,
                    "--timeout",
                    "20s",
                    "api",
                    "call",
                    "server.status",
                    "{}"
                ]
            );
            assert!(command_preview(&target).unwrap().contains(endpoint));
        }
        assert!(start_args(&Target::parse("ssh://box/custom.sock").unwrap().unwrap()).is_err());
        for old in [r#"{"api_revision":11}"#, "{}", "old rex"] {
            assert!(require_api12(old, "Remote").is_err());
        }
        require_api12(r#"{"api_revision":12}"#, "Remote").unwrap();
        require_api12("Welcome to the host!\n{\"api_revision\":12}\n\n", "Remote").unwrap();
        require_api12("{\n  \"api_revision\":12\n}", "Local").unwrap();
    }
}
