# Rex Rune SDK patch

Vendored from `https://github.com/a10y/rexrs` at
`780be3f9f0455c758273e57199af30bdaa145cf1`. The upstream manifest declares
`MIT OR Apache-2.0`; that revision ships no license text files, so the standard
`LICENSE-MIT` and `LICENSE-APACHE` texts were added here.

Local changes:

- `codegen/help.json` and `version.json` capture Rex `0.1.0-snapshot.2670`,
  commit `b070aae645f5cbfac65d65d031a8b712b2ebca36`, API revision 24. Both
  are verbatim outputs of `rex api list --json` and `server.version` on a
  disposable server. The catalog was byte-identical to snapshot 2646, so
  the generated files did not change. `python3 codegen/generate.py`
  regenerates the committed types and example tests. The API additions since
  upstream are `session.bootstrap.data_tickets` (8), layer `state` and
  `session.set_layer_state` (9), the Tailscale `control_url` (10) and
  `terminal.upload_begin` (11), optional `listeners` in `server.status` and
  `session.bootstrap.server_status` (12), `terminal.download_begin` (13), and
  revisions 14 through 21: `com.superlogical.terminal.ports`,
  `com.superlogical.tunnel.info`, `client_prompt_requested` /
  `client_prompt_ended`, `event_dropped`, `focus_history.reverse`,
  `undoable` on `block.close` / `session.close_window` / `session.destroy`
  plus `client.undo_close`, and `tiled_zoomed_block_id` on `session::Window`.
  Revisions 22 through 24 add `session.transfer_window`, the filesystem
  block's upload and download methods, and `program_resume_records` with
  `program_resume_changed`. Rex Rune's resume reader does not use the
  generated response type.
  Because `com.superlogical.terminal.ports` shares its process object shape
  with `com.superlogical.terminal.process`, `generate.py` names that shared
  struct `terminal::PortsResponseProcess` instead of `terminal::Ancestor`
  (nothing in `rex-rune` references `Ancestor`). No upstream revision bump or
  generator changes were needed. See `docs/server-api24.md`, `docs/server-api21.md`,
  `docs/server-api13.md` and `docs/server-preview14.md` in the client
  repository for the comparisons.
- `ClientBuilder::api_revisions(min..=max)` passes an explicit supported range
  to HELLO. The SDK default still advertises only its generated revision.
  Rex Rune advertises 10-24, validates WELCOME and keeps per-feature revision
  gates (uploads on 11, SSH setup on 12, floating layers and downloads on 13).
- A focused negotiation test accompanies the builder change.
- Hand-written `tests/listeners.rs` covers the optional listener metadata in
  status and bootstrap, including omitted/null lists and unknown enum-like strings.
- Hand-written `tests/download.rs` covers API-13 request serialization, optional
  cache validators, ticket-bearing and not-modified responses, and required
  metadata.
- Hand-written `tests/zoom.rs`, `tests/undo.rs`, `tests/focus_history.rs` and
  `tests/ports.rs` cover API 14-21 types: `tiled_zoomed_block_id`
  absent/null/set on `session::Window`, optional `undoable` flags and
  `client.undo_close` responses, minimal and full `focus_history.reverse`
  responses, and `com.superlogical.terminal.ports` responses with unknown
  `status` values and optional `pid`.
- The generator preserves `attach_session_ids` as `Option<Vec<String>>`:
  omitting it selects a default session, while an explicit `[]` attaches none.
  A serialization test covers this distinction.

Tests: `cargo test --manifest-path vendor/rexrs/Cargo.toml --all-features`.

Remove this patch once upstream has equivalent generated types and negotiation.
