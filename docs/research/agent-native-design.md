# Lyra — Agent-Native Design

Date: 2026-09-16. Reference implementation studied: `bjarneo/cliamp` (source vendored in
`./cliamp-src`, `ipc/v2.go`, `ipc/server.go`, `ipc/jobs.go`, `luaplugin/`). macOS sandbox
claims verified against Apple dev forums / QA1773 / entitlement docs (cited inline).

Goal: make Lyra a first-class citizen for AI agents, shell scripts, and other apps —
drive the whole player over a documented, versioned, machine-readable local protocol,
then layer MCP, deep links, a CLI, and a plugin runtime on top of it.

The core bet (proven by cliamp, mpv, and MPD): **one newline-delimited JSON protocol over
a Unix socket is the substrate**. Every other surface — `lyra` CLI, `lyra mcp`, `lyra://`
links, Shortcuts actions, plugins publishing state — is a thin adapter onto that socket.
Design the socket once, design it well, and everything else is near-free.

---

## 1. Local control socket

### 1.1 Where it lives

A sandboxed app can bind Unix domain sockets anywhere inside its own container — no
entitlement needed; it's ordinary file access (Apple DTS/Quinn, forums thread 126059).
WireGuard's App Store build does exactly this: the sandbox can't create `/var/run/…`
sockets, so it binds inside `~/Library/Containers/com.wireguard.macos.network-extension/Data`.

Two viable paths:

| Path | Entitlement | Notes |
|---|---|---|
| `~/Library/Containers/<bundle-id>/Data/Library/Application Support/Lyra/control.sock` | none | Zero new entitlements. Downside: macOS 14+ can TCC-prompt when a *bundled app* reads another app's container; bare binaries/scripts attribute to their responsible process (Terminal), which usually already has broad access — still, a prompt risk exists |
| `~/Library/Group Containers/<TEAMID>.lyra/control.sock` | `com.apple.security.application-groups` | **Recommended.** Group containers aren't a foreign app's container → no "access data from other apps" prompt; reachable by a sandboxed in-bundle CLI (§4) and XPC services; stable across bundle-id renames. Standard, MAS-allowed entitlement |

Recommendation: group container, socket `mode 0600`, plus a sidecar `control.sock.json`
`{"pid":…,"protocol":2,"version":"1.4.0","started":…}` for liveness/stale detection
(cliamp does the same with a `.pid` file). On launch: if socket exists but `connect()`
fails → reap and rebind; if a live process answers → refuse second instance (or forward:
see §1.5). Remove socket on clean shutdown.

The socket server belongs **in the Rust core**, not Swift: the FFI boundary already
exists, playback/queue/DB/viz state already lives there, and a tokio (or plain thread +
`poll`) listener keeps all protocol logic unit-testable without a UI. Swift only needs
`lyra_ipc_start(path)` / a callback for "UI-affecting" commands (window focus, theme).

### 1.2 Protocol — steal cliamp V2 wholesale

cliamp's v2 (`docs/remote-control.md`) is the best-designed local player IPC I've found.
NDJSON: one minified JSON object per line, both directions (mpv's `--input-ipc-server`
uses identical framing — `socat - $SOCKET` works for debugging).

**Envelope.** `{"version":2,"id":"<client-chosen>","method":"<verb>","params":{…}}`.
Responses echo `id` and always carry `version` — so events interleaved on the same
connection can't be confused with responses, and pipelined requests stay correlated
(mpv uses `request_id` the same way).

**Methods** (Lyra-shaped):

| Method | Returns |
|---|---|
| `capabilities` | Machine-readable operation list + param schemas + protocol version — agents self-discover, no docs needed |
| `state.get` | Full snapshot (below) |
| `spectrum.get` | Current viz bands (cliamp has this; Lyra's viz pipeline already computes them) |
| `operation.submit` | Starts an op → returns `{job:{id,operation,state:"queued"}}` immediately |
| `job.get` / `job.cancel` | Poll/cancel async work |
| `subscribe` | `{topics:["runtime.state","runtime.job","plugin.*"]}` → push event stream on the same connection |
| `plugin.call` | Invoke a plugin-declared command (§5) |

**Snapshot.** One self-describing object — the thing agents call most:

```json
{"version":2,"id":"s","ok":true,"snapshot":{
  "revision":18,"playlist_revision":7,
  "state":"playing","position":42.5,"duration":183.0,"seekable":true,
  "volume":0.8,"shuffle":false,"repeat":"off",
  "track":{"id":"…","title":"…","artist":"…","album":"…","source":"library|torrent|stream","provider_meta":{…}},
  "queue":{"length":12,"index":3},"eq":{"bands":[0,-1.5,…],"preamp":0},
  "viz":{"bands":[0.12,0.44,…]},"audio":{"device":"…","sample_rate":48000},
  "stream_error":null}}
```

**Revisions, the cliamp trick worth copying:** `revision` bumps on any meaningful state
change, `playlist_revision` on queue changes. Destructive ops accept
`if_revision`/`if_playlist_revision` → stale clients get `{"error":{"code":"conflict"}}`
instead of silently clobbering. This single feature eliminates the whole class of
"two controllers raced" bugs, for humans and agents alike.

**Jobs.** `operation.submit` is async-by-construction: fast ops often return
`job.state:"succeeded"` inline; slow ops (torrent resolve, library scan, provider fetch)
return `queued`/`running` and land in `job.get`. Jobs retained ~15 min, bounded.
This maps perfectly to Lyra: `library.scan`, `torrent.add`, `import.*` are naturally jobs.

**Events.** `subscribe` opens the push stream; retained topics mean a client that
connects mid-song immediately receives current `runtime.state` (cliamp: retained core
topics + `plugin.*`). Rule from cliamp worth keeping: **position ticks are not events** —
poll `state.get` for position, or you drown clients in noise. Each event carries a
monotonic `seq`; a gap → client re-`state.get`s.

Suggested event topics: `runtime.state`, `runtime.playback` (track changes, stop),
`runtime.job`, `library.scan`, `queue`, `plugin.<name>`.

**Operation set** (concrete, mapped to existing engine):

- Transport: `play pause toggle stop next prev seek.absolute seek.relative volume volume.set speed`
- Modes: `shuffle repeat`
- EQ: `eq.get eq.set eq.band.set`
- Queue: `queue.list queue.play queue.enqueue queue.remove queue.move queue.clear`
- Library: `library.search` (FTS5 → ranked results), `library.stats`, `library.scan` (job),
  `track.play track.queue` (full track objects round-trip, preserving torrent/provider meta)
- Sources: `url.load`, `torrent.add` (job, rqbit), `lyrics.get`
- Introspection: `capabilities state.get spectrum.get device.list device.set`

### 1.3 Security inside the sandbox

- `chmod 0600` on the socket dir + socket; verify peer uid with `getpeereid()`
  (`LOCAL_PEERCRED`) → reject non-same-uid. That's the entire trust boundary on macOS:
  anything running as the user can already read their music DB — document this, don't
  pretend otherwise. Optional hard mode: a random token in `control.token` (0600)
  required in `hello` — cheap defense-in-depth, breaks `socat` convenience though.
  Ship perms-only first; gate mutating ops behind token only if you later expose the
  socket beyond localhost semantics.
- Message cap (e.g. 1 MiB/line) and per-connection write backpressure so a wedged
  client can't grow queues.
- The deep-link surface (§3) shares this trust level: any local process can `open` a
  `lyra://` URL — same as any process can write to the socket. Don't add weaker auth
  on links than on the socket.

### 1.4 XPC alternative — considered, rejected (as the primary surface)

`NSXPCListener(machServiceName:)` can register a Mach service from a sandboxed app,
but sandbox rules require the service name be prefixed by an app group the app holds
(`<TEAMID>.lyra.ipc`) or a `mach-lookup.global-name` temporary exception (MAS-hostile).
Unsandboxed clients *can* `bootstrap_look_up` it — but they'd need a compiled XPC
client: no `socat`, no shell one-liners, no Python `socket` module. XPC gives typed
messages, audit-token peer identity, and launch-on-demand — genuinely better for a
bundled sandboxed helper (e.g. a login item). For agents and scripts, JSONL-over-UDS
wins on accessibility. If a sandboxed companion ever needs it, add a Mach endpoint
*alongside* — same command dispatcher behind both transports.

### 1.5 One instance, many controllers

cliamp refuses a second bind. For Lyra, prefer: second launch `open`s → forwards args
to the running instance via the socket and exits (single-instance + deep-link forward =
"open URL while running" for free).

---

## 2. MCP surface — bridge binary, not in-app server

**Recommendation: `lyra mcp` — an stdio MCP server built into the `lyra` CLI binary.**
Do not embed an HTTP/SSE MCP endpoint inside the app.

Why:

- Every real music-player MCP is a bridge: `apple-music-mcp` (JXA/AppleScript →
  Music.app), `mcp-applemusic`, `MCP-MusicAssistant`, `spotify-mcp` — all stdio sidecars
  against an existing API/socket. The pattern is settled.
- MCP transports in 2026: **stdio** and **Streamable HTTP**; HTTP+SSE is deprecated
  since spec 2025-03-26. Any HTTP listener in the app needs
  `com.apple.security.network.server` — Lyra likely already holds it for the Noise LAN
  remote, but an HTTP MCP endpoint is still the wrong shape: MCP clients overwhelmingly
  launch stdio servers; an in-app listener can't be spawned by the client config and
  dies with the app anyway.
- stdio bridge = zero new entitlements, versioned with the CLI, agents configure it as
  `{"command":"lyra","args":["mcp"]}` — done. The bridge is ~a day of work once the
  socket client exists (Rust: `rmcp`; or keep the whole thing in the same Rust binary).

Tool sketch (map 1:1 to socket ops; keep names verb_noun, return structured JSON):

| Tool | Socket op | Notes |
|---|---|---|
| `now_playing` | `state.get` (trimmed) | Track, position, state |
| `search_library` | `library.search` | FTS5; args `q`, `type`, `limit` |
| `play` / `pause` / `next` / `previous` / `stop` | `operation.submit` | Return post-op snapshot |
| `play_track` / `play_query` | `track.play` / search+play composite | Composite tool: agents want "play X" to just work — copy MCP-MusicAssistant's baked-in orchestration |
| `seek` / `set_volume` | `seek.absolute` / `volume.set` | Absolute-only (idempotent) |
| `queue_add` / `queue_list` / `queue_clear` / `queue_remove` | `queue.*` | `if_playlist_revision` supported |
| `eq_get` / `eq_set` | `eq.*` | Band array or `{"band":n,"gain":db}` |
| `library_stats` | `library.stats` | Counts, last scan |
| `library_scan` | `library.scan` job | Poll `job.get` |
| `viz_state` | `spectrum.get` | Live bands — lets agents build ambient tooling |
| `subscribe_events` | `subscribe` | Optional: expose as MCP resource/notifications; stdio MCP servers can emit notifications — nice-to-have, not v1 |
| `player_state` (resource `lyra://state`) | `state.get` | MCP resources give agents cheap reads |

Also expose MCP **prompts** later ("dj_set", "focus_playlist") — but tools+resources
cover 95% of use.

---

## 3. Deep links, App Intents, AppleScript

### 3.1 `lyra://` scheme — sandbox-safe, ~2 days

`CFBundleURLTypes` registration needs **no entitlement**; receiving URLs inside a
sandboxed app is unrestricted. Mirror cliamp's minimal design (`docs/url-scheme.md`):
`lyra://<verb>?<target>` — two verbs, four targets.

```
lyra://play?track=<library-id>
lyra://play?url=https%3A%2F%2F…            # http(s) only — validate
lyra://play?q=aphex+twin                   # top FTS match
lyra://queue?track=<id>
lyra://transport/next | /pause | /toggle   # verb-less convenience
lyra://search?q=…                          # opens UI focused on results
```

Handler → translates to socket op → forwards to running instance (or performs inline).
`open 'lyra://play?q=jazz'` works from any shell/agent. Caveats: URL opens can't return
values (fire-and-forget — pair with socket for confirmation); any local process can
open URLs (same trust boundary as socket — fine); percent-encode nested query strings.
`lyra protocol status` in CLI reports registration health.

### 3.2 App Intents / Shortcuts — the modern macOS surface, ~1 week

`AppIntents` framework + `AppShortcutsProvider`: sandbox-compatible, no entitlements,
and the actions show up in Shortcuts, Spotlight, Siri — and `shortcuts run "Lyra Play"`
is itself scriptable from a shell, so agents get it without any client code. This is
also the surface Apple Intelligence-era features will consume. Ship: Play (query),
Pause/Next/Previous, Set Volume, Add to Queue, Now Playing (returns track → composable
in Shortcuts), Start Radio/Mix. Each intent is a thin call into the same command
dispatcher — no new semantics, just parameter structs.

### 3.3 AppleScript dictionary — still worth it, low priority

An `.sdef` + `NSScriptCommand` handlers costs a few days and buys `osascript`, Alfred,
Raycast, Keyboard Maestro, Hammerspoon, and users migrating from Music.app muscle
memory (`tell application "Lyra" to play`). Sending Apple events *to other apps* needs
`com.apple.security.automation.apple-events`, but *being scriptable* doesn't. Worth
doing at P2 for ecosystem compatibility; every existing Apple Music MCP does its work
through Music.app's dictionary — a `lyra` dictionary slots into the same tooling
(`apple-music-mcp` could drive Lyra unmodified). Don't build it first: socket + App
Intents cover agents; sdef covers humans' legacy automation.

---

## 4. `lyra` CLI

One small binary, all subcommands are socket clients:

```
lyra play|pause|toggle|stop|next|prev
lyra seek <+30s|01:23>|volume <0-100>
lyra status|now-playing [--json]
lyra search <q> [--type track|album|artist] [--json]
lyra queue list|add|remove|clear|move
lyra eq get|set --band N --gain dB
lyra scan|stats
lyra events [--topics …]            # streams NDJSON to stdout
lyra open <lyra://uri>|protocol status|doctor
lyra mcp                            # §2
lyra plugins list|call <p> <cmd>    # §5
```

JSON output by default when stdout isn't a TTY (gh-style), `--pretty` for humans.
`lyra doctor`: finds socket, checks perms, pings, prints protocol/version — the single
command agents run first.

**Shipping it from a sandboxed .app** — the honest constraint matrix:

| Distribution | Mechanism | Constraint |
|---|---|---|
| Developer ID (direct) | `Lyra.app/Contents/MacOS/lyra` (or `Contents/Helpers/`); signed hardened-runtime, **no app-sandbox entitlement** | Exec'd from Terminal it runs unsandboxed → reaches any socket path. Install = `ln -sf` into `~/bin`/`/usr/local/bin` (user does it, or a "Install CLI" button that runs an `osascript do shell script` — the app itself can't write outside its container). Homebrew cask is what agents actually use: `brew install lyra` |
| Mac App Store | Every Mach-O in the bundle must carry `com.apple.security.app-sandbox` (QA1773); helper tools using `com.apple.security.inherit` **cannot run from Terminal** (nothing to inherit) | The workaround (Apple DTS): ship `lyra` as a standalone-sandboxed executable — own `app-sandbox` + `application-groups` entitlement → it runs from Terminal inside its own sandbox and reaches the **group-container socket** (another reason §1.1 picks the group container). Or skip MAS for the CLI and distribute it via Homebrew |

Decision: build the CLI to run happily *either* way — it only ever touches the socket
path and `control.sock.json`. For MAS, sign it sandboxed + app-group; for Developer ID,
leave it unsandboxed. One codebase, two signing configs.

Headless thought: cliamp's `--daemon` mode (player without TUI, same socket) is worth
copying eventually for Lyra-on-Linux or CI — but out of scope for the macOS app; flag
as a future Rust-core binary since the engine is already platform-separated.

---

## 5. Plugin/extension surface

cliamp's model (verified in `luaplugin/`): per-plugin Lua 5.1 VM (gopher-lua), stdlib
surgically stripped (`dofile/load/require/package/debug/io` removed; `os` →
time/date/clock/getenv only), `plugin.register({permissions={…}})` with **unknown
permission names rejected at load**, `cliamp.fs`/`cliamp.http`/`cliamp.exec` as
host-mediated replacements (`exec` = configurable binary allowlist; `http` blocks
private/loopback/multicast), SHA-256 trust file (`.trust.json`) — editing a plugin
re-prompts approval. Callbacks async, 5s timeout. `p:publish` emits retained
`plugin.<name>` events onto the IPC bus → **plugins become part of the agent surface
for free** (`plugin.call` from the socket).

Runtime options for Lyra's Rust core:

| | `mlua` (Lua 5.4 / **Luau**) | `rquickjs` (QuickJS) | `wasmtime` (WASM/component) |
|---|---|---|---|
| Isolation | Strip globals + `set_hook` instruction limits; Luau is sandboxed-by-design (Roblox) | Same pattern, `eval` gating | Strongest — capability-based WASI, fuel/epoch interruption, memory caps |
| Host API ergonomics | Excellent (`mlua` user data, serde) | Good | Component-model boilerplate, heavier |
| Size/dep | Tiny, `vendored` builds Luau in-tree | Small | Large (Cranelift), notable binary weight |
| Per-frame viz calls | Fine — keep band data as Lua table, callback on viz thread | Fine | Fine but more marshal overhead |
| Permission model | Implement once: manifest → gate which `lyra.*` tables get injected | Same | WASI caps give fs/net gating "for free", rest is custom anyway |
| Feasibility in Rust core | **High — ~2–3 wks for a cliamp-parity surface** | Similar | ~4–6 wks |

**Recommendation: `mlua` + Luau.** The threat model is "curated plugins from git repos,
user-approved" — not adversarial code exec — so Luau's hardening + manifest permissions
suffice; WASM's marginal isolation isn't worth component-model weight for a viz/data
API. (If plugins ever get untrusted install-by-default, revisit wasmtime.)

Plugin manifest sketch:

```lua
plugin.register({
  name = "lastfm", version = "1.0.0",
  permissions = {"events.subscribe","net.fetch","playback.control","viz.bands","keymap"},
})
p:on("track.change", function(t) … end)
lyra.viz.draw(function(bands, geom) … end)   -- live bands each frame
p:publish("now-playing", {track=…})           -- → IPC retained topic plugin.lastfm
```

Rules copied from cliamp that matter: reads (library query, state, publish, store)
need no permission; mutation (`playback.control`, `exec`, `keymap`) gated; plugins
declaratively callable over IPC (`plugin.call`); each plugin its own VM; never run
plugin code on the audio thread — viz callbacks consume band snapshots via a channel
on the render/viz thread, with instruction-count hooks to kill runaway frames.

Where plugins live: `~/Library/Group Containers/<TEAM>.lyra/plugins/` — same dir as
socket → agents and the sandboxed CLI can inspect/list them; or inside the app
container with access only via `lyra plugins` CLI. Group container again wins on
reachability. Trust file with SHA-256, re-approve on edit — copy cliamp wholesale.

---

## 6. Agent UX — the rules that make it pleasant

What separates a socket agents love from one they fight:

1. **`capabilities` first.** Self-describing op list with param schemas → agents don't
   guess. Add `lyra schema` dumping JSON Schema for every request/response.
2. **Stable error taxonomy.** `{"ok":false,"error":{"code":"not_found","message":…,"retryable":false}}`.
   Codes are an enum, never parse `message`: `not_found|conflict|invalid_param|
   not_running|job_failed|permission_denied|busy`. `retryable` is the field agents
   actually use.
3. **Absolute, idempotent ops.** `seek.absolute`, `volume.set`, `repeat.set` — not
   "volume up". Mutations take `if_revision` (optimistic concurrency) or an idempotency
   key for `queue.enqueue` (dedupe double-submits).
4. **Deterministic snapshots.** `state.get` always returns the full object, stable field
   order not required but complete self-description is — an agent should never need a
   second call to interpret the first.
5. **Jobs for anything slow.** Never block a request on torrent resolve/scan; return a
   job handle + events. Agents poll `job.get` or subscribe.
6. **NDJSON + `request id` echo.** Pipelining-safe, grep-able, `socat`-debuggable.
   One line = one message, minified.
7. **`--json` when non-TTY**, human tables otherwise; stable exit codes
   (0 ok, 2 not-running, 3 conflict, 4 invalid). `lyra doctor` as the
   "is it alive" probe; `lyra events` as the "tail -f" probe.
8. **Read-your-writes consistency.** Responses to mutating ops include the post-commit
   snapshot (cliamp jobs carry it) — saves a round-trip and proves the change landed.
9. **Version handshake.** First line on connect: server sends
   `{"version":2,"hello":{"app":"lyra","protocol":2,"capabilities":N}}` — clients know
   instantly what they're talking to.
10. **No hidden state for agents.** Everything observable via `state.get`/`*.list`;
    everything mutable via `operation.submit`. No "UI-only" features.

---

## 7. Discovery

Precedence the CLI/bridge use, in order:

1. `LYRA_SOCKET` env var (explicit override; tests, weird setups).
2. Well-known path: `~/Library/Group Containers/<TEAMID>.lyra/control.sock`.
3. Legacy/fallback: `~/Library/Containers/<bundle-id>/Data/…/control.sock`.
4. Sidecar `control.sock.json` (pid + protocol + version) → liveness check via
   `kill(pid,0)` + `hello`; stale → clear error, not a hang.
5. `lyra doctor` automates all of the above.

xattr/mDNS aren't needed for the *local* socket (a fixed path beats indirection —
mpv/yabai/AeroSpace all ship well-known paths). **mDNS is for the LAN remote:** the
existing Noise-XX remote should advertise `_lyra-remote._tcp` via Bonjour
(`NSNetService`/`NWListener` advertise — uses the network.server entitlement Lyra
already needs for LAN listening) so paired remotes and LAN agents find it without IP
config. Local agents never touch mDNS.

Bonus discoverability: ship a `docs/agent.md` + `lyra schema`/`capabilities` so an
agent can bootstrap itself end-to-end with zero human-written integration.

---

## 8. Entitlement audit (honest sandbox cost)

| Feature | Entitlement needed | Acceptable? |
|---|---|---|
| Unix socket in group container | `com.apple.security.application-groups` | Yes — standard, MAS-allowed, already common for apps with helpers |
| Sandboxed `lyra` CLI in bundle (MAS path) | `app-sandbox` + `application-groups` on the CLI binary | Yes; Developer-ID path needs nothing |
| MCP via `lyra mcp` stdio | none | Free |
| `lyra://` scheme | none (`CFBundleURLTypes`) | Free |
| App Intents / Shortcuts | none | Free |
| AppleScript sdef (receive) | none | Free |
| HTTP/SSE MCP or HTTP API in-app | `com.apple.security.network.server` | Only if we ever add it — likely already present for the Noise LAN remote; **not recommended** anyway |
| LAN mDNS advertise | covered by existing `network.server` | Already paid for |
| Lua plugins (in-process) | none | Free |

Net new entitlement burden for the whole design: **one** (`application-groups`), and
even that is avoidable if the socket lives in the app's own container and the CLI ships
unsandboxed outside MAS.

## 9. Phased plan

**P0 — Socket + CLI (≈3–4 wks).** NDJSON V2 protocol in Rust core (envelope, state.get,
operation.submit + jobs, subscribe w/ retained topics, revisions + `if_revision`,
capabilities, spectrum.get); group-container bind, 0600 + getpeereid; `lyra` CLI
(clap + serde, same Rust workspace, shared client lib); single-instance forward;
`lyra doctor`, `lyra schema`, `lyra events`. Exit criteria: `socat`, `lyra`, and an
agent with a raw socket all fully drive playback/search/queue; snapshot is complete.

**P1 — MCP bridge + deep links + intents (≈2 wks).** `lyra mcp` stdio server (rmcp),
tool list per §2 — mostly generated from `capabilities`; `lyra://` scheme + forward;
App Intents for the 6 core actions; `docs/agent.md`. (AppleScript sdef can trail into
P2.) Exit: `claude mcp add lyra -- lyra mcp` works; `open lyra://play?q=…` plays.

**P2 — Plugins (≈4–6 wks).** `mlua`+Luau in Rust core; sandbox + manifest permissions
(start: `events.subscribe`, `viz.bands`, `playback.control`, `net.fetch`, `keymap`,
`exec` with allowlist); trust store w/ SHA-256; `plugin.call` over IPC + `plugin.*`
retained topics; 2–3 bundled plugins (now-playing publisher, auto-EQ, a viz mode) as
the reference implementation; `lyra plugins` subcommands. Exit: a third-party Lua
plugin draws a visualizer and publishes events an agent consumes over the socket.

Dependencies between phases are strictly linear — each phase is independently shippable
and the socket protocol is the only thing that must be right in P0 (everything else
is an adapter). Biggest P0 risk is snapshot/operation coverage of the existing engine
surface; biggest honest constraint is MAS bundling rules for the CLI (§4).

---

### Appendix — wire example

```sh
$ echo '{"version":2,"id":"1","method":"capabilities"}' | socat - \
  ~/Library/Group\ Containers/TEAMID.lyra/control.sock
{"version":2,"id":"1","ok":true,"operations":[{"name":"seek.absolute","params":{"position_s":"f64"}}, …]}

$ lyra events --topics runtime.playback
{"version":2,"type":"event","seq":41,"topic":"runtime.playback","data":{"track":{…},"revision":19}}
{"version":2,"type":"event","seq":42,"topic":"runtime.job","data":{"id":"…","operation":"library.scan","state":"succeeded"}}
```
