# Terminal, search, selection and clipboard

## Rendering

Rex Rune decodes terminal output with libghostty-vt (Ghostty) through the
[`ghostty-vt`](../vendor/ghostty-vt/REX-RUNE.md) crate. The server owns terminal
content, colours and split geometry; the client never recolours output locally.

- **Cursor shapes** chosen by programs with DECSCUSR (block, bar or underline) in
  the focused pane; other panes show a hollow block. Settings → Terminal can
  choose a local default or **Force style** over program requests. Blinking
  cursors draw steadily: terminal content does not animate.
- **Underline styles** (single, double, curly, dotted and dashed) and colours that
  programs set with SGR 4:x and 58, and overlines (SGR 53).
- **Box-drawing and block elements** (U+2500–U+259F) are drawn as shapes on whole
  device pixels, as Ghostty draws them, rather than from the font, so borders,
  tables and block graphics meet their neighbours without seams or overlaps at any
  font or display scale.
- **Inline Kitty graphics** (RGB, RGBA and PNG), including image-bearing snapshots
  on reattach. Ghostty resolves placement, crop and paint order; the client caches
  decoded pixels and textures. Image storage is capped at 64 MiB per terminal.
  File, temporary-file and shared-memory image media are disabled: remote terminal
  output cannot read local files. Animated Kitty graphics are not implemented.
- **Synchronized output** (mode 2026): while a program holds an update open (for
  up to a second, as in Ghostty) the pane keeps its last frame, so full-screen
  programs redrawing after a resize don't flash blank.

## Links

Ctrl+click (⌘-click on macOS) opens an `http`, `https`, `ftp` or `mailto` link with
`xdg-open` (`open` on macOS), whether it is an OSC 8 hyperlink or a URL in the
text (including one that soft-wraps across rows). Holding Ctrl over a link
underlines it, shows a hand pointer and, after a moment, a tooltip naming its real
target, so OSC 8 text cannot disguise where it goes. Ctrl+click works over
programs that track the mouse too; clicks elsewhere still go to the program. Other
schemes, `file://` among them, are never opened, since output may come from a
remote host.

## Search

Terminal search (`Ctrl Shift F`) covers the focused pane's screen and scrollback,
with Ghostty's search. A bar over the terminal's top-right corner holds the field,
the selected match's place among all matches (`2/5`, or "No matches"), and Find
next, Find previous and Close buttons; the two arrows wait while there is nothing
to step through. Every match is marked in the chrome's accent, and the selected
one more strongly, with dark text. Enter or Find next (the up arrow) selects the
next older match and Shift+Enter or Find previous (the down arrow) the next newer
one, scrolling it into view; Escape or Close ends the search. The bar never hides
the selected match: when that match is under it, the bar moves to another corner
of the pane (the bottom-right first) and stays there until a selected match is
under it again or the search is closed. It floats over the terminal and never
resizes it. Find next and Find previous are also Commands rows and actions
(`client.search.next`, `client.search.previous`, no default keys) that step while
the keyboard is in the terminal; with the bar closed they bring it back on the
last text searched in that pane, at its newest match, and leave the keyboard with
the program (`Ctrl Shift F` moves it to the field). In a pane showing fewer rows
than the terminal has, the view follows the selected match. New output is searched
as it arrives, and the search survives a reconnect.

Known limit: once a terminal restored from a snapshot has been resized, Ghostty
can start from an older match and step toward newer ones; every match is still
reached.

## Selection and local history

Double-click selects a word (including shell paths), and triple-click selects a
line. Selection stays attached to terminal content as history or output scrolls;
dragging above or below the grid autoscrolls. Hold Shift to select locally or
scroll local history when a fullscreen TUI owns the mouse. Without Shift,
ordinary TUI mouse reporting is unchanged. **Select all** in the pane menu or
Commands selects the terminal's screen and scrollback; a copy is limited to 8 MiB.

Right-click opens that pane's menu, even when a terminal program tracks the
mouse. **Shift+right-click** sends the click to the program instead, while it is
live and tracks the mouse. This is separate from Shift+drag for local selection.

**Copy on selection** copies only a completed pointer selection, on release.
**Primary selection** additionally owns the desktop's PRIMARY selection and pastes
it on a middle click, never during a drag or over a mouse-reporting program. Both
copy options default off. X11 clipboard acceptance is verified; Wayland PRIMARY
depends on compositor support and is not yet validated.

## Clipboard (OSC 52)

A program in a terminal copies to your desktop clipboard by writing an OSC 52
sequence (`ESC ] 52 ; c ; <base64> BEL`). Rex Rune puts the text on the system
clipboard of the machine the client runs on, with no environment variable and no
setting, for a local server and for an `ssh://` host alike (checked live on X11
and on a private Wayland compositor, and through a private `ssh://` login).

- A copy is applied only from the focused pane of the selected session on the
  active host, while the Rex window has keyboard focus and no dialog or popup is
  open. Any other copy (a background tab, an unfocused split pane, another
  session, an unfocused window) is dropped without a notice and leaves the
  clipboard as it was.
- The selections `c`, `p` and `s` (and an empty one) all go to the clipboard; the
  primary selection is never set. The server drops a target of several letters
  such as `cp`.
- Read requests (`?`) are never answered, so a remote program can't read your
  clipboard.
- The server drops, without any event or notice here, a payload that is not valid
  base64, is empty or is not UTF-8 text, and a very large one: 6,000,000 bytes
  arrived and 6,291,456 did not in the server used for the check; the exact
  boundary was not measured. The client refuses text above 8 MiB from the server
  (`MAX_COPY_BYTES` in `src/connection/clipboard.rs`; this limit was read in the
  code, not reached in a test).
- Only the newest copy counts when several arrive together.

### Programs that don't copy through OSC 52

Some programs send OSC 52 only when an `SSH_*` variable is set, or prefer the
clipboard of the machine they run on. Neither is the client: **Rex Rune cannot set
environment variables in a terminal on a remote server, because the Rex server
owns the terminal's environment** (the session and block options have no
environment field; an `env` option is accepted and ignored). What the sources and
tests showed:

- **OpenAI Codex CLI** (read from the source of `rust-v0.167.1`; Codex itself was
  not run): it always tries the native clipboard first, so on a server that has
  `DISPLAY` or `WAYLAND_DISPLAY` the text lands in the server's own clipboard
  whatever else is set. It additionally sends OSC 52 when `SSH_TTY` or
  `SSH_CONNECTION` (any value) or `HERDR_ENV=1` is set, even if the native copy
  succeeded; under `TMUX`/`TMUX_PANE` it forwards through tmux instead (`tmux
  load-buffer -w`, then a passthrough sequence), which reaches Rex with tmux's
  default `set-clipboard`. It falls back to OSC 52 alone only if the native copy
  fails.
- **Neovim 0.10.x** enables its OSC 52 provider only when `SSH_TTY` is set (any
  value) and the `clipboard` option is empty (the default); without both there is
  no provider (checked with 0.10.4). **Neovim 0.11 and later** don't check
  `SSH_TTY` and, when the `clipboard` option is empty (the default), use OSC 52 on
  their own, which Rex advertises (checked with 0.12.6), but they pick `xclip`,
  `xsel` or `wl-copy` first when `DISPLAY` or `WAYLAND_DISPLAY` is set, whatever
  `SSH_TTY` says. **LazyVim** sets `clipboard` to empty only when
  `SSH_CONNECTION` is set, and that is what turns its OSC 52 provider on (checked
  live: `SSH_TTY` alone does not).
- `rex server start` hands `DISPLAY` and `WAYLAND_DISPLAY` from the shell that
  starts the server on to every terminal.

Workarounds on the machine that runs the Rex server (checked live):

- Put this in the file your login shell reads (`~/.profile` or `~/.bash_profile`
  for bash, `~/.zprofile` for zsh; not only `~/.bashrc` or `~/.zshrc`, since Rex
  starts terminals as login shells). It keys on `TERM_PROGRAM=rex`, which the
  server sets in every terminal, and leaves an existing `SSH_TTY` alone:

  ```sh
  if [ "$TERM_PROGRAM" = rex ] && [ -z "${SSH_TTY+x}" ]; then
    _rex_tty=$(tty 2>/dev/null)
    case $_rex_tty in /dev/*) export SSH_TTY=$_rex_tty ;; esac
    unset _rex_tty
  fi
  ```

  It worked in a default login bash, `rex run`, splits, a nested `bash` and a zsh
  login shell; it does nothing for a `--shell=none` terminal. To keep it off
  machines with a desktop session, add `&& [ -z "$DISPLAY$WAYLAND_DISPLAY" ]` to
  the condition (this variant was not checked); with that guard, Codex on a
  machine that has a display keeps copying only to that machine's clipboard.
- Or start a new server with `rex server start --env SSH_TTY=/dev/tty`. It applies
  to every terminal of that server and only when a new server is launched.

Both make every terminal on that server look like an SSH session, local clients
included: prompts such as Starship show the host and an SSH mark (checked with
Starship), some dotfiles branch on `SSH_*`, and by its source Codex then also
treats the session as remote: it turns off right-click paste, reading text from
the clipboard and primary-selection copy. Neither workaround sets
`SSH_CONNECTION`, which LazyVim needs, and neither stops a program that prefers
the server's own clipboard (Neovim 0.11 and later) from using it while `DISPLAY`
or `WAYLAND_DISPLAY` is set there.

## Insert path and Go to directory

**Insert path…** in Commands browses files and folders on the focused terminal's
machine. **Go to directory…** uses the same browser over folders only. Type a
directory path, filter its last component (start with `.` for hidden entries),
move with Up/Down and press Tab on a folder to descend. Enter or a click pastes
the choice into the captured pane; Escape returns to the terminal.

- **Insert path** inserts one absolute path, quoted for a POSIX shell, with no
  Enter or trailing space. Home paths show as `~` but insert as absolute paths.
- **Go to directory** types `cd` followed by the quoted folder. It never presses
  Enter: the client cannot see whether the prompt already holds text, so you run
  the line yourself.
- The text goes to whatever program is in the foreground. It is not right inside
  an already-open quote, and it assumes POSIX quoting; fish and csh quote
  differently.
- Names with control characters, line separators or invisible formatting
  characters (bidirectional controls, zero-width space) are refused; look-alike
  letters are not detected.
- The server must support `terminal.list_dir`. Older-server refusals and other
  errors stay in the browser.
- The first listing offers up to ten recently used paths per saved host, shared
  across its sessions on this computer; editing the field hides them. Recent
  folders share that history, and file recents are hidden in Go to directory.
  Settings → Terminal → **Show recently used paths** is on by default; turning
  it off forgets the history. The lists are plain text in `workspace.json`. A
  recent path may no longer exist; choosing it does not check.
- `pane.insert_path` and `pane.go_to_directory` are local-only and unbound by
  default; assign removable bindings in Settings → Shortcuts.

See [browser checks](parity-2026-11/M3.1-insert-path.md), [Go to directory
scope](parity-2026-11/M3.1-go-to-directory.md) and [recent
paths](parity-2026-11/M3.1-recent-paths.md).

## Uploading files

Drop files on a pane, or paste with Ctrl Shift V when the clipboard holds copied
files or only an image. Rex stores them on the machine that terminal runs on and
types their shell-quoted paths into it, as a paste, without pressing Enter. This
works the same for local and remote hosts and needs no SSH access: the bytes go
over the Rex connection with `upload_begin` (API revision 11). The server keeps
each file in a private directory under its state directory; files dropped
together are typed together, and nothing is typed unless every file arrived.
Folders can't be uploaded. A pasted image is uploaded as `clipboard.png`; when the
clipboard holds text as well, the text is pasted instead. A failed upload shows a
notice and is never retried. An older server shows a notice asking you to update
Rex on that host.

To save a file from the terminal's machine to this one, see
[Downloading files](downloads.md).
