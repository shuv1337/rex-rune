//! What differs between operating systems, for the app: desktop services that exist on
//! one platform and not another. The library's platform code (folders, processes, the
//! local endpoint) is `rex_rune::platform`.
//!
//! A `cfg` for an operating system belongs in here, not at a call site. A service that a
//! platform does not have says so through its interface, so the app can leave the feature
//! out instead of offering something that does nothing.
pub mod chrome;
pub mod keys;
pub mod open;
pub mod selection;

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
