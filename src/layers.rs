//! Floating layers: what the server's session view says about a window's layers, and the
//! refusals this client decides itself around the layer methods of API revision 13.
//!
//! `session.view` lists a window's layers from back to front, each with a `kind`, `bounds`,
//! `state`, layout and blocks; `session.float_block`, `session.new_layer`, `session.raise_layer`,
//! `session.set_layer_state`, `session.move_block` and `session.close_layer` change them. Those
//! methods were checked live at revision 13 only (see `docs/parity-2026-11/M4-floating-layers.md`),
//! so a host that negotiated 10–12 still attaches and renders its tiled layouts as before, and
//! gets no layer change from this client. [`unsupported`] and [`Blocked`] carry the words both
//! the rows and the notices use.
//!
//! Nothing here has a UI dependency, and nothing here invents layout: the view is the only
//! authority for which layer holds a block, which layers are visible, and what the layer's
//! bounds and order are. A layer kind this client does not know draws as floating and a state it
//! does not know draws as normal, as the server's documentation instructs.
use rexrs::api::session::{Layer, ViewLayoutNode, Window};

/// The API revision that advertises the layer methods and the view's layers.
pub const MIN_API_REVISION: u32 = 13;

/// The rectangle `Float pane` and `New floating pane` ask for, as `[x, y, w, h]` fractions of
/// the window: centred and inside the window at every size. It is the request's stated intent
/// only; the layer's real geometry is whatever the server reports afterwards.
pub const REQUESTED_BOUNDS: [f64; 4] = [0.15, 0.12, 0.7, 0.7];

/// The `direction`, `side` and `ratio` this client asks for when it returns a pane to the tiled
/// layer: beside the anchor, to its right. The server owns the resulting split.
pub const RETURN_DIRECTION: &str = "horizontal";
pub const RETURN_SIDE: &str = "after";
pub const RETURN_RATIO: f64 = 0.5;

/// The smallest floating layer size this client's pointer resize will shrink to, in pane-area
/// points. A layer another client already made smaller is never forced to grow.
pub const MIN_FLOATING_PT: (f32, f32) = (160.0, 96.0);

/// Which part of a floating layer a pointer gesture grabbed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    Move,
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

impl Grip {
    /// Every grip, move first, then the eight edges and corners.
    pub const ALL: [Self; 9] = [
        Self::Move,
        Self::North,
        Self::South,
        Self::East,
        Self::West,
        Self::NorthEast,
        Self::NorthWest,
        Self::SouthEast,
        Self::SouthWest,
    ];

    /// The eight resize grips around a floating layer's outer card rectangle.
    pub const RESIZE: [Self; 8] = [
        Self::North,
        Self::South,
        Self::East,
        Self::West,
        Self::NorthEast,
        Self::NorthWest,
        Self::SouthEast,
        Self::SouthWest,
    ];

    /// Short stable suffix for element keys.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::North => "n",
            Self::South => "s",
            Self::East => "e",
            Self::West => "w",
            Self::NorthEast => "ne",
            Self::NorthWest => "nw",
            Self::SouthEast => "se",
            Self::SouthWest => "sw",
        }
    }
}

/// A reason a floating-layer action did not run. The sentence is for a notice; the hint is the
/// few words a row's hint column can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// The host on screen is not connected.
    NotConnected,
    /// The host negotiated an API revision older than [`MIN_API_REVISION`].
    OldServer(u32),
    /// No pane on screen to name.
    NoPane,
    /// No tab on screen to put a floating layer in.
    NoTab,
    /// The captured layer (or the pane it named) is no longer what the view shows.
    Stale,
    /// The layer or its connection changed while a pointer move or resize was in flight.
    ChangedDuringDrag,
    /// The pane is already in a floating layer.
    AlreadyFloating,
    /// Minimizing would leave the window without a visible pane.
    LastVisible,
    /// The tiled layer has no block to return beside.
    NoAnchor,
    /// The layer is already in front of every other floating layer.
    InFront,
    /// The action needs a floating layer, and this one is tiled.
    NotFloating,
}

impl Blocked {
    /// What a notice says. It names the refusal, not a retry.
    pub fn sentence(self) -> String {
        match self {
            Self::NotConnected => "Not connected to this host.".into(),
            Self::OldServer(revision) => format!(
                "This host's Rex is API revision {revision}; floating panes need revision \
                 {MIN_API_REVISION}. Nothing was changed."
            ),
            Self::NoPane => "No pane on screen to act on.".into(),
            Self::NoTab => "Open a tab before adding a floating pane.".into(),
            Self::Stale => {
                "That floating layer changed since it was listed. Nothing was changed.".into()
            }
            Self::ChangedDuringDrag => {
                "That floating layer changed while you were moving it. Nothing was changed.".into()
            }
            Self::AlreadyFloating => {
                "This pane is already in a floating layer. Nothing was changed.".into()
            }
            Self::LastVisible => {
                "This layer holds the tab's only visible panes, and Rex keeps a tab's panes \
                 visible. Restore another layer or open another visible pane first."
                    .into()
            }
            Self::NoAnchor => {
                "This tab has no tiled pane to return beside, and this client returns a pane \
                 only beside a tiled one."
                    .into()
            }
            Self::InFront => "This layer is already in front.".into(),
            Self::NotFloating => "This layer is not a floating layer.".into(),
        }
    }

    /// What a disabled row's hint column says.
    pub fn hint(self) -> String {
        match self {
            Self::NotConnected => "Not connected".into(),
            Self::OldServer(revision) => {
                format!("Needs Rex API {MIN_API_REVISION}, not {revision}")
            }
            Self::NoPane => "No pane".into(),
            Self::NoTab => "No tab".into(),
            Self::Stale | Self::ChangedDuringDrag => "Layer changed".into(),
            Self::AlreadyFloating => "Already floating".into(),
            Self::LastVisible => "Only visible panes".into(),
            Self::NoAnchor => "No tiled pane beside it".into(),
            Self::InFront => "Already in front".into(),
            Self::NotFloating => "Not floating".into(),
        }
    }
}

/// Why this host cannot change floating layers: the negotiated revision, before anything is
/// sent. None while it can.
pub fn unsupported(connected: bool, api_revision: u32) -> Option<Blocked> {
    if !connected {
        return Some(Blocked::NotConnected);
    }
    (api_revision < MIN_API_REVISION).then_some(Blocked::OldServer(api_revision))
}

/// A layer the server's documented rule says to draw as floating: anything not `tiled`,
/// including a kind this client does not know.
pub fn floating(layer: &Layer) -> bool {
    layer.kind != "tiled"
}

/// A layer the server says not to draw. A state this client does not know draws as normal.
pub fn minimized(layer: &Layer) -> bool {
    layer.state.as_deref() == Some("minimized")
}

/// The window's tiled layer, the one that holds its layout and fills it.
pub fn tiled(window: &Window) -> Option<&Layer> {
    window.layers.iter().find(|layer| layer.kind == "tiled")
}

/// The layer whose blocks include `block`, as the view places it.
pub fn layer_of<'a>(window: &'a Window, block: &str) -> Option<&'a Layer> {
    window.layers.iter().find(|layer| {
        layer
            .blocks
            .iter()
            .any(|b| b.block_id.as_deref() == Some(block))
    })
}

/// The window's floating layers, back to front: the order the server draws them in.
pub fn floating_layers(window: &Window) -> impl Iterator<Item = &Layer> {
    window.layers.iter().filter(|layer| floating(layer))
}

/// The frontmost floating layer, which every other floating layer is behind.
pub fn frontmost(window: &Window) -> Option<&Layer> {
    floating_layers(window).last()
}

/// How many blocks the window would still show with every layer but its own rules applied:
/// the blocks of every layer that is not minimized. A tiled layer with no blocks contributes
/// none, so a window whose only panes float is counted from its floating layers.
pub fn visible_panes(window: &Window) -> usize {
    window
        .layers
        .iter()
        .filter(|layer| !minimized(layer))
        .map(|layer| layer.blocks.len())
        .sum()
}

/// Whether minimizing `layer` would leave the window without a visible pane, which the server
/// refuses: this layer is visible, holds panes, and every visible pane in the window is in it.
pub fn holds_last_visible(window: &Window, layer: &Layer) -> bool {
    !minimized(layer) && !layer.blocks.is_empty() && visible_panes(window) == layer.blocks.len()
}

fn first_block<'a>(node: &'a ViewLayoutNode, exclude: &str) -> Option<&'a str> {
    if let Some(id) = node.block_id.as_deref() {
        return (id != exclude).then_some(id);
    }
    let split = node.split.as_ref()?;
    first_block(split.before.as_deref()?, exclude)
        .or_else(|| first_block(split.after.as_deref()?, exclude))
}

/// The tiled block a floating pane returns beside: the first block of the window's tiled layout,
/// in layout order, never `block` itself. The server would accept an anchor in any layer; this
/// client's policy is a tiled one in the same window. None when the tiled layer has none.
pub fn tiled_anchor(window: &Window, block: &str) -> Option<String> {
    let layer = tiled(window)?;
    if let Some(node) = &layer.layout
        && let Some(found) = first_block(node, block)
    {
        return Some(found.to_owned());
    }
    // A view without a layout tree still lists the layer's blocks in layout order.
    layer
        .blocks
        .iter()
        .filter_map(|b| b.block_id.as_deref())
        .find(|id| *id != block)
        .map(str::to_owned)
}

/// The layer's bounds `[x, y, w, h]` as the server reported them: four finite fractions with
/// positive size and a non-negative origin. None when missing or malformed.
pub fn bounds(layer: &Layer) -> Option<[f64; 4]> {
    let rect: [f64; 4] = layer.bounds.as_slice().try_into().ok()?;
    let [x, y, w, h] = rect;
    (x.is_finite()
        && y.is_finite()
        && w.is_finite()
        && h.is_finite()
        && x >= 0.0
        && y >= 0.0
        && w > 0.0
        && h > 0.0)
        .then_some(rect)
}

/// The minimum `[w, h]` fraction for a pane area of `area_pt` points, capped at `1.0`. Returns
/// `None` for degenerate areas (`< 1.0` pt on either axis) or non-finite inputs.
pub fn min_fraction(area_pt: (f32, f32), min_pt: (f32, f32)) -> Option<[f64; 2]> {
    let (aw, ah) = area_pt;
    let (mw, mh) = min_pt;
    if !aw.is_finite()
        || !ah.is_finite()
        || !mw.is_finite()
        || !mh.is_finite()
        || aw < 1.0
        || ah < 1.0
        || mw < 0.0
        || mh < 0.0
    {
        return None;
    }
    Some([
        (f64::from(mw) / f64::from(aw)).clamp(0.0, 1.0),
        (f64::from(mh) / f64::from(ah)).clamp(0.0, 1.0),
    ])
}

/// Proposed `[x, y, w, h]` bounds after dragging `grip` from `start` by fractional `delta`,
/// clamped inside `[0.0, 1.0]` with opposite edges held fixed and `min` respected without
/// forcing a layer already smaller than `min` to grow.
pub fn dragged(start: [f64; 4], grip: Grip, delta: [f64; 2], min: [f64; 2]) -> [f64; 4] {
    let [sx, sy, sw, sh] = start;
    let [dx, dy] = delta;
    if !dx.is_finite() || !dy.is_finite() || (dx == 0.0 && dy == 0.0) {
        return start;
    }
    if grip == Grip::Move {
        let mut x = (sx + dx).clamp(0.0, (1.0 - sw).max(0.0));
        let mut y = (sy + dy).clamp(0.0, (1.0 - sh).max(0.0));
        // Keep `sw` and `sh` bit-identical while ensuring `x + sw <= 1.0` and `y + sh <= 1.0`.
        while x > 0.0 && x + sw > 1.0 {
            x = x.next_down();
        }
        while y > 0.0 && y + sh > 1.0 {
            y = y.next_down();
        }
        return [x, y, sw, sh];
    }

    let min_w = min[0].clamp(0.0, 1.0).min(sw);
    let min_h = min[1].clamp(0.0, 1.0).min(sh);
    let right = (sx + sw).min(1.0);
    let bottom = (sy + sh).min(1.0);

    let (x, mut w) = match grip {
        Grip::West | Grip::NorthWest | Grip::SouthWest => {
            if dx == 0.0 {
                (sx, sw)
            } else {
                let max_x = (right - min_w).max(0.0);
                let x = (sx + dx).clamp(0.0, max_x);
                let w = if x == sx { sw } else { (right - x).max(min_w) };
                (x, w)
            }
        }
        Grip::East | Grip::NorthEast | Grip::SouthEast => {
            if dx == 0.0 {
                (sx, sw)
            } else {
                let max_w = (1.0 - sx).max(min_w);
                let w = (sw + dx).clamp(min_w, max_w);
                (sx, w)
            }
        }
        Grip::North | Grip::South | Grip::Move => (sx, sw),
    };

    let (y, mut h) = match grip {
        Grip::North | Grip::NorthWest | Grip::NorthEast => {
            if dy == 0.0 {
                (sy, sh)
            } else {
                let max_y = (bottom - min_h).max(0.0);
                let y = (sy + dy).clamp(0.0, max_y);
                let h = if y == sy { sh } else { (bottom - y).max(min_h) };
                (y, h)
            }
        }
        Grip::South | Grip::SouthWest | Grip::SouthEast => {
            if dy == 0.0 {
                (sy, sh)
            } else {
                let max_h = (1.0 - sy).max(min_h);
                let h = (sh + dy).clamp(min_h, max_h);
                (sy, h)
            }
        }
        Grip::East | Grip::West | Grip::Move => (sy, sh),
    };

    while w > 0.0 && x + w > 1.0 {
        w = w.next_down();
    }
    while h > 0.0 && y + h > 1.0 {
        h = h.next_down();
    }
    [x, y, w, h]
}

/// Strictly validate and, if floating-point rounding put `x + w` or `y + h` above `1.0`,
/// step `w` or `h` down with [`f64::next_down`] so the server's unit-square check never rejects
/// the rectangle. Returns `None` if no valid positive rectangle inside `[0.0, 1.0]` remains.
pub fn sendable(bounds: [f64; 4]) -> Option<[f64; 4]> {
    let [x, y, mut w, mut h] = bounds;
    if !x.is_finite()
        || !y.is_finite()
        || !w.is_finite()
        || !h.is_finite()
        || x < 0.0
        || y < 0.0
        || x >= 1.0
        || y >= 1.0
        || w <= 0.0
        || h <= 0.0
    {
        return None;
    }
    if x + w > 1.0 {
        w = w.min(1.0 - x);
        while w > 0.0 && x + w > 1.0 {
            w = w.next_down();
        }
    }
    if y + h > 1.0 {
        h = h.min(1.0 - y);
        while h > 0.0 && y + h > 1.0 {
            h = h.next_down();
        }
    }
    (w > 0.0 && h > 0.0 && x + w <= 1.0 && y + h <= 1.0).then_some([x, y, w, h])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn window() -> Window {
        serde_json::from_value(json!({"window_id":"w","layers":[
            {"layer_id":"tiled","kind":"tiled","bounds":[0,0,1,1],
             "layout":{"split":{"direction":"horizontal","ratio":0.5,
                "before":{"block_id":"a"},"after":{"block_id":"b"}}},
             "blocks":[{"block_id":"a","label":"a"},{"block_id":"b","label":"b"}]},
            {"layer_id":"first","kind":"floating","bounds":[0.15,0.12,0.7,0.7],
             "layout":{"block_id":"f"},"blocks":[{"block_id":"f","label":"f"}]},
            {"layer_id":"second","kind":"stacked","bounds":[0.2,0.2,0.4,0.4],
             "layout":{"block_id":"g"},"blocks":[{"block_id":"g"}]}
        ]}))
        .unwrap()
    }

    #[test]
    fn the_gate_is_the_negotiated_revision_and_says_which_one() {
        assert_eq!(unsupported(false, 13), Some(Blocked::NotConnected));
        assert_eq!(unsupported(true, 10), Some(Blocked::OldServer(10)));
        assert_eq!(unsupported(true, 12), Some(Blocked::OldServer(12)));
        assert_eq!(unsupported(true, 13), None);
        assert_eq!(unsupported(true, 14), None);
        assert_eq!(MIN_API_REVISION, 13);
        // A row's hint and a notice's sentence both name the revision it needs, truthfully.
        for revision in 10..13 {
            let blocked = Blocked::OldServer(revision);
            assert!(blocked.hint().contains("13"), "{revision}");
            assert!(blocked.hint().contains(&revision.to_string()), "{revision}");
            assert!(
                blocked
                    .sentence()
                    .contains(&format!("API revision {revision}")),
                "{revision}"
            );
            assert!(blocked.sentence().contains("revision 13"), "{revision}");
        }
    }

    #[test]
    fn unknown_kinds_draw_as_floating_and_unknown_states_as_normal() {
        let window = window();
        let kinds: Vec<_> = floating_layers(&window)
            .map(|layer| layer.layer_id.as_deref().unwrap())
            .collect();
        assert_eq!(kinds, ["first", "second"]);
        assert_eq!(
            frontmost(&window).unwrap().layer_id.as_deref(),
            Some("second")
        );
        assert_eq!(tiled(&window).unwrap().layer_id.as_deref(), Some("tiled"));
        let mut later = window.clone();
        later.layers[2].state = Some("docked".into());
        assert!(!minimized(&later.layers[2]));
        assert_eq!(visible_panes(&later), 4);
        later.layers[2].state = Some("minimized".into());
        assert_eq!(visible_panes(&later), 3);
        // A minimized layer keeps its place in the stack: raising another still changes order.
        assert_eq!(
            frontmost(&later).unwrap().layer_id.as_deref(),
            Some("second")
        );
    }

    #[test]
    fn a_block_belongs_to_the_one_layer_the_view_places_it_in() {
        let window = window();
        let layer = |block: &str| {
            layer_of(&window, block)
                .and_then(|layer| layer.layer_id.clone())
                .unwrap_or_default()
        };
        assert_eq!(layer("a"), "tiled");
        assert_eq!(layer("g"), "second");
        assert_eq!(layer("missing"), "");
    }

    #[test]
    fn minimizing_counts_every_visible_pane_not_the_zoom_projection() {
        let mut window = window();
        assert!(!holds_last_visible(&window, &window.layers[1]));
        assert!(!holds_last_visible(&window, &window.layers[0]));
        // The tiled layer emptied by floating its last pane: the floating layer is all there is.
        window.layers[0].layout = None;
        window.layers[0].blocks.clear();
        window.layers[2].state = Some("minimized".into());
        assert!(holds_last_visible(&window, &window.layers[1]));
        // A zoomed block does not change which panes are visible.
        window.zoomed_block_id = Some("f".into());
        assert!(holds_last_visible(&window, &window.layers[1]));
        // A second visible pane in another layer ends that.
        window.layers[2].state = None;
        assert!(!holds_last_visible(&window, &window.layers[1]));
        // A minimized layer is never the last visible one.
        window.layers[1].state = Some("minimized".into());
        assert!(!holds_last_visible(&window, &window.layers[1]));
    }

    #[test]
    fn the_anchor_is_the_first_tiled_block_and_never_the_moving_pane() {
        let window = window();
        assert_eq!(tiled_anchor(&window, "f").as_deref(), Some("a"));
        assert_eq!(tiled_anchor(&window, "a").as_deref(), Some("b"));
        // With no tiled layer, or no block in it, there is nothing to return beside: this
        // client never makes one up.
        let mut empty = window.clone();
        empty.layers.remove(0);
        assert_eq!(tiled_anchor(&empty, "f"), None);
        let mut bare = window.clone();
        bare.layers[0].layout = None;
        bare.layers[0].blocks.clear();
        assert_eq!(tiled_anchor(&bare, "f"), None);
        // A layout that is one block, which is also the pane returning.
        let mut alone = window.clone();
        alone.layers[0].layout = Some(serde_json::from_value(json!({"block_id":"a"})).unwrap());
        alone.layers[0].blocks = alone.layers[0].blocks[..1].to_vec();
        assert_eq!(tiled_anchor(&alone, "f").as_deref(), Some("a"));
        assert_eq!(tiled_anchor(&alone, "a"), None);
    }

    #[test]
    fn bounds_reads_valid_rectangles_and_rejects_malformed_ones() {
        let mut layer = window().layers.remove(1);
        assert_eq!(bounds(&layer), Some([0.15, 0.12, 0.7, 0.7]));
        for bad in [
            vec![],
            vec![0.1, 0.1, 0.5],
            vec![0.1, 0.1, 0.5, 0.5, 0.5],
            vec![-0.01, 0.1, 0.5, 0.5],
            vec![0.1, -0.01, 0.5, 0.5],
            vec![0.1, 0.1, 0.0, 0.5],
            vec![0.1, 0.1, 0.5, 0.0],
            vec![0.1, 0.1, -0.2, 0.5],
            vec![f64::NAN, 0.1, 0.5, 0.5],
            vec![0.1, 0.1, f64::INFINITY, 0.5],
        ] {
            layer.bounds = bad.clone();
            assert_eq!(bounds(&layer), None, "{bad:?}");
        }
    }

    #[test]
    fn min_fraction_caps_at_one_and_rejects_degenerate_areas() {
        assert_eq!(
            min_fraction((1000.0, 500.0), (160.0, 96.0)),
            Some([0.16, 0.192])
        );
        assert_eq!(min_fraction((100.0, 50.0), (160.0, 96.0)), Some([1.0, 1.0]));
        assert_eq!(min_fraction((0.5, 500.0), MIN_FLOATING_PT), None);
        assert_eq!(min_fraction((500.0, 0.9), MIN_FLOATING_PT), None);
        assert_eq!(min_fraction((0.0, 0.0), MIN_FLOATING_PT), None);
        assert_eq!(min_fraction((-10.0, 500.0), MIN_FLOATING_PT), None);
        assert_eq!(min_fraction((f32::NAN, 500.0), MIN_FLOATING_PT), None);
        assert_eq!(min_fraction((f32::INFINITY, 500.0), MIN_FLOATING_PT), None);
    }

    #[test]
    fn move_keeps_size_bit_identical_and_clamps_to_unit_square() {
        let start = [0.2, 0.15, 0.37, 0.43];
        let min = [0.16, 0.14];
        for delta in [
            [0.0, 0.0],
            [0.1, -0.05],
            [-5.0, -5.0],
            [5.0, 5.0],
            [0.9, -0.9],
            [-0.9, 0.9],
        ] {
            let out = dragged(start, Grip::Move, delta, min);
            assert_eq!(
                out[2].to_bits(),
                start[2].to_bits(),
                "width changed for {delta:?}"
            );
            assert_eq!(
                out[3].to_bits(),
                start[3].to_bits(),
                "height changed for {delta:?}"
            );
            assert!(out[0] >= 0.0 && out[1] >= 0.0);
            assert!(out[0] + out[2] <= 1.0 && out[1] + out[3] <= 1.0, "{out:?}");
            assert_eq!(sendable(out), Some(out));
        }
        assert_eq!(
            dragged(start, Grip::Move, [-5.0, -5.0], min)[..2],
            [0.0, 0.0]
        );
    }

    #[test]
    fn every_resize_grip_holds_its_opposite_edge_fixed_and_respects_bounds_and_min() {
        let start = [0.2, 0.2, 0.5, 0.5];
        let min = [0.2, 0.15];
        let right = start[0] + start[2];
        let bottom = start[1] + start[3];
        let eps = 1e-12;

        // Grow and shrink each of the 8 grips.
        for &(grip, delta, expect_left, expect_right, expect_top, expect_bottom) in &[
            (
                Grip::West,
                [-0.1, 0.2],
                Some(0.1),
                Some(right),
                Some(0.2),
                Some(bottom),
            ),
            (
                Grip::East,
                [0.15, -0.2],
                Some(0.2),
                Some(0.85),
                Some(0.2),
                Some(bottom),
            ),
            (
                Grip::North,
                [0.2, -0.1],
                Some(0.2),
                Some(right),
                Some(0.1),
                Some(bottom),
            ),
            (
                Grip::South,
                [-0.2, 0.2],
                Some(0.2),
                Some(right),
                Some(0.2),
                Some(0.9),
            ),
            (
                Grip::NorthWest,
                [-0.1, -0.05],
                Some(0.1),
                Some(right),
                Some(0.15),
                Some(bottom),
            ),
            (
                Grip::NorthEast,
                [0.1, -0.05],
                Some(0.2),
                Some(0.8),
                Some(0.15),
                Some(bottom),
            ),
            (
                Grip::SouthWest,
                [-0.1, 0.1],
                Some(0.1),
                Some(right),
                Some(0.2),
                Some(0.8),
            ),
            (
                Grip::SouthEast,
                [0.1, 0.1],
                Some(0.2),
                Some(0.8),
                Some(0.2),
                Some(0.8),
            ),
        ] {
            let [x, y, w, h] = dragged(start, grip, delta, min);
            if let Some(el) = expect_left {
                assert!((x - el).abs() < eps, "{grip:?} left {x} != {el}");
            }
            if let Some(er) = expect_right {
                assert!(
                    ((x + w) - er).abs() < eps,
                    "{grip:?} right {} != {er}",
                    x + w
                );
            }
            if let Some(et) = expect_top {
                assert!((y - et).abs() < eps, "{grip:?} top {y} != {et}");
            }
            if let Some(eb) = expect_bottom {
                assert!(
                    ((y + h) - eb).abs() < eps,
                    "{grip:?} bottom {} != {eb}",
                    y + h
                );
            }
        }

        // Shrinking past `min` clamps to `min` without flipping the opposite edge.
        let w_shrunk = dragged(start, Grip::West, [0.9, 0.0], min);
        assert!((w_shrunk[2] - min[0]).abs() < eps);
        assert!(((w_shrunk[0] + w_shrunk[2]) - right).abs() < eps);
        let e_shrunk = dragged(start, Grip::East, [-0.9, 0.0], min);
        assert_eq!(e_shrunk[0], start[0]);
        assert!((e_shrunk[2] - min[0]).abs() < eps);
        let n_shrunk = dragged(start, Grip::North, [0.0, 0.9], min);
        assert!((n_shrunk[3] - min[1]).abs() < eps);
        assert!(((n_shrunk[1] + n_shrunk[3]) - bottom).abs() < eps);
        let s_shrunk = dragged(start, Grip::South, [0.0, -0.9], min);
        assert_eq!(s_shrunk[1], start[1]);
        assert!((s_shrunk[3] - min[1]).abs() < eps);

        // Zero delta or non-finite delta returns `start` unchanged.
        for grip in Grip::ALL {
            assert_eq!(dragged(start, grip, [0.0, 0.0], min), start);
            assert_eq!(dragged(start, grip, [f64::NAN, 0.1], min), start);
            assert_eq!(dragged(start, grip, [0.1, f64::INFINITY], min), start);
        }
    }

    #[test]
    fn a_layer_already_smaller_than_min_is_never_forced_to_grow() {
        let tiny = [0.4, 0.4, 0.05, 0.04];
        let min = [0.25, 0.2];
        // Shrinking further keeps its own smaller size; it never snaps up to `min`.
        for (grip, delta) in [
            (Grip::East, [-0.1, 0.0]),
            (Grip::West, [0.1, 0.0]),
            (Grip::South, [0.0, -0.1]),
            (Grip::North, [0.0, 0.1]),
            (Grip::SouthEast, [-0.1, -0.1]),
            (Grip::NorthWest, [0.1, 0.1]),
        ] {
            let out = dragged(tiny, grip, delta, min);
            assert!(out[2] <= tiny[2] + 1e-12, "{grip:?} grew width: {out:?}");
            assert!(out[3] <= tiny[3] + 1e-12, "{grip:?} grew height: {out:?}");
            assert!(
                out[2] >= tiny[2] - 1e-12 && out[3] >= tiny[3] - 1e-12,
                "{grip:?}: {out:?}"
            );
        }
        // Moving keeps its tiny size bit-identical.
        let moved = dragged(tiny, Grip::Move, [0.1, -0.1], min);
        assert_eq!(moved[2].to_bits(), tiny[2].to_bits());
        assert_eq!(moved[3].to_bits(), tiny[3].to_bits());
        // Growing outward still works smoothly without jumping straight to `min`.
        let grown = dragged(tiny, Grip::East, [0.02, 0.0], min);
        assert!((grown[2] - 0.07).abs() < 1e-12, "{grown:?}");
    }

    #[test]
    fn sendable_steps_rounding_overshoot_down_and_deterministic_sweep_always_passes() {
        // 1 ulp above 1.0 in f64 (which the live API-13 server rejects) steps down strictly.
        let over_x = [0.5, 0.1, f64::from_bits(0.5_f64.to_bits() + 2), 0.5];
        assert!(over_x[0] + over_x[2] > 1.0);
        let fixed = sendable(over_x).expect("stepped down");
        assert!(fixed[0] + fixed[2] <= 1.0);
        assert!(fixed[2] > 0.0);

        let over_y = [0.1, 0.5, 0.5, f64::from_bits(0.5_f64.to_bits() + 2)];
        assert!(over_y[1] + over_y[3] > 1.0);
        let fixed_y = sendable(over_y).expect("stepped down");
        assert!(fixed_y[1] + fixed_y[3] <= 1.0);
        assert!(fixed_y[3] > 0.0);

        // Exact 1.0 stays untouched.
        assert_eq!(sendable([0.3, 0.2, 0.7, 0.8]), Some([0.3, 0.2, 0.7, 0.8]));
        assert_eq!(sendable([0.0, 0.0, 1.0, 1.0]), Some([0.0, 0.0, 1.0, 1.0]));

        // Invalid rectangles return None.
        for bad in [
            [-0.01, 0.0, 0.5, 0.5],
            [0.0, -0.01, 0.5, 0.5],
            [1.0, 0.0, 0.1, 0.5],
            [0.0, 1.0, 0.5, 0.1],
            [0.1, 0.1, 0.0, 0.5],
            [0.1, 0.1, 0.5, 0.0],
            [f64::NAN, 0.1, 0.5, 0.5],
        ] {
            assert_eq!(sendable(bad), None, "{bad:?}");
        }

        // Deterministic sweep across starts, areas, all 9 grips and extreme deltas.
        let starts = [
            [0.0, 0.0, 1.0, 1.0],
            [0.15, 0.12, 0.7, 0.7],
            [0.3, 0.2, 0.7, 0.8],
            [0.01, 0.01, 0.05, 0.04],
            [0.7, 0.7, 0.29999999999999993, 0.29999999999999993],
            [
                0.1234567890123456,
                0.2345678901234567,
                0.4567890123456789,
                0.543210987654321,
            ],
        ];
        let areas = [(616.0_f32, 404.0_f32), (1000.0, 624.0), (1920.0, 1080.0)];
        let deltas = [-2.0, -0.7, -0.13, -1e-6, 0.0, 1e-6, 0.13, 0.7, 2.0];
        for area in areas {
            let min = min_fraction(area, MIN_FLOATING_PT).unwrap();
            for start in starts {
                for grip in Grip::ALL {
                    for &dx in &deltas {
                        for &dy in &deltas {
                            let proposed = dragged(start, grip, [dx, dy], min);
                            let [x, y, w, h] = sendable(proposed).unwrap_or_else(|| {
                                panic!("not sendable: start={start:?} grip={grip:?} d=({dx},{dy}) -> {proposed:?}")
                            });
                            assert!(
                                x >= 0.0
                                    && y >= 0.0
                                    && w > 0.0
                                    && h > 0.0
                                    && x + w <= 1.0
                                    && y + h <= 1.0,
                                "out of unit square: {:?}",
                                [x, y, w, h]
                            );
                        }
                    }
                }
            }
        }
    }
}
