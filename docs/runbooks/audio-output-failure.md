# Runbook: audio-output failure triage (engine → HAL/cpal)

Covers "no sound / stops / underruns" in the
decode → DSP → ring → output pipeline (`crates/lyra-engine`,
`crates/lyra-hal`, `crates/lyra-dsp`). The two output drivers share the
`OutputTap` ring contract: the RT callback does clear/pop/gain only.

## Symptoms

- Pressing play produces no sound, or playback stalls a few seconds in.
- Stutter/underruns on a USB DAC, especially after sample-rate changes.
- `lyra` CLI `state.get` shows playing but position never advances.

## Triage (in order)

### 1. Isolate the layer with the test suite

```sh
cargo test -p lyra-engine   # pipeline.rs: real-device output, realtime position advance, pause/resume/stop
cargo test -p lyra-hal      # enumerate/hog/rate-switch/IOProc tone, 0 underruns expected
```

- `pipeline.rs` green + no sound → suspect the device/driver, not decode.
- HAL tests failing with EAGAIN(35) right after `AudioDeviceStart`: **known
  device-settle behavior** — the driver retries ~500ms post-rate-change
  (see BLUEPRINT.md § engine). A single EAGAIN that recovers is not a bug.
- Default-device property flickering to none during transitions: also known;
  enumeration retries. Only file if it never resolves.

### 2. Check the compat path first

Force the cpal shared-mode f32 path (`OutputMode` compat) before touching
exclusive mode. If compat plays and exclusive doesn't, the fault is in the
hog/rate/format sequence, not decode or DSP:

1. Hog acquire (`kAudioDevicePropertyHogMode`, pid check).
2. Mixable-format off + stream select.
3. Physical format **and** virtual format pinned to the same integer ASBD
   from `AvailablePhysicalFormats` (never hand-roll the ASBD — 24-in-32
   `IsAlignedHigh` vs packed differs per DAC).
4. Rate set → wait on the `NominalSampleRate` listener (never sleep;
   USB reclock takes 50–500ms) → `CreateIOProcID` → `Start`.

A rate flip kills a running IOProc — per-track switching must sequence
hog → rate → settle → start. Teardown is Stop → DestroyIOProcID →
restore formats/rate → unhog.

### 3. Check DSP/ring suspects

- Interleaved-buffer math: CoreAudio reports
  `mDataByteSize = frames·ch·4` — dividing by 4 alone overruns `mData` 2×.
- Decoder-end ≠ playback-end: the `ended` flag must let the callback drain
  the ring before idling (regression = tracks cut off early).
- EQ/limiter rebuilds happen on the decode worker via commands, never by
  shared mutation on the RT thread.
- Integer-path only: f32→int + DoP packing happen in the worker; the
  callback stays memcpy-only. Volume on hardware (`VolumeScalar`) if
  present, else volume is compat-path-only.

### 4. Live-probe with the agent surface

```sh
cargo run -p lyra-cli -- state.get        # position advancing?
cargo run -p lyra-cli -- device.list      # expected device present?
cargo run -p lyra-cli -- spectrum.get     # DSP tap alive post-DSP?
```

Position frozen + spectrum dead → engine/decoder. Position advancing +
spectrum alive + silence → output driver/device.

## Escalation

File with `.github/ISSUE_TEMPLATE/bug.yml` (area:audio, priority P0 if no
workaround): macOS version, chip, DAC + connection, exclusive vs compat,
`cargo test -p lyra-engine -p lyra-hal` output, and whether the EAGAIN /
default-device flicker settled or persisted.
