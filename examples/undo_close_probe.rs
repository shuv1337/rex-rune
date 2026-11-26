//! Drive the production host worker against the disposable server from undo-close-private.py.
use anyhow::{Context, Result, ensure};
use rex_rune::{
    connection::{
        Command, Connection, HostCommand, HostConnection, HostModel, HumanClose,
        UNDO_CLOSE_REQUIRES_API,
    },
    layout::shell_layout,
};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

fn wait<T>(
    host: &HostConnection,
    what: &str,
    check: impl Fn(&HostModel) -> Option<T>,
) -> Result<T> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        {
            let model = host.model.lock().unwrap();
            if let Some(value) = check(&model) {
                return Ok(value);
            }
            ensure!(
                Instant::now() < deadline,
                "{what}: {}; notice: {:?}",
                model.status,
                model.notice
            );
        }
        let _ = host.wake.recv_timeout(Duration::from_millis(20));
    }
}

fn focused(host: &HostConnection) -> Result<(String, Connection)> {
    wait(host, "live focused terminal", |m| {
        let id = m.focused()?;
        let term = m.terminals.get(id)?;
        let live = term.model.lock().unwrap().connected;
        live.then(|| (id.into(), term.clone()))
    })
}

fn shell(term: &Connection, text: &str) -> Result<()> {
    term.commands.try_send(Command::Paste(text.into()))?;
    term.commands.try_send(Command::Key(json!({
        "key":"enter","mods":"","text":"","action":"press","composing":false
    })))?;
    Ok(())
}

fn output(host: &HostConnection, term: &Connection, expected: &str) -> Result<()> {
    wait(host, expected, |_| {
        let terminal = term.model.lock().unwrap();
        let frame = terminal.frame.as_ref()?;
        frame
            .cells
            .iter()
            .any(|row| {
                row.iter()
                    .map(|cell| cell.text.as_str())
                    .collect::<String>()
                    .contains(expected)
            })
            .then_some(())
    })
}

fn request(host: &HostConnection, method: &str, payload: Value) -> Result<()> {
    host.commands.send(HostCommand::Request {
        method: method.into(),
        payload,
    })?;
    Ok(())
}

fn undo(host: &HostConnection) -> Result<()> {
    let epoch = host.model.lock().unwrap().connection_epoch;
    host.commands.send(HostCommand::UndoClose { epoch })?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 3,
        "private Unix endpoint and expected API revision required"
    );
    let path = Path::new(
        args[1]
            .strip_prefix("unix://")
            .context("private Unix endpoint required")?,
    );
    ensure!(
        path.is_absolute()
            && path.file_name().is_some_and(|n| n == "rex.sock")
            && path
                .parent()
                .and_then(Path::file_name)
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rex-undo-close-")),
        "only the fixture's private socket is accepted"
    );
    let revision = args[2].parse::<u32>()?;
    let host = HostConnection::spawn(
        args[1].clone(),
        None,
        Some(("original".into(), Some("/bin/sh".into()))),
        None,
        true,
        None,
    );
    let (sid, anchor, tab) = wait(&host, "original session", |m| {
        m.connected.then_some(())?;
        Some((
            m.selected.clone()?,
            m.focused()?.to_owned(),
            m.window()?.window_id.clone()?,
        ))
    })?;
    ensure!(
        host.model.lock().unwrap().protocol.api_revision == Some(revision),
        "unexpected negotiated revision"
    );
    if revision < 20 {
        undo(&host)?;
        wait(&host, "old revision refusal", |m| {
            (m.connected && m.notice.as_deref() == Some(UNDO_CLOSE_REQUIRES_API)).then_some(())
        })?;
    }
    let layout = shell_layout(Some("/bin/sh"), None);
    request(
        &host,
        "session.new_split",
        json!({"session_id":sid,"anchor_id":anchor,
        "direction":"horizontal","side":"after","layout":layout,"focus":true}),
    )?;
    let split = wait(&host, "new split", |m| {
        m.focused().filter(|b| *b != anchor).map(str::to_owned)
    })?;
    let (_, term) = focused(&host)?;
    ensure!(term.model.lock().unwrap().block == split, "wrong split");
    if revision >= 20 {
        shell(&term, "rex_undo_held=alive; echo ready-$rex_undo_held")?;
        output(&host, &term, "ready-alive")?;
    }
    let mut previous = term;
    for (kind, fields, gone) in [
        (
            HumanClose::Pane,
            json!({"session_id":sid,"block_id":split}),
            "pane",
        ),
        (
            HumanClose::Tab,
            json!({"session_id":sid,"window_id":tab}),
            "tab",
        ),
        (HumanClose::Session, json!({"session_id":sid}), "session"),
    ] {
        if kind == HumanClose::Tab {
            // Keep the session open when the original tab closes.
            request(
                &host,
                "session.new_window",
                json!({"session_id":sid,"layout":layout,"focus":false}),
            )?;
            wait(&host, "second tab", |m| {
                (m.view()?.windows.len() == 2).then_some(())
            })?;
        }
        if kind == HumanClose::Session {
            // Restore into the original session while a different one is selected.
            host.commands.send(HostCommand::CreateSession {
                label: Some("other".into()),
                shell: Some("/bin/sh".into()),
            })?;
            wait(&host, "another session selected", |m| {
                m.selected.as_ref().filter(|id| **id != sid).cloned()
            })?;
        }
        host.commands.send(HostCommand::Close {
            kind,
            payload: fields,
        })?;
        wait(&host, gone, |m| {
            let view = m.views.get(&sid);
            let absent = match kind {
                HumanClose::Pane => view.is_some_and(|v| {
                    v.windows
                        .iter()
                        .flat_map(|w| &w.layers)
                        .flat_map(|l| &l.blocks)
                        .all(|b| b.block_id.as_deref() != Some(&split))
                }),
                HumanClose::Tab => view.is_some_and(|v| {
                    v.windows
                        .iter()
                        .all(|w| w.window_id.as_deref() != Some(&tab))
                }),
                HumanClose::Session => m.sessions.iter().all(|s| s.session_id != sid),
            };
            (m.connected && m.notice.is_none() && absent).then_some(())
        })?;
        if revision >= 20 {
            undo(&host)?;
            let restored = wait(&host, "same identity and fresh attached stream", |m| {
                if m.selected.as_deref() != Some(&sid)
                    || m.window()?.window_id.as_deref() != Some(&tab)
                    || m.focused() != Some(&split)
                    || !m.sessions.iter().any(|s| s.session_id == sid)
                {
                    return None;
                }
                let connection = m.terminals.get(&split)?;
                let terminal = connection.model.lock().unwrap();
                (terminal.connected
                    && terminal.restore_generation == 1
                    && !Arc::ptr_eq(&connection.model, &previous.model))
                .then(|| connection.clone())
            })?;
            shell(&restored, &format!("echo {gone}-$rex_undo_held"))?;
            output(&host, &restored, &format!("{gone}-alive"))?;
            previous = restored;
            println!(
                "API {revision}: restored {gone} with the original live PTY and a fresh stream"
            );
        } else {
            println!("API {revision}: closed {gone} without a schema error");
        }
    }
    if revision >= 20 {
        undo(&host)?;
        wait(&host, "empty history notice", |m| {
            (m.connected && m.notice.as_deref() == Some("Nothing to reopen")).then_some(())
        })?;
        println!("API {revision}: empty history kept the host connected");
    }
    drop(host);
    ensure!(
        rex_rune::connection::wait_for_hosts(Duration::from_secs(3)),
        "worker did not stop"
    );
    Ok(())
}
