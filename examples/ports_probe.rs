//! Protocol-only live probe for terminal listening ports (API 15-16) driven by
//! `scripts/ports-private.py`. Compiles with `--no-default-features` and refuses any socket
//! outside a private `rex-ports-*` fixture directory.
use anyhow::{Context, Result, bail, ensure};
use rex_rune::{
    connection::{HostCommand, HostConnection, HostModel, wait_for_hosts},
    platform,
    ports::{PORTS_EVENTS_MIN_API_REVISION, PORTS_MIN_API_REVISION, PortSnapshot, PortsStatus},
    transport,
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

fn posix_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
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
                .is_some_and(|n| n.starts_with("rex-ports-")),
        "only a private rex-ports-*/rex.sock endpoint is accepted"
    );
    Ok(canonical)
}

fn emit(event: &str, data: Value) {
    println!("{}", json!({"event": event, "data": data}));
}

fn snapshot_json(snap: &PortSnapshot) -> Value {
    let status = match snap.status {
        PortsStatus::Ok => "ok",
        PortsStatus::Partial => "partial",
        PortsStatus::Unsupported => "unsupported",
    };
    let ports: Vec<Value> = snap
        .ports
        .iter()
        .map(|entry| {
            let label = snap.process(entry.pid).map(|p| p.label.as_str());
            json!({
                "port": entry.port,
                "address": entry.address.canonical(),
                "pid": entry.pid,
                "first_seen_at": entry.first_seen_at,
                "label": label,
            })
        })
        .collect();
    json!({
        "status": status,
        "ports": ports,
        "truncated": snap.truncated,
        "capped": snap.capped,
        "complete": snap.is_complete(),
    })
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
        let _ = host.wake.recv_timeout(Duration::from_millis(25));
    }
}

fn dump_block_creators(endpoint: &str, max_api_revision: u32) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut client =
            transport::control_with_max_revision(endpoint, None, "ports-probe", max_api_revision)
                .await?;
        let api_revision = client.welcome().api_revision;
        let reply = client.call_raw("block_creator.list", json!({})).await?;
        emit(
            "block_creators",
            json!({
                "api_revision": api_revision,
                "payload": reply,
            }),
        );
        let _ = client.close().await;
        Ok(())
    })
}

fn send_write(host: &HostConnection, session: &str, block: &str, data: &str) -> Result<()> {
    host.commands.send(HostCommand::Request {
        method: "com.superlogical.terminal.write".into(),
        payload: json!({
            "session_id": session,
            "block_id": block,
            "args": {"data": data}
        }),
    })?;
    Ok(())
}

fn wait_port_file(path: &Path) -> Result<(u16, u32)> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            let mut parts = text.split_whitespace();
            if let (Some(p), Some(pid)) = (parts.next(), parts.next())
                && let (Ok(port), Ok(pid)) = (p.parse::<u16>(), pid.parse::<u32>())
            {
                return Ok((port, pid));
            }
        }
        ensure!(Instant::now() < deadline, "listener script did not bind");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn exercise_live(
    endpoint: &str,
    max_api_revision: u32,
    script: &Path,
    port_file: &Path,
    stop_file: &Path,
) -> Result<()> {
    let host = HostConnection::spawn_with_max_revision(
        endpoint.to_owned(),
        None,
        None,
        None,
        false,
        None,
        max_api_revision,
    );
    let (epoch, api_revision) = wait(&host, Duration::from_secs(8), "host connected", |m| {
        m.connected
            .then_some((m.connection_epoch, m.protocol.api_revision))
    })?;

    // Create a session whose shell pane stays open after exit so we can observe exit status.
    let session_label = format!("ports-live-rev-{max_api_revision}");
    host.commands.send(HostCommand::Request {
        method: "session.create".into(),
        payload: json!({
            "label": session_label,
            "initial_windows": [{
                "layout": {
                    "type": "block",
                    "block": {
                        "flavor": "com.superlogical.terminal.shell",
                        "options": {
                            "command": ["/bin/sh"],
                            "exit": {"on_completion": false}
                        }
                    }
                }
            }]
        }),
    })?;
    let sid = wait(&host, Duration::from_secs(8), "created session", |m| {
        m.sessions
            .iter()
            .find(|s| s.label == session_label)
            .map(|s| s.session_id.clone())
    })?;
    host.commands.send(HostCommand::Select(sid.clone()))?;
    let bid = wait(
        &host,
        Duration::from_secs(8),
        "attached terminal block",
        |m| {
            (m.selected.as_deref() == Some(sid.as_str()))
                .then(|| m.focused().map(str::to_owned))
                .flatten()
        },
    )?;

    emit(
        "connected",
        json!({
            "api_revision": api_revision,
            "session_id": sid,
            "block_id": bid,
        }),
    );

    // WatchPorts on API < 15 must send zero ports requests and keep ports/ports_reading empty.
    host.commands.send(HostCommand::WatchPorts {
        epoch,
        session: Some(sid.clone()),
    })?;
    if api_revision < PORTS_MIN_API_REVISION {
        host.commands.send(HostCommand::ReadPorts {
            epoch,
            session: sid.clone(),
        })?;
        // Flush command queue with a server-acknowledged session.set_label fence before
        // inspecting state so both WatchPorts and ReadPorts have been processed.
        let fence_label = format!("{session_label}-fenced");
        host.commands.send(HostCommand::Request {
            method: "session.set_label".into(),
            payload: json!({
                "session_id": sid,
                "label": fence_label,
            }),
        })?;
        wait(
            &host,
            Duration::from_secs(8),
            "API < 15 command fence",
            |m| {
                m.views
                    .get(&sid)
                    .and_then(|v| v.label.as_deref())
                    .filter(|l| *l == fence_label)
                    .map(|_| ())
            },
        )?;
        let model = host.model.lock().expect("host model");
        ensure!(model.connected, "host disconnected on API {api_revision}");
        ensure!(
            model.ports.is_empty()
                && model.ports_reading.is_empty()
                && model.ports_read_completions == 0,
            "unexpected ports state on API {api_revision}"
        );
        ensure!(
            model.notice.is_none(),
            "unexpected notice on API {api_revision}: {:?}",
            model.notice
        );
        emit("below_min_revision", json!({"api_revision": api_revision}));
        drop(model);
        drop(host);
        ensure!(
            wait_for_hosts(Duration::from_secs(3)),
            "worker did not stop"
        );
        return Ok(());
    }

    // Initial WatchPorts read on API 15+ completes with ok + empty ports.
    let init = wait(
        &host,
        Duration::from_secs(8),
        "initial WatchPorts read",
        |m| {
            (!m.ports_reading.contains(&bid))
                .then(|| m.ports.get(&bid).cloned())
                .flatten()
        },
    )?;
    emit("initial_read", snapshot_json(&init));
    ensure!(init.status == PortsStatus::Ok && init.ports.is_empty());

    // Start listener script inside the terminal block.
    send_write(
        &host,
        &sid,
        &bid,
        &format!("python3 {}\n", posix_quote(script)),
    )?;
    let (bound_port, listener_pid) = wait_port_file(port_file)?;
    emit(
        "listener_started",
        json!({"port": bound_port, "pid": listener_pid}),
    );

    if api_revision >= PORTS_EVENTS_MIN_API_REVISION {
        // On API 16+, passive ports_changed delivers the listening port without calling ReadPorts.
        let opened = wait(
            &host,
            Duration::from_secs(8),
            "ports_changed open event",
            |m| {
                m.ports
                    .get(&bid)
                    .filter(|snap| snap.ports.iter().any(|p| p.port == bound_port))
                    .cloned()
            },
        )?;
        emit("port_opened_event", snapshot_json(&opened));
        // Also exercise explicit ReadPorts while open, fencing on an actual completed reply.
        let before_reads = host
            .model
            .lock()
            .expect("host model")
            .ports_read_completions;
        host.commands.send(HostCommand::ReadPorts {
            epoch,
            session: sid.clone(),
        })?;
        let read_open = wait(&host, Duration::from_secs(8), "ReadPorts while open", |m| {
            (m.ports_read_completions > before_reads && !m.ports_reading.contains(&bid))
                .then(|| m.ports.get(&bid).cloned())
                .flatten()
                .filter(|snap| snap.ports.iter().any(|p| p.port == bound_port))
        })?;
        emit("port_opened_read", snapshot_json(&read_open));
    } else {
        // On pinned API 15, ports_changed is gated off by client; explicit ReadPorts returns the port.
        let deadline = Instant::now() + Duration::from_secs(8);
        let read_open = loop {
            let before_reads = host
                .model
                .lock()
                .expect("host model")
                .ports_read_completions;
            host.commands.send(HostCommand::ReadPorts {
                epoch,
                session: sid.clone(),
            })?;
            let snap = wait(&host, Duration::from_secs(5), "ReadPorts API 15", |m| {
                (m.ports_read_completions > before_reads && !m.ports_reading.contains(&bid))
                    .then(|| m.ports.get(&bid).cloned())
                    .flatten()
            })?;
            if snap.ports.iter().any(|p| p.port == bound_port) {
                break snap;
            }
            ensure!(
                Instant::now() < deadline,
                "API 15 ReadPorts never observed port"
            );
            std::thread::sleep(Duration::from_millis(200));
        };
        emit("port_opened_read", snapshot_json(&read_open));
    }

    // Stop the listener socket and nudge terminal activity so background scan runs promptly.
    std::fs::write(stop_file, "1")?;
    std::thread::sleep(Duration::from_millis(200));
    send_write(&host, &sid, &bid, "echo ports-closed\n")?;

    if api_revision >= PORTS_EVENTS_MIN_API_REVISION {
        let closed = wait(
            &host,
            Duration::from_secs(8),
            "ports_changed close event",
            |m| {
                m.ports
                    .get(&bid)
                    .filter(|snap| snap.status == PortsStatus::Ok && snap.ports.is_empty())
                    .cloned()
            },
        )?;
        emit("port_closed_event", snapshot_json(&closed));

        // Start listener again and exit the shell directly so the server emits the final
        // `ports_changed` event with `status: "unsupported"` and empty `ports`.
        let _ = std::fs::remove_file(port_file);
        let _ = std::fs::remove_file(stop_file);
        send_write(
            &host,
            &sid,
            &bid,
            &format!("exec python3 {}\n", posix_quote(script)),
        )?;
        let (exit_port, _) = wait_port_file(port_file)?;
        wait(
            &host,
            Duration::from_secs(8),
            "re-opened listener before exit",
            |m| {
                m.ports
                    .get(&bid)
                    .filter(|snap| snap.ports.iter().any(|p| p.port == exit_port))
                    .map(|_| ())
            },
        )?;
        std::fs::write(stop_file, "1")?;
        let exited = wait(
            &host,
            Duration::from_secs(8),
            "final ports_changed unsupported event on exit",
            |m| {
                m.ports
                    .get(&bid)
                    .filter(|snap| snap.status == PortsStatus::Unsupported && snap.ports.is_empty())
                    .cloned()
            },
        )?;
        emit(
            "shell_exited",
            json!({
                "passive_event": true,
                "snapshot": snapshot_json(&exited),
            }),
        );
    } else {
        // On pinned API 15, verify passive ports_changed did NOT clear the port before ReadPorts,
        // then confirm explicit ReadPorts sees the closed port cleanly.
        let deadline = Instant::now() + Duration::from_secs(8);
        let cleared = loop {
            let before_reads = host
                .model
                .lock()
                .expect("host model")
                .ports_read_completions;
            host.commands.send(HostCommand::ReadPorts {
                epoch,
                session: sid.clone(),
            })?;
            let snap = wait(
                &host,
                Duration::from_secs(5),
                "API 15 ReadPorts after close",
                |m| {
                    (m.ports_read_completions > before_reads && !m.ports_reading.contains(&bid))
                        .then(|| m.ports.get(&bid).cloned())
                        .flatten()
                },
            )?;
            if snap.status == PortsStatus::Ok && snap.ports.is_empty() {
                break snap;
            }
            ensure!(
                Instant::now() < deadline,
                "API 15 ReadPorts did not observe closed port"
            );
            std::thread::sleep(Duration::from_millis(200));
        };
        emit("pinned_15_closed_read", snapshot_json(&cleared));
    }

    // Close watch cleanly.
    host.commands.send(HostCommand::WatchPorts {
        epoch,
        session: None,
    })?;
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
    let mut dump_creators = false;
    let mut script: Option<PathBuf> = None;
    let mut port_file: Option<PathBuf> = None;
    let mut stop_file: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--max-api-revision" => {
                let raw = args.next().context("--max-api-revision requires a value")?;
                max_api_revision = raw.parse::<u32>()?;
            }
            "--dump-block-creators" => dump_creators = true,
            "--script" => script = Some(PathBuf::from(args.next().context("--script path")?)),
            "--port-file" => {
                port_file = Some(PathBuf::from(args.next().context("--port-file path")?))
            }
            "--stop-file" => {
                stop_file = Some(PathBuf::from(args.next().context("--stop-file path")?))
            }
            other if !other.starts_with("--") && endpoint.is_none() => {
                endpoint = Some(other.to_owned());
            }
            other => bail!("unknown argument: {other}"),
        }
    }

    let endpoint = endpoint.context("private unix:// endpoint required")?;
    validate_private_socket(&endpoint)?;

    if dump_creators {
        dump_block_creators(&endpoint, max_api_revision)
    } else {
        let script = script.context("--script required for live exercise")?;
        let port_file = port_file.context("--port-file required for live exercise")?;
        let stop_file = stop_file.context("--stop-file required for live exercise")?;
        exercise_live(&endpoint, max_api_revision, &script, &port_file, &stop_file)
    }
}
