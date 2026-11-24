//! Floating layers, slice 1: the layer verbs, their Commands and pane-menu rows, the cached
//! floating-layer list, and the confirmation that closes a layer. The server owns membership,
//! geometry, order and state; every test checks the one request sent (or that none was), never
//! a local layout.
use super::*;
use crate::app::{
    actions::{self, Remote},
    panel::{Entry, LayerAction, PaneAction},
};
use rex_rune::layers;
use serde_json::{Value, json};

#[path = "floating_tests/drag.rs"]
mod drag;
#[path = "floating_tests/history.rs"]
mod history;

/// The payload's keys, sorted: the layer methods reject a property they do not declare.
fn keys(payload: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = payload
        .as_object()
        .expect("an object payload")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

/// One floating layer with `blocks`, named `id`, optionally minimized.
fn layer(id: &str, state: Option<&str>, blocks: &[&str]) -> Value {
    let layout = match blocks {
        [one] => json!({"block_id": one}),
        [one, two] => json!({"split":{"direction":"horizontal","ratio":0.5,
            "before":{"block_id": one},"after":{"block_id": two}}}),
        _ => unreachable!(),
    };
    let listed: Vec<Value> = blocks
        .iter()
        .map(|b| json!({"block_id": b, "label": b.trim_start_matches("block:")}))
        .collect();
    let mut layer = json!({
        "layer_id": id,
        "kind": "floating",
        "bounds": [0.15, 0.12, 0.7, 0.7],
        "layout": layout,
        "blocks": listed,
    });
    if let Some(state) = state {
        layer["state"] = json!(state);
    }
    layer
}

/// The tab's tiled layer: `blocks` in one horizontal chain, or empty.
fn tiled(blocks: &[&str]) -> Value {
    let layout = match blocks {
        [] => Value::Null,
        [one] => json!({"block_id": one}),
        [one, two] => json!({"split":{"direction":"horizontal","ratio":0.5,
            "before":{"block_id": one},"after":{"block_id": two}}}),
        _ => unreachable!(),
    };
    let listed: Vec<Value> = blocks
        .iter()
        .map(|b| json!({"block_id": b, "label": b.trim_start_matches("block:")}))
        .collect();
    json!({
        "layer_id": "tiled",
        "kind": "tiled",
        "bounds": [0, 0, 1, 1],
        "layout": layout,
        "blocks": listed,
    })
}

/// Host 0 connected on session "s" at `revision`, showing tab "dev" (whose layers `dev` are,
/// back to front) with `focused`; connection epoch 7.
fn floating_workspace(
    revision: u32,
    dev: Vec<Value>,
    focused: &str,
) -> (Workspace, flume::Receiver<HostCommand>) {
    let mut ws = workspace_hosts(1);
    let rx = capture_host(&mut ws, 0);
    let model = &mut ws.hosts[0].model;
    model.connected = true;
    model.protocol.api_revision = revision;
    model.connection_epoch = 7;
    model.sessions = vec![session("s")];
    model.selected = Some("s".into());
    model.views.insert(
        "s".into(),
        serde_json::from_value(json!({
            "revision": 1, "session_id": "s", "active_window_id": "dev",
            "windows": [
                {"window_id": "dev", "label": "dev", "focused_block_id": focused,
                 "layers": dev},
                // Another tab with a floating layer of its own: the list is for the tab on
                // screen only, and no other tab's layout is read for it.
                {"window_id": "other", "label": "other", "layers": [
                    tiled(&["block:o"]),
                    {
                        "layer_id": "elsewhere", "kind": "floating", "state": "minimized",
                        "bounds": [0.2, 0.2, 0.5, 0.5],
                        "layout": {"block_id": "block:oh"},
                        "blocks": [{"block_id": "block:oh", "label": "hidden elsewhere"}]
                    }
                ]}
            ]
        }))
        .expect("a view"),
    );
    (ws, rx)
}

/// A tiled pane and one floating pane, focused where `focused` says.
fn tiled_and_floating(focused: &str) -> (Workspace, flume::Receiver<HostCommand>) {
    floating_workspace(
        13,
        vec![tiled(&["block:a"]), layer("floating", None, &["block:f"])],
        focused,
    )
}

fn tx() -> rune::Tx {
    rune::Tx::default()
}

/// The Commands row with this id, as the list shows it now.
fn command(ws: &Workspace, id: &str) -> crate::app::commands::Command {
    ws.commands()
        .into_iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no {id}"))
}

/// Take a row, as pressing it does.
fn choose(ws: &mut Workspace, id: &str) {
    let row = command(ws, id);
    assert!(row.entry.enabled, "{id} is disabled");
    ws.perform(row.entry.action, &mut tx());
}

fn notices(ws: &Workspace) -> Vec<&str> {
    ws.notices.iter().map(|n| n.text.as_str()).collect()
}

fn run(ws: &mut Workspace, name: &str) {
    assert!(
        actions::run(ws, name, &Value::Null, &mut tx()),
        "{name} refused its own arguments"
    );
}

fn perform(ws: &mut Workspace, target: crate::app::panel::LayerTarget, action: LayerAction) {
    ws.perform(Intent::Layer(target, action), &mut tx());
}

/// What the view says about the dev tab's layer `id`, unchanged by the client.
fn layer_state(ws: &Workspace, id: &str) -> Option<String> {
    ws.hosts[0].model.views["s"].windows[0]
        .layers
        .iter()
        .find(|l| l.layer_id.as_deref() == Some(id))
        .and_then(|l| l.state.clone())
}

#[test]
fn every_layer_verb_is_local_only_with_a_title_a_heading_and_a_commands_row() {
    for (name, title) in [
        ("pane.float", "Float pane"),
        ("pane.new_floating", "New floating pane"),
        ("pane.minimize_layer", "Minimize floating layer"),
        ("pane.restore_layer", "Restore and focus floating layer"),
        ("pane.raise_layer", "Bring floating layer to front"),
        ("pane.return_to_tiles", "Return pane to tiles"),
        ("pane.close_layer", "Close floating layer"),
    ] {
        let spec = actions::find(name).unwrap_or_else(|| panic!("{name} is registered"));
        assert_eq!(spec.title(), title, "{name}");
        assert_eq!(spec.category(), "Panes", "{name}");
        assert!(spec.accepts(&Value::Null), "{name}");
        assert!(
            matches!(spec.remote(), Remote::Never(reason) if !reason.is_empty()),
            "{name} must stay local: a returned client action must not change a layer"
        );
    }
    // Each verb has its Commands row, under the Pane group, saying what it does.
    let (ws, _rx) = tiled_and_floating("block:a");
    for (name, title) in [
        ("pane.float", "Float pane"),
        ("pane.new_floating", "New floating pane"),
        ("pane.minimize_layer", "Minimize floating layer"),
        ("pane.restore_layer", "Restore and focus floating layer"),
        ("pane.raise_layer", "Bring floating layer to front"),
        ("pane.return_to_tiles", "Return pane to tiles"),
        ("pane.close_layer", "Close floating layer…"),
    ] {
        let row = command(&ws, name);
        assert_eq!(row.entry.label, title, "{name}");
        assert_eq!(row.group, crate::app::commands::Group::Pane, "{name}");
    }
    // The remote allowed set is exactly the eight view actions it was: no layer verb joins it.
    let mut allowed: Vec<&str> = actions::SPECS
        .iter()
        .filter(|s| matches!(s.remote(), Remote::View { .. }))
        .map(|s| s.name)
        .collect();
    allowed.sort();
    assert_eq!(allowed.len(), 8, "{allowed:?}");
    for name in ["pane.float", "pane.close_layer", "pane.return_to_tiles"] {
        assert!(!allowed.contains(&name), "{name}");
    }
}

#[test]
fn api_10_to_12_refuse_every_verb_with_the_revision_and_write_nothing() {
    for revision in 10..13 {
        let (mut ws, rx) = tiled_and_floating("block:f");
        ws.hosts[0].model.protocol.api_revision = revision;
        assert_eq!(ws.layer_gate(), Some(layers::Blocked::OldServer(revision)));
        let target = ws.focused_layer().expect("a floating layer");
        for (name, action) in [
            ("pane.float", LayerAction::Float),
            ("pane.new_floating", LayerAction::NewFloating),
            ("pane.minimize_layer", LayerAction::Minimize),
            ("pane.restore_layer", LayerAction::Focus),
            ("pane.raise_layer", LayerAction::Raise),
            ("pane.return_to_tiles", LayerAction::ReturnToTiles),
            ("pane.close_layer", LayerAction::Close),
        ] {
            run(&mut ws, name);
            perform(&mut ws, target.clone(), action);
            let notices = notices(&ws);
            assert!(
                notices
                    .iter()
                    .any(|n| n.contains("floating panes need revision 13")),
                "{name} at {revision}: {notices:?}"
            );
        }
        // Not one command to the host, and no local view edited on its behalf.
        assert!(
            rx.try_iter().next().is_none(),
            "a revision {revision} host must see no layer request"
        );
        assert_eq!(layer_state(&ws, "floating"), None);
        // The rows stay listed, disabled, with the revision's reason in their hint column.
        for id in [
            "pane.float",
            "pane.minimize_layer",
            "pane.restore_layer",
            "pane.raise_layer",
            "pane.return_to_tiles",
            "pane.close_layer",
        ] {
            let row = command(&ws, id);
            assert!(!row.entry.enabled, "{id} at {revision}");
            assert!(
                row.entry.detail.contains("Needs Rex API 13"),
                "{id}: {:?}",
                row.entry.detail
            );
        }
        // And the pane menu stays as it was, with no dead layer rows.
        ws.pane_menu("block:f".into(), None);
        let labels: Vec<String> = ws.entries().into_iter().map(|e| e.label).collect();
        assert!(
            !labels
                .iter()
                .any(|l| l.contains("layer") || l.contains("floating")),
            "{labels:?}"
        );
    }
}

#[test]
fn float_asks_for_the_same_block_in_a_centered_rect_and_starts_nothing() {
    let (mut ws, rx) = tiled_and_floating("block:a");
    let before = ws.hosts[0].model.views["s"].revision;
    run(&mut ws, "pane.float");
    let sent = requests(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    let (method, session, payload) = &sent[0];
    assert_eq!(method, "session.float_block");
    assert_eq!(session, "s");
    assert_eq!(keys(payload), ["block_id", "bounds", "session_id"]);
    assert_eq!(payload["block_id"], "block:a");
    assert_eq!(
        payload["bounds"],
        json!(layers::REQUESTED_BOUNDS),
        "the request names a centred rectangle; the server reports the real one"
    );
    // The same block: no new block, tab, session or server, and nothing drawn ahead of the
    // server's reply.
    assert!(payload.get("layout").is_none() && payload.get("block_ids").is_none());
    assert_eq!(ws.hosts[0].model.views["s"].revision, before);
    let [x, y, w, h] = layers::REQUESTED_BOUNDS;
    assert!(w > 0. && h > 0. && x >= 0. && y >= 0. && x + w <= 1. && y + h <= 1.);
}

#[test]
fn floating_a_pane_that_already_floats_is_refused_before_any_request() {
    let (mut ws, rx) = tiled_and_floating("block:f");
    run(&mut ws, "pane.float");
    assert!(rx.try_iter().next().is_none());
    assert!(
        notices(&ws)
            .iter()
            .any(|n| n.contains("already in a floating layer")),
        "{:?}",
        notices(&ws)
    );
    assert!(
        !command(&ws, "pane.float").entry.enabled,
        "Float pane is disabled for a floating pane"
    );
    assert_eq!(
        command(&ws, "pane.float").entry.detail,
        layers::Blocked::AlreadyFloating.hint()
    );
}

#[test]
fn new_floating_starts_one_pane_in_the_tab_on_screen_from_the_shell_path() {
    let (mut ws, rx) = tiled_and_floating("block:a");
    run(&mut ws, "pane.new_floating");
    let sent = requests(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    let (method, session, payload) = &sent[0];
    assert_eq!(method, "session.new_layer");
    assert_eq!(session, "s");
    assert_eq!(
        keys(payload),
        [
            "bounds",
            "cwd_from",
            "focus",
            "layout",
            "session_id",
            "window_id"
        ]
    );
    assert_eq!(payload["window_id"], "dev");
    assert_eq!(payload["bounds"], json!(layers::REQUESTED_BOUNDS));
    assert_eq!(payload["focus"], true);
    // One new terminal from the existing creation path: the terminal shell flavor, in the
    // focused pane's folder, no placeholder program, no second block, no new tab.
    assert_eq!(
        payload["layout"]["block"]["flavor"],
        "com.superlogical.terminal.shell"
    );
    assert!(payload["layout"].get("split").is_none());
    assert_eq!(payload["cwd_from"], "block:a");
    assert!(payload.get("window_label").is_none());

    // With no tab on screen there is nothing to add a layer to: disabled, nothing sent.
    let (mut bare, rx) = floating_workspace(13, vec![tiled(&["block:a"])], "block:a");
    bare.hosts[0].model.views.clear();
    run(&mut bare, "pane.new_floating");
    assert!(rx.try_iter().next().is_none());
    assert!(
        notices(&bare).iter().any(|n| n.contains("Open a tab")),
        "{:?}",
        notices(&bare)
    );
    // With no tab there is no Pane group to list the row in: nothing claims it can run.
    assert!(
        bare.commands().iter().all(|c| c.id != "pane.new_floating"),
        "no pane, no Pane group"
    );
}

#[test]
fn minimize_is_one_state_change_and_the_last_visible_layer_explains_itself() {
    let (mut ws, rx) = tiled_and_floating("block:f");
    let target = ws.focused_layer().expect("a floating layer");
    perform(&mut ws, target, LayerAction::Minimize);
    let sent = requests(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "session.set_layer_state");
    assert_eq!(
        keys(&sent[0].2),
        ["layer_id", "session_id", "state"],
        "minimizing does not focus or raise"
    );
    assert_eq!(sent[0].2["layer_id"], "floating");
    assert_eq!(sent[0].2["state"], "minimized");
    // The client did not decide the new state itself.
    assert_eq!(layer_state(&ws, "floating"), None);

    // The tiled layer emptied: this floating layer holds the tab's only visible panes, and
    // Rex keeps a tab visible. The UI says so and sends nothing.
    let (mut ws, rx) = floating_workspace(
        13,
        vec![tiled(&[]), layer("floating", None, &["block:f"])],
        "block:f",
    );
    let target = ws.focused_layer().expect("a floating layer");
    perform(&mut ws, target, LayerAction::Minimize);
    assert!(rx.try_iter().next().is_none());
    assert!(
        notices(&ws)
            .iter()
            .any(|n| n.contains("only visible panes")),
        "{:?}",
        notices(&ws)
    );
    let row = command(&ws, "pane.minimize_layer");
    assert!(!row.entry.enabled);
    assert_eq!(row.entry.detail, layers::Blocked::LastVisible.hint());
}

#[test]
fn restore_and_focus_is_one_atomic_call_that_also_raises() {
    let (mut ws, rx) = floating_workspace(
        13,
        vec![
            tiled(&["block:a"]),
            layer("floating", Some("minimized"), &["block:f"]),
        ],
        "block:a",
    );
    let targets = ws.floating_layer_targets();
    assert_eq!(targets.len(), 1);
    assert!(targets[0].minimized);
    perform(&mut ws, targets[0].clone(), LayerAction::Focus);
    let sent = requests(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "session.set_layer_state");
    assert_eq!(
        keys(&sent[0].2),
        ["focus", "layer_id", "session_id", "state"]
    );
    assert_eq!(sent[0].2["layer_id"], "floating");
    assert_eq!(sent[0].2["state"], "normal");
    assert_eq!(sent[0].2["focus"], true);
    // One revision, not a state change followed by a raise or a focus.
    assert!(
        sent.iter()
            .all(|(method, ..)| method != "session.raise_layer")
    );
    assert!(
        sent.iter()
            .all(|(method, ..)| method != "session.focus_block")
    );
}

#[test]
fn bring_to_front_never_focuses_and_a_front_layer_is_already_there() {
    let (mut ws, rx) = floating_workspace(
        13,
        vec![
            tiled(&["block:a"]),
            layer("behind", None, &["block:b"]),
            layer("front", None, &["block:f"]),
        ],
        "block:b",
    );
    let behind = ws.focused_layer().expect("a floating layer");
    perform(&mut ws, behind, LayerAction::Raise);
    let sent = requests(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "session.raise_layer");
    assert_eq!(keys(&sent[0].2), ["layer_id", "session_id"]);
    assert_eq!(sent[0].2["layer_id"], "behind");

    // The frontmost layer is already where raising would put it.
    assert_eq!(
        layers::frontmost(&ws.hosts[0].model.views["s"].windows[0])
            .and_then(|l| l.layer_id.clone())
            .as_deref(),
        Some("front")
    );
    ws.hosts[0].model.views.get_mut("s").unwrap().windows[0].focused_block_id =
        Some("block:f".into());
    let front = ws.focused_layer().expect("the front layer");
    perform(&mut ws, front, LayerAction::Raise);
    assert!(rx.try_iter().next().is_none());
    assert!(
        notices(&ws).iter().any(|n| n.contains("already in front")),
        "{:?}",
        notices(&ws)
    );
    assert_eq!(
        command(&ws, "pane.raise_layer").entry.detail,
        layers::Blocked::InFront.hint()
    );
}

#[test]
fn return_to_tiles_uses_the_first_tiled_pane_and_refuses_without_one() {
    let (mut ws, rx) = floating_workspace(
        13,
        vec![
            tiled(&["block:a", "block:b"]),
            layer("floating", None, &["block:f"]),
        ],
        "block:f",
    );
    let target = ws.focused_layer().expect("a floating layer");
    perform(&mut ws, target, LayerAction::ReturnToTiles);
    let sent = requests(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "session.move_block");
    assert_eq!(
        keys(&sent[0].2),
        [
            "anchor_block_id",
            "block_id",
            "direction",
            "ratio",
            "session_id",
            "side"
        ]
    );
    assert_eq!(sent[0].2["block_id"], "block:f");
    assert_eq!(
        sent[0].2["anchor_block_id"], "block:a",
        "the tab's first tiled pane, named in the request"
    );
    assert_eq!(sent[0].2["direction"], "horizontal");
    assert_eq!(sent[0].2["side"], "after");
    assert_eq!(sent[0].2["ratio"], 0.5);
    // No local layout, no placeholder pane, no restart.
    assert!(sent[0].2.get("layout").is_none());

    // The tab has no tiled pane to return beside: disabled with the reason, nothing sent,
    // and no anchor is invented.
    let (mut ws, rx) = floating_workspace(
        13,
        vec![tiled(&[]), layer("floating", None, &["block:f"])],
        "block:f",
    );
    let target = ws.focused_layer().expect("a floating layer");
    perform(&mut ws, target, LayerAction::ReturnToTiles);
    assert!(rx.try_iter().next().is_none());
    assert!(
        notices(&ws)
            .iter()
            .any(|n| n.contains("no tiled pane to return beside")),
        "{:?}",
        notices(&ws)
    );
    let row = command(&ws, "pane.return_to_tiles");
    assert!(!row.entry.enabled);
    assert_eq!(row.entry.detail, layers::Blocked::NoAnchor.hint());
}

#[test]
fn closing_a_layer_names_every_pane_and_sends_one_close_only_on_confirm() {
    let (mut ws, rx) = floating_workspace(
        13,
        vec![
            tiled(&["block:a"]),
            layer("group", None, &["block:f", "block:g"]),
        ],
        "block:f",
    );
    run(&mut ws, "pane.close_layer");
    let panel = ws.panel.as_ref().expect("a confirmation");
    assert_eq!(panel.title, "Close f +1?");
    let PanelKind::Confirm {
        method,
        payload,
        description,
    } = &panel.kind
    else {
        panic!("not a confirmation");
    };
    assert_eq!(method, "session.close_layer");
    assert_eq!(payload["session_id"], "s");
    assert_eq!(payload["layer_id"], "group");
    // The multi-pane layer another client may have made is named pane by pane.
    assert!(description.contains("2 floating panes"), "{description}");
    assert!(description.contains("f, g"), "{description}");
    assert!(description.contains("test-0"), "{description}");
    assert!(
        description.contains("Closing the app itself"),
        "{description}"
    );
    let target = panel.layer.clone().expect("the layer it would close");
    assert_eq!(
        target.panes,
        [
            ("block:f".into(), "f".into()),
            ("block:g".into(), "g".into())
        ]
    );

    // Keep running: nothing is sent.
    ws.cancel_panel(&mut tx());
    assert!(ws.panel.is_none());
    assert!(rx.try_iter().next().is_none());

    // Again, and confirm: exactly one close_layer for this layer, and no separate block close.
    run(&mut ws, "pane.close_layer");
    ws.submit(&mut tx());
    let sent = requests(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "session.close_layer");
    assert_eq!(keys(&sent[0].2), ["layer_id", "session_id"]);
    assert_eq!(sent[0].2["layer_id"], "group");
}

#[test]
fn a_changed_or_reconnected_layer_closes_nothing() {
    for change in ["pane", "layer", "epoch"] {
        let (mut ws, rx) = floating_workspace(
            13,
            vec![
                tiled(&["block:a"]),
                layer("group", None, &["block:f", "block:g"]),
            ],
            "block:f",
        );
        run(&mut ws, "pane.close_layer");
        let panel = ws.panel.as_ref().expect("a confirmation");
        let target = panel.layer.clone().expect("a captured layer");
        match change {
            // Another client added a pane to the layer after the dialog was opened.
            "pane" => ws.hosts[0].model.views.get_mut("s").unwrap().windows[0].layers[1]
                .blocks
                .push(
                    serde_json::from_value(json!({"block_id": "block:late", "label": "late"}))
                        .unwrap(),
                ),
            "layer" => {
                ws.hosts[0].model.views.get_mut("s").unwrap().windows[0]
                    .layers
                    .remove(1);
            }
            _ => ws.hosts[0].model.connection_epoch += 1,
        }
        ws.submit(&mut tx());
        assert!(ws.panel.is_none(), "{change}");
        assert!(
            rx.try_iter().next().is_none(),
            "{change}: a changed layer is never retargeted"
        );
        assert!(
            notices(&ws)
                .iter()
                .any(|n| n.contains("changed since it was listed")),
            "{change}: {:?}",
            notices(&ws)
        );
        // A layer that disappeared, or a new connection, refuses a restore the same way. (A
        // layer that only gained a pane can still be restored, and that is not retargeting.)
        if change != "pane" {
            perform(&mut ws, target, LayerAction::Focus);
            assert!(rx.try_iter().next().is_none(), "{change}: restore");
        }
    }
}

#[test]
fn the_list_names_this_tabs_minimized_layers_and_restores_one_by_name() {
    let (mut ws, rx) = floating_workspace(
        13,
        vec![
            tiled(&["block:a"]),
            layer("shown", None, &["block:f"]),
            layer("hidden", Some("minimized"), &["block:g"]),
        ],
        "block:a",
    );
    let rows: Vec<(String, String, String)> = ws
        .commands()
        .into_iter()
        .filter(|c| c.id.starts_with("pane.layer."))
        .map(|c| (c.id.to_string(), c.entry.label, c.entry.detail))
        .collect();
    assert_eq!(
        rows,
        [
            ("pane.layer.shown".into(), "Show f".into(), String::new()),
            (
                "pane.layer.hidden".into(),
                "Restore g".into(),
                "Minimized, still running".into()
            ),
        ],
        "back to front, this tab only, and the hidden one is named"
    );
    assert!(
        !rows
            .iter()
            .any(|(_, label, _)| label.contains("hidden elsewhere")),
        "{rows:?}"
    );

    choose(&mut ws, "pane.layer.hidden");
    let sent = requests(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "session.set_layer_state");
    assert_eq!(sent[0].1, "s");
    assert_eq!(sent[0].2["layer_id"], "hidden");
    assert_eq!(sent[0].2["state"], "normal");
    assert_eq!(sent[0].2["focus"], true);
}

#[test]
fn the_pane_menu_offers_one_layer_row_and_that_row_lists_every_verb() {
    let (mut ws, _rx) = tiled_and_floating("block:a");
    ws.pane_menu("block:f".into(), None);
    let entries = ws.entries();
    let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
    // One row, marked as another list (a chevron), not six more rows in the menu itself.
    let row = entries
        .iter()
        .find(|e| e.label == "Floating layer…")
        .expect("the layer row");
    assert!(
        row.enabled && row.followup,
        "{} is not another list",
        row.label
    );
    assert!(!labels.contains(&"Minimize layer"), "{labels:?}");
    assert!(
        labels.contains(&"Close pane…"),
        "the pane's own close stays"
    );
    assert_eq!(
        labels.last().copied(),
        Some("Close pane…"),
        "Close pane is still last and destructive"
    );

    // The row opens the same pane's layer verbs, the close last and destructive.
    let intent = row.action.clone();
    ws.perform(intent, &mut tx());
    let panel = ws.panel.as_ref().expect("the layer menu");
    assert!(matches!(panel.kind, PanelKind::Layer));
    assert_eq!(panel.title, "dev · f");
    let entries = ws.entries();
    let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "New floating pane",
            "Minimize layer",
            "Focus layer",
            "Bring layer to front",
            "Return pane to tiles",
            "Close floating layer…"
        ]
    );
    let close = entries.last().expect("a last row");
    assert!(close.danger && close.enabled);

    // A tiled pane offers the two ways to make a floating layer, and no floating-only verb.
    ws.pane_menu("block:a".into(), None);
    let row = ws
        .entries()
        .into_iter()
        .find(|e| e.label == "Floating layer…")
        .expect("the layer row");
    ws.perform(row.action, &mut tx());
    let labels: Vec<String> = ws.entries().into_iter().map(|e| e.label).collect();
    assert_eq!(labels, ["Float pane", "New floating pane"]);

    // A layer that cannot return says why, rather than disappearing silently.
    ws.hosts[0].model.views.get_mut("s").unwrap().windows[0].layers = vec![
        serde_json::from_value(tiled(&[])).unwrap(),
        serde_json::from_value(layer("floating", None, &["block:f"])).unwrap(),
    ];
    ws.pane_menu("block:f".into(), None);
    let row = ws
        .entries()
        .into_iter()
        .find(|e| e.label == "Floating layer…")
        .expect("the layer row");
    ws.perform(row.action, &mut tx());
    let row = ws
        .entries()
        .into_iter()
        .find(|e| e.label == "Return pane to tiles")
        .expect("the row stays listed");
    assert!(!row.enabled);
    assert_eq!(row.detail, layers::Blocked::NoAnchor.hint());
}

#[test]
fn the_confirmation_opens_on_keep_running_and_return_keeps_the_layer() {
    let (mut ws, rx) = floating_workspace(
        13,
        vec![
            tiled(&["block:a"]),
            layer("group", None, &["block:f", "block:g"]),
        ],
        "block:f",
    );
    run(&mut ws, "pane.close_layer");
    let mut rt = runtime(ws);
    let path = focused(&rt).expect("a focused control");
    assert!(path.ends_with("cancel-close"), "focus starts on {path}");
    press(&mut rt, Key::Enter);
    assert!(rt.state.panel.is_none(), "Return pressed Keep running");
    assert!(rx.try_iter().next().is_none(), "and sent nothing");

    let (mut ws, _rx) = floating_workspace(
        13,
        vec![tiled(&["block:a"]), layer("group", None, &["block:f"])],
        "block:f",
    );
    run(&mut ws, "pane.close_layer");
    let rt = runtime(ws);
    assert!(has(&rt, "cancel-close") && has(&rt, "confirm-close"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn pane_close_and_close_focused_still_close_a_pane_not_a_layer() {
    let (mut ws, _rx) = tiled_and_floating("block:f");
    let target = ws.target("block:f").expect("a pane target");
    ws.perform(Intent::Pane(target, PaneAction::Close), &mut tx());
    let panel = ws.panel.as_ref().expect("a pane confirmation");
    assert!(
        matches!(&panel.kind, PanelKind::Confirm { method, .. } if method == "block.close"),
        "the pane's own Close is unchanged"
    );
}

/// The size, density and toolbar mode a capture run asks for: 1024×700 Comfortable with the
/// toolbar, unless `REX_M136_MINIMUM`, `REX_M136_COMPACT` or `REX_M136_NO_TOOLBAR` say
/// otherwise.
fn capture_shape() -> (f32, f32, bool, bool, &'static str) {
    let minimum = std::env::var_os("REX_M136_MINIMUM").is_some();
    let compact = std::env::var_os("REX_M136_COMPACT").is_some();
    let hidden = std::env::var_os("REX_M136_NO_TOOLBAR").is_some();
    let size = if minimum { (640., 480.) } else { (1024., 700.) };
    let suffix = match (minimum, compact, hidden) {
        (false, false, false) => "",
        (true, false, false) => "_minimum",
        (false, true, false) => "_compact",
        (true, true, false) => "_minimum_compact",
        (true, false, true) => "_minimum_no_toolbar",
        _ => "_other",
    };
    (size.0, size.1, compact, hidden, suffix)
}

/// A split tab with one floating layer and one minimized, and live terminals in every pane,
/// for the captures: the floating pane's menu, and the list that restores the hidden one.
fn capture_workspace(compact: bool, no_toolbar: bool) -> Workspace {
    let (mut ws, _rx) = tabbed_workspace("split");
    let (sender, _sent) = flume::unbounded();
    for (block, output) in [
        ("block:b", b"$ cargo test\r\nrunning 12 tests" as &[u8]),
        ("block:c", b"$ tail -f server.log"),
        ("block:f", b"$ cd ~/code/app"),
        ("block:g", b"$ git status"),
    ] {
        super::pane_verb_tests::framed_terminal(&mut ws, block, &sender, output);
    }
    if compact {
        ws.settings.pane_density = rex_rune::settings::PaneDensity::Compact;
    }
    if no_toolbar {
        ws.settings.interface.toolbar = rex_rune::settings::interface::ToolbarBehavior::Hidden;
    }
    ws.hosts[0].model.connection_epoch = 1;
    ws.hosts[0].model.protocol.api_revision = 13;
    let view = ws.hosts[0].model.views.get_mut("s").expect("a view");
    let split = view
        .windows
        .iter_mut()
        .find(|w| w.window_id.as_deref() == Some("split"))
        .expect("the split tab");
    split.layers[0].layer_id = Some("tiled".into());
    split.layers[0].bounds = vec![0., 0., 1., 1.];
    split
        .layers
        .push(serde_json::from_value(layer("float-a", None, &["block:f"])).expect("a layer"));
    split.layers.push(
        serde_json::from_value(layer("float-b", Some("minimized"), &["block:g"])).expect("a layer"),
    );
    split.focused_block_id = Some("block:f".into());
    ws
}

fn settle(rt: &mut Runtime<Workspace>) {
    for _ in 0..90 {
        rt.frame(1. / 60.);
    }
}

/// Real fonts are process-global: run each capture on its own with `REX_TEST_SNAP_DIR`.
#[test]
fn floating_pane_menu_draw() {
    let Some(mut camera) = super::pane_verb_tests::Camera::new() else {
        return;
    };
    let (w, h, compact, hidden, suffix) = capture_shape();
    let ws = capture_workspace(compact, hidden);
    let mut rt = Runtime::new(ws, view, (w, h), Env::new(true, 2.));
    rt.on_key = Some(crate::app::input::on_key);
    settle(&mut rt);
    rt.update(|s, _| s.pane_menu("block:f".into(), None));
    settle(&mut rt);
    camera.snap(&mut rt, &format!("floating_pane_menu{suffix}"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// Real fonts are process-global: run each capture on its own with `REX_TEST_SNAP_DIR`.
#[test]
fn floating_commands_draw() {
    let Some(mut camera) = super::pane_verb_tests::Camera::new() else {
        return;
    };
    let (w, h, compact, hidden, suffix) = capture_shape();
    let ws = capture_workspace(compact, hidden);
    let mut rt = Runtime::new(ws, view, (w, h), Env::new(true, 2.));
    rt.on_key = Some(crate::app::input::on_key);
    settle(&mut rt);
    rt.update(|s, _| {
        s.picker(false);
        if let Some(index) = s.entries().iter().position(|e| e.label == "Restore g") {
            let panel = s.panel.as_mut().expect("the list");
            panel.selected = index;
            panel.reveal = true;
        }
    });
    settle(&mut rt);
    // The headless runtime has no poll hook, so reveal the selected row by hand.
    rt.update(|s, tx| {
        crate::app::poll(s, 0., tx);
    });
    settle(&mut rt);
    camera.snap(&mut rt, &format!("floating_commands{suffix}"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

/// Real fonts are process-global: run each capture on its own with `REX_TEST_SNAP_DIR`.
#[test]
fn floating_layer_menu_draw() {
    let Some(mut camera) = super::pane_verb_tests::Camera::new() else {
        return;
    };
    let (w, h, compact, hidden, suffix) = capture_shape();
    let ws = capture_workspace(compact, hidden);
    let mut rt = Runtime::new(ws, view, (w, h), Env::new(true, 2.));
    rt.on_key = Some(crate::app::input::on_key);
    settle(&mut rt);
    rt.update(|s, tx| {
        s.pane_menu("block:f".into(), None);
        if let Some(row) = s
            .entries()
            .into_iter()
            .find(|e| e.label == "Floating layer…")
        {
            s.perform(row.action, tx);
        }
    });
    settle(&mut rt);
    camera.snap(&mut rt, &format!("floating_layer_menu{suffix}"));
    std::fs::remove_file(&rt.state.settings_path).ok();
}

#[test]
fn a_layer_verb_sends_only_its_own_request_and_leaves_the_terminal_alone() {
    let (mut ws, rx) = tiled_and_floating("block:a");
    let (sender, sent) = flume::unbounded();
    super::pane_verb_tests::framed_terminal(
        &mut ws,
        "block:a",
        &sender,
        b"$ cargo test\r\nselected line",
    );
    let key = ws.term_key("block:a");
    let selection = (
        ws.terminals[&key].has_selection(),
        ws.terminals[&key].selection_revision,
    );

    run(&mut ws, "pane.float");

    let commands: Vec<HostCommand> = rx.try_iter().collect();
    let methods: Vec<&str> = commands
        .iter()
        .map(|command| match command {
            HostCommand::Request { method, .. } => method.as_str(),
            _ => "not a request",
        })
        .collect();
    assert_eq!(methods, ["session.float_block"]);
    // Nothing was typed, resized, recoloured, re-selected or cleared for it: the terminal's
    // worker saw no command at all, and its selection state is what it was.
    assert!(sent.try_iter().next().is_none(), "the terminal was touched");
    assert_eq!(
        (
            ws.terminals[&key].has_selection(),
            ws.terminals[&key].selection_revision
        ),
        selection
    );
    assert_eq!(
        ws.model().focused().map(str::to_owned),
        Some("block:a".into())
    );
}

/// The open list, and the position of the row the menu draws under `label`.
fn find_row(ws: &Workspace, label: &str) -> (Vec<Entry>, usize) {
    let entries = ws.entries();
    let at = entries
        .iter()
        .position(|entry| entry.label == label)
        .unwrap_or_else(|| {
            panic!(
                "no {label} row in {:?}",
                entries.iter().map(|e| &e.label).collect::<Vec<_>>()
            )
        });
    (entries, at)
}

/// F's layer menu, opened from F's own pane-menu row while the keyboard is elsewhere.
fn open_layer_menu(ws: &mut Workspace, block: &str) {
    ws.pane_menu(block.into(), None);
    let (entries, at) = find_row(ws, "Floating layer…");
    ws.perform(entries[at].action.clone(), &mut tx());
    assert!(matches!(
        ws.panel.as_ref().map(|p| &p.kind),
        Some(PanelKind::Layer)
    ));
}

/// The one request a captured `New floating pane` row sends.
fn new_floating_request(ws: &mut Workspace, rx: &flume::Receiver<HostCommand>) -> Value {
    let (entries, at) = find_row(ws, "New floating pane");
    ws.perform(entries[at].action.clone(), &mut tx());
    let sent = requests(rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "session.new_layer", "{sent:?}");
    assert_eq!(sent[0].1, "s", "{sent:?}");
    sent[0].2.clone()
}

#[test]
fn a_captured_layer_menu_starts_its_new_pane_in_the_tab_and_folder_it_named() {
    // The keyboard is on the tiled pane A; the menu was opened on the floating pane F. Their
    // folders differ, so the request says which one it used.
    let (mut ws, rx) = floating_workspace(
        13,
        vec![tiled(&["block:a"]), layer("floating", None, &["block:f"])],
        "block:a",
    );
    open_layer_menu(&mut ws, "block:f");
    let payload = new_floating_request(&mut ws, &rx);
    assert_eq!(payload["cwd_from"], "block:f", "the menu's pane, not A's");
    assert_eq!(payload["window_id"], "dev");
    assert_eq!(payload["session_id"], "s");
    assert!(payload["layout"]["block"]["flavor"].is_string());
    assert_eq!(ws.hosts[0].model.views["s"].revision, 1);

    // Another client shows the other tab while this menu is still open. The menu still names
    // F's tab: the pane exists there, so the request goes there and nowhere else.
    let (mut ws, rx) = floating_workspace(
        13,
        vec![tiled(&["block:a"]), layer("floating", None, &["block:f"])],
        "block:a",
    );
    open_layer_menu(&mut ws, "block:f");
    let view = ws.hosts[0].model.views.get_mut("s").unwrap();
    view.active_window_id = Some("other".into());
    view.windows[1].focused_block_id = Some("block:o".into());
    let payload = new_floating_request(&mut ws, &rx);
    assert_eq!(
        payload["window_id"], "dev",
        "the captured tab, not the one another client raised"
    );
    assert_eq!(payload["cwd_from"], "block:f");
    assert_eq!(payload["session_id"], "s");

    // The focused path is unchanged: Commands and its shortcut use the pane and tab on screen.
    let (mut ws, rx) = floating_workspace(
        13,
        vec![tiled(&["block:a"]), layer("floating", None, &["block:f"])],
        "block:a",
    );
    run(&mut ws, "pane.new_floating");
    let sent = requests(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].2["window_id"], "dev");
    assert_eq!(
        sent[0].2["cwd_from"], "block:a",
        "the focused pane's folder"
    );
}

#[test]
fn a_captured_new_floating_pane_refuses_an_epoch_pane_or_tab_that_is_gone() {
    for change in ["epoch", "pane", "window"] {
        let (mut ws, rx) = floating_workspace(
            13,
            vec![tiled(&["block:a"]), layer("floating", None, &["block:f"])],
            "block:a",
        );
        open_layer_menu(&mut ws, "block:f");
        let (entries, at) = find_row(&ws, "New floating pane");
        let Intent::Layer(target, kind) = entries[at].action.clone() else {
            panic!("not a layer row")
        };
        match change {
            // A new connection: the captured view belongs to the old one.
            "epoch" => {
                ws.hosts[0].model.connection_epoch += 1;
            }
            // The pane the menu named left the view, so its folder cannot be read.
            "pane" => {
                ws.hosts[0].model.views.get_mut("s").unwrap().windows[0].layers[1]
                    .blocks
                    .clear();
            }
            // Its tab is gone altogether.
            "window" => {
                ws.hosts[0]
                    .model
                    .views
                    .get_mut("s")
                    .unwrap()
                    .windows
                    .remove(0);
            }
            _ => unreachable!(),
        }
        ws.perform(Intent::Layer(target, kind), &mut tx());
        let sent = requests(&rx);
        assert!(
            sent.is_empty(),
            "{change}: an invalidated menu must send nothing: {sent:?}"
        );
        assert!(
            notices(&ws)
                .iter()
                .any(|n| n.contains("changed since it was listed")),
            "{change}: {:?}",
            notices(&ws)
        );
    }
}

/// The label of the row the keyboard is on, as the open list draws it.
fn highlighted(rt: &Runtime<Workspace>) -> String {
    let entries = rt.state.entries();
    entries[rt.state.panel.as_ref().expect("an open list").selected]
        .label
        .clone()
}

/// Press Down until the keyboard is on `label`, as a person would.
fn highlight(rt: &mut Runtime<Workspace>, label: &str) {
    for _ in 0..rt.state.entries().len() {
        if highlighted(rt) == label {
            return;
        }
        press(rt, Key::Down);
    }
    panic!(
        "never reached {label}; the keyboard is on {}",
        highlighted(rt)
    );
}

/// Another client raising `layer_id`: the order the view reports back to front afterwards.
fn raised(ws: &mut Workspace, layer_id: &str) {
    let window = &mut ws.hosts[0]
        .model
        .views
        .get_mut("s")
        .expect("a view")
        .windows[0];
    let at = window
        .layers
        .iter()
        .position(|layer| layer.layer_id.as_deref() == Some(layer_id))
        .expect("the layer");
    let layer = window.layers.remove(at);
    window.layers.push(layer);
}

/// The host wake that carries the workspace's view into the app, as a reply does.
fn wake(ws: &mut Workspace) {
    *ws.hosts[0].connection.model.lock().expect("the model") = ws.hosts[0].model.clone();
    ws.sync(0, &mut tx());
}

/// Three floating layers, back to front, with the tiled pane A behind them.
fn three_layers(focused: &str) -> (Workspace, flume::Receiver<HostCommand>) {
    floating_workspace(
        13,
        vec![
            tiled(&["block:a"]),
            layer("alpha", None, &["block:alpha"]),
            layer("beta", None, &["block:beta"]),
            layer("gamma", None, &["block:gamma"]),
        ],
        focused,
    )
}

#[test]
fn a_layer_reorder_moves_the_keyboard_to_the_row_it_named_not_to_its_index() {
    let (ws, rx) = three_layers("block:a");
    let mut rt = runtime(ws);
    rt.update(|s, _| s.picker(false));
    highlight(&mut rt, "Show beta");
    assert_eq!(highlighted(&rt), "Show beta");

    // Another client raises alpha: [alpha, beta, gamma] becomes [beta, gamma, alpha], so the
    // index the keyboard was on now names gamma.
    rt.update(|s, _| raised(s, "alpha"));
    rt.update(|s, _| wake(s));
    assert_eq!(
        highlighted(&rt),
        "Show beta",
        "the highlight follows its row, not its old index"
    );
    press(&mut rt, Key::Enter);
    let sent = requests(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "session.set_layer_state");
    assert_eq!(
        sent[0].2["layer_id"], "beta",
        "Enter ran the row the keyboard named, never the one that took its place"
    );
    assert_eq!(sent[0].2["focus"], true);
}

#[test]
fn a_removed_layer_row_runs_nothing_and_does_not_retarget_its_neighbour() {
    // The selected layer disappears while the list is open: Enter sends nothing at all.
    let (ws, rx) = three_layers("block:a");
    let mut rt = runtime(ws);
    rt.update(|s, _| s.picker(false));
    highlight(&mut rt, "Show beta");
    rt.update(|s, _| {
        let window = &mut s.hosts[0].model.views.get_mut("s").unwrap().windows[0];
        window
            .layers
            .retain(|l| l.layer_id.as_deref() != Some("beta"));
    });
    rt.update(|s, _| wake(s));
    press(&mut rt, Key::Enter);
    let sent = requests(&rx);
    assert!(
        sent.is_empty(),
        "a removed row runs nothing at all: {sent:?}"
    );
    assert!(
        rt.state
            .notices
            .iter()
            .any(|n| n.text.contains("no longer in the list")),
        "{:?}",
        notices(&rt.state)
    );

    // A row that is not in the layer inventory keeps its own identity when one above it goes:
    // the row that shifts into its index is a different command.
    let (ws, rx) = three_layers("block:a");
    let mut rt = runtime(ws);
    rt.update(|s, _| s.picker(false));
    highlight(&mut rt, "Focus next pane");
    rt.update(|s, _| {
        let window = &mut s.hosts[0].model.views.get_mut("s").unwrap().windows[0];
        window
            .layers
            .retain(|l| l.layer_id.as_deref() != Some("alpha"));
    });
    rt.update(|s, _| wake(s));
    assert_eq!(highlighted(&rt), "Focus next pane");
    press(&mut rt, Key::Enter);
    let sent = requests(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(
        sent[0].0, "session.focus_next_pane",
        "the row that was named, not the one that moved into its index"
    );
}

#[test]
fn the_layer_menu_shows_each_bound_verbs_effective_shortcut_and_keeps_a_reason() {
    let bind = |ws: &mut Workspace, value: Value| {
        ws.settings.shortcut_file.overrides = rex_rune::keymap::Overrides::parse(&value);
        ws.local_keymap = actions::resolve(&ws.settings.shortcut_file.overrides);
        ws.keymap_generation += 1;
    };
    let (mut ws, _rx) = floating_workspace(
        13,
        vec![tiled(&["block:a"]), layer("floating", None, &["block:f"])],
        "block:f",
    );
    open_layer_menu(&mut ws, "block:f");
    // Unbound by default, as every layer verb is: the row keeps the words it has.
    for label in [
        "Minimize layer",
        "Return pane to tiles",
        "Close floating layer…",
    ] {
        let (entries, at) = find_row(&ws, label);
        assert!(
            entries[at].keys.is_empty(),
            "{label} has a default key: {:?}",
            entries[at].keys
        );
    }
    assert_eq!(
        find_row(&ws, "Minimize layer").0[find_row(&ws, "Minimize layer").1].detail,
        ""
    );

    // Bound: the menu spells the same hint Commands spells.
    bind(
        &mut ws,
        json!([{"keys":"ctrl+shift+m","action":"pane.minimize_layer"}]),
    );
    let (entries, at) = find_row(&ws, "Minimize layer");
    assert_eq!(
        entries[at].keys,
        [vec!["Ctrl".to_owned(), "Shift".to_owned(), "M".to_owned()]]
    );
    assert_eq!(entries[at].detail, "Ctrl Shift M");
    assert_eq!(
        entries[at].detail,
        command(&ws, "pane.minimize_layer").entry.detail,
        "the menu and Commands share one formatter"
    );
    // The row still names the menu's own pane: formatting never retargets it.
    match &entries[at].action {
        Intent::Layer(target, LayerAction::Minimize) => assert_eq!(target.block, "block:f"),
        other => panic!("{other:?}"),
    }

    // Rebound: the hint follows the binding.
    bind(
        &mut ws,
        json!([{"keys":"ctrl+alt+m","action":"pane.minimize_layer"}]),
    );
    let (entries, at) = find_row(&ws, "Minimize layer");
    assert_eq!(
        entries[at].keys,
        [vec!["Ctrl".to_owned(), "Alt".to_owned(), "M".to_owned()]]
    );

    // Removed again: an unbound row keeps the reason it shows, and a bound one shows the key
    // instead, exactly as its Commands row does.
    bind(&mut ws, json!([]));
    let (entries, at) = find_row(&ws, "Minimize layer");
    assert!(entries[at].keys.is_empty());
    ws.hosts[0].model.views.get_mut("s").unwrap().windows[0].layers = vec![
        serde_json::from_value(tiled(&[])).unwrap(),
        serde_json::from_value(layer("floating", None, &["block:f"])).unwrap(),
    ];
    let (entries, at) = find_row(&ws, "Return pane to tiles");
    assert!(!entries[at].enabled, "still no tiled pane");
    assert_eq!(entries[at].detail, layers::Blocked::NoAnchor.hint());
    bind(
        &mut ws,
        json!([{"keys":"ctrl+shift+r","action":"pane.return_to_tiles"}]),
    );
    let (entries, at) = find_row(&ws, "Return pane to tiles");
    assert_eq!(
        entries[at].keys,
        [vec!["Ctrl".to_owned(), "Shift".to_owned(), "R".to_owned()]]
    );
    bind(&mut ws, json!([]));
    let (entries, at) = find_row(&ws, "Return pane to tiles");
    assert_eq!(entries[at].detail, layers::Blocked::NoAnchor.hint());
}

/// Draw the open list as it is now, so its rows have drawn rectangles and the pointer's hover
/// has run. One second of frames settles the popup's entry and the keyboard's scroll.
fn draw(rt: &mut Runtime<Workspace>) {
    for _ in 0..60 {
        rt.frame(1. / 60.);
    }
}

/// The row `label` occupies in the open list, as Enter's selection sees it.
fn row_index(rt: &Runtime<Workspace>, label: &str) -> usize {
    rt.state
        .entries()
        .iter()
        .position(|entry| entry.label == label)
        .unwrap_or_else(|| panic!("no {label} row"))
}

/// The drawn rectangle of the open list's row `label`, which is what the pointer can hit.
fn row_rect(rt: &Runtime<Workspace>, label: &str) -> [f32; 4] {
    let key = format!("entry:{}", row_index(rt, label));
    rt.elements()
        .into_iter()
        .find(|element| !element.exiting && element.key.as_deref() == Some(key.as_str()))
        .unwrap_or_else(|| panic!("{label} is not drawn"))
        .rect
}

/// Move the pointer onto `label`'s row, as a person moving the mouse over it does.
fn hover(rt: &mut Runtime<Workspace>, label: &str) {
    draw(rt);
    let [x, y, w, h] = row_rect(rt, label);
    rt.pointer_move(x + w / 2., y + h / 2.);
    draw(rt);
}

/// The floating layers in `ids` order, back to front, after the tiled layer.
fn reorder_layers(ws: &mut Workspace, ids: &[&str]) {
    let window = &mut ws.hosts[0]
        .model
        .views
        .get_mut("s")
        .expect("a view")
        .windows[0];
    let mut layers = std::mem::take(&mut window.layers);
    let tiled = layers
        .iter()
        .position(|layer| layer.layer_id.as_deref() == Some("tiled"))
        .expect("the tiled layer");
    let mut ordered = vec![layers.remove(tiled)];
    for id in ids {
        let at = layers
            .iter()
            .position(|layer| layer.layer_id.as_deref() == Some(*id))
            .unwrap_or_else(|| panic!("no {id} layer"));
        ordered.push(layers.remove(at));
    }
    assert!(layers.is_empty(), "every floating layer is named once");
    window.layers = ordered;
}

/// Three floating layers whose names share a search word, so a filtered Commands list is
/// short enough for every one of its rows to be drawn where the pointer can reach it.
fn tagged_layers() -> (Workspace, flume::Receiver<HostCommand>) {
    floating_workspace(
        13,
        vec![
            tiled(&["block:a"]),
            layer("alpha", None, &["block:z-alpha"]),
            layer("beta", None, &["block:z-beta"]),
            layer("gamma", None, &["block:z-gamma"]),
        ],
        "block:a",
    )
}

/// Commands filtered to the three tagged layer rows, with the keyboard on the list.
fn open_tagged_commands(rt: &mut Runtime<Workspace>) {
    rt.update(|s, _| {
        s.picker(false);
        let panel = s.panel.as_mut().expect("the Commands list");
        panel.inputs[0] = rune::TextEdit::new("show z-");
        panel.inputs[0].focused = true;
        s.query_changed();
    });
    draw(rt);
    let labels: Vec<String> = rt.state.entries().into_iter().map(|e| e.label).collect();
    assert_eq!(
        labels,
        ["Show z-alpha", "Show z-beta", "Show z-gamma"],
        "{labels:?}"
    );
}

/// F's layer submenu, opened from F's own pane menu with the keyboard, as a person does.
fn open_layer_menu_from_keyboard(rt: &mut Runtime<Workspace>) {
    rt.update(|s, _| s.pane_menu("block:f".into(), None));
    draw(rt);
    highlight(rt, "Floating layer…");
    press(rt, Key::Enter);
    assert!(
        matches!(
            rt.state.panel.as_ref().map(|panel| &panel.kind),
            Some(PanelKind::Layer)
        ),
        "the pane menu's layer row opens the layer submenu"
    );
    assert_eq!(highlighted(rt), "New floating pane");
}

fn move_f_to_other_window(ws: &mut Workspace) {
    let view = ws.hosts[0].model.views.get_mut("s").unwrap();
    let moved = view.windows[0].layers.remove(1);
    view.windows[1].layers.push(moved);
    view.windows.remove(0);
    view.windows[0].focused_block_id = Some("block:f".into());
    view.active_window_id = Some("other".into());
}

fn pane_actions_key(rt: &mut Runtime<Workspace>) {
    rt.key(KeyEvent {
        key: Key::Command(' '),
        mods: Mods {
            ctrl: true,
            shift: true,
            ..Mods::default()
        },
    });
    draw(rt);
}

#[test]
fn the_open_layer_menu_refuses_a_pane_another_client_moved_to_another_window() {
    let (mut ws, rx) = tiled_and_floating("block:a");
    ws.hosts[0]
        .model
        .views
        .get_mut("s")
        .expect("a view")
        .windows
        .push(
            serde_json::from_value(json!({
                "window_id": "next", "label": "next", "focused_block_id": "block:n",
                "layers": [tiled(&["block:n"])],
            }))
            .expect("a second window"),
        );
    let mut rt = runtime(ws);
    open_layer_menu_from_keyboard(&mut rt);
    assert_eq!(rt.state.panel.as_ref().unwrap().title, "dev · f");

    // Another client moves F into the other window and deletes the one the menu named. F is
    // still in this session, so the menu stays open; it must still mean the tab it named.
    rt.update(|s, _| {
        let view = s.hosts[0].model.views.get_mut("s").expect("a view");
        let moved = view.windows[0].layers.remove(1);
        view.windows[1].layers.push(moved);
        view.windows.remove(0);
        view.active_window_id = Some("next".into());
    });
    rt.update(|s, _| wake(s));
    draw(&mut rt);
    assert!(rt.state.panel.is_none(), "the captured window is gone");
    press(&mut rt, Key::Enter);
    let sent = requests(&rx);
    assert!(
        sent.is_empty(),
        "Enter must not start the pane in the window F moved to: {sent:?}"
    );
}

#[test]
fn the_open_layer_menu_refuses_a_new_epoch_and_a_pane_or_tab_that_is_gone() {
    for change in ["epoch", "pane", "tab"] {
        let (ws, rx) = tiled_and_floating("block:a");
        let mut rt = runtime(ws);
        open_layer_menu_from_keyboard(&mut rt);
        rt.update(|s, _| match change {
            "epoch" => s.hosts[0].model.connection_epoch += 1,
            "pane" => {
                s.hosts[0].model.views.get_mut("s").unwrap().windows[0].layers[1]
                    .blocks
                    .clear();
            }
            "tab" => {
                s.hosts[0]
                    .model
                    .views
                    .get_mut("s")
                    .unwrap()
                    .windows
                    .remove(0);
            }
            _ => unreachable!(),
        });
        rt.update(|s, _| wake(s));
        draw(&mut rt);
        assert!(rt.state.panel.is_none(), "{change}: the submenu closes");
        press(&mut rt, Key::Enter);
        let sent = requests(&rx);
        assert!(
            sent.is_empty(),
            "{change}: an invalidated menu must send nothing: {sent:?}"
        );
    }
}

#[test]
fn the_open_layer_menu_still_starts_in_its_own_tab_when_another_client_raises_another() {
    let (ws, rx) = tiled_and_floating("block:a");
    let mut rt = runtime(ws);
    open_layer_menu_from_keyboard(&mut rt);
    assert_eq!(rt.state.panel.as_ref().unwrap().title, "dev · f");

    // Another client raises the other tab. F never moved, so the menu's own tab still exists:
    // its row runs, and it starts the pane in that tab, not the one now on screen.
    rt.update(|s, _| {
        let view = s.hosts[0].model.views.get_mut("s").expect("a view");
        view.active_window_id = Some("other".into());
        view.windows[1].focused_block_id = Some("block:o".into());
    });
    rt.update(|s, _| wake(s));
    draw(&mut rt);
    assert_eq!(rt.state.panel.as_ref().unwrap().title, "dev · f");
    assert!(find_row(&rt.state, "New floating pane").0[0].enabled);
    press(&mut rt, Key::Enter);
    let sent = requests(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].1, "s");
    assert_eq!(
        sent[0].2["window_id"], "dev",
        "the menu's tab, not the one raised"
    );
    assert_eq!(sent[0].2["cwd_from"], "block:f");
}

#[test]
fn a_pointer_choice_survives_a_reorder_and_enter_runs_the_row_it_lit() {
    let (ws, rx) = tagged_layers();
    let mut rt = runtime(ws);
    open_tagged_commands(&mut rt);
    highlight(&mut rt, "Show z-beta");
    let beta_before = row_index(&rt, "Show z-beta");
    assert_eq!(
        row_index(&rt, "Show z-alpha") + 1,
        beta_before,
        "the three layer rows are adjacent, alpha first"
    );

    // The pointer takes the selection to alpha; the keyboard's row no longer owns it.
    hover(&mut rt, "Show z-alpha");
    assert_eq!(highlighted(&rt), "Show z-alpha");

    // Another client reorders the layers to [gamma, alpha, beta]; alpha now sits exactly where
    // beta's keyboard identity was recorded.
    rt.pointer_move(0., 0.);
    rt.update(|s, _| reorder_layers(s, &["gamma", "alpha", "beta"]));
    rt.update(|s, _| wake(s));
    draw(&mut rt);
    assert_eq!(
        row_index(&rt, "Show z-alpha"),
        beta_before,
        "a stale keyboard identity would now point at alpha"
    );
    hover(&mut rt, "Show z-alpha");
    assert_eq!(highlighted(&rt), "Show z-alpha");
    press(&mut rt, Key::Enter);
    let sent = requests(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(
        sent[0].2["layer_id"], "alpha",
        "Enter ran the row the pointer lit, not the one the keyboard named earlier"
    );
}

#[test]
fn a_pointer_choice_on_the_keyboard_row_survives_its_removal() {
    let (ws, rx) = tagged_layers();
    let mut rt = runtime(ws);
    open_tagged_commands(&mut rt);
    highlight(&mut rt, "Show z-beta");
    let beta = row_index(&rt, "Show z-beta");

    // The pointer lands on the row the keyboard already had: the pointer owns the selection
    // now, so the row's keyboard identity is retired.
    hover(&mut rt, "Show z-beta");
    assert_eq!(highlighted(&rt), "Show z-beta");

    // Another client removes beta; gamma takes its place under the same highlight.
    rt.pointer_move(0., 0.);
    rt.update(|s, _| {
        let window = &mut s.hosts[0].model.views.get_mut("s").unwrap().windows[0];
        window
            .layers
            .retain(|layer| layer.layer_id.as_deref() != Some("beta"));
    });
    rt.update(|s, _| wake(s));
    draw(&mut rt);
    assert_eq!(row_index(&rt, "Show z-gamma"), beta);
    assert_eq!(
        highlighted(&rt),
        "Show z-gamma",
        "the highlight shows the row now at that index"
    );
    press(&mut rt, Key::Enter);
    let sent = requests(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(
        sent[0].2["layer_id"], "gamma",
        "Enter ran the visible row, never the removed one"
    );
}

#[test]
fn a_pointer_hover_after_a_removal_runs_the_row_it_lit_not_the_removed_one() {
    let (ws, rx) = tagged_layers();
    let mut rt = runtime(ws);
    open_tagged_commands(&mut rt);
    highlight(&mut rt, "Show z-beta");
    let beta = row_index(&rt, "Show z-beta");
    hover(&mut rt, "Show z-alpha");
    assert_eq!(highlighted(&rt), "Show z-alpha");

    rt.pointer_move(0., 0.);
    rt.update(|s, _| {
        let window = &mut s.hosts[0].model.views.get_mut("s").unwrap().windows[0];
        window
            .layers
            .retain(|layer| layer.layer_id.as_deref() != Some("beta"));
    });
    rt.update(|s, _| wake(s));
    draw(&mut rt);
    assert_eq!(row_index(&rt, "Show z-gamma"), beta);
    hover(&mut rt, "Show z-gamma");
    assert_eq!(highlighted(&rt), "Show z-gamma");
    press(&mut rt, Key::Enter);
    let sent = requests(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(
        sent[0].2["layer_id"], "gamma",
        "the hovered row ran; the removed beta was never retried"
    );
}
