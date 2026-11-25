//! On-demand, read-only Program Resume metadata for the current session.
use super::{
    activity::pane_name,
    panel::{Entry, Intent, PaneTarget, PanelKind},
    theme::*,
    widgets::highlight,
    workspace::{HostId, Workspace},
};
use rex_rune::{
    connection::{HostCommand, HostModel, activity},
    ports::HostLocality,
    program_resume::{MIN_API_REVISION, ResumeSummary, Trust},
};
use rexrs::api::session::LayerBlock;
use rune::{El, col, row, text};
use std::{borrow::Cow, time::SystemTime};

pub const ACTION: &str = "client.program_resume";
const EMPTY: &str = "No program in this session has reported how to resume.";
const FOOTER: &str =
    "Trusted chosen programs resume after a server restart; others return to a shell.";

#[derive(Clone, Debug)]
pub struct Target {
    pub host: HostId,
    pub session: Option<String>,
    epoch: u64,
    first: Option<String>,
}

#[derive(PartialEq, Eq)]
pub struct Watch {
    host: HostId,
    epoch: u64,
    session: String,
}

pub struct Presentation {
    pub verdict: &'static str,
    pub reason: Cow<'static, str>,
    cwd: Option<String>,
    age_and_pid: String,
}

pub struct Coverage {
    pub empty: String,
    pub footer: Vec<String>,
}

fn supported(model: &HostModel) -> bool {
    model.protocol.api_revision >= MIN_API_REVISION && !model.program_resume_unsupported
}

fn has_foreground_view(model: &HostModel, session: &str) -> bool {
    model.views.contains_key(session) && !model.background_views.contains(session)
}

fn blocks(model: &HostModel) -> impl Iterator<Item = (&LayerBlock, &str)> {
    model
        .view()
        .into_iter()
        .flat_map(|view| &view.windows)
        .flat_map(|window| &window.layers)
        .flat_map(|layer| &layer.blocks)
        .filter(|block| block.creator_name.as_deref() == Some(activity::TERMINAL))
        .filter_map(|block| Some((block, block.block_id.as_deref()?)))
}

impl Workspace {
    pub(super) fn program_resume_blocked(&self) -> Option<Cow<'static, str>> {
        let model = self.model();
        if !supported(model) {
            let host = &self.settings.hosts[self.active].label;
            Some(format!("Needs Rex API 24 on {host}").into())
        } else if !model.connected {
            Some("Host is disconnected.".into())
        } else {
            None
        }
    }

    pub(super) fn program_resume_menu_detail(&self, pane: &PaneTarget) -> Option<&str> {
        let model = self.model();
        if pane.host != self.active
            || model.selected.as_ref() != Some(&pane.session)
            || !model.connected
            || !supported(model)
        {
            return None;
        }
        Some(
            model
                .program_resume
                .get(&pane.block)
                .and_then(|snapshot| snapshot.records.iter().find(|record| record.chosen))
                .map_or("", |record| &record.program),
        )
    }

    pub fn show_program_resume(&mut self, first: Option<String>) {
        if let Some(reason) = self.program_resume_blocked() {
            self.inform(reason.into_owned());
            return;
        }
        if self.program_resume_target().is_some() {
            self.cancel_panel_quiet();
        }
        let model = self.model();
        let session = model.selected.clone();
        let title = session
            .as_ref()
            .and_then(|id| model.sessions.iter().find(|s| &s.session_id == id))
            .filter(|s| !s.label.is_empty())
            .map_or_else(
                || "Program resume".into(),
                |s| format!("Program resume · {}", s.label),
            );
        let target = Target {
            host: self.hosts[self.active].id,
            epoch: model.connection_epoch,
            session,
            first,
        };
        self.open_panel(
            PanelKind::ProgramResume(target),
            &title,
            &[("", "Filter programs…", "")],
        );
        // The pane menu only determines the first group, not which panes are watched.
        self.panel.as_mut().unwrap().target = None;
    }

    pub fn program_resume_target(&self) -> Option<&Target> {
        match &self.panel.as_ref()?.kind {
            PanelKind::ProgramResume(target) => Some(target),
            _ => None,
        }
    }

    pub(super) fn session_inspector_open(&self) -> bool {
        self.ports_target().is_some() || self.program_resume_target().is_some()
    }

    pub(super) fn session_inspector_out_of_context(&self) -> bool {
        self.ports_target()
            .is_some_and(|target| !self.ports_in_context(target))
            || self
                .program_resume_target()
                .is_some_and(|target| !self.program_resume_in_context(target))
    }

    pub(super) fn refresh_session_inspector_watches(&mut self) {
        self.refresh_ports_watch();
        self.refresh_program_resume_watch();
    }

    fn program_resume_in_context(&self, target: &Target) -> bool {
        let host = &self.hosts[self.active];
        host.id == target.host
            && host.model.connection_epoch == target.epoch
            && (!host.model.connected || host.model.selected == target.session)
    }

    fn refresh_program_resume_watch(&mut self) {
        let next = self.program_resume_target().and_then(|target| {
            let session = target.session.as_ref()?;
            // The worker drops an early watch. Do not mark it sent until sync supplies
            // an actual foreground view (a prefetched background view isn't enough).
            self.program_resume_view_ready(target).then(|| Watch {
                host: target.host,
                epoch: target.epoch,
                session: session.clone(),
            })
        });
        if self.program_resume_watch == next {
            return;
        }
        if let Some(previous) = self.program_resume_watch.take()
            && let Some(index) = self.host_index(previous.host)
        {
            // Send the old epoch even after a reconnect. The worker fences it off
            // from the new connection and has already discarded the old reads.
            self.send(
                index,
                HostCommand::WatchProgramResume {
                    epoch: previous.epoch,
                    session: None,
                },
            );
        }
        if let Some(watch) = &next {
            self.send(
                self.active,
                HostCommand::WatchProgramResume {
                    epoch: watch.epoch,
                    session: Some(watch.session.clone()),
                },
            );
        }
        self.program_resume_watch = next;
    }

    fn program_resume_view_ready(&self, target: &Target) -> bool {
        self.program_resume_in_context(target)
            && self.model().connected
            && supported(self.model())
            && target
                .session
                .as_deref()
                .is_some_and(|session| has_foreground_view(self.model(), session))
    }

    pub fn program_resume_entries(&self, target: &Target, lower: &str) -> Vec<Entry> {
        if !self.program_resume_view_ready(target) {
            return Vec::new();
        }
        let model = self.model();
        let now = SystemTime::now();
        let mut ordered: Vec<_> = blocks(model).collect();
        if let Some(index) = ordered
            .iter()
            .position(|(_, block)| Some(*block) == target.first.as_deref())
        {
            ordered[..=index].rotate_right(1);
        }
        let mut entries = Vec::new();
        for (pane, block) in ordered {
            let Some(snapshot) = model.program_resume.get(block) else {
                continue;
            };
            let group = pane_name(model, pane, self.generated_titles());
            for record in &snapshot.records {
                let (program, detail, presentation) = present(record, now);
                if !lower.is_empty()
                    && ![
                        group.as_str(),
                        &program,
                        &detail,
                        presentation.verdict,
                        &presentation.reason,
                    ]
                    .iter()
                    .any(|field| field.to_lowercase().contains(lower))
                {
                    continue;
                }
                let mut entry = Entry::new(program, detail, &group, Intent::InspectProgramResume)
                    .group_named(&group);
                entry.id = Some(record.key.row_id(block).into());
                entry.group_key = Some(block.into());
                entry.program_resume = Some(presentation);
                entries.push(entry);
            }
        }
        entries
    }

    pub fn program_resume_coverage(&self, target: &Target) -> Coverage {
        let model = self.model();
        let host = &self.settings.hosts[self.active];
        let mut footer = vec![FOOTER.into()];
        if HostLocality::from_endpoint(&host.endpoint) != HostLocality::Local {
            footer.push(format!("These programs run on {}.", host.label));
        }
        let mut reading = false;
        let mut has_records = false;
        let mut truncated = false;
        let mut untrusted = false;
        if self.program_resume_view_ready(target) {
            for (_, block) in blocks(model) {
                reading |= model.program_resume_reading.contains(block);
                if let Some(snapshot) = model.program_resume.get(block) {
                    has_records |= !snapshot.records.is_empty();
                    truncated |= snapshot.truncated;
                    untrusted |= snapshot
                        .records
                        .iter()
                        .any(|record| matches!(record.trust, Trust::Untrusted(_)));
                }
            }
        } else if target.session.is_some() {
            reading = true;
        }
        if truncated {
            footer.push("Some records aren't shown.".into());
        }
        if untrusted {
            footer.push(format!(
                "Trust rules are set in init.lua on {} (resume.trust).",
                host.label
            ));
        }
        let empty = if !model.connected {
            "Host is disconnected.".into()
        } else if !supported(model) {
            format!("Program resume needs Rex API 24 on {}.", host.label)
        } else if has_records {
            "No matching programs.".into()
        } else if reading {
            "Reading…".into()
        } else {
            EMPTY.into()
        };
        Coverage { empty, footer }
    }

    pub(super) fn seed_program_resume_selection(&mut self) {
        if self.program_resume_target().is_some()
            && self.panel.as_ref().is_some_and(|p| p.row.is_none())
        {
            self.select_row(0);
        }
    }
}

fn present(record: &ResumeSummary, now: SystemTime) -> (String, String, Presentation) {
    let label = if !record.owner_name.is_empty() && record.owner_name != record.program {
        format!("{} ({})", record.program, record.owner_name)
    } else {
        record.program.clone()
    };
    let (verdict, reason) = match &record.trust {
        Trust::Trusted { rule } => (
            if record.chosen {
                "Will resume"
            } else {
                "Not chosen · Trusted"
            },
            format!("Rule: {rule}").into(),
        ),
        Trust::Untrusted(reason) => ("Won't resume", reason.explain().into()),
    };
    let cwd = record.cwd.as_ref().filter(|cwd| !cwd.is_empty()).cloned();
    let pid = format!("pid {}", record.key.pid);
    let age_and_pid = if let Some(age) = record
        .reported_at
        .and_then(|at| activity::time::reported_elapsed_since(at, now))
    {
        let seconds = age.as_secs();
        let age: String = if seconds < 60 {
            "reported now".into()
        } else if seconds < 3600 {
            format!("reported {}m ago", seconds / 60)
        } else if seconds < 86400 {
            format!("reported {}h ago", seconds / 3600)
        } else {
            format!("reported {}d ago", seconds / 86400)
        };
        format!("{age} · {pid}")
    } else {
        pid
    };
    let detail = cwd.as_ref().map_or_else(
        || age_and_pid.clone(),
        |cwd| format!("{cwd} · {age_and_pid}"),
    );
    (
        label,
        detail,
        Presentation {
            verdict,
            reason,
            cwd,
            age_and_pid,
        },
    )
}

pub fn resume_row(entry: &Entry, summary: &Presentation, index: usize, selected: bool) -> El {
    let id = entry.id.as_deref().unwrap().to_owned();
    let select = move |s: &mut Workspace| {
        if let Some(index) = s
            .entries()
            .iter()
            .position(|entry| entry.id.as_deref() == Some(id.as_str()))
        {
            s.select_row(index);
        }
    };
    let label = |value: &str, size, colour| {
        text(value.to_owned())
            .font(ui(size))
            .color(rgb(colour))
            .min_w(0.)
            .shrink()
            .truncate()
            .help_when_truncated(value.to_owned())
    };
    let place = row()
        .gap(4.)
        .grow(1.)
        .shrink()
        .min_w(0.)
        .children(summary.cwd.as_deref().map(|cwd| label(cwd, 11., muted())))
        .children(
            summary
                .cwd
                .as_ref()
                .map(|_| text("·").font(ui(11.)).color(rgb(muted()))),
        )
        .child(
            text(summary.age_and_pid.clone())
                .font(ui(11.))
                .color(rgb(muted())),
        );
    let metadata = row()
        .w_full()
        .gap(12.)
        .child(place)
        .child(label(&summary.reason, 11., muted()).max_w(225.));
    col()
        .key(format!("entry:{index}"))
        .role("option")
        .label(format!(
            "{}, {}, {}, {}",
            entry.label, summary.verdict, entry.detail, summary.reason
        ))
        .w_full()
        .h(44.)
        .px(9.)
        .gap(2.)
        .center()
        .preserve_focus_on_press(true)
        .on_hover(select.clone())
        .on_press_tx(move |s, _| select(s))
        .children(selected.then(|| highlight("sel:program-resume", 7., false)))
        .child(
            row()
                .w_full()
                .gap(12.)
                .child(label(&entry.label, 13., ink()).grow(1.))
                .child(text(summary.verdict).font(ui(11.)).color(rgb(muted()))),
        )
        .child(metadata)
}
