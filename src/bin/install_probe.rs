//! Drives only the generated, fake-Rex loopback fixture; never a saved SSH host.
use anyhow::{Context, Result, bail, ensure};
use rex_rune::ssh::{
    Problem, Target, Tunnel, hostkey,
    install::{self, Answer, Event, Run},
};
use std::{
    io::{self, BufRead, Write},
    path::Path,
    sync::mpsc,
    time::Duration,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [mode, endpoint, work] = args.as_slice() else {
        bail!(
            "Usage: rex-install-probe <lookup|scan|trust|start|install|forward> <fixture endpoint> <fixture folder>"
        );
    };
    let work = Path::new(work).canonicalize()?;
    let rex = work.join("bin/rex");
    let port = std::fs::read_to_string(work.join("fixture-port"))?
        .trim()
        .parse::<u16>()?;
    let target = Target::parse(endpoint)?.context("SSH endpoint required")?;
    ensure!(
        port > 1024
            && target.host == "127.0.0.1"
            && target.port == Some(port)
            && target.user.is_none()
            && target.socket.is_none(),
        "Refusing a non-fixture endpoint"
    );
    ensure!(
        std::env::var_os("HOME").as_deref() == Some(work.join("local").as_os_str())
            && std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .next()
                .as_deref()
                == Some(work.join("bin").as_path())
            && !rex.symlink_metadata()?.is_symlink()
            && std::fs::read_to_string(&rex)?.starts_with("#!/bin/sh\n# Rex Rune fixture ONLY\n"),
        "Refusing to run without the private fake Rex fixture"
    );
    match mode.as_str() {
        "lookup" => match rex_rune::ssh::resolve(&target, None).await {
            Ok(_) => println!("sock"),
            Err(error) => match Problem::from_error(&error) {
                Some(Problem::NoServer(facts)) => {
                    println!("offer {:?}", target.setup_offer(&Problem::NoServer(facts)))
                }
                Some(Problem::UnknownKey) => println!("key unknown"),
                Some(Problem::ChangedKey) => println!("key changed"),
                Some(Problem::AuthFailed) | None => return Err(error),
            },
        },
        "scan" | "trust" => {
            let offer = hostkey::scan(&target, None).await?;
            println!("fingerprint {}", offer.fingerprint);
            io::stdout().flush()?;
            if mode == "trust" {
                ensure!(input()? == "trust", "Trust was not confirmed");
                hostkey::trust(&offer, None).await?;
                println!("trusted");
            }
        }
        "forward" => {
            let tunnel = Tunnel::open(&target, None).await?;
            let address = tunnel
                .endpoint()
                .strip_prefix("unix://")
                .context("Unix forward required")?;
            let _connection = rex_rune::platform::local::connect(Path::new(address)).await?;
            tokio::time::sleep(Duration::from_millis(500)).await;
            println!("forwarded");
        }
        "start" => {
            rex_rune::ssh::setup::start_personal(endpoint, None).await?;
            println!("fake start accepted");
        }
        "install" => {
            if let Err(error) = install::preflight(&target, None).await {
                println!("refused {error}");
                return Ok(());
            }
            let (send, recv) = mpsc::channel();
            let run = Run::spawn(&target, rex.as_os_str(), None, move |event| {
                let _ = send.send(event);
            })?;
            println!("running");
            loop {
                io::stdout().flush()?;
                match recv.recv_timeout(Duration::from_secs(25))? {
                    Event::Ready { serial } => {
                        println!("question {serial}");
                        io::stdout().flush()?;
                        match input()?.as_str() {
                            "yes" => run
                                .answer(serial, Answer::Yes)
                                .map_err(|e| anyhow::anyhow!("{e:?}"))?,
                            "no" => run
                                .answer(serial, Answer::No)
                                .map_err(|e| anyhow::anyhow!("{e:?}"))?,
                            "stop" => run.stop(),
                            _ => bail!("expected yes, no or stop"),
                        }
                    }
                    Event::Output(text) => {
                        // The fixture can stop during a step, not just at an idle prompt.
                        if text.contains("STOP-READY") {
                            println!("mid-step");
                            io::stdout().flush()?;
                            ensure!(input()? == "stop", "expected stop");
                            run.stop();
                        }
                        println!("output {}", text.escape_default());
                    }
                    Event::Unrecognized => {
                        println!("unrecognized");
                        io::stdout().flush()?;
                        ensure!(input()? == "stop", "expected stop");
                        run.stop();
                    }
                    Event::Exited { code } => {
                        println!("exit {code:?}");
                        break;
                    }
                    _ => {}
                }
            }
        }
        _ => bail!("Unknown probe mode"),
    }
    Ok(())
}

fn input() -> Result<String> {
    let mut line = String::new();
    ensure!(
        io::stdin().lock().read_line(&mut line)? > 0,
        "fixture controller closed"
    );
    Ok(line.trim().into())
}
