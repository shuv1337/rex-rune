//! The worker is the last epoch and revision check before a layout drop goes on the wire.
use super::*;

#[tokio::test]
async fn drops_are_sent_once_on_the_captured_epoch_and_transfer_requires_api_22() {
    for revision in [21, 22] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut ws = accept(&listener, "rex.control.v1").await;
            welcome(&mut ws, revision).await;
            let bootstrap = recv(&mut ws).await;
            assert_eq!(bootstrap["method"], "session.bootstrap");
            let mut initial = serde_json::to_value(boot()).unwrap();
            let original_list = json!([{"session_id":SESSION,"label":"source"},
                {"session_id":OTHER,"label":"target"}]);
            initial["attach_all"]["sessions"] = original_list.clone();
            reply(&mut ws, &bootstrap, 1, initial).await;
            let mut drops = Vec::new();
            let mut transferred = false;
            for seq in 2.. {
                let req = recv(&mut ws).await;
                let method = req["method"].as_str().unwrap();
                let id = req["payload"]["session_id"].as_str().unwrap_or(SESSION);
                let response = match method {
                    "session.move_window" | "session.swap_blocks" | "session.transfer_window" => {
                        drops.push((method.to_owned(), req["payload"].clone()));
                        transferred |= method == "session.transfer_window";
                        json!({"revision":seq, "to_revision":seq})
                    }
                    "session.move" => {
                        drops.push((method.to_owned(), req["payload"].clone()));
                        json!({})
                    }
                    "session.attach_all" => {
                        let mut sessions = original_list.clone();
                        sessions.as_array_mut().unwrap().reverse();
                        json!({"sessions":sessions})
                    }
                    "session.attach" => json!({"client_id":"client:test","view":view_of(id)}),
                    "session.view" if id == OTHER && transferred => {
                        json!({"session_id":OTHER,"revision":seq,"windows":[{"window_id":"w"}]})
                    }
                    "session.view" => view_of(id),
                    "test.sentinel" => {
                        reply(&mut ws, &req, seq, json!({})).await;
                        break;
                    }
                    _ => json!({}),
                };
                reply(&mut ws, &req, seq, response).await;
            }
            drops
        });
        let host = HostConnection::spawn(endpoint, None, None, None, false, None);
        connected(&host).await;
        let epoch = host.model.lock().unwrap().connection_epoch;
        host.commands
            .send(HostCommand::FetchView {
                id: 1,
                epoch,
                deadline: Instant::now() + Duration::from_secs(8),
                session: OTHER.into(),
            })
            .unwrap();
        until(&host, |m| m.views.contains_key(OTHER)).await;
        let transfer =
            || LayoutDrop::TransferWindow(session::TransferWindow::new(SESSION, "w", OTHER));
        for (epoch, operation) in [
            (
                epoch + 1,
                LayoutDrop::MoveSession(session::Move::new(SESSION)),
            ),
            (epoch + 1, transfer()),
            (epoch, LayoutDrop::MoveSession(session::Move::new(SESSION))),
            (
                epoch,
                LayoutDrop::MoveWindow(
                    session::MoveWindow::new(SESSION, "w").before_window_id("other"),
                ),
            ),
            (
                epoch,
                LayoutDrop::SwapBlocks(session::SwapBlocks::new(SESSION, "block:a", "block:b")),
            ),
            (epoch, transfer()),
        ] {
            host.commands
                .send(HostCommand::LayoutDrop { epoch, operation })
                .unwrap();
        }
        host.commands
            .send(HostCommand::Request {
                method: "test.sentinel".into(),
                payload: json!({}),
            })
            .unwrap();
        let actual = tokio::time::timeout(Duration::from_secs(8), server)
            .await
            .unwrap()
            .unwrap();
        let mut expected = vec![
            ("session.move".into(), json!({"session_id":SESSION})),
            (
                "session.move_window".into(),
                json!({"session_id":SESSION,"window_id":"w","before_window_id":"other"}),
            ),
            (
                "session.swap_blocks".into(),
                json!({"session_id":SESSION,"block_id":"block:a","other_block_id":"block:b"}),
            ),
        ];
        if revision == 22 {
            expected.push((
                "session.transfer_window".into(),
                json!({"session_id":SESSION,"window_id":"w","to_session_id":OTHER}),
            ));
        }
        assert_eq!(actual, expected);
        let model = host.model.lock().unwrap();
        assert_eq!(model.sessions[0].session_id, OTHER);
        assert_eq!(
            model.views[OTHER].windows[0].window_id.as_deref() == Some("w"),
            revision == 22,
            "only a completed transfer refreshes the cached destination"
        );
    }
}
