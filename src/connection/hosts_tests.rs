use super::*;
use crate::connection::hosts::{SharedHostState, WriteKind, WriteOutcome, WriteResult};

async fn wait_until_model(
    host: &HostConnection,
    predicate: impl Fn(&HostModel) -> bool,
) -> HostModel {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            {
                let model = host.model.lock().unwrap();
                if predicate(&model) {
                    return model.clone();
                }
            }
            let _ = host.wake.recv_async().await;
        }
    })
    .await
    .unwrap()
}

async fn wait_shared_revision(host: &HostConnection, min_rev: u64) -> HostModel {
    wait_until_model(host, |m| m.connected && m.shared_revision >= min_rev).await
}

async fn wait_session_label(host: &HostConnection, label: &str) -> HostModel {
    wait_until_model(host, |m| {
        m.connected && m.sessions.first().is_some_and(|s| s.label == label)
    })
    .await
}

/// Send an observable session event after a prior event on the same TCP/WebSocket stream so the
/// test can deterministically prove the client processed the prior event without fixed sleeps.
async fn send_label_fence(ws: &mut WebSocketStream<TcpStream>, seq: u64, label: &str) {
    send(
        ws,
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
        }),
    )
    .await;
}

/// Wait for client shutdown cleanly (ignoring any ping/pong frames, failing on unexpected requests).
async fn assert_clean_close(ws: &mut WebSocketStream<TcpStream>) {
    loop {
        match ws.next().await {
            Some(Ok(Message::Close(_))) | None => return,
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            other => panic!("unexpected frame before close: {other:?}"),
        }
    }
}

async fn bootstrap(ws: &mut WebSocketStream<TcpStream>) {
    let request = recv(ws).await;
    assert_eq!(request["method"], "session.bootstrap");
    send(
        ws,
        json!({"type":"response","id":request["id"],"seq":1,"payload":boot()}),
    )
    .await;
}

#[tokio::test]
async fn hosts_list_reads_once_skips_same_hash_rereads_on_change_and_never_writes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (seen_first, wait_first) = tokio::sync::oneshot::channel::<()>();
    let (seen_fence, wait_fence) = tokio::sync::oneshot::channel::<()>();
    let (seen_second, wait_second) = tokio::sync::oneshot::channel::<()>();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 13).await;
        bootstrap(&mut ws).await;

        // 1. Initial hosts.list read on connection.
        let req1 = recv(&mut ws).await;
        assert_eq!(req1["method"], "hosts.list");
        assert_eq!(req1["payload"], json!({}));
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req1["id"],
                "payload": {
                    "hash": "hash-1",
                    "hosts": [{
                        "id": "host:0199abcd-1234-7abc-8def-0123456789ab",
                        "label": "Studio",
                        "endpoints": ["studio.invalid"],
                        "unknown_future": {"keep": true}
                    }],
                    "diagnostics": [{
                        "severity": "warning",
                        "source": "init.lua:4",
                        "message": "duplicate endpoint"
                    }]
                }
            }),
        )
        .await;
        wait_first.await.unwrap();

        // 2. Same-hash hosts_changed event followed by a FIFO label fence: must NOT trigger
        // another hosts.list before step 3.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 0,
                "payload": {"type": "hosts_changed", "payload": {"hash": "hash-1"}}
            }),
        )
        .await;
        send_label_fence(&mut ws, 2, "fence-same-hash").await;
        wait_fence.await.unwrap();

        // 3. Different-hash hosts_changed event triggers exactly one re-read.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 0,
                "payload": {"type": "hosts_changed", "payload": {"hash": "hash-2"}}
            }),
        )
        .await;
        let req2 = recv(&mut ws).await;
        assert_eq!(req2["method"], "hosts.list");
        assert_eq!(req2["payload"], json!({}));
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req2["id"],
                "payload": {
                    "hash": "hash-2",
                    "hosts": [{
                        "id": "host:0199abcd-1234-7abc-8def-0123456789ab",
                        "label": "Studio Pro",
                        "endpoints": ["studio.invalid"]
                    }]
                }
            }),
        )
        .await;
        wait_second.await.unwrap();

        // 4. Conflict on a later change sets SharedHostState::Conflict without disconnecting.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 0,
                "payload": {"type": "hosts_changed", "payload": {"hash": "hash-3"}}
            }),
        )
        .await;
        let req3 = recv(&mut ws).await;
        assert_eq!(req3["method"], "hosts.list");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req3["id"],
                "error": {
                    "code": "conflict",
                    "message": "hosts/everywhere.json is not a JSON object",
                    "status": 409
                }
            }),
        )
        .await;
        done_rx.await.unwrap();

        // Zero hosts.put or hosts.remove ever sent: next frame must be Close.
        assert_clean_close(&mut ws).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    host.commands.send(HostCommand::WatchSharedHosts).unwrap();

    let m1 = wait_shared_revision(&host, 1).await;
    let SharedHostState::Loaded(list1) = &m1.shared else {
        panic!("expected Loaded, got {:?}", m1.shared);
    };
    assert_eq!(list1.hash, "hash-1");
    assert_eq!(list1.hosts.len(), 1);
    assert_eq!(list1.hosts[0].label, "Studio");
    assert_eq!(
        list1.hosts[0].raw.get("unknown_future"),
        Some(&json!({"keep": true}))
    );
    assert_eq!(list1.diagnostics.len(), 1);
    seen_first.send(()).unwrap();

    let fenced = wait_session_label(&host, "fence-same-hash").await;
    assert_eq!(
        fenced.shared_revision, 1,
        "same-hash hosts_changed must not bump revision"
    );
    seen_fence.send(()).unwrap();

    let m2 = wait_shared_revision(&host, 2).await;
    let SharedHostState::Loaded(list2) = &m2.shared else {
        panic!("expected Loaded, got {:?}", m2.shared);
    };
    assert_eq!(list2.hash, "hash-2");
    assert_eq!(list2.hosts[0].label, "Studio Pro");
    seen_second.send(()).unwrap();

    let m3 = wait_shared_revision(&host, 3).await;
    assert_eq!(
        m3.shared,
        SharedHostState::Conflict("hosts/everywhere.json is not a JSON object".into())
    );
    assert!(m3.connected);
    done_tx.send(()).unwrap();

    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn in_flight_read_with_reverting_hash_event_rereads_and_settles_on_latest() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (seen_h1, wait_h1) = tokio::sync::oneshot::channel::<()>();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 13).await;
        bootstrap(&mut ws).await;

        // Initial load settles on h1.
        let req1 = recv(&mut ws).await;
        assert_eq!(req1["method"], "hosts.list");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req1["id"],
                "payload": {
                    "hash": "h1",
                    "hosts": [{"id": "host:1", "label": "Original", "endpoints": ["a.invalid"]}]
                }
            }),
        )
        .await;
        wait_h1.await.unwrap();

        // Change to h2 -> client sends req2 (now in flight while cache is still h1).
        send(
            &mut ws,
            json!({"type": "event", "seq": 0, "payload": {"type": "hosts_changed", "payload": {"hash": "h2"}}}),
        )
        .await;
        let req2 = recv(&mut ws).await;
        assert_eq!(req2["method"], "hosts.list");

        // Before req2 answers, list reverts to h1! Even though cached hash is still h1,
        // the in-flight flag must force a third hosts.list rather than settling on stale h2.
        send(
            &mut ws,
            json!({"type": "event", "seq": 0, "payload": {"type": "hosts_changed", "payload": {"hash": "h1"}}}),
        )
        .await;
        let req3 = recv(&mut ws).await;
        assert_eq!(req3["method"], "hosts.list");

        // Answer stale req2 first (dropped by client), then answer req3 with reverted h1.
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req2["id"],
                "payload": {
                    "hash": "h2",
                    "hosts": [{"id": "host:1", "label": "Stale", "endpoints": ["a.invalid"]}]
                }
            }),
        )
        .await;
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req3["id"],
                "payload": {
                    "hash": "h1",
                    "hosts": [{"id": "host:1", "label": "Reverted", "endpoints": ["a.invalid"]}]
                }
            }),
        )
        .await;
        done_rx.await.unwrap();
        assert_clean_close(&mut ws).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    host.commands.send(HostCommand::WatchSharedHosts).unwrap();

    let m1 = wait_shared_revision(&host, 1).await;
    assert_eq!(m1.shared.hash(), Some("h1"));
    seen_h1.send(()).unwrap();

    let m2 = wait_shared_revision(&host, 2).await;
    let SharedHostState::Loaded(list) = &m2.shared else {
        panic!("expected Loaded, got {:?}", m2.shared);
    };
    assert_eq!(list.hash, "h1");
    assert_eq!(list.hosts[0].label, "Reverted");
    done_tx.send(()).unwrap();

    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn unknown_or_low_revision_hosts_list_is_quiet_and_unsupported() {
    for code in ["method_not_found", "revision_too_low"] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (seen_unsupported, wait_unsupported) = tokio::sync::oneshot::channel::<()>();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

        let server = tokio::spawn(async move {
            let mut ws = accept(&listener, "rex.control.v1").await;
            welcome(&mut ws, 13).await;
            bootstrap(&mut ws).await;
            let req = recv(&mut ws).await;
            assert_eq!(req["method"], "hosts.list");
            send(
                &mut ws,
                json!({
                    "type": "response",
                    "id": req["id"],
                    "error": {"code": code, "message": "unsupported", "status": 404}
                }),
            )
            .await;
            // Wait until the client has recorded Unsupported before sending hosts_changed
            // so the two frames never race in one TCP read buffer.
            wait_unsupported.await.unwrap();
            send(
                &mut ws,
                json!({
                    "type": "event",
                    "seq": 0,
                    "payload": {"type": "hosts_changed", "payload": {"hash": "new"}}
                }),
            )
            .await;
            send_label_fence(&mut ws, 2, "fence-unsupported").await;
            done_rx.await.unwrap();
            assert_clean_close(&mut ws).await;
        });

        let host = HostConnection::spawn(endpoint, None, None, None, false, None);
        host.commands.send(HostCommand::WatchSharedHosts).unwrap();
        let model = wait_shared_revision(&host, 1).await;
        assert_eq!(model.shared, SharedHostState::Unsupported, "{code}");
        assert!(model.connected);
        assert_eq!(model.notice, None);
        seen_unsupported.send(()).unwrap();

        let fenced = wait_session_label(&host, "fence-unsupported").await;
        assert_eq!(fenced.shared, SharedHostState::Unsupported, "{code}");
        assert_eq!(fenced.shared_revision, 1, "{code}");
        done_tx.send(()).unwrap();

        drop(host);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn failed_first_read_records_error_and_watch_command_retries() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 13).await;
        bootstrap(&mut ws).await;

        // First read returns an internal error -> Failed state (not stuck on Unloaded).
        let req1 = recv(&mut ws).await;
        assert_eq!(req1["method"], "hosts.list");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req1["id"],
                "error": {"code": "internal", "message": "temporary io failure", "status": 500}
            }),
        )
        .await;

        // A second WatchSharedHosts (e.g. toggling sync off->on) retries hosts.list.
        let req2 = recv(&mut ws).await;
        assert_eq!(req2["method"], "hosts.list");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req2["id"],
                "payload": {
                    "hash": "recovered",
                    "hosts": [{"id": "host:ok", "label": "Recovered", "endpoints": ["ok.invalid"]}]
                }
            }),
        )
        .await;
        done_rx.await.unwrap();
        assert_clean_close(&mut ws).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    host.commands.send(HostCommand::WatchSharedHosts).unwrap();

    let m1 = wait_shared_revision(&host, 1).await;
    assert_eq!(
        m1.shared,
        SharedHostState::Failed("temporary io failure".into())
    );

    // Trigger retry via WatchSharedHosts.
    host.commands.send(HostCommand::WatchSharedHosts).unwrap();
    let m2 = wait_shared_revision(&host, 2).await;
    assert_eq!(m2.shared.hash(), Some("recovered"));
    done_tx.send(()).unwrap();

    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn non_watched_host_never_sends_hosts_list() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 13).await;
        bootstrap(&mut ws).await;
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 0,
                "payload": {"type": "hosts_changed", "payload": {"hash": "h1"}}
            }),
        )
        .await;
        send_label_fence(&mut ws, 2, "fence-unwatched").await;
        done_rx.await.unwrap();
        assert_clean_close(&mut ws).await;
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    let model = wait_session_label(&host, "fence-unwatched").await;
    assert_eq!(model.shared, SharedHostState::Unloaded);
    assert_eq!(model.shared_revision, 0);
    done_tx.send(()).unwrap();
    drop(host);
    server.await.unwrap();
}

async fn wait_write_result(host: &HostConnection, id: u64) -> (HostModel, WriteResult) {
    let model = wait_until_model(host, |m| m.shared_writes.iter().any(|w| w.id == id)).await;
    let res = model
        .shared_writes
        .iter()
        .find(|w| w.id == id)
        .unwrap()
        .clone();
    (model, res)
}

#[tokio::test]
async fn put_shared_host_succeeds_refreshes_and_ignores_stale_epoch() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 13).await;
        bootstrap(&mut ws).await;

        // Initial hosts.list -> hash-1.
        let req1 = recv(&mut ws).await;
        assert_eq!(req1["method"], "hosts.list");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req1["id"],
                "payload": {
                    "hash": "hash-1",
                    "hosts": [{
                        "id": "host:1",
                        "label": "Studio",
                        "endpoints": ["studio.invalid"],
                        "custom_extra": {"k": 1}
                    }]
                }
            }),
        )
        .await;

        // Stale epoch PutSharedHost must NEVER hit the wire; next wire request is the valid put.
        let put = recv(&mut ws).await;
        assert_eq!(put["method"], "hosts.put");
        assert_eq!(put["payload"]["host"]["id"], "host:1");
        assert_eq!(put["payload"]["host"]["label"], "Studio Pro");
        assert_eq!(put["payload"]["host"]["custom_extra"], json!({"k": 1}));
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": put["id"],
                "payload": {
                    "host": {
                        "id": "host:1",
                        "label": "Studio Pro",
                        "endpoints": ["studio.invalid"],
                        "custom_extra": {"k": 1},
                        "updated_at": "2026-11-24T10:00:00Z"
                    },
                    "hash": "hash-2"
                }
            }),
        )
        .await;
        // Server also emits matching hosts_changed; client deduplicates with its in-flight reread.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 0,
                "payload": {"type": "hosts_changed", "payload": {"hash": "hash-2"}}
            }),
        )
        .await;

        let req2 = recv(&mut ws).await;
        assert_eq!(req2["method"], "hosts.list");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req2["id"],
                "payload": {
                    "hash": "hash-2",
                    "hosts": [{
                        "id": "host:1",
                        "label": "Studio Pro",
                        "endpoints": ["studio.invalid"],
                        "custom_extra": {"k": 1}
                    }]
                }
            }),
        )
        .await;
        send_label_fence(&mut ws, 2, "fence-after-put").await;
        done_rx.await.unwrap();
        assert_clean_close(&mut ws).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    host.commands.send(HostCommand::WatchSharedHosts).unwrap();
    let m1 = wait_shared_revision(&host, 1).await;
    let epoch = m1.connection_epoch;

    // 1. Stale epoch is refused locally without hitting the wire.
    host.commands
        .send(HostCommand::PutSharedHost {
            id: 1,
            epoch: epoch + 99,
            host: json!({"id": "host:1", "label": "Stale", "endpoints": ["studio.invalid"]}),
        })
        .unwrap();
    let (_, stale_res) = wait_write_result(&host, 1).await;
    assert_eq!(stale_res.kind, WriteKind::Put);
    assert!(matches!(
        stale_res.outcome,
        WriteOutcome::Failed { ref code, .. } if code.as_deref() == Some("stale_epoch")
    ));

    // 2. Valid epoch succeeds and refreshes the cached list once.
    host.commands
        .send(HostCommand::PutSharedHost {
            id: 2,
            epoch,
            host: json!({
                "id": "host:1",
                "label": "Studio Pro",
                "endpoints": ["studio.invalid"],
                "custom_extra": {"k": 1}
            }),
        })
        .unwrap();
    let (_, ok_res) = wait_write_result(&host, 2).await;
    match ok_res.outcome {
        WriteOutcome::Put { host: stored, hash } => {
            assert_eq!(stored.id, "host:1");
            assert_eq!(stored.label, "Studio Pro");
            assert_eq!(hash, "hash-2");
        }
        other => panic!("expected Put outcome, got {other:?}"),
    }
    let m2 = wait_shared_revision(&host, 2).await;
    assert_eq!(m2.shared.hash(), Some("hash-2"));
    let fenced = wait_session_label(&host, "fence-after-put").await;
    assert_eq!(
        fenced.shared_revision, 2,
        "matching hosts_changed must not trigger a redundant hosts.list"
    );

    done_tx.send(()).unwrap();
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn put_shared_host_errors_and_drop_preserve_cached_list_without_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        // First connection: initial load, conflict, not_supported, then abrupt drop on 3rd put.
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 13).await;
        bootstrap(&mut ws).await;

        let req1 = recv(&mut ws).await;
        assert_eq!(req1["method"], "hosts.list");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req1["id"],
                "payload": {
                    "hash": "hash-1",
                    "hosts": [{"id": "host:1", "label": "Studio", "endpoints": ["studio.invalid"]}]
                }
            }),
        )
        .await;

        let put1 = recv(&mut ws).await;
        assert_eq!(put1["method"], "hosts.put");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": put1["id"],
                "error": {"code": "conflict", "message": "label already used", "status": 409}
            }),
        )
        .await;

        let put2 = recv(&mut ws).await;
        assert_eq!(put2["method"], "hosts.put");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": put2["id"],
                "error": {"code": "not_supported", "message": "no state directory", "status": 400}
            }),
        )
        .await;

        let put3 = recv(&mut ws).await;
        assert_eq!(put3["method"], "hosts.put");
        // Drop connection without replying to put3!
        drop(ws);

        // Second connection after reconnect: must only bootstrap + hosts.list, NEVER replay put3!
        let mut ws2 = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws2, 13).await;
        bootstrap(&mut ws2).await;
        let req2 = recv(&mut ws2).await;
        assert_eq!(req2["method"], "hosts.list");
        send(
            &mut ws2,
            json!({
                "type": "response",
                "id": req2["id"],
                "payload": {
                    "hash": "hash-1",
                    "hosts": [{"id": "host:1", "label": "Studio", "endpoints": ["studio.invalid"]}]
                }
            }),
        )
        .await;
        send_label_fence(&mut ws2, 2, "reconnected-clean").await;
        done_rx.await.unwrap();
        assert_clean_close(&mut ws2).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    host.commands.send(HostCommand::WatchSharedHosts).unwrap();
    let m1 = wait_shared_revision(&host, 1).await;
    let epoch = m1.connection_epoch;

    host.commands
        .send(HostCommand::PutSharedHost {
            id: 10,
            epoch,
            host: json!({"label": "Studio", "endpoints": ["other.invalid"]}),
        })
        .unwrap();
    let (m_c, res_c) = wait_write_result(&host, 10).await;
    assert!(matches!(
        res_c.outcome,
        WriteOutcome::Failed { ref code, ref message }
            if code.as_deref() == Some("conflict") && message.contains("label already used")
    ));
    assert_eq!(m_c.shared.hash(), Some("hash-1"));
    assert!(m_c.connected);

    host.commands
        .send(HostCommand::PutSharedHost {
            id: 11,
            epoch,
            host: json!({"label": "New", "endpoints": ["new.invalid"]}),
        })
        .unwrap();
    let (m_ns, res_ns) = wait_write_result(&host, 11).await;
    assert!(matches!(
        res_ns.outcome,
        WriteOutcome::Failed { ref code, .. } if code.as_deref() == Some("not_supported")
    ));
    assert_eq!(m_ns.shared.hash(), Some("hash-1"));
    assert!(m_ns.connected);

    host.commands
        .send(HostCommand::PutSharedHost {
            id: 12,
            epoch,
            host: json!({"label": "Drop", "endpoints": ["drop.invalid"]}),
        })
        .unwrap();
    let (m_drop, res_drop) = wait_write_result(&host, 12).await;
    assert!(matches!(res_drop.outcome, WriteOutcome::Failed { .. }));
    assert_eq!(m_drop.shared.hash(), Some("hash-1"));

    // Kick reconnect immediately so test doesn't wait on backoff timer.
    host.commands.send(HostCommand::Reconnect).unwrap();
    let reconnected = wait_session_label(&host, "reconnected-clean").await;
    assert!(reconnected.connection_epoch > epoch);

    done_tx.send(()).unwrap();
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn remove_shared_host_succeeds_refreshes_and_enforces_epoch_and_errors() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 13).await;
        bootstrap(&mut ws).await;

        let req1 = recv(&mut ws).await;
        assert_eq!(req1["method"], "hosts.list");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req1["id"],
                "payload": {
                    "hash": "hash-1",
                    "hosts": [{"id": "host:1", "label": "Studio", "endpoints": ["studio.invalid"]}]
                }
            }),
        )
        .await;

        // First wire remove returns conflict.
        let rem1 = recv(&mut ws).await;
        assert_eq!(rem1["method"], "hosts.remove");
        assert_eq!(rem1["payload"], json!({"id": "host:1"}));
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": rem1["id"],
                "error": {"code": "conflict", "message": "hosts/everywhere.json is not a JSON object", "status": 409}
            }),
        )
        .await;

        // Second wire remove succeeds and triggers refresh to hash-2.
        let rem2 = recv(&mut ws).await;
        assert_eq!(rem2["method"], "hosts.remove");
        assert_eq!(rem2["payload"], json!({"id": "host:1"}));
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": rem2["id"],
                "payload": {"removed": true, "hash": "hash-2"}
            }),
        )
        .await;
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 0,
                "payload": {"type": "hosts_changed", "payload": {"hash": "hash-2"}}
            }),
        )
        .await;

        let req2 = recv(&mut ws).await;
        assert_eq!(req2["method"], "hosts.list");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req2["id"],
                "payload": {"hash": "hash-2", "hosts": []}
            }),
        )
        .await;
        done_rx.await.unwrap();
        assert_clean_close(&mut ws).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    host.commands.send(HostCommand::WatchSharedHosts).unwrap();
    let m1 = wait_shared_revision(&host, 1).await;
    let epoch = m1.connection_epoch;

    // Stale epoch refused locally.
    host.commands
        .send(HostCommand::RemoveSharedHost {
            id: 20,
            epoch: epoch + 5,
            host_id: "host:1".into(),
        })
        .unwrap();
    let (_, stale) = wait_write_result(&host, 20).await;
    assert_eq!(stale.kind, WriteKind::Remove);
    assert!(matches!(
        stale.outcome,
        WriteOutcome::Failed { ref code, .. } if code.as_deref() == Some("stale_epoch")
    ));

    // Conflict error preserves cached list.
    host.commands
        .send(HostCommand::RemoveSharedHost {
            id: 21,
            epoch,
            host_id: "host:1".into(),
        })
        .unwrap();
    let (mc, conflict) = wait_write_result(&host, 21).await;
    assert!(matches!(
        conflict.outcome,
        WriteOutcome::Failed { ref code, .. } if code.as_deref() == Some("conflict")
    ));
    assert_eq!(mc.shared.hash(), Some("hash-1"));

    // Success removes and refreshes to empty list.
    host.commands
        .send(HostCommand::RemoveSharedHost {
            id: 22,
            epoch,
            host_id: "host:1".into(),
        })
        .unwrap();
    let (_, ok) = wait_write_result(&host, 22).await;
    assert_eq!(
        ok.outcome,
        WriteOutcome::Removed {
            host_id: "host:1".into(),
            removed: true,
            hash: "hash-2".into(),
        }
    );
    let m2 = wait_shared_revision(&host, 2).await;
    let SharedHostState::Loaded(list2) = &m2.shared else {
        panic!("expected Loaded, got {:?}", m2.shared);
    };
    assert_eq!(list2.hash, "hash-2");
    assert!(list2.hosts.is_empty());

    done_tx.send(()).unwrap();
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn shared_host_writes_refused_while_disconnected_and_never_replayed_on_reconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        // Accept first TCP stream and close immediately so client enters disconnected backoff.
        let (stream, _) = listener.accept().await.unwrap();
        drop(stream);

        // Second connection succeeds cleanly; neither disconnected write may be replayed!
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 13).await;
        bootstrap(&mut ws).await;
        send_label_fence(&mut ws, 2, "connected-without-replay").await;
        done_rx.await.unwrap();
        assert_clean_close(&mut ws).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    // Wait until the first failed connection puts the worker into its disconnected backoff loop.
    wait_until_model(&host, |m| {
        !m.connected && m.status.starts_with("Disconnected")
    })
    .await;

    host.commands
        .send(HostCommand::PutSharedHost {
            id: 30,
            epoch: 0,
            host: json!({"label": "Offline", "endpoints": ["offline.invalid"]}),
        })
        .unwrap();
    host.commands
        .send(HostCommand::RemoveSharedHost {
            id: 31,
            epoch: 0,
            host_id: "host:1".into(),
        })
        .unwrap();

    let (_, r_put) = wait_write_result(&host, 30).await;
    let (_, r_rem) = wait_write_result(&host, 31).await;
    assert!(matches!(
        r_put.outcome,
        WriteOutcome::Failed { ref code, .. } if code.as_deref() == Some("disconnected")
    ));
    assert!(matches!(
        r_rem.outcome,
        WriteOutcome::Failed { ref code, .. } if code.as_deref() == Some("disconnected")
    ));

    host.commands.send(HostCommand::Reconnect).unwrap();
    let m = wait_session_label(&host, "connected-without-replay").await;
    assert!(m.connected);

    done_tx.send(()).unwrap();
    drop(host);
    server.await.unwrap();
}
