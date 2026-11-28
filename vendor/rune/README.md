# Vendored Rune

`crates/rune` is [anomalyco/rune](https://github.com/anomalyco/rune) at
`f1b079ccad541008073e73d0987b876f4c99c322` (MIT, see `LICENSE`). Its manifest
expands inherited workspace settings and pins internal dependencies to that
same revision. All other Rune crates and the eight upstream registry patches
come from that git revision.

Rex Rune enables `accessibility`, `dbus`, `metal-backend`, `ogg-audio`, `svg`,
and `tooling`. It does not use Hunspell or Rune's default embedded faces.
The app supplies the OpenCode theme's fonts; this also keeps `rune-text`
from expecting sibling Rune assets in Nix's flat vendor tree. `tooling` keeps `--snap`,
`--script`, and `--tree` available in release builds. Hunspell is opt-in on
both Linux and wasm at this revision.

## Local patches

- **System appearance in `Tx`:** native host events already update `Cx::system_dark`
  and wake the frame. `Tx::system_dark` gives the same snapshot to `AppSpec::poll`
  so Rex Rune can change its workspace palette and terminal declaration before
  the view commits, without starting a second platform listener.
- **Wayland presentation:** prefer Mailbox on top-level Wayland windows when
  the adapter advertises it, otherwise keep AutoVsync. Winit already paces
  these windows with frame callbacks; FIFO can block a second time on KWin
  with Mesa RADV. `RUNE_PRESENT_MODE` can override the mode for top-level
  native windows (`auto`, `fifo`, `fifo-relaxed`, `mailbox`, `immediate`).
  The selection is made on creation, same-device surface recovery, and GPU
  reattachment, including CPU-to-GPU handover. On Windows,
  `src/window/win32/handover.rs` calls `present::apply` before configuring
  the new swap chain so `RUNE_PRESENT_MODE` still applies. Embedded and overlay
  surfaces keep upstream policy. `PresentChoice` and `choose_present` are hidden
  test exports.
- **Optional GSettings session-bus loss ([#79](../../docs/investigations/79-session-bus-loss.md)):**
  clear GIO's exit-on-close flag before creating GSettings in the shared
  preference reader. This now covers animation, double-click interval, contrast,
  text scale and colour scheme fallbacks. Retain one owned shared connection
  reference for the backend's deferred acquisition and release the previous reference outside
  the mutex. An unavailable or closed bus leaves the preference unavailable.
  No retries or changes to the Settings portal or notification workers.
- **Native titlebar interactions (partly upstream):** top-level mouse moves
  handled by Rune outside macOS still wait for travel of more than 3 pt on
  either axis. The patched test in `src/window/host/draw_tests.rs` checks
  that 3 pt emits no drag and 4 pt emits one. Upstream's press state machine
  now handles release, focus loss and double-click pairing. AppKit still starts
  moves on mouse-down; touch, Windows non-client hit testing and native edge
  resizing keep upstream policy.
  Bare-region window menus no longer need a patch: X11 opens on release when
  the WM supports the menu, Wayland on press while the compositor serial is valid.
