# Vendored Rune: window-chrome additions

`crates/rune`, `crates/rune-motion`, `crates/rune-staging`, `crates/rune-taffy` and
`crates/procwatch` are [anomalyco/rune](https://github.com/anomalyco/rune) at
`d70929a4dae263d16d2e8088e7bfe53f58f619ad` (MIT, see `LICENSE`), with the changes below.
The only manifest changes are replacing `edition.workspace`/`license.workspace` with their
values and dropping `[lints] workspace = true`, since these crates sit outside Rune's
workspace. `rune-taffy` (the layout store `rune` depends on) is an unpatched snapshot.
`procwatch` is an optional dependency of `rune` (the `frame-profile` feature).

`rune-kit`, `rune-highlights` and `rune-theme-opencode` still come from the same git
revision. The root `Cargo.toml` patches `rune`, `rune-motion` and `rune-staging` to these
paths so the whole dependency graph sees a single `rune`. Nothing outside the vendored
`rune` depends on `rune-taffy` or `procwatch`, so they need no `[patch]` entry.

## Changes

- Top-level Linux Wayland windows prefer `wgpu::PresentMode::Mailbox` when advertised by
  `surface.get_capabilities(adapter).present_modes`, falling back to `AutoVsync` otherwise
  (`window/present.rs`, applied in `Host::try_create` and `Host::reattach`). Because winit
  already paces top-level Wayland windows through `wl_surface.frame` callbacks
  (`pre_present_notify`), FIFO can double-block on compositors/drivers such as KWin + Mesa RADV.
  X11, macOS, Windows, embedded surfaces, and Wayland layer-shell overlays (`Overlay::attach`,
  which rely on FIFO vsync under `ControlFlow::Poll`) stay on `AutoVsync` by default.
  `RUNE_PRESENT_MODE` (`auto`, `fifo`, `fifo-relaxed`/`fifo_relaxed`, `mailbox`, `immediate`)
  overrides top-level window present mode when supported by the surface, warning once and
  falling back cleanly otherwise. Hidden `PresentChoice` and `choose_present` exports let
  `rex-rune` unit-test the policy across every CI target. Remove this patch once upstream Rune
  picks a non-blocking present mode on Wayland.

- Backport [Rune #1454](https://github.com/anomalyco/rune/pull/1454)
  ([`263b7998`](https://github.com/anomalyco/rune/commit/263b79984c13f2c70fa3e0dd9c8b48122b9d3c8c)):
  top-level windows skip unchanged IME cursor rectangles to stop Wayland's idle redraw loop.
  This snapshot's overlays do not synchronize IME.
- Backport [Rune #1503](https://github.com/anomalyco/rune/pull/1503)
  ([`1ab171df`](https://github.com/anomalyco/rune/commit/1ab171df98cca8b2a9949a15e54e3f387fab57e6)):
  idle empty preedits no longer pull a scrolled text field back to its caret. Active composition
  and `App::on_ime` routing are unchanged. Remove both backports once vendored Rune is bumped
  past `1ab171df`.

- Linux's optional GNOME reduced-motion read clears exit-on-close on GIO's shared
  session connection before constructing GSettings (dconf can initialize GVfs).
  One owned current reference outlives synchronous construction so dconf's deferred
  acquisition uses that protected connection, too. Repeated samples replace/unref
  the previous reference, never accumulate them; closed references are released
  before reacquisition. The final current reference and GIO library have process
  lifetime. GDBusConnection and GObject ref/unref are thread-safe; the small mutex
  guards ownership exchange/closed-flag inspection, never connection I/O or unref.
  Losing the desktop bus must not raise SIGTERM and close terminals. Missing bus,
  GIO or schema, or GIO returning a retained closed singleton, is unavailable, not
  cached success. GNOME preference sampling may stay unavailable until app restart:
  no GLib dispatcher, retry loop or appearance recovery is added. The separate
  portal colour watcher keeps its last value and logs its stopped subscription
  once; notification recovery remains the independent notification worker's job.
  macOS/Windows paths are unchanged. Cause, private regression invocation and
  limits: [#79 notebook](../../docs/investigations/79-session-bus-loss.md).
  Remove this patch when upstream protects this GSettings path, including deferred
  backend acquisition/reference lifetime, from bus loss.

- `Runtime::physical_key_input` is public so Rex's headless tests can inject the native
  physical/logical pair, base character, repeat and release without scripts or private
  runtime access. No dispatch behavior changes. `src/app/physical_input_tests.rs` checks
  base/code/modifier delivery, repeat reporting and release routing through this API.

Rex runs in a borderless window and draws its own chrome: resize bands, a drag region and
the window manager's menu on right-click. It has no minimize or maximize buttons;
`examples/chrome_spike.rs` exercises those. A borderless Linux window cannot minimize,
edge-resize or open the window manager's menu, and its view cannot tell whether it is
maximized. The patch adds the following without changing existing behavior on macOS
(maximizing is `Tx::toggle_maximized`, which Rune has):

- `Cx::window_maximized()`, refreshed on every `Resized`; the view re-runs when it changes.
- `Cx::window_full_screen()` (winit's `fullscreen().is_some()`), refreshed on every `Resized` in
  the same way and, on macOS, on the full-screen notifications as well, so a full-screen window
  is not saved as the normal size. It sees full screen made by `Tx::toggle_full_screen`
  everywhere and the system's own on macOS and Wayland; on X11 winit reports only what the app
  asked for, so the window manager entering or leaving full screen is not seen. A change with
  no size change is missed off macOS.
- `Tx::minimize()` works on borderless windows off macOS (used by the spike only).
- `El::window_resize(WindowEdge)` (winit `drag_resize_window`, started inside the press),
  `WindowEdge`, `TitlebarAction::Resize` and `Cursor::ResizeEdge(WindowEdge)`.
- `Tx::show_window_menu(x, y)` (winit `show_window_menu`; Wayland and Windows only in winit).
- Off macOS, a `window_drag` press waits for 3pt of travel before `drag_window`, so the
  region's double-click (maximize) still reaches Rune: an X11 move grab otherwise takes the
  second press.
- `set_app_icon()` registers a native window icon on Linux, decoding any image the `image`
  dependency reads into unmodified RGBA before startup. All top-level windows opened after
  the call receive it through `WindowOptions`. This supplies X11's `_NET_WM_ICON` for task
  lists that do not resolve desktop-file icons; Wayland still uses the app ID and the
  installed desktop entry. Invalid images return `Err`. macOS and Windows keep Rune's own
  `set_app_icon`.
- `El::no_wrap()` for styled display text without ellipsis or fade. Rex owns terminal line
  breaks, so fractional-DPI width rounding must not wrap a text run onto another row
  (`rich()` turns wrapping on).
- Native dead keys are retained as `Key::Dead` events instead of being dropped, so terminal
  clients can send their physical press and `composing` state before the completed text.
  A completed key carries winit's composed text, so a dead key followed by Space types its
  accent rather than a space. Headless scripts accept
  `{"type":"key","dead":"~","code":"backquote"}` for this path.
- `text::set_app_font_paths()` and `TextSystem::new_with_paths()` register native font files by
  path, so large system fonts stay memory-mapped instead of being copied into process-lifetime
  byte buffers. Symbol and math fallbacks go through the same per-file cache as CJK and script
  fallbacks, so a file the app registered is not loaded twice. The embedded-font API is
  unchanged.
- `Tx::set_option_as_alt(bool)` asks for winit's `OptionAsAlt` (`Both` or `None`) on the macOS
  window, and the request lasts. The window gets `Both` only while no text field has focus, set
  after every frame that runs input, drawn or not (a covered window's and a timer's too, but not
  one whose surface is missing or refuses a new blend format), so a field focused by one key
  composes the next once that frame has run. winit replaces the key event before AppKit's text
  input sees it, so Option+E should be Alt+E rather than a dead accent (not tried by hand),
  which the app cannot undo afterwards. A no-op elsewhere. A window Rune creates again gets the
  request too. `Runtime::option_as_alt` (hidden) is what the window gets now, for headless
  tests.
- `Tx::set_option_alt(OptionAsAlt)` asks for one Option key alone as well: `OptionAsAlt` is
  `Neither`, `Left`, `Right` or `Both`, mapped to winit's `None`, `OnlyLeft`, `OnlyRight` and
  `Both` on the same path as above; `set_option_as_alt(bool)` is now `Both` or `Neither` through
  it, unchanged for its callers. winit rewrites a key only when an Option it was told to treat as
  Alt is down, so with both held the key is Alt. `Tx::alt_keys()` returns an `AltKeys { left,
  right }`: which Alt keys were held with the key a handler sees, from winit's
  `ModifiersChanged` (`lalt_state`/`ralt_state`) before it, carried with the queued key so a
  later modifier change doesn't alter it. Both are false where winit doesn't say (it does on
  macOS), for overlays, for keys without a physical origin and outside key handlers, so nothing
  changes for existing callers. `Runtime::physical_key_input_alt` is `physical_key_input` with
  the held Alt keys, for headless tests, and a script's `{"type":"key","code":…}` takes
  `"alt_side": "left" | "right" | "both"` for them. `Runtime::option_alt` (hidden) is what the
  window gets now, by side.
- `Pixels::rgba()` (hidden) is public instead of test-only, so the app's tests can check that
  each program icon `TextSystem::color_icon` rasterizes actually paints something.
- `text::installed_family_files(name)` is public (it was the private lookup behind
  `Family::Named`), and `text::installed_families()` lists every installed family as an
  `InstalledFamily { name, fixed_pitch }`, so an app can offer and resolve the system's fonts
  where there is no fontconfig (Core Text on macOS, DirectWrite on Windows). Both read one
  system collection per thread instead of building one per call. Names beginning with a dot
  (macOS's hidden families) are left out of the list. A face is fixed-pitch by fontconfig's
  rule: every character it maps advances one width, or two where one is twice the other, each
  within 1/33; a face that maps none is fixed-pitch if its `post` table says so. A family is
  fixed-pitch when all its faces are. Listing reads every installed face (35–60 ms with the
  files cached, and the system scan alone took about 220 ms on a first read, on an Apple
  silicon Mac with 325 families), so callers should run it off the UI thread.

Spike: `examples/chrome_spike.rs` and `scripts/chrome-spike-x11.py`.

Drop this copy once upstream Rune has equivalent APIs.
