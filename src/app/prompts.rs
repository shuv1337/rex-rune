//! Interactive `rex.ui` prompt queue, anti-spoofing modal, and waiting chip (API 17+).
//!
//! Prompt state lives on `Workspace.prompts`, never inside `Panel`, so demoting an open
//! prompt back to Waiting (for example when a local dialog opens) preserves draft text,
//! filter query, selection, checked items, and inline rejection messages.
use super::{
    dialogs,
    panel::PanelKind,
    theme::*,
    widgets::{Pill, Row, ScrollFx, glyph, list_row, pill},
    workspace::{HostId, Workspace},
};
use rex_rune::connection::{
    HostCommand,
    prompt::{
        EndReason, Prompt, PromptAnswer, PromptEvent, PromptEventKind, PromptIcon, PromptKind,
        ReplyOutcome,
    },
};
use rune::{Cursor, El, Icon, Key, KeyEvent, ScrollSpec, TextEdit, Tx, col, el, row, text};
use rune_kit::TextInput;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

/// Minimum idle time since the last keypress before a background prompt auto-opens.
pub const IDLE_DELAY: Duration = Duration::from_millis(750);
/// Minimum time a prompt must be open before Enter or primary-button submit is accepted.
pub const SUBMIT_GUARD: Duration = Duration::from_millis(300);
/// Post-cancel cool-down per host: subsequent prompts wait as a chip during this window.
pub const CANCEL_COOLDOWN: Duration = Duration::from_secs(10);
/// Maximum character count for `input` answers and `allow_other` text.
pub const MAX_INPUT_CHARS: usize = 4096;

const PICK_ROW_SINGLE: f32 = 28.;
const PICK_ROW_SUBTITLE: f32 = 40.;
const PICK_MAX_VISIBLE_ROWS: usize = 10;
const PICK_DETAIL_MAX_W: f32 = 96.;

macro_rules! prompt_icon {
    ($name:ident, $id:literal, $($el:literal),+) => {
        static $name: Icon = Icon {
            name: $id,
            view: 20.0,
            body: concat!(
                $(
                    "<",
                    $el,
                    r#" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#
                ),+
            ),
        };
    };
}

prompt_icon!(
    ICON_DOCUMENT,
    "rex-prompt-document",
    r#"path d="M5.5 3.5h6l3.5 3.5v9.5a1 1 0 0 1-1 1h-8.5a1 1 0 0 1-1-1v-12a1 1 0 0 1 1-1z""#,
    r#"path d="M11.5 3.5V7H15""#
);
prompt_icon!(
    ICON_BRANCH,
    "rex-prompt-branch",
    r#"circle cx="6.5" cy="5.5" r="1.75""#,
    r#"circle cx="6.5" cy="14.5" r="1.75""#,
    r#"circle cx="13.5" cy="7.5" r="1.75""#,
    r#"path d="M6.5 7.25v5.5""#,
    r#"path d="M13.5 9.25a3.5 3.5 0 0 1-3.5 3.5H8.25""#
);
prompt_icon!(
    ICON_CODE,
    "rex-prompt-code",
    r#"path d="m6.5 7-3 3 3 3""#,
    r#"path d="m13.5 7 3 3-3 3""#,
    r#"path d="m11 5.5-2 9""#
);
prompt_icon!(
    ICON_DATABASE,
    "rex-prompt-database",
    r#"ellipse cx="10" cy="5.5" rx="5.5" ry="2""#,
    r#"path d="M4.5 5.5v9c0 1.1 2.46 2 5.5 2s5.5-.9 5.5-2v-9""#,
    r#"path d="M4.5 10c0 1.1 2.46 2 5.5 2s5.5-.9 5.5-2""#
);
prompt_icon!(
    ICON_PACKAGE,
    "rex-prompt-package",
    r#"path d="M10 3.5 16 6.75v6.5L10 16.5l-6-3.25v-6.5z""#,
    r#"path d="M4 6.75 10 10l6-3.25""#,
    r#"path d="M10 10v6.5""#
);
prompt_icon!(
    ICON_GLOBE,
    "rex-prompt-globe",
    r#"circle cx="10" cy="10" r="6.25""#,
    r#"path d="M3.75 10h12.5""#,
    r#"ellipse cx="10" cy="10" rx="3" ry="6.25""#
);
prompt_icon!(
    ICON_LINK,
    "rex-prompt-link",
    r#"path d="M8.5 11.5a3 3 0 0 0 4.24 0l2-2a3 3 0 0 0-4.24-4.24l-1.1 1.1""#,
    r#"path d="M11.5 8.5a3 3 0 0 0-4.24 0l-2 2a3 3 0 0 0 4.24 4.24l1.1-1.1""#
);
prompt_icon!(
    ICON_PERSON,
    "rex-prompt-person",
    r#"circle cx="10" cy="7" r="2.75""#,
    r#"path d="M4.75 16a5.25 5.25 0 0 1 10.5 0""#
);
prompt_icon!(
    ICON_TAG,
    "rex-prompt-tag",
    r#"path d="M10.25 4H5a1 1 0 0 0-1 1v5.25l5.75 5.75a1.5 1.5 0 0 0 2.12 0l3.63-3.63a1.5 1.5 0 0 0 0-2.12z""#,
    r#"circle cx="7.25" cy="7.25" r="1""#
);
prompt_icon!(
    ICON_CLOCK,
    "rex-prompt-clock",
    r#"circle cx="10" cy="10" r="6.25""#,
    r#"path d="M10 6.5V10l2.5 1.5""#
);
prompt_icon!(
    ICON_STAR,
    "rex-prompt-star",
    r#"path d="m10 3.75 1.9 3.85 4.25.62-3.08 3 .73 4.23L10 13.45l-3.8 2 .73-4.23-3.08-3 4.25-.62z""#
);
prompt_icon!(
    ICON_GENERIC,
    "rex-prompt-generic",
    r#"circle cx="10" cy="10" r="3""#
);

/// Maps a wire `PromptIcon` to one of the 16 authored 20x20 stroke icons or the generic fallback.
pub fn icon_for(icon: Option<PromptIcon>) -> &'static Icon {
    match icon {
        Some(PromptIcon::Document) => &ICON_DOCUMENT,
        Some(PromptIcon::Folder) => &FOLDER,
        Some(PromptIcon::Branch) => &ICON_BRANCH,
        Some(PromptIcon::Terminal) => &TERMINAL,
        Some(PromptIcon::Code) => &ICON_CODE,
        Some(PromptIcon::Server) => &SERVER,
        Some(PromptIcon::Database) => &ICON_DATABASE,
        Some(PromptIcon::Package) => &ICON_PACKAGE,
        Some(PromptIcon::Globe) => &ICON_GLOBE,
        Some(PromptIcon::Link) => &ICON_LINK,
        Some(PromptIcon::Person) => &ICON_PERSON,
        Some(PromptIcon::Tag) => &ICON_TAG,
        Some(PromptIcon::Clock) => &ICON_CLOCK,
        Some(PromptIcon::Star) => &ICON_STAR,
        Some(PromptIcon::Gear) => &SETTINGS,
        Some(PromptIcon::Plus) => &PLUS,
        Some(PromptIcon::Generic) | None => &ICON_GENERIC,
    }
}

/// Stable identity of one live prompt offer on one host connection epoch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptKey {
    pub host: HostId,
    pub epoch: u64,
    pub call_id: String,
}

/// One active prompt held in `Workspace.prompts`, whether currently Open or Waiting.
pub struct ActivePrompt {
    pub host: HostId,
    pub epoch: u64,
    pub prompt: Arc<Prompt>,
    /// When the modal last displayed this prompt (for the 300 ms submit guard).
    pub opened_at: Option<Instant>,
    /// Draft answer text (for `Input`) or filter text (for `Pick`).
    pub edit: TextEdit,
    /// Selected row index in the current filtered pick view.
    pub selected: usize,
    /// Scroll the selected pick row into view on the next poll commit.
    pub reveal: bool,
    /// Checked item IDs in `Pick { multiple: true }`; survives filtering.
    pub checked: HashSet<String>,
    /// Inline rejection reason from `ReplyRejected` (`bad_request` / `RequestTooLarge`).
    pub error: Option<String>,
    /// Whether the 4096-character limit was just hit while editing.
    pub char_limit_hit: bool,
    /// True while a `HostCommand::PromptReply` answer is in flight.
    pub sending: bool,
    /// Tracks whether the 300 ms submit guard has already settled for redraws.
    guard_settled: bool,
}

impl ActivePrompt {
    fn new(host: HostId, epoch: u64, prompt: Arc<Prompt>) -> Self {
        let (initial_text, checked) = match &prompt.kind {
            PromptKind::Input => (prompt.value.clone().unwrap_or_default(), HashSet::new()),
            PromptKind::Pick {
                multiple, items, ..
            } => {
                let checked = if *multiple {
                    items
                        .iter()
                        .filter(|item| item.selected)
                        .map(|item| item.id.clone())
                        .collect()
                } else {
                    HashSet::new()
                };
                (String::new(), checked)
            }
        };
        let mut edit = TextEdit::new(&initial_text);
        edit.focused = true;
        Self {
            host,
            epoch,
            prompt,
            opened_at: None,
            edit,
            selected: 0,
            reveal: false,
            checked,
            error: None,
            char_limit_hit: false,
            sending: false,
            guard_settled: false,
        }
    }

    pub fn key(&self) -> PromptKey {
        PromptKey {
            host: self.host,
            epoch: self.epoch,
            call_id: self.prompt.call_id.clone(),
        }
    }

    pub fn submit_ready(&self, now: Instant) -> bool {
        self.opened_at
            .is_some_and(|opened| now.saturating_duration_since(opened) >= SUBMIT_GUARD)
    }
}

/// One row in the currently filtered pick list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickRow {
    /// Synthetic first row when `!multiple && allow_other && !query.is_empty()`.
    Other(String),
    /// Index into `PromptKind::Pick::items`.
    Item(usize),
}

/// Computes the filtered rows for a pick prompt using case-insensitive substring matching
/// across `title`, `subtitle`, and `detail`.
pub fn filtered_pick_rows(prompt: &Prompt, query: &str) -> Vec<PickRow> {
    let PromptKind::Pick {
        multiple,
        allow_other,
        items,
    } = &prompt.kind
    else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    if !multiple && *allow_other && !query.is_empty() {
        rows.push(PickRow::Other(query.to_owned()));
    }
    let needle = query.trim().to_lowercase();
    for (index, item) in items.iter().enumerate() {
        if needle.is_empty()
            || item.title.to_lowercase().contains(&needle)
            || item
                .subtitle
                .as_deref()
                .is_some_and(|s| s.to_lowercase().contains(&needle))
            || item
                .detail
                .as_deref()
                .is_some_and(|s| s.to_lowercase().contains(&needle))
        {
            rows.push(PickRow::Item(index));
        }
    }
    rows
}

/// Arrival-ordered prompt store held on `Workspace`.
#[derive(Default)]
pub struct PromptStore {
    pub queue: VecDeque<ActivePrompt>,
    pub open: Option<PromptKey>,
    pub cooldowns: HashMap<HostId, Instant>,
    pub list_fx: ScrollFx,
    scratch: TextEdit,
}

impl PromptStore {
    pub fn find(&self, key: &PromptKey) -> Option<&ActivePrompt> {
        self.queue
            .iter()
            .find(|p| p.host == key.host && p.epoch == key.epoch && p.prompt.call_id == key.call_id)
    }

    pub fn find_mut(&mut self, key: &PromptKey) -> Option<&mut ActivePrompt> {
        self.queue
            .iter_mut()
            .find(|p| p.host == key.host && p.epoch == key.epoch && p.prompt.call_id == key.call_id)
    }

    pub fn open_prompt(&self) -> Option<&ActivePrompt> {
        let key = self.open.as_ref()?;
        self.find(key)
    }

    pub fn open_prompt_mut(&mut self) -> Option<&mut ActivePrompt> {
        let key = self.open.clone()?;
        self.find_mut(&key)
    }

    pub fn waiting(&self) -> impl Iterator<Item = &ActivePrompt> {
        self.queue.iter().filter(|p| {
            self.open.as_ref().is_none_or(|open| {
                open.host != p.host || open.epoch != p.epoch || open.call_id != p.prompt.call_id
            })
        })
    }
}

fn clamp_chars(edit: &mut TextEdit, max_chars: usize) -> bool {
    if edit.text.chars().count() <= max_chars {
        return false;
    }
    let cut = edit
        .text
        .char_indices()
        .nth(max_chars)
        .map(|(byte, _)| byte)
        .unwrap_or(edit.text.len());
    edit.text.truncate(cut);
    edit.anchor = edit.anchor.min(edit.text.len());
    edit.focus = edit.focus.min(edit.text.len());
    true
}

impl Workspace {
    /// True while a `rex.ui` prompt modal is mounted on screen.
    pub fn is_prompt_open(&self) -> bool {
        matches!(
            self.panel.as_ref().map(|p| &p.kind),
            Some(PanelKind::Prompt(_))
        ) && self.prompts.open_prompt().is_some()
    }

    /// Keep direct terminal commands inert while a prompt or a session inspector is open.
    pub(super) fn sync_panel_terminal_guard(&mut self) {
        let open = self.is_prompt_open() || self.session_inspector_open();
        for term in self.terminals.values_mut() {
            if open && !term.panel_blocks_input {
                term.clear_local_echo();
            }
            term.panel_blocks_input = open;
        }
    }

    /// Reconcile `Workspace.prompts.open` against `self.panel`.
    /// If another local panel replaced `PanelKind::Prompt`, demote the prompt cleanly back
    /// to Waiting while preserving its draft text, checked items, selection, and error.
    pub fn reconcile_prompts(&mut self) {
        if let Some(open_key) = self.prompts.open.clone() {
            let still_mounted = matches!(
                self.panel.as_ref().map(|p| &p.kind),
                Some(PanelKind::Prompt(panel_key)) if *panel_key == open_key
            );
            let still_live = self.prompts.find(&open_key).is_some();
            if !still_mounted {
                self.prompts.open = None;
            } else if !still_live {
                self.prompts.open = None;
                self.close_prompt_panel(None);
            }
        }
        self.sync_panel_terminal_guard();
    }

    /// Waiting prompts in arrival order (excluding any currently open modal prompt).
    pub fn waiting_prompts(&self) -> Vec<&ActivePrompt> {
        self.prompts.waiting().collect()
    }

    /// True if at least one prompt is waiting in the queue as a chip.
    pub fn has_waiting_prompt(&self) -> bool {
        self.prompts.waiting().next().is_some()
    }

    fn has_focused_find_bar(&self) -> bool {
        self.terminals
            .values()
            .any(|t| t.search.as_ref().is_some_and(|edit| edit.focused))
    }

    fn prompt_in_cooldown(&self, host: HostId, now: Instant) -> bool {
        self.prompts
            .cooldowns
            .get(&host)
            .is_some_and(|until| now < *until)
    }

    /// Whether a newly arrived prompt for saved host `index` is allowed to auto-open now.
    pub(crate) fn can_auto_open_prompt(&self, index: usize, now: Instant) -> bool {
        let Some(host) = self.hosts.get(index) else {
            return false;
        };
        if self.panel.is_some() {
            return false;
        }
        if !self.window_focused {
            return false;
        }
        if self.in_copy_mode()
            || self.recording().is_some()
            || self.layer_drag.is_some()
            || self.drag.is_some()
            || self.bindings.sequence.depth > 0
            || self.has_focused_find_bar()
        {
            return false;
        }
        if self.prompt_in_cooldown(host.id, now) {
            return false;
        }
        let own_action = host.model.actions_in_flight > 0;
        let idle = self
            .last_keypress
            .is_none_or(|at| now.saturating_duration_since(at) >= IDLE_DELAY);
        own_action || idle
    }

    /// Toggle `settings::Host.prompts` for `host`, persist, and send `HostCommand::Prompts`.
    /// Turning it off immediately clears any local waiting or open prompt for that host.
    pub fn toggle_host_prompts(&mut self, host: usize, tx: Option<&mut Tx>) {
        if host >= self.settings.hosts.len() || host >= self.hosts.len() {
            return;
        }
        self.settings.hosts[host].prompts = !self.settings.hosts[host].prompts;
        let enabled = self.settings.hosts[host].prompts;
        self.send(host, HostCommand::Prompts(enabled));
        if !enabled {
            let host_id = self.hosts[host].id;
            self.clear_host_prompts(host_id, tx);
        }
        self.save();
    }

    /// Drop local prompt state for `host_id` without sending any wire reply.
    /// Used when toggling prompts off, removing a host, or replacing a host connection.
    pub(super) fn forget_host_prompts(&mut self, host_id: HostId) {
        self.prompts.cooldowns.remove(&host_id);
        self.clear_host_prompts(host_id, None);
    }

    fn clear_host_prompts(&mut self, host_id: HostId, tx: Option<&mut Tx>) {
        self.prompts.queue.retain(|p| p.host != host_id);
        if self
            .prompts
            .open
            .as_ref()
            .is_some_and(|open| open.host == host_id)
        {
            self.prompts.open = None;
            self.close_prompt_panel(tx);
        }
        self.sync_panel_terminal_guard();
    }

    fn close_prompt_panel(&mut self, tx: Option<&mut Tx>) {
        if !matches!(
            self.panel.as_ref().map(|p| &p.kind),
            Some(PanelKind::Prompt(_))
        ) {
            self.sync_panel_terminal_guard();
            return;
        }
        if let Some(mut back) = self.panel.as_mut().and_then(|p| p.back.take()) {
            if let Some(input) = back.inputs.first_mut() {
                input.focused = true;
            }
            self.panel = Some(*back);
        } else {
            self.panel = None;
            if let Some(tx) = tx {
                tx.blur();
            }
        }
        self.sync_panel_terminal_guard();
    }

    /// Open a queued prompt by key. If Settings is open, `open_panel` preserves it in `back`
    /// so answering or cancelling returns cleanly to Settings.
    pub fn open_prompt(&mut self, key: PromptKey, now: Instant) -> bool {
        let label = self
            .host_index(key.host)
            .map(|i| self.settings.hosts[i].label.clone())
            .unwrap_or_else(|| "host".into());
        let Some(active) = self.prompts.find_mut(&key) else {
            return false;
        };
        active.opened_at = Some(now);
        active.guard_settled = false;
        active.edit.focused = true;
        active.reveal = true;
        // Client-owned accessible name: never let a remote script pick the dialog's role label.
        let accessible_title = format!("Question from {label}");
        // Clear any pre-modal held-key identities since Rune confines key-up to the modal.
        self.forwarded.clear();
        self.open_panel(PanelKind::Prompt(key.clone()), &accessible_title, &[]);
        self.prompts.open = Some(key);
        self.sync_panel_terminal_guard();
        true
    }

    /// Open the oldest waiting prompt in the queue (used by the toolbar chip).
    pub fn open_oldest_waiting_prompt(&mut self) -> bool {
        let Some(key) = self.prompts.waiting().next().map(ActivePrompt::key) else {
            return false;
        };
        self.open_prompt(key, Instant::now())
    }

    /// Drain and reconcile `rex.ui` prompt events for host `index` during `sync`.
    pub(super) fn process_prompt_events(
        &mut self,
        index: usize,
        events: VecDeque<PromptEvent>,
        tx: &mut Tx,
    ) {
        self.process_prompt_events_at(index, events, Instant::now(), tx);
    }

    pub(crate) fn process_prompt_events_at(
        &mut self,
        index: usize,
        events: VecDeque<PromptEvent>,
        now: Instant,
        tx: &mut Tx,
    ) {
        self.reconcile_prompts();
        let host_id = self.hosts[index].id;
        let connected = self.hosts[index].model.connected;
        let epoch = self.hosts[index].model.connection_epoch;
        let label = self.settings.hosts[index].label.clone();

        // Check if an existing active prompt on this host belongs to a dropped connection or
        // an older epoch. Never send anything on disconnect.
        let mut disconnected_sending = false;
        let mut disconnected_open = false;
        let mut disconnected_waiting = false;
        let mut stale_open = false;
        self.prompts.queue.retain(|p| {
            if p.host != host_id {
                return true;
            }
            if !connected || p.epoch != epoch {
                let is_open = self.prompts.open.as_ref() == Some(&p.key());
                if is_open {
                    stale_open = true;
                }
                if p.sending {
                    disconnected_sending = true;
                } else if is_open {
                    disconnected_open = true;
                } else {
                    disconnected_waiting = true;
                }
                false
            } else {
                true
            }
        });
        if stale_open {
            self.prompts.open = None;
            self.close_prompt_panel(Some(tx));
        }
        if disconnected_sending {
            self.notify(format!(
                "{label} disconnected before confirming your answer."
            ));
        } else if disconnected_open {
            self.inform(format!(
                "{label}: disconnected while a question was open. Nothing was sent."
            ));
        } else if disconnected_waiting {
            self.inform(format!(
                "{label}: disconnected while a question was waiting. Nothing was sent."
            ));
        }

        if !connected {
            self.sync_panel_terminal_guard();
            return;
        }

        for event in events {
            if event.epoch != epoch {
                continue;
            }
            match event.kind {
                PromptEventKind::Requested(prompt) => {
                    if !self.settings.hosts[index].prompts {
                        continue;
                    }
                    // At most one active prompt per host.
                    if self
                        .prompts
                        .open
                        .as_ref()
                        .is_some_and(|open| open.host == host_id)
                    {
                        self.prompts.open = None;
                        self.close_prompt_panel(Some(tx));
                    }
                    self.prompts.queue.retain(|p| p.host != host_id);
                    let entry = ActivePrompt::new(host_id, epoch, prompt);
                    let key = entry.key();
                    self.prompts.queue.push_back(entry);
                    if self.can_auto_open_prompt(index, now) {
                        self.open_prompt(key, now);
                        tx.wake_after(SUBMIT_GUARD);
                    }
                }
                PromptEventKind::ReplyRejected { call_id, message } => {
                    let key = PromptKey {
                        host: host_id,
                        epoch,
                        call_id,
                    };
                    if let Some(active) = self.prompts.find_mut(&key) {
                        active.sending = false;
                        active.error = Some(message);
                        if self.prompts.open.as_ref() != Some(&key)
                            && self.can_auto_open_prompt(index, now)
                        {
                            self.open_prompt(key, now);
                            tx.wake_after(SUBMIT_GUARD);
                        }
                    }
                }
                PromptEventKind::Ended { call_id, reason } => {
                    let key = PromptKey {
                        host: host_id,
                        epoch,
                        call_id,
                    };
                    let existed = self.remove_prompt(&key, Some(tx));
                    if existed {
                        match reason {
                            EndReason::Answered => {
                                self.inform(format!(
                                    "{label}: question was answered on another device."
                                ));
                            }
                            EndReason::Timeout => {
                                self.inform(format!("{label}: question timed out."));
                            }
                            EndReason::Cancelled => {
                                self.inform(format!("{label}: question was cancelled."));
                            }
                            EndReason::Unknown => {
                                self.inform(format!("{label}: question ended."));
                            }
                        }
                    }
                }
                PromptEventKind::ReplyOutcome { call_id, outcome } => {
                    let key = PromptKey {
                        host: host_id,
                        epoch,
                        call_id,
                    };
                    let existed = self.remove_prompt(&key, Some(tx));
                    if existed {
                        match outcome {
                            ReplyOutcome::Accepted => {}
                            ReplyOutcome::NotFound => {
                                self.inform(format!(
                                    "{label}: question ended before your answer was used."
                                ));
                            }
                            ReplyOutcome::Failed => {
                                self.notify(format!(
                                    "{label}: answer was not confirmed. Check the host before trying again."
                                ));
                            }
                        }
                    }
                }
                PromptEventKind::Dropped => {
                    self.inform(format!(
                        "{label}: a question from a script was too large to show."
                    ));
                }
            }
        }
        self.sync_panel_terminal_guard();
    }

    fn remove_prompt(&mut self, key: &PromptKey, tx: Option<&mut Tx>) -> bool {
        let before = self.prompts.queue.len();
        self.prompts.queue.retain(|p| {
            !(p.host == key.host && p.epoch == key.epoch && p.prompt.call_id == key.call_id)
        });
        let existed = self.prompts.queue.len() != before;
        if self.prompts.open.as_ref() == Some(key) {
            self.prompts.open = None;
            self.close_prompt_panel(tx);
        }
        existed
    }

    /// Periodic poll hook: settles the 300 ms submit guard redraw and reveals selected pick rows.
    pub(super) fn poll_prompts(&mut self, now: Instant, tx: &mut Tx) -> bool {
        self.reconcile_prompts();
        let mut changed = false;
        if let Some(active) = self.prompts.open_prompt_mut() {
            if std::mem::take(&mut active.reveal) {
                tx.scroll_into_view(format!("prompt-row:{}", active.selected));
                changed = true;
            }
            if let Some(opened) = active.opened_at {
                let ready_at = opened + SUBMIT_GUARD;
                if now < ready_at {
                    tx.wake_after(ready_at.saturating_duration_since(now));
                } else if !active.guard_settled {
                    active.guard_settled = true;
                    changed = true;
                }
            }
        }
        changed
    }

    /// Mutable access to the open prompt's `TextEdit` (falls back to scratch if closed).
    pub fn prompt_edit(&mut self) -> &mut TextEdit {
        let idx = self.prompts.open.as_ref().and_then(|key| {
            self.prompts.queue.iter().position(|p| {
                p.host == key.host && p.epoch == key.epoch && p.prompt.call_id == key.call_id
            })
        });
        match idx {
            Some(idx) => &mut self.prompts.queue[idx].edit,
            None => &mut self.prompts.scratch,
        }
    }

    /// Called when the `input` field's text changes: enforces the 4096-char cap and clears errors.
    pub fn prompt_input_changed(&mut self) {
        if let Some(active) = self.prompts.open_prompt_mut() {
            active.char_limit_hit = clamp_chars(&mut active.edit, MAX_INPUT_CHARS);
            active.error = None;
        }
    }

    /// Called when the `pick` filter text changes: preserves checked items and resets selection.
    pub fn prompt_filter_changed(&mut self) {
        if let Some(active) = self.prompts.open_prompt_mut() {
            active.char_limit_hit = clamp_chars(&mut active.edit, MAX_INPUT_CHARS);
            active.selected = 0;
            active.reveal = true;
            active.error = None;
        }
    }

    pub fn select_prompt_row(&mut self, index: usize) {
        if let Some(active) = self.prompts.open_prompt_mut() {
            let count = filtered_pick_rows(&active.prompt, &active.edit.text).len();
            if index < count {
                active.selected = index;
            }
        }
    }

    fn step_prompt_row(&mut self, delta: isize) {
        if let Some(active) = self.prompts.open_prompt_mut() {
            let count = filtered_pick_rows(&active.prompt, &active.edit.text).len();
            if count == 0 {
                active.selected = 0;
                return;
            }
            let next = (active.selected as isize + delta).clamp(0, count as isize - 1) as usize;
            if next != active.selected {
                active.selected = next;
                active.reveal = true;
            }
        }
    }

    fn toggle_prompt_row_checked(&mut self, index: usize, step_after: isize) {
        if let Some(active) = self.prompts.open_prompt_mut() {
            let rows = filtered_pick_rows(&active.prompt, &active.edit.text);
            let PromptKind::Pick {
                multiple: true,
                items,
                ..
            } = &active.prompt.kind
            else {
                return;
            };
            if let Some(PickRow::Item(item_idx)) = rows.get(index) {
                let id = &items[*item_idx].id;
                if !active.checked.remove(id) {
                    active.checked.insert(id.clone());
                }
                active.error = None;
            }
            if step_after != 0 && !rows.is_empty() {
                let next = (index as isize + step_after).clamp(0, rows.len() as isize - 1) as usize;
                active.selected = next;
                active.reveal = true;
            }
        }
    }

    /// Click handler on a pick row: single pick selects and submits; multi-pick toggles check.
    pub fn click_prompt_row(&mut self, index: usize, tx: &mut Tx) {
        let multiple = self
            .prompts
            .open_prompt()
            .is_some_and(|p| matches!(p.prompt.kind, PromptKind::Pick { multiple: true, .. }));
        if multiple {
            if let Some(active) = self.prompts.open_prompt_mut() {
                active.selected = index;
            }
            self.toggle_prompt_row_checked(index, 0);
        } else {
            self.select_prompt_row(index);
            self.submit_open_prompt(tx, Instant::now(), false);
        }
    }

    /// Submit the currently open prompt, honoring repeat-ignore, 300 ms guard, and in-flight lock.
    pub fn submit_open_prompt(&mut self, tx: &mut Tx, now: Instant, is_repeat: bool) {
        if is_repeat {
            return;
        }
        let Some(open_key) = self.prompts.open.clone() else {
            return;
        };
        let Some(active) = self.prompts.find(&open_key) else {
            return;
        };
        if active.sending || !active.submit_ready(now) {
            return;
        }
        let answer = match &active.prompt.kind {
            PromptKind::Input => PromptAnswer::Text(active.edit.text.clone()),
            PromptKind::Pick {
                multiple: false,
                items,
                ..
            } => {
                let rows = filtered_pick_rows(&active.prompt, &active.edit.text);
                let Some(row) = rows.get(active.selected) else {
                    return;
                };
                match row {
                    PickRow::Other(typed) => PromptAnswer::Text(typed.clone()),
                    PickRow::Item(idx) => PromptAnswer::Ids(vec![items[*idx].id.clone()]),
                }
            }
            PromptKind::Pick {
                multiple: true,
                items,
                ..
            } => {
                let ids: Vec<String> = items
                    .iter()
                    .filter(|item| active.checked.contains(&item.id))
                    .map(|item| item.id.clone())
                    .collect();
                PromptAnswer::Ids(ids)
            }
        };

        let Some(host_index) = self.host_index(open_key.host) else {
            self.forget_host_prompts(open_key.host);
            return;
        };
        let model = &self.hosts[host_index].model;
        if !model.connected || model.connection_epoch != open_key.epoch {
            let label = self.settings.hosts[host_index].label.clone();
            self.remove_prompt(&open_key, Some(tx));
            self.inform(format!(
                "{label}: disconnected while a question was open. Nothing was sent."
            ));
            return;
        }
        if let Some(active) = self.prompts.find_mut(&open_key) {
            active.sending = true;
            active.error = None;
        }
        self.send(
            host_index,
            HostCommand::PromptReply {
                epoch: open_key.epoch,
                call_id: open_key.call_id,
                answer,
            },
        );
    }

    /// Cancel the open prompt (Escape, Cancel button, close-X): sends `Cancelled`, records
    /// the 10-second per-host cool-down, removes the prompt from the queue, and closes it.
    pub fn cancel_open_prompt(&mut self, tx: Option<&mut Tx>) {
        self.cancel_open_prompt_at(tx, Instant::now());
    }

    pub(crate) fn cancel_open_prompt_at(&mut self, tx: Option<&mut Tx>, now: Instant) {
        let Some(open_key) = self.prompts.open.clone() else {
            self.close_prompt_panel(tx);
            return;
        };
        // While an answer is in flight, keep the entry alive until server settlement so
        // `ReplyRejected` can show the inline error and `Failed` can raise its sticky notice.
        if self.prompts.find(&open_key).is_some_and(|p| p.sending) {
            return;
        }
        self.prompts.open = None;
        let mut found = false;
        self.prompts.queue.retain(|p| {
            if p.host == open_key.host
                && p.epoch == open_key.epoch
                && p.prompt.call_id == open_key.call_id
            {
                found = true;
                false
            } else {
                true
            }
        });
        self.prompts
            .cooldowns
            .insert(open_key.host, now + CANCEL_COOLDOWN);
        if found
            && let Some(host_index) = self.host_index(open_key.host)
            && self.hosts[host_index].model.connected
            && self.hosts[host_index].model.connection_epoch == open_key.epoch
        {
            self.send(
                host_index,
                HostCommand::PromptReply {
                    epoch: open_key.epoch,
                    call_id: open_key.call_id,
                    answer: PromptAnswer::Cancelled,
                },
            );
        }
        self.close_prompt_panel(tx);
    }

    /// Keyboard handler for `PanelKind::Prompt` (called from both `input::on_key` and
    /// inside the prompt's fields). Never allows any key to fall through to a terminal PTY.
    pub fn prompt_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        use super::platform::keys::{WindowMenuKey, window_menu_key};
        if window_menu_key(&ev.key, ev.mods) == Some(WindowMenuKey::Close) {
            self.cancel_open_prompt(Some(tx));
            return true;
        }
        let no_mods = !(ev.mods.ctrl || ev.mods.cmd || ev.mods.alt || ev.mods.logo);
        let is_pick = self
            .prompts
            .open_prompt()
            .is_some_and(|p| matches!(p.prompt.kind, PromptKind::Pick { .. }));
        let is_multi_pick = self
            .prompts
            .open_prompt()
            .is_some_and(|p| matches!(p.prompt.kind, PromptKind::Pick { multiple: true, .. }));
        match ev.key {
            Key::Escape if no_mods => {
                self.cancel_open_prompt(Some(tx));
                true
            }
            Key::Enter if no_mods && !ev.mods.shift => {
                if tx.key_repeat() {
                    return true;
                }
                if tx.focused() == Some("prompt-cancel") {
                    self.cancel_open_prompt(Some(tx));
                } else {
                    self.submit_open_prompt(tx, Instant::now(), false);
                }
                true
            }
            Key::Up if is_pick && no_mods => {
                self.step_prompt_row(-1);
                tx.instant();
                true
            }
            Key::Down if is_pick && no_mods => {
                self.step_prompt_row(1);
                tx.instant();
                true
            }
            Key::Tab if is_multi_pick && no_mods => {
                let selected = self.prompts.open_prompt().map(|p| p.selected).unwrap_or(0);
                let step = if ev.mods.shift { -1 } else { 1 };
                self.toggle_prompt_row_checked(selected, step);
                tx.instant();
                true
            }
            Key::Tab if no_mods => {
                // Let Rune cycle focus: field -> Cancel -> primary button -> field.
                false
            }
            _ => super::input::global_key(self, ev, tx),
        }
    }
}

fn prompt_field_key(s: &mut Workspace, ev: &KeyEvent, tx: &mut Tx) -> bool {
    s.prompt_key(ev, tx)
}

/// Waiting prompt toolbar chip, styled in `WARNING` at 28 pt height.
/// Returns `None` when no prompt is waiting.
pub fn chip(s: &Workspace, narrow: bool) -> Option<El> {
    let waiting = s.waiting_prompts();
    if waiting.is_empty() {
        return None;
    }
    let first = waiting[0];
    let host_label = s
        .host_index(first.host)
        .map(|i| s.settings.hosts[i].label.clone())
        .unwrap_or_else(|| "host".into());
    let text_label = if waiting.len() == 1 {
        format!("Question · {host_label}")
    } else {
        format!("{} questions", waiting.len())
    };
    let spoken = if waiting.len() == 1 {
        format!("Question from {host_label}")
    } else {
        format!("{} questions waiting", waiting.len())
    };
    let help = if waiting.len() == 1 {
        format!(
            "Question from {host_label}: {} · click to answer",
            first.prompt.title
        )
    } else {
        format!(
            "{} questions waiting · click to open the oldest ({host_label})",
            waiting.len()
        )
    };
    Some(super::toolbar::chip_press(
        row()
            .key("prompt-chip")
            .role("button")
            .label(spoken)
            .help(help)
            .h(28.)
            .shrink()
            .min_w(48.)
            .max_w(if narrow { 116. } else { 185. })
            .px(8.)
            .gap(5.)
            .center_y()
            .radius(10.)
            .continuous_corners()
            .fill(rgb(warning()).alpha(0.12))
            .border(1., rgb(warning()).alpha(0.42))
            .hover(rgb(warning()).alpha(0.20))
            .on_press_tx(|s: &mut Workspace, tx: &mut Tx| {
                s.open_oldest_waiting_prompt();
                tx.instant();
            })
            .child(
                text(text_label)
                    .key("prompt-chip-label")
                    .font(medium(11.5))
                    .color(rgb(warning()))
                    .truncate()
                    .shrink(),
            ),
    ))
}

/// Anti-spoofing `rex.ui` modal mounted directly on `dialogs::modal_frame_persistent`.
pub fn dialog(s: &Workspace, viewport: [f32; 2]) -> El {
    let Some(active) = s.prompts.open_prompt() else {
        return super::popups::overlay();
    };
    let k = kit();
    let now = Instant::now();
    let prompt = &active.prompt;
    let is_pick = matches!(prompt.kind, PromptKind::Pick { .. });
    let compact_h = viewport[1] < 560.;

    let (host_label, host_endpoint, host_icon) = match s.host_index(active.host) {
        Some(idx) => (
            s.settings.hosts[idx].label.clone(),
            s.settings.hosts[idx].endpoint.clone(),
            s.host_icon(idx),
        ),
        None => ("host".into(), String::new(), &SERVER),
    };

    let target_w: f32 = if is_pick { 560. } else { 520. };
    let width = target_w.min(viewport[0] * 0.9);
    let max_h = viewport[1] * 0.85;
    let inner_w = (width - 44.).max(200.);

    // 1. Client-owned anti-spoofing attribution bar.
    let mut attribution_words = row()
        .gap(6.)
        .center_y()
        .grow(1.)
        .shrink()
        .min_w(0.)
        .child(
            el().size(15., 15.)
                .center()
                .child(glyph(host_icon, 15., rgb(muted()))),
        )
        .child(text("Question from").font(ui(12.)).color(rgb(muted())))
        .child(
            text(host_label.clone())
                .key("prompt-host-label")
                .font(medium(12.))
                .color(rgb(ink()))
                .truncate()
                .shrink(),
        );
    if !host_endpoint.is_empty() {
        attribution_words = attribution_words.help(host_endpoint);
    }
    let mut close_x = row()
        .key("prompt-close")
        .role("button")
        .label("Cancel question")
        .help("Cancel question")
        .size(24., 24.)
        .center()
        .radius(8.)
        .hover(white(0.08))
        .cursor(Cursor::Pointer)
        .preserve_focus_on_press(true)
        .tab_index(-1)
        .on_press_tx(|s: &mut Workspace, tx: &mut Tx| s.cancel_open_prompt(Some(tx)))
        .child(glyph(&CLOSE, 13., rgb(muted())));
    if active.sending {
        close_x = close_x.inert().opacity(0.45);
    }

    let attribution = row()
        .key("prompt-attribution")
        .w_full()
        .gap(8.)
        .center_y()
        .child(attribution_words)
        .child(close_x);

    // 2. Script title (medium 15 pt, never the strong 17 pt local-dialog title slot).
    let title_el = text(prompt.title.clone())
        .key("prompt-title")
        .font(medium(15.))
        .color(rgb(ink()))
        .wrap()
        .max_lines(if compact_h { 2 } else { 4 })
        .help(prompt.title.clone());

    // 3. Subline naming the action or host script (each string in its own text element).
    let source_el = match &prompt.action {
        Some(action) => row()
            .key("prompt-source")
            .w_full()
            .gap(4.)
            .center_y()
            .child(text("Asked by action").font(ui(12.)).color(rgb(muted())))
            .child(
                text(action.clone())
                    .key("prompt-action-name")
                    .font(mono(12.))
                    .color(rgb(muted()))
                    .truncate()
                    .help_when_truncated(action.clone())
                    .shrink(),
            ),
        None => row().key("prompt-source").w_full().child(
            text("Asked by a script on this host")
                .font(ui(12.))
                .color(rgb(muted())),
        ),
    };

    let header = col()
        .w_full()
        .gap(4.)
        .child(attribution)
        .child(title_el)
        .child(source_el);

    let mut body = col()
        .key("prompt-modal")
        .w(width)
        .max_h(max_h)
        .padding(22.)
        .gap(14.)
        .on_key(prompt_field_key)
        .child(header);

    // 4. Kind-specific interactive content.
    let can_submit: bool;
    let default_confirm: &'static str;
    let sends_typed: bool;

    match &prompt.kind {
        PromptKind::Input => {
            can_submit = true;
            default_confirm = "Submit";
            sends_typed = true;
            let placeholder = prompt
                .placeholder
                .clone()
                .unwrap_or_else(|| "Type an answer…".into());
            let mut field_col = col().w_full().gap(6.).child(
                TextInput::new(&active.edit)
                    .placeholder(placeholder)
                    .text_style(ui(13.))
                    .height(34.)
                    .on_edit(|s: &mut Workspace| s.prompt_edit())
                    .on_change(|s: &mut Workspace, _| s.prompt_input_changed())
                    .on_key(prompt_field_key)
                    .width(None)
                    .build(k)
                    .key("prompt-input")
                    .tab_index(1),
            );
            if active.char_limit_hit {
                field_col = field_col.child(
                    text("Maximum 4,096 characters reached.")
                        .key("prompt-limit-hint")
                        .font(ui(12.))
                        .color(rgb(warning())),
                );
            }
            body = body.child(field_col);
        }
        PromptKind::Pick {
            multiple,
            allow_other,
            items,
        } => {
            default_confirm = "Choose";
            sends_typed = *allow_other;
            let rows = filtered_pick_rows(prompt, &active.edit.text);
            can_submit = *multiple || !rows.is_empty();
            let has_subtitles = items
                .iter()
                .any(|item| item.subtitle.as_deref().is_some_and(|s| !s.is_empty()));
            let row_h = if has_subtitles {
                PICK_ROW_SUBTITLE
            } else {
                PICK_ROW_SINGLE
            };

            let placeholder = prompt
                .placeholder
                .clone()
                .unwrap_or_else(|| "Filter options…".into());
            let mut filter_row = row()
                .key("prompt-filter-row")
                .w_full()
                .h(34.)
                .px(2.)
                .center_y()
                .radius(8.)
                .continuous_corners()
                .fill(white(0.045))
                .border(1., white(0.1))
                .child(
                    TextInput::new(&active.edit)
                        .placeholder(placeholder)
                        .leading_icon(&SEARCH)
                        .leading_icon_size(14.)
                        .text_style(ui(13.))
                        .height(32.)
                        .plain()
                        .on_edit(|s: &mut Workspace| s.prompt_edit())
                        .on_change(|s: &mut Workspace, _| s.prompt_filter_changed())
                        .on_key(prompt_field_key)
                        .width(None)
                        .build(k)
                        .key("prompt-filter")
                        .tab_index(1)
                        .grow(1.)
                        .shrink()
                        .min_w(0.),
                );
            if *multiple {
                filter_row = filter_row.child(
                    text(format!(
                        "{} of {} chosen",
                        active.checked.len(),
                        items.len()
                    ))
                    .key("prompt-count")
                    .font(ui(11.5))
                    .color(rgb(muted()))
                    .pr(10.),
                );
            }

            // Reserve vertical room for header, filter, optional 2-line error, footnote, buttons,
            // and gaps so 640x480 stays inside the 85% height cap even with wrapped errors.
            let chrome_reserve =
                if compact_h { 228. } else { 246. } + if active.error.is_some() { 50. } else { 0. };
            let min_rows = if compact_h && active.error.is_some() {
                2.
            } else {
                3.
            };
            let avail_list_h = (max_h - 44. - chrome_reserve).max(row_h * min_rows);
            let natural_h = if rows.is_empty() {
                row_h
            } else {
                (rows.len().min(PICK_MAX_VISIBLE_ROWS) as f32) * row_h
            };
            let list_h = natural_h.min(avail_list_h);
            let total_h = (rows.len() as f32) * row_h;
            let overflow = total_h > list_h + 1.;

            let mut list_content = col().w_full();
            if rows.is_empty() {
                list_content = list_content.child(
                    row()
                        .key("prompt-empty")
                        .w_full()
                        .h(row_h)
                        .px(9.)
                        .center_y()
                        .child(
                            text(if items.is_empty() {
                                "No options."
                            } else {
                                "No matching options."
                            })
                            .font(ui(12.5))
                            .color(rgb(muted())),
                        ),
                );
            } else {
                for (row_index, pick_row) in rows.iter().enumerate() {
                    let selected = row_index == active.selected;
                    let row_el = match pick_row {
                        PickRow::Other(typed) => {
                            let label = format!("Use \"{typed}\"");
                            list_row(
                                Row {
                                    key: format!("prompt-row:{row_index}"),
                                    surface: "prompt-pick",
                                    activity: None,
                                    program: None,
                                    icon: Some(&PLUS),
                                    subtitle: None,
                                    label: &label,
                                    detail: "Custom",
                                    detail_max_w: Some(PICK_DETAIL_MAX_W),
                                    tone: None,
                                    keys: &[],
                                    height: row_h,
                                    check: None,
                                    followup: false,
                                    enabled: true,
                                    danger: false,
                                    selected,
                                    actions: None,
                                    columns: None,
                                    hover_select: Some(Workspace::select_prompt_row),
                                },
                                row_index,
                                move |s: &mut Workspace, tx: &mut Tx| {
                                    s.click_prompt_row(row_index, tx)
                                },
                            )
                        }
                        PickRow::Item(item_idx) => {
                            let item = &items[*item_idx];
                            let checked = if *multiple {
                                Some(active.checked.contains(&item.id))
                            } else {
                                None
                            };
                            list_row(
                                Row {
                                    key: format!("prompt-row:{row_index}"),
                                    surface: "prompt-pick",
                                    activity: None,
                                    program: None,
                                    icon: Some(icon_for(item.icon)),
                                    subtitle: item.subtitle.as_deref(),
                                    label: &item.title,
                                    detail: item.detail.as_deref().unwrap_or(""),
                                    detail_max_w: Some(PICK_DETAIL_MAX_W),
                                    tone: None,
                                    keys: &[],
                                    height: row_h,
                                    check: checked,
                                    followup: false,
                                    enabled: true,
                                    danger: false,
                                    selected,
                                    actions: None,
                                    columns: None,
                                    hover_select: Some(Workspace::select_prompt_row),
                                },
                                row_index,
                                move |s: &mut Workspace, tx: &mut Tx| {
                                    s.click_prompt_row(row_index, tx)
                                },
                            )
                        }
                    };
                    list_content = list_content.child(row_el);
                }
            }

            let fx = &s.prompts.list_fx;
            let mut scroller = col()
                .key("prompt-list")
                .role("listbox")
                .w_full()
                .h(list_h)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .scroll_edge_fade(14.)
                .child(list_content);
            if overflow {
                scroller = scroller.on_scroll_offset(fx.watch(inner_w, list_h, |offset, max| {
                    if offset < max - 1. {
                        "More options below".into()
                    } else if offset > 1. {
                        "More options above".into()
                    } else {
                        String::new()
                    }
                }));
            }
            let mut list_box = el().w_full().child(scroller);
            if overflow {
                list_box = list_box.child(fx.thumb_el());
            }
            let mut pick_col = col().w_full().gap(8.).child(filter_row).child(list_box);
            if overflow {
                pick_col = pick_col.child(fx.hint_el());
            }
            body = body.child(pick_col);
        }
    }

    // 5. Inline error from ReplyRejected (`bad_request` / `RequestTooLarge`).
    if let Some(error) = &active.error {
        body = body.child(
            text(error.clone())
                .key("prompt-error")
                .font(ui(13.))
                .color(rgb(danger()))
                .wrap()
                .max_lines(2)
                .help_when_truncated(error.clone()),
        );
    }

    // 6. Client-owned destination footnote.
    let footnote_text = if sends_typed {
        format!("What you type is sent to {host_label}.")
    } else {
        format!("Your choice is sent to {host_label}.")
    };
    body = body.child(
        text(footnote_text)
            .key("prompt-footnote")
            .font(ui(12.))
            .color(rgb(muted()))
            .wrap(),
    );

    // 7. Action buttons (Cancel + primary submit with 300 ms / sending / empty guards).
    let primary_label = prompt.confirm.as_deref().unwrap_or(default_confirm);
    let ready = active.submit_ready(now);
    let sending = active.sending;

    let mut cancel_btn = pill("prompt-cancel", "Cancel", Pill::Neutral, 32., |s, tx| {
        s.cancel_open_prompt(Some(tx))
    })
    .tab_index(2);
    if sending {
        cancel_btn = cancel_btn.disabled(true).opacity(0.45);
    }

    let mut submit_btn = pill(
        "prompt-submit",
        primary_label,
        Pill::Primary,
        32.,
        |s, tx| s.submit_open_prompt(tx, Instant::now(), false),
    )
    .tab_index(3);

    if sending {
        submit_btn = submit_btn.disabled(true).opacity(0.45);
    } else if !can_submit {
        submit_btn = submit_btn.disabled(true).opacity(0.35);
    } else if !ready {
        submit_btn = submit_btn.inert().opacity(0.45);
    }

    body = body.child(
        row()
            .w_full()
            .gap(8.)
            .justify_end()
            .child(cancel_btn)
            .child(submit_btn),
    );

    dialogs::modal_frame_persistent(s, body)
}
