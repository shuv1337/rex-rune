# Vendored Rune

`crates/rune` is [anomalyco/rune](https://github.com/anomalyco/rune) at
`983e33b916b0da5719442d94ae04a00867c952a3` (MIT, see `LICENSE`). Its manifest
expands inherited workspace settings and pins internal dependencies to that
same revision. All other Rune crates and the eight upstream registry patches
come from that git revision.

Rex Rune enables `accessibility`, `dbus`, `embedded-fonts`, `metal-backend`,
`ogg-audio`, `svg`, and `tooling`. The first six cover the upstream defaults
other than Hunspell, which Rex Rune does not use. `tooling` keeps `--snap`,
`--script`, and `--tree` available in release builds. Rune's wasm target
still enables Hunspell, but native builds do not compile `spellbook`.

## Local patches

- **Wayland presentation:** prefer Mailbox on top-level Wayland windows when
  the adapter advertises it, otherwise keep AutoVsync. Winit already paces
  these windows with frame callbacks; FIFO can block a second time on KWin
  with Mesa RADV. `RUNE_PRESENT_MODE` can override the mode for top-level
  native windows (`auto`, `fifo`, `fifo-relaxed`, `mailbox`, `immediate`).
  The selection is made on creation and at GPU reattachment, including the
  Linux CPU-to-GPU handover. Embedded and overlay surfaces keep upstream
  policy. `PresentChoice` and `choose_present` are hidden test exports.
- **Optional GSettings session-bus loss ([#79](../../docs/investigations/79-session-bus-loss.md)):**
  clear GIO's exit-on-close flag before creating GSettings for either fallback
  preference read. Retain one owned shared connection reference for the
  backend's deferred acquisition and release the previous reference outside
  the mutex. An unavailable or closed bus leaves the preference unavailable.
  No retries or changes to the Settings portal or notification workers.
- **Native titlebar interactions:** outside macOS, a mouse-initiated window move
  starts only after 3 pt of travel. An immediate X11 move grab prevents the
  second click of a double click from reaching Rune. Release or focus loss
  cancels a pending move. macOS still starts a move inside mouse-down, as
  AppKit requires. Touch and native edge resizing use upstream behavior.
  Rune's native secondary-release menu also works on bare X11 and Wayland drag
  regions: it must not require the caption controls Rune only sets on Windows.
