//! Floating layers: the pane-menu and Commands verbs of #136's first slice, and the one request
//! each sends. The server owns membership, geometry, order and state; this module reads them
//! from the cached session view and asks for one change at a time. It never invents layout,
//! never changes a value optimistically, and never retargets: a captured layer that no longer
//! matches the view is reported, with nothing sent. Pointer drag and edge resize are not here.
use super::{
    activity,
    panel::{LayerAction, LayerTarget, PaneTarget, Panel, PanelKind},
    workspace::Workspace,
};
use rex_rune::{connection::HostCommand, layers};
use rexrs::api::session::{Layer, Window};
use rune::Tx;
use serde_json::{Value, json};
use std::collections::HashSet;

impl Workspace {
    /// The reason floating-layer actions cannot run on the host on screen, decided from the
    /// negotiated revision before anything is sent.
    pub fn layer_gate(&self) -> Option<layers::Blocked> {
        let model = self.model();
        layers::unsupported(model.connected, model.protocol.api_revision)
    }

    /// The layer holding the pane of a captured target, as a target for a layer verb.
    pub(super) fn capture_pane_layer(&self, target: &PaneTarget) -> Option<LayerTarget> {
        let window = self.pane_window(target)?;
        let layer = layers::layer_of(window, &target.block)?;
        self.layer_target_in(target.host, &target.session, window, layer, &target.block)
    }

    /// The layer holding the focused pane of the tab on screen.
    pub fn focused_layer(&self) -> Option<LayerTarget> {
        let session = self.model().selected.clone()?;
        let window = self.model().window()?;
        let block = window.focused_block_id.clone()?;
        let layer = layers::layer_of(window, &block)?;
        self.layer_target_in(self.active, &session, window, layer, &block)
    }

    /// The floating layers of the tab on screen, back to front, named: the list Commands shows
    /// so a minimized layer can be restored without its hidden pane's menu. Only the tab on
    /// screen; nothing is read in the background for it.
    pub fn floating_layer_targets(&self) -> Vec<LayerTarget> {
        let Some(session) = self.model().selected.clone() else {
            return Vec::new();
        };
        let Some(window) = self.model().window() else {
            return Vec::new();
        };
        let mut targets = Vec::new();
        for layer in layers::floating_layers(window) {
            let Some(block) = layer.blocks.first().and_then(|b| b.block_id.clone()) else {
                continue;
            };
            if let Some(target) = self.layer_target_in(self.active, &session, window, layer, &block)
            {
                targets.push(target);
            }
        }
        targets
    }

    pub(super) fn layer_target_in(
        &self,
        host: usize,
        session: &str,
        window: &Window,
        layer: &Layer,
        block: &str,
    ) -> Option<LayerTarget> {
        let state = self.hosts.get(host)?;
        let generated = self.generated_titles();
        let panes: Vec<_> = layer
            .blocks
            .iter()
            .map(|b| {
                Some((
                    b.block_id.clone()?,
                    activity::pane_name(&state.model, b, generated),
                ))
            })
            .collect::<Option<_>>()?;
        let pane = panes.iter().find(|(id, _)| id == block)?.1.clone();
        Some(LayerTarget {
            host,
            host_id: state.id,
            epoch: state.model.connection_epoch,
            session: session.into(),
            window: window.window_id.clone()?,
            layer: layer.layer_id.clone()?,
            block: block.into(),
            panes,
            host_name: self.settings.hosts[host].label.clone(),
            tab: activity::tab_name(&state.model, window, generated),
            pane,
            name: self.layer_name(host, layer),
            floating: layers::floating(layer),
            minimized: layers::minimized(layer),
        })
    }

    /// The layer as the menu names it: its first pane, and how many more it holds.
    fn layer_name(&self, host: usize, layer: &Layer) -> String {
        let generated = self.generated_titles();
        let model = &self.hosts[host].model;
        let first = layer
            .blocks
            .first()
            .map(|b| activity::pane_name(model, b, generated))
            .unwrap_or_else(|| "Terminal".into());
        match layer.blocks.len() {
            0 | 1 => first,
            n => format!("{first} +{}", n - 1),
        }
    }

    pub(super) fn layer_host(&self, target: &LayerTarget) -> Option<usize> {
        self.hosts
            .get(target.host)
            .filter(|host| host.id == target.host_id)
            .map(|_| target.host)
            .or_else(|| self.host_index(target.host_id))
    }

    pub(super) fn captured_layer_gate(&self, target: &LayerTarget) -> Option<layers::Blocked> {
        let Some(index) = self.layer_host(target) else {
            return Some(layers::Blocked::Stale);
        };
        let model = &self.hosts[index].model;
        layers::unsupported(model.connected, model.protocol.api_revision)
    }

    pub(super) fn layer_window(&self, target: &LayerTarget) -> Option<&Window> {
        let model = &self.hosts[self.layer_host(target)?].model;
        if model.connection_epoch != target.epoch
            || !model
                .sessions
                .iter()
                .any(|s| s.session_id == target.session)
        {
            return None;
        }
        let view = model.views.get(&target.session)?;
        view.windows
            .iter()
            .find(|w| w.window_id.as_deref() == Some(target.window.as_str()))
    }

    pub(super) fn current_layer(&self, target: &LayerTarget) -> Option<&Layer> {
        let layer = layers::layer_of(self.layer_window(target)?, &target.block)?;
        (layer.layer_id.as_deref() == Some(&target.layer)).then_some(layer)
    }

    fn layer_context_blocked(&self, target: &LayerTarget) -> Option<layers::Blocked> {
        self.captured_layer_gate(target).or_else(|| {
            self.layer_window(target)
                .and_then(|w| layers::layer_of(w, &target.block))
                .is_none()
                .then_some(layers::Blocked::Stale)
        })
    }

    pub(super) fn layer_root_blocked(
        &self,
        panel: &Panel,
        target: &LayerTarget,
    ) -> Option<layers::Blocked> {
        if panel.layer_invalid || panel.layer.as_ref() != Some(target) {
            Some(layers::Blocked::Stale)
        } else {
            self.layer_context_blocked(target)
        }
    }

    pub(super) fn sync_layer_menu(&mut self, tx: &mut Tx) {
        let Some(panel) = &self.panel else { return };
        let Some(target) = &panel.layer else { return };
        if let Some(reason) = self.layer_context_blocked(target) {
            if matches!(panel.kind, PanelKind::Pane) {
                self.panel.as_mut().unwrap().layer_invalid = true;
            } else {
                self.close_panel(tx);
                self.inform(reason.sentence());
            }
        } else if matches!(panel.kind, PanelKind::Confirm { .. }) {
            let error = self
                .layer_blocked(Some(target), LayerAction::Close)
                .map(layers::Blocked::sentence);
            self.panel.as_mut().unwrap().error = error;
        }
    }

    /// What stops `action` on this target now, checked against the cached view. None means it
    /// can run. A captured layer that changed is [`layers::Blocked::Stale`], never a new target.
    pub fn layer_blocked(
        &self,
        target: Option<&LayerTarget>,
        action: LayerAction,
    ) -> Option<layers::Blocked> {
        let Some(target) = target else {
            if let Some(gate) = self.layer_gate() {
                return Some(gate);
            }
            return match action {
                LayerAction::NewFloating => self.new_floating_blocked(),
                _ => Some(layers::Blocked::NoPane),
            };
        };
        if let Some(reason) = self.layer_context_blocked(target) {
            return Some(reason);
        }
        // `New floating pane` from a captured menu starts its pane in that menu's tab and
        // folder, not the ones on screen now, so only that pane still has to be there: another
        // client may have changed which tab is active without invalidating the menu.
        if action == LayerAction::NewFloating {
            return None;
        }
        let window = self.layer_window(target).expect("validated window");
        let Some(layer) = self.current_layer(target) else {
            return Some(layers::Blocked::Stale);
        };
        let floating = layers::floating(layer);
        match action {
            LayerAction::NewFloating => None,
            LayerAction::Float => floating.then_some(layers::Blocked::AlreadyFloating),
            LayerAction::Minimize => {
                if !floating {
                    Some(layers::Blocked::NotFloating)
                } else if layers::minimized(layer) {
                    Some(layers::Blocked::Stale)
                } else if layers::holds_last_visible(window, layer) {
                    Some(layers::Blocked::LastVisible)
                } else {
                    None
                }
            }
            LayerAction::Focus => (!floating).then_some(layers::Blocked::NotFloating),
            LayerAction::Raise => {
                if !floating {
                    Some(layers::Blocked::NotFloating)
                } else if layers::frontmost(window).and_then(|l| l.layer_id.as_deref())
                    == Some(target.layer.as_str())
                {
                    Some(layers::Blocked::InFront)
                } else {
                    None
                }
            }
            LayerAction::ReturnToTiles => {
                if !floating {
                    Some(layers::Blocked::NotFloating)
                } else if layers::tiled_anchor(window, &target.block).is_none() {
                    Some(layers::Blocked::NoAnchor)
                } else {
                    None
                }
            }
            LayerAction::Close => {
                if !floating {
                    Some(layers::Blocked::NotFloating)
                } else {
                    let now: Option<HashSet<&str>> =
                        layer.blocks.iter().map(|b| b.block_id.as_deref()).collect();
                    let captured = target.panes.iter().map(|(id, _)| id.as_str()).collect();
                    (now != Some(captured)).then_some(layers::Blocked::Stale)
                }
            }
        }
    }

    fn new_floating_blocked(&self) -> Option<layers::Blocked> {
        let model = self.model();
        if model.selected.is_none() {
            return Some(layers::Blocked::NoTab);
        }
        match model.window() {
            Some(window) if window.window_id.is_some() => None,
            _ => Some(layers::Blocked::NoTab),
        }
    }

    /// Run a layer verb on the focused pane's layer: the Commands and shortcut path. `New
    /// floating pane` needs no pane, only a tab.
    pub fn focused_layer_action(&mut self, action: LayerAction) {
        if action == LayerAction::NewFloating {
            self.new_floating_pane();
            return;
        }
        let Some(target) = self.focused_layer() else {
            let blocked = self
                .layer_blocked(None, action)
                .unwrap_or(layers::Blocked::NoPane);
            self.inform(blocked.sentence());
            return;
        };
        self.layer_action(target, action);
    }

    /// Run a layer verb on a captured target once, or say why it did not. A refused or stale
    /// target sends nothing and is never retried.
    pub fn layer_action(&mut self, target: LayerTarget, action: LayerAction) {
        if let Some(blocked) = self.layer_blocked(Some(&target), action) {
            self.inform(blocked.sentence());
            return;
        }
        match action {
            // The menu's own tab, session and folder, never the ones on screen now.
            LayerAction::NewFloating => self.new_floating_layer(
                self.layer_host(&target).expect("validated host"),
                &target.session,
                &target.window,
                Some(&target.block),
            ),
            LayerAction::Float => self.layer_request(
                &target,
                "session.float_block",
                json!({"block_id":target.block,"bounds":layers::REQUESTED_BOUNDS}),
            ),
            // The server keeps the block and its program; only the state changes.
            LayerAction::Minimize => self.layer_request(
                &target,
                "session.set_layer_state",
                json!({"layer_id":target.layer,"state":"minimized"}),
            ),
            // One call restores or focuses and raises, in one revision: no second request.
            LayerAction::Focus => self.layer_request(
                &target,
                "session.set_layer_state",
                json!({"layer_id":target.layer,"state":"normal","focus":true}),
            ),
            // Bring to front never focuses, so it is named for raising alone.
            LayerAction::Raise => self.layer_request(
                &target,
                "session.raise_layer",
                json!({"layer_id":target.layer}),
            ),
            LayerAction::ReturnToTiles => {
                let anchor = self
                    .layer_window(&target)
                    .and_then(|window| layers::tiled_anchor(window, &target.block));
                let Some(anchor) = anchor else {
                    self.inform(layers::Blocked::NoAnchor.sentence());
                    return;
                };
                self.layer_request(
                    &target,
                    "session.move_block",
                    json!({
                        "block_id": target.block,
                        "anchor_block_id": anchor,
                        "direction": layers::RETURN_DIRECTION,
                        "side": layers::RETURN_SIDE,
                        "ratio": layers::RETURN_RATIO,
                    }),
                );
            }
            LayerAction::Close => self.confirm_layer_close(target),
        }
    }

    fn layer_request(&mut self, target: &LayerTarget, method: &str, payload: Value) {
        let mut payload = payload;
        payload["session_id"] = json!(target.session);
        self.send(
            self.layer_host(target).expect("validated host"),
            HostCommand::Request {
                method: method.into(),
                payload,
            },
        );
    }

    /// Ask before closing a floating layer: its name, every pane in it and what ends. Declining
    /// sends nothing; a target that changed by the time it is confirmed closes nothing.
    fn confirm_layer_close(&mut self, target: LayerTarget) {
        let title = format!("Close {}?", target.name);
        let description = close_words(&target);
        self.confirm(
            &title,
            "session.close_layer",
            json!({"session_id":target.session,"layer_id":target.layer}),
            &description,
        );
        self.panel.as_mut().unwrap().layer = Some(target);
    }

    /// The confirmed close: one `session.close_layer` for the layer the dialog named, and only
    /// while that layer is still the one the view shows.
    pub fn confirmed_layer_close(&mut self, target: &LayerTarget) {
        if let Some(blocked) = self.layer_blocked(Some(target), LayerAction::Close) {
            self.inform(blocked.sentence());
            return;
        }
        self.layer_request(
            target,
            "session.close_layer",
            json!({"layer_id":target.layer}),
        );
    }

    /// One new terminal in a new floating layer of the tab on screen, from the same creation
    /// path as a split: the host's shell and this client's theme, in the focused pane's folder.
    /// No tab, session or server is created, and no existing pane moves.
    pub fn new_floating_pane(&mut self) {
        if let Some(blocked) = self.layer_blocked(None, LayerAction::NewFloating) {
            self.inform(blocked.sentence());
            return;
        }
        let model = self.model();
        let (Some(session), Some(window)) = (model.selected.clone(), model.window()) else {
            self.inform(layers::Blocked::NoTab.sentence());
            return;
        };
        let Some(window_id) = window.window_id.clone() else {
            self.inform(layers::Blocked::NoTab.sentence());
            return;
        };
        let from = self.focused_block();
        self.new_floating_layer(self.active, &session, &window_id, from.as_deref());
    }

    /// One new terminal in a new floating layer of `window` on `host`, in the folder of the
    /// pane `from` names. Every caller passes a context it captured, so a wake or another
    /// client's tab switch between the choice and the request cannot move it.
    fn new_floating_layer(&mut self, host: usize, session: &str, window: &str, from: Option<&str>) {
        let layout = self.new_terminal(host);
        let payload = json!({
            "window_id": window,
            "bounds": layers::REQUESTED_BOUNDS,
            "layout": layout,
            "focus": true,
        });
        self.open_terminal(host, "session.new_layer", payload, session, from);
    }
}

/// The words a floating-layer close confirmation shows: every pane in the layer and what the
/// call ends, for a layer another client may have made with several panes.
fn close_words(target: &LayerTarget) -> String {
    let host = &target.host_name;
    let mut names = target.panes.iter().map(|(_, name)| name.as_str());
    match target.panes.len() {
        0 => format!(
            "Closes this empty floating layer on {host}. Closing the app itself keeps programs \
             running."
        ),
        1 => format!(
            "Ends the program in the floating pane {} on {host}. Closing the app itself keeps \
             programs running.",
            names.next().unwrap_or("Terminal")
        ),
        n => {
            let first = names.next().unwrap_or("Terminal");
            let rest: Vec<_> = names.collect();
            format!(
                "Ends the programs in {n} floating panes on {host}: {first}, {}. Closing the app \
                 itself keeps programs running.",
                rest.join(", ")
            )
        }
    }
}

#[cfg(test)]
#[test]
fn close_words_name_every_pane_and_the_host() {
    let target = |names: &[&str]| LayerTarget {
        host: 0,
        host_id: crate::app::workspace::HostId(0),
        epoch: 0,
        session: "s".into(),
        window: "w".into(),
        layer: "l".into(),
        block: "b".into(),
        panes: names
            .iter()
            .map(|n| (n.to_string(), n.to_string()))
            .collect(),
        host_name: "test-0".into(),
        tab: "tab".into(),
        pane: "one".into(),
        name: "one".into(),
        floating: true,
        minimized: false,
    };
    let one = close_words(&target(&["one"]));
    assert!(one.contains("floating pane one"), "{one}");
    assert!(one.contains("test-0"), "{one}");
    assert!(
        one.contains("Closing the app itself keeps programs running"),
        "{one}"
    );
    let three = close_words(&target(&["one", "two", "three"]));
    assert!(three.contains("3 floating panes"), "{three}");
    for name in ["one", "two", "three"] {
        assert!(three.contains(name), "{three}");
    }
    assert!(three.contains("Ends the programs"), "{three}");
}
