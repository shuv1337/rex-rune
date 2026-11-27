//! Empty-session close checks on the production worker with a mock Rex control socket.
use super::*;
use session_close::Blocks;

async fn bootstrap(ws: &mut WebSocketStream<TcpStream>, revision: u32) {
    welcome(ws, revision).await;
    let req = next(ws).await;
    assert_eq!(req["method"], "session.bootstrap");
    reply(ws, &req, 1, json!(boot())).await;
}

async fn read_blocks(ws: &mut WebSocketStream<TcpStream>) -> Value {
    let req = next(ws).await;
    assert_eq!(req["method"], "session.list_blocks");
    assert_eq!(req["payload"], json!({"session_id": SESSION}));
    assert!(req["id"].as_str().unwrap().starts_with("detached:"));
    req
}

fn epoch(host: &HostConnection) -> u64 {
    host.model.lock().unwrap().connection_epoch
}

fn check(host: &HostConnection, epoch: u64, timeout: Duration) {
    host.commands
        .send(HostCommand::CheckEmptySession {
            epoch,
            session: SESSION.into(),
            deadline: Instant::now() + timeout,
        })
        .unwrap();
}

fn send_fence(host: &HostConnection) {
    host.commands
        .send(HostCommand::Request {
            method: "test.fence".into(),
            payload: json!({}),
        })
        .unwrap();
}

async fn fence(ws: &mut WebSocketStream<TcpStream>) {
    let req = next(ws).await;
    assert_eq!(req["method"], "test.fence");
    reply(ws, &req, 3, json!({})).await;
}

async fn outcome(host: &HostConnection, expected: Blocks) {
    until(host, |m| !m.session_close_checks.is_empty()).await;
    let model = host.model.lock().unwrap();
    let [result] = &model.session_close_checks[..] else {
        panic!("expected one block inventory result")
    };
    assert_eq!(result.epoch, model.connection_epoch);
    assert_eq!(result.session, SESSION);
    assert_eq!(result.blocks, expected);
}

#[tokio::test]
async fn checks_are_detached_deduplicated_and_supported_at_revision_zero() {
    for (revision, blocks, expected) in [
        (0, json!([]), Blocks::Empty),
        (20, json!([]), Blocks::Empty),
        (
            24,
            json!([{"block_id": "block:detached", "placement": "detached", "label": "shell"}]),
            Blocks::Present,
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let host = HostConnection::spawn(endpoint, None, None, None, false, None);
        let mut ws = accept(&listener, "rex.control.v1").await;
        bootstrap(&mut ws, revision).await;
        connected(&host).await;

        check(&host, epoch(&host), session_close::TIMEOUT);
        let read = read_blocks(&mut ws).await;
        check(&host, epoch(&host), session_close::TIMEOUT);
        send_fence(&host);
        fence(&mut ws).await;
        assert!(host.model.lock().unwrap().session_close_checks.is_empty());

        reply(&mut ws, &read, 4, json!({"blocks": blocks})).await;
        outcome(&host, expected).await;
        if expected == Blocks::Empty {
            host.commands
                .send(HostCommand::Close {
                    kind: HumanClose::Session,
                    payload: json!({"session_id": SESSION}),
                    epoch: Some(epoch(&host)),
                })
                .unwrap();
            let close = next(&mut ws).await;
            assert_eq!(close["method"], "session.destroy");
            let mut payload = json!({"session_id": SESSION});
            if revision >= 20 {
                payload["undoable"] = json!(true);
            }
            assert_eq!(close["payload"], payload);
            reply(&mut ws, &close, 5, json!({})).await;
            let followup = next(&mut ws).await;
            assert!(matches!(
                followup["method"].as_str(),
                Some("session.attach" | "session.view")
            ));
            send(
                &mut ws,
                json!({"type":"response","id":followup["id"],"seq":5,
                "error":{"code":"session_not_found","message":"closed","status":404}}),
            )
            .await;
            until(&host, |m| m.sessions.is_empty()).await;
        }
        send_fence(&host);
        fence(&mut ws).await;
    }
}

#[tokio::test]
async fn refusals_invalid_inventories_and_timeouts_are_unknown_and_never_retried() {
    for frame in [
        Some(json!({"error":{"code":"method_not_found","message":"not supported","status":404}})),
        Some(json!({"error":{"code":"internal","message":"cannot list blocks","status":500}})),
        Some(json!({"payload":{}})),
        Some(json!({"payload":{"blocks":null}})),
        None,
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let host = HostConnection::spawn(endpoint, None, None, None, false, None);
        let mut ws = accept(&listener, "rex.control.v1").await;
        bootstrap(&mut ws, 24).await;
        connected(&host).await;

        check(&host, epoch(&host), Duration::from_secs(1));
        let read = read_blocks(&mut ws).await;
        let late_reply = frame.is_none();
        if let Some(mut frame) = frame {
            frame["type"] = json!("response");
            frame["id"] = read["id"].clone();
            send(&mut ws, frame).await;
        }
        outcome(&host, Blocks::Unknown).await;
        if late_reply {
            reply(&mut ws, &read, 4, json!({"blocks":[]})).await;
        }
        send_fence(&host);
        fence(&mut ws).await;
        assert_eq!(host.model.lock().unwrap().session_close_checks.len(), 1);
    }
}

#[tokio::test]
async fn stale_checks_and_verified_closes_do_not_cross_a_reconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    let mut ws = accept(&listener, "rex.control.v1").await;
    bootstrap(&mut ws, 24).await;
    connected(&host).await;
    let old = epoch(&host);
    check(&host, old, session_close::TIMEOUT);
    read_blocks(&mut ws).await;
    host.commands.send(HostCommand::Reconnect).unwrap();
    until(&host, |m| !m.connected).await;
    assert!(host.model.lock().unwrap().session_close_checks.is_empty());
    drop(ws);

    let mut ws = tokio::time::timeout(Duration::from_secs(8), accept(&listener, "rex.control.v1"))
        .await
        .unwrap();
    bootstrap(&mut ws, 24).await;
    until(&host, |m| m.connected && m.connection_epoch != old).await;
    check(&host, old, session_close::TIMEOUT);
    host.commands
        .send(HostCommand::Close {
            kind: HumanClose::Session,
            payload: json!({"session_id": SESSION}),
            epoch: Some(old),
        })
        .unwrap();
    send_fence(&host);
    fence(&mut ws).await;
    assert!(host.model.lock().unwrap().session_close_checks.is_empty());
}
