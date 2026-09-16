# Pluggable Engines Survey — Lyra Music-Coach Stack

**Worker:** `engine-survey` · **Date:** 2026-09-16 · **Deliverable type:** component survey (no code)

Scope: every engine/library layer in `FLAC → offline analysis → note-highway game →
judged live input → practice loops → progress store`. We are deliberately **not** bound
to either existing codebase (Lyra = SwiftUI + Rust/C-ABI; dino-shred = Python +
sounddevice/pygame). Preference order: memory-safe Rust → permissively-licensed C/C++
behind our C ABI → embeddable ONNX/CoreML models. macOS arm64 required; Linux
nice-to-have; **GPL/AGPL/NC flagged loudly**.

Relationship to siblings: `transcription-pipeline.md` (sibling worker) owns the
*model-accuracy and map-schema* story; `dino-shred-pedagogy.md` §6–7 owns pedagogy and
the REUSE/NEW breakdown against current code. This doc owns the **which repo, what
license, how does it embed** layer — where the pipeline's choices (beat_this,
basic-pitch, RoFormer, DP tab solver) have already landed, this doc points at the
concrete embedding path rather than re-arguing the model choice.

All metadata (stars, license, last-push) pulled from the GitHub API / crates.io on
2026-09-16. "Active" = pushed within ~6 months.

---

## 0. Status quo (what we're comparing against)

| Piece | Today | Relevance |
|---|---|---|
| Audio out | `cpal` 0.18 output stream (compat) + `lyra-hal` CoreAudio IOProc driver (hog-mode, `coreaudio-sys` 0.2.18) | cpal is **output-only** in-tree; HAL trampoline signature already carries `_in_data/_in_time/_out_time` — full-duplex is a designed-in extension, not a redesign |
| Timestamps | `position_secs` = delivered-frames counter on cpal path; HAL gets `AudioTimeStamp` but ignores it | Needs promotion to device-clock anchoring for ms-accurate judging |
| dino-shred input | `sounddevice` (PortAudio) full-duplex, ADC/DAC timestamps, 5.33 ms blocks @48k | The semantics to match, in Rust |
| Ring | `ringbuf` SPSC (dep already); `VizTap` is output-side only | Input event ring = new but same pattern |
| Decode/DSP | symphonia (all formats), `rubato` SRC, `rustfft` | Already permissive, already in the workspace |
| Inference | none yet; BLUEPRINT commits `ort` + ONNX + CoreML EP | One runtime decision covers layers 2/3/4/7 |
| Store | sqlite via `rusqlite` (`lyra-store`) | coach/map tables = sibling migrations |

---

## 1. Duplex audio I/O

**Requirement:** simultaneous capture+playback, hardware timestamps for both
directions, sub-10 ms block granularity, device hot-swap, macOS arm64 (CoreAudio),
Linux nice-to-have. dino-shred's reference semantics: PortAudio's
`PaStreamCallbackTimeInfo{inputBufferAdcTime, outputBufferDacTime}` on one duplex
stream.

| Engine | Lang / crate | ★ | License | Last push | macOS arm64 | Verdict |
|---|---|---|---|---|---|---|
| **cpal** (`RustAudio/cpal`) | Rust, `cpal` 0.18.2 | 3.9k | Apache-2.0 | 2026-09 — very active | ✅ | **ADOPT (stay)** — details below |
| **miniaudio** (`mackron/miniaudio`) | C single-header; `maudio-sys` (MIT, 2026-09) bindings exist | 7.3k | MIT-0 / public domain | 2026-08 active | ✅ | **VIABLE-WITH-WORK** — best duplex story in C land |
| **PortAudio** + `portaudio` crate (`RustAudio/rust-portaudio`) | C + Rust bindings | 389 (bindings) | MIT | bindings 2024-10 | ✅ | **VIABLE** — semantic 1:1 with sounddevice |
| **cubeb** (`mozilla/cubeb`) + `cubeb`/`cubeb-core` crates (`mozilla/cubeb-rs`) | C++17 + Rust | 498 + 78 | ISC | 2026-09 active (ships in Firefox) | ✅ | **VIABLE** — real duplex, Mozilla-maintained |
| **RtAudio** (`thestk/rtaudio`) + `rtaudio` crate (codeberg Meadowlark) | C++ + Rust | 1.8k | custom permissive (MIT-equivalent) | 2026-09 active | ✅ | **VIABLE** — duplex streams w/ `RtAudioStreamStatus` timestamps |
| **coreaudio-rs** (`RustAudio/coreaudio-rs`) / raw `objc2-core-audio` | Rust | 297 | Apache-2.0 | 2026-06 | ✅ macOS-only | **ADOPT for the HAL path** — already Lyra's plan |
| libsoundio (`andrewrk/libsoundio`) + `soundio` crate | C | 2.1k | MIT | 2025-01 — dormant | ✅ | **SKIP** — author shelved it; rust bindings stale (2020–2023) |
| oboe-rs (`katyo/oboe-rs`) | Rust | 76 | none listed | 2024-03 | ❌ Android-only | **SKIP** — irrelevant to macOS, noted for completeness |
| Zig audio (`mach-sysaudio` lineage) | Zig | — | — | shelved | ❌ | **SKIP** — mach audio work was abandoned; nothing usable |
| rodio (`RustAudio/rodio`) | Rust | 2.5k | Apache-2.0 | 2026-09 | ✅ | **SKIP for input** — output-only by design (fine mixer if ever needed) |
| `web-audio-api-rs` (`orottier`) | Rust | 385 | MIT | 2026-09 | ✅ | **VIABLE (niche)** — full Web Audio impl on cpal; heavier than needed for a duplex tap |
| `audio-host`, `moosicbox_audio_output`, `saorsa-webrtc-audio` | Rust | small | mixed | — | ✅ | **SKIP** — wrappers around cpal, add nothing |

**The honest comparison.**

- *Duplex = the crux.* The judged-input path wants **one callback owning both
  directions on one device clock** (what PortAudio/miniaudio/cubeb/RtAudio all give
  you natively, and what an IOProc on a duplex-capable device gives you on CoreAudio).
  cpal today gives you two *separate* streams → two clocks → drift you must reconcile
  in software. **However:** cpal's duplex API *interface* merged in July 2026 (PR
  #1229) and the CoreAudio duplex backend has an in-flight implementation (PR #1096
  lineage). Within cpal 0.18.x lifecycle this likely lands. Score: **stay on cpal for
  the compat path, treat duplex as an upgrade that arrives**, and put the real
  timestamped duplex on the `lyra-hal` IOProc (see below).
- *miniaudio* genuinely could beat cpal-for-duplex today: `ma_device_type_duplex`
  hands you `pInput`/`pOutput` in a single callback with forced-lockstep rates,
  plus device-notification callbacks for hot-swap. It's one header, MIT-0, and is
  what `beat_this_cpp` (below) already embeds. The cost: a second audio backend in
  the dependency tree duplicating what cpal does. **Verdict: keep in the pocket** —
  if cpal duplex slips or HAL work stalls, `maudio-sys` + 200 lines of C ABI is a
  weekend fallback, not a migration.
- *The HAL path is the real answer.* A guitar interface (M-Track, Focusrite,
  UA Volt) is one CoreAudio device with input **and** output streams on the same
  hardware clock. `AudioDeviceCreateIOProcID` on that device delivers `inData` +
  `inTime`/`outTime` `AudioTimeStamp`s in one callback — the ADC/DAC pair dino-shred
  gets from PortAudio, but at HAL level with hog-mode exclusivity. `lyra-hal`'s
  trampoline already has the signature; the work is populating inData reads and a
  clock-translation table for the split-device case (mic on built-in, out on
  headphones — host-time ↔ sample-time via `AudioTimeStamp.mHostTime`, same math
  PortAudio does internally).
- *Hot-swap honesty:* nothing in Rust-land auto-migrates a live stream. cpal errors
  out → you rebuild; miniaudio notifies → you rebuild; HAL property listeners → you
  rebuild. Design for *rebuild-on-event* regardless of backend. `lyra-hal` already
  wraps property listeners.
- *vs dino-shred's sounddevice:* `portaudio` crate is the drop-in equivalent
  (same `inputBufferAdcTime` semantics, MIT) — but it's a 2024-era binding over a C
  lib; only worth it if we want bit-for-bit parity with the prototype during
  bring-up. **Verdict: prototyping-lane convenience, not the ship.**
- Supporting cast: `audio_thread_priority` (mozilla crate, active 2026-08 — promotes
  the callback to RT thread), `rtrb` (mgeier's — the sounddevice author's — SPSC
  ring, Apache-2, active; Lyra already has `ringbuf`, same job), `triple_buffer`.
  All trivially adoptable.

**Verdict: ADOPT the two-path plan already in Lyra's DNA** — cpal duplex when it
ships (or split streams + clock reconciliation until then), HAL IOProc duplex as the
"pro" path. miniaudio = documented fallback. PortAudio = dino-shred parity harness.

---

## 2. Onset + pitch detection — live input path (real-time)

**Requirement:** run on captured guitar audio at 5–10 ms hop, ms-accurate onset
timestamps, monophonic f0 first (rhythm/lead path), polyphony later. dino-shred's
EnergyGate is already custom Rust-portable logic; the pitch half needs an engine.

| Engine | Lang / repo | ★ | License | Last push | Latency class | Verdict |
|---|---|---|---|---|---|---|
| **SwiftF0** (`lars76/swift-f0`) | ONNX model, 96k params, 16 kHz | 186 | MIT | 2025-09 | ~10–20 ms (256-hop @16k) | **ADOPT for pitch-judged notes** — 42× CREPE speed, beats CREPE accuracy at 10 dB SNR, covers G1–C7 (guitar range ✓), WASM-proven → `ort` trivially |
| `pitch-core-onnx` (`gzivdo/pitch-core`) | Rust wrapper over SwiftF0 via ort | ~0 | Apache-2.0 | 2026-05 | same | **VIABLE** — does the ort plumbing for you; thin enough to crib instead of dep |
| `pitch-detection` (`sevagh/pitch-detection`) | Rust | 662 | MIT | 2025-01 (stable) | ~2–10 ms per frame | **ADOPT for the fast classical path** — McLeod/MPM-family + YIN-class autocorrelation; zero deps story |
| `pitch-estimate` (`hey-jj`) | Rust | ~0 | MIT | 2026-08, v0.1.0 | same | **VIABLE** — MPM+YIN per-frame; young but correct lineage |
| **aubio** (`aubio/aubio`) + `aubio`/`aubio-rs` crates | C | 3.8k | **GPL-3.0** ⛔ | 2026-04 active | — | **SKIP (license)** — *algorithm* reference only (its `specflux`/`yinfft`/`mcomb` are the classical baseline). Bindings (2021–2023) inherit GPL |
| pYIN Vamp plugin (`c4dm/qm-vamp-plugins` family) | C++ Vamp | 36 | **GPL-2.0** ⛔ | 2021 | — | **SKIP (license)** — reimplement: pYIN ≈ MPM candidates + HMM; that's ~200 lines of Rust on `pitch-detection` + `viterbi` crate |
| CREPE (`marl/crepe`, `maxrmorrison/torchcrepe`) | TF / torch | 1.4k / 524 | MIT | 2024–2025 | ~60+ ms frame @ full; `tiny` is faster | **VIABLE via ONNX export** — but SwiftF0 strictly beats it for this job; keep as eval baseline |
| PESTO (`SonyCSLParis/pesto`) | torch, ONNX official | 302 | **LGPL-3.0** ⚠ | 2025-10 | <10 ms | **VIABLE-WITH-REVIEW** — strong noisy-mic f0; LGPL ok if kept ONNX-boundary, but SwiftF0 (MIT) removes the need |
| basic-pitch (ONNX) on the input | `ort` | 5.6k | Apache-2.0 | 2025-11 | ~40–80 ms effective | **VIABLE for chord/poly judging** — run on a worker thread against the input; NeuralNote proves live deployment. Not for ms-timing |
| cycfi `q` DSP lib | C++ | 1.4k | BSL-1.0 | 2026-09 | ultra-low (bit-autocorrelation, built for guitar synths) | **VIABLE-WITH-WORK** — its pitch detector was designed *for guitar latency*; worth mining if classical path underperforms |
| `fundsp` (`SamiPerttu`) | Rust | 1.2k | Apache-2.0 | 2026-03 | RT-graph | **VIABLE** — composable DSP units if we want input monitoring FX (amp-ish tone for headphones); not a pitch detector per se |
| `audioFlux` (`libAudioFlux/audioFlux`) | C | 3.4k | MIT | 2026-03 | varies | **VIABLE** — MIT C toolbox with YIN-family pitch + onset + HPSS + chroma; the "permissive aubio" — good reference impl to FFI for offline baselines |
| Vamp host SDK (`vamp-plugin-sdk`, c4dm) | C ABI | — | BSD-3 | 2024 | — | **SKIP** — SDK is BSD and *hostable from Rust* via FFI, but every plugin you'd want (pYIN, NNLS-chroma, QM) is GPL. Ecosystem = license trap |
| `anira` (`anira-project`) / `RTNeural` (`jatinchowdhury18`) | C++ | 228 / 844 | Apache-2 / BSD-3 | 2026-09 / 2026-08 | RT-safe NN inference on audio thread | **VIABLE** — only if we run a net *inside* the callback; our design puts SwiftF0 on a worker → likely unnecessary |
| `nnnoiseless` (`jneem`) | Rust | 365 | BSD-3 | 2025-12 | RT denoise | **VIABLE** — RNNoise port for dirty-mic input gating pre-onset |

**Latency math (input path):** onset via spectral-flux/energy-gate on the callback
block = block-size latency (5.33 ms @48k/256, dino-shred-proven). Pitch: YIN/MPM need
~2–4 periods → at E2 (82 Hz) that's 25–50 ms *minimum physical* window; SwiftF0's
effective latency lands similar or better in practice with higher noise robustness.
**Design call: onset fires fast on the callback; pitch confirms on a worker thread
10–40 ms later; the judge timestamps by onset, validates pitch async** — matches
dino-shred's queue(64) architecture exactly, and means a slow pitch frame degrades
to "rhythm-correct" not "dropped event".

**Verdict:** `pitch-detection` (MPM/YIN) now, SwiftF0-via-`ort` as the robust second
opinion, EnergyGate port for onsets. aubio/pYIN/chordino stay paper-references —
GPL makes them reference material, never linked code.

---

## 3. Beat / downbeat / tempo / sections — offline map layer

| Engine | Repo | ★ | License | Last push | Verdict |
|---|---|---|---|---|---|
| **beat_this** | `CPJKU/beat_this` | 391 | MIT | 2026-05 | **ADOPT** — sibling doc's choice, confirmed here: beats+downbeats in one transformer pass, handles tempo/meter *changes* (no Markov constraint) |
| → ONNX path | `Rliop913/beat_this_onnxconvert` (export script, MIT) + `mosynthkey/beat_this_cpp` (full C++ ORT port, ships pre-converted `beat_this.onnx`, MIT, uses miniaudio+pocketFFT) | 0 / 14 | MIT | 2026-04 / 2026-07 | **ADOPT path** — ONNX export is proven twice over; port = mel front-end (rustfft) + `ort` session + small post-decode. `beat_this_cpp` is the reference implementation to crib |
| Beat-Transformer | `zhaojw1998/Beat-Transformer` | ~150 | CC-BY paper / code unlicensed-listed | 2022-era | **VIABLE** — demixed-spectrogram beat tracking (needs stems first); interesting only if RoFormer already ran — "stems improve beats" direction |
| madmom | `CPJKU/madmom` | 1.7k | code BSD-ish / **models CC-BY-NC-SA** ⛔ | 2026-03 | **SKIP (weights NC)** — DBN trackers were prior SOTA; can't ship weights. *Prototyping lane:* `imcmurray/madmom-modern` (py3.12/numpy2 fork) or `openmirlab/madmom-infer` (from-scratch numpy reimpl, verify license) to sanity-check against |
| all-in-one | `mir-aidj/all-in-one` | 844 | MIT code / madmom dep chain ⚠ | 2024-05 | **SKIP** — license chain dirty (madmom models transitively); the *joint seg+beat idea* is worth reimplementing |
| BeatNet | `mjhydri/BeatNet` | 516 | CC-BY-4.0 (code! odd) | 2026-04 | **SKIP** — online CRF tracker; we don't need online, license is a Creative-Commons-on-code smell |
| Essentia | `MTG/essentia` (+ `essentia.js`; MIT-licensed Rust bindings exist: `lagmoellertim/essentia-rs`) | 3.7k | **AGPL-3.0** ⛔ | 2026-08 | **SKIP (license)** — the full MIR toolbox; the bindings are MIT but *linking the AGPL core* is what matters. Prototype-only; never link |
| aubio tempo | aubio | — | GPL-3 ⛔ | — | **SKIP** — reference |
| librosa | librosa | — | ISC | — | **SKIP shipping** — dino-shred prototyping baseline (`beat_track`, chroma) |
| nnAudio | `KinWaiCheuk/nnAudio` | 1.1k | MIT | 2026-05 | **VIABLE (reference)** — GPU-side CQT/mel in torch; port targets if we want learned front-ends in Rust. `cqt-rs`/`qdft`/`dasp-rs` cover CQT natively |
| **msaf** (sections) | `urinieto/msaf` | 559 | MIT | 2026-05 | **VIABLE (prototype)** — python section-boundary framework; the *algorithms* (Foote checkerboard on SSM, SF, OLDA) are implementable in ~300 lines Rust on our own chroma/SSM |
| Rust-native MIR crates | `moenarch-audio-analysis-rhythm` (onset+tempo, MIT/Apache, 2026-06), `cochlea-features` (onsets/pitch/chroma/**key**, Apache/MIT, 2026-08), `resonant-analysis` (beat+pitch+onset, Apache, new), `stratum-dsp` (BPM for DJ, license NOASSERTION ⚠), `beat-track-rs` (energy-flux tempo, tiny) | all <30★ | mostly permissive | 2026 | **VIABLE-WITH-WORK** — a genuine Rust MIR micro-ecosystem appeared in 2026; immature individually but the chroma/onset/tempo primitives are legit building blocks for the chord+section layer |
| `rosa` (`danigb`) | librosa-matching features | ~86 dl | MIT | 2026-03 | **VIABLE** — pitch/chroma/mel parity with librosa if we want to port-reference features |

**Sections, honestly:** no Rust crate does structure segmentation. The Foote/SSM
novelty approach is deterministic classical DSP (self-similarity matrix from chroma
→ checkerboard kernel novelty curve → peaks) — implement in `lyra-map` on rustfft +
ndarray, verify against msaf in the Python lane. Confidence: high it's buildable,
zero that we should buy a dependency.

**Verdict:** `beat_this` via ONNX/`ort` for grid; chord layer = reimplemented
NNLS-chroma→Viterbi (GPL-free; see §layer-10 crates: `viterbi`, `cochlea-features`)
with BTC/ChordMini-family ONNX as P1 upgrade; sections = own Foote implementation,
validated in Python against msaf.

---

## 4. Polyphonic transcription — the note map

| Engine | Repo | ★ | License | Last push | Embeds in Rust? | Verdict |
|---|---|---|---|---|---|---|
| **Spotify basic-pitch** | `spotify/basic-pitch` | 5.6k | Apache-2.0 (code **and** models) | 2025-11 | ✅ **`nmp.onnx` ships in the wheel** — `ort` directly; `.mlpackage` variant → `coreml` for ANE | **ADOPT** — the pragmatic polyphonic AMT. Confidence-scored events. Known guitar failure modes (muted chugs, harmonics) handled by sibling doc's confidence policy |
| NeuralNote | `DamRsn/NeuralNote` | 2.9k | Apache-2.0 | 2025-01 | JUCE+ONNXRuntime reference | **ADOPT as proof/reference** — demonstrates basic-pitch live in a plugin; mine its frame-windowing |
| Runtime choice | `pykeio/ort` (2.5k★, Apache-2, `coreml` EP feature incl. `ComputeUnits::CPUAndNeuralEngine`) vs `sonos/tract` (3.1k, MIT/Apache, pure-Rust) vs `huggingface/candle` (21k, Apache-2) vs `coreml-native`/`candle-coreml` crates | — | all permissive | all active | — | **ADOPT `ort`** (already BLUEPRINT-committed; EP coverage incl. CoreML). tract = viable pure-Rust fallback if ORT binary weight becomes an issue; candle = where you'd port a *training* model, not needed for inference |
| demucs-rs-style pure-Rust | `tracel-ai/burn` (15.9k, Apache-2, wgpu/Metal backend) | — | Apache-2 | 2026-09 | ✅ via burn-import | **VIABLE** — the burn route is proven by demucs-rs (§7); overkill for basic-pitch |
| omnizart | `Music-and-Culture-Technology-Lab/omnizart` | 2.0k | MIT | 2026-05 (maintained!) | ❌ TF stack | **SKIP shipping** — python eval baseline only |
| MT3 / t5x | `magenta/mt3` | 1.8k | Apache-2 code | 2026-09 | ❌ JAX | **SKIP** — research-grade, unshippable stack |
| YourMT3+ | `mimbres/YourMT3` | 245 | **GPL-3.0** ⛔ *(correction: sibling doc lists Apache-2.0 — GitHub license detection + LICENSE file say GPL-3.0 as of 2026-09; verify before any use)* | 2024-11 | torch | **SKIP (license + stack)** — was the accuracy leader on mixtures; loses on both counts |
| ByteDance piano transcription | `bytedance/piano_transcription` | 2.0k | Apache-2.0 | **archived 2023** | torch | **SKIP** — piano-only; the "ByteDance guitar" the brief references doesn't exist as a repo — the *electric-guitar* line is EGDB/multi-loss-transformer (Sony CSL, below) |
| **Guitar-specific (tablature-aware AMT)** — this is the layer that matters for V3 | | | | | | |
| TabCNN | `andywiggins/tab-cnn` (original, TF, unlicensed-listed) + ONNX reproduction exists (`CrispStrobe/onnx_runtime_dart/tool/tabcnn` — per-string LogSoftmax ONNX, reproduces paper F1 0.745) | 83 | verify | 2021 | ✅ ONNX path proven | **VIABLE-WITH-WORK** — audio→(string,fret) directly, trained on GuitarSet; weak on electric/FX. A small, retrainable, ONNX-able architecture |
| FretNet / inhibition family | `cwitkowitz/guitar-transcription-continuous` + `cwitkowitz/amt-tools` + `guitar-transcription-with-inhibition` | 20–39 | MIT | 2022–2024 | torch research | **VIABLE (research lane)** — continuous-valued pitch contours streaming; the strongest open guitar-AMT line |
| TART | arXiv 2510.02597 + 2609.11904 | — | code availability unclear | 2025–2026 | — | **WATCH** — technique-aware (slides/bends/hits) + T5 string-fret; 71.8% string-fret F1 zero-shot. If code lands, P2 gold |
| EGDB / GOAT / GuitarSet / DadaGP / Guitar-TECHS | datasets: `ss12f32v` EGDB (Zenodo), `JackJamesLoth/GOAT-Dataset` (ISMIR 2025, access-gated), GuitarSet (CC-BY), `dada-bots/dadaGP` (MIT, 169★) | — | datasets vary (mostly CC) | — | n/a | **ADOPT as data/eval** — not engines, but the training+eval substrate for any guitar model we'd train or fine-tune |
| `charon-audio` (`ZCI-Tech`) | Rust stem-sep lib | 18 | MIT | 2026-01 | ✅ | see §7 — listed here because transcription quality depends on the stem front-end |

**Verdict:** basic-pitch (ONNX via `ort`) is the note-event engine — Apache-2.0
*including weights*, ONNX file ships in the wheel, ~10× realtime, NeuralNote proves
on-device. Its guitar weaknesses are a **confidence-policy problem** (sibling doc),
not an engine problem. Guitar-specific AMT (TabCNN-class, FretNet-class, TART) is a
P2 research lane — none ships today. For the *input* side polyphony, basic-pitch on
a worker thread is also the answer for "did they play the chord" judging.

---

## 5. Tablature solving — pitch → string/fret + format import

| Engine | Repo | ★ | License | Last push | Verdict |
|---|---|---|---|---|---|
| **`guitarpro` crate** (scorelib) | codeberg `slundi/scorelib` | — | MIT | 2026-08 active | **ADOPT for GP import** — Rust GP3/4/5(+verify 6/7) parser+writer; the permissive in-tree parser we need for user-supplied maps |
| **ruxguitar** | `agourlay/ruxguitar` | 207 | Apache-2.0 | 2026-08 | **ADOPT as reference/mine** — full GP3–GP7 player in Rust (nom parsers partly ported from TuxGuitar + guitarpro crate), tempo control 25–200%, per-track solo — its parser+playback model is ~our map-playback loop minus judging |
| PyGuitarPro | `Perlence/PyGuitarPro` | 367 | **LGPL-3.0** ⚠ | 2026-06 | **SKIP shipping** — dino-shred/tooling lane only (DadaGP's encoder is built on it — pinned 0.6!) |
| **alphaTab** | `CoderLine/alphaTab` | 1.8k | MPL-2.0 (weak copyleft — file-level, fine to consume) | 2026-09 | **ADOPT for GP3–8+MusicXML+alphaTex** — the standard. Embed paths: (a) its TypeScript build inside a headless JS engine (`rquickjs`/QuickJS or JavaScriptCore — importer core is mostly DOM-free) to *parse* GP → our SongMap; (b) `.NET Standard 2.0` core via .NET AOT — heavy; (c) WKWebView for *rendering*. Parsing-only via embedded JS is the interesting trick — verify alphaTab importer's DOM deps first |
| TuxGuitar | `helge17/tuxguitar` | 1.5k | LGPL-2.1 ⚠ | 2026-09 (maintained fork!) | **SKIP shipping** — format-behavior reference; ruxguitar already mined it |
| DadaGP tooling | `dada-bots/dadaGP` | 169 | MIT | 2022 | **VIABLE (data lane)** — GP→tokens encoder; useful if we train a solver; not a runtime dep |
| DP cost solver (Sayegh/Yazawa playability) | papers; `cwitkowitz/amt-tools` has tablature-translation utilities (MIT) | — | MIT | — | **ADOPT — build it** — ~200–400 lines of Rust dynamic programming (position cost + movement cost + open-string bias). Sibling doc already spec'd this as P1; no permissive standalone lib exists, and the problem is small |
| MIDI-to-Tab (masked LM) | Edwards et al., ISMIR 2024, QMUL — **no public code found** | — | CC-BY paper | — | **WATCH/reimplement** — BART masked-LM on DadaGP beat the DP solvers in a user study; P2 research lane if we want a learned solver |
| Capo/tuning detection | — | — | — | — | **ADOPT — build it** — estimate global cent offset from frame f0/chroma peaks vs 12-TET (librosa `estimate_tuning` equivalent = ~50 lines on our chroma); capo = infer from minimum observed fret in solved tabs. DadaGP's normalizer (capo→fret shift) is the reference semantics |
| Chart formats (.chart/.mid for Clone Hero/YARG) | `FireFox2000000/Moonscraper-Chart-Editor` (BSD-3, Unity ref), YARG.Core (LGPL ⚠) | 297 | BSD-3 / LGPL | — | **VIABLE** — `.chart` is a trivial text format — parse in 100 lines of Rust; unlocks a *massive* human-authored chart corpus (the same shortcut Rocksmith CDLC provides via `rstoolkit`/EoF — GPL, reference only) |
| `midly` | `kovaxis/midly` | 197 | Unlicense | 2024 | **ADOPT** — SMF parse for .mid charts/MIDI maps |
| `musicxml` crate | `hedgetechllc/musicxml` | 19 | MIT | 2024-11 | **VIABLE** — MusicXML parse for notation imports |
| `acorde-*` crates | score model + MusicXML/MIDI/MSCZ parse + SVG render | — | MIT? verify | 2026-09 (new!) | **WATCH** — a fresh pure-Rust score stack appeared Sep 2026; too young to adopt, right shape to track |

**Verdict:** `guitarpro` crate + ruxguitar parser lineage for GP import (both
permissive, both Rust); own DP solver for string/fret (no lib exists, problem is
small); alphaTab consumed at arm's length for the long tail of formats. Human-made
maps are the *graded* layer — this is where the reliability lives.

---

## 6. Time-stretch — practice tempo

**Requirement:** tempo down to ~40–60% with pitch preserved, good enough transients
for practicing to; real-time for live use, offline for pre-baked artifacts.

| Engine | Repo | ★ | License | Last push | Verdict |
|---|---|---|---|---|---|
| **signalsmith-stretch** | `Signalsmith-Audio/signalsmith-stretch` | 552 | MIT | 2026-01 | **ADOPT (quality path)** — header-only C++, genuinely top-tier quality (community consensus ≈ Rubber Band R3 territory), drops behind our C ABI trivially. `ssstretch` crate (MIT) bindings exist (repo link stale — vendor or rebind) |
| **timestretch** | `robmorgan/timestretch-rs` | 24 | MIT | 2026-09, v0.15 | **ADOPT (Rust-native path)** — pure-Rust hybrid PV+WSOLA, only dep rustfft (already ours), real-time engine w/ keylock ±20%, 12.7 ms pipeline, alloc-free callback. EDM-tuned presets but the engine is general. Best-in-class *Rust* stretcher, new but serious (CI artifact gates, architecture doc) |
| **Bungee** | `bungee-audio-stretch/bungee` | 348 | MPL-2.0 | 2026-08 | **VIABLE-WITH-WORK** — modern C++ granular stretcher, RT+offline, per-grain continuous tempo/pitch, zero/negative speeds (smooth scrub!). API is low-level (JUCE community reports friction vs signalsmith "just works") — the interesting stretcher to watch, not first pick |
| Rubber Band | `breakfastquay/rubberband` | 780 | **GPL-2.0** ⛔ (commercial license available) | 2025-03 | **SKIP (license)** — R3 is the quality reference; GPL kills it for a proprietary app unless we buy the commercial license — note as *commercial option* |
| SoundTouch | codeberg `soundtouch` (+ stale rust bindings) | — | **LGPL-2.1** ⚠ | maintained | **VIABLE-WITH-REVIEW** — LGPL dynamic-link is workable but signalsmith (MIT) is same-job better-licensed; SoundTouch quality < both above on music |
| `wsola` / `rodio-wsola` | `jhheider/pdcst` etc. | small | MIT/Apache | 2026-07 | **VIABLE (V1 fallback)** — pure-Rust WSOLA, time-domain only; fine for speech-rate-class slowdown, artifacts on dense mixes |
| `pitch_shift` | `NathanRoyer/pitch_shift` | 10 | none listed ⚠ | 2026-04 | **VIABLE** — phase vocoder, unlicensed repo — verify |
| `tdpsola` | crate | — | — | 2020 | **SKIP** — stale, formant-preserving speech algorithm |
| `sonic` (`ernestrc/sonic-rs`) | speech rate | 1 | MIT | 2018 | **SKIP** — speech-optimized |
| ztx | Zynaptiq commercial SDK (ZTX LE free tier: 1ch ≤48k, no dynamic params) | — | commercial | — | **SKIP** — commercial; listed for completeness as the quality ceiling reference |
| PaulStretch/NessStretch (`spluta/TimeStretch`, `essej/paulxstretch`) | extreme stretch | 124 | GPL ⚠ | 2024 | **SKIP** — 8×+ ambient stretch, wrong product (practice needs 0.4–1.2×) |

**Verdict:** `signalsmith-stretch` for the offline pre-bake path (the
transcription-pipeline's "render stretched copies at map-gen" plan) **and** as the
RT path if timestretch quality disappoints; `timestretch` (pure Rust, RT-safe) is
the in-callback live-tempo engine. Both MIT. Rubber Band's quality is not worth GPL
or a commercial license given signalsmith exists.

---

## 7. Source separation — stem dropout + cleaner AMT input

Offline-only. Front-end for both "transcribe the stem" and "minus-guitar karaoke".

| Engine | Repo | ★ | License | Last push | Verdict |
|---|---|---|---|---|---|
| **HT-Demucs ONNX** | export tooling: `StemSplit/demucs-onnx` (MIT, prebuilt ONNX on HF incl. **htdemucs_6s = drums/bass/vocals/other/guitar/piano**), `sevagh/demucs.onnx` (MIT, C++ ORT), upstream `facebookresearch/demucs` (MIT, archived but models stable) | 5 / 67 / 10.4k | MIT | 2026 | **ADOPT** — the export blockers are solved (STFT/istft moved outside the net — host does it in rustfft). `ort` + CoreML EP. htdemucs_6s gives the guitar stem directly |
| **demucs-rs** | `nikhilunni/demucs-rs` | 143 | Apache-2.0 | 2026-08 | **ADOPT (Rust-native alternative)** — pure-Rust HTDemucs on **Burn+wgpu → Metal GPU on macOS**, VST3/CLAP + WASM builds exist. This is the "no onnxruntime binary" path and it's GPU-accelerated — likely the *fastest* per-song stem time on M-series |
| `stem-splitter-core` | `gentij` (crate; AivinJoy mirror) | 24 | MIT/Apache per crates.io | 2026-04 | **VIABLE** — htdemucs via ort as a ready-made crate w/ model registry + SHA-256 cache — basically the `lyra-map` stem stage pre-built; evaluate vs. owning the 300 lines |
| **BS-RoFormer / MelBand-RoFormer** | arch: `lucidrains/BS-RoFormer` (MIT, 942★), training: `ZFTurbo/Music-Source-Separation-Training` (MIT, 1.5k★, active), infer: `openmirlab/bs-roformer-infer` (MIT, 48★) | — | MIT arch / **per-checkpoint licenses vary** ⚠ | 2026 | **ADOPT (quality lane)** — better than demucs on stems; ONNX export exists: `elicwhite/bs-roformer-sw-6stem-onnx` on HF (336 MB fp16, 6 stems incl. guitar, validated vs reference.npz). Guitar-specific: `becruily/mel-band-roformer-guitar` ckpt (used by `0ji54n/guitar_backingtrack`). **Verify checkpoint license before bundling/download-gating** |
| spleeter | `deezer/spleeter` | 28.5k | MIT | 2026-06 | **SKIP** — superseded in quality by both above; TF stack anyway |
| open-unmix | `sigsep/open-unmix-pytorch` | — | MIT | — | **SKIP** — older, weaker |
| UVR/MDX model zoo | various HF | — | per-model ⚠ | — | **VIABLE** — quality sources exist; license per-checkpoint is the gate |

**Verdict:** two runnable paths — `ort`+ONNX (demucs-onnx or BS-RoFormer exports,
CoreML EP) or pure-Rust `demucs-rs` (Burn/Metal). Sibling doc's
stem+mix-fused-AMT design stands; this layer supplies the stem. Ship as
on-demand model download (`~/Library/Application Support/Lyra/models/`,
SHA256-pinned) — matches BLUEPRINT's existing model-fetch pattern.

---

## 8. Game / render loop — where does the highway live?

**The honest call up front:** the note-highway is a 2D scrolling event renderer with
judgment flashes — *not* a game engine problem. Per pedagogy doc §7.2f it's literally
a `VizMode` + Canvas renderer + judgment-event FFI channel, and the codebase already
has the pattern (`LyraVizFrame` polled at ~60 Hz → `Canvas`).

| Option | Repo/stack | ★ | License | Verdict |
|---|---|---|---|---|
| **SwiftUI Canvas + (if needed) MTKView/Metal** | in-app | — | — | **ADOPT** — highway = polylines/rects/text at 60 Hz + judgment edge-triggers (the `VizState` beat-edge pattern already exists). Escalate to a CAMetalLayer under NSView only if Canvas misses frame budget — it won't for this workload |
| `wgpu` + `vello` | gfx-rs/linebender | 18k / 4.3k | Apache-2 | **VIABLE** — the answer *if* the trainer ever becomes standalone/cross-platform (Linux! Windows!) — GPU 2D renderer with the headroom for particles/highway at any res |
| `macroquad` | not-fl3 | 4.6k | Apache-2 | **VIABLE** — fastest path to a *standalone Rust* rhythm-game prototype (dino-shred's spiritual Rust port); immediate-mode, audio via... you'd still need an audio engine |
| `egui` | emilk | 30.6k | Apache-2 | **VIABLE** — if a standalone tool UI is wanted (map editor/correction UI), immediate-mode fits perfectly; not a 60fps-scrolling-graphics engine |
| `ggez` | ggez | 4.7k | MIT | **VIABLE** — same class as macroquad, slightly heavier |
| `skia-safe` / `tiny-skia` | — | — | BSD/MIT | **VIABLE** — 2D raster if embedding Skia in-app; MTKView is simpler on macOS |
| Bevy | bevyengine | 48k | Apache-2 | **SKIP** — ECS game engine inside a SwiftUI app = impedance mismatch + ~100× the binary/complexity budget for a scrolling-lanes renderer |
| Game audio engines (if standalone): `kira` (1.1k, Apache-2), `Firewheel` (BillyDM, 308★, Apache-2, new 2026, bevy_seedling integration), `oddio` (166★, stale), `fundsp` (Apache-2) | — | — | permissive | **VIABLE (standalone lane only)** — Firewheel is the interesting new one (real-time-safe graph, RT-priority aware); irrelevant while the game lives in-app where `lyra-engine` *is* the audio engine |
| Reference games | YARG (`YARC-Official/YARG`, LGPL-3 ⚠, Unity), Performous (GPL ⚠), FoFix (GPL ⚠), Clone Hero (closed) | — | — | **SKIP as deps; STUDY as design** — YARG/Core's chart ingestion + hit-window UX and Moonscraper's authoring model are the patterns to read, not link |

**Verdict:** in-app Canvas first (zero new deps, matches VIZ-CONTRACT, Swift draws
what Rust scored). Keep `wgpu+vello` as the documented escape hatch for a
cross-platform standalone build. No game engine. If a standalone Rust trainer ever
exists: `macroquad` or `wgpu`+`vello` + `Firewheel`/`kira` + the same `lyra-coach`
judge crate — the engine split stays identical.

---

## 9. Notation / tab rendering — displaying maps

| Engine | Repo | ★ | License | Last push | Verdict |
|---|---|---|---|---|---|
| **alphaTab** | `CoderLine/alphaTab` | 1.8k | MPL-2.0 | 2026-09 | **ADOPT for GP/score display** — renders GP3–8/MusicXML/alphaTex to SVG/Canvas incl. full tab+techniques; alphaSynth (SoundFont playback) included. In-app path: WKWebView hosting the JS build (proven), or offline pre-render to SVG in the JS engine trick (§5). MPL-2.0 is file-copyleft — linking fine |
| Verovio | `rism-digital/verovio` | 927 | **LGPL-3.0** ⚠ | 2026-09 | **VIABLE-WITH-REVIEW** — C++ MEI/CMN→SVG with official wasm build; best-in-class *engraving*, but LGPL and it's common-notation-first (tablature support thin) — wrong tool unless we render staff scores from MusicXML |
| OSMD | `opensheetmusicdisplay` | 2.0k | BSD-3 | 2026-09 | **VIABLE** — MusicXML→notation in browser only; if we show MusicXML in a WebView anyway it's the easy path, but alphaTab covers more of our formats |
| VexFlow | `vexflow/vexflow` | 235 (org fork) | MIT | 2026-08 | **VIABLE** — JS notation primitives; no tab; WebView-only |
| `acorde-render-svg`/`acorde-io` | crates, Sep-2026 new | — | verify | — | **WATCH** — pure-Rust/WASM score model + SVG renderer just appeared; exactly the right shape, weeks old |
| Custom SwiftUI tab render | — | — | — | — | **ADOPT for the highway** — the note-highway IS the tab display in a Rocksmith-style UI; a scrolling fretboard/string view drawn from SongMap is ~Canvas work, not an engraving problem. Reserve alphaTab for "show me the real score" mode |

**Verdict:** highway/tab HUD = our Canvas renderer (it's a game element, not
engraving); alphaTab (MPL-2.0) via WebView or pre-rendered SVG for full-score mode;
Verovio only if staff-notation engraving becomes a requirement (LGPL gate).

---

## 10. What the brief didn't name — the gaps this survey found

| Need | Engine | Repo | ★ | License | Verdict |
|---|---|---|---|---|---|
| **Score-following / played-take↔map alignment** | Matchmaker (real-time alignment: online DTW Dixon/Arzt, HMM, SKF followers) | `pymatchmaker/matchmaker` | 81 | Apache-2.0 | **VIABLE (reference)** — the *algorithms* for "where is the player in the chart right now"; Python lib, port the follower that wins to ~300 lines of Rust |
| Offline audio↔map sync | SyncToolbox (DTW audio↔MIDI), partitura (score reps) | `groupmm/synctoolbox` (verify license), `CPJKU/partitura` | 142 / 372 | MIT-ish / Apache-2 | **VIABLE (prototype/eval lane)** — aligning a recorded take to the map for post-analysis |
| DTW in Rust | `augurs-dtw` (grafana, MIT/Apache), `dtw_rs` (MIT), `dtw` | crates | — | MIT-class | **ADOPT** — onset-sequence DTW for take-alignment is ~free |
| **AEC — mic hears the speakers** | `sys-voice` (MIT — macOS **VoiceProcessingIO**: OS-level AEC/AGC for free), `webrtc-audio-processing` (tonarino, webrtc APM wrapper), `sonora`/`sonora-aec3` (dignifiedquire, BSD-3 **pure-Rust WebRTC AEC port**, 143k dl), `decibri-aec` (Apache-2) | — | 7 / — / 81 / 13 | permissive | **VIABLE — real product decision** — headphones-first UX avoids this entirely; if open-mic-through-speakers is a supported mode, `sys-voice` (VoiceProcessingIO AU) is the zero-dep macOS answer, sonora the Rust-native one |
| Denoise pre-detector | `nnnoiseless` (jneem, BSD-3 RNNoise port) | — | 365 | BSD-3 | **VIABLE** — cheap input cleanup for noisy rooms |
| **MIDI guitar input path** | `midir` | `Boddlnagg/midir` | 827 | MIT | **VIABLE** — Jammy/Fishman/hex-pickup owners get trivially-accurate judging; midir is CoreMIDI-backed on macOS. Feature-flag, ~200 lines |
| RT-thread utilities | `audio_thread_priority` (mozilla), `rtrb` (mgeier), `triple_buffer` | mozilla/mgeier | — / 344 | ISC / Apache-2 | **ADOPT as needed** — promote callback to RT, SPSC event ring (ringbuf already in-tree does the same job) |
| Forced alignment / lyrics | `wav2vec2-rs` (CTC forced alignment, wgpu) | crate | — | — | **WATCH** — lyric-timing → chord-boundary hints is a cute P3 fusion signal |
| CQT/chroma/spectral base | `cqt-rs` (MIT), `qdft`, `dasp-rs`, `spectrograms`, `audioFlux` (C, MIT) | — | — | MIT-class | **ADOPT selectively** — chroma for the chord layer, CQT for front-ends; audioFlux is the permissive kitchen-sink if a C FFI is acceptable |
| Chord diagrams (display) | `tombatossals/chords-db` | — | — | verify | **VIABLE** — static chord-shape database for the HUD |
| Ear-training / curriculum HUD | `rsear` (149 dl), GNU Solfege (dead, GPL), `cochlea-synth`/`fundsp` for drill audio | — | — | — | **SKIP engines** — curriculum is product logic (pedagogy doc §8); the *content* (interval/chord drills) is data + our judge, not a library |
| Vamp ecosystem | SDK BSD / plugins GPL | §2 | — | — | **SKIP** — covered above |

---

## 11. Recommended stack

### Best-in-class (what I'd build)

| Layer | Engine | Why | Integration surface | Confidence |
|---|---|---|---|---|
| Duplex I/O | **cpal (compat) + `lyra-hal` IOProc duplex (pro path)**; miniaudio fallback | cpal in-tree; HAL already designed for it; miniaudio if both stall | `lyra-engine` + `lyra-hal` | high |
| Onset (input) | **port dino-shred EnergyGate** to Rust (spectral-flux/energy on callback block) | proven in the prototype, ms timestamps, zero deps | new `lyra-coach` crate | high |
| Pitch (input) | **`pitch-detection` (MPM/YIN) + SwiftF0 ONNX via `ort`** as robust path | classical latency now; 96k-param MIT net for noisy/distorted input | `lyra-coach` + `ort` in `lyra-map` | high |
| Beat grid | **beat_this → ONNX → `ort`** (export via Rliop913 script or beat_this_cpp's prebuilt) | SOTA-class, MIT, meter changes, ONNX proven twice | `lyra-map` | high |
| Chords | **reimplemented NNLS-chroma→Viterbi** (own ~200 lines on rustfft+`viterbi`), upgrade path BTC/ChordMini ONNX | GPL-free, deterministic, "N.C." state | `lyra-map` | high |
| Sections | **own Foote/SSM** on rustfft (validate vs `msaf` in python lane) | no permissive lib exists; algorithm is classical | `lyra-map` | medium-high |
| Notes (map) | **basic-pitch `nmp.onnx` via `ort`** (CoreML EP) | Apache-2 incl. weights, confidence-scored, ships ONNX | `lyra-map` | high |
| String/fret | **own DP playability solver**; MIDI-to-Tab reimpl as P2 | small problem, no lib exists | `lyra-map` | medium-high |
| Tab/GP import | **`guitarpro` crate + ruxguitar parser lineage**; `midly` for .mid/.chart | permissive Rust parsers exist now | `lyra-map`/`lyra-formats` | high |
| Time-stretch | **`timestretch` (Rust, RT) + `signalsmith-stretch` (C++, quality/offline)** | both MIT; RT-in-callback + best-quality bake | `lyra-dsp`/`lyra-engine` | high |
| Separation | **htdemucs_6s / BS-RoFormer-6stem ONNX via `ort`**, or `demucs-rs` (Burn/Metal) | guitar stem exists in both; Burn path = pure Rust+GPU | `lyra-map` | medium-high (checkpoint-license check) |
| Judging | **own judge** (port dino-shred `Judge`) + `matchmaker` follower ported as needed | pedagogy doc owns this | `lyra-coach` | high |
| Highway render | **SwiftUI Canvas** (VizMode pattern); MTKView escape; `wgpu+vello` if standalone | zero new deps, contract exists | `app/` SwiftUI | high |
| Score display | **alphaTab** (WebView or headless-JS pre-render) | the standard, MPL-2.0 OK | `app/` WKWebView or map-gen | medium-high |
| Progress store | **rusqlite in `lyra-store`** (coach_* tables) | substrate exists | `lyra-store` | high |
| Inference runtime | **`ort` + CoreML EP** everywhere | one runtime for all models | `lyra-map` | high |

### "If we built nothing custom" (fastest shippable V1)

| Layer | Pick | Cost |
|---|---|---|
| Input | `portaudio` crate duplex (sounddevice parity) or miniaudio via `maudio-sys` | semantics match dino-shred today |
| Onset/pitch | `pitch-detection` + `pitch-core-onnx` (SwiftF0) | two crates, done |
| Map | `stem-splitter-core` (stems) → `ort`+basic-pitch ONNX → `guitarpro` crate import for graded content | stems+notes+human maps with zero model work |
| Grid | `beat_this_cpp` ONNX artifact + `ort` (or moenarch/beat-track-rs for a quick tempo floor) | proven artifact exists |
| Stretch | `timestretch` crate | drop-in |
| Highway | SwiftUI Canvas | in-app |
| Display | alphaTab in WKWebView | proven embed |
| Store | rusqlite | exists |

The delta between the two columns is mostly *ownership of the DSP seams* (own onset
detector, own Viterbi, own DP solver, own Foote) — each is 200–400 lines of Rust the
ecosystem genuinely lacks, and owning them removes the last license edges.

### License wall (loud version)

- **Never link:** aubio GPL-3 · Essentia AGPL-3 (incl. essentia.js, and essentia-rs
  bindings inherit) · pYIN/NNLS-chordino/QM-vamp GPL-2 · Rubber Band GPL-2 ·
  madmom *weights* CC-BY-NC-SA · YourMT3 GPL-3 *(correction vs sibling doc)* ·
  Gist GPL-3 · TuxGuitar/PyGuitarPro/Verovio LGPL-3 (Verovio is dynamic-link
  workable; the rest are reference-only) · YARG/Performous/FoFix/EoF GPL family.
- **Verify per-checkpoint:** RoFormer/Demucs community model weights (ZFTurbo MSST
  models are generally MIT-architecture but checkpoint terms vary on HF).
- **Free and clear:** everything in the recommended table is MIT/Apache/BSD/MPL —
  the entire stack ships in a proprietary app with attribution.

### Things to watch (not ready, right shape)

- `acorde-*` (Sep-2026 Rust score stack), TART code release, cpal duplex backend
  landing (interface merged Jul 2026), Bungee maturity, `moenarch`/`cochlea`/
  `resonant` Rust-MIR crates hardening, Firewheel for any standalone lane.

---

## Appendix — per-layer one-liners for the record

- **Lyra's existing viz `BeatDetect` is not a beat tracker** (pedagogy doc §7.1 —
  confirmed; it's a visual transient pulse, never feed the chart).
- `web-audio-api-rs` can also render the *offline* graph (decode→stretch→mix→export
  stretched artifacts) — viable alternative orchestration if `lyra-map` wants a
  graph abstraction.
- `matchmaker`'s followers solve "player position in chart" which doubles as the
  A-B-loop auto-follow and the "wait for me" practice mode — worth porting beyond
  judging.
- Chart-corpus shortcut: `.chart` (Moonscraper/Clone Hero) parsing is ~100 lines —
  a second source of human-graded maps alongside GP files.
- `sonora` (pure-Rust WebRTC AEC) existing at 143k dl means open-speaker judging is
  a buildable feature, not a research project — but headphones-first is still the
  right default UX.
