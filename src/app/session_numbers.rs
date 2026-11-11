//! Go to session by number, and the keys the switcher's rows show for it.
//!
//! Sessions are counted as the switcher lists them: host after host in the saved order, each
//! host's sessions in the server's order. While the switcher is open the count is of the rows
//! it shows, so with a filter typed the first match is session 1.
use super::{
    actions::{self, When},
    keycaps::Chords,
    panel::{Entry, Intent, PanelKind},
    workspace::Workspace,
};
use rune::Tx;
use serde_json::Value;

pub const GOTO: &str = "session.goto";

impl Workspace {
    /// Whether the session switcher is open and listing sessions, not renaming one.
    pub(super) fn switching(&self) -> bool {
        self.panel.as_ref().is_some_and(|panel| {
            matches!(panel.kind, PanelKind::Picker(true)) && panel.rename.is_none()
        })
    }

    /// The sessions Go to session counts, as (host, session).
    fn numbered_sessions(&self) -> Vec<(usize, String)> {
        if self.switching() {
            return self
                .entries()
                .into_iter()
                .filter_map(|entry| match entry.action {
                    Intent::Select(host, session) => Some((host, session)),
                    _ => None,
                })
                .collect();
        }
        self.hosts
            .iter()
            .enumerate()
            .flat_map(|(index, host)| {
                host.model
                    .sessions
                    .iter()
                    .map(move |session| (index, session.session_id.clone()))
            })
            .collect()
    }

    /// Show session `index` (from 1; -1 is the last), as choosing its row in the switcher
    /// does. A number past the end does nothing.
    pub(super) fn goto_session(&mut self, index: i64, tx: &mut Tx) {
        let sessions = self.numbered_sessions();
        let place = actions::tab_index(&serde_json::json!({ "index": index }), sessions.len());
        if let Some((host, session)) = place.and_then(|place| sessions.into_iter().nth(place)) {
            self.perform(Intent::Select(host, session), tx);
        }
    }

    /// Give the switcher's session rows the keys that show them: row n the effective binding
    /// of Go to session n, and the last row that of Go to last session when it has no other.
    pub(super) fn number_sessions(&self, entries: &mut [Entry]) {
        // A row being renamed has the keyboard, and no number picks a row meanwhile.
        if !self.switching() {
            return;
        }
        // Each number's first binding in the order keys are tried, among those that can run
        // with the switcher open.
        let mut keys: Vec<(i64, Chords)> = Vec::new();
        for bound in self.effective_bindings() {
            let binding = &bound.binding;
            let args = binding.args.as_ref().unwrap_or(&Value::Null);
            if binding.action != GOTO
                || actions::when(args).is_some_and(|when| when != When::Switching)
            {
                continue;
            }
            let (Some(index), Some(chords)) = (args["index"].as_i64(), bound.chords.as_ref())
            else {
                continue;
            };
            if !keys.iter().any(|(known, _)| *known == index) {
                keys.push((index, chords.iter().map(|chord| chord.keycaps()).collect()));
            }
        }
        if keys.is_empty() {
            return;
        }
        let chords = |index: i64| {
            keys.iter()
                .find(|(known, _)| *known == index)
                .map(|(_, chords)| chords.clone())
        };
        let mut rows: Vec<&mut Entry> = entries
            .iter_mut()
            .filter(|entry| matches!(entry.action, Intent::Select(..)))
            .collect();
        let last = rows.len();
        for (place, entry) in rows.iter_mut().enumerate() {
            let number = chords(place as i64 + 1);
            if let Some(keys) = number.or_else(|| chords(-1).filter(|_| place + 1 == last)) {
                entry.detail = super::keycaps::spell(&keys);
                entry.keys = keys;
            }
        }
    }
}
