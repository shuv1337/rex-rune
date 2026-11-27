//! One-shot block inventories for a person's request to close a session with no tabs.
use super::{Client, Shared};
use futures_util::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use rexrs::api::{Method, session};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

pub const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocks {
    Empty,
    Present,
    Unknown,
    SessionGone,
}

#[derive(Clone, Debug)]
pub struct Reply {
    pub epoch: u64,
    pub session: String,
    pub blocks: Blocks,
}

// Unlike the generated response type, this requires an explicit, non-null array.
// An absent inventory can never authorize a session close.
#[derive(serde::Deserialize)]
struct Inventory {
    blocks: Vec<session::Block>,
}

#[derive(Default)]
pub(super) struct Reader {
    sessions: HashSet<String>,
    reads: FuturesUnordered<BoxFuture<'static, Reply>>,
}

impl Reader {
    pub async fn start(
        &mut self,
        control: &mut Client,
        shared: &Shared,
        epoch: u64,
        session: String,
        deadline: Instant,
    ) -> rexrs::Result<()> {
        if shared.model.lock().expect("host model").connection_epoch != epoch
            || self.sessions.contains(&session)
        {
            return Ok(());
        }
        let deadline = deadline.min(Instant::now() + TIMEOUT);
        let mut result = Reply {
            epoch,
            session,
            blocks: Blocks::Unknown,
        };
        if deadline <= Instant::now() {
            record(shared, result);
            return Ok(());
        }
        let deadline = tokio::time::Instant::from_std(deadline);
        // The method is available at API 0. Treat a refusal on any server as unknown.
        let request = control.transport().detach(
            session::ListBlocks::NAME,
            session::ListBlocks::new(&result.session),
        );
        let reply = match tokio::time::timeout_at(deadline, request).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(error @ (rexrs::Error::Transport(_) | rexrs::Error::Closed { .. }))) => {
                return Err(error);
            }
            _ => {
                record(shared, result);
                return Ok(());
            }
        };
        self.sessions.insert(result.session.clone());
        self.reads.push(Box::pin(async move {
            if let Ok(Ok(text)) = tokio::time::timeout_at(deadline, reply).await {
                result.blocks = match crate::transport::detached_result::<Inventory>(&text) {
                    Ok(inventory) if inventory.blocks.is_empty() => Blocks::Empty,
                    Ok(_) => Blocks::Present,
                    Err(rexrs::Error::Api(rexrs::ApiError::SessionNotFound { .. })) => {
                        Blocks::SessionGone
                    }
                    Err(_) => Blocks::Unknown,
                };
            }
            result
        }));
        Ok(())
    }

    pub fn in_flight(&self) -> bool {
        !self.reads.is_empty()
    }

    pub async fn finished(&mut self, shared: &Shared) {
        let reply = self.reads.next().await.expect("in-flight session check");
        self.sessions.remove(&reply.session);
        record(shared, reply);
    }
}

fn record(shared: &Shared, reply: Reply) {
    shared.update(|m| {
        if m.connected && m.connection_epoch == reply.epoch {
            m.session_close_checks.push(reply);
        }
    });
}
