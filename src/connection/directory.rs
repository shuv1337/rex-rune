//! One read-only directory request. Results are correlated and never replayed.
use super::{Client, Shared};
use rexrs::api::{
    BlockMethod,
    terminal::{ListDir, ListDirResponse},
};
use std::{
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

pub struct Request {
    pub id: u64,
    pub epoch: u64,
    pub session: String,
    pub block: String,
    pub args: ListDir,
    pub deadline: Instant,
    /// Closing the browser cancels requests that have not been sent yet.
    pub open: Weak<()>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    Disconnected,
    Unsupported,
    Timeout,
    Request,
}
impl Failure {
    pub fn message(&self) -> &'static str {
        match self {
            Self::Disconnected => "Host is disconnected. Try again after reconnecting.",
            Self::Unsupported => "This Rex server can't list paths. Update Rex on the host.",
            Self::Timeout => "Rex didn't answer in time. Edit the path to try again.",
            Self::Request => "Couldn't list this folder. Edit the path to try again.",
        }
    }
}

#[derive(Clone)]
pub struct Reply {
    pub id: u64,
    pub epoch: u64,
    pub result: Result<ListDirResponse, Failure>,
}

pub(super) fn finish(shared: &Shared, request: &Request, result: Result<ListDirResponse, Failure>) {
    if request.open.strong_count() == 0 {
        return;
    }
    shared.update(|m| {
        m.directory = Some(Arc::new(Reply {
            id: request.id,
            epoch: request.epoch,
            result,
        }));
    });
}

pub(super) async fn run(control: &mut Client, shared: &Shared, request: Request) {
    if request.open.strong_count() == 0 {
        return;
    }
    let epoch = shared.model.lock().expect("host model").connection_epoch;
    if epoch != request.epoch {
        finish(shared, &request, Err(Failure::Disconnected));
        return;
    }
    let remaining = request.deadline.saturating_duration_since(Instant::now());
    let result = if remaining.is_zero() {
        Err(Failure::Timeout)
    } else {
        let payload = serde_json::json!({"session_id":request.session, "block_id":request.block, "args":request.args});
        match tokio::time::timeout(
            remaining.min(Duration::from_secs(8)),
            control.call_raw(ListDir::NAME, payload),
        )
        .await
        {
            Err(_) | Ok(Err(rexrs::Error::TimedOut)) => Err(Failure::Timeout),
            Ok(Err(rexrs::Error::Api(
                rexrs::ApiError::MethodNotFound { .. }
                | rexrs::ApiError::RevisionTooLow { .. }
                | rexrs::ApiError::NotSupported { .. },
            ))) => Err(Failure::Unsupported),
            Ok(Err(_)) => Err(Failure::Request),
            Ok(Ok(value)) => serde_json::from_value(value).map_err(|_| Failure::Request),
        }
    };
    finish(shared, &request, result);
}
