//! Reaching a Rex server over SSH. An `ssh://[user@]host[:port][/socket]` endpoint forwards the
//! server's Unix socket on that machine to a local one with `ssh -L`, and the client attaches
//! to the local end like any `unix://` endpoint. Only `sshd` is needed on the other side, so a
//! machine with Tailscale SSH, or any SSH key already set up, works without configuring Rex
//! there. Without a `/socket`, the machine is asked where its server listens.
use anyhow::{Context, Result, bail, ensure};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::AsyncReadExt;

pub mod hostkey;
pub mod install;
pub mod setup;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyProblem {
    Unknown,
    Changed,
}

impl std::fmt::Display for KeyProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unknown => "SSH host key not trusted. Choose Check host key… before connecting.",
            Self::Changed => {
                "SSH host key changed. Connection refused. Choose Check host key… for details."
            }
        })
    }
}

impl std::error::Error for KeyProblem {}

fn key_problem(stderr: &str) -> Option<KeyProblem> {
    if stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")
        || stderr.contains("REVOKED HOST KEY DETECTED")
        || stderr.contains("POSSIBLE DNS SPOOFING DETECTED")
        || stderr.lines().any(|line| {
            line.starts_with("Host key for ")
                && line.ends_with("has changed and you have requested strict checking.")
        })
    {
        Some(KeyProblem::Changed)
    } else if stderr
        .lines()
        .any(|line| line.starts_with("Certificate invalid:"))
    {
        None
    } else if stderr.lines().any(|line| {
        line.starts_with("No ")
            && line.contains(" host key is known for ")
            && line.ends_with("and you have requested strict checking.")
    }) {
        Some(KeyProblem::Unknown)
    } else {
        None
    }
}

/// Facts from the last line of the read-only remote lookup. Legacy replies carry
/// only the user; an unknown OS or uid cannot authorize an installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    pub user: String,
    pub os: String,
    pub uid: Option<u32>,
    pub rex: bool,
}

impl Facts {
    fn parse(line: &str) -> Result<Self> {
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        match fields.as_slice() {
            [user, os, uid, rex @ ("rex" | "norex")] => Ok(Self {
                user: (*user).into(),
                os: (*os).into(),
                uid: Some(uid.parse()?),
                rex: *rex == "rex",
            }),
            [user] => Ok(Self {
                user: (*user).into(),
                os: String::new(),
                uid: None,
                rex: false,
            }),
            _ => bail!("The SSH host did not report usable setup facts."),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    NoServer(Facts),
    UnknownKey,
    ChangedKey,
}

impl Problem {
    pub fn from_error(error: &anyhow::Error) -> Option<Self> {
        if let Some(missing) = error.downcast_ref::<MissingServer>() {
            return Some(Self::NoServer(missing.facts.clone()));
        }
        match error.downcast_ref::<KeyProblem>()? {
            KeyProblem::Unknown => Some(Self::UnknownKey),
            KeyProblem::Changed => Some(Self::ChangedKey),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupOffer {
    Start,
    Install,
    Key,
}

impl SetupOffer {
    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "Start personal server…",
            Self::Install => "Install Rex on this host…",
            Self::Key => "Check host key…",
        }
    }
}

#[derive(Debug)]
pub struct MissingServer {
    pub facts: Facts,
    message: String,
}
impl std::fmt::Display for MissingServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for MissingServer {}

/// Where an `ssh://` endpoint points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The login name; none lets `ssh` apply its configuration and the local user name.
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
    /// The Rex socket's absolute path on that machine, when the user named it; none asks the
    /// machine (see [`RESOLVE`]).
    pub socket: Option<String>,
}

const USAGE: &str = "Use ssh://user@host, with an optional :port and /path/to/server.sock";

impl Target {
    /// `None` for endpoints of other schemes; an error for a malformed `ssh://` one.
    pub fn parse(endpoint: &str) -> Result<Option<Target>> {
        let Some(rest) = endpoint.strip_prefix("ssh://") else {
            return Ok(None);
        };
        ensure!(!rest.contains(char::is_whitespace), "{USAGE}");
        let (rest, socket) = match rest.find('/') {
            Some(slash) => {
                let socket = &rest[slash..];
                ensure!(
                    socket.len() > 1 && !socket.contains(['\'', '"', '$', '`', '\\']),
                    "Use an absolute socket path after the host, such as /run/rex/server.sock"
                );
                (&rest[..slash], Some(socket))
            }
            None => (rest, None),
        };
        let (user, address) = match rest.rsplit_once('@') {
            Some((user, address)) => (Some(user), address),
            None => (None, rest),
        };
        // An IPv6 address is written in brackets, as in a URL.
        let (host, after) = match address.strip_prefix('[') {
            Some(inner) => {
                let (host, after) = inner.rsplit_once(']').with_context(|| USAGE)?;
                (host, after)
            }
            None => match address.rsplit_once(':') {
                Some((host, _)) => (host, &address[host.len()..]),
                None => (address, ""),
            },
        };
        let port = match after.strip_prefix(':') {
            Some(port) => Some(
                port.parse::<u16>()
                    .ok()
                    .filter(|p| *p > 0)
                    .with_context(|| format!("\"{port}\" is not a port number"))?,
            ),
            None => {
                ensure!(after.is_empty(), "{USAGE}");
                None
            }
        };
        ensure!(!host.is_empty() && !host.starts_with('-'), "{USAGE}");
        ensure!(
            user.is_none_or(|u| !u.is_empty() && !u.starts_with('-') && !u.contains(':')),
            "{USAGE}"
        );
        Ok(Some(Target {
            user: user.map(str::to_owned),
            host: host.to_owned(),
            port,
            socket: socket.map(str::to_owned),
        }))
    }

    /// The `ssh://` endpoint of this target.
    pub fn endpoint(&self) -> String {
        let mut out = String::from("ssh://");
        if let Some(user) = &self.user {
            out.push_str(user);
            out.push('@');
        }
        out.push_str(&self.host_in_url());
        if let Some(port) = self.port {
            out.push_str(&format!(":{port}"));
        }
        if let Some(socket) = &self.socket {
            out.push_str(socket);
        }
        out
    }

    pub fn setup_offer(&self, problem: &Problem) -> Option<SetupOffer> {
        match problem {
            Problem::UnknownKey | Problem::ChangedKey => Some(SetupOffer::Key),
            Problem::NoServer(facts) if self.socket.is_none() && facts.rex => {
                Some(SetupOffer::Start)
            }
            Problem::NoServer(facts)
                if self.socket.is_none()
                    && facts.os == "Linux"
                    && facts.uid.is_some_and(|uid| uid != 0) =>
            {
                Some(SetupOffer::Install)
            }
            _ => None,
        }
    }

    fn missing_server(&self, facts: Facts) -> MissingServer {
        let user = &facts.user;
        let guide = match self.setup_offer(&Problem::NoServer(facts.clone())) {
            Some(offer @ (SetupOffer::Start | SetupOffer::Install)) => {
                format!("Choose {} to review setup.", offer.label())
            }
            _ => "Set up Rex on the host by hand; see docs/hosts.md.".into(),
        };
        let message = format!(
            "No Rex server found on {} for {user}: nothing in {user}'s runtime directory, /run/rex or /var/run/rex. Attachment never starts a server. {guide} If a server already runs elsewhere, name its socket in the SSH endpoint.",
            self.host
        );
        MissingServer { facts, message }
    }

    /// The host as a URL writes it: an IPv6 address in brackets.
    fn host_in_url(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        }
    }

    /// `user@host` as `ssh` takes it.
    pub fn destination(&self) -> String {
        match &self.user {
            Some(user) => format!("{user}@{}", self.host),
            None => self.host.clone(),
        }
    }

    /// The options every `ssh` run uses: never prompt or update known hosts, refuse unknown
    /// and changed keys, notice a dead link within a minute, and keep clear of a
    /// `ControlMaster` in the user's configuration, which would take the forward over and keep
    /// it after this `ssh` ends, so the next forward of that path never listens. Logging stays
    /// at its default so a server's banner (how Tailscale SSH explains a refused login) is
    /// seen.
    fn options(&self) -> Vec<String> {
        let mut args = self.config_options();
        args.extend([
            "-o".into(),
            "StrictHostKeyChecking=yes".into(),
            "-o".into(),
            "UpdateHostKeys=no".into(),
        ]);
        args
    }

    fn config_options(&self) -> Vec<String> {
        let mut args = vec![
            "-o".into(),
            "ControlMaster=no".into(),
            "-o".into(),
            "ControlPath=none".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ConnectTimeout=10".into(),
            "-o".into(),
            "ServerAliveInterval=15".into(),
            "-o".into(),
            "ServerAliveCountMax=3".into(),
        ];
        if let Some(port) = self.port {
            args.push("-p".into());
            args.push(port.to_string());
        }
        args
    }

    /// The local socket the forward binds: fixed per endpoint within this process, so panes
    /// reconnect through the same path after a new tunnel replaces a dead one, and in a
    /// directory of this process's own, so another instance (or a probe) forwarding the same
    /// endpoint neither takes this path over nor removes it when it closes.
    pub fn local_socket(&self) -> PathBuf {
        let mut hasher = DefaultHasher::new();
        self.endpoint().hash(&mut hasher);
        tunnels_directory()
            .join(format!("ssh-{}", std::process::id()))
            .join(format!("{:016x}.sock", hasher.finish()))
    }
}

/// Where every process's tunnel sockets live.
fn tunnels_directory() -> PathBuf {
    crate::platform::dirs::runtime()
}

/// Finds the Rex socket on the other machine, trying in order: the path `rex server status`
/// reports, the login user's own server in the runtime directory (Linux) or under
/// Application Support (macOS), then a shared server's `/run/rex/server.sock` or FreeBSD's
/// `/var/run/rex/server.sock`. Prints `sock <path>` for the first that is a socket, else
/// `none <user> <os> <uid> <rex|norex>`. Wrapped in `sh -c '…'`, so no single quotes.
const RESOLVE: &str = r#"p=""; for c in "$(rex server status --autostart=false 2>/dev/null | grep -o "unix:///[^[:space:]]*\.sock" | head -n 1 | sed "s#^unix://##")" "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/rex/server.sock" "$HOME/Library/Application Support/rex/server.sock" /run/rex/server.sock /var/run/rex/server.sock; do if [ -n "$c" ] && [ -S "$c" ]; then p="$c"; break; fi; done; if [ -n "$p" ]; then printf "sock %s\n" "$p"; else r=norex; if command -v rex >/dev/null 2>&1 || [ -x "$HOME/.local/bin/rex" ]; then r=rex; fi; printf "none %s %s %s %s\n" "$(id -un)" "$(uname -s)" "$(id -u)" "$r"; fi"#;

/// The check for a socket the user named: `sock <path>` when it is one, else `missing <path>`.
fn check_script(socket: &str) -> String {
    format!(
        "if [ -S \"{socket}\" ]; then printf \"sock %s\\n\" \"{socket}\"; else printf \"missing %s\\n\" \"{socket}\"; fi"
    )
}

/// A running `ssh -L` forward. Dropping it ends the forward.
pub struct Tunnel {
    child: tokio::process::Child,
    /// Held here because `Child::wait` closes the stdin it still owns, and that would end the
    /// remote `cat` and with it the login the forward lives on.
    _stdin: Option<tokio::process::ChildStdin>,
    stderr: Arc<Mutex<String>>,
    local: PathBuf,
    endpoint: String,
    target: Target,
}

impl Tunnel {
    /// A forward for `endpoint` if it is an `ssh://` one, else `None`.
    pub async fn for_endpoint(endpoint: &str) -> Result<Option<Tunnel>> {
        match Target::parse(endpoint)? {
            Some(target) => Ok(Some(Tunnel::open(&target).await?)),
            None => Ok(None),
        }
    }

    /// Find the Rex socket on `target`, then forward it to a local socket and wait until the
    /// local end accepts connections.
    pub async fn open(target: &Target) -> Result<Tunnel> {
        crate::platform::local::require_ssh()?;
        let remote = resolve(target).await?;
        let local = target.local_socket();
        if let Some(parent) = local.parent() {
            crate::platform::local::prepare_tunnel_directory(parent).await?;
        }
        // Only this process's own earlier forward of the endpoint can be here.
        let _ = tokio::fs::remove_file(&local).await;
        // The remote `cat` ends when its input does, so if this process dies without dropping
        // the tunnel, the closed stdin pipe still ends the forward instead of leaving `ssh`
        // behind.
        let child = tokio::process::Command::new("ssh")
            .args(target.options())
            .args([
                "-T",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "StreamLocalBindUnlink=yes",
            ])
            .arg("-L")
            .arg(format!("{}:{remote}", local.display()))
            .arg("--")
            .arg(target.destination())
            .arg("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(ssh_error)?;
        let mut tunnel = Tunnel::adopt(child, target, local);
        let started = std::time::Instant::now();
        loop {
            if let Some(status) = tunnel.child.try_wait()? {
                return Err(tunnel.exit_error(status));
            }
            if crate::platform::local::connect(&tunnel.local).await.is_ok() {
                return Ok(tunnel);
            }
            ensure!(
                started.elapsed() < Duration::from_secs(20),
                "ssh {} did not open the forward in time",
                tunnel.target.destination()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn adopt(mut child: tokio::process::Child, target: &Target, local: PathBuf) -> Tunnel {
        let stderr = Arc::new(Mutex::new(String::new()));
        if let Some(pipe) = child.stderr.take() {
            let sink = stderr.clone();
            tokio::spawn(drain(pipe, sink));
        }
        Tunnel {
            _stdin: child.stdin.take(),
            child,
            stderr,
            endpoint: format!("unix://{}", local.display()),
            local,
            target: target.clone(),
        }
    }

    /// The `unix://` endpoint of the local end.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Resolves once the forward ends, with why.
    pub async fn closed(&mut self) -> anyhow::Error {
        match self.child.wait().await {
            Ok(status) => self.exit_error(status),
            Err(error) => anyhow::Error::from(error).context("waiting for ssh"),
        }
    }

    fn exit_error(&self, status: std::process::ExitStatus) -> anyhow::Error {
        let problem = key_problem(&self.stderr.lock().expect("ssh stderr"));
        problem.map_or_else(
            || anyhow::anyhow!("{}", self.exit_message(status)),
            Into::into,
        )
    }

    fn exit_message(&self, status: std::process::ExitStatus) -> String {
        let stderr = self.stderr.lock().expect("ssh stderr");
        match last_line(&stderr) {
            Some(line) => format!("ssh {}: {line}", self.target.destination()),
            None if status.success() => {
                format!("ssh {} closed the forward", self.target.destination())
            }
            None => format!("ssh {} exited ({status})", self.target.destination()),
        }
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let _ = std::fs::remove_file(&self.local);
        // Gone once the last forward of this process ends; it fails harmlessly otherwise.
        if let Some(parent) = self.local.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

/// Wait for a tunnel to end; pends forever without one, for `select!` arms.
pub async fn closed(tunnel: &mut Option<Tunnel>) -> anyhow::Error {
    match tunnel {
        Some(tunnel) => tunnel.closed().await,
        None => std::future::pending().await,
    }
}

async fn drain(mut pipe: tokio::process::ChildStderr, sink: Arc<Mutex<String>>) {
    let mut buffer = [0u8; 1024];
    while let Ok(n) = pipe.read(&mut buffer).await {
        if n == 0 {
            break;
        }
        let mut text = sink.lock().expect("ssh stderr");
        text.push_str(&String::from_utf8_lossy(&buffer[..n]));
        if text.len() > 8192 {
            let cut = text.len() - 4096;
            text.drain(..cut);
        }
    }
}

/// The last line `ssh` printed that explains something, without its `ssh: ` prefix. A
/// server's parting words (such as Tailscale SSH refusing a user) beat the generic "Connection
/// closed" that follows them.
fn last_line(stderr: &str) -> Option<String> {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("Warning: Permanently added"))
        .collect();
    let generic = |l: &str| {
        l.starts_with("Connection closed by")
            || l.starts_with("Connection to ")
            || l.starts_with("Transferred:")
            || l.starts_with("Bytes per second:")
    };
    lines
        .iter()
        .rfind(|l| !generic(l))
        .or(lines.last())
        .map(|l| l.strip_prefix("ssh: ").unwrap_or(l).to_owned())
}

fn ssh_error(error: std::io::Error) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        anyhow::anyhow!("ssh isn't installed on this machine")
    } else {
        anyhow::Error::from(error).context("run ssh")
    }
}

/// Ask the other machine where its Rex server listens, or whether the socket the user named
/// is there.
pub async fn resolve(target: &Target) -> Result<String> {
    resolve_with(target, std::ffi::OsStr::new("ssh")).await
}

async fn resolve_with(target: &Target, ssh: &std::ffi::OsStr) -> Result<String> {
    let (stdout, stderr) = lookup(target, ssh).await?;
    enum Answer {
        Sock(String),
        None(String),
        Missing(String),
    }
    let answer = stdout.lines().rev().find_map(|l| {
        l.strip_prefix("sock ")
            .map(|p| Answer::Sock(p.to_owned()))
            .or_else(|| {
                l.strip_prefix("none ")
                    .map(|u| Answer::None(u.trim().to_owned()))
            })
            .or_else(|| {
                l.strip_prefix("missing ")
                    .map(|p| Answer::Missing(p.trim().to_owned()))
            })
    });
    match answer {
        Some(Answer::Sock(path)) => Ok(path),
        Some(Answer::None(line)) => Err(target.missing_server(Facts::parse(&line)?).into()),
        Some(Answer::Missing(path)) => bail!(
            "{path} on {} is not a socket. Is Rex running there?",
            target.host
        ),
        None => bail!(
            "ssh {} did not report a Rex socket: {}",
            target.destination(),
            last_line(&stderr).unwrap_or_else(|| stdout.trim().to_owned())
        ),
    }
}

async fn lookup(target: &Target, ssh: &std::ffi::OsStr) -> Result<(String, String)> {
    crate::platform::local::require_ssh()?;
    let script = match &target.socket {
        Some(socket) => check_script(socket),
        None => RESOLVE.to_owned(),
    };
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(ssh)
            .args(target.options())
            .arg("--")
            .arg(target.destination())
            .arg(format!("sh -c '{script}'"))
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .with_context(|| format!("ssh {} did not answer", target.destination()))?
    .map_err(ssh_error)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        if let Some(problem) = key_problem(&stderr) {
            return Err(problem.into());
        }
        bail!(
            "ssh {}: {}",
            target.destination(),
            last_line(&stderr).unwrap_or_else(|| format!("exited ({})", output.status))
        );
    }
    Ok((
        String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr.into_owned(),
    ))
}

// Tunnel fixtures execute Unix shells and depend on Unix socket paths.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn only_key_errors_are_classified_and_change_takes_priority() {
        assert_eq!(
            key_problem(
                "No ED25519 host key is known for box and you have requested strict checking.\nHost key verification failed."
            ),
            Some(KeyProblem::Unknown)
        );
        assert_eq!(
            key_problem(
                "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!\nHost key verification failed.\n"
            ),
            Some(KeyProblem::Changed)
        );
        for (stderr, expected) in [
            ("Host key verification failed.\r\n", None),
            (
                "WARNING: POSSIBLE DNS SPOOFING DETECTED!\nHost key verification failed.\n",
                Some(KeyProblem::Changed),
            ),
            (
                "Host key for box has changed and you have requested strict checking.\nHost key verification failed.\n",
                Some(KeyProblem::Changed),
            ),
            (
                "Certificate invalid: name is not a listed principal\nHost key verification failed.\n",
                None,
            ),
            (
                "Certificate invalid: expired\nNo ED25519 host key is known for box and you have requested strict checking.\nHost key verification failed.\n",
                None,
            ),
        ] {
            assert_eq!(key_problem(stderr), expected, "{stderr:?}");
        }
        assert_eq!(key_problem("box: Permission denied (publickey)."), None);
        assert_eq!(
            key_problem("ssh: connect to host box port 22: Connection refused"),
            None
        );
        assert!(
            Target::parse("ssh://box")
                .unwrap()
                .unwrap()
                .options()
                .contains(&"StrictHostKeyChecking=yes".into())
        );
    }

    #[test]
    fn targets_parse_and_print() {
        let t = Target::parse("ssh://shuv@box.tail1234.ts.net")
            .unwrap()
            .unwrap();
        assert_eq!(t.user.as_deref(), Some("shuv"));
        assert_eq!(t.host, "box.tail1234.ts.net");
        assert_eq!(t.port, None);
        assert_eq!(t.endpoint(), "ssh://shuv@box.tail1234.ts.net");
        assert_eq!(t.destination(), "shuv@box.tail1234.ts.net");

        let t = Target::parse("ssh://box:2222").unwrap().unwrap();
        assert_eq!(t.user, None);
        assert_eq!(t.port, Some(2222));
        assert_eq!(t.socket, None);
        assert_eq!(t.endpoint(), "ssh://box:2222");
        assert_eq!(t.destination(), "box");

        let t = Target::parse("ssh://ops@box:2222/run/rex/server.sock")
            .unwrap()
            .unwrap();
        assert_eq!(t.user.as_deref(), Some("ops"));
        assert_eq!(t.port, Some(2222));
        assert_eq!(t.socket.as_deref(), Some("/run/rex/server.sock"));
        assert_eq!(t.endpoint(), "ssh://ops@box:2222/run/rex/server.sock");
        assert_eq!(t.destination(), "ops@box");

        let t = Target::parse("ssh://me@[fd7a::1]:22/run/rex/server.sock")
            .unwrap()
            .unwrap();
        assert_eq!(t.host, "fd7a::1");
        assert_eq!(t.port, Some(22));
        assert_eq!(t.endpoint(), "ssh://me@[fd7a::1]:22/run/rex/server.sock");
        assert_eq!(t.destination(), "me@fd7a::1");
        let t = Target::parse("ssh://[fd7a::1]").unwrap().unwrap();
        assert_eq!(t.host, "fd7a::1");
        assert_eq!(t.port, None);

        assert!(Target::parse("https://box").unwrap().is_none());
        assert!(Target::parse("unix:///tmp/x.sock").unwrap().is_none());
        for bad in [
            "ssh://",
            "ssh://@box",
            "ssh://-box",
            "ssh://me@-box",
            "ssh://-user@box",
            "ssh://[-box]",
            "ssh://box:x",
            "ssh://box:0",
            "ssh://box/",
            "ssh://box/run/a b.sock",
            "ssh://box/run/$(x).sock",
            "ssh://a b",
            "ssh://[fd7a::1",
            "ssh://[fd7a::1]x",
        ] {
            assert!(Target::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn facts_from_current_and_legacy_replies_guide_setup() {
        let target = Target::parse("ssh://ops@box:2222").unwrap().unwrap();
        for (last, expected) in [
            ("ops Linux 1000 norex", Some(SetupOffer::Install)),
            ("ops Linux 0 norex", None),
            ("ops Darwin 1000 norex", None),
            ("ops FreeBSD 1000 rex", Some(SetupOffer::Start)),
            ("ops", None),
        ] {
            let facts = Facts::parse(last).unwrap();
            assert_eq!(facts.user, "ops");
            assert_eq!(
                target.setup_offer(&Problem::NoServer(facts.clone())),
                expected
            );
            let message = target.missing_server(facts).to_string();
            assert!(message.contains("Attachment never starts a server"));
            assert!(message.contains(expected.map_or("docs/hosts.md", SetupOffer::label)));
        }
        assert_eq!(
            Facts::parse("me Linux 1009 rex").unwrap(),
            Facts {
                user: "me".into(),
                os: "Linux".into(),
                uid: Some(1009),
                rex: true
            }
        );
        assert!(Facts::parse("ops Linux -1 norex").is_err());
        assert!(Facts::parse("ops Linux 1000 maybe").is_err());
    }

    #[test]
    fn the_local_socket_is_fixed_per_endpoint_and_owned_by_this_process() {
        let a = Target::parse("ssh://shuv@box").unwrap().unwrap();
        let b = Target::parse("ssh://shuv@box").unwrap().unwrap();
        let c = Target::parse("ssh://root@box").unwrap().unwrap();
        let d = Target::parse("ssh://shuv@box/run/rex/server.sock")
            .unwrap()
            .unwrap();
        assert_eq!(a.local_socket(), b.local_socket());
        assert_ne!(a.local_socket(), c.local_socket());
        assert_ne!(a.local_socket(), d.local_socket());
        assert!(a.local_socket().to_string_lossy().ends_with(".sock"));
        let own = format!("ssh-{}", std::process::id());
        assert_eq!(
            a.local_socket().parent().unwrap().file_name().unwrap(),
            own.as_str(),
            "another instance forwarding the same endpoint uses another directory"
        );
    }

    #[tokio::test]
    async fn waiting_for_a_tunnel_to_end_leaves_its_login_running() {
        // Stands in for `ssh … cat`, which ends when its stdin does.
        let child = tokio::process::Command::new("sh")
            .args(["-c", "cat >/dev/null; echo the login ended >&2; exit 3"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let target = Target::parse("ssh://me@box").unwrap().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("rex-tunnel.sock");
        let mut tunnel = Tunnel::adopt(child, &target, local);
        assert!(
            tokio::time::timeout(Duration::from_millis(300), tunnel.closed())
                .await
                .is_err(),
            "the forward lasts as long as this process holds it"
        );
        // This process ending closes the pipe, and that ends the forward.
        tunnel._stdin = None;
        let error = tokio::time::timeout(Duration::from_secs(5), tunnel.closed())
            .await
            .expect("the login ends with its stdin");
        assert!(error.to_string().starts_with("ssh me@box"), "{error}");
    }

    #[test]
    fn ssh_stays_clear_of_a_shared_control_master() {
        let target = Target::parse("ssh://me@box").unwrap().unwrap();
        let options = target.options();
        for option in ["ControlMaster=no", "ControlPath=none"] {
            assert!(options.iter().any(|o| o == option), "{option}: {options:?}");
        }
    }

    #[test]
    fn the_check_script_names_the_socket() {
        let script = check_script("/run/rex/server.sock");
        assert!(script.contains("-S \"/run/rex/server.sock\""));
        assert!(!script.contains('\''), "it is wrapped in sh -c '…'");
    }

    #[test]
    fn the_last_useful_stderr_line_is_reported() {
        assert_eq!(
            last_line(
                "Warning: Permanently added 'box' (ED25519) to the list of known hosts.\nshuv@box: Permission denied (publickey).\n"
            ),
            Some("shuv@box: Permission denied (publickey).".into())
        );
        assert_eq!(last_line("\n  \n"), None);
        assert_eq!(
            last_line("ssh: connect to host box port 22: Connection refused"),
            Some("connect to host box port 22: Connection refused".into())
        );
        assert_eq!(
            last_line(
                "tailscale: tailnet policy does not permit you to SSH as user \"me\"\nConnection closed by 100.1.2.3 port 22\n"
            ),
            Some("tailscale: tailnet policy does not permit you to SSH as user \"me\"".into())
        );
        assert_eq!(
            last_line("Connection closed by 100.1.2.3 port 22\n"),
            Some("Connection closed by 100.1.2.3 port 22".into())
        );
    }

    #[test]
    fn the_resolve_script_has_no_single_quotes_and_knows_the_documented_paths() {
        assert!(!RESOLVE.contains('\''), "it is wrapped in sh -c '…'");
        for path in [
            "rex server status",
            "/rex/server.sock",
            "/run/rex/server.sock",
            "/var/run/rex/server.sock",
            "Application Support/rex/server.sock",
        ] {
            assert!(RESOLVE.contains(path), "{path}");
        }
        // The shell accepts it.
        let status = std::process::Command::new("sh")
            .arg("-n")
            .arg("-c")
            .arg(RESOLVE)
            .status()
            .unwrap();
        assert!(status.success());
    }
}
