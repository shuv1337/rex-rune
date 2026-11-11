//! A form opened from a Settings row (a host's default shell), drawn over
//! Settings against that row instead of in its place: Settings stays where it was, inert, as
//! under a popup button's list (`settings_kit::value_popup`, `popups::list_popup`).
use super::{
    panel::{Form, Panel, PanelKind},
    popups::dismiss_layer,
    settings_kit::under,
    theme::*,
    workspace::Workspace,
};
use rune::{El, col, el};

/// The form's width, and how far it sits from its row.
const WIDTH: f32 = 380.;
const GAP: f32 = 4.;
const PADDING: f32 = 16.;

/// The forms that open against the row that asked for them.
pub fn anchors(kind: &PanelKind) -> bool {
    matches!(kind, PanelKind::Form(Form::HostShell(_)))
}

/// The key of the row `panel`'s form is drawn against, when it was opened from one in
/// Settings.
pub fn anchor(panel: &Panel) -> Option<&str> {
    if !anchors(&panel.kind) {
        return None;
    }
    under(panel)?;
    panel.anchor_to.as_deref()
}

/// About how tall the form can get: its heading, fields, an error line, note and buttons.
/// It only decides which side of the row the form opens on, and it counts the error line
/// whether or not there is one, so the form does not change sides when one appears.
fn estimate(panel: &Panel) -> f32 {
    let fields = panel.inputs.len() as f32;
    2. * PADDING + 92. + fields * 86. + 28.
}

/// Where the row the open form is drawn against is, `[x, y, w, h]`. None when the form was
/// not opened from a row, or the row is not laid out: the form is a dialog then.
pub fn row(s: &Workspace) -> Option<[f32; 4]> {
    rune::memo::window_layout(anchor(s.panel.as_ref()?)?)
}

/// `body`, the form's heading, fields and buttons, in a popover against its row, which is
/// at `row`, over Settings.
pub fn popover(s: &Workspace, viewport: [f32; 2], row: [f32; 4], body: El) -> El {
    let panel = s.panel.as_ref().expect("form panel");
    let key = panel.anchor_to.clone().unwrap_or_default();
    let [cx, cy, cw, ch] = row;
    let [vw, vh] = viewport;
    let width = WIDTH.min(vw - 16.);
    let below = vh - (cy + ch + GAP) - 8.;
    let above = cy - GAP - 8.;
    let surface = glass(
        col()
            .key("form-popover")
            .role("dialog")
            .label(panel.title.clone())
            .w(width)
            .padding(PADDING)
            .preserve_focus_on_press(true)
            // Its padding takes a press, so one between two fields is not a press outside.
            .on_press(|_: &mut Workspace| {})
            .child(body),
        12.,
        true,
    )
    .transition(pop_in())
    // The form has the keyboard; Escape goes back to Settings.
    .modal(true, |s: &mut Workspace| s.cancel_panel_quiet());
    // Its trailing edge on the row's, where the row shows the value being edited.
    let x = (cx + cw - width).clamp(8., (vw - width - 8.).max(8.));
    // Under the row, or over it where only that side has the room. In a window too small
    // for either, against the window's bottom edge, over part of the row: the whole form
    // stays on screen. Pinned by the edge it is placed by, whatever its height turns out to
    // be.
    let needed = estimate(panel);
    let surface = if needed <= below {
        surface.absolute(x, cy + ch + GAP)
    } else if needed <= above {
        surface.pin(Some(x), None, None, Some(vh - (cy - GAP)))
    } else {
        surface.max_h(vh - 16.).pin(Some(x), None, None, Some(8.))
    };
    super::settings_view::settings_under(s, viewport)
        .child(dismiss_layer())
        // Removed with the form, this hands the keyboard back to the row (see `list_popup`).
        .child(
            el().key("form-focus-return")
                .absolute(0., 0.)
                .size(0., 0.)
                .focus_return(key),
        )
        .child(surface)
}
