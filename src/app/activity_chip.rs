//! A quiet entry point into Activity, not a read receipt or a server inventory.
use super::{
    theme::{danger, warning},
    workspace::Workspace,
};
use rex_linux::connection::activity::ProgramState;

pub struct Summary {
    pub count: usize,
    pub needs_you: bool,
}

impl Summary {
    pub fn label(&self) -> String {
        match (self.needs_you, self.count) {
            (true, 1) => "1 needs you".into(),
            (true, n) => format!("{n} need you"),
            (false, 1) => "1 error".into(),
            (false, n) => format!("{n} errors"),
        }
    }

    pub fn short(&self) -> String {
        if self.needs_you {
            format!("Need {}", self.count)
        } else {
            self.label()
        }
    }

    pub fn tone(&self) -> u32 {
        if self.needs_you { warning() } else { danger() }
    }
}

impl Workspace {
    pub fn activity_chip(&self) -> Option<Summary> {
        if !self.settings.notifications.show_in_tabs {
            return None;
        }
        let (mut waiting, mut errors) = (0, 0);
        for (host, session, window, activity) in self.activity_tabs() {
            // On screen, even when the native window is unfocused. This is not tab_seen:
            // the chip never acknowledges a tab or closes a desktop notification.
            let model = &self.hosts[host].model;
            if host == self.active
                && model.selected.as_deref() == Some(session)
                && model
                    .views
                    .get(session)
                    .is_some_and(|v| v.active_window_id.as_deref() == Some(window))
            {
                continue;
            }
            match activity.state {
                ProgramState::Blocked => waiting += 1,
                ProgramState::Error => errors += 1,
                _ => {}
            }
        }
        if waiting > 0 {
            Some(Summary {
                count: waiting,
                needs_you: true,
            })
        } else if errors > 0 {
            Some(Summary {
                count: errors,
                needs_you: false,
            })
        } else {
            None
        }
    }
}
