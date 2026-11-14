# Parity analysis: Rex Linux 0.6.0 vs Rex Beta 0.1.0 (macOS), November 2026

This folder holds the November 2026 comparison of Rex Linux (Rust on Rune, commit `ad196ff`, version 0.6.0) with Rex Beta 0.1.0 (1932) for macOS, Bleeding Edge channel, running against Rex Server 0.1.0-preview.15 (API revision 13). It was written on 2026-11-11 (Pacific Time). Nothing in it changes source code.

**Start with [GAP-REPORT.md](GAP-REPORT.md).** It is the master report: verdict, scorecard, full inventory, UI quality, blocked items, roadmap and an evidence index. Everything else is the material it was built from.

Implementation follow-up records, each with scope and verification. They do not revise the baseline counts below.

- M2.1: [recovery: searchable, read-only Shortcuts](M2.1.md).
- M2.3: [recovery: Settings search and navigation](M2.3.md).
- M2.5: [Terminal display and selection recovery](M2.5.md).
- M2.6: [focused font and typography recovery](M2.6.md).
- M2.7: [panes, first piece only](M2.7-panes.md); [header-only fading](M2.7-header-fading.md).
- M2.8: [toolbar tabs fill available space](M2.8-tab-fill.md); [tab icon count (small second slice)](M2.8-tab-icons.md); [tab icon colour policy (small third slice)](M2.8-tab-icon-colours.md).
- M2.10: [generated titles (unreleased)](generated-titles.md).
- M3.1: [Insert path (focused slice)](M3.1-insert-path.md); [Go to directory](M3.1-go-to-directory.md); [Recently used paths](M3.1-recent-paths.md).
- M3.2: [Activity list (partial)](M3.2-activity-list.md); [Activity by session (partial)](M3.2-activity-by-session.md); [elapsed time on Activity rows](M3.2-elapsed-time.md); [conditional Activity toolbar chip](M3.2-activity-chip.md); [one status pass per connection](M3.2-status-sweep.md); [session activity words in the sidebar (partial)](M3.2-session-words.md).
- M3.3: [first, read-only Server settings slice](M3.3-server-settings.md); [running duration and confirmed Stop server (partial)](M3.3-server-controls.md); [Pair device (partial)](M3.3-pair-device.md).
- M3.4: [notification quality and health (partial)](M3.4-notification-quality.md); [optional activity sounds (partial)](M3.4-sounds.md).
- M3.7: [prompt navigation verified and implemented](prompt-navigation.md).
- M4.4: [recovery: shortcut recorder](M4.4.md).
- M7: [terminal fonts without fontconfig, and bundled Nerd Font symbols](M7-fonts.md).
- No milestone: [Pane header on a single pane](single-pane-header.md); [pane header program icon](pane-header-icon.md).

## Layout

| Path | What it is |
|---|---|
| [GAP-REPORT.md](GAP-REPORT.md) | Master gap report assembled from the six domain files |
| [gaps/](gaps/) | Six verified domain analyses. Each has a feature matrix, a beta UI specification, UI quality deltas, recommended work items and a verification log (what the adversarial pass corrected and what it confirmed) |
| [beta/](beta/) | Twelve specs of Rex Beta, written from its screenshots and accessibility trees (`b01` to `b12`) |
| [rune/](rune/) | Three specs of Rex Linux as rendered, written from its screenshots (`r01` to `r03`) |
| [code/](code/) | Four inventories of what Rex Linux's code does (`c01` to `c04`) |

### Domain files in `gaps/`

| File | Domain | Matrix ID style |
|---|---|---|
| [d1-workspace-chrome.md](gaps/d1-workspace-chrome.md) | Window, toolbar, tabs, panes, sessions and sidebar, activity, menus and overlays that act on them | `W`, `T`, `TB`, `S`, `P`, `A`, `PN`, `C`, `F`, `G`, `N` |
| [d2-interface-themes.md](gaps/d2-interface-themes.md) | Settings shell, Interface page and sub-pages, Themes, theme editor, theme switcher | `S`, `I`, `T`, `P`, `TH`, `E`, `SW`, `X` |
| [d3-terminal.md](gaps/d3-terminal.md) | Terminal settings and behaviour: fonts, cursor, selection, links, find, context menu, Go to Directory, Insert Path | `T` |
| [d4-shortcuts-commands-menus.md](gaps/d4-shortcuts-commands-menus.md) | Shortcuts page and keymap, action registry, Commands palette, menu bar, context menus | `A` to `G` |
| [d5-hosts-notifications-integrations-onboarding.md](gaps/d5-hosts-notifications-integrations-onboarding.md) | Remote hosts, Server page, pairing, notifications and sounds, integrations, onboarding and migration | `d5-NN` |
| [d6-developer-diagnostics-updates.md](gaps/d6-developer-diagnostics-updates.md) | Developer settings, Diagnostics window, RexSync, Network, GPU, updates, About | `d6.area.name` |

IDs are only unique inside a domain (`A01` exists in d1 and d4). The master report always names the domain with the ID.

### Supporting specs

- `beta/`: b01 main window, app menu and Server; b02 Interface; b03 Themes; b04 Terminal; b05 Shortcuts; b06 Integrations, Notifications, Remote Hosts; b07 Developer; b08 menu bar; b09 overlays; b10 Diagnostics; b11 RexSync; b12 onboarding.
- `rune/`: r01 workspace, Settings, pickers and forms (screenshots 00-17); r02 client label, hosts, discovery, shortcuts (18-29); r03 sidebar, menus, overlays (30-49).
- `code/`: c01 settings, themes and terminal rendering; c02 workspace chrome; c03 keys and shortcuts; c04 hosts, discovery, SSH, remote control, notifications, agents, updates.

## Evidence outside this folder

The screenshots and accessibility trees live at the repository root and were untracked at the time of writing:

- [`../../rex-beta-ui-2026-11-10/`](../../rex-beta-ui-2026-11-10/): `screenshots/` (137 PNGs, numbered 001-137, 1x so one pixel is one point), `evidence/` (accessibility trees as JSON, some full-display captures), `captures.jsonl`, `capture.py`, `COVERAGE.md` (what was and was not captured), `index.html`.
- [`../../rex-rune-2026-11-11/`](../../rex-rune-2026-11-11/): `screenshots/` (50 PNGs, numbered 00-49, 2560x1410, captured at Interface zoom 130%), `thumbnails/`, `navigation/`, `manifest.json`, `README.md`, `index.html`.

In the domain files `B###` means a beta screenshot and `L##` a Linux screenshot (`d3` uses bare zero-padded numbers for beta and `Linux NN` for Linux). Section 7 of the master report maps every screenshot to the file that covers it.

## Vocabulary used in the matrices

- **Status:** `parity` (equal capability), `partial` (Linux has part of it), `missing`, `linux-ahead` (both have it, Linux does more), `linux-only` (no beta counterpart, or the beta behaviour was never captured so "ahead" is not claimed), `not-applicable` (a platform idiom; the Linux equivalent is stated in the row).
- **Blocker:** `none` (client-only), `needs-server-api` (no such method in the vendored SDK), `needs-rune-work` (a `vendor/rune` change), `macos-only`, `product-rule` (collides with DESIGN.md, PRODUCT.md or AGENTS.md; a decision, not a law), `unknown`.
- **Effort:** S under a day, M one to three days, L about a week, XL longer.
- **Impact:** low, medium, high, the value of closing the row for a Linux user.
- **Kind:** `feature`, `ui-quality` or `both` (d1, d4, d5, d6).

## Row counts

The six matrices hold 586 raw rows: d1 118, d2 86, d3 89, d4 152, d5 75, d6 66. The master report's verdict quotes these raw totals (parity 61, partial 156, missing 257, linux-ahead 19, linux-only 53, not-applicable 40); capabilities scored by two domains are counted in both.

## Caveats

- **Linux measurements.** `rune/r01` and `rune/r03` assume 2x captures and halve pixel sizes. The captures are 1x at 130% interface zoom, so those halved numbers are wrong; use the code constants in `src/app/theme.rs` and `popups.rs` (d1 and d4 document the check).
- **Beta behaviour not captured:** Left and Right toolbar layouts, light appearance and Glass, multi-tab and multi-pane layouts, floating panes, tab activity states, the Activity popover with idle processes, Go to Anything, hover reveals and history. Rows that depend on them say `not captured`.
- **Linux states not captured:** first-run or empty workspace, connection and authentication failures, the personal SSH server confirmation, update available or installing, an already-installed integration, split and zoomed layouts, incoming activity notices and alternate theme variants (rune-2026-11-11 README). Rows about them can only be read from code; the completeness review at the end of the master report records which were re-checked.
- **Native Rune look.** Rex Linux takes Rune's native rendering and kit defaults; the beta's hex values describe its design language and are not a target for pixel matching.
- **Evidence defect (fixed):** Linux screenshot 39 (`rename-pane`) was cited by no gap file; the completeness review added it to d1 TB12 (see the end of the master report).
- **Honesty rule.** The README and DESIGN.md must not claim any feature in the roadmap until it is implemented and checked (AGENTS.md).
