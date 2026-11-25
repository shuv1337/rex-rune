//! End-to-end tests for pipelined terminal input, coalesced motion/resize, and batched
//! metadata/theme reads on high-latency connections.
use super::*;

fn empty_snapshot() -> Vec<u8> {
    let source = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
        cols: 20,
        rows: 4,
        max_scrollback: 1 << 12,
    })
    .unwrap();
    ghostty_vt::snapshot::encode(&source).unwrap().to_vec()
}

fn spawn_data_server(mut ws: WebSocketStream<TcpStream>) {
    tokio::spawn(async move {
        let Some(Ok(Message::Binary(hello))) = ws.next().await else {
            return;
        };
        if hello.len() != 17 || hello[0] != 1 {
            return;
        }
        let mut welcome = vec![0u8; 32];
        welcome[0] = 0x81;
        welcome[24] = 1;
        if ws.send(Message::Binary(welcome.into())).await.is_err() {
            return;
        }
        let _ = ws.send(Message::Binary(empty_snapshot().into())).await;
        // Keep the data socket open until the client closes.
        while let Some(Ok(frame)) = ws.next().await {
            if matches!(frame, Message::Close(_)) {
                break;
            }
        }
    });
}

fn key_payload(key: &str, text: &str, action: &str) -> Value {
    json!({
        "key": key,
        "mods": "",
        "consumed_mods": "",
        "text": text,
        "unshifted_codepoint": text.chars().next().map_or(0, |c| c as u32),
        "composing": false,
        "action": action,
    })
}

fn send_term(host: &HostConnection, block: &str, command: Command) {
    host.commands
        .send(HostCommand::Terminal {
            session: SESSION.into(),
            block: block.into(),
            command,
        })
        .unwrap();
}

async fn wait_pane_live(host: &HostConnection, block: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let live = {
                let m = host.model.lock().unwrap();
                m.terminals
                    .get(block)
                    .is_some_and(|t| t.model.lock().unwrap().connected)
            };
            if live {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("pane stream did not become live");
}

type Ws = WebSocketStream<TcpStream>;
const BLOCK: &str = "block:a";
const WAIT: Duration = Duration::from_secs(5);

fn pane_boot(ids: &[&str]) -> Value {
    let mut value = json!(boot());
    let blocks = ids
        .iter()
        .map(|id| json!({"block_id":id,"creator_name":activity::TERMINAL,"owner":"client:test"}))
        .collect::<Vec<_>>();
    value["selected_session"]["view"]["windows"] = json!([{"window_id":"w1","focused_block_id":ids[0],"layers":[{"kind":"tiled","bounds":[0,0,1,1],"layout":{"block_id":ids[0]},"blocks":blocks}]}]);
    value["data_tickets"] = json!(ids.iter().map(|id| json!({"session_id":SESSION,"block_id":id,"data_ref_id":"terminal","subprotocol":wire::PROTOCOL,"ticket":"AAAAAAAAAAAAAAAAAAAAAA","expires_in_ms":60000})).collect::<Vec<_>>());
    value
}

async fn start_control(listener: &TcpListener) -> Ws {
    let mut control = accept(listener, "rex.control.v1").await;
    welcome(&mut control, 21).await;
    let req = next(&mut control, "session.bootstrap").await;
    reply(&mut control, &req, 1, pane_boot(&[BLOCK])).await;
    for name in ["program_status", "title", "process"] {
        let req = next(&mut control, &format!("{}.{name}", activity::TERMINAL)).await;
        reply(&mut control, &req, 2, json!({})).await;
    }
    control
}

async fn live_pane() -> (TcpListener, HostConnection, Ws) {
    tokio::time::timeout(WAIT, async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = HostConnection::spawn_with_prompts(
            format!("http://{}", listener.local_addr().unwrap()),
            None,
            None,
            None,
            true,
            None,
            "test".into(),
            None,
            true,
        );
        let control = start_control(&listener).await;
        spawn_data_server(accept(&listener, wire::PROTOCOL).await);
        wait_pane_live(&host, BLOCK).await;
        (listener, host, control)
    })
    .await
    .unwrap()
}

async fn next(ws: &mut Ws, method: &str) -> Value {
    let req = tokio::time::timeout(WAIT, recv(ws))
        .await
        .expect("request deadline");
    assert_eq!(req["method"], method);
    req
}

fn key(host: &HostConnection, text: &str) {
    send_term(
        host,
        BLOCK,
        Command::Key(key_payload("key_a", text, "press")),
    );
}

// A directed reply behind earlier commands in the channel proves the worker read them,
// while the server still holds their predecessors. No wall-clock absence assertions.
async fn prompt_barrier(host: &HostConnection, ws: &mut Ws, id: &str) {
    send(ws, json!({"type":"event","seq":3,"payload":{"type":"client_prompt_requested","payload":{
        "call_id":id,"action":"ask","timeout_ms":10000,"prompt":{"kind":"input","title":"Ready?"}
    }}})).await;
    until(host, |m| {
        m.prompt_events
            .iter()
            .any(|e| matches!(&e.kind, prompt::PromptEventKind::Requested(p) if p.call_id == id))
    })
    .await;
    let epoch = host.model.lock().unwrap().connection_epoch;
    host.commands
        .send(HostCommand::PromptReply {
            epoch,
            call_id: id.into(),
            answer: prompt::PromptAnswer::Text("yes".into()),
        })
        .unwrap();
    let req = next(ws, "client.reply").await;
    assert_eq!(req["payload"]["call_id"], id);
    reply(ws, &req, 4, json!({})).await;
}

async fn close(host: HostConnection, ws: &mut Ws) {
    drop(host);
    assert!(matches!(
        tokio::time::timeout(WAIT, ws.next()).await.unwrap(),
        Some(Ok(Message::Close(_)))
    ));
}

#[tokio::test]
async fn rapid_keystrokes_and_events_pipeline_without_waiting_for_prior_replies() {
    let (_listener, host, mut ws) = live_pane().await;
    for (code, ch) in [("key_a", "a"), ("key_b", "b"), ("key_c", "c")] {
        send_term(&host, BLOCK, Command::Key(key_payload(code, ch, "press")));
        send_term(&host, BLOCK, Command::Key(key_payload(code, "", "release")));
    }
    send_term(&host, BLOCK, Command::Paste("xyz".into()));
    send_term(&host, BLOCK, Command::Wheel(json!({"delta_y":-1})));
    send_term(
        &host,
        BLOCK,
        Command::SendKey(key_payload("key_x", "x", "press")),
    );
    let mut held = Vec::new();
    for ch in ["a", "b", "c"] {
        for (action, text) in [("press", ch), ("release", "")] {
            let req = next(&mut ws, "client.key").await;
            assert_eq!(req["payload"]["key"]["action"], action);
            assert_eq!(req["payload"]["key"]["text"], text);
            held.push(req);
        }
    }
    for kind in ["paste", "wheel", "key"] {
        let req = next(&mut ws, "block.event").await;
        assert_eq!(req["payload"]["event"]["type"], kind);
        held.push(req);
    }
    for req in held.iter().rev() {
        reply(&mut ws, req, 5, json!({})).await;
    }
    assert!(host.model.lock().unwrap().connected);
    close(host, &mut ws).await;
}

#[test]
fn adjacent_mouse_motion_and_resize_coalesce_without_reordering_other_input() {
    let (tx, rx) = flume::unbounded();
    let mut deferred = VecDeque::new();
    let motion = |x: u16, button: Option<&str>, mods: &str| {
        let mut payload = json!({"action": "motion", "x": x, "y": 0, "mods": mods});
        if let Some(button) = button {
            payload["button"] = json!(button);
        }
        HostCommand::Terminal {
            session: SESSION.into(),
            block: "block:a".into(),
            command: Command::Mouse(payload),
        }
    };
    let resize = |cols: u16, rows: u16| HostCommand::Terminal {
        session: SESSION.into(),
        block: "block:a".into(),
        command: Command::Resize(cols, rows, 9, 18),
    };
    let press_key = |ch: &str| HostCommand::Terminal {
        session: SESSION.into(),
        block: "block:a".into(),
        command: Command::Key(key_payload("key_a", ch, "press")),
    };

    // 1. Consecutive motions with same button/mods collapse to the newest cell, stopping before a key.
    tx.send(motion(2, None, "")).unwrap();
    tx.send(motion(3, None, "")).unwrap();
    tx.send(press_key("a")).unwrap();
    tx.send(motion(4, Some("left"), "")).unwrap();
    tx.send(motion(5, Some("left"), "ctrl")).unwrap();
    tx.send(HostCommand::Terminal {
        session: SESSION.into(),
        block: "block:a".into(),
        command: Command::Mouse(
            json!({"action": "press", "button": "left", "x": 5, "y": 0, "mods": ""}),
        ),
    })
    .unwrap();
    tx.send(resize(80, 24)).unwrap();
    tx.send(resize(90, 25)).unwrap();
    tx.send(resize(100, 30)).unwrap();
    tx.send(press_key("b")).unwrap();

    let mut first = Command::Mouse(json!({"action": "motion", "x": 1, "y": 0, "mods": ""}));
    coalesce_terminal_command(SESSION, "block:a", &mut first, &mut deferred, &rx);
    match first {
        Command::Mouse(p) => assert_eq!(p["x"], 3, "motions 1..=3 collapsed to 3"),
        _ => panic!("expected mouse motion"),
    }

    // Next command in deferred must be the untouched key 'a'.
    match deferred.pop_front().unwrap() {
        HostCommand::Terminal {
            command: Command::Key(k),
            ..
        } => assert_eq!(k["text"], "a"),
        _ => panic!("expected key 'a' next"),
    }

    // Motion with button="left", mods="" must NOT collapse into motion with mods="ctrl".
    let HostCommand::Terminal {
        mut command,
        session,
        block,
    } = rx.try_recv().unwrap()
    else {
        panic!("expected motion 4");
    };
    coalesce_terminal_command(&session, &block, &mut command, &mut deferred, &rx);
    match command {
        Command::Mouse(p) => assert_eq!((p["x"].as_u64(), p["mods"].as_str()), (Some(4), Some(""))),
        _ => panic!("expected motion 4"),
    }

    // Motion with mods="ctrl" must NOT collapse into a mouse press.
    let HostCommand::Terminal {
        mut command,
        session,
        block,
    } = deferred.pop_front().unwrap()
    else {
        panic!("expected motion 5");
    };
    coalesce_terminal_command(&session, &block, &mut command, &mut deferred, &rx);
    match command {
        Command::Mouse(p) => assert_eq!(p["x"], 5),
        _ => panic!("expected motion 5"),
    }
    match deferred.pop_front().unwrap() {
        HostCommand::Terminal {
            command: Command::Mouse(p),
            ..
        } => assert_eq!(p["action"], "press"),
        _ => panic!("expected mouse press next"),
    }

    // Three adjacent resizes collapse to the final size, leaving key 'b' queued after.
    let HostCommand::Terminal {
        mut command,
        session,
        block,
    } = rx.try_recv().unwrap()
    else {
        panic!("expected first resize");
    };
    coalesce_terminal_command(&session, &block, &mut command, &mut deferred, &rx);
    assert!(matches!(command, Command::Resize(100, 30, 9, 18)));
    match deferred.pop_front().unwrap() {
        HostCommand::Terminal {
            command: Command::Key(k),
            ..
        } => assert_eq!(k["text"], "b"),
        _ => panic!("expected key 'b' after coalesced resize"),
    }
}

#[tokio::test]
async fn capacity_allows_prompt_reply_and_one_tail_reply_releases_one_slot() {
    let (_listener, host, mut ws) = live_pane().await;
    for i in 0..=64 {
        key(&host, &i.to_string());
    }
    let mut held = Vec::new();
    for i in 0..64 {
        let req = next(&mut ws, "client.key").await;
        assert_eq!(req["payload"]["key"]["text"], i.to_string());
        held.push(req);
    }
    prompt_barrier(&host, &mut ws, "full").await;
    reply(&mut ws, &held[63], 5, json!({})).await;
    let last = next(&mut ws, "client.key").await;
    assert_eq!(last["payload"]["key"]["text"], "64");
    close(host, &mut ws).await;
}

#[tokio::test]
async fn perform_received_time_is_fresh_while_input_is_outstanding() {
    let (_listener, host, mut ws) = live_pane().await;
    key(&host, "held");
    next(&mut ws, "client.key").await;

    async fn perform(ws: &mut Ws, host: &HostConnection, id: &str) -> Instant {
        send(
            ws,
            json!({"type":"event","seq":3,"payload":{"type":"client_perform_requested",
            "payload":{"call_id":id,"action":"test.perform"}}}),
        )
        .await;
        until(host, |m| m.performs.iter().any(|p| p.request.call_id == id)).await;
        host.model
            .lock()
            .unwrap()
            .performs
            .iter()
            .find(|p| p.request.call_id == id)
            .unwrap()
            .received
    }

    // Handling the first event marks the loop busy. A second key on the wire proves
    // the loop has returned to dispatch, without settling either input request.
    perform(&mut ws, &host, "first").await;
    let before_dispatch = Instant::now();
    key(&host, "also held");
    next(&mut ws, "client.key").await;
    assert!(perform(&mut ws, &host, "second").await >= before_dispatch);
    close(host, &mut ws).await;
}

#[tokio::test]
async fn visibility_bypasses_a_full_pipeline_in_both_command_queues() {
    let (_listener, host, mut ws) = live_pane().await;
    // Queue the whole burst before dispatch so motion coalescing must peek the
    // following visibility command into the deferred FIFO as it fills slot 64.
    host.commands
        .send(HostCommand::Request {
            method: "test.barrier".into(),
            payload: json!({}),
        })
        .unwrap();
    let barrier = next(&mut ws, "test.barrier").await;
    for i in 1..input::MAX_IN_FLIGHT {
        key(&host, &i.to_string());
    }
    send_term(
        &host,
        BLOCK,
        Command::Mouse(json!({"action":"motion","x":1,"y":2})),
    );
    host.commands.send(HostCommand::Visibility(false)).unwrap();
    key(&host, "first queued");
    key(&host, "second queued");
    reply(&mut ws, &barrier, 5, json!({})).await;
    let mut held = Vec::new();
    for i in 1..input::MAX_IN_FLIGHT {
        let req = next(&mut ws, "client.key").await;
        assert_eq!(req["payload"]["key"]["text"], i.to_string());
        held.push(req);
    }
    next(&mut ws, "block.event").await;
    until(&host, |m| !m.visible).await;
    // This visibility command comes from the channel, behind the two queued keys.
    host.commands.send(HostCommand::Visibility(true)).unwrap();
    until(&host, |m| m.visible).await;
    prompt_barrier(&host, &mut ws, "still full").await;
    for text in ["first queued", "second queued"] {
        reply(&mut ws, &held.pop().unwrap(), 6, json!({})).await;
        let req = next(&mut ws, "client.key").await;
        assert_eq!(req["payload"]["key"]["text"], text);
    }
    close(host, &mut ws).await;
}

#[tokio::test]
async fn reconnect_discards_pending_and_sent_input() {
    let (listener, host, mut ws) = live_pane().await;
    key(&host, "held");
    next(&mut ws, "client.key").await;
    send_term(&host, BLOCK, Command::Clear);
    key(&host, "queued");
    prompt_barrier(&host, &mut ws, "queued").await;
    drop(ws);
    until(&host, |m| !m.connected).await;
    host.commands.send(HostCommand::Reconnect).unwrap();
    let mut ws = tokio::time::timeout(WAIT, start_control(&listener))
        .await
        .unwrap();
    connected(&host).await;
    key(&host, "fresh");
    let req = next(&mut ws, "client.key").await;
    assert_eq!(req["payload"]["key"]["text"], "fresh");
    close(host, &mut ws).await;
}

#[tokio::test]
async fn resize_waits_for_input_and_coalesces_before_the_next_key() {
    let (_listener, host, mut ws) = live_pane().await;
    key(&host, "old grid");
    let held = next(&mut ws, "client.key").await;
    for cols in [80, 90, 100] {
        send_term(&host, BLOCK, Command::Resize(cols, 24, 8, 16));
    }
    key(&host, "new grid");
    prompt_barrier(&host, &mut ws, "resizing").await;
    reply(&mut ws, &held, 5, json!({})).await;
    let resize = next(&mut ws, "com.superlogical.terminal.resize").await;
    assert_eq!(resize["payload"]["args"]["columns"], 100);
    reply(&mut ws, &resize, 6, json!({})).await;
    let req = next(&mut ws, "client.key").await;
    assert_eq!(req["payload"]["key"]["text"], "new grid");
    close(host, &mut ws).await;
}

#[tokio::test]
async fn dependent_operations_wait_behind_held_keys() {
    for method in [
        "session.destroy",
        "client.undo_close",
        "focus_history.reverse",
        "session.view",
        "session.focus_window",
        "com.superlogical.terminal.clear",
        "com.superlogical.terminal.reset",
    ] {
        let (_listener, host, mut ws) = live_pane().await;
        let epoch = host.model.lock().unwrap().connection_epoch;
        let deadline = Instant::now() + WAIT;
        key(&host, "held");
        let held = next(&mut ws, "client.key").await;
        let cmd = match method {
            "session.destroy" => HostCommand::Close {
                kind: HumanClose::Session,
                payload: json!({"session_id":SESSION}),
            },
            "client.undo_close" => HostCommand::UndoClose { epoch },
            "focus_history.reverse" => HostCommand::ReverseFocusHistory {
                epoch,
                id: 1,
                scope: FocusHistoryScope::Any,
                cursor: None,
                deadline,
            },
            "session.view" => HostCommand::Select(SESSION.into()),
            "session.focus_window" => HostCommand::Navigate {
                id: 1,
                session: SESSION.into(),
                window: Some("w1".into()),
                deadline,
            },
            name => HostCommand::Terminal {
                session: SESSION.into(),
                block: BLOCK.into(),
                command: if name.ends_with(".clear") {
                    Command::Clear
                } else {
                    Command::Reset
                },
            },
        };
        host.commands.send(cmd).unwrap();
        prompt_barrier(&host, &mut ws, method).await;
        reply(&mut ws, &held, 5, json!({})).await;
        let req = next(&mut ws, method).await;
        send(&mut ws, json!({"type":"response","id":req["id"],"error":{"code":"bad_request","message":"test complete"}})).await;
        close(host, &mut ws).await;
    }
}

#[tokio::test]
async fn pipelined_input_api_error_notifies_without_disconnecting() {
    let (_listener, host, mut ws) = live_pane().await;
    let epoch = host.model.lock().unwrap().connection_epoch;
    key(&host, "a");
    key(&host, "b");
    let a = next(&mut ws, "client.key").await;
    let b = next(&mut ws, "client.key").await;
    send(&mut ws, json!({"type":"response","id":a["id"],"error":{"code":"bad_request","message":"lua handler failed"}})).await;
    reply(&mut ws, &b, 3, json!({})).await;
    until(&host, |m| {
        m.notice
            .as_deref()
            .is_some_and(|n| n.contains("lua handler failed"))
    })
    .await;
    key(&host, "c");
    let c = next(&mut ws, "client.key").await;
    assert_eq!(c["payload"]["key"]["text"], "c");
    assert_eq!(host.model.lock().unwrap().connection_epoch, epoch);
    close(host, &mut ws).await;
}

#[tokio::test]
async fn query_activity_pipelines_multi_pane_metadata_in_one_round_trip() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 21).await;
        let boot_req = recv(&mut ws).await;
        let mut boot_val = serde_json::to_value(boot()).unwrap();
        boot_val["selected_session"]["view"] = json!({
            "session_id": SESSION,
            "label": "snapshot",
            "revision": 1,
            "windows": [{
                "window_id": "w1",
                "layers": [{
                    "kind": "tiled",
                    "layout": {"block_id": "block:a"},
                    "blocks": [
                        {"block_id": "block:a", "creator_name": activity::TERMINAL},
                        {"block_id": "block:b", "creator_name": activity::TERMINAL},
                        {"block_id": "block:c", "creator_name": activity::TERMINAL}
                    ]
                }]
            }]
        });
        reply(&mut ws, &boot_req, 1, boot_val).await;

        // Receive ALL 9 metadata requests before sending ANY reply!
        let mut reqs = Vec::with_capacity(9);
        for _ in 0..9 {
            let req = tokio::time::timeout(Duration::from_secs(2), recv(&mut ws))
                .await
                .expect("all 9 metadata requests must be pipelined");
            reqs.push(req);
        }
        let expected_order: Vec<(&str, &str)> = ["block:a", "block:b", "block:c"]
            .into_iter()
            .flat_map(|b| {
                [
                    ("com.superlogical.terminal.program_status", b),
                    ("com.superlogical.terminal.title", b),
                    ("com.superlogical.terminal.process", b),
                ]
            })
            .collect();
        let actual_order: Vec<(&str, &str)> = reqs
            .iter()
            .map(|r| {
                (
                    r["method"].as_str().unwrap(),
                    r["payload"]["block_id"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(actual_order, expected_order);

        for (i, req) in reqs.iter().enumerate() {
            let block = req["payload"]["block_id"].as_str().unwrap();
            let payload = match req["method"].as_str().unwrap() {
                "com.superlogical.terminal.title" => json!({"title": format!("title-{block}")}),
                "com.superlogical.terminal.process" => json!({"foreground": {"name": "vim"}}),
                _ => json!({"records": []}),
            };
            reply(&mut ws, req, 2 + i as u64, payload).await;
        }
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    until(&host, |m| {
        m.title("block:a") == Some("title-block:a")
            && m.title("block:b") == Some("title-block:b")
            && m.title("block:c") == Some("title-block:c")
            && m.programs.get("block:c").map(String::as_str) == Some("vim")
    })
    .await;
    drop(host);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn inconclusive_probe_stays_serial_until_success_then_themes_use_chunks_of_eight() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = HostConnection::spawn(
        format!("http://{}", listener.local_addr().unwrap()),
        None,
        None,
        None,
        true,
        Some(SetTheme::new().background("#111111")),
    );
    let mut ws = accept(&listener, "rex.control.v1").await;
    welcome(&mut ws, 21).await;
    let req = next(&mut ws, "session.bootstrap").await;
    let blocks = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"];
    reply(&mut ws, &req, 1, pane_boot(&blocks)).await;
    let data = tokio::spawn(async move {
        for _ in blocks {
            spawn_data_server(accept(&listener, wire::PROTOCOL).await);
        }
    });
    let first = next(&mut ws, SetTheme::NAME).await;
    send(&mut ws, json!({"type":"response","id":first["id"],"error":{"code":"block_not_found","message":"closed"}})).await;
    let second = next(&mut ws, SetTheme::NAME).await;
    assert!(!host.model.lock().unwrap().theme_declared);
    // A disabled prompt is answered by the worker even while a batch is outstanding.
    async fn probe_barrier(ws: &mut Ws, id: &str) {
        send(ws,json!({"type":"event","seq":3,"payload":{"type":"client_prompt_requested","payload":{
            "call_id":id,"action":"ask","timeout_ms":10000,"prompt":{"kind":"input","title":"Probe"}
        }}})).await;
        let req = next(ws, "client.reply").await;
        assert_eq!(req["payload"]["error"]["code"], "not_supported");
        reply(ws, &req, 3, json!({})).await;
    }
    probe_barrier(&mut ws, "serial").await;
    reply(&mut ws, &second, 4, json!({})).await;
    let mut held = Vec::new();
    for _ in 0..8 {
        held.push(next(&mut ws, SetTheme::NAME).await);
    }
    probe_barrier(&mut ws, "eight").await;
    for req in held {
        reply(&mut ws, &req, 5, json!({})).await;
    }
    let last = next(&mut ws, SetTheme::NAME).await;
    reply(&mut ws, &last, 6, json!({})).await;
    for _ in blocks {
        for name in ["program_status", "title", "process"] {
            let req = next(&mut ws, &format!("{}.{name}", activity::TERMINAL)).await;
            reply(&mut ws, &req, 7, json!({})).await;
        }
    }
    tokio::time::timeout(WAIT, data).await.unwrap().unwrap();
    close(host, &mut ws).await;
}

#[tokio::test]
async fn dropped_block_metadata_cannot_publish_late_batch_replies() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    let mut ws = accept(&listener, "rex.control.v1").await;
    welcome(&mut ws, 21).await;
    let req = next(&mut ws, "session.bootstrap").await;
    reply(&mut ws, &req, 1, pane_boot(&[BLOCK])).await;
    let mut held = Vec::new();
    for name in ["program_status", "title", "process"] {
        held.push(next(&mut ws, &format!("{}.{name}", activity::TERMINAL)).await);
    }
    send(
        &mut ws,
        json!({"type":"event","seq":10,"payload":{"type":"event_dropped",
        "payload":{"session_id":SESSION,"event_type":"block_event","block_id":BLOCK}}}),
    )
    .await;
    for (req, value) in held.iter().zip([
        json!({"records":[]}),
        json!({"title":"stale"}),
        json!({"foreground":{"name":"stale"}}),
    ]) {
        reply(&mut ws, req, 11, value).await;
    }
    // Recovery has begun but has not yet learned that the pane is gone.
    let refresh = next(&mut ws, "session.attach").await;
    {
        let m = host.model.lock().unwrap();
        assert!(!m.status_read.contains(BLOCK));
        assert!(!m.titles.contains_key(BLOCK));
        assert!(!m.programs.contains_key(BLOCK));
    }
    reply(
        &mut ws,
        &refresh,
        12,
        json!({"client_id":"client:test","view":{"session_id":SESSION,"revision":2}}),
    )
    .await;
    close(host, &mut ws).await;
}

#[tokio::test]
async fn control_protocol_errors_abort_both_batches() {
    for theme in [None, Some(SetTheme::new())] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let method = if theme.is_some() {
            SetTheme::NAME
        } else {
            "com.superlogical.terminal.program_status"
        };
        let host = HostConnection::spawn(endpoint, None, None, None, theme.is_some(), theme);
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 21).await;
        let req = next(&mut ws, "session.bootstrap").await;
        reply(&mut ws, &req, 1, pane_boot(&[BLOCK])).await;
        next(&mut ws, method).await;
        send(&mut ws, json!({"type":"welcome","protocol":"rex.control.v1","api_revision":21,
            "limits":{"max_message_bytes":65536},"client":{"id":"duplicate","reconnect_token":"test-token"}})).await;
        until(&host, |m| {
            !m.connected && m.status.contains("second welcome")
        })
        .await;
        assert!(!host.model.lock().unwrap().theme_declared);
        drop(host);
    }
}
