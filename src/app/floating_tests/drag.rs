//! Floating layers, slice 2: headless Rune tests for pointer drag-move from floating headers,
//! 8-way edge/corner resize, per-layer divider z-order, cancellation, API 10-12 gate,
//! multi-pane layers, OnHover header retention, Compact density, and real-font ghost captures.
use super::*;
use crate::app::{
    floating_drag::{GRIP_INSIDE, GRIP_OUTSIDE, HEADER_DRAG_SLOP},
    panes,
    workspace::HostId,
};
use rex_rune::{
    connection::Command,
    layers::{self, Grip},
    settings::{PaneDensity, PaneHeader},
};
use rune::{Cursor, WindowEdge};

#[derive(Debug, PartialEq)]
struct ResizeCmd {
    epoch: u64,
    session: String,
    layer: String,
    bounds: [f64; 4],
    raise_first: bool,
}

fn resizes(rx: &flume::Receiver<HostCommand>) -> Vec<ResizeCmd> {
    rx.try_iter()
        .filter_map(|cmd| match cmd {
            HostCommand::ResizeLayer {
                epoch,
                session,
                layer,
                bounds,
                raise_first,
            } => Some(ResizeCmd {
                epoch,
                session,
                layer,
                bounds,
                raise_first,
            }),
            _ => None,
        })
        .collect()
}

fn all_commands(rx: &flume::Receiver<HostCommand>) -> Vec<HostCommand> {
    rx.try_iter().collect()
}

fn layer_at(id: &str, bounds: [f64; 4], blocks: &[&str]) -> Value {
    let mut l = layer(id, None, blocks);
    l["bounds"] = json!(bounds);
    l
}

fn rect_of(rt: &Runtime<Workspace>, key: &str) -> [f32; 4] {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing element {key}"))
        .rect
}

fn has_el(rt: &Runtime<Workspace>, key: &str) -> bool {
    rt.elements().iter().any(|e| e.key.as_deref() == Some(key))
}

fn grip_count(rt: &Runtime<Workspace>) -> usize {
    rt.elements()
        .iter()
        .filter(|e| {
            e.key
                .as_deref()
                .is_some_and(|k| k.starts_with("layer-grip:"))
        })
        .count()
}

fn rects_overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[0] + b[2] && a[0] + a[2] > b[0] && a[1] < b[1] + b[3] && a[1] + a[3] > b[1]
}

fn two_host_runtime(
    revision: u32,
    dev: Vec<Value>,
    focused: &str,
) -> (
    Runtime<Workspace>,
    flume::Receiver<HostCommand>,
    flume::Receiver<HostCommand>,
) {
    let (mut ws, intended) = floating_workspace(revision, dev, focused);
    let mut other = workspace_hosts(2);
    other.hosts[1].id = HostId(202);
    let decoy = capture_host(&mut other, 1);
    let mut state = other.hosts.remove(1);
    state.model = ws.hosts[0].model.clone();
    state.model.connection_epoch = 19;
    ws.hosts.push(state);
    ws.settings.hosts.push(other.settings.hosts.remove(1));
    ws.hosts[0].id = HostId(101);
    let mut rt = runtime(ws);
    draw(&mut rt);
    // Drain any initial visibility/claim commands before the gesture under test.
    let _ = intended.try_iter().count();
    let _ = decoy.try_iter().count();
    (rt, intended, decoy)
}

fn sync_wake(rt: &mut Runtime<Workspace>, change: impl FnOnce(&mut Workspace)) {
    rt.update(|s, tx| {
        change(s);
        *s.hosts[0].connection.model.lock().unwrap() = s.hosts[0].model.clone();
        s.sync_host(s.hosts[0].id, tx);
    });
    draw(rt);
}

#[test]
fn header_move_draws_ghost_without_moving_live_card_and_sends_exact_bounds() {
    let (mut rt, intended, decoy) = two_host_runtime(
        13,
        vec![
            tiled(&["block:a"]),
            layer_at("floating", [0.15, 0.12, 0.5, 0.5], &["block:f"]),
        ],
        "block:f",
    );
    let card_before = rect_of(&rt, "pane:block:f");
    let hdr = rect_of(&rt, "pane-header:block:f");
    // Grab open header space (clear of trailing buttons).
    let sx = hdr[0] + 50.0;
    let sy = hdr[1] + hdr[3] / 2.0;

    rt.pointer_move(sx, sy);
    rt.pointer_down(sx, sy);
    draw(&mut rt);
    assert!(
        !has_el(&rt, "layer-drag-ghost:floating"),
        "ghost stays hidden before crossing the 4 pt drag slop"
    );

    let (dx, dy) = (80.0_f32, 48.0_f32);
    rt.pointer_move(sx + dx, sy + dy);
    draw(&mut rt);
    assert!(has_el(&rt, "layer-drag-ghost:floating"));
    assert_eq!(
        rect_of(&rt, "pane:block:f"),
        card_before,
        "live card never moves ahead of the server"
    );
    assert!(
        all_commands(&intended).is_empty(),
        "nothing is sent while dragging"
    );

    let [_, _, iw, ih] = panes::inner(&rt.state);
    let expected = layers::dragged(
        [0.15, 0.12, 0.5, 0.5],
        Grip::Move,
        [f64::from(dx) / f64::from(iw), f64::from(dy) / f64::from(ih)],
        layers::min_fraction((iw, ih), layers::MIN_FLOATING_PT).unwrap(),
    );
    let ghost_rect = rect_of(&rt, "layer-drag-ghost:floating");
    let expected_card = panes::card_rect(&rt.state, expected);
    // Ghost is inside panes area, so offset by area origin in window coordinates.
    let [ax, ay, _, _] = rt.state.area;
    assert!((ghost_rect[0] - (ax + expected_card[0])).abs() < 0.5);
    assert!((ghost_rect[1] - (ay + expected_card[1])).abs() < 0.5);
    assert!((ghost_rect[2] - expected_card[2]).abs() < 0.5);
    assert!((ghost_rect[3] - expected_card[3]).abs() < 0.5);

    rt.pointer_up(sx + dx, sy + dy);
    draw(&mut rt);
    assert!(!has_el(&rt, "layer-drag-ghost:floating"));
    assert!(rt.state.layer_drag.is_none());
    assert!(all_commands(&decoy).is_empty(), "decoy host sees nothing");

    let sent = resizes(&intended);
    assert_eq!(
        sent,
        vec![ResizeCmd {
            epoch: 7,
            session: "s".into(),
            layer: "floating".into(),
            bounds: expected,
            raise_first: false,
        }]
    );
    // Width and height stayed bit-identical on move; position shifted independently of helper.
    assert!((sent[0].bounds[0] - (0.15 + f64::from(dx) / f64::from(iw))).abs() < 1e-12);
    assert!((sent[0].bounds[1] - (0.12 + f64::from(dy) / f64::from(ih))).abs() < 1e-12);
    assert_eq!(sent[0].bounds[2].to_bits(), 0.5_f64.to_bits());
    assert_eq!(sent[0].bounds[3].to_bits(), 0.5_f64.to_bits());
}

#[test]
fn dragging_a_non_frontmost_layer_requests_raise_first() {
    let (mut rt, intended, decoy) = two_host_runtime(
        13,
        vec![
            tiled(&["block:a"]),
            layer_at("back", [0.05, 0.05, 0.45, 0.45], &["block:b"]),
            layer_at("front", [0.45, 0.4, 0.45, 0.45], &["block:f"]),
        ],
        "block:f",
    );
    let hdr = rect_of(&rt, "pane-header:block:b");
    let (sx, sy) = (hdr[0] + 40.0, hdr[1] + hdr[3] / 2.0);
    rt.pointer_move(sx, sy);
    rt.pointer_down(sx, sy);
    rt.pointer_move(sx + 60.0, sy + 20.0);
    rt.pointer_up(sx + 60.0, sy + 20.0);
    draw(&mut rt);

    assert!(all_commands(&decoy).is_empty());
    let sent = all_commands(&intended);
    assert_eq!(sent.len(), 1, "only ResizeLayer, no accidental focus_block");
    match &sent[0] {
        HostCommand::ResizeLayer {
            epoch,
            session,
            layer,
            raise_first,
            ..
        } => {
            assert_eq!(*epoch, 7);
            assert_eq!(session, "s");
            assert_eq!(layer, "back");
            assert!(*raise_first, "non-frontmost layer asks to raise first");
        }
        other => panic!("unexpected command: {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn click_under_slop_sends_no_layer_request_and_focuses_unfocused_pane() {
    let (mut rt, intended, _) = two_host_runtime(
        13,
        vec![
            tiled(&["block:a"]),
            layer_at("floating", [0.15, 0.12, 0.5, 0.5], &["block:f"]),
        ],
        "block:a",
    );
    // Attach a terminal entry for block:f so focus_block is sent through its host sender.
    rt.update(|s, _| {
        let host_tx = s.hosts[0].connection.commands.clone();
        super::pane_verb_tests::framed_terminal(s, "block:f", &host_tx, b"ready\r\n");
    });
    draw(&mut rt);
    let _ = intended.try_iter().count();

    let hdr = rect_of(&rt, "pane-header:block:f");
    let (sx, sy) = (hdr[0] + 40.0, hdr[1] + hdr[3] / 2.0);
    rt.pointer_move(sx, sy);
    rt.pointer_down(sx, sy);
    // Move strictly under 4 pt slop.
    rt.pointer_move(sx + (HEADER_DRAG_SLOP - 1.5), sy + 1.0);
    draw(&mut rt);
    assert!(!has_el(&rt, "layer-drag-ghost:floating"));
    rt.pointer_up(sx + (HEADER_DRAG_SLOP - 1.5), sy + 1.0);
    draw(&mut rt);

    let cmds = all_commands(&intended);
    assert!(
        cmds.iter()
            .all(|c| !matches!(c, HostCommand::ResizeLayer { .. })),
        "a click under 4 pt sends no ResizeLayer"
    );
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            HostCommand::Request { method, payload }
                if method == "session.focus_block" && payload["block_id"] == "block:f"
        )),
        "a click under 4 pt still focuses an unfocused floating pane"
    );
}

#[test]
fn header_buttons_retry_strip_right_click_and_ne_corner_stay_disjoint() {
    let (mut rt, intended, _) = two_host_runtime(
        13,
        vec![
            tiled(&["block:a"]),
            layer_at("floating", [0.15, 0.12, 0.6, 0.6], &["block:f"]),
        ],
        "block:f",
    );
    // Verify top-right NE corner L-strips do NOT overlap the Close button.
    let close_rect = rect_of(&rt, "pane-close:block:f");
    let ne_h = rect_of(&rt, "layer-grip:floating:ne:h");
    let ne_v = rect_of(&rt, "layer-grip:floating:ne:v");
    assert!(
        !rects_overlap(close_rect, ne_h),
        "Close {close_rect:?} overlaps NE horiz arm {ne_h:?}"
    );
    assert!(
        !rects_overlap(close_rect, ne_v),
        "Close {close_rect:?} overlaps NE vert arm {ne_v:?}"
    );

    // Helper that presses down, verifies layer_drag stays None while held, then releases.
    let press_button_without_arming_drag = |rt: &mut Runtime<Workspace>, key: &str| {
        let r = rect_of(rt, key);
        let (cx, cy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_move(cx, cy);
        rt.pointer_down(cx, cy);
        draw(rt);
        assert!(
            rt.state.layer_drag.is_none(),
            "holding pointer_down on {key} must not arm layer_drag"
        );
        rt.pointer_up(cx, cy);
        draw(rt);
        assert!(rt.state.layer_drag.is_none());
    };

    // Clicking Close opens the close confirmation instead of starting a drag.
    press_button_without_arming_drag(&mut rt, "pane-close:block:f");
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Confirm { .. })
    ));
    assert!(resizes(&intended).is_empty());
    press(&mut rt, Key::Escape);
    draw(&mut rt);

    // Clicking Split right starts no layer drag and sends split.
    press_button_without_arming_drag(&mut rt, "pane-split-right:block:f");
    let sent = all_commands(&intended);
    assert!(
        sent.iter()
            .all(|c| !matches!(c, HostCommand::ResizeLayer { .. }))
    );
    assert!(sent.iter().any(|c| matches!(
        c,
        HostCommand::NewTerminal { method, .. } | HostCommand::Request { method, .. }
            if method == "session.new_split"
    )));

    // Right-clicking the floating header opens the pane menu and starts no drag.
    secondary_click(&mut rt, "pane-header:block:f");
    draw(&mut rt);
    assert!(rt.state.layer_drag.is_none());
    assert!(matches!(
        rt.state.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Pane)
    ));
    press(&mut rt, Key::Escape);
    draw(&mut rt);

    // Retry pill on a reconnecting floating pane keeps its press and starts no drag.
    rt.update(|s, _| {
        let host_tx = s.hosts[0].connection.commands.clone();
        super::pane_verb_tests::framed_terminal(s, "block:f", &host_tx, b"ready\r\n");
        let key = s.term_key("block:f");
        s.terminals[&key].connection.model.lock().unwrap().connected = false;
    });
    draw(&mut rt);
    let _ = intended.try_iter().count();
    assert!(has_el(&rt, "strip-action:block:f"));
    press_button_without_arming_drag(&mut rt, "strip-action:block:f");
    let sent = all_commands(&intended);
    assert!(
        sent.iter()
            .all(|c| !matches!(c, HostCommand::ResizeLayer { .. }))
    );
    assert!(sent.iter().any(|c| matches!(
        c,
        HostCommand::Terminal {
            command: Command::Reconnect,
            ..
        }
    )));
}

#[test]
fn all_eight_resize_grips_set_edge_cursors_and_send_expected_bounds() {
    let start = [0.2, 0.2, 0.5, 0.5];
    let cases = [
        (
            "layer-grip:floating:n",
            Grip::North,
            WindowEdge::North,
            (10.0_f32, -40.0_f32),
        ),
        (
            "layer-grip:floating:s",
            Grip::South,
            WindowEdge::South,
            (-10.0, 40.0),
        ),
        (
            "layer-grip:floating:e",
            Grip::East,
            WindowEdge::East,
            (50.0, 10.0),
        ),
        (
            "layer-grip:floating:w",
            Grip::West,
            WindowEdge::West,
            (-50.0, -10.0),
        ),
        (
            "layer-grip:floating:nw:h",
            Grip::NorthWest,
            WindowEdge::NorthWest,
            (-40.0, -30.0),
        ),
        (
            "layer-grip:floating:nw:v",
            Grip::NorthWest,
            WindowEdge::NorthWest,
            (-30.0, -25.0),
        ),
        (
            "layer-grip:floating:ne:h",
            Grip::NorthEast,
            WindowEdge::NorthEast,
            (45.0, -30.0),
        ),
        (
            "layer-grip:floating:ne:v",
            Grip::NorthEast,
            WindowEdge::NorthEast,
            (35.0, -20.0),
        ),
        (
            "layer-grip:floating:sw:h",
            Grip::SouthWest,
            WindowEdge::SouthWest,
            (-40.0, 35.0),
        ),
        (
            "layer-grip:floating:sw:v",
            Grip::SouthWest,
            WindowEdge::SouthWest,
            (-30.0, 25.0),
        ),
        (
            "layer-grip:floating:se:h",
            Grip::SouthEast,
            WindowEdge::SouthEast,
            (45.0, 35.0),
        ),
        (
            "layer-grip:floating:se:v",
            Grip::SouthEast,
            WindowEdge::SouthEast,
            (35.0, 25.0),
        ),
    ];

    for (key, grip, edge, (dx, dy)) in cases {
        let (mut rt, intended, decoy) = two_host_runtime(
            13,
            vec![
                tiled(&["block:a"]),
                layer_at("floating", start, &["block:f"]),
            ],
            "block:f",
        );
        let r = rect_of(&rt, key);
        let (sx, sy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_move(sx, sy);
        assert_eq!(
            rt.cursor(),
            Cursor::ResizeEdge(edge),
            "{key} cursor mismatch"
        );

        let [_, _, iw, ih] = panes::inner(&rt.state);
        let expected = layers::dragged(
            start,
            grip,
            [f64::from(dx) / f64::from(iw), f64::from(dy) / f64::from(ih)],
            layers::min_fraction((iw, ih), layers::MIN_FLOATING_PT).unwrap(),
        );

        rt.pointer_down(sx, sy);
        rt.pointer_move(sx + dx, sy + dy);
        draw(&mut rt);
        assert!(
            has_el(&rt, "layer-drag-ghost:floating"),
            "{key} must draw ghost"
        );
        rt.pointer_up(sx + dx, sy + dy);
        draw(&mut rt);

        assert!(all_commands(&decoy).is_empty(), "{key}");
        let sent = resizes(&intended);
        assert_eq!(
            sent,
            vec![ResizeCmd {
                epoch: 7,
                session: "s".into(),
                layer: "floating".into(),
                bounds: expected,
                raise_first: false,
            }],
            "{key}"
        );

        // Independent directional and opposite-edge assertions (without calling `layers::dragged`).
        let [gx, gy, gw, gh] = sent[0].bounds;
        let fdx = f64::from(dx) / f64::from(iw);
        let fdy = f64::from(dy) / f64::from(ih);
        let eps = 1e-12;
        match grip {
            Grip::North => {
                assert_eq!(gx.to_bits(), 0.2_f64.to_bits(), "{key}");
                assert_eq!(gw.to_bits(), 0.5_f64.to_bits(), "{key}");
                assert!((gy - (0.2 + fdy)).abs() < eps && gy < 0.2, "{key}");
                assert!(((gy + gh) - 0.7).abs() < eps, "{key} bottom moved");
            }
            Grip::South => {
                assert_eq!(gx.to_bits(), 0.2_f64.to_bits(), "{key}");
                assert_eq!(gw.to_bits(), 0.5_f64.to_bits(), "{key}");
                assert_eq!(gy.to_bits(), 0.2_f64.to_bits(), "{key}");
                assert!((gh - (0.5 + fdy)).abs() < eps && gh > 0.5, "{key}");
            }
            Grip::East => {
                assert_eq!(gx.to_bits(), 0.2_f64.to_bits(), "{key}");
                assert_eq!(gy.to_bits(), 0.2_f64.to_bits(), "{key}");
                assert_eq!(gh.to_bits(), 0.5_f64.to_bits(), "{key}");
                assert!((gw - (0.5 + fdx)).abs() < eps && gw > 0.5, "{key}");
            }
            Grip::West => {
                assert_eq!(gy.to_bits(), 0.2_f64.to_bits(), "{key}");
                assert_eq!(gh.to_bits(), 0.5_f64.to_bits(), "{key}");
                assert!((gx - (0.2 + fdx)).abs() < eps && gx < 0.2, "{key}");
                assert!(((gx + gw) - 0.7).abs() < eps, "{key} right moved");
            }
            Grip::NorthWest => {
                assert!((gx - (0.2 + fdx)).abs() < eps && gx < 0.2, "{key}");
                assert!((gy - (0.2 + fdy)).abs() < eps && gy < 0.2, "{key}");
                assert!(((gx + gw) - 0.7).abs() < eps, "{key} right moved");
                assert!(((gy + gh) - 0.7).abs() < eps, "{key} bottom moved");
            }
            Grip::NorthEast => {
                assert_eq!(gx.to_bits(), 0.2_f64.to_bits(), "{key}");
                assert!((gw - (0.5 + fdx)).abs() < eps && gw > 0.5, "{key}");
                assert!((gy - (0.2 + fdy)).abs() < eps && gy < 0.2, "{key}");
                assert!(((gy + gh) - 0.7).abs() < eps, "{key} bottom moved");
            }
            Grip::SouthWest => {
                assert_eq!(gy.to_bits(), 0.2_f64.to_bits(), "{key}");
                assert!((gh - (0.5 + fdy)).abs() < eps && gh > 0.5, "{key}");
                assert!((gx - (0.2 + fdx)).abs() < eps && gx < 0.2, "{key}");
                assert!(((gx + gw) - 0.7).abs() < eps, "{key} right moved");
            }
            Grip::SouthEast => {
                assert_eq!(gx.to_bits(), 0.2_f64.to_bits(), "{key}");
                assert_eq!(gy.to_bits(), 0.2_f64.to_bits(), "{key}");
                assert!((gw - (0.5 + fdx)).abs() < eps && gw > 0.5, "{key}");
                assert!((gh - (0.5 + fdy)).abs() < eps && gh > 0.5, "{key}");
            }
            Grip::Move => unreachable!(),
        }
    }
}

#[test]
fn cancellation_matrix_sends_nothing_and_never_rearms_on_later_motion() {
    enum Cancel {
        Escape,
        Pointer,
        WindowFocusLost,
        PanelOpen,
        SettingsOpen,
        ConfirmOpen,
        UpdatesSettingsOpen,
        Disconnect,
        EpochChange,
        LayerGone,
        LayerMinimized,
        TabZoomed,
        TabSwitched,
        BecameFullBleed,
        RemoteBoundsChange,
    }

    for (name, kind, expect_notice) in [
        ("escape", Cancel::Escape, false),
        ("cancel_pointer", Cancel::Pointer, false),
        ("window_focus_false", Cancel::WindowFocusLost, false),
        ("panel_open", Cancel::PanelOpen, false),
        ("settings_open", Cancel::SettingsOpen, false),
        ("confirm_open", Cancel::ConfirmOpen, false),
        ("updates_settings_open", Cancel::UpdatesSettingsOpen, false),
        ("disconnect", Cancel::Disconnect, true),
        ("epoch_change", Cancel::EpochChange, true),
        ("layer_gone", Cancel::LayerGone, true),
        ("layer_minimized", Cancel::LayerMinimized, true),
        ("tab_zoomed", Cancel::TabZoomed, true),
        ("tab_switched", Cancel::TabSwitched, true),
        ("became_full_bleed", Cancel::BecameFullBleed, true),
        ("remote_bounds_change", Cancel::RemoteBoundsChange, true),
    ] {
        let (mut rt, intended, decoy) = two_host_runtime(
            13,
            vec![
                tiled(&["block:a"]),
                layer_at("floating", [0.15, 0.12, 0.5, 0.5], &["block:f"]),
            ],
            "block:f",
        );
        // Attach a terminal so we can also prove Escape never leaks to the PTY.
        rt.update(|s, _| {
            let host_tx = s.hosts[0].connection.commands.clone();
            super::pane_verb_tests::framed_terminal(s, "block:f", &host_tx, b"ready\r\n");
        });
        sync_wake(&mut rt, |_| {});
        let _ = intended.try_iter().count();

        let hdr = rect_of(&rt, "pane-header:block:f");
        let (sx, sy) = (hdr[0] + 50.0, hdr[1] + hdr[3] / 2.0);
        rt.pointer_move(sx, sy);
        rt.pointer_down(sx, sy);
        rt.pointer_move(sx + 40.0, sy + 20.0);
        draw(&mut rt);
        assert!(has_el(&rt, "layer-drag-ghost:floating"), "{name}");

        match kind {
            Cancel::Escape => {
                press(&mut rt, Key::Escape);
                draw(&mut rt);
            }
            Cancel::Pointer => {
                // Rune's window_focus(false) triggers cancel_pointer() -> cancelled Phase::Up.
                rt.window_focus(false);
                draw(&mut rt);
            }
            Cancel::WindowFocusLost => {
                // Direct Workspace::window_focus(false) clears drag even before Rune releases.
                rt.update(|s, _| s.window_focus(false));
                draw(&mut rt);
            }
            Cancel::PanelOpen => {
                rt.update(|s, _| s.picker(false));
                draw(&mut rt);
            }
            Cancel::SettingsOpen => {
                rt.update(|s, tx| s.perform(Intent::Settings, tx));
                draw(&mut rt);
            }
            Cancel::ConfirmOpen => {
                rt.update(|s, _| {
                    s.confirm("Close?", "session.close_block", json!({}), "Ends pane.");
                });
                draw(&mut rt);
            }
            Cancel::UpdatesSettingsOpen => {
                rt.update(|s, _| s.open_updates_settings());
                draw(&mut rt);
            }
            Cancel::Disconnect => {
                sync_wake(&mut rt, |s| s.hosts[0].model.connected = false);
            }
            Cancel::EpochChange => {
                sync_wake(&mut rt, |s| s.hosts[0].model.connection_epoch = 8);
            }
            Cancel::LayerGone => {
                sync_wake(&mut rt, |s| {
                    s.hosts[0].model.views.get_mut("s").unwrap().windows[0]
                        .layers
                        .retain(|l| l.layer_id.as_deref() != Some("floating"));
                });
            }
            Cancel::LayerMinimized => {
                sync_wake(&mut rt, |s| {
                    s.hosts[0].model.views.get_mut("s").unwrap().windows[0].layers[1].state =
                        Some("minimized".into());
                });
            }
            Cancel::TabZoomed => {
                sync_wake(&mut rt, |s| {
                    s.hosts[0].model.views.get_mut("s").unwrap().windows[0].zoomed_block_id =
                        Some("block:f".into());
                });
            }
            Cancel::TabSwitched => {
                sync_wake(&mut rt, |s| {
                    s.hosts[0]
                        .model
                        .views
                        .get_mut("s")
                        .unwrap()
                        .active_window_id = Some("other".into());
                });
            }
            Cancel::BecameFullBleed => {
                // Last tiled pane closes while dragging the floating layer -> tab becomes lone
                // full-bleed floating pane and must cancel cleanly instead of stranding the ghost.
                sync_wake(&mut rt, |s| {
                    let tiled_layer =
                        &mut s.hosts[0].model.views.get_mut("s").unwrap().windows[0].layers[0];
                    tiled_layer.layout = None;
                    tiled_layer.blocks.clear();
                });
            }
            Cancel::RemoteBoundsChange => {
                sync_wake(&mut rt, |s| {
                    s.hosts[0].model.views.get_mut("s").unwrap().windows[0].layers[1].bounds =
                        vec![0.25, 0.25, 0.5, 0.5];
                });
            }
        }

        assert!(
            rt.state.layer_drag.is_none(),
            "{name}: layer_drag must clear on cancellation"
        );
        assert!(
            !has_el(&rt, "layer-drag-ghost:floating"),
            "{name}: ghost must clear on cancellation"
        );
        if expect_notice {
            assert!(
                !rt.state.notices.is_empty(),
                "{name}: remote/epoch cancellation should inform the user"
            );
        }

        // Subsequent Move and Up MUST NOT re-arm or send anything!
        rt.pointer_move(sx + 100.0, sy + 60.0);
        draw(&mut rt);
        assert!(
            rt.state.layer_drag.is_none() && !has_el(&rt, "layer-drag-ghost:floating"),
            "{name}: later Move must never re-arm"
        );
        rt.pointer_up(sx + 100.0, sy + 60.0);
        draw(&mut rt);

        // Check exact commands emitted during the cancelled gesture (ignore Claim/PrefetchViews
        // bookkeeping that `sync_host` emits on a connect/disconnect transition; Escape/local
        // cancellations emit literally zero commands of any kind).
        let sent: Vec<_> = all_commands(&intended)
            .into_iter()
            .filter(|c| !matches!(c, HostCommand::PrefetchViews(_) | HostCommand::Claim { .. }))
            .collect();
        assert!(
            sent.is_empty(),
            "{name}: expected zero action/layer/terminal commands, got {:?}",
            sent.iter()
                .map(|c| match c {
                    HostCommand::ResizeLayer { .. } => "ResizeLayer",
                    HostCommand::Request { method, .. } => method.as_str(),
                    HostCommand::NewTerminal { method, .. } => method.as_str(),
                    HostCommand::Terminal { .. } => "Terminal",
                    HostCommand::Claim { .. } => "Claim",
                    HostCommand::Visibility(_) => "Visibility",
                    HostCommand::Select(_) => "Select",
                    HostCommand::RefreshSessions => "RefreshSessions",
                    HostCommand::Identify => "Identify",
                    HostCommand::Keymap(_) => "Keymap",
                    HostCommand::Theme(_) => "Theme",
                    _ => "Other",
                })
                .collect::<Vec<_>>()
        );
        assert!(all_commands(&decoy).is_empty(), "{name}: decoy");
    }
}

#[test]
fn api_10_to_12_and_disconnected_emit_zero_grips_and_send_zero_commands() {
    for (connected, rev) in [(true, 10), (true, 11), (true, 12), (false, 13)] {
        let (mut rt, intended, _) = two_host_runtime(
            rev,
            vec![
                tiled(&["block:a"]),
                layer_at("floating", [0.15, 0.12, 0.5, 0.5], &["block:f"]),
            ],
            "block:f",
        );
        if !connected {
            sync_wake(&mut rt, |s| s.hosts[0].model.connected = false);
        }
        assert_eq!(
            grip_count(&rt),
            0,
            "connected={connected} rev={rev} must emit zero grips"
        );
        let hdr = rect_of(&rt, "pane-header:block:f");
        let (sx, sy) = (hdr[0] + 50.0, hdr[1] + hdr[3] / 2.0);
        rt.pointer_move(sx, sy);
        rt.pointer_down(sx, sy);
        rt.pointer_move(sx + 80.0, sy + 40.0);
        draw(&mut rt);
        assert!(rt.state.layer_drag.is_none());
        assert!(!has_el(&rt, "layer-drag-ghost:floating"));
        rt.pointer_up(sx + 80.0, sy + 40.0);
        draw(&mut rt);
        assert!(resizes(&intended).is_empty());
    }
}

#[test]
fn tiled_divider_under_floating_card_does_not_steal_pointer_input() {
    // Two tiled panes split at x=0.5, with a floating layer directly covering x=0.5.
    let (mut rt, intended, _) = two_host_runtime(
        13,
        vec![
            tiled(&["block:a", "block:b"]),
            layer_at("floating", [0.25, 0.15, 0.5, 0.6], &["block:f"]),
        ],
        "block:f",
    );
    let div_rect = rect_of(&rt, "divider:block:a:block:b");
    let hdr_rect = rect_of(&rt, "pane-header:block:f");
    // Point directly on the tiled divider's x-coordinate, inside the floating header!
    let px = div_rect[0] + div_rect[2] / 2.0;
    let py = hdr_rect[1] + hdr_rect[3] / 2.0;
    assert!(
        px > hdr_rect[0] && px < hdr_rect[0] + hdr_rect[2],
        "divider x={px} must lie inside floating header {hdr_rect:?}"
    );

    rt.pointer_move(px, py);
    rt.pointer_down(px, py);
    rt.pointer_move(px + 60.0, py + 20.0);
    draw(&mut rt);
    assert!(
        rt.state.drag.is_none(),
        "tiled divider underneath must NOT steal pointer input"
    );
    assert!(
        rt.state.layer_drag.is_some(),
        "floating header above the tiled divider receives the drag"
    );
    rt.pointer_up(px + 60.0, py + 20.0);
    draw(&mut rt);
    assert_eq!(resizes(&intended).len(), 1);
}

#[test]
fn multi_pane_floating_layer_keeps_outer_grips_and_inner_divider() {
    let (mut rt, intended, _) = two_host_runtime(
        13,
        vec![
            tiled(&["block:a"]),
            layer_at("floating", [0.15, 0.12, 0.7, 0.7], &["block:f", "block:g"]),
        ],
        "block:f",
    );
    // Exactly one set of outer grips (4 edges + 4*2 corner strips = 12 elements) for the layer.
    assert_eq!(grip_count(&rt), 12);
    assert!(has_el(&rt, "divider:block:f:block:g"));

    // Inner split divider still resizes the split (`session.resize_pane`), not the layer.
    let div = rect_of(&rt, "divider:block:f:block:g");
    let (dx, dy) = (div[0] + div[2] / 2.0, div[1] + div[3] / 2.0);
    rt.pointer_move(dx, dy);
    rt.pointer_down(dx, dy);
    rt.pointer_move(dx + 40.0, dy);
    rt.pointer_up(dx + 40.0, dy);
    draw(&mut rt);
    let sent = all_commands(&intended);
    assert!(
        sent.iter().any(|c| matches!(
            c,
            HostCommand::Request { method, .. } if method == "session.resize_pane"
        )),
        "inner floating split divider must still send session.resize_pane"
    );
    assert!(
        sent.iter()
            .all(|c| !matches!(c, HostCommand::ResizeLayer { .. }))
    );

    // Outer East grip still resizes the floating layer (`HostCommand::ResizeLayer`).
    let east = rect_of(&rt, "layer-grip:floating:e");
    let (ex, ey) = (east[0] + east[2] / 2.0, east[1] + east[3] / 2.0);
    rt.pointer_move(ex, ey);
    rt.pointer_down(ex, ey);
    rt.pointer_move(ex + 30.0, ey);
    rt.pointer_up(ex + 30.0, ey);
    draw(&mut rt);
    assert_eq!(resizes(&intended).len(), 1);
}

#[test]
fn on_hover_floating_header_stays_mounted_while_grabbed() {
    let (mut rt, intended, _) = two_host_runtime(
        13,
        vec![
            tiled(&["block:a"]),
            layer_at("floating", [0.15, 0.12, 0.5, 0.5], &["block:f"]),
        ],
        "block:f",
    );
    rt.update(|s, _| {
        s.settings.pane_header = PaneHeader::OnHover;
        s.header_reveal.hover = Some("block:f".into());
    });
    draw(&mut rt);
    let hdr = rect_of(&rt, "pane-header:block:f");
    let (sx, sy) = (hdr[0] + 50.0, hdr[1] + hdr[3] / 2.0);
    rt.pointer_move(sx, sy);
    rt.pointer_down(sx, sy);
    // Simulate hover leaving the reveal band while dragging far downward.
    rt.update(|s, _| s.header_reveal.hover = None);
    rt.pointer_move(sx + 30.0, sy + 180.0);
    draw(&mut rt);
    assert!(
        has_el(&rt, "pane-header:block:f"),
        "OnHover header must stay mounted while grabbed"
    );
    assert!(has_el(&rt, "layer-drag-ghost:floating"));
    rt.pointer_up(sx + 30.0, sy + 180.0);
    draw(&mut rt);
    assert_eq!(resizes(&intended).len(), 1);
    assert!(
        !has_el(&rt, "pane-header:block:f"),
        "OnHover header hides again once released outside the reveal band"
    );

    // Opening Find/search mid-drag on an OnHover floating header cancels the drag cleanly
    // instead of stranding the ghost outline when the floating header hides for search.
    rt.update(|s, _| {
        let host_tx = s.hosts[0].connection.commands.clone();
        super::pane_verb_tests::framed_terminal(s, "block:f", &host_tx, b"ready\r\n");
        s.header_reveal.hover = Some("block:f".into());
    });
    draw(&mut rt);
    let _ = intended.try_iter().count();
    let hdr = rect_of(&rt, "pane-header:block:f");
    let (sx, sy) = (hdr[0] + 50.0, hdr[1] + hdr[3] / 2.0);
    rt.pointer_move(sx, sy);
    rt.pointer_down(sx, sy);
    rt.pointer_move(sx + 40.0, sy + 60.0);
    draw(&mut rt);
    assert!(has_el(&rt, "layer-drag-ghost:floating"));
    rt.update(|s, _| s.open_search());
    draw(&mut rt);
    assert!(rt.state.layer_drag.is_none());
    assert!(!has_el(&rt, "layer-drag-ghost:floating"));
    rt.pointer_up(sx + 40.0, sy + 60.0);
    draw(&mut rt);
    assert!(resizes(&intended).is_empty());
}

#[test]
fn minimum_640x480_extremes_stay_sendable_and_lone_full_bleed_has_no_grips() {
    let (mut ws, intended) = floating_workspace(
        13,
        vec![
            tiled(&["block:a"]),
            layer_at("floating", [0.15, 0.12, 0.5, 0.5], &["block:f"]),
        ],
        "block:f",
    );
    ws.settings.pane_density = PaneDensity::Compact;
    let mut rt = runtime_at(ws, (640.0, 480.0));
    draw(&mut rt);
    let _ = intended.try_iter().count();

    // Drag SE corner way past the bottom-right window edge.
    let se = rect_of(&rt, "layer-grip:floating:se:h");
    let (sx, sy) = (se[0] + se[2] / 2.0, se[1] + se[3] / 2.0);
    rt.pointer_move(sx, sy);
    rt.pointer_down(sx, sy);
    rt.pointer_move(sx + 2000.0, sy + 2000.0);
    draw(&mut rt);
    assert!(has_el(&rt, "layer-drag-ghost:floating"));
    rt.pointer_up(sx + 2000.0, sy + 2000.0);
    draw(&mut rt);
    let sent = resizes(&intended);
    assert_eq!(sent.len(), 1);
    let [x, y, w, h] = sent[0].bounds;
    assert!(x >= 0.0 && y >= 0.0 && w > 0.0 && h > 0.0 && x + w <= 1.0 && y + h <= 1.0);
    assert_eq!(layers::sendable(sent[0].bounds), Some(sent[0].bounds));

    // Lone full-bleed floating pane (empty tiled layer, default PaneHeader::Auto): zero grips.
    let (ws_lone, _) = floating_workspace(
        13,
        vec![
            tiled(&[]),
            layer_at("floating", [0.15, 0.12, 0.5, 0.5], &["block:f"]),
        ],
        "block:f",
    );
    let mut rt_lone = runtime_at(ws_lone, (640.0, 480.0));
    draw(&mut rt_lone);
    assert_eq!(
        grip_count(&rt_lone),
        0,
        "lone full-bleed floating pane stays untouched with zero grips"
    );
    // Sanity-check grip constants stay within documented 4 pt outside / 3 pt inside band.
    assert_eq!((GRIP_OUTSIDE, GRIP_INSIDE), (4.0, 3.0));
}

#[test]
fn floating_cards_grips_and_drag_work_over_tiled_zoom_with_stable_tiled_geometry() {
    let (mut ws, intended) = floating_workspace(
        21,
        vec![
            tiled(&["block:a", "block:b"]),
            layer_at("floating", [0.25, 0.2, 0.5, 0.5], &["block:f"]),
        ],
        "block:a",
    );
    {
        let window = &mut ws.hosts[0].model.views.get_mut("s").unwrap().windows[0];
        window.zoomed_block_id = Some("block:a".into());
        window.tiled_zoomed_block_id = Some("block:a".into());
    }
    let mut rt = runtime_at(ws, (1280.0, 800.0));
    draw(&mut rt);
    let _ = intended.try_iter().count();

    // Zoomed tiled pane fills the pane area, hidden tiled sibling is absent, and the floating
    // pane with its resize grips draws on top.
    assert!(has_el(&rt, "pane:block:a"));
    assert!(!has_el(&rt, "pane:block:b"));
    assert!(has_el(&rt, "pane:block:f"));
    assert!(has_el(&rt, "pane-zoomed:block:a"));
    assert!(!has_el(&rt, "pane-zoomed:block:f"));
    assert!(
        grip_count(&rt) > 0,
        "floating grips must stay active over tiled zoom"
    );
    let tiled_rect_when_tiled_focused = rect_of(&rt, "pane:block:a");

    // Moving focus to the floating pane clears `zoomed_block_id` while keeping
    // `tiled_zoomed_block_id` set (API 21 server state): zoomed tiled pane keeps exact rect.
    sync_wake(&mut rt, |s| {
        let window = &mut s.hosts[0].model.views.get_mut("s").unwrap().windows[0];
        window.focused_block_id = Some("block:f".into());
        window.zoomed_block_id = None;
        window.tiled_zoomed_block_id = Some("block:a".into());
    });
    assert_eq!(
        rect_of(&rt, "pane:block:a"),
        tiled_rect_when_tiled_focused,
        "moving focus to floating pane must not resize the zoomed tiled pane"
    );
    assert!(has_el(&rt, "pane-zoomed:block:a"));
    assert!(!has_el(&rt, "pane-zoomed:block:f"));
    assert!(grip_count(&rt) > 0);

    // Pane menu / Commands labels reflect each pane's own zoom state accurately.
    rt.state.pane_menu("block:a".into(), None);
    assert!(
        rt.state.entries().iter().any(|e| e.label == "Restore pane"),
        "tiled-zoomed pane must offer Restore pane even while floating pane is focused"
    );
    rt.state.panel = None;
    rt.state.pane_menu("block:f".into(), None);
    assert!(
        rt.state.entries().iter().any(|e| e.label == "Zoom pane"),
        "unzoomed floating pane over tiled zoom must offer Zoom pane"
    );
    rt.state.panel = None;
    draw(&mut rt);

    // Dragging the floating pane's header commits a ResizeLayer while tiled zoom stays active.
    let hdr = rect_of(&rt, "pane-header:block:f");
    let (sx, sy) = (hdr[0] + 40.0, hdr[1] + hdr[3] / 2.0);
    rt.pointer_move(sx, sy);
    rt.pointer_down(sx, sy);
    rt.pointer_move(sx + 40.0, sy + 24.0);
    draw(&mut rt);
    assert!(has_el(&rt, "layer-drag-ghost:floating"));
    rt.pointer_up(sx + 40.0, sy + 24.0);
    draw(&mut rt);
    let sent = resizes(&intended);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].layer, "floating");

    // Floating zoom (`zoomed_block_id != tiled_zoomed_block_id`) covers the full window and
    // removes floating resize grips.
    sync_wake(&mut rt, |s| {
        let window = &mut s.hosts[0].model.views.get_mut("s").unwrap().windows[0];
        window.zoomed_block_id = Some("block:f".into());
        window.tiled_zoomed_block_id = Some("block:a".into());
    });
    assert!(has_el(&rt, "pane:block:f"));
    assert!(!has_el(&rt, "pane:block:a"));
    assert_eq!(grip_count(&rt), 0);
}

/// Real fonts are process-global: run each capture on its own with `REX_TEST_SNAP_DIR`.
#[test]
fn floating_drag_ghost_draw() {
    let Some(mut camera) = super::pane_verb_tests::Camera::new() else {
        return;
    };
    let (w, h, compact, hidden, suffix) = capture_shape();
    let mut ws = capture_workspace(compact, hidden);
    if compact {
        ws.toggle_pane_headers();
    }
    let mut rt = Runtime::new(ws, view, (w, h), Env::new(true, 2.));
    rt.on_key = Some(crate::app::input::on_key);
    settle(&mut rt);
    let hdr = rect_of(&rt, "pane-header:block:f");
    let (sx, sy) = (hdr[0] + 40.0, hdr[1] + hdr[3] / 2.0);
    rt.pointer_move(sx, sy);
    rt.pointer_down(sx, sy);
    rt.pointer_move(sx + 48.0, sy + 32.0);
    settle(&mut rt);
    assert!(has_el(&rt, "layer-drag-ghost:float-a"));
    camera.snap(&mut rt, &format!("floating_drag_ghost{suffix}"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// Real fonts are process-global: run each capture on its own with `REX_TEST_SNAP_DIR`.
#[test]
fn floating_resize_ghost_draw() {
    let Some(mut camera) = super::pane_verb_tests::Camera::new() else {
        return;
    };
    let (w, h, compact, hidden, suffix) = capture_shape();
    let ws = capture_workspace(compact, hidden);
    let mut rt = Runtime::new(ws, view, (w, h), Env::new(true, 2.));
    rt.on_key = Some(crate::app::input::on_key);
    settle(&mut rt);
    let se = rect_of(&rt, "layer-grip:float-a:se:h");
    let (sx, sy) = (se[0] + se[2] / 2.0, se[1] + se[3] / 2.0);
    rt.pointer_move(sx, sy);
    rt.pointer_down(sx, sy);
    rt.pointer_move(sx - 64.0, sy - 40.0);
    settle(&mut rt);
    assert!(has_el(&rt, "layer-drag-ghost:float-a"));
    camera.snap(&mut rt, &format!("floating_resize_ghost{suffix}"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}
