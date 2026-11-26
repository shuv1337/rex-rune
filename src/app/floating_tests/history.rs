use super::*;
use crate::app::workspace::HostId;
use rexrs::api::session::Window;

const ROOT: &str = "Floating layer…";
const NEW: &str = "New floating pane";
const CLOSE: &str = "Close floating layer…";
const FOCUS: &str = "Focus layer";

fn report(rt: &mut Runtime<Workspace>, change: impl FnOnce(&mut Workspace)) {
    rt.update(|s, tx| {
        change(s);
        *s.hosts[0].connection.model.lock().unwrap() = s.hosts[0].model.clone();
        s.sync_host(s.hosts[0].id, tx);
    });
    draw(rt);
}

fn tick(rt: &mut Runtime<Workspace>) {
    report(rt, |_| {});
}

fn window(s: &mut Workspace) -> &mut Window {
    &mut s.hosts[0].model.views.get_mut("s").unwrap().windows[0]
}

fn title(rt: &Runtime<Workspace>, expected: &str) {
    assert_eq!(rt.state.panel.as_ref().unwrap().title, expected);
}

fn row_enabled(rt: &Runtime<Workspace>, label: &str, enabled: bool) {
    let at = row_index(rt, label);
    assert_eq!(rt.state.entries()[at].enabled, enabled, "{label}");
    row_rect(rt, label);
}

fn stale_root(rt: &Runtime<Workspace>) {
    assert!(matches!(&rt.state.panel, Some(p) if matches!(p.kind, PanelKind::Pane)));
    row_enabled(rt, ROOT, false);
    assert_eq!(
        rt.state.entries()[row_index(rt, ROOT)].detail,
        "Layer changed"
    );
}

fn choose_row(rt: &mut Runtime<Workspace>, label: &str) {
    highlight(rt, label);
    draw(rt);
    row_rect(rt, label);
    press(rt, Key::Enter);
    draw(rt);
}

fn click_row(rt: &mut Runtime<Workspace>, label: &str) {
    draw(rt);
    click(rt, &format!("entry:{}", row_index(rt, label)));
    draw(rt);
}

struct History {
    rt: Runtime<Workspace>,
    intended: flume::Receiver<HostCommand>,
    decoy: flume::Receiver<HostCommand>,
}

impl History {
    fn from_fixture(mut s: Workspace, intended: flume::Receiver<HostCommand>) -> Self {
        let mut other = workspace_hosts(2);
        other.hosts[1].id = HostId(202);
        let decoy = capture_host(&mut other, 1);
        let mut state = other.hosts.remove(1);
        state.model = s.hosts[0].model.clone();
        state.model.connection_epoch = 18;
        s.hosts.push(state);
        s.settings.hosts.push(other.settings.hosts.remove(1));
        s.hosts[0].id = HostId(101);
        s.settings.hosts[0].label = "Opening host".into();
        Self {
            rt: runtime(s),
            intended,
            decoy,
        }
    }

    fn pane(blocks: &[&str]) -> Self {
        let (s, rx) = floating_workspace(
            13,
            vec![tiled(&["block:a"]), layer("floating", None, blocks)],
            "block:f",
        );
        Self::from_fixture(s, rx)
    }

    fn root(&mut self, name: &str) {
        tick(&mut self.rt);
        pane_actions_key(&mut self.rt);
        tick(&mut self.rt);
        title(&self.rt, name);
        row_enabled(&self.rt, ROOT, true);
        self.zero();
    }

    fn submenu(&mut self, name: &str) {
        tick(&mut self.rt);
        choose_row(&mut self.rt, ROOT);
        tick(&mut self.rt);
        title(&self.rt, name);
        assert!(matches!(
            self.rt.state.panel.as_ref().unwrap().kind,
            PanelKind::Layer
        ));
        assert_eq!(highlighted(&self.rt), NEW);
        self.zero();
    }

    fn confirm(&mut self) {
        tick(&mut self.rt);
        choose_row(&mut self.rt, CLOSE);
        tick(&mut self.rt);
        self.confirm_words();
        self.zero();
    }

    fn confirm_words(&self) {
        title(&self.rt, "Close f +1?");
        let PanelKind::Confirm { description, .. } = &self.rt.state.panel.as_ref().unwrap().kind
        else {
            panic!("no layer confirmation");
        };
        assert_eq!(
            description,
            "Ends the programs in 2 floating panes on Opening host: f, g. Closing the app itself keeps programs running."
        );
        assert!(focused(&self.rt).unwrap().ends_with("cancel-close"));
        assert!(
            self.rt
                .elements()
                .iter()
                .any(|e| { !e.exiting && e.focused && e.label.as_deref() == Some("Keep running") })
        );
        assert!(has(&self.rt, "confirm-close"));
    }

    fn zero(&self) {
        assert_eq!(requests(&self.intended), []);
        assert_eq!(requests(&self.decoy), []);
    }

    fn recipient(&self) {
        assert_eq!(self.rt.state.hosts[0].id, HostId(101));
        assert_eq!(self.rt.state.hosts[0].model.connection_epoch, 7);
    }

    fn sent(&self, method: &str, payload: Value) {
        self.recipient();
        assert_eq!(requests(&self.decoy), []);
        assert_eq!(
            requests(&self.intended),
            [(method.to_owned(), "s".to_owned(), payload)]
        );
    }

    fn new_sent(&self, tab: &str) {
        self.recipient();
        assert_eq!(requests(&self.decoy), []);
        let sent: Vec<_> = self
            .intended
            .try_iter()
            .filter(|command| {
                matches!(
                    command,
                    HostCommand::Request { .. }
                        | HostCommand::NewTerminal { .. }
                        | HostCommand::Navigate { .. }
                )
            })
            .collect();
        let [
            HostCommand::NewTerminal {
                method,
                payload,
                cwd_from,
            },
        ] = sent.as_slice()
        else {
            panic!(
                "expected exactly one deferred creation, got {} commands",
                sent.len()
            );
        };
        assert_eq!(method, "session.new_layer");
        assert_eq!(payload["session_id"], "s");
        assert_eq!(payload["window_id"], tab);
        assert_eq!(cwd_from, "block:f");
        assert_eq!(
            keys(payload),
            ["bounds", "focus", "layout", "session_id", "window_id"]
        );
    }

    fn gone(&self) {
        assert!(self.rt.state.panel.is_none(), "lineage should end");
    }
}

#[test]
fn fresh_root_after_move_names_and_creates_in_the_new_window() {
    let mut h = History::pane(&["block:f"]);
    report(&mut h.rt, move_f_to_other_window);
    h.root("other · f");
    h.submenu("other · f");
    press(&mut h.rt, Key::Enter);
    tick(&mut h.rt);
    h.gone();
    h.new_sent("other");
}

#[test]
fn the_root_menu_cannot_recapture_a_pane_moved_to_another_window() {
    let mut h = History::pane(&["block:f"]);
    h.root("dev · f");
    highlight(&mut h.rt, ROOT);
    report(&mut h.rt, move_f_to_other_window);
    title(&h.rt, "dev · f");
    stale_root(&h.rt);
    assert_eq!(highlighted(&h.rt), ROOT);
    press(&mut h.rt, Key::Enter);
    tick(&mut h.rt);
    stale_root(&h.rt);
    h.zero();
}

#[test]
fn move_after_submenu_dismisses_instead_of_creating_in_the_new_window() {
    let mut h = History::pane(&["block:f"]);
    h.root("dev · f");
    h.submenu("dev · f");
    report(&mut h.rt, move_f_to_other_window);
    h.gone();
    assert!(notices(&h.rt.state).contains(&layers::Blocked::Stale.sentence().as_str()));
    press(&mut h.rt, Key::Enter);
    tick(&mut h.rt);
    h.zero();
}

#[derive(Clone, Copy, Debug)]
enum Loss {
    Host,
    Epoch,
    Offline,
    Session,
    Window,
    Pane,
}

impl Loss {
    const ALL: [Self; 6] = [
        Self::Host,
        Self::Epoch,
        Self::Offline,
        Self::Session,
        Self::Window,
        Self::Pane,
    ];

    fn apply(self, s: &mut Workspace) {
        let host = &mut s.hosts[0];
        match self {
            Self::Host => host.id = HostId(303),
            Self::Epoch => host.model.connection_epoch = 8,
            Self::Offline => host.model.connected = false,
            Self::Session => host.model.sessions.clear(),
            Self::Window => {
                host.model.views.get_mut("s").unwrap().windows.remove(0);
            }
            Self::Pane => window(s).layers.remove(1).blocks.clear(),
        }
    }
}

#[test]
fn disconnect_does_not_put_close_pane_under_the_highlighted_layer_row() {
    let mut h = History::pane(&["block:f"]);
    h.root("dev · f");
    highlight(&mut h.rt, ROOT);
    let selected = row_index(&h.rt, ROOT);
    report(&mut h.rt, |s| Loss::Offline.apply(s));
    let row =
        h.rt.state
            .entries()
            .get(selected)
            .map(|e| (e.label.clone(), e.enabled, e.detail.clone()));
    press(&mut h.rt, Key::Enter);
    tick(&mut h.rt);
    h.zero();
    assert!(
        matches!(&h.rt.state.panel, Some(p) if matches!(p.kind, PanelKind::Pane)),
        "Enter replaced the pane menu with {:?}; row at the original index: {row:?}",
        h.rt.state.panel.as_ref().map(|p| &p.kind)
    );
    assert_eq!(row, Some((ROOT.into(), false, "Layer changed".into())));
    row_enabled(&h.rt, ROOT, false);
}

#[test]
fn identity_losses_on_both_sides_of_submenu_entry_and_dispatch_never_retarget_or_replay() {
    for loss in Loss::ALL {
        let mut h = History::pane(&["block:f"]);
        h.root("dev · f");
        highlight(&mut h.rt, ROOT);
        report(&mut h.rt, |s| loss.apply(s));
        if h.rt.state.panel.is_some() {
            title(&h.rt, "dev · f");
            stale_root(&h.rt);
            press(&mut h.rt, Key::Enter);
            stale_root(&h.rt);
        }
        tick(&mut h.rt);
        h.zero();

        let mut h = History::pane(&["block:f"]);
        h.root("dev · f");
        h.submenu("dev · f");
        report(&mut h.rt, |s| loss.apply(s));
        h.gone();
        press(&mut h.rt, Key::Enter);
        tick(&mut h.rt);
        h.zero();

        let mut h = History::pane(&["block:f"]);
        h.root("dev · f");
        h.submenu("dev · f");
        press(&mut h.rt, Key::Enter);
        h.new_sent("dev");
        report(&mut h.rt, |s| loss.apply(s));
        h.gone();
        h.zero();
    }
}

#[test]
fn offline_observation_cannot_revive_a_root_even_when_its_context_returns() {
    let mut h = History::pane(&["block:f"]);
    h.root("dev · f");
    report(&mut h.rt, |s| s.hosts[0].model.connected = false);
    stale_root(&h.rt);
    report(&mut h.rt, |s| s.hosts[0].model.connected = true);
    stale_root(&h.rt);
    click_row(&mut h.rt, ROOT);
    tick(&mut h.rt);
    h.zero();
}

#[test]
fn a_root_opened_without_floating_support_keeps_the_branch_hidden() {
    for connected in [false, true] {
        let mut h = History::pane(&["block:f"]);
        report(&mut h.rt, |s| {
            s.hosts[0].model.connected = connected;
            s.hosts[0].model.protocol.api_revision = Some(if connected { 12 } else { 13 });
        });
        pane_actions_key(&mut h.rt);
        title(&h.rt, "dev · f");
        assert!(h.rt.state.entries().iter().all(|e| e.label != ROOT));
        report(&mut h.rt, |s| {
            s.hosts[0].model.connected = true;
            s.hosts[0].model.protocol.api_revision = Some(13);
        });
        title(&h.rt, "dev · f");
        assert!(h.rt.state.entries().iter().all(|e| e.label != ROOT));
        h.zero();
    }
}

#[test]
fn pane_moved_inside_its_window_leaves_only_new_floating_eligible() {
    for delete in [false, true] {
        let mut h = History::pane(&["block:f", "block:g"]);
        h.root("dev · f");
        h.submenu("dev · f");
        highlight(&mut h.rt, FOCUS);
        report(&mut h.rt, |s| {
            let w = window(s);
            w.layers[1] = serde_json::from_value(layer("floating", None, &["block:g"])).unwrap();
            if delete {
                w.layers.remove(1);
            }
            w.layers
                .push(serde_json::from_value(layer("replacement", None, &["block:f"])).unwrap());
        });
        for label in [
            "Minimize layer",
            FOCUS,
            "Bring layer to front",
            "Return pane to tiles",
            CLOSE,
        ] {
            row_enabled(&h.rt, label, false);
        }
        assert_eq!(highlighted(&h.rt), FOCUS);
        press(&mut h.rt, Key::Enter);
        tick(&mut h.rt);
        h.zero();
        row_enabled(&h.rt, NEW, true);
        press(&mut h.rt, Key::Up);
        assert_eq!(highlighted(&h.rt), NEW);
        press(&mut h.rt, Key::Enter);
        tick(&mut h.rt);
        h.gone();
        h.new_sent("dev");
    }
}

fn membership(s: &mut Workspace, panes: &[&str]) {
    window(s).layers[1] = serde_json::from_value(layer("floating", None, panes)).unwrap();
}

#[test]
fn membership_between_root_and_submenu_blocks_close_but_preserves_focus_recipient() {
    for (before, after) in [
        (&["block:f"][..], &["block:f", "block:g"][..]),
        (&["block:f", "block:g"][..], &["block:f"][..]),
    ] {
        let mut h = History::pane(before);
        h.root("dev · f");
        report(&mut h.rt, |s| membership(s, after));
        h.submenu("dev · f");
        row_enabled(&h.rt, CLOSE, false);
        h.zero();
        row_enabled(&h.rt, FOCUS, true);
        choose_row(&mut h.rt, FOCUS);
        tick(&mut h.rt);
        h.gone();
        h.sent(
            "session.set_layer_state",
            json!({
                "session_id":"s", "layer_id":"floating", "state":"normal", "focus":true
            }),
        );
    }
}

#[test]
fn missing_identity_after_submenu_cannot_open_close_confirmation() {
    for loss in Loss::ALL {
        let mut h = History::pane(&["block:f", "block:g"]);
        h.root("dev · f");
        h.submenu("dev · f");
        highlight(&mut h.rt, CLOSE);
        report(&mut h.rt, |s| loss.apply(s));
        h.gone();
        press(&mut h.rt, Key::Enter);
        tick(&mut h.rt);
        h.zero();
    }
}

#[test]
fn changed_membership_before_or_after_close_choice_requires_new_consent() {
    for before_confirm in [true, false] {
        let mut h = History::pane(&["block:f", "block:g"]);
        h.root("dev · f");
        h.submenu("dev · f");
        if before_confirm {
            highlight(&mut h.rt, CLOSE);
            report(&mut h.rt, |s| membership(s, &["block:f"]));
            row_enabled(&h.rt, CLOSE, false);
            press(&mut h.rt, Key::Enter);
            tick(&mut h.rt);
            title(&h.rt, "dev · f");
        } else {
            h.confirm();
            report(&mut h.rt, |s| membership(s, &["block:f"]));
            h.confirm_words();
            click(&mut h.rt, "confirm-close");
            tick(&mut h.rt);
            h.confirm_words();
            h.zero();
            press(&mut h.rt, Key::Enter);
            tick(&mut h.rt);
            h.gone();
        }
        h.zero();
    }
}

fn benign(s: &mut Workspace) {
    let v = s.hosts[0].model.views.get_mut("s").unwrap();
    v.active_window_id = Some("other".into());
    let w = &mut v.windows[0];
    w.label = Some("Renamed tab".into());
    w.layers[1].blocks.reverse();
    w.layers[1]
        .blocks
        .iter_mut()
        .for_each(|b| b.label = Some("Renamed pane".into()));
    v.windows.swap(0, 1);
    s.settings.hosts[0].label = "Renamed host".into();
}

#[test]
fn confirmation_keeps_opening_names_and_accepts_reordered_ids_despite_remote_focus() {
    let mut h = History::pane(&["block:f", "block:g"]);
    h.root("dev · f");
    h.submenu("dev · f");
    h.confirm();
    report(&mut h.rt, benign);
    h.confirm_words();
    let dev = &h.rt.state.hosts[0].model.views["s"].windows[1];
    assert_eq!(dev.window_id.as_deref(), Some("dev"));
    assert_eq!(
        dev.layers[1]
            .blocks
            .iter()
            .map(|b| b.block_id.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["block:g", "block:f"]
    );
    click(&mut h.rt, "confirm-close");
    tick(&mut h.rt);
    h.gone();
    h.sent(
        "session.close_layer",
        json!({"session_id":"s", "layer_id":"floating"}),
    );
    tick(&mut h.rt);
    h.zero();
}

#[test]
fn pending_confirmation_dismisses_after_identity_loss() {
    for loss in Loss::ALL {
        let mut h = History::pane(&["block:f", "block:g"]);
        h.root("dev · f");
        h.submenu("dev · f");
        h.confirm();
        report(&mut h.rt, |s| loss.apply(s));
        h.gone();
        assert!(
            !h.rt
                .elements()
                .iter()
                .any(|e| !e.exiting && e.key.as_deref() == Some("confirm-close"))
        );
        press(&mut h.rt, Key::Enter);
        tick(&mut h.rt);
        h.zero();
    }
}

#[test]
fn keep_running_and_escape_end_the_entire_confirmation_lineage() {
    for cancel in [Key::Enter, Key::Escape] {
        let mut h = History::pane(&["block:f", "block:g"]);
        h.root("dev · f");
        h.submenu("dev · f");
        h.confirm();
        report(&mut h.rt, benign);
        h.confirm_words();
        press(&mut h.rt, cancel);
        tick(&mut h.rt);
        h.gone();
        h.zero();
    }
}

#[test]
fn submenu_escape_ends_the_lineage_without_restoring_the_root() {
    let mut h = History::pane(&["block:f", "block:g"]);
    h.root("dev · f");
    h.submenu("dev · f");
    report(&mut h.rt, benign);
    title(&h.rt, "dev · f");
    press(&mut h.rt, Key::Escape);
    tick(&mut h.rt);
    h.gone();
    h.zero();
}

#[test]
fn deliberate_root_reopen_after_dismiss_and_move_captures_fresh_context() {
    let mut h = History::pane(&["block:f"]);
    h.root("dev · f");
    h.submenu("dev · f");
    press(&mut h.rt, Key::Escape);
    report(&mut h.rt, move_f_to_other_window);
    h.gone();
    h.zero();
    h.root("other · f");
    h.submenu("other · f");
    press(&mut h.rt, Key::Enter);
    tick(&mut h.rt);
    h.new_sent("other");
}

#[test]
fn benign_wakes_on_either_side_of_submenu_open_keep_new_in_its_opening_window() {
    for before_submenu in [true, false] {
        let mut h = History::pane(&["block:f", "block:g"]);
        h.root("dev · f");
        if before_submenu {
            report(&mut h.rt, benign);
        }
        h.submenu("dev · f");
        if !before_submenu {
            report(&mut h.rt, benign);
        }
        title(&h.rt, "dev · f");
        row_enabled(&h.rt, NEW, true);
        press(&mut h.rt, Key::Enter);
        tick(&mut h.rt);
        h.gone();
        h.new_sent("dev");
    }
}

#[test]
fn changing_the_active_host_does_not_change_the_open_menus_eligibility_or_recipient() {
    let mut h = History::pane(&["block:f"]);
    h.root("dev · f");
    report(&mut h.rt, |s| {
        s.active = 1;
        s.hosts[1].model.protocol.api_revision = Some(12);
    });
    row_enabled(&h.rt, ROOT, true);
    h.submenu("dev · f");
    press(&mut h.rt, Key::Enter);
    tick(&mut h.rt);
    h.gone();
    h.new_sent("dev");
}

fn commands() -> History {
    let (s, rx) = tagged_layers();
    let mut h = History::from_fixture(s, rx);
    tick(&mut h.rt);
    h.rt.key(KeyEvent {
        key: Key::Command('p'),
        mods: Mods {
            ctrl: true,
            shift: true,
            ..Mods::default()
        },
    });
    draw(&mut h.rt);
    h.rt.paste("show z-");
    tick(&mut h.rt);
    let labels: Vec<_> = h.rt.state.entries().into_iter().map(|e| e.label).collect();
    assert_eq!(labels, ["Show z-alpha", "Show z-beta", "Show z-gamma"]);
    h.zero();
    h
}

fn remove_layer(s: &mut Workspace, id: &str) {
    window(s)
        .layers
        .retain(|l| l.layer_id.as_deref() != Some(id));
}

fn shown(h: &History, layer: &str) {
    h.sent(
        "session.set_layer_state",
        json!({
            "session_id":"s", "layer_id":layer, "state":"normal", "focus":true
        }),
    );
}

#[test]
fn commands_keyboard_follows_the_id_on_reorder_and_never_substitutes_on_removal() {
    for remove in [false, true] {
        let mut h = commands();
        highlight(&mut h.rt, "Show z-beta");
        assert_eq!(highlighted(&h.rt), "Show z-beta");
        report(&mut h.rt, |s| {
            if remove {
                remove_layer(s, "beta");
            } else {
                reorder_layers(s, &["beta", "gamma", "alpha"]);
            }
        });
        assert_eq!(
            highlighted(&h.rt),
            if remove {
                "Show z-gamma"
            } else {
                "Show z-beta"
            }
        );
        press(&mut h.rt, Key::Enter);
        tick(&mut h.rt);
        if remove {
            h.zero();
        } else {
            h.gone();
            shown(&h, "beta");
        }
    }
}

#[test]
fn commands_same_index_hover_retires_keyboard_id_before_and_after_inventory_changes() {
    for change_before_hover in [false, true] {
        for remove in [false, true] {
            let mut h = commands();
            highlight(&mut h.rt, "Show z-beta");
            let old_index = row_index(&h.rt, "Show z-beta");
            let change = |s: &mut Workspace| {
                if remove {
                    remove_layer(s, "beta");
                } else {
                    reorder_layers(s, &["gamma", "beta", "alpha"]);
                }
            };
            if change_before_hover {
                report(&mut h.rt, change);
            }
            let hover_label = if change_before_hover && remove {
                "Show z-gamma"
            } else {
                "Show z-beta"
            };
            assert_eq!(row_index(&h.rt, hover_label), old_index);
            hover(&mut h.rt, hover_label);
            assert_eq!(highlighted(&h.rt), hover_label);
            h.rt.pointer_move(0., 0.);
            if !change_before_hover {
                report(&mut h.rt, change);
            }
            if remove {
                assert_eq!(highlighted(&h.rt), "Show z-gamma");
            } else {
                report(&mut h.rt, |s| {
                    reorder_layers(s, &["beta", "alpha", "gamma"])
                });
                assert_eq!(highlighted(&h.rt), "Show z-alpha");
            }
            tick(&mut h.rt);
            press(&mut h.rt, Key::Enter);
            tick(&mut h.rt);
            h.gone();
            shown(&h, if remove { "gamma" } else { "alpha" });
        }
    }
    let mut h = commands();
    highlight(&mut h.rt, "Show z-gamma");
    hover(&mut h.rt, "Show z-gamma");
    h.rt.pointer_move(0., 0.);
    report(&mut h.rt, |s| remove_layer(s, "gamma"));
    assert_eq!(h.rt.state.entries().len(), 2);
    press(&mut h.rt, Key::Enter);
    tick(&mut h.rt);
    h.zero();
}

#[test]
fn commands_pointer_keeps_position_until_keyboard_takes_back_identity() {
    for take_keyboard in [false, true] {
        for remove in [false, true] {
            let mut h = commands();
            hover(&mut h.rt, "Show z-gamma");
            h.rt.pointer_move(0., 0.);
            report(&mut h.rt, |s| {
                reorder_layers(s, &["gamma", "alpha", "beta"])
            });
            hover(&mut h.rt, "Show z-beta");
            assert_eq!(highlighted(&h.rt), "Show z-beta");
            h.rt.pointer_move(0., 0.);
            tick(&mut h.rt);
            if take_keyboard {
                press(&mut h.rt, Key::Up);
                assert_eq!(highlighted(&h.rt), "Show z-alpha");
                report(&mut h.rt, |s| {
                    if remove {
                        remove_layer(s, "alpha");
                    } else {
                        reorder_layers(s, &["alpha", "beta", "gamma"]);
                    }
                });
                assert_eq!(
                    highlighted(&h.rt),
                    if remove {
                        "Show z-beta"
                    } else {
                        "Show z-alpha"
                    }
                );
            }
            press(&mut h.rt, Key::Enter);
            tick(&mut h.rt);
            if take_keyboard && remove {
                h.zero();
            } else {
                shown(&h, if take_keyboard { "alpha" } else { "beta" });
            }
        }
    }
}
