use super::*;
use crate::connection::hosts::SharedHostState;

async fn wait_shared_revision(host: &HostConnection, min_rev: u64) -> HostModel {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            {
                let model = host.model.lock().unwrap();
                if model.connected && model.shared_revision >= min_rev {
                    return model.clone();
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

async fn bootstrap(ws: &mut WebSocketStream<TcpStream>) {
    let request = recv(ws).await;
    assert_eq!(request["method"], "session.bootstrap");
    send(
        ws,
        json!({"type":"response","id":request["id"],"seq":1,"payload":boot()}),
    )
    .await;
}

#[tokio::test]
async fn hosts_list_reads_once_skips_same_hash_rereads_on_change_and_never_writes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (seen_first, wait_first) = tokio::sync::oneshot::channel::<()>();
    let (seen_second, wait_second) = tokio::sync::oneshot::channel::<()>();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 13).await;
        bootstrap(&mut ws).await;

        // 1. Initial hosts.list read on connection.
        let req1 = recv(&mut ws).await;
        assert_eq!(req1["method"], "hosts.list");
        assert_eq!(req1["payload"], json!({}));
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req1["id"],
                "payload": {
                    "hash": "hash-1",
                    "hosts": [{
                        "id": "host:0199abcd-1234-7abc-8def-0123456789ab",
                        "label": "Studio",
                        "endpoints": ["studio.ts.net"],
                        "unknown_future": {"keep": true}
                    }],
                    "diagnostics": [{
                        "severity": "warning",
                        "source": "init.lua:4",
                        "message": "duplicate endpoint"
                    }]
                }
            }),
        )
        .await;
        wait_first.await.unwrap();

        // 2. Same-hash hosts_changed event must NOT trigger another hosts.list.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 0,
                "payload": {"type": "hosts_changed", "payload": {"hash": "hash-1"}}
            }),
        )
        .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(120), ws.next())
                .await
                .is_err(),
            "same-hash hosts_changed must not send a request"
        );

        // 3. Different-hash hosts_changed event triggers exactly one re-read.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 0,
                "payload": {"type": "hosts_changed", "payload": {"hash": "hash-2"}}
            }),
        )
        .await;
        let req2 = recv(&mut ws).await;
        assert_eq!(req2["method"], "hosts.list");
        assert_eq!(req2["payload"], json!({}));
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req2["id"],
                "payload": {
                    "hash": "hash-2",
                    "hosts": [{
                        "id": "host:0199abcd-1234-7abc-8def-0123456789ab",
                        "label": "Studio Pro",
                        "endpoints": ["studio.ts.net"]
                    }]
                }
            }),
        )
        .await;
        wait_second.await.unwrap();

        // 4. Conflict on a later change sets SharedHostState::Conflict without disconnecting.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 0,
                "payload": {"type": "hosts_changed", "payload": {"hash": "hash-3"}}
            }),
        )
        .await;
        let req3 = recv(&mut ws).await;
        assert_eq!(req3["method"], "hosts.list");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req3["id"],
                "error": {
                    "code": "conflict",
                    "message": "hosts/everywhere.json is not a JSON object",
                    "status": 409
                }
            }),
        )
        .await;
        done_rx.await.unwrap();

        // Zero hosts.put or hosts.remove ever sent: next frame must be Close.
        let closing = ws.next().await;
        assert!(
            matches!(closing, Some(Ok(Message::Close(_)))),
            "unexpected message before close: {closing:?}"
        );
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    host.commands.send(HostCommand::WatchSharedHosts).unwrap();

    let m1 = wait_shared_revision(&host, 1).await;
    let SharedHostState::Loaded(list1) = &m1.shared else {
        panic!("expected Loaded, got {:?}", m1.shared);
    };
    assert_eq!(list1.hash, "hash-1");
    assert_eq!(list1.hosts.len(), 1);
    assert_eq!(list1.hosts[0].label, "Studio");
    assert_eq!(
        list1.hosts[0].raw.get("unknown_future"),
        Some(&json!({"keep": true}))
    );
    assert_eq!(list1.diagnostics.len(), 1);
    seen_first.send(()).unwrap();

    let m2 = wait_shared_revision(&host, 2).await;
    let SharedHostState::Loaded(list2) = &m2.shared else {
        panic!("expected Loaded, got {:?}", m2.shared);
    };
    assert_eq!(list2.hash, "hash-2");
    assert_eq!(list2.hosts[0].label, "Studio Pro");
    seen_second.send(()).unwrap();

    let m3 = wait_shared_revision(&host, 3).await;
    assert_eq!(
        m3.shared,
        SharedHostState::Conflict("hosts/everywhere.json is not a JSON object".into())
    );
    assert!(m3.connected);
    done_tx.send(()).unwrap();

    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn unknown_hosts_list_method_is_quiet_and_unsupported() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 13).await;
        bootstrap(&mut ws).await;
        let req = recv(&mut ws).await;
        assert_eq!(req["method"], "hosts.list");
        send(
            &mut ws,
            json!({
                "type": "response",
                "id": req["id"],
                "error": {"code": "method_not_found", "message": "no such method", "status": 404}
            }),
        )
        .await;
        // Even if hosts_changed arrived, Unsupported sends no more reads.
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 0,
                "payload": {"type": "hosts_changed", "payload": {"hash": "new"}}
            }),
        )
        .await;
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });

    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    host.commands.send(HostCommand::WatchSharedHosts).unwrap();
    let model = wait_shared_revision(&host, 1).await;
    assert_eq!(model.shared, SharedHostState::Unsupported);
    assert!(model.connected);
    assert_eq!(model.notice, None);
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn non_watched_host_never_sends_hosts_list() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 13).await;
        bootstrap(&mut ws).await;
        send(
            &mut ws,
            json!({
                "type": "event",
                "seq": 0,
                "payload": {"type": "hosts_changed", "payload": {"hash": "h1"}}
            }),
        )
        .await;
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    connected(&host).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    {
        let model = host.model.lock().unwrap();
        assert_eq!(model.shared, SharedHostState::Unloaded);
        assert_eq!(model.shared_revision, 0);
    }
    drop(host);
    server.await.unwrap();
}
