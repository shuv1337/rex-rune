use super::*;
#[path = "cwd_tests.rs"]
mod cwd_tests;
#[path = "directory_tests.rs"]
mod directory_tests;
// These fixtures use the POSIX download store and Unix symbolic links.
#[cfg(unix)]
#[path = "download_tests.rs"]
mod download_tests;
#[path = "fetch_view_tests.rs"]
mod fetch_view_tests;
#[path = "hosts_tests.rs"]
mod hosts_tests;
#[path = "keymap_tests.rs"]
mod keymap_tests;
#[path = "navigation_tests.rs"]
mod navigation_tests;
#[path = "perform_tests.rs"]
mod perform_tests;
#[path = "server_controls_tests.rs"]
mod server_controls_tests;
#[path = "server_info_tests.rs"]
mod server_info_tests;
#[cfg(unix)]
#[path = "ssh_agent_tests.rs"]
mod ssh_agent_tests;
#[path = "status_sweep_tests.rs"]
mod status_sweep_tests;
#[path = "verb_tests.rs"]
mod verb_tests;
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

#[test]
fn startup_size_prefers_cli_then_saved_size_then_default_and_keeps_the_minimum() {
    use clap::Parser;
    let options = Options::parse_from(["rex-rune"]);
    let legacy: crate::settings::Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(options.window_size, None);
    assert_eq!(options.startup_size(&legacy), (1440., 900.));
    assert!(
        serde_json::to_value(&legacy)
            .unwrap()
            .get("window_size")
            .is_none()
    );
    let mut settings = crate::settings::Settings {
        window_size: Some([1100., 750.]),
        ..legacy
    };
    assert_eq!(options.startup_size(&settings), (1100., 750.));
    for (arg, expected) in [
        ("1280x720", (1280., 720.)),
        ("320x200", (640., 480.)),
        ("invalid", (1100., 750.)),
        ("infx900", (1100., 750.)),
        ("NaNx900", (1100., 750.)),
    ] {
        let options = Options::parse_from(["rex-rune", "--window-size", arg]);
        assert_eq!(options.startup_size(&settings), expected, "{arg}");
    }
    settings.window_size = Some([320., 200.]);
    assert_eq!(options.startup_size(&settings), (640., 480.));
}

const SESSION: &str = "session:00000000-0000-0000-0000-000000000001";

fn ticket(session: &str, block: &str, expires_in_ms: u64) -> session::DataTicket {
    session::DataTicket {
        session_id: session.into(),
        block_id: block.into(),
        data_ref_id: "terminal".into(),
        subprotocol: Some(wire::PROTOCOL.into()),
        ticket: "single-use".into(),
        expires_in_ms,
    }
}

#[test]
fn tickets_are_scoped_expiring_and_single_use() {
    let mut state = ControlState::default();
    let mut wrong = ticket(SESSION, "wrong", 60_000);
    wrong.subprotocol = Some("other".into());
    state.add_tickets(
        vec![
            ticket(SESSION, "valid", 60_000),
            ticket(SESSION, "expired", 0),
            wrong,
        ],
        Instant::now(),
    );
    assert!(state.take_ticket("other-session", "valid").is_none());
    assert!(state.take_ticket(SESSION, "wrong").is_none());
    assert!(state.take_ticket(SESSION, "expired").is_none());
    assert!(state.take_ticket(SESSION, "valid").unwrap().bundled);
    assert!(state.take_ticket(SESSION, "valid").is_none());
    state.add_tickets(
        vec![ticket(SESSION, "slow", 100)],
        Instant::now() - Duration::from_secs(1),
    );
    assert!(state.take_ticket(SESSION, "slow").is_none());
    state.add_tickets(vec![ticket(SESSION, "removed", 60_000)], Instant::now());
    state.prune(&HostModel::default());
    assert!(state.tickets.is_empty());
}

#[test]
fn selection_and_bootstrap_preserve_explicit_intent() {
    let sessions = vec![session::Session {
        session_id: SESSION.into(),
        label: "work".into(),
    }];
    assert_eq!(select_session(&sessions, None).as_deref(), Some(SESSION));
    assert_eq!(
        select_session(&sessions, Some("work")).as_deref(),
        Some(SESSION)
    );
    assert_eq!(
        select_session(&sessions, Some(SESSION)).as_deref(),
        Some(SESSION)
    );
    assert!(select_session(&sessions, Some("missing")).is_none());
    let mut duplicate = sessions.clone();
    duplicate.push(session::Session {
        session_id: "other".into(),
        label: "work".into(),
    });
    assert!(select_session(&duplicate, Some("work")).is_none());
    for (wanted, creating, expected) in [
        (None, false, None),
        (Some("work"), false, Some(json!([]))),
        (Some("session:bad-id"), false, Some(json!([]))),
        (None, true, Some(json!([]))),
        (Some(SESSION), false, Some(json!([SESSION]))),
    ] {
        let request =
            serde_json::to_value(bootstrap_request(wanted, creating, std::iter::empty())).unwrap();
        assert_eq!(request.get("attach_session_ids").cloned(), expected);
        assert_eq!(request["data_tickets"][0]["subprotocol"], wire::PROTOCOL);
    }
    let retained = bootstrap_request(
        Some(SESSION),
        false,
        [SESSION.to_owned(), SESSION.to_owned()].into_iter(),
    );
    assert_eq!(retained.attach_session_ids, Some(vec![SESSION.into()]));
}

#[allow(clippy::result_large_err)] // tungstenite's callback fixes the error type.
async fn accept(listener: &TcpListener, protocol: &str) -> WebSocketStream<TcpStream> {
    let (tcp, _) = listener.accept().await.unwrap();
    tokio_tungstenite::accept_hdr_async(
        tcp,
        |_: &_, mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
            response
                .headers_mut()
                .insert("Sec-WebSocket-Protocol", protocol.parse().unwrap());
            Ok(response)
        },
    )
    .await
    .unwrap()
}

async fn recv(ws: &mut WebSocketStream<TcpStream>) -> Value {
    serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap()
}

async fn send(ws: &mut WebSocketStream<TcpStream>, value: Value) {
    ws.send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
}

async fn welcome(ws: &mut WebSocketStream<TcpStream>, revision: u32) -> Value {
    let hello = recv(ws).await;
    assert_eq!(hello["api_revision"], 21);
    assert_eq!(hello["api_min_revision"], 10);
    send(ws, json!({"type":"welcome", "protocol":"rex.control.v1", "api_revision":revision,
        "limits":{"max_message_bytes":65536}, "client":{"id":"client:test", "reconnect_token":"test-token"}})).await;
    hello
}

fn boot() -> session::BootstrapResponse {
    session::BootstrapResponse {
        client_id: "client:test".into(),
        attach_all: session::AttachAllResponse {
            sessions: vec![session::Session {
                session_id: SESSION.into(),
                label: "snapshot".into(),
            }],
        },
        selected_session: Some(session::SelectedSession {
            session: session::Session {
                session_id: SESSION.into(),
                label: "snapshot".into(),
            },
            view: session::ViewResponse {
                session_id: Some(SESSION.into()),
                label: Some("snapshot".into()),
                revision: 1,
                ..Default::default()
            },
        }),
        ..Default::default()
    }
}

async fn connected(host: &HostConnection) {
    tokio::time::timeout(Duration::from_secs(8), async {
        while !host.model.lock().unwrap().connected {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn bootstrap_is_first_and_newer_pre_response_events_survive() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 10).await;
        let request = recv(&mut ws).await;
        assert_eq!(request["method"], "session.bootstrap");
        // An older destruction must not remove a session present in the snapshot.
        send(&mut ws, json!({"type":"event","seq":9,"payload":{"type":"session_destroyed","payload":{"session_id":SESSION}}})).await;
        // A newer event can arrive BEFORE the bootstrap response.
        send(&mut ws, json!({"type":"event","seq":11,"payload":{"type":"session_event","payload":{
            "session_id":SESSION,"event":{"type":"session_label_changed","payload":{"label":"newer"}}}}})).await;
        send(
            &mut ws,
            json!({"type":"response","id":request["id"],"seq":10,"payload":boot()}),
        )
        .await;
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    {
        let model = host.model.lock().unwrap();
        assert_eq!(model.sessions[0].label, "newer");
        assert_eq!(model.selected.as_deref(), Some(SESSION));
        assert_eq!(model.protocol.bootstrap_requests, 1);
        assert_eq!(model.protocol.attach_requests, 0);
    }
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn unsupported_negotiated_revision_is_rejected() {
    for revision in [9, 22] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut ws = accept(&listener, "rex.control.v1").await;
            welcome(&mut ws, revision).await;
        });
        let error = transport::control(&endpoint, None).await.err().unwrap();
        assert_eq!(
            error.to_string(),
            format!("Rex negotiated API revision {revision}; this client supports 10–21")
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn supported_revisions_negotiate() {
    for revision in 10..=21 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut ws = accept(&listener, "rex.control.v1").await;
            welcome(&mut ws, revision).await;
            assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
        });
        let control = transport::control(&endpoint, None).await.unwrap();
        assert_eq!(control.welcome().api_revision, revision);
        control.close().await.unwrap();
        server.await.unwrap();
    }
}

#[test]
fn snapshot_cutoffs_are_scoped_to_the_state_they_replace() {
    let state = ControlState {
        bootstrap_seq: 10,
        list_seq: 15,
        view_seq: HashMap::from([(SESSION.into(), 20)]),
        ..Default::default()
    };
    let event = |seq, name: &str, session: Option<&str>| rexrs::Event {
        seq,
        name: name.into(),
        session_id: session.map(str::to_owned),
        payload: json!({}),
    };
    assert!(!state.includes(&event(20, "block_closed", Some(SESSION))));
    assert!(state.includes(&event(21, "block_closed", Some(SESSION))));
    assert!(state.includes(&event(19, "block_closed", Some("other-session"))));
    assert!(!state.includes(&event(15, "session_destroyed", None)));
    assert!(state.includes(&event(16, "session_destroyed", Some(SESSION))));
    assert!(!state.includes(&event(19, "session_label_changed", Some(SESSION))));
    assert!(state.includes(&event(21, "session_label_changed", Some(SESSION))));
    // List-level `event_dropped` carrying `payload.session_id` gates against `list_seq`,
    // not `view_seq`.
    let dropped = |seq, event_type: &str, session: Option<&str>| rexrs::Event {
        seq,
        name: "event_dropped".into(),
        session_id: None,
        payload: match session {
            Some(id) => json!({"event_type": event_type, "session_id": id, "size": 100000}),
            None => json!({"event_type": event_type, "size": 100000}),
        },
    };
    assert!(!state.includes(&dropped(15, "session_created", Some(SESSION))));
    assert!(state.includes(&dropped(16, "session_created", Some(SESSION))));
    assert!(!state.includes(&dropped(15, "session_moved", Some(SESSION))));
    assert!(state.includes(&dropped(16, "session_moved", Some(SESSION))));
    assert!(!state.includes(&dropped(15, "session_destroyed", Some(SESSION))));
    assert!(state.includes(&dropped(16, "session_destroyed", Some(SESSION))));
    assert!(!state.includes(&dropped(15, "session_label_changed", Some(SESSION))));
    assert!(state.includes(&dropped(16, "session_label_changed", Some(SESSION))));
    // Only `session_view_changed` drops are superseded by a newer `session.view`;
    // `block_event` and `program_status_removed` drops carry metadata not in `session.view`.
    assert!(!state.includes(&dropped(20, "session_view_changed", Some(SESSION))));
    assert!(state.includes(&dropped(21, "session_view_changed", Some(SESSION))));
    assert!(!state.includes(&dropped(10, "block_event", Some(SESSION))));
    assert!(state.includes(&dropped(20, "block_event", Some(SESSION))));
    assert!(state.includes(&dropped(20, "program_status_removed", Some(SESSION))));
    assert!(state.includes(&dropped(21, "block_event", Some(SESSION))));
}

#[tokio::test]
async fn failed_bootstrap_closes_and_retries_without_old_identity() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut first = accept(&listener, "rex.control.v1").await;
        welcome(&mut first, 11).await;
        let request = recv(&mut first).await;
        assert_eq!(request["method"], "session.bootstrap");
        send(&mut first, json!({"type":"response","id":request["id"],"error":{"code":"session_not_found","message":"closed during setup","status":404}})).await;
        assert!(matches!(first.next().await, Some(Ok(Message::Close(_)))));
        let mut second = accept(&listener, "rex.control.v1").await;
        let hello = welcome(&mut second, 11).await;
        assert!(hello.get("reconnect_token").is_none());
        let request = recv(&mut second).await;
        assert_eq!(request["method"], "session.bootstrap");
        send(
            &mut second,
            json!({"type":"response","id":request["id"],"seq":1,"payload":boot()}),
        )
        .await;
        assert!(matches!(second.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    assert_eq!(host.model.lock().unwrap().protocol.bootstrap_requests, 2);
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn rejected_bundle_gets_fresh_ticket_and_new_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for accepted in [false, true] {
            let mut ws = accept(&listener, wire::PROTOCOL).await;
            let message = ws.next().await.unwrap().unwrap().into_data();
            assert_eq!(message.len(), 17);
            assert_eq!(message[0], 1);
            let mut response = if accepted { vec![0; 32] } else { vec![0x82, 1] };
            if accepted {
                response[0] = 0x81;
                response[24] = 1;
            }
            ws.send(Message::Binary(response.into())).await.unwrap();
        }
    });
    let (sender, commands) = flume::unbounded();
    let host = tokio::spawn(async move {
        for bundled in [true, false] {
            let HostCommand::Ticket {
                session,
                block,
                reply,
            } = commands.recv_async().await.unwrap()
            else {
                panic!("expected ticket");
            };
            assert_eq!(session, SESSION);
            assert_eq!(block, "block");
            assert!(
                reply
                    .send(Ok(TerminalTicket {
                        token: "AAAAAAAAAAAAAAAAAAAAAA".into(),
                        bundled
                    }))
                    .is_ok()
            );
        }
    });
    let (_, welcome) = open_data(&endpoint, SESSION, "block", &sender)
        .await
        .unwrap();
    assert_eq!(welcome.nonce, 1);
    host.await.unwrap();
    server.await.unwrap();
}

/// A data server for one upload: the HELLO, then every byte until the empty end message,
/// closed with `code`.
async fn upload_server(listener: TcpListener, code: u16) -> Vec<u8> {
    use tokio_tungstenite::tungstenite::protocol::{CloseFrame, frame::coding::CloseCode};
    let mut ws = accept(&listener, crate::upload::PROTOCOL).await;
    let hello = ws.next().await.unwrap().unwrap().into_data();
    assert_eq!((hello.len(), hello[0]), (17, 1));
    let mut welcome = vec![0; 32];
    welcome[0] = 0x81;
    welcome[24] = 1;
    ws.send(Message::Binary(welcome.into())).await.unwrap();
    let mut received = Vec::new();
    loop {
        let bytes = ws.next().await.unwrap().unwrap().into_data();
        if bytes.is_empty() {
            break;
        }
        assert!(bytes.len() <= 64 * 1024);
        received.extend_from_slice(&bytes);
    }
    let reason = if code == 1000 { "" } else { "too large" };
    ws.close(Some(CloseFrame {
        code: CloseCode::from(code),
        reason: reason.into(),
    }))
    .await
    .unwrap();
    while ws.next().await.is_some() {}
    received
}

#[tokio::test]
async fn uploads_stream_exact_bytes_end_empty_and_succeed_only_on_a_normal_close() {
    use crate::upload::{File, send};
    let bytes: Vec<u8> = (0..200_000u32).map(|i| i as u8).collect();
    let path = std::env::temp_dir().join(format!("rex-upload-test-{}", std::process::id()));
    std::fs::write(&path, &bytes).unwrap();
    for (file, code) in [
        (File::path(path.clone()), 1000),
        (File::bytes("clipboard.png", bytes.clone()), 1000),
        (File::bytes("clipboard.png", bytes.clone()), 4009),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(upload_server(listener, code));
        let size = file.size().unwrap();
        let result = send(&endpoint, "AAAAAAAAAAAAAAAAAAAAAA", &file, size).await;
        assert_eq!(server.await.unwrap(), bytes);
        match code {
            1000 => result.unwrap(),
            _ => assert!(format!("{:#}", result.unwrap_err()).contains("too large")),
        }
    }
    std::fs::remove_file(path).unwrap();
}

const FIXTURE: &str = include_str!("fixtures/block_events.jsonl");
const FIXTURE_BLOCK: &str = "block:01a19b73-2201-7d4a-9b0b-39967ffa0cd5";

#[test]
fn captured_block_events_update_activity_programs_and_notices() {
    use activity::{BlockedKind, ProgramState};
    let mut model = HostModel::default();
    for line in FIXTURE.lines() {
        let payload: Value = serde_json::from_str(line).unwrap();
        let (block, event) = activity::parse_block_event(&payload).expect("a terminal event");
        assert_eq!(block, FIXTURE_BLOCK);
        model.apply_block_event(SESSION, block, event);
    }
    let records = &model.activity[FIXTURE_BLOCK];
    let ids: Vec<_> = records.keys().map(String::as_str).collect();
    // "kid" reported done, then was removed when the foreground process changed.
    assert_eq!(ids, ["", "build", "build/test"]);
    assert_eq!(records[""].state, ProgramState::Working);
    assert_eq!(records[""].app.as_deref(), Some("claude"));
    assert_eq!(records["build"].state, ProgramState::Blocked);
    assert_eq!(records["build"].kind, Some(BlockedKind::Permission));
    assert_eq!(records["build/test"].state, ProgramState::Error);
    // The shell's argv0 is "/usr/bin/bash"; only its basename shows.
    assert_eq!(model.programs[FIXTURE_BLOCK], "bash");
    let notices: Vec<_> = model
        .notices
        .iter()
        .map(|n| (n.session.as_str(), n.title.as_str(), n.body.as_str()))
        .collect();
    assert_eq!(
        notices,
        [
            (SESSION, "", "hello there"),
            (SESSION, "Title", "Body text")
        ]
    );
}

#[test]
fn a_title_lasts_while_the_program_that_set_it_runs() {
    let mut model = HostModel::default();
    let replay = |model: &mut HostModel, lines: &str| {
        for line in lines.lines() {
            let payload: Value = serde_json::from_str(line).unwrap();
            let (block, event) = activity::parse_block_event(&payload).unwrap();
            model.apply_block_event(SESSION, block, event);
        }
    };
    let block = "block:01a19e0b-91d8-7213-8fda-04f8b42016e6";
    let fixture: Vec<_> = include_str!("fixtures/title_events.jsonl")
        .lines()
        .collect();
    // A program titles the terminal just before the server reports it in the foreground.
    replay(&mut model, fixture[0]);
    replay(&mut model, fixture[1]);
    assert_eq!(model.title(block), Some("OC | USB device exploration"));
    // Back at the prompt, the shell titles it again.
    replay(&mut model, &fixture[2..].join("\n"));
    assert_eq!(model.title(block), Some("shuv@shuvdev:~"));

    // A program that exits without resetting the title leaves a stale one behind.
    let title =
        activity::parse_title(&json!({"title":"nvim notes.md","owner":{"pid":7,"name":"nvim"}}));
    model.apply_block_event(SESSION, "b".into(), title);
    let foreground = |pids: &[u64]| {
        activity::parse_process(&json!({
            "foreground": {"pid": pids[0], "name": "x"},
            "ancestors": pids[1..].iter().map(|p| json!({"pid": p})).collect::<Vec<_>>(),
        }))
    };
    model.apply_block_event(SESSION, "b".into(), foreground(&[7, 1]));
    assert_eq!(model.title("b"), Some("nvim notes.md"));
    // A child of the titling program keeps its title; the shell coming back drops it.
    model.apply_block_event(SESSION, "b".into(), foreground(&[9, 7, 1]));
    assert_eq!(model.title("b"), Some("nvim notes.md"));
    model.apply_block_event(SESSION, "b".into(), foreground(&[1]));
    assert_eq!(model.title("b"), None);
    // A cleared title is gone; an unknown owner can't go stale.
    let clear = activity::parse_title(&json!({"title":""}));
    model.apply_block_event(SESSION, "b".into(), clear);
    assert!(!model.titles.contains_key("b"));
    model.apply_block_event(
        SESSION,
        "b".into(),
        activity::parse_title(&json!({"title":"t"})),
    );
    assert_eq!(model.title("b"), Some("t"));
}

#[test]
fn unknown_states_kinds_and_events_stay_quiet() {
    use activity::{BlockEvent, ProgramState, StatusRecord};
    let record = StatusRecord::parse(&json!({"state":"sleeping","kind":"coffee","app":"x"}));
    assert_eq!(record.state, ProgramState::Idle);
    assert_eq!(record.kind, None);
    assert_eq!(record.app.as_deref(), Some("x"));
    let event = |creator: &str, name: &str| json!({"block_id":"b","creator_name":creator,"name":name,"payload":{}});
    assert!(activity::parse_block_event(&event("other.creator", "process_changed")).is_none());
    assert_eq!(
        activity::parse_block_event(&event(activity::TERMINAL, "bell")),
        Some(("b".into(), BlockEvent::Bell))
    );
    assert_eq!(
        activity::parse_block_event(&event(activity::TERMINAL, "process_changed")),
        Some((
            "b".into(),
            BlockEvent::Process {
                name: None,
                executable: None,
                script: None,
                pids: Vec::new(),
                place: Default::default()
            }
        ))
    );
    assert!(ProgramState::Blocked > ProgramState::Error);
    assert!(ProgramState::Error > ProgramState::Working);
    assert!(ProgramState::Working > ProgramState::Done);
    assert!(ProgramState::Done > ProgramState::Idle);
}

#[test]
fn status_records_keep_their_progress() {
    use activity::StatusRecord;
    let parse = |v: Value| StatusRecord::parse(&v).progress;
    assert_eq!(parse(json!({"state":"working","progress":42})), Some(42));
    assert_eq!(parse(json!({"state":"working","progress":100})), Some(100));
    assert_eq!(parse(json!({"state":"working","progress":101})), None);
    assert_eq!(parse(json!({"state":"working","progress":null})), None);
    assert_eq!(parse(json!({"state":"working"})), None);
    // The live event path carries it too.
    let (_, event) = activity::parse_block_event(&json!({"block_id":"b",
        "creator_name":activity::TERMINAL,"name":"program_status_changed",
        "payload":{"id":"","record":{"state":"working","progress":7}}}))
    .unwrap();
    assert!(
        matches!(event, activity::BlockEvent::StatusChanged { record, .. } if record.progress == Some(7))
    );
}

#[test]
fn progress_reports_set_and_clear_a_blocks_progress() {
    use activity::TerminalProgress::*;
    let report = |payload: Value| {
        activity::parse_block_event(&json!({"block_id":"b","creator_name":activity::TERMINAL,
            "name":"progress_report","payload":payload}))
    };
    let mut model = HostModel::default();
    for (payload, progress) in [
        (json!({"state":"set","progress":42}), Some(Set(Some(42)))),
        (json!({"state":"set"}), Some(Set(None))),
        (json!({"state":"indeterminate"}), Some(Indeterminate)),
        (json!({"state":"pause","progress":7}), Some(Paused(Some(7)))),
        (json!({"state":"error","progress":101}), Some(Error(None))),
        (json!({"state":"remove"}), None),
    ] {
        let (block, event) = report(payload).expect("a progress report");
        model.apply_block_event("s", block, event);
        assert_eq!(model.progress.get("b").copied(), progress);
    }
    assert!(report(json!({"state":"sparkle"})).is_none());
    model.apply_block_event(
        "s",
        "b".into(),
        activity::BlockEvent::Progress(Some(Set(None))),
    );
    model.forget_block("b");
    assert!(model.progress.is_empty(), "a closed block's progress goes");

    // A report belongs to the program in the foreground: when another takes its place, or
    // the shell comes back, the report goes with it; the same program keeps it.
    let process = |name: Option<&str>| activity::BlockEvent::Process {
        name: name.map(str::to_owned),
        executable: name.map(str::to_owned),
        script: None,
        pids: name.map(|_| vec![7]).unwrap_or_default(),
        place: Default::default(),
    };
    model.apply_block_event("s", "b".into(), process(Some("cargo")));
    model.apply_block_event(
        "s",
        "b".into(),
        activity::BlockEvent::Progress(Some(Set(Some(5)))),
    );
    model.apply_block_event("s", "b".into(), process(Some("cargo")));
    assert_eq!(
        model.progress.get("b").copied(),
        Some(Set(Some(5))),
        "same program"
    );
    model.apply_block_event("s", "b".into(), process(None));
    assert!(
        model.progress.is_empty(),
        "the program ended without clearing it"
    );
    model.apply_block_event(
        "s",
        "b".into(),
        activity::BlockEvent::Progress(Some(Set(Some(5)))),
    );
    model.apply_block_event("s", "b".into(), process(Some("vim")));
    assert!(
        model.progress.is_empty(),
        "another program took the foreground"
    );
}

#[test]
fn progress_goes_when_another_process_of_the_same_name_takes_the_foreground() {
    use activity::TerminalProgress::Set;
    let mut model = HostModel::default();
    let python = |pids: Vec<u64>| activity::BlockEvent::Process {
        name: Some("python".into()),
        executable: Some("python".into()),
        script: None,
        pids,
        place: Default::default(),
    };
    let progress = activity::BlockEvent::Progress(Some(Set(Some(42))));
    model.apply_block_event("s", "b".into(), python(vec![11, 1]));
    model.apply_block_event("s", "b".into(), progress);
    model.apply_block_event("s", "b".into(), python(vec![11, 1]));
    assert_eq!(
        model.progress.get("b").copied(),
        Some(Set(Some(42))),
        "the same process keeps its report"
    );
    model.apply_block_event("s", "b".into(), python(vec![12, 1]));
    assert!(
        model.progress.is_empty(),
        "a new python must not inherit the last one's report"
    );
}

#[test]
fn progress_goes_with_the_process_not_just_its_name() {
    use activity::{BlockEvent, TerminalProgress::Set};
    let mut model = HostModel::default();
    let python = |pids: &[u64]| BlockEvent::Process {
        name: Some("python".into()),
        executable: Some("python".into()),
        script: None,
        pids: pids.to_vec(),
        place: Default::default(),
    };
    let report = BlockEvent::Progress(Some(Set(Some(42))));
    model.apply_block_event("s", "b".into(), python(&[11, 1]));
    model.apply_block_event("s", "b".into(), report.clone());

    // Rex repeats `process_changed` when the process only moves directory, and to each client
    // that starts receiving the terminal.
    model.apply_block_event("s", "b".into(), python(&[11, 1]));
    assert_eq!(
        model.progress.get("b"),
        Some(&Set(Some(42))),
        "same process"
    );

    // A child in the foreground leaves the program that reported running beneath it.
    model.apply_block_event("s", "b".into(), python(&[12, 11, 1]));
    assert_eq!(model.progress.get("b"), Some(&Set(Some(42))), "same parent");

    // Back to the parent: the foreground process that may have reported is gone.
    model.apply_block_event("s", "b".into(), python(&[11, 1]));
    assert!(model.progress.is_empty(), "the child ended");

    // Another run of the same program, seen without the shell between them.
    model.apply_block_event("s", "b".into(), report);
    model.apply_block_event("s", "b".into(), python(&[13, 1]));
    assert!(model.progress.is_empty(), "the job was replaced");
}

#[test]
fn program_names_prefer_the_script_and_drop_login_dashes() {
    let name = |v: Value| activity::program_name(&v);
    assert_eq!(
        name(json!({"name":"node","script":"/home/u/.bun/bin/pi","argv0":"node"})).as_deref(),
        Some("pi")
    );
    assert_eq!(
        name(json!({"name":"zsh","argv0":"-zsh"})).as_deref(),
        Some("zsh")
    );
    assert_eq!(name(json!({"name":"nvim"})).as_deref(), Some("nvim"));
    assert_eq!(name(Value::Null), None);
}

#[test]
fn a_scripts_executable_is_its_interpreter() {
    let executable = |v: Value| activity::executable_name(&v);
    assert_eq!(
        executable(json!({"name":"node","script":"/home/u/.bun/bin/sq","argv0":"node"})).as_deref(),
        Some("node")
    );
    assert_eq!(
        executable(json!({"argv0":"/usr/bin/python3.12","script":"/u/tool"})).as_deref(),
        Some("python3.12")
    );
    assert_eq!(
        executable(json!({"name":"zsh","argv0":"-zsh"})).as_deref(),
        Some("zsh")
    );
    assert_eq!(executable(Value::Null), None);

    let mut model = HostModel::default();
    let event = activity::parse_process(&json!({
        "foreground": {"pid": 9, "name": "node", "argv0": "node", "script": "/u/bin/sq"}
    }));
    model.apply_block_event("s", "b".into(), event);
    assert_eq!(model.programs["b"], "sq");
    assert_eq!(model.executables["b"], "node");
    assert_eq!(model.scripts["b"], "/u/bin/sq");
    model.apply_block_event(
        "s",
        "b".into(),
        activity::parse_process(&json!({"foreground": {}})),
    );
    assert!(
        !model.executables.contains_key("b") && !model.scripts.contains_key("b"),
        "the shell has no foreground"
    );
    let script = |v: Value| activity::script_path(&v);
    assert_eq!(
        script(json!({"script":"C:\\npm\\copilot\\npm-loader.js"})).as_deref(),
        Some("C:/npm/copilot/npm-loader.js")
    );
    assert_eq!(script(json!({"script":"", "name":"zsh"})), None);
}

const OTHER: &str = "session:00000000-0000-0000-0000-000000000002";

async fn reply(ws: &mut WebSocketStream<TcpStream>, request: &Value, seq: u64, payload: Value) {
    send(
        ws,
        json!({"type":"response","id":request["id"],"seq":seq,"payload":payload}),
    )
    .await;
}

#[tokio::test]
async fn activity_follows_every_session_and_queries_the_selected_one() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 11).await;
        let request = recv(&mut ws).await;
        assert_eq!(request["method"], "session.bootstrap");
        assert_eq!(request["payload"]["event_scope"], "all");
        let mut boot = serde_json::to_value(boot()).unwrap();
        boot["attach_all"]["sessions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"session_id":OTHER,"label":"other"}));
        boot["selected_session"]["view"]["windows"] = json!([{"window_id":"w",
            "layers":[{"kind":"tiled","layout":{"block_id":"block:a"},
                "blocks":[{"block_id":"block:a","creator_name":activity::TERMINAL}]}]}]);
        reply(&mut ws, &request, 1, boot).await;
        // Metadata is read once per selected-session block, even on a hidden host.
        let status = recv(&mut ws).await;
        assert_eq!(status["method"], "com.superlogical.terminal.program_status");
        assert_eq!(status["payload"]["block_id"], "block:a");
        reply(
            &mut ws,
            &status,
            2,
            json!({"records":[{"state":"blocked","kind":"question","app":"codex"}]}),
        )
        .await;
        let title = recv(&mut ws).await;
        assert_eq!(title["method"], "com.superlogical.terminal.title");
        reply(
            &mut ws,
            &title,
            3,
            json!({"title":"Codex · fix tests","owner":{"pid":41,"name":"node"}}),
        )
        .await;
        let process = recv(&mut ws).await;
        assert_eq!(process["method"], "com.superlogical.terminal.process");
        reply(
            &mut ws,
            &process,
            4,
            json!({"home":"/home/u", "foreground":{"name":"node","argv0":"codex","cwd":"/home/u/rex"}}),
        )
        .await;
        // The background sweep attempts the unplaced layout before later events.
        let view = prefetched(&mut ws, OTHER).await;
        send(&mut ws, json!({"type":"response","id":view["id"],"error":{"code":"conflict","message":"no","status":409}})).await;
        // attach_all replaces the scope, so after session_created it must still ask for all.
        send(&mut ws, json!({"type":"event","seq":5,"payload":{"type":"session_created","payload":{"session_id":OTHER}}})).await;
        let list = recv(&mut ws).await;
        assert_eq!(list["method"], "session.attach_all");
        assert_eq!(list["payload"]["event_scope"], "all");
        reply(
            &mut ws,
            &list,
            6,
            json!({"sessions":[{"session_id":SESSION,"label":"snapshot"},
            {"session_id":OTHER,"label":"other"}]}),
        )
        .await;
        // A report from an unattached session: its view is fetched once to place it.
        let report = |seq: u64| {
            json!({"type":"event","seq":seq,"payload":{"type":"session_event","payload":{
            "session_id":OTHER,"event":{"type":"block_event","payload":{"block_id":"block:z",
                "creator_name":activity::TERMINAL,"name":"program_status_changed",
                "payload":{"id":"","record":{"state":"working"}}}}}}})
        };
        send(&mut ws, report(7)).await;
        send(&mut ws, report(8)).await;
        // No second session.view: the next message is the client closing.
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            {
                let m = host.model.lock().unwrap();
                if m.activity.contains_key("block:z") && m.programs.contains_key("block:a") {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    {
        let m = host.model.lock().unwrap();
        let a = &m.activity["block:a"][""];
        assert_eq!(a.state, activity::ProgramState::Blocked);
        assert_eq!(m.title("block:a"), Some("Codex · fix tests"));
        assert_eq!(a.app.as_deref(), Some("codex"));
        assert_eq!(m.programs["block:a"], "codex");
        assert_eq!(
            m.generated_title("block:a").as_deref(),
            Some("codex - ~/rex")
        );
        assert_eq!(m.sessions.len(), 2);
    }
    // Let the second report land before closing.
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn destroying_a_session_forgets_it_without_an_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 11).await;
        let request = recv(&mut ws).await;
        reply(&mut ws, &request, 1, serde_json::to_value(boot()).unwrap()).await;
        let destroy = recv(&mut ws).await;
        assert_eq!(destroy["method"], "session.destroy");
        reply(&mut ws, &destroy, 2, json!({})).await;
        // The client re-reads the session it acted on, which is gone now.
        let view = recv(&mut ws).await;
        assert_eq!(view["method"], "session.view");
        send(
            &mut ws,
            json!({"type":"response","id":view["id"],"error":{"code":"session_not_found",
            "message":"session not found","status":404}}),
        )
        .await;
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    host.commands
        .send(HostCommand::Request {
            method: "session.destroy".into(),
            payload: json!({"session_id": SESSION}),
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(8), async {
        while !host.model.lock().unwrap().sessions.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    {
        let m = host.model.lock().unwrap();
        assert_eq!(m.notice, None);
        assert_eq!(m.selected, None);
        assert!(m.connected);
    }
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn refreshing_sessions_reads_the_list_and_selected_layout_then_sweeps_new_sessions() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 11).await;
        let request = recv(&mut ws).await;
        reply(&mut ws, &request, 1, serde_json::to_value(boot()).unwrap()).await;
        let list = recv(&mut ws).await;
        assert_eq!(list["method"], "session.attach_all");
        reply(
            &mut ws,
            &list,
            2,
            json!({"sessions":[{"session_id":SESSION,"label":"renamed"},
                {"session_id":OTHER,"label":"elsewhere"}]}),
        )
        .await;
        let view = recv(&mut ws).await;
        assert_eq!(view["method"], "session.view");
        assert_eq!(view["payload"]["session_id"], SESSION);
        reply(
            &mut ws,
            &view,
            3,
            json!({"session_id":SESSION,"label":"renamed","revision":2}),
        )
        .await;
        let background = prefetched(&mut ws, OTHER).await;
        reply(&mut ws, &background, 4, view_of(OTHER)).await;
        // Neither an attach nor a create follows: the next thing is the client leaving.
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    host.commands.send(HostCommand::RefreshSessions).unwrap();
    until(&host, |m| {
        m.sessions.len() == 2
            && m.views.get(SESSION).is_some_and(|v| v.revision == 2)
            && m.views.contains_key(OTHER)
    })
    .await;
    {
        let m = host.model.lock().unwrap();
        assert_eq!(m.sessions[0].label, "renamed");
        assert_eq!(m.selected.as_deref(), Some(SESSION));
        assert_eq!(m.notice, None);
        assert!(m.connected);
    }
    drop(host);
    server.await.unwrap();
}

#[test]
fn clipboard_events_are_not_replaced_by_a_view_snapshot() {
    let state = ControlState {
        bootstrap_seq: 10,
        view_seq: HashMap::from([(SESSION.into(), 20)]),
        ..Default::default()
    };
    for name in ["clipboard_written", "selection_copied"] {
        let event = rexrs::Event {
            seq: 19,
            name: "block_event".into(),
            session_id: Some(SESSION.into()),
            payload: json!({"creator_name": activity::TERMINAL, "name": name,
                "block_id": "block:copy", "payload": {"id": 1, "bytes": 4, "text": "copy"}}),
        };
        assert!(
            state.includes(&event),
            "a view contains no clipboard text: {name}"
        );
        assert!(!state.includes(&rexrs::Event { seq: 9, ..event }));
    }
}

#[tokio::test]
async fn clipboard_reads_pin_ids_and_count_utf8_bytes_for_both_copy_events() {
    for (event, method) in [
        (
            "clipboard_written",
            "com.superlogical.terminal.clipboard_read",
        ),
        ("selection_copied", "com.superlogical.terminal.blob_read"),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut ws = accept(&listener, "rex.control.v1").await;
            welcome(&mut ws, 11).await;
            let mut offset = 0;
            for (index, data) in ["世", "界🦀\n"].into_iter().enumerate() {
                let request = recv(&mut ws).await;
                assert_eq!(request["method"], method);
                assert_eq!(request["payload"]["session_id"], SESSION);
                assert_eq!(request["payload"]["block_id"], "block:copy");
                assert_eq!(
                    request["payload"]["args"],
                    json!({"id": 42, "offset": offset, "max_bytes": 49152})
                );
                offset += data.len();
                send(
                    &mut ws,
                    json!({"type":"response", "id":request["id"], "payload":{
                    "id":42,"data":data,"total":11,"eof": index == 1}}),
                )
                .await;
            }
            assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
        });
        let mut client = transport::control(&endpoint, None).await.unwrap();
        let payload =
            json!({"name": event, "block_id": "block:copy", "payload": {"id": 42, "bytes": 11}});
        let copy = clipboard::read(&mut client, SESSION, &payload)
            .await
            .unwrap();
        assert_eq!(copy.text, "世界🦀\n");
        assert_eq!(copy.session, SESSION);
        assert_eq!(copy.block, "block:copy");
        client.close().await.unwrap();
        server.await.unwrap();
    }
}

#[tokio::test]
async fn clipboard_reads_reject_stalled_truncated_changed_and_oversized_results() {
    for part in [
        json!({"id":42,"data":"","total":5,"eof":false}),
        json!({"id":42,"data":"copy","total":5,"eof":true}),
        json!({"id":43,"data":"copy","total":4,"eof":true}),
        json!({"id":42,"data":"copy","total":3,"eof":true}),
        json!({"id":42,"data":"","total":-1,"eof":true}),
        json!({"id":42,"data":"","total":8 * 1024 * 1024 + 1,"eof":false}),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut ws = accept(&listener, "rex.control.v1").await;
            welcome(&mut ws, 11).await;
            let request = recv(&mut ws).await;
            send(
                &mut ws,
                json!({"type":"response","id":request["id"],"payload":part}),
            )
            .await;
            assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
        });
        let mut client = transport::control(&endpoint, None).await.unwrap();
        assert!(
            clipboard::read(
                &mut client,
                SESSION,
                &json!({"name":"clipboard_written",
            "block_id":"block:copy","payload":{"id":42}})
            )
            .await
            .is_err()
        );
        client.close().await.unwrap();
        server.await.unwrap();
    }
}

#[test]
fn font_preferences_use_saved_values_unless_explicitly_overridden() {
    use clap::Parser;
    let settings = crate::settings::Settings {
        font_family: "IBM Plex Mono".into(),
        font_fallbacks: vec!["Inter".into()],
        font_size: 22.,
        ..Default::default()
    };
    let before = serde_json::to_value(&settings).unwrap();
    let defaults = Options::parse_from(["rex-rune"]);
    assert_eq!(
        defaults.fonts(&settings),
        ("IBM Plex Mono", settings.font_fallbacks.as_slice())
    );
    assert_eq!(defaults.font_size, None);
    let explicit = Options::parse_from([
        "rex-rune",
        "--font",
        "TX02 Nerd Font Mono",
        "--font-size",
        "15",
        "--font-fallback",
        "Noto Sans CJK JP",
        "--font-fallback",
        "Inter",
    ]);
    let (family, fallbacks) = explicit.fonts(&settings);
    assert_eq!(family, "TX02 Nerd Font Mono");
    assert_eq!(fallbacks, ["Noto Sans CJK JP", "Inter"]);
    assert_eq!(
        explicit.font_size,
        Some(15.),
        "an explicit 15 is not the default sentinel"
    );
    assert_eq!(
        serde_json::to_value(&settings).unwrap(),
        before,
        "CLI family choices do not overwrite preferences"
    );
    let primary_only = Options::parse_from(["rex-rune", "--font", "Inter"]);
    assert_eq!(
        primary_only.fonts(&settings),
        ("Inter", settings.font_fallbacks.as_slice())
    );
    let fallback_only = Options::parse_from(["rex-rune", "--font-fallback", "Noto Sans CJK JP"]);
    assert_eq!(fallback_only.fonts(&settings).0, "IBM Plex Mono");
    for args in [
        ["rex-rune", "--font", " "],
        ["rex-rune", "--font-fallback", "a,b"],
        ["rex-rune", "--font-size", "NaN"],
        ["rex-rune", "--font-size", "inf"],
    ] {
        assert!(Options::try_parse_from(args).is_err());
    }
}

const THIRD: &str = "session:00000000-0000-0000-0000-000000000003";

/// A bootstrap that lists `others` after the selected session.
fn boot_with(others: &[&str]) -> Value {
    let mut boot = serde_json::to_value(boot()).unwrap();
    for id in others {
        boot["attach_all"]["sessions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"session_id":id,"label":id}));
    }
    boot
}

fn view_of(id: &str) -> Value {
    json!({"session_id":id,"revision":1,"windows":[{"window_id":format!("{id}/tab")}]})
}

/// The next message, which must come well inside the 8 s request timeout.
async fn next(ws: &mut WebSocketStream<TcpStream>) -> Value {
    tokio::time::timeout(Duration::from_secs(2), recv(ws))
        .await
        .expect("a message in time")
}

async fn prefetched(ws: &mut WebSocketStream<TcpStream>, id: &str) -> Value {
    let view = next(ws).await;
    assert_eq!(view["method"], "session.view");
    assert_eq!(view["payload"]["session_id"], id);
    assert!(view["id"].as_str().unwrap().starts_with("detached:"));
    view
}

async fn until(host: &HostConnection, done: impl Fn(&HostModel) -> bool) {
    tokio::time::timeout(Duration::from_secs(8), async {
        while !done(&host.model.lock().unwrap()) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

fn prefetch(host: &HostConnection, ids: &[&str]) {
    let ids = ids.iter().map(|id| (*id).to_owned()).collect();
    host.commands.send(HostCommand::PrefetchViews(ids)).unwrap();
}

fn focus_window(host: &HostConnection) {
    host.commands
        .send(HostCommand::Request {
            method: "session.focus_window".into(),
            payload: json!({"session_id":SESSION,"window_id":"w"}),
        })
        .unwrap();
}

/// Answer a control request and the view refresh that follows it.
async fn answer_request(ws: &mut WebSocketStream<TcpStream>) {
    let request = next(ws).await;
    assert_eq!(request["method"], "session.focus_window");
    reply(ws, &request, 3, json!({})).await;
    let refresh = next(ws).await;
    assert_eq!(refresh["method"], "session.view");
    assert_eq!(refresh["payload"]["session_id"], SESSION);
    reply(ws, &refresh, 3, json!({"session_id":SESSION,"revision":2})).await;
}

#[tokio::test]
async fn a_slow_prefetch_neither_delays_control_requests_nor_overlaps_another() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (sent, mut started) = tokio::sync::mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 11).await;
        let boot = recv(&mut ws).await;
        reply(&mut ws, &boot, 1, boot_with(&[OTHER, THIRD])).await;
        let slow = prefetched(&mut ws, OTHER).await;
        sent.send(()).unwrap();
        // Unanswered, it holds back the next fetch but not the request.
        answer_request(&mut ws).await;
        reply(&mut ws, &slow, 4, view_of(OTHER)).await;
        let view = prefetched(&mut ws, THIRD).await;
        reply(&mut ws, &view, 5, view_of(THIRD)).await;
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    prefetch(&host, &[OTHER, THIRD]);
    started.recv().await.unwrap();
    focus_window(&host);
    until(&host, |m| {
        m.views.contains_key(OTHER) && m.views.contains_key(THIRD)
    })
    .await;
    assert!(host.model.lock().unwrap().views[SESSION].revision == 2);
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn leaving_sidebar_eligibility_keeps_the_activity_sweep() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (sent, mut started) = tokio::sync::mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 11).await;
        let boot = recv(&mut ws).await;
        reply(&mut ws, &boot, 1, boot_with(&[OTHER, THIRD])).await;
        let late = prefetched(&mut ws, OTHER).await;
        sent.send(()).unwrap();
        // Collapsing the sidebar must not discard the sweep\'s read.
        answer_request(&mut ws).await;
        reply(&mut ws, &late, 4, view_of(OTHER)).await;
        let view = prefetched(&mut ws, THIRD).await;
        reply(&mut ws, &view, 5, view_of(THIRD)).await;
        sent.send(()).unwrap();
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    prefetch(&host, &[OTHER, THIRD]);
    started.recv().await.unwrap();
    prefetch(&host, &[]);
    focus_window(&host);
    started.recv().await.unwrap();
    until(&host, |m| m.views.contains_key(THIRD)).await;
    assert!(host.model.lock().unwrap().views.contains_key(OTHER));
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn prefetch_picks_up_new_sessions_and_restarts_after_reconnect() {
    const NEW: &str = "session:00000000-0000-0000-0000-000000000004";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 11).await;
        let boot = recv(&mut ws).await;
        reply(&mut ws, &boot, 1, boot_with(&[OTHER])).await;
        let view = prefetched(&mut ws, OTHER).await;
        reply(&mut ws, &view, 2, view_of(OTHER)).await;
        send(&mut ws, json!({"type":"event","seq":3,"payload":{"type":"session_created","payload":{"session_id":NEW}}})).await;
        let list = next(&mut ws).await;
        assert_eq!(list["method"], "session.attach_all");
        let sessions = &boot_with(&[OTHER, NEW])["attach_all"];
        reply(&mut ws, &list, 4, sessions.clone()).await;
        let view = prefetched(&mut ws, NEW).await;
        reply(&mut ws, &view, 5, view_of(NEW)).await;
        // The client drops this connection; a new one fetches every view again.
        assert!(!matches!(ws.next().await, Some(Ok(Message::Text(_)))));
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 11).await;
        let boot = recv(&mut ws).await;
        reply(&mut ws, &boot, 1, boot_with(&[OTHER, NEW])).await;
        for id in [OTHER, NEW] {
            let view = prefetched(&mut ws, id).await;
            reply(&mut ws, &view, 2, view_of(id)).await;
        }
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    prefetch(&host, &[OTHER, NEW]);
    until(&host, |m| m.views.contains_key(NEW)).await;
    host.commands.send(HostCommand::Reconnect).unwrap();
    // Only a Reconnect that arrives during the delay ends it; any later one would drop the
    // new connection too.
    until(&host, |m| !m.connected).await;
    host.commands.send(HostCommand::Reconnect).unwrap();
    until(&host, |m| {
        m.protocol.bootstrap_requests == 2
            && m.views.contains_key(OTHER)
            && m.views.contains_key(NEW)
    })
    .await;
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn a_failed_prefetch_is_retried_after_a_backoff() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 11).await;
        let boot = recv(&mut ws).await;
        reply(&mut ws, &boot, 1, boot_with(&[OTHER, THIRD])).await;
        let view = prefetched(&mut ws, OTHER).await;
        send(&mut ws, json!({"type":"response","id":view["id"],"error":{"code":"conflict","message":"busy","status":409}})).await;
        let failed = Instant::now();
        let view = prefetched(&mut ws, THIRD).await;
        assert!(failed.elapsed() < Duration::from_millis(500));
        reply(&mut ws, &view, 2, view_of(THIRD)).await;
        let view = prefetched(&mut ws, OTHER).await;
        assert!(failed.elapsed() >= Duration::from_millis(950));
        reply(&mut ws, &view, 3, view_of(OTHER)).await;
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    prefetch(&host, &[OTHER, THIRD]);
    until(&host, |m| m.views.contains_key(OTHER)).await;
    drop(host);
    server.await.unwrap();
}

/// A scripted control server for theme declarations: it answers every request, records each
/// one as (method, payload), sends the events the test pushes, and takes the client back
/// after a reconnect. Data sockets are accepted and dropped.
struct ThemeServer {
    endpoint: String,
    events: tokio::sync::mpsc::UnboundedSender<Value>,
    requests: tokio::sync::mpsc::UnboundedReceiver<(String, Value)>,
}

impl ThemeServer {
    /// Serves one session whose single window holds `blocks` (`[id, owner]`, owner none for
    /// nobody); `set_theme` fails with `theme_error` when given.
    #[allow(clippy::result_large_err)] // tungstenite's callback fixes the error type.
    async fn start(blocks: &[(&str, Option<&str>)], theme_error: Option<&'static str>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let blocks: Vec<Value> = blocks
            .iter()
            .map(|(id, owner)| json!({"block_id":id,"creator_name":activity::TERMINAL,"owner":owner}))
            .collect();
        let view = json!({"session_id":SESSION,"label":"snapshot","revision":1,
            "windows":[{"window_id":"w","layers":[{"kind":"tiled",
                "layout":{"block_id":blocks[0]["block_id"]},"blocks":blocks}]}]});
        let (events, mut pushed) = tokio::sync::mpsc::unbounded_channel::<Value>();
        let (record, requests) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut seq = 1;
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut path = String::new();
                let mut ws = tokio_tungstenite::accept_hdr_async(
                    tcp,
                    |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                     mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                        path = request.uri().path().to_owned();
                        if let Some(protocol) = request.headers().get("Sec-WebSocket-Protocol") {
                            response
                                .headers_mut()
                                .insert("Sec-WebSocket-Protocol", protocol.clone());
                        }
                        Ok(response)
                    },
                )
                .await
                .unwrap();
                if path != "/api/control" {
                    continue;
                }
                welcome(&mut ws, 11).await;
                loop {
                    let request: Value = tokio::select! {
                        message = ws.next() => match message {
                            Some(Ok(Message::Text(text))) => serde_json::from_str(&text).unwrap(),
                            Some(Ok(_)) => continue,
                            _ => break,
                        },
                        Some(event) = pushed.recv() => {
                            seq += 1;
                            send(&mut ws, json!({"type":"event","seq":seq,"payload":{"type":"session_event",
                                "payload":{"session_id":SESSION,"event":event}}})).await;
                            continue;
                        }
                    };
                    seq += 1;
                    let method = request["method"].as_str().unwrap_or_default().to_owned();
                    let _ = record.send((method.clone(), request["payload"].clone()));
                    let payload = match method.as_str() {
                        "session.bootstrap" => {
                            let mut boot = serde_json::to_value(boot()).unwrap();
                            boot["selected_session"]["view"] = view.clone();
                            boot
                        }
                        "session.attach_all" => {
                            json!({"sessions":[{"session_id":SESSION,"label":"snapshot"},
                                {"session_id":OTHER,"label":"other"}]})
                        }
                        "session.create" => json!({"session_id":OTHER}),
                        "session.attach" => json!({"client_id":"client:test",
                            "view":{"session_id":OTHER,"revision":1}}),
                        "data.connect" => json!({"ticket":"AAAAAAAAAAAAAAAAAAAAAA"}),
                        "com.superlogical.terminal.set_theme" if theme_error.is_some() => {
                            send(
                                &mut ws,
                                json!({"type":"response","id":request["id"],
                                "error":{"code":theme_error,"message":"no","status":404}}),
                            )
                            .await;
                            continue;
                        }
                        _ => json!({}),
                    };
                    reply(&mut ws, &request, seq, payload).await;
                }
            }
        });
        Self {
            endpoint,
            events,
            requests,
        }
    }

    fn owner(&self, block: &str, owner: Option<&str>) {
        self.events
            .send(json!({"type":"block_owner_changed","payload":{"block_id":block,"owner":owner}}))
            .unwrap();
    }

    /// The `set_theme` calls, as (block, args), up to the next `method` request, and that
    /// request's payload.
    async fn themes_until(&mut self, method: &str) -> (Vec<(String, Value)>, Value) {
        let mut themes = Vec::new();
        loop {
            let (name, payload) =
                tokio::time::timeout(Duration::from_secs(8), self.requests.recv())
                    .await
                    .unwrap_or_else(|_| panic!("no {method}"))
                    .unwrap();
            if name == "com.superlogical.terminal.set_theme" {
                assert_eq!(payload["session_id"], SESSION);
                themes.push((
                    payload["block_id"].as_str().unwrap().to_owned(),
                    payload["args"].clone(),
                ));
            } else if name == method {
                return (themes, payload);
            }
        }
    }

    /// The `set_theme` calls an event caused: a session_created after it makes the client
    /// list sessions once it has handled the event.
    async fn themes_after_events(&mut self) -> Vec<(String, Value)> {
        self.events
            .send(json!({"type":"session_created","payload":{"session_id":OTHER}}))
            .unwrap();
        self.themes_until("session.attach_all").await.0
    }
}

/// The `set_theme` calls a command caused: a request queued after it is handled after the
/// declarations it triggers.
async fn themes_after_command(
    server: &mut ThemeServer,
    host: &HostConnection,
    command: HostCommand,
) -> Vec<(String, Value)> {
    host.commands.send(command).unwrap();
    host.commands
        .send(HostCommand::Request {
            method: "test.sentinel".into(),
            payload: json!({}),
        })
        .unwrap();
    server.themes_until("test.sentinel").await.0
}

fn sorted(mut themes: Vec<(String, Value)>) -> Vec<(String, Value)> {
    themes.sort_by(|a, b| a.0.cmp(&b.0));
    themes
}

fn declared(blocks: &[&str], args: &Value) -> Vec<(String, Value)> {
    blocks
        .iter()
        .map(|b| ((*b).to_owned(), args.clone()))
        .collect()
}

#[tokio::test]
async fn themes_go_only_to_blocks_this_client_may_theme_once_each() {
    let mut server = ThemeServer::start(
        &[
            ("block:a", None),
            ("block:b", Some("client:test")),
            ("block:c", Some("client:other")),
        ],
        None,
    )
    .await;
    let first = SetTheme::new().background("#111111");
    let host = HostConnection::spawn(
        server.endpoint.clone(),
        None,
        None,
        None,
        true,
        Some(first.clone()),
    );
    let first = serde_json::to_value(&first).unwrap();
    let (themes, _) = server
        .themes_until("com.superlogical.terminal.program_status")
        .await;
    assert_eq!(
        sorted(themes),
        declared(&["block:a", "block:b"], &first),
        "unowned and owned blocks, not another client's"
    );

    server.owner("block:c", Some("client:test"));
    assert_eq!(
        server.themes_after_events().await,
        declared(&["block:c"], &first),
        "taking a block declares to it, with no resize"
    );
    server.owner("block:a", Some("client:other"));
    server.owner("block:a", Some("client:test"));
    assert_eq!(
        server.themes_after_events().await,
        [],
        "Rex keeps this client's declaration while another owns the block"
    );

    server.owner("block:b", Some("client:other"));
    assert_eq!(server.themes_after_events().await, []);
    let second = SetTheme::new().background("#eeeeee").foreground("#222222");
    let themes =
        themes_after_command(&mut server, &host, HostCommand::Theme(Some(second.clone()))).await;
    let second_json = serde_json::to_value(&second).unwrap();
    assert_eq!(
        sorted(themes),
        declared(&["block:a", "block:c"], &second_json)
    );
    assert_eq!(
        themes_after_command(&mut server, &host, HostCommand::Theme(Some(second.clone()))).await,
        [],
        "the same theme again is not repeated"
    );

    let created = {
        host.commands
            .send(HostCommand::CreateSession {
                label: None,
                shell: None,
            })
            .unwrap();
        server.themes_until("session.create").await.1
    };
    assert_eq!(
        created["initial_windows"][0]["layout"]["block"]["options"]["theme"], second_json,
        "new blocks start with the declared theme"
    );

    let empty = json!({});
    let themes = themes_after_command(
        &mut server,
        &host,
        HostCommand::Theme(Some(SetTheme::new())),
    )
    .await;
    assert_eq!(
        sorted(themes),
        declared(&["block:a", "block:c"], &empty),
        "Rex default colours declare an empty theme"
    );
    assert_eq!(
        themes_after_command(&mut server, &host, HostCommand::Theme(None)).await,
        [],
        "Follow server after Rex default has nothing to reset"
    );
    server.owner("block:b", Some("client:test"));
    assert_eq!(
        server.themes_after_events().await,
        declared(&["block:b"], &empty),
        "under Follow server a block taken back is reset once, not left in the old theme"
    );
    drop(host);
}

#[tokio::test]
async fn follow_server_declares_nothing_and_resets_what_it_declared() {
    let mut server = ThemeServer::start(&[("block:a", None)], None).await;
    let host = HostConnection::spawn(server.endpoint.clone(), None, None, None, true, None);
    let (themes, _) = server
        .themes_until("com.superlogical.terminal.program_status")
        .await;
    assert_eq!(themes, []);
    let theme = SetTheme::new().background("#111111");
    let themes =
        themes_after_command(&mut server, &host, HostCommand::Theme(Some(theme.clone()))).await;
    assert_eq!(
        themes,
        declared(&["block:a"], &serde_json::to_value(&theme).unwrap())
    );
    assert_eq!(
        themes_after_command(&mut server, &host, HostCommand::Theme(None)).await,
        declared(&["block:a"], &json!({})),
        "one empty theme undoes the declaration"
    );
    assert_eq!(
        themes_after_command(&mut server, &host, HostCommand::Theme(None)).await,
        []
    );
    host.commands
        .send(HostCommand::CreateSession {
            label: None,
            shell: None,
        })
        .unwrap();
    let created = server.themes_until("session.create").await.1;
    assert_eq!(
        created["initial_windows"][0]["layout"]["block"].get("options"),
        None
    );
    drop(host);
}

#[tokio::test]
async fn follow_server_declares_nothing_and_records_no_declaration() {
    let mut server = ThemeServer::start(&[("block:a", None)], None).await;
    let host = HostConnection::spawn(server.endpoint.clone(), None, None, None, true, None);
    let (themes, _) = server
        .themes_until("com.superlogical.terminal.program_status")
        .await;
    assert_eq!(themes, []);
    server.owner("block:a", Some("client:test"));
    assert_eq!(server.themes_after_events().await, []);
    host.commands.send(HostCommand::Reconnect).unwrap();
    server.themes_until("session.bootstrap").await;
    let (themes, _) = server
        .themes_until("com.superlogical.terminal.program_status")
        .await;
    assert_eq!(themes, []);
    assert!(!host.model.lock().unwrap().theme_declared);
    drop(host);
}

/// A client that once declared a theme follows the server with an empty declaration.
#[tokio::test]
async fn a_reset_goes_to_each_eligible_block_once_per_connection() {
    let mut server = ThemeServer::start(
        &[("block:a", None), ("block:b", Some("client:other"))],
        None,
    )
    .await;
    let reset = SetTheme::new();
    let empty = json!({});
    let host = HostConnection::spawn(
        server.endpoint.clone(),
        None,
        None,
        None,
        true,
        Some(reset.clone()),
    );
    let (themes, _) = server
        .themes_until("com.superlogical.terminal.program_status")
        .await;
    assert_eq!(
        themes,
        declared(&["block:a"], &empty),
        "not another client's"
    );
    assert!(host.model.lock().unwrap().theme_declared);
    assert_eq!(
        themes_after_command(&mut server, &host, HostCommand::Theme(Some(reset.clone()))).await,
        [],
        "once per block on a connection"
    );
    server.owner("block:b", Some("client:test"));
    assert_eq!(
        server.themes_after_events().await,
        declared(&["block:b"], &empty),
        "taking a block resets it"
    );
    server.owner("block:b", Some("client:other"));
    server.owner("block:b", Some("client:test"));
    assert_eq!(server.themes_after_events().await, []);

    host.commands.send(HostCommand::Reconnect).unwrap();
    server.themes_until("session.bootstrap").await;
    let (themes, _) = server
        .themes_until("com.superlogical.terminal.program_status")
        .await;
    assert_eq!(
        themes,
        declared(&["block:a"], &empty),
        "again after reconnect"
    );
    server.owner("block:b", Some("client:test"));
    assert_eq!(
        server.themes_after_events().await,
        declared(&["block:b"], &empty)
    );
    drop(host);
}

#[tokio::test]
async fn themes_are_declared_again_after_reconnect_with_the_latest_choice() {
    let mut server = ThemeServer::start(&[("block:a", None)], None).await;
    let first = SetTheme::new().background("#111111");
    let host = HostConnection::spawn(
        server.endpoint.clone(),
        None,
        None,
        None,
        true,
        Some(first.clone()),
    );
    let (themes, _) = server
        .themes_until("com.superlogical.terminal.program_status")
        .await;
    assert_eq!(
        themes,
        declared(&["block:a"], &serde_json::to_value(&first).unwrap())
    );
    host.commands.send(HostCommand::Reconnect).unwrap();
    // Changed while disconnected: the next connection declares it, and only it.
    let second = SetTheme::new().background("#222222");
    host.commands
        .send(HostCommand::Theme(Some(second.clone())))
        .unwrap();
    host.commands.send(HostCommand::Reconnect).unwrap();
    server.themes_until("session.bootstrap").await;
    let (themes, _) = server
        .themes_until("com.superlogical.terminal.program_status")
        .await;
    assert_eq!(
        themes,
        declared(&["block:a"], &serde_json::to_value(&second).unwrap())
    );
    drop(host);
}

#[tokio::test]
async fn a_server_without_set_theme_is_told_once_and_never_again() {
    let mut server = ThemeServer::start(
        &[("block:a", None), ("block:b", None)],
        Some("method_not_found"),
    )
    .await;
    let host = HostConnection::spawn(
        server.endpoint.clone(),
        None,
        None,
        None,
        true,
        Some(SetTheme::new().background("#111111")),
    );
    let (themes, _) = server
        .themes_until("com.superlogical.terminal.program_status")
        .await;
    assert_eq!(themes.len(), 1, "the first refusal stops declaring");
    {
        let m = host.model.lock().unwrap();
        assert!(m.themes_unsupported);
        assert!(
            m.notice
                .as_deref()
                .is_some_and(|n| n.contains("terminal themes"))
        );
    }
    let other = SetTheme::new().background("#333333");
    assert_eq!(
        themes_after_command(&mut server, &host, HostCommand::Theme(Some(other))).await,
        []
    );
    host.commands
        .send(HostCommand::CreateSession {
            label: None,
            shell: None,
        })
        .unwrap();
    let created = server.themes_until("session.create").await.1;
    assert_eq!(
        created["initial_windows"][0]["layout"]["block"].get("options"),
        None,
        "new blocks carry no theme either"
    );
    drop(host);
}

#[tokio::test]
async fn searching_needs_no_server_while_the_host_reconnects() {
    // Nothing listens here, so the host stays between attempts.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let host = HostConnection::spawn(endpoint, None, None, None, true, None);
    let send = |command| {
        host.commands
            .send(HostCommand::Terminal {
                session: "s".into(),
                block: "b".into(),
                command,
            })
            .unwrap();
    };
    let notice = || host.model.lock().unwrap().notice.clone();
    // A refused TCP connection takes longer on Windows. Observe the retry state rather
    // than assuming the first connection attempt has finished after 200 ms.
    until(&host, |m| m.status.starts_with("Disconnected")).await;
    send(Command::Search("needle".into()));
    send(Command::SearchStep(true));
    send(Command::PromptStep(true));
    send(Command::PromptStep(false));
    send(Command::Search(String::new()));
    // A reply to the next command proves the preceding searches were consumed.
    let (reply, ticket) = tokio::sync::oneshot::channel();
    host.commands
        .send(HostCommand::Ticket {
            session: "s".into(),
            block: "b".into(),
            reply,
        })
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(8), ticket)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    assert_eq!(notice(), None, "a search is this client's own");

    // Input still isn't taken for later.
    send(Command::Reconnect);
    until(&host, |m| {
        m.notice
            .as_deref()
            .is_some_and(|n| n.contains("not sent while disconnected"))
    })
    .await;
    drop(host);
}

#[tokio::test]
async fn a_worker_gets_its_search_whatever_the_control_connection_is_doing() {
    let (commands, received) = flume::unbounded();
    let workers = HashMap::from([(
        "b".to_owned(),
        PaneWorker {
            session: "s".into(),
            commands,
            task: tokio::spawn(async {}),
            size: None,
        },
    )]);
    assert!(forward_search(
        &workers,
        "b",
        &Command::Search("needle".into())
    ));
    assert!(forward_search(&workers, "b", &Command::SearchStep(false)));
    assert!(forward_search(&workers, "b", &Command::PromptStep(true)));
    assert!(forward_search(&workers, "b", &Command::PromptStep(false)));
    assert!(
        forward_search(&workers, "gone", &Command::SearchStep(false)),
        "a closed pane's search goes nowhere, and needs no notice"
    );
    assert!(!forward_search(&workers, "b", &Command::Reconnect));
    assert!(matches!(
        received.try_recv(),
        Ok(DataCommand::Search(needle)) if needle == "needle"
    ));
    assert!(matches!(
        received.try_recv(),
        Ok(DataCommand::SearchStep(false))
    ));
    assert!(matches!(
        received.try_recv(),
        Ok(DataCommand::PromptStep(true))
    ));
    assert!(matches!(
        received.try_recv(),
        Ok(DataCommand::PromptStep(false))
    ));
    assert!(received.try_recv().is_err());
}

#[test]
fn the_old_name_answers_version_as_the_old_updater_checks_it() {
    let version = env!("CARGO_PKG_VERSION");
    assert_eq!(
        Options::version_line(false),
        format!("rex-rune {version}\n")
    );
    // The 0.7.x updater runs `bin/rex-linux --version` and installs only on this exact line.
    assert_eq!(
        Options::version_line(true),
        format!("rex-linux {version}\n")
    );
    let options = Options::from_arguments(["rex-linux", "--session", "work"].map(Into::into), true);
    assert_eq!(options.session.as_deref(), Some("work"));
}

#[tokio::test]
async fn copy_mode_worker_round_trip_yank_generation_guard_and_auto_exit() {
    use crate::terminal::copy_mode::{CopyModeInput, CopyModeOp, CopyMotion, CopyUnit, CopyView};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (push_tx, push_rx) = flume::unbounded::<Vec<u8>>();

    let mut source = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
        cols: 20,
        rows: 4,
        max_scrollback: 1 << 16,
    })
    .unwrap();
    source.vt_write(b"alpha beta\r\ngamma delta\r\n");
    let snapshot = ghostty_vt::snapshot::encode(&source).unwrap().to_vec();
    drop(source);

    // Fake data server: handshake, send Ghostty snapshot, then forward pushed PTY bytes.
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, wire::PROTOCOL).await;
        let hello = ws.next().await.unwrap().unwrap().into_data();
        assert_eq!((hello.len(), hello[0]), (17, 1));
        let mut welcome = vec![0u8; 32];
        welcome[0] = 0x81;
        welcome[24] = 1;
        ws.send(Message::Binary(welcome.into())).await.unwrap();
        ws.send(Message::Binary(snapshot.into())).await.unwrap();

        while let Ok(bytes) = push_rx.recv_async().await {
            ws.send(Message::Binary(bytes.into())).await.unwrap();
        }
    });

    let (host_tx, host_rx) = flume::unbounded::<HostCommand>();
    let ticket_task = tokio::spawn(async move {
        if let Ok(HostCommand::Ticket { reply, .. }) = host_rx.recv_async().await {
            let _ = reply.send(Ok(TerminalTicket {
                token: "AAAAAAAAAAAAAAAAAAAAAA".into(),
                bundled: false,
            }));
        }
    });

    let model = Arc::new(Mutex::new(Model::default()));
    let (wake_tx, wake_rx) = flume::bounded(1);
    let (host_wake_tx, _host_wake_rx) = flume::bounded(1);
    let (cmd_tx, cmd_rx) = flume::unbounded::<DataCommand>();

    let pane = PaneShared {
        model: model.clone(),
        wake: wake_tx,
        host_wake: host_wake_tx,
    };

    async fn wait_for(
        model: &Arc<Mutex<Model>>,
        wake_rx: &flume::Receiver<()>,
        pred: impl Fn(&Model) -> bool,
    ) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if pred(&model.lock().unwrap()) {
                    return;
                }
                wake_rx.recv_async().await.unwrap();
            }
        })
        .await
        .unwrap();
    }

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let worker = tokio::task::spawn_local(run_data(
                endpoint,
                SESSION.into(),
                "block:copy".into(),
                host_tx,
                cmd_rx,
                pane,
            ));

            wait_for(&model, &wake_rx, |m| m.connected && m.frame.is_some()).await;
            let generation = model.lock().unwrap().restore_generation;
            assert!(generation >= 1);

            // 1. Stale generation is ignored without stamping selection_revision or entering copy mode.
            let rev_before = model.lock().unwrap().revision;
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation: generation + 99,
                    epoch: 1,
                    revision: 42,
                    visible_rows: None,
                    op: CopyModeOp::Begin,
                }))
                .unwrap();
            // Deterministic fence: a benign CursorStyle update proves the stale CopyMode
            // command ahead of it in the FIFO channel was already processed and ignored.
            cmd_tx
                .send(DataCommand::CursorStyle(crate::terminal::CursorShape::Bar))
                .unwrap();
            wait_for(&model, &wake_rx, |m| m.revision > rev_before).await;
            {
                let m = model.lock().unwrap();
                let frame = m.frame.as_ref().unwrap();
                assert_eq!(
                    frame.copy, None,
                    "stale generation must not enter copy mode"
                );
                assert_eq!(
                    frame.selection_revision, 0,
                    "stale generation must not stamp selection_revision"
                );
            }

            // 2. Valid Begin + Top + Select(Char) + WordEnd -> check frame.copy & selection_revision.
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 1,
                    visible_rows: None,
                    op: CopyModeOp::Begin,
                }))
                .unwrap();
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 2,
                    visible_rows: None,
                    op: CopyModeOp::Move {
                        motion: CopyMotion::Top,
                        count: 1,
                    },
                }))
                .unwrap();
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 3,
                    visible_rows: None,
                    op: CopyModeOp::Select(CopyUnit::Char),
                }))
                .unwrap();
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 4,
                    visible_rows: None,
                    op: CopyModeOp::Move {
                        motion: CopyMotion::WordEnd,
                        count: 1,
                    },
                }))
                .unwrap();

            wait_for(&model, &wake_rx, |m| {
                m.frame.as_ref().is_some_and(|f| f.selection_revision == 4)
            })
            .await;
            {
                let m = model.lock().unwrap();
                let frame = m.frame.as_ref().unwrap();
                assert_eq!(
                    frame.copy,
                    Some(CopyView {
                        epoch: 1,
                        cursor: (4, 0),
                        wide: false,
                        unit: Some(CopyUnit::Char),
                        display_start: 0,
                    })
                );
                assert_eq!(frame.selection_text.as_deref(), Some("alpha"));
            }

            // 3. Back-to-back Yank (rev 5) + End (rev 6) delivers CopyYank without racing!
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 5,
                    visible_rows: None,
                    op: CopyModeOp::Yank,
                }))
                .unwrap();
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 6,
                    visible_rows: None,
                    op: CopyModeOp::End,
                }))
                .unwrap();
            wait_for(&model, &wake_rx, |m| {
                m.frame.as_ref().is_some_and(|f| f.selection_revision == 6)
            })
            .await;
            {
                let mut m = model.lock().unwrap();
                assert_eq!(m.frame.as_ref().unwrap().copy, None);
                let yank = m.copy_yank.take().expect("yank preserved across End");
                assert_eq!(
                    (yank.epoch, yank.revision, yank.text.as_str()),
                    (1, 5, "alpha")
                );
                assert_eq!(m.copy_ended_epoch, 0, "explicit End does not bump auto-end");
            }

            // 4. Re-enter copy mode (epoch 2), then stream alternate-screen switch over live PTY
            //    to verify automatic exit and monotonic copy_ended_epoch reporting.
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 2,
                    revision: 7,
                    visible_rows: None,
                    op: CopyModeOp::Begin,
                }))
                .unwrap();
            wait_for(&model, &wake_rx, |m| {
                m.frame.as_ref().is_some_and(|f| f.selection_revision == 7)
            })
            .await;
            assert!(model.lock().unwrap().frame.as_ref().unwrap().copy.is_some());

            push_tx.send(b"\x1b[?1049h".to_vec()).unwrap();
            wait_for(&model, &wake_rx, |m| m.copy_ended_epoch == 2).await;
            {
                let m = model.lock().unwrap();
                assert_eq!(m.frame.as_ref().unwrap().copy, None);
                assert_eq!(m.copy_ended_epoch, 2);
            }

            // 5. Disconnected forwarding covers Command::CopyMode too.
            let (fw_tx, fw_rx) = flume::unbounded();
            let workers = HashMap::from([(
                "b".to_owned(),
                PaneWorker {
                    session: "s".into(),
                    commands: fw_tx,
                    task: tokio::task::spawn_local(async {}),
                    size: None,
                },
            )]);
            let sample = CopyModeInput {
                generation,
                epoch: 3,
                revision: 8,
                visible_rows: None,
                op: CopyModeOp::Begin,
            };
            assert!(forward_search(&workers, "b", &Command::CopyMode(sample)));
            assert!(matches!(
                fw_rx.try_recv(),
                Ok(DataCommand::CopyMode(got)) if got == sample
            ));

            drop(push_tx);
            cmd_tx.send(DataCommand::Stop).unwrap();
            worker.await.unwrap();
        })
        .await;

    ticket_task.await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn copy_mode_and_pointer_selection_coordinate_without_clobber_or_orphans() {
    use crate::terminal::{
        copy_mode::{CopyModeInput, CopyModeOp, CopyMotion, CopyUnit},
        selection::{Geometry, Input as SelInput, Phase},
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());

    let mut source = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
        cols: 20,
        rows: 4,
        max_scrollback: 1 << 16,
    })
    .unwrap();
    for i in 0..20 {
        source.vt_write(format!("line-{i:02} alpha beta\r\n").as_bytes());
    }
    let snapshot = ghostty_vt::snapshot::encode(&source).unwrap().to_vec();
    drop(source);

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, wire::PROTOCOL).await;
        let _hello = ws.next().await.unwrap().unwrap().into_data();
        let mut welcome = vec![0u8; 32];
        welcome[0] = 0x81;
        welcome[24] = 1;
        ws.send(Message::Binary(welcome.into())).await.unwrap();
        ws.send(Message::Binary(snapshot.into())).await.unwrap();
        while ws.next().await.is_some() {}
    });

    let (host_tx, host_rx) = flume::unbounded::<HostCommand>();
    let ticket_task = tokio::spawn(async move {
        if let Ok(HostCommand::Ticket { reply, .. }) = host_rx.recv_async().await {
            let _ = reply.send(Ok(TerminalTicket {
                token: "AAAAAAAAAAAAAAAAAAAAAA".into(),
                bundled: false,
            }));
        }
    });

    let model = Arc::new(Mutex::new(Model::default()));
    let (wake_tx, wake_rx) = flume::bounded(1);
    let (host_wake_tx, _host_wake_rx) = flume::bounded(1);
    let (cmd_tx, cmd_rx) = flume::unbounded::<DataCommand>();
    let pane = PaneShared {
        model: model.clone(),
        wake: wake_tx,
        host_wake: host_wake_tx,
    };

    async fn wait_for(
        model: &Arc<Mutex<Model>>,
        wake_rx: &flume::Receiver<()>,
        pred: impl Fn(&Model) -> bool,
    ) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if pred(&model.lock().unwrap()) {
                    return;
                }
                wake_rx.recv_async().await.unwrap();
            }
        })
        .await
        .unwrap();
    }

    let geom = Geometry {
        columns: 20,
        cell_width: 10,
        padding_left: 0,
        screen_height: 80,
    };

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let worker = tokio::task::spawn_local(run_data(
                endpoint,
                SESSION.into(),
                "block:coord".into(),
                host_tx,
                cmd_rx,
                pane,
            ));

            wait_for(&model, &wake_rx, |m| m.connected && m.frame.is_some()).await;
            let generation = model.lock().unwrap().restore_generation;

            // (a) Active edge pointer drag (which would autoscroll) -> CopyMode::Begin stops the drag.
            cmd_tx
                .send(DataCommand::Selection(SelInput {
                    generation,
                    revision: 1,
                    phase: Phase::Down(1),
                    at: (0, 2),
                    position: (0., 40.),
                    geometry: geom,
                }))
                .unwrap();
            cmd_tx
                .send(DataCommand::Selection(SelInput {
                    generation,
                    revision: 2,
                    phase: Phase::Move,
                    at: (0, 0),
                    position: (0., -20.),
                    geometry: geom,
                }))
                .unwrap();
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 3,
                    visible_rows: None,
                    op: CopyModeOp::Begin,
                }))
                .unwrap();
            wait_for(&model, &wake_rx, |m| {
                m.frame.as_ref().is_some_and(|f| f.selection_revision == 3)
            })
            .await;
            let cursor_after_begin = model.lock().unwrap().frame.as_ref().unwrap().copy.unwrap();
            assert_eq!(
                model.lock().unwrap().frame.as_ref().unwrap().selection_text,
                None,
                "Begin clears prior pointer selection and stops drag"
            );

            // (b) Select(Char) in copy mode -> stray Selection(Phase::Up) and idle Phase::Move
            //     must NOT drop copy mode's ownership or leave an orphaned Ghostty highlight on End.
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 4,
                    visible_rows: None,
                    op: CopyModeOp::Move {
                        motion: CopyMotion::Top,
                        count: 1,
                    },
                }))
                .unwrap();
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 5,
                    visible_rows: None,
                    op: CopyModeOp::Select(CopyUnit::Char),
                }))
                .unwrap();
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 6,
                    visible_rows: None,
                    op: CopyModeOp::Move {
                        motion: CopyMotion::WordEnd,
                        count: 1,
                    },
                }))
                .unwrap();
            // Stray Phase::Up + idle Phase::Move while no drag is active:
            cmd_tx
                .send(DataCommand::Selection(SelInput {
                    generation,
                    revision: 7,
                    phase: Phase::Up,
                    at: (0, 0),
                    position: (0., 0.),
                    geometry: geom,
                }))
                .unwrap();
            cmd_tx
                .send(DataCommand::Selection(SelInput {
                    generation,
                    revision: 8,
                    phase: Phase::Move,
                    at: (2, 0),
                    position: (20., 0.),
                    geometry: geom,
                }))
                .unwrap();
            wait_for(&model, &wake_rx, |m| {
                m.frame.as_ref().is_some_and(|f| f.selection_revision == 8)
            })
            .await;
            {
                let m = model.lock().unwrap();
                let frame = m.frame.as_ref().unwrap();
                assert_eq!(frame.copy.unwrap().unit, Some(CopyUnit::Char));
                assert_eq!(frame.selection_text.as_deref(), Some("line"));
                assert_eq!(cursor_after_begin.epoch, 1);
            }

            // Now End copy mode: Ghostty's selection MUST be cleared (no orphan highlight).
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 9,
                    visible_rows: None,
                    op: CopyModeOp::End,
                }))
                .unwrap();
            wait_for(&model, &wake_rx, |m| {
                m.frame.as_ref().is_some_and(|f| f.selection_revision == 9)
            })
            .await;
            {
                let m = model.lock().unwrap();
                let frame = m.frame.as_ref().unwrap();
                assert_eq!(frame.copy, None);
                assert_eq!(
                    frame.selection_text, None,
                    "End clears copy-mode selection cleanly after stray pointer Up/Move"
                );
            }

            cmd_tx.send(DataCommand::Stop).unwrap();
            worker.await.unwrap();
        })
        .await;

    ticket_task.await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn copy_mode_search_to_match_and_steps_settle_when_queued_without_pre_tick() {
    use crate::terminal::copy_mode::{CopyModeInput, CopyModeOp, CopyMotion};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());

    let mut source = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
        cols: 30,
        rows: 6,
        max_scrollback: 1 << 16,
    })
    .unwrap();
    source.vt_write(b"needle-01 top\r\nfiller\r\nneedle-02 mid\r\nfiller\r\nneedle-03 bot");
    let snapshot = ghostty_vt::snapshot::encode(&source).unwrap().to_vec();
    drop(source);

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, wire::PROTOCOL).await;
        let _hello = ws.next().await.unwrap().unwrap().into_data();
        let mut welcome = vec![0u8; 32];
        welcome[0] = 0x81;
        welcome[24] = 1;
        ws.send(Message::Binary(welcome.into())).await.unwrap();
        ws.send(Message::Binary(snapshot.into())).await.unwrap();
        while ws.next().await.is_some() {}
    });

    let (host_tx, host_rx) = flume::unbounded::<HostCommand>();
    let ticket_task = tokio::spawn(async move {
        if let Ok(HostCommand::Ticket { reply, .. }) = host_rx.recv_async().await {
            let _ = reply.send(Ok(TerminalTicket {
                token: "AAAAAAAAAAAAAAAAAAAAAA".into(),
                bundled: false,
            }));
        }
    });

    let model = Arc::new(Mutex::new(Model::default()));
    let (wake_tx, wake_rx) = flume::bounded(1);
    let (host_wake_tx, _host_wake_rx) = flume::bounded(1);
    let (cmd_tx, cmd_rx) = flume::unbounded::<DataCommand>();
    let pane = PaneShared {
        model: model.clone(),
        wake: wake_tx,
        host_wake: host_wake_tx,
    };

    async fn wait_for(
        model: &Arc<Mutex<Model>>,
        wake_rx: &flume::Receiver<()>,
        pred: impl Fn(&Model) -> bool,
    ) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if pred(&model.lock().unwrap()) {
                    return;
                }
                wake_rx.recv_async().await.unwrap();
            }
        })
        .await
        .unwrap();
    }

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let worker = tokio::task::spawn_local(run_data(
                endpoint,
                SESSION.into(),
                "block:search-race".into(),
                host_tx,
                cmd_rx,
                pane,
            ));

            wait_for(&model, &wake_rx, |m| m.connected && m.frame.is_some()).await;
            let generation = model.lock().unwrap().restore_generation;

            // Enter copy mode and move cursor away from the bottom match to row 0.
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 1,
                    visible_rows: None,
                    op: CopyModeOp::Begin,
                }))
                .unwrap();
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 2,
                    visible_rows: None,
                    op: CopyModeOp::Move {
                        motion: CopyMotion::Top,
                        count: 1,
                    },
                }))
                .unwrap();
            wait_for(&model, &wake_rx, |m| {
                m.frame.as_ref().is_some_and(|f| f.selection_revision == 2)
            })
            .await;

            // Case 1: Queue DataCommand::Search + CopyModeOp::ToMatch in the same burst
            // with zero pre-tick (mirrors `/` + fast typing + Enter within 16ms).
            cmd_tx
                .send(DataCommand::Search("needle-03".into()))
                .unwrap();
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 3,
                    visible_rows: None,
                    op: CopyModeOp::ToMatch,
                }))
                .unwrap();
            wait_for(&model, &wake_rx, |m| {
                m.frame.as_ref().is_some_and(|f| {
                    f.selection_revision == 3
                        && f.search
                            .as_ref()
                            .is_some_and(|s| s.total == 1 && s.selected.is_some())
                })
            })
            .await;
            {
                let m = model.lock().unwrap();
                let frame = m.frame.as_ref().unwrap();
                let sel = frame.search.as_ref().unwrap().selected.unwrap();
                assert_eq!(
                    frame.copy.unwrap().cursor,
                    sel.start,
                    "ToMatch queued immediately after Search must jump once search settles"
                );
            }

            // Case 2: Queue DataCommand::Search + ToMatch + SearchStep(true) in one burst
            // (mirrors `2n` re-opening `find.last` without an open find bar).
            cmd_tx.send(DataCommand::Search("needle".into())).unwrap();
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 4,
                    visible_rows: None,
                    op: CopyModeOp::ToMatch,
                }))
                .unwrap();
            cmd_tx.send(DataCommand::SearchStep(true)).unwrap();
            wait_for(&model, &wake_rx, |m| {
                m.frame.as_ref().is_some_and(|f| {
                    f.selection_revision == 4
                        && f.search
                            .as_ref()
                            .is_some_and(|s| s.total == 3 && s.index == Some(1))
                })
            })
            .await;
            {
                let m = model.lock().unwrap();
                let frame = m.frame.as_ref().unwrap();
                let sel = frame.search.as_ref().unwrap().selected.unwrap();
                assert_eq!(
                    frame.copy.unwrap().cursor,
                    sel.start,
                    "queued SearchStep after Search+ToMatch must land on index 1"
                );
                assert_eq!(sel.start.1, 2, "needle-02 is on row 2");
            }

            // Case 3: A manual motion queued before the search settles cancels the pending jump.
            cmd_tx
                .send(DataCommand::Search("needle-03".into()))
                .unwrap();
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 5,
                    visible_rows: None,
                    op: CopyModeOp::ToMatch,
                }))
                .unwrap();
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 6,
                    visible_rows: None,
                    op: CopyModeOp::Move {
                        motion: CopyMotion::Top,
                        count: 1,
                    },
                }))
                .unwrap();
            wait_for(&model, &wake_rx, |m| {
                m.frame.as_ref().is_some_and(|f| {
                    f.selection_revision == 6 && f.search.as_ref().is_some_and(|s| s.total == 1)
                })
            })
            .await;
            {
                let m = model.lock().unwrap();
                let frame = m.frame.as_ref().unwrap();
                assert_eq!(
                    frame.copy.unwrap().cursor.1,
                    0,
                    "subsequent Move must cancel pending search jump"
                );
            }

            cmd_tx.send(DataCommand::Stop).unwrap();
            worker.await.unwrap();
        })
        .await;

    ticket_task.await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn search_step_with_copy_mode_off_is_never_dropped_while_unsettled() {
    use crate::terminal::copy_mode::{CopyModeInput, CopyModeOp};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());

    let mut source = ghostty_vt::Terminal::new(ghostty_vt::TerminalOptions {
        cols: 30,
        rows: 6,
        max_scrollback: 1 << 16,
    })
    .unwrap();
    source.vt_write(b"needle-01 top\r\nfiller\r\nneedle-02 mid\r\nfiller\r\nneedle-03 bot");
    let snapshot = ghostty_vt::snapshot::encode(&source).unwrap().to_vec();
    drop(source);

    let (push_tx, mut push_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, wire::PROTOCOL).await;
        let _hello = ws.next().await.unwrap().unwrap().into_data();
        let mut welcome = vec![0u8; 32];
        welcome[0] = 0x81;
        welcome[24] = 1;
        ws.send(Message::Binary(welcome.into())).await.unwrap();
        ws.send(Message::Binary(snapshot.into())).await.unwrap();
        while let Some(frame) = push_rx.recv().await {
            ws.send(Message::Binary(frame.into())).await.unwrap();
        }
        while ws.next().await.is_some() {}
    });

    let (host_tx, host_rx) = flume::unbounded::<HostCommand>();
    let ticket_task = tokio::spawn(async move {
        if let Ok(HostCommand::Ticket { reply, .. }) = host_rx.recv_async().await {
            let _ = reply.send(Ok(TerminalTicket {
                token: "AAAAAAAAAAAAAAAAAAAAAA".into(),
                bundled: false,
            }));
        }
    });

    let model = Arc::new(Mutex::new(Model::default()));
    let (wake_tx, wake_rx) = flume::bounded(1);
    let (host_wake_tx, _host_wake_rx) = flume::bounded(1);
    let (cmd_tx, cmd_rx) = flume::unbounded::<DataCommand>();
    let pane = PaneShared {
        model: model.clone(),
        wake: wake_tx,
        host_wake: host_wake_tx,
    };

    async fn wait_for(
        model: &Arc<Mutex<Model>>,
        wake_rx: &flume::Receiver<()>,
        pred: impl Fn(&Model) -> bool,
    ) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if pred(&model.lock().unwrap()) {
                    return;
                }
                wake_rx.recv_async().await.unwrap();
            }
        })
        .await
        .unwrap();
    }

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let worker = tokio::task::spawn_local(run_data(
                endpoint,
                SESSION.into(),
                "block:search-off".into(),
                host_tx,
                cmd_rx,
                pane,
            ));

            wait_for(&model, &wake_rx, |m| m.connected && m.frame.is_some()).await;
            let generation = model.lock().unwrap().restore_generation;

            // (a) Copy mode OFF: establish an active search (`total == 3`, `index == Some(0)`).
            cmd_tx.send(DataCommand::Search("needle".into())).unwrap();
            wait_for(&model, &wake_rx, |m| {
                m.frame.as_ref().is_some_and(|f| {
                    f.search
                        .as_ref()
                        .is_some_and(|s| s.total == 3 && s.index == Some(0))
                })
            })
            .await;

            // Re-send `Search("needle")` (which marks `settled = false` while keeping the active
            // `Search` instance) followed immediately by `SearchStep(true)` in the same burst,
            // plus streaming terminal output, with copy mode OFF. The step must NOT be dropped.
            push_tx.send(b"\r\nextra output".to_vec()).unwrap();
            cmd_tx.send(DataCommand::Search("needle".into())).unwrap();
            cmd_tx.send(DataCommand::SearchStep(true)).unwrap();
            wait_for(&model, &wake_rx, |m| {
                m.frame.as_ref().is_some_and(|f| {
                    f.copy.is_none()
                        && f.search
                            .as_ref()
                            .is_some_and(|s| s.total == 3 && s.index == Some(1))
                })
            })
            .await;

            // (b) Copy mode ON: `Search` + `ToMatch` + `SearchStep(true)` queued while unsettled
            // still queues and settles cleanly onto index 1 with the copy cursor on that match.
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 1,
                    visible_rows: None,
                    op: CopyModeOp::Begin,
                }))
                .unwrap();
            cmd_tx.send(DataCommand::Search(String::new())).unwrap();
            cmd_tx.send(DataCommand::Search("needle".into())).unwrap();
            cmd_tx
                .send(DataCommand::CopyMode(CopyModeInput {
                    generation,
                    epoch: 1,
                    revision: 2,
                    visible_rows: None,
                    op: CopyModeOp::ToMatch,
                }))
                .unwrap();
            cmd_tx.send(DataCommand::SearchStep(true)).unwrap();
            wait_for(&model, &wake_rx, |m| {
                m.frame.as_ref().is_some_and(|f| {
                    f.selection_revision == 2
                        && f.copy.is_some()
                        && f.search
                            .as_ref()
                            .is_some_and(|s| s.total == 3 && s.index == Some(1))
                })
            })
            .await;
            {
                let m = model.lock().unwrap();
                let frame = m.frame.as_ref().unwrap();
                let sel = frame.search.as_ref().unwrap().selected.unwrap();
                assert_eq!(frame.copy.unwrap().cursor, sel.start);
            }

            drop(push_tx);
            cmd_tx.send(DataCommand::Stop).unwrap();
            worker.await.unwrap();
        })
        .await;

    ticket_task.await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn client_prompt_requested_refuses_immediately_and_prompt_ended_is_quiet() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 21).await;
        let boot_req = recv(&mut ws).await;
        assert_eq!(boot_req["method"], "session.bootstrap");
        // Send a prompt BEFORE bootstrap finishes with seq below bootstrap's cutoff (seq 1 < 10).
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 1,
                "payload": {
                    "type": "client_prompt_requested",
                    "payload": {
                        "call_id": "prompt-early",
                        "action": "git.branch",
                        "prompt": {"kind": "input", "title": "Branch"}
                    }
                }
            }),
        )
        .await;
        reply(
            &mut ws,
            &boot_req,
            10,
            serde_json::to_value(boot()).unwrap(),
        )
        .await;

        let first_reply = recv(&mut ws).await;
        assert_eq!(first_reply["method"], "client.reply");
        assert_eq!(first_reply["payload"]["call_id"], "prompt-early");
        assert_eq!(first_reply["payload"]["error"]["code"], "not_supported");
        assert!(
            first_reply["payload"]["error"]["message"]
                .as_str()
                .unwrap()
                .contains("git.branch")
        );
        // Simulate another client having answered first -> `not_found` must not disconnect.
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": first_reply["id"],
                "seq": 11,
                "error": {"code": "not_found", "message": "already answered", "status": 404}
            }),
        )
        .await;

        // `client_prompt_ended` is ignored quietly; a second prompt still answers cleanly.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 12,
                "payload": {
                    "type": "client_prompt_ended",
                    "payload": {"call_id": "prompt-early", "reason": "answered"}
                }
            }),
        )
        .await;
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 13,
                "payload": {
                    "type": "client_prompt_requested",
                    "payload": {
                        "call_id": "prompt-live",
                        "prompt": {"kind": "pick", "title": "Choice", "items": []}
                    }
                }
            }),
        )
        .await;
        let second_reply = recv(&mut ws).await;
        assert_eq!(second_reply["method"], "client.reply");
        assert_eq!(second_reply["payload"]["call_id"], "prompt-live");
        assert_eq!(second_reply["payload"]["error"]["code"], "not_supported");
        reply(&mut ws, &second_reply, 14, json!({})).await;
        done_tx.send(()).unwrap();
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    tokio::time::timeout(Duration::from_secs(5), done_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(host.model.lock().unwrap().connected);
    assert!(host.model.lock().unwrap().performs.is_empty());
    drop(host);
    server.await.unwrap();
}

/// Receive the next control request, transparently answering any interleaved background
/// `status_sweep` reads (`session.view` or `com.superlogical.terminal.program_status`) that
/// `handle_bg` recognizes so mock tests never race with paced sweep traffic.
async fn recv_answering_sweep(
    ws: &mut WebSocketStream<TcpStream>,
    handle_bg: &mut impl FnMut(&Value) -> Option<(u64, Value)>,
) -> Value {
    loop {
        let req = recv(ws).await;
        if let Some((seq, payload)) = handle_bg(&req) {
            reply(ws, &req, seq, payload).await;
            continue;
        }
        return req;
    }
}

/// Drain frames until the client closes the WebSocket, answering any trailing background
/// sweep requests that were already queued when the test dropped the host.
async fn expect_close_answering_sweep(
    ws: &mut WebSocketStream<TcpStream>,
    handle_bg: &mut impl FnMut(&Value) -> Option<(u64, Value)>,
) {
    use std::io::ErrorKind;
    use tokio_tungstenite::tungstenite::error::{Error as WsError, ProtocolError};

    loop {
        match ws.next().await {
            None
            | Some(Ok(Message::Close(_)))
            | Some(Err(WsError::Protocol(ProtocolError::ResetWithoutClosingHandshake))) => return,
            Some(Err(WsError::Io(err)))
                if matches!(
                    err.kind(),
                    ErrorKind::ConnectionAborted
                        | ErrorKind::ConnectionReset
                        | ErrorKind::BrokenPipe
                        | ErrorKind::UnexpectedEof
                ) =>
            {
                return;
            }
            Some(Ok(Message::Text(text))) => {
                let req: Value = serde_json::from_str(&text).unwrap();
                if let Some((seq, payload)) = handle_bg(&req) {
                    let msg = json!({"type":"response","id":req["id"],"seq":seq,"payload":payload});
                    if ws
                        .send(Message::Text(msg.to_string().into()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
            Some(Ok(_)) => {}
            other => panic!("expected WebSocket Close frame, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn event_dropped_recovers_session_list_keymap_and_hosts() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (hh1_go_tx, hh1_go_rx) = tokio::sync::oneshot::channel();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 21).await;
        let boot_req = recv(&mut ws).await;
        let mut initial = boot();
        initial.selected_session.as_mut().unwrap().view.windows = vec![
            serde_json::from_value(json!({
                "window_id": "w1",
                "focused_block_id": "b1",
                "layers": [{
                    "kind": "tiled",
                    "bounds": [0, 0, 1, 1],
                    "layout": {"block_id": "b1"},
                    "blocks": [{"block_id": "b1", "creator_name": "com.superlogical.terminal"}]
                }]
            }))
            .unwrap(),
        ];
        reply(
            &mut ws,
            &boot_req,
            10,
            serde_json::to_value(initial).unwrap(),
        )
        .await;

        // Initial activity reads for b1.
        for expected in [
            "com.superlogical.terminal.program_status",
            "com.superlogical.terminal.title",
            "com.superlogical.terminal.process",
        ] {
            let req = recv(&mut ws).await;
            assert_eq!(req["method"], expected);
            let payload = match expected {
                "com.superlogical.terminal.title" => json!({"title": "before"}),
                _ => json!({}),
            };
            reply(&mut ws, &req, 10, payload).await;
        }

        // Keymap registration and WatchSharedHosts initial read.
        let km_reply = |hash: &str, generation: u64| {
            let mut resp = rexrs::api::client::KeymapResponse::default();
            resp.config.hash = hash.into();
            resp.config.generation = generation;
            resp.config.instance_id = "inst".into();
            serde_json::to_value(resp).unwrap()
        };
        let km_req = recv(&mut ws).await;
        assert_eq!(km_req["method"], "client.keymap");
        reply(&mut ws, &km_req, 11, km_reply("h1", 1)).await;
        let hosts_req = recv(&mut ws).await;
        assert_eq!(hosts_req["method"], "hosts.list");
        reply(&mut ws, &hosts_req, 11, json!({"hash": "hh1", "hosts": []})).await;
        // Wait for the client to observe `hh1` before sending the live status and drop sequence.
        hh1_go_rx.await.unwrap();

        // Deliver a live status_changed record on b1 so it is marked `live: true`. A dropped
        // StatusRemoved afterwards must be cleared when program_status is re-read as empty.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 12,
                "payload": {
                    "type": "session_event",
                    "payload": {
                        "session_id": SESSION,
                        "event": {
                            "type": "block_event",
                            "payload": {
                                "block_id": "b1",
                                "creator_name": "com.superlogical.terminal",
                                "name": "program_status_changed",
                                "payload": {
                                    "id": "",
                                    "record": {"state": "working", "app": "build"}
                                }
                            }
                        }
                    }
                }
            }),
        )
        .await;

        // 1. Session-scoped event_dropped -> refreshes view, downgrades live status records,
        //    and re-reads b1 metadata.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 13,
                "payload": {
                    "type": "event_dropped",
                    "payload": {
                        "event_type": "block_event",
                        "session_id": SESSION,
                        "block_id": "b1",
                        "size": 100000
                    }
                }
            }),
        )
        .await;
        let view_req = recv(&mut ws).await;
        assert_eq!(view_req["method"], "session.view");
        assert_eq!(view_req["payload"]["session_id"], SESSION);
        reply(
            &mut ws,
            &view_req,
            14,
            json!({
                "session_id": SESSION,
                "label": "snapshot",
                "revision": 2,
                "windows": [{
                    "window_id": "w1",
                    "focused_block_id": "b1",
                    "layers": [{
                        "kind": "tiled",
                        "bounds": [0, 0, 1, 1],
                        "layout": {"block_id": "b1"},
                        "blocks": [{"block_id": "b1", "creator_name": "com.superlogical.terminal"}]
                    }]
                }]
            }),
        )
        .await;
        for expected in [
            "com.superlogical.terminal.program_status",
            "com.superlogical.terminal.title",
            "com.superlogical.terminal.process",
        ] {
            let req = recv(&mut ws).await;
            assert_eq!(req["method"], expected);
            let payload = match expected {
                "com.superlogical.terminal.title" => json!({"title": "after-drop"}),
                _ => json!({}),
            };
            reply(&mut ws, &req, 14, payload).await;
        }

        // 2. Server-wide session_* event_dropped -> re-lists sessions.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 15,
                "payload": {
                    "type": "event_dropped",
                    "payload": {"event_type": "session_created", "size": 100000}
                }
            }),
        )
        .await;
        let list_req = recv(&mut ws).await;
        assert_eq!(list_req["method"], "session.attach_all");
        reply(
            &mut ws,
            &list_req,
            16,
            json!({"sessions": [
                {"session_id": SESSION, "label": "snapshot"},
                {"session_id": OTHER, "label": "other"}
            ]}),
        )
        .await;

        let mut bg_other = |req: &Value| -> Option<(u64, Value)> {
            let method = req["method"].as_str()?;
            if method == "session.view" && req["payload"]["session_id"] == OTHER {
                return Some((
                    16,
                    json!({"session_id": OTHER, "revision": 1, "windows": []}),
                ));
            }
            if method == "com.superlogical.terminal.program_status"
                && req["payload"]["session_id"] == OTHER
            {
                return Some((16, json!({})));
            }
            None
        };

        // 3. Server-wide keymap_changed event_dropped -> refreshes keymap.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 17,
                "payload": {
                    "type": "event_dropped",
                    "payload": {"event_type": "keymap_changed", "size": 100000}
                }
            }),
        )
        .await;
        let km_req2 = recv_answering_sweep(&mut ws, &mut bg_other).await;
        assert_eq!(km_req2["method"], "client.keymap");
        reply(&mut ws, &km_req2, 18, km_reply("h2", 2)).await;

        // 4. Server-wide hosts_changed event_dropped -> re-reads hosts.list.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 19,
                "payload": {
                    "type": "event_dropped",
                    "payload": {"event_type": "hosts_changed", "size": 100000}
                }
            }),
        )
        .await;
        let hosts_req2 = recv_answering_sweep(&mut ws, &mut bg_other).await;
        assert_eq!(hosts_req2["method"], "hosts.list");
        reply(
            &mut ws,
            &hosts_req2,
            20,
            json!({"hash": "hh2", "hosts": []}),
        )
        .await;

        done_tx.send(()).unwrap();
        expect_close_answering_sweep(&mut ws, &mut bg_other).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    until(&host, |m| m.title("b1") == Some("before")).await;
    host.commands
        .send(HostCommand::Keymap(Box::new(keymap::Declaration {
            generation: 1,
            request: rexrs::api::client::Keymap::new(),
            overrides: crate::keymap::Overrides::default(),
        })))
        .unwrap();
    until(&host, |m| {
        m.keymap
            .as_ref()
            .is_some_and(|k| k.reply.config.hash == "h1")
    })
    .await;
    host.commands.send(HostCommand::WatchSharedHosts).unwrap();
    until(&host, |m| m.shared.hash() == Some("hh1")).await;
    hh1_go_tx.send(()).unwrap();

    tokio::time::timeout(Duration::from_secs(5), done_rx)
        .await
        .unwrap()
        .unwrap();
    until(&host, |m| {
        m.title("b1") == Some("after-drop")
            && !m.activity.contains_key("b1")
            && m.sessions.len() == 2
            && m.keymap
                .as_ref()
                .is_some_and(|k| k.reply.config.hash == "h2")
            && m.shared.hash() == Some("hh2")
    })
    .await;
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn event_dropped_with_session_id_routes_list_level_session_events() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (step1_tx, step1_rx) = tokio::sync::oneshot::channel();
    let (step1_go_tx, step1_go_rx) = tokio::sync::oneshot::channel();
    let (step2_go_tx, step2_go_rx) = tokio::sync::oneshot::channel();
    let (step3_go_tx, step3_go_rx) = tokio::sync::oneshot::channel();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 21).await;
        let boot_req = recv(&mut ws).await;
        reply(
            &mut ws,
            &boot_req,
            10,
            serde_json::to_value(boot()).unwrap(),
        )
        .await;

        // 1. `event_dropped` for `session_created` WITH `payload.session_id` populated must
        //    call `session.attach_all` (not `session.view`) so `m.sessions` learns about OTHER.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 11,
                "payload": {
                    "type": "event_dropped",
                    "payload": {
                        "event_type": "session_created",
                        "session_id": OTHER,
                        "size": 100000
                    }
                }
            }),
        )
        .await;
        let list_req = recv(&mut ws).await;
        assert_eq!(list_req["method"], "session.attach_all");
        reply(
            &mut ws,
            &list_req,
            12,
            json!({"sessions": [
                {"session_id": SESSION, "label": "snapshot"},
                {"session_id": OTHER, "label": "other"}
            ]}),
        )
        .await;
        // Let the initial background sweep probe OTHER and fail so OTHER stays unplaced
        // (in `m.sessions`, absent from `m.views`, marked in `state.viewed`). Under the old
        // bug, `event_dropped(session_destroyed, session_id=OTHER)` cleared `state.viewed`
        // without removing OTHER from `m.sessions`, triggering a second `session.view` poll.
        let initial_other_view = recv(&mut ws).await;
        assert_eq!(initial_other_view["method"], "session.view");
        assert_eq!(initial_other_view["payload"]["session_id"], OTHER);
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": initial_other_view["id"],
                "seq": 12,
                "error": {"code": "internal", "message": "transient", "status": 500}
            }),
        )
        .await;
        step1_tx.send(()).unwrap();
        step1_go_rx.await.unwrap();

        // 2. `event_dropped` for `session_destroyed` WITH `payload.session_id` on unplaced
        //    session OTHER must remove OTHER on its own (without waiting for a later
        //    `session.attach_all` and without polling `session.view` for OTHER again).
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 13,
                "payload": {
                    "type": "event_dropped",
                    "payload": {
                        "event_type": "session_destroyed",
                        "session_id": OTHER,
                        "size": 100000
                    }
                }
            }),
        )
        .await;
        step2_go_rx.await.unwrap();

        // 3. `session_label_changed` with `session_id` re-lists and updates cached view label.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 14,
                "payload": {
                    "type": "event_dropped",
                    "payload": {
                        "event_type": "session_label_changed",
                        "session_id": SESSION,
                        "size": 100000
                    }
                }
            }),
        )
        .await;
        let mut saw_other_view_after_destroy = false;
        let mut bg = |req: &Value| -> Option<(u64, Value)> {
            if req["method"] == "session.view" && req["payload"]["session_id"] == OTHER {
                saw_other_view_after_destroy = true;
                return Some((
                    13,
                    json!({"session_id": OTHER, "revision": 1, "windows": []}),
                ));
            }
            None
        };
        let list_req2 = recv_answering_sweep(&mut ws, &mut bg).await;
        assert_eq!(list_req2["method"], "session.attach_all");
        reply(
            &mut ws,
            &list_req2,
            15,
            json!({"sessions": [{"session_id": SESSION, "label": "renamed-via-drop"}]}),
        )
        .await;
        step3_go_rx.await.unwrap();

        // 4. Unnamed `session_label_changed` drop (no `session_id`) also reconciles cached
        //    view labels from the re-listed sessions.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 16,
                "payload": {
                    "type": "event_dropped",
                    "payload": {
                        "event_type": "session_label_changed",
                        "size": 100000
                    }
                }
            }),
        )
        .await;
        let list_req3 = recv_answering_sweep(&mut ws, &mut bg).await;
        assert_eq!(list_req3["method"], "session.attach_all");
        reply(
            &mut ws,
            &list_req3,
            17,
            json!({"sessions": [{"session_id": SESSION, "label": "renamed-unnamed-drop"}]}),
        )
        .await;
        done_tx.send(()).unwrap();
        expect_close_answering_sweep(&mut ws, &mut bg).await;
        assert!(
            !saw_other_view_after_destroy,
            "destroyed unplaced session must not be queried with session.view"
        );
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    tokio::time::timeout(Duration::from_secs(5), step1_rx)
        .await
        .unwrap()
        .unwrap();
    until(&host, |m| {
        m.sessions.iter().any(|s| s.session_id == OTHER) && !m.views.contains_key(OTHER)
    })
    .await;
    step1_go_tx.send(()).unwrap();
    // Prove `session_destroyed` itself removed OTHER before `session_label_changed` is sent.
    until(&host, |m| !m.sessions.iter().any(|s| s.session_id == OTHER)).await;
    step2_go_tx.send(()).unwrap();
    until(&host, |m| {
        m.sessions
            .iter()
            .any(|s| s.session_id == SESSION && s.label == "renamed-via-drop")
            && m.views.get(SESSION).and_then(|v| v.label.as_deref()) == Some("renamed-via-drop")
    })
    .await;
    step3_go_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), done_rx)
        .await
        .unwrap()
        .unwrap();
    until(&host, |m| {
        m.sessions
            .iter()
            .any(|s| s.session_id == SESSION && s.label == "renamed-unnamed-drop")
            && m.views.get(SESSION).and_then(|v| v.label.as_deref()) == Some("renamed-unnamed-drop")
    })
    .await;
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn recreated_block_and_session_ids_reread_layout_and_activity() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (phase1_tx, phase1_rx) = tokio::sync::oneshot::channel();
    let (proceed_tx, proceed_rx) = tokio::sync::oneshot::channel();
    let (phase2_tx, phase2_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 21).await;
        let boot_req = recv(&mut ws).await;
        let mut initial = boot();
        let view_with_b1 = |rev: u64| {
            json!({
                "session_id": SESSION,
                "label": "snapshot",
                "revision": rev,
                "windows": [{
                    "window_id": "w1",
                    "focused_block_id": "b1",
                    "layers": [{
                        "kind": "tiled",
                        "bounds": [0, 0, 1, 1],
                        "layout": {"block_id": "b1"},
                        "blocks": [{"block_id": "b1", "creator_name": "com.superlogical.terminal"}]
                    }]
                }]
            })
        };
        initial.selected_session.as_mut().unwrap().view =
            serde_json::from_value(view_with_b1(1)).unwrap();
        reply(
            &mut ws,
            &boot_req,
            10,
            serde_json::to_value(initial).unwrap(),
        )
        .await;

        // Initial b1 queries.
        for expected in [
            "com.superlogical.terminal.program_status",
            "com.superlogical.terminal.title",
            "com.superlogical.terminal.process",
        ] {
            let req = recv(&mut ws).await;
            assert_eq!(req["method"], expected);
            let payload = match expected {
                "com.superlogical.terminal.title" => json!({"title": "first"}),
                _ => json!({}),
            };
            reply(&mut ws, &req, 10, payload).await;
        }

        // 1. `block_closed` clears b1's once-read marks; when b1 returns (undone close),
        //    the client must re-query b1's status/title/process.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 11,
                "payload": {
                    "type": "session_event",
                    "payload": {
                        "session_id": SESSION,
                        "event": {"type": "block_closed", "payload": {"block_id": "b1"}}
                    }
                }
            }),
        )
        .await;
        let view_req = recv(&mut ws).await;
        assert_eq!(view_req["method"], "session.view");
        reply(
            &mut ws,
            &view_req,
            12,
            json!({"session_id": SESSION, "label": "snapshot", "revision": 2, "windows": []}),
        )
        .await;
        // Now b1 comes back with the same ID via session_view_changed.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 13,
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
        let view_req2 = recv(&mut ws).await;
        assert_eq!(view_req2["method"], "session.view");
        reply(&mut ws, &view_req2, 14, view_with_b1(3)).await;
        for expected in [
            "com.superlogical.terminal.program_status",
            "com.superlogical.terminal.title",
            "com.superlogical.terminal.process",
        ] {
            let req = recv(&mut ws).await;
            assert_eq!(req["method"], expected);
            let payload = match expected {
                "com.superlogical.terminal.title" => json!({"title": "reopened-block"}),
                _ => json!({}),
            };
            reply(&mut ws, &req, 14, payload).await;
        }
        phase1_tx.send(()).unwrap();
        // Wait for the client test task to observe "reopened-block" before destroying SESSION.
        proceed_rx.await.unwrap();

        // 2. `session_destroyed` clears SESSION + b1; when SESSION is recreated with the same ID,
        //    selecting it re-reads its view and b1's metadata.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 15,
                "payload": {
                    "type": "session_destroyed",
                    "payload": {"session_id": SESSION}
                }
            }),
        )
        .await;
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 16,
                "payload": {
                    "type": "session_created",
                    "payload": {"session_id": SESSION}
                }
            }),
        )
        .await;
        let list_req = recv(&mut ws).await;
        assert_eq!(list_req["method"], "session.attach_all");
        reply(
            &mut ws,
            &list_req,
            17,
            json!({"sessions": [{"session_id": SESSION, "label": "snapshot"}]}),
        )
        .await;

        // Background sweep may read `session.view` and `program_status` for the recreated
        // session before or during `session.attach`; answer both transparently.
        let mut bg_session = |req: &Value| -> Option<(u64, Value)> {
            match req["method"].as_str()? {
                "session.view" => Some((17, view_with_b1(4))),
                "com.superlogical.terminal.program_status" => Some((17, json!({}))),
                _ => None,
            }
        };
        let attach_req = recv_answering_sweep(&mut ws, &mut bg_session).await;
        assert_eq!(attach_req["method"], "session.attach");
        assert_eq!(attach_req["payload"]["session_id"], SESSION);
        reply(
            &mut ws,
            &attach_req,
            18,
            json!({"client_id": "client:test", "view": view_with_b1(4)}),
        )
        .await;

        let mut saw_title = false;
        let mut saw_process = false;
        while !(saw_title && saw_process) {
            let req = recv_answering_sweep(&mut ws, &mut bg_session).await;
            match req["method"].as_str().unwrap_or("") {
                "com.superlogical.terminal.title" => {
                    saw_title = true;
                    reply(&mut ws, &req, 18, json!({"title": "reopened-session"})).await;
                }
                "com.superlogical.terminal.process" => {
                    saw_process = true;
                    reply(&mut ws, &req, 18, json!({})).await;
                }
                other => panic!("unexpected request after attach: {other}"),
            }
        }
        phase2_tx.send(()).unwrap();
        expect_close_answering_sweep(&mut ws, &mut bg_session).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    tokio::time::timeout(Duration::from_secs(5), phase1_rx)
        .await
        .unwrap()
        .unwrap();
    until(&host, |m| m.title("b1") == Some("reopened-block")).await;
    proceed_tx.send(()).unwrap();
    until(&host, |m| {
        m.selected.is_none() && m.sessions.iter().any(|s| s.session_id == SESSION)
    })
    .await;
    host.commands
        .send(HostCommand::Select(SESSION.into()))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), phase2_rx)
        .await
        .unwrap()
        .unwrap();
    until(&host, |m| m.title("b1") == Some("reopened-session")).await;
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn event_dropped_cancels_superseded_in_flight_status_sweep_read() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (req2_seen_tx, req2_seen_rx) = tokio::sync::oneshot::channel();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 21).await;
        let boot_req = recv(&mut ws).await;
        let mut initial = boot();
        initial.attach_all.sessions.push(session::Session {
            session_id: OTHER.into(),
            label: "other".into(),
        });
        reply(
            &mut ws,
            &boot_req,
            10,
            serde_json::to_value(initial).unwrap(),
        )
        .await;

        let other_view = |rev: u64| {
            json!({
                "session_id": OTHER,
                "label": "other",
                "revision": rev,
                "windows": [{
                    "window_id": "w2",
                    "focused_block_id": "b2",
                    "layers": [{
                        "kind": "tiled",
                        "bounds": [0, 0, 1, 1],
                        "layout": {"block_id": "b2"},
                        "blocks": [{"block_id": "b2", "creator_name": "com.superlogical.terminal"}]
                    }]
                }]
            })
        };

        // 1. Background sweep loads OTHER's view, then starts a detached `program_status` read
        //    for b2 (`req1`). Keep `req1` in flight without replying yet.
        let view_req1 = recv(&mut ws).await;
        assert_eq!(view_req1["method"], "session.view");
        assert_eq!(view_req1["payload"]["session_id"], OTHER);
        reply(&mut ws, &view_req1, 11, other_view(1)).await;

        let req1 = recv(&mut ws).await;
        assert_eq!(req1["method"], "com.superlogical.terminal.program_status");
        assert_eq!(req1["payload"]["session_id"], OTHER);
        assert_eq!(req1["payload"]["block_id"], "b2");

        // 2. While `req1` is in flight, b2 reports a record, removes it live, and then hits a
        //    session-scoped `event_dropped`. The client must cancel `req1` in `status_sweep`,
        //    refresh OTHER's view, and start a fresh `program_status` read (`req2`).
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 12,
                "payload": {
                    "type": "session_event",
                    "payload": {
                        "session_id": OTHER,
                        "event": {
                            "type": "block_event",
                            "payload": {
                                "block_id": "b2",
                                "creator_name": "com.superlogical.terminal",
                                "name": "program_status_changed",
                                "payload": {
                                    "id": "stale",
                                    "record": {"state": "error", "app": "old"}
                                }
                            }
                        }
                    }
                }
            }),
        )
        .await;
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 13,
                "payload": {
                    "type": "session_event",
                    "payload": {
                        "session_id": OTHER,
                        "event": {
                            "type": "block_event",
                            "payload": {
                                "block_id": "b2",
                                "creator_name": "com.superlogical.terminal",
                                "name": "program_status_removed",
                                "payload": {"ids": ["stale"]}
                            }
                        }
                    }
                }
            }),
        )
        .await;
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 14,
                "payload": {
                    "type": "event_dropped",
                    "payload": {
                        "event_type": "block_event",
                        "session_id": OTHER,
                        "block_id": "b2",
                        "size": 100000
                    }
                }
            }),
        )
        .await;

        let view_req2 = recv(&mut ws).await;
        assert_eq!(view_req2["method"], "session.view");
        assert_eq!(view_req2["payload"]["session_id"], OTHER);
        reply(&mut ws, &view_req2, 15, other_view(2)).await;

        let req2 = recv(&mut ws).await;
        assert_eq!(req2["method"], "com.superlogical.terminal.program_status");
        assert_eq!(req2["payload"]["session_id"], OTHER);
        assert_eq!(req2["payload"]["block_id"], "b2");
        assert_ne!(req2["id"], req1["id"]);

        // 3. Answer the fresh recovery read (`req2`) first and wait until the client applies it.
        reply(
            &mut ws,
            &req2,
            16,
            json!({"records": [{"id": "fresh", "state": "done", "app": "current"}]}),
        )
        .await;
        req2_seen_rx.await.unwrap();

        // 4. Now send a late non-empty reply on the superseded `req1` ID followed by a fence
        //    event. Because `req1` was cancelled, the late reply must be discarded and must not
        //    restore the removed `"stale"` record or clobber `"fresh"`.
        reply(
            &mut ws,
            &req1,
            17,
            json!({"records": [{"id": "stale", "state": "error", "app": "old"}]}),
        )
        .await;
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 18,
                "payload": {
                    "type": "session_event",
                    "payload": {
                        "session_id": SESSION,
                        "event": {
                            "type": "session_label_changed",
                            "payload": {"label": "fenced"}
                        }
                    }
                }
            }),
        )
        .await;

        done_tx.send(()).unwrap();
        expect_close_answering_sweep(&mut ws, &mut |_| None).await;
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    until(&host, |m| {
        m.status_read.contains("b2")
            && m.activity
                .get("b2")
                .is_some_and(|r| r.len() == 1 && r.contains_key("fresh"))
    })
    .await;
    req2_seen_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), done_rx)
        .await
        .unwrap()
        .unwrap();
    until(&host, |m| {
        m.sessions
            .iter()
            .any(|s| s.session_id == SESSION && s.label == "fenced")
            && !m.activity_reading
    })
    .await;
    {
        let m = host.model.lock().unwrap();
        let records = m.activity.get("b2").expect("fresh recovery record");
        assert_eq!(records.len(), 1);
        assert_eq!(records["fresh"].state, activity::ProgramState::Done);
        assert!(!records.contains_key("stale"));
    }
    drop(host);
    server.await.unwrap();
}
