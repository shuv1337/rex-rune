# Rex Rune

**TL;DR:** Attach to an existing Rex server and work with its sessions, tabs and splits.

Rex Rune is an unofficial, independent native Linux client for
[Rex](https://www.superlogical.com/rex), the terminal multiplexer from
Superlogical, with source builds for macOS and Windows. It is not affiliated
with, endorsed by or supported by Superlogical or the Rex authors. It is written
in Rust on [Rune](https://github.com/anomalyco/rune) (wgpu/winit, `rune-kit`
components) with libghostty-vt for terminal emulation, and talks to the server
through the [`rexrs`](https://github.com/a10y/rexrs) control SDK.

Rex Rune attaches to an existing Rex server. Saved hosts contain sessions, each
session's Rex windows become tabs, and the server defines the terminal splits.
Sessions and their programs stay on the server when the app closes, so any other
Rex client can attach to the same sessions.

![Rex Rune in the default Midnight theme, with three tabs, the sessions sidebar and a three-pane split showing a fictional project's file tree, source code and checks](docs/screenshot.png)

## Install

**Rex Rune is a desktop client, not a Rex server.** Every method below installs
only the client, its launcher and icons; it never installs, replaces or shadows
the `rex` server.

- **Linux (stable binaries, 0.10.0, x86_64 and aarch64).** Arch and Omarchy:
  `sudo pacman -Syu rex-rune-bin` from the Rex package repository. Other Linux
  systems with glibc 2.39+ can use the signed installer:
  `curl --proto '=https' --tlsv1.2 -fsSL https://github.com/shuv1337/rex-rune/releases/latest/download/install.sh | sh`.
  `.deb`, tarball, Nix, signatures, uninstall and repository setup are in
  [Installing Rex Rune](docs/install.md).
- **macOS: source build only.** Build with the Xcode command-line tools and Zig;
  an optional local ad-hoc `Rex Rune.app` wrapper enables notifications. See
  [macOS](docs/building.md#macos).
- **Windows: MSVC source build only.** CI builds and tests it; native UI and
  server attachment are untested. See [Windows](docs/building.md#windows).

Prerequisites are in [Building and development](docs/building.md); signatures
and installer caveats are in [Installing Rex Rune](docs/install.md#verify-a-download).

## Capabilities

- **Hosts, sessions, tabs and panes.** Multiple saved hosts; Unix-socket,
  HTTPS/WSS and SSH endpoints; sessions from each host become tabs. Splits open
  right, left, down, up or best fit; focus moves by direction or cycles through a
  tab's panes, and panes zoom, close, rename and swap. A single toolbar carries
  session identity, tabs and window controls; an optional sidebar lists hosts,
  sessions and tabs, or tabs can stay in the toolbar. Tabs close behind a
  confirmation, rename, mute, move in the server's order or fill the available
  space; Back/Forward revisits tabs, sessions and hosts. Pane headers show before
  their name, with per-pane activity and split/zoom/close buttons. Searchable
  session switcher (`Ctrl K`) and Commands (`Ctrl Shift P`); prompt navigation;
  generated program/directory titles; a window title of `<tab> - <session> ·
  <host>`; full screen. See [Hosts and connections](docs/hosts.md) and
  [Configuration and settings](docs/configuration.md).
- **Attachment-only connection.** Ordinary attachment, reconnecting and
  prefetching never start, stop, reconfigure or create anything on a server. A
  missing default server on an SSH host offers a separately confirmed, one-time
  **Start personal server…** action; it is never automatic. Each saved host has
  its own default shell, and new tabs and splits inherit the source pane's shell
  directory. Idle TCP connections are probed, so a dropped client reconnects;
  failed input is paused, never replayed. **Stop server…** and **Pair device…**
  are explicit, confirmed, local-only actions. See [Hosts and
  connections](docs/hosts.md).
- **A full terminal.** Ghostty decoding with live output and snapshot restore;
  physical-layout key press/release forwarding, dead-key composition and IME;
  bracketed paste; mouse and wheel reporting for terminal programs; local
  scrollback; cursor shapes with an optional local override; underline styles and
  colours; overlines; box-drawing and block characters drawn on device pixels;
  inline Kitty graphics (RGB, RGBA and PNG) with a 64 MiB image cap; synchronized
  output. Search (`Ctrl Shift F`) covers screen and scrollback, marks every match
  and keeps the bar clear of the selected match. Links open on Ctrl+click
  (⌘-click on macOS) only for `http`, `https`, `ftp` and `mailto`, with a target
  tooltip. See [Terminal, search, selection and
  clipboard](docs/terminal.md).
- **Selection, clipboard and files.** Word/line selection, drag autoscroll and
  Shift selection over mouse-reporting programs; copy on selection and opt-in
  PRIMARY ownership; OSC 52 copies from the focused pane and never answers reads.
  **Insert path…** and **Go to directory…** browse the terminal's machine and
  paste one shell-quoted path without pressing Enter, with per-host recent paths.
  Drop or paste files to upload them through Rex (API 11+). **Download file…**
  saves files from the terminal's machine over the connection (API 13+), never
  overwrites, opens or runs them, and shows progress and partial-file cleanup.
  See [Terminal](docs/terminal.md), [Downloading files](docs/downloads.md) and the
  [clipboard workarounds](docs/terminal.md#programs-that-dont-copy-through-osc-52).
- **Fonts, themes and layout.** Chrome palettes **Midnight**, **Rosé Pine**,
  **Rosé Pine Moon** and **Gruvbox Dark** pair with terminal themes the client
  declares to the server; **Follow server** keeps server colours, and Ghostty
  theme files are supported. Installed font families with a searchable picker,
  named fallbacks, bundled IBM Plex Mono and Symbols Nerd Font, live line
  height/spacing/bold/italic metrics, and per-platform stand-ins when the chosen
  family is missing. Interface zoom (70–150%), independent pane padding, compact
  or comfortable density, active-pane outline, header fading, tab icon count and
  colour, tab fill and toolbar visibility. A borderless window with its own close
  button, edge resizing and drag region. On macOS, Option can act as Alt, compose
  characters, or be split between left and right, and the toolbar is the window's
  title row. See [Configuration and settings](docs/configuration.md) and
  [Shortcuts](docs/shortcuts.md).
- **Activity and notifications.** Programs that report status with OSC 7501 mark
  tabs and panes with Needs you, Error, Done or Bell; OSC 9;4 progress draws a
  thin bar. The Activity list groups known tabs across connected hosts and
  sessions, by priority or by session, with filters, local ages and a conditional
  toolbar chip; a once-per-connection status pass covers unopened sessions, and
  sidebar and switcher rows carry a session's most pressing word. Desktop
  notifications use the freedesktop service on Linux and the macOS notification
  centre from a bundled, ad-hoc-signed, registered app; clicking one shows its tab
  (not yet checked by hand on macOS). Per-kind switches, tab mute, notification
  health checks and an optional test sound are in Settings → Notifications. See
  [notification quality](docs/parity-2026-11/M3.4-notification-quality.md),
  [sounds](docs/parity-2026-11/M3.4-sounds.md) and [macOS
  notifications](docs/parity-2026-11/M11-macos-notifications.md).
- **Shortcuts and remote control.** Layered keymaps combine built-in defaults,
  server `init.lua`, `shortcuts.json` and recorded shortcuts, with named modes,
  sequences and per-row recording. Rex action compatibility is a focused subset,
  not full Mac parity. **Remote control is off by default for every host** and
  permits exactly eight navigation actions through `rex do` and
  `client.perform`; it cannot type, access the clipboard, open dialogs, change
  zoom or cross hosts. See [Shortcuts](docs/shortcuts.md).
- **Settings and maintenance.** Settings (⌘, on macOS, `Ctrl ,` elsewhere) covers
  Server facts, running duration, Stop server and Pair device; Interface;
  Terminal; Shortcuts; Integrations (Rex's skill and hooks for Claude Code, Codex,
  OpenCode, Pi and OMP); Notifications; Remote Hosts; Updates; and About.
  **Copy version info** and **Copy debug info** omit host names, addresses,
  session names, client ids and credentials; About also shows app and server facts,
  Report an issue and Rex help. Signed update notices and checks, **Update now**
  for verified install.sh and pacman installs, and install-channel guidance are
  documented in [Self-updates](docs/self-updates.md). See [Configuration and
  settings](docs/configuration.md).

This is a focused compatibility client, not full Rex parity. Ordinary attachment
never creates or destroys sessions, and the client never invents server layout.

## Platform support

Published binaries are Linux-only. "CI" means automated build and tests, not
hands-on acceptance on every desktop.

| Area | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Published builds | Stable x86_64 and aarch64 binaries, current **0.10.0**; pacman, `.deb`, tarball, `install.sh`, Nix | Source only; no release, installer or notarization | Source only for MSVC; no release or installer |
| Native UI and input | Exercised with private X11 and Wayland fixtures, not every desktop, compositor or input method | Title row and window checked by hand on macOS 27 Apple silicon; physical Option, dead keys and Finder-started acceptance not checked | Full unit/headless suite in CI (993 passed, 4 ignored); no native window, physical input, clipboard, IME, HiDPI or server attachment check |
| Server attachment | Unix socket, HTTPS/WSS and SSH hosts | Local socket, HTTPS/WSS and SSH hosts checked from source builds | Network endpoints compile through the unchanged TCP/Rustls path; untested. Unix sockets and SSH hosts are refused; no named pipes |
| Notifications | freedesktop service over D-Bus | Notification centre only from a bundled, signed, registered `Rex Rune.app`; permission is requested once. Posting, replacement, seen-close, click and foreground banners after permission are unverified by hand | Not available; Settings reports no service |
| Sounds | Optional activity sounds through an installed freedesktop player | `afplay` and the system alert sounds | Not available |
| Downloads and uploads | API 13 downloads and API 11 uploads | Downloads and uploads as on Linux, subject to the macOS `~/Downloads` privacy prompt | Downloads refused before creating a file; uploads compile but are untested end to end |
| SSH hosts | Unix-socket forwarding with `ssh -L`, plus opt-in personal-server setup | As on Linux, with Rex found inside its app when not on `PATH` | Refused before starting SSH; no loopback forwarding |
| Updates | pacman guidance and **Update now** where the helper is installed; signed installer updates for user installs | Settings reports a source build; nothing is installed | Source builds only; no installer or restart after an update |
| Accessibility | AT-SPI tree on Linux; inspected with a client script, never with a screen reader | Not checked | Not checked |

Linux input fixtures are not a claim about every environment. The macOS app
bundle is a local convenience, not a release. Synthetic events, private servers
and CI never replace a manual check. See [Building and
development](docs/building.md) for exact platform steps and
[Accessibility](docs/accessibility.md) for known AT-SPI gaps.

## Usage

```sh
# Create a persistent session on the local Rex server.
rex-rune --new work

# Attach to an existing session by exact label or ID.
rex-rune --session work

# Attach to a remote server (for example one exposed over Tailscale).
rex-rune --endpoint https://my-host.example.ts.net --session work

# Attach to the Rex server on another machine through its SSH login.
rex-rune --endpoint ssh://me@my-host.example.ts.net --session work
```

Without `--session`, the workspace restores the saved host and session. Missing
or ambiguous session names never pick an arbitrary session. `--check-update`
verifies the signed manifest and exits without starting the UI. The full option
table is in [Configuration and settings](docs/configuration.md#command-line-options).

## Configuration

Preferences live in `$XDG_CONFIG_HOME/rex-rune/workspace.json` (on macOS,
`~/Library/Application Support/rex-rune/workspace.json`); shortcut overrides live
in `shortcuts.json` beside it. Settings covers themes, fonts, panes, tabs,
toolbar, shortcuts, notifications, hosts, updates and About. Paths, keys and
per-page behaviour are in [Configuration and settings](docs/configuration.md) and
[Shortcuts](docs/shortcuts.md). Use `--config <path>` for a separate workspace.

## Development

```sh
cargo build --locked --release --bin rex-rune
cargo test --locked --all-features
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
```

Rust 1.96+, Zig 0.17.x and the platform libraries are required. Mise and Nix
recipes, macOS and Windows builds, private-server acceptance scripts and vendored
patches are in [Building and development](docs/building.md). The code layout,
conventions and platform boundary rules are in [AGENTS.md](AGENTS.md). Release
signing and verification are in [Publishing a binary release](docs/releases.md).

## Documentation

- [Docs index](docs/README.md) — every document and verification record.
- [Installing](docs/install.md), [building and development](docs/building.md), [configuration](docs/configuration.md) and [shortcuts](docs/shortcuts.md).
- [Hosts and connections](docs/hosts.md), [terminal and clipboard](docs/terminal.md) and [downloading files](docs/downloads.md).
- [Accessibility](docs/accessibility.md), [renamed from Rex Linux](docs/legacy-rex-linux.md), [Nix](docs/nix.md), [self-updates](docs/self-updates.md) and [server compatibility](docs/README.md#server-compatibility).

## Contributing

This repository does not accept pull requests from outside contributors. If you
hit a bug, have a feature idea, or have a change on a fork branch or commit you
want considered, open an issue and link to it. Maintainers harvest changes from
forks, cherry-pick or rewrite what fits, and credit the author with a
`Co-authored-by:` trailer and in [CHANGELOG.md](CHANGELOG.md). See
[AGENTS.md](AGENTS.md) for conventions, and [DESIGN.md](DESIGN.md) /
[PRODUCT.md](PRODUCT.md) for design and product intent. The [fork harvest
record](docs/fork-harvest-2026-11.md) describes the approach.

## Third-party software and assets

| Component | License |
| --- | --- |
| [anomalyco/rune](https://github.com/anomalyco/rune) (UI toolkit; vendored crates plus `rune-kit` and `rune-theme-opencode` from git) | MIT |
| [a10y/rexrs](https://github.com/a10y/rexrs) (Rex control SDK; vendored) | MIT OR Apache-2.0 |
| [Ghostty](https://github.com/ghostty-org/ghostty) (terminal core and snapshot format, built by `ghostty-vt-sys`) | MIT |
| [`ghostty-vt`](https://crates.io/crates/ghostty-vt) Rust bindings, pinned `=0.8.0` (vendored with patches) | Apache-2.0 |
| Inter and IBM Plex Mono (bundled through `rune-theme-opencode`) | SIL Open Font License 1.1; license texts also appear in `vendor/rune/crates/rune/assets/` |
| [Symbols Nerd Font](https://github.com/ryanoasis/nerd-fonts) 3.5.1 (`resources/fonts/`, compiled into the app) | MIT; the icon sets inside it keep their own licenses, listed in [resources/fonts/README.md](resources/fonts/README.md) |
| Rex app icon (`resources/icons/`) | Superlogical artwork, not covered by this repository's license; see [resources/icons/README.md](resources/icons/README.md) |
| Program icons (`resources/program-icons/`, compiled into the app) | Superlogical's Rex icon set; the logos in it are their owners' trademarks. Not covered by this repository's license; see [resources/program-icons/NOTICE.md](resources/program-icons/NOTICE.md) |

Other crates are used under their own licenses; see `Cargo.lock`. Vendored
patches and their pinned revisions are described in [Building and
development](docs/building.md#vendored-dependencies).

## Acknowledgements

- [Superlogical Rex](https://www.superlogical.com/rex), the server and protocol
  this client speaks. The implementation follows the server's `/llms.txt` API
  revision 13 contract; revisions 10–12 are also supported.
- [anomalyco/rune](https://github.com/anomalyco/rune) and
  [a10y/rexrs](https://github.com/a10y/rexrs).
- [Ghostty](https://github.com/ghostty-org/ghostty) for the terminal core.
- [m4nik1/rex-linux-client](https://github.com/m4nik1/rex-linux-client) and
  [noahdunnagan/rmx](https://github.com/noahdunnagan/rmx), whose protocol work
  served as references.

## License

MIT. See [LICENSE](LICENSE). Vendored code keeps its own licenses in `vendor/`.
