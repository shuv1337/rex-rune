//! Worker-level tests for detached `HostCommand::FetchView` delivery, concurrency,
//! atomicity, API error mapping, epoch scoping, and disconnect dropping.
use super::*;

async fn wait_wake(host: &HostConnection, ready: impl Fn(&HostModel) -> bool) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if ready(&host.model.lock().unwrap()) {
                return;
            }
            host.wake.recv_async().await.unwrap();
        }
    })
    .await
    .unwrap();
}

async fn next_fetch_outcome(host: &HostConnection, id: u64) -> ViewFetchOutcome {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            {
                let mut m = host.model.lock().unwrap();
                if let Some(pos) = m.view_fetches.iter().position(|(got, _)| *got == id) {
                    return m.view_fetches.remove(pos).1;
                }
            }
            host.wake.recv_async().await.unwrap();
        }
    })
    .await
    .unwrap()
}

fn boot_with_extra(extra: &[&str]) -> Value {
    let mut b = serde_json::to_value(boot()).unwrap();
    let sessions = b["attach_all"]["sessions"].as_array_mut().unwrap();
    for id in extra {
        sessions.push(json!({"session_id": id, "label": id}));
    }
    b
}

/// Complete bootstrap and fail the initial status-sweep view reads for `extra` sessions
/// so `model.sessions` lists them, `model.views` holds only `SESSION`, and status_sweep
/// will not send further unsolicited `session.view` requests.
async fn bootstrap_unloaded(ws: &mut WebSocketStream<TcpStream>, extra: &[&str]) {
    let req = recv(ws).await;
    assert_eq!(req["method"], "session.bootstrap");
    reply(ws, &req, 1, boot_with_extra(extra)).await;
    let mut failed = HashSet::new();
    while failed.len() < extra.len() {
        let req = recv(ws).await;
        assert_eq!(req["method"], "session.view", "{req}");
        let sid = req["payload"]["session_id"].as_str().unwrap().to_owned();
        assert!(extra.contains(&sid.as_str()), "unexpected session {sid}");
        failed.insert(sid);
        send(
            ws,
            json!({
                "type": "response",
                "id": req["id"],
                "error": {"code": "internal", "message": "sweep skipped", "status": 500}
            }),
        )
        .await;
    }
}

#[tokio::test]
async fn fetch_view_short_circuits_loaded_unlisted_expired_and_stale_epoch_without_rpc() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 13).await;
        bootstrap_unloaded(&mut ws, &["s1"]).await;
        // Only a sentinel request sent after all four FetchView commands should arrive:
        // none of the four short-circuited FetchView commands may emit `session.view`.
        let req = recv(&mut ws).await;
        assert_eq!(
            req["method"], "session.list",
            "expected no session.view RPC before sentinel; got {req}"
        );
        reply(
            &mut ws,
            &req,
            2,
            json!({"sessions": [
                {"session_id": SESSION, "label": "main"},
                {"session_id": "s1", "label": "s1"}
            ]}),
        )
        .await;
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    wait_wake(&host, |m| m.connected && !m.activity_reading).await;
    let epoch = host.model.lock().unwrap().connection_epoch;

    // 1. Stale epoch -> ignored silently, no outcome, no RPC.
    host.commands
        .send(HostCommand::FetchView {
            id: 1,
            epoch: epoch + 99,
            deadline: Instant::now() + Duration::from_secs(5),
            session: "s1".into(),
        })
        .unwrap();
    // 2. Already loaded session -> Loaded immediately, no RPC.
    host.commands
        .send(HostCommand::FetchView {
            id: 2,
            epoch,
            deadline: Instant::now() + Duration::from_secs(5),
            session: SESSION.into(),
        })
        .unwrap();
    // 3. Unlisted session -> NotFound immediately, no RPC.
    host.commands
        .send(HostCommand::FetchView {
            id: 3,
            epoch,
            deadline: Instant::now() + Duration::from_secs(5),
            session: "unlisted".into(),
        })
        .unwrap();
    // 4. Past-deadline request for listed unloaded session -> Failed immediately, no RPC.
    host.commands
        .send(HostCommand::FetchView {
            id: 4,
            epoch,
            deadline: Instant::now() - Duration::from_secs(1),
            session: "s1".into(),
        })
        .unwrap();
    // Sentinel control request to prove the worker processed all four commands with zero RPCs.
    host.commands
        .send(HostCommand::Request {
            method: "session.list".into(),
            payload: json!({}),
        })
        .unwrap();

    assert_eq!(next_fetch_outcome(&host, 2).await, ViewFetchOutcome::Loaded);
    assert_eq!(
        next_fetch_outcome(&host, 3).await,
        ViewFetchOutcome::NotFound
    );
    assert_eq!(next_fetch_outcome(&host, 4).await, ViewFetchOutcome::Failed);
    assert!(
        !host
            .model
            .lock()
            .unwrap()
            .view_fetches
            .iter()
            .any(|(id, _)| *id == 1),
        "stale epoch must produce no outcome"
    );

    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn fetch_view_is_detached_atomic_coalesces_duplicates_and_maps_api_not_found() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 13).await;
        bootstrap_unloaded(&mut ws, &["s1", "s2"]).await;

        // Receive both detached session.view requests (s1 and s2) AND an interleaved
        // control request before answering s1, proving FetchView does not block the loop
        // and duplicate s1 fetches coalesce to one RPC.
        let req_s1 = recv(&mut ws).await;
        assert_eq!(req_s1["method"], "session.view");
        assert_eq!(req_s1["payload"]["session_id"], "s1");

        let req_s2 = recv(&mut ws).await;
        assert_eq!(req_s2["method"], "session.view");
        assert_eq!(req_s2["payload"]["session_id"], "s2");

        let control_req = recv(&mut ws).await;
        assert_eq!(
            control_req["method"], "session.list",
            "interleaved control traffic must not block behind in-flight FetchView"
        );
        reply(
            &mut ws,
            &control_req,
            2,
            json!({"sessions": [
                {"session_id": SESSION, "label": "main"},
                {"session_id": "s1", "label": "s1"},
                {"session_id": "s2", "label": "s2"}
            ]}),
        )
        .await;

        // Answer s2 with API not_found first: connection must stay alive and report NotFound.
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req_s2["id"],
                "error": {"code": "not_found", "message": "session gone", "status": 404}
            }),
        )
        .await;

        // Now answer s1 with its view: both coalesced waiters (10 and 11) get Loaded.
        reply(&mut ws, &req_s1, 3, view_of("s1")).await;

        // Verify connection is still alive after API not_found by handling one more call.
        let alive = recv(&mut ws).await;
        assert_eq!(alive["method"], "session.list");
        reply(
            &mut ws,
            &alive,
            4,
            json!({"sessions": [
                {"session_id": SESSION, "label": "main"},
                {"session_id": "s1", "label": "s1"}
            ]}),
        )
        .await;

        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    wait_wake(&host, |m| m.connected && !m.activity_reading).await;
    let epoch = host.model.lock().unwrap().connection_epoch;
    let deadline = Instant::now() + Duration::from_secs(5);

    // Enqueue s1 twice (must coalesce to one session.view RPC), s2, and an interleaved request.
    for (id, session) in [(10, "s1"), (11, "s1"), (12, "s2")] {
        host.commands
            .send(HostCommand::FetchView {
                id,
                epoch,
                deadline,
                session: session.into(),
            })
            .unwrap();
    }
    host.commands
        .send(HostCommand::Request {
            method: "session.list".into(),
            payload: json!({}),
        })
        .unwrap();

    assert_eq!(
        next_fetch_outcome(&host, 12).await,
        ViewFetchOutcome::NotFound
    );

    // Check atomic update on the wake that delivers s1's Loaded outcome:
    // views, background_views, and both coalesced outcomes must all be present together,
    // while selected remains SESSION.
    wait_wake(&host, |m| m.view_fetches.iter().any(|(id, _)| *id == 10)).await;
    {
        let mut m = host.model.lock().unwrap();
        assert!(m.connected, "API not_found must not disconnect host");
        assert!(m.views.contains_key("s1"));
        assert!(m.background_views.contains("s1"));
        assert_eq!(m.selected.as_deref(), Some(SESSION));
        assert!(m.view_fetches.contains(&(10, ViewFetchOutcome::Loaded)));
        assert!(m.view_fetches.contains(&(11, ViewFetchOutcome::Loaded)));
        m.view_fetches.clear();
    }

    // Confirm control connection still works after API not_found.
    host.commands
        .send(HostCommand::Request {
            method: "session.list".into(),
            payload: json!({}),
        })
        .unwrap();
    wait_wake(&host, |m| m.connected).await;

    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn fetch_view_while_disconnected_is_dropped_cleanly_without_notice_or_replay() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (drop_tx, drop_rx) = tokio::sync::oneshot::channel::<()>();
    let (reconnected_tx, reconnected_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        // First connection: bootstrap then drop when signaled.
        let mut ws1 = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws1, 13).await;
        bootstrap_unloaded(&mut ws1, &["s1"]).await;
        drop_rx.await.unwrap();
        let _ = ws1.close(None).await;
        drop(ws1);

        // Second connection: after reconnect, FetchView sent while disconnected must NOT replay.
        let mut ws2 = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws2, 13).await;
        bootstrap_unloaded(&mut ws2, &["s1"]).await;
        reconnected_tx.send(()).unwrap();
        assert!(matches!(ws2.next().await, Some(Ok(Message::Close(_)))));
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    wait_wake(&host, |m| m.connected && !m.activity_reading).await;
    let old_epoch = host.model.lock().unwrap().connection_epoch;

    drop_tx.send(()).unwrap();
    wait_wake(&host, |m| !m.connected).await;

    // Send FetchView while disconnected: must be dropped silently (no notice, no replay).
    host.commands
        .send(HostCommand::FetchView {
            id: 99,
            epoch: old_epoch,
            deadline: Instant::now() + Duration::from_secs(5),
            session: "s1".into(),
        })
        .unwrap();
    host.commands.send(HostCommand::Reconnect).unwrap();
    reconnected_rx.await.unwrap();
    wait_wake(&host, |m| {
        m.connected && m.connection_epoch > old_epoch && !m.activity_reading
    })
    .await;

    {
        let m = host.model.lock().unwrap();
        assert!(
            m.notice.is_none(),
            "disconnected FetchView must not emit a notice: {:?}",
            m.notice
        );
        assert!(
            m.view_fetches.is_empty(),
            "disconnected FetchView must not leak across reconnect: {:?}",
            m.view_fetches
        );
    }

    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn fetch_view_refreshes_when_session_view_changed_arrives_mid_fetch() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (event_seen_tx, event_seen_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 13).await;
        bootstrap_unloaded(&mut ws, &["s1"]).await;

        let req1 = recv(&mut ws).await;
        assert_eq!(req1["method"], "session.view");
        assert_eq!(req1["payload"]["session_id"], "s1");

        // Deliver session_view_changed for s1 BEFORE answering req1, followed by a
        // sentinel label change on the same ordered event stream so the test waits on
        // the worker having processed session_view_changed with zero sleep.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 2,
                "payload": {
                    "type": "session_event",
                    "payload": {
                        "session_id": "s1",
                        "event": {"type": "session_view_changed", "payload": {"revision": 2}}
                    }
                }
            }),
        )
        .await;
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 3,
                "payload": {
                    "type": "session_event",
                    "payload": {
                        "session_id": SESSION,
                        "event": {"type": "session_label_changed", "payload": {"label": "event-synced"}}
                    }
                }
            }),
        )
        .await;
        event_seen_rx.await.unwrap();

        // Answer req1 with stale revision 1.
        reply(
            &mut ws,
            &req1,
            2,
            json!({
                "revision": 1,
                "session_id": "s1",
                "active_window_id": "w-old",
                "windows": [{"window_id": "w-old", "label": "old", "layers": []}]
            }),
        )
        .await;

        // The worker must immediately follow up with a fresh session.view for s1.
        let req2 = recv(&mut ws).await;
        assert_eq!(req2["method"], "session.view");
        assert_eq!(req2["payload"]["session_id"], "s1");
        reply(
            &mut ws,
            &req2,
            4,
            json!({
                "revision": 2,
                "session_id": "s1",
                "active_window_id": "w-new",
                "windows": [{"window_id": "w-new", "label": "new", "layers": []}]
            }),
        )
        .await;

        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    wait_wake(&host, |m| m.connected && !m.activity_reading).await;
    let epoch = host.model.lock().unwrap().connection_epoch;

    host.commands
        .send(HostCommand::FetchView {
            id: 42,
            epoch,
            deadline: Instant::now() + Duration::from_secs(5),
            session: "s1".into(),
        })
        .unwrap();

    // Wait for the ordered sentinel event after session_view_changed, then release req1's reply.
    wait_wake(&host, |m| {
        m.sessions
            .iter()
            .any(|s| s.session_id == SESSION && s.label == "event-synced")
    })
    .await;
    event_seen_tx.send(()).unwrap();

    assert_eq!(
        next_fetch_outcome(&host, 42).await,
        ViewFetchOutcome::Loaded
    );
    wait_wake(&host, |m| {
        m.views.get("s1").is_some_and(|v| v.revision == 2)
    })
    .await;
    {
        let m = host.model.lock().unwrap();
        assert_eq!(m.views["s1"].revision, 2);
        assert_eq!(m.views["s1"].active_window_id.as_deref(), Some("w-new"));
        assert!(m.background_views.contains("s1"));
        assert_eq!(m.selected.as_deref(), Some(SESSION));
    }

    drop(host);
    server.await.unwrap();
}
