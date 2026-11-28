//! Real Rune pointer dispatch with captured host commands, never a real Rex server.
use super::{pane_verb_tests::framed_terminal, *};
use crate::app::{
    drag::{Cue, Item, Rail, insertion},
    scratch::Folder,
};
use rex_rune::{
    connection::Command,
    download::{Failure, State, Transfer},
};
use rune::{ElementInfo, PointerButton};
use serde_json::{Value, json};

const EPOCH: u64 = 7;

fn workspace_for_drag(sidebar: bool, vertical: bool) -> (Workspace, flume::Receiver<HostCommand>) {
    let (mut tabbed, _) = tabbed_workspace("split");
    let mut s = workspace_hosts(2);
    let rx = capture_host(&mut s, 0);
    s.hosts[0].model = std::mem::take(&mut tabbed.hosts[0].model);
    let model = &mut s.hosts[0].model;
    model.connection_epoch = EPOCH;
    model.protocol.api_revision = Some(22);
    model.sessions.extend([session("second"), session("third")]);
    // These hosts intentionally have a session ID in common.
    s.hosts[1].model.sessions = vec![session("second")];
    s.hosts[1].model.connected = true;
    s.sidebar.set_open(sidebar);
    s.settings.vertical_tabs = vertical;
    (s, rx)
}

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..40 {
        rt.frame(1. / 60.);
    }
}

fn element(rt: &Runtime<Workspace>, key: &str) -> ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| !e.exiting && e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing {key}"))
}

fn at(rt: &Runtime<Workspace>, key: &str, x: f32, y: f32) -> (f32, f32) {
    let [left, top, w, h] = element(rt, key).rect;
    (left + w * x, top + h * y)
}

fn visible_center(rt: &Runtime<Workspace>, key: &str) -> (f32, f32) {
    let [x, y, w, h] = element(rt, key).visible;
    assert!(w > 0. && h > 0., "{key} is clipped");
    (x + w / 2., y + h / 2.)
}

fn frame(rt: &mut Runtime<Workspace>) {
    rt.frame(1. / 60.);
}

fn down(rt: &mut Runtime<Workspace>, p: (f32, f32)) {
    rt.pointer_move(p.0, p.1);
    rt.pointer_down(p.0, p.1);
    frame(rt);
}

fn motion(rt: &mut Runtime<Workspace>, p: (f32, f32)) {
    rt.pointer_move(p.0, p.1);
    frame(rt);
}

fn up(rt: &mut Runtime<Workspace>, p: (f32, f32)) {
    rt.pointer_up(p.0, p.1);
    frame(rt);
}

fn commands(rx: &flume::Receiver<HostCommand>) -> Vec<(u64, &'static str, Value)> {
    rx.try_iter()
        .filter_map(|cmd| match cmd {
            HostCommand::LayoutDrop { epoch, operation } => {
                let (method, payload) = operation.wire();
                Some((epoch, method, payload))
            }
            HostCommand::Navigate { .. } => panic!("drag also navigated"),
            HostCommand::Request { method, .. }
                if matches!(
                    method.as_str(),
                    "session.move"
                        | "session.move_window"
                        | "session.swap_blocks"
                        | "session.transfer_window"
                ) =>
            {
                panic!("drag sent an unscoped {method}")
            }
            _ => None,
        })
        .collect()
}

#[test]
fn slots_follow_midpoints_and_never_ask_for_either_side_of_the_source() {
    let rows = [
        ("a", [10., 20., 30., 28.]),
        ("b", [42., 20., 30., 28.]),
        ("c", [74., 20., 30., 28.]),
        ("d", [106., 20., 30., 28.]),
    ];
    let clip = [8., 10., 130., 48.];
    let slot = |x, y| insertion(&rows, "c", Rail::Toolbar, clip, [x, y], 1., None);
    assert_eq!(
        slot(12., 32.),
        Some(Cue::Insert {
            before: Some("a".into()),
            bar: [9., 22., 2., 24.]
        }),
    );
    assert!(matches!(slot(40., 32.), Some(Cue::Insert { before: Some(b), .. }) if b == "b"));
    assert_eq!(slot(69., 32.), None, "before itself");
    assert_eq!(slot(113., 32.), None, "immediately after itself");
    assert!(matches!(
        slot(132., 32.),
        Some(Cue::Insert { before: None, .. })
    ));
    assert_eq!(slot(18., 49.), None, "off-axis");
    assert_eq!(slot(140., 32.), None, "outside the tab rail");
    assert_eq!(
        insertion(
            &rows,
            "c",
            Rail::Toolbar,
            [15., 20., 80., 28.],
            [10., 30.],
            1.,
            None
        ),
        None,
        "clipped rows do not catch drops",
    );
}

#[test]
fn toolbar_reorders_at_minimum_size_without_selecting_or_moving_pills_locally() {
    let (s, rx) = workspace_for_drag(false, false);
    let mut rt = runtime_at(s, (640., 480.));
    settle(&mut rt);
    let from = visible_center(&rt, "tab:split");
    let to = at(&rt, "tab:single", 0.25, 0.5);
    let before = element(&rt, "tab:split").rect;
    down(&mut rt, from);
    motion(&mut rt, to);
    assert!(
        has(&rt, "drag-insertion"),
        "from={from:?} to={to:?}; tab rail={:?}; source visible={:?}; target visible={:?}; gesture={}, cue={:?}",
        element(&rt, "tabs").rect,
        element(&rt, "tab:split").visible,
        element(&rt, "tab:single").visible,
        rt.state.rearrange.gesture.is_some(),
        rt.state.rearrange.cue
    );
    let bar = element(&rt, "drag-insertion");
    assert_eq!(bar.rect[2], 2.);
    assert!(bar.inert);
    assert_eq!(element(&rt, "tab:split").opacity, 0.55);
    assert_eq!(element(&rt, "tab:split").rect, before);
    assert!(commands(&rx).is_empty(), "no request before release");
    up(&mut rt, to);
    assert_eq!(
        commands(&rx),
        [(
            EPOCH,
            "session.move_window",
            json!({"session_id":"s", "window_id":"split", "before_window_id":"single"})
        )]
    );
    assert_eq!(
        rt.state.model().view().unwrap().windows[0]
            .window_id
            .as_deref(),
        Some("single")
    );
    assert!(!has(&rt, "drag-insertion"));
    assert_eq!(element(&rt, "tab:split").opacity, 1.);
}

#[test]
fn short_tab_clicks_keep_select_rename_middle_close_and_right_menu() {
    let (s, rx) = workspace_for_drag(false, false);
    let mut rt = runtime(s);
    settle(&mut rt);
    let p = at(&rt, "tab:single", 0.5, 0.5);
    down(&mut rt, p);
    let short = (p.0 + 4., p.1 + 4.);
    motion(&mut rt, short);
    assert!(!rt.state.rearrange.dimmed(&Item::Tab(
        Rail::Toolbar,
        rt.state.hosts[0].id,
        "s".into(),
        "single".into(),
    )));
    assert!(rt.state.rearrange.cue.is_none());
    assert!(!has(&rt, "drag-insertion"));
    up(&mut rt, short);
    let sent = rx.try_iter().collect::<Vec<_>>();
    assert!(
        sent.iter()
            .any(|c| matches!(c, HostCommand::Navigate { window: Some(id), .. } if id == "single"))
    );
    assert!(
        !sent
            .iter()
            .any(|c| matches!(c, HostCommand::LayoutDrop { .. }))
    );

    let p = at(&rt, "tab:split", 0.5, 0.5);
    down(&mut rt, p);
    up(&mut rt, p);
    down(&mut rt, p);
    up(&mut rt, p);
    assert!(matches!(rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Form(Form::RenameWindow(id))) if id == "split"));
    press(&mut rt, Key::Escape);
    rt.button_down(p.0, p.1, PointerButton::Middle);
    frame(&mut rt);
    assert!(rt.state.rearrange.gesture.is_none());
    rt.button_up(p.0, p.1, PointerButton::Middle);
    frame(&mut rt);
    assert!(matches!(rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Confirm { method, .. }) if method == "session.close_window"));
    press(&mut rt, Key::Escape);
    rt.button_down(p.0, p.1, PointerButton::Secondary);
    rt.button_up(p.0, p.1, PointerButton::Secondary);
    frame(&mut rt);
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Window)
    ));
}

#[test]
fn sidebar_tabs_reorder_and_transfer_but_never_to_another_hosts_matching_id() {
    for vertical in [false, true] {
        let (s, rx) = workspace_for_drag(true, vertical);
        let mut rt = runtime(s);
        settle(&mut rt);
        let key = if vertical {
            "sidebar-tab:0:s:single"
        } else {
            "tab:single"
        };
        let from = at(&rt, key, 0.4, 0.5);
        if vertical {
            let end = at(&rt, "sidebar-tab:0:s:split", 0.4, 0.85);
            down(&mut rt, from);
            motion(&mut rt, end);
            assert_eq!(element(&rt, "drag-insertion").rect[3], 2.);
            up(&mut rt, end);
            assert_eq!(
                commands(&rx),
                [(
                    EPOCH,
                    "session.move_window",
                    json!({"session_id":"s", "window_id":"single"})
                )]
            );
        }
        let to = at(&rt, "session:0:second", 0.5, 0.5);
        let other = at(&rt, "session:1:second", 0.5, 0.5);
        down(&mut rt, from);
        motion(&mut rt, to);
        assert!(has(&rt, "session-target:0:second"));
        motion(&mut rt, other);
        assert!(!has(&rt, "session-target:0:second"));
        assert!(!has(&rt, "session-target:1:second"));
        up(&mut rt, other);
        assert!(commands(&rx).is_empty());

        down(&mut rt, from);
        motion(&mut rt, to);
        up(&mut rt, to);
        assert_eq!(
            commands(&rx),
            [(
                EPOCH,
                "session.transfer_window",
                json!({"session_id":"s", "window_id":"single", "to_session_id":"second"})
            )]
        );
        assert_eq!(
            rt.state.model().view().unwrap().windows.len(),
            2,
            "wait for Rex"
        );

        rt.update(|s, _| s.hosts[0].model.protocol.api_revision = Some(21));
        down(&mut rt, from);
        motion(&mut rt, to);
        assert!(!has(&rt, "session-target:0:second"));
        up(&mut rt, to);
        assert!(commands(&rx).is_empty(), "transfer needs API 22");
    }
}

#[test]
fn session_drag_reorders_only_its_host_under_zoom_and_no_ops_at_origin() {
    let (mut s, rx) = workspace_for_drag(true, false);
    s.settings.ui_scale = 1.3;
    let mut rt = runtime_at(s, (1200., 800.));
    settle(&mut rt);
    let from = at(&rt, "session:0:second", 0.5, 0.5);
    let first = at(&rt, "session:0:s", 0.5, 0.22);
    let third_top = at(&rt, "session:0:third", 0.5, 0.25);
    let last = at(&rt, "session:0:third", 0.5, 0.8);
    let foreign = at(&rt, "session:1:second", 0.5, 0.8);
    for p in [third_top, from, foreign] {
        down(&mut rt, from);
        motion(&mut rt, first);
        assert!(has(&rt, "drag-insertion"));
        assert_eq!(element(&rt, "session:0:second").opacity, 0.55);
        motion(&mut rt, p);
        assert!(!has(&rt, "drag-insertion"));
        up(&mut rt, p);
        assert!(commands(&rx).is_empty());
    }
    for (p, payload) in [
        (
            first,
            json!({"session_id":"second", "before_session_id":"s"}),
        ),
        (last, json!({"session_id":"second"})),
    ] {
        down(&mut rt, from);
        motion(&mut rt, p);
        up(&mut rt, p);
        assert_eq!(commands(&rx), [(EPOCH, "session.move", payload)]);
    }
    assert_eq!(rt.state.model().sessions[1].session_id, "second");
}

#[test]
fn pane_header_swaps_tiled_panes_but_buttons_and_floating_headers_do_not_arm_it() {
    let (mut s, rx) = workspace_for_drag(false, false);
    let window = &mut s.hosts[0].model.views.get_mut("s").unwrap().windows[1];
    window.layers.push(
        serde_json::from_value(json!({
            "kind":"floating", "layer_id":"float", "bounds":[0.2, 0.45, 0.4, 0.4],
            "layout":{"block_id":"block:f"}, "blocks":[{"block_id":"block:f"}]
        }))
        .unwrap(),
    );
    let mut rt = runtime(s);
    settle(&mut rt);
    let from = at(&rt, "pane-header:block:b", 0.1, 0.5);
    let origin = at(&rt, "pane:block:b", 0.1, 0.7);
    let to = at(&rt, "pane:block:c", 0.8, 0.7);
    let floating = at(&rt, "pane-header:block:f", 0.25, 0.5);
    let zoom = at(&rt, "pane-zoom:block:b", 0.5, 0.5);
    down(&mut rt, zoom);
    motion(&mut rt, to);
    assert!(rt.state.rearrange.gesture.is_none());
    up(&mut rt, to);
    assert!(commands(&rx).is_empty());
    down(&mut rt, zoom);
    assert!(rt.state.rearrange.gesture.is_none());
    up(&mut rt, zoom);
    assert!(
        rx.try_iter()
            .any(|c| matches!(c, HostCommand::Request { method, payload }
        if method == "session.zoom_block" && payload["block_id"] == "block:b"))
    );

    down(&mut rt, floating);
    assert!(rt.state.layer_drag.is_some());
    assert!(rt.state.rearrange.gesture.is_none());
    rt.cancel_pointer();
    frame(&mut rt);

    down(&mut rt, from);
    motion(&mut rt, origin);
    assert!(!has(&rt, "pane-drop-target:block:b"));
    motion(&mut rt, floating);
    assert!(!has(&rt, "pane-drop-target:block:f"));
    assert!(!has(&rt, "pane-drop-target:block:b"));
    motion(&mut rt, to);
    let ring = element(&rt, "pane-drop-target:block:c");
    assert!(ring.inert);
    assert_eq!(ring.fill.a, 0.10);
    assert_eq!(element(&rt, "pane-header:block:b").opacity, 0.85);
    let layout = rt.state.model().window().unwrap().clone();
    up(&mut rt, to);
    assert_eq!(
        commands(&rx),
        [(
            EPOCH,
            "session.swap_blocks",
            json!({"session_id":"s", "block_id":"block:b", "other_block_id":"block:c"})
        )]
    );
    assert_eq!(rt.state.model().window(), Some(&layout));
}

#[test]
fn escape_cancels_and_never_reaches_the_pty_or_rearms_the_drag() {
    let (mut s, rx) = workspace_for_drag(false, false);
    let (send, keys) = flume::unbounded();
    let _terminal = framed_terminal(&mut s, "block:b", &send, b"live terminal");
    let mut rt = runtime(s);
    settle(&mut rt);
    press(&mut rt, Key::Escape);
    assert!(
        keys.try_iter().any(|c| matches!(
            c,
            HostCommand::Terminal {
                command: Command::Key(_),
                ..
            }
        )),
        "the test has PTY input routing"
    );
    let from = at(&rt, "pane-header:block:b", 0.1, 0.5);
    let to = at(&rt, "pane:block:c", 0.7, 0.5);
    down(&mut rt, from);
    motion(&mut rt, to);
    assert!(has(&rt, "pane-drop-target:block:c"));
    press(&mut rt, Key::Escape);
    assert!(rt.state.rearrange.gesture.is_none());
    assert!(!has(&rt, "pane-drop-target:block:c"));
    assert!(!keys.try_iter().any(|c| matches!(
        c,
        HostCommand::Terminal {
            command: Command::Key(_),
            ..
        }
    )));
    motion(&mut rt, (to.0 + 20., to.1));
    up(&mut rt, to);
    assert!(commands(&rx).is_empty());
}

#[test]
fn model_and_ui_changes_cancel_an_armed_drag_before_release() {
    for reason in [
        "epoch",
        "offline",
        "source gone",
        "tab switch",
        "session switch",
        "host switch",
        "modal",
        "focus",
        "copy",
        "pointer",
    ] {
        let (mut s, rx) = workspace_for_drag(false, false);
        let sender = s.hosts[0].connection.commands.clone();
        let _terminal = framed_terminal(&mut s, "block:b", &sender, b"content");
        let mut rt = runtime(s);
        settle(&mut rt);
        let from = at(&rt, "tab:split", 0.5, 0.5);
        let to = at(&rt, "tab:single", 0.25, 0.5);
        down(&mut rt, from);
        motion(&mut rt, to);
        assert!(has(&rt, "drag-insertion"), "{reason}");
        match reason {
            "modal" => rt.update(|s, _| s.picker(false)),
            "host switch" => rt.update(|s, tx| s.activate_host(1, tx)),
            "focus" => rt.window_focus(false),
            "copy" => rt.update(|s, tx| s.enter_copy_mode(tx)),
            "pointer" => rt.cancel_pointer(),
            _ => rt.update(|s, tx| {
                let mut model = s.hosts[0].model.clone();
                match reason {
                    "epoch" => model.connection_epoch += 1,
                    "offline" => model.connected = false,
                    "source gone" => {
                        model.views.get_mut("s").unwrap().windows.pop();
                    }
                    "tab switch" => {
                        model.views.get_mut("s").unwrap().active_window_id = Some("single".into())
                    }
                    "session switch" => model.selected = Some("second".into()),
                    _ => unreachable!(),
                }
                *s.hosts[0].connection.model.lock().unwrap() = model;
                s.sync(0, tx);
            }),
        }
        frame(&mut rt);
        assert!(rt.state.rearrange.gesture.is_none(), "{reason}");
        assert!(!has(&rt, "drag-insertion"), "{reason}");
        motion(&mut rt, (to.0 + 1., to.1));
        up(&mut rt, to);
        assert!(commands(&rx).is_empty(), "{reason}");
    }
}

#[test]
fn saved_downloads_offer_drag_out_at_gesture_start_without_capturing_the_folder_button() {
    let folder = Folder::new("download-drag");
    let path = folder.join("saved.txt");
    let (mut s, _rx) = workspace_for_drag(false, false);
    let host = s.hosts[0].id;
    for (id, state) in [
        (
            1,
            State::Saved {
                path: path.clone(),
                bytes: 5,
                short_of: None,
            },
        ),
        (
            2,
            State::Receiving {
                received: 2,
                hint: 5,
            },
        ),
        (3, State::Failed(Failure::NoAnswer)),
        (4, State::Cancelled),
    ] {
        let mut transfer = Transfer::new(
            id,
            "s".into(),
            "block:b".into(),
            format!("test-{id}.txt"),
            folder.path().to_owned(),
            EPOCH,
        );
        transfer.state = state;
        s.hosts[0].model.downloads.push(transfer);
    }
    s.open_panel(PanelKind::Downloads, "Downloads", &[]);
    let mut rt = runtime(s);
    settle(&mut rt);
    assert!(element(&rt, "download:1").drag_out);
    for id in [2, 3, 4] {
        assert!(!element(&rt, &format!("download:{id}")).drag_out);
    }
    let p = at(&rt, "download-name:1", 0.2, 0.5);
    let to = (p.0 + 30., p.1);
    let starts = |rt: &Runtime<Workspace>| {
        rt.events
            .iter()
            .filter(|e| e.starts_with("drag-out "))
            .count()
    };
    down(&mut rt, p);
    motion(&mut rt, to);
    up(&mut rt, to);
    assert_eq!(
        starts(&rt),
        0,
        "a missing path is checked only when pressed and dragged"
    );
    std::fs::write(&path, "saved").unwrap();
    down(&mut rt, p);
    motion(&mut rt, to);
    assert_eq!(starts(&rt), 1, "the same mounted row now offers the file");
    assert_eq!(rt.state.downloads.drag_starts, [(host, 1)]);
    assert!(
        rt.state.downloads.dragging.is_none(),
        "headless Rune ends the OS drag as cancelled"
    );
    up(&mut rt, to);
    rt.update(|s, _| s.downloads.dragging = Some((host, 1)));
    settle(&mut rt);
    assert!((element(&rt, "download:1").opacity - 0.55).abs() < 0.001);
    rt.update(|s, _| s.downloads.dragging = None);
    settle(&mut rt);
    click(&mut rt, "download-show:1");
    assert_eq!(rt.state.downloads.shown, [path]);
    assert_eq!(starts(&rt), 1);
}

#[test]
fn leaving_downloads_mid_drag_clears_the_source_before_reopening() {
    let dir = Folder::new("download-drag-close");
    let (mut s, _rx) = workspace_for_drag(false, false);
    let host = s.hosts[0].id;
    let mut transfer = Transfer::new(
        1,
        "s".into(),
        "block:b".into(),
        "saved.txt".into(),
        dir.path().to_owned(),
        EPOCH,
    );
    transfer.state = State::Saved {
        path: dir.join("saved.txt"),
        bytes: 5,
        short_of: None,
    };
    s.hosts[0].model.downloads.push(transfer);
    s.open_panel(PanelKind::Downloads, "Downloads", &[]);
    let mut rt = runtime(s);
    let leave: [fn(&mut Workspace, &mut rune::Tx); 4] = [
        Workspace::close_panel,
        Workspace::cancel_panel,
        |s, _| s.cancel_panel_quiet(),
        |s, _| s.picker(false),
    ];
    for leave in leave {
        // Stand in for an OS drag whose end callback won't reach the unmounted row.
        rt.update(|s, _| s.downloads.dragging = Some((host, 1)));
        settle(&mut rt);
        assert!((element(&rt, "download:1").opacity - 0.55).abs() < 0.001);
        rt.update(leave);
        assert!(rt.state.downloads.dragging.is_none());
        rt.update(|s, _| s.open_panel(PanelKind::Downloads, "Downloads", &[]));
        settle(&mut rt);
        assert!((element(&rt, "download:1").opacity - 1.).abs() < 0.001);
    }
}

/// Run by itself with REX_TEST_SNAP_DIR to check the cues in the rendered app at 640 pt.
#[test]
fn drag_cues_draw() {
    let Some(mut camera) = super::pane_verb_tests::Camera::new() else {
        return;
    };
    let (mut s, _rx) = workspace_for_drag(false, false);
    let (sender, _keys) = flume::unbounded();
    let _left = framed_terminal(&mut s, "block:b", &sender, b"$ cargo test\r\nrunning tests");
    let _right = framed_terminal(&mut s, "block:c", &sender, b"$ tail -f server.log");
    let mut rt = crate::app::fonts::test_runtime(s, view, (640., 480.), Env::new(true, 2.));
    settle(&mut rt);
    let from = visible_center(&rt, "tab:split");
    let to = at(&rt, "tab:single", 0.25, 0.5);
    down(&mut rt, from);
    motion(&mut rt, to);
    settle(&mut rt);
    assert!(has(&rt, "drag-insertion"));
    camera.snap(&mut rt, "drag-toolbar-640");
    rt.cancel_pointer();
    frame(&mut rt);

    let from = at(&rt, "pane-header:block:b", 0.14, 0.5);
    let to = at(&rt, "pane:block:c", 0.5, 0.5);
    down(&mut rt, from);
    motion(&mut rt, to);
    settle(&mut rt);
    assert!(has(&rt, "pane-drop-target:block:c"));
    camera.snap(&mut rt, "drag-pane-640");
    rt.cancel_pointer();
    rt.update(|s, _| s.sidebar.set_open(true));
    settle(&mut rt);

    let from = at(&rt, "tab:single", 0.25, 0.5);
    let to = at(&rt, "session:0:second", 0.5, 0.5);
    down(&mut rt, from);
    motion(&mut rt, to);
    settle(&mut rt);
    assert!(has(&rt, "session-target:0:second"));
    camera.snap(&mut rt, "drag-transfer-640");
}
