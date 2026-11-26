//! Live worker probe for scripts/api-compat-private.py.
//! Exercises down-negotiation across API revisions 0..=24 against a private disposable server.
use anyhow::{Context, Result, bail, ensure};
use rex_rune::{
    connection::{HostCommand, HostConnection, HostModel, wait_for_hosts},
    layout, transport,
};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn private_endpoint(raw: &str) -> Result<String> {
    let path = raw.strip_prefix("unix://").unwrap_or(raw);
    let path = PathBuf::from(path)
        .canonicalize()
        .context("private socket must exist")?;
    let endpoint = format!("unix://{}", path.display());
    ensure!(
        endpoint != transport::local_endpoint()
            && path.file_name().is_some_and(|n| n == "rex.sock")
            && path
                .parent()
                .and_then(Path::file_name)
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rex-api-compat-")),
        "refusing anything outside a private rex-api-compat-*/rex.sock fixture"
    );
    Ok(endpoint)
}

fn wait<T>(host: &HostConnection, what: &str, read: impl Fn(&HostModel) -> Option<T>) -> Result<T> {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if let Some(value) = read(&host.model.lock().expect("host model")) {
            return Ok(value);
        }
        ensure!(Instant::now() < deadline, "timed out waiting for {what}");
        let _ = host.wake.recv_timeout(Duration::from_millis(25));
    }
}

fn frame_text(frame: &rex_rune::terminal::Frame) -> String {
    let total = usize::from(frame.rows) * usize::from(frame.cols);
    if total == 0 {
        return String::new();
    }
    frame.selected_text(0, total - 1)
}

fn pane_count(model: &HostModel, sid: &str) -> usize {
    model
        .views
        .get(sid)
        .and_then(|v| v.windows.first())
        .map_or(0, |w| w.layers.iter().map(|l| l.blocks.len()).sum())
}

fn verify_common(model: &HostModel, pinned_rev: u32) -> Result<()> {
    ensure!(
        model.protocol.api_revision == Some(pinned_rev),
        "expected negotiated API Some({pinned_rev}), got {:?}",
        model.protocol.api_revision
    );
    ensure!(
        model
            .server_info
            .api_revisions
            .is_some_and(|(min, max)| min == 0 && max >= pinned_rev),
        "expected server_info.api_revisions with min == 0 and max >= {pinned_rev}, got {:?}",
        model.server_info.api_revisions
    );
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let raw_endpoint = args.next().context("private Unix endpoint required")?;
    let endpoint = private_endpoint(&raw_endpoint)?;
    let pinned_rev: u32 = args
        .next()
        .context("pinned API revision required")?
        .parse()
        .context("pinned API revision must be an integer")?;
    let mode = args.next().context("mode required")?;
    ensure!(args.next().is_none(), "unexpected extra arguments");

    let host =
        HostConnection::spawn_with_max_revision(endpoint, None, None, None, true, None, pinned_rev);
    let (epoch, sid, block_id, initial_panes) = wait(&host, "ready session and terminals", |m| {
        if !m.connected {
            return None;
        }
        let sid = m.selected.as_ref()?;
        let count = pane_count(m, sid);
        if count == 0 || m.terminals.len() < count {
            return None;
        }
        if !m.terminals.values().all(|t| {
            let tm = t.model.lock().expect("terminal model");
            tm.connected && tm.frame.is_some()
        }) {
            return None;
        }
        let focused = m.focused()?.to_owned();
        Some((m.connection_epoch, sid.clone(), focused, count))
    })?;

    {
        let model = host.model.lock().expect("host model");
        verify_common(&model, pinned_rev)?;
    }

    match mode.as_str() {
        "rev0" => {
            ensure!(pinned_rev == 0, "rev0 mode requires pinned_rev == 0");
            {
                let model = host.model.lock().expect("host model");
                ensure!(
                    model.protocol.bundled_tickets_used == 0
                        && model.protocol.data_connect_requests >= 1,
                    "rev 0 should use data.connect: bundled={}, data_connect={}",
                    model.protocol.bundled_tickets_used,
                    model.protocol.data_connect_requests
                );
            }
            // Verify live output streaming over the terminal data channel on API 0.
            host.commands.send(HostCommand::Request {
                method: "com.superlogical.terminal.write".into(),
                payload: json!({
                    "session_id": sid,
                    "block_id": block_id,
                    "args": {"data": "echo API0_LIVE_OK\n"}
                }),
            })?;
            wait(&host, "live terminal output on API 0", |m| {
                let pane = m.terminals.get(&block_id)?;
                let tm = pane.model.lock().expect("terminal model");
                let frame = tm.frame.as_ref()?;
                frame_text(frame).contains("API0_LIVE_OK").then_some(())
            })?;

            // Verify session.new_split via HostCommand::NewTerminal translates anchor_id -> anchor_block_id.
            host.commands.send(HostCommand::NewTerminal {
                method: "session.new_split".into(),
                payload: json!({
                    "session_id": sid,
                    "anchor_id": block_id,
                    "direction": "horizontal",
                    "side": "after",
                    "layout": layout::shell_layout(None, None),
                    "focus": true,
                }),
                cwd_from: block_id.clone(),
            })?;
            wait(&host, "rev 0 NewTerminal split pane", |m| {
                let count = pane_count(m, &sid);
                (count == initial_panes + 1
                    && m.terminals.len() >= count
                    && m.terminals
                        .values()
                        .all(|t| t.model.lock().expect("terminal model").connected))
                .then_some(())
            })?;

            // Verify session.new_split via HostCommand::Request also translates anchor_id -> anchor_block_id.
            host.commands.send(HostCommand::Request {
                method: "session.new_split".into(),
                payload: json!({
                    "session_id": sid,
                    "anchor_id": block_id,
                    "direction": "vertical",
                    "side": "after",
                    "layout": layout::shell_layout(None, None),
                    "focus": true,
                }),
            })?;
            wait(&host, "rev 0 Request split pane", |m| {
                let count = pane_count(m, &sid);
                (count == initial_panes + 2
                    && m.terminals.len() >= count
                    && m.terminals
                        .values()
                        .all(|t| t.model.lock().expect("terminal model").connected))
                .then_some(())
            })?;

            // Verify StopServer is refused locally on API 0 without stopping the server.
            host.commands.send(HostCommand::StopServer { epoch })?;
            wait(&host, "rev 0 local StopServer refusal", |m| {
                let notice = m.notice.as_deref()?;
                (notice.contains("API revision 4")
                    && notice.contains("revision 0")
                    && m.server_stop_result.as_deref() == Some(notice))
                .then_some(())
            })?;
            host.commands.send(HostCommand::Request {
                method: "session.set_label".into(),
                payload: json!({"session_id": sid, "label": "compat-rev0-alive"}),
            })?;
            wait(&host, "rev 0 server still alive after refused stop", |m| {
                m.sessions
                    .iter()
                    .any(|s| s.session_id == sid && s.label == "compat-rev0-alive")
                    .then_some(())
            })?;
        }
        "rev1" => {
            ensure!(pinned_rev == 1, "rev1 mode requires pinned_rev == 1");
            {
                let model = host.model.lock().expect("host model");
                ensure!(
                    model.protocol.bundled_tickets_used == 0
                        && model.protocol.data_connect_requests >= 1,
                    "rev 1 should use data.connect: bundled={}, data_connect={}",
                    model.protocol.bundled_tickets_used,
                    model.protocol.data_connect_requests
                );
            }
            host.commands.send(HostCommand::NewTerminal {
                method: "session.new_split".into(),
                payload: json!({
                    "session_id": sid,
                    "anchor_id": block_id,
                    "direction": "horizontal",
                    "side": "after",
                    "layout": layout::shell_layout(None, None),
                    "focus": true,
                }),
                cwd_from: block_id.clone(),
            })?;
            wait(&host, "rev 1 split pane via anchor_id", |m| {
                let count = pane_count(m, &sid);
                (count == initial_panes + 1
                    && m.terminals.len() >= count
                    && m.terminals
                        .values()
                        .all(|t| t.model.lock().expect("terminal model").connected))
                .then_some(())
            })?;
        }
        "rev3" => {
            ensure!(pinned_rev == 3, "rev3 mode requires pinned_rev == 3");
            {
                let model = host.model.lock().expect("host model");
                ensure!(
                    model.protocol.bundled_tickets_used == 0
                        && model.protocol.data_connect_requests >= 1,
                    "rev 3 should use data.connect"
                );
            }
            host.commands.send(HostCommand::StopServer { epoch })?;
            wait(&host, "rev 3 local StopServer refusal", |m| {
                let notice = m.notice.as_deref()?;
                (notice.contains("API revision 4")
                    && notice.contains("revision 3")
                    && m.server_stop_result.as_deref() == Some(notice))
                .then_some(())
            })?;
            host.commands.send(HostCommand::Request {
                method: "session.set_label".into(),
                payload: json!({"session_id": sid, "label": "compat-rev3-alive"}),
            })?;
            wait(&host, "rev 3 server still alive after refused stop", |m| {
                m.sessions
                    .iter()
                    .any(|s| s.session_id == sid && s.label == "compat-rev3-alive")
                    .then_some(())
            })?;
        }
        "rev7" => {
            ensure!(pinned_rev == 7, "rev7 mode requires pinned_rev == 7");
            let model = host.model.lock().expect("host model");
            ensure!(
                model.protocol.bundled_tickets_used == 0
                    && model.protocol.data_connect_requests >= 1,
                "rev 7 should omit data_tickets and use data.connect: bundled={}, data_connect={}",
                model.protocol.bundled_tickets_used,
                model.protocol.data_connect_requests
            );
        }
        "bundled" => {
            ensure!(pinned_rev >= 8, "bundled mode requires pinned_rev >= 8");
            let model = host.model.lock().expect("host model");
            ensure!(
                model.protocol.bundled_tickets_used >= 1
                    && model.protocol.data_connect_requests == 0,
                "rev {pinned_rev} should use bundled tickets: bundled={}, data_connect={}",
                model.protocol.bundled_tickets_used,
                model.protocol.data_connect_requests
            );
        }
        "stop" => {
            ensure!(pinned_rev == 4, "stop mode requires pinned_rev == 4");
            {
                let model = host.model.lock().expect("host model");
                ensure!(
                    model.protocol.bundled_tickets_used == 0
                        && model.protocol.data_connect_requests >= 1,
                    "rev 4 should use data.connect"
                );
            }
            host.commands.send(HostCommand::StopServer { epoch })?;
            wait(&host, "rev 4 StopServer accepted", |m| {
                m.notice
                    .as_deref()
                    .is_some_and(|n| n.contains("Server stop accepted"))
                    .then_some(())
            })?;
        }
        other => bail!("unknown mode: {other}"),
    }

    let summary = {
        let m = host.model.lock().expect("host model");
        json!({
            "mode": mode,
            "api_revision": m.protocol.api_revision,
            "api_revisions": m.server_info.api_revisions,
            "bootstrap_requests": m.protocol.bootstrap_requests,
            "bundled_tickets_used": m.protocol.bundled_tickets_used,
            "data_connect_requests": m.protocol.data_connect_requests,
            "panes": pane_count(&m, &sid),
        })
    };
    drop(host);
    ensure!(
        wait_for_hosts(Duration::from_secs(3)),
        "host worker did not stop cleanly"
    );
    println!("{summary}");
    Ok(())
}
