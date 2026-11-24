//! client.action has a server-side 30-second budget, unlike ordinary control requests.
use super::{Client, Shared};
use anyhow::{Result, anyhow};
use rexrs::api::{
    Method,
    client::{Action, ActionResponse, ClientAction},
};
use std::{
    future::Future,
    pin::Pin,
    task::Poll,
    time::{Duration, Instant},
};
use tokio::{
    sync::oneshot,
    time::{Instant as TokioInstant, Sleep},
};

pub const TIMEOUT: Duration = Duration::from_secs(35);
#[derive(Clone)]
pub struct Queued {
    pub generation: u64,
    pub session: Option<String>,
    pub block: Option<String>,
    pub action: ClientAction,
}

struct Pending {
    generation: u64,
    session: Option<String>,
    block: Option<String>,
    response: oneshot::Receiver<String>,
    timer: Pin<Box<Sleep>>,
    exclusive: bool,
}

impl Pending {
    fn promote(&mut self, deadline: Instant) {
        self.exclusive = false;
        let end = TokioInstant::from_std(deadline) + TIMEOUT;
        if end > self.timer.deadline() {
            self.timer.as_mut().reset(end);
        }
    }
}

/// Before a prompt, preserve the exclusive action wait. Rex relaxes its ordered queue
/// when the action asks a question; at that point ordinary host work may run again.
/// Outstanding waits belong to this socket only and are never replayed.
#[derive(Default)]
pub(super) struct Waits(Vec<Pending>);

impl Waits {
    pub fn exclusive(&self) -> bool {
        self.0.last().is_some_and(|pending| pending.exclusive)
    }

    pub fn promote(&mut self, deadline: Instant) {
        for pending in &mut self.0 {
            pending.promote(deadline);
        }
    }

    pub async fn start(
        &mut self,
        control: &mut Client,
        shared: &Shared,
        generation: u64,
        request: Action,
        prompt_deadline: Option<Instant>,
    ) -> Result<()> {
        let session = request.session_id.clone();
        let block = request.block_id.clone();
        let timer = Box::pin(tokio::time::sleep(TIMEOUT));
        match control.transport().detach(Action::NAME, request).await {
            Ok(response) => {
                let mut pending = Pending {
                    generation,
                    session,
                    block,
                    response,
                    timer,
                    exclusive: true,
                };
                if let Some(deadline) = prompt_deadline {
                    pending.promote(deadline);
                }
                self.0.push(pending);
                Ok(())
            }
            Err(error) => action_error(shared, error),
        }
    }

    /// Cancel-safe until a reply or deadline is ready. This arm also runs in the
    /// exclusive wait, so an earlier promoted action can still finish there.
    pub async fn finished(&mut self, shared: &Shared) -> Result<()> {
        let (pending, reply) = std::future::poll_fn(|cx| {
            for (index, pending) in self.0.iter_mut().enumerate() {
                if let Poll::Ready(response) = Pin::new(&mut pending.response).poll(cx) {
                    return Poll::Ready((self.0.remove(index), Some(response)));
                }
                if pending.timer.as_mut().poll(cx).is_ready() {
                    return Poll::Ready((self.0.remove(index), None));
                }
            }
            Poll::Pending
        })
        .await;
        let text = reply
            .ok_or_else(|| anyhow!("Rex client.action timed out; action was not retried"))?
            .map_err(rexrs::Error::transport)?;
        let reply: ActionResponse = match crate::transport::detached_result(&text) {
            Ok(reply) => reply,
            Err(error) => return action_error(shared, error),
        };
        if !reply.client_actions.is_empty() {
            shared.update(|m| {
                m.client_actions
                    .extend(reply.client_actions.into_iter().map(|action| Queued {
                        generation: pending.generation,
                        session: pending.session.clone(),
                        block: pending.block.clone(),
                        action,
                    }));
            });
        }
        Ok(())
    }
}

fn action_error(shared: &Shared, error: rexrs::Error) -> Result<()> {
    match error {
        error @ (rexrs::Error::Json(_) | rexrs::Error::RequestTooLarge { .. }) => {
            // Request sizing and typed reply decoding leave the transport usable.
            shared.update(|m| m.notice = Some(format!("Rex client.action failed: {error}")));
            Ok(())
        }
        error => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn action_promotion_extends_the_wait_and_does_not_shorten_it() {
        let now = TokioInstant::now();
        let (_send, response) = oneshot::channel();
        let mut waits = Waits(vec![Pending {
            generation: 0,
            session: None,
            block: None,
            response,
            timer: Box::pin(tokio::time::sleep_until(now + TIMEOUT)),
            exclusive: true,
        }]);
        assert!(waits.exclusive());
        let expected = now + Duration::from_secs(300) + TIMEOUT;
        waits.promote((now + Duration::from_secs(300)).into_std());
        assert!(!waits.exclusive());
        assert_eq!(waits.0[0].timer.deadline(), expected);
        waits.promote(now.into_std());
        assert_eq!(waits.0[0].timer.deadline(), expected);
    }
}
