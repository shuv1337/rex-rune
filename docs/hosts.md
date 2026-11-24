# Hosts and connections

## Attachment-only by default

Attaching and reconnecting never request a server start, stop, replacement or
reconfiguration and do not need a local `rex` executable. An already-configured
socket-activated service can start when something connects. SSH setup is separate
and always explicit. Settings → Integrations runs local `rex skill` with
`--autostart=false`. The explicit CLI features search `PATH`, then `$XDG_BIN_HOME`
or `~/.local/bin` for desktop launches. An unavailable server is shown as
disconnected and retried. Closing the client leaves server programs running.

A saved host contains its sessions. Each session's Rex windows become tabs, and
the server defines the terminal splits. Host endpoints must be `unix://` with an
absolute path, `https://`, `http://`, `wss://` or `ws://` URLs, or
`ssh://[user@]host[:port][/socket]`.

## Endpoints and discovery

**Add host** (the sidebar's footer, Settings → Remote Hosts, Commands, or the
switcher's footer) lists the machines on this device's tailnet, from `tailscale
status --json`, searchable by name. Only machines Tailscale shows to this device
appear; this machine itself and iPhones and iPads (which run neither Rex nor
`sshd`) are left out. A tailnet without MagicDNS lists machines by their Tailscale
IP. Each online machine is checked for a Rex server reachable over the tailnet, at
the same addresses the official clients try for a bare host name:
`https://<machine>` on port 443 (built-in Tailscale or `tailscale serve`), then
`https://` and `http://` on port 7297. The check negotiates a control connection
and reads `server.status`; it never attaches to a session or changes the server.

- **Rex servers** show the endpoint that actually answered. On API 12, selecting
  one opens **Connect to Rex**, showing each reported listener's configured
  endpoint, actual bound addresses, transport and admission rules before
  **Connect**. Bound addresses are server facts, not proof that an alternate
  address is reachable or admits you; only the checked endpoint is selected.
  Unknown rules are shown verbatim. Revision 10/11 servers, empty listener lists
  and failed status reads say **listener details unavailable** and retain
  one-press attachment.
- Any other machine, online or offline, opens a form with its name, an **SSH
  user** (your user name is offered) and an optional **Rex socket**, and connects
  as `ssh://user@<machine>[/socket]`. The form says what the check found: nothing
  listening on those ports, or that it couldn't tell (a timeout or a certificate
  this client doesn't trust), in which case an `https://` endpoint can still be
  entered by hand. It also says whether Tailscale SSH is on there, in which case
  the tailnet's policy signs the login in with no keys; otherwise your SSH keys
  and configuration apply.
- A machine already added stays listed, marked **Added** (its tooltip names the
  saved hosts that reach it); pressing it adds another login over SSH (another
  user, or another socket). The same endpoint is never added twice.
- **Enter an endpoint…** takes any endpoint by hand, as before. Without Tailscale,
  the list says so and this is what remains.

## SSH hosts

An `ssh://` host is reached by forwarding the Rex server's Unix socket on that
machine to a local one with `ssh -L` (OpenSSH's Unix-socket forwarding, so only
`sshd` is needed there). Unless the endpoint names the socket
(`ssh://user@host/run/rex/server.sock`), the client first asks the machine where
it is: the path `rex server status --autostart=false` prints, then the login
user's own server (`$XDG_RUNTIME_DIR/rex/server.sock` on Linux,
`~/Library/Application Support/rex/server.sock` on macOS), then a shared server's
`/run/rex/server.sock` or `/var/run/rex/server.sock`. If none is a socket, a
read-only lookup reports the login, operating system, numeric uid and whether Rex
is present on `PATH` or at `~/.local/bin/rex`. Windows clients do not support
`ssh://` yet; add an HTTPS endpoint instead.

`ssh` runs with `BatchMode=yes` (it never prompts; use an agent for keys with
passphrases), `StrictHostKeyChecking=yes` (unknown and changed keys are refused),
keep-alives and `ControlMaster=no` (a
shared connection in your SSH configuration is not used, since it would keep the
forward after this client lets go). The local end lives in a directory of this
process's own under `$XDG_RUNTIME_DIR/rex-rune` (on macOS, `rex-rune-<uid>` in
your own temporary folder), which must be yours alone or the forward is refused,
so two instances (or a probe alongside the app) forwarding the same machine don't
disturb each other, and it is removed when the forward ends; directories left by
a crashed instance are swept on the next forward. When the forward drops, the host
reconnects through a new one like any other disconnect, and the reason `ssh` gave
(for example "tailnet policy does not permit you to SSH as user …") is shown next
to Retry. Edit host… shows the `ssh://` endpoint, where the user name and socket
can be changed.

Ordinary attachment uses socket forwarding and never asks Rex to auto-start a
server. No local Rex executable or Tailscale is required for SSH attachment; your
OpenSSH configuration and aliases still apply.

### SSH agent

On Linux and macOS, **Settings → Remote Hosts → SSH** lets you choose which SSH
agent `ssh` uses for `ssh://` hosts (saved as `ssh.agent` in `workspace.json`,
omitted when set to **System default**):

- **System default** inherits the process's `SSH_AUTH_SOCK` untouched.
- **Detected agents** lists known agents whose sockets exist on this machine
  (Linux: 1Password, Bitwarden including Flatpak/Snap, `gpg-agent`,
  `gcr-ssh-agent`, GNOME Keyring, OpenSSH/systemd user agent, `yubikey-agent`;
  macOS: Secretive, 1Password, Bitwarden, `yubikey-agent`, `gpg-agent`), even
  when `SSH_AUTH_SOCK` already points to one so you can pin it for desktop
  launches.
- **Custom socket…** accepts an absolute (`/…`) or home-relative (`~/…`) socket
  path without `..` segments or control characters, and can be saved even before
  the socket exists on disk.

Opening **Remote Hosts**, picking an agent, or pressing **Refresh** probes the
configured and detected Unix sockets off the UI thread using
`SSH_AGENTC_REQUEST_IDENTITIES` and reads only the 9-byte reply header to show
the public key count; key blobs, fingerprints and comments are never read or
stored. Rex Rune sets `SSH_AUTH_SOCK` in child `ssh` and SSH setup environments
only (never `-o IdentityAgent`), so `IdentityAgent` in `~/.ssh/config` still
takes precedence for hosts it names. When a chosen or refreshed agent answers
with at least one key, disconnected `ssh://` hosts that failed SSH
authentication retry immediately while already-connected hosts stay untouched.
On Windows, the SSH section is omitted and any saved `ssh.agent` value is
preserved untouched on disk.
Discovery uses `server.status` listener metadata after a documented tailnet
endpoint answers; it never guesses alternate reachable addresses from wildcard,
loopback or Unix bindings. Built-in Tailscale is not in the server's listener
list.

The SSH user is the login on that machine, which is also the user whose Rex server
is forwarded; it is not a Rex account in the sense of Rex's login server, which is
chosen in an `https://user@host` address instead.

## Check the host key

For an unfamiliar key, use **Check host key…** in Host unavailable, the host menu
or Commands. Compare the displayed SHA256 fingerprint with the machine itself
or ask its administrator. **Not now** is the default. **Trust this key** appends
the one key you saw (ED25519 preferred, then ECDSA, then RSA) to the first
`UserKnownHostsFile`, respecting `HostKeyAlias` and `HashKnownHosts`. It doesn't
rewrite old keys or change your configuration. OpenSSH checks trust before the
append and again afterwards; the client requests a reconnect only on success.
A changed key or a verification failure without an explicit unknown-key
diagnostic has no Trust button. Verify the change independently and remove
the old key yourself if the machine was legitimately reinstalled.

For `ProxyJump`, `ProxyCommand`, or an unsupported `UserKnownHostsFile` list or
missing custom parent directory, connect once with `ssh` in a terminal to check
and trust the key. Already-trusted proxied hosts can still use Start and Install.
In-app trust also refuses a symlinked known-hosts file. `ssh -G` loses the quotes
around paths; do not use in-app trust
if a quoted known-hosts path includes a space followed by `/`, which can look
like the start of a second path. Setup also refuses an SSH configuration with
`StrictHostKeyChecking no`; Rex makes its own SSH calls during setup.

## SSH setup

If SSH works but finds no default socket, the same three surfaces offer:

- **Start personal server…** if Rex is installed. The preview defaults to
  **Not now**. Explicit confirmation checks for preview.14/API 12 locally and
  remotely, then requests one native start; Rex uses `rex server start` as the
  login user. The server keeps running after the app closes, until it is stopped
  or the host restarts. On Linux with systemd, logging out also ends it and its
  sessions unless lingering is already enabled (`loginctl enable-linger`). Start
  does not enable lingering or install packages or services. Closing a busy
  Start dialog doesn't cancel that one attempt.
- **Install Rex on this host…** only for a non-root Linux login without Rex.
  It previews `rex --color never server install <endpoint>` from this computer
  (local preview.14/API 12 or newer), without `--yes`. You see Rex's output and
  answer each recognized question separately; **No** is the default. Yes can
  place Rex in `~/.local/bin` without sudo, enable the systemd user socket, turn
  on lingering, or add the host to the local Rex CLI. Use **Stop install** or
  close the live dialog to stop; a step already under way may finish or stop
  part-way. If a question isn't recognized, stop and copy the command to run it
  yourself. The dialog keeps the transcript even on success.

No setup is offered for failed authentication, transport failure or a missing
explicitly named socket. Install is manual for root, for macOS, FreeBSD and
Windows targets, and for Linux when Rex is already installed: choose the
appropriate server setup for the machine, then attach to its existing socket
or HTTPS listener. On a Linux host with systemd, run `rex server install
'ssh://user@host'` yourself to configure socket activation and lingering.
Setup never creates a session or retries a mutation. After an install that
approved at least one step, the client requests a fresh read-only connection
check if the saved host still needs one.

## Remote control

Each saved host has a **Remote control** switch, off by default, including local
hosts. The view-only actions and targeted tab-mute action it permits, their
five-second and typing-pause rules, and the refusal list are in
[Remote control](shortcuts.md#remote-control).

## When a host disconnects

A transport failure pauses input on affected panes; input is never replayed.
Parsed output survives reconnects and session changes. TCP connections are probed
when idle, so a server that dropped this client while the network was gone (a
laptop asleep, for example) is noticed within about 30 seconds and the host and
its terminals reconnect. **Reconnect terminal** in the pane menu resumes a single
paused stream.
