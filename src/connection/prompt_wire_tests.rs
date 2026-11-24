use super::*;
use prompt::{EndReason, PromptAnswer, PromptEvent, PromptEventKind, ReplyOutcome};
use tokio::{sync::oneshot, time::timeout};

type Ws = WebSocketStream<TcpStream>;
const WAIT: Duration = Duration::from_secs(5);
const REPLY_WAIT: Duration = Duration::from_secs(12);

fn no_sweep(req: &Value) -> Option<(u64, Value)> {
    panic!("unexpected request: {req}");
}

async fn next(ws: &mut Ws, method: &str) -> Value {
    let req = timeout(WAIT, recv_answering_sweep(ws, &mut |_| None))
        .await
        .expect("request did not arrive");
    assert_eq!(req["method"], method, "{req}");
    req
}

async fn close(ws: &mut Ws) {
    timeout(WAIT, expect_close_answering_sweep(ws, &mut no_sweep))
        .await
        .expect("client did not close");
}

async fn fixture() -> (TcpListener, HostConnection) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    (listener, host)
}

async fn boot_req(listener: &TcpListener, rev: u32) -> (Ws, Value) {
    let mut ws = timeout(WAIT, accept(listener, "rex.control.v1"))
        .await
        .unwrap();
    timeout(WAIT, welcome(&mut ws, rev)).await.unwrap();
    let req = next(&mut ws, "session.bootstrap").await;
    (ws, req)
}

async fn boot_empty(ws: &mut Ws, req: &Value) {
    let boot = session::BootstrapResponse {
        client_id: "client:test".into(),
        ..Default::default()
    };
    reply(ws, req, 10, json!(boot)).await;
}

async fn event(ws: &mut Ws, name: &str, payload: Value) {
    // Below bootstrap's cutoff on purpose. These are directed, not session events.
    send(
        ws,
        json!({
            "type":"event", "seq":1, "payload":{"type":name,"payload":payload}
        }),
    )
    .await;
}

async fn offer(ws: &mut Ws, call_id: &str, kind: &str, timeout_ms: u64) {
    event(
        ws,
        "client_prompt_requested",
        json!({
            "call_id":call_id, "action":"ask", "timeout_ms":timeout_ms,
            "prompt":{"kind":kind,"title":"Ask"}
        }),
    )
    .await;
}

async fn ended(ws: &mut Ws, call_id: &str, reason: &str) {
    event(
        ws,
        "client_prompt_ended",
        json!({"call_id":call_id,"reason":reason}),
    )
    .await;
}

async fn error(ws: &mut Ws, req: &Value, code: &str, message: &str) {
    send(
        ws,
        json!({
            "type":"response","id":req["id"],"seq":11,
            "error":{"code":code,"message":message,"status":400}
        }),
    )
    .await;
}

fn command(host: &HostConnection, command: HostCommand) {
    host.commands.send(command).unwrap();
}

fn barrier(host: &HostConnection) {
    command(
        host,
        HostCommand::Request {
            method: "server.version".into(),
            payload: json!({}),
        },
    );
}

async fn pass_barrier(ws: &mut Ws) {
    let req = next(ws, "server.version").await;
    reply(ws, &req, 11, json!({})).await;
}

async fn until<T>(host: &HostConnection, predicate: impl Fn(&mut HostModel) -> Option<T>) -> T {
    timeout(REPLY_WAIT, async {
        loop {
            if let Some(value) = predicate(&mut host.model.lock().unwrap()) {
                return value;
            }
            host.wake.recv_async().await.unwrap();
        }
    })
    .await
    .expect("host model did not reach expected state")
}

async fn prompt_event(host: &HostConnection) -> PromptEvent {
    until(host, |m| m.prompt_events.pop_front()).await
}

async fn requested(host: &HostConnection, id: &str) -> u64 {
    let evt = prompt_event(host).await;
    let PromptEventKind::Requested(prompt) = evt.kind else {
        panic!("expected offer, got {:?}", evt.kind);
    };
    assert_eq!(prompt.call_id, id);
    evt.epoch
}

async fn observed(host: &HostConnection, epoch: u64, kind: PromptEventKind) {
    assert_eq!(prompt_event(host).await, PromptEvent { epoch, kind });
    assert!(host.model.lock().unwrap().connected);
}

fn answer(host: &HostConnection, epoch: u64, id: &str, answer: PromptAnswer) {
    command(
        host,
        HostCommand::PromptReply {
            epoch,
            call_id: id.into(),
            answer,
        },
    );
}

async fn recv_answer(ws: &mut Ws, id: &str, result: Value) -> Value {
    let req = next(ws, "client.reply").await;
    assert_eq!(req["payload"], json!({"call_id":id,"result":result}));
    req
}

async fn recv_refusal(ws: &mut Ws, id: &str) {
    let req = next(ws, "client.reply").await;
    assert_eq!(req["payload"]["call_id"], id);
    assert_eq!(req["payload"]["error"]["code"], "not_supported");
    assert!(req["payload"].get("result").is_none());
    reply(ws, &req, 11, json!({})).await;
}

fn action(host: &HostConnection, name: &str) {
    command(
        host,
        HostCommand::Action {
            generation: 42,
            request: Box::new(
                rexrs::api::client::Action::new(name)
                    .session_id(SESSION)
                    .block_id("pane"),
            ),
        },
    );
}

#[tokio::test]
async fn action_wait_promotes_on_prompt_and_applies_client_actions_after_reply() {
    let (listener, host) = fixture().await;
    let (action_seen, wait_for_action) = oneshot::channel();
    let (barrier_queued, wait_for_barrier) = oneshot::channel();
    let (release_action, wait_for_release) = oneshot::channel();
    let server =
        tokio::spawn(async move {
            let (mut ws, boot) = boot_req(&listener, 21).await;
            boot_empty(&mut ws, &boot).await;
            let action = next(&mut ws, "client.action").await;
            assert_eq!(action["payload"]["action"], "ask");
            action_seen.send(()).unwrap();
            wait_for_barrier.await.unwrap();
            // Poke the transport without letting the outstanding action return.
            event(&mut ws, "hosts_changed", json!({})).await;
            // No integer timeout means no suspension budget. Even in exclusive mode the
            // malformed request gets its refusal before the queued ordinary command.
            event(
                &mut ws,
                "client_prompt_requested",
                json!({
                    "call_id":"malformed","prompt":{"kind":"future"}
                }),
            )
            .await;
            recv_refusal(&mut ws, "malformed").await;
            offer(&mut ws, "action-prompt", "input", 60_000).await;
            // The previously queued command can now run, with the action still unanswered.
            pass_barrier(&mut ws).await;
            let req = recv_answer(&mut ws, "action-prompt", json!({"text":""})).await;
            reply(&mut ws, &req, 12, json!({})).await;
            wait_for_release.await.unwrap();
            reply(&mut ws, &action, 13, json!({
            "client_actions":[{"action":"probe.first"},{"action":"probe.second","args":{"x":1}}]
        }))
        .await;
            close(&mut ws).await;
        });
    connected(&host).await;
    command(&host, HostCommand::Prompts(true));
    action(&host, "ask");
    timeout(WAIT, wait_for_action).await.unwrap().unwrap();
    barrier(&host);
    barrier_queued.send(()).unwrap();
    let epoch = requested(&host, "action-prompt").await;
    assert!(host.model.lock().unwrap().client_actions.is_empty());
    answer(
        &host,
        epoch,
        "action-prompt",
        PromptAnswer::Text(String::new()),
    );
    observed(
        &host,
        epoch,
        PromptEventKind::ReplyOutcome {
            call_id: "action-prompt".into(),
            outcome: ReplyOutcome::Accepted,
        },
    )
    .await;
    release_action.send(()).unwrap();
    until(&host, |m| {
        (m.client_actions.len() == 2).then(|| {
            for (queued, name) in m.client_actions.iter().zip(["probe.first", "probe.second"]) {
                assert_eq!(queued.generation, 42);
                assert_eq!(queued.session.as_deref(), Some(SESSION));
                assert_eq!(queued.block.as_deref(), Some("pane"));
                assert_eq!(queued.action.action, name);
            }
        })
    })
    .await;
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn deferred_commands_settle_before_backoff_and_newer_settings_win() {
    let (listener, host) = fixture().await;
    let (action_seen, wait_for_action) = oneshot::channel();
    let (drained, wait_for_drain) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut ws, boot) = boot_req(&listener, 21).await;
        boot_empty(&mut ws, &boot).await;
        let _unanswered = next(&mut ws, "client.action").await;
        action_seen.send(()).unwrap();
        timeout(WAIT, wait_for_drain).await.unwrap().unwrap();
        ws.close(None).await.unwrap();
        close(&mut ws).await;
        let (mut ws, boot) = boot_req(&listener, 21).await;
        boot_empty(&mut ws, &boot).await;
        close(&mut ws).await;
    });
    connected(&host).await;
    let epoch = host.model.lock().unwrap().connection_epoch;
    action(&host, "no_prompt");
    timeout(WAIT, wait_for_action).await.unwrap().unwrap();
    let open = Arc::new(());
    command(
        &host,
        HostCommand::ListDir(Box::new(directory::Request {
            id: 71,
            epoch,
            session: SESSION.into(),
            block: "pane".into(),
            args: rexrs::api::terminal::ListDir::new().path("."),
            deadline: Instant::now() + REPLY_WAIT,
            open: Arc::downgrade(&open),
        })),
    );
    command(&host, HostCommand::Visibility(true));
    // Wait for dequeue, not just enqueue, so a still-buffered channel command
    // cannot accidentally pass the test through the regular disconnect handler.
    timeout(WAIT, async {
        while !host.commands.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    {
        let model = host.model.lock().unwrap();
        assert!(!model.visible);
        assert!(model.directory.is_none());
    }
    drained.send(()).unwrap();
    until(&host, |m| (!m.connected).then_some(())).await;
    // Queue a newer setting while the socket is down, before the two-second retry.
    command(&host, HostCommand::Visibility(false));
    let (send_ticket, ticket_reply) = oneshot::channel();
    command(
        &host,
        HostCommand::Ticket {
            session: SESSION.into(),
            block: "pane".into(),
            reply: send_ticket,
        },
    );
    let receipt = timeout(
        Duration::from_secs(1),
        until(&host, |m| m.directory.clone()),
    )
    .await
    .expect("deferred listing must fail before reconnect backoff elapses");
    assert_eq!((receipt.id, receipt.epoch), (71, epoch));
    assert_eq!(receipt.result, Err(directory::Failure::Disconnected));
    assert!(matches!(
        timeout(WAIT, ticket_reply).await.unwrap().unwrap(),
        Err(message) if message == "Host is reconnecting"
    ));
    let visible = until(&host, |m| {
        (m.connected && m.connection_epoch > epoch).then_some(m.visible)
    })
    .await;
    assert!(!visible, "the deferred setting overwrote a newer one");
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn prompt_bad_request_keeps_offer_and_ended_discards_the_right_id() {
    let (listener, host) = fixture().await;
    let (end_now, wait_to_end) = oneshot::channel();
    let (finish, wait_to_finish) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut ws, boot) = boot_req(&listener, 21).await;
        boot_empty(&mut ws, &boot).await;
        pass_barrier(&mut ws).await;
        offer(&mut ws, "pick", "pick", 60_000).await;
        ended(&mut ws, "wrong", "answered").await;
        let req = recv_answer(&mut ws, "pick", json!({"ids":["unknown"]})).await;
        error(&mut ws, &req, "bad_request", " Try \n \x1b\u{202e}again ").await;
        let req = recv_answer(&mut ws, "pick", json!({"cancelled":true})).await;
        reply(&mut ws, &req, 12, json!({})).await;
        offer(&mut ws, "second", "input", 60_000).await;
        wait_to_end.await.unwrap();
        ended(&mut ws, "second", "an_unknown_reason").await;
        wait_to_finish.await.unwrap();
        pass_barrier(&mut ws).await;
        close(&mut ws).await;
    });
    connected(&host).await;
    command(&host, HostCommand::Prompts(true));
    barrier(&host);
    let epoch = requested(&host, "pick").await;
    answer(&host, epoch, "wrong", PromptAnswer::Cancelled);
    answer(
        &host,
        epoch,
        "pick",
        PromptAnswer::Ids(vec!["unknown".into()]),
    );
    observed(
        &host,
        epoch,
        PromptEventKind::ReplyRejected {
            call_id: "pick".into(),
            message: "Try again".into(),
        },
    )
    .await;
    answer(&host, epoch, "pick", PromptAnswer::Cancelled);
    observed(
        &host,
        epoch,
        PromptEventKind::ReplyOutcome {
            call_id: "pick".into(),
            outcome: ReplyOutcome::Accepted,
        },
    )
    .await;
    assert_eq!(requested(&host, "second").await, epoch);
    end_now.send(()).unwrap();
    observed(
        &host,
        epoch,
        PromptEventKind::Ended {
            call_id: "second".into(),
            reason: EndReason::Unknown,
        },
    )
    .await;
    answer(&host, epoch, "second", PromptAnswer::Cancelled);
    barrier(&host);
    finish.send(()).unwrap();
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn prompt_not_found_other_error_and_reply_timeout_never_retry_or_disconnect() {
    let (listener, host) = fixture().await;
    let (resume, wait_for_timeout) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut ws, boot) = boot_req(&listener, 21).await;
        boot_empty(&mut ws, &boot).await;
        pass_barrier(&mut ws).await;
        for (id, code) in [
            ("missing", "not_found"),
            ("failed", "internal"),
            ("timeout", ""),
        ] {
            offer(&mut ws, id, "input", 60_000).await;
            let req = recv_answer(&mut ws, id, json!({"cancelled":true})).await;
            if !code.is_empty() {
                error(&mut ws, &req, code, "test").await;
            }
        }
        timeout(REPLY_WAIT, wait_for_timeout)
            .await
            .unwrap()
            .unwrap();
        pass_barrier(&mut ws).await;
        close(&mut ws).await;
    });
    connected(&host).await;
    command(&host, HostCommand::Prompts(true));
    barrier(&host);
    for (id, outcome) in [
        ("missing", ReplyOutcome::NotFound),
        ("failed", ReplyOutcome::Failed),
        ("timeout", ReplyOutcome::Failed),
    ] {
        let epoch = requested(&host, id).await;
        answer(&host, epoch, id, PromptAnswer::Cancelled);
        observed(
            &host,
            epoch,
            PromptEventKind::ReplyOutcome {
                call_id: id.into(),
                outcome,
            },
        )
        .await;
        // Once done, even an explicit duplicate answer stays local.
        answer(&host, epoch, id, PromptAnswer::Cancelled);
    }
    barrier(&host);
    resume.send(()).unwrap();
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn prompt_unknown_and_busy_refuse_disable_clears_and_drop_surfaces() {
    let (listener, host) = fixture().await;
    let (disable_seen, wait_for_disable) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut ws, boot) = boot_req(&listener, 21).await;
        boot_empty(&mut ws, &boot).await;
        pass_barrier(&mut ws).await;
        offer(&mut ws, "future", "future_kind", 60_000).await;
        recv_refusal(&mut ws, "future").await;
        offer(&mut ws, "first", "input", 60_000).await;
        offer(&mut ws, "busy", "input", 60_000).await;
        recv_refusal(&mut ws, "busy").await;
        event(
            &mut ws,
            "event_dropped",
            json!({"event_type":"client_prompt_requested","size":100_000}),
        )
        .await;
        recv_refusal(&mut ws, "first").await;
        event(
            &mut ws,
            "event_dropped",
            json!({"event_type":"client_prompt_requested","size":100_000}),
        )
        .await;
        // Observing the later refusal proves the earlier drop was processed while off.
        offer(&mut ws, "disabled", "input", 60_000).await;
        recv_refusal(&mut ws, "disabled").await;
        disable_seen.send(()).unwrap();
        pass_barrier(&mut ws).await;
        close(&mut ws).await;
    });
    connected(&host).await;
    command(&host, HostCommand::Prompts(true));
    barrier(&host);
    let epoch = requested(&host, "first").await;
    observed(&host, epoch, PromptEventKind::Dropped).await;
    command(&host, HostCommand::Prompts(false));
    observed(
        &host,
        epoch,
        PromptEventKind::Ended {
            call_id: "first".into(),
            reason: EndReason::Cancelled,
        },
    )
    .await;
    timeout(WAIT, wait_for_disable).await.unwrap().unwrap();
    assert!(host.model.lock().unwrap().prompt_events.is_empty());
    answer(&host, epoch, "first", PromptAnswer::Cancelled);
    barrier(&host);
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn oversized_prompt_reply_is_rejected_locally_and_can_be_shortened() {
    let (listener, host) = fixture().await;
    let server = tokio::spawn(async move {
        let (mut ws, boot) = boot_req(&listener, 21).await;
        boot_empty(&mut ws, &boot).await;
        pass_barrier(&mut ws).await;
        offer(&mut ws, "input", "input", 60_000).await;
        // The negotiated 65536-byte limit keeps the earlier answer off the wire.
        let req = recv_answer(&mut ws, "input", json!({"text":"short"})).await;
        reply(&mut ws, &req, 11, json!({})).await;
        close(&mut ws).await;
    });
    connected(&host).await;
    command(&host, HostCommand::Prompts(true));
    barrier(&host);
    let epoch = requested(&host, "input").await;
    answer(
        &host,
        epoch,
        "input",
        PromptAnswer::Text("界".repeat(24_000)),
    );
    observed(
        &host,
        epoch,
        PromptEventKind::ReplyRejected {
            call_id: "input".into(),
            message: "The answer is too large for this server. Shorten it and try again.".into(),
        },
    )
    .await;
    answer(&host, epoch, "input", PromptAnswer::Text("short".into()));
    observed(
        &host,
        epoch,
        PromptEventKind::ReplyOutcome {
            call_id: "input".into(),
            outcome: ReplyOutcome::Accepted,
        },
    )
    .await;
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn prompt_local_deadline_expires_without_a_reply() {
    let (listener, host) = fixture().await;
    let server = tokio::spawn(async move {
        let (mut ws, boot) = boot_req(&listener, 21).await;
        boot_empty(&mut ws, &boot).await;
        pass_barrier(&mut ws).await;
        offer(&mut ws, "expires", "input", 30).await;
        pass_barrier(&mut ws).await;
        close(&mut ws).await;
    });
    connected(&host).await;
    command(&host, HostCommand::Prompts(true));
    barrier(&host);
    let epoch = requested(&host, "expires").await;
    observed(
        &host,
        epoch,
        PromptEventKind::Ended {
            call_id: "expires".into(),
            reason: EndReason::Timeout,
        },
    )
    .await;
    answer(&host, epoch, "expires", PromptAnswer::Cancelled);
    barrier(&host);
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn prompt_disconnect_drops_wait_events_and_stale_epoch_replies() {
    let (listener, host) = fixture().await;
    let (new_offer, wait_for_new_offer) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut ws, boot) = boot_req(&listener, 21).await;
        boot_empty(&mut ws, &boot).await;
        let _action = next(&mut ws, "client.action").await;
        offer(&mut ws, "reused", "input", 60_000).await;
        let _unanswered = recv_answer(&mut ws, "reused", json!({"text":"old"})).await;
        ws.close(None).await.unwrap();
        let (mut ws, boot) = boot_req(&listener, 21).await;
        offer(&mut ws, "early-future", "future_kind", 60_000).await;
        recv_refusal(&mut ws, "early-future").await;
        boot_empty(&mut ws, &boot).await;
        wait_for_new_offer.await.unwrap();
        offer(&mut ws, "reused", "input", 60_000).await;
        let req = recv_answer(&mut ws, "reused", json!({"text":"fresh"})).await;
        reply(&mut ws, &req, 11, json!({})).await;
        close(&mut ws).await;
    });
    connected(&host).await;
    command(&host, HostCommand::Prompts(true));
    action(&host, "ask");
    let epoch = until(&host, |m| m.prompt_events.front().map(|evt| evt.epoch)).await;
    answer(&host, epoch, "reused", PromptAnswer::Text("old".into()));
    until(&host, |m| {
        (!m.connected && m.prompt_events.is_empty()).then_some(())
    })
    .await;
    command(&host, HostCommand::Reconnect);
    until(&host, |m| {
        (m.connected && m.connection_epoch != epoch).then(|| {
            assert!(m.prompt_events.is_empty());
            assert!(m.client_actions.is_empty());
        })
    })
    .await;
    new_offer.send(()).unwrap();
    let current = requested(&host, "reused").await;
    assert_ne!(epoch, current);
    answer(&host, epoch, "reused", PromptAnswer::Text("stale".into()));
    answer(&host, current, "reused", PromptAnswer::Text("fresh".into()));
    observed(
        &host,
        current,
        PromptEventKind::ReplyOutcome {
            call_id: "reused".into(),
            outcome: ReplyOutcome::Accepted,
        },
    )
    .await;
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn prompt_api_16_ignores_even_unexpected_prompt_events() {
    let (listener, host) = fixture().await;
    let (sent, wait_for_events) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut ws, boot) = boot_req(&listener, 16).await;
        offer(&mut ws, "early", "future_kind", 60_000).await;
        boot_empty(&mut ws, &boot).await;
        pass_barrier(&mut ws).await;
        offer(&mut ws, "late", "input", 60_000).await;
        ended(&mut ws, "late", "answered").await;
        event(
            &mut ws,
            "event_dropped",
            json!({"event_type":"client_prompt_requested","size":200}),
        )
        .await;
        // Outgoing verification: the next command should be the barrier, never client.reply.
        sent.send(()).unwrap();
        pass_barrier(&mut ws).await;
        close(&mut ws).await;
    });
    connected(&host).await;
    command(&host, HostCommand::Prompts(true));
    barrier(&host);
    timeout(WAIT, wait_for_events).await.unwrap().unwrap();
    barrier(&host);
    command(&host, HostCommand::Visibility(true));
    until(&host, |m| m.visible.then_some(())).await;
    assert!(host.model.lock().unwrap().prompt_events.is_empty());
    drop(host);
    server.await.unwrap();
}
