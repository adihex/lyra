---
name: qa-desktop
description: >
  QA tests for the Lyra desktop app. Native macOS SwiftUI shell
  (.build/Lyra.app) over the Rust core. Tests library, discover, coach,
  map, EQ, visuals, devices, menu-bar player, desktop pet, and onboarding
  by driving the real UI.
---

# qa-desktop Sub-Skill

**Test tool:** `droid-control` native-desktop route (`desktop-control`
driver) plus Capture and Verify. Invoke the `droid-control` skill before
interacting with the app and follow its routing tables. Load Compose only
when both `video_evidence` and `droid_control.compose` are true in
`.factory/skills/qa/config.yaml`. Never duplicate raw driver commands here;
this file describes WHAT to test, droid-control owns HOW.

## Testing Target

- Build: `make app` → `.build/Lyra.app` (ad-hoc codesigned). Launch with
  `open .build/Lyra.app`. Kill between runs with
  `pkill -f 'Lyra.app/Contents/MacOS/Lyra'` (the `make dev` pattern).
- This repo has NO preview deployments. The ONLY branch-code target is a
  local build + local launch. NEVER fall back to a remote environment (dev,
  staging, prod) when testing a PR branch; remote deployments run different
  code. If the `.app` cannot be built or no display is available, report ALL
  qa-desktop flows as BLOCKED ("No display available -- cannot drive the
  native UI") and stop. CLI flows still run normally.
- `new_user` runs use a fresh profile (empty library, default settings);
  `local_user` runs use the populated developer library. Prefer temp
  fixture dirs for scan/import so the developer DB is never modified.

## Flow Menu (the orchestrator picks only diff-relevant flows)

- **D1 launch/smoke:** app opens, main window appears, no crash on first
  paint. Success: window snapshot shows the library chrome.
- **D2 library:** browse, FTS search, sort/filter, double-click-to-play
  (`primaryAction`). Success: search narrows rows; play starts audible
  output and the position advances.
- **D3 discover:** discover pane loads; guest-DJ/radio/journey entries
  render. Success: no empty-error state on a populated library.
- **D4 coach lane:** live input lane (onset/pitch/judge/follower +
  calibration). Needs mic entitlement path; on headless CI report BLOCKED.
- **D5 song maps:** map pane renders grid/sections/chords/notes/tab from a
  `.lyramap`. Success: sections align with playback position.
- **D6 EQ:** parametric EQ dots-on-response-curve; curve comes from
  `lyra_engine_eq_response` (real coefficients). Success: band drag updates
  the curve and audible output; restore defaults after.
- **D7 visuals:** spectrum/spectrogram/waveform/meters animate during
  playback. Success: bars move at ~60fps; spectrogram scrolls.
- **D8 devices/output:** device list, hog-mode toggle, rate switching.
  Success: switch echoes in settings; hog requires a real output device
  (headless CI → BLOCKED with that note).
- **D9 remote panes (view only):** browse + queue edit + DSP presets render.
  LAN pairing handshake is OUT OF SCOPE; never attempt pairing, never revoke
  devices.
- **D10 menu-bar player:** `.window`-style mini player shows artwork,
  position, controls. Success: toggle from the main window works; sliders
  respond (`.menu` style would kill them).
- **D11 desktop pet:** pet renders over the wallpaper, theme variants load.
  Success: no crash on theme switch; pet follows the display.
- **D12 onboarding/empty states (new_user):** first-run copy, missing-file
  reconciliation, Music.app import prompt. Success: empty library shows the
  designed empty state, not an error.
- **D13 scan/import:** drop a fixture dir on the library; parallel resumable
  indexer shows per-folder progress. Success: fixture tracks appear; remove
  fixtures after.
- **D14 signal-path transparency:** chain diagram file→decode→DSP→out with
  integrity badge. Success: badge color matches the real path state.
- **D15 media keys:** play/pause/next from the keyboard. Success: keys drive
  Lyra (requires `playbackState = .playing`; otherwise keys go to Music.app
  -- assert the state first).

## Personas

- `local_user`: populated library. Full menu except pairing.
- `new_user`: empty library. Prefer D1, D12, D13 (first scan), D10.

## Authentication in CI

No login. No secrets. If `video_evidence` is true the workflow provides
`QA_EVIDENCE_TOKEN`/`REPO_ID` for uploads; the agent never mints tokens.

## Cleanup (after EVERY mutating test)

Clear the queue, restore EQ bands, remove fixture dirs, quit the app.
Verify (relaunch shows the pre-test state). Never leave the developer
library DB modified.

## Known Failure Modes

1. **No display → BLOCKED.** GitHub-hosted macOS runners and headless SSH
   sessions cannot drive the native UI. Report BLOCKED, do not retry.
2. **Ad-hoc codesign re-prompts each build.** The firewall/Local Network
   permission may re-prompt after `make app`. Allow it once per build host.
3. **Media keys go to Music.app.** Until `playbackState = .playing` is set,
   the system cannot route keys to Lyra. Set state first, then test keys.
4. **Table diffing at 100k rows.** Dataset swaps need `.id(contentID)` bump
   or the UI stalls for seconds. Stalls here are a known perf shape, not a
   hang; wait before snapshotting.
5. **Hog mode needs a real device.** Headless/VM audio devices reject hog +
   integer-format pinning. Report BLOCKED with the device name when it bites.
6. **`@State` is unavailable under CLT swiftc.** View-local state lives in
   the `ObservableObject` VM; if a view looks unresponsive, check for a bare
   `@State` regression in the diff.
7. **Rate flips kill a running IOProc.** Per-track rate switching must
   sequence hog→rate→settle→start; `AudioDeviceStart` EAGAIN(35) while the
   device settles (~500ms) is expected, retry, do not fail the test.
