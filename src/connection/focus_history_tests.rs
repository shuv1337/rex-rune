//! Server-side focus history (`focus_history.reverse`, API 19+) on the production host worker.
use super::*;

const TAB_A: &str = "tab:a";
const TAB_B: &str = "tab:b";
const BLOCK_A: &str = "block:a";
const BLOCK_B: &str = "block:b";
const OTHER_TAB: &str = "tab:other";
const OTHER_BLOCK: &str = "block:other";

fn view_for(session: &str, revision: u64, active_window: &str, active_block: &str) -> Value {
    if session == SESSION {
        json!({
            "session_id": SESSION,
            "label": "snapshot",
            "revision": revision,
            "active_window_id": active_window,
            "windows": [
                {
                    "window_id": TAB_A,
                    "focused_block_id": if active_window == TAB_A { active_block } else { BLOCK_A },
                    "layers": [{
                        "kind": "tiled",
                        "layout": {
                            "split": {
                                "direction": "horizontal",
                                "ratio": 0.5,
                                "before": {"block_id": BLOCK_A},
                                "after": {"block_id": BLOCK_B}
                            }
                        },
                        "blocks": [
                            {"block_id": BLOCK_A, "creator_name": activity::TERMINAL},
                            {"block_id": BLOCK_B, "creator_name": activity::TERMINAL}
                        ]
                    }]
                },
                {
                    "window_id": TAB_B,
                    "focused_block_id": BLOCK_B,
                    "layers": [{
                        "kind": "tiled",
                        "layout": {"block_id": BLOCK_B},
                        "blocks": [{"block_id": BLOCK_B, "creator_name": activity::TERMINAL}]
                    }]
                }
            ]
        })
    } else {
        json!({
            "session_id": OTHER,
            "label": "other",
            "revision": revision,
            "active_window_id": OTHER_TAB,
            "windows": [{
                "window_id": OTHER_TAB,
                "focused_block_id": OTHER_BLOCK,
                "layers": [{
                    "kind": "tiled",
                    "layout": {"block_id": OTHER_BLOCK},
                    "blocks": [{"block_id": OTHER_BLOCK, "creator_name": activity::TERMINAL}]
                }]
            }]
        })
    }
}

fn boot_two_sessions() -> Value {
    let mut initial = serde_json::to_value(boot()).unwrap();
    initial["selected_session"]["view"] = view_for(SESSION, 1, TAB_A, BLOCK_A);
    initial["attach_all"]["sessions"]
        .as_array_mut()
        .unwrap()
        .push(json!({"session_id": OTHER, "label": "other"}));
    initial
}

async fn send_error(ws: &mut WebSocketStream<TcpStream>, req: &Value, code: &str, status: u16) {
    send(
        ws,
        json!({
            "type": "response",
            "id": req["id"],
            "error": {"code": code, "message": "test refusal", "status": status}
        }),
    )
    .await;
}

fn future_deadline() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

/// Read the next non-metadata request, answering background metadata and status-sweep queries.
async fn next_non_meta(ws: &mut WebSocketStream<TcpStream>, seq: u64) -> Value {
    loop {
        let req = recv(ws).await;
        match req["method"].as_str().unwrap_or("") {
            "com.superlogical.terminal.program_status" => {
                reply(ws, &req, seq, json!({"records": []})).await;
            }
            "com.superlogical.terminal.title" => {
                reply(ws, &req, seq, json!({"title": ""})).await;
            }
            "com.superlogical.terminal.process" => {
                reply(ws, &req, seq, json!({})).await;
            }
            "session.view" if req["payload"]["session_id"] == OTHER => {
                reply(ws, &req, seq, view_for(OTHER, 1, OTHER_TAB, OTHER_BLOCK)).await;
            }
            "session.attach" if req["payload"]["session_id"] == OTHER => {
                reply(
                    ws,
                    &req,
                    seq,
                    json!({"client_id": "client:test", "view": view_for(OTHER, 1, OTHER_TAB, OTHER_BLOCK)}),
                )
                .await;
            }
            _ => return req,
        }
    }
}

fn send_sentinel(host: &HostConnection) {
    host.commands
        .send(HostCommand::Request {
            method: "test.sentinel".into(),
            payload: json!({}),
        })
        .unwrap();
}

#[tokio::test]
async fn focus_history_api18_gate_sends_zero_requests_and_reports_unsupported() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 18).await;
        let req = recv(&mut ws).await;
        assert_eq!(req["method"], "session.bootstrap");
        reply(&mut ws, &req, 1, boot_two_sessions()).await;
        // The very next non-meta request MUST be our fence, proving zero focus_history.reverse.
        let fence = next_non_meta(&mut ws, 1).await;
        assert_eq!(
            fence["method"], "test.sentinel",
            "API 18 must never send focus_history.reverse, got: {fence}"
        );
        reply(&mut ws, &fence, 2, json!({})).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    let epoch = host.model.lock().unwrap().connection_epoch;
    host.commands
        .send(HostCommand::ReverseFocusHistory {
            epoch,
            id: 7,
            scope: FocusHistoryScope::Any,
            cursor: None,
            deadline: future_deadline(),
        })
        .unwrap();
    until(&host, |m| {
        m.focus_history_result.as_ref().is_some_and(|r| r.id == 7)
    })
    .await;
    {
        let m = host.model.lock().unwrap();
        assert!(m.connected);
        assert_eq!(m.notice.as_deref(), Some(FOCUS_HISTORY_UNSUPPORTED_NOTICE));
        assert_eq!(
            m.focus_history_result,
            Some(FocusHistoryResult::not_reversed(epoch, 7))
        );
    }
    send_sentinel(&host);
    tokio::time::timeout(Duration::from_secs(8), server)
        .await
        .unwrap()
        .unwrap();
    drop(host);
}

#[tokio::test]
async fn focus_history_unsupported_api_errors_degrade_without_disconnect() {
    for (code, status) in [
        ("method_not_found", 404),
        ("revision_too_low", 400),
        ("not_supported", 501),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut ws = accept(&listener, "rex.control.v1").await;
            welcome(&mut ws, 19).await;
            let req = recv(&mut ws).await;
            assert_eq!(req["method"], "session.bootstrap");
            reply(&mut ws, &req, 1, boot_two_sessions()).await;
            let rev = next_non_meta(&mut ws, 1).await;
            assert_eq!(rev["method"], "focus_history.reverse");
            send_error(&mut ws, &rev, code, status).await;
            let fence = next_non_meta(&mut ws, 1).await;
            assert_eq!(fence["method"], "test.sentinel");
            reply(&mut ws, &fence, 2, json!({})).await;
        });

        let host = HostConnection::spawn(endpoint, None, None, None, false, None);
        connected(&host).await;
        let epoch = host.model.lock().unwrap().connection_epoch;
        host.commands
            .send(HostCommand::ReverseFocusHistory {
                epoch,
                id: 11,
                scope: FocusHistoryScope::Any,
                cursor: None,
                deadline: future_deadline(),
            })
            .unwrap();
        until(&host, |m| {
            m.focus_history_result.as_ref().is_some_and(|r| r.id == 11)
        })
        .await;
        {
            let m = host.model.lock().unwrap();
            assert!(m.connected, "{code} must not disconnect");
            assert_eq!(m.notice.as_deref(), Some(FOCUS_HISTORY_UNSUPPORTED_NOTICE));
            assert_eq!(
                m.focus_history_result,
                Some(FocusHistoryResult::not_reversed(epoch, 11))
            );
        }
        send_sentinel(&host);
        tokio::time::timeout(Duration::from_secs(8), server)
            .await
            .unwrap()
            .unwrap();
        drop(host);
    }
}

#[tokio::test]
async fn focus_history_emits_exact_wire_payloads_for_all_five_scopes_and_cursor() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let cases = [
        (FocusHistoryScope::Any, None, json!({})),
        (FocusHistoryScope::Pane, None, json!({"window_id": TAB_A})),
        (
            FocusHistoryScope::Tab,
            Some(41),
            json!({"session_id": SESSION, "not_window_id": TAB_A, "cursor": 41}),
        ),
        (
            FocusHistoryScope::TabAnySession,
            Some(40),
            json!({"not_window_id": TAB_A, "cursor": 40}),
        ),
        (
            FocusHistoryScope::Session,
            Some(39),
            json!({"not_session_id": SESSION, "cursor": 39}),
        ),
    ];
    let expected: Vec<Value> = cases.iter().map(|(_, _, p)| p.clone()).collect();
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 21).await;
        let req = recv(&mut ws).await;
        assert_eq!(req["method"], "session.bootstrap");
        reply(&mut ws, &req, 1, boot_two_sessions()).await;
        let mut seq = 1u64;
        for (idx, exp_payload) in expected.iter().enumerate() {
            let rev = next_non_meta(&mut ws, seq).await;
            assert_eq!(rev["method"], "focus_history.reverse");
            assert_eq!(rev["payload"], *exp_payload, "scope index {idx}");
            seq += 1;
            reply(
                &mut ws,
                &rev,
                seq,
                json!({
                    "reversed": true,
                    "cursor": 100 - idx as u64,
                    "session_id": SESSION,
                    "window_id": TAB_A,
                    "block_id": BLOCK_B
                }),
            )
            .await;
            // Same-session reverse refreshes the target view via session.attach so it never
            // re-writes server-side focus history.
            let view = next_non_meta(&mut ws, seq).await;
            assert_eq!(view["method"], "session.attach");
            assert_eq!(view["payload"]["session_id"], SESSION);
            seq += 1;
            reply(
                &mut ws,
                &view,
                seq,
                json!({"client_id": "client:test", "view": view_for(SESSION, seq, TAB_A, BLOCK_B)}),
            )
            .await;
        }
        let fence = next_non_meta(&mut ws, seq).await;
        assert_eq!(fence["method"], "test.sentinel");
        reply(&mut ws, &fence, seq + 1, json!({})).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    let epoch = host.model.lock().unwrap().connection_epoch;
    for (idx, (scope, cursor, _)) in cases.iter().enumerate() {
        let id = (idx + 1) as u64;
        let expected_rev = 1 + 2 * id;
        host.commands
            .send(HostCommand::ReverseFocusHistory {
                epoch,
                id,
                scope: *scope,
                cursor: *cursor,
                deadline: future_deadline(),
            })
            .unwrap();
        until(&host, |m| {
            m.focus_history_result.as_ref().is_some_and(|r| r.id == id)
                && m.view().is_some_and(|v| v.revision >= expected_rev)
        })
        .await;
        let m = host.model.lock().unwrap();
        assert_eq!(
            m.focus_history_result,
            Some(FocusHistoryResult {
                epoch,
                id,
                reversed: true,
                cursor: Some(100 - idx as u64),
                session_id: Some(SESSION.into()),
                window_id: Some(TAB_A.into()),
                block_id: Some(BLOCK_B.into()),
            })
        );
        assert_eq!(m.focused(), Some(BLOCK_B));
    }
    send_sentinel(&host);
    tokio::time::timeout(Duration::from_secs(8), server)
        .await
        .unwrap()
        .unwrap();
    drop(host);
}

#[tokio::test]
async fn focus_history_cross_session_switches_selected_attaches_and_reversed_false_keeps_selection()
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 21).await;
        let req = recv(&mut ws).await;
        assert_eq!(req["method"], "session.bootstrap");
        let mut initial = serde_json::to_value(boot()).unwrap();
        initial["selected_session"]["view"] = view_for(SESSION, 1, TAB_A, BLOCK_A);
        reply(&mut ws, &req, 1, initial).await;

        // 1. Cross-session reverse returns OTHER (not yet listed or attached).
        let rev1 = next_non_meta(&mut ws, 1).await;
        assert_eq!(rev1["method"], "focus_history.reverse");
        assert_eq!(rev1["payload"], json!({"not_session_id": SESSION}));
        reply(
            &mut ws,
            &rev1,
            2,
            json!({
                "reversed": true,
                "cursor": 12,
                "session_id": OTHER,
                "window_id": OTHER_TAB,
                "block_id": OTHER_BLOCK
            }),
        )
        .await;
        let list_req = next_non_meta(&mut ws, 2).await;
        assert_eq!(list_req["method"], "session.attach_all");
        reply(
            &mut ws,
            &list_req,
            3,
            json!({"sessions": [
                {"session_id": SESSION, "label": "snapshot"},
                {"session_id": OTHER, "label": "other"}
            ]}),
        )
        .await;
        // Worker attaches OTHER to show it; read explicitly without swallowing via next_non_meta.
        let att = loop {
            let req = recv(&mut ws).await;
            match req["method"].as_str().unwrap_or("") {
                "com.superlogical.terminal.program_status" => {
                    reply(&mut ws, &req, 3, json!({"records": []})).await;
                }
                "com.superlogical.terminal.title" => {
                    reply(&mut ws, &req, 3, json!({"title": ""})).await;
                }
                "com.superlogical.terminal.process" => {
                    reply(&mut ws, &req, 3, json!({})).await;
                }
                _ => break req,
            }
        };
        assert_eq!(att["method"], "session.attach");
        assert_eq!(att["payload"]["session_id"], OTHER);
        reply(
            &mut ws,
            &att,
            4,
            json!({"client_id": "client:test", "view": view_for(OTHER, 2, OTHER_TAB, OTHER_BLOCK)}),
        )
        .await;

        // 2. Switching back to already-attached SESSION via Select uses session.view (records focus).
        let sel = next_non_meta(&mut ws, 4).await;
        assert_eq!(sel["method"], "session.view");
        assert_eq!(sel["payload"]["session_id"], SESSION);
        reply(&mut ws, &sel, 5, view_for(SESSION, 4, TAB_A, BLOCK_A)).await;

        // 3. Reversed == false leaves selected session intact and notifies calmly.
        let rev2 = next_non_meta(&mut ws, 5).await;
        assert_eq!(rev2["method"], "focus_history.reverse");
        assert_eq!(rev2["payload"], json!({}));
        reply(&mut ws, &rev2, 6, json!({"reversed": false})).await;

        let fence = next_non_meta(&mut ws, 6).await;
        assert_eq!(fence["method"], "test.sentinel");
        reply(&mut ws, &fence, 7, json!({})).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    let epoch = host.model.lock().unwrap().connection_epoch;

    // Cross-session reverse switches selection to OTHER and loads its view.
    host.commands
        .send(HostCommand::ReverseFocusHistory {
            epoch,
            id: 1,
            scope: FocusHistoryScope::Session,
            cursor: None,
            deadline: future_deadline(),
        })
        .unwrap();
    until(&host, |m| {
        m.focus_history_result.as_ref().is_some_and(|r| r.id == 1)
            && m.selected.as_deref() == Some(OTHER)
    })
    .await;
    {
        let m = host.model.lock().unwrap();
        assert_eq!(m.selected.as_deref(), Some(OTHER));
        assert!(!m.background_views.contains(OTHER));
        assert_eq!(m.focused(), Some(OTHER_BLOCK));
        assert_eq!(m.notice, None);
        assert_eq!(
            m.focus_history_result.as_ref().and_then(|r| r.cursor),
            Some(12)
        );
    }

    // Select already-attached SESSION -> server sees session.view.
    host.commands
        .send(HostCommand::Select(SESSION.into()))
        .unwrap();
    until(&host, |m| m.selected.as_deref() == Some(SESSION)).await;

    // reversed: false keeps SESSION selected and reports calm notice.
    host.commands
        .send(HostCommand::ReverseFocusHistory {
            epoch,
            id: 2,
            scope: FocusHistoryScope::Any,
            cursor: None,
            deadline: future_deadline(),
        })
        .unwrap();
    until(&host, |m| {
        m.focus_history_result.as_ref().is_some_and(|r| r.id == 2)
    })
    .await;
    {
        let m = host.model.lock().unwrap();
        assert_eq!(m.selected.as_deref(), Some(SESSION));
        assert_eq!(m.notice.as_deref(), Some(FOCUS_HISTORY_EMPTY_NOTICE));
        assert_eq!(
            m.focus_history_result,
            Some(FocusHistoryResult::not_reversed(epoch, 2))
        );
    }

    send_sentinel(&host);
    tokio::time::timeout(Duration::from_secs(8), server)
        .await
        .unwrap()
        .unwrap();
    drop(host);
}

#[tokio::test]
async fn focus_history_stale_epoch_ignored_and_disconnect_clears_without_replay() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (in_flight_tx, in_flight) = tokio::sync::oneshot::channel();
    let (drop_tx, drop_control) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 21).await;
        let req = recv(&mut ws).await;
        assert_eq!(req["method"], "session.bootstrap");
        reply(&mut ws, &req, 1, boot_two_sessions()).await;

        // First reverse completes so focus_history_result is populated.
        let rev0 = next_non_meta(&mut ws, 1).await;
        assert_eq!(rev0["method"], "focus_history.reverse");
        reply(&mut ws, &rev0, 2, json!({"reversed": false})).await;

        // Second reverse hangs until the control socket drops; a third queued behind it
        // must be discarded before reconnect, never replayed.
        let rev1 = next_non_meta(&mut ws, 2).await;
        assert_eq!(rev1["method"], "focus_history.reverse");
        in_flight_tx.send(()).unwrap();
        drop_control.await.unwrap();
        drop(ws);

        // Reconnect: neither queued rev2 nor a stale-epoch rev3 may be sent.
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 21).await;
        let req = recv(&mut ws).await;
        assert_eq!(req["method"], "session.bootstrap");
        reply(&mut ws, &req, 1, boot_two_sessions()).await;
        let fence = next_non_meta(&mut ws, 1).await;
        assert_eq!(
            fence["method"], "test.sentinel",
            "reconnected host must never replay queued/stale focus_history.reverse, got: {fence}"
        );
        reply(&mut ws, &fence, 2, json!({})).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    let original_epoch = host.model.lock().unwrap().connection_epoch;

    // Populate focus_history_result before disconnect.
    host.commands
        .send(HostCommand::ReverseFocusHistory {
            epoch: original_epoch,
            id: 1,
            scope: FocusHistoryScope::Any,
            cursor: None,
            deadline: future_deadline(),
        })
        .unwrap();
    until(&host, |m| {
        m.focus_history_result.as_ref().is_some_and(|r| r.id == 1)
    })
    .await;

    // Send one in-flight reverse, queue another behind it, then drop the socket.
    host.commands
        .send(HostCommand::ReverseFocusHistory {
            epoch: original_epoch,
            id: 2,
            scope: FocusHistoryScope::Any,
            cursor: None,
            deadline: future_deadline(),
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(8), in_flight)
        .await
        .unwrap()
        .unwrap();
    host.commands
        .send(HostCommand::ReverseFocusHistory {
            epoch: original_epoch,
            id: 3,
            scope: FocusHistoryScope::Any,
            cursor: Some(5),
            deadline: future_deadline(),
        })
        .unwrap();
    drop_tx.send(()).unwrap();

    // After reconnect, epoch advances and previous focus_history_result is cleared.
    until(&host, |m| {
        m.connected && m.connection_epoch > original_epoch
    })
    .await;
    assert_eq!(host.model.lock().unwrap().focus_history_result, None);

    // A stale-epoch command after reconnect is silently ignored.
    host.commands
        .send(HostCommand::ReverseFocusHistory {
            epoch: original_epoch,
            id: 4,
            scope: FocusHistoryScope::Any,
            cursor: None,
            deadline: future_deadline(),
        })
        .unwrap();
    send_sentinel(&host);
    tokio::time::timeout(Duration::from_secs(8), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(host.model.lock().unwrap().focus_history_result, None);
    drop(host);
}

#[tokio::test]
async fn focus_history_missing_scope_context_sends_no_malformed_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 21).await;
        let req = recv(&mut ws).await;
        assert_eq!(req["method"], "session.bootstrap");
        // Empty server: no selected session or active window.
        let empty_boot = session::BootstrapResponse {
            client_id: "client:test".into(),
            ..Default::default()
        };
        reply(&mut ws, &req, 1, serde_json::to_value(&empty_boot).unwrap()).await;
        let fence = next_non_meta(&mut ws, 1).await;
        assert_eq!(
            fence["method"], "test.sentinel",
            "missing session/window must not send focus_history.reverse, got: {fence}"
        );
        reply(&mut ws, &fence, 2, json!({})).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    let epoch = host.model.lock().unwrap().connection_epoch;
    for (idx, scope) in [
        FocusHistoryScope::Pane,
        FocusHistoryScope::Tab,
        FocusHistoryScope::TabAnySession,
        FocusHistoryScope::Session,
    ]
    .into_iter()
    .enumerate()
    {
        let id = (idx + 1) as u64;
        host.commands
            .send(HostCommand::ReverseFocusHistory {
                epoch,
                id,
                scope,
                cursor: None,
                deadline: future_deadline(),
            })
            .unwrap();
        until(&host, |m| {
            m.focus_history_result.as_ref().is_some_and(|r| r.id == id)
        })
        .await;
        let m = host.model.lock().unwrap();
        assert!(m.connected);
        assert!(m.notice.is_some());
        assert_eq!(
            m.focus_history_result,
            Some(FocusHistoryResult::not_reversed(epoch, id))
        );
    }
    send_sentinel(&host);
    tokio::time::timeout(Duration::from_secs(8), server)
        .await
        .unwrap()
        .unwrap();
    drop(host);
}

#[tokio::test]
async fn focus_history_expired_during_exclusive_action_never_sends_rpc() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (action_started_tx, action_started) = tokio::sync::oneshot::channel();
    let (release_action_tx, release_action) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 21).await;
        let req = recv(&mut ws).await;
        assert_eq!(req["method"], "session.bootstrap");
        let mut initial = serde_json::to_value(boot()).unwrap();
        initial["selected_session"]["view"] = view_for(SESSION, 1, TAB_A, BLOCK_A);
        reply(&mut ws, &req, 1, initial).await;

        let action = next_non_meta(&mut ws, 1).await;
        assert_eq!(action["method"], "client.action");
        action_started_tx.send(()).unwrap();
        release_action.await.unwrap();
        reply(&mut ws, &action, 2, json!({})).await;

        // After the exclusive action finishes, the expired queued reverse MUST NOT be sent;
        // the very next non-meta request must be our sentinel.
        let fence = next_non_meta(&mut ws, 2).await;
        assert_eq!(
            fence["method"], "test.sentinel",
            "expired queued reverse must never send focus_history.reverse, got: {fence}"
        );
        reply(&mut ws, &fence, 3, json!({})).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    let epoch = host.model.lock().unwrap().connection_epoch;

    // Hold an exclusive client.action so subsequent commands queue in deferred_commands.
    host.commands
        .send(HostCommand::Action {
            generation: 1,
            request: Box::new(rexrs::api::client::Action::new("test.hold")),
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(8), action_started)
        .await
        .unwrap()
        .unwrap();

    // Queue a reverse with a short deadline, wait past expiry, then release the action.
    host.commands
        .send(HostCommand::ReverseFocusHistory {
            epoch,
            id: 9,
            scope: FocusHistoryScope::Any,
            cursor: None,
            deadline: Instant::now() + Duration::from_millis(80),
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(160)).await;
    release_action_tx.send(()).unwrap();

    until(&host, |m| {
        m.focus_history_result.as_ref().is_some_and(|r| r.id == 9)
    })
    .await;
    {
        let m = host.model.lock().unwrap();
        assert!(m.connected);
        assert_eq!(m.notice.as_deref(), Some(FOCUS_HISTORY_EXPIRED_NOTICE));
        assert_eq!(
            m.focus_history_result,
            Some(FocusHistoryResult::not_reversed(epoch, 9))
        );
    }
    send_sentinel(&host);
    tokio::time::timeout(Duration::from_secs(8), server)
        .await
        .unwrap()
        .unwrap();
    drop(host);
}

#[tokio::test]
async fn focus_history_detach_removes_attached_and_select_reattaches_then_records() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 21).await;
        let req = recv(&mut ws).await;
        assert_eq!(req["method"], "session.bootstrap");
        let mut initial = serde_json::to_value(boot()).unwrap();
        initial["selected_session"]["view"] = view_for(SESSION, 1, TAB_A, BLOCK_A);
        reply(&mut ws, &req, 1, initial).await;

        let detach = next_non_meta(&mut ws, 1).await;
        assert_eq!(detach["method"], "session.detach");
        assert_eq!(detach["payload"]["session_id"], SESSION);
        reply(&mut ws, &detach, 2, json!({})).await;

        // Next Select(SESSION) must re-attach via session.attach (not skip straight to
        // session.view), then follow with session.view on API 19+ to record focus.
        let att = next_non_meta(&mut ws, 2).await;
        assert_eq!(att["method"], "session.attach");
        assert_eq!(att["payload"]["session_id"], SESSION);
        reply(
            &mut ws,
            &att,
            3,
            json!({"client_id": "client:test", "view": view_for(SESSION, 2, TAB_A, BLOCK_A)}),
        )
        .await;
        let view = next_non_meta(&mut ws, 3).await;
        assert_eq!(view["method"], "session.view");
        assert_eq!(view["payload"]["session_id"], SESSION);
        reply(&mut ws, &view, 4, view_for(SESSION, 2, TAB_A, BLOCK_A)).await;

        let fence = next_non_meta(&mut ws, 4).await;
        assert_eq!(fence["method"], "test.sentinel");
        reply(&mut ws, &fence, 5, json!({})).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;

    host.commands
        .send(HostCommand::Request {
            method: "session.detach".into(),
            payload: json!({"session_id": SESSION}),
        })
        .unwrap();
    host.commands
        .send(HostCommand::Select(SESSION.into()))
        .unwrap();
    send_sentinel(&host);
    tokio::time::timeout(Duration::from_secs(8), server)
        .await
        .unwrap()
        .unwrap();
    drop(host);
}

#[tokio::test]
async fn focus_history_attached_passive_events_use_attach_while_unattached_and_detached_use_view() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (step1_tx, step1_rx) = tokio::sync::oneshot::channel();
    let (step2_go_tx, step2_go_rx) = tokio::sync::oneshot::channel();
    let (step3_go_tx, step3_go_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 21).await;
        let req = recv(&mut ws).await;
        assert_eq!(req["method"], "session.bootstrap");
        reply(&mut ws, &req, 1, boot_two_sessions()).await;

        let next_strict = async |ws: &mut WebSocketStream<TcpStream>, seq: u64| loop {
            let req = recv(ws).await;
            match req["method"].as_str().unwrap_or("") {
                "com.superlogical.terminal.program_status" => {
                    reply(ws, &req, seq, json!({"records": []})).await;
                }
                "com.superlogical.terminal.title" => {
                    reply(ws, &req, seq, json!({"title": ""})).await;
                }
                "com.superlogical.terminal.process" => {
                    reply(ws, &req, seq, json!({})).await;
                }
                _ => return req,
            }
        };

        // 1. Unattached background prefetch for OTHER MUST use session.view (never attach
        //    unopened sessions).
        let pref = next_strict(&mut ws, 1).await;
        assert_eq!(
            pref["method"], "session.view",
            "unattached prefetch must use session.view, got: {pref}"
        );
        assert_eq!(pref["payload"]["session_id"], OTHER);
        reply(
            &mut ws,
            &pref,
            2,
            view_for(OTHER, 1, OTHER_TAB, OTHER_BLOCK),
        )
        .await;

        // 2. Passive session_view_changed on STILL-UNATTACHED OTHER MUST also use session.view
        //    (never silently join unopened session).
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 3,
                "payload": {
                    "type": "session_event",
                    "payload": {
                        "session_id": OTHER,
                        "event": {"type": "session_view_changed", "payload": {"revision": 2}}
                    }
                }
            }),
        )
        .await;
        let ev_unattached = next_strict(&mut ws, 3).await;
        assert_eq!(
            ev_unattached["method"], "session.view",
            "unattached passive refresh must use session.view, got: {ev_unattached}"
        );
        assert_eq!(ev_unattached["payload"]["session_id"], OTHER);
        reply(
            &mut ws,
            &ev_unattached,
            4,
            view_for(OTHER, 2, OTHER_TAB, OTHER_BLOCK),
        )
        .await;

        // 3. Passive block_closed on ALREADY-ATTACHED locally selected SESSION MUST use
        //    session.attach (not session.view) so an idle client never overwrites another
        //    client's focus history.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 5,
                "payload": {
                    "type": "session_event",
                    "payload": {
                        "session_id": SESSION,
                        "event": {"type": "block_closed", "payload": {"block_id": BLOCK_B}}
                    }
                }
            }),
        )
        .await;
        let ev_selected = next_strict(&mut ws, 5).await;
        assert_eq!(
            ev_selected["method"], "session.attach",
            "attached selected block_closed must use session.attach, got: {ev_selected}"
        );
        assert_eq!(ev_selected["payload"]["session_id"], SESSION);
        reply(
            &mut ws,
            &ev_selected,
            6,
            json!({"client_id": "client:test", "view": view_for(SESSION, 2, TAB_A, BLOCK_A)}),
        )
        .await;
        step1_tx.send(()).unwrap();

        // 4. Client deliberately selects OTHER (attaching + recording via session.view),
        //    then selects SESSION back so OTHER is now ALREADY-ATTACHED & offscreen.
        step2_go_rx.await.unwrap();
        let att_other = next_strict(&mut ws, 6).await;
        assert_eq!(att_other["method"], "session.attach");
        assert_eq!(att_other["payload"]["session_id"], OTHER);
        reply(
            &mut ws,
            &att_other,
            7,
            json!({"client_id": "client:test", "view": view_for(OTHER, 2, OTHER_TAB, OTHER_BLOCK)}),
        )
        .await;
        let view_other = next_strict(&mut ws, 7).await;
        assert_eq!(view_other["method"], "session.view");
        assert_eq!(view_other["payload"]["session_id"], OTHER);
        reply(
            &mut ws,
            &view_other,
            8,
            view_for(OTHER, 2, OTHER_TAB, OTHER_BLOCK),
        )
        .await;

        let sel_session = next_strict(&mut ws, 8).await;
        assert_eq!(sel_session["method"], "session.view");
        assert_eq!(sel_session["payload"]["session_id"], SESSION);
        reply(
            &mut ws,
            &sel_session,
            9,
            view_for(SESSION, 2, TAB_A, BLOCK_A),
        )
        .await;

        // 5. Passive session_view_changed on ALREADY-ATTACHED offscreen OTHER MUST use
        //    session.attach (not session.view).
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 10,
                "payload": {
                    "type": "session_event",
                    "payload": {
                        "session_id": OTHER,
                        "event": {"type": "session_view_changed", "payload": {"revision": 3}}
                    }
                }
            }),
        )
        .await;
        let ev_attached_other = next_strict(&mut ws, 10).await;
        assert_eq!(
            ev_attached_other["method"], "session.attach",
            "attached offscreen session_view_changed must use session.attach, got: {ev_attached_other}"
        );
        assert_eq!(ev_attached_other["payload"]["session_id"], OTHER);
        reply(
            &mut ws,
            &ev_attached_other,
            11,
            json!({"client_id": "client:test", "view": view_for(OTHER, 3, OTHER_TAB, OTHER_BLOCK)}),
        )
        .await;

        // 6. Client detaches OTHER; subsequent offscreen session_view_changed on OTHER MUST
        //    use session.view and NOT silently re-attach OTHER.
        step3_go_rx.await.unwrap();
        let det = next_strict(&mut ws, 11).await;
        assert_eq!(det["method"], "session.detach");
        assert_eq!(det["payload"]["session_id"], OTHER);
        reply(&mut ws, &det, 12, json!({})).await;

        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 13,
                "payload": {
                    "type": "session_event",
                    "payload": {
                        "session_id": OTHER,
                        "event": {"type": "session_view_changed", "payload": {"revision": 4}}
                    }
                }
            }),
        )
        .await;
        let ev_detached_other = next_strict(&mut ws, 13).await;
        assert_eq!(
            ev_detached_other["method"], "session.view",
            "detached offscreen session_view_changed must not re-attach, got: {ev_detached_other}"
        );
        assert_eq!(ev_detached_other["payload"]["session_id"], OTHER);
        reply(
            &mut ws,
            &ev_detached_other,
            14,
            view_for(OTHER, 4, OTHER_TAB, OTHER_BLOCK),
        )
        .await;

        let fence = next_strict(&mut ws, 14).await;
        assert_eq!(fence["method"], "test.sentinel");
        reply(&mut ws, &fence, 15, json!({})).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    tokio::time::timeout(Duration::from_secs(8), step1_rx)
        .await
        .unwrap()
        .unwrap();

    // Step 4: attach OTHER deliberately via Select, then return to SESSION.
    step2_go_tx.send(()).unwrap();
    host.commands
        .send(HostCommand::Select(OTHER.into()))
        .unwrap();
    until(&host, |m| m.selected.as_deref() == Some(OTHER)).await;
    host.commands
        .send(HostCommand::Select(SESSION.into()))
        .unwrap();
    until(&host, |m| {
        m.selected.as_deref() == Some(SESSION)
            && m.views.get(OTHER).is_some_and(|v| v.revision >= 3)
    })
    .await;

    // Step 6: detach OTHER, verify subsequent offscreen refresh advances rev without re-attaching.
    step3_go_tx.send(()).unwrap();
    host.commands
        .send(HostCommand::Request {
            method: "session.detach".into(),
            payload: json!({"session_id": OTHER}),
        })
        .unwrap();
    until(&host, |m| {
        m.views.get(OTHER).is_some_and(|v| v.revision >= 4)
    })
    .await;
    send_sentinel(&host);
    tokio::time::timeout(Duration::from_secs(8), server)
        .await
        .unwrap()
        .unwrap();
    drop(host);
}

#[tokio::test]
async fn focus_history_followup_attach_bounded_by_shared_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (attach_seen_tx, attach_seen_rx) = tokio::sync::oneshot::channel::<()>();
    let (hold_done_tx, hold_done_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 21).await;
        let req = recv(&mut ws).await;
        assert_eq!(req["method"], "session.bootstrap");
        reply(&mut ws, &req, 1, boot_two_sessions()).await;

        let rev = next_non_meta(&mut ws, 1).await;
        assert_eq!(rev["method"], "focus_history.reverse");
        reply(
            &mut ws,
            &rev,
            2,
            json!({
                "reversed": true,
                "cursor": 77,
                "session_id": OTHER,
                "window_id": OTHER_TAB,
                "block_id": OTHER_BLOCK
            }),
        )
        .await;

        // Receive follow-up session.attach for OTHER and deliberately hold the reply past
        // the caller's deadline to verify the worker does not hang for a fresh 8s timeout.
        let att = loop {
            let req = recv(&mut ws).await;
            match req["method"].as_str().unwrap_or("") {
                "com.superlogical.terminal.program_status" => {
                    reply(&mut ws, &req, 2, json!({"records": []})).await;
                }
                "com.superlogical.terminal.title" => {
                    reply(&mut ws, &req, 2, json!({"title": ""})).await;
                }
                "com.superlogical.terminal.process" => {
                    reply(&mut ws, &req, 2, json!({})).await;
                }
                "session.view" if req["payload"]["session_id"] == OTHER => {
                    reply(&mut ws, &req, 2, view_for(OTHER, 1, OTHER_TAB, OTHER_BLOCK)).await;
                }
                _ => break req,
            }
        };
        assert_eq!(att["method"], "session.attach");
        assert_eq!(att["payload"]["session_id"], OTHER);
        let _ = attach_seen_tx.send(());
        let _ = hold_done_rx.await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    let epoch = host.model.lock().unwrap().connection_epoch;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(1);
    host.commands
        .send(HostCommand::ReverseFocusHistory {
            epoch,
            id: 21,
            scope: FocusHistoryScope::Session,
            cursor: None,
            deadline,
        })
        .unwrap();

    // Confirm the mock server received follow-up session.attach before the shared deadline,
    // and verify FocusHistoryResult (reversed: true, cursor: 77) is already published while
    // follow-up attach is still pending (so UI deadline expiry can never lose the cursor).
    tokio::time::timeout(Duration::from_secs(4), attach_seen_rx)
        .await
        .expect("timed out waiting for follow-up session.attach")
        .expect("attach_seen channel closed");
    let expected_result = Some(FocusHistoryResult {
        epoch,
        id: 21,
        reversed: true,
        cursor: Some(77),
        session_id: Some(OTHER.into()),
        window_id: Some(OTHER_TAB.into()),
        block_id: Some(OTHER_BLOCK.into()),
    });
    {
        let m = host.model.lock().unwrap();
        assert_eq!(m.focus_history_result, expected_result);
        assert_eq!(m.selected.as_deref(), Some(SESSION));
        assert_eq!(m.notice, None);
    }

    // Follow-up attach must time out promptly at the shared 1s deadline (well under 8s).
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if host.model.lock().unwrap().notice.as_deref()
                == Some(FOCUS_HISTORY_SYNC_TIMEOUT_NOTICE)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("follow-up attach exceeded shared reverse deadline");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "elapsed {:?} should stay bounded by shared deadline (below 8s CALL_TIMEOUT)",
        started.elapsed()
    );
    {
        let m = host.model.lock().unwrap();
        assert!(m.connected);
        assert_eq!(
            m.selected.as_deref(),
            Some(SESSION),
            "m.selected must stay preserved when follow-up target sync times out"
        );
        assert_eq!(m.notice.as_deref(), Some(FOCUS_HISTORY_SYNC_TIMEOUT_NOTICE));
        assert_eq!(m.focus_history_result, expected_result);
    }
    let _ = hold_done_tx.send(());
    drop(host);
    tokio::time::timeout(Duration::from_secs(8), server)
        .await
        .unwrap()
        .unwrap();
}
