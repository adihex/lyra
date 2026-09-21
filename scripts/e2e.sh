#!/usr/bin/env bash
# e2e.sh — drive a live Lyra host over its IPC socket and assert the app's
# observable contract: scan → library rows → play → next/prev → pause →
# stop. The same script runs on macOS (against the real .app) and Linux
# (lyra-gui under Xvfb, or lyrad headless) — the host differs, the wire
# doesn't.
#
# Env:
#   LYRA_BIN        path to the `lyra` CLI           (default: lyra on PATH)
#   LYRA_E2E_HOST   path to lyrad or lyra-gui        (Linux/headless host)
#   LYRA_E2E_APP    path to .build/Lyra.app          (macOS host — `open` it)
#   LYRA_E2E_DIR    scratch dir                      (default: mktemp -d)
#   LYRA_E2E_SOCKET explicit socket path             (default: <dir>/control.sock)
#   LYRA_E2E_NO_XVFB=1  don't wrap lyra-gui in xvfb-run
#   LYRA_E2E_KEEP=1 keep the scratch dir + host log on exit
#
# Audio posture: if the host machine has an output device (or a null ALSA
# sink) transport asserts run fully; otherwise the script still covers
# scan/search/queue/state and reports the transport section as skipped.

set -euo pipefail

LYRA="${LYRA_BIN:-lyra}"
DIR="${LYRA_E2E_DIR:-$(mktemp -d)}"
FIXTURES="$DIR/fixtures"
DATA="$DIR/data"
SOCK="${LYRA_E2E_SOCKET:-$DIR/control.sock}"
HOST_BIN="${LYRA_E2E_HOST:-}"
APP="${LYRA_E2E_APP:-}"
LOG="$DIR/host.log"
HOST_PID=""

mkdir -p "$FIXTURES" "$DATA"

say()  { printf '\n== %s\n' "$*"; }
fail() { printf 'E2E FAIL: %s\n' "$*" >&2; exit 1; }

cleanup() {
    if [ -n "${HOST_PID}" ]; then
        kill "${HOST_PID}" 2>/dev/null || true
        wait "${HOST_PID}" 2>/dev/null || true
    fi
    if [ "${LYRA_E2E_KEEP:-}" != "1" ]; then
        rm -rf "$DIR"
    else
        echo "kept $DIR (host log: $LOG)"
    fi
}
trap cleanup EXIT

# CLI wrapper — no --socket on macOS so the app's container path is
# discovered the same way a user runs it.
lyra() {
    if [ -n "$APP" ]; then
        "$LYRA" --json "$@"
    else
        "$LYRA" --json --socket "$SOCK" "$@"
    fi
}

jcheck() {  # jcheck '<python expr over data>'  — asserts on JSON from stdin
    python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception as e:
    print(f"bad json: {e}", file=sys.stderr); sys.exit(1)
ok = eval(sys.argv[1], {"d": d, "len": len, "any": any, "all": all, "isinstance": isinstance})
if not ok:
    print(f"assert failed: {sys.argv[1]}\n{json.dumps(d)[:800]}", file=sys.stderr)
    sys.exit(1)
' "$1"
}

say "fixtures"
python3 - "$FIXTURES" <<'PY'
# WAVs with a RIFF LIST/INFO chunk — lofty reads INAM/IART/IPRD as
# title/artist/album so the FTS contract is exercised too.
import math, struct, sys, wave, os
os.makedirs(sys.argv[1], exist_ok=True)
for name, hz, secs in [("tone-a.wav", 440, 2.5), ("tone-b.wav", 523, 2.0), ("tone-c.wav", 659, 1.5)]:
    p = os.path.join(sys.argv[1], name)
    with wave.open(p, "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(44100)
        frames = bytearray()
        for n in range(int(44100 * secs)):
            frames += struct.pack("<h", int(12000 * math.sin(2 * math.pi * hz * n / 44100)))
        w.writeframes(frames)
    stem = name[:-4]
    payload = b"INFO"
    for k, v in [(b"INAM", f"Fixture {stem}".encode()), (b"IART", b"E2E Artist"), (b"IPRD", b"E2E Album")]:
        v += b"\0"
        payload += k + struct.pack("<I", len(v)) + v + (b"\0" if len(v) % 2 else b"")
    with open(p, "ab") as f:
        f.write(b"LIST" + struct.pack("<I", len(payload)) + payload)
    with open(p, "r+b") as f:  # grow RIFF size to cover the LIST chunk
        f.seek(4); f.write(struct.pack("<I", os.path.getsize(p) - 8))
print("wrote 3 wavs")
PY
ls "$FIXTURES"/*.wav >/dev/null || fail "fixture generation"

say "start host"
if [ -n "$APP" ]; then
    open "$APP" >>"$LOG" 2>&1 || fail "open $APP"
elif [ -n "$HOST_BIN" ]; then
    if [[ "$HOST_BIN" == *lyra-gui* && "${LYRA_E2E_NO_XVFB:-}" != "1" ]] && command -v xvfb-run >/dev/null; then
        xvfb-run -a "$HOST_BIN" --socket "$SOCK" --data-dir "$DATA" >>"$LOG" 2>&1 &
    else
        "$HOST_BIN" --socket "$SOCK" --data-dir "$DATA" >>"$LOG" 2>&1 &
    fi
    HOST_PID=$!
else
    fail "set LYRA_E2E_HOST (lyrad/lyra-gui) or LYRA_E2E_APP (.app bundle)"
fi

# wait for the socket/CLI to answer
up=""
for _ in $(seq 1 80); do
    if [ -n "$APP" ]; then
        lyra capabilities >/dev/null 2>&1 && { up=1; break; }
    else
        [ -S "$SOCK" ] && lyra capabilities >/dev/null 2>&1 && { up=1; break; }
    fi
    sleep 0.25
done
[ -n "$up" ] || fail "host never came up — see $LOG (LYRA_E2E_KEEP=1 to keep)"

say "capabilities"
lyra capabilities | jcheck 'isinstance(d, list) and len(d) > 0 or isinstance(d, dict) and len(d.get("operations", d.get("capabilities", []))) > 0' \
    || fail "capabilities empty"

say "scan library"
lyra scan --path "$FIXTURES" >/dev/null || fail "scan op"
for _ in $(seq 1 240); do
    lyra stats 2>/dev/null | jcheck 'd.get("tracks", 0) >= 3' 2>/dev/null && break || true
    sleep 0.5
done
lyra stats | jcheck 'd.get("tracks", 0) >= 3' || fail "scan never indexed fixtures"

say "search (fts)"
lyra search tone | jcheck 'len(d.get("results", [])) >= 3' || fail "search returned <3"

say "queue/state"
# host drains library.reload after the scan job → published queue grows
for _ in $(seq 1 60); do
    lyra state 2>/dev/null | jcheck 'd.get("queue", {}).get("length", 0) >= 3' 2>/dev/null && break || true
    sleep 0.5
done
lyra state | jcheck 'd.get("queue", {}).get("length", 0) >= 3' || fail "queue never populated"
lyra queue list | jcheck 'len((d.get("queue") or {}).get("items", [])) >= 3' \
    || fail "queue.list missing items"

say "transport"
lyra next >/dev/null || fail "next"
sleep 0.5
STATE=$(lyra state)
if echo "$STATE" | jcheck 'd.get("track") is not None' 2>/dev/null; then
    # output device exists — full transport contract (track order isn't
    # asserted, just that next/prev actually step the queue)
    FIRST=$(echo "$STATE" | python3 -c 'import json,sys; print(json.load(sys.stdin)["track"]["path"])')
    [ -n "$FIRST" ] || fail "track has no path"
    echo "$STATE" | jcheck 'd.get("queue", {}).get("index", -1) >= 0' || fail "queue.index unset"
    lyra next >/dev/null && sleep 0.4
    SECOND=$(lyra state | python3 -c 'import json,sys; print(json.load(sys.stdin)["track"]["path"])')
    [ -n "$SECOND" ] && [ "$SECOND" != "$FIRST" ] || fail "next didn't advance the queue"
    lyra prev >/dev/null && sleep 0.4
    lyra state | jcheck 'd.get("queue", {}).get("index", -1) == 0' || fail "prev didn't return to 0"
    lyra pause >/dev/null || fail "pause"
    lyra play >/dev/null || fail "play(resume)"
    lyra volume 0.5 >/dev/null || fail "volume"
    lyra eq get | jcheck 'len(d.get("bands", [])) > 0' || fail "eq.get"
    lyra seek 0.5 >/dev/null || fail "seek"
    lyra stop >/dev/null || fail "stop"
else
    echo "no output device — transport play asserts skipped (scan/search/queue covered)"
fi

say "state shape"
lyra state | jcheck '"track" in d and "queue" in d and "playlist_revision" in d' \
    || fail "state missing fields"

say "PASS"
