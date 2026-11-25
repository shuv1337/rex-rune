use super::*;
use tokio::time::Instant;

/// One title-only wake publishes every title on the host. Keep the earliest deadline
/// when visible and hidden changes arrive together.
#[derive(Default)]
struct Throttle {
    last: Option<Instant>,
    pending: Option<Instant>,
}

impl Throttle {
    fn changed(&mut self, visible: bool, now: Instant) -> bool {
        let interval = if visible {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(2)
        };
        let due = self.last.map_or(now, |last| last + interval);
        self.pending = Some(self.pending.map_or(due, |pending| pending.min(due)));
        self.take_due(now)
    }

    fn take_due(&mut self, now: Instant) -> bool {
        if self.pending.is_some_and(|due| now >= due) {
            self.pending = None;
            self.last = Some(now);
            true
        } else {
            false
        }
    }
}

pub(super) struct HostWake {
    sender: flume::Sender<()>,
    titles: Mutex<Throttle>,
}

impl HostWake {
    pub(super) fn new(sender: flume::Sender<()>) -> Arc<Self> {
        Arc::new(Self {
            sender,
            titles: Mutex::default(),
        })
    }

    pub(super) fn notify(&self) {
        self.titles.lock().expect("title throttle").pending = None;
        let _ = self.sender.try_send(());
    }

    fn title_changed(&self, visible: bool, now: Instant) {
        if self
            .titles
            .lock()
            .expect("title throttle")
            .changed(visible, now)
        {
            let _ = self.sender.try_send(());
        }
    }

    pub(super) fn flush_due(&self) {
        if self
            .titles
            .lock()
            .expect("title throttle")
            .take_due(Instant::now())
        {
            let _ = self.sender.try_send(());
        }
    }

    pub(super) fn reset_titles(&self) {
        *self.titles.lock().expect("title throttle") = Throttle::default();
    }
}

impl Shared {
    pub(super) fn block_event(&self, session: &str, block: String, change: BlockEvent) {
        let BlockEvent::Title { text, owner } = change else {
            self.update(|m| m.apply_block_event(session, block, change));
            return;
        };
        let mut m = self.model.lock().expect("host model");
        let visible = m.visible
            && m.selected.as_deref() == Some(session)
            && m.window().is_some_and(|w| {
                w.layers.iter().any(|layer| {
                    layer
                        .blocks
                        .iter()
                        .any(|b| b.block_id.as_ref() == Some(&block))
                        && crate::layout::block_visible(w, layer, &block)
                })
            });
        if m.set_title(block, text, owner) {
            self.wake.title_changed(visible, Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_wakes_keep_the_earliest_deadline_until_delivery() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut throttle = Throttle::default();
        assert!(throttle.changed(false, start));
        assert!(!throttle.changed(false, at(100)));
        assert!(!throttle.changed(true, at(150)));
        assert!(!throttle.changed(false, at(249)));
        assert!(throttle.take_due(at(250)));
        assert!(!throttle.changed(false, at(251)));
        assert!(!throttle.take_due(at(2249)));
        assert!(throttle.take_due(at(2250)));
        assert!(!throttle.take_due(at(4000)));
        assert!(throttle.changed(false, at(4250)));
        assert!(throttle.changed(true, at(4500)));
    }
}
