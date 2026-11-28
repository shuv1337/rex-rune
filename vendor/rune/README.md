# Vendored Rune

`crates/rune` is [anomalyco/rune](https://github.com/anomalyco/rune) at
`e9233e3881e2eb7e6de2e6f54c9b84bc637bf68b` (MIT, see `LICENSE`). Its manifest
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
  the view commits, without starting a second platform listener. The changes
  are confined to `src/el/tx.rs` and `src/runtime/frame/latch.rs`.
- **Presentation override:** upstream [#2681](https://github.com/anomalyco/rune/pull/2681)
  chooses Mailbox when a frame-callback-paced Wayland surface advertises it,
  including paced overlays, and AutoVsync otherwise. `RUNE_PRESENT_MODE`
  (`auto`, `fifo`, `fifo-relaxed`, `mailbox`, `immediate`) overrides that
  choice only for native top-level windows. Creation, recovery, reattachment
  and Windows CPU-to-GPU handover use the capabilities Rune already queried.
  Embedded surfaces and overlays always keep upstream policy.
  `src/window/present.rs` implements the override; `src/window/surface.rs`,
  `src/window/host/{create,gpu_loss}.rs`, `src/window/win32/handover.rs` and
  `src/overlay/surface.rs` scope it. `src/window.rs` and `src/lib.rs` expose
  `PresentChoice` and `choose_present` for Rex's tests.
- **Optional GSettings session-bus loss ([#79](../../docs/investigations/79-session-bus-loss.md)):**
  clear GIO's exit-on-close flag before creating GSettings in the shared
  preference reader. This now covers animation, double-click interval, contrast,
  text scale and colour scheme fallbacks. Retain one owned shared connection
  reference for the backend's deferred acquisition and release the previous reference outside
  the mutex. An unavailable or closed bus leaves the preference unavailable.
  No retries or changes to the Settings portal or notification workers. Files:
  `src/platform/preferences.rs` and its safety comment in `src/platform/text_scale.rs`.
- **Native titlebar interactions (partly upstream):** top-level mouse moves
  handled by Rune outside macOS still wait for travel of more than 3 pt on
  either axis. The patched test in `src/window/host/draw_tests.rs` checks
  that 3 pt emits no drag and 4 pt emits one. Upstream's press state machine
  now handles release, focus loss and double-click pairing. AppKit still starts
  moves on mouse-down; touch, Windows non-client hit testing and native edge
  resizing keep upstream policy. The implementation is in `src/runtime/input/titlebar.rs`
  and `src/window/host.rs`.
  Bare-region window menus no longer need a patch: X11 opens on release when
  the WM supports the menu, Wayland on press while the compositor serial is valid.
