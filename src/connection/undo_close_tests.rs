//! Human close and undo on the production host worker, over mock control and data sockets.
use super::*;

const BLOCK: &str = "block:reopen";
const TAB: &str = "reopen-tab";

fn close(host: &HostConnection, kind: HumanClose, target: Value) {
    let mut payload = target;
    payload["session_id"] = json!(SESSION);
    host.commands
        .send(HostCommand::Close {
            kind,
            payload,
            epoch: None,
        })
        .unwrap();
}

fn epoch(host: &HostConnection) -> u64 {
    host.model.lock().unwrap().connection_epoch
}

fn undo(host: &HostConnection) {
    host.commands
        .send(HostCommand::UndoClose { epoch: epoch(host) })
        .unwrap();
}

async fn error(ws: &mut WebSocketStream<TcpStream>, req: &Value, code: &str, status: u16) {
    send(
        ws,
        json!({"type":"response","id":req["id"],
        "error":{"code":code,"message":"test refusal","status":status}}),
    )
    .await;
}

fn reopen_view(revision: u64, pane: bool, tab: bool) -> Value {
    let mut view = json!({"session_id":SESSION,"revision":revision,"active_window_id":"stay",
        "windows":[{"window_id":"stay","layers":[]}]});
    if tab {
        view["active_window_id"] = json!(TAB);
        let mut window = json!({"window_id":TAB,"layers":[]});
        if pane {
            window["focused_block_id"] = json!(BLOCK);
            window["layers"] = json!([{"kind":"tiled","layout":{"block_id":BLOCK},
                "blocks":[{"block_id":BLOCK,"creator_name":activity::TERMINAL}]}]);
        }
        view["windows"].as_array_mut().unwrap().push(window);
    }
    view
}

async fn fresh_streams(listener: TcpListener) {
    let snapshot = {
        let mut vt = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
            cols: 24,
            rows: 4,
            max_scrollback: 4096,
        })
        .unwrap();
        vt.vt_write(b"fresh stream\r\n");
        ghostty_vt::snapshot::encode(&vt).unwrap().to_vec()
    };
    for nonce in 1..=4 {
        let mut ws = accept(&listener, wire::PROTOCOL).await;
        let hello = ws.next().await.unwrap().unwrap().into_data();
        assert_eq!((hello.len(), hello[0]), (17, 1), "must open, not resume");
        let mut welcome = vec![0; 32];
        welcome[0] = 0x81;
        welcome[24] = nonce;
        ws.send(Message::Binary(welcome.into())).await.unwrap();
        ws.send(Message::Binary(snapshot.clone().into()))
            .await
            .unwrap();
        while let Some(Ok(frame)) = ws.next().await {
            if frame.is_close() {
                break;
            }
        }
    }
}

async fn terminal_at(host: &HostConnection, count: u64, title: &str) -> Arc<Mutex<Model>> {
    until(host, |m| {
        m.connected
            && m.selected.as_deref() == Some(SESSION)
            && m.focused() == Some(BLOCK)
            && m.title(BLOCK) == Some(title)
            && m.status_read.contains(BLOCK)
            && m.protocol.data_connect_requests == count
            && m.terminals.get(BLOCK).is_some_and(|t| {
                let t = t.model.lock().unwrap();
                t.connected && t.restore_generation == 1
            })
    })
    .await;
    host.model.lock().unwrap().terminals[BLOCK].model.clone()
}

#[tokio::test]
async fn undo_close_pane_tab_and_session_restore_focus_metadata_and_data() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        let data = tokio::spawn(fresh_streams(listener));
        welcome(&mut ws, 20).await;
        let req = recv(&mut ws).await;
        assert_eq!(req["method"], "session.bootstrap");
        let mut initial = serde_json::to_value(boot()).unwrap();
        initial["selected_session"]["view"] = reopen_view(1, true, true);
        initial["attach_all"]["sessions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"session_id":OTHER,"label":"other"}));
        reply(&mut ws, &req, 1, initial).await;
        let mut seq = 1;
        let mut revision = 1;
        let mut held = None;
        let mut restores = 0;
        let mut follow = Vec::new();
        let mut process_reads = 0;
        while let Some(Ok(Message::Text(text))) = ws.next().await {
            let req: Value = serde_json::from_str(&text).unwrap();
            let payload = &req["payload"];
            let method = req["method"].as_str().unwrap();
            seq += 1;
            let response = match method {
                "block.close" | "session.close_window" | "session.destroy" => {
                    assert_eq!(held, None);
                    let target = match method {
                        "block.close" => {
                            json!({"session_id":SESSION,"block_id":BLOCK,"undoable":true})
                        }
                        "session.close_window" => {
                            json!({"session_id":SESSION,"window_id":TAB,"undoable":true})
                        }
                        _ => json!({"session_id":SESSION,"undoable":true}),
                    };
                    assert_eq!(*payload, target);
                    held = Some(method.to_owned());
                    revision += 1;
                    // The server does not send a control block_closed event for a held pane.
                    json!({})
                }
                "client.undo_close" => {
                    assert_eq!(*payload, json!({}));
                    let Some(kind) = held.take() else {
                        error(&mut ws, &req, "not_found", 404).await;
                        continue;
                    };
                    restores += 1;
                    revision += 1;
                    follow = if kind == "session.destroy" {
                        vec!["session.attach", "session.attach_all"]
                    } else {
                        vec!["session.view"]
                    };
                    json!({"session_id":SESSION,"window_id":TAB,"block_id":BLOCK})
                }
                "session.view" if payload["session_id"] == OTHER => view_of(OTHER),
                "session.view" => {
                    if held.as_deref() == Some("session.destroy") {
                        error(&mut ws, &req, "session_not_found", 404).await;
                        continue;
                    }
                    if !follow.is_empty() {
                        assert_eq!(follow.pop(), Some(method));
                    }
                    reopen_view(
                        revision,
                        held.is_none(),
                        held.as_deref() != Some("session.close_window"),
                    )
                }
                "session.attach_all" => {
                    assert_eq!(follow.pop(), Some(method));
                    json!({"sessions":[{"session_id":SESSION,"label":"snapshot"},
                        {"session_id":OTHER,"label":"other"}]})
                }
                "session.attach" if payload["session_id"] == OTHER => {
                    json!({"client_id":"client:test","view":view_of(OTHER)})
                }
                "session.attach" => {
                    assert_eq!(payload["session_id"], SESSION);
                    if held.as_deref() == Some("session.destroy") {
                        error(&mut ws, &req, "session_not_found", 404).await;
                        continue;
                    }
                    if !follow.is_empty() {
                        assert_eq!(follow.pop(), Some(method));
                    }
                    json!({
                        "client_id": "client:test",
                        "view": reopen_view(
                            revision,
                            held.is_none(),
                            held.as_deref() != Some("session.close_window"),
                        )
                    })
                }
                "data.connect" => {
                    assert_eq!(payload["block_id"], BLOCK);
                    json!({"ticket":"AAAAAAAAAAAAAAAAAAAAAA"})
                }
                "com.superlogical.terminal.program_status" => json!({"records":[]}),
                "com.superlogical.terminal.title" => json!({"title":format!("read {restores}")}),
                "com.superlogical.terminal.process" => {
                    process_reads += 1;
                    json!({})
                }
                "test.sentinel" => json!({}),
                _ => panic!("unexpected request: {req}"),
            };
            reply(&mut ws, &req, seq, response).await;
        }
        assert_eq!(restores, 3);
        assert_eq!(process_reads, 4);
        data.await.unwrap();
    });
    let host = HostConnection::spawn(endpoint, None, None, None, true, None);
    let mut previous = terminal_at(&host, 1, "read 0").await;
    for (index, kind, payload) in [
        (1, HumanClose::Pane, json!({"block_id":BLOCK})),
        (2, HumanClose::Tab, json!({"window_id":TAB})),
        (3, HumanClose::Session, json!({})),
    ] {
        close(&host, kind, payload);
        until(&host, |m| {
            !m.terminals.contains_key(BLOCK)
                && m.title(BLOCK).is_none()
                && !m.status_read.contains(BLOCK)
        })
        .await;
        if kind == HumanClose::Session {
            until(&host, |m| {
                !m.sessions.iter().any(|s| s.session_id == SESSION)
            })
            .await;
            host.commands
                .send(HostCommand::Select(OTHER.into()))
                .unwrap();
            until(&host, |m| m.selected.as_deref() == Some(OTHER)).await;
        }
        undo(&host);
        let restored = terminal_at(&host, index + 1, &format!("read {index}")).await;
        assert!(!Arc::ptr_eq(&previous, &restored));
        assert_eq!(
            host.model
                .lock()
                .unwrap()
                .window()
                .unwrap()
                .window_id
                .as_deref(),
            Some(TAB)
        );
        previous = restored;
    }
    undo(&host);
    until(&host, |m| m.notice.as_deref() == Some("Nothing to reopen")).await;
    assert!(host.model.lock().unwrap().connected);
    drop(host);
    tokio::time::timeout(Duration::from_secs(8), server)
        .await
        .unwrap()
        .unwrap();
}

async fn start_mock(revision: u32, ws: &mut WebSocketStream<TcpStream>) {
    welcome(ws, revision).await;
    let req = recv(ws).await;
    assert_eq!(req["method"], "session.bootstrap");
    reply(ws, &req, 1, json!(boot())).await;
}

async fn sentinel(ws: &mut WebSocketStream<TcpStream>) {
    let req = next(ws).await;
    assert_eq!(req["method"], "test.sentinel");
    reply(ws, &req, 100, json!({})).await;
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
async fn undo_close_revision_gate_and_automated_close_shape() {
    for revision in [13, 19, 20] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut ws = accept(&listener, "rex.control.v1").await;
            start_mock(revision, &mut ws).await;
            let kinds = [HumanClose::Pane, HumanClose::Tab, HumanClose::Session];
            for (seq, kind) in (2..).zip(kinds) {
                let req = next(&mut ws).await;
                assert_eq!(req["method"], kind.method());
                assert_eq!(req["payload"].get("undoable"), None, "API {revision}");
                reply(&mut ws, &req, seq, json!({})).await;
                let view = next(&mut ws).await;
                if revision >= FOCUS_HISTORY_MIN_API_REVISION {
                    assert_eq!(view["method"], "session.attach");
                    reply(
                        &mut ws,
                        &view,
                        seq,
                        json!({"client_id": "client:test", "view": view_of(SESSION)}),
                    )
                    .await;
                } else {
                    assert_eq!(view["method"], "session.view");
                    reply(&mut ws, &view, seq, view_of(SESSION)).await;
                }
            }
            if revision == 20 {
                let req = next(&mut ws).await;
                assert_eq!(req["method"], "client.undo_close");
                error(&mut ws, &req, "conflict", 409).await;
            }
            sentinel(&mut ws).await;
        });
        let host = HostConnection::spawn(endpoint, None, None, None, false, None);
        connected(&host).await;
        for (kind, payload) in [
            (HumanClose::Pane, json!({"block_id":BLOCK})),
            (HumanClose::Tab, json!({"window_id":TAB})),
            (HumanClose::Session, json!({})),
        ] {
            if revision < 20 {
                close(&host, kind, payload);
            } else {
                let mut payload = payload;
                payload["session_id"] = json!(SESSION);
                host.commands
                    .send(HostCommand::Request {
                        method: kind.method().into(),
                        payload,
                    })
                    .unwrap();
            }
        }
        undo(&host);
        until(&host, |m| {
            m.connected
                && m.notice
                    .as_deref()
                    .is_some_and(|n| n == UNDO_CLOSE_REQUIRES_API || n.contains("test refusal"))
        })
        .await;
        send_sentinel(&host);
        tokio::time::timeout(Duration::from_secs(8), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn undo_close_stale_and_disconnected_commands_are_never_replayed() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (in_flight_tx, in_flight) = tokio::sync::oneshot::channel();
    let (drop_tx, drop_control) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        start_mock(20, &mut ws).await;
        let req = next(&mut ws).await;
        assert_eq!(req["method"], "client.undo_close");
        in_flight_tx.send(()).unwrap();
        drop_control.await.unwrap();
        drop(ws);
        let mut ws = accept(&listener, "rex.control.v1").await;
        start_mock(20, &mut ws).await;
        sentinel(&mut ws).await;
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    let original = epoch(&host);
    undo(&host);
    tokio::time::timeout(Duration::from_secs(8), in_flight)
        .await
        .unwrap()
        .unwrap();
    undo(&host);
    drop_tx.send(()).unwrap();
    until(&host, |m| m.connected && m.connection_epoch > original).await;
    host.commands
        .send(HostCommand::UndoClose { epoch: original })
        .unwrap();
    send_sentinel(&host);
    tokio::time::timeout(Duration::from_secs(8), server)
        .await
        .unwrap()
        .unwrap();
}
