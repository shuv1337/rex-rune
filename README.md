# Rex Rune

Rex Rune is an unofficial, independent native client for [Rex](https://www.superlogical.com/rex),
the terminal multiplexer from Superlogical. It attaches to an existing Rex
server, so sessions, panes and programs stay on the server when the app closes.
It is written in Rust on [Rune](https://github.com/anomalyco/rune) with
libghostty-vt for terminal emulation and the
[`rexrs`](https://github.com/a10y/rexrs) control SDK. Published binaries are
Linux-only; macOS and Windows build from source. Rex Rune is not affiliated
with, endorsed by or supported by Superlogical or the Rex authors.

![Rex Rune in the default Midnight theme, with three tabs, the sessions sidebar and a three-pane split showing a fictional project's file tree, source code and checks](docs/screenshot.png)

## Install

Rex Rune is a desktop client, not a Rex server: every method installs only the
client, launcher and icons, never the `rex` server.

- **Linux.** Arch and Omarchy: `sudo pacman -Syu rex-rune-bin` after the one-time
  [repository and key setup](docs/install.md#arch-linux-and-omarchy-pacman).
  Other glibc 2.39+ systems use the signed installer:
  `curl --proto '=https' --tlsv1.2 -fsSL https://github.com/shuv1337/rex-rune/releases/latest/download/install.sh | sh`.
  `.deb`, tarball, Nix, signatures and uninstall are in
  [Installing Rex Rune](docs/install.md).
- **macOS: source build only.** Build with the Xcode command-line tools and Zig;
  an optional local ad-hoc `Rex Rune.app` wrapper enables notifications. See
  [macOS](docs/building.md#macos).
- **Windows: MSVC source build only.** CI builds and tests it; native UI and
  server attachment are untested. See [Windows](docs/building.md#windows).

## Capabilities

- **Hosts and discovery.** Saved hosts over Unix sockets, HTTPS/WSS and SSH;
  tailnet discovery lists machines and, on API 12, each listener's endpoint,
  transport and admission rules.
- **Sessions and tabs.** A host contains sessions; each session's Rex windows
  become tabs. Searchable switcher (`Ctrl K`), optional sidebar, Back/Forward
  history and a `<tab> - <session> · <host>` window title.
- **Tab management.** Tabs close behind a confirmation, rename, mute, reorder in
  server order, close others or fill the toolbar; optional generated program and
  directory titles.
- **Panes.** Splits right, left, down, up or best fit; resize, balance, move to a
  tab, zoom, swap, rename, copy the pane ID, close and focus by direction.
  Headers show the pane's own program icon before its name, its activity word
  and buttons, with configurable placement.
- **Attachment only.** Attaching, reconnecting and prefetching never start, stop
  or change a server or create or destroy a session; a missing SSH server offers
  one confirmed start. Failed input pauses and is never replayed.
- **Terminal emulation.** Ghostty decoding, snapshot restore, physical-layout
  keys, dead keys and IME, bracketed paste, mouse reporting, scrollback,
  cursor/underline/overline styles, device-pixel box drawing, static Kitty
  graphics and synchronized output.
- **Search and links.** `Ctrl Shift F` searches screen and scrollback and marks
  every match; Ctrl+click (⌘-click) opens http, https, ftp and mailto links with
  a target tooltip, where the platform can.
- **Selection and clipboard.** Word and line selection, drag autoscroll, Shift
  selection over mouse-reporting programs, copy on selection, opt-in PRIMARY
  ownership and focused-pane OSC 52 copies that never answer reads.
- **Paths and uploads.** **Insert path…** and **Go to directory…** browse the
  terminal's machine and paste a quoted path, or `cd <path>`, without Enter;
  drop or paste files to upload them (API 11+).
- **Downloads.** **Download file…** saves from the terminal's machine (API 13+),
  with progress and partial cleanup, and never overwrites, opens or runs the
  file.
- **Fonts and themes.** Chrome palettes, terminal themes declared to the server
  including Ghostty files, installed fonts with a picker and ordered fallbacks,
  bundled IBM Plex Mono and Symbols Nerd Font, font size and live metrics.
- **Layout and interface.** Zoom, independent padding, density, pane outline,
  header fading, tab icon count and colour, tab fill, toolbar visibility, full
  screen, and a Linux borderless window; on macOS the toolbar is the title row
  and Option can act as Alt, compose, or differ by side.
- **Activity.** OSC 7501 marks Needs you, Error, Done or Bell; OSC 9;4 draws
  progress; Rex program icons mark tabs and headers. Activity groups by priority
  or session with filters, ages, a chip and unopened-session coverage.
- **Notifications and sounds.** Desktop notifications on Linux/BSD and macOS,
  including program-sent OSC 9 and OSC 777. Optional default-off activity
  sounds, per-kind switches, tab mute, service check and test.
- **Shortcuts.** Layered keymaps from defaults, `init.lua`, `shortcuts.json` and
  recording; Commands lists every action; Rex action compatibility is a focused
  subset, not Mac parity.
- **Remote control.** Off by default for every host; exactly eight navigation
  actions can zoom panes and switch the display to the requesting host, but none
  can type, use the clipboard, open dialogs, change interface zoom or act on
  another host.
- **Settings and maintenance.** Server facts, duration, confirmed **Stop
  server…** and a local **Pair device…** QR dialog that sends no server request;
  Interface, Terminal, Shortcuts, Integrations, Notifications, Remote Hosts,
  Updates and About, with Settings search and history. Copy version or debug
  info omits host names, addresses, sessions, ids and credentials.
- **Integrations and updates.** Rex's skill and hooks for Claude Code, Codex,
  OpenCode, Pi and OMP; signed update checks, channel guidance and **Update now**
  for eligible install.sh and pacman installs.

This is a focused compatibility client, not full Rex parity; ordinary attachment
never creates or destroys sessions or invents server layout.

## Platform support

Published binaries are Linux-only. "CI" means automated build and tests, not
hands-on acceptance on every desktop.

| Area | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Published builds | Stable x86_64 and aarch64 binaries, current **0.10.0**; pacman, `.deb`, tarball, `install.sh`, Nix | Source only; no release, installer or notarization | Source only for MSVC; no release or installer |
| Native UI and input | Exercised with private X11 and Wayland fixtures, not every desktop, compositor or input method | Title row and window checked by hand on macOS 27 Apple silicon; physical Option, dead keys and Finder-started acceptance not checked | Full unit/headless suite in CI (993 passed, 4 ignored); native window, physical input, clipboard, IME, HiDPI and server attachment untested |
| Server attachment | Unix socket, HTTPS/WSS and SSH hosts | Local socket, HTTPS/WSS and SSH hosts checked from source builds | Network endpoints compile through the unchanged TCP/Rustls path; untested. Unix sockets and SSH hosts are refused; no named pipes |
| Links and folders | `xdg-open` and the file manager | `open` and Finder | **Not available**: opening a link or Show in folder fails with the system's "program not found" |
| Notifications | freedesktop service over D-Bus | Notification centre only from a bundled, signed, registered `Rex Rune.app`; permission is requested once. Posting, replacement, seen-close, click and foreground banners after permission are unverified by hand | Not available; Settings reports no service |
| Sounds | Optional activity sounds through an installed freedesktop player | `afplay` and the system alert sounds; implementation and fake-player tests only, with no native playback or audio-receipt check | Not available |
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
in `shortcuts.json` beside it. Use `--config <path>` for a separate workspace.
Paths, keys and per-page behaviour are in [Configuration and
settings](docs/configuration.md) and [Shortcuts](docs/shortcuts.md).

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

## License and credits

MIT. See [LICENSE](LICENSE). Vendored code keeps its own licenses in
[`vendor/`](vendor/).

Rex Rune uses [anomalyco/rune](https://github.com/anomalyco/rune) (MIT; UI
toolkit, `rune-kit` and `rune-theme-opencode` from git), [a10y/rexrs](https://github.com/a10y/rexrs)
(MIT OR Apache-2.0; vendored control SDK), [Ghostty](https://github.com/ghostty-org/ghostty)
(MIT; terminal core and snapshot format) through the pinned
[`ghostty-vt`](https://crates.io/crates/ghostty-vt) bindings (Apache-2.0,
vendored), Inter and IBM Plex Mono (SIL Open Font License 1.1, via
`rune-theme-opencode`), and the [Symbols Nerd Font](https://github.com/ryanoasis/nerd-fonts)
3.5.1 (MIT; each icon set keeps its own license, listed in
[resources/fonts/README.md](resources/fonts/README.md)). Superlogical's Rex app
and program icons are their artwork and trademarks, not covered by this
repository's license; see [resources/icons/README.md](resources/icons/README.md)
and [resources/program-icons/NOTICE.md](resources/program-icons/NOTICE.md).
Other crates keep their own licenses in `Cargo.lock`, and vendored patch
details are in [Building and
development](docs/building.md#vendored-dependencies).

Thanks to [Superlogical Rex](https://www.superlogical.com/rex) for the server
and protocol this client follows (API revision 13, with revisions 10–12 also
supported), and to [m4nik1/rex-linux-client](https://github.com/m4nik1/rex-linux-client)
and [noahdunnagan/rmx](https://github.com/noahdunnagan/rmx) for protocol work
that served as references.
