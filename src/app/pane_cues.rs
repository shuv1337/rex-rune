//! The cues that say which pane has the keyboard, and the pointer taking it there: Settings →
//! Interface → Customize panes. All of it is chrome. The outline and the fade are drawn
//! around and above a terminal, never over its content, and hover focus moves the session's
//! focus without typing into, resizing or claiming a pane.
use super::workspace::Workspace;
use rex_linux::layout;
use rexrs::api::session::Window;
use std::time::Duration;

/// A faded header's name and buttons. Its activity word, progress bar and exception strip
/// stay whole: trouble is never dimmed.
pub const FADE: f32 = 0.45;

/// How long the pointer rests on a pane before Focus follows mouse focuses it, so crossing
/// one on the way to another leaves it alone.
pub const FOCUS_AFTER: Duration = Duration::from_millis(60);

/// Whether the pane with the keyboard is outlined in this tab. A card always can be; a pane
/// drawn flat (`flat`: Compact, or a tab's only pane edge to edge) only while the tab shows
/// another to tell it from.
pub fn outlines(s: &Workspace, window: &Window, flat: bool) -> bool {
    s.settings.highlights_active_pane() && (!flat || layout::placements(window, None).0.len() > 1)
}

/// The opacity of the name and buttons in `block`'s header.
pub fn header_opacity(s: &Workspace, block: &str) -> f32 {
    let interface = &s.settings.interface;
    let inactive = s.focused_block().as_deref() != Some(block);
    if interface.fade_inactive_pane_headers && inactive
        || interface.fade_pane_headers_in_inactive_window && !s.window_focused
    {
        FADE
    } else {
        1.
    }
}

impl Workspace {
    /// Focus follows mouse: the pointer came to rest on `block`'s pane. It asks the server
    /// to focus the pane, as a press on its header does, and nothing more: no key, no resize
    /// and no size ownership. Nothing moves while a menu or dialog is open, a divider is
    /// being dragged, or the window is not the active one.
    pub fn hover_focus(&mut self, block: &str) {
        if !self.settings.interface.focus_follows_mouse
            || !self.window_focused
            || self.panel.is_some()
            || self.drag.is_some()
            || self.focused_block().as_deref() == Some(block)
        {
            return;
        }
        if let Some(term) = self.terminals.get(&self.term_key(block)) {
            let _ = term.connection.commands.focus();
        }
    }
}
