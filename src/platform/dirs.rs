//! The per-user folders this client reads and writes.
//!
//! | Folder | Linux and other Unix | macOS |
//! | --- | --- | --- |
//! | settings | `$XDG_CONFIG_HOME/rex-rune` (`~/.config/rex-rune`) | `~/Library/Application Support/rex-rune` |
//! | state | `$XDG_STATE_HOME/rex-rune` (`~/.local/state/rex-rune`) | `~/Library/Application Support/rex-rune` |
//! | runtime | `$XDG_RUNTIME_DIR/rex-rune`, else `<tmp>/rex-rune-<uid>` | `<tmp>/rex-rune-<uid>` |
//!
//! Settings are `workspace.json` and `shortcuts.json`; state is `update.json` and
//! `downloads.json`; runtime holds the sockets of SSH tunnels. On macOS the temporary
//! folder is already private to the user. A relative `XDG_*` value is used as it is given,
//! as before these moved here.
use std::path::PathBuf;

/// The folder name this client uses under each base folder.
pub const APP: &str = "rex-rune";

/// The user's home folder, or an empty path when `HOME` is unset.
pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Where the settings files live.
pub fn config() -> PathBuf {
    config_base().join(APP)
}

/// Where update and download records live.
pub fn state() -> PathBuf {
    state_base().join(APP)
}

/// Where this user's SSH tunnel sockets live, one folder per process.
pub fn runtime() -> PathBuf {
    match xdg_runtime() {
        Some(runtime) => runtime.join(APP),
        None => std::env::temp_dir().join(format!("{APP}-{}", super::process::user_id())),
    }
}

/// The folders that held `rex-linux` folders through 0.7.x, for the one-time move in
/// [`crate::legacy`]. Empty where that client never ran.
pub fn legacy_bases() -> Vec<PathBuf> {
    if cfg!(target_os = "linux") {
        vec![config_base(), state_base()]
    } else {
        Vec::new()
    }
}

fn application_support() -> PathBuf {
    home().join("Library/Application Support")
}

fn config_base() -> PathBuf {
    if cfg!(target_os = "macos") {
        return application_support();
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
}

fn state_base() -> PathBuf {
    if cfg!(target_os = "macos") {
        return application_support();
    }
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home().join(".local/state"))
}

/// `$XDG_RUNTIME_DIR` where sessions have one: not on macOS, which sets none.
pub(super) fn xdg_runtime() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return None;
    }
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|runtime| !runtime.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_and_state_are_named_for_the_client_under_the_platforms_folder() {
        assert!(config().ends_with(APP) && state().ends_with(APP));
        if cfg!(target_os = "macos") {
            let support = home().join("Library/Application Support").join(APP);
            assert_eq!(config(), support);
            assert_eq!(state(), support);
            assert!(legacy_bases().is_empty(), "rex-linux never ran on macOS");
        }
        if cfg!(target_os = "linux") {
            assert_eq!(legacy_bases().len(), 2);
        }
        // The runtime folder is this user's alone: named for them unless the session gives one.
        let runtime = runtime();
        let name = runtime.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name == APP || name == format!("{APP}-{}", crate::platform::process::user_id()));
    }
}
