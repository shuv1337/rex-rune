use super::*;
use serde_json::json;

const READ: &str = "com.superlogical.terminal.program_resume_records";
const B1: &str = "block:00000000-0000-0000-0000-000000000001";
const B2: &str = "block:00000000-0000-0000-0000-000000000002";
const B3: &str = "block:00000000-0000-0000-0000-000000000003";
const BACK: &str = "block:00000000-0000-0000-0000-000000000004";
const OTHER: &str = "session:00000000-0000-0000-0000-000000000002";
const SENTINEL: &str = "synthetic-resume-wire-secret";

fn view(id: &str, revision: u64, terminal: &[&str]) -> session::ViewResponse {
    session::ViewResponse {
        session_id: Some(id.into()),
        revision,
        windows: vec![session::Window {
            window_id: Some("w1".into()),
            layers: vec![session::Layer {
                blocks: terminal
                    .iter()
                    .map(|block| session::LayerBlock {
                        block_id: Some((*block).into()),
                        creator_name: Some(activity::TERMINAL.into()),
                        ..Default::default()
                    })
                    .chain([session::LayerBlock {
                        block_id: Some("fs".into()),
                        creator_name: Some("com.superlogical.fs".into()),
                        ..Default::default()
                    }])
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn snapshot(program: &str) -> Value {
    json!({"records": [{
        "owner": {"pid": 12, "name": "shell"},
        "program": program,
        "reported_at": "2026-11-25T22:31:01Z",
        "chosen": true
    }]})
}

fn report(seq: u64, id: &str, block: &str, body: Value) -> Value {
    json!({"type": "event", "seq": seq, "payload": {
        "type": "session_event",
        "payload": {"session_id": id, "event": {
            "type": "block_event",
            "payload": {"creator_name": activity::TERMINAL, "name": program_resume::EVENT,
                        "block_id": block, "payload": body}
        }}
    }})
}

struct Mock {
    ws: WebSocketStream<TcpStream>,
    control: Client,
    shared: Shared,
    wake: flume::Receiver<()>,
    state: ControlState,
    sweep: status_sweep::Reader,
}

impl Mock {
    async fn new(revision: u32, blocks: &[&str]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut ws = accept(&listener, "rex.control.v1").await;
            welcome(&mut ws, revision).await;
            ws
        });
        let control = transport::control(&endpoint, None).await.unwrap();
        let model = HostModel {
            connected: true,
            selected: Some(SESSION.into()),
            views: [
                (SESSION.into(), view(SESSION, 1, blocks)),
                (OTHER.into(), view(OTHER, 1, &[BACK])),
            ]
            .into(),
            background_views: [OTHER.into()].into(),
            ..Default::default()
        };
        let (wake_tx, wake) = flume::bounded(1);
        Self {
            ws: server.await.unwrap(),
            control,
            shared: Shared {
                model: Arc::new(Mutex::new(model)),
                wake: title_wake::HostWake::new(wake_tx),
            },
            wake,
            state: ControlState::default(),
            sweep: status_sweep::Reader::default(),
        }
    }

    async fn event(&mut self, raw: Value) {
        send(&mut self.ws, raw).await;
        let event = self.control.next_event().await.unwrap();
        handle_event(
            event,
            &mut self.control,
            &self.shared,
            &mut HashMap::new(),
            &mut self.state,
            &mut self.sweep,
            None,
            &mut server_info::Reader::default(),
            &mut hosts::Reader::default(),
        )
        .await
        .unwrap();
    }

    async fn watch(&mut self, id: Option<&str>) {
        program_resume::watch(
            &mut self.control,
            &self.shared,
            &mut self.state,
            id.map(str::to_owned),
        )
        .await
        .unwrap();
    }

    async fn only_fence(&mut self) {
        let _fence = self
            .control
            .transport()
            .detach("test.fence", json!({}))
            .await
            .unwrap();
        assert_eq!(recv(&mut self.ws).await["method"], "test.fence");
    }

    async fn request(&mut self, id: &str, block: &str) -> Value {
        let req = recv(&mut self.ws).await;
        assert_eq!(req["method"], READ);
        assert_eq!(
            req["payload"],
            json!({"session_id": id, "block_id": block, "args": {}})
        );
        req
    }

    async fn send_response(&mut self, req: &Value, seq: u64, mut body: Value) {
        // Unlike the event, the read carries launch arguments, even if it arrives late.
        if let Some(record) = body["records"].get_mut(0) {
            record["command"] = json!([SENTINEL]);
        }
        send(
            &mut self.ws,
            json!({"type": "response", "id": req["id"], "seq": seq, "payload": body}),
        )
        .await;
        self.flush().await;
    }

    async fn flush(&mut self) {
        send(
            &mut self.ws,
            json!({"type": "event", "seq": 100, "payload": {"type": "fence", "payload": {}}}),
        )
        .await;
        assert_eq!(self.control.next_event().await.unwrap().name, "fence");
    }

    async fn finish(&mut self) {
        let finished = self.state.resume_reads.finished().await;
        program_resume::record_reply(&self.shared, &mut self.state, finished);
        self.state
            .resume_reads
            .pump(&mut self.control, &self.shared)
            .await
            .unwrap();
    }

    fn model(&self) -> HostModel {
        self.shared.model.lock().unwrap().clone()
    }

    fn label(&self, block: &str) -> String {
        self.model().program_resume[block].records[0]
            .program
            .clone()
    }

    fn replace(&mut self, id: &str, revision: u64, blocks: &[&str]) {
        replace_view(
            &self.shared,
            &mut self.state,
            &mut self.sweep,
            id,
            view(id, revision, blocks),
        );
    }
}

#[tokio::test]
async fn api_23_discards_delivered_events_and_sends_no_reads() {
    let mut mock = Mock::new(23, &[B1]).await;
    mock.watch(Some(SESSION)).await;
    mock.event(report(2, SESSION, B1, snapshot("ignored")))
        .await;
    mock.only_fence().await;
    assert!(mock.model().program_resume.is_empty());
    assert!(mock.model().program_resume_reading.is_empty());
    assert!(mock.model().activity.is_empty());
    assert!(mock.state.resume_seq.is_empty());
    assert!(mock.wake.try_recv().is_err());
}

#[tokio::test]
async fn watches_only_foreground_terminals_once_and_limits_concurrency() {
    let mut mock = Mock::new(24, &[B1, B2, B3]).await;
    mock.only_fence().await;
    mock.watch(Some(OTHER)).await;
    // Even a selected but background-only view cannot be watched.
    mock.shared.model.lock().unwrap().selected = Some(OTHER.into());
    mock.watch(Some(OTHER)).await;
    mock.only_fence().await;
    mock.shared.model.lock().unwrap().selected = Some(SESSION.into());
    mock.watch(Some(SESSION)).await;
    let first = mock.request(SESSION, B1).await;
    let second = mock.request(SESSION, B2).await;
    assert_eq!(mock.state.resume_reads.in_flight(), 2);
    mock.only_fence().await;
    mock.send_response(&first, 1, snapshot("first")).await;
    mock.finish().await;
    let third = mock.request(SESSION, B3).await;
    for request in [&second, &third] {
        mock.send_response(request, 2, json!({"records": []})).await;
        mock.finish().await;
    }
    mock.watch(Some(SESSION)).await;
    mock.only_fence().await;
    assert_eq!(mock.model().program_resume.len(), 3);
    mock.watch(None).await;
    mock.watch(Some(SESSION)).await;
    mock.request(SESSION, B1).await;
    mock.request(SESSION, B2).await;
    mock.shared.model.lock().unwrap().selected = Some(OTHER.into());
    program_resume::reconcile_selected(&mock.shared, &mut mock.state);
    assert!(mock.model().program_resume_reading.is_empty());
    assert_eq!(mock.state.resume_reads.in_flight(), 0);
    mock.only_fence().await;
}

#[tokio::test]
async fn events_ignore_layout_floor_and_only_wake_for_changed_records() {
    let mut mock = Mock::new(24, &[B1]).await;
    mock.state.view_seq.insert(SESSION.into(), 99);
    mock.watch(Some(SESSION)).await;
    mock.wake.try_recv().unwrap();
    let old = mock.request(SESSION, B1).await;
    mock.event(report(3, SESSION, B1, snapshot("first"))).await;
    assert_eq!(mock.label(B1), "first");
    mock.wake.try_recv().unwrap();
    mock.event(report(3, SESSION, B1, snapshot("duplicate")))
        .await;
    mock.event(report(4, SESSION, B1, snapshot("first"))).await;
    assert!(mock.wake.try_recv().is_err());
    mock.send_response(&old, 2, snapshot("old")).await;
    mock.finish().await;
    assert_eq!(mock.label(B1), "first");
    mock.wake.try_recv().unwrap(); // The stale reply still clears the reading mark.
    mock.watch(None).await;
    mock.watch(Some(SESSION)).await;
    let tied = mock.request(SESSION, B1).await;
    mock.send_response(&tied, 4, snapshot("tie goes to reply"))
        .await;
    mock.finish().await;
    assert_eq!(mock.label(B1), "tie goes to reply");
    mock.event(report(4, SESSION, B1, snapshot("duplicate again")))
        .await;
    mock.event(report(5, OTHER, BACK, snapshot("passive background")))
        .await;
    mock.event(report(6, "unheld", "new", snapshot("unheld")))
        .await;
    assert_eq!(mock.label(B1), "tie goes to reply");
    assert_eq!(mock.label(BACK), "passive background");
    let model = mock.model();
    assert_eq!(model.program_resume.len(), 2);
    assert!(model.activity.is_empty());
    assert!(model.bells.is_empty());
    assert!(model.notices.is_empty());
    assert!(mock.state.urgent_views.is_empty());
    mock.only_fence().await;
}

#[tokio::test]
async fn named_and_unnamed_drops_reread_only_watched_blocks_and_ignore_late_replies() {
    let mut mock = Mock::new(24, &[B1]).await;
    mock.event(report(1, OTHER, BACK, snapshot("background")))
        .await;
    mock.event(report(2, SESSION, B1, snapshot("initial")))
        .await;
    mock.watch(Some(SESSION)).await;
    let superseded = mock.request(SESSION, B1).await;
    mock.event(
        json!({"type": "event", "seq": 5, "payload": {"type": "event_dropped",
        "payload": {"event_type": "block_event", "block_event_name": program_resume::EVENT,
                    "session_id": SESSION, "block_id": B1}}}),
    )
    .await;
    assert_eq!(mock.state.resume_seq[B1], 5);
    assert!(!mock.model().program_resume.contains_key(B1));
    let replacement = mock.request(SESSION, B1).await;
    mock.send_response(&superseded, 4, snapshot("late")).await;
    mock.event(report(5, SESSION, B1, snapshot("old event")))
        .await;
    assert!(!mock.model().program_resume.contains_key(B1));
    mock.send_response(&replacement, 5, snapshot("recovered"))
        .await;
    mock.finish().await;
    assert_eq!(mock.label(B1), "recovered");
    mock.event(
        json!({"type": "event", "seq": 6, "payload": {"type": "event_dropped",
        "payload": {"event_type": "block_event"}}}),
    )
    .await;
    let broad = mock.request(SESSION, B1).await;
    assert!(mock.model().program_resume.is_empty());
    assert_eq!(mock.state.resume_seq[BACK], 6);
    mock.only_fence().await;
    mock.watch(None).await;
    mock.send_response(&broad, 6, snapshot("closed")).await;
    mock.event(
        json!({"type": "event", "seq": 7, "payload": {"type": "event_dropped",
        "payload": {"event_type": "block_event", "block_event_name": program_resume::EVENT}}}),
    )
    .await;
    mock.only_fence().await;
    assert!(mock.model().program_resume.is_empty());
    assert!(mock.model().program_resume_reading.is_empty());
}

#[tokio::test]
async fn timeout_does_not_erase_a_newer_event() {
    let mut mock = Mock::new(24, &[B1]).await;
    tokio::time::pause();
    mock.watch(Some(SESSION)).await;
    let late = mock.request(SESSION, B1).await;
    mock.event(report(2, SESSION, B1, snapshot("live"))).await;
    mock.wake.try_recv().unwrap();
    tokio::time::advance(Duration::from_secs(8)).await;
    mock.finish().await;
    assert_eq!(mock.label(B1), "live");
    assert!(mock.model().program_resume_reading.is_empty());
    mock.wake.try_recv().unwrap();
    mock.only_fence().await;
    mock.send_response(&late, 3, snapshot("too late")).await;
    assert_eq!(mock.label(B1), "live");
}

#[tokio::test]
async fn unsupported_is_per_connection_without_disconnect_or_further_reads() {
    for code in ["method_not_found", "revision_too_low", "not_supported"] {
        let mut mock = Mock::new(24, &[B1, B2, B3]).await;
        mock.watch(Some(SESSION)).await;
        let unsupported = mock.request(SESSION, B1).await;
        mock.request(SESSION, B2).await;
        mock.event(report(2, SESSION, B1, snapshot("keep event")))
            .await;
        mock.wake.try_recv().unwrap();
        send(
            &mut mock.ws,
            json!({"type":"response", "id":unsupported["id"],"seq":1,
            "error": {"code":code,"message":SENTINEL}}),
        )
        .await;
        mock.flush().await;
        mock.finish().await;
        mock.wake.try_recv().unwrap();
        mock.watch(None).await;
        mock.watch(Some(SESSION)).await;
        mock.only_fence().await;
        let model = mock.model();
        assert!(model.connected && model.program_resume_unsupported);
        assert!(model.program_resume_reading.is_empty() && model.notice.is_none());
        assert_eq!(mock.label(B1), "keep event");
    }
}

#[tokio::test]
async fn failed_read_wakes_without_a_snapshot() {
    let mut mock = Mock::new(24, &[B1]).await;
    mock.watch(Some(SESSION)).await;
    let failed = mock.request(SESSION, B1).await;
    mock.wake.try_recv().unwrap();
    mock.send_response(&failed, 2, json!({"records": "invalid"}))
        .await;
    mock.finish().await;
    mock.wake.try_recv().unwrap();
    let model = mock.model();
    assert!(model.program_resume_reading.is_empty());
    assert!(model.program_resume.is_empty());
    assert!(!model.program_resume_unsupported);
}

#[tokio::test]
async fn identical_and_ineligible_replies_wake_to_clear_reading() {
    for still_eligible in [true, false] {
        let mut mock = Mock::new(24, &[B1]).await;
        mock.event(report(2, SESSION, B1, snapshot("unchanged")))
            .await;
        mock.watch(Some(SESSION)).await;
        let request = mock.request(SESSION, B1).await;
        mock.wake.try_recv().unwrap();
        if !still_eligible {
            mock.shared.model.lock().unwrap().selected = Some(OTHER.into());
        }
        mock.send_response(&request, 2, snapshot("unchanged")).await;
        mock.finish().await;
        mock.wake.try_recv().unwrap();
        assert_eq!(mock.label(B1), "unchanged");
        assert!(mock.model().program_resume_reading.is_empty());
    }
}

#[tokio::test]
async fn a_named_drop_wakes_when_it_cancels_a_read_without_a_snapshot() {
    let mut mock = Mock::new(24, &[B1]).await;
    mock.watch(Some(SESSION)).await;
    mock.request(SESSION, B1).await;
    mock.wake.try_recv().unwrap();
    mock.shared.model.lock().unwrap().selected = Some(OTHER.into());
    mock.event(
        json!({"type": "event", "seq": 5, "payload": {"type": "event_dropped",
        "payload": {"event_type": "block_event", "block_event_name": program_resume::EVENT,
                    "session_id": SESSION, "block_id": B1}}}),
    )
    .await;
    mock.wake.try_recv().unwrap();
    assert!(mock.model().program_resume_reading.is_empty());
    assert!(mock.model().program_resume.is_empty());
    mock.only_fence().await;
}

#[tokio::test]
async fn block_close_undo_transfer_destroy_and_disconnect_forget_incarnations() {
    let mut mock = Mock::new(24, &[B1, B2]).await;
    mock.watch(Some(SESSION)).await;
    let closed = mock.request(SESSION, B1).await;
    let undone = mock.request(SESSION, B2).await;
    mock.event(report(9, SESSION, B1, snapshot("source"))).await;
    mock.event(report(10, SESSION, B2, snapshot("old block")))
        .await;
    mock.event(json!({"type": "event", "seq": 11, "payload": {
        "type": "block_closed", "payload": {"block_id": B1}}}))
        .await;
    mock.replace(SESSION, 2, &[]);
    assert!(mock.state.resume_seq.is_empty());
    assert!(mock.model().program_resume.is_empty());
    assert!(mock.model().program_resume_reading.is_empty());
    mock.replace(SESSION, 3, &[B2]); // Undo restores the same ID.
    mock.state
        .resume_reads
        .pump(&mut mock.control, &mock.shared)
        .await
        .unwrap();
    let restored = mock.request(SESSION, B2).await;
    mock.send_response(&undone, 10, snapshot("old incarnation"))
        .await;
    mock.send_response(&restored, 12, snapshot("restored"))
        .await;
    mock.finish().await;
    assert_eq!(mock.label(B2), "restored");
    mock.replace(OTHER, 2, &[B1]); // Destination observes block_created with the reused ID.
    mock.event(report(13, OTHER, B1, snapshot("destination")))
        .await;
    mock.send_response(&closed, 14, snapshot("old source"))
        .await;
    assert_eq!(mock.label(B1), "destination");
    mock.event(
        json!({"type": "event", "seq": 14, "payload": {"type": "session_destroyed",
        "payload": {"session_id": OTHER}}}),
    )
    .await;
    assert!(!mock.model().program_resume.contains_key(B1));
    assert!(!mock.state.resume_seq.contains_key(B1));
    mock.shared.status("disconnected", false);
    assert!(mock.model().program_resume.is_empty());
    assert!(mock.model().program_resume_reading.is_empty());
}

#[tokio::test]
async fn relist_forgets_a_removed_session() {
    let mut mock = Mock::new(24, &[B1]).await;
    mock.event(report(1, OTHER, BACK, snapshot("removed")))
        .await;
    let list = list(
        &mut mock.control,
        &mock.shared,
        &mut mock.state,
        &mut mock.sweep,
    );
    let respond = async {
        let req = recv(&mut mock.ws).await;
        assert_eq!(req["method"], "session.attach_all");
        send(
            &mut mock.ws,
            json!({"type": "response", "id": req["id"], "seq": 2,
                "payload": {"sessions": [{"session_id": SESSION, "label": "remaining"}]}}),
        )
        .await;
    };
    let (result, ()) = tokio::join!(list, respond);
    result.unwrap();
    assert!(!mock.state.resume_seq.contains_key(BACK));
    assert!(!mock.model().views.contains_key(OTHER));
    assert!(!mock.model().program_resume.contains_key(BACK));
}

async fn non_metadata(ws: &mut WebSocketStream<TcpStream>) -> Value {
    loop {
        let req = recv(ws).await;
        let method = req["method"].as_str().unwrap();
        if matches!(
            method,
            "com.superlogical.terminal.program_status"
                | "com.superlogical.terminal.title"
                | "com.superlogical.terminal.process"
        ) {
            send(
                ws,
                json!({"type":"response","id":req["id"],"seq":2,"payload":{}}),
            )
            .await;
        } else {
            return req;
        }
    }
}

async fn wait_host(host: &HostConnection, check: impl Fn(&HostModel) -> bool) -> u64 {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            {
                let model = host.model.lock().unwrap();
                if check(&model) {
                    return model.connection_epoch;
                }
            }
            host.wake.recv_async().await.unwrap();
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn reconnect_discards_reads_and_old_epoch_watches() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (sent_tx, sent_rx) = tokio::sync::oneshot::channel();
    let (fenced, fence) = flume::bounded(1);
    let server = tokio::spawn(async move {
        let mut sent_tx = Some(sent_tx);
        for first in [true, false] {
            let mut ws = accept(&listener, "rex.control.v1").await;
            welcome(&mut ws, 24).await;
            let req = recv(&mut ws).await;
            assert_eq!(req["method"], "session.bootstrap");
            let mut boot = boot();
            boot.selected_session.as_mut().unwrap().view = view(SESSION, 1, &[B1]);
            send(
                &mut ws,
                json!({"type": "response","id": req["id"],"seq":1,"payload":boot}),
            )
            .await;
            if first {
                send(
                    &mut ws,
                    report(2, SESSION, B1, snapshot("before reconnect")),
                )
                .await;
            }
            // No read may precede this request, including attach and a queued stale watch.
            let req = non_metadata(&mut ws).await;
            assert_eq!(req["method"], "test.fence");
            send(
                &mut ws,
                json!({"type":"response","id":req["id"],"seq":2,"payload":{}}),
            )
            .await;
            fenced.send_async(()).await.unwrap();
            if first {
                let req = non_metadata(&mut ws).await;
                assert_eq!(req["method"], READ);
                assert_eq!(req["payload"]["block_id"], B1);
                sent_tx.take().unwrap().send(()).unwrap();
                // Reconnect deliberately drops the control socket without a Close frame.
                assert!(matches!(
                    ws.next().await,
                    Some(Err(tokio_tungstenite::tungstenite::Error::Protocol(
                        tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake
                    ))) | None
                ));
            } else {
                assert!(matches!(
                    ws.next().await,
                    Some(Ok(Message::Close(_))) | None
                ));
            }
        }
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false, None);
    let epoch = wait_host(&host, |m| m.connected && m.program_resume.contains_key(B1)).await;
    let watch = |epoch| {
        host.commands
            .send(HostCommand::WatchProgramResume {
                epoch,
                session: Some(SESSION.into()),
            })
            .unwrap();
    };
    let send_fence = || {
        host.commands
            .send(HostCommand::Request {
                method: "test.fence".into(),
                payload: json!({}),
            })
            .unwrap();
    };
    watch(epoch + 1);
    send_fence();
    tokio::time::timeout(Duration::from_secs(10), fence.recv_async())
        .await
        .unwrap()
        .unwrap();
    watch(epoch);
    tokio::time::timeout(Duration::from_secs(10), sent_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(
        host.model
            .lock()
            .unwrap()
            .program_resume_reading
            .contains(B1)
    );
    host.commands.send(HostCommand::Reconnect).unwrap();
    wait_host(&host, |m| m.connected && m.connection_epoch > epoch).await;
    watch(epoch);
    send_fence();
    tokio::time::timeout(Duration::from_secs(10), fence.recv_async())
        .await
        .unwrap()
        .unwrap();
    {
        let model = host.model.lock().unwrap();
        assert!(model.program_resume.is_empty());
        assert!(model.program_resume_reading.is_empty());
        assert!(!model.program_resume_unsupported);
    }
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn host_budget_applies_across_blocks() {
    use crate::program_resume::{MAX_RECORDS_PER_BLOCK, MAX_RECORDS_PER_HOST};
    let count = MAX_RECORDS_PER_HOST / MAX_RECORDS_PER_BLOCK + 1;
    let ids: Vec<String> = (0..count).map(|n| format!("block:{n}")).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut mock = Mock::new(24, &refs).await;
    for (n, block) in refs.iter().enumerate() {
        let row = snapshot("demo")["records"][0].clone();
        program_resume::apply_event(
            &mock.shared,
            &mut mock.state,
            SESSION,
            block,
            n as u64 + 1,
            &json!({"records": vec![row; MAX_RECORDS_PER_BLOCK]}),
        );
    }
    let model = mock.model();
    assert_eq!(
        model
            .program_resume
            .values()
            .map(|s| s.records.len())
            .sum::<usize>(),
        MAX_RECORDS_PER_HOST
    );
    let overflow = &model.program_resume[refs[count - 1]];
    assert!(overflow.truncated && overflow.records.is_empty());
}
