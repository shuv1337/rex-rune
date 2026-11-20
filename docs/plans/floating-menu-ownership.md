# Floating-menu ownership — #139

**APPROVED DESIGN · IMPLEMENTED · PR REVIEW PENDING**

Orchestrator decision B; source `b2e6355378494744ebb178daf64aa6d508caf344`,
base `fcc1974846891201ff9d8daf9d82d7012647407e`.
#139 stays OPEN/UNMERGED; #136 remains incomplete.

## Boundary and ownership

P2: open F’s pane-root menu in W1; another client moves F to W2/removes W1.
Before the fix, after a wake the floating branch combined W1’s title with
recaptured W2/epoch and New floating sent W2.

The server remains authoritative. Its cached view supplies current eligibility,
**not replacement user intent**. At deliberate pane-root opening, capture HostId plus
connection epoch, session ID, window ID, block ID, layer ID, close-pane IDs
paired with names, and host/tab/pane/layer display names. A host index is only
a lookup hint, never identity. Capture from the window actually containing F.

Storage: reuse root `Panel.layer`; pass that captured value through
`Intent::LayerMenu` and owned follow-up clones, without extending generic `PaneTarget`.
Rows, hover, submenu opening, confirmation and submit never recapture it.
Missing capture leaves the floating branch absent; no focused fallback.
Pane/tab/layer names stay opening snapshots, not lookup keys. Current
state and API/connection eligibility use the captured host, not the active host.
For the captured layer, current floating/minimized state determines verbs and
Restore/Focus wording; a missing layer never selects a replacement.

Structural fence: `layer_menu` accepts the captured value, not a pane to resolve.
Neither `workspace.rs::layer_menu` nor the parent row recaptures; the root row
packages the opening snapshot in its intent.
Root/submenu entries and `floating.rs` validate it. `pane_window` currently searches
session-wide (`pane_verbs.rs:21–35`); unrelated pane actions remain unchanged.

## Lifetime, follow-ups and selection

Root → submenu → dispatch/confirmation transfers the same capture through panel
replacement. **Preserve existing dismissal:** no new Back navigation, parent stack,
or return-to-submenu on Keep running/Escape. Keep running remains default;
cancel/dismiss writes nothing and ends the floating lineage. Unrelated panel
replacement or completed dispatch also ends it. Only deliberate root reopening
captures fresh context; a closed parent cannot revive it.

Close names all N captured panes and consequences. Validate their ID **set**
before/on confirmation: changed membership refuses; reorders preserve consent
and display order. N and names come from the same captured ID/name pairs.
The comparison is set equality, not display order.

Generic Commands verbs resolve focus at invocation, unlike captured context.
**R2 stays unchanged:** inventory rows use `Command.id` (`pane.layer.<layer>`),
not a new host/epoch-scoped ID scheme. Keyboard selection follows that ID;
removal refuses Enter. The submenu stays positional, without new row IDs.
Pointer hover clears keyboard identity even at the same index; Enter uses the
visible enabled row. Keyboard movement records its ID where one exists.

## Validation policy

Validate on wakes, follow-ups and dispatch/confirm; dismissal never restores a parent.
`src/app/tabs.rs:205–239` guards only `Panel.tab`, not this lineage.

- Missing/replaced host, disconnect, epoch replacement, unavailable session/window,
  or F missing/moved outside W1: invalidate the lineage; zero floating writes.
  Roots opened on pre-13 or disconnected hosts never show the row. If a root
  opened with the row remains open, it stays visible-disabled with a reason;
  existing whole-root dismissal may still occur. Submenu/confirmation dismisses
  with a reason. Reconnect cannot revive the invalid capture.
- F moves within W1 to another layer, or captured layer disappears: disable
  layer-dependent Float/Minimize/Focus/Raise/Return/Close; never adopt its new layer.
  **New floating remains valid** against original W1 and cwd-source F.
- Other panes join/leave the same layer: non-destructive verbs retain that layer;
  Close disables/refuses, including pending confirmation. Keep running/dismiss
  remains available; fresh root opening is needed to consent to a different set.
- Title, geometry, stacking/order and remote active-tab changes are benign.
  Derive last-visible, minimized/frontmost, API support and existing tiled-anchor
  eligibility now. Return may choose the current eligible anchor within W1;
  no placeholder or frozen layout facts.

Validation is against observations: a pane joining after the last observation
but before server close can still be closed. This known limit is not made atomic;
no `if_revision`, new server contract, optimistic layout or automatic replay.

## History matrix results

Rune Runtime histories: input → wake → view → recipient. Tests drive the
keyboard and pointer, wake the workspace with host reports, check the live
panel, and inspect both host queues. New floating counts the deferred
`HostCommand::NewTerminal` as one creation and checks its cwd source.
R=root, L=submenu, C=confirmation; original context H/E/S/W1/F/L1.
“Zero” means **ZERO WRITES**; after dispatch, never replay.

| Transition / wake placement | Result | Runtime test |
|---|---|---|
| Wake before R opens; F already in W2 | Fresh R names W2; New → H/E/S/W2, cwd F | `fresh_root_after_move_names_and_creates_in_the_new_window` |
| R → wake moves F to W2/removes W1 → attempt L → New | Zero; stale branch cannot open L | `the_root_menu_cannot_recapture_a_pane_moved_to_another_window` |
| R → L → same wake → New | Zero; L dismisses | `move_after_submenu_dismisses_instead_of_creating_in_the_new_window` |
| Before/after R→L or L→direct dispatch: host/epoch/session/window/F replaced or missing | Zero before send; after send, no replay | `identity_losses_on_both_sides_of_submenu_entry_and_dispatch_never_retarget_or_replay` |
| L → wake moves F within W1/deletes old layer → dispatch | New → W1/cwd F; layer verbs disabled | `pane_moved_inside_its_window_leaves_only_new_floating_eligible` |
| R → membership wake → L → Close | Close disabled/zero; Focus still L1 | `membership_between_root_and_submenu_blocks_close_but_preserves_focus_recipient` |
| L → host/epoch/window/F missing/replaced wake → C | No C opens; zero | `missing_identity_after_submenu_cannot_open_close_confirmation` |
| Wake before/after L→C; pane set changes | No new valid C; pending confirm zero, Keep running works | `changed_membership_before_or_after_close_choice_requires_new_consent` |
| C → benign reorder/title/other-tab wake → Confirm | One `session.close_layer` for S/L1; observed membership equals captured set (IDs not in payload) | `confirmation_keeps_opening_names_and_accepts_reordered_ids_despite_remote_focus` |
| C → epoch/missing-identity wake → Confirm | Zero; dialog dismissed | `pending_confirmation_dismisses_after_identity_loss` |
| Wake before/after C→Keep running/cancel | Zero; whole menu dismissed, no retained L | `keep_running_and_escape_end_the_entire_confirmation_lineage` |
| Wake before/after L→Escape/dismiss | Zero; no parent restoration | `submenu_escape_ends_the_lineage_without_restoring_the_root` |
| Dismiss → wake → deliberate R reopen | Fresh capture; New uses newly named window | `deliberate_root_reopen_after_dismiss_and_move_captures_fresh_context` |
| R/L → benign reorder/rename/remote W2 focus → New | H/E/S/W1, cwd F; never W2 | `benign_wakes_on_either_side_of_submenu_open_keep_new_in_its_opening_window` |
| Keyboard A → wake reorder/remove → Enter | A if present; zero if absent | `commands_keyboard_follows_the_id_on_reorder_and_never_substitutes_on_removal` |
| Wake before/after keyboard→same-index hover; reorder/remove | Visible enabled row, not retired keyboard ID; absent row zero | `commands_same_index_hover_retires_keyboard_id_before_and_after_inventory_changes` |
| Pointer → wake → hover B → keyboard selection A → wake | B before keyboard takeover; thereafter A by ID, or zero | `commands_pointer_keeps_position_until_keyboard_takes_back_identity` |

The disconnect regression is also checked directly by
`disconnect_does_not_put_close_pane_under_the_highlighted_layer_row`: on
`51a0b4d`, Enter opened a `block.close` confirmation. With the fix, the disabled
row retains its position, Enter stays in the pane root, and both hosts see zero
writes. `a_root_opened_without_floating_support_keeps_the_branch_hidden` checks
the inverse: a later wake does not insert a row absent at opening.

## Acceptance and gates

Red-before on the `7ca954b` implementation: the root history queued one
`session.new_layer` for `other` (W2), from `block:f`. The fixed history queues zero.
Temporarily restoring submenu recapture and allowing the stale parent row to
open reproduced the same W2 write; the mutation was removed.
The private X11 acceptance fixture checks native lifecycle and captures source
and binary identity in its receipts; the PR records the exact-head run.
R2 hover and R4 five-caller repairs remain outside rework scope.
No generic revision manager, broad refactor, slice 2, release/install/restart.

## Review basis

Astra authored; DeepSeek cross-family read requested D1–D4 clarifications,
incorporated here by the lead; this is not a final implementation PASS.
Language-neutral rat-stack: `skills/rat-stack-mode` → root intent versus current
view; `skills/uncomplect` → captured-value interface fence; `skills/test-behaviour`
and `lore/tests-that-earn-their-place` → recipient histories; `repo/AGENTS.md` and
`lore/implicit-state-machines` → scope and explicit transitions. Rust conventions win.
Three unavailable router leaves returned HTTP 422; not claimed as followed.
