//! Floating layers, slice 2: pointer drag-move from a floating pane's header and 8-way
//! edge/corner resize on API 13 hosts.
//!
//! The server stays authoritative for every layer's bounds and z-order. During a gesture this
//! module draws only an inert accent ghost outline at the proposed rectangle; live cards and
//! PTY sizes never change speculatively. On release, the target's host ID, connection epoch,
//! session, window, layer membership, state and starting bounds are re-validated before a
//! single epoch-scoped [`HostCommand::ResizeLayer`] is dispatched. Any cancellation or
//! concurrent change clears the gesture, sends nothing, and never re-arms on later motion.
use super::{
    pane_header::full_bleed,
    panel::LayerTarget,
    panes,
    theme::{CARD_RADIUS, STILL, accent, rgb},
    workspace::{HostId, Workspace},
};
use rex_rune::{
    connection::HostCommand,
    layers::{self, Blocked, Grip, MIN_FLOATING_PT},
    settings::PaneDensity,
};
use rexrs::api::session::{Layer, Window};
use rune::{Cursor, El, Phase, PointerButton, Tx, el};

/// Pointer travel in window points before a floating header press turns into a drag-move.
/// Shorter presses stay clicks (focusing the pane without sending any layer request).
pub const HEADER_DRAG_SLOP: f32 = 4.0;

/// How far each resize grip reaches outside and inside a floating layer's card rectangle.
pub const GRIP_OUTSIDE: f32 = 4.0;
pub const GRIP_INSIDE: f32 = 3.0;
/// How far each L-shaped corner grip extends along both meeting edges. The L-shape keeps the
/// top-right corner's interior clear of the header's Close button (`y + 3..27`, `x + w - 32..8`).
pub const CORNER_ARM: f32 = 12.0;

/// One active or pending pointer drag/resize on a floating layer.
#[derive(Clone, Debug)]
pub struct LayerDrag {
    pub target: LayerTarget,
    pub grip: Grip,
    pub start_bounds: [f64; 4],
    pub proposed: [f64; 4],
    /// True once resize begins (on Down for edge/corner grips) or once a header drag crosses
    /// [`HEADER_DRAG_SLOP`]. Only an active drag draws a ghost outline or commits on release.
    pub active: bool,
    /// The pane whose header was grabbed (`Grip::Move`), so `OnHover` keeps it mounted.
    pub grabbed_block: Option<String>,
    /// True while a header press can still end as a click-to-focus (`!active`).
    pub focus_on_click: bool,
}

impl Workspace {
    /// Whether floating layers in the active tab can be grabbed for move or resize right now:
    /// connected host at API >= 13, no open panel, and a tab whose floating layers draw at
    /// their server bounds (not zoomed, not a lone full-bleed pane).
    pub(super) fn can_drag_floating_layers(&self) -> bool {
        if self.panel.is_some() || self.layer_gate().is_some() {
            return false;
        }
        let Some(window) = self.model().window() else {
            return false;
        };
        rex_rune::layout::full_window_zoom(window).is_none() && !full_bleed(self, window)
    }

    /// The block whose floating header is currently held by a pointer move gesture.
    pub(super) fn grabbed_header(&self) -> Option<&str> {
        self.layer_drag
            .as_ref()
            .and_then(|d| d.grabbed_block.as_deref())
    }

    /// End any floating-layer drag silently (Escape, pointer cancel, focus loss, panel open,
    /// host/session switch). Never sends a server request.
    pub fn cancel_layer_drag(&mut self) {
        self.layer_drag = None;
    }

    /// Re-validate an in-flight floating-layer drag after a host wake. If the connection, API
    /// revision, active session/tab, full-bleed state, layer identity, state, zoom or server
    /// bounds changed underneath the gesture, cancel it and inform the user once (when active
    /// and the cause is remote/epoch invalidation) without sending anything.
    pub(super) fn sync_layer_drag(&mut self) {
        let Some(drag) = self.layer_drag.as_ref() else {
            return;
        };
        if let Err(reason) = self.validate_layer_drag(drag) {
            let active = drag.active;
            self.layer_drag = None;
            if active && let Some(reason) = reason {
                self.inform(reason.sentence());
            }
        }
    }

    /// Start a floating-layer drag on `Phase::Down` only. Never called from `Phase::Move`.
    pub(super) fn begin_layer_drag(
        &mut self,
        host_id: HostId,
        layer_id: &str,
        block: &str,
        grip: Grip,
    ) {
        self.clear_focus_history_walk();
        self.layer_drag = None;
        if !self.can_drag_floating_layers() {
            return;
        }
        let Some(host) = self.host_index(host_id) else {
            return;
        };
        if host != self.active {
            return;
        }
        let [_, _, iw, ih] = panes::inner(self);
        if layers::min_fraction((iw, ih), MIN_FLOATING_PT).is_none() {
            return;
        }
        let model = &self.hosts[host].model;
        let Some(session) = model.selected.clone() else {
            return;
        };
        let Some(window) = model.window() else {
            return;
        };
        let Some(layer) = window
            .layers
            .iter()
            .find(|l| l.layer_id.as_deref() == Some(layer_id))
        else {
            return;
        };
        if !layers::floating(layer) || layers::minimized(layer) {
            return;
        }
        let Some(start_bounds) = layers::bounds(layer) else {
            return;
        };
        let Some(target) = self.layer_target_in(host, &session, window, layer, block) else {
            return;
        };
        self.drag = None;
        self.layer_drag = Some(LayerDrag {
            target,
            grip,
            start_bounds,
            proposed: start_bounds,
            active: grip != Grip::Move,
            grabbed_block: (grip == Grip::Move).then(|| block.to_owned()),
            focus_on_click: grip == Grip::Move,
        });
    }

    /// Advance an already-started floating-layer drag on `Phase::Move`. If the drag was
    /// cancelled (or never started), does nothing and never re-arms.
    pub(super) fn update_layer_drag(&mut self, dx: f32, dy: f32, tx: &mut Tx) {
        let Some(drag) = self.layer_drag.as_ref() else {
            return;
        };
        if let Err(reason) = self.validate_layer_drag(drag) {
            let active = drag.active;
            self.layer_drag = None;
            if active && let Some(reason) = reason {
                self.inform(reason.sentence());
            }
            return;
        }
        let [_, _, iw, ih] = panes::inner(self);
        let Some(min) = layers::min_fraction((iw, ih), MIN_FLOATING_PT) else {
            self.layer_drag = None;
            return;
        };
        let scale = f64::from(self.ui_scale());
        if !scale.is_finite() || scale <= 0.0 {
            self.layer_drag = None;
            return;
        }
        let drag = self.layer_drag.as_mut().expect("checked above");
        if !drag.active {
            if dx.hypot(dy) < HEADER_DRAG_SLOP {
                tx.live_only();
                return;
            }
            drag.active = true;
        }
        let delta = [
            f64::from(dx) / (f64::from(iw) * scale),
            f64::from(dy) / (f64::from(ih) * scale),
        ];
        drag.proposed = layers::dragged(drag.start_bounds, drag.grip, delta, min);
        tx.instant();
    }

    /// Commit or discard a floating-layer drag on `Phase::Up`. Always clears `layer_drag`
    /// immediately; dispatches at most one epoch-scoped [`HostCommand::ResizeLayer`].
    pub(super) fn finish_layer_drag(&mut self, dx: f32, dy: f32, inside: bool, tx: &mut Tx) {
        let Some(drag) = self.layer_drag.take() else {
            return;
        };
        if !drag.active {
            if drag.focus_on_click && inside {
                let id = &drag.target.block;
                if self.focused_block().as_deref() != Some(id.as_str()) {
                    self.exit_copy_mode();
                    if let Some(t) = self.terminals.get(&self.term_key(id)) {
                        let _ = t.connection.commands.focus();
                    }
                }
                tx.blur();
            }
            return;
        }
        let (host, raise_first) = match self.validate_layer_drag(&drag) {
            Ok(valid) => valid,
            Err(Some(reason)) => {
                self.inform(reason.sentence());
                return;
            }
            Err(None) => return,
        };
        let [_, _, iw, ih] = panes::inner(self);
        let Some(min) = layers::min_fraction((iw, ih), MIN_FLOATING_PT) else {
            return;
        };
        let scale = f64::from(self.ui_scale());
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let delta = [
            f64::from(dx) / (f64::from(iw) * scale),
            f64::from(dy) / (f64::from(ih) * scale),
        ];
        let proposed = layers::dragged(drag.start_bounds, drag.grip, delta, min);
        if proposed == drag.start_bounds {
            return;
        }
        let Some(bounds) = layers::sendable(proposed) else {
            return;
        };
        if bounds == drag.start_bounds {
            return;
        }
        self.send(
            host,
            HostCommand::ResizeLayer {
                epoch: drag.target.epoch,
                session: drag.target.session,
                layer: drag.target.layer,
                bounds,
                raise_first,
            },
        );
    }

    /// Validate that `drag` still targets the exact same connected API-13 host, active session
    /// and active tab, connection epoch, unzoomed non-full-bleed window, visible floating layer,
    /// member block and starting bounds, with no open panel. Returns `(host_index, raise_first)`
    /// when valid, `Err(None)` for silent local cancellation (panel open), or `Err(Some(reason))`
    /// for remote/epoch invalidation.
    fn validate_layer_drag(&self, drag: &LayerDrag) -> Result<(usize, bool), Option<Blocked>> {
        if self.panel.is_some() {
            return Err(None);
        }
        if let Some(gate) = self.captured_layer_gate(&drag.target) {
            return Err(Some(match gate {
                Blocked::Stale => Blocked::ChangedDuringDrag,
                other => other,
            }));
        }
        let Some(host) = self.layer_host(&drag.target) else {
            return Err(Some(Blocked::ChangedDuringDrag));
        };
        if host != self.active {
            return Err(Some(Blocked::ChangedDuringDrag));
        }
        let model = &self.hosts[host].model;
        if model.selected.as_deref() != Some(drag.target.session.as_str()) {
            return Err(Some(Blocked::ChangedDuringDrag));
        }
        let Some(window) = model
            .window()
            .filter(|w| w.window_id.as_deref() == Some(drag.target.window.as_str()))
        else {
            return Err(Some(Blocked::ChangedDuringDrag));
        };
        if rex_rune::layout::full_window_zoom(window).is_some() || full_bleed(self, window) {
            return Err(Some(Blocked::ChangedDuringDrag));
        }
        let Some(layer) = self.current_layer(&drag.target) else {
            return Err(Some(Blocked::ChangedDuringDrag));
        };
        if !layers::floating(layer) || layers::minimized(layer) {
            return Err(Some(Blocked::ChangedDuringDrag));
        }
        if layers::bounds(layer) != Some(drag.start_bounds) {
            return Err(Some(Blocked::ChangedDuringDrag));
        }
        let is_frontmost = layers::frontmost(window).and_then(|l| l.layer_id.as_deref())
            == Some(drag.target.layer.as_str());
        Ok((host, !is_frontmost))
    }
}

/// If `block`'s header should accept floating-layer drag-move, returns `(host_id, layer_id)`.
pub(super) fn draggable_header(
    s: &Workspace,
    window: &Window,
    block: &str,
) -> Option<(HostId, String)> {
    if !s.can_drag_floating_layers() {
        return None;
    }
    let layer = layers::layer_of(window, block)?;
    if !layers::floating(layer) || layers::minimized(layer) {
        return None;
    }
    layers::bounds(layer)?;
    let layer_id = layer.layer_id.clone()?;
    let host_id = s.hosts.get(s.active)?.id;
    Some((host_id, layer_id))
}

/// Attach a floating-layer drag handler (`Grip::Move` for headers, or an edge/corner grip).
pub(super) fn attach_drag(
    el: El,
    host_id: HostId,
    layer_id: String,
    block: String,
    grip: Grip,
) -> El {
    el.on_drag(move |s: &mut Workspace, g, tx| {
        if tx.button() != PointerButton::Primary {
            return;
        }
        match g.phase {
            Phase::Down => s.begin_layer_drag(host_id, &layer_id, &block, grip),
            Phase::Move => s.update_layer_drag(g.dx, g.dy, tx),
            Phase::Up => {
                if g.cancelled() {
                    s.cancel_layer_drag();
                } else {
                    let inside = g.x >= 0. && g.y >= 0. && g.x <= g.w && g.y <= g.h;
                    s.finish_layer_drag(g.dx, g.dy, inside, tx);
                }
            }
        }
    })
}

fn edge_cursor(grip: Grip) -> Cursor {
    match grip {
        Grip::North | Grip::South => Cursor::ResizeNs,
        Grip::East | Grip::West => Cursor::ResizeEw,
        Grip::NorthEast | Grip::SouthWest => Cursor::ResizeNesw,
        Grip::NorthWest | Grip::SouthEast => Cursor::ResizeNwse,
        Grip::Move => Cursor::Default,
    }
}

/// The 8-way edge and L-shaped corner resize grips for one visible floating layer, emitted
/// once per layer around its outer `card_rect(bounds)`.
pub(super) fn grips(s: &Workspace, layer: &Layer) -> Vec<El> {
    if !s.can_drag_floating_layers() || !layers::floating(layer) || layers::minimized(layer) {
        return Vec::new();
    }
    let Some(bounds) = layers::bounds(layer) else {
        return Vec::new();
    };
    let Some(layer_id) = layer.layer_id.as_deref() else {
        return Vec::new();
    };
    let Some(block) = layer.blocks.first().and_then(|b| b.block_id.as_deref()) else {
        return Vec::new();
    };
    let Some(host_id) = s.hosts.get(s.active).map(|h| h.id) else {
        return Vec::new();
    };
    let [x, y, w, h] = panes::card_rect(s, bounds);
    if w <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    let out = GRIP_OUTSIDE;
    let ins = GRIP_INSIDE;
    let band = out + ins;
    let cx = CORNER_ARM.min(w / 2.0);
    let cy = CORNER_ARM.min(h / 2.0);
    let mid_w = (w - 2.0 * cx).max(0.0);
    let mid_h = (h - 2.0 * cy).max(0.0);
    let v_arm = (cy - ins).max(0.0);

    let mut out_els = Vec::with_capacity(12);
    let mut push = |key: String, rect: [f32; 4], grip: Grip| {
        if rect[2] <= 0.0 || rect[3] <= 0.0 {
            return;
        }
        let strip = el()
            .key(key)
            .absolute(rect[0], rect[1])
            .size(rect[2], rect[3])
            .cursor(edge_cursor(grip))
            .preserve_focus_on_press(true)
            .motion(STILL);
        out_els.push(attach_drag(
            strip,
            host_id,
            layer_id.to_owned(),
            block.to_owned(),
            grip,
        ));
    };

    // Four edge bands between the corners (4 pt outside, 3 pt inside).
    push(
        format!("layer-grip:{layer_id}:n"),
        [x + cx, y - out, mid_w, band],
        Grip::North,
    );
    push(
        format!("layer-grip:{layer_id}:s"),
        [x + cx, y + h - ins, mid_w, band],
        Grip::South,
    );
    push(
        format!("layer-grip:{layer_id}:w"),
        [x - out, y + cy, band, mid_h],
        Grip::West,
    );
    push(
        format!("layer-grip:{layer_id}:e"),
        [x + w - ins, y + cy, band, mid_h],
        Grip::East,
    );

    // Four L-shaped corners (two non-overlapping strips each: horizontal arm covers the
    // corner intersection, vertical arm continues down/up along the vertical edge).
    push(
        format!("layer-grip:{layer_id}:nw:h"),
        [x - out, y - out, cx + out, band],
        Grip::NorthWest,
    );
    push(
        format!("layer-grip:{layer_id}:nw:v"),
        [x - out, y + ins, band, v_arm],
        Grip::NorthWest,
    );
    push(
        format!("layer-grip:{layer_id}:ne:h"),
        [x + w - cx, y - out, cx + out, band],
        Grip::NorthEast,
    );
    push(
        format!("layer-grip:{layer_id}:ne:v"),
        [x + w - ins, y + ins, band, v_arm],
        Grip::NorthEast,
    );
    push(
        format!("layer-grip:{layer_id}:sw:h"),
        [x - out, y + h - ins, cx + out, band],
        Grip::SouthWest,
    );
    push(
        format!("layer-grip:{layer_id}:sw:v"),
        [x - out, y + h - cy, band, v_arm],
        Grip::SouthWest,
    );
    push(
        format!("layer-grip:{layer_id}:se:h"),
        [x + w - cx, y + h - ins, cx + out, band],
        Grip::SouthEast,
    );
    push(
        format!("layer-grip:{layer_id}:se:v"),
        [x + w - ins, y + h - cy, band, v_arm],
        Grip::SouthEast,
    );

    out_els
}

/// The inert accent ghost outline drawn on top of all layers while a drag or resize is active.
pub(super) fn ghost(s: &Workspace) -> Option<El> {
    let drag = s.layer_drag.as_ref().filter(|d| d.active)?;
    let r = panes::card_rect(s, drag.proposed);
    if r[2] <= 0.0 || r[3] <= 0.0 {
        return None;
    }
    let radius = if s.settings.pane_density == PaneDensity::Compact {
        0.
    } else {
        CARD_RADIUS
    };
    let mut outline = el()
        .key(format!("layer-drag-ghost:{}", drag.target.layer))
        .absolute(r[0], r[1])
        .size(r[2], r[3])
        .border(1.5, rgb(accent()))
        .fill(rgb(accent()).alpha(0.06))
        .inert()
        .motion(STILL);
    if radius > 0. {
        outline = outline.radius(radius).continuous_corners();
    }
    Some(outline)
}
