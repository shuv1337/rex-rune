//! Metadata-only live worker probe for scripts/program-resume-private.py.
use anyhow::{Context, Result, bail, ensure};
use rex_rune::{
    connection::{HostCommand, HostConnection, HostModel, wait_for_hosts},
    program_resume::Trust,
    transport,
};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn private_endpoint(path: &Path) -> Result<String> {
    let path = path.canonicalize().context("private socket must exist")?;
    let endpoint = format!("unix://{}", path.display());
    ensure!(
        endpoint != transport::local_endpoint()
            && path.file_name().is_some_and(|n| n == "rex.sock")
            && path
                .parent()
                .and_then(Path::file_name)
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rex-resume-")),
        "refusing anything outside a private rex-resume-*/rex.sock fixture"
    );
    Ok(endpoint)
}

fn wait<T>(host: &HostConnection, read: impl Fn(&HostModel) -> Option<T>) -> Result<T> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(value) = read(&host.model.lock().expect("host model")) {
            return Ok(value);
        }
        ensure!(Instant::now() < deadline, "private worker did not settle");
        let _ = host.wake.recv_timeout(Duration::from_millis(30));
    }
}

fn summary(model: &HostModel) -> Value {
    let snapshots: serde_json::Map<String, Value> = model
        .program_resume
        .iter()
        .map(|(block, snapshot)| {
            let records: Vec<_> = snapshot
                .records
                .iter()
                .map(|record| {
                    let verdict = match &record.trust {
                        Trust::Trusted { rule } => json!({"trusted": true, "rule": rule}),
                        Trust::Untrusted(reason) => {
                            json!({"trusted": false, "reason": format!("{reason:?}")})
                        }
                    };
                    json!({"pid": record.key.pid, "program": record.program,
                           "owner_name": record.owner_name, "cwd": record.cwd,
                           "chosen": record.chosen, "verdict": verdict})
                })
                .collect();
            (
                block.clone(),
                json!({"records": records, "truncated": snapshot.truncated}),
            )
        })
        .collect();
    json!({
        "api_revision": model.protocol.api_revision,
        "epoch": model.connection_epoch,
        "connected": model.connected,
        "reading": model.program_resume_reading,
        "unsupported": model.program_resume_unsupported,
        "snapshots": snapshots
    })
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().context("private Rex socket path required")?);
    let endpoint = private_endpoint(&path)?;
    let session = args
        .next()
        .context("session ID required")?
        .into_string()
        .map_err(|_| anyhow::anyhow!("invalid session ID"))?;
    ensure!(session.starts_with("session:"), "session ID required");
    let max = args
        .next()
        .and_then(|arg| arg.to_str()?.parse().ok())
        .context("maximum API revision required")?;
    ensure!(args.next().is_none(), "unexpected arguments");
    let host = HostConnection::spawn_with_max_revision(
        endpoint,
        Some(session.clone()),
        None,
        None,
        true,
        None,
        max,
    );
    let epoch = wait(&host, |m| {
        (m.connected
            && m.selected.as_deref() == Some(session.as_str())
            && m.views.contains_key(&session))
        .then_some(m.connection_epoch)
    })?;
    let stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    writeln!(
        stdout,
        "{}",
        summary(&host.model.lock().expect("host model"))
    )?;
    stdout.flush()?;
    for (step, line) in stdin.lines().enumerate() {
        match line?.as_str() {
            "watch" => host.commands.send(HostCommand::WatchProgramResume {
                epoch,
                session: Some(session.clone()),
            })?,
            "close" => host.commands.send(HostCommand::WatchProgramResume {
                epoch,
                session: None,
            })?,
            "snapshot" => {}
            "quit" => break,
            _ => bail!("unknown private probe operation"),
        }
        let label = format!("resume-fixture-{max}-{step}");
        host.commands.send(HostCommand::Request {
            method: "session.set_label".into(),
            payload: json!({"session_id": session, "label": label}),
        })?;
        let snap = wait(&host, |m| {
            (m.connected
                && m.connection_epoch == epoch
                && m.sessions
                    .iter()
                    .any(|s| s.session_id == session && s.label == label))
            .then(|| summary(m))
        })?;
        writeln!(stdout, "{snap}")?;
        stdout.flush()?;
    }
    drop(host);
    ensure!(
        wait_for_hosts(Duration::from_secs(3)),
        "private worker did not stop"
    );
    Ok(())
}
