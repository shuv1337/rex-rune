//! No desktop notification service: Windows has none of the ones this client speaks, and
//! says so in Settings instead of offering a switch that does nothing.
use crate::app::notify::{Backend, Event, Note, Service, Session};

/// Shown instead of a service state. The wording is the platform's, not a daemon's.
fn unavailable() -> Service {
    Service::Unavailable("Not available on this platform".into())
}

pub struct Desktop;

impl Desktop {
    pub fn new() -> Self {
        Self
    }
}

impl Backend for Desktop {
    async fn ready(&mut self) -> Result<String, Service> {
        Err(unavailable())
    }

    async fn send(&mut self, _note: &Note, _replaces: u32) -> Result<u32, Service> {
        Err(unavailable())
    }

    async fn close(&mut self, _id: u32) -> Result<(), Service> {
        Err(unavailable())
    }
}

impl Session for Desktop {
    fn identity(&self) -> Option<String> {
        None
    }

    async fn event(&mut self) -> Event {
        std::future::pending().await
    }
}

/// There is no channel to post to after a failed re-exec; stderr already says so.
pub fn restart_failure(_message: &str) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("{}", unavailable().reason()))
}
