//! Vim-style keyboard copy mode on the focused terminal pane.
//!
//! All terminal state (tracked cursor/anchor, word/WORD motions, Ghostty
//! selections, and race-free yank formatting) lives in
//! [`rex_rune::terminal::copy_mode`]. This module owns the UI session on the
//! focused pane, layout-independent key dispatch, repeat counts, `g`/`y`
//! prefixes, search handoff (`/`/`?`/`n`/`N`), and lifecycle reconciliation.
use super::{
    bindings::Input,
    platform::keys::modifiers_on,
    workspace::{HostId, Workspace},
};
use rex_rune::{
    connection::Command,
    layers,
    terminal::copy_mode::{CopyModeInput, CopyModeOp, CopyMotion, CopyUnit, MAX_COUNT},
};
use rune::{Key, KeyEvent, TextEdit, Tx};

/// Single registered workspace action that enters copy mode on the focused pane.
pub const ACTION: &str = "pane.copy_mode";

/// Multi-key prefix pending inside copy mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefix {
    /// `g` waiting for `g` (`gg` / `Ngg`).
    G,
    /// `y` waiting for `y` (`yy` / `Nyy`).
    Y,
}

/// Direction recorded by `/` (forward / newer) or `?` (backward / older).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchDirection {
    #[default]
    Forward,
    Backward,
}

impl SearchDirection {
    /// Argument to [`Command::SearchStep`]: in [`rex_rune::terminal::search`],
    /// `older = true` moves upward toward older scrollback (`?`) and
    /// `older = false` moves downward toward newer lines (`/`).
    pub fn step_older(self, opposite: bool) -> bool {
        let backward = matches!(self, Self::Backward);
        backward ^ opposite
    }
}

/// Active UI copy-mode session tied to one focused pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub key: (HostId, String),
    pub session: String,
    pub window: String,
    pub generation: u64,
    pub epoch: u64,
    pub unit: Option<CopyUnit>,
    pub count: Option<u32>,
    pub prefix: Option<Prefix>,
    pub opened_search: bool,
    pub search_direction: SearchDirection,
}

impl Session {
    /// Compact label for the toolbar mode chip (`copy`, `copy · visual`,
    /// `copy · 12`, `copy · g`, `copy · y`, etc.).
    pub fn chip_label(&self) -> String {
        let mut label = match self.unit {
            None => "copy".to_owned(),
            Some(CopyUnit::Char) => "copy · visual".to_owned(),
            Some(CopyUnit::Line) => "copy · line".to_owned(),
            Some(CopyUnit::Block) => "copy · block".to_owned(),
        };
        let suffix = match (self.count, self.prefix) {
            (Some(n), Some(Prefix::G)) => Some(format!("{n}g")),
            (Some(n), Some(Prefix::Y)) => Some(format!("{n}y")),
            (Some(n), None) => Some(n.to_string()),
            (None, Some(Prefix::G)) => Some("g".to_owned()),
            (None, Some(Prefix::Y)) => Some("y".to_owned()),
            (None, None) => None,
        };
        if let Some(suffix) = suffix {
            label.push_str(" · ");
            label.push_str(&suffix);
        }
        label
    }
}

/// Extract a logical character for a plain/shifted key without Ctrl/Alt/Super.
fn logical_char(ev: &KeyEvent, shift: bool) -> Option<char> {
    let Key::Text(s) = &ev.key else {
        return None;
    };
    let mut chars = s.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if shift && ch.is_ascii_lowercase() {
        Some(ch.to_ascii_uppercase())
    } else {
        Some(ch)
    }
}

/// Extract the ASCII letter for a `Ctrl+<letter>` press across platforms.
fn ctrl_letter(ev: &KeyEvent, base: Option<char>) -> Option<char> {
    let ch = match &ev.key {
        Key::Command(c) => Some(*c),
        Key::Text(s) if s.chars().count() == 1 => {
            let c = s.chars().next()?;
            if ('\x01'..='\x1a').contains(&c) {
                Some(((c as u8) - 1 + b'a') as char)
            } else {
                Some(c)
            }
        }
        _ => None,
    }
    .or(base)?;
    Some(ch.to_ascii_lowercase())
}

impl Workspace {
    /// Whether copy mode is currently active on the focused pane.
    pub fn in_copy_mode(&self) -> bool {
        self.copy_mode.is_some()
    }

    /// Whether copy mode is currently active on `key`.
    pub fn copy_mode_on(&self, key: &(HostId, String)) -> bool {
        self.copy_mode.as_ref().is_some_and(|s| &s.key == key)
    }

    /// The active copy-mode session, if any.
    #[cfg(test)]
    pub fn active_copy_mode(&self) -> Option<&Session> {
        self.copy_mode.as_ref()
    }

    /// Label for the toolbar mode chip when copy mode is active.
    pub fn copy_mode_chip_label(&self) -> Option<String> {
        self.copy_mode.as_ref().map(Session::chip_label)
    }

    /// Whether either copy mode or a host key mode is showing in the mode chip.
    pub fn has_mode_chip(&self) -> bool {
        self.in_copy_mode() || self.key_mode().is_some()
    }

    /// Whether the focused pane can currently enter copy mode.
    pub fn can_enter_copy_mode(&self) -> bool {
        let model = self.model();
        if !model.connected {
            return false;
        }
        let Some(block) = self.focused_block() else {
            return false;
        };
        let Some(window) = model.window() else {
            return false;
        };
        if layers::layer_of(window, &block).is_some_and(layers::minimized) {
            return false;
        }
        let key = self.term_key(&block);
        self.terminals
            .get(&key)
            .is_some_and(|t| t.status().live && t.frame.is_some())
    }

    /// Enter copy mode on the focused pane. Idempotent when already active on that pane.
    pub fn enter_copy_mode(&mut self, tx: &mut Tx) {
        self.clear_focus_history_walk();
        let model = self.model();
        if !model.connected {
            self.inform("Copy mode requires a connected terminal.");
            return;
        }
        let Some(session_id) = model.selected.clone() else {
            self.inform("Copy mode requires a focused terminal.");
            return;
        };
        let Some(window) = model.window() else {
            self.inform("Copy mode requires a focused terminal.");
            return;
        };
        let Some(window_id) = window.window_id.clone() else {
            self.inform("Copy mode requires a focused terminal.");
            return;
        };
        let Some(block) = self.focused_block() else {
            self.inform("Copy mode requires a focused terminal.");
            return;
        };
        if layers::layer_of(window, &block).is_some_and(layers::minimized) {
            self.inform("Copy mode requires a visible terminal.");
            return;
        }
        let key = self.term_key(&block);
        if self.copy_mode_on(&key) {
            // Already active on this pane: ensure find bar doesn't keep focus and stay idempotent.
            if let Some(term) = self.terminals.get_mut(&key) {
                if let Some(edit) = &mut term.search {
                    edit.focused = false;
                }
                term.preedit.clear();
            }
            tx.blur();
            return;
        }
        let ready = self
            .terminals
            .get(&key)
            .is_some_and(|t| t.status().live && t.frame.is_some());
        if !ready {
            self.inform("Copy mode requires a connected terminal with content.");
            return;
        }
        // Entering on a fresh pane cleanly exits any earlier session and cancels drags/modes.
        // Keep `forwarded` intact so key-up releases for keys pressed before entering copy
        // mode (such as Ctrl/Shift from Ctrl+Shift+X on kitty key-release clients) still drain.
        self.exit_copy_mode();
        self.cancel_layer_drag();
        self.drop_hover = None;
        self.bindings.clear_modes();
        self.bindings.sequence.clear();
        tx.blur();

        let cell = self.metrics().cell;
        let Some(term) = self.terminals.get_mut(&key) else {
            return;
        };
        term.preedit.clear();
        if let Some(edit) = &mut term.search {
            edit.focused = false;
        }
        let (generation, epoch) = term.begin_copy_mode(cell);
        self.copy_mode = Some(Session {
            key,
            session: session_id,
            window: window_id,
            generation,
            epoch,
            unit: None,
            count: None,
            prefix: None,
            opened_search: false,
            search_direction: SearchDirection::Forward,
        });
    }

    /// Exit copy mode cleanly, sending `CopyModeOp::End` to the pane worker and clearing
    /// any search opened during copy mode.
    pub fn exit_copy_mode(&mut self) {
        self.finish_copy_mode(true);
    }

    fn finish_copy_mode(&mut self, send_end: bool) {
        let Some(session) = self.copy_mode.take() else {
            return;
        };
        let cell = self.metrics().cell;
        if let Some(term) = self.terminals.get_mut(&session.key) {
            if send_end {
                term.send_copy_mode(session.generation, session.epoch, cell, CopyModeOp::End);
            }
            term.clear_copy_mode_active();
            if session.opened_search
                && let Some(edit) = term.search.take()
            {
                term.find.corner = Default::default();
                if !edit.text.is_empty() {
                    term.find.last = edit.text.clone();
                }
                term.send(Command::Search(String::new()));
            }
        }
    }

    /// Reconcile copy mode after a host/session/window/focus update.
    pub fn reconcile_copy_mode(&mut self) {
        let Some(session) = self.copy_mode.as_ref() else {
            return;
        };
        let valid = self.hosts.get(self.active).is_some_and(|host| {
            host.id == session.key.0
                && host.model.connected
                && host.model.selected.as_deref() == Some(session.session.as_str())
                && host.model.window().is_some_and(|w| {
                    w.window_id.as_deref() == Some(session.window.as_str())
                        && w.focused_block_id.as_deref() == Some(session.key.1.as_str())
                        && !layers::layer_of(w, &session.key.1).is_some_and(layers::minimized)
                })
        }) && self.terminals.contains_key(&session.key);
        if !valid {
            self.exit_copy_mode();
        }
    }

    /// Reconcile copy mode after a terminal worker wakes with a new frame, snapshot reset,
    /// or worker auto-exit (`copy_ended_epoch >= active.epoch`).
    pub fn reconcile_copy_mode_wake(&mut self, key: &(HostId, String)) {
        let Some(session) = self.copy_mode.as_ref() else {
            return;
        };
        if &session.key != key {
            return;
        }
        let Some(term) = self.terminals.get(key) else {
            self.copy_mode = None;
            return;
        };
        if term.restore_generation() != session.generation
            || term.copy_ended_epoch() >= session.epoch
            || !term.copy_mode_is_active(session.epoch)
        {
            self.finish_copy_mode(false);
            return;
        }
        if let Some(view) = term.frame.as_ref().and_then(|f| f.copy)
            && view.epoch == session.epoch
            && let Some(active) = self.copy_mode.as_mut()
            && term.frame.as_ref().map(|f| f.selection_revision) == Some(term.selection_revision)
        {
            active.unit = view.unit;
        }
    }

    /// Send `CopyModeOp::Scroll(delta)` while copy mode is active on the focused pane.
    pub fn copy_mode_scroll(&mut self, delta: i32) {
        if delta == 0 {
            return;
        }
        self.send_active_copy_op(CopyModeOp::Scroll(delta));
    }

    /// Yank an active visual selection in copy mode (`Ctrl+Shift+C` / `pane.copy`) and exit.
    /// Returns `true` if copy mode handled the request (so caller never falls back to stale
    /// pointer selection text while in copy mode).
    pub fn yank_copy_mode_selection(&mut self) -> bool {
        let Some(session) = self.copy_mode.as_ref() else {
            return false;
        };
        if session.unit.is_some() {
            self.send_active_copy_op(CopyModeOp::Yank);
            self.exit_copy_mode();
        }
        true
    }

    /// Handle `Enter` in the find bar while copy mode is active: blur the find bar back to
    /// copy mode, keep search highlights visible, and jump the copy cursor straight to the
    /// selected match without skipping it.
    pub fn confirm_copy_mode_search(&mut self, key: &(HostId, String), tx: &mut Tx) {
        let empty = self
            .terminals
            .get_mut(key)
            .and_then(|t| t.search.as_mut())
            .is_none_or(|edit| {
                edit.focused = false;
                edit.text.is_empty()
            });
        if empty {
            self.close_search(key, tx);
            if let Some(session) = self.copy_mode.as_mut().filter(|s| &s.key == key) {
                session.opened_search = false;
            }
            return;
        }
        if let Some(term) = self.terminals.get_mut(key)
            && let Some(edit) = &term.search
        {
            term.find.last = edit.text.clone();
        }
        tx.blur();
        self.send_active_copy_op(CopyModeOp::ToMatch);
    }

    /// Handle `Escape` in the find bar while copy mode is active: close/blur the find bar
    /// and return focus to copy mode without exiting copy mode.
    pub fn cancel_copy_mode_search(&mut self, key: &(HostId, String), tx: &mut Tx) {
        self.close_search(key, tx);
        if let Some(session) = self.copy_mode.as_mut().filter(|s| &s.key == key) {
            session.opened_search = false;
        }
    }

    /// Open the pane's find bar from copy mode (`/` forward, `?` backward, or `Ctrl+Shift+F`).
    pub fn open_copy_mode_search(&mut self, direction: SearchDirection) {
        let Some(key) = self.copy_mode.as_ref().map(|s| s.key.clone()) else {
            return;
        };
        if let Some(session) = self.copy_mode.as_mut() {
            session.count = None;
            session.prefix = None;
            session.search_direction = direction;
            session.opened_search = true;
        }
        if let Some(term) = self.terminals.get_mut(&key) {
            let mut edit = TextEdit::new("");
            edit.focused = true;
            term.search = Some(edit);
            term.send(Command::Search(String::new()));
        }
    }

    /// Dispatch a key press while copy mode is active (and neither a panel nor the find bar
    /// holds keyboard focus). Always returns `true` so nothing ever leaks to the PTY.
    pub fn copy_mode_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let input = Input {
            event: ev,
            code: tx.physical_key(),
            base: tx.key_base(),
            repeat: tx.key_repeat(),
        };
        let identity = input.identity();
        self.forwarded.remove(&identity);

        // Modifier-only presses (e.g. Shift before `G` or `$`, Ctrl before `Ctrl+V`)
        // must be swallowed without disturbing pending counts or prefixes.
        if matches!(ev.key, Key::Modifier { .. }) {
            return true;
        }
        // Dead keys never reach the PTY.
        if matches!(ev.key, Key::Dead(_)) {
            self.bindings.consumed.insert(identity, true);
            return true;
        }

        let mods = modifiers_on(ev.mods, rex_rune::keymap::command_key());
        let plain_mods = !mods.ctrl && !mods.alt && !mods.logo;
        let ctrl_only = mods.ctrl && !mods.alt && !mods.logo && !mods.shift;
        // On Nordic/European layouts, copy-mode symbols (`$`, `^`, `/`, `?`) can be produced
        // via macOS Option (`alt`) or Windows/Linux AltGr (`ctrl+alt`).
        let symbol_mods = !mods.logo && (plain_mods || mods.alt);

        // 1. Escape / Ctrl+C hierarchy: pending count/prefix -> visual selection -> exit.
        let is_escape = ev.key == Key::Escape && !mods.alt && !mods.logo && !mods.ctrl;
        let is_ctrl_c = ctrl_only && ctrl_letter(ev, input.base) == Some('c');
        if is_escape || is_ctrl_c {
            self.bindings.consumed.insert(identity, true);
            let (has_pending, has_unit) = self.copy_mode.as_ref().map_or((false, false), |s| {
                (s.count.is_some() || s.prefix.is_some(), s.unit.is_some())
            });
            if has_pending {
                if let Some(s) = self.copy_mode.as_mut() {
                    s.count = None;
                    s.prefix = None;
                }
            } else if has_unit {
                if let Some(s) = self.copy_mode.as_mut() {
                    s.unit = None;
                }
                self.send_active_copy_op(CopyModeOp::ClearSelection);
            } else {
                self.exit_copy_mode();
            }
            return true;
        }

        // 2. Ctrl chords: Ctrl+U/D/B/F (paging), Ctrl+V (block visual).
        if ctrl_only && let Some(letter) = ctrl_letter(ev, input.base) {
            match letter {
                'u' => {
                    self.bindings.consumed.insert(identity, false);
                    let count = self.take_copy_count_clear_prefix().unwrap_or(1);
                    self.send_active_copy_op(CopyModeOp::Move {
                        motion: CopyMotion::HalfPageUp,
                        count,
                    });
                }
                'd' => {
                    self.bindings.consumed.insert(identity, false);
                    let count = self.take_copy_count_clear_prefix().unwrap_or(1);
                    self.send_active_copy_op(CopyModeOp::Move {
                        motion: CopyMotion::HalfPageDown,
                        count,
                    });
                }
                'b' => {
                    self.bindings.consumed.insert(identity, false);
                    let count = self.take_copy_count_clear_prefix().unwrap_or(1);
                    self.send_active_copy_op(CopyModeOp::Move {
                        motion: CopyMotion::PageUp,
                        count,
                    });
                }
                'f' => {
                    self.bindings.consumed.insert(identity, false);
                    let count = self.take_copy_count_clear_prefix().unwrap_or(1);
                    self.send_active_copy_op(CopyModeOp::Move {
                        motion: CopyMotion::PageDown,
                        count,
                    });
                }
                'v' => {
                    self.bindings.consumed.insert(identity, true);
                    self.take_copy_count_clear_prefix();
                    self.toggle_copy_unit(CopyUnit::Block);
                }
                _ => {
                    self.bindings.consumed.insert(identity, true);
                    self.take_copy_count_clear_prefix();
                }
            }
            return true;
        }

        // 3. Special navigation / Enter keys (without Ctrl/Alt/Logo).
        if plain_mods {
            match &ev.key {
                Key::Left | Key::Backspace => {
                    self.bindings.consumed.insert(identity, false);
                    let count = self.take_copy_count_clear_prefix().unwrap_or(1);
                    self.send_active_copy_op(CopyModeOp::Move {
                        motion: CopyMotion::Left,
                        count,
                    });
                    return true;
                }
                Key::Down => {
                    self.bindings.consumed.insert(identity, false);
                    let count = self.take_copy_count_clear_prefix().unwrap_or(1);
                    self.send_active_copy_op(CopyModeOp::Move {
                        motion: CopyMotion::Down,
                        count,
                    });
                    return true;
                }
                Key::Up => {
                    self.bindings.consumed.insert(identity, false);
                    let count = self.take_copy_count_clear_prefix().unwrap_or(1);
                    self.send_active_copy_op(CopyModeOp::Move {
                        motion: CopyMotion::Up,
                        count,
                    });
                    return true;
                }
                Key::Right => {
                    self.bindings.consumed.insert(identity, false);
                    let count = self.take_copy_count_clear_prefix().unwrap_or(1);
                    self.send_active_copy_op(CopyModeOp::Move {
                        motion: CopyMotion::Right,
                        count,
                    });
                    return true;
                }
                Key::Home => {
                    self.bindings.consumed.insert(identity, true);
                    self.take_copy_count_clear_prefix();
                    self.send_active_copy_op(CopyModeOp::Move {
                        motion: CopyMotion::LineStart,
                        count: 1,
                    });
                    return true;
                }
                Key::End => {
                    self.bindings.consumed.insert(identity, true);
                    self.take_copy_count_clear_prefix();
                    self.send_active_copy_op(CopyModeOp::Move {
                        motion: CopyMotion::LineEnd,
                        count: 1,
                    });
                    return true;
                }
                Key::PageUp => {
                    self.bindings.consumed.insert(identity, false);
                    let count = self.take_copy_count_clear_prefix().unwrap_or(1);
                    self.send_active_copy_op(CopyModeOp::Move {
                        motion: CopyMotion::PageUp,
                        count,
                    });
                    return true;
                }
                Key::PageDown => {
                    self.bindings.consumed.insert(identity, false);
                    let count = self.take_copy_count_clear_prefix().unwrap_or(1);
                    self.send_active_copy_op(CopyModeOp::Move {
                        motion: CopyMotion::PageDown,
                        count,
                    });
                    return true;
                }
                Key::Enter => {
                    self.bindings.consumed.insert(identity, true);
                    self.take_copy_count_clear_prefix();
                    if self.copy_mode.as_ref().is_some_and(|s| s.unit.is_some()) {
                        self.send_active_copy_op(CopyModeOp::Yank);
                        self.exit_copy_mode();
                    }
                    return true;
                }
                _ => {}
            }
        }

        // 4. Layout-independent logical character dispatch (plain/shifted for all bindings,
        // plus Option / AltGr for copy-mode symbols `$`, `^`, `/`, `?`).
        let ch = if plain_mods {
            logical_char(ev, mods.shift)
        } else if symbol_mods {
            logical_char(ev, mods.shift).filter(|c| matches!(c, '$' | '^' | '/' | '?'))
        } else {
            None
        };
        if let Some(ch) = ch {
            let pending_prefix = self.copy_mode.as_ref().and_then(|s| s.prefix);
            if let Some(prefix) = pending_prefix {
                self.bindings.consumed.insert(identity, true);
                let count = self.take_copy_count_clear_prefix();
                match (prefix, ch) {
                    (Prefix::G, 'g') => {
                        let motion = match count {
                            Some(n) => CopyMotion::LineNumber(n as usize),
                            None => CopyMotion::Top,
                        };
                        self.send_active_copy_op(CopyModeOp::Move { motion, count: 1 });
                    }
                    (Prefix::Y, 'y') => {
                        self.send_active_copy_op(CopyModeOp::YankLine {
                            count: count.unwrap_or(1),
                        });
                        self.exit_copy_mode();
                    }
                    // Any other key after `g` or `y` cancels the prefix cleanly without leaking.
                    _ => {}
                }
                return true;
            }

            match ch {
                '1'..='9' => {
                    self.bindings.consumed.insert(identity, true);
                    let digit = u32::from(ch as u8 - b'0');
                    if let Some(s) = self.copy_mode.as_mut() {
                        let next = s
                            .count
                            .unwrap_or(0)
                            .saturating_mul(10)
                            .saturating_add(digit)
                            .clamp(1, MAX_COUNT);
                        s.count = Some(next);
                    }
                }
                '0' => {
                    self.bindings.consumed.insert(identity, true);
                    if self.copy_mode.as_ref().is_some_and(|s| s.count.is_some()) {
                        if let Some(s) = self.copy_mode.as_mut() {
                            let next = s.count.unwrap_or(0).saturating_mul(10).clamp(1, MAX_COUNT);
                            s.count = Some(next);
                        }
                    } else {
                        self.send_active_copy_op(CopyModeOp::Move {
                            motion: CopyMotion::LineStart,
                            count: 1,
                        });
                    }
                }
                'h' | 'j' | 'k' | 'l' | 'w' | 'b' | 'e' | 'W' | 'B' | 'E' => {
                    self.bindings.consumed.insert(identity, false);
                    let count = self.take_copy_count_clear_prefix().unwrap_or(1);
                    let motion = match ch {
                        'h' => CopyMotion::Left,
                        'j' => CopyMotion::Down,
                        'k' => CopyMotion::Up,
                        'l' => CopyMotion::Right,
                        'w' => CopyMotion::WordForward,
                        'b' => CopyMotion::WordBackward,
                        'e' => CopyMotion::WordEnd,
                        'W' => CopyMotion::BigWordForward,
                        'B' => CopyMotion::BigWordBackward,
                        'E' => CopyMotion::BigWordEnd,
                        _ => unreachable!(),
                    };
                    self.send_active_copy_op(CopyModeOp::Move { motion, count });
                }
                '^' | '$' | 'H' | 'M' | 'L' => {
                    self.bindings.consumed.insert(identity, true);
                    self.take_copy_count_clear_prefix();
                    let motion = match ch {
                        '^' => CopyMotion::FirstNonBlank,
                        '$' => CopyMotion::LineEnd,
                        'H' => CopyMotion::ViewportTop,
                        'M' => CopyMotion::ViewportMiddle,
                        'L' => CopyMotion::ViewportBottom,
                        _ => unreachable!(),
                    };
                    self.send_active_copy_op(CopyModeOp::Move { motion, count: 1 });
                }
                'g' => {
                    self.bindings.consumed.insert(identity, true);
                    if let Some(s) = self.copy_mode.as_mut() {
                        s.prefix = Some(Prefix::G);
                    }
                }
                'G' => {
                    self.bindings.consumed.insert(identity, true);
                    let motion = match self.take_copy_count_clear_prefix() {
                        Some(n) => CopyMotion::LineNumber(n as usize),
                        None => CopyMotion::Bottom,
                    };
                    self.send_active_copy_op(CopyModeOp::Move { motion, count: 1 });
                }
                'v' => {
                    self.bindings.consumed.insert(identity, true);
                    self.take_copy_count_clear_prefix();
                    self.toggle_copy_unit(CopyUnit::Char);
                }
                'V' => {
                    self.bindings.consumed.insert(identity, true);
                    self.take_copy_count_clear_prefix();
                    self.toggle_copy_unit(CopyUnit::Line);
                }
                'o' => {
                    self.bindings.consumed.insert(identity, true);
                    self.take_copy_count_clear_prefix();
                    self.send_active_copy_op(CopyModeOp::SwapEnds);
                }
                'y' => {
                    self.bindings.consumed.insert(identity, true);
                    if self.copy_mode.as_ref().is_some_and(|s| s.unit.is_some()) {
                        self.take_copy_count_clear_prefix();
                        self.send_active_copy_op(CopyModeOp::Yank);
                        self.exit_copy_mode();
                    } else if let Some(s) = self.copy_mode.as_mut() {
                        s.prefix = Some(Prefix::Y);
                    }
                }
                'Y' => {
                    self.bindings.consumed.insert(identity, true);
                    let count = self.take_copy_count_clear_prefix().unwrap_or(1);
                    self.send_active_copy_op(CopyModeOp::YankLine { count });
                    self.exit_copy_mode();
                }
                '/' => {
                    self.bindings.consumed.insert(identity, true);
                    self.open_copy_mode_search(SearchDirection::Forward);
                }
                '?' => {
                    self.bindings.consumed.insert(identity, true);
                    self.open_copy_mode_search(SearchDirection::Backward);
                }
                'n' | 'N' => {
                    self.bindings.consumed.insert(identity, false);
                    let count = self.take_copy_count_clear_prefix().unwrap_or(1).min(100);
                    self.step_copy_mode_search(ch == 'N', count);
                }
                'q' => {
                    self.bindings.consumed.insert(identity, true);
                    self.exit_copy_mode();
                }
                _ => {
                    // Any other key is swallowed and cancels a partial count cleanly.
                    self.bindings.consumed.insert(identity, true);
                    self.take_copy_count_clear_prefix();
                }
            }
            return true;
        }

        // Everything else (unbound chords, Alt/Super combinations, etc.) is swallowed.
        self.bindings.consumed.insert(identity, true);
        self.take_copy_count_clear_prefix();
        true
    }

    fn take_copy_count_clear_prefix(&mut self) -> Option<u32> {
        let session = self.copy_mode.as_mut()?;
        session.prefix = None;
        session.count.take()
    }

    fn toggle_copy_unit(&mut self, unit: CopyUnit) {
        if let Some(session) = self.copy_mode.as_mut() {
            session.unit = match session.unit {
                Some(existing) if existing == unit => None,
                _ => Some(unit),
            };
        }
        self.send_active_copy_op(CopyModeOp::Select(unit));
    }

    fn step_copy_mode_search(&mut self, opposite: bool, count: u32) {
        let Some((key, older)) = self
            .copy_mode
            .as_ref()
            .map(|s| (s.key.clone(), s.search_direction.step_older(opposite)))
        else {
            return;
        };
        let has_open_query = self
            .terminals
            .get(&key)
            .and_then(|t| t.search.as_ref())
            .is_some_and(|e| !e.text.is_empty());
        if has_open_query {
            if let Some(term) = self.terminals.get_mut(&key) {
                for _ in 0..count {
                    term.send(Command::SearchStep(older));
                }
            }
            return;
        }
        let last = self
            .terminals
            .get(&key)
            .map(|t| t.find.last.clone())
            .unwrap_or_default();
        if last.is_empty() {
            return;
        }
        if let Some(session) = self.copy_mode.as_mut() {
            session.opened_search = true;
        }
        if let Some(term) = self.terminals.get_mut(&key) {
            term.search = Some(TextEdit::new(last.as_str()));
            term.send(Command::Search(last));
        }
        self.send_active_copy_op(CopyModeOp::ToMatch);
        // `Command::Search` settles on the newest match (index 0): an `older` (`?`) search
        // is already at match 1 so it needs `count - 1` extra steps, while a forward (`!older`)
        // step wraps from newest to oldest on step 1 and advances forward across `count` steps.
        let extra_steps = if older {
            count.saturating_sub(1)
        } else {
            count
        };
        if extra_steps > 0
            && let Some(term) = self.terminals.get_mut(&key)
        {
            for _ in 0..extra_steps {
                term.send(Command::SearchStep(older));
            }
        }
    }

    fn send_active_copy_op(&mut self, op: CopyModeOp) {
        let Some((key, generation, epoch)) = self
            .copy_mode
            .as_ref()
            .map(|s| (s.key.clone(), s.generation, s.epoch))
        else {
            return;
        };
        let cell = self.metrics().cell;
        if let Some(term) = self.terminals.get_mut(&key) {
            term.send_copy_mode(generation, epoch, cell, op);
        }
    }
}

/// Build a stamped [`CopyModeInput`] with `visible_rows` only when the pane has laid out.
pub fn input(
    generation: u64,
    epoch: u64,
    revision: u64,
    visible_rows: Option<u16>,
    op: CopyModeOp,
) -> CopyModeInput {
    CopyModeInput {
        generation,
        epoch,
        revision,
        visible_rows,
        op,
    }
}
