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
