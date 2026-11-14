# Building and development

Rex Rune builds from source on Linux, macOS and Windows. Published binaries are
Linux-only; macOS and Windows have no release or installer. This page holds the
complete prerequisites, platform build recipes and development commands. For
ownership of source files and deeper internals, see [AGENTS.md](../AGENTS.md).

## Requirements

- Rust 1.96 or newer (edition 2024; see `rust-version` in
  [`Cargo.toml`](../Cargo.toml)).
- [Zig](https://ziglang.org/) 0.17.x. The `ghostty-vt` crate builds Ghostty's VT
  library from a checksum-pinned source archive with Zig.
- Linux with Wayland or X11, plus the usual graphics/input libraries: Vulkan,
  xkbcommon, fontconfig and the Wayland/X11 client libraries. For macOS or
  Windows source builds, see [macOS](#macos) and [Windows](#windows).
- A running Rex server that supports API revision 10, 11, 12 or 13. Verified with
  Linux servers **0.0.1794323808-g757adf (API 13)** and **0.1.0-preview.14
  (`7e460d0`, API 12)**; see [API-13 compatibility results](server-api13.md) and
  [preview.14 results](server-preview14.md). Rex is in alpha, so older revisions
  aren't supported; update Rex on a host that refuses the connection. Uploading
  files needs revision 11.

## Linux

```sh
cargo build --locked --release --bin rex-rune
```

Optionally, [mise](https://mise.jdx.dev/) can provide the toolchain:
[`mise.toml`](../mise.toml) pins Rust 1.96.1 (with Clippy and rustfmt) and Zig
0.17.1, and `mise run build` runs the command above. Run `mise trust` and `mise
install` once first. Mise does not provide the native Linux libraries listed in
[Requirements](#requirements).

The repository is also a Nix flake: `nix build .#rex-rune` builds a package with
the desktop entry and icons, and `nix develop` gives a shell with the toolchain
and libraries. See [Nix packaging](nix.md).

Known desktop limits: physical Wayland pointer gestures (move, edge resize,
double-click, window menu), multi-seat Wayland desktops and input methods beyond
dead-key composition still need hands-on validation. On X11, winit has no
window-menu request, so right-clicking the drag region does nothing there.

[`Cargo.lock`](../Cargo.lock) pins the dependency graph. Server connections use
Rustls with the system trust roots; release metadata checks use Rustls with
WebPKI roots and verify Ed25519 Minisign signatures against the embedded release
public keys. The workspace also builds three probes (`rex-probe`,
`rex-workspace-probe`, `rex-update-probe`) that do not need the UI; use
`--no-default-features` to build only those.

### Desktop install from a checkout

```sh
bash scripts/install-desktop.sh [--copy] [path/to/rex-rune]
bash scripts/install-desktop.sh --uninstall
```

This installs the desktop launcher and icons for this checkout's binary. See
[Installing Rex Rune](install.md#desktop-launcher-from-a-checkout) for the paths,
requirements and uninstall behaviour.

## macOS

A source build only: there is no macOS release, and no release signing. Checked
on Apple Silicon with macOS 27; Intel Macs are untested.

```sh
xcode-select --install                 # the command-line tools, if they are not installed
brew install zig                       # Zig 0.17.x
rustup toolchain install 1.96.1        # or `mise install`, which also provides Zig
cargo +1.96.1 build --locked --release --bin rex-rune
python3 scripts/macos-app.py           # wraps it in target/release/Rex Rune.app
open "target/release/Rex Rune.app" --args --new work
```

- **The app bundle is what notifications need.**
  [`scripts/macos-app.py`](../scripts/macos-app.py) copies the release binary to
  `target/release/Rex Rune.app` (bundle id `dev.shuv.rex-rune`, the Rex icon, the
  version in `Cargo.toml`), signs it ad hoc (`codesign --force --sign -`) and
  registers it with LaunchServices. macOS posts notifications only for a bundled,
  signed, registered app; a bare `target/release/rex-rune` runs everything else
  and says in Settings → Notifications why it cannot post. This is a local
  convenience, not a release: nothing is notarized and no Developer ID signs it,
  and every run of the script signs the copy again. `target/release/rex-rune`
  keeps working on its own, and `python3 scripts/macos-app.py --unregister`
  unregisters and removes the bundle.
- **Notifications ask once.** The first request (opening Settings → Notifications
  is enough) shows the system's permission prompt, and until it is answered
  Settings says it is waiting. A prompt left unanswered while the app quits can be
  recorded as a refusal; turn **Rex Rune** back on in System Settings →
  Notifications then. Notifications are only posted while the client runs from
  this bundle. See [the macOS notification record](parity-2026-11/M11-macos-notifications.md)
  for what was and was not checked after permission.
- **Zig must be 0.17.x.** If Homebrew has linked another version (`zig@0.15`,
  say), put a 0.17 `zig` first on `PATH` or point `ZIG` at it.
- **The local server** is found at `~/Library/Application Support/rex/server.sock`,
  where the Rex app's server listens. `REX_SERVER` or `--endpoint` names another.
- **Settings and state** (`workspace.json`, `shortcuts.json`, `update.json`,
  `downloads.json`) are in `~/Library/Application Support/rex-rune`, not under
  `~/.config` and `~/.local/state`. SSH tunnel sockets are in your own temporary
  folder (`getconf DARWIN_USER_TEMP_DIR`), under `rex-rune-<uid>`.
- **Fonts** come from the system's own list (Core Text), so the font picker and
  `--font` see every installed family however the client is started, with or
  without Homebrew on `PATH`. SF Mono is offered under that name. If the terminal
  family is not installed (the initial `JetBrainsMono Nerd Font`, say), terminals
  use SF Mono, else Menlo; the saved choice is not changed. See
  [M7 font checks](parity-2026-11/M7-fonts.md).

What differs from Linux today:

| Area | On macOS |
| --- | --- |
| Shortcuts | On the Command key: see [Shortcuts on macOS](shortcuts.md#shortcuts-on-macos) |
| Window | The toolbar is the title bar, as in Rex for macOS: one 38 pt row with the system's close, minimize and zoom buttons at its start, then the sidebar toggle, the focused pane's program and directory (or the tabs' pills while there are several in the toolbar), drag space, the session (a press opens Switch session), Commands, New tab and Settings. A tab with one pane has no header row of its own: its buttons end the title row. Split panes keep their headers. Empty space in the row moves the window, and a double-click on it does what System Settings says for title bars. With the toolbar hidden or shown on hover, a title strip with the pane's title and buttons keeps the traffic lights clear; in full screen it goes. Edge resizing and full screen are the system's; ⌃⌘F and **Enter full screen** in Commands also toggle it |
| Primary selection | macOS has none, so the setting is not shown |
| Links and Show in folder | Opened with `open`; Show in folder selects the file in Finder |
| Sounds | Played with `afplay` from the system's alert sounds: Ping when a program needs you, Basso for an error, Glass when one is done |
| Tailnet machines in Add host | `tailscale` from `PATH`, else the one inside Tailscale.app |
| Integrations and Start personal server | `rex` from `PATH` or `~/.local/bin`, else the one inside Rex.app or Rex Beta.app |
| Ghostty theme names | Also read from Ghostty.app's own themes |
| Notifications | The system's notification centre, from `Rex Rune.app` only: a bare `target/release/rex-rune` says why it cannot post. Clicking one is meant to show its tab; that is implemented but not yet checked by hand. No urgency level. macOS asks for permission once; Settings reports when notifications are off |
| Fonts | Core Text, as above; no fontconfig. A missing family falls back to SF Mono, then Menlo |
| Downloads | The first save into `~/Downloads` can raise a macOS privacy prompt for the program that started the client; if it is refused, the download fails with a permission error |
| Updates | Settings → Updates reports a source build; nothing is installed for you |
| `ssh://` hosts, uploads, everything on the server | As on Linux |

The title row has been looked at on a Mac (macOS 27, Apple silicon) with one
pane, a split, two tabs, the smallest window, in and after full screen, with the
toolbar hidden and under the light system appearance. Full screen was entered and
left by the app, not with the green button or the menu bar that slides down over a
full-screen window, and moving the window by the row and double-clicking it are
checked by the headless tests, not by hand. While a list or dialog is open, a press
on the row closes it rather than moving the window. A window in full screen does
not save its size, so the next start opens at the last windowed size.

CI builds the app and runs the unit and headless UI tests on macOS for every pull
request. Physical Option-key presses, dead-key composition, Finder-started
acceptance and every native macOS interaction are not checked by that CI job.

## Windows

A source build only, for `x86_64-pc-windows-msvc`. There is no Windows release or
installer. Nobody has run the native client UI on Windows or checked attachment
to a real server there. CI on `windows-2025` compiles the probes and app, runs the
full unit and headless UI suite (including platform and DirectWrite font tests),
separately runs the no-UI Ghostty VT and keymap integration tests, and checks
Clippy, `--version` and `--help`. On 2026-11-14 that suite reported **993 passed,
0 failed, 4 ignored**. Headless tests do not validate a native window, real
input, clipboard/IME, display scaling or server attachment. Existing platform
exclusions remain: Unix download-store/SSH fixtures and the Linux installer do
not run on Windows; tests that need a private display, bus or server, or special
font setup remain opt-in.

Install [Visual Studio Build
Tools](https://visualstudio.microsoft.com/downloads/) with the **Desktop
development with C++** workload and its Windows SDK. Install
[rustup](https://rustup.rs/) and [Zig](https://ziglang.org/) 0.17.x (CI uses
0.17.1). Put Zig on `PATH`, then use a Developer PowerShell for Visual Studio:

```powershell
rustup toolchain install 1.96.1-x86_64-pc-windows-msvc
rustup override set 1.96.1-x86_64-pc-windows-msvc
cargo build --locked --no-default-features --bins
cargo test --locked --no-default-features --test cursor_policy --test prompt_navigation --test keymap_fixture
cargo test --locked --all-features --no-fail-fast
cargo build --locked --bin rex-rune
.\target\debug\rex-rune.exe --version
.\target\debug\rex-rune.exe --help
```

If the `ghostty-vt-sys` build script cannot download its source because TLS
verification fails, fetch and verify the archive yourself. These are the exact URL
and SHA-256 from the crate's `GHOSTTY.lock` and [CI workflow](../.github/workflows/ci.yml).
Stop if the hash does not match. Use a new, empty extraction folder:

```powershell
$archive = Join-Path $env:TEMP "rex-ghostty.tar.gz"
$source = Join-Path $env:TEMP ("rex-ghostty-" + [guid]::NewGuid())
curl.exe --proto '=https' --tlsv1.2 -fsSLo $archive https://tip.files.ghostty.org/83fb95624b0439ee660c2679b43b38c8b63769cd/libghostty-vt-source.tar.gz
if ($LASTEXITCODE -ne 0) { throw "Ghostty download failed" }
$expected = "ed57ae9b95581c88f2614e976c1d3f146c02d443a3c26139200eb9de6e645eb4"
if ((Get-FileHash $archive -Algorithm SHA256).Hash.ToLower() -ne $expected) { throw "Ghostty hash mismatch" }
New-Item -ItemType Directory $source | Out-Null
tar.exe -xzf $archive -C $source --strip-components=1
if ($LASTEXITCODE -ne 0) { throw "Ghostty extraction failed" }
$env:GHOSTTY_SOURCE_DIR = $source
cargo build --locked --bin rex-rune
```

| Area | On Windows |
| --- | --- |
| VT decoding and command-line startup | Linked and run in CI; no native window check |
| `https://`, `wss://`, `http://`, `ws://` hosts | The unchanged TCP and Rustls path compiles; real server attachment is untested |
| Local host and `unix://` | Refused with a message suggesting an `https://` host. `REX_SERVER` overrides the default; otherwise Local keeps an unsupported `unix://` placeholder, which stays in the sidebar as disconnected and is retried every few seconds, also when you start with `--endpoint`. Named pipes are not implemented |
| `ssh://` hosts | Refused before starting SSH; no loopback TCP forwarding |
| Downloads | Refused before creating a file or asking the server for a ticket; CI tests the refusal boundary, not a working download store |
| Uploads | Compile with portable random IDs; end-to-end uploads are untested |
| Folders | Settings in `%APPDATA%\rex-rune`, state in `%LOCALAPPDATA%\rex-rune`, home from `USERPROFILE`, else `HOMEDRIVE` with `HOMEPATH`. Missing AppData variables fall back to `<home>\AppData\Roaming` and `<home>\AppData\Local`. The default download folder is `<home>\Downloads`, though downloads are refused |
| Fonts | Listed and resolved through DirectWrite by the code macOS uses with Core Text; a missing family falls back to Cascadia Mono, then Consolas. CI lists and resolves Consolas through DirectWrite and runs the headless UI suite; native window rendering remains untested |
| Window chrome, input, clipboard and IME | Headless UI behavior is tested in CI; native interaction is not tested on Windows |
| Primary selection | Windows has none; the setting is not shown |
| Notifications, sounds, opening links, Show in folder | Not available yet. Opening a link or a folder fails with the system's "program not found"; Settings reports no notification service and no sound player |
| Updates | Source builds only; no installer or restart after an update |

To try a host, run:

```powershell
.\target\debug\rex-rune.exe --endpoint https://my-host.example.ts.net --session work
```

This is not a verified Windows attachment recipe.

## Development

```sh
cargo test --locked --all-features
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings

# Protocol probes against a Rex server (endpoint optional; they create and remove their own sessions).
cargo run --locked --no-default-features --bin rex-probe
cargo run --locked --no-default-features --bin rex-workspace-probe

# File downloads against a disposable server you control (creates a session there; refuses the default per-user socket).
cargo run --locked --no-default-features --bin rex-download-probe -- unix:///path/to/private.sock /path/to/workdir
```

With mise, `mise run test`, `fmt`, `clippy` and `test-rexrs` run the first three
commands and the vendored SDK tests; `mise run check` runs them all.

`python3 scripts/keymap-acceptance.py` checks layered bindings, native sequences,
Lua forwarding, queued client actions, file/server reload, removals and the
send-key escape hatch and explicit remote-action refusal on its own disposable Rex
server and Xvfb display. It uses `target/release/rex-rune` unless `REX_BIN`
selects another build, and preserves logs/captures in the printed temporary
directory.

`python3 scripts/shortcut-recorder-acceptance.py` exercises the row recorder
through native X11 input on another private server: capture isolation, precedence
above server/file bindings, default removal, restart persistence, sequences and
removal. It also honors `REX_BIN` and preserves its artifacts. See the
[M4.4 verification record](parity-2026-11/M4.4.md) for headless and rendered
checks.

The `scripts/` directory holds integration checks; each script's docstring lists
its requirements (Xvfb, xdotool, a `rex` binary, `wayland-info`, …).
`x11-smoke.py`, `rune-acceptance-x11.py` and `wayland-seat-smoke.py` use a server
you start: they require `REX_SERVER` to name a disposable one (autostart is off,
and they exit rather than fall back to the default server); every other script
starts its own private server. Every script that launches the client honours
`REX_BIN`, and runs it with a `tailscale` that finds no tailnet and no update
check. X11 checks use an isolated display, and those that select text by pointer
pin the pane padding in their private `workspace.json`, so pointer positions come
from the layout they set rather than the app's default. Of the Wayland scripts,
`mouse-check.py`, `readme-screenshot.py` and `demo-record.py` start a private
headless compositor; `wayland-seat-smoke.py` opens its window on the compositor
`WAYLAND_DISPLAY` names, so point it at a disposable one.

- `connection-only-smoke.py`: a missing or stale socket must never execute the Rex
  CLI.
- `ssh-setup-smoke.py`: the real opt-in setup UI against a private localhost sshd
  and preview.14 executable, with isolated HOME/XDG state and SSH config. Checks
  attachment-only, Not now, confirmed native start, zero created sessions and
  subsequent `ssh -L` attachment; stops its server and sshd by PID. Requires
  explicit `REX_CLI` and `REX_BIN`; no installation or real host setup is
  performed.
- `download-acceptance.py`: file downloads through the built binary and the real
  UI, on its own private API 13 and API 12 servers, a private Xvfb and (when
  `sshd` can run as this user) a private SSH host. Covers names with spaces and
  Unicode (and, through the core library only, a newline), a leading dash, `~` and
  relative paths, a symlink, a folder and a FIFO refused, an empty file, a 1 GiB
  file cancelled partway, a file growing during the transfer, the same file twice,
  the "too old" message on API 12, the fourth concurrent transfer, low space (a
  size-limited tmpfs in a private user namespace), one transfer over `ssh://`, and
  quitting or killing the app mid-transfer; it checks every saved file's SHA-256,
  that nothing is executable, that a stub `xdg-open` is never called and that no
  download row offers **Open** (a finished row has **Show in folder** only). It
  also checks that the server holds the 1 GiB question's file until Escape
  releases it. No case presses **Show in folder**, so the `xdg-open` stub only
  proves nothing opens a file by itself. Prints PASS, FAIL or SKIP per case; a
  skipped case (no API 12 `rex`, `sshd`, `unshare` or probe) does not fail the run
  unless `--strict` is given, which is the form to use for a release (it also
  requires at least 15 saved files to have been compared). The artifact folder
  under `~/.cache/agent-ws` is deleted when every case passed and kept otherwise,
  or with `--keep`. Also needs the client (`REX_BIN`) and `rex-download-probe`
  built; `REX12_BIN` selects the API 12 `rex`.
  [`private_ssh.py`](../scripts/private_ssh.py) is the private sshd helper it
  shares with `remote-control-acceptance.py`.
- `navigation-x11.py`: isolated single-host private-server checks for global tab
  shortcuts, reverse wrapping and mouse Back/Forward across sessions; also
  exercises named history Commands. Cross-host navigation and reconnect behavior
  are covered by headless/protocol tests, not this script.
- `fonts-macos-private.py`: on macOS, the built client with `PATH=/usr/bin:/bin`
  (as Finder starts it) against a private Rex server and HOME: the missing default
  family drawn in SF Mono, the picker's Monospaced filter for Menlo, SF Mono, Hack
  and Meslo Nerd Fonts and Helvetica, and Menlo chosen, saved and resolved after
  reopening, with captures. Set `REX_BIN`; `REX_SERVER_BIN` names the Rex server
  binary if not the one in Rex.app or Rex Beta.app.
- `notification-macos-private.py`: on macOS, a private Rex server and HOME: the
  bare binary's Settings page says to start `Rex Rune.app`. It never launches the
  real bundle: it runs a copy with a throwaway bundle id
  (`dev.shuv.rex-rune.acceptance.<random>`), whose page must say it is waiting for
  permission and whose **Send test notification** must fail at once with that
  reason, then unregisters and deletes the copy. It never answers the system
  permission prompt, and macOS keeps a notification entry for each throwaway id.
  See the [M11 record](parity-2026-11/M11-macos-notifications.md).
- `font-metrics-x11.py`: private-server/Xvfb font-metrics acceptance: live +20%
  height/spacing, owned versus unowned resize behavior, real PTY mouse coordinates
  and synthetic grid captures. Set `REX_BIN` to the release build; never touches
  the daily server/display/clipboard.
- `process-cwd-contract.py`: private-server process-directory and
  missing-directory fallback checks without OSC 7; set `REX_BIN` to also check
  keyboard/pane-menu splits and new tabs through the real client on X11. Keeps
  logs/captures under `~/.cache/agent-ws/`.
- `pane-padding-x11.py`: private Rex/Xvfb acceptance for independent padding with
  M2.6 metrics, owned/unowned sizes, outline persistence and exact PTY mouse
  coordinates; no personal server or clipboard.
- `x11-smoke.py`: real X11 events (forms, clipboard, exact pointer copy and raw
  Kitty key isolation). Requires `REX_SERVER` explicitly set to a disposable
  server (no autostart); `REX_BIN` selects a debug or release client. Runs the
  client with its own HOME and the pane padding pinned.
- `scroll-selection-x11.py`: private-server Unicode copy checks after scrolling,
  drag autoscroll, and Shift selection in a fullscreen mouse-tracking TUI without
  breaking ordinary mouse reports.
- `kitty-graphics-x11.py`: private-server pixel checks for live RGBA, chunked PNG,
  image restoration after reattach and deletion.
- `clipboard-x11.py`: isolated private-server acceptance check for this feature;
  see its docstring.
- `alt-arrow-x11.py`: isolated private-server acceptance check for this feature;
  see its docstring.
- `sidebar-x11.py`: private-server check that the sidebar's open state survives
  relaunch, with captures of both tab positions.
- `rune-acceptance-x11.py`: window-corner, terminal-key and frame-CPU acceptance
  checks on X11. Requires `REX_SERVER` (disposable); also the shared helpers
  (`Desktop`, `grid_origin`) the other X11 scripts import.
- `chrome-spike-x11.py` (with the `chrome_spike` example): borderless window chrome
  on X11.
- `wayland-seat-smoke.py`: Wayland seat/resource selection on a multi-seat
  compositor. Requires `REX_SERVER` (disposable) and uses `REX_BIN`.
- `mouse-check.py`: terminal mouse reporting with real pointer input on a private
  headless Wayland display.
- `readme-gallery.py`: regenerates the README hero and Settings gallery on private
  X11 with real fonts and a fictional workspace. See
  [capture provenance and rerun steps](readme-captures.md).
- `readme-screenshot.py`: regenerates `docs/screenshot.png` from a private Rex
  server and headless compositor, with a throwaway home and prompt so no local
  details appear.
- `docs-captures.py`: regenerates the documentation screenshots through Rune's
  `--script`/`--snap`/`--tree` tooling, headless, against two private servers; the
  sidebar is opened by the saved config, the Add host list sees no tailnet and the
  `cargo test -q` pane prints fixed text, so reruns are reproducible.
- `demo-record.py`: records a demo video of the app on a private headless
  compositor (uses the `tools/vptr` virtual-pointer helper).

## Vendored dependencies

Three dependencies are vendored with small, documented patches and wired in
through `[patch]` in [`Cargo.toml`](../Cargo.toml):

- [`vendor/rune/`](../vendor/rune/README.md): the `rune`, `rune-motion`,
  `rune-staging`, `rune-taffy` and `procwatch` crates from `anomalyco/rune` at
  `d70929a` (MIT), with dead-key forwarding and window-chrome additions for a
  borderless Linux window (maximized state, minimize, edge resize, window menu,
  native window icon).
- [`vendor/rexrs/`](../vendor/rexrs/REX-RUNE.md): `a10y/rexrs` at `780be3f` (MIT
  OR Apache-2.0), plus revision-13 generated types, explicit API-revision range
  negotiation and preservation of an empty `attach_session_ids`.
- [`vendor/ghostty-vt/`](../vendor/ghostty-vt/REX-RUNE.md): the crates.io
  `ghostty-vt` 0.8.0 source (Apache-2.0; Ghostty itself is MIT), plus a safe
  wrapper for Ghostty's render-state image iterator, image-aware snapshot
  decoding, a fixed temporary-file medium setting and grayscale PNG normalization.

Each patch is meant to be dropped once the corresponding upstream release has
equivalent APIs. The [contributor guide](../AGENTS.md#vendored-patches) has the
rules for keeping them minimal and documented.

## Release and verification references

- [Publishing a binary release](releases.md) covers the release workflow, eight
  packages, attestations, signing and recovery.
- [Self-updates](self-updates.md) covers the in-app updater and pacman helper.
- [The docs index](README.md#verification-records) lists platform and milestone
  verification records, including the ones for this project's native checks.
- [AGENTS.md](../AGENTS.md) describes the code layout, conventions and the
  checks required for protocol and state-machine changes.
