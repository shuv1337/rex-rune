//! Command-line tools this client runs, and where a platform keeps them when they are not
//! on `PATH`.
//!
//! On macOS `tailscale` and `rex` ship inside their apps, not in a folder `PATH` names:
//! the Tailscale app's own program answers as the CLI, and the Rex app carries its server
//! and CLI as a helper.
use std::path::{Path, PathBuf};

/// Where a Mac app keeps `name`, in the order they are tried. Empty for any other tool
/// and on every other platform.
fn bundled_candidates(name: &str) -> &'static [&'static str] {
    if !cfg!(target_os = "macos") {
        return &[];
    }
    match name {
        "tailscale" => &["/Applications/Tailscale.app/Contents/MacOS/Tailscale"],
        "rex" => &[
            "/Applications/Rex.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex",
            "/Applications/Rex Beta.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex",
        ],
        _ => &[],
    }
}

/// The copy of `name` inside its app, if this platform has apps that carry one and it is
/// installed.
pub fn bundled(name: &str) -> Option<PathBuf> {
    bundled_candidates(name)
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

/// Whether a folder of `PATH` holds `name`.
pub fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
}

/// What to run for `name`: the name itself when `PATH` has it, else the copy inside its
/// app, else the name again, so that a missing tool fails as "not found" under its own name.
pub fn program(name: &str) -> PathBuf {
    if on_path(name) {
        return Path::new(name).into();
    }
    bundled(name).unwrap_or_else(|| Path::new(name).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_is_its_own_name_unless_only_an_app_has_it() {
        // No app carries this, anywhere.
        assert_eq!(
            program("rex-rune-no-such-tool"),
            Path::new("rex-rune-no-such-tool")
        );
        assert!(bundled("sh").is_none());
        for name in ["tailscale", "rex"] {
            let program = program(name);
            if on_path(name) {
                assert_eq!(program, Path::new(name));
            } else if let Some(bundled) = bundled(name) {
                assert!(bundled.is_absolute() && bundled.ends_with(program.file_name().unwrap()));
                assert_eq!(program, bundled);
            } else {
                assert_eq!(program, Path::new(name));
            }
        }
        if !cfg!(target_os = "macos") {
            assert!(bundled("tailscale").is_none() && bundled("rex").is_none());
        }
    }
}
