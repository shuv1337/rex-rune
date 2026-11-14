# Hosts and connections

## Attachment-only by default

Attaching and reconnecting never start, stop, replace or reconfigure a Rex server
and do not need a `rex` executable. A missing default server on an SSH host offers
a separately confirmed, one-time **Start personal server…** action; it never
enables automatic starts on future connections. Settings → Integrations runs local
`rex skill` with `--autostart=false`. Both explicit CLI features search `PATH`,
then `$XDG_BIN_HOME` or `~/.local/bin` for desktop launches. An unavailable server
is shown as disconnected and retried. Closing the client leaves server programs
running.

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
`/run/rex/server.sock` or `/var/run/rex/server.sock`; "No Rex server found on …
for …" names those places when none is a socket and suggests a manual
`rex server install 'ssh://[user@]host[:port]'` (preview.14 or newer) for a Linux
host. That command asks before installing or configuring anything; the app never
runs it.

`ssh` runs with `BatchMode=yes` (it never prompts; use an agent for keys with
passphrases), `StrictHostKeyChecking=accept-new` (a machine's key is trusted the
first time and a changed key is refused), keep-alives and `ControlMaster=no` (a
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

Ordinary attachment keeps using socket forwarding and **never auto-starts** a
server. The separate, confirmed setup action below can invoke the Rex CLI's
native `-S ssh://host` auto-start once. No local Rex executable or Tailscale is
required for SSH attachment; your OpenSSH configuration and aliases still apply.
Discovery uses `server.status` listener metadata after a documented tailnet
endpoint answers; it never guesses alternate reachable addresses from wildcard,
loopback or Unix bindings. Built-in Tailscale is not in the server's listener
list.

The SSH user is the login on that machine, which is also the user whose Rex server
is forwarded; it is not a Rex account in the sense of Rex's login server, which is
chosen in an `https://user@host` address instead.

## SSH setup

When SSH succeeds but finds no default server, **Host unavailable**, the host's
menu and Commands offer **Start personal server…**. It is not offered for SSH
authentication/transport failures, an already connected host, or a missing
explicitly named socket.

The dialog names the host and full `ssh://` endpoint, previews the command and
explains what changes: Rex starts a personal server as that SSH login, may create
its user config/state, and leaves it running after the client closes. It creates
no sessions, installs no packages and creates no system services. **Not now** is
the default (including Enter); only **Start personal server** authorizes a single
attempt. Closing the busy dialog does not cancel that already-authorized start.

Setup requires a local and remote `rex` reporting API 12 or newer (preview.14).
Version checks run before starting. A missing or old executable, SSH refusal or
failed start is shown with the command's error and never retried automatically.
After success, the client reconnects through the original `ssh -L` path; aliases,
optional users/ports and SSH configuration still apply. No persistent auto-start
preference is saved.

The native command is `rex --autostart=true --server ssh://host --timeout 20s api
call server.status '{}'`. Using `api call server.status`, rather than `rex server
status`, matters: the latter deliberately does not auto-start even with the flag.
This read-only API call creates no session.

Installation is deliberately not automated: if Rex is missing, run `rex server
install 'ssh://host'` yourself in a terminal. Rex's installer asks before each
change and adds the host; the app neither answers its prompts nor installs
packages/services. You can instead edit the endpoint to an existing custom socket
and remain attachment-only.

## Remote control

Each saved host has a **Remote control** switch, off by default, including local
hosts. The eight actions it permits, their five-second and typing-pause rules,
and the refusal list are in [Remote control](shortcuts.md#remote-control).

## When a host disconnects

A transport failure pauses input on affected panes; input is never replayed.
Parsed output survives reconnects and session changes. TCP connections are probed
when idle, so a server that dropped this client while the network was gone (a
laptop asleep, for example) is noticed within about 30 seconds and the host and
its terminals reconnect. **Reconnect terminal** in the pane menu resumes a single
paused stream.
