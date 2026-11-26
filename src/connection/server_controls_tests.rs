use super::*;

async fn ready(ws: &mut WebSocketStream<TcpStream>) {
    welcome(ws, 13).await;
    let request = next(ws).await;
    assert_eq!(
        request["method"], "session.bootstrap",
        "reconnect only attaches, never starts a server"
    );
    reply(ws, &request, 1, serde_json::to_value(boot()).unwrap()).await;
}

async fn stop_case(failure: Option<(&'static str, u16)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        ready(&mut ws).await;
        let request = next(&mut ws).await;
        assert_eq!(request["method"], "server.stop");
        assert_eq!(request["payload"], json!({}), "no if_idle");
        match failure {
            Some(("drop", _)) => drop(ws),
            Some((code, status)) => {
                send(
                    &mut ws,
                    json!({"type":"response","id":request["id"],
                    "error":{"code":code,"status":status,"message":"private refusal"}}),
                )
                .await;
                // A barrier command proves refusal sends nothing else and is not retried.
                let barrier = next(&mut ws).await;
                assert_eq!(barrier["method"], "test.barrier");
                reply(&mut ws, &barrier, 2, json!({})).await;
                drop(ws);
            }
            None => {
                reply(&mut ws, &request, 2, json!({})).await;
                assert!(
                    matches!(ws.next().await, Some(Ok(Message::Close(_)))),
                    "client must close after the reply"
                );
            }
        }
        // The real run_host loop reconnects, with no stop replay or start request.
        let mut ws = accept(&listener, "rex.control.v1").await;
        ready(&mut ws).await;
        assert!(
            matches!(ws.next().await, Some(Ok(Message::Close(_)))),
            "nothing is replayed"
        );
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    let epoch = host.model.lock().unwrap().connection_epoch;
    host.commands
        .send(HostCommand::StopServer { epoch })
        .unwrap();
    until(&host, |m| m.notice.is_some()).await;
    let notice = host.model.lock().unwrap().notice.clone().unwrap();
    assert_eq!(
        host.model.lock().unwrap().server_stop_result.as_deref(),
        Some(notice.as_str())
    );
    match failure {
        Some(("drop", _)) => assert!(
            notice.contains("Could not confirm server stop")
                && notice.contains("Nothing was retried")
        ),
        Some(_) => {
            assert!(
                notice.contains("Could not stop server")
                    && notice.contains("private refusal")
                    && notice.contains("Nothing was retried"),
                "{notice}"
            );
            host.commands
                .send(HostCommand::Request {
                    method: "test.barrier".into(),
                    payload: json!({}),
                })
                .unwrap();
        }
        None => assert!(
            notice.contains("Server stop accepted") && notice.contains("will not start it again")
        ),
    }
    until(&host, |m| m.connected && m.connection_epoch == 2).await;
    // Once a server answers again an accepted stop is old news; a failure is kept.
    assert_eq!(
        host.model.lock().unwrap().server_stop_result.is_none(),
        failure.is_none()
    );
    drop(host);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn successful_stop_closes_then_only_reconnects_without_start_or_replay() {
    stop_case(None).await;
}
#[tokio::test]
async fn stop_403_is_reported_without_retry() {
    stop_case(Some(("forbidden", 403))).await;
}
#[tokio::test]
async fn stop_not_supported_is_reported_without_retry() {
    stop_case(Some(("not_supported", 400))).await;
}
#[tokio::test]
async fn stop_conflict_is_reported_without_retry() {
    stop_case(Some(("conflict", 409))).await;
}
#[tokio::test]
async fn stop_drop_is_reported_without_retry() {
    stop_case(Some(("drop", 0))).await;
}

#[tokio::test]
async fn stale_stop_epoch_never_reaches_the_wire() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        ready(&mut ws).await;
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    host.commands
        .send(HostCommand::StopServer { epoch: 0 })
        .unwrap();
    until(&host, |m| m.notice.is_some()).await;
    assert!(
        host.model
            .lock()
            .unwrap()
            .notice
            .as_deref()
            .unwrap()
            .contains("connection changed")
    );
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn resize_layer_sends_raise_then_resize_and_blocks_stale_epoch_or_old_api() {
    // 1. API 13: raise_first=false sends only session.resize_layer; raise_first=true sends
    //    session.raise_layer then session.resize_layer; stale epoch never reaches the wire.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        ready(&mut ws).await;
        let first = next(&mut ws).await;
        assert_eq!(first["method"], "session.resize_layer");
        assert_eq!(
            first["payload"],
            json!({"session_id": "s", "layer_id": "fl", "bounds": [0.2, 0.2, 0.5, 0.5]})
        );
        reply(&mut ws, &first, 2, json!({"revision": 2})).await;
        let view1 = next(&mut ws).await;
        assert_eq!(view1["method"], "session.view");
        reply(
            &mut ws,
            &view1,
            2,
            json!({"revision": 2, "session_id": "s", "windows": []}),
        )
        .await;

        let raise = next(&mut ws).await;
        assert_eq!(raise["method"], "session.raise_layer");
        assert_eq!(
            raise["payload"],
            json!({"session_id": "s", "layer_id": "back"})
        );
        reply(&mut ws, &raise, 3, json!({"revision": 3})).await;
        let second = next(&mut ws).await;
        assert_eq!(second["method"], "session.resize_layer");
        assert_eq!(
            second["payload"],
            json!({"session_id": "s", "layer_id": "back", "bounds": [0.1, 0.1, 0.4, 0.4]})
        );
        reply(&mut ws, &second, 4, json!({"revision": 4})).await;
        let view2 = next(&mut ws).await;
        assert_eq!(view2["method"], "session.view");
        reply(
            &mut ws,
            &view2,
            4,
            json!({"revision": 4, "session_id": "s", "windows": []}),
        )
        .await;

        // Stale epoch must never reach the wire before close.
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    let epoch = host.model.lock().unwrap().connection_epoch;
    host.commands
        .send(HostCommand::ResizeLayer {
            epoch,
            session: "s".into(),
            layer: "fl".into(),
            bounds: [0.2, 0.2, 0.5, 0.5],
            raise_first: false,
        })
        .unwrap();
    host.commands
        .send(HostCommand::ResizeLayer {
            epoch,
            session: "s".into(),
            layer: "back".into(),
            bounds: [0.1, 0.1, 0.4, 0.4],
            raise_first: true,
        })
        .unwrap();
    host.commands
        .send(HostCommand::ResizeLayer {
            epoch: epoch + 99,
            session: "s".into(),
            layer: "fl".into(),
            bounds: [0.3, 0.3, 0.4, 0.4],
            raise_first: false,
        })
        .unwrap();
    until(&host, |m| {
        m.notice
            .as_deref()
            .is_some_and(|n| n.contains("changed while you were moving it"))
    })
    .await;
    drop(host);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();

    // 2. API 12 host: ResizeLayer is rejected at the worker before reaching the wire.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 12).await;
        let boot_req = next(&mut ws).await;
        assert_eq!(boot_req["method"], "session.bootstrap");
        reply(&mut ws, &boot_req, 1, serde_json::to_value(boot()).unwrap()).await;
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    let epoch = host.model.lock().unwrap().connection_epoch;
    host.commands
        .send(HostCommand::ResizeLayer {
            epoch,
            session: "s".into(),
            layer: "fl".into(),
            bounds: [0.2, 0.2, 0.5, 0.5],
            raise_first: true,
        })
        .unwrap();
    until(&host, |m| {
        m.notice
            .as_deref()
            .is_some_and(|n| n.contains("API revision 12") && n.contains("revision 13"))
    })
    .await;
    drop(host);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn stop_server_is_blocked_below_api_4_and_sent_on_api_4() {
    // API 3: StopServer must never reach the wire; a barrier request arrives first.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 3).await;
        let boot_req = next(&mut ws).await;
        assert_eq!(boot_req["method"], "session.bootstrap");
        assert!(boot_req["payload"].get("data_tickets").is_none());
        reply(&mut ws, &boot_req, 1, serde_json::to_value(boot()).unwrap()).await;
        let barrier = next(&mut ws).await;
        assert_eq!(
            barrier["method"], "test.barrier",
            "server.stop must not be sent on API 3"
        );
        reply(&mut ws, &barrier, 2, json!({})).await;
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    let epoch = host.model.lock().unwrap().connection_epoch;
    host.commands
        .send(HostCommand::StopServer { epoch })
        .unwrap();
    until(&host, |m| m.notice.is_some()).await;
    {
        let model = host.model.lock().unwrap();
        let notice = model.notice.as_deref().unwrap();
        assert!(
            notice.contains("API revision 4") && notice.contains("revision 3"),
            "{notice}"
        );
        assert_eq!(model.server_stop_result.as_deref(), Some(notice));
    }
    host.commands
        .send(HostCommand::Request {
            method: "test.barrier".into(),
            payload: json!({}),
        })
        .unwrap();
    until(&host, |m| m.notice.is_none()).await;
    drop(host);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();

    // API 4: StopServer sends server.stop over the wire and closes cleanly on reply.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 4).await;
        let boot_req = next(&mut ws).await;
        assert_eq!(boot_req["method"], "session.bootstrap");
        assert!(boot_req["payload"].get("data_tickets").is_none());
        reply(&mut ws, &boot_req, 1, serde_json::to_value(boot()).unwrap()).await;
        let stop_req = next(&mut ws).await;
        assert_eq!(stop_req["method"], "server.stop");
        assert_eq!(stop_req["payload"], json!({}));
        reply(&mut ws, &stop_req, 2, json!({})).await;
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    let epoch = host.model.lock().unwrap().connection_epoch;
    host.commands
        .send(HostCommand::StopServer { epoch })
        .unwrap();
    until(&host, |m| {
        m.notice
            .as_deref()
            .is_some_and(|n| n.contains("Server stop accepted"))
    })
    .await;
    drop(host);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}
