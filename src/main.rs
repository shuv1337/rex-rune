use clap::Parser;
mod app;

fn main() -> anyhow::Result<()> {
    let options = rex_linux::connection::Options::parse();
    if options.check_update {
        let code = rex_linux::update::run_cli_check(&rex_linux::update::CheckConfig::default());
        std::process::exit(code);
    }
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let recovery_notice = match rex_linux::update::installer::recover_current_install() {
        Ok(Some(binary)) => {
            // No workspace actions have run yet; preserve all launch arguments.
            Some(recovery_error(reexec(binary, &args)))
        }
        Ok(None) => None,
        Err(error) => Some(recovery_error(error)),
    };
    if let Some(binary) = app::run(options, recovery_notice)? {
        // The workspace is gone and host shutdown has completed before replacing
        // the process. Preserve per-launch font/window/connection overrides.
        return Err(reexec(binary, &restart_args(args)));
    }
    Ok(())
}

fn recovery_error(error: impl std::fmt::Display) -> String {
    let notice = format!(
        "Update recovery failed: {error:#}. Rex will still open. See docs/self-updates.md for manual recovery."
    );
    eprintln!("{notice}");
    notice
}

fn reexec(binary: std::path::PathBuf, args: &[std::ffi::OsString]) -> anyhow::Error {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(binary).args(args).exec().into()
}

fn restart_args(args: Vec<std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    let mut result = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        // --new was already fulfilled on this launch; never create a second session.
        if arg == "--new" {
            args.next();
        } else if arg == "--" {
            result.push(arg);
            result.extend(args);
            break;
        } else if !arg.to_string_lossy().starts_with("--new=") {
            result.push(arg);
        }
    }
    result
}

#[cfg(test)]
mod launch_tests {
    use super::*;

    #[test]
    fn restart_keeps_launch_overrides_without_repeating_session_creation() {
        let args = [
            "--font",
            "Mono",
            "--font-size=18",
            "--window-size",
            "900x600",
            "--new",
            "Shell",
            "--config",
            "/private/workspace.json",
            "--",
            "--snap",
            "out.png",
        ];
        let expected = [
            "--font",
            "Mono",
            "--font-size=18",
            "--window-size",
            "900x600",
            "--config",
            "/private/workspace.json",
            "--",
            "--snap",
            "out.png",
        ];
        assert_eq!(
            restart_args(args.map(Into::into).into()),
            expected.map(std::ffi::OsString::from)
        );
        assert!(restart_args(vec!["--new=Shell".into()]).is_empty());
    }
}
