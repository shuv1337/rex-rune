use super::*;

fn view(active: &str) -> session::ViewResponse {
    session::ViewResponse {
        active_window_id: Some(active.into()),
        windows: ["front", "back"]
            .map(|id| session::Window {
                window_id: Some(id.into()),
                layers: vec![session::Layer {
                    blocks: vec![session::LayerBlock {
                        block_id: Some(id.into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            })
            .into(),
        ..Default::default()
    }
}

fn title(block: &str, text: &str, pid: u64) -> rexrs::Event {
    rexrs::Event {
        name: "block_event".into(),
        seq: 10,
        session_id: Some(SESSION.into()),
        payload: json!({"block_id":block,"creator_name":activity::TERMINAL,
            "name":"title_changed","payload":{"title":text,"owner":{"pid":pid}}}),
    }
}

#[tokio::test]
async fn title_events_store_immediately_and_only_wake_when_due() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (client, _socket) = tokio::join!(transport::control(&endpoint, None), async {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 10).await;
        ws
    });
    let mut control = client.unwrap();
    tokio::time::pause();
    // Count every attempted wake, even when the consumer hasn't taken the previous one.
    let (sender, wakes) = flume::unbounded();
    let shared = Shared {
        wake: title_wake::HostWake::new(sender),
        model: Arc::new(Mutex::new(HostModel {
            selected: Some(SESSION.into()),
            views: HashMap::from([(SESSION.into(), view("front"))]),
            visible: true,
            ..Default::default()
        })),
    };
    let mut state = ControlState::default();
    let mut sweep = status_sweep::Reader::default();
    let mut workers = HashMap::new();
    macro_rules! event {
        ($ev:expr) => {
            handle_event(
                $ev,
                &mut control,
                &shared,
                &mut workers,
                &mut state,
                &mut sweep,
                None,
                &mut Default::default(),
                &mut Default::default(),
            )
            .await
            .unwrap()
        };
    }
    let count = || wakes.drain().count();
    for (block, interval) in [("back", 2000), ("front", 250)] {
        tokio::time::advance(Duration::from_secs(5)).await;
        event!(title(block, " \t", 1));
        assert_eq!(count(), 0, "clearing a missing title");
        event!(title(block, "first", 1));
        assert_eq!(count(), 1, "first after quiet");
        event!(title(block, " first ", 1));
        tokio::time::advance(Duration::from_secs(5)).await;
        shared.wake.flush_due();
        assert_eq!(count(), 0, "same parsed title neither wakes nor schedules");
        event!(title(block, "first", 2));
        assert_eq!(count(), 1, "owner change matters to generated titles");
        for text in ["second", "third", "last"] {
            event!(title(block, text, 2));
            assert_eq!(shared.model.lock().unwrap().title(block), Some(text));
        }
        let mut other = title("offscreen", "another session", 3);
        other.session_id = Some("elsewhere".into());
        event!(other);
        tokio::time::advance(Duration::from_millis(interval - 1)).await;
        shared.wake.flush_due();
        assert_eq!(count(), 0, "{block} burst");
        tokio::time::advance(Duration::from_millis(1)).await;
        shared.wake.flush_due();
        assert_eq!(count(), 1, "{block} deadline");
        assert_eq!(shared.model.lock().unwrap().title(block), Some("last"));
        tokio::time::advance(Duration::from_secs(5)).await;
        shared.wake.flush_due();
        assert_eq!(count(), 0, "idle");
    }

    // A host switch, a new active window, and a bell each deliver a waiting title.
    shared.update(|m| m.visible = false);
    count();
    for (i, text) in ["show host", "show tab", "bell"].into_iter().enumerate() {
        let block = if i == 0 { "front" } else { "back" };
        event!(title(block, "starting", 2));
        assert_eq!(count(), 1);
        event!(title(block, text, 2));
        if i == 0 {
            tokio::time::advance(Duration::from_millis(250)).await;
            shared.wake.flush_due();
        }
        assert_eq!(count(), 0);
        match i {
            0 => handle_command(
                HostCommand::Visibility(true),
                &mut control,
                &shared,
                &mut workers,
                &mut state,
                &mut sweep,
                &mut None,
            )
            .await
            .unwrap(),
            1 => replace_view(&shared, &mut state, &mut sweep, SESSION, view("back")),
            _ => {
                let mut bell = title("back", "", 0);
                bell.payload["name"] = json!("bell");
                event!(bell);
            }
        }
        assert_eq!(count(), 1);
        assert_eq!(shared.model.lock().unwrap().title(block), Some(text));
        tokio::time::advance(Duration::from_secs(5)).await;
        shared.wake.flush_due();
        assert_eq!(count(), 0, "no trailing wake after {text}");
    }

    event!(title("back", "before disconnect", 2));
    event!(title("back", "pending", 2));
    assert_eq!(count(), 1);
    shared.status("Disconnected", false);
    shared.status("Connected", true);
    assert_eq!(count(), 2);
    event!(title("back", "new connection", 2));
    assert_eq!(count(), 1, "new connection gets its own first wake");
    tokio::time::advance(Duration::from_secs(5)).await;
    shared.wake.flush_due();
    assert_eq!(count(), 0);
}

#[tokio::test]
async fn host_loop_delivers_the_final_hidden_title_without_another_event() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    let mut ws = accept(&listener, "rex.control.v1").await;
    welcome(&mut ws, 10).await;
    let request = recv(&mut ws).await;
    reply(&mut ws, &request, 1, json!(boot())).await;
    // Fence startup wakes before sending titles. Navigating to the already-selected
    // session with no target window completes locally in the host loop.
    host.commands
        .send(HostCommand::Navigate {
            id: 1,
            session: SESSION.into(),
            window: None,
            deadline: Instant::now() + Duration::from_secs(8),
        })
        .unwrap();
    until(&host, |m| m.navigation_result == Some((1, true))).await;
    host.wake.drain().for_each(drop);
    for (seq, text) in [(2, "first"), (3, "last")] {
        let ev = title("background", text, 1);
        send(
            &mut ws,
            json!({"type":"event","seq":seq,"payload":{"type":"session_event",
            "payload":{"session_id":SESSION,"event":{"type":ev.name,"payload":ev.payload}}}}),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                host.wake.recv_async().await.unwrap();
                if host.model.lock().unwrap().title("background") == Some(text) {
                    break;
                }
            }
        })
        .await
        .expect("a wake must deliver the title even if the spinner stops");
    }
    drop(host);
    assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
}
