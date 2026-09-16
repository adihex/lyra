# Lyra — Transcription Pipeline: FLAC → Note/Chord Map → Guitar Tablature

Date: 2026-09-18 · Status: research/design. **No code.** This document owns the technical
middle of the Rocksmith-style flow — how a song file becomes a beat grid, chord track, note
map, and playable tablature, how good each stage can realistically be in 2026, and how the
product degrades when it isn't.

**Sits on:** `dino-shred-integration.md` (boundary, coach endpoint contract) and
`dino-shred-pedagogy.md` (pedagogical spine, representation ladder, measurement schema).
This document does not repeat pedagogy; it produces the artifact pedagogy consumes. Where
pedagogy says *"the map generator"* or *"`map_conf` field"*, this is the thing that fills it.

**Verified against:** `~/lyra-src` @ HEAD, `~/lyra-research/dino-shred` @
`feat/guitar-dino-v1` (a8c9e2e), plus published models/papers/datasets cited inline.

---

## 0. Executive opinion

Three honest claims up front:

1. **The beat grid + chord track is the product.** Beats, downbeats, sections, and
   chord-with-bass are achievable today at quality that supports a Rocksmith-adjacent
   strum-along mode, entirely on-device, with permissively-licensed models, in tens of
   seconds per song. This is the 80% value at ~20% complexity claim made precise below.
2. **Note-level guitar tab from arbitrary FLACs is research-grade, not product-grade.**
   The best published multi-instrument AMT systems score ~0.6 note-F1 on mixed-pop
   benchmarks (2025 AMT Challenge, arXiv:2603.27528); specialized solo-guitar tablature
   systems reach ~0.8–0.9 tablature-F **on clean solo recordings only**. On real commercial
   mixes, expect a usable-but-flawed map on a good song and a broken map on a bad one.
   The design therefore ships note maps as *confidence-carrying hypotheses* with a
   human-edit path, never as ground truth.
3. **Every stage runs offline, on-device, in Rust.** Decode is already solved
   (`TrackDecoder` in `crates/lyra-formats`). All inference is ONNX via the `ort` crate
   with the CoreML execution provider — the runtime strategy `BLUEPRINT.md` §data-ML
   already commits to. No Python, no network dependency, no GPL/AGPL code in the app
   (aubio, chordino, Essentia, madmom's *models* are all disqualified on license —
   details in §2). The only real-time audio in the whole system is the **user's guitar
   input**, which the pedagogy doc already owns.

The pipeline is tiered by construction: **grid → chords → notes → tab**, each layer
independently optional, each carrying confidence, and each able to fail without sinking
the ones below it. A song whose note layer collapses still has a chord track; a song whose
grid collapses is honestly reported as "cannot be charted" instead of emitting garbage.

---

## 1. The contract: what the artifact is

The pipeline's output is a single versioned artifact per song, the **song map**
(`.lyramap`, §4.5). Everything downstream — the highway renderer, the judge, the
difficulty system, the measurement schema — reads this and nothing else.

Layers, in generation order:

| Layer | Content | Produced by | Consumed by |
|---|---|---|---|
| **Grid** | `beats[]`, `downbeats[]`, meter map, swing ratio, per-beat confidence | beat_this ONNX | Conductor (chart positioning), quantizer, strum chart |
| **Sections** | labeled segments (verse/chorus/solo…) with boundaries snapped to bars | chroma self-similarity | looping, difficulty, "map confidence" UI |
| **Chords** | root/quality/bass segments + strum onsets + posterior confidence | HPCP-chroma + HMM (P0), conformer ONNX (P1) | strum-along charts, harmony display |
| **Notes** | quantized+raw onset/offset, MIDI pitch, bend cents, per-note confidence | basic-pitch ONNX on mix **and** guitar stem, fused | note highway, judge, tab solver |
| **Tab** | per-note (string,fret) + techniques (palm-mute, slide, bend, harmonic) + solver score | constrained DP/Viterbi solver conditioned on tuning+capo | tab rendering, fingering hints |
| **Layers** | per-note difficulty-layer bitmask (Rocksmith NLD-style) | density selection over notes+chords | adaptive difficulty |
| **Quality** | per-section and global confidence aggregates, stage status flags | rollup | product degradation policy (§7) |

Two clocks coexist deliberately: **audio time** (seconds, for the judge and for sync with
playback position) and **grid time** (bar:beat:tick, for rendering and notation). Every
timed event carries both. This is the same split dino-shred already makes — obstacle
position derives from beat index while judgment happens in stream-clock seconds
(`dino-shred/docs/decisions/0004`, `rhythm/conductor.py`).

Who consumes what:

- **dino-shred `Conductor`** — spec'd for exactly this: *"V2 constructs Conductor from an
  extracted `beats[]` array — `np.searchsorted` for nearest_beat"* (spec §2, conductor.py,
  ADR-0004). Our `grid.beats[]` drops straight in. No constant-BPM assumption survives;
  `beat_time(n)` becomes a table lookup.
- **dino-shred `Judge`** — windows stay ±30/±60/±100 ms (`rhythm/judge.py:19`), but the
  judge learns to *read per-note policy* from the map (graded/advisory/ghost, §5).
- **Lyra UI** — a `VizMode` sibling (`app/Sources/LyraApp/Viz/VizMode.swift`) renders the
  highway + tab; sections/quality drive the map-confidence badge.
- **Pedagogy measurement schema** — the `pitch_target`, `pitch_conf`, `map_conf`,
  `feedback_mode` fields in `dino-shred-pedagogy.md` §8 are populated from
  `NoteEvent.conf` / `TabEvent.conf` defined here.

---

## 2. State of the art, 2026 — honest survey

### 2.1 Taxonomy first, because the terms get conflated

Four different problems are all called "transcription":

1. **Monophonic f0 tracking** — one note at a time (singing, whistling, a *single* guitar
   line). Solved-ish: YIN/pYIN family and CREPE-family neural nets both work well.
2. **Polyphonic AMT** — note onsets/offsets/pitches in a mixture. This is where the
   quality cliff lives. Piano SOTA (Kong et al. 2020, `bytedance/piano_transcription`;
   hFT-Transformer, Sony ISMIR 2023, 5.5M params) hits ~0.97 onset-F1 / ~0.83–0.87
   note-with-offset F1 on MAESTRO — because piano is percussive, well-dataset'd (200h
   MAESTRO with ~3ms-aligned labels), and timbrally uniform. Multi-instrument mixtures
   are far worse: the **2025 AMT Challenge** (arXiv:2603.27528) reports note-F1 ≈ 0.60
   for the best system (MIROS), 0.59 for a YourMT3 MoE variant, 0.39 for the MT3
   baseline — and 0.063 for unconditioned basic-pitch. That is the honest ceiling on
   "transcribe the guitar out of a finished rock mix."
3. **Instrument-specific AMT with tablature** — joint pitch+string estimation for guitar.
   Real but narrow: TabCNN (Wiggins, standard tuning, solo audio), FretNet/amt-tools
   (cwitkowitz, MIT), FretNet-style six-string softmax models. Best numbers (~0.8+ tab-F)
   are on GuitarSet-style *isolated* audio, standard tuning.
4. **Structure models** — beats, downbeats, meter, sections, chords. Separate, simpler,
   and much more solved than note AMT. This is why §6 sequences chords first.

### 2.2 The model/library table

Every entry: what it does, license (the binding constraint — Lyra is a proprietary app),
Apple-silicon runtime character, offline-ability, honest accuracy, polyphony limit.

#### Polyphonic AMT

| System | License | Runtime | Offline | Honest accuracy | Polyphony |
|---|---|---|---|---|---|
| **Spotify Basic Pitch** (`spotify/basic-pitch`, ISMIR 2022 demo) | Apache-2.0 (code + the `model.onnx` shipped in the wheel) | ~10× realtime, <20 MB peak mem per vendor; CQT+harmonic-stacking front end, 3-head CNN (frame/onset/contour). ONNX export ships in package → `ort`+CoreML works; NeuralNote (Apache-2.0) proves on-device deployment | ✅ | GuitarSet-style solo guitar: note-F1 in the low 0.7s in the paper; trained partly on GuitarSet. Full mixes: collapses (0.063 note-F1 unconditioned on the 2025 AMT Challenge mixture benchmark — it has no instrument awareness) | Polyphonic by design; honest limit ~4–6 concurrent voices before spurious/dropped notes dominate |
| **MT3** (Google/Magenta, ICLR 2022) | Apache-2.0 code; JAX/T5X | Impractical in Rust — JAX stack, T5X checkpoints; re-export to ONNX is a project | ✅ (research) | Multi-instrument seq2seq; ~0.39 note-F1 on the 2025 challenge mixture benchmark; better on cleaner/stemmed input | Full mixtures; weak on under-represented instruments vs. its training mix |
| **YourMT3 / YourMT3+** (mimbres, MLSP 2024) | Apache-2.0 (code; PyTorch) | PyTorch research stack; ONNX export = P2 spike | ✅ | 2025 challenge: YPTF-MoE variant ≈0.59 note-F1 — current open SOTA-ish for multi-instrument | Same caveat: mixture eval, instrument-conditioned decoding helps |
| **Kong et al. piano transcription** (`bytedance/piano_transcription`) | Apache-2.0 | PyTorch | ✅ | 96.7% onset F1 on MAESTRO — the anchor showing what a *well-solved* AMT target looks like; **not applicable to guitar** | Piano only |

#### Monophonic f0 (user-input path + melodic-line assist)

| System | License | Runtime | Notes |
|---|---|---|---|
| **CREPE** (marl/crepe, ICASSP 2018) | MIT | tiny→full capacity: 486k–22M params, TF; torchcrepe for PyTorch | Monophonic only. ~360-bin 20-cent pitch classes C1–B7. On-device viable at `small`/`tiny`; still heavier than classical DSP for the mic path |
| **pYIN / YIN** | pYIN Vamp plugin is **GPL** (disqualified in-app); algorithm is published (Mauch & Dixon 2014; de Cheveigné & Kawahara 2002) — clean-room Rust impls exist as crates (`pitch-detection` McLeod, `pitch-estimate` MPM+YIN, `yin_rs`) | Real-time trivially | The right latency class for the *input* detector (5–10 ms hops), monophonic only, octave errors under distortion |
| **libf0** (groupri/libf0) | MIT | Python (NumPy) | YIN, pYIN, Melodia-inspired, SWIPE in one package — the prototyping/reference impl, not the shipped runtime |
| **PESTO** (Sony CSL) | **LGPL-3.0** — usable via dynamic-link/ONNX-boundary reading, needs license review | ~130k params, self-supervised, ONNX export official, <10 ms algo latency | Strong mic-path candidate if LGPL posture is accepted; otherwise SwiftF0 |
| **SwiftF0** (lars76) | MIT | 96k params, 389 KB ONNX, ~42× faster than CREPE, 16 kHz, 46.9–2093.75 Hz | Sweet spot for the live pitch judge: covers E2–~E7, ONNX, MIT. `pitch-core-onnx` crate already wraps it |
| **aubio** | **GPL-3.0 — disqualified** for in-app use | — | Onset + pitch references only; do not link |
| **Essentia** | **AGPLv3** (commercial license exists) | — | Broad MIR toolbox; same license problem. Prototype-only |

#### Beat/downbeat/meter (the grid layer)

| System | License | Runtime | Notes |
|---|---|---|---|
| **beat_this** (CPJKU, ISMIR 2024) | MIT | PyTorch, small transformer (few-M params), no DBN post-processing | Beats **and** downbeats in one pass, ~0.88–0.9+ F1 on standard sets, handles tempo/meter *changes* (no Markov constraint — key advantage over madmom). Community ONNX export exists (repo issue #12, used in a JUCE port); P0 assumes exporting it is a bounded spike |
| **madmom** | BSD code but **model files CC-BY-NC-SA — non-commercial only, disqualified** | — | DBN beat/downbeat trackers were the prior SOTA; unusable weights. Keep as a research reference only |
| **all-in-one** (Kim & Nam, WASPAA 2023) | Code depends on madmom (NC models transitively) | — | Beats+downbeats+segments+tempo jointly; nice concept, license chain is dirty. The *segmentation* idea gets re-implemented, not imported |
| **BeatNet** (mjhydri) | Unclear license + madmom feature dependency | — | CRF/particle-filter online tracker; designed for streaming (we don't need online) |
| **librosa** | ISC | Python | Prototyping only (beat_track, chroma); nothing ships |
| **`lyra-viz` `BeatDetect`** (`crates/lyra-viz/src/beat.rs`) | ours | RT-callback safe | **Not a beat tracker.** It is an energy/flux transient *pulse* for visuals: `beat` is a decaying envelope, no tempo, no phase, no bar model (see `docs/VIZ-CONTRACT.md`). It must never feed the chart. The grid comes from offline beat_this |

#### Chord recognition

| System | License | Runtime | Notes |
|---|---|---|---|
| **NNLS-Chroma + chordino HMM** (Mauch & Dixon ISMIR 2010; Sonic Visualiser plugin) | **GPLv2 — algorithm only, reimplement** | trivial CPU | The architecture to copy, not the code: log-freq chroma → bass chroma → HMM/Viterbi over ~25 (maj/min) to ~170 (extended) chord states. ~200 lines of Rust on rustfft for chroma + small Viterbi. Deterministic, zero weights, zero license risk |
| **ACE / consonance conformer** (ISMIR 2025-family), ChordFormer, BTC (jayg996/BTC-ISMIR19), ChordNet/ChordMini | research code, mixed licenses — verify per repo | PyTorch; ONNX export = small spike | Decomposed heads (root/bass/quality/notes) give natural **posterior margins** for confidence and an explicit "N.C."/unknown state. P1 upgrade path |
| **Chord AI** (commercial app) | proprietary | on-device | Existence proof: real-time chords *and voicings/positions* on a phone. The quality bar users will compare against |

#### Source separation (front end)

| System | License | Runtime | Notes |
|---|---|---|---|
| **Demucs** (Meta) | MIT code | htdemucs: heavy; hybrid transformer | Superseded for our purpose by RoFormer-class models; the conceptual ancestor |
| **BS-RoFormer / MelBand-RoFormer stems** | model licenses vary per checkpoint — **the elicwhite ONNX exports on HF ship a 6-stem BS-RoFormer-SW** (drums/bass/vocals/guitar/piano/other), 670 MB fp32 / ~336 MB fp16 | Sliding-window STFT masker; **host does STFT/iSTFT** (rustfft), model is pure ONNX → `ort`+CoreML. ~0.3–1× realtime on M-series CPU → a 4-min song ≈ 4–13 min. GPU/ANE cuts it substantially | This is the heavyweight stage — hence *optional, background-only*. Verify checkpoint license before bundling/download-gating |

**Stem-then-transcribe vs. direct polyphonic AMT — the honest answer:** for commercial
mixes, isolating a guitar stem first is the difference between "degraded" and "useless"
(direct mixture AMT tops out ~0.6 note-F1 *on the best system*, and that's
instrument-agnostic). But separation artifacts are real: bleed, ghost harmonics, smeared
attacks, dropped palm-mutes — the stem is itself a probabilistic artifact. all-in-one
demonstrated demixed input *improving* beat tracking; for notes the published
head-to-head evidence is thin. **Design decision: run AMT on both the mix and the stem
and fuse per-note** — agreement between the two runs is a cheap, model-free confidence
signal, and the map keeps provenance per note (`source: mix | stem`). Never trust the
stem alone.

### 2.3 Calibration: what the numbers mean for a 4-minute rock song

- Beat/downbeat grid: F1 ~0.85–0.95 on mainstream rock/pop; degrades on rubato,
  free-time, and live recordings with crowd noise — but beat_this tracks *tempo drift*
  well (it's not a fixed-BPM tracker).
- Chord track (maj/min vocabulary): ~0.75–0.85 correct-segment overlap on pop corpora;
  extended-vocab (7/9/sus/add) drops to ~0.5–0.65; metal/drone/ambient is a failure class.
- Note map on a clean-ish guitar stem: note-onset F1 ~0.6–0.75 realistic; with-offset
  and velocity worse. On dense/distorted mixes: half that, plus octave and harmonic
  ghosts.
- Tablature string/fret correctness: ~0.8–0.9 tab-F is the published ceiling *for clean
  solo standard-tuned audio* (TabCNN-class on GuitarSet). On our actual input (stem of a
  commercial mix), no honest published number exists; treat ~0.5–0.7 as planning reality
  and design the UI so a wrong fingering is a *different* answer, not an *error* (§3.6).

---

## 3. Guitar specifically: pitch → tablature is a second, harder problem

AMT emits `{onset, offset, pitch}` — a piano-roll. Tablature additionally requires
`(string, fret)` for every note, and that map is **not determined by pitch**:

- Standard tuning: a given pitch is playable at up to ~5 positions (e.g. A4 = open A
  string, 5th-fret E string, 10th-fret B string…). Across the fretboard there are
  ~6 strings × ~20 frets but only ~49 distinct pitches — each pitch has ~2–4 physical
  realizations on average.
- Position choice encodes *style*: the same phrase played open-position vs. at the 12th
  fret is a different part. Timbral differences (string gauge, fret position) are real
  but weak features — published string-classification-from-audio work
  (inharmonicity-based, e.g. Bastas & Koutoupis ICASSP 2022) helps only marginally on
  DI, less on produced mixes.

### 3.1 The solver approach (what we build at P1)

The classical formulation — and still the right default — is **constrained decoding**:
a dynamic-programming/Viterbi search over positions minimizing a playability cost:

- **Hori & Sagayama (IOHMM, 2013)** and **Yazawa** model position as a hidden state with
  fingering-conditioned transition costs.
- **Radisavljevic & Driessen (ICMC 2004)** — "path difference learning for guitar
  fingering": cost learned/parameterized from hand geometry.
- **Parncutt-family ergonomic costs**: hand-position center, span (≈4–5 frets max for
  most hands), movement distance, string-crossing penalty, open-string preference where
  idiomatic, barre-chord penalty, prefer-one-position-per-phrase.

Cost terms, concretely, for a candidate path assigning positions to the note sequence:

```
cost = Σ position_shift(|hand_pos_t − hand_pos_{t−1}|)
     + Σ stretch(span of simultaneous notes > 4–5 frets → +∞)
     + Σ open_string_penalty (genre-weighted)
     + Σ string_skip / awkward_motion
     + Σ string_change_rate
     − Σ phrase_continuity (stay in position)
```

This is **deterministic, testable, and license-free** — a few hundred lines of Rust. Its
honest output is *"a physically plausible way to play these notes,"* not *"the way the
record plays them."* Those are different claims; the product must not blur them.

### 3.2 The learned approaches (P2 research lane)

- **MIDI→TAB masked-LM** (Edwards et al., ISMIR 2024, arXiv:2407.05825): pretrained on
  **DadaGP** (26,181 GuitarPro songs, research-access), fine-tuned on pro
  transcriptions; beat Guitar Pro's own auto-fingering in a playability user study.
- **Fretting-Transformer** (arXiv:2506.14223, 2025): T5 encoder-decoder, MIDI→tab with
  tuning/capo conditioning, beats A*-search and Guitar Pro's fingering on DadaGP+GuitarToday.
- **"From MIDI to rich tablatures"** (arXiv:2407.09052): optimization over fingering +
  techniques using mySongBook statistics — also annotates *techniques* (hammer/pull,
  slide), which is the right richer target than bare (string,fret).
- **HeavyCNN** (Brno thesis, 2025): multi-task CNN — pitch+onset+string — with **tuning
  as model input**, trained on 61h of VST-rendered GuitarPro in dropped tunings; the
  honest takeaway is the *dataset recipe* (synthesize from GP files in arbitrary tunings)
  more than the architecture.

Datasets that anchor all of this: **GuitarSet** (3h, 360 solo excerpts — tiny),
**Guitar-TECHS** (ICASSP 2025, 5h+ electric, hexaphonic-pickup ground truth incl.
palm-mute/bend/harmonic labels — the best new per-string supervision), **DadaGP**
(symbolic only), and the synthetic-render trick above.

### 3.3 What Rocksmith and GuitarPro actually do — the bar and the shortcut

- **Rocksmith charts are hand-authored.** The community toolchain (Editor on Fire →
  DLC Builder → RocksmithToolkitLib SNG/PSARC) exists precisely because charts require a
  human charter. The SNG format carries per-string **tuning**, **capo**, phrases,
  sections, and **NLD — "linked difficulty"**: lower levels are *the same chart with
  notes removed*, which is exactly our `layer_mask` design (§4). Rocksmith never
  transcribes anything; it ships an editor's answer key.
- **GuitarPro** (the editor) auto-assigns fingerings on MIDI import via a built-in
  heuristic — the thing Fretting-Transformer benchmarks against and beats. Its value to
  us is different: **user-imported GP/MusicXML files are near-ground-truth maps.** A
  `.gp5` drop should *replace* the generated note+tab layers while keeping our
  audio-aligned grid (import path = P2; PyGuitarPro is LGPL — read-only sidecar or a
  Rust re-parse; alphaTab's MPL-2.0 importer is readable reference code).

### 3.4 Tuning and capo — latent variables, estimated jointly

The solver can't run until it knows the fretboard's pitch mapping:

- **Global tuning offset**: estimate cents-deviation of strong stable f0s vs. concert
  pitch (many records are ±20¢ or a half-step down). Cheap, reliable.
- **Non-standard tuning**: published precedent is thin — Khatri & Dillingham (U.
  Rochester, Duan group) identify tuning *from a MIDI transcription* via an LSTM
  classifier + a DP that picks the tuning minimizing note-location cost; an ACM 2024
  paper ("Acoustic Classification of Guitar Tunings with Deep Learning") does it
  directly from audio and notes the field is "underdeveloped." Practical approach:
  given the note multiset, **score a candidate dictionary** {standard, half-step-down,
  drop-D, DADGAD, open-G, open-D, …} by (a) how often detected pitch-classes equal
  open-string classes, (b) minimum required fret span, (c) open-position chord-shape
  coverage. Open-string-heavy music detects well; pure-position playing is genuinely
  underdetermined — say so in the map (`tuning.conf`).
- **Capo**: after tuning, if the inferred minimum comfortable position sits at fret
  k>0 with open-position pitch-classes transposed by k, emit `capo: k` and re-anchor.
  Capoed songs otherwise produce absurd high-fret "open chord" fingerings — a classic
  failure this single inference prevents.
- **Hard limit**: tuning+capo are *only* inferable from pitch content; two competing
  hypotheses can fit identically. The map carries `tuning: {strings, conf,
  alternatives[]}` and the UI offers the top alternates ("detected drop-D · also
  possible: standard").

### 3.5 Techniques — best-effort flags, never silently

basic-pitch's **contour head** gives per-frame continuous pitch → we can mark:
`bend` (contour excursion ≥ ~70¢), `slide` (sustained glide into a new pitch class),
`vibrato` (periodic ±30–80¢ oscillation). `palm_mute` ≈ short sustain + low spectral
centroid at onset. `harmonic` ≈ pitch an octave+ above any physically frettable
position of the detected string. `mute/chug` ≈ onset with no resolvable pitch — which
belongs in the chart as a *rhythm event*, not dropped (dino-shred's `ChugClassifier`
already treats these as first-class, `audio/detect.py`).

### 3.6 Honest verdict

The tab layer emits **a playable hypothesis with a solver score**, displayed as
"fingering suggestion," and the difficulty system prefers positions that survived the
solver's margin — never presented as "this is how the record plays it." When a verified
source exists (imported GP file, future human-curated map), it wins outright.

---

## 4. Lyra pipeline architecture

### 4.1 The stage graph

```
FLAC/any ByteSource
 │
 ▼  TrackDecoder (crates/lyra-formats/src/lib.rs:159 — symphonia over ByteSource;
 │    the same decoder the RT path uses, driven as fast as next_block() returns;
 │    remote/torrent sources should flow through CachingSource so fetched bytes
 │    serve playback AND analysis once)
 │    → mono f32 buses: 22050 Hz (basic-pitch input), 44100 stereo (stems, chroma)
 │
 ├─► [gate]  "is there guitar?"   ← reuse BLUEPRINT §data-ML EfficientAT-style tagger
 │            (mn10 instrument tags) — no guitar → status=unsupported, stop
 │
 ├─► [P1]  stem separation — BS-RoFormer-SW-6stem ONNX (host STFT/iSTFT via rustfft)
 │         → guitar stem bus @ 44100
 │
 ├─► GRID    beat_this ONNX → beats[] + downbeats[] + per-beat activation conf
 │           → meter = beats-per-bar from downbeat spacing histogram
 │           → tempo per-section from inter-beat intervals (NOT a constant —
 │             the map is a beats[] table, so rubato/accelerando are representable)
 │
 ├─► SECTIONS chroma self-similarity novelty + repetition grouping (Foote-style
 │            checkerboard on the HPCP we already compute for chords) → labeled
 │            segments snapped to nearest bar
 │
 ├─► CHORDS  HPCP/bass chroma → HMM Viterbi (maj/min+N.C. P0; extended vocab P1 via
 │           conformer ONNX) → segments {t0,t1,root,quality,bass,conf}
 │           + strum onsets aligned to grid → strum chart events
 │
 ├─► NOTES   basic-pitch ONNX → frame/onset/contour activations @~86fps
 │           → note assembly (port of inference.py post-processing: onset×frame
 │             hysteresis, min-note-len, energy gate, bend extraction — ~300 lines
 │             of Rust; NeuralNote proves this ports)
 │           → run on MIX and on STEM, fuse per-note (agreement → conf boost)
 │
 ├─► TUNING  pitch multiset → offset cents → candidate-tuning dictionary score
 │           → capo inference (§3.4)
 │
 ├─► QUANTIZE snap onsets to grid positions (keep raw_time AND grid_pos);
 │           subdivision vote (2-vs-3), swing ratio, unquantizable → rubato flag
 │
 ├─► TAB     DP/Viterbi position solver conditioned on tuning+capo → (string,fret)
 │           + technique flags + solver margin per note
 │
 └─► LAYERS  per-note layer_mask (§4.4) + per-section quality rollup
 │
 ▼
.lyramap  →  maps/<audio_hash>.lyramap  +  registry row in lyra-store
```

Ordering matters for correctness: **tuning/capo before the solver, grid before
quantization, chords usable before notes exist** (partial-map delivery, §4.5).

### 4.2 Where each stage runs — all Rust, no exceptions in the shipping path

| Stage | Home | Why |
|---|---|---|
| Decode, resample, STFT/HPCP, Viterbi, DP solver, quantization | new **`lyra-map`** crate (with reusable DSP going into `lyra-dsp`) | Pure Rust, zero deps issues; `rustfft` already a workspace dep (`Cargo.toml:78`) |
| ONNX inference (beat_this, basic-pitch, RoFormer, chord conformer, tagger) | `lyra-map` via **`ort`** (MIT/Apache; `coreml` EP feature exists) | BLUEPRINT already commits to `ort`+ONNX+CoreML-EP; one runtime for every model |
| Model weights | **on-demand download** → `~/Library/Application Support/Lyra/models/`, SHA256-pinned | Don't bundle ~1 GB in the DMG; network.client entitlement already exists |
| Orchestration queue | `lyra-map` on background threads; never the RT path (`lyra-dsp` doc comment: *"runs on the CoreAudio render thread — allocation-free"*) | Analysis is CPU-heavy batch work; the audio callback stays untouched |

**Why not Python in-product:** the pedagogy doc's two-path design already confines Python
to the dino-shred dev lane. For Lyra, Python sidecars mean shipping a runtime, fighting
notarization, and inheriting dependency licenses (aubio/chordino GPL, madmom models
NC — all the good research tools are poison). The one legitimate sidecar is a
**`lyra-mapgen` CLI** — same Rust crates, headless binary — so maps can be batch-generated
on a beefy host (the workspace already SSHes to a JioPC VM; `lyra-fs`'s remote
`ByteSource` means mapgen can even read the remote library directly) and dropped into
`maps/` — which also serves dino-shred's planned `tools/levelgen` (spec §2: mp3 →
level.json via beat_this — same artifact, superset schema).

**Remote service tradeoff**: Klangio's Guitar2Tabs API proves commercial audio→tab as a
service exists. It violates Lyra's offline-first posture and adds per-song cost;
listed as an optional P3 premium lane only — the default is on-device.

### 4.3 Real-time vs offline — the split is absolute

Everything above is **offline precompute**, triggered on demand ("Learn this song") or
opportunistically in a background queue (precedent: `lyra-viz/src/waveform.rs` computes
seekbar peaks at import time and stores them in the DB — same shape). Per-song wall
clock on Apple silicon, 4-min song, honest ranges:

- P0 set (decode + grid + chords + sections): **~30–90 s** on CPU.
- + stems (RoFormer): **+3–8 min** CPU (CoreML/ANE substantially less — needs benching).
- + notes + tab solve: **+30–90 s**.

So: strum-along mode is ready inside a minute of asking; the full note map lands a few
minutes later, **incrementally** — the map file is written layer-by-layer and the UI
upgrades the session live (grid-only → +chords → +notes) rather than blocking.

The *other* real-time leg — the user's guitar input detector — is explicitly out of
scope here (pedagogy doc owns it): YIN/MPM (`pitch-estimate`, `pitch-detection` crates)
or SwiftF0-ONNX via `pitch-core-onnx`, whichever the latency/latency-vs-accuracy
tradeoff settles. Its only dependency on this document is the `NoteEvent` schema it
gets judged against.

### 4.4 Difficulty layers — Rocksmith NLD, honestly

Per-note `layer_mask` assigns membership at map-gen time (not per-session):

- **L0** — downbeat anchors: bass/root skeleton notes on beats 1 (and 3).
- **L1** — + chord stabs at chord-change boundaries (strum events).
- **L2** — + full chord voicings + eighth-note skeleton.
- **L3** — + melody/riff notes above conf τ.
- **L4** — full density (confident notes only; ghosts stay ghosts at every level).

This composes with the pedagogy doc's *representation* ladder (highway+tab → map-off):
density and representation are orthogonal axes, both driven off the same artifact.

### 4.5 Cache format — `.lyramap` + a thin store registry

**Decision: a single-file, content-addressed artifact.** Rationale:

- A map is a few hundred KB, always loaded whole into memory for a session — a
  relational store buys nothing at read time (unlike `tracks`/`peaks`, it's never
  queried row-wise).
- Keyed by **audio content hash** (BLAKE3 of file bytes — we decode the file anyway;
  same recording = same map across local/torrent/remote copies), so torrents of
  identical files share maps.
- Self-contained + portable → the `lyra-mapgen` CLI, a teammate's batch run, or a
  future "community map" import are all the same object.
- Versioned → `pipeline_version` bump forces regeneration, not migration.

```c
// .lyramap file
struct LyraMapHeader {           // 16 B
    char  magic[4];              // "LYRM"
    u16   format_version;        // = 1
    u16   flags;                 // bit0: payload is zstd
    u64   payload_len;
}                                  // followed by zstd(serde_json(SongMap))
```

`lyra-store` gains a registry migration (the `waveform_peaks` table is the precedent
for "offline analysis result keyed to a track"):

```sql
CREATE TABLE track_maps (
  audio_hash    TEXT PRIMARY KEY,      -- BLAKE3 of source bytes
  map_path      TEXT NOT NULL,         -- maps/<audio_hash>.lyramap
  pipeline_ver  TEXT NOT NULL,         -- e.g. "mapgen-0.3 + beat_this-1.0 + bp-icassp22"
  status        TEXT NOT NULL,         -- pending|grid|chords|notes|done|unsupported|failed
  overall_conf  REAL,
  updated_at    INTEGER NOT NULL
);
CREATE INDEX idx_track_maps_status ON track_maps(status);
```

Tracks resolve to maps via `audio_hash`; a `tracks.audio_hash` column (or on-demand
hash) joins them. `status` implements the tiered model — the UI can offer strum mode
as soon as `status >= 'chords'`.

### 4.6 Plug points, concretely

| Consumer | Interface |
|---|---|
| `lyra-formats` | Reuse `TrackDecoder` + `ByteSource` unchanged. Analysis wants "decode whole file to mono f32 fast" — add an `OfflineDecode` helper in `lyra-map`, not in formats |
| `lyra-engine` | **Unchanged.** Map consumption keys off `position_secs()`; the decode/tap path (`engine.rs:387-394`) is never touched. The viz tap stays a display channel — `docs/VIZ-CONTRACT.md` is explicit that `LyraVizFrame` is not a judgment clock, and `beat.rs` stays visual-only |
| `lyra-store` | `track_maps` registry table (above); pedagogy-doc `coach_*` tables are a sibling migration |
| `lyra-ffi` | Hand C ABI like the rest (`lyra-ffi/src/lib.rs`): `lyra_map_status(path)`, `lyra_map_generate(path, tier)` → job id, `lyra_map_load(path)` → opaque handle + `lyra_map_json(handle)` for the Swift bridge (JSON like `lyra_lib_tracks` in `Bridge.swift`) |
| Swift app | New `VizMode` case renders highway+tab; map quality badge; "why is this dimmed" explainer; correction UI is P2 |
| `lyra-torrent` | Unchanged; content-hash map keys mean a torrented FLAC's map is generated once and shared |
| Search | `crates/lyra-search` doesn't exist; the provider pattern in `torrent-search-design.md` is the precedent if community-verified maps ever become a fetched resource |
| dino-shred | `Conductor` gets constructed from `grid.beats[]` (its spec already plans this — `searchsorted` nearest-beat, ADR-0004); `Judge` windows unchanged; chart JSON = a projection of `.lyramap` |

---

## 5. Uncertainty representation — the product feature, not a footnote

AMT output is a posterior, not an answer. The map that survives contact with users is
one whose wrongness is *visible and non-punitive*.

### 5.1 Per-layer confidence model

Every event carries `conf ∈ [0,1]` + `provenance`; confidences are *composed*, not
averaged blindly:

- **Beat**: `conf = activation peak` at the placed beat (beat_this emits logits).
- **Chord**: `conf = posterior margin` (top1 − top2 of the chord-state posterior) —
  a chord the model is torn between looks exactly like a low margin.
- **Note**: `conf = onset_strength × frame_strength × source_agreement` where
  `source_agreement = 1.0` if mix-run and stem-run independently placed the same note
  within ±30 ms & same pitch class, ~0.6 if only one did, ~0.4 if they disagree on
  octave. Two weak models *agreeing* is real evidence — use it.
- **Tab position**: `sf_conf` = solver margin (cost gap between chosen and
  runner-up assignment) × note conf — a confident note can still have an arbitrary
  fingering.
- **Section/global**: `section_conf` = median note/chord conf in section; the
  `overall_conf` in `track_maps` is the p25 of section confs (worst-section-weighted,
  not mean — one bad solo shouldn't hide behind 3 clean verses).

### 5.2 Judge policy — three tiers, no silent wrongness

The map's per-note `conf` maps to judgment policy (consumed by dino-shred's `Judge`,
windows ±30/±60/±100 ms at `judge.py:19`):

| Tier | conf | Rendered | Judged |
|---|---|---|---|
| **graded** | ≥ τ_hi (~0.7) | full opacity | normal windows |
| **advisory** | τ_lo..τ_hi | dimmed w/ marker | scored, **never penalized** — "Lyra thinks X; you played Y" is shown as *feedback*, not failure |
| **ghost** | < τ_lo (~0.4) | faint outline | ungraded — exists for context/flow |

Plus structural forgiveness, orthogonal to confidence:

- **Octave tolerance**: input-side monophonic detectors octave-error constantly under
  distortion; a hit an exact octave off the target pitch class earns *partial credit*
  by default.
- **Timing bias self-check**: if the user's *median* signed error against a given note
  across plays is e.g. +18±5 ms, suspect the map's onset placement, not the player —
  flag the note `suspect_timing` (below).
- **Rubato sections**: unquantizable passages (`rubato` flag from the quantizer) widen
  windows or suspend grading — the grid itself is uncertain there.

### 5.3 Trust-the-player refinement

A map is a hypothesis; aggregate player performance is new evidence. When N attempts
consistently deviate from a note (same direction, tight spread), bump a per-note
`suspect` counter → the note gets marked for review and eventually re-solved/demoted.
Conversely, consistently-nailed ghost notes get promoted. This makes the map *better
with use* and is cheap: the pedagogy schema already logs `pitch_target`/`pitch_detected`/
`map_conf` per event — the aggregation is a `lyra-store` rollup, not new machinery.

### 5.4 UI language

- One honest badge per song: `Map: 82% · generated` (or `verified` for imported GP).
  Tapping it opens the section-level breakdown — "the solo (2:34–3:10) is low-confidence."
- Highway encodes conf as opacity (graded) → outline (ghost) — the same channel the
  player already reads; a one-time "dimmed = Lyra is unsure, you're not graded on
  these" coachmark.
- Failure is communicated, not hidden (§7): "This mix is too dense for a reliable note
  map — chord mode works great though" is a *feature statement*, not an apology.

---

## 6. Chord transcription — the 80/20 that ships first

Chords + grid is strum-along mode: for a huge fraction of what people want
(campfire-strum songs, rhythm practice, the pedagogy doc's chord-stability ladder),
it *is* the product.

**Implementation, P0 — zero models, zero licenses:**

1. HPCP/log-frequency chroma: 36-bin (3 bins/semitone) CQT-style magnitude chroma —
   ~150 lines on rustfft over the 44100 mono bus; plus a bass-chroma band for root-vs-
   inversion.
2. HMM/Viterbi over chord states: {12 roots × {maj,min}} + N.C. at P0 (extend to
   +7/sus/dim vocab P1 — same decoder, more states). Emission = template cosine
   similarity; transition = key-aware chord-transition prior; the Chordino/NNLS
   *architecture* (Mauch & Dixon ISMIR 2010) reimplemented — the plugin code itself is
   GPLv2, so we write our own, which is ~an afternoon of DP anyway.
3. Expected: ~0.75–0.85 segment accuracy maj/min on pop/rock; emits `conf` = posterior
   margin; ambiguous → emits `quality: "maj/min?"` alternate rather than forcing.
4. **Strum events**: spectral-flux onsets (the same flux family as `lyra-viz`'s
   transient logic, but offline/high-resolution) clustered to chord segments → "strum
   on beat 2-and" chart events.

**P1 upgrade**: an exported conformer (ACE/consonance-family, ISMIR 2025; or BTC-class)
ONNX — decomposed root/bass/quality heads give extended vocabulary + real posteriors +
explicit N.C. ~Few-MB model, same slot in the pipeline.

**Sequencing rationale**: chords ship first because (a) accuracy is already
product-grade for maj/min vocab, (b) the dependency graph is shallow (needs only
grid+chroma), (c) it exercises the entire artifact/judge/UI machinery with a forgiving
ground truth, (d) chord segments *assist* the note layer (harmonic prior for the
solver's voicing inference and for note-map sanity checks).

---

## 7. Failure-mode honesty — songs that defeat this, and what the product says

| Input class | Why it fails | Detection | Product response |
|---|---|---|---|
| Dense/wall-of-sound mixes | masking → mixture AMT ~0.3–0.5 note-F1 | stem energy ratio; note-conf rollup | chord mode only; badge "map limited" |
| Heavy distortion/saturation | added harmonics masquerade as notes; octave ghosts | harmonic-vs-f0 disagreement, low agreement mix↔stem | demote note layer to advisory; keep chug/rhythm events (a muted-chug chart is still playable) |
| Fingerstyle/arpeggio | actually *easier* note-wise, but positions ambiguous | — | full notes; tab shown as suggestion |
| Capos | open-position pitch classes transposed | capo inference §3.4 | `capo: k` in map; alternates offered |
| Odd tunings | fretboard mapping wrong → garbage fingering | tuning dictionary score; `tuning.conf` low | top-2 tuning alternates in UI; notes still valid (pitches don't depend on tuning) |
| Multiple layered/double-tracked guitars | two near-identical parts blur; stereo doubles | mid/side analysis; per-pan stems P2 | transcribe the dominant part; label "lead layer"; honesty badge |
| Octave/pitch effects, whammy | real pitch ≠ fretted pitch | contour discontinuities | techniques flag `fx`; advisory tier |
| No clear guitar stem | gate fails | EfficientAT-style tagger + stem energy | `status=unsupported`: "no guitar found — this song can't be charted" |
| Percussive/muted-heavy playing | unpitched onsets aren't notes | onset-without-pitch events | chug events (rhythm chart) — dino-shred already has this vocabulary |
| Fast dense passages (shred, sweep) | onset density > model resolution | local onset-rate vs conf | section marked low-conf; layer-fade: L4 only |
| Omitted/added-tone chords | vocab mismatch | posterior margin low | emit maj/min + "?" alternate; never force an exotic label |
| Live recordings: crowd, bleed, tempo drift | SNR + non-studio performance | grid conf, stem bleed | grid usually survives (beat_this handles drift); notes often don't — degrade honestly |
| Rubato/free-time (no stable grid) | `beat_time(n)` undefined | downbeat spacing unstable, beat conf floor | `status=grid_failed`: honest "cannot chart this" |

The contract: **status flags are per-layer, and the UI says which layer failed and why
in one sentence.** Silence is the bug — a confidently wrong map teaches wrong notes.

---

## 8. Recommendation — P0 / P1 / P2

### P0 — "Strum companion" (grid + chords + sections + artifact + judge wiring)

- `lyra-map` crate: offline decode (TrackDecoder), resample→mono, HPCP chroma,
  chord HMM/Viterbi, chroma-novelty segmentation, `.lyramap` writer + `track_maps`
  migration, `lyra_map_*` FFI, analysis job queue.
- beat_this → ONNX export spike (community export exists), `ort`+CoreML.
- Highway renders **grid + chord events**; judge consumes `beats[]` + strum events.
- Expected: beats/downbeats ~0.85–0.95 F1 mainstream; chords ~0.75–0.85 maj/min;
  ~30–90 s/song CPU. Everything permissively licensed (MIT/Apache + our code).
- Effort: the bounded kind — one new crate, two ONNX models, one Viterbi.

### P1 — "Note map" (stems + notes + tuning + solver + layers + uncertainty policy)

- RoFormer 6-stem ONNX stage (verify checkpoint license; download-gated weights);
  basic-pitch ONNX + Rust note-assembly port; mix/stem fusion confidences;
  tuning+capo estimation; DP position solver; technique flags; layer_mask;
  advisory/ghost judging + trust-the-player rollup; section-confidence badge UI.
- Expected: note-onset F1 ~0.6–0.75 on clean-ish stems, lower on dense mixes —
  *surfaced* via the quality system, not hidden; tab positions = playable hypotheses.
- Also ships: `lyra-mapgen` headless CLI (dino-shred `tools/levelgen` superset).

### P2 — "Verified & learned" (imports, learned fretting, corrections, minus-guitar)

- GP/MusicXML import → `provenance: imported` maps (PyGuitarPro is LGPL — Rust
  re-parse or read-only sidecar; alphaTab MPL-2.0 as reference).
- Fretting-Transformer-class learned solver (needs DadaGP-style data access or
  synthetic GP→audio rendering à la HeavyCNN); Guitar-TECHS for technique labels.
- In-app correction UI ("this note is 5th fret not 7th") → `provenance: human_edited`,
  highest tier, syncs through the same artifact.
- Minus-guitar stems for karaoke-over-the-record playthrough.
- Optional remote transcription service as a paid tier (privacy-flagged).

### Data-format sketch (the `SongMap` payload)

```rust
struct SongMap {
    map_id: String,               // blake3 of payload bytes
    audio_hash: String,           // blake3 of source audio — join key to tracks
    pipeline_ver: String,         // "mapgen-0.3|beat_this-1.0|bp-icassp22|rof-sw6"
    duration_s: f32,
    tuning: Tuning { strings: [i8;6], global_cents: i16, capo: u8,
                     conf: f32, alternates: Vec<Tuning> },
    grid: BeatGrid,
    sections: Vec<Section>,       // {t0,t1,label,conf}
    chords: Vec<ChordEvent>,
    notes: Vec<NoteEvent>,
    quality: Quality { overall: f32, per_section: Vec<f32>,
                       layer_status: LayerStatus },   // grid|chords|notes|tab ok/degraded/failed
}
struct BeatGrid {
    beats: Vec<BeatPt>,           // BeatPt{t_s, conf}
    bars: Vec<u32>,               // indices into beats = downbeats
    meter: Vec<(u32 /*bar*/, u8 /*beats_per_bar*/)>,  // supports changes
    swing: Option<f32>,           // 0.5=straight .. ~0.67=triplet feel
}
struct ChordEvent { t0: f32, t1: f32, root: u8, quality: ChordQuality,
                    bass: Option<u8>, conf: f32, alt: Option<ChordQuality> }
struct NoteEvent {
    onset_s: f32, offset_s: f32,          // audio time — the judge's clock
    bar: u32, beat: u8, tick: u16,        // grid position — the renderer's clock
    midi: f32,                            // continuous (bend-aware)
    bend_cents: Option<i16>,
    techniques: TechFlags,                // palm_mute|slide|bend|vibrato|harmonic|chug|fx
    conf: f32,                            // composed: onset×frame×source-agreement
    source: Provenance,                   // Mix|Stem|Imported|HumanEdited
    pos: Option<(u8 /*string*/, u8 /*fret*/)>, sf_conf: Option<f32>,
    layer_mask: u8,                       // L0..L4 membership (§4.4)
    suspect: bool,                        // trust-the-player flag (§5.3)
}
```

Chart-position derives from `(bar,beat,tick)` so the highway scrolls at constant
musical speed even through tempo drift; judgment uses `onset_s`. Both clocks in one
event — that is the whole schema trick.

### Evaluation gate (how we know any of this works)

Build a small ground-truth harness alongside `lyra-mapgen`: GuitarSet excerpts (public,
aligned), a handful of GP-verified recordings, and VST-rendered synthetic guitar in
varied tunings (the HeavyCNN recipe). Score each pipeline version with mir_eval-style
note-onset-F1 / chord-segment accuracy / beat-F1; `pipeline_ver` in every map ties a
quality claim to a reproducible build. Ship nothing whose numbers we can't print.

---

## 9. What this document does NOT claim

- Auto-generated tab is **not** publication-quality and never presented as the
  recording's actual fingering — it's a confidence-carrying playable hypothesis.
- Nothing real-time touches the audio callback; `lyra-viz`'s `BeatDetect`/`VizFrame`
  stay strictly a display channel per `docs/VIZ-CONTRACT.md`.
- No GPL/AGPL/NC-licensed artifact ships inside Lyra (aubio, chordino code, Essentia,
  madmom models, pYIN Vamp). Algorithms get reimplemented; models are MIT/Apache only.
- The user's-guitar input path (mic/DI pitch detection, latency calibration) is owned
  by `dino-shred-pedagogy.md`; this document only defines the `NoteEvent` schema it's
  judged against.
- `crates/lyra-search` does not exist — nothing here depends on it.

## Appendix — license ledger

Permissive (ship): basic-pitch Apache-2.0 · MT3 Apache-2.0 · YourMT3+ Apache-2.0 ·
CREPE MIT · SwiftF0 MIT · libf0 MIT · beat_this MIT · Demucs MIT · ort MIT/Apache ·
rustfft MIT/Apache · pitch-detection/pitch-estimate crates · DadaGP (research) ·
Guitar-TECHS (research) · alphaTab MPL-2.0 (weak copyleft, reference/importer OK).
Conditional (review before use): PESTO LGPL-3.0 · PyGuitarPro LGPL-3.0 · RoFormer
checkpoint licenses vary · madmom *code* BSD but *models* CC-BY-NC-SA (NC — excluded) ·
MT3 weights (verify).
Excluded (license): aubio GPL-3.0 · chordino/NNLS GPL-2.0 (reimplement algorithm) ·
Essentia AGPLv3 · pYIN Vamp plugin GPL.
Commercial reference (no code): Chord AI · Klangio Guitar2Tabs · Rocksmith (hand-authored
charts, NLD difficulty design precedent) · GuitarPro auto-fingering (baseline the
learned solvers beat).
