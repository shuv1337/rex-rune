use clap::Parser;
mod app;

fn main() -> anyhow::Result<()> {
    let options = rex_linux::connection::Options::parse();
    if options.check_update {
        let code = rex_linux::update::run_cli_check(&rex_linux::update::CheckConfig::default());
        std::process::exit(code);
    }
    if rex_linux::update::installer::recover_current_install()? {
        use std::os::unix::process::CommandExt;
        // Recovery may have replaced the executable we just started. No app
        // actions have run yet, so preserve this launch's arguments exactly once.
        let error = std::process::Command::new(
            rex_linux::update::installer::InstallLayout::current()?.binary(),
        )
        .args(std::env::args_os().skip(1))
        .exec();
        return Err(error.into());
    }
    app::run(options)
}
