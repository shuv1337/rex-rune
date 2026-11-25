use super::*;
use crate::ports::{MAX_PORTS_PER_BLOCK, MAX_PORTS_PER_HOST, PortsStatus};
use std::collections::HashSet;

const B1: &str = "block:00000000-0000-0000-0000-000000000001";
const B2: &str = "block:00000000-0000-0000-0000-000000000002";
const B3: &str = "block:00000000-0000-0000-0000-000000000003";
const B4: &str = "block:00000000-0000-0000-0000-000000000004";
const B5: &str = "block:00000000-0000-0000-0000-000000000005";
const B_UNHELD: &str = "block:00000000-0000-0000-0000-000000000099";
const SESSION_OTHER: &str = "session:00000000-0000-0000-0000-000000000002";

fn view_with_blocks(session: &str, revision: u64, blocks: &[&str]) -> session::ViewResponse {
    view_with_label(session, "work", revision, blocks)
}

fn view_with_label(
    session: &str,
    label: &str,
    revision: u64,
    blocks: &[&str],
) -> session::ViewResponse {
    session::ViewResponse {
        session_id: Some(session.into()),
        label: Some(label.into()),
        revision,
        active_window_id: Some("w1".into()),
        windows: vec![session::Window {
            window_id: Some("w1".into()),
            focused_block_id: blocks.first().map(|b| (*b).into()),
            layers: vec![session::Layer {
                layer_id: Some("l1".into()),
                blocks: blocks
                    .iter()
                    .map(|b| session::LayerBlock {
                        block_id: Some((*b).into()),
                        creator_name: Some(activity::TERMINAL.into()),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

fn boot_with_blocks(blocks: &[&str], extra_sessions: &[&str]) -> session::BootstrapResponse {
    let mut sessions = vec![session::Session {
        session_id: SESSION.into(),
        label: "work".into(),
    }];
    for s in extra_sessions {
        sessions.push(session::Session {
            session_id: (*s).into(),
            label: "other".into(),
        });
    }
    session::BootstrapResponse {
        client_id: "client:test".into(),
        attach_all: session::AttachAllResponse { sessions },
        selected_session: Some(session::SelectedSession {
            session: session::Session {
                session_id: SESSION.into(),
                label: "work".into(),
            },
            view: view_with_blocks(SESSION, 1, blocks),
        }),
        ..Default::default()
    }
}

fn ports_payload(status: &str, entries: &[(u16, &str, u32, &str)]) -> Value {
    let ports: Vec<Value> = entries
        .iter()
        .map(|(port, addr, pid, _)| {
            json!({
                "port": port,
                "address": addr,
                "protocol": "tcp",
                "first_seen_at": "2026-11-24T12:00:00Z",
                "pid": pid
            })
        })
        .collect();
    let processes: Vec<Value> = entries
        .iter()
        .map(|(_, _, pid, name)| {
            json!({
                "pid": pid,
                "user_id": 1000,
                "name": name
            })
        })
        .collect();
    json!({
        "status": status,
        "ports": ports,
        "processes": processes
    })
}

fn ports_changed_event(seq: u64, session: &str, block: &str, body: Value) -> Value {
    json!({
        "type": "event",
        "seq": seq,
        "payload": {
            "type": "session_event",
            "payload": {
                "session_id": session,
                "event": {
                    "type": "block_event",
                    "payload": {
                        "block_id": block,
                        "creator_name": activity::TERMINAL,
                        "name": "ports_changed",
                        "payload": body
                    }
                }
            }
        }
    })
}

fn label_fence(seq: u64, label: &str) -> Value {
    json!({
        "type": "event",
        "seq": seq,
        "payload": {
            "type": "session_event",
            "payload": {
                "session_id": SESSION,
                "event": {
                    "type": "session_label_changed",
                    "payload": {"label": label}
                }
            }
        }
    })
}

async fn wait_model(host: &HostConnection, pred: impl Fn(&HostModel) -> bool) -> HostModel {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            {
                let m = host.model.lock().unwrap();
                if pred(&m) {
                    return m.clone();
                }
            }
            let _ = host.wake.recv_async().await;
        }
    })
    .await
    .unwrap()
}

/// Answer the `3 * block_count` synchronous initial `query_activity` RPCs
/// (`program_status`, `title`, `process`) sent immediately after bootstrap so the host loop
/// enters its main select loop without waiting on metadata timeouts.
async fn drain_initial_meta(ws: &mut WebSocketStream<TcpStream>, block_count: usize, seq: u64) {
    for _ in 0..(block_count * 3) {
        let req = recv(ws).await;
        assert!(
            matches!(
                req["method"].as_str(),
                Some(
                    "com.superlogical.terminal.program_status"
                        | "com.superlogical.terminal.title"
                        | "com.superlogical.terminal.process"
                )
            ),
            "expected initial metadata RPC, got {}",
            req["method"]
        );
        send(
            ws,
            json!({"type": "response", "id": req["id"], "seq": seq, "payload": {}}),
        )
        .await;
    }
}

/// Answer standard metadata queries (`program_status`, `title`, `process`) until another request
/// arrives or the socket closes.
async fn recv_skipping_meta(ws: &mut WebSocketStream<TcpStream>, seq: u64) -> Option<Value> {
    loop {
        let Some(Ok(frame)) = ws.next().await else {
            return None;
        };
        match frame {
            Message::Text(text) => {
                let req: Value = serde_json::from_str(&text).unwrap();
                match req["method"].as_str() {
                    Some(
                        "com.superlogical.terminal.program_status"
                        | "com.superlogical.terminal.title"
                        | "com.superlogical.terminal.process",
                    ) => {
                        send(
                            ws,
                            json!({"type": "response", "id": req["id"], "seq": seq, "payload": {}}),
                        )
                        .await;
                    }
                    _ => return Some(req),
                }
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => return None,
            other => panic!("unexpected frame: {other:?}"),
        }
    }
}

#[tokio::test]
async fn seq_combining_view_seq_exemption_and_activity_isolation() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (step1_tx, step1_rx) = tokio::sync::oneshot::channel::<()>();
    let (step2_tx, step2_rx) = tokio::sync::oneshot::channel::<()>();
    let (step3_tx, step3_rx) = tokio::sync::oneshot::channel::<()>();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 16).await;
        let boot_req = recv(&mut ws).await;
        assert_eq!(boot_req["method"], "session.bootstrap");
        // Bootstrap seq = 1; view_seq[SESSION] starts at 1.
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": boot_req["id"],
                "seq": 1,
                "payload": boot_with_blocks(&[B1], &[])
            }),
        )
        .await;
        drain_initial_meta(&mut ws, 1, 1).await;

        // Trigger a session.view refresh so view_seq[SESSION] jumps to 20.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 2,
                "payload": {
                    "type": "session_event",
                    "payload": {
                        "session_id": SESSION,
                        "event": {"type": "session_view_changed", "payload": {"revision": 2}}
                    }
                }
            }),
        )
        .await;
        let view_req = recv_skipping_meta(&mut ws, 2).await.unwrap();
        assert_eq!(view_req["method"], "session.view");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": view_req["id"],
                "seq": 20,
                "payload": view_with_blocks(SESSION, 2, &[B1])
            }),
        )
        .await;

        // 1. Passive ports_changed for unheld SESSION_OTHER/B2 is ignored (never stored),
        // while ports_changed for SESSION/B1 with seq = 5 (BELOW view_seq = 20!) MUST apply.
        send(
            &mut ws,
            ports_changed_event(
                4,
                SESSION_OTHER,
                B2,
                ports_payload("ok", &[(9999, "127.0.0.1", 99, "other")]),
            ),
        )
        .await;
        send(
            &mut ws,
            ports_changed_event(
                5,
                SESSION,
                B1,
                ports_payload("ok", &[(3000, "127.0.0.1", 10, "vite")]),
            ),
        )
        .await;
        step1_rx.await.unwrap();

        // 2. Client requests ReadPorts -> reply has seq = 5 (TIE with event seq 5).
        // Tie goes to reply! Port 3001 from reply MUST replace port 3000.
        let req = recv_skipping_meta(&mut ws, 5).await.unwrap();
        assert_eq!(req["method"], "com.superlogical.terminal.ports");
        assert_eq!(req["payload"]["block_id"], B1);
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req["id"],
                "seq": 5,
                "payload": ports_payload("ok", &[(3001, "127.0.0.1", 11, "reply-wins")])
            }),
        )
        .await;
        step2_rx.await.unwrap();

        // 3. Event with seq = 5 (equal to reply seq 5) and seq = 4 (older) MUST be ignored.
        send(
            &mut ws,
            ports_changed_event(
                5,
                SESSION,
                B1,
                ports_payload("ok", &[(4000, "127.0.0.1", 12, "stale-tie")]),
            ),
        )
        .await;
        send(
            &mut ws,
            ports_changed_event(
                4,
                SESSION,
                B1,
                ports_payload("ok", &[(4001, "127.0.0.1", 13, "stale-old")]),
            ),
        )
        .await;
        // Use seq = 21 (> view_seq = 20) for the label fence because session_label_changed is
        // filtered by view_seq, unlike ports_changed.
        send(&mut ws, label_fence(21, "after-stale")).await;
        step3_rx.await.unwrap();

        // 4. Strictly newer event (seq = 7 > 5) MUST win.
        send(
            &mut ws,
            ports_changed_event(
                7,
                SESSION,
                B1,
                ports_payload("ok", &[(5000, "::1", 14, "newer-event")]),
            ),
        )
        .await;
        done_rx.await.unwrap();
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    let m1 = wait_model(&host, |m| {
        m.connected && m.ports.get(B1).is_some_and(|s| s.ports.len() == 1)
    })
    .await;
    assert_eq!(m1.ports[B1].ports[0].port, 3000);
    assert!(!m1.ports.contains_key(B2));
    assert!(m1.activity.is_empty());
    assert!(m1.notices.is_empty());
    assert!(m1.bells.is_empty());
    assert!(m1.progress.is_empty());
    step1_tx.send(()).unwrap();

    host.commands
        .send(HostCommand::ReadPorts {
            epoch: m1.connection_epoch,
            session: SESSION.into(),
        })
        .unwrap();
    let m2 = wait_model(&host, |m| {
        m.ports
            .get(B1)
            .and_then(|s| s.ports.first())
            .is_some_and(|p| p.port == 3001)
    })
    .await;
    assert_eq!(m2.ports[B1].process(Some(11)).unwrap().label, "reply-wins");
    assert!(!m2.ports_reading.contains(B1));
    step2_tx.send(()).unwrap();

    let m3 = wait_model(&host, |m| {
        m.sessions.iter().any(|s| s.label == "after-stale")
    })
    .await;
    assert_eq!(m3.ports[B1].ports[0].port, 3001);
    step3_tx.send(()).unwrap();

    let m4 = wait_model(&host, |m| {
        m.ports
            .get(B1)
            .and_then(|s| s.ports.first())
            .is_some_and(|p| p.port == 5000)
    })
    .await;
    assert_eq!(m4.ports[B1].ports[0].address.canonical(), "::1");
    done_tx.send(()).unwrap();

    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn api_revision_15_ignores_ports_changed_but_allows_read_ports() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (step1_tx, step1_rx) = tokio::sync::oneshot::channel::<()>();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 15).await;
        let boot_req = recv(&mut ws).await;
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": boot_req["id"],
                "seq": 1,
                "payload": boot_with_blocks(&[B1], &[])
            }),
        )
        .await;
        drain_initial_meta(&mut ws, 1, 1).await;

        // Server sends ports_changed even though negotiated revision is 15.
        send(
            &mut ws,
            ports_changed_event(
                2,
                SESSION,
                B1,
                ports_payload("ok", &[(3000, "127.0.0.1", 10, "ignored-event")]),
            ),
        )
        .await;
        send(&mut ws, label_fence(3, "after-rev15-event")).await;
        step1_rx.await.unwrap();

        // Explicit ReadPorts MUST still work at API 15.
        let req = recv_skipping_meta(&mut ws, 3).await.unwrap();
        assert_eq!(req["method"], "com.superlogical.terminal.ports");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req["id"],
                "seq": 3,
                "payload": ports_payload("ok", &[(4567, "127.0.0.1", 22, "read-15")])
            }),
        )
        .await;
        done_rx.await.unwrap();
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    let m1 = wait_model(&host, |m| {
        m.connected && m.sessions.iter().any(|s| s.label == "after-rev15-event")
    })
    .await;
    assert_eq!(m1.protocol.api_revision, 15);
    assert!(
        m1.ports.is_empty(),
        "API 15 must drop passive ports_changed without storing"
    );
    assert!(m1.activity.is_empty());
    assert!(m1.notices.is_empty());
    step1_tx.send(()).unwrap();

    host.commands
        .send(HostCommand::ReadPorts {
            epoch: m1.connection_epoch,
            session: SESSION.into(),
        })
        .unwrap();
    let m2 = wait_model(&host, |m| {
        m.ports
            .get(B1)
            .and_then(|s| s.ports.first())
            .is_some_and(|p| p.port == 4567)
    })
    .await;
    assert_eq!(m2.ports[B1].process(Some(22)).unwrap().label, "read-15");
    done_tx.send(()).unwrap();

    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn ports_changed_never_marks_urgent_views_or_activity() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 15).await;
        let _ = ws.next().await;
    });

    let mut control = transport::control(&endpoint, None).await.unwrap();
    let mut model = HostModel {
        connected: true,
        sessions: vec![
            session::Session {
                session_id: SESSION.into(),
                label: "work".into(),
            },
            session::Session {
                session_id: SESSION_OTHER.into(),
                label: "other".into(),
            },
        ],
        ..Default::default()
    };
    model
        .views
        .insert(SESSION.into(), view_with_blocks(SESSION, 1, &[B1]));
    let (wake_tx, _wake_rx) = flume::bounded(1);
    let shared = Shared {
        model: Arc::new(Mutex::new(model)),
        wake: wake_tx,
    };
    let mut workers = HashMap::new();
    let mut state = ControlState::default();
    let mut sweep = status_sweep::Reader::default();
    let mut server_reads = server_info::Reader::default();
    let mut hosts_reads = hosts::Reader::default();

    let ev = rexrs::Event {
        seq: 2,
        name: "block_event".into(),
        session_id: Some(SESSION_OTHER.into()),
        payload: json!({
            "block_id": B2,
            "creator_name": activity::TERMINAL,
            "name": "ports_changed",
            "payload": ports_payload("ok", &[(3000, "127.0.0.1", 1, "bg")])
        }),
    };
    handle_event(
        ev,
        &mut control,
        &shared,
        &mut workers,
        &mut state,
        &mut sweep,
        None,
        &mut server_reads,
        &mut hosts_reads,
    )
    .await
    .unwrap();

    assert!(
        state.urgent_views.is_empty(),
        "ports_changed must never queue urgent_views"
    );
    {
        let m = shared.model.lock().unwrap();
        assert!(m.ports.is_empty());
        assert!(m.activity.is_empty());
        assert!(m.bells.is_empty());
        assert!(m.notices.is_empty());
    }
    let _ = control.close().await;
    server.await.unwrap();
}

#[tokio::test]
async fn lifecycle_cleanup_block_closed_undo_close_disconnect_and_remove_session() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (s1_tx, s1_rx) = tokio::sync::oneshot::channel::<()>();
    let (s2_tx, s2_rx) = tokio::sync::oneshot::channel::<()>();
    let (s3_tx, s3_rx) = tokio::sync::oneshot::channel::<()>();
    let (s4_tx, s4_rx) = tokio::sync::oneshot::channel::<()>();
    let (s5_tx, s5_rx) = tokio::sync::oneshot::channel::<()>();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        // First connection
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 20).await;
        let boot_req = recv(&mut ws).await;
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": boot_req["id"],
                "seq": 1,
                "payload": boot_with_blocks(&[B1, B2], &[])
            }),
        )
        .await;
        drain_initial_meta(&mut ws, 2, 1).await;

        // Populate ports on B1 and B2.
        send(
            &mut ws,
            ports_changed_event(
                2,
                SESSION,
                B1,
                ports_payload("ok", &[(3000, "127.0.0.1", 1, "app1")]),
            ),
        )
        .await;
        send(
            &mut ws,
            ports_changed_event(
                3,
                SESSION,
                B2,
                ports_payload("ok", &[(4000, "127.0.0.1", 2, "app2")]),
            ),
        )
        .await;
        s1_rx.await.unwrap();

        // 1. Undoable close removes B2 via session_view_changed WITHOUT block_closed.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 4,
                "payload": {
                    "type": "session_event",
                    "payload": {
                        "session_id": SESSION,
                        "event": {"type": "session_view_changed", "payload": {"revision": 2}}
                    }
                }
            }),
        )
        .await;
        let view_req = recv_skipping_meta(&mut ws, 4).await.unwrap();
        assert_eq!(view_req["method"], "session.view");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": view_req["id"],
                "seq": 4,
                "payload": view_with_blocks(SESSION, 2, &[B1])
            }),
        )
        .await;
        s2_rx.await.unwrap();

        // 2. Undo restore brings B2 back via revision 3; ports_seq for B2 was cleared so a fresh
        // event applies cleanly.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 5,
                "payload": {
                    "type": "session_event",
                    "payload": {
                        "session_id": SESSION,
                        "event": {"type": "session_view_changed", "payload": {"revision": 3}}
                    }
                }
            }),
        )
        .await;
        let view_req2 = recv_skipping_meta(&mut ws, 5).await.unwrap();
        assert_eq!(view_req2["method"], "session.view");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": view_req2["id"],
                "seq": 5,
                "payload": view_with_blocks(SESSION, 3, &[B1, B2])
            }),
        )
        .await;
        drain_initial_meta(&mut ws, 1, 5).await;
        send(
            &mut ws,
            ports_changed_event(
                6,
                SESSION,
                B2,
                ports_payload("ok", &[(4001, "127.0.0.1", 3, "restored")]),
            ),
        )
        .await;
        s3_rx.await.unwrap();

        // 3. block_closed removes B1 immediately.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 7,
                "payload": {
                    "type": "session_event",
                    "payload": {
                        "session_id": SESSION,
                        "event": {"type": "block_closed", "payload": {"block_id": B1}}
                    }
                }
            }),
        )
        .await;
        let view_req3 = recv_skipping_meta(&mut ws, 7).await.unwrap();
        assert_eq!(view_req3["method"], "session.view");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": view_req3["id"],
                "seq": 7,
                "payload": view_with_blocks(SESSION, 4, &[B2])
            }),
        )
        .await;
        s4_rx.await.unwrap();

        // 4. Disconnect must clear m.ports BEFORE reconnect!
        ws.close(None).await.unwrap();
        s5_rx.await.unwrap();

        let mut ws2 = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws2, 20).await;
        let boot_req2 = recv(&mut ws2).await;
        send(
            &mut ws2,
            json!({
                "type": "response",
                "id": boot_req2["id"],
                "seq": 1,
                "payload": boot_with_blocks(&[B2], &[])
            }),
        )
        .await;
        drain_initial_meta(&mut ws2, 1, 1).await;

        // 5. session_destroyed removes session and any ports on B2.
        send(
            &mut ws2,
            ports_changed_event(
                2,
                SESSION,
                B2,
                ports_payload("ok", &[(4002, "127.0.0.1", 4, "after-reconnect")]),
            ),
        )
        .await;
        done_rx.await.unwrap();
        send(
            &mut ws2,
            json!({
                "type": "event",
                "seq": 3,
                "payload": {"type": "session_destroyed", "payload": {"session_id": SESSION}}
            }),
        )
        .await;
        let _ = recv_skipping_meta(&mut ws2, 3).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    let m1 = wait_model(&host, |m| {
        m.ports.contains_key(B1) && m.ports.contains_key(B2)
    })
    .await;
    assert_eq!(m1.ports[B1].ports[0].port, 3000);
    assert_eq!(m1.ports[B2].ports[0].port, 4000);
    s1_tx.send(()).unwrap();

    // After undoable close, B2 is gone from m.ports.
    let m2 = wait_model(&host, |m| {
        m.views.get(SESSION).is_some_and(|v| v.revision == 2) && !m.ports.contains_key(B2)
    })
    .await;
    assert!(m2.ports.contains_key(B1));
    s2_tx.send(()).unwrap();

    // After undo restore, B2 gets fresh snapshot 4001.
    let m3 = wait_model(&host, |m| {
        m.ports
            .get(B2)
            .and_then(|s| s.ports.first())
            .is_some_and(|p| p.port == 4001)
    })
    .await;
    assert_eq!(m3.ports[B2].process(Some(3)).unwrap().label, "restored");
    s3_tx.send(()).unwrap();

    // After block_closed, B1 is removed from m.ports.
    let m4 = wait_model(&host, |m| {
        m.views.get(SESSION).is_some_and(|v| v.revision == 4) && !m.ports.contains_key(B1)
    })
    .await;
    assert!(m4.ports.contains_key(B2));
    let epoch1 = m4.connection_epoch;
    s4_tx.send(()).unwrap();

    // Wait for disconnect and assert m.ports AND m.ports_reading are ALREADY cleared before reconnect!
    let disconnected = wait_model(&host, |m| !m.connected).await;
    assert!(
        disconnected.ports.is_empty(),
        "m.ports must clear immediately on disconnect"
    );
    assert!(disconnected.ports_reading.is_empty());
    s5_tx.send(()).unwrap();
    host.commands.send(HostCommand::Reconnect).unwrap();

    // Reconnect loads epoch 2 with 4002.
    let m5 = wait_model(&host, |m| {
        m.connected
            && m.connection_epoch > epoch1
            && m.ports
                .get(B2)
                .and_then(|s| s.ports.first())
                .is_some_and(|p| p.port == 4002)
    })
    .await;
    assert_eq!(m5.ports.len(), 1);
    done_tx.send(()).unwrap();

    // session_destroyed clears B2 ports too.
    wait_model(&host, |m| m.sessions.is_empty() && m.ports.is_empty()).await;

    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn queued_read_pumps_when_in_flight_blocks_disappear_via_replace_view() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 16).await;
        let boot_req = recv(&mut ws).await;
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": boot_req["id"],
                "seq": 1,
                "payload": boot_with_blocks(&[B1, B2, B3, B4], &[])
            }),
        )
        .await;
        drain_initial_meta(&mut ws, 4, 1).await;

        // First 3 blocks (B1, B2, B3) go in flight while B4 sits in queue.
        let mut active = HashSet::new();
        for _ in 0..3 {
            let req = recv_skipping_meta(&mut ws, 1).await.unwrap();
            assert_eq!(req["method"], "com.superlogical.terminal.ports");
            active.insert(req["payload"]["block_id"].as_str().unwrap().to_owned());
        }
        assert_eq!(active.len(), 3);
        assert!(!active.contains(B4));

        // Do NOT answer B1/B2/B3. Instead remove all 3 via session_view_changed -> replace_view
        // while B4 survives! The cancelled slots MUST immediately pump B4.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 2,
                "payload": {
                    "type": "session_event",
                    "payload": {
                        "session_id": SESSION,
                        "event": {"type": "session_view_changed", "payload": {"revision": 2}}
                    }
                }
            }),
        )
        .await;
        let view_req = recv_skipping_meta(&mut ws, 2).await.unwrap();
        assert_eq!(view_req["method"], "session.view");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": view_req["id"],
                "seq": 2,
                "payload": view_with_blocks(SESSION, 2, &[B4])
            }),
        )
        .await;

        let b4_req = recv_skipping_meta(&mut ws, 2).await.unwrap();
        assert_eq!(b4_req["method"], "com.superlogical.terminal.ports");
        assert_eq!(b4_req["payload"]["block_id"], B4);
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": b4_req["id"],
                "seq": 2,
                "payload": ports_payload("ok", &[(8004, "127.0.0.1", 4, "survivor")])
            }),
        )
        .await;
        done_rx.await.unwrap();
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    let m0 = wait_model(&host, |m| m.connected).await;
    host.commands
        .send(HostCommand::WatchPorts {
            epoch: m0.connection_epoch,
            session: Some(SESSION.into()),
        })
        .unwrap();

    let m = wait_model(&host, |m| {
        m.ports
            .get(B4)
            .and_then(|s| s.ports.first())
            .is_some_and(|p| p.port == 8004)
            && m.ports_reading.is_empty()
    })
    .await;
    assert_eq!(m.ports.len(), 1);
    assert_eq!(m.ports[B4].process(Some(4)).unwrap().label, "survivor");
    done_tx.send(()).unwrap();

    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn event_dropped_ports_changed_watched_vs_closed_and_unrelated_names() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (s1_tx, s1_rx) = tokio::sync::oneshot::channel::<()>();
    let (s2_tx, s2_rx) = tokio::sync::oneshot::channel::<()>();
    let (s3_tx, s3_rx) = tokio::sync::oneshot::channel::<()>();
    let (s4_tx, s4_rx) = tokio::sync::oneshot::channel::<()>();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 18).await;
        let boot_req = recv(&mut ws).await;
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": boot_req["id"],
                "seq": 1,
                "payload": boot_with_blocks(&[B1], &[])
            }),
        )
        .await;
        drain_initial_meta(&mut ws, 1, 1).await;

        // Populate B1 via ports_changed.
        send(
            &mut ws,
            ports_changed_event(
                2,
                SESSION,
                B1,
                ports_payload("ok", &[(3000, "127.0.0.1", 10, "svc")]),
            ),
        )
        .await;
        s1_rx.await.unwrap();

        // 1. Unrelated specific block_event_name ("title_changed") must NOT drop ports.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 3,
                "payload": {
                    "type": "event_dropped",
                    "payload": {
                        "event_type": "block_event",
                        "session_id": SESSION,
                        "block_id": B1,
                        "block_event_name": "title_changed",
                        "size": 2000000
                    }
                }
            }),
        )
        .await;
        let view_req = recv_skipping_meta(&mut ws, 3).await.unwrap();
        assert_eq!(view_req["method"], "session.view");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": view_req["id"],
                "seq": 3,
                "payload": view_with_blocks(SESSION, 2, &[B1])
            }),
        )
        .await;
        drain_initial_meta(&mut ws, 1, 3).await;
        s2_rx.await.unwrap();

        // 2. Targeted ports_changed drop while NOT watched: drops snapshot, sends ZERO re-reads
        // and ZERO session.view refreshes.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 4,
                "payload": {
                    "type": "event_dropped",
                    "payload": {
                        "event_type": "block_event",
                        "session_id": SESSION,
                        "block_id": B1,
                        "block_event_name": "ports_changed",
                        "size": 2000000
                    }
                }
            }),
        )
        .await;
        send(&mut ws, label_fence(5, "dropped-closed")).await;
        s3_rx.await.unwrap();

        // 3. Client opens WatchPorts -> initial read arrives. Hold it, then deliver a
        // ports_changed drop at seq=10, answer the old read with stale seq=6 (must be ignored
        // because drop advanced ports_seq to 10!), then answer the re-read with seq=10!
        let first_read = recv_skipping_meta(&mut ws, 6).await.unwrap();
        assert_eq!(first_read["method"], "com.superlogical.terminal.ports");
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 10,
                "payload": {
                    "type": "event_dropped",
                    "payload": {
                        "event_type": "block_event",
                        "session_id": SESSION,
                        "block_id": B1,
                        "block_event_name": "ports_changed",
                        "size": 2000000
                    }
                }
            }),
        )
        .await;
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": first_read["id"],
                "seq": 6,
                "payload": ports_payload("ok", &[(1111, "127.0.0.1", 1, "stale")])
            }),
        )
        .await;
        let reread = recv_skipping_meta(&mut ws, 10).await.unwrap();
        assert_eq!(reread["method"], "com.superlogical.terminal.ports");
        assert_ne!(reread["id"], first_read["id"]);
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": reread["id"],
                "seq": 10,
                "payload": ports_payload("ok", &[(7777, "127.0.0.1", 7, "fresh")])
            }),
        )
        .await;
        s4_rx.await.unwrap();

        // 4. Generic block_event drop WITHOUT block_event_name while watched: drops snapshot AND
        // re-reads B1 (alongside normal session.view refresh).
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 12,
                "payload": {
                    "type": "event_dropped",
                    "payload": {
                        "event_type": "block_event",
                        "session_id": SESSION,
                        "block_id": B1,
                        "size": 2000000
                    }
                }
            }),
        )
        .await;
        let mut saw_ports = false;
        let mut saw_view = false;
        while !(saw_ports && saw_view) {
            let req = recv_skipping_meta(&mut ws, 12).await.unwrap();
            if req["method"] == "com.superlogical.terminal.ports" {
                saw_ports = true;
                send(
                    &mut ws,
                    json!({
                        "type": "response",
                        "id": req["id"],
                        "seq": 12,
                        "payload": ports_payload("ok", &[(8888, "127.0.0.1", 8, "after-generic")])
                    }),
                )
                .await;
            } else if req["method"] == "session.view" {
                saw_view = true;
                send(
                    &mut ws,
                    json!({
                        "type": "response",
                        "id": req["id"],
                        "seq": 12,
                        "payload": view_with_blocks(SESSION, 3, &[B1])
                    }),
                )
                .await;
            } else {
                panic!("unexpected method: {}", req["method"]);
            }
        }
        drain_initial_meta(&mut ws, 1, 12).await;
        done_rx.await.unwrap();
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    let m1 = wait_model(&host, |m| m.ports.contains_key(B1)).await;
    assert_eq!(m1.ports[B1].ports[0].port, 3000);
    s1_tx.send(()).unwrap();

    // Unrelated title_changed drop kept port 3000 intact.
    let m2 = wait_model(&host, |m| {
        m.views.get(SESSION).is_some_and(|v| v.revision == 2)
    })
    .await;
    assert_eq!(m2.ports[B1].ports[0].port, 3000);
    s2_tx.send(()).unwrap();

    // Targeted ports_changed drop while closed cleared B1 without re-reading.
    let m3 = wait_model(&host, |m| {
        m.sessions.iter().any(|s| s.label == "dropped-closed")
    })
    .await;
    assert!(!m3.ports.contains_key(B1));
    assert!(m3.ports_reading.is_empty());
    s3_tx.send(()).unwrap();

    // Open WatchPorts -> drop during in-flight -> stale seq=6 ignored, fresh seq=10 wins.
    host.commands
        .send(HostCommand::WatchPorts {
            epoch: m3.connection_epoch,
            session: Some(SESSION.into()),
        })
        .unwrap();
    let m4 = wait_model(&host, |m| {
        m.ports
            .get(B1)
            .and_then(|s| s.ports.first())
            .is_some_and(|p| p.port == 7777)
    })
    .await;
    assert_eq!(m4.ports[B1].process(Some(7)).unwrap().label, "fresh");
    s4_tx.send(()).unwrap();

    // Generic block_event drop without name re-reads watched session and lands port 8888.
    let m5 = wait_model(&host, |m| {
        m.ports
            .get(B1)
            .and_then(|s| s.ports.first())
            .is_some_and(|p| p.port == 8888)
    })
    .await;
    assert_eq!(
        m5.ports[B1].process(Some(8)).unwrap().label,
        "after-generic"
    );
    done_tx.send(()).unwrap();

    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn event_dropped_guards_unheld_mismatched_session_and_newer_watermark() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 18).await;
        let _ = ws.next().await;
    });

    let mut control = transport::control(&endpoint, None).await.unwrap();
    let mut model = HostModel {
        connected: true,
        selected: Some(SESSION.into()),
        ..Default::default()
    };
    model
        .views
        .insert(SESSION.into(), view_with_blocks(SESSION, 1, &[B1]));
    model.views.insert(
        SESSION_OTHER.into(),
        view_with_blocks(SESSION_OTHER, 1, &[B2]),
    );
    let (wake_tx, _wake_rx) = flume::bounded(1);
    let shared = Shared {
        model: Arc::new(Mutex::new(model)),
        wake: wake_tx,
    };
    let mut state = ControlState::default();

    // Seed B1 with a snapshot at seq = 10.
    ports::apply_event(
        &shared,
        &mut state,
        SESSION,
        B1,
        10,
        &ports_payload("ok", &[(3000, "127.0.0.1", 10, "kept")]),
    );
    assert_eq!(state.ports_seq.get(B1), Some(&10));

    // (a) Targeted event_dropped for an unheld block must NOT insert into ports_seq.
    ports::handle_dropped(
        &mut control,
        &shared,
        &mut state,
        Some(SESSION),
        Some(B_UNHELD),
        20,
    )
    .await
    .unwrap();
    assert!(!state.ports_seq.contains_key(B_UNHELD));
    ports::handle_dropped(&mut control, &shared, &mut state, None, Some(B_UNHELD), 20)
        .await
        .unwrap();
    assert!(!state.ports_seq.contains_key(B_UNHELD));

    // (b) Targeted event_dropped with mismatched session_id (SESSION_OTHER naming B1) must NOT
    // invalidate SESSION's B1 or advance its watermark.
    ports::handle_dropped(
        &mut control,
        &shared,
        &mut state,
        Some(SESSION_OTHER),
        Some(B1),
        50,
    )
    .await
    .unwrap();
    assert_eq!(state.ports_seq.get(B1), Some(&10));
    assert_eq!(shared.model.lock().unwrap().ports[B1].ports[0].port, 3000);

    // (c) Targeted event_dropped with drop_seq <= existing ports_seq (9 and 10 <= 10) must NOT
    // discard the newer stored snapshot.
    for stale_drop in [9, 10] {
        ports::handle_dropped(
            &mut control,
            &shared,
            &mut state,
            Some(SESSION),
            Some(B1),
            stale_drop,
        )
        .await
        .unwrap();
        assert_eq!(state.ports_seq.get(B1), Some(&10));
        assert_eq!(shared.model.lock().unwrap().ports[B1].ports[0].port, 3000);
    }

    // Sanity: strictly newer drop_seq (11 > 10) for matching held (SESSION, B1) DOES invalidate.
    ports::handle_dropped(
        &mut control,
        &shared,
        &mut state,
        Some(SESSION),
        Some(B1),
        11,
    )
    .await
    .unwrap();
    assert_eq!(state.ports_seq.get(B1), Some(&11));
    assert!(!shared.model.lock().unwrap().ports.contains_key(B1));

    let _ = control.close().await;
    server.await.unwrap();
}

#[tokio::test]
async fn foreground_session_eligibility_rejects_background_views_and_cancels_on_switch() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (step1_tx, step1_rx) = tokio::sync::oneshot::channel::<()>();
    let (step2_tx, step2_rx) = tokio::sync::oneshot::channel::<()>();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 16).await;
        let boot_req = recv(&mut ws).await;
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": boot_req["id"],
                "seq": 1,
                "payload": boot_with_blocks(&[B1, B2, B3, B4], &[SESSION_OTHER])
            }),
        )
        .await;
        drain_initial_meta(&mut ws, 4, 1).await;

        // status_sweep fetches SESSION_OTHER's view into m.views + m.background_views.
        let bg_view = recv_skipping_meta(&mut ws, 1).await.unwrap();
        assert_eq!(bg_view["method"], "session.view");
        assert_eq!(bg_view["payload"]["session_id"], SESSION_OTHER);
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": bg_view["id"],
                "seq": 1,
                "payload": view_with_blocks(SESSION_OTHER, 1, &[B5])
            }),
        )
        .await;
        step1_rx.await.unwrap();

        // (a) Client sends WatchPorts(SESSION_OTHER) + ReadPorts(SESSION_OTHER) + RefreshSessions
        // fence. ZERO com.superlogical.terminal.ports requests may arrive before session.attach_all!
        let fence_list = recv_skipping_meta(&mut ws, 1).await.unwrap();
        assert_eq!(
            fence_list["method"], "session.attach_all",
            "background prefetched session must never trigger ports reads"
        );
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": fence_list["id"],
                "seq": 2,
                "payload": {
                    "sessions": [
                        {"session_id": SESSION, "label": "bg-rejected"},
                        {"session_id": SESSION_OTHER, "label": "other"}
                    ]
                }
            }),
        )
        .await;
        let fence_view = recv_skipping_meta(&mut ws, 2).await.unwrap();
        assert_eq!(fence_view["method"], "session.view");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": fence_view["id"],
                "seq": 2,
                "payload": view_with_label(SESSION, "bg-rejected", 2, &[B1, B2, B3, B4])
            }),
        )
        .await;
        step2_rx.await.unwrap();

        // (b) Client starts WatchPorts on foreground SESSION (3 in flight, B4 queued), then
        // immediately switches to SESSION_OTHER via Select(SESSION_OTHER).
        // Do NOT answer B1..B3; switching sessions MUST cancel them and clear ports_reading.
        for _ in 0..3 {
            let req = recv_skipping_meta(&mut ws, 2).await.unwrap();
            assert_eq!(req["method"], "com.superlogical.terminal.ports");
            assert_eq!(req["payload"]["session_id"], SESSION);
        }
        let attach_other = recv_skipping_meta(&mut ws, 2).await.unwrap();
        assert_eq!(attach_other["method"], "session.attach");
        assert_eq!(attach_other["payload"]["session_id"], SESSION_OTHER);
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": attach_other["id"],
                "seq": 3,
                "payload": {
                    "client_id": "client:test",
                    "view": view_with_blocks(SESSION_OTHER, 1, &[B5])
                }
            }),
        )
        .await;
        // Answer metadata for SESSION_OTHER's B5, then expect our deterministic RefreshSessions
        // fence (proving queued B4 for SESSION was cancelled and never sent!).
        let post_switch = recv_skipping_meta(&mut ws, 3).await.unwrap();
        assert_eq!(
            post_switch["method"], "session.attach_all",
            "unexpected request after switching away from watched session"
        );
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": post_switch["id"],
                "seq": 4,
                "payload": {
                    "sessions": [
                        {"session_id": SESSION, "label": "bg-rejected"},
                        {"session_id": SESSION_OTHER, "label": "switched-clean"}
                    ]
                }
            }),
        )
        .await;
        let post_view = recv_skipping_meta(&mut ws, 4).await.unwrap();
        assert_eq!(post_view["method"], "session.view");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": post_view["id"],
                "seq": 4,
                "payload": view_with_label(SESSION_OTHER, "switched-clean", 2, &[B5])
            }),
        )
        .await;
        done_rx.await.unwrap();
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    let m1 = wait_model(&host, |m| {
        m.connected
            && m.views.contains_key(SESSION_OTHER)
            && m.background_views.contains(SESSION_OTHER)
    })
    .await;
    step1_tx.send(()).unwrap();

    // Attempt to WatchPorts and ReadPorts on background prefetched SESSION_OTHER + fence.
    host.commands
        .send(HostCommand::WatchPorts {
            epoch: m1.connection_epoch,
            session: Some(SESSION_OTHER.into()),
        })
        .unwrap();
    host.commands
        .send(HostCommand::ReadPorts {
            epoch: m1.connection_epoch,
            session: SESSION_OTHER.into(),
        })
        .unwrap();
    host.commands.send(HostCommand::RefreshSessions).unwrap();

    let m2 = wait_model(&host, |m| {
        m.views.get(SESSION).and_then(|v| v.label.as_deref()) == Some("bg-rejected")
    })
    .await;
    assert!(m2.ports.is_empty());
    assert!(m2.ports_reading.is_empty());
    step2_tx.send(()).unwrap();

    // Start WatchPorts on foreground SESSION (4 blocks: 3 in flight, 1 queued), then switch away.
    host.commands
        .send(HostCommand::WatchPorts {
            epoch: m2.connection_epoch,
            session: Some(SESSION.into()),
        })
        .unwrap();
    wait_model(&host, |m| m.ports_reading.len() == 4).await;
    host.commands
        .send(HostCommand::Select(SESSION_OTHER.into()))
        .unwrap();

    let m3 = wait_model(&host, |m| {
        m.selected.as_deref() == Some(SESSION_OTHER) && m.ports_reading.is_empty()
    })
    .await;
    assert!(m3.ports_reading.is_empty());
    host.commands.send(HostCommand::RefreshSessions).unwrap();
    let m4 = wait_model(&host, |m| {
        m.views.get(SESSION_OTHER).and_then(|v| v.label.as_deref()) == Some("switched-clean")
    })
    .await;
    assert!(m4.ports.is_empty());
    assert!(m4.ports_reading.is_empty());
    done_tx.send(()).unwrap();

    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn api_revision_14_sends_zero_ports_requests() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 14).await;
        let boot_req = recv(&mut ws).await;
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": boot_req["id"],
                "seq": 1,
                "payload": boot_with_blocks(&[B1], &[])
            }),
        )
        .await;
        drain_initial_meta(&mut ws, 1, 1).await;

        // Deterministic command fence: after WatchPorts + ReadPorts, client sends RefreshSessions.
        // The session.view reply ALSO adds a new block B2 to prove that WatchPorts on API 14 never
        // registered `watched` and replace_view never inserts B2 into ports_reading!
        let fence_list = recv_skipping_meta(&mut ws, 1).await.unwrap();
        assert_eq!(
            fence_list["method"], "session.attach_all",
            "API 14 must never send com.superlogical.terminal.ports"
        );
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": fence_list["id"],
                "seq": 2,
                "payload": {"sessions": [{"session_id": SESSION, "label": "api14-fenced"}]}
            }),
        )
        .await;
        let fence_view = recv_skipping_meta(&mut ws, 2).await.unwrap();
        assert_eq!(fence_view["method"], "session.view");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": fence_view["id"],
                "seq": 2,
                "payload": view_with_label(SESSION, "api14-fenced", 2, &[B1, B2])
            }),
        )
        .await;
        // Answer metadata for newly added B2; assert no com.superlogical.terminal.ports arrives.
        drain_initial_meta(&mut ws, 1, 2).await;
        done_rx.await.unwrap();
        assert!(recv_skipping_meta(&mut ws, 2).await.is_none());
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    let m = wait_model(&host, |m| m.connected && m.protocol.api_revision == 14).await;
    host.commands
        .send(HostCommand::WatchPorts {
            epoch: m.connection_epoch,
            session: Some(SESSION.into()),
        })
        .unwrap();
    host.commands
        .send(HostCommand::ReadPorts {
            epoch: m.connection_epoch,
            session: SESSION.into(),
        })
        .unwrap();
    host.commands.send(HostCommand::RefreshSessions).unwrap();

    let fenced = wait_model(&host, |m| {
        m.views.get(SESSION).and_then(|v| v.label.as_deref()) == Some("api14-fenced")
            && m.views
                .get(SESSION)
                .is_some_and(|v| view_blocks(v).any(|b| b == B2))
    })
    .await;
    assert!(fenced.ports.is_empty());
    assert!(
        fenced.ports_reading.is_empty(),
        "API 14 WatchPorts + replace_view adding B2 must never populate ports_reading"
    );
    done_tx.send(()).unwrap();
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn concurrency_cap_three_in_flight_and_unsupported_error_codes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 15).await;
        let boot_req = recv(&mut ws).await;
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": boot_req["id"],
                "seq": 1,
                "payload": boot_with_blocks(&[B1, B2, B3, B4, B5], &[])
            }),
        )
        .await;
        drain_initial_meta(&mut ws, 5, 1).await;

        // First 3 ports requests arrive concurrently (cap = 3).
        let mut first_three = Vec::new();
        for _ in 0..3 {
            let req = recv_skipping_meta(&mut ws, 1).await.unwrap();
            assert_eq!(req["method"], "com.superlogical.terminal.ports");
            first_three.push(req);
        }
        // Deterministic fence: RefreshSessions was queued immediately after WatchPorts, so the
        // 4th request MUST be session.attach_all (proving no 4th ports read started while 3 are
        // in flight).
        let fence_list = recv_skipping_meta(&mut ws, 1).await.unwrap();
        assert_eq!(
            fence_list["method"], "session.attach_all",
            "more than 3 port reads in flight at once"
        );
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": fence_list["id"],
                "seq": 2,
                "payload": {"sessions": [{"session_id": SESSION, "label": "work"}]}
            }),
        )
        .await;
        let fence_view = recv_skipping_meta(&mut ws, 2).await.unwrap();
        assert_eq!(fence_view["method"], "session.view");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": fence_view["id"],
                "seq": 2,
                "payload": view_with_blocks(SESSION, 1, &[B1, B2, B3, B4, B5])
            }),
        )
        .await;

        // Reply to B1 with method_not_found, B2 with revision_too_low, B3 with ok.
        for req in first_three {
            let id = req["id"].clone();
            let block = req["payload"]["block_id"].as_str().unwrap();
            match block {
                B1 => {
                    send(
                        &mut ws,
                        json!({
                            "type": "response",
                            "id": id,
                            "seq": 2,
                            "error": {"code": "method_not_found", "status": 404, "message": "not found"}
                        }),
                    )
                    .await;
                }
                B2 => {
                    send(
                        &mut ws,
                        json!({
                            "type": "response",
                            "id": id,
                            "seq": 2,
                            "error": {"code": "revision_too_low", "status": 400, "message": "needs 15"}
                        }),
                    )
                    .await;
                }
                B3 => {
                    send(
                        &mut ws,
                        json!({
                            "type": "response",
                            "id": id,
                            "seq": 2,
                            "payload": ports_payload("ok", &[(8003, "127.0.0.1", 3, "b3")])
                        }),
                    )
                    .await;
                }
                other => panic!("unexpected block in first batch: {other}"),
            }
        }

        // Now B4 and B5 arrive as slots free up.
        let mut seen = HashSet::new();
        for _ in 0..2 {
            let req = recv_skipping_meta(&mut ws, 2).await.unwrap();
            assert_eq!(req["method"], "com.superlogical.terminal.ports");
            let block = req["payload"]["block_id"].as_str().unwrap().to_owned();
            seen.insert(block.clone());
            let port = if block == B4 { 8004 } else { 8005 };
            send(
                &mut ws,
                json!({
                    "type": "response",
                    "id": req["id"],
                    "seq": 2,
                    "payload": ports_payload("ok", &[(port, "127.0.0.1", 4, "tail")])
                }),
            )
            .await;
        }
        assert!(seen.contains(B4) && seen.contains(B5));
        done_rx.await.unwrap();
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    let m0 = wait_model(&host, |m| m.connected && m.protocol.api_revision == 15).await;
    // Stale epoch is ignored.
    host.commands
        .send(HostCommand::WatchPorts {
            epoch: m0.connection_epoch + 99,
            session: Some(SESSION.into()),
        })
        .unwrap();
    // Live epoch reads all 5 blocks respecting concurrency cap 3, followed by deterministic fence.
    host.commands
        .send(HostCommand::WatchPorts {
            epoch: m0.connection_epoch,
            session: Some(SESSION.into()),
        })
        .unwrap();
    host.commands.send(HostCommand::RefreshSessions).unwrap();

    let m = wait_model(&host, |m| m.ports.len() == 5 && m.ports_reading.is_empty()).await;
    assert!(m.connected);
    assert!(m.notice.is_none());
    assert_eq!(m.ports[B1].status, PortsStatus::Unsupported);
    assert!(m.ports[B1].ports.is_empty());
    assert_eq!(m.ports[B2].status, PortsStatus::Unsupported);
    assert!(m.ports[B2].ports.is_empty());
    assert_eq!(m.ports[B3].ports[0].port, 8003);
    assert_eq!(m.ports[B4].ports[0].port, 8004);
    assert_eq!(m.ports[B5].ports[0].port, 8005);

    done_tx.send(()).unwrap();
    drop(host);
    server.await.unwrap();
}

#[test]
fn host_wide_4096_port_budget_enforced_across_blocks() {
    let block_ids: Vec<String> = (1..=17)
        .map(|i| format!("block:00000000-0000-0000-0000-{i:012}"))
        .collect();
    let block_refs: Vec<&str> = block_ids.iter().map(String::as_str).collect();

    let mut model = HostModel {
        connected: true,
        ..Default::default()
    };
    model
        .views
        .insert(SESSION.into(), view_with_blocks(SESSION, 1, &block_refs));
    let (wake_tx, _wake_rx) = flume::bounded(1);
    let shared = Shared {
        model: Arc::new(Mutex::new(model)),
        wake: wake_tx,
    };
    let mut state = ControlState::default();

    let full_block_payload = || {
        let ports: Vec<Value> = (1..=MAX_PORTS_PER_BLOCK as u16)
            .map(|p| {
                json!({
                    "port": p,
                    "address": "127.0.0.1",
                    "protocol": "tcp",
                    "first_seen_at": "2026-11-24T12:00:00Z",
                    "pid": p
                })
            })
            .collect();
        let processes: Vec<Value> = (1..=MAX_PORTS_PER_BLOCK as u16)
            .map(|p| json!({"pid": p, "user_id": 1000, "name": format!("p{p}")}))
            .collect();
        json!({"status": "ok", "ports": ports, "processes": processes})
    };

    // 16 blocks * 256 ports = 4096 ports exactly.
    for (idx, block) in block_ids[..16].iter().enumerate() {
        ports::apply_event(
            &shared,
            &mut state,
            SESSION,
            block,
            (idx + 1) as u64,
            &full_block_payload(),
        );
    }
    // 17th block has 0 remaining budget: trimmed to 0 ports with capped = true.
    ports::apply_event(
        &shared,
        &mut state,
        SESSION,
        &block_ids[16],
        17,
        &full_block_payload(),
    );

    let model = shared.model.lock().unwrap();
    let total: usize = model.ports.values().map(|s| s.ports.len()).sum();
    assert_eq!(total, MAX_PORTS_PER_HOST);
    assert!(!model.ports[&block_ids[0]].capped);
    assert!(model.ports[&block_ids[16]].capped);
    assert!(model.ports[&block_ids[16]].ports.is_empty());
    assert!(model.ports[&block_ids[16]].processes.is_empty());
}

#[test]
fn late_reply_for_forgotten_block_is_ignored() {
    let mut model = HostModel {
        connected: true,
        ..Default::default()
    };
    model
        .views
        .insert(SESSION.into(), view_with_blocks(SESSION, 1, &[B1]));
    model.ports_reading.insert(B2.into());
    let (wake_tx, _wake_rx) = flume::bounded(1);
    let shared = Shared {
        model: Arc::new(Mutex::new(model)),
        wake: wake_tx,
    };
    let mut state = ControlState::default();

    ports::record_reply(
        &shared,
        &mut state,
        ports::FinishedRead {
            session: SESSION.into(),
            block: B2.into(),
            outcome: ports::ReplyOutcome::Snapshot {
                seq: 10,
                payload: ports_payload("ok", &[(9090, "127.0.0.1", 1, "ghost")]),
            },
        },
    );
    let model = shared.model.lock().unwrap();
    assert!(!model.ports.contains_key(B2));
    assert!(!model.ports_reading.contains(B2));
    assert!(!state.ports_seq.contains_key(B2));
}
