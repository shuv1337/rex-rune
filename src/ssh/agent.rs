//! SSH agent discovery and safe RFC/OpenSSH wire probing.
//!
//! Sends only `SSH_AGENTC_REQUEST_IDENTITIES` (message type 11) over the local Unix socket
//! with a short timeout and reads only the 9-byte header (`length`, `SSH_AGENT_IDENTITIES_ANSWER`
//! type 12, `nkeys`). Key blobs, fingerprints, comments and private keys are never read,
//! stored or logged.
use crate::{
    platform::local::{self, HostOs, UnixSocketError},
    settings::ssh::{SshAgent, expand_socket, parse_socket},
};
use std::{
    collections::HashMap,
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const SSH_AGENT_FAILURE: u8 = 5;
const SSH_AGENTC_REQUEST_IDENTITIES: [u8; 5] = [0, 0, 0, 1, 11];
const SSH_AGENT_IDENTITIES_ANSWER: u8 = 12;
/// Reject oversized replies from malformed or hostile peers.
const MAX_REPLY_BYTES: u32 = 4 * 1024 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_millis(350);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentHealth {
    /// System default when `SSH_AUTH_SOCK` is unset or empty.
    NotSet,
    /// Socket path does not exist on disk.
    NotFound,
    /// Path exists or was configured, but the agent did not answer cleanly.
    NotAnswering(String),
    /// Agent answered with this many public keys.
    Answering { keys: u32 },
}

impl AgentHealth {
    pub fn is_answering(&self) -> bool {
        matches!(self, Self::Answering { .. })
    }

    pub fn has_keys(&self) -> bool {
        matches!(self, Self::Answering { keys } if *keys > 0)
    }
}

/// A known SSH agent socket candidate for an OS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub name: &'static str,
    /// Portable representation saved to `workspace.json` (`~/…` when under home, else absolute).
    pub saved: String,
    /// Concrete filesystem path to check and probe.
    pub path: PathBuf,
}

/// A known agent found on disk, with its live probe result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectedAgent {
    pub name: &'static str,
    pub saved: String,
    pub path: PathBuf,
    pub health: AgentHealth,
}

/// One complete discovery pass for Settings → Remote Hosts → SSH.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// Health of the process's inherited `SSH_AUTH_SOCK`.
    pub system_health: AgentHealth,
    /// Known agent name if `SSH_AUTH_SOCK` matches a known candidate path.
    pub system_name: Option<&'static str>,
    /// Health of the currently selected `SshAgent` setting.
    pub selected_health: AgentHealth,
    /// Known agent name if the selected socket matches a known candidate path.
    pub selected_name: Option<&'static str>,
    /// Detected known agents whose socket currently exists on disk.
    pub detected: Vec<DetectedAgent>,
}

fn is_posix_absolute(path: &Path) -> bool {
    path.is_absolute() || path.to_str().is_some_and(|s| s.starts_with('/'))
}

/// Pure table of known SSH agent socket locations for `os`.
/// Never reads real environment variables or touches the filesystem.
pub fn candidates_for(
    os: HostOs,
    home: Option<&Path>,
    runtime_dir: Option<&Path>,
    gnupg_home: Option<&Path>,
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let home = home.filter(|p| !p.as_os_str().is_empty() && is_posix_absolute(p));
    let runtime_dir = runtime_dir.filter(|p| !p.as_os_str().is_empty() && is_posix_absolute(p));
    let gnupg_home = gnupg_home.filter(|p| !p.as_os_str().is_empty() && is_posix_absolute(p));

    let push = |out: &mut Vec<Candidate>, name: &'static str, saved: String, path: PathBuf| {
        if !out.iter().any(|existing| existing.path == path) {
            out.push(Candidate { name, saved, path });
        }
    };
    let push_home = |out: &mut Vec<Candidate>, name: &'static str, rel: &str| {
        if let Some(home) = home {
            push(out, name, format!("~/{rel}"), home.join(rel));
        }
    };
    let push_abs = |out: &mut Vec<Candidate>, name: &'static str, path: PathBuf| {
        let saved = match home.and_then(|h| path.strip_prefix(h).ok()) {
            Some(rest) if !rest.as_os_str().is_empty() => {
                format!("~/{}", rest.to_string_lossy().replace('\\', "/"))
            }
            _ => path.to_string_lossy().replace('\\', "/"),
        };
        push(out, name, saved, path);
    };

    match os {
        HostOs::Linux => {
            push_home(&mut out, "1Password", ".1password/agent.sock");
            push_home(&mut out, "Bitwarden", ".bitwarden-ssh-agent.sock");
            push_home(
                &mut out,
                "Bitwarden",
                ".var/app/com.bitwarden.desktop/data/.bitwarden-ssh-agent.sock",
            );
            push_home(
                &mut out,
                "Bitwarden",
                "snap/bitwarden/current/.bitwarden-ssh-agent.sock",
            );
            if let Some(runtime) = runtime_dir {
                push_abs(&mut out, "gpg-agent", runtime.join("gnupg/S.gpg-agent.ssh"));
            }
            if let Some(gnupg) = gnupg_home {
                push_abs(&mut out, "gpg-agent", gnupg.join("S.gpg-agent.ssh"));
            }
            push_home(&mut out, "gpg-agent", ".gnupg/S.gpg-agent.ssh");
            if let Some(runtime) = runtime_dir {
                push_abs(&mut out, "gcr-ssh-agent", runtime.join("gcr/ssh"));
                push_abs(&mut out, "GNOME Keyring", runtime.join("keyring/ssh"));
                push_abs(&mut out, "OpenSSH agent", runtime.join("ssh-agent.socket"));
                push_abs(&mut out, "OpenSSH agent", runtime.join("openssh_agent"));
                push_abs(
                    &mut out,
                    "yubikey-agent",
                    runtime.join("yubikey-agent/yubikey-agent.sock"),
                );
            }
        }
        HostOs::MacOs => {
            push_home(
                &mut out,
                "Secretive",
                "Library/Containers/com.maxgoedjen.Secretive.SecretAgent/Data/socket.ssh",
            );
            push_home(
                &mut out,
                "1Password",
                "Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock",
            );
            push_home(&mut out, "1Password", ".1password/agent.sock");
            push_home(
                &mut out,
                "Bitwarden",
                "Library/Containers/com.bitwarden.desktop/Data/.bitwarden-ssh-agent.sock",
            );
            push_home(&mut out, "Bitwarden", ".bitwarden-ssh-agent.sock");
            push_abs(
                &mut out,
                "yubikey-agent",
                PathBuf::from("/opt/homebrew/var/run/yubikey-agent.sock"),
            );
            push_abs(
                &mut out,
                "yubikey-agent",
                PathBuf::from("/usr/local/var/run/yubikey-agent.sock"),
            );
            if let Some(gnupg) = gnupg_home {
                push_abs(&mut out, "gpg-agent", gnupg.join("S.gpg-agent.ssh"));
            }
            push_home(&mut out, "gpg-agent", ".gnupg/S.gpg-agent.ssh");
        }
        HostOs::Windows => {}
    }

    out
}

/// Probe a single Unix socket with a short timeout using RFC/OpenSSH agent framing.
pub fn probe_socket(path: &Path) -> AgentHealth {
    probe_socket_with_timeout(path, PROBE_TIMEOUT)
}

pub(crate) fn probe_socket_with_timeout(path: &Path, timeout: Duration) -> AgentHealth {
    match local::with_unix_socket(path, timeout, |stream| probe_io(stream)) {
        Ok(health) => health,
        Err(UnixSocketError::NotFound) => AgentHealth::NotFound,
        Err(UnixSocketError::NotSocket) => AgentHealth::NotAnswering("Not a Unix socket".into()),
        Err(UnixSocketError::WrongType) => {
            AgentHealth::NotAnswering("Could not establish a stream connection".into())
        }
        Err(UnixSocketError::Refused) => AgentHealth::NotAnswering("Agent is not listening".into()),
        Err(UnixSocketError::PermissionDenied) => {
            AgentHealth::NotAnswering("Permission denied".into())
        }
        Err(UnixSocketError::TimedOut) => AgentHealth::NotAnswering("Timed out".into()),
        Err(UnixSocketError::Unavailable(reason)) => AgentHealth::NotAnswering(reason.into()),
        Err(UnixSocketError::Other(reason)) => AgentHealth::NotAnswering(reason),
    }
}

/// Send `SSH_AGENTC_REQUEST_IDENTITIES` and read only the 9-byte answer header (`len`, `type`,
/// `nkeys`). Never reads or stores any key blob or comment bytes.
pub(crate) fn probe_io<S: Read + Write + ?Sized>(stream: &mut S) -> AgentHealth {
    if let Err(error) = stream
        .write_all(&SSH_AGENTC_REQUEST_IDENTITIES)
        .and_then(|()| stream.flush())
    {
        return io_health(error);
    }

    let mut len_buf = [0u8; 4];
    if let Err(error) = stream.read_exact(&mut len_buf) {
        return io_health(error);
    }
    let len = u32::from_be_bytes(len_buf);
    if len == 0 {
        return AgentHealth::NotAnswering("Invalid agent reply".into());
    }
    if len > MAX_REPLY_BYTES {
        return AgentHealth::NotAnswering("Agent reply too large".into());
    }

    let mut kind = [0u8; 1];
    if let Err(error) = stream.read_exact(&mut kind) {
        return io_health(error);
    }
    if kind[0] == SSH_AGENT_FAILURE {
        return AgentHealth::NotAnswering("Agent refused key list request".into());
    }
    if kind[0] != SSH_AGENT_IDENTITIES_ANSWER {
        return AgentHealth::NotAnswering("Unexpected agent reply".into());
    }
    if len < 5 {
        return AgentHealth::NotAnswering("Truncated agent reply".into());
    }

    let mut count_buf = [0u8; 4];
    if let Err(error) = stream.read_exact(&mut count_buf) {
        return io_health(error);
    }
    let keys = u32::from_be_bytes(count_buf);
    // Each identity entry in RFC SSH_AGENT_IDENTITIES_ANSWER has at least two 4-byte lengths
    // (key blob length + comment length). A zero-key reply has exact body length 5 (type + nkeys).
    // Reject impossible headers without reading any blobs.
    if (keys == 0 && len != 5) || (keys > 0 && u64::from(len) < 5 + u64::from(keys) * 8) {
        return AgentHealth::NotAnswering("Malformed agent reply".into());
    }
    AgentHealth::Answering { keys }
}

fn io_health(error: std::io::Error) -> AgentHealth {
    // macOS/BSD XNU can return EPROTOTYPE when writing to a Unix socket whose peer just closed.
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::EPROTOTYPE) {
        return AgentHealth::NotAnswering(
            "Agent connection closed before the reply was complete".into(),
        );
    }
    match error.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => {
            AgentHealth::NotAnswering("Timed out".into())
        }
        ErrorKind::UnexpectedEof => {
            AgentHealth::NotAnswering("Agent closed connection without replying".into())
        }
        ErrorKind::PermissionDenied => AgentHealth::NotAnswering("Permission denied".into()),
        ErrorKind::ConnectionRefused => AgentHealth::NotAnswering("Agent is not listening".into()),
        ErrorKind::BrokenPipe
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::NotConnected => AgentHealth::NotAnswering(
            "Agent connection closed before the reply was complete".into(),
        ),
        _ => AgentHealth::NotAnswering(error.to_string()),
    }
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

fn match_known_name(candidates: &[Candidate], path: &Path) -> Option<&'static str> {
    candidates
        .iter()
        .find(|candidate| same_path(&candidate.path, path))
        .map(|candidate| candidate.name)
}

/// Discover and probe known agents and the configured choice against explicit paths/env.
pub fn discover_in(
    os: HostOs,
    home: Option<&Path>,
    runtime_dir: Option<&Path>,
    gnupg_home: Option<&Path>,
    env_sock: Option<&Path>,
    configured: &SshAgent,
) -> Report {
    let env_sock = env_sock.filter(|p| !p.as_os_str().is_empty());
    let candidates = candidates_for(os, home, runtime_dir, gnupg_home);

    let mut cache: HashMap<PathBuf, AgentHealth> = HashMap::new();
    let mut probe_cached = |path: &Path| -> AgentHealth {
        let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(existing) = cache.get(&key) {
            return existing.clone();
        }
        let health = probe_socket(path);
        cache.insert(key, health.clone());
        health
    };

    let system_name = env_sock.and_then(|path| match_known_name(&candidates, path));
    let system_health = match env_sock {
        Some(path) => probe_cached(path),
        None => AgentHealth::NotSet,
    };

    let mut detected = Vec::new();
    let mut seen_canonical: Vec<PathBuf> = Vec::new();
    for candidate in &candidates {
        if std::fs::metadata(&candidate.path).is_err() {
            continue;
        }
        let canonical =
            std::fs::canonicalize(&candidate.path).unwrap_or_else(|_| candidate.path.clone());
        if seen_canonical.contains(&canonical) {
            continue;
        }
        seen_canonical.push(canonical);
        let health = probe_cached(&candidate.path);
        detected.push(DetectedAgent {
            name: candidate.name,
            saved: candidate.saved.clone(),
            path: candidate.path.clone(),
            health,
        });
    }

    let (selected_health, selected_name) = match configured {
        SshAgent::System => (system_health.clone(), system_name),
        SshAgent::Socket(raw) => match parse_socket(raw) {
            Err(reason) => (AgentHealth::NotAnswering(reason.into()), None),
            Ok(valid) => match expand_socket(&valid, home.unwrap_or_else(|| Path::new(""))) {
                Some(path) => {
                    let name = match_known_name(&candidates, &path);
                    let health = probe_cached(&path);
                    (health, name)
                }
                None => (
                    AgentHealth::NotAnswering("Home directory is unavailable".into()),
                    None,
                ),
            },
        },
    };

    Report {
        system_health,
        system_name,
        selected_health,
        selected_name,
        detected,
    }
}

/// Live discovery using the current machine's OS, home, runtime directory and `SSH_AUTH_SOCK`.
/// Call only from the background probe thread in Settings, never at startup or in unit tests.
pub fn discover(configured: &SshAgent) -> Report {
    let home = crate::platform::dirs::home();
    let runtime = crate::platform::dirs::xdg_runtime();
    let gnupg = std::env::var_os("GNUPGHOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let env_sock = std::env::var_os("SSH_AUTH_SOCK")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    discover_in(
        local::HOST_OS,
        (!home.as_os_str().is_empty()).then_some(home.as_path()),
        runtime.as_deref(),
        gnupg.as_deref(),
        env_sock.as_deref(),
        configured,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_table_covers_linux_and_macos_without_reading_real_env() {
        let home = Path::new("/fake/home/alice");
        let runtime = Path::new("/fake/run/1000");
        let gnupg = Path::new("/fake/gnupg");

        let linux = candidates_for(HostOs::Linux, Some(home), Some(runtime), Some(gnupg));
        assert!(linux.iter().all(|c| !c.saved.contains('\\')));
        let linux_paths: Vec<(&str, PathBuf)> =
            linux.iter().map(|c| (c.name, c.path.clone())).collect();
        for expected in [
            ("1Password", home.join(".1password/agent.sock")),
            ("Bitwarden", home.join(".bitwarden-ssh-agent.sock")),
            (
                "Bitwarden",
                home.join(".var/app/com.bitwarden.desktop/data/.bitwarden-ssh-agent.sock"),
            ),
            (
                "Bitwarden",
                home.join("snap/bitwarden/current/.bitwarden-ssh-agent.sock"),
            ),
            ("gpg-agent", runtime.join("gnupg/S.gpg-agent.ssh")),
            ("gpg-agent", gnupg.join("S.gpg-agent.ssh")),
            ("gpg-agent", home.join(".gnupg/S.gpg-agent.ssh")),
            ("gcr-ssh-agent", runtime.join("gcr/ssh")),
            ("GNOME Keyring", runtime.join("keyring/ssh")),
            ("OpenSSH agent", runtime.join("ssh-agent.socket")),
            ("OpenSSH agent", runtime.join("openssh_agent")),
            (
                "yubikey-agent",
                runtime.join("yubikey-agent/yubikey-agent.sock"),
            ),
        ] {
            assert!(
                linux_paths.contains(&expected),
                "missing Linux candidate {expected:?} in {linux_paths:?}"
            );
        }

        let mac = candidates_for(HostOs::MacOs, Some(home), None, Some(gnupg));
        assert!(mac.iter().all(|c| !c.saved.contains('\\')));
        let mac_paths: Vec<(&str, PathBuf)> =
            mac.iter().map(|c| (c.name, c.path.clone())).collect();
        for expected in [
            (
                "Secretive",
                home.join(
                    "Library/Containers/com.maxgoedjen.Secretive.SecretAgent/Data/socket.ssh",
                ),
            ),
            (
                "1Password",
                home.join("Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock"),
            ),
            ("1Password", home.join(".1password/agent.sock")),
            (
                "Bitwarden",
                home.join(
                    "Library/Containers/com.bitwarden.desktop/Data/.bitwarden-ssh-agent.sock",
                ),
            ),
            ("Bitwarden", home.join(".bitwarden-ssh-agent.sock")),
            (
                "yubikey-agent",
                PathBuf::from("/opt/homebrew/var/run/yubikey-agent.sock"),
            ),
            (
                "yubikey-agent",
                PathBuf::from("/usr/local/var/run/yubikey-agent.sock"),
            ),
            ("gpg-agent", gnupg.join("S.gpg-agent.ssh")),
            ("gpg-agent", home.join(".gnupg/S.gpg-agent.ssh")),
        ] {
            assert!(
                mac_paths.contains(&expected),
                "missing macOS candidate {expected:?} in {mac_paths:?}"
            );
        }

        // Duplicate GNUPGHOME == ~/.gnupg is deduplicated.
        let dedup = candidates_for(HostOs::Linux, Some(home), None, Some(&home.join(".gnupg")));
        assert_eq!(
            dedup
                .iter()
                .filter(|c| c.path == home.join(".gnupg/S.gpg-agent.ssh"))
                .count(),
            1
        );

        assert!(candidates_for(HostOs::Windows, Some(home), None, None).is_empty());
    }

    #[test]
    fn wire_parser_handles_valid_and_hostile_frames() {
        struct MockStream {
            written: Vec<u8>,
            reply: std::io::Cursor<Vec<u8>>,
        }
        impl Read for MockStream {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                self.reply.read(buf)
            }
        }
        impl Write for MockStream {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.written.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let run = |reply: Vec<u8>| {
            let mut stream = MockStream {
                written: Vec::new(),
                reply: std::io::Cursor::new(reply),
            };
            let health = probe_io(&mut stream);
            assert_eq!(stream.written, SSH_AGENTC_REQUEST_IDENTITIES);
            health
        };

        // 0 keys: len=5, type=12, nkeys=0
        assert_eq!(
            run(vec![0, 0, 0, 5, 12, 0, 0, 0, 0]),
            AgentHealth::Answering { keys: 0 }
        );
        // Oversized length (> 4 MiB) rejected before reading payload
        assert!(matches!(
            run(vec![0x01, 0x00, 0x00, 0x00, 12, 0, 0, 0, 1]),
            AgentHealth::NotAnswering(msg) if msg.contains("too large")
        ));
        // Zero length rejected
        assert!(matches!(
            run(vec![0, 0, 0, 0]),
            AgentHealth::NotAnswering(_)
        ));
        // Wrong type (SSH_AGENT_FAILURE = 5)
        assert!(matches!(
            run(vec![0, 0, 0, 1, 5]),
            AgentHealth::NotAnswering(msg) if msg.contains("refused")
        ));
        // Unknown type
        assert!(matches!(
            run(vec![0, 0, 0, 5, 99, 0, 0, 0, 0]),
            AgentHealth::NotAnswering(msg) if msg.contains("Unexpected")
        ));
        // Truncated (len=3 for type 12)
        assert!(matches!(
            run(vec![0, 0, 0, 3, 12, 0, 0]),
            AgentHealth::NotAnswering(msg) if msg.contains("Truncated")
        ));
        // Impossible key count for stated length
        assert!(matches!(
            run(vec![0, 0, 0, 5, 12, 0, 0, 0, 3]),
            AgentHealth::NotAnswering(msg) if msg.contains("Malformed")
        ));
        // Zero keys with trailing garbage (len != 5) rejected
        assert!(matches!(
            run(vec![0, 0, 0, 6, 12, 0, 0, 0, 0, 0xff]),
            AgentHealth::NotAnswering(msg) if msg.contains("Malformed")
        ));
    }

    #[test]
    fn io_health_maps_closed_and_stale_socket_errors() {
        use std::io::Error;

        let closed_mid_reply = AgentHealth::NotAnswering(
            "Agent connection closed before the reply was complete".into(),
        );
        for kind in [
            ErrorKind::BrokenPipe,
            ErrorKind::ConnectionReset,
            ErrorKind::ConnectionAborted,
            ErrorKind::NotConnected,
        ] {
            assert_eq!(
                io_health(Error::from(kind)),
                closed_mid_reply,
                "expected {kind:?} to report closed connection before complete reply"
            );
        }
        #[cfg(unix)]
        assert_eq!(
            io_health(Error::from_raw_os_error(libc::EPROTOTYPE)),
            closed_mid_reply
        );
        assert_eq!(
            io_health(Error::from(ErrorKind::ConnectionRefused)),
            AgentHealth::NotAnswering("Agent is not listening".into())
        );
        assert_eq!(
            io_health(Error::from(ErrorKind::TimedOut)),
            AgentHealth::NotAnswering("Timed out".into())
        );
        assert_eq!(
            io_health(Error::from(ErrorKind::WouldBlock)),
            AgentHealth::NotAnswering("Timed out".into())
        );
        assert_eq!(
            io_health(Error::from(ErrorKind::UnexpectedEof)),
            AgentHealth::NotAnswering("Agent closed connection without replying".into())
        );
        assert_eq!(
            io_health(Error::from(ErrorKind::PermissionDenied)),
            AgentHealth::NotAnswering("Permission denied".into())
        );
    }

    #[test]
    fn probe_io_reports_post_connect_closure_on_partial_reply() {
        struct PartialFailStream {
            reply: std::io::Cursor<Vec<u8>>,
            fail_kind: ErrorKind,
        }
        impl Read for PartialFailStream {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.reply.read(buf)?;
                if n == 0 {
                    return Err(std::io::Error::from(self.fail_kind));
                }
                Ok(n)
            }
        }
        impl Write for PartialFailStream {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        for fail_kind in [ErrorKind::ConnectionReset, ErrorKind::ConnectionAborted] {
            // Valid length (5) + SSH_AGENT_IDENTITIES_ANSWER (12), then connection drops
            // before the 4-byte key count arrives.
            let mut stream = PartialFailStream {
                reply: std::io::Cursor::new(vec![0, 0, 0, 5, SSH_AGENT_IDENTITIES_ANSWER]),
                fail_kind,
            };
            assert_eq!(
                probe_io(&mut stream),
                AgentHealth::NotAnswering(
                    "Agent connection closed before the reply was complete".into()
                ),
                "expected partial reply followed by {fail_kind:?} to report closed connection"
            );
        }
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::{os::unix::net::UnixListener, thread};

        fn identities_frame(nkeys: u32) -> Vec<u8> {
            let mut body = vec![SSH_AGENT_IDENTITIES_ANSWER];
            body.extend_from_slice(&nkeys.to_be_bytes());
            for i in 0..nkeys {
                let blob = format!("dummy-key-blob-{i}");
                let comment = format!("comment-{i}");
                body.extend_from_slice(&(blob.len() as u32).to_be_bytes());
                body.extend_from_slice(blob.as_bytes());
                body.extend_from_slice(&(comment.len() as u32).to_be_bytes());
                body.extend_from_slice(comment.as_bytes());
            }
            let mut frame = Vec::with_capacity(4 + body.len());
            frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
            frame.extend_from_slice(&body);
            frame
        }

        fn spawn_fake_agent(
            path: &Path,
            reply: Option<Vec<u8>>,
            delay: Duration,
        ) -> thread::JoinHandle<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            let listener = UnixListener::bind(path).unwrap();
            thread::spawn(move || {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut req = [0u8; 5];
                let _ = stream.read_exact(&mut req);
                assert_eq!(req, SSH_AGENTC_REQUEST_IDENTITIES);
                if !delay.is_zero() {
                    thread::sleep(delay);
                }
                if let Some(bytes) = reply {
                    let _ = stream.write_all(&bytes);
                    let _ = stream.flush();
                }
            })
        }

        #[test]
        fn probes_fake_unix_agents_for_zero_keys_three_keys_errors_and_timeouts() {
            let dir = tempfile::tempdir().unwrap();

            // 0 keys
            let zero_sock = dir.path().join("zero.sock");
            let t0 = spawn_fake_agent(&zero_sock, Some(identities_frame(0)), Duration::ZERO);
            assert_eq!(probe_socket(&zero_sock), AgentHealth::Answering { keys: 0 });
            t0.join().unwrap();

            // 3 keys
            let three_sock = dir.path().join("three.sock");
            let t3 = spawn_fake_agent(&three_sock, Some(identities_frame(3)), Duration::ZERO);
            assert_eq!(
                probe_socket(&three_sock),
                AgentHealth::Answering { keys: 3 }
            );
            t3.join().unwrap();

            // Malformed oversized reply
            let bad_len = dir.path().join("bad-len.sock");
            let tb = spawn_fake_agent(
                &bad_len,
                Some(vec![0x02, 0x00, 0x00, 0x00, 12]),
                Duration::ZERO,
            );
            assert!(matches!(
                probe_socket(&bad_len),
                AgentHealth::NotAnswering(msg) if msg.contains("too large")
            ));
            tb.join().unwrap();

            // Wrong reply type
            let bad_type = dir.path().join("bad-type.sock");
            let tt = spawn_fake_agent(&bad_type, Some(vec![0, 0, 0, 1, 77]), Duration::ZERO);
            assert!(matches!(
                probe_socket(&bad_type),
                AgentHealth::NotAnswering(msg) if msg.contains("Unexpected")
            ));
            tt.join().unwrap();

            // Unresponsive peer hits read timeout cleanly
            let hang_sock = dir.path().join("hang.sock");
            let th = spawn_fake_agent(&hang_sock, None, Duration::from_millis(250));
            assert!(matches!(
                probe_socket_with_timeout(&hang_sock, Duration::from_millis(60)),
                AgentHealth::NotAnswering(msg) if msg.contains("Timed out")
            ));
            th.join().unwrap();

            // Stale / closed socket file -> connection refused
            let closed_sock = dir.path().join("closed.sock");
            {
                let _listener = UnixListener::bind(&closed_sock).unwrap();
            }
            let closed = probe_socket(&closed_sock);
            assert!(
                matches!(closed, AgentHealth::NotAnswering(ref msg) if msg.contains("not listening")),
                "expected stale socket to report not listening, got {closed:?}"
            );

            // Non-existent path -> NotFound
            assert_eq!(
                probe_socket(&dir.path().join("missing.sock")),
                AgentHealth::NotFound
            );

            // Regular file -> Not a Unix socket
            let regular = dir.path().join("plain.txt");
            std::fs::write(&regular, b"hello").unwrap();
            assert!(matches!(
                probe_socket(&regular),
                AgentHealth::NotAnswering(msg) if msg.contains("Not a Unix socket")
            ));

            // Live datagram socket -> EPROTOTYPE must not claim the listener is absent
            let dgram_sock = dir.path().join("dgram.sock");
            let _dgram = std::os::unix::net::UnixDatagram::bind(&dgram_sock).unwrap();
            let dgram_health = probe_socket(&dgram_sock);
            assert_eq!(
                dgram_health,
                AgentHealth::NotAnswering("Could not establish a stream connection".into())
            );
        }

        #[test]
        fn discover_in_keeps_system_default_and_still_lists_matching_detected_agent() {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join("home");
            let runtime = dir.path().join("run");
            let onepw = home.join(".1password/agent.sock");
            let openssh = runtime.join("ssh-agent.socket");

            // 1Password is both SSH_AUTH_SOCK and a detected known agent; it should only be
            // probed once thanks to caching, and appear BOTH as system_name AND in detected!
            let t1 = spawn_fake_agent(&onepw, Some(identities_frame(2)), Duration::ZERO);
            let t2 = spawn_fake_agent(&openssh, Some(identities_frame(1)), Duration::ZERO);

            let report = discover_in(
                HostOs::Linux,
                Some(&home),
                Some(&runtime),
                None,
                Some(&onepw),
                &SshAgent::System,
            );
            t1.join().unwrap();
            t2.join().unwrap();

            assert_eq!(report.system_name, Some("1Password"));
            assert_eq!(report.system_health, AgentHealth::Answering { keys: 2 });
            assert_eq!(report.selected_name, Some("1Password"));
            assert_eq!(report.selected_health, AgentHealth::Answering { keys: 2 });
            assert_eq!(report.detected.len(), 2);
            assert_eq!(report.detected[0].name, "1Password");
            assert_eq!(report.detected[0].saved, "~/.1password/agent.sock");
            assert_eq!(
                report.detected[0].health,
                AgentHealth::Answering { keys: 2 }
            );
            assert_eq!(report.detected[1].name, "OpenSSH agent");
            assert_eq!(
                report.detected[1].health,
                AgentHealth::Answering { keys: 1 }
            );
        }

        #[test]
        fn discover_in_handles_unset_env_and_missing_custom_socket() {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join("home");
            std::fs::create_dir_all(&home).unwrap();
            let report = discover_in(
                HostOs::Linux,
                Some(&home),
                None,
                None,
                None,
                &SshAgent::Socket("~/custom/missing.sock".into()),
            );
            assert_eq!(report.system_health, AgentHealth::NotSet);
            assert_eq!(report.system_name, None);
            assert_eq!(report.selected_health, AgentHealth::NotFound);
            assert_eq!(report.selected_name, None);
            assert!(report.detected.is_empty());
        }
    }
}
