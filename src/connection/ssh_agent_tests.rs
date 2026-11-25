use super::*;
use std::os::unix::fs::PermissionsExt;
use tokio::net::UnixListener;

fn spawn_with_fake_ssh(
    endpoint: &str,
    ssh_program: PathBuf,
    ssh_auth_sock: Option<PathBuf>,
    tunnel_dir: PathBuf,
) -> HostConnection {
    HostConnection::spawn_internal(
        endpoint.into(),
        Startup {
            label: "test".into(),
            preferred: None,
            create: None,
            block: None,
            theme: None,
            ssh_auth_sock,
            prompts_enabled: false,
            max_api_revision: transport::API_REVISION,
            ssh_program: Some(ssh_program),
            tunnel_dir: Some(tunnel_dir),
        },
        false,
    )
}

#[allow(clippy::result_large_err)] // tungstenite's callback fixes the error type.
async fn accept_unix(listener: &UnixListener) -> WebSocketStream<tokio::net::UnixStream> {
    // Tunnel::open probes readiness with a raw connect-and-drop before WebSocket connects.
    loop {
        let (stream, _) = listener.accept().await.unwrap();
        if let Ok(ws) = tokio_tungstenite::accept_hdr_async(
            stream,
            |_: &_, mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                response
                    .headers_mut()
                    .insert("Sec-WebSocket-Protocol", "rex.control.v1".parse().unwrap());
                Ok(response)
            },
        )
        .await
        {
            return ws;
        }
    }
}

async fn wait_for(timeout: Duration, predicate: impl Fn() -> bool) {
    tokio::time::timeout(timeout, async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("timed out waiting for host state");
}

#[tokio::test]
async fn ssh_agent_command_does_not_drop_connected_host_and_wakes_only_auth_failed_ssh() {
    let dir = tempfile::tempdir().unwrap();
    let good_sock = dir.path().join("good-agent.sock");
    let bad_sock = dir.path().join("bad-agent.sock");
    let count_file = dir.path().join("attempts");

    // Start a real mock Rex control server on a local Unix socket; our fake `ssh -L`
    // simply symlinks the requested local tunnel path to this server socket and waits on stdin.
    let server_sock = dir.path().join("rex-server.sock");
    let listener = UnixListener::bind(&server_sock).unwrap();
    let server = tokio::spawn(async move {
        let mut ws = accept_unix(&listener).await;
        let hello =
            serde_json::from_str::<Value>(ws.next().await.unwrap().unwrap().to_text().unwrap())
                .unwrap();
        assert_eq!(hello["api_revision"], 24);
        ws.send(Message::Text(
            json!({
                "type": "welcome",
                "protocol": "rex.control.v1",
                "api_revision": 13,
                "limits": {"max_message_bytes": 65536},
                "client": {"id": "client:ssh-test", "reconnect_token": "tok"}
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
        let req =
            serde_json::from_str::<Value>(ws.next().await.unwrap().unwrap().to_text().unwrap())
                .unwrap();
        assert_eq!(req["method"], "session.bootstrap");
        ws.send(Message::Text(
            json!({
                "type": "response",
                "id": req["id"],
                "seq": 1,
                "payload": boot()
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
        while let Some(Ok(msg)) = ws.next().await {
            if matches!(msg, Message::Close(_)) {
                break;
            }
        }
    });

    let fake_ssh = dir.path().join("fake-ssh");
    std::fs::write(
        &fake_ssh,
        format!(
            r#"#!/bin/sh
echo "attempt" >> '{}'
if [ "${{SSH_AUTH_SOCK-}}" != '{}' ]; then
  echo "tester@box: Permission denied (publickey)." >&2
  exit 255
fi
forward=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-L" ]; then
    forward="$arg"
  fi
  prev="$arg"
done
if [ -n "$forward" ]; then
  local_sock="${{forward%%:*}}"
  ln -sf '{}' "$local_sock"
  exec cat >/dev/null
fi
printf 'sock /run/rex/server.sock\n'
"#,
            count_file.display(),
            good_sock.display(),
            server_sock.display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake_ssh, std::fs::Permissions::from_mode(0o700)).unwrap();

    // Start with the wrong agent: first attempt fails with AuthFailed and enters 2s backoff.
    let tunnel_dir = dir.path().join("tunnels");
    let host = spawn_with_fake_ssh(
        "ssh://tester@auth-test-box",
        fake_ssh.clone(),
        Some(bad_sock.clone()),
        tunnel_dir.clone(),
    );
    wait_for(Duration::from_secs(5), || {
        host.model.lock().unwrap().ssh_problem == Some(crate::ssh::Problem::AuthFailed)
    })
    .await;

    // Sending SshAgent with wake_auth_retry = false must NOT wake backoff.
    host.commands
        .send(HostCommand::SshAgent {
            socket: Some(good_sock.clone()),
            wake_auth_retry: false,
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(
        !host.model.lock().unwrap().connected,
        "wake_auth_retry=false must not wake backoff immediately"
    );

    // Sending SshAgent with wake_auth_retry = true immediately wakes AuthFailed SSH host!
    let wake_started = Instant::now();
    host.commands
        .send(HostCommand::SshAgent {
            socket: Some(good_sock.clone()),
            wake_auth_retry: true,
        })
        .unwrap();
    connected(&host).await;
    assert!(
        tunnel_dir
            .join(format!("ssh-{}", std::process::id()))
            .is_dir(),
        "tunnel socket must live inside the test's isolated tunnel_dir"
    );
    assert!(
        wake_started.elapsed() < Duration::from_secs(1),
        "expected immediate auth retry wake, took {:?}",
        wake_started.elapsed()
    );
    let epoch_before = host.model.lock().unwrap().connection_epoch;
    let attempts_before = std::fs::read_to_string(&count_file)
        .unwrap()
        .lines()
        .count();

    // Updating SshAgent while connected must NEVER drop the live connection or bump epoch.
    host.commands
        .send(HostCommand::SshAgent {
            socket: Some(bad_sock),
            wake_auth_retry: true,
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    {
        let model = host.model.lock().unwrap();
        assert!(model.connected, "connected host must stay connected");
        assert_eq!(model.connection_epoch, epoch_before);
    }
    assert_eq!(
        std::fs::read_to_string(&count_file)
            .unwrap()
            .lines()
            .count(),
        attempts_before,
        "connected host must not spawn ssh again"
    );

    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn ssh_agent_wake_ignores_key_problems_no_server_and_non_ssh_hosts() {
    let dir = tempfile::tempdir().unwrap();
    let good_sock = dir.path().join("good-agent.sock");

    for (name, stderr_or_stdout, expected_problem) in [
        (
            "unknown-key",
            "echo 'No ED25519 host key is known for box and you have requested strict checking.' >&2; exit 255",
            crate::ssh::Problem::UnknownKey,
        ),
        (
            "changed-key",
            "echo 'WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!' >&2; exit 255",
            crate::ssh::Problem::ChangedKey,
        ),
        (
            "no-server",
            "printf 'none tester Linux 1000 rex\\n'",
            crate::ssh::Problem::NoServer(crate::ssh::Facts {
                user: "tester".into(),
                os: "Linux".into(),
                uid: Some(1000),
                rex: true,
            }),
        ),
    ] {
        let count_file = dir.path().join(format!("{name}.count"));
        let fake_ssh = dir.path().join(format!("ssh-{name}"));
        std::fs::write(
            &fake_ssh,
            format!(
                "#!/bin/sh\necho x >> '{}'\n{stderr_or_stdout}\n",
                count_file.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake_ssh, std::fs::Permissions::from_mode(0o700)).unwrap();

        let host = spawn_with_fake_ssh(
            &format!("ssh://tester@{name}"),
            fake_ssh,
            None,
            dir.path().join(format!("tunnels-{name}")),
        );
        wait_for(Duration::from_secs(5), || {
            host.model.lock().unwrap().ssh_problem.as_ref() == Some(&expected_problem)
        })
        .await;
        let before = std::fs::read_to_string(&count_file)
            .unwrap()
            .lines()
            .count();
        host.commands
            .send(HostCommand::SshAgent {
                socket: Some(good_sock.clone()),
                wake_auth_retry: true,
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        let after = std::fs::read_to_string(&count_file)
            .unwrap()
            .lines()
            .count();
        assert_eq!(
            after, before,
            "{name} must NOT wake early on HostCommand::SshAgent"
        );
        drop(host);
    }

    // Non-SSH host (unix://) also ignores wake_auth_retry cleanly.
    let unix_host = HostConnection::spawn(
        format!("unix://{}", dir.path().join("missing-rex.sock").display()),
        None,
        None,
        None,
        false,
        None,
    );
    wait_for(Duration::from_secs(5), || {
        unix_host
            .model
            .lock()
            .unwrap()
            .status
            .starts_with("Disconnected")
    })
    .await;
    unix_host
        .commands
        .send(HostCommand::SshAgent {
            socket: Some(good_sock),
            wake_auth_retry: true,
        })
        .unwrap();
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(!unix_host.model.lock().unwrap().connected);
    drop(unix_host);
}
