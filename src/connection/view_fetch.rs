//! On-demand read-only `session.view` fetches driven through detached control transport
//! requests so remote tab-state calls never block the host loop or serialize behind slow
//! replies. Duplicate in-flight fetches for the same session coalesce cleanly.
use crate::transport::Client;
use rexrs::api::{Method, session};
use serde::Deserialize;
use std::{
    future::Future,
    pin::Pin,
    task::Poll,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

/// Outcome of a `HostCommand::FetchView` request delivered to the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewFetchOutcome {
    Loaded,
    NotFound,
    Failed,
}

/// Cap on concurrent detached `session.view` fetches and coalesced waiters per session.
const MAX_IN_FLIGHT: usize = 16;
const MAX_WAITERS: usize = 16;
const MAX_RPC_WAIT: Duration = Duration::from_secs(8);

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    seq: u64,
    payload: Option<session::ViewResponse>,
    error: Option<ResponseError>,
}

#[derive(Deserialize)]
struct ResponseError {
    #[serde(default)]
    code: String,
    #[serde(default)]
    status: u16,
}

impl ResponseError {
    fn outcome(&self) -> ViewFetchOutcome {
        if matches!(self.code.as_str(), "not_found" | "session_not_found") || self.status == 404 {
            ViewFetchOutcome::NotFound
        } else {
            ViewFetchOutcome::Failed
        }
    }
}

pub(super) enum Event {
    Reply {
        session: String,
        ids: Vec<u64>,
        result: Result<(u64, session::ViewResponse), ViewFetchOutcome>,
    },
    Expired {
        session: String,
        ids: Vec<u64>,
        finished: bool,
    },
}

struct InFlight {
    session: String,
    waiters: Vec<(u64, Instant)>,
    reply: oneshot::Receiver<String>,
    timer: Pin<Box<tokio::time::Sleep>>,
}

impl InFlight {
    fn earliest_deadline(&self) -> Instant {
        self.waiters
            .iter()
            .map(|&(_, d)| d)
            .min()
            .unwrap_or_else(Instant::now)
    }

    fn reset_timer(&mut self) {
        let next = self.earliest_deadline();
        self.timer
            .as_mut()
            .reset(tokio::time::Instant::from_std(next));
    }
}

#[derive(Default)]
pub(super) struct Reader {
    entries: Vec<InFlight>,
}

impl Reader {
    pub fn reset(&mut self) {
        self.entries.clear();
    }

    pub fn in_flight(&self) -> bool {
        !self.entries.is_empty()
    }

    pub fn reading(&self, session: &str) -> bool {
        self.entries.iter().any(|e| e.session == session)
    }

    /// Start or coalesce a detached `session.view` fetch. Returns `Ok(true)` when queued,
    /// `Ok(false)` when refused locally (expired or over capacity), and `Err` on transport
    /// failure.
    pub async fn start(
        &mut self,
        control: &mut Client,
        id: u64,
        deadline: Instant,
        session: String,
    ) -> rexrs::Result<bool> {
        let now = Instant::now();
        if deadline <= now {
            return Ok(false);
        }
        let capped = deadline.min(now + MAX_RPC_WAIT);
        if let Some(existing) = self.entries.iter_mut().find(|e| e.session == session) {
            if existing.waiters.len() >= MAX_WAITERS {
                return Ok(false);
            }
            existing.waiters.push((id, capped));
            existing.reset_timer();
            return Ok(true);
        }
        if self.entries.len() >= MAX_IN_FLIGHT {
            return Ok(false);
        }
        let reply = control
            .transport()
            .detach(session::View::NAME, session::View::new(&session))
            .await?;
        let timer = Box::pin(tokio::time::sleep_until(tokio::time::Instant::from_std(
            capped,
        )));
        self.entries.push(InFlight {
            session,
            waiters: vec![(id, capped)],
            reply,
            timer,
        });
        Ok(true)
    }

    /// Wait for the first detached fetch to answer or for any waiter's deadline to expire.
    /// Cancel-safe: state is mutated only in the poll that returns `Poll::Ready`.
    pub async fn finished(&mut self) -> Event {
        if self.entries.is_empty() {
            return std::future::pending().await;
        }
        std::future::poll_fn(|cx| {
            // Replies win over deadlines so an arrived answer is never dropped.
            for index in 0..self.entries.len() {
                if let Poll::Ready(text) = Pin::new(&mut self.entries[index].reply).poll(cx) {
                    let entry = self.entries.remove(index);
                    let ids = entry.waiters.into_iter().map(|(id, _)| id).collect();
                    let result = parse_reply(text.ok());
                    return Poll::Ready(Event::Reply {
                        session: entry.session,
                        ids,
                        result,
                    });
                }
            }
            let mut index = 0;
            while index < self.entries.len() {
                if self.entries[index].timer.as_mut().poll(cx).is_ready() {
                    let now = tokio::time::Instant::now().into_std();
                    let entry = &mut self.entries[index];
                    let mut expired = Vec::new();
                    entry.waiters.retain(|&(id, deadline)| {
                        if deadline <= now {
                            expired.push(id);
                            false
                        } else {
                            true
                        }
                    });
                    if entry.waiters.is_empty() {
                        let entry = self.entries.remove(index);
                        if !expired.is_empty() {
                            return Poll::Ready(Event::Expired {
                                session: entry.session,
                                ids: expired,
                                finished: true,
                            });
                        }
                        continue;
                    }
                    entry.reset_timer();
                    if !expired.is_empty() {
                        let session = entry.session.clone();
                        return Poll::Ready(Event::Expired {
                            session,
                            ids: expired,
                            finished: false,
                        });
                    }
                    // Poll the reset timer with `cx` so its waker is always registered.
                    if entry.timer.as_mut().poll(cx).is_ready() {
                        let earliest = entry.earliest_deadline();
                        entry.waiters.retain(|&(id, deadline)| {
                            if deadline <= earliest {
                                expired.push(id);
                                false
                            } else {
                                true
                            }
                        });
                        if entry.waiters.is_empty() {
                            let entry = self.entries.remove(index);
                            return Poll::Ready(Event::Expired {
                                session: entry.session,
                                ids: expired,
                                finished: true,
                            });
                        }
                        let session = entry.session.clone();
                        entry.reset_timer();
                        return Poll::Ready(Event::Expired {
                            session,
                            ids: expired,
                            finished: false,
                        });
                    }
                }
                index += 1;
            }
            Poll::Pending
        })
        .await
    }
}

fn parse_reply(text: Option<String>) -> Result<(u64, session::ViewResponse), ViewFetchOutcome> {
    let Some(text) = text else {
        return Err(ViewFetchOutcome::Failed);
    };
    let Ok(response) = serde_json::from_str::<Response>(&text) else {
        return Err(ViewFetchOutcome::Failed);
    };
    if let Some(error) = response.error {
        return Err(error.outcome());
    }
    response
        .payload
        .map(|view| (response.seq, view))
        .ok_or(ViewFetchOutcome::Failed)
}
