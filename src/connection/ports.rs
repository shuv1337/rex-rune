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

    /// Start queued reads up to [`IN_FLIGHT`]. Never sends anything below API 15.
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
            let present = {
                let model = shared.model.lock().expect("host model");
                has_view_block(&model, &session, &block)
            };
            if !present {
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
    #[derive(serde::Deserialize)]
    struct WireError {
        code: String,
        #[serde(default)]
        status: u16,
        #[serde(default)]
        message: String,
    }
    #[derive(serde::Deserialize)]
    struct Envelope {
        #[serde(default)]
        seq: u64,
        payload: Option<Value>,
        error: Option<WireError>,
    }
    let Ok(envelope) = serde_json::from_str::<Envelope>(text) else {
        return ReplyOutcome::Failed;
    };
    if let Some(error) = envelope.error {
        let api_error = rexrs::ApiError::from_wire(&error.code, error.status, error.message);
        return match api_error {
            rexrs::ApiError::MethodNotFound { .. }
            | rexrs::ApiError::RevisionTooLow { .. }
            | rexrs::ApiError::NotSupported { .. } => {
                ReplyOutcome::Unsupported { seq: envelope.seq }
            }
            _ => ReplyOutcome::Failed,
        };
    }
    ReplyOutcome::Snapshot {
        seq: envelope.seq,
        payload: envelope.payload.unwrap_or_else(|| json!({})),
    }
}

fn has_view_block(model: &HostModel, session: &str, block: &str) -> bool {
    model
        .views
        .get(session)
        .is_some_and(|v| view_blocks(v).any(|b| b == block))
}

fn clear_reading(shared: &Shared, blocks: &[String]) {
    let mut model = shared.model.lock().expect("host model");
    let mut changed = false;
    for block in blocks {
        changed |= model.ports_reading.remove(block);
    }
    drop(model);
    if changed {
        let _ = shared.wake.try_send(());
    }
}

/// Set or clear the open/watched ports session. When `Some(session)`, starts on-demand reads for
/// that session's terminal blocks (API 15+ only). When `None`, cancels all queued and in-flight
/// port reads.
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
            let _ = shared.wake.try_send(());
        }
        return Ok(());
    };

    state.ports_reads.watched = Some(session.clone());
    let cancelled = state.ports_reads.cancel_other_sessions(&session);
    if !cancelled.is_empty() {
        clear_reading(shared, &cancelled);
    }
    enqueue_session_blocks(control, shared, state, &session).await
}

/// Trigger one-shot on-demand reads for all terminal blocks in `session` (API 15+ only) without
/// changing the watched session.
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
            let _ = shared.wake.try_send(());
        }
    }
    state.ports_reads.pump(control, shared).await
}

/// Apply a passive `ports_changed` block event.
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
        let _ = shared.wake.try_send(());
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
                changed |= store_snapshot(&mut model, &finished.block, snapshot);
                state.ports_seq.insert(finished.block, seq);
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
        let _ = shared.wake.try_send(());
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
/// Drops affected cached snapshots, advances `ports_seq` to at least `drop_seq` so stale
/// in-flight replies cannot overwrite, cancels affected in-flight reads, and queues re-reads
/// only for blocks belonging to the currently watched session (on API 15+).
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
        state.ports_reads.cancel_block(block);
        let entry = state.ports_seq.entry(block.to_owned()).or_insert(0);
        *entry = (*entry).max(drop_seq);

        let mut model = shared.model.lock().expect("host model");
        changed |= model.ports.remove(block).is_some();
        changed |= model.ports_reading.remove(block);
        if let Some(watched_sid) = &watched {
            let belongs = session_id.is_none_or(|s| s == watched_sid)
                && has_view_block(&model, watched_sid, block);
            if belongs {
                reread.push((watched_sid.clone(), block.to_owned()));
            }
        }
    } else if let Some(sid) = session_id {
        let was_watched = watched.as_deref() == Some(sid);
        state.ports_reads.cancel_session(sid);
        if was_watched {
            state.ports_reads.watched = Some(sid.to_owned());
        }
        let mut model = shared.model.lock().expect("host model");
        let blocks: Vec<String> = model
            .views
            .get(sid)
            .map(|v| view_blocks(v).collect())
            .unwrap_or_default();
        for block in blocks {
            let entry = state.ports_seq.entry(block.clone()).or_insert(0);
            *entry = (*entry).max(drop_seq);
            changed |= model.ports.remove(&block).is_some();
            changed |= model.ports_reading.remove(&block);
            if was_watched {
                reread.push((sid.to_owned(), block));
            }
        }
    } else {
        state.ports_reads.clear();
        state.ports_reads.watched = watched.clone();
        let mut model = shared.model.lock().expect("host model");
        let mut all_blocks: HashSet<String> = model.ports.keys().cloned().collect();
        for view in model.views.values() {
            all_blocks.extend(view_blocks(view));
        }
        for block in all_blocks {
            let entry = state.ports_seq.entry(block).or_insert(0);
            *entry = (*entry).max(drop_seq);
        }
        changed |= !model.ports.is_empty();
        changed |= !model.ports_reading.is_empty();
        model.ports.clear();
        model.ports_reading.clear();
        if let Some(watched_sid) = &watched
            && let Some(view) = model.views.get(watched_sid)
        {
            for block in view_blocks(view) {
                reread.push((watched_sid.clone(), block));
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
        state.ports_reads.pump(control, shared).await?;
    }

    if changed {
        let _ = shared.wake.try_send(());
    }
    Ok(())
}
