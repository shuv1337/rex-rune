//! Local SSH agent preference for `ssh://` hosts.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};

/// Which SSH agent `ssh` uses for `ssh://` hosts unless `~/.ssh/config` sets `IdentityAgent`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SshAgent {
    /// Inherit the process's `SSH_AUTH_SOCK` untouched.
    #[default]
    System,
    /// Pass this socket path (`/` or `~/`) as `SSH_AUTH_SOCK`.
    Socket(String),
}

impl SshAgent {
    pub fn is_system(&self) -> bool {
        matches!(self, Self::System)
    }

    pub fn socket(&self) -> Option<&str> {
        match self {
            Self::System => None,
            Self::Socket(path) => Some(path.as_str()),
        }
    }

    /// Resolve a configured socket to an absolute path under `home`, or `None` for
    /// `System` (or an invalid/unexpandable path from a hand-edited file).
    pub fn resolve(&self, home: &Path) -> Option<PathBuf> {
        match self {
            Self::System => None,
            Self::Socket(path) => expand_socket(path, home),
        }
    }

    /// Resolve against the current user's home directory.
    pub fn resolve_default(&self) -> Option<PathBuf> {
        self.resolve(&crate::platform::dirs::home())
    }
}

impl Serialize for SshAgent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::System => serializer.serialize_none(),
            Self::Socket(path) => serializer.serialize_str(path),
        }
    }
}

impl<'de> Deserialize<'de> for SshAgent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Option::<String>::deserialize(deserializer)?;
        Ok(match value {
            Some(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    Self::System
                } else {
                    Self::Socket(trimmed.to_owned())
                }
            }
            None => Self::System,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SshConfig {
    #[serde(skip_serializing_if = "SshAgent::is_system")]
    pub agent: SshAgent,
}

impl SshConfig {
    pub fn is_default(&self) -> bool {
        self.agent.is_system()
    }
}

/// Validate a custom SSH agent socket path: must start with `/` or `~/`, contain no control
/// characters or `..` segments, and not end with `/`. The socket does not need to exist on
/// disk yet (on-demand or hardware-key agents may create it later).
pub fn parse_socket(input: &str) -> Result<String, &'static str> {
    let path = input.trim();
    if path.is_empty() {
        return Err("Enter an absolute socket path (/…) or one under your home folder (~/…).");
    }
    if path.chars().any(char::is_control) {
        return Err("Socket path cannot contain control characters.");
    }
    if !path.starts_with('/') && !path.starts_with("~/") {
        return Err("Use an absolute path (/…) or one under your home folder (~/…).");
    }
    if path.ends_with('/') {
        return Err("Enter the socket path, not a folder ending in /.");
    }
    let body = path.strip_prefix("~/").unwrap_or(path);
    if body.split('/').any(|segment| segment == "..") {
        return Err("Socket path cannot contain .. segments.");
    }
    Ok(path.to_owned())
}

fn is_posix_absolute(path: &Path) -> bool {
    path.is_absolute() || path.to_str().is_some_and(|s| s.starts_with('/'))
}

/// Expand a validated socket path (`/` or `~/`) against `home`.
pub fn expand_socket(socket: &str, home: &Path) -> Option<PathBuf> {
    let path = parse_socket(socket).ok()?;
    match path.strip_prefix("~/") {
        Some(rest) => {
            if home.as_os_str().is_empty() || !is_posix_absolute(home) {
                return None;
            }
            Some(home.join(rest))
        }
        None => Some(PathBuf::from(path)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn default_ssh_agent_is_system_and_omitted_from_json() {
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.ssh.agent, SshAgent::System);
        let value = serde_json::to_value(&old).unwrap();
        assert!(
            value.get("ssh").is_none(),
            "default ssh config must be omitted from workspace.json"
        );

        let empty_obj: Settings = serde_json::from_str(r#"{"ssh":{}}"#).unwrap();
        assert_eq!(empty_obj.ssh.agent, SshAgent::System);

        let null_agent: Settings = serde_json::from_str(r#"{"ssh":{"agent":null}}"#).unwrap();
        assert_eq!(null_agent.ssh.agent, SshAgent::System);

        let blank_agent: Settings = serde_json::from_str(r#"{"ssh":{"agent":"  "}}"#).unwrap();
        assert_eq!(blank_agent.ssh.agent, SshAgent::System);
    }

    #[test]
    fn ssh_agent_socket_round_trips_under_ssh_agent_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        for socket in [
            "~/.1password/agent.sock",
            "/run/user/1000/ssh-agent.socket",
            "~/Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock",
        ] {
            let mut settings = Settings::default();
            settings.ssh.agent = SshAgent::Socket(socket.into());
            settings.save(&path).unwrap();
            let raw: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(raw["ssh"]["agent"], socket);
            let loaded = Settings::load(&path).unwrap();
            assert_eq!(loaded.ssh.agent, SshAgent::Socket(socket.into()));
        }

        let mut reset = Settings::default();
        reset.ssh.agent = SshAgent::System;
        reset.save(&path).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(raw.get("ssh").is_none());
    }

    #[test]
    fn parse_and_expand_socket_validate_paths() {
        for valid in [
            "/run/user/1000/ssh-agent.socket",
            "~/.1password/agent.sock",
            "  /opt/homebrew/var/run/yubikey-agent.sock  ",
            "~/Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock",
            "/tmp/has..dots.sock",
        ] {
            assert_eq!(parse_socket(valid).unwrap(), valid.trim(), "{valid:?}");
        }

        for invalid in [
            "",
            "   ",
            "agent.sock",
            "relative/agent.sock",
            "~other/agent.sock",
            "~",
            "~/",
            "/",
            "/tmp/dir/",
            "~/.1password/",
            "/tmp/../etc/passwd",
            "~/../other/agent.sock",
            "/tmp/..",
            "/tmp/bad\nline.sock",
            "/tmp/bad\0null.sock",
        ] {
            assert!(parse_socket(invalid).is_err(), "should reject {invalid:?}");
            assert!(
                expand_socket(invalid, Path::new("/home/alice")).is_none(),
                "should not expand {invalid:?}"
            );
        }

        let home = Path::new("/home/alice");
        assert_eq!(
            expand_socket("~/.1password/agent.sock", home),
            Some(PathBuf::from("/home/alice/.1password/agent.sock"))
        );
        assert_eq!(
            expand_socket("/run/user/1000/gcr/ssh", home),
            Some(PathBuf::from("/run/user/1000/gcr/ssh"))
        );
        assert_eq!(
            expand_socket("~/.1password/agent.sock", Path::new("")),
            None
        );
        assert_eq!(
            expand_socket("~/.1password/agent.sock", Path::new("relative")),
            None
        );
    }
}
