# Rex server preview.14 compatibility

Verified on Linux x86-64 on **2026-11-09 (PST)** for the next client patch release,
0.4.1. The client now advertises **API revisions 10–12**, with uploads still gated
on revision 11. No server lifecycle operations were added to attachment.

## Server and isolation

- Release: `0.1.0-preview.14`, commit
  `7e460d02f51573990bfcdc6c44a5606d11c7d47a` (Linux amd64).
- Server build time: **2026-11-09 13:00:41 PST**.
- Archive: `rex-linux-amd64.tar.gz`, from the preview channel's
  [release manifest](https://blob.superlogical.com/api/releases/rex/preview).
- SHA-256: `36ece39653ecc4c1a95ae0ddf9a728f9a3df2ab663488b970f991b1378ff940a`.
- Extracted directly under `~/.cache/agent-ws/rex-preview14/bin`; no installer was
  executed. In particular, **`install.sh --user` also runs `rex server install`**
  in this release, so it is not suitable for an isolated binary-only test.
- The foreground server had a private Unix socket, home, XDG directories, empty
  Lua config and state directory, with `--restore=false`. Every CLI/API call used
  the explicit `REX_SERVER` and `--autostart=false`; test scripts used that binary
  through a scratch-only `PATH`. No existing server, package or service was changed.
- The keymap script creates a second private preview.14 server, using its own Lua
  fixture and explicit socket. It terminates that process by PID. The main
  throwaway server is likewise stopped by PID after verification.

## Actual protocol diff

Baseline: the previous SDK capture, `0.0.1793910724-g2e71f8`
(`2e71f83491f43b3999641a64b28b7397d0e38ce2`, API 11).
Both servers' live `/llms.txt` documents were compared; the baseline binary was
also run on a separate private socket, not queried through an existing server.
The new `rex api list --json` and `server.version` captures are committed in
`vendor/rexrs/codegen/`, regenerated with `python3 codegen/generate.py`.

The complete semantic change to the API help capture is two added properties:

```text
+ server.status.output_schema.properties.listeners
+ session.bootstrap.output_schema.properties.server_status.properties.listeners
```

Both have the same schema:

```text
listeners: null | array<object>  (optional; older revisions omit it)
  endpoint: string              (required; configured endpoint)
  transport: string             (required; unix/named_pipe/tcp/tailscale)
  addresses: null | string[]    (optional; actual bound addresses)
  admission: string            (optional; peer_credentials/anonymous/
                                tailscale_owner/tailscale_identified/login_server)
  primary: boolean             (optional; true for the primary listener)
```

Unknown transport/admission strings are preserved. Built-in Tailscale remains in
`tailscale`, not this list. The private server reported one Unix listener, its
socket path as the bound address, `peer_credentials` admission and `primary: true`.
The CLI's `rex server status --json` also includes this list.

- **74 API methods before and after; none added or removed.** All other method
  schemas, descriptions, examples and validation notes are identical.
- HTTP and block-type help sections are identical (including terminal APIs).
- No event schema changed. The only `/llms.txt` changes are revision 11 → 12 and
  the two listener descriptions above.
- `client.keymap`, `client.key`, `client.action`, `client.perform`, `client.reply`
  and their event/error semantics are unchanged.
- No SDK upstream revision bump was needed: this is a refresh of the existing
  vendored generator's inputs and outputs, retaining the local negotiation and
  explicit-empty-bootstrap patches. Tests cover omitted/null listener lists,
  optional fields, actual bound addresses and unknown enum-like strings.

## Adopted

- Negotiate revision 12, retaining 10 and 11. Tests accept each supported
  revision and reject 9 and 13. SDK status/bootstrap responses decode listeners.
- Answer `client_perform_requested` on its originating connection with
  `client.reply`, the original `call_id`, and `not_supported` plus a useful
  explanation naming the requested action. No interface action is executed.
  Directed requests bypass session snapshot sequence filtering. API errors
  (including an expired call's `not_found`) and reply timeouts are logged at debug
  without disconnecting the host; replies are never retried.
- Preserve `ssh -L` forwarding and make socket lookup explicitly
  `rex server status --autostart=false`. When no server is found, explain that
  Rex Linux only attaches and suggest manually running
  `rex server install 'ssh://[user@]host[:port]'` on Linux. The hint preserves SSH
  aliases, optional user, port and bracketed IPv6; a custom socket can still be
  entered instead. The app does not run the suggested command.

The shipped `rex skill show` guide says to inspect an app's actions before using
`client.perform`, show refusal messages to the person, and never enable Remote
Control on their behalf. Rex Linux has no Remote Control toggle: remote interface
execution is unimplemented, hence `not_supported`, not a pretend configurable
`refused` state. Local shortcuts and queued actions returned by `client.action`
continue to work normally. The app’s actions listed by `client.inspect` are
**local-only**; advertising them does not make them remotely callable. Requests
from other clients via `client.perform` are refused with `not_supported`.

## Deferred deliberately

- Native SSH auto-start was deferred in 0.4.1. For 0.4.2, a separate, explicitly
  confirmed Start personal server action invokes native `rex -S ssh://…` once,
  only after successful SSH socket discovery reports no default server. It checks
  local/remote API-12 support first, creates no session, and reconnects using the
  unchanged `ssh -L` transport. Attachment still needs only OpenSSH. Installation
  and service management remain manual. See [0.4.2 verification](0.4.2-verification.md).
- Listener-driven Add host discovery was deferred in 0.4.1 and is now implemented
  for 0.4.2: bounded, read-only control hello and `server.status` requests enrich
  the existing endpoint checks. Add host shows the endpoint that answered and
  reviews actual bindings and admission rules before Connect. Missing/null/empty
  metadata (including API 10/11), status failures and unknown rule strings are
  handled without inventing access or reachability. No session is attached by
  discovery. Listener addresses never redirect probes to unverified hosts.
- Executing remote UI actions or cross-server `client_call_requested` work.
  The client advertises no cross-server `servers` list. Remote control needs a
  separate product/permission design, not a patch-sized implicit enablement.
- Service shutdown persistence, no-logind routing, installer/offline workflows:
  these are server/CLI fixes, not client wire changes. No service installation,
  shutdown, remote auto-start or Alpine deployment was exercised here.

## Caveats

A remote `rex` older than support for `--autostart=false` will reject
`rex server status --autostart=false`. Socket discovery suppresses that error
and falls back to the default socket paths; it cannot discover a custom socket
through that older CLI. Supply an explicit `ssh://host/path/to/socket` instead.

## Verification

Builds used an isolated `CARGO_TARGET_DIR` under the scratch directory. The
release binary was passed explicitly as `REX_BIN` to the UI scripts.

- `cargo test --locked --all-features`: **287 passed, 2 ignored** (129 library,
  157 headless UI, 1 fixture). Ignored tests are the pre-existing CJK-font
  environment check and known restored/resized Ghostty search-order limitation.
- `cargo build --locked --release --bin rex-linux`: **passed**.
- `cargo fmt --all -- --check`: **passed**.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: **passed**.
  The existing vendored Rune `unused_mut` dependency warning is still emitted;
  no new client lint warnings remain.
- `cargo test --manifest-path vendor/rexrs/Cargo.toml --all-features`:
  **156 passed**, including six doctests; ten live tests ignored by default.
  All **10 live SDK tests** were then run against the private preview.14 socket
  and passed (nine blocking, one async).
- `scripts/keymap-acceptance.py`: **passed** on its private preview.14 server,
  including layered bindings, sequences, Lua forwarding, queued local actions,
  override/config reload, unbound Kitty keys and `pane.send_key`. Added live
  checks make two remote action requests and verify an explicit refusal message,
  no timeout, and no created tab.
- `scripts/x11-smoke.py`: **passed** against the explicit preview.14 socket:
  forms, host entry, clipboard, exact pointer selection, raw Kitty key isolation,
  releases and modified keys.
- `rex-probe` and `rex-workspace-probe`: **passed** with
  `REX_EXPECT_REVISION=12`: snapshot/resume, Unicode, stable reconnect identity,
  bootstrap tickets, input, ownership, sessions, layouts, split/resize/zoom and
  external changes. Repeated against a private baseline API-11 server with
  `REX_EXPECT_REVISION=11`: **passed**. Revision 10 is covered by mock negotiation,
  not a newly installed revision-10 server.

One initial full-suite run hit the existing closed-loopback-port tailnet test
with `Connection reset by peer` rather than `Connection refused`; two subsequent
complete runs passed unchanged. Fixed its released-port race by reserving the
port with a bound, non-listening socket for the duration of the test. The final
full suite and repeated focused runs passed with that fix.

No Wayland acceptance, remote SSH installation or service lifecycle claims are
made. Local raw captures and build/UI/probe logs remain in the scratch directory;
only protocol captures, generated types, tests and documentation are committed.
