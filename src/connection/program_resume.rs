//! Detached on-demand Program Resume reads and passive block-event sync (API 24+).
use super::{Client, ControlState, HostModel, Shared, activity, view_blocks};
use crate::{
    program_resume::{MAX_RECORDS_PER_HOST, MIN_API_REVISION, ResumeSnapshot},
    transport::DetachedEnvelope,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::VecDeque, time::Duration};
use tokio::{sync::oneshot, time::Sleep};

pub(super) const EVENT: &str = "program_resume_changed";
const METHOD: &str = "com.superlogical.terminal.program_resume_records";
const IN_FLIGHT: usize = 2;
const READ_TIMEOUT: Duration = Duration::from_secs(8);

pub(super) fn is_resume_changed(payload: &Value) -> bool {
    payload["creator_name"] == activity::TERMINAL && payload["name"] == EVENT
}

#[derive(Debug)]
enum ReplyOutcome {
    Snapshot { seq: u64, snapshot: ResumeSnapshot },
    Unsupported,
    Failed,
}

pub(super) struct FinishedRead {
    session: String,
    block: String,
    outcome: ReplyOutcome,
}

struct Read {
    session: String,
    block: String,
    reply: oneshot::Receiver<String>,
    deadline: std::pin::Pin<Box<Sleep>>,
}

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

    pub(super) fn cancel_block(&mut self, block: &str) {
        self.reads.retain(|read| read.block != block);
        self.queue.retain(|(_, id)| id != block);
    }

    pub(super) fn cancel_session(&mut self, session: &str) {
        if self.watched.as_deref() == Some(session) {
            *self = Self::default();
        }
    }

    fn clear(&mut self, model: &mut HostModel) -> bool {
        *self = Self::default();
        let changed = !model.program_resume_reading.is_empty();
        model.program_resume_reading.clear();
        changed
    }

    fn enqueue(&mut self, model: &mut HostModel, session: &str, block: String) -> bool {
        if model.program_resume_reading.insert(block.clone()) {
            self.queue.push_back((session.to_owned(), block));
            true
        } else {
            false
        }
    }

    pub(super) fn enqueue_new_watched_blocks(
        &mut self,
        model: &mut HostModel,
        session: &str,
        added: impl IntoIterator<Item = String>,
    ) -> bool {
        let mut changed = false;
        if self.watched.as_deref() == Some(session) && eligible(model, session) {
            for block in added {
                changed |= self.enqueue(model, session, block);
            }
        }
        changed
    }

    pub(super) async fn pump(
        &mut self,
        control: &mut Client,
        shared: &Shared,
    ) -> rexrs::Result<()> {
        while self.reads.len() < IN_FLIGHT {
            let Some((session, block)) = self.queue.pop_front() else {
                break;
            };
            {
                let mut model = shared.model.lock().expect("host model");
                if control.welcome().api_revision < MIN_API_REVISION
                    || !eligible(&model, &session)
                    || !has_block(&model, &session, &block)
                {
                    let changed = model.program_resume_reading.remove(&block);
                    drop(model);
                    if changed {
                        shared.wake.notify();
                    }
                    continue;
                }
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
                deadline: Box::pin(tokio::time::sleep(READ_TIMEOUT)),
            });
        }
        Ok(())
    }

    /// Cancel-safe. A read leaves the queue only when a reply or its deadline wins.
    pub(super) async fn finished(&mut self) -> FinishedRead {
        use std::{future::Future, pin::Pin, task::Poll};
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
        FinishedRead {
            session: read.session,
            block: read.block,
            outcome: text.as_deref().map_or(ReplyOutcome::Failed, decode_reply),
        }
    }
}

fn decode_reply(text: &str) -> ReplyOutcome {
    // Do not deserialize the remote message or details, which could echo a secret.
    #[derive(Deserialize)]
    struct Error {
        code: ErrorCode,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum ErrorCode {
        MethodNotFound,
        RevisionTooLow,
        NotSupported,
        #[serde(other)]
        Other,
    }
    let Ok(reply) = serde_json::from_str::<DetachedEnvelope<'_, Error>>(text) else {
        return ReplyOutcome::Failed;
    };
    if let Some(error) = reply.error {
        return match error.code {
            ErrorCode::MethodNotFound | ErrorCode::RevisionTooLow | ErrorCode::NotSupported => {
                ReplyOutcome::Unsupported
            }
            ErrorCode::Other => ReplyOutcome::Failed,
        };
    }
    let Some(snapshot) = reply
        .payload
        .and_then(|raw| ResumeSnapshot::decode_metadata(raw.get()))
    else {
        return ReplyOutcome::Failed;
    };
    ReplyOutcome::Snapshot {
        seq: reply.seq,
        snapshot,
    }
}

fn eligible(model: &HostModel, session: &str) -> bool {
    !model.program_resume_unsupported
        && model.selected.as_deref() == Some(session)
        && !model.background_views.contains(session)
        && model.views.contains_key(session)
}

fn has_block(model: &HostModel, session: &str, block: &str) -> bool {
    model
        .views
        .get(session)
        .is_some_and(|view| view_blocks(view).any(|id| id == block))
}

pub(super) fn reconcile_selected(shared: &Shared, state: &mut ControlState) {
    let changed = if let Some(session) = state.resume_reads.watched.as_deref() {
        let mut model = shared.model.lock().expect("host model");
        !eligible(&model, session) && state.resume_reads.clear(&mut model)
    } else {
        false
    };
    if changed {
        shared.wake.notify();
    }
}

pub(super) async fn watch(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    session: Option<String>,
) -> rexrs::Result<()> {
    if control.welcome().api_revision < MIN_API_REVISION {
        return Ok(());
    }
    let changed = {
        let mut model = shared.model.lock().expect("host model");
        if let Some(session) = session {
            // A watch sent before the selected view arrives is dropped. The UI must resend it
            // when the view arrives.
            if state.resume_reads.watched.as_deref() == Some(&session)
                || !eligible(&model, &session)
            {
                return Ok(());
            }
            let blocks: Vec<_> = view_blocks(&model.views[&session]).collect();
            let mut changed = state.resume_reads.clear(&mut model);
            state.resume_reads.watched = Some(session.clone());
            changed |= state
                .resume_reads
                .enqueue_new_watched_blocks(&mut model, &session, blocks);
            changed
        } else {
            state.resume_reads.clear(&mut model)
        }
    };
    if changed {
        shared.wake.notify();
    }
    state.resume_reads.pump(control, shared).await
}

pub(super) fn apply_event(
    shared: &Shared,
    state: &mut ControlState,
    session: &str,
    block: &str,
    seq: u64,
    body: &Value,
) {
    if seq <= state.resume_seq.get(block).copied().unwrap_or(0) {
        return;
    }
    let mut model = shared.model.lock().expect("host model");
    if !has_block(&model, session, block) {
        return;
    }
    // The server sends only metadata in events. Use the same typed parser as for replies.
    let Some(snapshot) = serde_json::to_string(body)
        .ok()
        .and_then(|text| ResumeSnapshot::decode_metadata(&text))
    else {
        return;
    };
    state.resume_seq.insert(block.to_owned(), seq);
    let changed = store_snapshot(&mut model, block, snapshot);
    drop(model);
    if changed {
        shared.wake.notify();
    }
}

pub(super) fn record_reply(shared: &Shared, state: &mut ControlState, finished: FinishedRead) {
    let mut model = shared.model.lock().expect("host model");
    let mut changed = model.program_resume_reading.remove(&finished.block);
    if eligible(&model, &finished.session) && has_block(&model, &finished.session, &finished.block)
    {
        let last = state.resume_seq.get(&finished.block).copied().unwrap_or(0);
        match finished.outcome {
            ReplyOutcome::Snapshot { seq, snapshot } if seq >= last => {
                changed |= store_snapshot(&mut model, &finished.block, snapshot);
                state.resume_seq.insert(finished.block, seq);
            }
            ReplyOutcome::Unsupported => {
                changed |= !model.program_resume_unsupported;
                model.program_resume_unsupported = true;
                changed |= state.resume_reads.clear(&mut model);
            }
            _ => {}
        }
    }
    drop(model);
    if changed {
        shared.wake.notify();
    }
}

fn store_snapshot(model: &mut HostModel, block: &str, mut snapshot: ResumeSnapshot) -> bool {
    let others = model
        .program_resume
        .iter()
        .filter(|(id, _)| id.as_str() != block)
        .map(|(_, snap)| snap.records.len())
        .sum();
    snapshot.apply_host_budget(MAX_RECORDS_PER_HOST.saturating_sub(others));
    if model.program_resume.get(block) == Some(&snapshot) {
        return false;
    }
    model.program_resume.insert(block.to_owned(), snapshot);
    true
}

pub(super) async fn handle_dropped(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    session: Option<&str>,
    block: Option<&str>,
    seq: u64,
) -> rexrs::Result<()> {
    if control.welcome().api_revision < MIN_API_REVISION {
        return Ok(());
    }
    let mut changed = false;
    {
        let mut model = shared.model.lock().expect("host model");
        let held: Vec<_> = model
            .views
            .iter()
            .filter(|(id, _)| session.is_none_or(|session| session == id.as_str()))
            .flat_map(|(id, view)| {
                view_blocks(view)
                    .filter(|bid| block.is_none_or(|block| block == bid))
                    .map(|bid| (id.clone(), bid))
            })
            .collect();
        for (id, block) in held {
            let last = state.resume_seq.entry(block.clone()).or_default();
            if *last >= seq {
                continue;
            }
            *last = seq;
            state.resume_reads.cancel_block(&block);
            changed |= model.program_resume.remove(&block).is_some();
            changed |= model.program_resume_reading.remove(&block);
            changed |= state
                .resume_reads
                .enqueue_new_watched_blocks(&mut model, &id, [block]);
        }
    }
    if changed {
        shared.wake.notify();
    }
    state.resume_reads.pump(control, shared).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_summaries_debug_and_outcomes_never_contain_the_sentinel() {
        const SENTINEL: &str = "synthetic-argv-secret";
        let reply = json!({
            "seq": 1, "payload": {"records": [{
                "owner": {"pid": 1}, "program": "demo", "reported_at": "2026-11-25T22:31:01Z",
                "command": [SENTINEL]
            }]}
        });
        let outcome = decode_reply(&reply.to_string());
        assert!(!format!("{outcome:?}").contains(SENTINEL));
        let ReplyOutcome::Snapshot { snapshot, .. } = outcome else {
            panic!("expected a snapshot");
        };
        assert_eq!(snapshot.records[0].program, "demo");
        for code in [
            "method_not_found",
            "revision_too_low",
            "not_supported",
            SENTINEL,
        ] {
            let outcome = decode_reply(
                &json!({"error": {"code": code, "message": SENTINEL, "details": [SENTINEL]}})
                    .to_string(),
            );
            assert!(!format!("{outcome:?}").contains(SENTINEL));
            assert!(match code {
                SENTINEL => matches!(outcome, ReplyOutcome::Failed),
                _ => matches!(outcome, ReplyOutcome::Unsupported),
            });
        }
    }
}
