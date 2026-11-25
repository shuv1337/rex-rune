//! On-demand detached `com.superlogical.terminal.ports` reads (API 15+) and passive
//! `ports_changed` block-event sync (API 16+).
//!
//! Zero eager reads happen at attach and zero reads run for background sessions.
use super::{Client, ControlState, HostModel, Shared, activity, view_blocks};
use crate::ports::{MAX_PORTS_PER_HOST, PORTS_MIN_API_REVISION, PortSnapshot};
use serde_json::{Value, json};
use std::{
    collections::{HashSet, VecDeque},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

/// Maximum concurrent detached `com.superlogical.terminal.ports` reads per host connection.
pub(super) const IN_FLIGHT: usize = 3;
/// Per-read deadline for detached `com.superlogical.terminal.ports` calls.
const READ_TIMEOUT: Duration = Duration::from_secs(8);
const METHOD: &str = "com.superlogical.terminal.ports";

/// True when a `block_event` payload is a terminal `ports_changed` event.
pub fn is_ports_changed(payload: &Value) -> bool {
    payload["creator_name"] == activity::TERMINAL && payload["name"] == "ports_changed"
}

pub(super) struct FinishedRead {
    pub session: String,
    pub block: String,
    pub outcome: ReplyOutcome,
}

pub(super) enum ReplyOutcome {
    Snapshot { seq: u64, payload: Value },
    Unsupported { seq: u64 },
    Failed,
}

struct Read {
    session: String,
    block: String,
    reply: oneshot::Receiver<String>,
    deadline: std::pin::Pin<Box<tokio::time::Sleep>>,
}

/// Manages detached on-demand `com.superlogical.terminal.ports` reads for one connection.
#[derive(Default)]
pub(super) struct Reader {
    watched: Option<String>,
    queue: VecDeque<(String, String)>,
    reads: Vec<Read>,
}

impl Reader {
    pub(super) fn in_flight(&self) -> usize {
        self.reads.len()
    }

    pub(super) fn can_pump(&self) -> bool {
        !self.queue.is_empty() && self.reads.len() < IN_FLIGHT
    }

    pub(super) fn watched(&self) -> Option<&str> {
        self.watched.as_deref()
    }

    fn has_block(&self, block: &str) -> bool {
        self.reads.iter().any(|r| r.block == block) || self.queue.iter().any(|(_, b)| b == block)
    }

    pub(super) fn cancel_block(&mut self, block: &str) {
        self.reads.retain(|read| read.block != block);
        self.queue.retain(|(_, b)| b != block);
    }

    pub(super) fn cancel_session(&mut self, session: &str) {
        self.reads.retain(|read| read.session != session);
        self.queue.retain(|(s, _)| s != session);
        if self.watched.as_deref() == Some(session) {
            self.watched = None;
        }
    }

    pub(super) fn clear(&mut self) {
        self.watched = None;
        self.queue.clear();
        self.reads.clear();
    }

    fn all_blocks(&self) -> Vec<String> {
        self.reads
            .iter()
            .map(|r| r.block.clone())
            .chain(self.queue.iter().map(|(_, b)| b.clone()))
            .collect()
    }

    fn cancel_other_sessions(&mut self, keep_session: &str) -> Vec<String> {
        let mut cancelled = Vec::new();
        self.reads.retain(|read| {
            let keep = read.session == keep_session;
            if !keep {
                cancelled.push(read.block.clone());
            }
            keep
        });
        self.queue.retain(|(s, b)| {
            let keep = s == keep_session;
            if !keep {
                cancelled.push(b.clone());
            }
            keep
        });
        cancelled
    }

    fn enqueue(&mut self, session: &str, block: String) -> bool {
        if self.has_block(&block) {
            return false;
        }
        self.queue.push_back((session.to_owned(), block));
        true
    }

    /// Queue newly added terminal blocks when the currently watched foreground session's view
    /// gains blocks (for example, opening a split or undo-restoring a pane while Ports is open).
    /// Caller must pump queued reads afterward when a slot is free.
    pub(super) fn enqueue_new_watched_blocks(
        &mut self,
        model: &mut HostModel,
        session: &str,
        added: impl IntoIterator<Item = String>,
    ) {
        if self.watched.as_deref() != Some(session) || !is_eligible_session(model, session) {
            return;
        }
        for block in added {
            if self.enqueue(session, block.clone()) {
                model.ports_reading.insert(block);
            }
        }
    }

    /// Start queued reads up to [`IN_FLIGHT`]. Never sends anything below API 15 or for
    /// ineligible/background sessions.
    pub(super) async fn pump(
        &mut self,
        control: &mut Client,
        shared: &Shared,
    ) -> rexrs::Result<()> {
        if control.welcome().api_revision < PORTS_MIN_API_REVISION {
            let dropped: Vec<String> = self.queue.drain(..).map(|(_, b)| b).collect();
            if !dropped.is_empty() {
                clear_reading(shared, &dropped);
            }
            return Ok(());
        }
        while self.reads.len() < IN_FLIGHT {
            let Some((session, block)) = self.queue.pop_front() else {
                break;
            };
            let eligible = {
                let model = shared.model.lock().expect("host model");
                is_eligible_session(&model, &session) && has_view_block(&model, &session, &block)
            };
            if !eligible {
                clear_reading(shared, std::slice::from_ref(&block));
                continue;
            }
            let reply = control
                .transport()
                .detach(
                    METHOD,
                    json!({"session_id": session, "block_id": block, "args": {}}),
                )
                .await?;
            self.reads.push(Read {
                session,
                block,
                reply,
                deadline: Box::pin(tokio::time::sleep_until(tokio::time::Instant::from_std(
                    Instant::now() + READ_TIMEOUT,
                ))),
            });
        }
        Ok(())
    }

    /// The first read to answer or expire. Cancel-safe: removes a read only when returning it.
    pub(super) async fn finished(&mut self) -> FinishedRead {
        use std::{future::Future, pin::Pin, task::Poll};
        if self.reads.is_empty() {
            return std::future::pending().await;
        }
        let (index, text) = std::future::poll_fn(|cx| {
            for (index, read) in self.reads.iter_mut().enumerate() {
                if let Poll::Ready(reply) = Pin::new(&mut read.reply).poll(cx) {
                    return Poll::Ready((index, reply.ok()));
                }
                if read.deadline.as_mut().poll(cx).is_ready() {
                    return Poll::Ready((index, None));
                }
            }
            Poll::Pending
        })
        .await;
        let read = self.reads.remove(index);
        let outcome = text.as_deref().map_or(ReplyOutcome::Failed, decode_reply);
        FinishedRead {
            session: read.session,
            block: read.block,
            outcome,
        }
    }
}

fn decode_reply(text: &str) -> ReplyOutcome {
    match crate::transport::detached_result_with_seq::<Value>(text) {
        Ok(reply) => ReplyOutcome::Snapshot {
            seq: reply.seq,
            payload: reply.value,
        },
        Err(rexrs::Error::Api(
            rexrs::ApiError::MethodNotFound { .. }
            | rexrs::ApiError::RevisionTooLow { .. }
            | rexrs::ApiError::NotSupported { .. },
        )) => {
            #[derive(serde::Deserialize)]
            struct SeqOnly {
                #[serde(default)]
                seq: u64,
            }
            let seq = serde_json::from_str::<SeqOnly>(text).map_or(0, |s| s.seq);
            ReplyOutcome::Unsupported { seq }
        }
        Err(_) => ReplyOutcome::Failed,
    }
}

/// Only the host's currently selected foreground session (not background-prefetched views) is
/// eligible for on-demand `com.superlogical.terminal.ports` reads.
fn is_eligible_session(model: &HostModel, session: &str) -> bool {
    model.selected.as_deref() == Some(session)
        && !model.background_views.contains(session)
        && model.views.contains_key(session)
}

fn has_view_block(model: &HostModel, session: &str, block: &str) -> bool {
    model
        .views
        .get(session)
        .is_some_and(|v| view_blocks(v).any(|b| b == block))
}

fn find_view_session(model: &HostModel, block: &str) -> Option<String> {
    model
        .views
        .iter()
        .find(|(_, v)| view_blocks(v).any(|b| b == block))
        .map(|(sid, _)| sid.clone())
}

fn clear_reading(shared: &Shared, blocks: &[String]) {
    let mut model = shared.model.lock().expect("host model");
    let mut changed = false;
    for block in blocks {
        changed |= model.ports_reading.remove(block);
    }
    drop(model);
    if changed {
        shared.wake.notify();
    }
}

/// Cancel queued/in-flight reads and drop the watched session when the host switches away from it
/// (or loses its selected session).
pub(super) fn reconcile_selected(shared: &Shared, state: &mut ControlState) {
    let selected = shared.model.lock().expect("host model").selected.clone();
    let cancelled = match selected.as_deref() {
        Some(sid) => {
            if state.ports_reads.watched.as_deref() != Some(sid) {
                state.ports_reads.watched = None;
            }
            state.ports_reads.cancel_other_sessions(sid)
        }
        None => {
            let blocks = state.ports_reads.all_blocks();
            state.ports_reads.clear();
            blocks
        }
    };
    if !cancelled.is_empty() {
        clear_reading(shared, &cancelled);
    }
}

/// Set or clear the open/watched ports session. When `Some(session)`, starts on-demand reads for
/// that session's terminal blocks only if it is the active foreground session (API 15+ only).
/// When `None`, cancels all queued and in-flight port reads.
pub(super) async fn watch(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    session: Option<String>,
) -> rexrs::Result<()> {
    let Some(session) = session else {
        state.ports_reads.clear();
        let changed = {
            let mut model = shared.model.lock().expect("host model");
            let had = !model.ports_reading.is_empty();
            model.ports_reading.clear();
            had
        };
        if changed {
            shared.wake.notify();
        }
        return Ok(());
    };

    if control.welcome().api_revision < PORTS_MIN_API_REVISION {
        state.ports_reads.clear();
        let changed = {
            let mut model = shared.model.lock().expect("host model");
            let had = !model.ports_reading.is_empty();
            model.ports_reading.clear();
            had
        };
        if changed {
            shared.wake.notify();
        }
        return Ok(());
    }

    let eligible = {
        let model = shared.model.lock().expect("host model");
        is_eligible_session(&model, &session)
    };
    if !eligible {
        return Ok(());
    }

    state.ports_reads.watched = Some(session.clone());
    let cancelled = state.ports_reads.cancel_other_sessions(&session);
    if !cancelled.is_empty() {
        clear_reading(shared, &cancelled);
    }
    enqueue_session_blocks(control, shared, state, &session).await
}

/// Trigger one-shot on-demand reads for all terminal blocks in the active foreground `session`
/// (API 15+ only) without changing the watched session.
pub(super) async fn read_once(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    session: &str,
) -> rexrs::Result<()> {
    enqueue_session_blocks(control, shared, state, session).await
}

async fn enqueue_session_blocks(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    session: &str,
) -> rexrs::Result<()> {
    if control.welcome().api_revision < PORTS_MIN_API_REVISION {
        return Ok(());
    }
    let blocks: Vec<String> = {
        let model = shared.model.lock().expect("host model");
        if !is_eligible_session(&model, session) {
            return Ok(());
        }
        model
            .views
            .get(session)
            .map(|view| view_blocks(view).collect())
            .unwrap_or_default()
    };
    if blocks.is_empty() {
        return Ok(());
    }
    let mut added = Vec::new();
    for block in blocks {
        if state.ports_reads.enqueue(session, block.clone()) {
            added.push(block);
        }
    }
    if !added.is_empty() {
        let changed = {
            let mut model = shared.model.lock().expect("host model");
            let mut changed = false;
            for block in &added {
                changed |= model.ports_reading.insert(block.clone());
            }
            changed
        };
        if changed {
            shared.wake.notify();
        }
    }
    state.ports_reads.pump(control, shared).await
}

/// Apply a passive `ports_changed` block event (caller must gate on API 16+).
///
/// Follows the Rex combining rule: an event applies only when `event_seq > ports_seq[block]`
/// (strictly greater). Only blocks currently present in a held view are stored. Wakes the UI
/// only if the stored snapshot changed.
pub(super) fn apply_event(
    shared: &Shared,
    state: &mut ControlState,
    session: &str,
    block: &str,
    event_seq: u64,
    body: &Value,
) {
    let last_seq = state.ports_seq.get(block).copied().unwrap_or(0);
    if event_seq <= last_seq {
        return;
    }
    let mut model = shared.model.lock().expect("host model");
    if !has_view_block(&model, session, block) {
        return;
    }
    let snapshot = PortSnapshot::parse(body);
    let changed = store_snapshot(&mut model, block, snapshot);
    state.ports_seq.insert(block.to_owned(), event_seq);
    drop(model);
    if changed {
        shared.wake.notify();
    }
}

/// Apply the result of a finished detached `com.superlogical.terminal.ports` read.
///
/// Follows the Rex combining rule: a reply applies when `reply_seq >= ports_seq[block]` (tie goes
/// to the reply). Wakes the UI only if `ports` or `ports_reading` changed.
pub(super) fn record_reply(shared: &Shared, state: &mut ControlState, finished: FinishedRead) {
    let mut model = shared.model.lock().expect("host model");
    let mut changed = model.ports_reading.remove(&finished.block);
    let last_seq = state.ports_seq.get(&finished.block).copied().unwrap_or(0);

    match finished.outcome {
        ReplyOutcome::Snapshot { seq, payload } => {
            if seq >= last_seq && has_view_block(&model, &finished.session, &finished.block) {
                let snapshot = PortSnapshot::parse(&payload);
                let _ = store_snapshot(&mut model, &finished.block, snapshot);
                state.ports_seq.insert(finished.block, seq);
                model.ports_read_completions = model.ports_read_completions.saturating_add(1);
                changed = true;
            }
        }
        ReplyOutcome::Unsupported { seq } => {
            if seq >= last_seq && has_view_block(&model, &finished.session, &finished.block) {
                changed |= store_snapshot(&mut model, &finished.block, PortSnapshot::unsupported());
                state.ports_seq.insert(finished.block, seq);
            }
        }
        ReplyOutcome::Failed => {}
    }

    drop(model);
    if changed {
        shared.wake.notify();
    }
}

/// Store `snapshot` for `block` after enforcing the host-wide port budget (`MAX_PORTS_PER_HOST`).
/// Returns `true` if `model.ports` actually changed.
fn store_snapshot(model: &mut HostModel, block: &str, mut snapshot: PortSnapshot) -> bool {
    let other_ports: usize = model
        .ports
        .iter()
        .filter(|(id, _)| id.as_str() != block)
        .map(|(_, snap)| snap.ports.len())
        .sum();
    let remaining = MAX_PORTS_PER_HOST.saturating_sub(other_ports);
    snapshot.apply_host_budget(remaining);
    if model.ports.get(block) == Some(&snapshot) {
        return false;
    }
    model.ports.insert(block.to_owned(), snapshot);
    true
}

/// Recover from an `event_dropped` notice that may have dropped `ports_changed` events.
///
/// Guards against unheld blocks, mismatched `(session_id, block_id)` pairs, and drops already
/// superseded by a stored snapshot (`existing_seq >= drop_seq`). Drops stale snapshots, advances
/// `ports_seq` to at least `drop_seq`, cancels affected in-flight reads, re-reads only blocks in
/// the currently watched foreground session (API 15+), and pumps any queued reads into freed slots.
pub(super) async fn handle_dropped(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    session_id: Option<&str>,
    block_id: Option<&str>,
    drop_seq: u64,
) -> rexrs::Result<()> {
    let watched = state.ports_reads.watched().map(str::to_owned);
    let mut reread: Vec<(String, String)> = Vec::new();
    let mut changed = false;

    if let Some(block) = block_id {
        let mut model = shared.model.lock().expect("host model");
        // Verify held-view membership and (when provided) session ownership before mutating state.
        let owner_session = match session_id {
            Some(sid) => has_view_block(&model, sid, block).then(|| sid.to_owned()),
            None => find_view_session(&model, block),
        };
        let Some(owner_session) = owner_session else {
            return Ok(());
        };
        let existing_seq = state.ports_seq.get(block).copied().unwrap_or(0);
        if model.ports.contains_key(block) && existing_seq >= drop_seq {
            return Ok(());
        }
        state.ports_reads.cancel_block(block);
        state
            .ports_seq
            .insert(block.to_owned(), existing_seq.max(drop_seq));
        changed |= model.ports.remove(block).is_some();
        changed |= model.ports_reading.remove(block);
        if watched.as_deref() == Some(owner_session.as_str())
            && is_eligible_session(&model, &owner_session)
        {
            reread.push((owner_session, block.to_owned()));
        }
    } else if let Some(sid) = session_id {
        let mut model = shared.model.lock().expect("host model");
        let blocks: Vec<String> = model
            .views
            .get(sid)
            .map(|v| view_blocks(v).collect())
            .unwrap_or_default();
        let eligible_watch = watched.as_deref() == Some(sid) && is_eligible_session(&model, sid);
        for block in blocks {
            let existing_seq = state.ports_seq.get(&block).copied().unwrap_or(0);
            if model.ports.contains_key(&block) && existing_seq >= drop_seq {
                continue;
            }
            state.ports_reads.cancel_block(&block);
            state
                .ports_seq
                .insert(block.clone(), existing_seq.max(drop_seq));
            changed |= model.ports.remove(&block).is_some();
            changed |= model.ports_reading.remove(&block);
            if eligible_watch {
                reread.push((sid.to_owned(), block));
            }
        }
    } else {
        let mut model = shared.model.lock().expect("host model");
        let mut held: Vec<(String, String)> = Vec::new();
        let mut seen = HashSet::new();
        for (sid, view) in &model.views {
            for block in view_blocks(view) {
                if seen.insert(block.clone()) {
                    held.push((sid.clone(), block));
                }
            }
        }
        for (sid, block) in held {
            let existing_seq = state.ports_seq.get(&block).copied().unwrap_or(0);
            if model.ports.contains_key(&block) && existing_seq >= drop_seq {
                continue;
            }
            state.ports_reads.cancel_block(&block);
            state
                .ports_seq
                .insert(block.clone(), existing_seq.max(drop_seq));
            changed |= model.ports.remove(&block).is_some();
            changed |= model.ports_reading.remove(&block);
            if watched.as_deref() == Some(sid.as_str()) && is_eligible_session(&model, &sid) {
                reread.push((sid, block));
            }
        }
    }

    if control.welcome().api_revision >= PORTS_MIN_API_REVISION && !reread.is_empty() {
        let mut added = Vec::new();
        for (sid, block) in reread {
            if state.ports_reads.enqueue(&sid, block.clone()) {
                added.push(block);
            }
        }
        if !added.is_empty() {
            let mut model = shared.model.lock().expect("host model");
            for block in added {
                changed |= model.ports_reading.insert(block);
            }
        }
    }

    // Pump both newly queued re-reads and any previously queued reads whose slot just freed up.
    if state.ports_reads.can_pump() {
        state.ports_reads.pump(control, shared).await?;
    }

    if changed {
        shared.wake.notify();
    }
    Ok(())
}
