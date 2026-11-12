//! One pass of old program status, after selected-session restoration. Layout reads use
//! Prefetch's detached slot; the host loop shares its pace and never runs both slots at once.
use super::{Client, ControlState, Shared, activity, view_blocks};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

pub(super) enum Work {
    View(String),
    Status(String, String),
}

/// Scan the current inventory, not a frozen startup queue: new sessions and panes join in
/// server order. Attempt sets live for the whole connection, even after a failure/removal.
pub(super) fn next(shared: &Shared, state: &ControlState) -> Option<Work> {
    let model = shared.model.lock().expect("host model");
    for session in &model.sessions {
        let id = &session.session_id;
        if let Some(view) = model.views.get(id) {
            if let Some(block) = view_blocks(view).find(|b| !state.status_queried.contains(b)) {
                return Some(Work::Status(id.clone(), block));
            }
        } else if !state.viewed.contains(id) {
            return Some(Work::View(id.clone()));
        }
    }
    None
}

pub(super) fn coverage(shared: &Shared, state: &ControlState, reading: bool) {
    let incomplete = {
        let m = shared.model.lock().expect("host model");
        m.sessions.iter().any(|s| match m.views.get(&s.session_id) {
            Some(view) => view_blocks(view).any(|b| state.status_failed.contains(&b)),
            None => state.viewed.contains(&s.session_id),
        })
    };
    let changed = {
        let m = shared.model.lock().expect("host model");
        m.activity_reading != reading || m.activity_incomplete != incomplete
    };
    if changed {
        shared.update(|m| {
            m.activity_reading = reading;
            m.activity_incomplete = incomplete;
        });
    }
}

pub(super) fn record(
    shared: &Shared,
    state: &mut ControlState,
    session: &str,
    block: &str,
    reply: Option<Value>,
) {
    let Some(reply) = reply else {
        state.status_failed.insert(block.into());
        return;
    };
    let records = activity::parse_status(&reply);
    shared.update(|m| {
        if !m
            .views
            .get(session)
            .is_some_and(|v| view_blocks(v).any(|b| b == block))
        {
            return;
        }
        // Same snapshot semantics as the selected-session query. These are old reports,
        // not live news: no notification or unseen-Done receipt is invented here.
        m.activity.remove(block);
        if !records.is_empty() {
            m.activity
                .insert(block.into(), records.into_iter().collect());
        }
    });
}

#[derive(Default)]
pub(super) struct Reader {
    read: Option<Read>,
}

struct Read {
    session: String,
    block: String,
    reply: oneshot::Receiver<String>,
    deadline: Instant,
}
impl Reader {
    pub fn in_flight(&self) -> bool {
        self.read.is_some()
    }

    pub async fn start(
        &mut self,
        control: &mut Client,
        session: String,
        block: String,
    ) -> rexrs::Result<()> {
        let reply = control
            .transport()
            .detach(
                &format!("{}.program_status", activity::TERMINAL),
                json!({"session_id":session,"block_id":block,"args":{}}),
            )
            .await?;
        self.read = Some(Read {
            session,
            block,
            reply,
            deadline: Instant::now() + Duration::from_secs(8),
        });
        Ok(())
    }

    /// Cancel-safe bounded wait; the control socket continues receiving in the host loop.
    pub async fn finished(&mut self) -> (String, String, Option<Value>) {
        let Some(read) = &mut self.read else {
            return std::future::pending().await;
        };
        let text = tokio::time::timeout_at(
            tokio::time::Instant::from_std(read.deadline),
            &mut read.reply,
        )
        .await
        .ok()
        .and_then(Result::ok);
        let payload = text
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(|v| v["error"].is_null() && v["payload"].is_object())
            .map(|mut v| v["payload"].take());
        let read = self.read.take().expect("status in flight");
        (read.session, read.block, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_silent_read_expires_and_a_late_reply_has_no_receiver() {
        let (sender, reply) = oneshot::channel();
        let mut reader = Reader {
            read: Some(Read {
                session: "s".into(),
                block: "b".into(),
                reply,
                deadline: Instant::now(),
            }),
        };
        assert_eq!(reader.finished().await, ("s".into(), "b".into(), None));
        assert!(!reader.in_flight());
        assert!(sender.send("late".into()).is_err());
    }
}
