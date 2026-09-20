---
name: qa-cli
description: >
  QA tests for the lyra CLI app. Thin socket client over the NDJSON IPC
  protocol (crates/lyra-cli + crates/lyra-ipc). Tests transport, queue,
  library, EQ, devices, events, and error handling by running the real
  binary against a real socket.
---

# qa-cli Sub-Skill

**Test tool:** direct process execution. No interactive driver. Run the built
binary (or `cargo run -p lyra-cli --`) with `--json` for assertions.
CLI flows never produce video; text transcripts ARE the evidence.

## Connection

- Socket resolution order: `--socket <path>` flag, then `$LYRA_SOCKET` env,
  then well-known paths (see `crates/lyra-ipc/src/paths.rs`).
- In CI there is no running app. Start the mock sidecar from
  `crates/lyra-ipc/src/mock.rs` (or a fixture socket server) on a temp
  socket dir, export `LYRA_SOCKET` pointing at it, and run the CLI against
  that. Never point scan/import tests at the developer's real library DB;
  prefer an isolated temp HOME/profile dir.
- Exit codes (§6 rule 7): `0` ok, `2` not-running (no socket), `3` conflict
  (stale revision retryable), `4` invalid (bad args, e.g. bad seek spec).

## Flow Menu (the orchestrator picks only diff-relevant flows)

- **F1 transport:** `play`, `pause`, `toggle`, `stop`, `next`, `prev`.
  Success: exit 0, `state` snapshot reflects the change (e.g. `playing`
  flips), mutating ops echo a `snapshot` (read-your-writes).
- **F2 seek/volume:** `seek +30s`, `seek -10`, `seek 83`, `seek 01:23`,
  `volume 50`, `volume 0.5`. Success: position/volume in the snapshot moves
  by the expected delta. Covers `parse_seek` parsing + `seek.absolute` /
  `seek.relative` ops.
- **F3 state/status/now-playing:** `state`, `status`, `now-playing`.
  Success: exit 0, JSON snapshot parses, one-line human status prints.
- **F4 queue:** `queue list`, `queue add --track <id>` /
  `--query <text> [--position N]`, `queue remove <index>`,
  `queue move <from> <to>`, `queue clear`. Success: `queue list` reflects
  each mutation; empty queue prints `(empty)`.
- **F5 search:** `search <q> [--type t] [--limit N]`. Success: FTS results
  return for a known term; nonsense query returns zero results (not an
  error). Covers `library.search`.
- **F6 scan (async job):** `scan [--path <dir>]`. Success: returns a job
  object with an `id`; poll `job.get` until `done`; temp fixture dir gets
  imported. Scans run as retained jobs (`is_async_op`), not inline results.
- **F7 stats:** `stats`. Success: exit 0, track/album counts parse as
  numbers. Covers `library.stats`.
- **F8 devices:** `devices`, `devices --use-device <name>`. Success: list
  shows at least one device; switch echoes the new device in the snapshot.
- **F9 EQ:** `eq get`, `eq set --band N --gain DB`,
  `eq set --bands '...'` `[--preamp DB]`. Success: `eq get` after set
  returns the written bands. Capture defaults before mutating and restore
  after (cleanup rule).
- **F10 subscribe/events:** `subscribe [topics...]` (defaults
  `runtime.state runtime.job queue`). Success: NDJSON lines stream on
  mutation; position ticks never arrive as events (clients poll
  `state.get`). Cancel with interrupt; assert at least one event per
  mutation.
- **F11 capabilities/doctor:** `capabilities`, `doctor`. Success:
  `capabilities` lists the protocol version plus the full op table;
  `doctor` reports socket, sidecar, perms, ping. A new op must appear here
  (integration check for any protocol change).
- **F12 negative tests (at least 1 per run):** bad seek spec
  (`seek banana` → exit 4), unknown method via raw socket
  (`unknown_method` error code), `queue remove 9999` on empty queue
  (error, not crash), CLI with no socket (exit 2). Never parse `message`;
  match stable `code` (`not_found`, `conflict`, `invalid_param`,
  `not_running`, `unknown_method`, ...).

## Personas

- `local_user`: populated library + settings. Full menu, including
  mutation flows (restore state after).
- `new_user`: empty library, default settings. Prefer F3/F5 (empty
  results, not errors), F6 (first scan of a fixture dir), F11.

## Authentication in CI

No login. The CLI needs no credentials. These env vars are the only knobs:

- `LYRA_SOCKET`: socket path for the test sidecar.
- `CI=true`: autonomous mode (no prompts).
- `LYRA_TEST_HOME` (when set by the harness): isolated profile dir for
  scan/import tests.

## Cleanup (after EVERY mutating test)

`lyra queue clear`, restore EQ bands to captured defaults, remove temp
fixture dirs and temp socket dirs, kill any test sidecars. Verify cleanup
(e.g. `queue list` shows `(empty)`). Never leave the developer library DB
modified.

## Known Failure Modes

1. **No socket → exit 2.** The app (or mock sidecar) is not running. Start
   the sidecar on a temp socket and export `LYRA_SOCKET` before retrying.
2. **Scan returns a job, not results.** `library.scan` and `torrent.add`
   are async (`is_async_op`). Poll `job.get` with the returned id; do not
   assert inline results.
3. **Stale revision → exit 3 (`conflict`, retryable).** Re-read state and
   resubmit with the fresh revision.
4. **Bad seek spec → exit 4.** `parse_seek` rejects anything that is not
   `+30s`/`-10`/`83`/`01:23` shaped. This is expected behavior, assert it.
5. **MusicBrainz 1 req/s governor.** Live metadata tests must serialize
   MB lookups and tolerate 429s; prefer the stubbed path when offline.
6. **Sandboxed SSH keys.** Inside the sandbox the exec-path SSH cannot read
   `~/.ssh`; remote-library flows need a bookmarked key file or the russh
   path. Report as BLOCKED with that note when it bites.
