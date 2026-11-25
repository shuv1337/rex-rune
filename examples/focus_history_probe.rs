//! Protocol-only live probe for server-side focus history (`focus_history.reverse`, API 19+)
//! driven by `scripts/focus-history-private.py`. Compiles with `--no-default-features` and
//! refuses any socket outside a private `rex-focus-history-*` fixture directory.
use anyhow::{Context, Result, bail, ensure};
use rex_rune::{
    connection::{
        FOCUS_HISTORY_EMPTY_NOTICE, FOCUS_HISTORY_MIN_API_REVISION,
        FOCUS_HISTORY_UNSUPPORTED_NOTICE, FocusHistoryResult, FocusHistoryScope, HostCommand,
        HostConnection, HostModel, wait_for_hosts,
    },
    layout::shell_layout,
    platform, transport,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn canonical_socket_path(path: &Path) -> Result<PathBuf> {
    if path.exists() || path.symlink_metadata().is_ok() {
        return std::fs::canonicalize(path)
            .with_context(|| format!("canonicalize socket path {}", path.display()));
    }
    let parent = path
        .parent()
        .context("socket path must have a parent directory")?;
    let name = path
        .file_name()
        .context("socket path must have a file name")?;
    Ok(std::fs::canonicalize(parent)
        .with_context(|| format!("canonicalize socket parent {}", parent.display()))?
        .join(name))
}

fn resolve_if_possible(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok().or_else(|| {
        path.parent()
            .and_then(|p| std::fs::canonicalize(p).ok())
            .and_then(|p| path.file_name().map(|n| p.join(n)))
    })
}

fn validate_private_socket(endpoint: &str) -> Result<PathBuf> {
    let default_endpoint = transport::local_endpoint();
    ensure!(
        endpoint != default_endpoint,
        "refusing default per-user Rex socket"
    );
    let raw = endpoint
        .strip_prefix("unix://")
        .context("private unix:// endpoint required")?;
    let path = Path::new(raw);
    ensure!(path.is_absolute(), "absolute unix:// socket path required");

    let canonical = canonical_socket_path(path)?;
    if let Some(default_raw) = default_endpoint.strip_prefix("unix://") {
        let default_path = Path::new(default_raw);
        ensure!(
            canonical != default_path
                && resolve_if_possible(default_path).as_deref() != Some(canonical.as_path()),
            "refusing symlink or path resolving to default local Rex socket"
        );
    }
    let default_socket = platform::local::default_socket();
    ensure!(
        path != default_socket.as_path()
            && canonical != default_socket.as_path()
            && resolve_if_possible(&default_socket).as_deref() != Some(canonical.as_path()),
        "refusing symlink or path resolving to default per-user Rex socket"
    );
    ensure!(
        !matches!(
            canonical.to_str(),
            Some("/run/rex/server.sock" | "/var/run/rex/server.sock")
        ) && canonical.file_name().is_some_and(|n| n == "rex.sock")
            && canonical
                .parent()
                .and_then(Path::file_name)
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rex-focus-history-")),
        "only a private rex-focus-history-*/rex.sock endpoint is accepted"
    );
    Ok(canonical)
}

fn emit(event: &str, data: Value) {
    println!("{}", json!({"event": event, "data": data}));
}

fn wait<T>(
    host: &HostConnection,
    timeout: Duration,
    what: &str,
    check: impl Fn(&HostModel) -> Option<T>,
) -> Result<T> {
    let deadline = Instant::now() + timeout;
    loop {
        {
            let model = host.model.lock().expect("host model");
            if let Some(value) = check(&model) {
                return Ok(value);
            }
            ensure!(
                Instant::now() < deadline,
                "timed out waiting for {what}: status={}, notice={:?}",
                model.status,
                model.notice
            );
        }
        let _ = host.wake.recv_timeout(Duration::from_millis(20));
    }
}

fn request(host: &HostConnection, method: &str, payload: Value) -> Result<()> {
    host.commands.send(HostCommand::Request {
        method: method.into(),
        payload,
    })?;
    Ok(())
}

fn reverse(
    host: &HostConnection,
    epoch: u64,
    id: &mut u64,
    scope: FocusHistoryScope,
    cursor: Option<u64>,
) -> Result<FocusHistoryResult> {
    *id += 1;
    let req_id = *id;
    host.commands.send(HostCommand::ReverseFocusHistory {
        epoch,
        id: req_id,
        scope,
        cursor,
        deadline: Instant::now() + Duration::from_secs(10),
    })?;
    wait(host, Duration::from_secs(8), "focus_history_result", |m| {
        m.focus_history_result
            .as_ref()
            .filter(|r| r.epoch == epoch && r.id == req_id)
            .cloned()
    })
}

fn focus_block(host: &HostConnection, session: &str, block: &str) -> Result<()> {
    host.commands.send(HostCommand::Select(session.into()))?;
    wait(host, Duration::from_secs(8), "select session", |m| {
        (m.selected.as_deref() == Some(session)).then_some(())
    })?;
    request(
        host,
        "session.focus_block",
        json!({"session_id": session, "block_id": block}),
    )?;
    wait(host, Duration::from_secs(8), "focused block", |m| {
        (m.selected.as_deref() == Some(session) && m.focused() == Some(block)).then_some(())
    })?;
    Ok(())
}

fn exercise(endpoint: &str, max_api_revision: u32) -> Result<()> {
    let host = HostConnection::spawn_with_max_revision(
        endpoint.to_owned(),
        None,
        Some(("fh-s1".into(), Some("/bin/sh".into()))),
        None,
        false,
        None,
        max_api_revision,
    );
    let (epoch, api_revision, s1, w1a, b1a) =
        wait(&host, Duration::from_secs(8), "session 1 ready", |m| {
            m.connected.then_some(())?;
            let sid = m.selected.clone()?;
            let wid = m.window()?.window_id.clone()?;
            let bid = m.focused()?.to_owned();
            Some((m.connection_epoch, m.protocol.api_revision, sid, wid, bid))
        })?;
    emit(
        "connected",
        json!({
            "api_revision": api_revision,
            "session_1": s1,
            "window_1a": w1a,
            "block_1a": b1a,
        }),
    );

    let mut req_id = 0u64;
    if api_revision < FOCUS_HISTORY_MIN_API_REVISION {
        let res = reverse(&host, epoch, &mut req_id, FocusHistoryScope::Any, None)?;
        ensure!(!res.reversed && res.cursor.is_none());
        {
            let m = host.model.lock().expect("host model");
            ensure!(m.connected, "host disconnected on API {api_revision}");
            ensure!(
                m.notice.as_deref() == Some(FOCUS_HISTORY_UNSUPPORTED_NOTICE),
                "unexpected notice: {:?}",
                m.notice
            );
        }
        // Server-acknowledged fence proving the worker stayed healthy and sent zero reverses.
        let fence_label = format!("fh-fence-{api_revision}");
        request(
            &host,
            "session.set_label",
            json!({"session_id": s1, "label": fence_label}),
        )?;
        wait(&host, Duration::from_secs(8), "below-min fence", |m| {
            m.views
                .get(&s1)
                .and_then(|v| v.label.as_deref())
                .filter(|l| *l == fence_label)
                .map(|_| ())
        })?;
        emit("below_min_revision", json!({"api_revision": api_revision}));
        drop(host);
        ensure!(
            wait_for_hosts(Duration::from_secs(3)),
            "worker did not stop"
        );
        return Ok(());
    }

    // Build topology: s1 has w1a (split: b1a, b1b) and w1b (b1c); s2 has w2 (b2).
    let layout = shell_layout(Some("/bin/sh"), None);
    request(
        &host,
        "session.new_split",
        json!({
            "session_id": s1,
            "anchor_id": b1a,
            "direction": "horizontal",
            "side": "after",
            "layout": layout,
            "focus": true,
        }),
    )?;
    let b1b = wait(&host, Duration::from_secs(8), "split b1b", |m| {
        m.focused().filter(|b| *b != b1a).map(str::to_owned)
    })?;

    request(
        &host,
        "session.new_window",
        json!({
            "session_id": s1,
            "layout": shell_layout(Some("/bin/sh"), None),
            "focus": true,
        }),
    )?;
    let (w1b, b1c) = wait(&host, Duration::from_secs(8), "tab w1b", |m| {
        let win = m.window()?;
        let wid = win.window_id.as_deref()?;
        if wid == w1a {
            return None;
        }
        Some((wid.to_owned(), win.focused_block_id.clone()?))
    })?;

    host.commands.send(HostCommand::CreateSession {
        label: Some("fh-s2".into()),
        shell: Some("/bin/sh".into()),
    })?;
    let (s2, w2, b2) = wait(&host, Duration::from_secs(8), "session 2", |m| {
        let sid = m.selected.as_deref().filter(|id| *id != s1)?;
        let win = m.window()?;
        Some((
            sid.to_owned(),
            win.window_id.clone()?,
            win.focused_block_id.clone()?,
        ))
    })?;
    emit(
        "topology",
        json!({
            "s1": s1,
            "w1a": w1a,
            "b1a": b1a,
            "b1b": b1b,
            "w1b": w1b,
            "b1c": b1c,
            "s2": s2,
            "w2": w2,
            "b2": b2,
        }),
    );

    // 1. Verify Select between already-attached sessions records focus on the server,
    //    and toggle without cursor bounces cleanly back and forth across sessions.
    focus_block(&host, &s1, &b1c)?;
    host.commands.send(HostCommand::Select(s2.clone()))?;
    wait(&host, Duration::from_secs(8), "select s2", |m| {
        (m.selected.as_deref() == Some(s2.as_str()) && m.focused() == Some(b2.as_str()))
            .then_some(())
    })?;
    host.commands.send(HostCommand::Select(s1.clone()))?;
    wait(&host, Duration::from_secs(8), "select s1", |m| {
        (m.selected.as_deref() == Some(s1.as_str()) && m.focused() == Some(b1c.as_str()))
            .then_some(())
    })?;

    let t1 = reverse(&host, epoch, &mut req_id, FocusHistoryScope::Any, None)?;
    ensure!(
        t1.reversed
            && t1.session_id.as_deref() == Some(s2.as_str())
            && t1.block_id.as_deref() == Some(b2.as_str()),
        "toggle 1 mismatch: {t1:?}"
    );
    ensure!(
        host.model.lock().expect("host model").selected.as_deref() == Some(s2.as_str()),
        "toggle 1 did not switch HostModel::selected to s2"
    );
    let t2 = reverse(&host, epoch, &mut req_id, FocusHistoryScope::Any, None)?;
    ensure!(
        t2.reversed
            && t2.session_id.as_deref() == Some(s1.as_str())
            && t2.block_id.as_deref() == Some(b1c.as_str()),
        "toggle 2 mismatch: {t2:?}"
    );
    ensure!(
        host.model.lock().expect("host model").selected.as_deref() == Some(s1.as_str()),
        "toggle 2 did not switch HostModel::selected back to s1"
    );
    emit(
        "toggle_verified",
        json!({"first_block": t1.block_id, "second_block": t2.block_id}),
    );

    // 2. Verify stepping with returned cursor walks through 3+ distinct blocks until reversed=false.
    focus_block(&host, &s2, &b2)?;
    focus_block(&host, &s1, &b1a)?;
    focus_block(&host, &s1, &b1b)?;
    focus_block(&host, &s1, &b1c)?;

    let mut visited = Vec::new();
    let mut cursor = None;
    for _ in 0..8 {
        let step = reverse(&host, epoch, &mut req_id, FocusHistoryScope::Any, cursor)?;
        if !step.reversed {
            ensure!(step.cursor.is_none());
            ensure!(
                host.model.lock().expect("host model").notice.as_deref()
                    == Some(FOCUS_HISTORY_EMPTY_NOTICE)
            );
            break;
        }
        let blk = step.block_id.clone().context("missing block_id")?;
        ensure!(
            !visited.contains(&blk),
            "cursor walk revisited block {blk} in {visited:?}"
        );
        visited.push(blk);
        cursor = step.cursor;
    }
    ensure!(
        visited == vec![b1b.clone(), b1a.clone(), b2.clone()],
        "unexpected cursor walk order: {visited:?}"
    );
    ensure!(
        host.model.lock().expect("host model").selected.as_deref() == Some(s2.as_str()),
        "cursor walk did not end with s2 selected"
    );
    emit("walk_verified", json!({"visited": visited}));

    // 3. Verify Pane, Tab, Session, and TabAnySession scopes land on exact expected targets.
    focus_block(&host, &s2, &b2)?;
    focus_block(&host, &s1, &b1a)?;
    focus_block(&host, &s1, &b1c)?;
    focus_block(&host, &s1, &b1b)?;

    let pane = reverse(&host, epoch, &mut req_id, FocusHistoryScope::Pane, None)?;
    ensure!(
        pane.reversed
            && pane.session_id.as_deref() == Some(s1.as_str())
            && pane.window_id.as_deref() == Some(w1a.as_str())
            && pane.block_id.as_deref() == Some(b1a.as_str()),
        "Pane scope mismatch: {pane:?}"
    );

    let tab = reverse(&host, epoch, &mut req_id, FocusHistoryScope::Tab, None)?;
    ensure!(
        tab.reversed
            && tab.session_id.as_deref() == Some(s1.as_str())
            && tab.window_id.as_deref() == Some(w1b.as_str())
            && tab.block_id.as_deref() == Some(b1c.as_str()),
        "Tab scope mismatch: {tab:?}"
    );

    let sess = reverse(&host, epoch, &mut req_id, FocusHistoryScope::Session, None)?;
    ensure!(
        sess.reversed
            && sess.session_id.as_deref() == Some(s2.as_str())
            && sess.window_id.as_deref() == Some(w2.as_str())
            && sess.block_id.as_deref() == Some(b2.as_str()),
        "Session scope mismatch: {sess:?}"
    );
    ensure!(
        host.model.lock().expect("host model").selected.as_deref() == Some(s2.as_str()),
        "Session scope did not switch HostModel::selected to s2"
    );

    let tab_any = reverse(
        &host,
        epoch,
        &mut req_id,
        FocusHistoryScope::TabAnySession,
        None,
    )?;
    ensure!(
        tab_any.reversed
            && tab_any.session_id.as_deref() == Some(s1.as_str())
            && tab_any.window_id.as_deref() == Some(w1b.as_str())
            && tab_any.block_id.as_deref() == Some(b1c.as_str()),
        "TabAnySession scope mismatch: {tab_any:?}"
    );
    ensure!(
        host.model.lock().expect("host model").selected.as_deref() == Some(s1.as_str()),
        "TabAnySession scope did not switch HostModel::selected back to s1"
    );

    emit(
        "scopes_verified",
        json!({
            "pane": pane.block_id,
            "tab": tab.window_id,
            "session": sess.session_id,
            "tab_any_session": tab_any.window_id,
        }),
    );

    // 4. Verify offscreen layout changes and background prefetch NEVER pollute focus history.
    // Create session C (s3) alongside A (s1) and B (s2).
    host.commands.send(HostCommand::CreateSession {
        label: Some("fh-s3".into()),
        shell: Some("/bin/sh".into()),
    })?;
    let (s3, b3) = wait(&host, Duration::from_secs(8), "session 3", |m| {
        let sid = m.selected.as_deref().filter(|id| *id != s1 && *id != s2)?;
        let bid = m.focused()?.to_owned();
        Some((sid.to_owned(), bid))
    })?;

    // Start a short-lived split pane in A (s1) that exits on its own in ~1.2s, generating
    // a server-originated block_closed + session_view_changed on A while our client is on C.
    focus_block(&host, &s1, &b1a)?;
    let short_layout = json!({
        "block": {
            "flavor": "com.superlogical.terminal.shell",
            "options": {"command": ["/bin/sh", "-c", "sleep 1.2"]}
        }
    });
    let rev_a_before_split = host
        .model
        .lock()
        .expect("host model")
        .views
        .get(&s1)
        .map(|v| v.revision)
        .unwrap_or(0);
    request(
        &host,
        "session.new_split",
        json!({
            "session_id": s1,
            "anchor_id": b1a,
            "direction": "horizontal",
            "side": "after",
            "layout": short_layout,
            "focus": true,
        }),
    )?;
    let rev_a_after_split = wait(&host, Duration::from_secs(8), "A split view rev", |m| {
        m.views
            .get(&s1)
            .map(|v| v.revision)
            .filter(|&r| r > rev_a_before_split)
    })?;

    // Visit A -> B -> C explicitly so C is current and B is previous.
    focus_block(&host, &s1, &b1a)?;
    focus_block(&host, &s2, &b2)?;
    focus_block(&host, &s3, &b3)?;

    // Wait while staying on C for A's short-lived pane to exit AND for this worker's
    // offscreen background refresh of A to finish (view revision advances).
    let rev_a_after_exit = wait(
        &host,
        Duration::from_secs(8),
        "offscreen A view refresh after pane exit",
        |m| {
            m.views
                .get(&s1)
                .map(|v| v.revision)
                .filter(|&r| r > rev_a_after_split)
        },
    )?;
    ensure!(
        host.model.lock().expect("host model").selected.as_deref() == Some(s3.as_str()),
        "client unexpectedly left C during offscreen A refresh"
    );

    // Also connect a second HostConnection selecting C and prefetching A and B in the
    // background, proving Prefetch/status_sweep on API 19+ never pollutes focus history.
    let observer = HostConnection::spawn_with_max_revision(
        endpoint.to_owned(),
        Some(s3.clone()),
        None,
        None,
        false,
        None,
        max_api_revision,
    );
    wait(&observer, Duration::from_secs(8), "observer on C", |m| {
        (m.connected && m.selected.as_deref() == Some(s3.as_str())).then_some(())
    })?;
    observer
        .commands
        .send(HostCommand::PrefetchViews(std::collections::HashSet::from(
            [s1.clone(), s2.clone()],
        )))?;
    wait(
        &observer,
        Duration::from_secs(8),
        "observer prefetched A and B",
        |m| (m.views.contains_key(&s1) && m.views.contains_key(&s2)).then_some(()),
    )?;
    drop(observer);

    // Now reverse from C (Any scope): MUST land on B (s2), NOT A (s1) and NOT C (s3)!
    let bg_any = reverse(&host, epoch, &mut req_id, FocusHistoryScope::Any, None)?;
    ensure!(
        bg_any.reversed && bg_any.session_id.as_deref() == Some(s2.as_str()),
        "offscreen A refresh/prefetch polluted Any reverse: expected B ({s2}), got {bg_any:?}"
    );
    // Restore C and check Session scope too.
    host.commands.send(HostCommand::Select(s3.clone()))?;
    wait(&host, Duration::from_secs(8), "reselect C", |m| {
        (m.selected.as_deref() == Some(s3.as_str())).then_some(())
    })?;
    let bg_sess = reverse(&host, epoch, &mut req_id, FocusHistoryScope::Session, None)?;
    ensure!(
        bg_sess.reversed && bg_sess.session_id.as_deref() == Some(s2.as_str()),
        "offscreen A refresh/prefetch polluted Session reverse: expected B ({s2}), got {bg_sess:?}"
    );
    emit(
        "offscreen_verified",
        json!({
            "s1": s1,
            "s2": s2,
            "s3": s3,
            "rev_a_after_split": rev_a_after_split,
            "rev_a_after_exit": rev_a_after_exit,
            "any_session": bg_any.session_id,
            "session_scope_session": bg_sess.session_id,
        }),
    );

    drop(host);
    ensure!(
        wait_for_hosts(Duration::from_secs(3)),
        "worker did not stop"
    );
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut endpoint: Option<String> = None;
    let mut max_api_revision = transport::API_REVISION;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--max-api-revision" => {
                let raw = args.next().context("--max-api-revision requires a value")?;
                max_api_revision = raw.parse::<u32>()?;
            }
            other if !other.starts_with("--") && endpoint.is_none() => {
                endpoint = Some(other.to_owned());
            }
            other => bail!("unknown argument: {other}"),
        }
    }

    let endpoint = endpoint.context("private unix:// endpoint required")?;
    validate_private_socket(&endpoint)?;
    exercise(&endpoint, max_api_revision)
}
