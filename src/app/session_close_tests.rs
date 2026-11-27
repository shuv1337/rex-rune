//! Empty-session close behavior through the headless workspace and the real card.
use super::*;
use rex_rune::connection::{
    HumanClose,
    actions::Queued,
    session_close::{Blocks, Reply},
};
use rexrs::api::session::ViewResponse;
use serde_json::json;

const EPOCH: u64 = 7;

fn fixture(view: Option<ViewResponse>) -> (Workspace, flume::Receiver<HostCommand>) {
    let mut ws = workspace();
    let commands = capture_host(&mut ws, 0);
    let model = &mut ws.hosts[0].model;
    model.connected = true;
    model.connection_epoch = EPOCH;
    model.sessions = vec![session("s")];
    model.selected = Some("s".into());
    model.views.extend(view.map(|view| ("s".into(), view)));
    *ws.hosts[0].connection.model.lock().unwrap() = ws.hosts[0].model.clone();
    (ws, commands)
}

fn expect_check(commands: &flume::Receiver<HostCommand>) {
    assert!(matches!(
        commands.try_recv(),
        Ok(HostCommand::CheckEmptySession { epoch: EPOCH, session, .. }) if session == "s"
    ));
    assert!(commands.is_empty());
}

fn start(ws: &mut Workspace, commands: &flume::Receiver<HostCommand>) {
    ws.close_session_on_screen(&mut rune::Tx::default());
    expect_check(commands);
    assert!(ws.panel.is_none(), "wait for the block inventory first");
}

fn deliver(ws: &mut Workspace, blocks: Blocks) {
    ws.hosts[0]
        .connection
        .model
        .lock()
        .unwrap()
        .session_close_checks
        .push(Reply {
            epoch: EPOCH,
            session: "s".into(),
            blocks,
        });
    ws.sync(0, &mut rune::Tx::default());
}

fn tab_view() -> ViewResponse {
    serde_json::from_value(json!({"windows":[{"window_id":"new"}], "revision":2})).unwrap()
}

fn description(ws: &Workspace) -> &str {
    let panel = ws.panel.as_ref().expect("a confirmation");
    assert_eq!(panel.title, "Close s-label?");
    let PanelKind::Confirm {
        method,
        payload,
        description,
    } = &panel.kind
    else {
        panic!("expected a close confirmation");
    };
    assert_eq!(method, "session.destroy");
    assert_eq!(payload, &json!({"session_id":"s"}));
    assert!(description.contains("test-0"));
    assert!(!description.contains("0 tabs"));
    description
}

#[test]
fn verified_empty_session_closes_once_without_confirmation() {
    let (mut ws, commands) = fixture(Some(ViewResponse::default()));
    start(&mut ws, &commands);
    ws.close_session_on_screen(&mut rune::Tx::default());
    assert!(commands.is_empty(), "do not check the same session again");
    deliver(&mut ws, Blocks::Empty);
    assert!(ws.panel.is_none());
    assert!(matches!(
        commands.try_recv(),
        Ok(HostCommand::Close { kind: HumanClose::Session, payload, epoch: Some(EPOCH) })
            if payload == json!({"session_id":"s"})
    ));
    deliver(&mut ws, Blocks::Empty);
    assert!(commands.is_empty(), "a reply is consumed only once");
}

#[test]
fn detached_blocks_and_unavailable_inventories_ask_to_confirm_without_a_tab_count() {
    for (blocks, expected) in [
        (Blocks::Present, "still has background programs"),
        (Blocks::Unknown, "Ends every program in this session"),
    ] {
        let (mut ws, commands) = fixture(Some(ViewResponse::default()));
        start(&mut ws, &commands);
        deliver(&mut ws, blocks);
        assert!(description(&ws).contains(expected));
        assert!(commands.is_empty(), "no close until the person confirms");
        ws.submit(&mut rune::Tx::default());
        assert!(matches!(
            commands.try_recv(),
            Ok(HostCommand::Close { kind: HumanClose::Session, payload, .. })
                if payload == json!({"session_id":"s"})
        ));
    }
}

#[test]
fn unopened_and_nonempty_sessions_ask_immediately_without_a_block_check() {
    for (view, expected) in [
        (None, "Ends every program in this session"),
        (Some(tab_view()), "Ends every program in its 1 tab on"),
    ] {
        let (mut ws, commands) = fixture(view);
        ws.close_session_on_screen(&mut rune::Tx::default());
        assert!(description(&ws).contains(expected));
        assert!(commands.is_empty(), "no wire request before confirmation");
    }
}

#[test]
fn a_new_tab_while_the_inventory_is_pending_restores_tab_count_confirmation() {
    let (mut ws, commands) = fixture(Some(ViewResponse::default()));
    start(&mut ws, &commands);
    ws.hosts[0]
        .connection
        .model
        .lock()
        .unwrap()
        .views
        .insert("s".into(), tab_view());
    deliver(&mut ws, Blocks::Empty);
    assert!(description(&ws).contains("Ends every program in its 1 tab on"));
    assert!(commands.is_empty());
}

#[test]
fn a_disappeared_session_or_changed_connection_discards_the_result() {
    let changes: [fn(&mut rex_rune::connection::HostModel); 3] = [
        |model| model.sessions.clear(),
        |model| model.connected = false,
        |model| model.connection_epoch += 1,
    ];
    for change in changes {
        for blocks in [Blocks::Empty, Blocks::Present] {
            let (mut ws, commands) = fixture(Some(ViewResponse::default()));
            start(&mut ws, &commands);
            change(&mut ws.hosts[0].connection.model.lock().unwrap());
            deliver(&mut ws, blocks);
            assert!(ws.panel.is_none());
            assert!(commands.is_empty());
        }
    }
}

#[test]
fn an_explicit_session_not_found_discards_the_check_before_the_list_catches_up() {
    let (mut ws, commands) = fixture(Some(ViewResponse::default()));
    start(&mut ws, &commands);
    deliver(&mut ws, Blocks::SessionGone);
    assert!(ws.panel.is_none());
    assert!(commands.is_empty());
}

#[test]
fn a_returned_client_action_cannot_request_an_unconfirmed_close() {
    let (mut ws, commands) = fixture(Some(ViewResponse::default()));
    ws.hosts[0]
        .connection
        .model
        .lock()
        .unwrap()
        .client_actions
        .push(Queued {
            generation: ws.keymap_generation,
            session: Some("s".into()),
            block: None,
            action: rexrs::api::client::ClientAction {
                action: "session.close".into(),
                args: None,
            },
        });
    ws.sync(0, &mut rune::Tx::default());
    assert!(ws.panel.is_none());
    assert!(commands.is_empty());
}

#[test]
fn empty_card_offers_both_actions_and_close_uses_the_same_check() {
    let (mut ws, commands) = fixture(Some(ViewResponse::default()));
    ws.viewport = [640., 480.];
    ws.sidebar.toggle();
    let mut rt = runtime_at(ws, (640., 480.));
    let elements = rt.elements();
    let rect = |id| {
        elements
            .iter()
            .find(|e| e.key.as_deref() == Some(id))
            .unwrap()
            .rect
    };
    for label in ["New tab", "Close session"] {
        assert!(elements.iter().any(|e| e.text.as_deref() == Some(label)));
    }
    let new = rect("empty-action");
    let close = rect("empty-close-session");
    if let Some(mut camera) = super::pane_verb_tests::Camera::new() {
        camera.snap(&mut rt, "empty-session-minimum");
    }
    assert_eq!(new[1], close[1]);
    assert!(new[0] + new[2] < close[0]);
    assert!(close[0] + close[2] < 640., "Close session rect: {close:?}");
    click(&mut rt, "empty-close-session");
    expect_check(&commands);
    assert!(rt.state.panel.is_none());
}
