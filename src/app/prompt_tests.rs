//! Headless UI tests and real-font visual captures for `rex.ui` prompts (#144 slice 2).
use super::*;
use crate::app::{
    panel::{PanelKind, SettingsSection},
    prompts::{self, CANCEL_COOLDOWN, IDLE_DELAY, MAX_INPUT_CHARS, SUBMIT_GUARD},
};
use rex_rune::{
    connection::{
        Command, HostCommand,
        perform::{
            Outcome as PerformOutcome, Pending as PerformPending, Request as PerformRequest,
            Requester,
        },
        prompt::{
            EndReason, Prompt, PromptAnswer, PromptEvent, PromptEventKind, PromptIcon, PromptItem,
            PromptKind, ReplyOutcome,
        },
    },
    settings::{ChromeTheme, interface::ToolbarBehavior},
};
use rune::{Env, Ime, Key, KeyEvent, Mods, PhysicalInput, PhysicalKeyCode as Code, Runtime, Tx};
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

fn input_prompt(call_id: &str, title: &str) -> Arc<Prompt> {
    Arc::new(Prompt {
        call_id: call_id.into(),
        action: Some("deploy_release".into()),
        title: title.into(),
        placeholder: Some("v1.2.3".into()),
        value: Some("v1.0.0".into()),
        confirm: Some("Deploy".into()),
        kind: PromptKind::Input,
        timeout_ms: 60_000,
        deadline: Instant::now() + Duration::from_secs(60),
    })
}

fn pick_item(
    id: &str,
    title: &str,
    subtitle: Option<&str>,
    icon: Option<PromptIcon>,
    detail: Option<&str>,
    selected: bool,
) -> PromptItem {
    PromptItem {
        id: id.into(),
        title: title.into(),
        subtitle: subtitle.map(str::to_owned),
        icon,
        detail: detail.map(str::to_owned),
        selected,
    }
}

fn pick_prompt(
    call_id: &str,
    title: &str,
    multiple: bool,
    allow_other: bool,
    items: Vec<PromptItem>,
) -> Arc<Prompt> {
    Arc::new(Prompt {
        call_id: call_id.into(),
        action: Some("choose_target".into()),
        title: title.into(),
        placeholder: Some("Filter environments…".into()),
        value: None,
        confirm: None,
        kind: PromptKind::Pick {
            multiple,
            allow_other,
            items,
        },
        timeout_ms: 60_000,
        deadline: Instant::now() + Duration::from_secs(60),
    })
}

fn connected_ws(count: usize) -> (Workspace, Vec<flume::Receiver<HostCommand>>) {
    let mut ws = workspace_hosts(count);
    let mut rxs = Vec::new();
    for i in 0..count {
        let rx = capture_host(&mut ws, i);
        {
            let mut m = ws.hosts[i].connection.model.lock().unwrap();
            m.connected = true;
            m.connection_epoch = 1;
            m.sessions = vec![session("s")];
            m.selected = Some("s".into());
        }
        ws.hosts[i].model.connected = true;
        ws.hosts[i].model.connection_epoch = 1;
        rxs.push(rx);
    }
    (ws, rxs)
}

fn push_prompt_event(ws: &mut Workspace, host: usize, kind: PromptEventKind) {
    let epoch = ws.hosts[host]
        .connection
        .model
        .lock()
        .unwrap()
        .connection_epoch;
    ws.hosts[host]
        .connection
        .model
        .lock()
        .unwrap()
        .prompt_events
        .push_back(PromptEvent { epoch, kind });
    let mut tx = Tx::default();
    ws.sync(host, &mut tx);
}

fn push_prompt_event_at(ws: &mut Workspace, host: usize, kind: PromptEventKind, now: Instant) {
    let mut tx = Tx::default();
    ws.sync(host, &mut tx);
    let epoch = ws.hosts[host].model.connection_epoch;
    ws.process_prompt_events_at(
        host,
        VecDeque::from([PromptEvent { epoch, kind }]),
        now,
        &mut tx,
    );
}

fn prompt_replies(rx: &flume::Receiver<HostCommand>) -> Vec<(u64, String, PromptAnswer)> {
    rx.try_iter()
        .filter_map(|cmd| match cmd {
            HostCommand::PromptReply {
                epoch,
                call_id,
                answer,
            } => Some((epoch, call_id, answer)),
            _ => None,
        })
        .collect()
}

fn prompts_toggles(rx: &flume::Receiver<HostCommand>) -> Vec<bool> {
    rx.try_iter()
        .filter_map(|cmd| match cmd {
            HostCommand::Prompts(enabled) => Some(enabled),
            _ => None,
        })
        .collect()
}

fn make_submit_ready(ws: &mut Workspace) {
    if let Some(active) = ws.prompts.open_prompt_mut() {
        active.opened_at = Some(Instant::now() - SUBMIT_GUARD - Duration::from_millis(20));
    }
}

#[test]
fn never_opens_over_local_panels_demotion_preserves_draft_and_settings_returns_via_keep_back() {
    let (mut ws, rxs) = connected_ws(1);
    let rx = &rxs[0];

    // 1. Never opens over HostKey, InstallSsh, Confirm, or Settings.
    let blocking_panels = [
        PanelKind::HostKey {
            endpoint: "ssh://user@host".into(),
        },
        PanelKind::InstallSsh {
            endpoint: "ssh://user@host".into(),
        },
        PanelKind::Confirm {
            method: "block.close".into(),
            payload: serde_json::json!({}),
            description: "Close pane?".into(),
        },
        PanelKind::Settings,
    ];
    for (idx, kind) in blocking_panels.into_iter().enumerate() {
        ws.open_panel(kind, "Local panel", &[]);
        push_prompt_event(
            &mut ws,
            0,
            PromptEventKind::Requested(input_prompt(&format!("call-{idx}"), "Script question")),
        );
        assert!(
            !ws.is_prompt_open(),
            "prompt must wait as a chip instead of clobbering panel {idx}"
        );
        assert!(ws.has_waiting_prompt());
        assert!(
            !matches!(
                ws.panel.as_ref().map(|p| &p.kind),
                Some(PanelKind::Prompt(_))
            ),
            "existing local panel must remain mounted"
        );
        // Clear between cases via Ended.
        push_prompt_event(
            &mut ws,
            0,
            PromptEventKind::Ended {
                call_id: format!("call-{idx}"),
                reason: EndReason::Cancelled,
            },
        );
    }

    // 2. Open a waiting prompt from Settings: answering returns to Settings (`keep_back`).
    ws.perform(Intent::Settings, &mut Tx::default());
    assert!(matches!(
        ws.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Settings)
    ));
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::Requested(input_prompt("from-settings", "From Settings")),
    );
    assert!(!ws.is_prompt_open());
    assert!(ws.open_oldest_waiting_prompt());
    assert!(ws.is_prompt_open());

    // Edit draft text and demote by opening Commands (a local panel).
    ws.prompt_edit().text = "release-2026-11".into();
    ws.picker(false);
    assert!(
        !ws.is_prompt_open(),
        "opening a local panel demotes the open prompt back to Waiting"
    );
    assert!(ws.has_waiting_prompt());
    assert_eq!(ws.prompts.queue[0].edit.text, "release-2026-11");

    // Return to Settings and reopen the waiting prompt: draft is still intact!
    ws.perform(Intent::Settings, &mut Tx::default());
    assert!(ws.open_oldest_waiting_prompt());
    assert_eq!(ws.prompt_edit().text, "release-2026-11");

    // Submit answer and confirm ReplyOutcome::Accepted returns cleanly to Settings.
    make_submit_ready(&mut ws);
    let mut tx = Tx::default();
    ws.submit_open_prompt(&mut tx, Instant::now(), false);
    let replies = prompt_replies(rx);
    assert_eq!(replies.len(), 1);
    assert!(matches!(&replies[0].2, PromptAnswer::Text(t) if t == "release-2026-11"));

    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::ReplyOutcome {
            call_id: "from-settings".into(),
            outcome: ReplyOutcome::Accepted,
        },
    );
    assert!(!ws.is_prompt_open());
    assert!(
        matches!(
            ws.panel.as_ref().map(|p| &p.kind),
            Some(PanelKind::Settings)
        ),
        "closing a prompt opened from Settings must return to Settings"
    );
}

#[test]
fn idle_rule_own_action_exception_submit_guards_and_cancel_cooldown() {
    let (mut ws, rxs) = connected_ws(1);
    let rx = &rxs[0];
    let base = Instant::now();

    // Recent keypress (< 750 ms) without an in-flight action forces prompt to chip.
    ws.last_keypress = Some(base);
    let t_typing = base + Duration::from_millis(100);
    push_prompt_event_at(
        &mut ws,
        0,
        PromptEventKind::Requested(input_prompt("typing-1", "While typing")),
        t_typing,
    );
    assert!(
        !ws.is_prompt_open(),
        "prompt arriving during active typing must wait as a chip"
    );
    assert!(ws.has_waiting_prompt());

    // Own client.action in flight overrides the 750 ms idle rule and auto-opens immediately.
    ws.hosts[0]
        .connection
        .model
        .lock()
        .unwrap()
        .actions_in_flight = 1;
    ws.last_keypress = Some(t_typing);
    let t_open = t_typing + Duration::from_millis(50);
    push_prompt_event_at(
        &mut ws,
        0,
        PromptEventKind::Requested(input_prompt("own-action", "Own action question")),
        t_open,
    );
    assert!(
        ws.is_prompt_open(),
        "prompt from this client's own in-flight action auto-opens even while typing"
    );

    // Within 300 ms of opening, Enter submit is inert (exact boundary: 299 ms).
    let mut tx = Tx::default();
    ws.submit_open_prompt(&mut tx, t_open + Duration::from_millis(299), false);
    assert!(
        prompt_replies(rx).is_empty(),
        "submit must be inert for 300 ms after opening"
    );

    // Even after 300 ms, Enter key repeat (repeat == true) is ignored.
    let t_ready = t_open + SUBMIT_GUARD;
    ws.submit_open_prompt(&mut tx, t_ready, true);
    assert!(
        prompt_replies(rx).is_empty(),
        "submit must ignore Enter key repeats"
    );

    // Cancelling sends Cancelled and triggers a 10-second per-host cool-down.
    ws.cancel_open_prompt_at(Some(&mut tx), t_ready);
    let replies = prompt_replies(rx);
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].1, "own-action");
    assert!(matches!(replies[0].2, PromptAnswer::Cancelled));
    assert!(!ws.is_prompt_open());

    // Even with idle typing + own action, a new prompt during the 10 s cool-down waits as chip!
    ws.last_keypress = Some(t_ready);
    let t_during_cooldown = t_ready + IDLE_DELAY + Duration::from_secs(2);
    push_prompt_event_at(
        &mut ws,
        0,
        PromptEventKind::Requested(input_prompt("loop-1", "Looping script")),
        t_during_cooldown,
    );
    assert!(
        !ws.is_prompt_open(),
        "post-cancel 10 s cool-down must prevent a buggy Lua loop from trapping the window"
    );
    assert!(ws.has_waiting_prompt());

    // Once the 10 s cool-down expires, new prompts can auto-open again.
    let t_after_cooldown = t_ready + CANCEL_COOLDOWN;
    push_prompt_event_at(
        &mut ws,
        0,
        PromptEventKind::Requested(input_prompt("after-cool", "After cool-down")),
        t_after_cooldown,
    );
    assert!(ws.is_prompt_open());
}

#[test]
fn scrim_click_does_nothing_escape_cancels_and_bad_request_keeps_draft_with_inline_error() {
    let (mut ws, rxs) = connected_ws(1);
    let rx = &rxs[0];
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::Requested(input_prompt("scrim-1", "Confirm tag")),
    );
    assert!(ws.is_prompt_open());
    ws.prompt_edit().text = "my-draft-tag".into();

    let mut rt = runtime_at(ws, (1024., 700.));
    for _ in 0..10 {
        rt.frame(1. / 60.);
    }
    assert!(has(&rt, "dialog"));
    assert!(has(&rt, "prompt-attribution"));
    assert!(has(&rt, "prompt-title"));
    assert!(has(&rt, "prompt-input"));
    assert!(has(&rt, "scrim"));

    // Clicking the dark scrim outside the modal does NOTHING.
    rt.pointer_down(10., 10.);
    rt.pointer_up(10., 10.);
    rt.frame(1. / 60.);
    assert!(
        rt.state.is_prompt_open(),
        "scrim click must not dismiss or cancel a rex.ui prompt"
    );
    assert!(prompt_replies(rx).is_empty());

    // Submit and receive ReplyRejected (bad_request): prompt stays open, error shown inline, draft kept.
    make_submit_ready(&mut rt.state);
    rt.update(|s, tx| s.submit_open_prompt(tx, Instant::now(), false));
    assert_eq!(prompt_replies(rx).len(), 1);
    assert!(rt.state.prompts.open_prompt().unwrap().sending);

    // Duplicate submit while reply is in flight is ignored.
    rt.update(|s, tx| s.submit_open_prompt(tx, Instant::now(), false));
    assert!(prompt_replies(rx).is_empty());

    push_prompt_event(
        &mut rt.state,
        0,
        PromptEventKind::ReplyRejected {
            call_id: "scrim-1".into(),
            message: "Tag must start with v.".into(),
        },
    );
    rt.frame(1. / 60.);
    assert!(rt.state.is_prompt_open());
    assert!(!rt.state.prompts.open_prompt().unwrap().sending);
    assert_eq!(rt.state.prompt_edit().text, "my-draft-tag");
    assert!(has(&rt, "prompt-error"));

    // Escape cancels the prompt cleanly and sends Cancelled.
    press(&mut rt, Key::Escape);
    rt.frame(1. / 60.);
    assert!(!rt.state.is_prompt_open());
    let replies = prompt_replies(rx);
    assert_eq!(replies.len(), 1);
    assert!(matches!(replies[0].2, PromptAnswer::Cancelled));
}

#[test]
fn single_pick_multiple_pick_with_filter_and_allow_other_work_exact_ids_and_order() {
    let (mut ws, rxs) = connected_ws(1);
    let rx = &rxs[0];

    let items = vec![
        pick_item(
            "id/z\x01raw",
            "Zulu host",
            Some("us-west"),
            Some(PromptIcon::Server),
            Some("prod"),
            false,
        ),
        pick_item(
            "id/a",
            "Alpha branch",
            Some("main"),
            Some(PromptIcon::Branch),
            Some("clean"),
            true,
        ),
        pick_item(
            "id/m",
            "Middle db",
            Some("postgres"),
            Some(PromptIcon::Database),
            Some("rw"),
            false,
        ),
    ];

    // 1. Single pick with allow_other.
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::Requested(pick_prompt(
            "p-single",
            "Pick target",
            false,
            true,
            items.clone(),
        )),
    );
    assert!(ws.is_prompt_open());
    make_submit_ready(&mut ws);

    // Filter to "zulu": row 0 is Use "zulu", row 1 is Zulu host.
    ws.prompt_edit().text = "zulu".into();
    ws.prompt_filter_changed();
    let mut tx = Tx::default();
    // Submit row 0 -> PromptAnswer::Text("zulu").
    ws.submit_open_prompt(&mut tx, Instant::now(), false);
    let replies = prompt_replies(rx);
    assert_eq!(replies.len(), 1);
    assert!(matches!(&replies[0].2, PromptAnswer::Text(t) if t == "zulu"));

    // Reject so we can also test selecting the exact opaque ID on row 1.
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::ReplyRejected {
            call_id: "p-single".into(),
            message: "Pick a listed item".into(),
        },
    );
    ws.select_prompt_row(1);
    ws.submit_open_prompt(&mut tx, Instant::now(), false);
    let replies = prompt_replies(rx);
    assert_eq!(replies.len(), 1);
    assert!(matches!(&replies[0].2, PromptAnswer::Ids(ids) if ids == &["id/z\x01raw"]));

    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::ReplyOutcome {
            call_id: "p-single".into(),
            outcome: ReplyOutcome::Accepted,
        },
    );

    // 2. Multiple pick: preselected item, filter preserves checks, Tab/Shift+Tab toggle,
    // submits in original listed order (NOT alphabetical or toggle order), empty allowed.
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::Requested(pick_prompt(
            "p-multi",
            "Choose services",
            true,
            false,
            items,
        )),
    );
    assert!(ws.is_prompt_open());
    // "id/a" started selected=true.
    assert!(ws.prompts.open_prompt().unwrap().checked.contains("id/a"));
    make_submit_ready(&mut ws);

    // Filter to "db" (only Middle db visible), press Tab to check "id/m".
    ws.prompt_edit().text = "db".into();
    ws.prompt_filter_changed();
    ws.prompt_key(
        &KeyEvent {
            key: Key::Tab,
            mods: Mods::default(),
        },
        &mut tx,
    );
    assert!(ws.prompts.open_prompt().unwrap().checked.contains("id/m"));
    assert!(
        ws.prompts.open_prompt().unwrap().checked.contains("id/a"),
        "checked state must survive filtering"
    );

    // Clear filter, check row 0 ("id/z\x01raw") via Tab (moves selection to row 1), then Shift+Tab
    // on row 1 ("id/a") unchecks "id/a" and moves back to row 0.
    ws.prompt_edit().text.clear();
    ws.prompt_filter_changed();
    ws.prompt_key(
        &KeyEvent {
            key: Key::Tab,
            mods: Mods::default(),
        },
        &mut tx,
    );
    assert_eq!(ws.prompts.open_prompt().unwrap().selected, 1);
    ws.prompt_key(
        &KeyEvent {
            key: Key::Tab,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
        },
        &mut tx,
    );
    assert_eq!(ws.prompts.open_prompt().unwrap().selected, 0);

    // Checked items are now "id/z\x01raw" (index 0) and "id/m" (index 2) -> submitted in listed order!
    ws.submit_open_prompt(&mut tx, Instant::now(), false);
    let replies = prompt_replies(rx);
    assert_eq!(replies.len(), 1);
    assert!(
        matches!(&replies[0].2, PromptAnswer::Ids(ids) if ids == &["id/z\x01raw", "id/m"]),
        "multiple pick must submit checked IDs in original listed order"
    );

    // Test empty multiple selection is allowed.
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::ReplyRejected {
            call_id: "p-multi".into(),
            message: "Try again".into(),
        },
    );
    ws.prompts.open_prompt_mut().unwrap().checked.clear();
    ws.submit_open_prompt(&mut tx, Instant::now(), false);
    let replies = prompt_replies(rx);
    assert_eq!(replies.len(), 1);
    assert!(matches!(&replies[0].2, PromptAnswer::Ids(ids) if ids.is_empty()));
}

#[test]
fn zero_pty_leaks_and_remote_performs_refused_while_prompt_is_open() {
    let (mut ws, _host_rx) = tabbed_workspace("single");
    let host_rx = capture_host(&mut ws, 0);
    ws.settings.hosts[0].remote_control = true;
    {
        let mut m = ws.hosts[0].connection.model.lock().unwrap();
        m.connected = true;
        m.connection_epoch = 1;
        m.sessions = ws.hosts[0].model.sessions.clone();
        m.selected = ws.hosts[0].model.selected.clone();
        m.views = ws.hosts[0].model.views.clone();
    }
    ws.hosts[0].model.connected = true;
    ws.hosts[0].model.connection_epoch = 1;
    let (term_tx, term_rx) = flume::unbounded();
    super::pane_verb_tests::framed_terminal(&mut ws, "block:a", &term_tx, b"$ ");
    {
        let key = ws.term_key("block:a");
        let conn = ws.terminals[&key].connection.clone();
        ws.hosts[0]
            .connection
            .model
            .lock()
            .unwrap()
            .terminals
            .insert("block:a".into(), conn);
    }

    // Send an actual pre-modal key press through Rune so `KeyA` sits in `forwarded`.
    let mut rt = runtime_at(ws, (1024., 700.));
    rt.frame(1. / 60.);
    let held_ev = KeyEvent {
        key: Key::Text("a".into()),
        mods: Mods::default(),
    };
    rt.physical_key_input(
        PhysicalInput::Key {
            key: Code::KeyA,
            mods: Mods::default(),
            pressed: true,
            repeat: false,
        },
        held_ev.clone(),
        None,
    );
    rt.frame(1. / 60.);
    assert!(
        !rt.state.forwarded.is_empty(),
        "pre-modal press must be recorded in forwarded before opening prompt"
    );

    // Open prompt (treating the pre-modal press as old enough for auto-open).
    let now = Instant::now();
    rt.state.last_keypress = Some(now - IDLE_DELAY - Duration::from_secs(1));
    push_prompt_event_at(
        &mut rt.state,
        0,
        PromptEventKind::Requested(input_prompt("pty-guard", "No PTY leaks")),
        now,
    );
    assert!(rt.state.is_prompt_open());
    assert!(
        rt.state.forwarded.is_empty(),
        "opening a prompt modal must clear pre-modal forwarded key identities"
    );
    rt.frame(1. / 60.);

    // Drain the pre-modal key press before verifying zero leaks while open and on dismiss.
    let _ = term_rx.try_iter().count();

    // Route the held key's release through Rune while the prompt modal is open, plus
    // ordinary keys, special keys, IME preedit/commit, and direct Term::try_send.
    // Put Enter before Tab so Tab does not focus Cancel and activate it on Enter.
    rt.physical_key_input(
        PhysicalInput::Key {
            key: Code::KeyA,
            mods: Mods::default(),
            pressed: false,
            repeat: false,
        },
        held_ev.clone(),
        None,
    );
    rt.frame(1. / 60.);
    for key in [
        Key::Text("a".into()),
        Key::Text("b".into()),
        Key::Backspace,
        Key::Up,
        Key::Down,
        Key::Enter,
        Key::Tab,
    ] {
        press(&mut rt, key);
    }
    assert!(rt.state.is_prompt_open());
    rt.update(|s, tx| {
        crate::app::input::on_ime(s, &Ime::Preedit("nihon".into(), None), tx);
        crate::app::input::on_ime(s, &Ime::Commit("日本".into()), tx);
        // Defense-in-depth: Term::try_send itself refuses PTY input while prompt is open.
        let key = s.term_key("block:a");
        let term = s.terminals.get_mut(&key).unwrap();
        assert!(!term.try_send(Command::Paste("MUST_NOT_LEAK".into())));
        assert!(!term.try_send(Command::Key(
            serde_json::json!({"key":"key_a","action":"press"})
        )));
    });

    let pty_leak = |c: &HostCommand| {
        matches!(
            c,
            HostCommand::Terminal {
                command: Command::Key(_)
                    | Command::KeyWithPrefix { .. }
                    | Command::Paste(_)
                    | Command::SendKey(_)
                    | Command::Mouse(_)
                    | Command::Wheel(_)
                    | Command::Reset
                    | Command::Clear,
                ..
            }
        )
    };
    let leaked: Vec<_> = term_rx.try_iter().filter(pty_leak).collect();
    assert!(
        leaked.is_empty(),
        "zero PTY bytes may leak while prompt is open"
    );

    // Remote perform (`client.perform`) must be refused while a prompt panel is open.
    rt.update(|s, tx| {
        s.remote_perform(
            0,
            PerformPending {
                epoch: 1,
                received: Instant::now(),
                request: PerformRequest {
                    call_id: "remote-1".into(),
                    action: "pane.zoom".into(),
                    args: serde_json::Value::Null,
                    requester: Requester::default(),
                    session_id: Some("s".into()),
                    block_id: Some("block:a".into()),
                },
            },
            tx,
        );
    });
    let perform_replies: Vec<_> = host_rx
        .try_iter()
        .filter_map(|c| match c {
            HostCommand::PerformReply { outcome, .. } => Some(outcome),
            _ => None,
        })
        .collect();
    assert_eq!(perform_replies.len(), 1);
    assert!(matches!(
        &perform_replies[0],
        PerformOutcome::Error { code, .. } if *code == "refused"
    ));

    // Dismiss the modal and prove a stale repeat or release of the pre-modal held key
    // cannot leak to the PTY after close.
    press(&mut rt, Key::Escape);
    rt.frame(1. / 60.);
    assert!(!rt.state.is_prompt_open());
    rt.physical_key_input(
        PhysicalInput::Key {
            key: Code::KeyA,
            mods: Mods::default(),
            pressed: true,
            repeat: true,
        },
        held_ev.clone(),
        None,
    );
    rt.physical_key_input(
        PhysicalInput::Key {
            key: Code::KeyA,
            mods: Mods::default(),
            pressed: false,
            repeat: false,
        },
        held_ev,
        None,
    );
    rt.frame(1. / 60.);
    let post_close_leaked: Vec<_> = term_rx.try_iter().filter(pty_leak).collect();
    assert!(
        post_close_leaked.is_empty(),
        "stale repeat or release of a pre-modal held key must not leak to PTY (leaked {} commands)",
        post_close_leaked.len()
    );
}

#[test]
fn waiting_chip_fits_640x480_with_live_chips_commands_lists_it_and_settings_switch_persists() {
    let (mut ws, rxs) = connected_ws(2);
    ws.settings.hosts[0].label = "workstation".into();
    ws.settings.hosts[1].label = "buildbox".into();

    // Hold a local panel open so prompts wait as chips.
    ws.picker(false);
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::Requested(input_prompt("w1", "Enter release notes")),
    );
    assert!(!ws.is_prompt_open());
    assert!(ws.has_waiting_prompt());

    // Commands lists "Answer question from workstation…".
    let cmd_labels: Vec<String> = ws.entries().into_iter().map(|e| e.label).collect();
    assert!(
        cmd_labels
            .iter()
            .any(|l| l == "Answer question from workstation…"),
        "Commands must list waiting prompt entry: {cmd_labels:?}"
    );

    // Close Commands so we can inspect the waiting chip at 640x480 alongside live chips.
    ws.close_panel(&mut Tx::default());
    ws.enter_key_mode("resize", false);

    let mut rt = runtime_at(ws, (640., 480.));
    for _ in 0..60 {
        rt.frame(1. / 60.);
    }
    assert!(has(&rt, "prompt-chip"));
    assert!(has(&rt, "mode-chip"));
    assert!(has(&rt, "settings"));
    assert!(has(&rt, "close-app"));

    let chip_rect = super::pane_verb_tests::rect(&rt, "prompt-chip");
    let mode_rect = super::pane_verb_tests::rect(&rt, "mode-chip");
    let id_rect = super::pane_verb_tests::rect(&rt, "identity");
    let settings_rect = super::pane_verb_tests::rect(&rt, "settings");
    let close_rect = super::pane_verb_tests::rect(&rt, "close-app");
    assert!(chip_rect[2] >= 48. && chip_rect[3] == 28.);
    assert!(
        close_rect[0] + close_rect[2] <= 640.5,
        "Close button must stay inside 640 pt window with prompt chip live: id={id_rect:?} mode={mode_rect:?} chip={chip_rect:?} settings={settings_rect:?} close={close_rect:?} viewport={:?}",
        rt.state.viewport
    );
    assert!(
        chip_rect[0] + chip_rect[2] <= settings_rect[0] + 0.5,
        "prompt chip must not overlap Settings button at 640x480"
    );

    // Second host waiting switches chip label to "2 questions".
    rt.update(|s, tx| {
        s.picker(false);
        push_prompt_event(
            s,
            1,
            PromptEventKind::Requested(input_prompt("w2", "Second host question")),
        );
        s.close_panel(tx);
    });
    rt.frame(1. / 60.);
    let chip_text = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some("prompt-chip-label"))
        .and_then(|e| e.text)
        .unwrap_or_default();
    assert_eq!(chip_text, "2 questions");

    // Hidden toolbar still floats the waiting prompt chip.
    rt.update(|s, _| s.settings.interface.toolbar = ToolbarBehavior::Hidden);
    rt.frame(1. / 60.);
    assert!(has(&rt, "prompt-chip"));

    // Settings -> Remote Hosts has searchable "Allow script prompts" switch that persists & refuses live prompts.
    rt.update(|s, tx| {
        s.settings.interface.toolbar = ToolbarBehavior::Always;
        s.perform(Intent::Settings, tx);
        s.open_section(SettingsSection::Hosts);
    });
    rt.frame(1. / 60.);
    assert!(has(&rt, "host-prompts:0"));

    // Search Settings for "script prompts" finds the row on Remote Hosts.
    rt.state.settings_nav.search = rune::TextEdit::new("script prompts");
    rt.state.settings_search_changed();
    assert!(
        rt.state
            .settings_nav
            .hits()
            .iter()
            .any(|h| h.label == "Allow script prompts" && h.page == SettingsSection::Hosts),
        "Settings search must index Allow script prompts"
    );

    // Toggling off sends HostCommand::Prompts(false), persists, and clears host 0's waiting prompt.
    rt.update(|s, tx| s.toggle_host_prompts(0, Some(tx)));
    assert!(!rt.state.settings.hosts[0].prompts);
    assert_eq!(prompts_toggles(&rxs[0]), vec![false]);
    assert!(
        !rt.state
            .prompts
            .queue
            .iter()
            .any(|p| p.host == rt.state.hosts[0].id),
        "disabling prompts clears local waiting/open prompt state for that host"
    );
    let reloaded = rex_rune::settings::Settings::load(&rt.state.settings_path).unwrap();
    assert!(!reloaded.hosts[0].prompts);
    assert!(reloaded.hosts[1].prompts);
}

#[test]
fn lifecycle_notices_char_limit_and_disconnect_or_remove_send_nothing() {
    let (mut ws, rxs) = connected_ws(2);
    ws.settings.hosts[0].label = "devbox".into();
    ws.settings.hosts[1].label = "prod".into();

    // 1. Answered on another device.
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::Requested(input_prompt("n1", "Q1")),
    );
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::Ended {
            call_id: "n1".into(),
            reason: EndReason::Answered,
        },
    );
    assert!(!ws.is_prompt_open());
    assert!(
        ws.notices
            .iter()
            .any(|n| n.text.contains("answered on another device"))
    );

    // 2. Timeout.
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::Requested(input_prompt("n2", "Q2")),
    );
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::Ended {
            call_id: "n2".into(),
            reason: EndReason::Timeout,
        },
    );
    assert!(ws.notices.iter().any(|n| n.text.contains("timed out")));

    // 3. Dropped (too large to show).
    push_prompt_event(&mut ws, 0, PromptEventKind::Dropped);
    assert!(
        ws.notices
            .iter()
            .any(|n| n.text.contains("too large to show"))
    );

    // 4. ReplyOutcome::NotFound and ReplyOutcome::Failed (sticky).
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::Requested(input_prompt("n3", "Q3")),
    );
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::ReplyOutcome {
            call_id: "n3".into(),
            outcome: ReplyOutcome::NotFound,
        },
    );
    assert!(
        ws.notices
            .iter()
            .any(|n| n.text.contains("before your answer was used"))
    );

    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::Requested(input_prompt("n4", "Q4")),
    );
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::ReplyOutcome {
            call_id: "n4".into(),
            outcome: ReplyOutcome::Failed,
        },
    );
    assert!(
        ws.notices
            .iter()
            .any(|n| n.sticky && n.text.contains("not confirmed")),
        "unconfirmed reply must raise a sticky notice"
    );

    // 5. 4096-char cap enforced on edit.
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::Requested(input_prompt("n5", "Q5")),
    );
    ws.prompt_edit().text = "x".repeat(MAX_INPUT_CHARS + 50);
    ws.prompt_input_changed();
    assert_eq!(ws.prompt_edit().text.chars().count(), MAX_INPUT_CHARS);
    assert!(ws.prompts.open_prompt().unwrap().char_limit_hit);

    // 6. Disconnect clears open prompt with open-specific notice and sends NOTHING.
    ws.notices.clear();
    ws.hosts[0].connection.model.lock().unwrap().connected = false;
    ws.sync(0, &mut Tx::default());
    assert!(!ws.is_prompt_open());
    assert!(prompt_replies(&rxs[0]).is_empty());
    assert!(
        ws.notices.iter().any(|n| !n.sticky
            && n.text
                .contains("disconnected while a question was open. Nothing was sent.")),
        "open-prompt disconnect must report question was open and nothing sent: {:?}",
        ws.notices.iter().map(|n| &n.text).collect::<Vec<_>>()
    );

    // 7. Removing a host clears its prompt and sends NOTHING.
    push_prompt_event(
        &mut ws,
        1,
        PromptEventKind::Requested(input_prompt("n6", "Q6")),
    );
    assert!(ws.is_prompt_open());
    ws.remove_host(1, &mut Tx::default());
    assert!(!ws.is_prompt_open());
    assert!(prompt_replies(&rxs[1]).is_empty());
}

#[test]
fn dialog_accessible_name_is_client_owned_even_with_hostile_script_title() {
    let (mut ws, _rxs) = connected_ws(1);
    ws.settings.hosts[0].label = "workstation".into();
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::Requested(input_prompt(
            "hostile-a11y",
            "System Password Required: Enter sudo password",
        )),
    );
    let mut rt = runtime_at(ws, (1024., 700.));
    for _ in 0..10 {
        rt.frame(1. / 60.);
    }
    let dialog_label = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some("dialog"))
        .and_then(|e| e.label);
    assert_eq!(
        dialog_label.as_deref(),
        Some("Question from workstation"),
        "dialog accessible label must be client-owned, never controlled by script title"
    );
    let prompt_title = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some("prompt-title"))
        .and_then(|e| e.text);
    assert_eq!(
        prompt_title.as_deref(),
        Some("System Password Required: Enter sudo password")
    );
}

#[test]
fn bootstrap_prompt_event_before_connected_is_preserved_until_connected() {
    let (mut ws, _rxs) = connected_ws(1);
    {
        let mut m = ws.hosts[0].connection.model.lock().unwrap();
        m.connected = false;
        m.connection_epoch = 2;
        // Stale epoch-1 event must be discarded while disconnected.
        m.prompt_events.push_back(PromptEvent {
            epoch: 1,
            kind: PromptEventKind::Requested(input_prompt("stale-epoch", "Stale")),
        });
        // Current epoch-2 event published during bootstrap before `connected` becomes true.
        m.prompt_events.push_back(PromptEvent {
            epoch: 2,
            kind: PromptEventKind::Requested(input_prompt("boot-2", "Bootstrap offer")),
        });
    }
    // Intervening UI wake while still disconnected must NOT drain or lose the epoch-2 offer.
    ws.sync(0, &mut Tx::default());
    assert!(!ws.is_prompt_open());
    assert!(!ws.has_waiting_prompt());
    {
        let m = ws.hosts[0].connection.model.lock().unwrap();
        assert_eq!(m.prompt_events.len(), 1);
        assert_eq!(m.prompt_events[0].epoch, 2);
    }

    // Once bootstrap finishes and `connected` flips to true, the buffered offer opens cleanly.
    ws.hosts[0].connection.model.lock().unwrap().connected = true;
    ws.sync(0, &mut Tx::default());
    assert!(ws.is_prompt_open());
    assert_eq!(ws.prompts.open_prompt().unwrap().prompt.call_id, "boot-2");
}

#[test]
fn sending_prompt_ignores_cancel_reopens_on_reject_and_distinguishes_disconnect_wording() {
    let (mut ws, rxs) = connected_ws(1);
    let rx = &rxs[0];
    ws.settings.hosts[0].label = "buildbox".into();

    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::Requested(input_prompt("inflight-1", "Confirm version")),
    );
    assert!(ws.is_prompt_open());
    ws.prompt_edit().text = "v2.3.4".into();
    make_submit_ready(&mut ws);

    let mut rt = runtime_at(ws, (1024., 700.));
    for _ in 0..10 {
        rt.frame(1. / 60.);
    }
    rt.update(|s, tx| s.submit_open_prompt(tx, Instant::now(), false));
    assert_eq!(prompt_replies(rx).len(), 1);
    assert!(rt.state.prompts.open_prompt().unwrap().sending);
    rt.frame(1. / 60.);

    // While sending == true, Escape and Cancel must be ignored so the entry survives settlement.
    press(&mut rt, Key::Escape);
    rt.frame(1. / 60.);
    assert!(
        rt.state.is_prompt_open(),
        "Escape while answer is in flight must keep the entry open"
    );
    assert!(prompt_replies(rx).is_empty());

    // Even if a local panel demotes the sending prompt, ReplyRejected reopens with draft intact
    // once no local panel is open.
    rt.update(|s, tx| {
        s.picker(false);
        s.close_panel(tx);
    });
    assert!(!rt.state.is_prompt_open());
    push_prompt_event(
        &mut rt.state,
        0,
        PromptEventKind::ReplyRejected {
            call_id: "inflight-1".into(),
            message: "Tag already exists.".into(),
        },
    );
    assert!(
        rt.state.is_prompt_open(),
        "ReplyRejected must reopen a demoted sending prompt when no panel is open"
    );
    assert!(!rt.state.prompts.open_prompt().unwrap().sending);
    assert_eq!(rt.state.prompt_edit().text, "v2.3.4");
    assert_eq!(
        rt.state.prompts.open_prompt().unwrap().error.as_deref(),
        Some("Tag already exists.")
    );

    // Re-submit and disconnect while sending == true: sticky unconfirmed notice, NOT "Nothing was sent".
    make_submit_ready(&mut rt.state);
    rt.update(|s, tx| s.submit_open_prompt(tx, Instant::now(), false));
    assert_eq!(prompt_replies(rx).len(), 1);
    rt.state.notices.clear();
    rt.state.hosts[0].connection.model.lock().unwrap().connected = false;
    rt.update(|s, tx| s.sync(0, tx));
    assert!(!rt.state.is_prompt_open());
    assert!(
        rt.state.notices.iter().any(|n| n.sticky
            && n.text
                .contains("buildbox disconnected before confirming your answer.")),
        "disconnect while sending must raise sticky unconfirmed notice: {:?}",
        rt.state
            .notices
            .iter()
            .map(|n| (&n.text, n.sticky))
            .collect::<Vec<_>>()
    );
    assert!(
        !rt.state
            .notices
            .iter()
            .any(|n| n.text.contains("Nothing was sent")),
        "disconnect while sending must not claim nothing was sent"
    );

    // Reconnect and verify waiting-only disconnect wording ("while a question was waiting").
    {
        let mut m = rt.state.hosts[0].connection.model.lock().unwrap();
        m.connected = true;
        m.connection_epoch = 2;
    }
    rt.update(|s, tx| {
        s.sync(0, tx);
        s.picker(false);
    });
    push_prompt_event(
        &mut rt.state,
        0,
        PromptEventKind::Requested(input_prompt("wait-disc", "Waiting Q")),
    );
    assert!(!rt.state.is_prompt_open());
    assert!(rt.state.has_waiting_prompt());
    rt.state.notices.clear();
    rt.state.hosts[0].connection.model.lock().unwrap().connected = false;
    rt.update(|s, tx| s.sync(0, tx));
    assert!(!rt.state.has_waiting_prompt());
    assert!(
        rt.state.notices.iter().any(|n| !n.sticky
            && n.text
                .contains("disconnected while a question was waiting. Nothing was sent.")),
        "waiting-only disconnect must say 'was waiting': {:?}",
        rt.state.notices.iter().map(|n| &n.text).collect::<Vec<_>>()
    );
}

#[test]
fn reply_rejected_while_typing_stays_waiting_chip_with_draft_and_error() {
    let (mut ws, rxs) = connected_ws(1);
    let rx = &rxs[0];
    let base = Instant::now();

    push_prompt_event_at(
        &mut ws,
        0,
        PromptEventKind::Requested(input_prompt("reject-typing", "Confirm tag")),
        base,
    );
    assert!(ws.is_prompt_open());
    ws.prompt_edit().text = "v2.3.4".into();

    let t_submit = base + SUBMIT_GUARD;
    let mut tx = Tx::default();
    ws.submit_open_prompt(&mut tx, t_submit, false);
    assert_eq!(prompt_replies(rx).len(), 1);
    assert!(ws.prompts.open_prompt().unwrap().sending);

    // Demote via a local panel, return to the terminal, and record a recent keypress (< 750 ms).
    ws.picker(false);
    ws.close_panel(&mut tx);
    assert!(!ws.is_prompt_open());
    let t_key = t_submit + Duration::from_millis(50);
    ws.last_keypress = Some(t_key);

    // ReplyRejected arrives 150 ms after typing with no action in flight: must stay a chip.
    let t_reject = t_key + Duration::from_millis(150);
    push_prompt_event_at(
        &mut ws,
        0,
        PromptEventKind::ReplyRejected {
            call_id: "reject-typing".into(),
            message: "Tag already exists.".into(),
        },
        t_reject,
    );
    assert!(
        !ws.is_prompt_open(),
        "ReplyRejected during active typing must not steal focus"
    );
    assert!(ws.has_waiting_prompt());
    assert!(!ws.prompts.queue[0].sending);
    assert_eq!(ws.prompts.queue[0].edit.text, "v2.3.4");
    assert_eq!(
        ws.prompts.queue[0].error.as_deref(),
        Some("Tag already exists.")
    );

    // Opening the waiting chip reveals the preserved draft and inline error.
    assert!(ws.open_oldest_waiting_prompt());
    assert_eq!(ws.prompt_edit().text, "v2.3.4");
    assert_eq!(
        ws.prompts.open_prompt().unwrap().error.as_deref(),
        Some("Tag already exists.")
    );
}

#[test]
fn pick_modal_with_wrapped_error_fits_within_85_percent_cap_at_640x480() {
    let (mut ws, _rxs) = connected_ws(1);
    ws.settings.hosts[0].label = "workstation".into();
    let items: Vec<PromptItem> = (0..8)
        .map(|i| {
            pick_item(
                &format!("item-{i}"),
                &format!("Cluster node {i}"),
                Some("us-west-2 production replica"),
                Some(PromptIcon::Server),
                Some("healthy"),
                false,
            )
        })
        .collect();
    push_prompt_event(
        &mut ws,
        0,
        PromptEventKind::Requested(pick_prompt(
            "pick-err-640",
            "Choose a deployment target with a longer two-line prompt title",
            false,
            false,
            items,
        )),
    );
    ws.prompts.open_prompt_mut().unwrap().error = Some(
        "The selected deployment target is locked by another release pipeline. Pick another replica or retry once the active rollout finishes.".into(),
    );

    let mut rt = runtime_at(ws, (640., 480.));
    for _ in 0..60 {
        rt.frame(1. / 60.);
    }
    let dialog_rect = super::pane_verb_tests::rect(&rt, "dialog");
    let submit_rect = super::pane_verb_tests::rect(&rt, "prompt-submit");
    let cancel_rect = super::pane_verb_tests::rect(&rt, "prompt-cancel");
    assert!(
        dialog_rect[3] <= 480. * 0.85 + 1.,
        "pick modal with wrapped error must stay within 85% cap at 640x480: dialog={dialog_rect:?}"
    );
    assert!(
        submit_rect[1] + submit_rect[3] <= dialog_rect[1] + dialog_rect[3] + 0.5,
        "Submit button must stay inside dialog bounds: dialog={dialog_rect:?} submit={submit_rect:?}"
    );
    assert!(
        cancel_rect[1] + cancel_rect[3] <= dialog_rect[1] + dialog_rect[3] + 0.5,
        "Cancel button must stay inside dialog bounds: dialog={dialog_rect:?} cancel={cancel_rect:?}"
    );
}

#[test]
fn all_16_prompt_icons_plus_generic_are_distinct_and_valid_svg_icons() {
    let all = [
        PromptIcon::Document,
        PromptIcon::Folder,
        PromptIcon::Branch,
        PromptIcon::Terminal,
        PromptIcon::Code,
        PromptIcon::Server,
        PromptIcon::Database,
        PromptIcon::Package,
        PromptIcon::Globe,
        PromptIcon::Link,
        PromptIcon::Person,
        PromptIcon::Tag,
        PromptIcon::Clock,
        PromptIcon::Star,
        PromptIcon::Gear,
        PromptIcon::Plus,
        PromptIcon::Generic,
    ];
    let mut names = std::collections::HashSet::new();
    for kind in all {
        let icon = prompts::icon_for(Some(kind));
        assert_eq!(icon.view, 20.0);
        assert!(!icon.body.is_empty());
        assert!(names.insert(icon.name), "duplicate icon name {}", icon.name);
    }
    assert_eq!(
        prompts::icon_for(None).name,
        prompts::icon_for(Some(PromptIcon::Generic)).name
    );
}

// ---------------------------------------------------------------------------
// Real-font visual captures (run individually with REX_TEST_SNAP_DIR).
// Set REX_M144_MINIMUM=1 for 640x480 captures.
// ---------------------------------------------------------------------------

fn capture_size() -> ((f32, f32), &'static str) {
    if std::env::var("REX_M144_MINIMUM").as_deref() == Ok("1") {
        ((640., 480.), "640")
    } else {
        ((1024., 700.), "1024")
    }
}

fn capture_scene(
    theme: ChromeTheme,
    size: (f32, f32),
    setup: impl FnOnce(&mut Workspace),
) -> Runtime<Workspace> {
    let (mut ws, _host_rx) = tabbed_workspace("single");
    ws.settings.chrome_theme = theme;
    ws.settings.hosts[0].label = "workstation".into();
    ws.settings.hosts[0].endpoint = "ssh://dev@workstation.ts.net".into();
    ws.hosts[0].connection.model.lock().unwrap().connected = true;
    ws.hosts[0]
        .connection
        .model
        .lock()
        .unwrap()
        .connection_epoch = 1;
    ws.hosts[0].model.connected = true;
    ws.hosts[0].model.connection_epoch = 1;
    let (sender, _sent) = flume::unbounded();
    super::pane_verb_tests::framed_terminal(
        &mut ws,
        "block:a",
        &sender,
        b"$ rex do deploy_release\r\nWaiting for confirmation on connected clients...",
    );
    setup(&mut ws);
    let mut rt = Runtime::new(ws, view, size, Env::new(true, 2.));
    rt.on_key = Some(crate::app::input::on_key);
    for _ in 0..24 {
        rt.frame(1. / 60.);
    }
    rt
}

#[test]
fn prompt_input_draw() {
    use super::pane_verb_tests::Camera;
    let Some(mut camera) = Camera::new() else {
        return;
    };
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");
    let (size, tag) = capture_size();
    for (theme, suffix) in [
        (ChromeTheme::Midnight, "midnight"),
        (ChromeTheme::GruvboxDark, "gruvbox"),
    ] {
        let mut rt = capture_scene(theme, size, |ws| {
            push_prompt_event(
                ws,
                0,
                PromptEventKind::Requested(input_prompt("cap-in", "Enter release tag to deploy")),
            );
            make_submit_ready(ws);
        });
        camera.snap(&mut rt, &format!("prompt-input-{tag}-{suffix}"));
    }
}

#[test]
fn prompt_pick_draw() {
    use super::pane_verb_tests::Camera;
    let Some(mut camera) = Camera::new() else {
        return;
    };
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");
    let (size, tag) = capture_size();
    let items = vec![
        pick_item(
            "prod",
            "Production cluster",
            Some("us-east-1 · 6 nodes"),
            Some(PromptIcon::Server),
            Some("v2.4.1"),
            false,
        ),
        pick_item(
            "staging",
            "Staging cluster",
            Some("us-west-2 · 2 nodes"),
            Some(PromptIcon::Globe),
            Some("v2.5.0-rc1"),
            false,
        ),
        pick_item(
            "branch",
            "Feature branch",
            Some("feat/rex-ui-prompts"),
            Some(PromptIcon::Branch),
            Some("3 ahead"),
            false,
        ),
        pick_item(
            "db",
            "Analytics warehouse",
            Some("postgres-replica-02"),
            Some(PromptIcon::Database),
            Some("read-only"),
            false,
        ),
        pick_item(
            "pkg",
            "Build artifacts",
            Some("linux-x86_64 release"),
            Some(PromptIcon::Package),
            Some("48 MB"),
            false,
        ),
    ];
    for (theme, suffix) in [
        (ChromeTheme::Midnight, "midnight"),
        (ChromeTheme::GruvboxDark, "gruvbox"),
    ] {
        let items = items.clone();
        let mut rt = capture_scene(theme, size, move |ws| {
            push_prompt_event(
                ws,
                0,
                PromptEventKind::Requested(pick_prompt(
                    "cap-pick",
                    "Choose deployment target",
                    false,
                    true,
                    items,
                )),
            );
            make_submit_ready(ws);
        });
        camera.snap(&mut rt, &format!("prompt-pick-{tag}-{suffix}"));
    }
}

#[test]
fn prompt_pick_multiple_draw() {
    use super::pane_verb_tests::Camera;
    let Some(mut camera) = Camera::new() else {
        return;
    };
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");
    let (size, tag) = capture_size();
    let items = vec![
        pick_item(
            "api",
            "API gateway",
            Some("Primary HTTP edge"),
            Some(PromptIcon::Code),
            Some("healthy"),
            true,
        ),
        pick_item(
            "worker",
            "Background worker",
            Some("Async job runner"),
            Some(PromptIcon::Terminal),
            Some("idle"),
            false,
        ),
        pick_item(
            "docs",
            "Docs publisher",
            Some("Static site pipeline"),
            Some(PromptIcon::Document),
            Some("ready"),
            true,
        ),
        pick_item(
            "cron",
            "Scheduled tasks",
            Some("Nightly maintenance"),
            Some(PromptIcon::Clock),
            Some("paused"),
            false,
        ),
        pick_item(
            "tags",
            "Release tags",
            Some("Signed git tags"),
            Some(PromptIcon::Tag),
            Some("v0.12.0"),
            false,
        ),
    ];
    for (theme, suffix) in [
        (ChromeTheme::Midnight, "midnight"),
        (ChromeTheme::GruvboxDark, "gruvbox"),
    ] {
        let items = items.clone();
        let mut rt = capture_scene(theme, size, move |ws| {
            push_prompt_event(
                ws,
                0,
                PromptEventKind::Requested(pick_prompt(
                    "cap-multi",
                    "Select services to restart",
                    true,
                    false,
                    items,
                )),
            );
            make_submit_ready(ws);
        });
        camera.snap(&mut rt, &format!("prompt-pick-multiple-{tag}-{suffix}"));
    }
}

#[test]
fn prompt_chip_draw() {
    use super::pane_verb_tests::Camera;
    let Some(mut camera) = Camera::new() else {
        return;
    };
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");
    let (size, tag) = capture_size();
    for (theme, suffix) in [
        (ChromeTheme::Midnight, "midnight"),
        (ChromeTheme::GruvboxDark, "gruvbox"),
    ] {
        let mut rt = capture_scene(theme, size, |ws| {
            ws.last_keypress = Some(Instant::now());
            ws.enter_key_mode("resize", false);
            push_prompt_event(
                ws,
                0,
                PromptEventKind::Requested(input_prompt("cap-chip", "Waiting prompt")),
            );
        });
        camera.snap(&mut rt, &format!("prompt-chip-{tag}-{suffix}"));
    }
}

#[test]
fn prompt_hostile_title_draw() {
    use super::pane_verb_tests::Camera;
    let Some(mut camera) = Camera::new() else {
        return;
    };
    let _families = rune::text::with_families("Inter", "IBM Plex Mono");
    let (size, tag) = capture_size();
    for (theme, suffix) in [
        (ChromeTheme::Midnight, "midnight"),
        (ChromeTheme::GruvboxDark, "gruvbox"),
    ] {
        let mut rt = capture_scene(theme, size, |ws| {
            let hostile = Arc::new(Prompt {
                call_id: "cap-hostile".into(),
                action: None,
                title: "System Password Required: Enter sudo passphrase for local admin account immediately to unlock keychain and continue!".into(),
                placeholder: Some("Password".into()),
                value: None,
                confirm: Some("Authorize".into()),
                kind: PromptKind::Input,
                timeout_ms: 60_000,
                deadline: Instant::now() + Duration::from_secs(60),
            });
            push_prompt_event(ws, 0, PromptEventKind::Requested(hostile));
            make_submit_ready(ws);
        });
        camera.snap(&mut rt, &format!("prompt-hostile-{tag}-{suffix}"));
    }
}
