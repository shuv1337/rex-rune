# Keyboard shortcuts, actions and remote control

Global tab navigation uses cached tab order without enabling extra prefetch: a
session whose view is not loaded is one stop, and selecting it loads its tabs.
Crossing sessions or hosts settles after 150 ms without another cycle, coalescing
fast repeats rather than attaching each intermediate session. The first stop can
attach before key repeat begins. During settling there is no target preview, and
typing still goes to the displayed pane. Back/Forward history is local to this
window and survives reconnects; unavailable targets are skipped. Commands also
exposes these navigation actions. Ctrl Tab is a client default; remove it (see
[recipes](#more-recipes)) to pass it to editors such as Neovim.

## Default shortcuts

| Shortcut | Behavior | Action (and args where required) |
| --- | --- | --- |
| Ctrl K | Search sessions across saved hosts | `session.switch` |
| Ctrl Shift P | Search commands, sessions and current-session tabs | `client.palette.toggle` |
| Ctrl Shift N / T | New session on the active host / new tab | `session.new` / `client.tab.new` |
| Ctrl Shift D / Ctrl Alt D | Split right / down (left, up and best fit have no default key) | `pane.split`, `{"direction":"right"}` / `{"direction":"down"}` |
| Ctrl Tab / Ctrl Shift Tab | Next / previous tab across connected hosts and sessions, in sidebar order (wraps) | `client.tab.next_global` / `client.tab.previous_global` |
| Ctrl PageUp / PageDown | Previous / next tab within the current session (server-controlled, no wrap) | `client.tab.previous` / `client.tab.next` |
| Ctrl Shift Up / Down | Previous / next shell prompt (OSC 133; viewport only) | `client.prompt.previous` / `client.prompt.next` |
| Ctrl Shift PageUp / PageDown | Move the current tab left / right in its session's tab order (the server's order, so every client sees it) | `client.tab.move`, `{"direction":"left"}` / `{"direction":"right"}` |
| Mouse Back / Forward | Revisit tabs, sessions and hosts in navigation history | `client.history.back` / `client.history.forward` |
| Ctrl Alt arrows | Focus an adjacent pane (Alt arrows reach terminal programs) | `pane.focus`, `{"direction":"left"}` (or `right`, `up`, `down`) |
| Ctrl Shift Enter | Zoom / restore the focused pane | `pane.zoom` |
| Ctrl Shift Space | Focused-pane actions | `pane.actions` |
| Ctrl Shift W | Close the focused pane, or the tab when it has one pane (asks first) | `client.close` |
| Ctrl , | Settings | `client.settings` |
| Ctrl Shift B | Collapse / restore the sidebar | `client.sidebar.toggle` |
| Ctrl Alt Shift T | Reveal / put away the toolbar when it has no row | `client.toolbar.toggle` |
| Ctrl Shift C / V | Copy selection / paste text (bracketed-paste aware), or upload copied files or an image | `pane.copy` / `pane.paste` |
| Ctrl Shift F | Search the focused terminal's screen and scrollback | `client.search` |
| Ctrl Shift R | Resume the focused terminal's data stream | `pane.reconnect` |
| Shift PageUp / PageDown, wheel | Scroll local history | `pane.scroll`, `{"lines":-20}` / `{"lines":20}` (wheel is pointer input) |
| Ctrl + / Ctrl − / Ctrl 0 | Enlarge / shrink / reset the whole interface | `client.zoom.in` / `client.zoom.out` / `client.zoom.reset` |

These are the client defaults on Linux; [on macOS they are on the Command
key](#shortcuts-on-macos). Bound chords do not reach terminal programs, and every
other key goes to the focused pane. Interface zoom changes in 10% steps (70–150%),
is saved as `ui_scale`, and is independent of the terminal font size. `Ctrl =` and
the numpad `+`/`−` also work. Commands lists the zoom actions too, and every other
action here that needs no argument.

The following actions have no default key binding:

| Action | Args schema | Behavior |
| --- | --- | --- |
| `client.tab.goto` | `{"index": integer >= 1}` | Focus the 1-based tab in the current session |
| `client.tab.close` | None | Confirm closing the current tab |
| `client.tab.rename` | None | Open the current tab's rename dialog |
| `client.tab.mute` | None | Mute the current tab on this client, or unmute it |
| `client.tab.close_others` | None | Confirm closing every other tab of the current session |
| `session.rename` | None | Open the current session's rename dialog |
| `session.next` / `session.previous` | None | Show the next / previous session, in sidebar order across connected hosts (wraps) |
| `session.refresh` | None | Read every connected host's session list again, and the layout of the session each shows (read-only; sessions normally follow the server's events without it) |
| `session.close` | None | Confirm closing the current session |
| `client.sidebar.collapse_others` | None | Collapse every host in the sidebar except the current one |
| `client.theme.choose` | None | Open the terminal theme list |
| `client.font.increase` / `client.font.decrease` / `client.font.reset` | None | Step the terminal font size one point within 8–48 pt, or return it to 15 pt. It applies to every terminal and is saved as `font_size`; interface zoom is untouched. `Ctrl Alt =`, `Ctrl Alt -` and `Ctrl Alt 0` are free to bind, but then stop reaching terminal programs |
| `client.about` | None | Open Settings → About |
| `client.version.copy` | None | Copy the version info, as Settings → About does |
| `client.debug.copy` | None | Copy the debug info, as Settings → About does |
| `client.issue.report` | None | Open the issue form in the browser |
| `pane.close` | None | Confirm closing the focused pane |
| `pane.focus_next` | None | Focus the next pane of the current tab, in the server's layout order (wraps) |
| `pane.focus_previous` | None | Focus the previous pane of the current tab (wraps) |
| `client.help` | None | Open this guide for the installed version in the browser |
| `client.search.next` | None | Find next: select the next older match in the focused terminal. With its search closed, search again for the last text searched there, or open the field when there is none |
| `client.search.previous` | None | Find previous: select the next newer match, or reopen the search as Find next does |
| `pane.select_all` | None | Select the focused terminal's screen and scrollback |
| `pane.copy_id` | None | Copy the focused pane's block ID to the clipboard |
| `pane.clear` | None | Send Rex's `terminal.clear` for the focused terminal, once. What it clears is the server's doing; this client has not yet been checked against a live server for it |
| `pane.reset` | None | Send Rex's `terminal.reset` for the focused terminal, once. What it resets is the server's doing; this client has not yet been checked against a live server for it |
| `pane.send_key` | `{"key": "single chord"}` | Send a synthesizable key directly to the PTY, bypassing Lua bindings |
| `pane.download` | None | Open the Download file… dialog for the focused pane (API revision 13). Never available to `client.perform` |
| `client.pane_headers.toggle` | None | Float the header on every pane that has none, or put them back; not saved |

Omit `args` for actions with no arguments (an empty object also works). For
argument-taking actions, supply exactly the required field: `pane.split` accepts
`direction` = `right`, `left`, `down`, `up` or `best_fit` (right of a pane wider
than it is tall, otherwise below it); `pane.focus` accepts `left`, `right`, `up`
and `down`; `pane.scroll` takes a signed 32-bit integer `lines` (negative scrolls
up). `pane.send_key` takes a single supported chord, not a sequence; for example
`{"key":"ctrl+b"}`.

## Shortcuts on macOS

On macOS the defaults are on Command (⌘), which terminal programs do not use, so
Control keeps its terminal meanings (`Ctrl C`, `Ctrl T`, `Ctrl W`) and nothing
needs Shift to stay clear of them. Each Linux default above is the same key with
Command in Control's place, except these:

| Shortcut on macOS | Linux default | Action |
| --- | --- | --- |
| ⌘T | Ctrl Shift T | New tab |
| ⌘W | Ctrl Shift W | Close the focused pane, or the tab |
| ⌘C / ⌘V | Ctrl Shift C / V | Copy / paste |
| ⌘F | Ctrl Shift F | Search the terminal |
| ⌘D / ⇧⌘D | Ctrl Shift D / Ctrl Alt D | Split right / down |
| ⌥⌘ arrows | Ctrl Alt arrows | Focus an adjacent pane |
| ⇧⌘[ / ⇧⌘] (and ⌘ Page Up / Down) | Ctrl PageUp / PageDown | Previous / next tab in the session |
| ⌘A | none | Select the terminal's screen and scrollback (`pane.select_all`) |
| ⌃⌘F | none | Enter or leave full screen (`client.full_screen.toggle`). A debug build's View menu has the system's own Enter Full Screen item, which answers ⌃⌘F first, so there rebinding it changes nothing. Not yet pressed by hand |
| Ctrl Tab / Ctrl Shift Tab | the same | Next / previous tab across sessions |
| Shift PageUp / PageDown | the same | Scroll local history |

So ⌘K switches sessions, ⇧⌘P opens Commands, ⌘, opens Settings and ⌘= / ⌘− / ⌘0
zoom.

- **Unbound Command keys** go to the program as Super, for the few programs that
  read it. ⌘M and ⌘W are the exception when nothing is bound to them: they stay
  the window's Minimize and Close. With a popup or Settings open, ⌘W closes that,
  not the window. ⌘Q and ⌘H are the system's.
- **Option** follows Settings → Terminal → **Option key**, as in Rex for macOS.
  **Automatic (by keyboard layout)**, the default, makes Option act as Alt on the
  U.S. and ABC layouts and type characters on every other layout, and reads the
  layout again when macOS says you picked another. **Acts as Alt** always sends
  Alt with the key, so Option+B reaches the program as Alt+B (ESC b) and
  Option+Shift+B as Alt+Shift+B. **Composes characters** types what macOS
  composes: ∫ for Option+B, or @ for Option+L on a German layout. **Left sends
  Alt** makes the left Option act as Alt and leaves the right one composing
  characters, dead keys included; **Right sends Alt** is the mirror. With both
  Options held, the key is Alt. Option with Control or Command, and Option with a
  key that types nothing (an arrow), are Alt in every mode. Option is Alt only
  while none of Rex's own fields has the keyboard: those fields (find, Commands,
  Settings, Add host) get Option's characters, from the first key after Rex has
  handled the one that focused them. In Composes characters, a shortcut bound to
  an Option key still takes it unless the chord is a dead key on that layout
  (Option+E, I, U, N or \` on U.S.), which goes to the system's input and reaches
  the pane as the composed text. Automatic reads the keyboard layout, not the
  input method, so Pinyin, Kotoeri or Korean over ABC counts as U.S.; Dvorak and
  Colemak count as other layouts. The encodings were checked by sending these key
  events to a Rex server on macOS. For the left and right choices,
  [`scripts/option-sides-private.py`](../scripts/option-sides-private.py) sends
  keys through the built client to a private Rex server: with Left sends Alt, left
  Option+B reaches the program as ESC b and right Option+B as ∫ on U.S., and the
  mirror with Right sends Alt. Those keys are sent as the window delivers them, so
  how macOS reports which Option is down, and the window turning that key into
  Alt, were not exercised. Pressing the keys by hand, whether Option+E is Alt+E
  rather than a dead accent in Acts as Alt or on the side that sends Alt, and
  switching layouts while Rex runs have not been tried.
- **Links** open on ⌘-click, because macOS makes Control+click the secondary
  click.
- **A `shortcuts.json` written on Linux** keeps its meaning: `ctrl+…` there is
  the Control key on macOS too, not Command. Write `cmd+…` for Command. In
  `shortcuts.json` and on the Shortcuts page Command is spelled `cmd`, and keycaps
  read Cmd and Option. The layer of built-in defaults is still named `linux` where
  a report names a binding's source, on every platform.

## Configuring shortcuts

In **Settings → Shortcuts**, press a row’s keys to replace or remove them, or
**Record shortcut** to give an unbound action a key. Type a chord or a sequence of
up to four chords; **Enter** or **Save** saves, **Escape**, **Cancel** or a click
outside cancels. **Backspace** takes back a chord; with none typed it removes the
old binding, as the Remove button does. Start with Ctrl, Alt, Super (on macOS:
Cmd, Ctrl, Option), a function key or Shift with a named key. Plain typing and IME
composition are not recorded. Workspace shortcuts are captured, not run, while
the recorder has the keyboard. Conflicts are warned about before saving,
including overlapping sequences.

Recordings live in **`recorded_shortcuts` in workspace.json**, above the user’s
file, and are declared to every saved host. Replacing a mode-scoped binding keeps
its mode; conditional bindings keep their arguments. Actions needing arguments
that the row cannot supply stay read-only references. The **⋯** menu beside the
filter copies the shortcuts-file path or the recorded layer as JSON, and offers
**Reset all…** with confirmation. The copied JSON is a **workspace.json
fragment**, not a shortcuts.json file: it preserves named modes and
action-scoped removals. Reset forgets only recordings, never edits either user or
server configuration. **Change shortcuts…** in Commands opens this page; **Ctrl
Enter** (⌘ Enter on macOS) on a command opens its recorder when that command has a
valid binding target.

For file-based overrides, edit `$XDG_CONFIG_HOME/rex-rune/shortcuts.json`
(normally `~/.config/rex-rune/shortcuts.json`). With a custom `--config
/path/demo.json`, edit `/path/demo.shortcuts.json`. The app never writes this
file, except for the one-time import of custom bindings from nivox's legacy
`keybindings` field.

The preferred format is an object; a bare array also works. A missing, empty or
whitespace-only file means no overrides. Bad entries are skipped; invalid JSON
uses defaults and appears in Settings → Shortcuts → Problems.

```json
{
  "shortcuts": [
    {"keys": "ctrl+b>c", "action": "client.tab.new"},
    {"keys": "ctrl+tab", "action": null},
    {"keys": "ctrl+shift+tab", "action": null}
  ]
}
```

Layers run lowest to highest: **built-in defaults → each server’s Rex config →
shortcuts.json → recorded shortcuts**. The app sends its defaults, override layers
and action registry to `client.keymap`; `rex.bind`, `rex.unbind`, Lua-function
sequences and config-file actions participate in the same resolution. The active
host’s map is used—not a union of hosts. When that host is offline, has not
answered, or rejects the keymap request, local defaults plus overrides work
instead. A keymap error never disconnects the host; Settings → Shortcuts retains
its explanation. Config changes refresh by hash and server instance, and every
reconnect registers again.

## Rex action compatibility

Settings → Shortcuts lists supported actions even when **Unbound**, with their
action names; compatibility spellings are marked **alias**. This is a focused
subset of the [official Mac action
reference](https://www.superlogical.com/rex/docs/reference/mac-actions), not full
Mac parity. Every newly added compatibility action is **local-only**; the remote
allowed set below is unchanged.

| Status | Actions / behavior |
| --- | --- |
| Supported | `client.config.reload` rereads local shortcut overrides and requests `server.config_reload` on the active host (its `init.lua`, not a server restart). Rex permits reload only over a local Unix socket or SSH-forwarded socket; HTTPS/Tailscale hosts return an error notice. `client.config.copy_path` copies that host's reported config path. |
| Supported | `pane.resize` takes `direction` (`left/right/up/down`) and optional positive numeric `amount` (percentage points of the split, default 5). It moves the next border from server geometry, clamped like a divider drag. `pane.balance` evenly distributes branches along each same-direction split chain; `pane.move` swaps with the neighbour in `direction`. |
| Supported | `pane.rename`, `pane.close_others` (confirmation), `pane.move_to_tab` (picker for another tab in the same session; optional `window_id`). No program is restarted by a move. |
| Supported | `session.close/next/previous/refresh`; `client.host.switch/add/remove`; `client.tab.close_others` and `client.tab.move` (`direction`: `left/right` or `backward/forward`). |
| Supported | `pane.select_all`, `pane.focus_next/focus_previous`, `client.font.increase/decrease/reset`, and `client.scroll.page_up/page_down` (one visible page). |
| Supported | `client.mode.enter` (`name`, optional boolean `once`), `client.mode.exit/exit_all`: host-defined modes, nesting, sequences, exclusive modes, blocked keys and Escape to leave. |
| Aliases | `pane.focus.left/right/up/down`, `pane.split.left/right/up/down/auto`, `pane.move.left/right/up/down`; `client.tab.move.backward/forward`; `client.find.open/next/previous` for `client.search[.next/.previous]`; `client.theme.change` for `client.theme.choose`. Find next/previous also accept `when = "searching"`. |
| Supported | `client.prompt.previous/next` (`Ctrl Shift Up/Down`) moves only this client’s viewport between shell OSC 133 prompt groups, relative to the viewport top, without wrapping. No marks means no movement. Commands lists both; `pane.prompt_previous/prompt_next` are aliases. No PTY input, ownership claim or server layout change. Verified through real snapshots and live streams; see [M3.7 evidence](parity-2026-11/prompt-navigation.md). |
| Recognized, not supported | `pane.move_to_new_tab`: the supported server API cannot create a window containing an existing block; `session.new_window` starts new programs. This produces an explicit Problems diagnostic and a notice when invoked, not a silent no-op. |
| Not applicable | Mac system permissions, Quick Look and Sparkle updater actions. Native multi-window, floating-pane controls and other reference actions not listed here are not implemented by this compatibility pass. |

Declare modes in the host's `init.lua`, using [Rex's key-mode
syntax](https://www.superlogical.com/rex/docs/customize/key-modes); local
`shortcuts.json` can bind entry/exit actions but does not define mode-scoped keys.
Modes are cleared when the host, session, connection or keymap changes. Resize
amounts and the same-session move picker above describe Linux behavior: the Mac
action index does not publish full argument schemas, and exact Mac behavior beyond
its documented examples is not claimed. Move-to-new-tab remains a gap in
[#44](https://github.com/shuv1337/rex-rune/issues/44).

## Remote control

Settings → Remote Hosts has a **Remote control** switch for each saved host, **off
by default, including local hosts**. Changing an endpoint turns it off again.
Commands can turn it on or off for the host on screen. With it on, `rex do` and
`client.perform` can run exactly eight actions: `session.select`, `client.tab.goto`,
`client.tab.next`, `client.tab.previous`, `pane.focus`, `pane.zoom`,
`pane.scroll`, and `client.sidebar.toggle` on that host. These requests cannot
create or close terminals, type, access the clipboard, open dialogs (so none can
start a download), change interface zoom, or affect any host other than the
requesting host. Refusals name the server command to use instead where one exists
(`rex window new`, `rex send-key`, `rex capture`, etc.). Actions remain listed even
with the switch off.

`session.select` shows a session on the requesting host, switching to that host if
needed; an optional `window_id` selects a tab. It replies only after navigation is
acknowledged, or fails after five seconds. Unknown sessions/tabs return
`not_found`. All other requests must match the host, session and pane on screen.
Navigation runs only after a pause in typing, and never while a text field has
focus. The pause must last at least 1.5 seconds. All remote actions are refused
while a text field has focus, a panel is open or a key sequence is pending.
Requests not acted on within five seconds are discarded, including time buffered
during a busy connection step (counted conservatively from that step's start).
Replies never cross a reconnect. Every performed request gets a six-second in-app
notice naming host, requester and action; nothing raises the window or sends a
desktop notification. The first request refused because control is off shows one
explanatory notice per host per launch. Requester identity is displayed, not used
as a second permission filter.

The **Client label**, also in Remote Hosts, defaults to this computer's current
hostname (or “Rex Rune” if empty), without saving that default as an override. It
is visible to clients on every connected server and can be edited (reopen the app
to apply it). Use `rex -C LABEL actions` to list actions, then, for example, `rex
-C LABEL do client.sidebar.toggle`. Local shortcuts and queued actions from this
client's own `client.action` are independent of Remote control.

```sh
rex -C LABEL do session.select session_id="$REX_SESSION"
rex -C LABEL do session.select session_id=SESSION window_id=WINDOW
rex -C LABEL do client.tab.goto index=2
rex -C LABEL do client.tab.goto index=-1  # last tab
```

`session.select` can also be bound with fixed arguments in `shortcuts.json` or
queued by Lua; it does not add an argument-free Commands entry. Cross-host remote
control was checked against two private preview.14 servers, with the second
reached through a real `ssh://` socket forward and disposable SSH keys. Run
`scripts/remote-control-acceptance.py` with `REX_BIN` pointing to a built client
and preview.14's `rex` on `PATH`; it owns its servers, sockets, HOME/XDG state and
Xvfb display under `~/.cache/agent-ws/`, and stops all its processes by PID. It
also captures Settings → Remote Hosts and the performed-request notice. If a
private `sshd` cannot run, it reports that and checks two local sockets instead.

## More recipes

Later entries win; `null` frees those exact keys for the terminal. Hand edits
reload when the window regains focus or Settings opens, gated by file mtime and
length. `cmd`/`super` means Super, not Ctrl. Physical keys use W3C names such as
`ctrl+[KeyT]`; sequences use `>`, Escape cancels, and a failed tail goes to the
terminal without retrying it as a new shortcut. Non-ASCII bindings such as
`ctrl+é` work in the client but are not readable by the server's parser, like the
client-only `ctrl++` extension. Ordinary terminal keys still use `client.key`.

```json
{
  "shortcuts": [
    {"keys":"ctrl+shift+t", "action":null},
    {"keys":"ctrl+b>c", "action":"client.tab.new"},
    {"keys":"ctrl+b>shift+5", "action":"pane.split", "args":{"direction":"right"}},
    {"keys":"ctrl+b>ctrl+b", "action":"pane.send_key", "args":{"key":"ctrl+b"}},
    {"keys":"ctrl+1", "action":"client.tab.goto", "args":{"index":1}}
  ]
}
```

`pane.send_key` is the explicit escape hatch through `block.event` (press and
release); normal keys and forwarded Lua sequences use `client.key`, with the
server’s original prefix spellings. Action arguments are checked before dispatch.
Keys accept modifier aliases, named keys, F1–F12, W3C physical codes and `>`
sequences. Letters follow the keyboard layout, with physical US-position fallbacks
on non-Latin layouts; Shift stays a modifier instead of being folded into the key.
The shortcut recorder, key modes and pane-resize bindings are described above.
Commands runs a server action with `client.action` (origin `palette`) for the
session and pane on screen, once: a failure is a notice and is never retried.
Server actions that require arguments are not listed. Cross-server
`client_call_requested` relaying is also not implemented
([#20](https://github.com/shuv1337/rex-rune/issues/20)). Agents should use Rex's
session, window and block commands for server state, and show refusal messages.

The parser, sequence machine, action-registry structure and repeat suppression
adapt Andrea Zito ([nivox](https://github.com/nivox))’s `8037d78` work. Importing a
legacy `keybindings` field drops copied defaults, writes only custom entries once,
and reports a move only when at least one custom entry remains.

Server action calls use a dedicated 35-second timeout (Rex allows an action 30
seconds). The host's control loop is serial: later input and control commands
queue behind a running action. Failed or timed-out input is never replayed
automatically. Known returned client actions are performed once, in order;
unknown names are ignored. No cross-server `servers` list is advertised. A
returned action whose original host, session, pane or local keymap generation is
no longer current is skipped with an informational notice rather than applied to
a different target.

Settings → Shortcuts is a searchable list and recorder for supported actions,
including unbound actions and aliases that are bound. Alternate bindings share a
row; keycaps spell a literal `+` as **Plus** and join sequence steps with “then”.
Filter by action name, category or keys (for example `ctrl+shift+d` or
`ctrl+b>c`). Key matches include shadowed bindings in priority order within each
mode, with source, mode and conflict notes. Escape clears the filter, then closes
Settings. Persistent Problems include unsupported-action diagnostics; Files lists
both config paths. Recording never writes `shortcuts.json`. Commands shows the
highest binding for each action and argument set, or no shortcut when unbound.
Only settings-source diagnostic sets toast, once per distinct set per host; file
and default-layer diagnostics stay in Problems. A pending sequence shows `Ctrl B
…` in the toolbar. Host/pane/session/focus/map changes and opening a dialog cancel
it.

Outside the recorder, only popup navigation/editing keys and Escape cancelling a
sequence stay reserved; zoom is global (including fields), but still rebindable.
Hold-to-repeat works for zoom, terminal font size, pane focus (by direction or
next/previous) and scroll, within-session tab navigation (Ctrl PageUp/PageDown)
and global tab cycling. Other completed shortcuts and prefix repeats are
suppressed, and releases only reach terminals when presses did.
