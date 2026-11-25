//! One control connection per host; stable, independently streamed terminals by block ID.
use crate::{
    terminal::{Frame, Renderer, TerminalStream},
    transport::{self, Client},
    wire,
};
use activity::{BlockEvent, DesktopNotice, StatusRecord, TerminalProgress};
use anyhow::{Context, Result};
use rexrs::api::{BlockMethod, Method, session, terminal::SetTheme};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

pub mod activity;
mod batch;
use batch::{declare_themes, query_activity};
pub mod clipboard;
pub mod directory;
pub mod downloads;
pub mod focus_history;
pub mod hosts;
mod input;
pub mod ports;
mod prefetch;
pub mod server_info;
mod status_sweep;
mod title_wake;
pub mod titles;
mod view_fetch;

pub use focus_history::{
    FOCUS_HISTORY_DISCONNECTED, FOCUS_HISTORY_DISCONNECTED_NOTICE, FOCUS_HISTORY_EMPTY_NOTICE,
    FOCUS_HISTORY_EXPIRED_NOTICE, FOCUS_HISTORY_MIN_API_REVISION, FOCUS_HISTORY_REQUIRES_API,
    FOCUS_HISTORY_SYNC_TIMEOUT_NOTICE, FOCUS_HISTORY_UNSUPPORTED_NOTICE, FocusHistoryResult,
    FocusHistoryScope,
};
pub use view_fetch::ViewFetchOutcome;

/// The label new tabs get. A tab that still has it shows its program's terminal title.
pub const DEFAULT_TAB_LABEL: &str = "Shell";

#[derive(Clone, Debug, clap::Parser)]
#[command(
    version,
    about = "Native multi-host workspace for existing Rex servers"
)]
pub struct Options {
    #[arg(long, default_value_t = transport::local_endpoint())]
    pub endpoint: String,
    /// Initially select a session by ID or exact label.
    #[arg(long)]
    pub session: Option<String>,
    /// Initially focus this terminal block.
    #[arg(long)]
    pub block: Option<String>,
    /// Explicitly create a persistent session.
    #[arg(long, conflicts_with = "session")]
    pub new: Option<String>,
    /// Override the saved terminal family for this launch.
    #[arg(long, value_parser = crate::settings::parse_font_family)]
    pub font: Option<String>,
    /// Override saved fallback families, in order; repeat for more than one.
    #[arg(long, value_parser = crate::settings::parse_font_family)]
    pub font_fallback: Vec<String>,
    /// Override the saved terminal size (8–48 pt).
    #[arg(long, value_parser = parse_font_size)]
    pub font_size: Option<f32>,
    /// Use a separate workspace preferences file.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Startup size, as WxH; else the size the window last had, else 1440x900.
    #[arg(long)]
    pub window_size: Option<String>,
    /// Check signed release metadata for updates and exit (0 = up to date, 10 = update available, 1 = error).
    #[arg(long)]
    pub check_update: bool,
    /// Rune's own flags (`--snap out.png`, `--tree`, `--script steps.json`), after `--`.
    #[arg(last = true, hide = true)]
    pub rune: Vec<String>,
}

impl Options {
    /// The command line of this process. Started through the old `rex-linux` name, it names
    /// itself that way: the 0.7.x updater and older scripts check that `--version` prints
    /// exactly `rex-linux X.Y.Z` before they install or accept the binary.
    pub fn from_command_line() -> Self {
        Self::from_arguments(std::env::args_os(), crate::legacy::invoked_as_legacy())
    }

    fn from_arguments(
        arguments: impl IntoIterator<Item = std::ffi::OsString>,
        legacy_name: bool,
    ) -> Self {
        use clap::{CommandFactory as _, FromArgMatches as _};
        let mut command = Self::command();
        if legacy_name {
            command = command.name(crate::legacy::NAME);
        }
        let matches = command.get_matches_from(arguments);
        Self::from_arg_matches(&matches).unwrap_or_else(|error| error.exit())
    }

    /// What `--version` prints, under the current name or the old one.
    #[cfg(test)]
    fn version_line(legacy_name: bool) -> String {
        use clap::CommandFactory as _;
        let mut command = Self::command();
        if legacy_name {
            command = command.name(crate::legacy::NAME);
        }
        command.render_version()
    }
}

impl Options {
    pub fn startup_size(&self, settings: &crate::settings::Settings) -> (f32, f32) {
        self.window_size
            .as_deref()
            .and_then(|size| size.split_once('x'))
            .and_then(|(w, h)| Some([w.parse::<f32>().ok()?, h.parse::<f32>().ok()?]))
            .filter(|size| size.iter().all(|v| v.is_finite()))
            .or(settings.window_size)
            .filter(|size| size.iter().all(|v| v.is_finite()))
            .map_or((1440., 900.), |[w, h]| (w.max(640.), h.max(480.)))
    }

    pub fn fonts<'a>(&'a self, settings: &'a crate::settings::Settings) -> (&'a str, &'a [String]) {
        (
            self.font.as_deref().unwrap_or(&settings.font_family),
            if self.font_fallback.is_empty() {
                &settings.font_fallbacks
            } else {
                &self.font_fallback
            },
        )
    }
}

fn parse_font_size(value: &str) -> std::result::Result<f32, String> {
    let size: f32 = value
        .parse()
        .map_err(|_| "Enter a font size in points.".to_owned())?;
    if !size.is_finite() {
        return Err("Font size must be finite.".into());
    }
    Ok(size.clamp(8., 48.))
}

pub mod actions;
pub mod keymap;
pub mod perform;
pub mod prompt;

pub enum Command {
    /// Local receipt carried beside a reset input, never serialized on the wire.
    EchoReset(Box<(Command, InputReceipt)>),
    Selection(crate::terminal::selection::Input),
    CopyMode(crate::terminal::copy_mode::CopyModeInput),
    Key(Value),
    KeyWithPrefix {
        key: Value,
        prefix: Vec<String>,
    },
    /// Explicit pane.send_key only; normal keys always use client.key.
    SendKey(Value),
    Paste(String),
    /// A `block.event` mouse payload: `action`, `button`, `x`, `y` (cells) and `mods`.
    Mouse(Value),
    /// A `block.event` wheel payload: `x`, `y`, `delta_x` and `delta_y`.
    Wheel(Value),
    Resize(u16, u16, u16, u16),
    Scroll(isize),
    /// Search the terminal's screen and scrollback for this text; empty stops searching.
    Search(String),
    /// Select the next older search match (`true`) or the next newer one.
    SearchStep(bool),
    /// Move this client viewport to the previous (true) or next OSC 133 prompt.
    PromptStep(bool),
    /// Local cursor default; never sent to the server or used to claim ownership.
    CursorStyle(crate::terminal::CursorShape),
    TakeControl,
    Reconnect,
    /// `terminal.reset`: the terminal back to its starting state, screen and scrollback
    /// cleared, the program left running. Sent once, to this block only.
    Reset,
    /// `terminal.clear`: the screen and scrollback cleared, the current line kept.
    Clear,
}

impl Command {
    fn untracked(&self) -> &Self {
        match self {
            Self::EchoReset(tracked) => &tracked.0,
            other => other,
        }
    }
}

/// Host-wide smoothed round trip. Zero encodes unknown; one encodes a measured zero.
#[derive(Default, Debug)]
pub struct Rtt {
    value: std::sync::atomic::AtomicU32,
    enabled: std::sync::atomic::AtomicBool,
}
impl Rtt {
    /// Local preference shared by this host's control and data workers.
    pub fn enable(&self, enabled: bool) {
        self.enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }
    fn enabled(&self) -> bool {
        self.enabled.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn ms(&self) -> Option<u32> {
        self.value
            .load(std::sync::atomic::Ordering::Relaxed)
            .checked_sub(1)
    }
    fn record(&self, elapsed: Duration) {
        let sample = elapsed.as_millis().min(5000) as u32;
        let value = self
            .ms()
            .map_or(sample, |ms| (3 * ms.min(5000) + sample) / 4);
        self.value
            .store(value + 1, std::sync::atomic::Ordering::Relaxed);
    }
    fn clear(&self) {
        self.value.store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Only opt-in panes allocate a counter shared by the control and data workers.
#[derive(Default)]
pub struct InputAcks {
    issued: AtomicU64,
    slot: OnceLock<Arc<AtomicU64>>,
}
pub struct InputReceipt {
    seq: u64,
    slot: Arc<AtomicU64>,
}
impl InputReceipt {
    fn complete(self) {
        self.slot.fetch_max(self.seq, Ordering::Release);
    }
}
impl InputAcks {
    fn slot(&self) -> Arc<AtomicU64> {
        self.slot.get_or_init(Default::default).clone()
    }
    fn disconnect(&self) {
        if let Some(slot) = self.slot.get() {
            slot.store(0, Ordering::Release);
        }
    }
    /// Only server-bound resets need a reply. View operations keep their ordinary frame barrier.
    pub fn track_reset(&self, command: Command) -> (Command, Option<u64>) {
        if !matches!(
            command,
            Command::Key(_)
                | Command::KeyWithPrefix { .. }
                | Command::SendKey(_)
                | Command::Paste(_)
                | Command::Mouse(_)
                | Command::Resize(..)
                | Command::Reset
                | Command::Clear
        ) {
            return (command, None);
        }
        let seq = self.issued.fetch_add(1, Ordering::Relaxed) + 1;
        (
            Command::EchoReset(Box::new((
                command,
                InputReceipt {
                    seq,
                    slot: self.slot(),
                },
            ))),
            Some(seq),
        )
    }
}

#[derive(Default)]
pub struct Model {
    pub input_acks: InputAcks,
    pub rtt: Arc<Rtt>,
    pub stream_epoch: u64,
    pub frame: Option<Arc<Frame>>,
    pub status: String,
    pub session: String,
    pub block: String,
    pub connected: bool,
    pub control_connected: bool,
    pub owner: bool,
    pub revision: u64,
    pub restore_generation: u64,
    pub input_events: u64,
    /// Words for the user from the pane's worker (a failed search), taken once shown.
    pub notice: Option<String>,
    /// Latest completed copy-mode yank (`Yank` / `YankLine`), taken once by the UI so a
    /// fast `Yank` followed by `End` never races with selection clearing.
    pub copy_yank: Option<crate::terminal::copy_mode::CopyYank>,
    /// Highest copy-mode epoch ended automatically by the pane worker (screen switch,
    /// lost tracked reference, or fresh snapshot/reconnect); monotonic so the UI
    /// reconciles once.
    pub copy_ended_epoch: u64,
}

#[derive(Clone)]
pub struct TerminalSender {
    host: flume::Sender<HostCommand>,
    session: String,
    block: String,
}
impl TerminalSender {
    /// Commands for one block, sent through its host's worker.
    pub fn new(host: flume::Sender<HostCommand>, session: String, block: String) -> Self {
        Self {
            host,
            session,
            block,
        }
    }
    pub fn focus(&self) -> Result<()> {
        self.host
            .send(HostCommand::Request {
                method: "session.focus_block".into(),
                payload: json!({"session_id":self.session,"block_id":self.block}),
            })
            .map_err(|_| anyhow::anyhow!("host worker stopped"))
    }
    pub fn try_send(&self, command: Command) -> Result<()> {
        self.host
            .send(HostCommand::Terminal {
                session: self.session.clone(),
                block: self.block.clone(),
                command,
            })
            .map_err(|_| anyhow::anyhow!("host worker stopped"))
    }
}

#[derive(Clone)]
pub struct Connection {
    pub model: Arc<Mutex<Model>>,
    pub commands: TerminalSender,
    pub wake: flume::Receiver<()>,
}

#[derive(Clone, Default)]
pub struct ProtocolStats {
    pub api_revision: u32,
    pub bootstrap_requests: u64,
    pub attach_requests: u64,
    pub bundled_tickets_used: u64,
    pub data_connect_requests: u64,
}

/// What the person is told when the server took a stop request.
const STOP_ACCEPTED: &str = "Server stop accepted. Rex Rune will not start it again.";

#[derive(Clone, Default)]
pub struct HostModel {
    pub rtt: Arc<Rtt>,
    /// Setup problems from the last read-only SSH lookup.
    pub ssh_problem: Option<crate::ssh::Problem>,
    pub visible: bool,
    pub connected: bool,
    pub status: String,
    pub notice: Option<String>,
    pub client_id: String,
    pub keymap: Option<Arc<keymap::Snapshot>>,
    pub keymap_error: Option<String>,
    pub keymap_revision: u64,
    /// Taken once before cloning the worker model into the UI.
    pub client_actions: Vec<actions::Queued>,
    pub performs: Vec<perform::Pending>,
    /// Epoch-scoped prompt transitions, oldest first; drain once in the consumer.
    pub prompt_events: VecDeque<prompt::PromptEvent>,
    /// Number of `client.action` calls currently awaiting a server response on this connection.
    pub actions_in_flight: usize,
    /// This launch's downloads from the host, oldest first; see [`crate::download`].
    pub downloads: Vec<crate::download::Transfer>,
    pub connection_epoch: u64,
    /// Last explicit stop outcome, kept on Server settings as well as in the host notice.
    pub server_stop_result: Option<String>,
    pub protocol: ProtocolStats,
    pub sessions: Vec<session::Session>,
    pub selected: Option<String>,
    /// Completion of the latest client navigation request (id, success), not a notice.
    pub navigation_result: Option<(u64, bool)>,
    /// Completion of the latest `focus_history.reverse` request on this connection epoch.
    pub focus_history_result: Option<FocusHistoryResult>,
    /// Completed on-demand read-only session view fetches, drained once by the UI.
    pub view_fetches: Vec<(u64, ViewFetchOutcome)>,
    /// Most recent read-only directory reply; only its browser/request may consume it.
    pub directory: Option<Arc<directory::Reply>>,
    pub views: HashMap<String, session::ViewResponse>,
    /// Sessions whose layout is held only for activity: read in the background and never
    /// selected on this connection. The app lists and steps through their tabs only where
    /// it lists them, so knowing a layout is not the same as having opened the session.
    pub background_views: HashSet<String>,
    pub terminals: HashMap<String, Connection>,
    /// Each terminal block's OSC 7501 records, by record id (empty for the root record).
    pub activity: HashMap<String, BTreeMap<String, StatusRecord>>,
    /// Each terminal block's foreground program, when the server named one.
    pub programs: HashMap<String, String>,
    /// The executable behind each block's foreground program (a script's interpreter), for
    /// its icon only.
    pub executables: HashMap<String, String>,
    /// The path of each block's foreground script, for its icon only.
    pub scripts: HashMap<String, String>,
    /// Each terminal block's foreground process and its ancestors, by pid.
    pub foreground: HashMap<String, Vec<u64>>,
    /// Each terminal block's title, as a program last set it, and the pid that set it.
    pub titles: HashMap<String, (String, Option<u64>)>,
    /// Each terminal's foreground directory, home and own shell, for display-only titles.
    pub places: HashMap<String, titles::Place>,
    /// Each terminal block's OSC 9;4 progress, until its program removes it.
    pub progress: HashMap<String, TerminalProgress>,
    /// Program notifications not yet taken by the UI.
    pub notices: Vec<DesktopNotice>,
    /// Bells not yet taken by the UI, as (session, block).
    pub bells: Vec<(String, String)>,
    /// Latest terminal-initiated copy, consumed once by the foreground UI.
    pub clipboard: Option<clipboard::ClipboardCopy>,
    /// Bumped whenever `activity` is dropped wholesale (on reconnect), so the UI starts its
    /// before-and-after comparison over instead of reading the gap as programs finishing.
    pub activity_epoch: u64,
    /// Panes whose program status was read on this connection: a first report from one of
    /// these is a state this client watched begin. Presentation only.
    pub status_read: HashSet<String>,
    /// Initial activity coverage on this connection, not a read receipt.
    pub activity_reading: bool,
    pub activity_incomplete: bool,
    /// The server refused `set_theme` on this connection.
    pub themes_unsupported: bool,
    /// A theme was declared to this server, by `set_theme` or in a new block's layout.
    pub theme_declared: bool,
    /// What the server says about itself; see [`server_info`].
    pub server_info: server_info::ServerInfo,
    /// Shared host list from this server (`hosts.list` / `hosts_changed`).
    pub shared: hosts::SharedHostState,
    /// Bumped whenever `shared` updates from a `hosts.list` reply.
    pub shared_revision: u64,
    /// Completed shared-host write outcomes (`hosts.put` / `hosts.remove`), taken once by the UI.
    pub shared_writes: Vec<hosts::WriteResult>,
    /// Listening TCP ports last read or reported for each terminal block in a held view.
    pub ports: HashMap<String, crate::ports::PortSnapshot>,
    /// Terminal blocks with an on-demand `com.superlogical.terminal.ports` read in flight or queued.
    pub ports_reading: HashSet<String>,
    /// Monotonic count of completed `com.superlogical.terminal.ports` reads on this connection.
    pub ports_read_completions: u64,
}
impl HostModel {
    /// Record a block's owner (none when nobody owns it), in its session's view and its pane.
    fn set_owner(&mut self, block: &str, owner: Option<&str>) {
        for view in self.views.values_mut() {
            for b in view
                .windows
                .iter_mut()
                .flat_map(|w| w.layers.iter_mut())
                .flat_map(|l| l.blocks.iter_mut())
                .filter(|b| b.block_id.as_deref() == Some(block))
            {
                b.owner = owner.map(str::to_owned);
            }
        }
        if let Some(pane) = self.terminals.get(block) {
            pane.model.lock().expect("terminal model").owner = owner == Some(&self.client_id);
        }
    }

    /// Whether Rex applies this client's theme to `block` at once: this client owns it, or
    /// nobody does.
    fn themable(&self, session: &str, block: &str) -> bool {
        self.views.get(session).is_some_and(|view| {
            view.windows
                .iter()
                .flat_map(|w| &w.layers)
                .flat_map(|l| &l.blocks)
                .find(|b| b.block_id.as_deref() == Some(block))
                .is_some_and(|b| b.owner.as_deref().is_none_or(|o| o == self.client_id))
        })
    }

    pub fn apply_block_event(&mut self, session: &str, block: String, event: BlockEvent) {
        match event {
            BlockEvent::StatusChanged { id, mut record } => {
                let read = self.status_read.contains(&block);
                let records = self.activity.entry(block).or_default();
                record.observe_at(records.get(&id), read, std::time::Instant::now());
                records.insert(id, record);
            }
            BlockEvent::StatusRemoved { ids } => {
                if let Some(records) = self.activity.get_mut(&block) {
                    for id in ids {
                        records.remove(&id);
                    }
                    if records.is_empty() {
                        self.activity.remove(&block);
                    }
                }
            }
            BlockEvent::Process {
                name,
                executable,
                script,
                pids,
                place,
            } => {
                self.set_place(&block, place);
                // A progress report belongs to the program in the foreground; one that ends
                // without clearing its report (a crash, a kill) must not leave it on the tab.
                // Another run of the same program has the same name, so its pid counts too;
                // a child taking the foreground leaves its parent's pid in the chain.
                let was = self.programs.get(&block).cloned();
                let left = self
                    .foreground
                    .get(&block)
                    .and_then(|chain| chain.first())
                    .is_some_and(|pid| !pids.contains(pid));
                match name {
                    Some(name) => self.programs.insert(block.clone(), name),
                    None => self.programs.remove(&block),
                };
                match executable {
                    Some(executable) => self.executables.insert(block.clone(), executable),
                    None => self.executables.remove(&block),
                };
                match script {
                    Some(script) => self.scripts.insert(block.clone(), script),
                    None => self.scripts.remove(&block),
                };
                if left || self.programs.get(&block) != was.as_ref() {
                    self.progress.remove(&block);
                }
                if pids.is_empty() {
                    self.foreground.remove(&block);
                } else {
                    self.foreground.insert(block, pids);
                }
            }
            BlockEvent::Title { text, owner } => {
                self.set_title(block, text, owner);
            }
            BlockEvent::Notification { title, body } => {
                // A program spamming notifications can't grow this without bound.
                if self.notices.len() < 32 {
                    self.notices.push(DesktopNotice {
                        session: session.into(),
                        block,
                        title,
                        body,
                    });
                }
            }
            BlockEvent::Progress(Some(progress)) => {
                self.progress.insert(block, progress);
            }
            BlockEvent::Progress(None) => {
                self.progress.remove(&block);
            }
            BlockEvent::Bell => {
                // A program ringing in a loop can't grow this without bound.
                if self.bells.len() < 32 {
                    self.bells.push((session.into(), block));
                }
            }
        }
    }
    fn set_title(&mut self, block: String, text: String, owner: Option<u64>) -> bool {
        if text.is_empty() {
            return self.titles.remove(&block).is_some();
        }
        let value = (text, owner);
        if self.titles.get(&block) == Some(&value) {
            return false;
        }
        self.titles.insert(block, value);
        true
    }

    fn forget_block(&mut self, block: &str) {
        self.activity.remove(block);
        self.status_read.remove(block);
        self.programs.remove(block);
        self.executables.remove(block);
        self.scripts.remove(block);
        self.foreground.remove(block);
        self.titles.remove(block);
        self.places.remove(block);
        self.progress.remove(block);
        self.ports.remove(block);
        self.ports_reading.remove(block);
    }

    /// The block's terminal title while it's current. Shells rarely clear a title when the
    /// program that set it exits, so it counts only while that program, or one it runs under,
    /// is still in the foreground.
    pub fn title(&self, block: &str) -> Option<&str> {
        let (text, owner) = self.titles.get(block)?;
        match (owner, self.foreground.get(block)) {
            (Some(pid), Some(pids)) if !pids.contains(pid) => None,
            _ => Some(text),
        }
    }

    pub fn view(&self) -> Option<&session::ViewResponse> {
        self.views.get(self.selected.as_ref()?)
    }
    pub fn window(&self) -> Option<&session::Window> {
        let view = self.view()?;
        view.windows
            .iter()
            .find(|w| w.window_id == view.active_window_id)
            .or(view.windows.first())
    }
    pub fn focused(&self) -> Option<&str> {
        self.window()?.focused_block_id.as_deref()
    }
}

pub const UNDO_CLOSE_MIN_API_REVISION: u32 = 20;
pub const UNDO_CLOSE_REQUIRES_API: &str = "Requires Rex Server API 20 or later";
pub const UNDO_CLOSE_DISCONNECTED: &str = "Connect to the host to undo a close";

/// Only the close methods that accept `undoable`. Probes and automated requests use `Request`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HumanClose {
    Pane,
    Tab,
    Session,
}

impl HumanClose {
    pub fn method(self) -> &'static str {
        match self {
            Self::Pane => "block.close",
            Self::Tab => "session.close_window",
            Self::Session => "session.destroy",
        }
    }
}

pub enum HostCommand {
    Close {
        kind: HumanClose,
        payload: Value,
    },
    /// Undo only on the connection the person asked on, with no replay.
    UndoClose {
        epoch: u64,
    },
    /// Step back through server-side focus history on this connection epoch only (API 19+).
    /// Never replayed across disconnect/reconnect or sent past `deadline`.
    ReverseFocusHistory {
        epoch: u64,
        id: u64,
        scope: FocusHistoryScope,
        cursor: Option<u64>,
        deadline: Instant,
    },
    /// A person's confirmed, single attempt on this connection only. Never replayed.
    StopServer {
        epoch: u64,
    },
    PerformReply {
        epoch: u64,
        call_id: String,
        outcome: perform::Outcome,
    },
    /// Opt in only while a prompt consumer is available. Off by default.
    Prompts(bool),
    /// Answer an offered prompt on the same connection, never replayed.
    PromptReply {
        epoch: u64,
        call_id: String,
        answer: prompt::PromptAnswer,
    },
    Keymap(Box<keymap::Declaration>),
    /// Read the server's version once on each connection from now on, and its status again
    /// when the server says it changed; see [`server_info`]. Read-only, and a refusal is quiet.
    Identify,
    /// Read `hosts.list` once per connection and re-read on `hosts_changed`; see [`hosts`].
    WatchSharedHosts,
    /// Open or close on-demand listening-port inspection for `session` on `epoch` only (API 15+).
    /// `Some(session)` starts reads for that session's terminal blocks and enables `event_dropped`
    /// re-reads for it; `None` clears the watched session and cancels in-flight port reads.
    WatchPorts {
        epoch: u64,
        session: Option<String>,
    },
    /// One-shot on-demand listening-port read for `session`'s terminal blocks on `epoch` only
    /// (API 15+), without changing the watched session.
    ReadPorts {
        epoch: u64,
        session: String,
    },
    /// Create or update one host in this server's shared host list (`hosts.put`) on `epoch` only.
    /// Never auto-retried or queued across reconnects.
    PutSharedHost {
        id: u64,
        epoch: u64,
        host: Value,
    },
    /// Remove one host from this server's shared host list (`hosts.remove`) on `epoch` only.
    /// Never auto-retried or queued across reconnects.
    RemoveSharedHost {
        id: u64,
        epoch: u64,
        host_id: String,
    },
    Action {
        generation: u64,
        request: Box<rexrs::api::client::Action>,
    },
    Visibility(bool),
    /// Take size ownership of these blocks, as the native Rex clients do when they attach or
    /// come forward. Failures are silent: another client may have closed a block meanwhile.
    Claim {
        session: String,
        blocks: Vec<String>,
    },
    Select(String),
    /// Read the session list and the selected session's layout again, on request. Read-only:
    /// nothing is attached, created or closed.
    RefreshSessions,
    /// Select a session and optionally focus a window as one correlated navigation.
    Navigate {
        id: u64,
        /// The UI's deadline, including time spent queued behind other commands.
        deadline: Instant,
        session: String,
        window: Option<String>,
    },
    /// The sessions whose views to load in the background, for listing their tabs; empty
    /// stops loading.
    PrefetchViews(HashSet<String>),
    /// Read-only `session.view` fetch for a listed unloaded session, without attaching or
    /// selecting it.
    FetchView {
        id: u64,
        epoch: u64,
        deadline: Instant,
        session: String,
    },
    /// A new session: the server picks a unique label when `label` is none; `shell` starts
    /// its first terminal.
    CreateSession {
        label: Option<String>,
        shell: Option<String>,
    },
    Request {
        method: String,
        payload: Value,
    },
    /// Move or resize a floating layer on this connection epoch only (API revision 13).
    /// Checked against the live epoch and negotiated API revision right before sending; never
    /// replayed across disconnect/reconnect.
    ResizeLayer {
        epoch: u64,
        session: String,
        layer: String,
        bounds: [f64; 4],
        raise_first: bool,
    },
    /// Explicit tab/split creation, inheriting the named source block's shell directory.
    /// A failed or slow read leaves the server's default starting directory unchanged.
    NewTerminal {
        method: String,
        payload: Value,
        cwd_from: String,
    },
    Terminal {
        session: String,
        block: String,
        command: Command,
    },
    /// Upload files to the block's machine; Rex types their paths into it once all arrive.
    Upload {
        session: String,
        block: String,
        files: Vec<crate::upload::File>,
    },
    /// Download a file from the block's machine into a folder here (API revision 13). The
    /// transfer appears in `HostModel::downloads` under the request's id.
    Download(Box<downloads::Request>),
    ListDir(Box<directory::Request>),
    /// Start the stream of a download that is `AwaitingConfirmation`.
    DownloadConfirm(u64),
    /// Stop a download at any point; its partial file is deleted.
    DownloadCancel(u64),
    /// Update `SSH_AUTH_SOCK` for future SSH runs on this host without dropping live
    /// connections. When `wake_auth_retry` is true and this is a disconnected `ssh://` host
    /// whose last error was `Problem::AuthFailed`, cut short backoff and retry immediately.
    SshAgent {
        socket: Option<PathBuf>,
        wake_auth_retry: bool,
    },
    Reconnect,
    /// The terminal theme to declare from now on, also while disconnected; none declares
    /// nothing (Follow server).
    Theme(Option<SetTheme>),
    Shutdown,
    Ticket {
        session: String,
        block: String,
        reply: tokio::sync::oneshot::Sender<std::result::Result<TerminalTicket, String>>,
    },
}

// Tokens deliberately have no Debug implementation.
pub struct TerminalTicket {
    token: String,
    bundled: bool,
}

struct PendingTicket {
    token: String,
    expires: Instant,
}

#[derive(Default)]
struct ControlState {
    // Earliest possible arrival of events buffered during the preceding busy step.
    performs_since: Option<Instant>,
    prompts_enabled: bool,
    prompts: prompt::PromptState,
    actions: actions::Waits,
    bootstrap_seq: u64,
    list_seq: u64,
    view_seq: HashMap<String, u64>,
    /// Sessions attached on this control connection (via `session.bootstrap` or `session.attach`).
    attached: HashSet<String>,
    /// Blocks whose title and initial process metadata were asked for.
    queried: HashSet<String>,
    status_queried: HashSet<String>,
    status_failed: HashSet<String>,
    /// The blocks whose background status reads are out, each with the record ids removed
    /// live meanwhile: a detached reply is handled after events that arrived behind it,
    /// so the read must not bring those records back.
    status_reading: HashMap<String, HashSet<String>>,
    /// Sessions whose initial background session.view was attempted (including failures).
    viewed: HashSet<String>,
    /// Unplaced sessions a live report arrived for: their layout is read next, ahead of
    /// the rest of the pass, so the report's tab (and its notification) does not wait.
    urgent_views: Vec<String>,
    /// The session whose background layout read is out, and whether its layout changed
    /// meanwhile: that change was ignored for want of a layout, so it is read once more.
    view_reading: Option<String>,
    view_stale: bool,
    /// Sessions with an in-flight on-demand `FetchView` whose layout changed meanwhile.
    fetch_reading: HashSet<String>,
    fetch_stale: HashSet<String>,
    /// Last attempted declaration per block, including inconclusive replies.
    themed: HashMap<String, SetTheme>,
    theme_set_supported: Option<bool>,
    // This pool is scoped to the terminal data ref and subprotocol; other sources
    // are filtered out before insertion. It lives only as long as this control socket.
    tickets: HashMap<(String, String), PendingTicket>,
    /// Highest control `seq` applied to each block's listening-ports snapshot.
    ports_seq: HashMap<String, u64>,
    /// On-demand detached `com.superlogical.terminal.ports` reader for this connection.
    ports_reads: ports::Reader,
}

impl ControlState {
    async fn prompt_event(
        &mut self,
        control: &mut Client,
        shared: &Shared,
        event: rexrs::Event,
    ) -> Result<()> {
        if let Some(deadline) = self
            .prompts
            .event(control, shared, self.prompts_enabled, event)
            .await?
        {
            self.actions.promote(deadline);
        }
        Ok(())
    }

    fn add_tickets(&mut self, tickets: Vec<session::DataTicket>, started: Instant) {
        for ticket in tickets {
            if ticket.data_ref_id != "terminal"
                || ticket
                    .subprotocol
                    .as_deref()
                    .is_some_and(|p| p != wire::PROTOCOL)
            {
                continue;
            }
            if let Some(expires) = started.checked_add(Duration::from_millis(ticket.expires_in_ms))
            {
                self.tickets.insert(
                    (ticket.session_id, ticket.block_id),
                    PendingTicket {
                        token: ticket.ticket,
                        expires,
                    },
                );
            }
        }
    }

    fn take_ticket(&mut self, session: &str, block: &str) -> Option<TerminalTicket> {
        let ticket = self.tickets.remove(&(session.into(), block.into()))?;
        (Instant::now() < ticket.expires).then_some(TerminalTicket {
            token: ticket.token,
            bundled: true,
        })
    }

    fn prune(&mut self, model: &HostModel) {
        self.view_seq.retain(|id, _| model.views.contains_key(id));
        self.tickets.retain(|(id, block), ticket| {
            Instant::now() < ticket.expires
                && model.views.get(id).is_some_and(|view| {
                    view.windows
                        .iter()
                        .flat_map(|w| &w.layers)
                        .flat_map(|l| &l.blocks)
                        .any(|b| b.block_id.as_deref() == Some(block))
                })
        });
    }

    fn forget_block(&mut self, block: &str) {
        self.queried.remove(block);
        self.status_queried.remove(block);
        self.status_failed.remove(block);
        self.status_reading.remove(block);
        self.themed.remove(block);
        self.tickets.retain(|(_, b), _| b != block);
        self.ports_seq.remove(block);
        self.ports_reads.cancel_block(block);
    }

    fn forget_session(&mut self, id: &str, blocks: impl IntoIterator<Item = String>) {
        self.view_seq.remove(id);
        self.attached.remove(id);
        self.viewed.remove(id);
        self.urgent_views.retain(|s| s != id);
        if self.view_reading.as_deref() == Some(id) {
            self.view_stale = true;
        }
        self.fetch_stale.remove(id);
        self.tickets.retain(|(s, _), _| s != id);
        self.ports_reads.cancel_session(id);
        for block in blocks {
            self.forget_block(&block);
        }
    }

    fn includes(&self, event: &rexrs::Event) -> bool {
        let mut floor = self.bootstrap_seq;
        let event_type = if event.name == "event_dropped" {
            event.payload["event_type"].as_str().unwrap_or("")
        } else {
            event.name.as_str()
        };
        let list_event = matches!(
            event_type,
            "session_created" | "session_moved" | "session_destroyed"
        );
        if list_event || event_type == "session_label_changed" {
            floor = floor.max(self.list_seq);
        }
        // Views contain state, not transient clipboard events, ports_changed events, or
        // list-level session events.
        let session = event.session_id.as_deref().or_else(|| {
            (event.name == "event_dropped")
                .then(|| event.payload["session_id"].as_str())
                .flatten()
        });
        let is_copy = event.name == "block_event" && clipboard::is_copy(&event.payload);
        let is_ports = event.name == "block_event" && ports::is_ports_changed(&event.payload);
        // `session.view` carries layout, not program status, title, or process metadata, so
        // only `session_view_changed` drops are superseded by a newer `view_seq`.
        let view_covered = event.name != "event_dropped" || event_type == "session_view_changed";
        if !list_event
            && !is_copy
            && !is_ports
            && view_covered
            && let Some(id) = session
        {
            floor = floor.max(self.view_seq.get(id).copied().unwrap_or(0));
        }
        event.seq > floor
    }
}

fn select_session(sessions: &[session::Session], wanted: Option<&str>) -> Option<String> {
    let Some(wanted) = wanted else {
        return sessions.first().map(|s| s.session_id.clone());
    };
    let mut matches = sessions
        .iter()
        .filter(|s| wanted == s.session_id || wanted == s.label);
    let first = matches.next()?;
    matches.next().is_none().then(|| first.session_id.clone())
}

fn session_id(value: &str) -> bool {
    let Some(uuid) = value.strip_prefix("session:") else {
        return false;
    };
    uuid.len() == 36
        && uuid.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

fn bootstrap_request(
    wanted: Option<&str>,
    creating: bool,
    retained: impl Iterator<Item = String>,
) -> session::Bootstrap {
    let mut ids: Vec<_> = retained.collect();
    if let Some(id) = wanted.filter(|s| session_id(s)) {
        ids.push(id.into());
    }
    ids.sort();
    ids.dedup();
    let request = session::Bootstrap::new("all").data_tickets([session::BootstrapDataTicket::new(
        "com.superlogical.terminal",
        "terminal",
    )
    .subprotocol(wire::PROTOCOL)]);
    if wanted.is_some() || creating || !ids.is_empty() {
        request.attach_session_ids(ids)
    } else {
        request
    }
}

pub struct HostConnection {
    pub model: Arc<Mutex<HostModel>>,
    pub commands: flume::Sender<HostCommand>,
    pub wake: flume::Receiver<()>,
}
impl Drop for HostConnection {
    fn drop(&mut self) {
        let _ = self.commands.send(HostCommand::Shutdown);
    }
}

#[derive(Clone)]
struct Shared {
    model: Arc<Mutex<HostModel>>,
    wake: Arc<title_wake::HostWake>,
}
impl Shared {
    fn update(&self, change: impl FnOnce(&mut HostModel)) {
        change(&mut self.model.lock().expect("host model"));
        self.wake.notify();
    }
    fn status(&self, status: impl Into<String>, connected: bool) {
        self.update(|m| {
            m.status = status.into();
            m.connected = connected;
            if !connected {
                m.rtt.clear();
                self.wake.reset_titles();
                m.clipboard = None;
                m.prompt_events.clear();
                m.actions_in_flight = 0;
                m.keymap = None;
                m.keymap_revision += 1;
                m.ports.clear();
                m.ports_reading.clear();
                m.focus_history_result = None;
            }
            for terminal in m.terminals.values() {
                let mut pane = terminal.model.lock().expect("terminal model");
                if pane.control_connected != connected {
                    pane.stream_epoch += 1;
                    pane.input_acks.disconnect();
                }
                pane.control_connected = connected;
            }
        });
    }
}

struct PaneShared {
    model: Arc<Mutex<Model>>,
    wake: flume::Sender<()>,
    host_wake: Arc<title_wake::HostWake>,
}
impl PaneShared {
    fn update(&self, change: impl FnOnce(&mut Model)) {
        let mut m = self.model.lock().expect("terminal model");
        change(&mut m);
        m.revision += 1;
        let _ = self.wake.try_send(());
    }
    fn status(&self, status: impl Into<String>, connected: bool) {
        self.update(|m| {
            m.status = status.into();
            if !connected || !m.connected {
                m.stream_epoch += 1;
                m.input_acks.disconnect();
            }
            m.connected = connected;
        });
        self.host_wake.notify();
    }
}

enum DataCommand {
    CursorStyle(crate::terminal::CursorShape),
    Selection(crate::terminal::selection::Input),
    CopyMode(crate::terminal::copy_mode::CopyModeInput),
    Visible(bool),
    Scroll(isize),
    Search(String),
    SearchStep(bool),
    PromptStep(bool),
    Bottom,
    Reconnect,
    Stop,
}
struct PaneWorker {
    session: String,
    commands: flume::Sender<DataCommand>,
    task: tokio::task::JoinHandle<()>,
    size: Option<(u16, u16, u16, u16)>,
}
impl Drop for PaneWorker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The host worker threads that have started and not yet ended, named by their model's address.
static RUNNING_HOSTS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

fn running_hosts() -> std::sync::MutexGuard<'static, Vec<usize>> {
    RUNNING_HOSTS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Lists one host worker thread for as long as it lives, runtime and tasks included.
struct HostThread(usize);
impl Drop for HostThread {
    fn drop(&mut self) {
        running_hosts().retain(|host| *host != self.0);
    }
}

fn wait_until(timeout: Duration, done: impl Fn(&[usize]) -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if done(&running_hosts()) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Waits until every host worker thread has ended, or `timeout` has passed; returns whether they
/// all have. To exit cleanly: drop the [`HostConnection`]s (each sends `HostCommand::Shutdown`),
/// then call this. A host that ends drops its transfers, and a transfer's partial file is deleted
/// as it drops, so a download in flight leaves nothing behind. The threads are detached: a
/// process that exits without waiting ends them mid-transfer, and the partial files stay for
/// [`crate::download::Records::clean_up_leftovers`] at the next launch.
pub fn wait_for_hosts(timeout: Duration) -> bool {
    wait_until(timeout, |hosts| hosts.is_empty())
}

impl HostConnection {
    pub fn spawn(
        endpoint: String,
        preferred: Option<String>,
        // A session to create on first connect: its label and the shell it starts.
        create: Option<(String, Option<String>)>,
        block: Option<String>,
        visible: bool,
        // The terminal theme to declare; none declares nothing.
        theme: Option<SetTheme>,
    ) -> Self {
        Self::spawn_labeled(
            endpoint,
            preferred,
            create,
            block,
            visible,
            theme,
            crate::settings::hostname(),
        )
    }

    pub fn spawn_labeled(
        endpoint: String,
        preferred: Option<String>,
        create: Option<(String, Option<String>)>,
        block: Option<String>,
        visible: bool,
        theme: Option<SetTheme>,
        label: String,
    ) -> Self {
        Self::spawn_with_ssh_agent(
            endpoint, preferred, create, block, visible, theme, label, None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_with_ssh_agent(
        endpoint: String,
        preferred: Option<String>,
        create: Option<(String, Option<String>)>,
        block: Option<String>,
        visible: bool,
        theme: Option<SetTheme>,
        label: String,
        ssh_auth_sock: Option<PathBuf>,
    ) -> Self {
        Self::spawn_with_prompts(
            endpoint,
            preferred,
            create,
            block,
            visible,
            theme,
            label,
            ssh_auth_sock,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn_with_prompts(
        endpoint: String,
        preferred: Option<String>,
        create: Option<(String, Option<String>)>,
        block: Option<String>,
        visible: bool,
        theme: Option<SetTheme>,
        label: String,
        ssh_auth_sock: Option<PathBuf>,
        prompts_enabled: bool,
    ) -> Self {
        Self::spawn_internal(
            endpoint,
            Startup {
                preferred,
                create,
                block,
                theme,
                label,
                ssh_auth_sock,
                prompts_enabled,
                max_api_revision: transport::API_REVISION,
                #[cfg(test)]
                ssh_program: None,
                #[cfg(test)]
                tunnel_dir: None,
            },
            visible,
        )
    }

    /// Spawn a host worker offering at most `max_api_revision` in HELLO (clamped to
    /// `MIN_API_REVISION..=API_REVISION`). Used by protocol probes to verify lower-revision
    /// behavior against live servers.
    pub fn spawn_with_max_revision(
        endpoint: String,
        preferred: Option<String>,
        create: Option<(String, Option<String>)>,
        block: Option<String>,
        visible: bool,
        theme: Option<SetTheme>,
        max_api_revision: u32,
    ) -> Self {
        Self::spawn_internal(
            endpoint,
            Startup {
                preferred,
                create,
                block,
                theme,
                label: crate::settings::hostname(),
                ssh_auth_sock: None,
                prompts_enabled: false,
                max_api_revision,
                #[cfg(test)]
                ssh_program: None,
                #[cfg(test)]
                tunnel_dir: None,
            },
            visible,
        )
    }

    fn spawn_internal(endpoint: String, startup: Startup, visible: bool) -> Self {
        let model = Arc::new(Mutex::new(HostModel {
            visible,
            status: "Connecting…".into(),
            ..Default::default()
        }));
        let (commands, receiver) = flume::unbounded();
        let (wake, notifications) = flume::bounded(1);
        let shared = Shared {
            model: model.clone(),
            wake: title_wake::HostWake::new(wake),
        };
        let sender = commands.clone();
        let alive = HostThread(Arc::as_ptr(&model) as usize);
        running_hosts().push(alive.0);
        std::thread::Builder::new()
            .name("rex-host".into())
            .spawn(move || {
                // Ends after the runtime and its tasks are gone, which deletes partial files.
                let _alive = alive;
                let result = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(anyhow::Error::from)
                    .and_then(|runtime| {
                        tokio::task::LocalSet::new().block_on(
                            &runtime,
                            run_host(endpoint, startup, receiver, sender, &shared),
                        )
                    });
                if let Err(error) = result {
                    shared.status(format!("{error:#}"), false);
                }
            })
            .expect("start host worker");
        Self {
            model,
            commands,
            wake: notifications,
        }
    }
}

async fn request(control: &mut Client, method: &str, payload: Value) -> Result<Value> {
    Ok(
        tokio::time::timeout(Duration::from_secs(8), control.call_raw(method, payload))
            .await
            .context("Rex request timed out; action was not retried")??,
    )
}

/// Read on the server, not this machine or an OSC 7 cache. Bound the optional read so a
/// missing/unsupported process query cannot hold up creation indefinitely.
async fn shell_dir_of(control: &mut Client, session: &str, block: &str) -> Option<String> {
    let method = format!("{}.process", activity::TERMINAL);
    let args = json!({"session_id":session,"block_id":block,"args":{}});
    let reply = tokio::time::timeout(Duration::from_secs(1), control.call_raw(&method, args));
    activity::shell_dir(&reply.await.ok()?.ok()?)
}

async fn attach(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    sweep: &mut status_sweep::Reader,
    id: &str,
) -> Result<()> {
    attach_within(control, shared, state, sweep, id, Duration::from_secs(8)).await
}

async fn attach_within(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    sweep: &mut status_sweep::Reader,
    id: &str,
    timeout: Duration,
) -> Result<()> {
    shared.update(|m| m.protocol.attach_requests += 1);
    let reply =
        tokio::time::timeout(timeout, control.call_with_seq(session::Attach::new(id))).await??;
    state.view_seq.insert(id.into(), reply.seq);
    state.attached.insert(id.into());
    replace_view(shared, state, sweep, id, reply.value.view);
    Ok(())
}
fn apply_view(shared: &Shared, id: &str, view: session::ViewResponse) {
    shared.update(|m| apply_view_to_model(m, id, view));
}

fn replace_view(
    shared: &Shared,
    state: &mut ControlState,
    sweep: &mut status_sweep::Reader,
    id: &str,
    view: session::ViewResponse,
) {
    shared.update(|m| {
        let mut added = Vec::new();
        if let Some(old) = m.views.get(id) {
            if old.revision > view.revision {
                return;
            }
            let previous: HashSet<_> = view_blocks(old).collect();
            let current: HashSet<_> = view_blocks(&view).collect();
            let gone: Vec<_> = previous.difference(&current).cloned().collect();
            added = current.difference(&previous).cloned().collect();
            for block in gone {
                // An undoable close removes the block from the view and ends its stream
                // now, but does not deliver a control block_closed event while it is held.
                // Reconcile drops the worker; clear these reads even for unstreamed blocks.
                sweep.cancel_block(&block);
                state.forget_block(&block);
                m.forget_block(&block);
            }
        }
        apply_view_to_model(m, id, view);
        if !added.is_empty() {
            state.ports_reads.enqueue_new_watched_blocks(m, id, added);
        }
    });
}

fn apply_view_to_model(m: &mut HostModel, id: &str, view: session::ViewResponse) {
    if m.views
        .get(id)
        .is_some_and(|old| old.revision > view.revision)
    {
        return;
    }
    if let Some(label) = &view.label
        && let Some(s) = m.sessions.iter_mut().find(|s| s.session_id == id)
    {
        s.label = label.clone();
    }
    for block in view
        .windows
        .iter()
        .flat_map(|w| &w.layers)
        .flat_map(|l| &l.blocks)
    {
        if let Some(connection) = block.block_id.as_ref().and_then(|id| m.terminals.get(id)) {
            connection.model.lock().expect("terminal model").owner =
                block.owner.as_deref() == Some(&m.client_id);
        }
    }
    m.views.insert(id.into(), view);
}

const VIEW_FETCH_CAP: usize = 64;

fn record_view_fetches(
    m: &mut HostModel,
    entries: impl IntoIterator<Item = (u64, ViewFetchOutcome)>,
) {
    m.view_fetches.extend(entries);
    if m.view_fetches.len() > VIEW_FETCH_CAP {
        let extra = m.view_fetches.len() - VIEW_FETCH_CAP;
        m.view_fetches.drain(..extra);
    }
}
async fn refresh(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    sweep: &mut status_sweep::Reader,
    id: &str,
) -> Result<()> {
    let reply = tokio::time::timeout(
        Duration::from_secs(8),
        control.call_with_seq(session::View::new(id)),
    )
    .await??;
    state.view_seq.insert(id.into(), reply.seq);
    replace_view(shared, state, sweep, id, reply.value);
    Ok(())
}

/// Read an already-attached session's view via `session.attach` rather than `session.view`.
/// On API 19+, `session.view` records focus in the server's focus history while
/// `session.attach` returns the full fresh view without altering focus history.
async fn refresh_background(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    sweep: &mut status_sweep::Reader,
    id: &str,
) -> Result<()> {
    refresh_background_within(control, shared, state, sweep, id, Duration::from_secs(8)).await
}

async fn refresh_background_within(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    sweep: &mut status_sweep::Reader,
    id: &str,
    timeout: Duration,
) -> Result<()> {
    let reply =
        tokio::time::timeout(timeout, control.call_with_seq(session::Attach::new(id))).await??;
    state.view_seq.insert(id.into(), reply.seq);
    state.attached.insert(id.into());
    replace_view(shared, state, sweep, id, reply.value.view);
    Ok(())
}

/// Refresh `id`'s view for a passive event or post-request sync without altering
/// server-side focus history when this connection is already attached to `id` on API 19+.
/// Unattached sessions keep using `session.view` so Rex Rune never joins unopened or
/// explicitly detached sessions.
async fn refresh_for_event(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    sweep: &mut status_sweep::Reader,
    id: &str,
) -> Result<()> {
    if control.welcome().api_revision >= FOCUS_HISTORY_MIN_API_REVISION
        && state.attached.contains(id)
    {
        refresh_background(control, shared, state, sweep, id).await
    } else {
        refresh(control, shared, state, sweep, id).await
    }
}
async fn list(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    sweep: &mut status_sweep::Reader,
) -> Result<()> {
    list_within(control, shared, state, sweep, Duration::from_secs(8)).await
}

async fn list_within(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    sweep: &mut status_sweep::Reader,
    timeout: Duration,
) -> Result<()> {
    let reply = tokio::time::timeout(
        timeout,
        control.call_with_seq(session::AttachAll::new("all")),
    )
    .await??;
    state.list_seq = reply.seq;
    let mut removed = Vec::<(String, Vec<String>)>::new();
    shared.update(|m| {
        let prev_sessions: HashSet<String> =
            m.sessions.iter().map(|s| s.session_id.clone()).collect();
        m.sessions = reply.value.sessions;
        for session in &m.sessions {
            if let Some(view) = m.views.get_mut(&session.session_id) {
                view.label = Some(session.label.clone());
            }
        }
        let current: HashSet<String> = m.sessions.iter().map(|s| s.session_id.clone()).collect();
        let mut gone: HashSet<String> = m
            .views
            .keys()
            .filter(|id| !current.contains(*id))
            .cloned()
            .collect();
        for id in prev_sessions {
            if !current.contains(&id) {
                gone.insert(id);
            }
        }
        for id in gone {
            m.background_views.remove(&id);
            let blocks: Vec<String> = m
                .views
                .remove(&id)
                .map(|view| view_blocks(&view).collect())
                .unwrap_or_default();
            for block in &blocks {
                m.forget_block(block);
            }
            removed.push((id, blocks));
        }
        if m.selected.as_ref().is_some_and(|id| !current.contains(id)) {
            m.selected = None;
        }
    });
    for (id, blocks) in removed {
        sweep.cancel_session(&id);
        state.forget_session(&id, blocks);
    }
    Ok(())
}

fn reconcile(
    endpoint: &str,
    shared: &Shared,
    sender: &flume::Sender<HostCommand>,
    workers: &mut HashMap<String, PaneWorker>,
) {
    let model = shared.model.lock().expect("host model").clone();
    // A block can disappear while control is disconnected, or before its
    // block_closed event is drained. The current view is authoritative.
    let stale: Vec<_> = workers
        .iter()
        .filter(|(id, worker)| {
            !model
                .sessions
                .iter()
                .any(|s| s.session_id == worker.session)
                || model.views.get(&worker.session).is_some_and(|view| {
                    !view
                        .windows
                        .iter()
                        .flat_map(|w| &w.layers)
                        .flat_map(|l| &l.blocks)
                        .any(|b| b.block_id.as_ref() == Some(id))
                })
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in stale {
        workers.remove(&id);
        shared.update(|m| {
            m.terminals.remove(&id);
        });
    }
    let mut visible = HashSet::new();
    if model.visible
        && let (Some(session_id), Some(window)) = (&model.selected, model.window())
    {
        for layer in &window.layers {
            for block in &layer.blocks {
                let Some(id) = &block.block_id else {
                    continue;
                };
                if block.creator_name.as_deref() != Some("com.superlogical.terminal") {
                    continue;
                }
                if !crate::layout::block_visible(window, layer, id) {
                    continue;
                }
                visible.insert(id.clone());
                if workers.contains_key(id) {
                    continue;
                }
                let state = Arc::new(Mutex::new(Model {
                    rtt: model.rtt.clone(),
                    status: "Restoring terminal…".into(),
                    session: session_id.clone(),
                    block: id.clone(),
                    owner: block.owner.as_deref() == Some(&model.client_id),
                    control_connected: model.connected,
                    ..Default::default()
                }));
                let (wake, receiver) = flume::bounded(1);
                let connection = Connection {
                    model: state.clone(),
                    commands: TerminalSender::new(sender.clone(), session_id.clone(), id.clone()),
                    wake: receiver,
                };
                shared.update(|m| {
                    m.terminals.insert(id.clone(), connection);
                });
                let pane = PaneShared {
                    model: state,
                    wake,
                    host_wake: shared.wake.clone(),
                };
                let (commands, data_commands) = flume::unbounded();
                let task = tokio::task::spawn_local(run_data(
                    endpoint.to_owned(),
                    session_id.clone(),
                    id.clone(),
                    sender.clone(),
                    data_commands,
                    pane,
                ));
                workers.insert(
                    id.clone(),
                    PaneWorker {
                        session: session_id.clone(),
                        commands,
                        task,
                        size: None,
                    },
                );
            }
        }
    }
    for (id, worker) in workers {
        let _ = worker
            .commands
            .send(DataCommand::Visible(visible.contains(id)));
    }
}

/// A host switch publishes pending titles and updates pane workers without a control RPC.
fn apply_visibility(
    visible: bool,
    endpoint: &str,
    shared: &Shared,
    sender: &flume::Sender<HostCommand>,
    workers: &mut HashMap<String, PaneWorker>,
    state: &mut ControlState,
) {
    shared.update(|m| m.visible = visible);
    ports::reconcile_selected(shared, state);
    state.prune(&shared.model.lock().expect("host model"));
    reconcile(endpoint, shared, sender, workers);
}

fn view_blocks(view: &session::ViewResponse) -> impl Iterator<Item = String> + '_ {
    view.windows
        .iter()
        .flat_map(|w| &w.layers)
        .flat_map(|l| &l.blocks)
        .filter(|b| b.creator_name.as_deref() == Some(activity::TERMINAL))
        .filter_map(|b| b.block_id.clone())
}

fn remove_session(
    shared: &Shared,
    workers: &mut HashMap<String, PaneWorker>,
    state: &mut ControlState,
    sweep: &mut status_sweep::Reader,
    id: &str,
) {
    let blocks: Vec<_> = workers
        .iter()
        .filter(|(_, w)| w.session == id)
        .map(|(id, _)| id.clone())
        .collect();
    for block in &blocks {
        workers.remove(block);
    }
    let mut forgotten = blocks.clone();
    shared.update(|m| {
        m.sessions.retain(|s| s.session_id != id);
        m.background_views.remove(id);
        if let Some(view) = m.views.remove(id) {
            for block in view_blocks(&view) {
                m.forget_block(&block);
                forgotten.push(block);
            }
        }
        for block in &blocks {
            m.terminals.remove(block);
            m.forget_block(block);
        }
        if m.selected.as_deref() == Some(id) {
            m.selected = None;
        }
    });
    sweep.cancel_session(id);
    state.forget_session(id, forgotten);
}

fn apply_bootstrap(
    shared: &Shared,
    state: &mut ControlState,
    reply: rexrs::Reply<session::BootstrapResponse>,
    started: Instant,
) {
    state.bootstrap_seq = reply.seq;
    state.list_seq = reply.seq;
    let boot = reply.value;
    shared.update(|m| {
        m.server_info.bootstrapped(&boot.server_status);
        m.sessions = boot.attach_all.sessions;
    });
    for attached in boot
        .attached_sessions
        .into_iter()
        .chain(boot.selected_session.map(|s| session::AttachedSession {
            session: s.session,
            view: s.view,
        }))
    {
        state
            .view_seq
            .insert(attached.session.session_id.clone(), reply.seq);
        state.attached.insert(attached.session.session_id.clone());
        apply_view(shared, &attached.session.session_id, attached.view);
    }
    state.add_tickets(boot.data_tickets, started);
}

/// Take directed prompt events even if the server has not answered bootstrap yet.
/// Other events still go through the normal post-bootstrap sequence cutoff.
async fn bootstrap(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    boot: session::Bootstrap,
    queued: &mut VecDeque<rexrs::Event>,
) -> Result<rexrs::Reply<session::BootstrapResponse>> {
    tokio::time::timeout(Duration::from_secs(8), async {
        let mut response = control
            .transport()
            .detach(session::Bootstrap::NAME, boot)
            .await?;
        loop {
            tokio::select! {
                biased;
                reply = &mut response => return Ok(transport::detached_result_with_seq(&reply?)?),
                event = control.next_event() => {
                    let event = event?;
                    if prompt::PromptState::is_event(&event) {
                        state.prompt_event(control, shared, event).await?;
                    } else {
                        queued.push_back(event);
                    }
                }
            }
        }
    })
    .await?
}

#[allow(clippy::too_many_arguments)]
async fn handle_event(
    event: rexrs::Event,
    control: &mut Client,
    shared: &Shared,
    workers: &mut HashMap<String, PaneWorker>,
    state: &mut ControlState,
    sweep: &mut status_sweep::Reader,
    keymap: Option<&keymap::Declaration>,
    server_reads: &mut server_info::Reader,
    hosts_reads: &mut hosts::Reader,
) -> Result<()> {
    // Directed requests, not session updates: even an unattached session or a sequence below
    // bootstrap's cutoff needs an answer on this same connection.
    if event.name == "client_perform_requested" {
        return perform::receive(control, shared, event.payload, state.performs_since).await;
    }
    if prompt::PromptState::is_event(&event) {
        return state.prompt_event(control, shared, event).await;
    }
    // Config-wide and server-wide events have no session sequence. Do not broaden includes().
    if event.name == "keymap_changed" {
        let refresh = {
            let m = shared.model.lock().expect("host model");
            m.connected && keymap::changed(m.keymap.as_deref(), &event.payload)
        };
        if refresh && let Some(declaration) = keymap {
            keymap::refresh(control, shared, declaration).await;
        }
        return Ok(());
    }
    if event.name == "hosts_changed" {
        let reread = hosts::changed(
            &shared.model.lock().expect("host model").shared,
            hosts_reads.in_flight_hash(),
            &event.payload,
        );
        if reread {
            hosts_reads
                .reread_for(control, event.payload["hash"].as_str())
                .await?;
        }
        return Ok(());
    }
    if event.name == "tailscale_status_changed" {
        server_reads.reread_status(control).await?;
        return Ok(());
    }
    if event.name == "event_dropped" && event.payload["session_id"].as_str().is_none() {
        match event.payload["event_type"].as_str() {
            Some("keymap_changed") => {
                let connected = shared.model.lock().expect("host model").connected;
                if connected && let Some(declaration) = keymap {
                    keymap::refresh(control, shared, declaration).await;
                }
                return Ok(());
            }
            Some("hosts_changed") => {
                let supported = !matches!(
                    shared.model.lock().expect("host model").shared,
                    hosts::SharedHostState::Unsupported
                );
                if supported {
                    hosts_reads.reread_for(control, None).await?;
                }
                return Ok(());
            }
            Some("tailscale_status_changed") => {
                server_reads.reread_status(control).await?;
                return Ok(());
            }
            _ => {}
        }
    }
    if !state.includes(&event) {
        return Ok(());
    }
    match event.name.as_str() {
        "event_dropped" => {
            let event_type = event.payload["event_type"].as_str().unwrap_or("");
            let block_event_name = event.payload["block_event_name"].as_str();
            let dropped_block = event.payload["block_id"].as_str();
            let session_id = event
                .session_id
                .as_deref()
                .or_else(|| event.payload["session_id"].as_str());
            if event_type == "block_event" && block_event_name == Some("ports_changed") {
                ports::handle_dropped(control, shared, state, session_id, dropped_block, event.seq)
                    .await?;
                return Ok(());
            }
            if event_type.is_empty() || (event_type == "block_event" && block_event_name.is_none())
            {
                ports::handle_dropped(control, shared, state, session_id, dropped_block, event.seq)
                    .await?;
            }
            match event_type {
                "session_destroyed" => {
                    if let Some(id) = session_id {
                        remove_session(shared, workers, state, sweep, id);
                    } else {
                        list(control, shared, state, sweep).await?;
                    }
                }
                "session_created" | "session_moved" | "session_label_changed" => {
                    list(control, shared, state, sweep).await?;
                }
                _ if let Some(id) = session_id => {
                    let dropped_block = event.payload["block_id"].as_str().map(str::to_owned);
                    // Cancel any in-flight background status reads for this session before
                    // awaiting `refresh` so late replies cannot race with recovery.
                    sweep.cancel_session(id);
                    let (should_refresh, mut forget) = {
                        let m = shared.model.lock().expect("host model");
                        let has_view = m.views.contains_key(id);
                        let blocks: Vec<String> = m
                            .views
                            .get(id)
                            .map(|v| view_blocks(v).collect())
                            .unwrap_or_default();
                        (has_view || m.selected.as_deref() == Some(id), blocks)
                    };
                    if let Some(block) = dropped_block {
                        forget.push(block);
                    }
                    if should_refresh {
                        match refresh_for_event(control, shared, state, sweep, id).await {
                            Ok(()) => {
                                let new_blocks: Vec<String> = shared
                                    .model
                                    .lock()
                                    .expect("host model")
                                    .views
                                    .get(id)
                                    .map(|v| view_blocks(v).collect())
                                    .unwrap_or_default();
                                forget.extend(new_blocks);
                            }
                            Err(error)
                                if matches!(
                                    error.downcast_ref::<rexrs::Error>(),
                                    Some(rexrs::Error::Api(
                                        rexrs::ApiError::SessionNotFound { .. }
                                    ))
                                ) =>
                            {
                                remove_session(shared, workers, state, sweep, id);
                                return Ok(());
                            }
                            Err(error) => return Err(error),
                        }
                    } else {
                        state.viewed.remove(id);
                        if state.view_reading.as_deref() == Some(id) {
                            state.view_stale = true;
                        }
                        if state.fetch_reading.contains(id) {
                            state.fetch_stale.insert(id.to_owned());
                        }
                    }
                    shared.update(|m| {
                        for block in &forget {
                            m.status_read.remove(block);
                            // Downgrade live records so a dropped StatusRemoved is not kept
                            // stuck by status_sweep::merge's live-record preservation rule.
                            if let Some(records) = m.activity.get_mut(block) {
                                for record in records.values_mut() {
                                    record.live = false;
                                }
                            }
                        }
                    });
                    for block in &forget {
                        state.queried.remove(block);
                        state.status_queried.remove(block);
                        state.status_failed.remove(block);
                        state.status_reading.remove(block);
                        sweep.cancel_block(block);
                    }
                }
                _ if event_type.starts_with("session_") => {
                    list(control, shared, state, sweep).await?;
                }
                _ => {
                    log::debug!("ignoring unhandled server-wide event_dropped: {event_type:?}");
                }
            }
        }
        "session_created" | "session_moved" => {
            list(control, shared, state, sweep).await?;
        }
        "session_destroyed" => {
            if let Some(id) = event.payload["session_id"]
                .as_str()
                .or(event.session_id.as_deref())
            {
                remove_session(shared, workers, state, sweep, id);
            }
        }
        "session_label_changed" => {
            if let Some(id) = event.session_id.as_deref()
                && let Some(label) = event.payload["label"].as_str()
            {
                shared.update(|m| {
                    if let Some(s) = m.sessions.iter_mut().find(|s| s.session_id == id) {
                        s.label = label.into();
                    }
                });
            }
        }
        "session_view_changed" => {
            if let Some(id) = event.session_id.as_deref() {
                let revision = shared
                    .model
                    .lock()
                    .expect("host model")
                    .views
                    .get(id)
                    .map(|v| v.revision);
                if revision.is_some_and(|revision| {
                    event.payload["revision"]
                        .as_u64()
                        .is_some_and(|new| new > revision)
                }) {
                    refresh_for_event(control, shared, state, sweep, id).await?;
                } else if revision.is_none() {
                    // The reply on its way may predate this change.
                    if state.view_reading.as_deref() == Some(id) {
                        state.view_stale = true;
                    }
                    if state.fetch_reading.contains(id) {
                        state.fetch_stale.insert(id.to_owned());
                    }
                }
            }
        }
        "block_closed" => {
            if let Some(block) = event.payload["block_id"].as_str() {
                if let Some(worker) = workers.remove(block) {
                    let _ = worker.commands.send(DataCommand::Stop);
                }
                sweep.cancel_block(block);
                state.forget_block(block);
                shared.update(|m| {
                    m.terminals.remove(block);
                    m.forget_block(block);
                });
            }
            if let Some(id) = event.session_id.as_deref()
                && shared
                    .model
                    .lock()
                    .expect("host model")
                    .views
                    .contains_key(id)
            {
                refresh_for_event(control, shared, state, sweep, id).await?;
            }
        }
        "block_event" => {
            if clipboard::is_copy(&event.payload) {
                if let (Some(session), Some(block)) = (
                    event.session_id.as_deref(),
                    event.payload["block_id"].as_str(),
                ) {
                    let accepts = clipboard::accepts(
                        &shared.model.lock().expect("host model"),
                        session,
                        block,
                    );
                    if accepts {
                        match clipboard::read(control, session, &event.payload).await {
                            Ok(copy) => shared.update(|m| {
                                if clipboard::accepts(m, &copy.session, &copy.block) {
                                    m.clipboard = Some(copy);
                                }
                            }),
                            Err(_) => shared.update(|m| {
                                m.notice = Some("Copy failed: terminal clipboard text could not be read (limit 8 MiB).".into());
                            }),
                        }
                    }
                }
                return Ok(());
            }
            if ports::is_ports_changed(&event.payload) {
                if control.welcome().api_revision >= crate::ports::PORTS_EVENTS_MIN_API_REVISION
                    && let (Some(session), Some(block)) = (
                        event.session_id.as_deref(),
                        event.payload["block_id"].as_str(),
                    )
                {
                    ports::apply_event(
                        shared,
                        state,
                        session,
                        block,
                        event.seq,
                        &event.payload["payload"],
                    );
                }
                return Ok(());
            }
            let (Some(id), Some((block, change))) = (
                event.session_id.as_deref(),
                activity::parse_block_event(&event.payload),
            ) else {
                return Ok(());
            };
            if let BlockEvent::StatusRemoved { ids } = &change
                && let Some(removed) = state.status_reading.get_mut(&block)
            {
                removed.extend(ids.iter().cloned());
            }
            let unplaced = {
                let m = shared.model.lock().expect("host model");
                !m.views.contains_key(id) && m.sessions.iter().any(|s| s.session_id == id)
            };
            shared.block_event(id, block, change);
            // Read-only, and allowed for any session once attach_all is active. The layout
            // read is left to the paced background reader, so it never holds this event up,
            // but it goes to the front: a live report should not wait for the whole pass.
            if unplaced && !state.viewed.contains(id) && !state.urgent_views.iter().any(|s| s == id)
            {
                state.urgent_views.push(id.to_owned());
            }
        }
        // Becoming the owner, even without a resize, makes the block themable.
        "block_owner_changed" => {
            if let Some(block) = event.payload["block_id"].as_str() {
                shared.update(|m| m.set_owner(block, event.payload["owner"].as_str()));
            }
        }
        _ => {}
    }
    ports::reconcile_selected(shared, state);
    if state.ports_reads.can_pump() {
        state.ports_reads.pump(control, shared).await?;
    }
    Ok(())
}

/// What a host connection starts with.
struct Startup {
    label: String,
    preferred: Option<String>,
    create: Option<(String, Option<String>)>,
    block: Option<String>,
    theme: Option<SetTheme>,
    ssh_auth_sock: Option<PathBuf>,
    prompts_enabled: bool,
    max_api_revision: u32,
    #[cfg(test)]
    ssh_program: Option<PathBuf>,
    #[cfg(test)]
    tunnel_dir: Option<PathBuf>,
}

async fn run_host(
    configured: String,
    startup: Startup,
    commands: flume::Receiver<HostCommand>,
    sender: flume::Sender<HostCommand>,
    shared: &Shared,
) -> Result<()> {
    let Startup {
        mut preferred,
        mut create,
        block: mut initial_block,
        mut theme,
        label,
        mut ssh_auth_sock,
        mut prompts_enabled,
        max_api_revision,
        #[cfg(test)]
        ssh_program,
        #[cfg(test)]
        tunnel_dir,
    } = startup;
    let mut token: Option<String> = None;
    let mut workers = HashMap::<String, PaneWorker>::new();
    let mut retry = 0;
    let mut deferred_commands = VecDeque::new();
    let mut prefetch = prefetch::Prefetch::default();
    let mut keymap: Option<Box<keymap::Declaration>> = None;
    let mut server_reads = server_info::Reader::default();
    let mut hosts_reads = hosts::Reader::default();
    let mut directory_reads = directory::Reader::default();
    let mut view_fetches = view_fetch::Reader::default();
    // An ssh:// host is reached through a forward of its Rex socket; every attempt opens a
    // fresh one, and its end is a disconnect like any other.
    let ssh = crate::ssh::Target::parse(&configured)?;
    let mut tunnel: Option<crate::ssh::Tunnel> = None;
    loop {
        let result: Result<bool> = async {
            let endpoint = match &ssh {
                Some(target) => {
                    shared.status("Connecting over SSH…", false);
                    let opened = {
                        #[cfg(test)]
                        if ssh_program.is_some() || tunnel_dir.is_some() {
                            crate::ssh::Tunnel::open_with(
                                target,
                                ssh_program
                                    .as_deref()
                                    .map_or(std::ffi::OsStr::new("ssh"), std::path::Path::as_os_str),
                                ssh_auth_sock.as_deref(),
                                tunnel_dir.as_deref(),
                            )
                            .await?
                        } else {
                            crate::ssh::Tunnel::open(target, ssh_auth_sock.as_deref()).await?
                        }
                        #[cfg(not(test))]
                        {
                            crate::ssh::Tunnel::open(target, ssh_auth_sock.as_deref()).await?
                        }
                    };
                    let local = opened.endpoint().to_owned();
                    tunnel = Some(opened);
                    local
                }
                None => configured.clone(),
            };
            let mut control = transport::control_with_max_revision(
                &endpoint,
                token.as_deref(),
                &label,
                max_api_revision,
            )
            .await?;
            let identity = control.welcome().client.as_ref().context("Rex client identity")?;
            token = Some(identity.reconnect_token.clone());
            let wanted = preferred.clone().or_else(|| shared.model.lock().expect("host model").selected.clone());
            shared.update(|m| {
                m.connection_epoch += 1;
                m.themes_unsupported = false;
                m.performs.clear();
                m.prompt_events.clear();
                m.actions_in_flight = 0;
                m.view_fetches.clear();
                m.focus_history_result = None;
                m.client_id = identity.id.clone();
                m.protocol.api_revision = control.welcome().api_revision;
                m.server_info = server_info::ServerInfo::welcomed(control.welcome());
                // A server is answering again, so "stop accepted" is no longer the news. A
                // refusal or an unconfirmed stop stays: the person has not seen it resolved.
                if m.server_stop_result.as_deref() == Some(STOP_ACCEPTED) {
                    m.server_stop_result = None;
                }
                m.views.clear();
                m.background_views.clear();
                // Reports made while disconnected were missed; the per-connection pass refills them.
                m.activity.clear();
                m.status_read.clear();
                m.programs.clear();
                m.executables.clear();
                m.scripts.clear();
                m.foreground.clear();
                m.titles.clear();
                m.places.clear();
                m.progress.clear();
                m.ports.clear();
                m.ports_reading.clear();
                m.ports_read_completions = 0;
                m.activity_epoch += 1;
                m.activity_reading = true;
                m.activity_incomplete = false;
            });
            let mut state = ControlState { prompts_enabled, ..Default::default() };
            let mut sweep = status_sweep::Reader::default();
            let mut downloads = downloads::Downloads::default();
            let mut inputs = input::Queue::new(shared.model.lock().expect("host model").rtt.clone());
            let mut busy_since = Some(Instant::now());
            let boot = bootstrap_request(wanted.as_deref(), create.is_some(), workers.values().map(|w| w.session.clone()));
            let started = Instant::now();
            shared.update(|m| m.protocol.bootstrap_requests += 1);
            let mut deferred_events = VecDeque::new();
            let result = bootstrap(&mut control, shared, &mut state, boot, &mut deferred_events).await;
            let reply = match result {
                Ok(reply) => reply,
                Err(error) => {
                    // Setup may be partially applied. A graceful close ends the identity;
                    // never reuse its token or continue this partially initialized socket.
                    token = None;
                    let _ = tokio::time::timeout(Duration::from_secs(2), control.close()).await;
                    return Err(error);
                }
            };
            if inputs.rtt.enabled() { inputs.rtt.record(started.elapsed()); }
            apply_bootstrap(shared, &mut state, reply, started);
            if let Some((label, shell)) = create.take() {
                // Take before calling: an uncertain response must never create twice.
                let layout = crate::layout::shell_layout(shell.as_deref(), new_block_theme(shared, theme.as_ref()));
                let created = request(&mut control, "session.create", json!({"label":label,"initial_windows":[{"layout":layout}]})).await?;
                preferred = created["session_id"].as_str().map(str::to_owned);
                list(&mut control, shared, &mut state, &mut sweep).await?;
            }
            let selected = {
                let model = shared.model.lock().expect("host model");
                let wanted = preferred.clone().or(wanted);
                select_session(&model.sessions, wanted.as_deref())
            };
            if selected.is_some() { preferred = None; }
            shared.update(|m| {
                if let Some(id) = &selected {
                    m.background_views.remove(id);
                }
                m.selected = selected.clone();
            });
            let session_ids: HashSet<_> = shared.model.lock().expect("host model").sessions.iter().map(|s| s.session_id.clone()).collect();
            let stale: HashSet<_> = workers.values().filter(|w| !session_ids.contains(&w.session)).map(|w| w.session.clone()).collect();
            for id in stale { remove_session(shared, &mut workers, &mut state, &mut sweep, &id); }
            let mut attached: HashSet<String> = workers.values().map(|w| w.session.clone()).collect();
            if let Some(id) = &selected { attached.insert(id.clone()); }
            for id in attached {
                if !state.view_seq.contains_key(&id) { attach(&mut control, shared, &mut state, &mut sweep, &id).await?; }
            }
            if let (Some(id), Some(block)) = (&selected, initial_block.take()) {
                request(&mut control, "session.focus_block", json!({"session_id":id,"block_id":block})).await?;
                refresh(&mut control, shared, &mut state, &mut sweep, id).await?;
            }
            state.performs_since = busy_since;
            server_reads.reset();
            hosts_reads.reset();
            deferred_events.extend(control.drain_events());
            while let Some(event) = deferred_events.pop_front() {
                handle_event(
                    event,
                    &mut control,
                    shared,
                    &mut workers,
                    &mut state,
                    &mut sweep,
                    keymap.as_deref(),
                    &mut server_reads,
                    &mut hosts_reads,
                )
                .await?;
            }
            state.prune(&shared.model.lock().expect("host model"));
            shared.update(|m| m.ssh_problem = None);
            shared.status("Connected", true);
            reconcile(&endpoint, shared, &sender, &mut workers);
            for (block, worker) in &workers {
                if let Some((cols, rows, w, h)) = worker.size {
                    resize(&mut control, &worker.session, block, cols, rows, w, h).await?;
                }
            }
            reconcile(&endpoint, shared, &sender, &mut workers);
            declare_themes(&mut control, shared, &mut state, &workers, theme.as_ref(), &mut deferred_events).await?;
            query_activity(&mut control, shared, &mut state, &mut deferred_events).await?;
            // Restore terminal sizes and state before a best-effort keymap can wait.
            if let Some(declaration) = &keymap { keymap::refresh(&mut control, shared, declaration).await; }
            server_reads.start(&mut control).await?;
            hosts_reads.start(&mut control).await?;
            // The last connection's listing can no longer be answered.
            directory_reads.abandon(shared, directory::Failure::Disconnected);
            retry = 0;
            prefetch.reset();
            view_fetches.reset();
            let mut pace = tokio::time::interval(prefetch::PACE);
            pace.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut expiry = tokio::time::interval(Duration::from_millis(100));
            let mut metadata_due = false;
            loop {
                let exclusive = state.actions.exclusive();
                if metadata_due && !exclusive && inputs.in_flight() == 0 {
                    declare_themes(&mut control, shared, &mut state, &workers, theme.as_ref(), &mut deferred_events).await?;
                    query_activity(&mut control, shared, &mut state, &mut deferred_events).await?;
                    metadata_due = false;
                }
                state.performs_since = state.performs_since.or(busy_since.take());
                if !exclusive && (state.performs_since.is_some() || !deferred_events.is_empty()) {
                    // Buffered requests share the busy step's start, even when it took less
                    // than MAX_WAIT. They must not gain another five seconds in the UI queue.
                    use futures_util::FutureExt;
                    if let Some(event) = deferred_events.pop_front().map(Ok)
                        .or_else(|| control.next_event().now_or_never())
                    {
                        let event = event?;
                        if prompt::PromptState::is_event(&event) {
                            state.prompt_event(&mut control, shared, event).await?;
                            continue;
                        }
                        handle_event(
                            event,
                            &mut control,
                            shared,
                            &mut workers,
                            &mut state,
                            &mut sweep,
                            keymap.as_deref(),
                            &mut server_reads,
                            &mut hosts_reads,
                        )
                        .await?;
                        state.prune(&shared.model.lock().expect("host model"));
                        reconcile(&endpoint, shared, &sender, &mut workers);
                        metadata_due = true;
                        continue;
                    }
                    state.performs_since = None;
                }
                if !exclusive && state.ports_reads.can_pump() {
                    state.ports_reads.pump(&mut control, shared).await?;
                }
                let next = (!exclusive).then(|| status_sweep::next(shared, &state)).flatten();
                let out = sweep.in_flight() + usize::from(prefetch.in_flight());
                if !exclusive {
                    status_sweep::coverage(shared, &state, next.is_some() || out > 0);
                }
                // A few reads at once, at most one of them a layout; starts stay paced.
                let room = !exclusive && out < status_sweep::IN_FLIGHT;
                let due = match &next {
                    Some(status_sweep::Work::Status(..)) => room.then(Instant::now),
                    Some(status_sweep::Work::View(_)) => {
                        (room && !prefetch.in_flight()).then(Instant::now)
                    }
                    None if room => {
                        prefetch.due(&shared.model.lock().expect("host model"), Instant::now())
                    }
                    None => None,
                };
                let queued_ready = !exclusive && deferred_commands.front().is_some_and(|c| inputs.admits(c, shared));
                let blocked_front = !queued_ready && !deferred_commands.is_empty();
                tokio::select! {
                    _ = expiry.tick() => {
                        shared.wake.flush_due();
                        state.prompts.expire(shared);
                        if exclusive { continue; }
                        busy_since = Some(Instant::now());
                        perform::expire(&mut control, shared).await?;
                    }
                    result = inputs.finished(shared), if inputs.in_flight() > 0 => {
                        result?;
                    }
                    result = state.actions.finished(shared) => {
                        if let Err(error) = result {
                            shared.update(|m| m.notice = Some(format!("{error:#}")));
                            if !matches!(error.downcast_ref::<rexrs::Error>(), Some(rexrs::Error::Api(_))) { return Err(error); }
                        }
                    }
                    result = state.prompts.finished() => {
                        if matches!(result, Err(rexrs::Error::Transport(_) | rexrs::Error::Closed { .. })) {
                            result?;
                        } else {
                            state.prompts.record(shared, result);
                        }
                    }
                    _ = prefetch::wait(&mut pace, due.unwrap_or_else(Instant::now)), if due.is_some() => {
                        busy_since = Some(Instant::now());
                        match next {
                            Some(status_sweep::Work::Status(session, block)) => {
                                state.status_queried.insert(block.clone());
                                state.status_reading.insert(block.clone(), HashSet::new());
                                sweep.start(&mut control, session, block).await?;
                            }
                            work => {
                                let id = match work {
                                    Some(status_sweep::Work::View(id)) => Some(id),
                                    _ => prefetch.next(&shared.model.lock().expect("host model"), Instant::now()),
                                };
                                if let Some(id) = id {
                                    state.viewed.insert(id.clone());
                                    state.urgent_views.retain(|s| *s != id);
                                    state.view_reading = Some(id.clone());
                                    state.view_stale = false;
                                    prefetch.start(&mut control, id).await?;
                                }
                            }
                        }
                    }
                    (id, text) = prefetch.finished(), if !exclusive && prefetch.in_flight() => {
                        state.view_reading = None;
                        let stale = std::mem::take(&mut state.view_stale);
                        if let Some((seq, view)) = prefetch.result(&id, text, Instant::now()) {
                            let placed = {
                                let m = shared.model.lock().expect("host model");
                                m.views.contains_key(&id) || !m.sessions.iter().any(|s| s.session_id == id)
                            };
                            if !placed {
                                state.view_seq.insert(id.clone(), seq);
                                apply_view(shared, &id, view);
                                shared.update(|m| {
                                    if m.selected.as_deref() != Some(id.as_str()) {
                                        m.background_views.insert(id.clone());
                                    }
                                });
                                // Its layout changed while this read was out, and that
                                // event had no layout to compare with: read it once more.
                                if stale {
                                    let _ = refresh_for_event(&mut control, shared, &mut state, &mut sweep, &id).await;
                                }
                            }
                        }
                    }
                    (session, block, reply) = sweep.finished(), if !exclusive && sweep.in_flight() > 0 => {
                        let removed = state.status_reading.remove(&block).unwrap_or_default();
                        status_sweep::record(shared, &mut state, &session, &block, reply, &removed);
                    }
                    finished = state.ports_reads.finished(), if !exclusive && state.ports_reads.in_flight() > 0 => {
                        ports::record_reply(shared, &mut state, finished);
                        state.ports_reads.pump(&mut control, shared).await?;
                    }
                    reply = server_reads.finished(), if !exclusive && server_reads.in_flight() => server_info::record(shared, reply),
                    reply = hosts_reads.finished(), if !exclusive && hosts_reads.in_flight() => hosts::record(shared, reply),
                    _ = directory_reads.finished(shared), if !exclusive && directory_reads.in_flight() => {}
                    fetched = view_fetches.finished(), if !exclusive && view_fetches.in_flight() => {
                        match fetched {
                            view_fetch::Event::Reply { session, ids, result: Ok((seq, view)) } => {
                                if !view_fetches.reading(&session) {
                                    state.fetch_reading.remove(&session);
                                }
                                let stale = state.fetch_stale.remove(&session);
                                let mut placed = false;
                                shared.update(|m| {
                                    if !m.sessions.iter().any(|s| s.session_id == session) {
                                        record_view_fetches(
                                            m,
                                            ids.iter().map(|&id| (id, ViewFetchOutcome::NotFound)),
                                        );
                                        return;
                                    }
                                    if !m.views.contains_key(&session) {
                                        apply_view_to_model(m, &session, view);
                                        if m.selected.as_deref() != Some(session.as_str()) {
                                            m.background_views.insert(session.clone());
                                        }
                                        placed = true;
                                    }
                                    record_view_fetches(
                                        m,
                                        ids.iter().map(|&id| (id, ViewFetchOutcome::Loaded)),
                                    );
                                });
                                if placed {
                                    state.view_seq.insert(session.clone(), seq);
                                    if stale {
                                        let _ = refresh_for_event(&mut control, shared, &mut state, &mut sweep, &session).await;
                                    }
                                }
                            }
                            view_fetch::Event::Reply { session, ids, result: Err(outcome) } => {
                                if !view_fetches.reading(&session) {
                                    state.fetch_reading.remove(&session);
                                    state.fetch_stale.remove(&session);
                                }
                                shared.update(|m| {
                                    record_view_fetches(m, ids.into_iter().map(|id| (id, outcome)));
                                });
                            }
                            view_fetch::Event::Expired { session, ids, finished } => {
                                if finished && !view_fetches.reading(&session) {
                                    state.fetch_reading.remove(&session);
                                    state.fetch_stale.remove(&session);
                                }
                                shared.update(|m| {
                                    record_view_fetches(
                                        m,
                                        ids.into_iter().map(|id| (id, ViewFetchOutcome::Failed)),
                                    );
                                });
                            }
                        }
                    }
                    event = control.next_event() => {
                        let event = event?;
                        if prompt::PromptState::is_event(&event) {
                            state.prompt_event(&mut control, shared, event).await?;
                            continue;
                        }
                        if exclusive {
                            deferred_events.push_back(event);
                            continue;
                        }
                        busy_since = Some(Instant::now());
                        handle_event(
                            event,
                            &mut control,
                            shared,
                            &mut workers,
                            &mut state,
                            &mut sweep,
                            keymap.as_deref(),
                            &mut server_reads,
                            &mut hosts_reads,
                        )
                        .await?;
                        state.prune(&shared.model.lock().expect("host model"));
                        reconcile(&endpoint, shared, &sender, &mut workers);
                        metadata_due = true;
                    }
                    error = crate::ssh::closed(&mut tunnel) => return Err(error),
                    command = async {
                        if queued_ready {
                            Ok(deferred_commands.pop_front().expect("ready front"))
                        } else {
                            commands.recv_async().await
                        }
                    } => {
                        let Ok(mut command) = command else { control.close().await?; return Ok(true); };
                        if matches!(command, HostCommand::Shutdown) { control.close().await?; return Ok(true); }
                        if matches!(command, HostCommand::Reconnect) { return Ok(false); }
                        if let HostCommand::Prompts(enabled) = command {
                            prompts_enabled = enabled;
                            state.prompts_enabled = enabled;
                            if !enabled { state.prompts.disable(&mut control, shared).await?; }
                            continue;
                        }
                        if let HostCommand::PromptReply { epoch, call_id, answer } = command {
                            state.prompts.answer(&mut control, shared, epoch, call_id, answer).await?;
                            continue;
                        }
                        if let HostCommand::Visibility(visible) = command {
                            if exclusive {
                                deferred_commands.push_back(HostCommand::Visibility(visible));
                            } else {
                                apply_visibility(
                                    visible,
                                    &endpoint,
                                    shared,
                                    &sender,
                                    &mut workers,
                                    &mut state,
                                );
                                metadata_due = true;
                            }
                            continue;
                        }
                        if exclusive || blocked_front || !inputs.admits(&command, shared) {
                            deferred_commands.push_back(command);
                            continue;
                        }
                        let command_started = Instant::now();
                        if !matches!(command, HostCommand::Terminal { .. }) {
                            busy_since = Some(command_started);
                        }
                        if let HostCommand::Action { generation, request } = command {
                            state.prompts.expire(shared);
                            let prompt_deadline = state.prompts.deadline();
                            state.actions.start(&mut control, shared, generation, *request, prompt_deadline).await?;
                            continue;
                        }
                        if let HostCommand::SshAgent { socket, .. } = command {
                            ssh_auth_sock = socket;
                            continue;
                        }
                        if let HostCommand::StopServer { epoch } = command {
                            if epoch != shared.model.lock().expect("host model").connection_epoch {
                                shared.update(|m| m.notice = Some("Stop not sent: the connection changed. Review Stop server again.".into()));
                                continue;
                            }
                            match request(&mut control, "server.stop", json!({})).await {
                                Ok(_) => {
                                    shared.update(|m| {
                                        m.notice = Some(STOP_ACCEPTED.into());
                                        m.server_stop_result = m.notice.clone();
                                    });
                                    // The API replies before shutdown and asks the caller to close.
                                    let _ = control.close().await;
                                    return Ok(false);
                                }
                                Err(error) => {
                                    let api = error.downcast_ref::<rexrs::Error>().and_then(|e| e.api());
                                    let message = api.map(|e| crate::download::display_text(&e.to_string()));
                                    shared.update(|m| {
                                        m.notice = Some(message.map_or_else(
                                            || "Could not confirm server stop: the connection dropped or did not reply. Check the host before trying again. Nothing was retried.".into(),
                                            |message| format!("Could not stop server: {message}. Nothing was retried.")));
                                        m.server_stop_result = m.notice.clone();
                                    });
                                    if api.is_none() { return Err(error); }
                                }
                            }
                            continue;
                        }
                        if let HostCommand::ResizeLayer {
                            epoch,
                            session,
                            layer,
                            bounds,
                            raise_first,
                        } = command
                        {
                            let live_epoch = shared.model.lock().expect("host model").connection_epoch;
                            if epoch != live_epoch {
                                shared.update(|m| {
                                    m.notice = Some(crate::layers::Blocked::ChangedDuringDrag.sentence());
                                });
                                continue;
                            }
                            let rev = control.welcome().api_revision;
                            if rev < crate::layers::MIN_API_REVISION {
                                shared.update(|m| {
                                    m.notice = Some(crate::layers::Blocked::OldServer(rev).sentence());
                                });
                                continue;
                            }
                            let Some(bounds) = crate::layers::sendable(bounds) else {
                                continue;
                            };
                            let outcome: Result<()> = async {
                                if raise_first {
                                    request(
                                        &mut control,
                                        "session.raise_layer",
                                        json!({"session_id": session, "layer_id": layer}),
                                    )
                                    .await?;
                                }
                                request(
                                    &mut control,
                                    "session.resize_layer",
                                    json!({"session_id": session, "layer_id": layer, "bounds": bounds}),
                                )
                                .await?;
                                after_request(
                                    &mut control,
                                    shared,
                                    &mut workers,
                                    &mut state,
                                    &mut sweep,
                                    Some(&session),
                                )
                                .await
                            }
                            .await;
                            if let Err(error) = outcome {
                                shared.update(|m| m.notice = Some(format!("{error:#}")));
                                if !matches!(error.downcast_ref::<rexrs::Error>(), Some(rexrs::Error::Api(_))) {
                                    return Err(error);
                                }
                            }
                            state.prune(&shared.model.lock().expect("host model"));
                            reconcile(&endpoint, shared, &sender, &mut workers);
                            metadata_due = true;
                            continue;
                        }
                        if let HostCommand::Keymap(new) = command {
                            keymap::refresh(&mut control, shared, &new).await;
                            keymap = Some(new);
                            continue;
                        }
                        if let HostCommand::PrefetchViews(sessions) = command { prefetch.set_eligible(sessions); continue; }
                        if let HostCommand::FetchView { id, epoch, deadline, session } = command {
                            let (live_epoch, already_loaded, listed) = {
                                let m = shared.model.lock().expect("host model");
                                (
                                    m.connection_epoch,
                                    m.views.contains_key(&session),
                                    m.sessions.iter().any(|s| s.session_id == session),
                                )
                            };
                            if epoch != live_epoch {
                                continue;
                            }
                            if already_loaded {
                                shared.update(|m| record_view_fetches(m, [(id, ViewFetchOutcome::Loaded)]));
                                continue;
                            }
                            if !listed {
                                shared.update(|m| record_view_fetches(m, [(id, ViewFetchOutcome::NotFound)]));
                                continue;
                            }
                            if Instant::now() >= deadline {
                                shared.update(|m| record_view_fetches(m, [(id, ViewFetchOutcome::Failed)]));
                                continue;
                            }
                            if view_fetches.start(&mut control, id, deadline, session.clone()).await? {
                                state.fetch_reading.insert(session);
                            } else {
                                shared.update(|m| record_view_fetches(m, [(id, ViewFetchOutcome::Failed)]));
                            }
                            continue;
                        }
                        if let HostCommand::Identify = command {
                            server_reads.want();
                            server_reads.start(&mut control).await?;
                            continue;
                        }
                        if let HostCommand::WatchSharedHosts = command {
                            let retryable = shared
                                .model
                                .lock()
                                .expect("host model")
                                .shared
                                .retryable();
                            hosts_reads.watch(&mut control, retryable).await?;
                            continue;
                        }
                        if let HostCommand::WatchPorts { epoch, session } = command {
                            if epoch == shared.model.lock().expect("host model").connection_epoch {
                                ports::watch(&mut control, shared, &mut state, session).await?;
                            }
                            continue;
                        }
                        if let HostCommand::ReadPorts { epoch, session } = command {
                            if epoch == shared.model.lock().expect("host model").connection_epoch {
                                ports::read_once(&mut control, shared, &mut state, &session).await?;
                            }
                            continue;
                        }
                        if let HostCommand::PutSharedHost { id, epoch, host } = command {
                            hosts::execute_put(&mut control, shared, &mut hosts_reads, id, epoch, host).await?;
                            continue;
                        }
                        if let HostCommand::RemoveSharedHost { id, epoch, host_id } = command {
                            hosts::execute_remove(&mut control, shared, &mut hosts_reads, id, epoch, host_id).await?;
                            continue;
                        }
                        if let HostCommand::Upload { session, block, files } = command {
                            if let Err(error) = upload(&mut control, shared, &endpoint, &session, &block, files).await {
                                shared.update(|m| m.notice = Some(format!("Upload failed: {error:#}")));
                                if !matches!(error.downcast_ref::<rexrs::Error>(), Some(rexrs::Error::Api(_)) | None) { return Err(error); }
                            }
                            continue;
                        }
                        if let HostCommand::ListDir(request) = command {
                            directory_reads.start(&mut control, shared, *request).await?;
                            continue;
                        }
                        if let HostCommand::Download(request) = command {
                            downloads.start(&mut control, shared, &endpoint, *request, crate::download::new_ref).await?;
                            continue;
                        }
                        if let HostCommand::DownloadConfirm(id) = command { downloads.signal(id, downloads::Signal::Confirm); continue; }
                        if let HostCommand::DownloadCancel(id) = command { downloads.signal(id, downloads::Signal::Cancel); continue; }
                        if let HostCommand::Navigate { id, deadline, .. } = &command
                            && Instant::now() >= *deadline
                        {
                            shared.update(|m| {
                                m.navigation_result = Some((*id, false));
                                m.notice = Some("Navigation expired before it could be sent. Try again.".into());
                            });
                            continue;
                        }
                        if let HostCommand::ReverseFocusHistory { epoch, id, deadline, .. } = &command
                            && Instant::now() >= *deadline
                        {
                            focus_history::expire(shared, *epoch, *id);
                            continue;
                        }
                        if matches!(
                            command,
                            HostCommand::Select(_)
                                | HostCommand::Navigate { .. }
                                | HostCommand::CreateSession { .. }
                                | HostCommand::ReverseFocusHistory { .. }
                        ) {
                            preferred = None;
                        }
                        if let HostCommand::Terminal { session, block, command } = &mut command {
                            coalesce_terminal_command(
                                session,
                                block,
                                command,
                                &mut deferred_commands,
                                &commands,
                            );
                        }
                        let navigation = match &command { HostCommand::Navigate { id, .. } => Some(*id), _ => None };
                        let outcome = handle_command(
                            command,
                            &mut control,
                            shared,
                            &mut workers,
                            &mut state,
                            &mut sweep,
                            &mut inputs,
                            &mut theme,
                        )
                        .await;
                        if let Some(id) = navigation {
                            shared.update(|m| m.navigation_result = Some((id, outcome.is_ok())));
                        }
                        let effect = match outcome {
                            Ok(effect) => effect,
                            Err(error) => {
                                shared.update(|m| m.notice = Some(format!("{error:#}")));
                                if !matches!(error.downcast_ref::<rexrs::Error>(), Some(rexrs::Error::Api(_))) { return Err(error); }
                                CommandEffect::Full
                            }
                        };
                        if effect != CommandEffect::None
                            || command_started.elapsed() >= Duration::from_millis(5)
                        {
                            busy_since = Some(command_started);
                        }
                        match effect {
                            CommandEffect::None => {}
                            CommandEffect::Claimed => {
                                metadata_due = true;
                            }
                            CommandEffect::Full => {
                                ports::reconcile_selected(shared, &mut state);
                                state.prune(&shared.model.lock().expect("host model"));
                                reconcile(&endpoint, shared, &sender, &mut workers);
                                metadata_due = true;
                            }
                        }
                    }
                }
            }
        }.await;
        tunnel = None;
        if matches!(result, Ok(true)) {
            deferred_commands.clear();
            shared.update(|m| {
                m.prompt_events.clear();
                m.actions_in_flight = 0;
            });
            return Ok(());
        }
        shared.update(|m| {
            m.performs.clear();
            m.view_fetches.clear();
            m.ssh_problem = result
                .as_ref()
                .err()
                .and_then(crate::ssh::Problem::from_error);
        });
        shared.status(
            match result {
                Err(e) => format!("Disconnected · {e:#}"),
                _ => "Reconnecting…".into(),
            },
            false,
        );
        let mut retry_now = false;
        let mut delay = None;
        loop {
            // Settle the older, deferred work before reading newer commands or starting
            // the retry timer. Drain the channel again at the end so nothing is replayed.
            let (command, in_backoff) = if let Some(command) = deferred_commands.pop_front() {
                (command, false)
            } else if retry_now {
                let Ok(command) = commands.try_recv() else {
                    break;
                };
                (command, false)
            } else {
                let delay = delay.get_or_insert_with(|| {
                    retry += 1;
                    Box::pin(tokio::time::sleep(Duration::from_secs(
                        (1_u64 << retry.min(3)).min(8),
                    )))
                });
                tokio::select! {
                    _ = delay => {
                        retry_now = true;
                        continue;
                    }
                    command = commands.recv_async() => {
                        let Ok(command) = command else { return Ok(()); };
                        (command, true)
                    }
                }
            };
            match command {
                HostCommand::Shutdown => return Ok(()),
                HostCommand::Reconnect => retry_now = true,
                HostCommand::Select(id) => preferred = Some(id),
                HostCommand::Navigate { id, .. } => reject_navigation(shared, id),
                HostCommand::Visibility(visible) => shared.update(|m| m.visible = visible),
                HostCommand::PrefetchViews(sessions) => prefetch.set_eligible(sessions),
                HostCommand::Theme(new) => theme = new,
                HostCommand::SshAgent { socket, wake_auth_retry } => {
                    ssh_auth_sock = socket;
                    if in_backoff
                        && wake_auth_retry
                        && ssh.is_some()
                        && shared.model.lock().expect("host model").ssh_problem
                            == Some(crate::ssh::Problem::AuthFailed)
                    {
                        retry = 0;
                        retry_now = true;
                    }
                }
                HostCommand::Keymap(new) => keymap = Some(new),
                HostCommand::Prompts(enabled) => prompts_enabled = enabled,
                HostCommand::Identify => server_reads.want(),
                HostCommand::WatchSharedHosts => hosts_reads.want(),
                HostCommand::PutSharedHost { id, epoch, .. } => hosts::reject_write(
                    shared,
                    id,
                    epoch,
                    hosts::WriteKind::Put,
                    "disconnected",
                    if in_backoff {
                        "Shared host change not sent while disconnected. Try again after reconnecting."
                    } else {
                        "Shared host change not sent: the connection dropped first. Try again after reconnecting."
                    },
                ),
                HostCommand::RemoveSharedHost { id, epoch, .. } => hosts::reject_write(
                    shared,
                    id,
                    epoch,
                    hosts::WriteKind::Remove,
                    "disconnected",
                    if in_backoff {
                        "Shared host removal not sent while disconnected. Try again after reconnecting."
                    } else {
                        "Shared host removal not sent: the connection dropped first. Try again after reconnecting."
                    },
                ),
                HostCommand::Ticket { reply, .. } => {
                    let _ = reply.send(Err("Host is reconnecting".into()));
                }
                HostCommand::Terminal { block, command: Command::Selection(input), .. } if in_backoff => {
                    if let Some(worker) = workers.get(&block) {
                        let _ = worker.commands.send(DataCommand::Selection(input));
                    }
                }
                HostCommand::Terminal { block, command, .. } if in_backoff && matches!(command.untracked(), Command::Resize(..)) => {
                    if let (Some(worker),Command::Resize(a,b,c,d)) = (workers.get_mut(&block),command.untracked()) {
                        worker.size = Some((*a,*b,*c,*d));
                    }
                }
                HostCommand::Terminal { block, command, .. }
                    if forward_search(&workers, &block, &command) => {}
                HostCommand::ListDir(request) => {
                    directory::finish(shared, &request, Err(directory::Failure::Disconnected))
                }
                HostCommand::Download(request) => {
                    downloads::refuse(shared, *request, crate::download::Failure::Disconnected)
                }
                // Confirmed, but the connection failed before it was read: say so, since
                // the dialog has already closed.
                HostCommand::StopServer { .. } if !in_backoff => shared.update(|m| {
                    m.notice = Some(
                        "Stop not sent: the connection dropped first. Check the host before trying again. Nothing was retried."
                            .into(),
                    );
                    m.server_stop_result = m.notice.clone();
                }),
                HostCommand::ResizeLayer { .. } => shared.update(|m| {
                    m.notice = Some(crate::layers::Blocked::NotConnected.sentence());
                }),
                HostCommand::UndoClose { .. } => shared.update(|m| {
                    m.notice = Some(UNDO_CLOSE_DISCONNECTED.into());
                }),
                HostCommand::ReverseFocusHistory { epoch, .. } => {
                    if in_backoff
                        && epoch == shared.model.lock().expect("host model").connection_epoch
                    {
                        shared.update(|m| {
                            m.notice = Some(FOCUS_HISTORY_DISCONNECTED_NOTICE.into());
                        });
                    }
                }
                HostCommand::Claim { .. }
                | HostCommand::PerformReply { .. }
                | HostCommand::PromptReply { .. }
                | HostCommand::FetchView { .. }
                | HostCommand::WatchPorts { .. }
                | HostCommand::ReadPorts { .. }
                | HostCommand::DownloadConfirm(_)
                | HostCommand::DownloadCancel(_) => {}
                _ if in_backoff => shared.update(|m| {
                    m.notice = Some("Action not sent while disconnected. Try it again after reconnecting.".into());
                }),
                _ => {}
            }
        }
    }
}

fn reject_navigation(shared: &Shared, id: u64) {
    shared.update(|m| {
        m.navigation_result = Some((id, false));
        m.notice =
            Some("Navigation not sent while disconnected. Try again after reconnecting.".into());
    });
}

/// Search and prompt navigation run on the pane's own terminal, so they need no server.
/// Cursor defaults are local drawing policy too. All reach the pane's worker even while
/// the host reconnects.
/// False for any other command.
fn forward_search(workers: &HashMap<String, PaneWorker>, block: &str, command: &Command) -> bool {
    let command = match command {
        Command::Search(needle) => DataCommand::Search(needle.clone()),
        Command::SearchStep(older) => DataCommand::SearchStep(*older),
        Command::PromptStep(older) => DataCommand::PromptStep(*older),
        Command::CursorStyle(shape) => DataCommand::CursorStyle(*shape),
        Command::CopyMode(input) => DataCommand::CopyMode(*input),
        _ => return false,
    };
    if let Some(worker) = workers.get(block) {
        let _ = worker.commands.send(command);
    }
    true
}

/// Reserves every file of one drop or paste as a batch, then streams them off the control
/// loop. Nothing is retried: a failure leaves nothing typed and nothing on the server.
async fn upload(
    control: &mut Client,
    shared: &Shared,
    endpoint: &str,
    session: &str,
    block: &str,
    files: Vec<crate::upload::File>,
) -> Result<()> {
    use crate::upload;
    if files.is_empty() {
        return Ok(());
    }
    anyhow::ensure!(
        control.welcome().api_revision >= upload::MIN_API_REVISION,
        "this host's Rex server is too old to receive files; update Rex there"
    );
    let sizes = files
        .iter()
        .map(upload::File::size)
        .collect::<Result<Vec<_>>>()?;
    let batch = if files.len() > 1 {
        Some(upload::new_uuid()?)
    } else {
        None
    };
    let mut tickets = Vec::with_capacity(files.len());
    for (index, (file, size)) in files.iter().zip(&sizes).enumerate() {
        let mut begin = rexrs::api::terminal::UploadBegin::new(
            format!("upload:{}", upload::new_uuid()?),
            file.name.clone(),
            i64::try_from(*size).context("file too large")?,
        );
        if let Some(batch) = &batch {
            begin = begin
                .batch(batch.clone())
                .index(index as i64)
                .count(files.len() as i64);
        }
        let reply = tokio::time::timeout(
            Duration::from_secs(8),
            control.call(begin.on(session.to_owned(), block.to_owned())),
        )
        .await
        .context("Rex request timed out; upload was not retried")??;
        tickets.push(reply.ticket);
    }
    let shared = shared.clone();
    let endpoint = endpoint.to_owned();
    tokio::task::spawn_local(async move {
        let sends = files
            .iter()
            .zip(&tickets)
            .zip(&sizes)
            .map(|((file, ticket), size)| upload::send(&endpoint, ticket, file, *size));
        let failed = futures_util::future::join_all(sends)
            .await
            .into_iter()
            .find_map(Result::err);
        if let Some(error) = failed {
            shared.update(|m| m.notice = Some(format!("Upload failed: {error:#}")));
        }
    });
    Ok(())
}

/// Clear a successful request's notice and reconcile its session, which may have ended.
async fn after_request(
    control: &mut Client,
    shared: &Shared,
    workers: &mut HashMap<String, PaneWorker>,
    state: &mut ControlState,
    sweep: &mut status_sweep::Reader,
    session: Option<&str>,
) -> Result<()> {
    shared.update(|m| m.notice = None);
    if let Some(id) = session {
        // Destroying a session, or closing its last tab, ends it: nothing to re-read.
        match refresh_for_event(control, shared, state, sweep, id).await {
            Err(error)
                if matches!(
                    error.downcast_ref::<rexrs::Error>(),
                    Some(rexrs::Error::Api(rexrs::ApiError::SessionNotFound { .. }))
                ) =>
            {
                remove_session(shared, workers, state, sweep, id)
            }
            result => result?,
        }
    }
    Ok(())
}

async fn undo_close(
    control: &mut Client,
    shared: &Shared,
    state: &mut ControlState,
    sweep: &mut status_sweep::Reader,
    epoch: u64,
) -> Result<()> {
    if shared.model.lock().expect("host model").connection_epoch != epoch {
        return Ok(());
    }
    if control.welcome().api_revision < UNDO_CLOSE_MIN_API_REVISION {
        shared.update(|m| m.notice = Some(UNDO_CLOSE_REQUIRES_API.into()));
        return Ok(());
    }
    let reopened = async {
        let reply = request(control, "client.undo_close", json!({})).await;
        let reply = match reply {
            Err(error)
                if matches!(
                    error.downcast_ref::<rexrs::Error>(),
                    Some(rexrs::Error::Api(rexrs::ApiError::NotFound { .. }))
                ) =>
            {
                shared.update(|m| m.notice = Some("Nothing to reopen".into()));
                return Ok(());
            }
            result => result?,
        };
        let id = reply["session_id"]
            .as_str()
            .context("missing reopened session ID")?;
        let listed = shared
            .model
            .lock()
            .expect("host model")
            .sessions
            .iter()
            .any(|s| s.session_id == id);
        if !listed {
            list(control, shared, state, sweep).await?;
        }
        let selected = shared.model.lock().expect("host model").selected.as_deref() == Some(id);
        if selected {
            refresh(control, shared, state, sweep, id).await?;
        } else {
            attach(control, shared, state, sweep, id).await?;
        }
        shared.update(|m| {
            m.background_views.remove(id);
            m.selected = Some(id.into());
            m.notice = None;
        });
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(error) = reopened {
        shared.update(|m| m.notice = Some(format!("Could not finish Undo close: {error:#}")));
        if matches!(
            error.downcast_ref::<rexrs::Error>(),
            Some(rexrs::Error::Transport(_) | rexrs::Error::Closed { .. })
        ) {
            return Err(error);
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CommandEffect {
    /// Local pane forwarding or terminal input/resize: no layout or metadata changed.
    None,
    /// Input took size ownership of a block, which may make it newly eligible for `set_theme`.
    Claimed,
    /// Command may have changed sessions, views, workers, themes, or unqueried blocks.
    Full,
}

fn is_mouse_motion(payload: &Value) -> bool {
    payload["action"].as_str() == Some("motion")
}

/// Collapse consecutive queued `Resize` or identical-button/mods `Mouse(motion)` events for the
/// same pane so a slow link never replays stale intermediate drag/resize states. Never reorders
/// across any other command.
fn coalesce_terminal_command(
    session: &str,
    block: &str,
    command: &mut Command,
    deferred: &mut VecDeque<HostCommand>,
    incoming: &flume::Receiver<HostCommand>,
) {
    let coalescable = matches!(command.untracked(), Command::Resize(..))
        || matches!(command, Command::Mouse(payload) if is_mouse_motion(payload));
    if !coalescable {
        return;
    }
    loop {
        if deferred.is_empty() {
            let Ok(next) = incoming.try_recv() else {
                break;
            };
            deferred.push_back(next);
        }
        let Some(HostCommand::Terminal {
            session: next_session,
            block: next_block,
            command: next,
        }) = deferred.front()
        else {
            break;
        };
        if next_session != session || next_block != block {
            break;
        }
        let matches_next = match (command.untracked(), next.untracked()) {
            // Keep the last receipt when collapsing tracked resizes. Do not drop a receipt
            // in favour of an untracked resize after the preference was switched off.
            (Command::Resize(..), Command::Resize(..)) => {
                !matches!(command, Command::EchoReset(_)) || matches!(next, Command::EchoReset(_))
            }
            (Command::Mouse(current), Command::Mouse(next_payload)) => {
                is_mouse_motion(next_payload)
                    && next_payload.get("button") == current.get("button")
                    && next_payload.get("mods") == current.get("mods")
            }
            _ => false,
        };
        if !matches_next {
            break;
        }
        if let Some(HostCommand::Terminal { command: newer, .. }) = deferred.pop_front() {
            *command = newer;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_command(
    command: HostCommand,
    control: &mut Client,
    shared: &Shared,
    workers: &mut HashMap<String, PaneWorker>,
    state: &mut ControlState,
    sweep: &mut status_sweep::Reader,
    inputs: &mut input::Queue,
    theme: &mut Option<SetTheme>,
) -> Result<CommandEffect> {
    match command {
        HostCommand::PerformReply {
            epoch,
            call_id,
            outcome,
        } => {
            if shared.model.lock().expect("host model").connection_epoch == epoch {
                perform::reply(control, &call_id, &outcome).await?;
            }
        }
        // Declared to every block it is due for right after this command.
        HostCommand::Theme(new) => *theme = new,
        HostCommand::Claim { session, blocks } => {
            if blocks.is_empty() {
                return Ok(CommandEffect::Full);
            }
            let Ok(result) = request(
                control,
                "session.own_block",
                json!({"session_id":session,"block_ids":blocks,"mode":"take"}),
            )
            .await
            else {
                return Ok(CommandEffect::Full);
            };
            shared.update(|m| {
                for b in result["blocks"].as_array().into_iter().flatten() {
                    if let Some(id) = b["block_id"].as_str() {
                        m.set_owner(id, b["owner"].as_str());
                    }
                }
            });
        }
        HostCommand::Navigate {
            session, window, ..
        } => {
            let selected = shared.model.lock().expect("host model").selected.clone();
            let api19 = control.welcome().api_revision >= FOCUS_HISTORY_MIN_API_REVISION;
            if selected.as_ref() != Some(&session) {
                if api19 && state.attached.contains(&session) {
                    if window.is_none() {
                        refresh(control, shared, state, sweep, &session).await?;
                    }
                } else {
                    attach(control, shared, state, sweep, &session).await?;
                    if api19 && window.is_none() {
                        refresh(control, shared, state, sweep, &session).await?;
                    }
                }
            }
            if let Some(window) = window {
                request(
                    control,
                    "session.focus_window",
                    json!({"session_id":session,"window_id":window}),
                )
                .await?;
                refresh(control, shared, state, sweep, &session).await?;
            }
            shared.update(|m| {
                m.background_views.remove(&session);
                m.selected = Some(session);
                m.notice = None;
            });
        }
        HostCommand::Select(id) => {
            let api19 = control.welcome().api_revision >= FOCUS_HISTORY_MIN_API_REVISION;
            if api19 && state.attached.contains(&id) {
                refresh(control, shared, state, sweep, &id).await?;
            } else {
                attach(control, shared, state, sweep, &id).await?;
                if api19 {
                    refresh(control, shared, state, sweep, &id).await?;
                }
            }
            shared.update(|m| {
                m.background_views.remove(&id);
                m.selected = Some(id);
                m.notice = None;
            });
        }
        HostCommand::RefreshSessions => {
            list(control, shared, state, sweep).await?;
            // A selected session that has gone is forgotten by the list, not re-read.
            let selected = shared.model.lock().expect("host model").selected.clone();
            after_request(control, shared, workers, state, sweep, selected.as_deref()).await?;
        }
        HostCommand::CreateSession { label, shell } => {
            let layout = crate::layout::shell_layout(
                shell.as_deref(),
                new_block_theme(shared, theme.as_ref()),
            );
            let mut payload =
                json!({"initial_windows":[{"window_label":DEFAULT_TAB_LABEL,"layout":layout}]});
            if let Some(label) = label.filter(|l| !l.trim().is_empty()) {
                payload["label"] = json!(label);
            }
            let created = request(control, "session.create", payload).await?;
            let id = created["session_id"].as_str().context("new session ID")?;
            list(control, shared, state, sweep).await?;
            attach(control, shared, state, sweep, id).await?;
            shared.update(|m| {
                m.background_views.remove(id);
                m.selected = Some(id.into());
                m.notice = None;
            });
        }
        HostCommand::UndoClose { epoch } => {
            undo_close(control, shared, state, sweep, epoch).await?
        }
        HostCommand::ReverseFocusHistory {
            epoch,
            id,
            scope,
            cursor,
            deadline,
        } => {
            focus_history::reverse(
                control, shared, state, sweep, epoch, id, scope, cursor, deadline,
            )
            .await?
        }
        HostCommand::Close { .. } | HostCommand::Request { .. } => {
            let (method, payload) = match command {
                HostCommand::Close { kind, mut payload } => {
                    if control.welcome().api_revision >= UNDO_CLOSE_MIN_API_REVISION {
                        payload["undoable"] = json!(true);
                    }
                    (std::borrow::Cow::Borrowed(kind.method()), payload)
                }
                HostCommand::Request { method, payload } => {
                    (std::borrow::Cow::Owned(method), payload)
                }
                _ => unreachable!(),
            };
            let session_id = payload["session_id"].as_str().map(str::to_owned);
            request(control, &method, payload).await?;
            let after_session = if method == "session.detach" {
                if let Some(detached) = session_id.as_deref() {
                    state.attached.remove(detached);
                }
                None
            } else {
                session_id.as_deref()
            };
            after_request(control, shared, workers, state, sweep, after_session).await?;
        }
        HostCommand::NewTerminal {
            method,
            mut payload,
            cwd_from,
        } => {
            let session = payload["session_id"]
                .as_str()
                .context("new terminal session")?
                .to_owned();
            if let Some(dir) = shell_dir_of(control, &session, &cwd_from).await {
                crate::layout::with_cwd(&mut payload["layout"], &dir);
            }
            // Creation is sent once. A failure follows the ordinary request path; never replay.
            request(control, &method, payload).await?;
            after_request(control, shared, workers, state, sweep, Some(&session)).await?;
        }
        HostCommand::Ticket {
            session,
            block,
            reply,
        } => {
            state.prune(&shared.model.lock().expect("host model"));
            if let Some(ticket) = state.take_ticket(&session, &block) {
                if reply.send(Ok(ticket)).is_ok() {
                    shared.update(|m| m.protocol.bundled_tickets_used += 1);
                }
                return Ok(CommandEffect::Full);
            }
            shared.update(|m| m.protocol.data_connect_requests += 1);
            let result = request(control, "data.connect", json!({"session_id":session,"block_id":block,"data_ref_id":"terminal","subprotocol":wire::PROTOCOL})).await;
            let _ = reply.send(result.as_ref().map_err(|e| format!("{e:#}")).and_then(|v| {
                v["ticket"]
                    .as_str()
                    .map(|token| TerminalTicket {
                        token: token.into(),
                        bundled: false,
                    })
                    .ok_or_else(|| "Missing data ticket".into())
            }));
            result?;
        }
        HostCommand::Terminal {
            session,
            block,
            command,
        } => {
            let Some(worker) = workers.get_mut(&block) else {
                return Ok(CommandEffect::None);
            };
            let (command, receipt) = match command {
                Command::EchoReset(tracked) => {
                    let (command, receipt) = *tracked;
                    (command, Some(receipt))
                }
                command => (command, None),
            };
            match command {
                Command::Selection(input) => {
                    let _ = worker.commands.send(DataCommand::Selection(input));
                    return Ok(CommandEffect::None);
                }
                Command::CopyMode(input) => {
                    let _ = worker.commands.send(DataCommand::CopyMode(input));
                    return Ok(CommandEffect::None);
                }
                Command::Resize(cols, rows, w, h) => {
                    worker.size = Some((cols, rows, w, h));
                    resize(control, &session, &block, cols, rows, w, h).await?;
                    if let Some(receipt) = receipt {
                        receipt.complete();
                    }
                    return Ok(CommandEffect::None);
                }
                Command::Scroll(delta) => {
                    let _ = worker.commands.send(DataCommand::Scroll(delta));
                    return Ok(CommandEffect::None);
                }
                Command::Search(needle) => {
                    let _ = worker.commands.send(DataCommand::Search(needle));
                    return Ok(CommandEffect::None);
                }
                Command::SearchStep(older) => {
                    let _ = worker.commands.send(DataCommand::SearchStep(older));
                    return Ok(CommandEffect::None);
                }
                Command::CursorStyle(shape) => {
                    let _ = worker.commands.send(DataCommand::CursorStyle(shape));
                    return Ok(CommandEffect::None);
                }
                Command::PromptStep(older) => {
                    let _ = worker.commands.send(DataCommand::PromptStep(older));
                    return Ok(CommandEffect::None);
                }
                Command::Reconnect => {
                    let _ = worker.commands.send(DataCommand::Reconnect);
                    return Ok(CommandEffect::None);
                }
                // Neither is input: the block is not claimed. A refusal becomes the host's
                // notice and a transport failure reconnects; either way it is not sent again.
                verb @ (Command::Reset | Command::Clear) => {
                    let method = match verb {
                        Command::Reset => "com.superlogical.terminal.reset",
                        _ => "com.superlogical.terminal.clear",
                    };
                    request(
                        control,
                        method,
                        json!({"session_id":session,"block_id":block,"args":{}}),
                    )
                    .await?;
                    if let Some(receipt) = receipt {
                        receipt.complete();
                    }
                    // The history the view may have been scrolled into is gone.
                    let _ = worker.commands.send(DataCommand::Bottom);
                }
                input => {
                    let (connected, owner) = {
                        let m = shared.model.lock().expect("host model");
                        let Some(connection) = m.terminals.get(&block) else {
                            return Ok(CommandEffect::None);
                        };
                        let m = connection.model.lock().expect("terminal model");
                        (m.connected && m.control_connected, m.owner)
                    };
                    if !connected {
                        return Ok(CommandEffect::None);
                    }
                    // Releases, pointer motion and wheel turns never claim the block; presses do.
                    let claimed = !owner && !input::passive(&input).unwrap_or(false);
                    if claimed {
                        let result = request(
                            control,
                            "session.own_block",
                            json!({"session_id":session,"block_ids":[block],"mode":"take"}),
                        )
                        .await?;
                        shared.update(|m| {
                            match result["blocks"]
                                .as_array()
                                .and_then(|blocks| blocks.iter().find(|b| b["block_id"] == block))
                            {
                                Some(b) => m.set_owner(&block, b["owner"].as_str()),
                                None => {
                                    if let Some(connection) = m.terminals.get(&block) {
                                        connection.model.lock().expect("terminal model").owner =
                                            false;
                                    }
                                }
                            }
                        });
                    }
                    // Typing, pasting and clicking return the view to the live screen; pointer
                    // motion, releases and wheel turns leave it (and its frame) alone.
                    if !matches!(&input, Command::Mouse(m) if m["action"] != "press")
                        && !matches!(input, Command::Wheel(_))
                    {
                        let _ = worker.commands.send(DataCommand::Bottom);
                    }
                    if matches!(
                        input,
                        Command::Key(_)
                            | Command::KeyWithPrefix { .. }
                            | Command::SendKey(_)
                            | Command::Paste(_)
                    ) && let Some(connection) = shared
                        .model
                        .lock()
                        .expect("host model")
                        .terminals
                        .get(&block)
                    {
                        connection
                            .model
                            .lock()
                            .expect("terminal model")
                            .input_events += 1;
                    }
                    if let Some((method, payload)) = input::wire_request(&session, &block, input) {
                        inputs
                            .send(control, shared, method, payload, receipt)
                            .await?;
                    }
                    return Ok(if claimed {
                        CommandEffect::Claimed
                    } else {
                        CommandEffect::None
                    });
                }
            }
        }
        HostCommand::ListDir(_)
        | HostCommand::Visibility(_)
        | HostCommand::Action { .. }
        | HostCommand::Prompts(_)
        | HostCommand::PromptReply { .. }
        | HostCommand::Reconnect
        | HostCommand::SshAgent { .. }
        | HostCommand::Shutdown
        | HostCommand::StopServer { .. }
        | HostCommand::ResizeLayer { .. }
        | HostCommand::PrefetchViews(_)
        | HostCommand::FetchView { .. }
        | HostCommand::Keymap(_)
        | HostCommand::Identify
        | HostCommand::WatchSharedHosts
        | HostCommand::WatchPorts { .. }
        | HostCommand::ReadPorts { .. }
        | HostCommand::PutSharedHost { .. }
        | HostCommand::RemoveSharedHost { .. }
        | HostCommand::Download(_)
        | HostCommand::DownloadConfirm(_)
        | HostCommand::DownloadCancel(_)
        | HostCommand::Upload { .. } => {
            unreachable!()
        }
    }
    Ok(CommandEffect::Full)
}

/// The theme a block this client creates starts with, unless the server refused themes.
fn new_block_theme<'a>(shared: &Shared, theme: Option<&'a SetTheme>) -> Option<&'a SetTheme> {
    let mut m = shared.model.lock().expect("host model");
    let theme = theme.filter(|_| !m.themes_unsupported);
    m.theme_declared |= theme.is_some();
    theme
}

async fn resize(
    control: &mut Client,
    session: &str,
    block: &str,
    cols: u16,
    rows: u16,
    w: u16,
    h: u16,
) -> Result<()> {
    request(control, "com.superlogical.terminal.resize", json!({"session_id":session,"block_id":block,"args":{"columns":cols,"rows":rows,"cell_width_px":w,"cell_height_px":h}})).await?;
    Ok(())
}

fn fail_copy_mode(
    copy_mode: &mut crate::terminal::copy_mode::Controller,
    shared: &PaneShared,
    error: anyhow::Error,
) {
    let ended = copy_mode.reset();
    shared.update(|m| {
        m.notice = Some(format!("Copy mode failed: {error:#}"));
        if let Some(epoch) = ended {
            m.copy_ended_epoch = m.copy_ended_epoch.max(epoch);
        }
    });
}

fn sample_echo_facts<E>(
    rtt: &Rtt,
    frame: &mut Frame,
    position: u64,
    ack: u64,
    screen: impl FnOnce() -> Result<ghostty_vt::screen::Screen, E>,
) -> Result<(), E> {
    if rtt.enabled() {
        frame.alt_screen = screen()? == ghostty_vt::screen::Screen::Alternate;
        frame.output_position = position;
        frame.acked_input = ack;
    }
    Ok(())
}

async fn run_data(
    endpoint: String,
    session: String,
    block: String,
    host: flume::Sender<HostCommand>,
    commands: flume::Receiver<DataCommand>,
    shared: PaneShared,
) {
    let rtt = shared.model.lock().expect("terminal model").rtt.clone();
    let mut welcome: Option<wire::Welcome> = None;
    let mut stream: Option<TerminalStream> = None;
    let mut selection = None;
    let mut copy_mode = crate::terminal::copy_mode::Controller::default();
    let mut pending_yank: Option<crate::terminal::copy_mode::CopyYank> = None;
    let mut selection_revision: u64 = 0;
    let mut search = crate::terminal::search::Controller::default();
    let mut prompt_anchor = false;
    let mut visible = true;
    let mut cursor_style = crate::terminal::CursorShape::Block;
    let mut renderer = match Renderer::new() {
        Ok(r) => r,
        Err(e) => {
            shared.status(format!("Renderer failed: {e:#}"), false);
            return;
        }
    };
    loop {
        let result: Result<bool> = async {
            let mut socket = None;
            if let (Some(previous), Some(state)) = (&welcome, &stream) {
                let mut next = transport::socket(&endpoint, "/api/data", wire::PROTOCOL).await?;
                if let Ok(resumed) = wire::resume(&mut next, &previous.token, state.received).await {
                    anyhow::ensure!(resumed.nonce == previous.nonce, "Rex changed nonce during resume");
                    welcome = Some(resumed); socket = Some(next);
                    shared.status("Live · stream resumed", state.terminal.is_some());
                }
            }
            if socket.is_none() {
                let (next, fresh) = open_data(&endpoint, &session, &block, &host).await?;
                selection = Some(crate::terminal::selection::Controller::new()?);
                let ended = copy_mode.reset();
                pending_yank = None;
                selection_revision = 0;
                search.forget_terminal();
                prompt_anchor = false;
                let mut restored = TerminalStream::new(fresh.nonce);
                restored.set_cursor_style(cursor_style)?;
                stream = Some(restored); welcome = Some(fresh);
                shared.update(|m| {
                    m.restore_generation += 1;
                    m.copy_yank = None;
                    if let Some(epoch) = ended {
                        m.copy_ended_epoch = m.copy_ended_epoch.max(epoch);
                    }
                });
                renderer = Renderer::new()?; socket = Some(next);
                shared.status("Restoring terminal…", false);
            }
            let mut socket = socket.context("data socket")?;
            let stream = stream.as_mut().context("terminal stream")?;
            let selection = selection.as_mut().context("selection controller")?;
            let mut pending_copy_search: Option<PendingCopySearch> = None;
            let mut ack_slot: Option<Arc<AtomicU64>> = None;
            let mut output_ack = 0;
            let mut dirty = true;
            let mut tick = tokio::time::interval(Duration::from_millis(16));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    bytes = wire::next_binary(&mut socket) => {
                        let bytes = bytes?;
                        if !bytes.is_empty() && rtt.enabled() {
                            // Stamp at receipt. A delayed render or local redraw after the ack
                            // must not turn pre-ack data into a post-ack frame.
                            let slot = ack_slot.get_or_insert_with(|| shared.model.lock().expect("terminal model").input_acks.slot());
                            output_ack = slot.load(Ordering::Acquire);
                        }
                        if let Err(error) = stream.feed(&bytes) { welcome = None; return Err(error); }
                        dirty = true;
                    }
                    command = commands.recv_async() => match command {
                        Ok(DataCommand::Selection(input)) => if input.generation == shared.model.lock().expect("terminal model").restore_generation && let Some(terminal) = &stream.terminal {
                            let replaces = matches!(
                                input.phase,
                                crate::terminal::selection::Phase::Down(_)
                                    | crate::terminal::selection::Phase::Clear
                                    | crate::terminal::selection::Phase::All
                            ) || (matches!(input.phase, crate::terminal::selection::Phase::Move)
                                && selection.is_dragging());
                            selection.apply(terminal, input)?;
                            selection_revision = selection.revision;
                            if replaces {
                                copy_mode.clear_owned_selection();
                            }
                            dirty = true;
                        },
                        Ok(DataCommand::CopyMode(input)) => if input.generation == shared.model.lock().expect("terminal model").restore_generation && let Some(terminal) = &mut stream.terminal {
                            let is_to_match = matches!(input.op, crate::terminal::copy_mode::CopyModeOp::ToMatch);
                            match copy_mode.apply(terminal, Some(&search), input) {
                                Ok(outcome) if outcome.accepted => {
                                    prompt_anchor = false;
                                    selection_revision = input.revision;
                                    if let Err(error) = selection.stop(terminal) {
                                        shared.update(|m| m.notice = Some(format!("Copy mode failed: {error:#}")));
                                    }
                                    if let Some(yank) = outcome.yank {
                                        pending_yank = Some(yank);
                                    }
                                    if is_to_match && search.has_needle() && !search.is_settled() {
                                        pending_copy_search = Some(PendingCopySearch {
                                            epoch: input.epoch,
                                            steps: Vec::new(),
                                        });
                                    } else {
                                        pending_copy_search = None;
                                    }
                                    dirty = true;
                                }
                                Ok(_) => {}
                                Err(error) => {
                                    pending_copy_search = None;
                                    fail_copy_mode(&mut copy_mode, &shared, error);
                                    dirty = true;
                                }
                            }
                        },
                        Ok(DataCommand::Visible(value)) => { if value && !visible { dirty = true; } else if !value && let Some(terminal) = &stream.terminal { selection.stop(terminal)?; } visible = value; }
                        Ok(DataCommand::Scroll(delta)) => if let Some(terminal) = &mut stream.terminal {
                            prompt_anchor = false;
                            pending_copy_search = None;
                            let before = terminal.scrollbar()?.offset as usize;
                            terminal.scroll_viewport(ghostty_vt::terminal::ScrollViewport::Delta(delta));
                            selection.after_scroll(terminal)?;
                            if let Err(error) = copy_mode.after_viewport_scroll_delta(terminal, before, delta) {
                                fail_copy_mode(&mut copy_mode, &shared, error);
                            }
                            dirty = true;
                        },
                        Ok(DataCommand::CursorStyle(shape)) => { cursor_style = shape; stream.set_cursor_style(shape)?; dirty = true; },
                        Ok(DataCommand::PromptStep(older)) => if let Some(terminal) = &mut stream.terminal {
                            pending_copy_search = None;
                            let before = terminal.scrollbar()?.offset;
                            terminal.scroll_viewport(ghostty_vt::terminal::ScrollViewport::DeltaPrompt(if older { -1 } else { 1 }));
                            if terminal.scrollbar()?.offset != before {
                                prompt_anchor = !terminal.is_viewport_active()?;
                            }
                            selection.after_scroll(terminal)?;
                            if let Err(error) = copy_mode.after_viewport_scroll(terminal) {
                                fail_copy_mode(&mut copy_mode, &shared, error);
                            }
                            dirty = true;
                        },
                        Ok(DataCommand::Bottom) => if let Some(terminal) = &mut stream.terminal {
                            prompt_anchor = false;
                            pending_copy_search = None;
                            terminal.scroll_viewport(ghostty_vt::terminal::ScrollViewport::Bottom);
                            if let Err(error) = copy_mode.after_scroll_bottom(terminal) {
                                fail_copy_mode(&mut copy_mode, &shared, error);
                            }
                            dirty = true;
                        },
                        // A search that fails is dropped with a notice; the terminal's stream is not its to end.
                        Ok(DataCommand::Search(needle)) => {
                            prompt_anchor = false;
                            pending_copy_search = None;
                            if let Err(error) = search.set_needle(stream.terminal.as_ref(), needle) {
                                search = Default::default();
                                shared.update(|m| m.notice = Some(format!("Search failed: {error:#}")));
                            }
                            dirty = true;
                        }
                        // Ghostty scrolls the selected match into view.
                        Ok(DataCommand::SearchStep(older)) => if let Some(terminal) = &mut stream.terminal {
                            prompt_anchor = false;
                            if search.has_needle()
                                && !search.is_settled()
                                && let Some(epoch) = copy_mode.active_epoch()
                            {
                                pending_copy_search
                                    .get_or_insert(PendingCopySearch {
                                        epoch,
                                        steps: Vec::new(),
                                    })
                                    .steps
                                    .push(older);
                            } else {
                                if let Err(error) = search.step(terminal, older) {
                                    search = Default::default();
                                    pending_copy_search = None;
                                    shared.update(|m| m.notice = Some(format!("Search failed: {error:#}")));
                                }
                                selection.after_scroll(terminal)?;
                                if copy_mode.is_active()
                                    && let Err(error) = copy_mode.move_to_match(terminal, &search)
                                {
                                    fail_copy_mode(&mut copy_mode, &shared, error);
                                }
                            }
                            dirty = true;
                        },
                        Ok(DataCommand::Reconnect) => { shared.status("Reconnecting terminal…", false); return Ok(false); }
                        Ok(DataCommand::Stop) | Err(_) => return Ok(true),
                    },
                    _ = tick.tick(), if visible && commands.is_empty() && (dirty || stream.terminal.as_ref().is_some_and(|t| selection.autoscrolling(t))) => {
                        // Mid synchronized update: keep showing the last frame, check again next tick.
                        if stream.render_held(Instant::now())? { continue; }
                        if let Some(terminal) = &mut stream.terminal {
                            selection.tick(terminal)?;
                            let mut search_pending = match search.update(terminal) {
                                Ok(pending) => pending,
                                Err(error) => {
                                    search = Default::default();
                                    pending_copy_search = None;
                                    shared.update(|m| m.notice = Some(format!("Search failed: {error:#}")));
                                    false
                                }
                            };
                            if search_pending && pending_copy_search.is_some() {
                                for _ in 0..64 {
                                    match search.update(terminal) {
                                        Ok(false) => {
                                            search_pending = false;
                                            break;
                                        }
                                        Ok(true) => {}
                                        Err(error) => {
                                            search = Default::default();
                                            pending_copy_search = None;
                                            shared.update(|m| m.notice = Some(format!("Search failed: {error:#}")));
                                            search_pending = false;
                                            break;
                                        }
                                    }
                                }
                            }
                            if !search_pending
                                && let Some(pending) = pending_copy_search.take()
                                && copy_mode.active_epoch() == Some(pending.epoch)
                                && search.has_needle()
                            {
                                let mut failed = false;
                                for older in pending.steps {
                                    if let Err(error) = search.step(terminal, older) {
                                        search = Default::default();
                                        shared.update(|m| m.notice = Some(format!("Search failed: {error:#}")));
                                        failed = true;
                                        break;
                                    }
                                }
                                if !failed {
                                    selection.after_scroll(terminal)?;
                                    if let Err(error) = copy_mode.move_to_match(terminal, &search) {
                                        fail_copy_mode(&mut copy_mode, &shared, error);
                                    }
                                }
                            }
                            if let Err(error) = copy_mode.sync(terminal) {
                                fail_copy_mode(&mut copy_mode, &shared, error);
                            }
                            let mut frame = renderer.frame(terminal)?;
                            frame.viewport_active = terminal.is_viewport_active()?;
                            if frame.viewport_active { prompt_anchor = false; }
                            sample_echo_facts(&rtt, &mut frame, stream.received, output_ack, || terminal.active_screen())?;
                            frame.prompt_anchor = prompt_anchor;
                            frame.selection_revision = selection_revision;
                            frame.copy = match copy_mode.view(terminal) {
                                Ok(view) => view,
                                Err(error) => {
                                    fail_copy_mode(&mut copy_mode, &shared, error);
                                    None
                                }
                            };
                            frame.search = match search.results(terminal) {
                                Ok(results) => results,
                                Err(error) => { search = Default::default(); shared.update(|m| m.notice = Some(format!("Search failed: {error:#}"))); None }
                            };
                            let yank = pending_yank.take();
                            let ended = copy_mode.ended_epoch();
                            let mut transitioned = false;
                            shared.update(|m| {
                                frame.stream_epoch = m.stream_epoch;
                                m.frame = Some(Arc::new(frame));
                                if let Some(yank) = yank {
                                    m.copy_yank = Some(yank);
                                }
                                if ended > m.copy_ended_epoch {
                                    m.copy_ended_epoch = ended;
                                }
                                if !m.connected { m.connected = true; m.status = "Live · snapshot restored".into(); transitioned = true; }
                                if let Some(code) = stream.exit_code { m.status = format!("Process exited · {code}"); }
                            });
                            if transitioned { shared.host_wake.notify(); }
                            dirty = search_pending;
                        } else {
                            dirty = false;
                        }
                    }
                }
            }
        }.await;
        match result {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                if error
                    .downcast_ref::<wire::DataClosed>()
                    .is_some_and(|closed| closed.code == Some(4000))
                {
                    shared.status("Terminal closed on the server", false);
                    return;
                }
                shared.status(format!("Reconnecting · {error:#}"), false);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

/// Queued copy-mode jump/steps waiting for an in-flight search scan to settle.
struct PendingCopySearch {
    epoch: u64,
    steps: Vec<bool>,
}

async fn open_data(
    endpoint: &str,
    session: &str,
    block: &str,
    host: &flume::Sender<HostCommand>,
) -> Result<(transport::Socket, wire::Welcome)> {
    loop {
        let (reply, ticket) = tokio::sync::oneshot::channel();
        host.send(HostCommand::Ticket {
            session: session.into(),
            block: block.into(),
            reply,
        })
        .map_err(|_| anyhow::anyhow!("host worker stopped"))?;
        let ticket = ticket.await?.map_err(anyhow::Error::msg)?;
        let mut next = transport::socket(endpoint, "/api/data", wire::PROTOCOL).await?;
        match wire::handshake(&mut next, &ticket.token).await {
            Ok(welcome) => return Ok((next, welcome)),
            // The pool removed this single-use token before handing it out. The
            // next request goes through data.connect on a fresh socket.
            Err(error) if ticket.bundled && error.is::<wire::HandshakeRejected>() => {}
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests;
