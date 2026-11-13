//! Where a Rex server on this machine listens.
use std::path::PathBuf;

/// The endpoint of this user's own server: `REX_SERVER` when set, else the socket the Rex
/// server makes by default on Unix. Windows keeps a valid but unsupported placeholder.
///
/// | Platform | Socket |
/// | --- | --- |
/// | Linux and other Unix | `$XDG_RUNTIME_DIR/rex/server.sock`, else `/run/user/<uid>/rex/server.sock` |
/// | macOS | `~/Library/Application Support/rex/server.sock` |
/// | Windows | `unix:///rex/server.sock` (unsupported placeholder) |
pub fn default_endpoint() -> String {
    std::env::var("REX_SERVER").unwrap_or_else(|_| format!("unix://{}", default_socket().display()))
}

/// The socket of this user's own server on Unix, whatever `REX_SERVER` says: the one a
/// probe that creates sessions must never be pointed at. A placeholder on Windows.
pub fn default_socket() -> PathBuf {
    #[cfg(windows)]
    {
        // A local Windows Rex server listens on a named pipe (W8). Keep a valid endpoint
        // for the default Local host until there is a pipe transport, and refuse at connect.
        // It is saved in workspace.json as it is: when the pipe transport arrives, a saved
        // Local host with exactly this endpoint is the one to point at the pipe.
        PathBuf::from("/rex/server.sock")
    }
    #[cfg(unix)]
    {
        let folder = if cfg!(target_os = "macos") {
            super::dirs::home().join("Library/Application Support")
        } else {
            super::dirs::xdg_runtime().unwrap_or_else(|| {
                PathBuf::from(format!("/run/user/{}", super::process::user_id()))
            })
        };
        folder.join("rex/server.sock")
    }
}

/// Open a local Unix socket, or explain why this platform cannot attach to it.
pub async fn connect(path: &std::path::Path) -> anyhow::Result<Box<dyn crate::transport::Io>> {
    #[cfg(unix)]
    {
        Ok(Box::new(tokio::net::UnixStream::connect(path).await?))
    }
    #[cfg(windows)]
    {
        let _ = path;
        anyhow::bail!("Unix sockets are not available on Windows yet; add an https:// host")
    }
}

/// SSH forwards to a local Unix socket. Refuse before discovering or starting anything.
pub fn require_ssh() -> anyhow::Result<()> {
    if cfg!(windows) {
        anyhow::bail!("ssh:// hosts are not available on Windows yet");
    }
    Ok(())
}

/// Keep the tunnel folder private and remove only dead processes' old socket folders.
pub async fn prepare_tunnel_directory(parent: &std::path::Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use anyhow::Context;
        // Every process's folder, then this one's: each ours alone, or no tunnel.
        for directory in [parent.parent().unwrap_or(parent), parent] {
            super::dirs::private(directory).with_context(|| {
                format!(
                    "{} is not a private folder for the tunnel",
                    directory.display()
                )
            })?;
        }
        if let Some(all) = parent.parent() {
            sweep_stale_tunnels(all);
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        let _ = parent;
        require_ssh()
    }
}

/// Remove the socket directories of processes that are gone, which a crash or a kill leaves
/// behind. Only `ssh-<pid>` directories whose process no longer exists are touched.
#[cfg(unix)]
fn sweep_stale_tunnels(directory: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("ssh-"))
            .and_then(|p| p.parse::<u32>().ok())
        else {
            continue;
        };
        if pid != std::process::id() && !crate::platform::process::exists(pid) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[tokio::test]
    async fn unsupported_transports_explain_how_to_connect() {
        let error =
            crate::transport::socket("unix:///rex/server.sock", "/api/control", "rex.json.v1")
                .await
                .err()
                .unwrap();
        assert_eq!(
            error.to_string(),
            "Unix sockets are not available on Windows yet; add an https:// host"
        );
        let error = crate::ssh::Tunnel::for_endpoint("ssh://example.com")
            .await
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "ssh:// hosts are not available on Windows yet"
        );
        assert!(
            crate::ssh::Tunnel::for_endpoint("https://example.com")
                .await
                .unwrap()
                .is_none()
        );
        let endpoint = default_endpoint();
        crate::settings::validate_host("Local", &endpoint).unwrap();
        assert_eq!(
            endpoint,
            std::env::var("REX_SERVER")
                .unwrap_or_else(|_| format!("unix://{}", default_socket().display()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn stale_tunnel_directories_of_gone_processes_are_swept() {
        let root = std::env::temp_dir().join(format!("rex-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // Linux pids never exceed 4194304, so this process is gone.
        let gone = root.join("ssh-4194305");
        let own = root.join(format!("ssh-{}", std::process::id()));
        let other = root.join("notes");
        for d in [&gone, &own, &other] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("x.sock"), b"").unwrap();
        }
        sweep_stale_tunnels(&root);
        assert!(!gone.exists(), "a gone process's directory is removed");
        assert!(own.exists(), "this process's directory stays");
        assert!(other.exists(), "anything else is left alone");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_default_socket_is_where_this_platforms_server_listens() {
        let socket = default_socket();
        assert!(socket.ends_with("rex/server.sock"));
        if cfg!(target_os = "macos") {
            assert_eq!(
                socket,
                crate::platform::dirs::home().join("Library/Application Support/rex/server.sock")
            );
        } else if cfg!(unix) && std::env::var_os("XDG_RUNTIME_DIR").is_none() {
            assert!(socket.starts_with("/run/user"));
        }
    }
}
