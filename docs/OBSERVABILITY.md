# Observability

Lyra is a single desktop application (Rust core + Swift UI), not a fleet
of servers. There is no appliance to SSH into and no hosted dashboard —
observability here means three things:

1. **Release/deploy health** — did CI pass, did the DMG build, where is it?
2. **Local structured logs** — `tracing` events with request/operation IDs.
3. **In-process metrics** — lock-free counters read from debug tooling.

## 1. Release and deploy health

All release machinery lives in `.github/workflows/`. Check the repo's
**Actions tab** first; check the **Releases page** second.

| Workflow | Trigger | What it proves |
|---|---|---|
| `ci.yml` (`lint-test`, `macos-latest`) | push to `master`, PRs | `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` all green |
| `release.yml` (`dmg`, `macos-latest`) | tag `v*`, manual dispatch (`version`, `create_release`) | version stamped into `Info.plist`, `make app PROFILE=release`, UDZO DMG + `shasum -a 256`, `upload-artifact` (`Lyra-<version>-dmg`), attach to GitHub release via `gh release create`/`upload --clobber` |
| `semgrep.yml` (`scan`, `ubuntu-latest`) | PRs, pushes to `master`, weekly cron | static security scan |
| `style-checks.yml` | push to `master`, PRs | complexity budget, oversized-file guard, debt markers, dependency cooling-off |

Where to look, concretely:

- **Is the build healthy?** Actions → latest `ci` run on `master`.
- **Did the release go out?** Actions → `release` run for the tag, then
  Releases → `Lyra <tag>` → the `.dmg` asset (compare its `shasum`
  against the value printed in the `Package DMG` step log).
- **Release failed?** Follow `docs/runbooks/release-pipeline-failure.md`;
  for no-audio-after-update reports see
  `docs/runbooks/audio-output-failure.md`.

## 2. Logs

Logging is `tracing` throughout (`tracing-subscriber` initialized once in
`lyra_ffi::init_logging`, `crates/lyra-ffi/src/lib.rs`): filter with
`RUST_LOG`, default `lyra=info` when unset. Outbound crates that matter:

- `lyra-remote` — every connection runs inside a `remote_conn` span
  (`addr`, `conn_id` minted at accept); every phone tap runs inside a
  `remote_command` span (`request_id`, `op` index); the client side wraps
  sends in `remote_request`. Pair/connect rejections, throttle hits, and
  decrypt failures are `warn!` events with that context attached.
- `lyra-net` — retries log `op`, `attempt`, `delay_ms`, and the error;
  breaker transitions log `breaker opened / half-open / closed / re-opened`
  with the operation name.
- `lyra-engine` — decode open failures and decode errors are `warn!`
  events; hog-mode denials log at `warn`.

### Request/operation IDs

`crates/lyra-remote/src/trace.rs` is the contract. The transport is
Noise-encrypted TCP (not HTTP), so there is no `X-Request-ID` header —
instead the ID rides inside the encrypted JSON envelope
`{"rid": "<id>", "cmd": {...}}` and the server echoes it back as `rid`
in the response. Rules:

- `new_request_id()` mints 16 lowercase hex chars.
- `normalize_request_id()` honors a caller-supplied ID when it matches
  `[A-Za-z0-9-_]{1,64}` (the future HTTP-interop shape) and mints one
  otherwise — a bad ID never fails the operation.
- Bare legacy commands (no envelope) are still accepted; the server mints
  an ID so every operation traces end to end.
- Clients propagating an upstream ID use
  `Session::send_with_id(cmd, id)`.

## 3. Metrics

No scrape endpoint exists (desktop app, no server). Both metric owners
expose the same three reads — snapshot, text, log:

- `lyra_remote::Host::metrics()` → `HostSnapshot`
  (`connections_total`, `pair_attempts`, `pairings_ok`, `connects_ok`,
  `rejects`, `commands_total`, `command_errors`);
  `metrics_text()` renders Prometheus exposition-style text;
  `log_metrics()` emits one structured `info!` event.
- `lyra_engine::Engine::metrics()` → `EngineSnapshot`
  (`play/pause/resume/stop/seek/set_band_requests`, `decode_blocks`,
  `decode_errors`, `eof`, `underruns` — underruns are counted in the RT
  callback via lock-free atomics);
  `metrics_text()` / `EngineSnapshot::to_json()` / `log_metrics()`.

To inspect at runtime: call these from debug tooling/FFI harnesses, or
raise logging and grep the `remote metrics` / `engine metrics` events.

## 4. Resilience (what guards the network)

`crates/lyra-net/src/resilience.rs`:

- `RetryPolicy` (default: 3 attempts, 200 ms base, 5 s cap, exponential)
  retries only transient failures — timeouts, connects, HTTP 429/5xx
  (`is_transient_request_error`). 4xx/decode errors return immediately.
- `CircuitBreaker` (default: 5 consecutive failures → open, 30 s probe
  timeout) degrades to single probe attempts while open — never a
  synthetic error, never a retry storm.
- `LrcLib::get` and `MusicBrainz::release` both run through these;
  `breaker_state()` reports the effective state; `with_policy()` is the
  tuning/test seam. MusicBrainz 1 req/s citizenship is still the caller's
  shared rate-limiter (see the `governor` TODO in `lyra-net`); retries sit
  on top of it.

## 5. Explicitly not wired (and why)

- **Hosted error tracking / APM / alerting / product analytics**
  (Sentry, PagerDuty, APM vendors, analytics SDKs): the repo has no
  accounts, no production infra, and no consent pipeline for telemetry.
  Per `docs/PRIVACY.md` this is a privacy-sensitive local music app — no
  analytics SDK ships without consent infrastructure. What exists instead
  is vendor-neutral: structured error context on every new log event
  (request/operation IDs, attempt counts, breaker transitions) that any
  future shipper can forward.
- **Profiling instrumentation**: no sampling profiler is integrated;
  performance-critical paths (RT callback, decode worker) are documented
  with their budgets in-code. Adds no value as a checked-in stub.
