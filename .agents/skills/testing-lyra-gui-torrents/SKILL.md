---
name: testing-lyra-gui-torrents
description: How to smoke-test lyra-gui (GTK4) and the lyra-torrent BitTorrent path on a headless Linux VM — null-PCM audio device, loopback seeder via the seed_track example, magnet entry in the Library pane, IPC socket for state asserts.
---

# Testing lyra-gui + lyra-torrent on a headless Linux VM

## Audio device on device-less VMs (AWS etc.)

cpal sees no output device and `lyra_engine_new()` returns null, which silently
disables the `play_torrent`/`play_path` paths and makes `scripts/e2e.sh` skip
its whole transport section. `snd-dummy` may not exist (AWS kernels ship
without sound modules). Fix: a userspace null ALSA PCM — no kernel module needed:

```sh
printf 'pcm.!default { type null }\n' > ~/.asoundrc
```

cpal's "default" then resolves to the null sink; the host logs no
"no output device" warning, `play_torrent` runs stream→decode→output for real,
and e2e.sh runs the full transport asserts. Caveat: the null sink doesn't pace
— position advances faster than realtime (a 6 s file "plays" in <1 s), so poll
`lyra state` immediately after triggering play and assert `state=playing` +
position advancing rather than sampling slowly.

## Loopback torrent swarm (no external network needed)

`cargo run -p lyra-torrent --example seed_track -- /path/file.flac 16100`
creates a single-file torrent, hash-verifies, seeds on 127.0.0.1:16100 and
prints a magnet with an `x.pe=127.0.0.1:16100` peer hint. Lyra's `add_opts`
extracts `x.pe` into `initial_peers`, so a trackerless magnet resolves metadata
from the loopback peer. Generate fixtures with
`ffmpeg -f lavfi -i sine=frequency=440:duration=N:sample_rate=44100 -ac 2 out.flac`.

Port note: `EngineConfig.listen_port_range` collapses to `r.start` under
librqbit 9 (`listen: ListenerOptions`) — no range fallback. A seed_track on the
same port as a test's seeder makes `TorrentEngine::new_with_config` fail with
"rqbit session: error starting listeners". Give every concurrent seeder a
distinct port.

## GUI surfaces for torrents

- Library pane → "Add torrent" button reveals an entry accepting a magnet URI
  *or* a local `.torrent` path. Add is off-main-thread (≤45 s metadata wait).
- Added torrents appear as chips; audio files (is_audio ext filter) appear as
  `torrent://id/idx` rows; double-click a row → `lyra_engine_play_torrent` →
  rqbit `stream()` → decode → output.
- IPC `operation.submit {operation:"torrent.add", params:{magnet|path}}` also
  works against the running GUI (same in-process engine), but does NOT refresh
  the GUI list (the host's `torrent.added` handler only logs). Prefer the GUI's
  own Add field.
- `state` shows `track:null` while a torrent row plays — `current_track()`
  can't resolve `torrent://` ids into the library table. Judge playback by
  `state`/`position`/`viz.bands`, not the track field.
- Session persistence (`<data-dir>/torrents/.session`) restores managed
  torrents + piece bitmaps across relaunches — relaunch the app to verify.

## Populating the library via the GUI (no IPC shortcut needed)

"Scan folder…" opens a GTK `FileChooserDialog`. Headless scripting of the
chooser: click the button, then `Ctrl+L` reveals the location bar — type the
absolute fixture dir, `Enter` to confirm the folder row, then `Enter` again
(or click "Select") to dismiss. `Host::sync_dir_blocking` runs and the rows
reload via `Msg::ScanDone`. For an empty-library check use a fresh
`--data-dir`; the empty-state swap is driven by `table_stack` pages
(rows/empty/filtered) — filter a non-matching string to reach the "filtered"
page without deleting the library.

## Running the GUI for recording on this box

`DISPLAY=:0 ./target/debug/lyra-gui --socket /tmp/x/control.sock --data-dir /tmp/x/data`
— the computer tool captures :0 (the real desktop, not Xvfb :99). Maximize via
`wmctrl -i -r <winid> -b add,maximized_vert,maximized_horz`. Get the window id
from `wmctrl -l` (title is `lyra-gui`). Keep stderr → a log file; boot lines
confirm rqbit session/DHT/persistence.

## Devin Secrets Needed

None for the loopback swarm. Real-network torrent tests need outbound UDP
(trackers/DHT) + TCP peer egress — check by adding a well-seeded public torrent
(e.g. Debian netinst .torrent from cdimage.debian.org) and watching
`stats.finished`/file bytes.
