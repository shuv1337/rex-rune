# Configuration and settings

## Command-line options

```sh
# Create a persistent session on the local Rex server.
rex-rune --new work

# Reattach to an existing session by exact label or ID.
rex-rune --session work

# Attach to a remote server (for example one exposed over Tailscale).
rex-rune --endpoint https://my-host.example.ts.net --session work

# Attach to the Rex server on another machine through its SSH login.
rex-rune --endpoint ssh://me@my-host.example.ts.net --session work

# Choose a terminal font and size.
rex-rune --session work --font 'JetBrainsMono Nerd Font' --font-size 15
```

| Option | Meaning |
| --- | --- |
| `--endpoint <URL>` | `unix:///absolute/path.sock`, `https://…`, `http://…`, `wss://…`, `ws://…` or `ssh://[user@]host[:port][/socket]` (see [Hosts](hosts.md)). Defaults to `$REX_SERVER`, then `unix://$XDG_RUNTIME_DIR/rex/server.sock` (or `unix:///run/user/<uid>/rex/server.sock` if `XDG_RUNTIME_DIR` is unset); on macOS, `unix://$HOME/Library/Application Support/rex/server.sock`. On Windows, set `REX_SERVER` or name a network endpoint; the default local placeholder reports that Unix sockets are unavailable. |
| `--session <LABEL\|ID>` | Select a session by exact label or ID. Missing or ambiguous names never pick an arbitrary session. |
| `--new <LABEL>` | Explicitly create a persistent session (conflicts with `--session`). Ordinary attachment never creates or destroys sessions. |
| `--block <ID>` | Initially focus this terminal block. |
| `--font <FAMILY>` | Override the saved terminal family for this launch. Fonts are resolved through the system's font list (fontconfig on Linux, Core Text on macOS, DirectWrite on Windows); an unavailable primary family uses the platform's monospaced font (SF Mono, else Menlo, on macOS; Cascadia Mono, else Consolas, on Windows) or else bundled IBM Plex Mono, and reports it on stderr. |
| `--font-fallback <FAMILY>` | Override the saved fallback list for this launch. Repeat for an ordered list. System script/symbol fallbacks remain available. |
| `--font-size <PT>` | Override the saved terminal size (8–48 pt, initially 15). |
| `--config <PATH>` | Use a separate workspace preferences file. |
| `--window-size <WxH>` | Override the saved startup size (default: the last non-maximized size, else `1440x900`); tiling compositors may override it. |
| `--check-update` | Verify the latest signed release manifest, print channel-specific upgrade guidance, and exit without starting the UI. |

Without `--session`, the workspace restores the saved host and session; on a new
host it selects the first listed session. Rune's own developer flags go after `--`
(for example `-- --snap out.png`). `REX_FRAME_STATS=1` prints per-frame CPU
percentiles.

## Files and preferences

Preferences are stored as JSON in `$XDG_CONFIG_HOME/rex-rune/workspace.json`
(default `~/.config/rex-rune/workspace.json`; on macOS
`~/Library/Application Support/rex-rune/workspace.json`). The file holds:

- saved hosts (label, endpoint, collapsed state, optional default shell), the last
  host and session per host, whether the sidebar is open and its width, and the
  last non-maximized window size in points (`window_size`, saved after resizing
  settles for one second);
- tab position (`vertical_tabs`), pane density (`pane_density`: `"compact"` or
  `"comfortable"`, the default), pane headers (`pane_header`: `"auto"`, the
  default, `"always"`, `"on_hover"` or `"hidden"`; an unknown value is Auto),
  horizontal pane padding (`pane_padding`: 0–32 pt, default 8 pt) and optional
  vertical padding (`interface.pane_padding_vertical`; absent follows the legacy
  horizontal value on all four sides);
- terminal font family (`font_family`), ordered fallback families
  (`font_fallbacks`), font size, the chrome palette (`chrome_theme`:
  `"midnight"`, `"rose_pine"`, `"rose_pine_moon"` or `"gruvbox_dark"`; an unknown
  value is Midnight), the terminal theme (`terminal_theme`: `"follow_server"`,
  `"builtin_default"` for Rex's own colours, `"follow_chrome"`, or
  `"named:<name>"` with `rex/midnight`, `rex/rose-pine`, `rex/rose-pine-moon`,
  `rex/gruvbox-dark` or a Ghostty theme's file name), whether you have picked a
  terminal theme yourself (`terminal_theme_chosen`) and whether this client has
  ever declared one (`terminal_theme_declared`);
- interface zoom (`ui_scale`), the corner notices appear in (`notice_position`),
  the activity and notification choices (`notifications`), the Activity popup's
  last view (`activity_view`: `"session"` or `"priority"`, the default, omitted
  when saved), the tabs muted on this client (`muted_tabs`), automatic update
  checks (`updates.check`, default `true`), automatic installation for eligible
  install.sh installs (`updates.install_automatically`, default `false`), and
  whether agents may name sessions (`agent_session_naming`).

Update check state and dismissed versions are stored in
`$XDG_STATE_HOME/rex-rune/update.json` (on macOS,
`~/Library/Application Support/rex-rune/update.json`). Use `--config <path>` to
keep a separate workspace. Shortcut overrides live in `shortcuts.json` and are
described in [Configuring shortcuts](shortcuts.md#configuring-shortcuts).

## Settings

Settings (the gear button in the toolbar, `Ctrl ,`, or Settings… in Commands) has
these pages:

- **Server**: facts last reported by the active host, running duration while
  connected, confirmed **Stop server…**, credential-free address copy and **Pair**
  beside an available Tailscale HTTPS address; [scope and
  evidence](parity-2026-11/M3.3-server-settings.md).
- **Interface**: chrome theme, zoom, toolbar visibility, pane density, pane
  headers, active-pane outline, header fading, sidebar, tab position, tab icon
  count, tab fill, download folder and notice corner.
- **Terminal**: font picker, style, size, Advanced font settings and ordered
  fallbacks, pane padding, theme, cursor, selection, sequence hints, each host's
  default shell and generated titles.
- **Shortcuts**, **Integrations** (Rex's skill and hooks for coding agents on this
  machine), **Notifications** (activity in tabs, desktop notifications, optional
  sounds and which program changes notify), **Remote Hosts** (add, edit or remove
  saved hosts), **Updates** and **About**.

A host's name, endpoint and default shell can also be edited from its sidebar
header's right-click menu, from Edit host… in Commands, or from an offline
session's menu. Removing a host only stops this client connecting to it; its
sessions keep running on the server. A changed endpoint connects to that server as
a new host. How hosts are added is described in [Hosts and connections](hosts.md).

**About** shows the app icon, version and how it was installed, the build commit
and date, and for the active host, when connected, its Rex server's version, the
negotiated API revision and the range of revisions the server supports. The
server's version comes from one read-only `server.version` request on each
connection (its status arrives with the session list); nothing is attached,
created or changed, and a server that doesn't answer is shown as not reported
rather than guessed. **Copy version info** copies the client version, commit,
build date, build channel (`official`, `nix` or `source`), OS and architecture,
and the server version and API revisions. **Copy debug info** adds the install
channel, display server, desktop, interface theme and zoom, terminal font, and for
each saved host its kind (Unix socket, HTTPS, SSH), connection state, API revision
and server version. Both are in Commands too, and neither contains a host name,
address, session name, client id or credential. Open the repository, this
version’s release notes, the MIT license or the issue tracker from the same page.
Source archives without Git show `unknown` for build metadata.

**Stop server…** in Settings → Server and Commands (`host.stop_server`, unbound
and local-only) is offered only while connected through `unix://` or `ssh://`.
**Stop server ends every program in the server’s sessions**, disconnects other
clients, and Rex Rune will not start it again. The confirmation names the host and
the last reported session count, with **Cancel** as the default. Confirmation
sends one `server.stop` without `if_idle`; failures are shown and never retried.
Ordinary reconnect only tries to attach. **Running for** uses the reported start
time and this client’s clock (`now`, whole `m`, `h`, `d`), refreshes locally once a
minute only on this page, and disappears while disconnected or when the time is
unusable. [Checks and limits](parity-2026-11/M3.3-server-controls.md).

**Pair device…** in Commands (unbound by default) opens the same local-only dialog
as Server → Pair. It shows a black-on-white QR code and, under it, exactly what the
code holds: the sanitized, last-reported HTTPS address (a bare origin without its
trailing slash, as Rex for macOS's code is), only while that host is connected and
reports HTTPS available. The other device must be able to reach that address, for
example on the same Tailscale network; this is not a reachability check, token
exchange or server setting. Addresses over 213 bytes remain visible as text
without a code. The encoder and 1×/2× headless dialog captures are checked with
independent tools, not an actual phone or Rex accepting a host on another device.
[Verification and limits](parity-2026-11/M3.3-pair-device.md).

**Search Settings** finds pages and labelled settings by name, helper or section
(for example `padding` or `download folder`). Up/Down selects a result; Enter or a
click opens it, scrolling to and briefly marking a matching row. Typing on the
section list starts a search too. Back/Forward, `Alt Left/Right` and mouse
Back/Forward revisit Settings pages at their previous scroll positions; they do
not navigate the workspace behind the dialog. `Alt Left/Right` are fixed Settings
navigation keys while the dialog has focus, not remappable workspace actions.
Escape clears the active search before closing. The separate Shortcuts filter
still searches actions and key chords; Search Settings links to that page rather
than filtering its actions. Search and history start fresh each time Settings
opens.

## Interface

**Toolbar visibility** in Interface settings (`interface.toolbar`) offers **Always
show** (`"always"`, the default and fallback for unknown values), **Show on
hover** (`"on_hover"`) and **Hidden** (`"hidden"`). The latter two give the
toolbar's 52 pt row to the terminal area. Show on hover floats it after resting on
the top 8 pt for 80 ms and hides it 320 ms after leaving; floating never moves or
resizes terminal content. **Reveal toolbar** (`Ctrl Alt Shift T`,
`client.toolbar.toggle`, also in Commands and searchable Shortcuts) pins it over
the terminals until run again, without saving or changing server layout. It cannot
be invoked remotely. Downloads and pending-sequence chips float when the toolbar
is absent, with the same narrow-window priority as before. Settings (`Ctrl ,`) and
Commands (`Ctrl Shift P`, including Close app) remain keyboard recovery paths;
close a panel with Escape first. Your window manager's configured Move and Close
keys still work with no titlebar visible. Bindings remain removable/rebindable.
[M2.7 scope and checks](plans/m2.7-toolbar-visibility.md).

In Interface settings, **Pane density → Compact** removes outer borders, shadows,
rounded corners, gaps and outer margins, and, unless Pane headers says otherwise,
pane headers. One straight separator remains at each split boundary. Split
placement and divider dragging stay available, and reconnect controls float over a
pane that has no header. Pane actions remain on right-click or `Ctrl Shift Space`;
per-pane activity words and progress bars sit in the header, so they show only
where there is one, and tab activity remains. **Comfortable** is the default and
keeps pane cards. A single, unzoomed pane stays edge to edge in either mode, unless
Pane headers is Always; in Comfortable it has its header row across the top (8 pt
down while the toolbar is hidden or shown on hover, clear of the edge that reveals
it).

**Active pane outline** in Interface offers Auto (the existing density-following
default), On and Off. Auto outlines Comfortable cards, not Compact panes. On also
outlines the focused Compact pane in a split; a single edge-to-edge pane has no
outline. This changes only chrome, never terminal colours or opacity. Saved as
`interface.highlight_active_pane`: absent/null for Auto, `true` for On, `false`
for Off. Padding controls in Terminal edit one axis without changing the other;
their first use pins vertical padding so subsequent horizontal edits are
independent. Old files keep their uniform-padding semantics until then. Only
already-owned panes report changed sizes; preferences never claim ownership or
change the server's splits. See [partial M2.7 scope and
evidence](parity-2026-11/M2.7-panes.md).

**Fade inactive pane headers** and **Fade headers in inactive windows** in
Interface → Panes are both off by default. The first fades names and available
buttons on panes without the keyboard; the second fades them on every pane while
the native window is unfocused. They combine without extra dimming and apply only
where a header is already visible. Hover restores the name and available icons;
disabled icons keep their existing cue. Activity words, progress, exceptions, the
Zoomed chip and terminal content are unchanged. Saved as
`interface.fade_inactive_pane_headers` and
`interface.fade_pane_headers_in_inactive_window`; no server request is sent by
either preference. Native focus-in still has the existing visible-pane ownership
behavior. See [header-fading checks and
limits](parity-2026-11/M2.7-header-fading.md).

A pane's header shows its program's icon, then its name, a `Zoomed` chip while it
fills its tab, its program's activity, and four buttons: Split right, Split down,
Zoom (or Restore) and Close. The buttons act on that pane, focused or not; Close
asks first, naming the pane, tab and host. Zoom is dimmed on the only pane of a
tab, and the header's Close is dimmed on the last pane of a session (one tab with
one pane), as in Rex for macOS; the pane menu, Commands and the tab's menu still
close it. **Pane headers** (`pane_header`) chooses where headers are: **Auto**
(`"auto"`, the default) shows them on every pane in Comfortable density, a single
pane included, and nowhere in Compact; **Always** (`"always"`) on every pane in
either density, including a single pane, which then is a card; **On hover**
(`"on_hover"`) floats a pane's header over its top edge while the pointer rests
there, without moving or resizing the terminal; **Hidden** (`"hidden"`) never.
Reveal pane headers in Commands (`client.pane_headers.toggle`) floats the header
on every pane that has none until you run it again; it is not saved. The icon is
that pane's own program, from the same mapping and cached metadata as its tab's,
drawn before the name at 13 pt in a 15 pt slot with tabs' 8 pt gap; it follows
Show tab icons (`interface.tab_icon_count`, where 0 hides it) and Use colour for
tab icons, and fades with the name where header fading is on, coming back under
the pointer. A header too narrow to hold it beside its name and the buttons, the
activity word, the `Zoomed` chip and the reconnecting strip it draws keeps its name
and buttons instead, so the glyph never pushes Close out of its pane or Retry over
the buttons. It is chrome only: the header row stays 30 pt, so no terminal moves
or resizes. See [checks and captures](parity-2026-11/pane-header-icon.md).

In Interface settings, **Tab position → Sidebar** lists each session's tabs under
it in the sidebar, with their program icon, label and activity word, and takes the
tab strip out of the toolbar while the sidebar is open; closing the sidebar brings
the strip back without changing the setting. Choosing Sidebar opens the sidebar.
Clicking a tab selects its session and window, and Commands and the keyboard
shortcuts then act on it as usual. To list the tabs of sessions this client hasn't
attached, it reads their layouts with read-only `session.view` requests, one at a
time, for expanded hosts while the sidebar shows tabs; it never attaches them or
opens their terminals, and a failed read is retried after 1, 5 and then every 30
seconds. **Top** keeps tabs in the toolbar.

**Show tab icons** in Interface → Sidebar and tabs saves a count from 0 to 5
(`interface.tab_icon_count`, default 1), with a small live preview. Zero removes
icons and their spacing; additional icons show distinct existing program glyphs
from cached pane metadata, focused pane first, then server pane order. Toolbar and
sidebar tabs share the choice, and a pane header draws its own pane's one glyph
while the count is above zero. **Use colour for tab icons** chooses Always, Active
pane (only its first representative glyph in the tab on screen), Active tab, or
Never. The local `interface.tab_icon_color` defaults to `"never"`, including
absent and unknown values; other stored values are `"always"`, `"active_pane"` and
`"active_tab"`. Only existing glyphs are tinted; labels, status and terminal
output stay unchanged. Always tints every header's glyph; Active pane and Active
tab tint it only while its pane has the keyboard. The inert active/inactive
preview uses the same renderer. Shell-theme styling remains deferred. See [count
checks](parity-2026-11/M2.8-tab-icons.md) and [colour checks and
limits](parity-2026-11/M2.8-tab-icon-colours.md).

**Tabs fill available space** in Interface → Sidebar and tabs is off by default
(`interface.tabs_fill_available_space`). When enabled, toolbar tabs share the
available track width, retaining overflow scrolling and at least 48 pt of window
drag space. Sidebar rows keep their layout; the saved choice applies whenever tabs
return to the toolbar. See [partial M2.8 checks and
limits](parity-2026-11/M2.8-tab-fill.md).

## Themes

Chrome palettes (Settings → Interface → Theme): **Midnight** (the default), **Rosé
Pine**, **Rosé Pine Moon** and **Gruvbox Dark**, all dark, applied at once without
restarting. Each pairs with a built-in terminal theme of the same name whose
background is the chrome's ground. Until you pick a terminal theme yourself,
choosing a palette moves the terminal theme from Follow server to **Follow
chrome**, which declares the palette's paired theme to the server as below; a
terminal theme you picked is left alone.

Terminal themes (Settings → Terminal → Theme). **Follow server**, the default,
declares nothing and shows whatever colours the server has, as long as this client
has never declared a theme. **Rex default colours**, **Follow chrome** (the chrome
palette's paired theme), the built-in **Midnight**, **Rosé Pine**, **Rosé Pine
Moon** and **Gruvbox Dark**, or any Ghostty theme file are declared to the server
with Rex's `set_theme`, which the server applies to panes this client owns or
nobody owns; while another client owns a pane, the server keeps that client's
colours and applies ours when this client takes the pane. New tabs and splits are
created with the theme. Ghostty themes are read from
`$XDG_CONFIG_HOME/ghostty/themes` (default `~/.config/ghostty/themes`), then
`$GHOSTTY_RESOURCES_DIR/themes`, then `ghostty/themes` under each
`$XDG_DATA_DIRS` entry; the first file with a name wins. Selection and cursor-text
colours from a theme are drawn by this client. Rex keeps this client's last theme
for each pane across launches and applies it whenever this client owns the pane,
so once this client has declared any theme (remembered as
`terminal_theme_declared`, never cleared automatically), Follow server sends one
empty `set_theme`, Rex's own colours, to each pane this client owns or nobody owns:
once per pane per connection, when its stream starts, when this client takes it,
and again after a reconnect. A server without `set_theme` is reported once and the
choice is kept for other hosts.

## Terminal display, selection and fonts

Headed cards include steady typography, cursor and selection previews. Cursor
style chooses Block, Bar or Underline as the local default; explicit program
requests win unless **Force style** is on. **Copy on selection** copies only a
completed pointer selection, on release. **Primary selection** additionally owns
the desktop's PRIMARY selection and pastes it on a middle click, never during a
drag or over a mouse-reporting program. Both copy options default off. Selection
foreground/background each offer Theme, Cell foreground and Cell background; equal
cell-derived colours use the cell's other colour for legibility. Rounded corners
are opt-in, preserving existing square selections. **Show key sequence hints**
controls both toolbar and floating chips without changing key dispatch. On macOS,
**Option key** chooses whether Option, or only the left or right one, acts as Alt
or types characters ([Shortcuts on
macOS](shortcuts.md#shortcuts-on-macos)). These preferences are saved under
`terminal`; existing top-level font and `generated_titles` keys are unchanged.
Nothing changes ordinary output colours, server layout or ownership. X11 clipboard
acceptance is verified; Wayland PRIMARY depends on compositor support and is not
yet validated. The settings-file path is in About. See [M2.5 evidence and
limits](parity-2026-11/M2.5.md).

**Fonts:** in Terminal settings, **Terminal font → Choose…** opens an anchored
searchable picker, with **Monospaced | All fonts**, a current check and a sample of
the highlighted requested face (missing glyphs can fall back). List rows keep the
UI font; the selected-family sample requests that family's face, so opening the
picker does not load every installed font. The catalog of installed families
(fontconfig's on Linux, Core Text's on macOS) is read once per launch, on a thread
of its own; without fontconfig on Linux the bundled families remain available.
Monospaced excludes proportional and mixed-spacing families; installed dual-width
CJK families count as fixed-pitch. **Font style** chooses the face of ordinary
text. **Advanced font settings** has line height (−20% to +100%), character
spacing (−3 to +12 points, or −30% to +100%), program bold/italic rendering, and an
ordered fallback list with Add, Move up/down and Remove. The new subpage
participates in Settings search and Back/Forward history. Family changes take
effect when you reopen the client; they do not restart the server or its sessions.
The initial family is `JetBrainsMono Nerd Font`. If it (or the family you chose) is
not installed, terminals use SF Mono, else Menlo, on macOS, Cascadia Mono, else
Consolas, on Windows, and bundled IBM Plex Mono on Linux; the saved choice is kept
for when it is installed. Missing glyphs come from the named fallbacks in order,
then bundled IBM Plex Mono, then the bundled Symbols Nerd Font, then automatic
system fallbacks for other scripts and symbols; an empty fallback list uses only
the latter three. The symbols font draws Powerline and Nerd Font icons in a prompt
when the terminal font has none; its wide icons (a folder, say) extend into the
next cell, as in a Nerd Font, so prompts leave a space after them, and Powerline
separators are a little shorter than the row. The launch font stack resolves only
the families you name; picker samples lazily load the highlighted family through
Rune, not every installed font file. Font files are memory-mapped rather than
copied. Linux discovery of the automatic fallbacks includes fontconfig's fonts,
including fonts installed through Nix. Fallback faces supply missing glyphs
without changing the primary font's grid metrics; install a suitable CJK font,
such as Noto Sans CJK, for Chinese, Japanese and Korean. `--font` and
`--font-fallback` take precedence over saved family choices without replacing
them, so remove those flags from launchers when you want Settings to control the
family. Saved-versus-active help explains pending changes and launch overrides.
Style and metric preferences apply live to the grid, previews, sprites, cursor,
selection, find and mouse coordinates; only owned panes send new sizes, with no
ownership claim or split-layout change. Defaults preserve the existing metrics and
output. New values live under `terminal.font`; `font_family`, `font_size` and
`font_fallbacks` stay top-level. See [M2.6 checks and
limits](parity-2026-11/M2.6.md).

A host's default shell (empty means the server user's login shell) is used for
every new session, tab and split this client creates on that host. New tabs and
splits start in the source pane's shell directory on that server: a new tab or
split uses the focused pane; a pane-menu split uses the pane it names. The client
asks the server for the shell's process directory, so no OSC 7 or local path
lookup is needed; a foreground editor's directory is not used. If the read is
unavailable or takes over one second, creation uses the server's default
directory. Rex falls back to its home directory if the inherited directory no
longer exists. New sessions keep their existing default.

## Integrations, notifications and sounds

Settings → Integrations installs, updates or removes Rex's skill and session hooks
for Claude Code, Codex, OpenCode, Pi and OMP on this machine with the local `rex
skill` command. OMP reads the shared `~/.agents/skills/rex` skill used by Codex,
OpenCode and Pi; it does not have a Rex session hook. `rex skill` has no OMP agent
yet, so until it does, OMP's row changes Pi's skill: installing or removing it for
one does the same for the other. Each agent's row shows its state as a chip
(Installed, Update available, Not installed, Not found, Conflict or Unreadable)
beside the actions that state allows, and a skill several agents share is noted
once under the list. Every change is previewed with a dry run first, including
every agent affected by that shared skill. "Let agents name sessions" is passed to
every setup explicitly.

Settings → Notifications holds the activity-in-tabs switch, per-kind desktop
notification switches, the mute rules and optional sounds. Its desktop-service
section checks the platform's notification service on page open and offers **Send
test notification**; see [notification quality](parity-2026-11/M3.4-notification-quality.md).
On macOS it reports the bundle and permission state in macOS's own words; see
[the M11 record](parity-2026-11/M11-macos-notifications.md). Optional sounds are
described in [sounds](parity-2026-11/M3.4-sounds.md).
