//! API 15 adds `com.superlogical.terminal.ports` with forward-compatible `status` and optional `pid`.
use rexrs::api::{
    BlockMethod,
    terminal::{Ports, PortsResponse},
};
use serde_json::json;

#[test]
fn ports_response_preserves_unknown_status_and_optional_pid() {
    assert_eq!(Ports::NAME, "com.superlogical.terminal.ports");
    assert_eq!(serde_json::to_value(Ports::new()).unwrap(), json!({}));

    let payload = json!({
        "status": "future_status",
        "ports": [
            {
                "port": 3000,
                "address": "127.0.0.1",
                "protocol": "tcp",
                "first_seen_at": "2026-11-24T17:40:34Z",
                "pid": 4242
            },
            {
                "port": 8080,
                "address": "::1",
                "protocol": "tcp",
                "first_seen_at": "2026-11-24T17:40:35Z"
            }
        ],
        "processes": [
            {
                "pid": 4242,
                "user_id": 1000,
                "name": "node",
                "path": "/usr/bin/node",
                "script": "/srv/app/server.js",
                "cwd": "/srv/app"
            }
        ],
        "truncated": true,
        "id": 9
    });

    let decoded: PortsResponse = serde_json::from_value(payload.clone()).unwrap();
    assert_eq!(decoded.status, "future_status");
    assert_eq!(decoded.ports.len(), 2);
    assert_eq!(decoded.ports[0].port, 3000);
    assert_eq!(decoded.ports[0].pid, Some(4242));
    assert_eq!(decoded.ports[1].port, 8080);
    assert_eq!(decoded.ports[1].pid, None);
    assert_eq!(decoded.processes.len(), 1);
    assert_eq!(decoded.processes[0].pid, 4242);
    assert_eq!(decoded.processes[0].name, "node");
    assert_eq!(decoded.truncated, Some(true));
    assert_eq!(decoded.id, Some(9));
    assert_eq!(serde_json::to_value(&decoded).unwrap(), payload);

    let empty: PortsResponse = serde_json::from_value(json!({
        "status": "ok",
        "ports": null,
        "processes": null
    }))
    .unwrap();
    assert_eq!(empty.status, "ok");
    assert!(empty.ports.is_empty());
    assert!(empty.processes.is_empty());
    assert_eq!(empty.truncated, None);
    assert_eq!(empty.id, None);
}
