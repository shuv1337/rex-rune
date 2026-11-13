//! "Update now" for a pacman install: asking the package's own root helper, through
//! `pkexec`, to upgrade `rex-rune-bin` to one version.
//!
//! The client decides nothing here that matters. It passes a version and reads an exit
//! code. The helper (`resources/pacman/rex-rune-update`) treats its caller as hostile: it
//! downloads the package itself, checks it against the Rex package key and this machine's
//! pacman keyring, and installs only a newer release. polkit asks for an administrator's
//! password before any of it runs. `docs/plans/pacman-update-now.md` has the reasoning.
use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Where the package installs its helper, and where polkit allows it to be run from.
pub const HELPER: &str = "/usr/lib/rex-rune/rex-rune-update";
pub const PKEXEC: &str = "/usr/bin/pkexec";

/// The helper and the program that runs it as root, both present.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Helper {
    pub pkexec: PathBuf,
    pub helper: PathBuf,
}

/// How an update ended, from the exit code alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The package was upgraded.
    Installed,
    /// The person closed polkit's dialog.
    Dismissed,
    /// Nothing was installed; `sentence` says why.
    Refused(Refusal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// pkexec's 127: not authorized, no authentication agent, or pkexec itself failed.
    NotAuthorized,
    /// The helper's 65: not installed under this name, or already at that version or newer.
    NotNewer,
    /// 66: `IgnorePkg` or `IgnoreGroup`.
    HeldBack,
    /// 67: another update, or another pacman.
    Busy,
    /// 68.
    Download,
    /// 69.
    Signature,
    /// 70.
    Untrusted,
    /// 71.
    WrongPackage,
    /// 72.
    OtherPackages,
    /// 73.
    Pacman,
    /// 74: no systemd, a missing tool, or the update service did not start or finish.
    Unsupported,
    /// `pkexec` could not be started at all.
    NoPkexec,
    /// Any other exit code, or a signal.
    Unexpected(Option<i32>),
}

impl Helper {
    /// The installed helper, where this platform has packages and both files exist.
    pub fn installed() -> Option<Self> {
        if !crate::platform::HAS_RELEASES {
            return None;
        }
        Self::at(Path::new(PKEXEC), Path::new(HELPER))
    }

    pub fn at(pkexec: &Path, helper: &Path) -> Option<Self> {
        (pkexec.is_file() && helper.is_file()).then(|| Self {
            pkexec: pkexec.to_owned(),
            helper: helper.to_owned(),
        })
    }

    /// What is run, in full: `pkexec --disable-internal-agent <helper> <version>`. The
    /// agent that asks for the password is the desktop's own; pkexec's text-mode one would
    /// have no terminal to ask on.
    pub fn arguments(&self, version: &str) -> Vec<String> {
        vec![
            "--disable-internal-agent".into(),
            self.helper.to_string_lossy().into_owned(),
            version.to_owned(),
        ]
    }

    /// Run the helper for `version` and wait for it. Blocks for as long as the dialog is
    /// open and the update takes; call it from a worker thread.
    pub fn update(&self, version: &str) -> Outcome {
        self.update_with(version, run)
    }

    /// As `update`, with the process started by `run` (tests never start `pkexec`).
    pub fn update_with(
        &self,
        version: &str,
        run: impl FnOnce(&Path, &[String]) -> io::Result<Option<i32>>,
    ) -> Outcome {
        // The helper checks this again and trusts nothing here; this only keeps a version
        // that cannot be one from ever reaching a command line.
        if super::StableVersion::parse(version).is_err() {
            return Outcome::Refused(Refusal::Unexpected(None));
        }
        match run(&self.pkexec, &self.arguments(version)) {
            Ok(code) => Outcome::from_exit(code),
            Err(_) => Outcome::Refused(Refusal::NoPkexec),
        }
    }
}

/// Start the program with nothing to read and nowhere to write, and wait for its exit code
/// (`None` if a signal ended it).
fn run(program: &Path, arguments: &[String]) -> io::Result<Option<i32>> {
    Ok(Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .code())
}

impl Outcome {
    /// The helper's exit codes are 64 to 74; 126 and 127 are pkexec's own.
    pub fn from_exit(code: Option<i32>) -> Self {
        Self::Refused(match code {
            Some(0) => return Self::Installed,
            Some(126) => return Self::Dismissed,
            Some(127) => Refusal::NotAuthorized,
            Some(65) => Refusal::NotNewer,
            Some(66) => Refusal::HeldBack,
            Some(67) => Refusal::Busy,
            Some(68) => Refusal::Download,
            Some(69) => Refusal::Signature,
            Some(70) => Refusal::Untrusted,
            Some(71) => Refusal::WrongPackage,
            Some(72) => Refusal::OtherPackages,
            Some(73) => Refusal::Pacman,
            Some(74) => Refusal::Unsupported,
            other => Refusal::Unexpected(other),
        })
    }
}

impl Refusal {
    /// Why nothing was installed, as the end of "Update failed: ". No full stop.
    pub fn sentence(&self) -> String {
        match self {
            Self::NotAuthorized => "it was not authorized. It needs an administrator's password, which your desktop's polkit agent asks for; if no dialog appeared, no agent is running".into(),
            Self::NotNewer => "the installed package is already this version or newer, or is not rex-rune-bin. If it was updated another way, restart Rex Rune".into(),
            Self::HeldBack => "rex-rune-bin is held back in pacman.conf (IgnorePkg or IgnoreGroup)".into(),
            Self::Busy => "another update or another pacman is running. Try again when it has finished".into(),
            Self::Download => "the package could not be downloaded from pkg.shuv.dev. A new release can take a few minutes to appear there".into(),
            Self::Signature => "the package was not signed by the Rex package key, or that key has expired. Nothing was installed".into(),
            Self::Untrusted => "this computer's pacman keyring does not trust the Rex package key. Nothing was installed".into(),
            Self::WrongPackage => "the package site served something other than the package asked for. Nothing was installed".into(),
            Self::OtherPackages => "installing it would change other packages too".into(),
            Self::Pacman => "pacman refused or failed; `journalctl -t rex-rune-update` has its words".into(),
            Self::Unsupported => "this system cannot run the update helper (it needs systemd), or the update did not finish; `journalctl -t rex-rune-update` says which".into(),
            Self::NoPkexec => "pkexec could not be started".into(),
            Self::Unexpected(Some(code)) => format!("the update helper ended with exit code {code}"),
            Self::Unexpected(None) => "the update helper did not finish".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn helper() -> Helper {
        Helper {
            pkexec: PKEXEC.into(),
            helper: HELPER.into(),
        }
    }

    #[test]
    fn the_command_is_pkexec_the_helper_and_one_version() {
        let mut seen = None;
        let outcome = helper().update_with("1.2.3", |program, arguments| {
            seen = Some((program.to_owned(), arguments.to_vec()));
            Ok(Some(0))
        });
        assert_eq!(outcome, Outcome::Installed);
        let (program, arguments) = seen.unwrap();
        assert_eq!(program, Path::new("/usr/bin/pkexec"));
        assert_eq!(
            arguments,
            [
                "--disable-internal-agent",
                "/usr/lib/rex-rune/rex-rune-update",
                "1.2.3"
            ]
        );
    }

    #[test]
    fn nothing_but_a_version_reaches_the_command_line() {
        for version in [
            "",
            "latest",
            "1.2",
            "1.2.3-1",
            "1.2.3 --help",
            "--in-service",
            "1.2.3;id",
            "../1.2.3",
            "v1.2.3",
        ] {
            let mut ran = false;
            let outcome = helper().update_with(version, |_, _| {
                ran = true;
                Ok(Some(0))
            });
            assert!(!ran, "{version:?} was run");
            assert_eq!(outcome, Outcome::Refused(Refusal::Unexpected(None)));
        }
    }

    #[test]
    fn every_exit_code_has_one_meaning() {
        assert_eq!(Outcome::from_exit(Some(0)), Outcome::Installed);
        assert_eq!(Outcome::from_exit(Some(126)), Outcome::Dismissed);
        for (code, refusal) in [
            (127, Refusal::NotAuthorized),
            (65, Refusal::NotNewer),
            (66, Refusal::HeldBack),
            (67, Refusal::Busy),
            (68, Refusal::Download),
            (69, Refusal::Signature),
            (70, Refusal::Untrusted),
            (71, Refusal::WrongPackage),
            (72, Refusal::OtherPackages),
            (73, Refusal::Pacman),
            (74, Refusal::Unsupported),
            // 64 is "not a version": this client never sends one, so it is unexpected.
            (64, Refusal::Unexpected(Some(64))),
            (1, Refusal::Unexpected(Some(1))),
            (255, Refusal::Unexpected(Some(255))),
        ] {
            assert_eq!(Outcome::from_exit(Some(code)), Outcome::Refused(refusal));
            let sentence = refusal.sentence();
            assert!(
                !sentence.is_empty() && !sentence.ends_with('.'),
                "{sentence}"
            );
        }
        // Killed by a signal: not a success.
        assert_eq!(
            Outcome::from_exit(None),
            Outcome::Refused(Refusal::Unexpected(None))
        );
    }

    #[test]
    fn only_success_counts_as_installed() {
        for code in (1..=255).map(Some).chain([None, Some(-1), Some(256)]) {
            assert_ne!(Outcome::from_exit(code), Outcome::Installed, "{code:?}");
        }
    }

    #[test]
    fn a_pkexec_that_cannot_start_is_a_refusal() {
        let outcome = helper().update_with("1.2.3", |_, _| {
            Err(io::Error::new(io::ErrorKind::NotFound, "no pkexec"))
        });
        assert_eq!(outcome, Outcome::Refused(Refusal::NoPkexec));
    }

    #[test]
    fn the_helper_is_offered_only_when_both_files_exist() {
        let temp = tempfile::tempdir().unwrap();
        let pkexec = temp.path().join("pkexec");
        let update = temp.path().join("rex-rune-update");
        assert_eq!(Helper::at(&pkexec, &update), None);
        std::fs::write(&pkexec, "").unwrap();
        assert_eq!(Helper::at(&pkexec, &update), None);
        std::fs::write(&update, "").unwrap();
        assert!(Helper::at(&pkexec, &update).is_some());
        // A folder of that name is not the helper.
        std::fs::remove_file(&update).unwrap();
        std::fs::create_dir(&update).unwrap();
        assert_eq!(Helper::at(&pkexec, &update), None);
    }
}
