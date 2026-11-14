//! What differs between operating systems, for the app: desktop services that exist on
//! one platform and not another. The library's platform code (folders, processes, the
//! local endpoint) is `rex_rune::platform`.
//!
//! A `cfg` for an operating system belongs in here, not at a call site. A service that a
//! platform does not have says so through its interface, so the app can leave the feature
//! out instead of offering something that does nothing.
pub mod chrome;
pub mod fonts;
pub mod keys;
pub mod notify;
pub mod open;
pub mod selection;

/// Whether a Unix sound player can be run. Windows has no sound player backend yet.
pub fn sound_player_executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(windows)]
    {
        let _ = path;
        false
    }
}

/// Replace this process after an update on Unix. Windows has no installer or restart yet.
pub fn restart(binary: std::path::PathBuf, args: &[std::ffi::OsString]) -> anyhow::Error {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        std::process::Command::new(binary).args(args).exec().into()
    }
    #[cfg(windows)]
    {
        let _ = (binary, args);
        anyhow::anyhow!(
            "Restart after an update is not available on Windows; restart the client manually"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_players_need_a_supported_platform_and_executable_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("player");
        std::fs::write(&path, b"").unwrap();
        assert!(!sound_player_executable(&path));
        assert!(!sound_player_executable(temp.path()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(sound_player_executable(&path));
        }
        #[cfg(windows)]
        assert!(!sound_player_executable(&std::env::current_exe().unwrap()));
    }
}
