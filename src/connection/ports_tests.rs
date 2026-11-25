use super::*;
use crate::ports::{MAX_PORTS_PER_BLOCK, MAX_PORTS_PER_HOST, PortsStatus};
use std::collections::HashSet;

const B1: &str = "block:00000000-0000-0000-0000-000000000001";
const B2: &str = "block:00000000-0000-0000-0000-000000000002";
const B3: &str = "block:00000000-0000-0000-0000-000000000003";
const B4: &str = "block:00000000-0000-0000-0000-000000000004";
const B5: &str = "block:00000000-0000-0000-0000-000000000005";
const SESSION_OTHER: &str = "session:00000000-0000-0000-0000-000000000002";

fn view_with_blocks(session: &str, revision: u64, blocks: &[&str]) -> session::ViewResponse {
    session::ViewResponse {
        session_id: Some(session.into()),
        label: Some("work".into()),
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

/// Answer standard metadata queries (`program_status`, `title`, `process`) and background
/// `session.view` for `SESSION_OTHER` until another request arrives or the socket closes.
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
                    Some("session.view")
                        if req["payload"]["session_id"].as_str() == Some(SESSION_OTHER) =>
                    {
                        send(
                            ws,
                            json!({
                                "type": "response",
                                "id": req["id"],
                                "seq": seq,
                                "payload": view_with_blocks(SESSION_OTHER, 1, &[])
                            }),
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
async fn ports_changed_never_marks_urgent_views_or_activity() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 16).await;
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
async fn lifecycle_cleanup_block_closed_undo_close_remove_session_and_reconnect() {
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
        // Restored B2 triggers query_activity once (3 metadata RPCs).
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

        // 4. Reconnect clears B2's ports snapshot cleanly.
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

    // Wait for disconnect, trigger immediate reconnect.
    wait_model(&host, |m| !m.connected).await;
    s5_tx.send(()).unwrap();
    host.commands.send(HostCommand::Reconnect).unwrap();

    // Reconnect clears old B2 snapshot and loads epoch 2 with 4002.
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
        // Generic event_dropped re-queries B1 metadata (program_status, title, process).
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
        // Reply to the cancelled first_read with stale seq=6 (must be ignored).
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
        // The watched session MUST trigger a fresh re-read (and ZERO session.view!).
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
        // Drain initial metadata and wait for close; any com.superlogical.terminal.ports panics.
        let next = recv_skipping_meta(&mut ws, 1).await;
        assert!(
            next.is_none(),
            "API 14 must never send com.superlogical.terminal.ports, got: {next:?}"
        );
        done_rx.await.unwrap();
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

        // First 3 ports requests arrive concurrently (cap = 3).
        let mut first_three = Vec::new();
        for _ in 0..3 {
            let req = recv_skipping_meta(&mut ws, 1).await.unwrap();
            assert_eq!(req["method"], "com.superlogical.terminal.ports");
            first_three.push(req);
        }
        // Assert no 4th request is sent while 3 are in flight.
        let extra =
            tokio::time::timeout(Duration::from_millis(80), recv_skipping_meta(&mut ws, 1)).await;
        assert!(extra.is_err(), "more than 3 port reads in flight at once");

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
    // Live epoch reads all 5 blocks respecting concurrency cap 3.
    host.commands
        .send(HostCommand::WatchPorts {
            epoch: m0.connection_epoch,
            session: Some(SESSION.into()),
        })
        .unwrap();

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
