# Dino-Shred × Lyra — Building a Real Learning Instrument, Not a Game With a Guitar

Date: 2026-09-16. Status: research + architecture recommendation.

Grounded in two real codebases, both read in full for this document:

- **dino-shred** @ `feat/guitar-dino-v1` (PR #1) — `dino_shred/audio/{engine,detect,clicks}.py`,
  `dino_shred/rhythm/{conductor,judge,calibrate}.py`, `dino_shred/game/{game,spawner,objects}.py`,
  `dino_shred/config.py`, ADRs `docs/decisions/0001–0004`, `docs/learning/01-the-audio-clock.md`,
  `docs/research/2026-07-03-audio-stack-research.md`.
- **Lyra** @ `~/lyra-src` uncommitted working tree — `docs/VIZ-CONTRACT.md`,
  `crates/lyra-viz/src/{frame,beat,spectrum,level,osc,lib}.rs`,
  `crates/lyra-engine/src/lib.rs`, `crates/lyra-formats/src/lib.rs`,
  `crates/lyra-ffi/src/lib.rs`, `crates/lyra-hal/src/lib.rs`,
  `app/Sources/LyraApp/{Bridge.swift, Viz/*.swift}`, `BLUEPRINT.md`, `entitlements.plist`,
  `docs/research/dino-shred-integration.md`.

Terminology used throughout: **REUSE** = the capability already exists in one of the two
codebases and can be adopted as-is or with a thin port. **NEW** = a capability that does not
exist anywhere in either codebase and must be built. **ABSENT-BY-DESIGN** = something that
looks like it exists but does not do what a naive reading suggests — these are the traps.

---

## 1. Executive summary

The question was: how do we make this a genuinely good *learning* experience — not just a
game with a guitar — and what do 2026-state tools and practices say?

The honest answer from the literature is that the hard part is not the game, the audio
pipeline, or the transcription. It is **transfer**: most guitar-learning games produce
players who are good at the game and bad at guitar outside it. Rocksmith is the canonical
example — millions of engaged users, a documented pattern of "teaches songs, not guitar,"
and a note-highway representation that trains screen-reading rather than fretboard
knowledge. The way out is not a better highway; it is a *practice curriculum* that happens
to have a game inside it.

Five load-bearing conclusions:

1. **dino-shred's V1 (onset/rhythm only) is pedagogically defensible — but only as a
   deliberately narrow foundation.** Timing is the one musical skill where the measurement
   is unambiguous (a millisecond error against a grid), the feedback is immediate, and the
   existing `Conductor`/`Judge`/`Calibrate` stack is genuinely correct. The danger is that
   rhythm-only gameplay becomes the *definition* of musicianship for the learner. The
   document below specifies the scaffolding that keeps it a foundation: explicit pulse
   and subdivision teaching, metronome withdrawal, and a gate that forces pitch work to
   enter before rhythm fluency plateaus into game fluency.

2. **The timing architecture in dino-shred is the correct model and Lyra does not have
   it.** dino-shred judges onsets by hardware ADC timestamp against a drift-free beat
   grid (`t0 + n·period`), at a measured ~15–25 ms string-to-judgment against a ≤40 ms
   budget. Lyra's `VizFrame` pipeline is a *display* channel: Swift polls a snapshot at
   ~60 Hz (`VizRuntime.pump()` in `app/Sources/LyraApp/Viz/VizSurface.swift`), the tap
   runs in the **decode worker** (`VizTap::push`, `crates/lyra-engine/src/lib.rs:567`)
   up to ~4.3 s ahead of audible position at 44.1 kHz (RING_SAMPLES = 192 000 stereo
   frames ≈ 1 s @192k), and `VizFrame.beat` is a transient *pulse* from `BeatDetect`
   (1.6× EMA, 100 ms holdoff, ~150 ms decay) — not a beat grid, not beat timestamps, not
   a phase estimate. Judgment in Lyra must run on a new timestamped-input path; the viz
   channel is for rendering only. This is the single most important architectural fact
   in this document.

3. **The 2026 transcription stack is good enough to generate practice material but not
   good enough to be a teacher.** `beat_this` (ISMIR 2024) gives reliable beat/downbeat
   grids; Basic Pitch/CREPE-class note transcription and chord recognition remain
   error-prone on guitar specifically (transient attacks, inharmonicity, capo/tuning
   variance, string/fret ambiguity — the same pitch is playable at up to five fretboard
   positions). The design consequence: generated maps carry per-note/per-section
   *confidence*, and confidence determines which practice features are allowed to use
   them. Beats → safe for grading. Chord roots → safe for accompaniment display.
   Exact fret/fingering → never graded without human-verified maps or a confidence
   floor. A wrong note graded as "wrong" is worse than no judgment — it teaches the
   player to distrust the machine *and* reinforces errors.

4. **The incumbent anti-pattern to avoid is "the map is the music."** Every successful
   mechanic in Rocksmith (Riff Repeater, adaptive difficulty, slow-down practice) exists
   to serve the highway. The learning-science version inverts this: the highway is one
   *scaffold* among several (tab, staff, fretboard diagram, no-visual), and the
   curriculum's job is to *fade* it. The metric that proves this product works is not
   hit-rate-on-highway; it is **map-off performance** — can the player reproduce the
   material with the screen blanked, on a delay, days later. That is the retention
   transfer test the literature says matters and no incumbent measures.

5. **The practice-loop features are mostly cheap because Lyra's decode layer is already
   source-agnostic.** `TrackDecoder.seek()` is accurate, `Engine::position_secs()` is a
   real delivered-samples clock, and any `ByteSource` decodes — so A–B section looping
   is UI work, and *offline* tempo variants (render 60/70/80% time-stretched copies at
   map-generation time, keep them as practice artifacts) sidestep the missing real-time
   time-stretcher entirely for V1–V2. Real-time stretch can come later; it is marked
   NEW and should not block anything.

The recommendation, in one sentence: **keep dino-shred as the fast-iteration learning
prototype where the pedagogy is proven, and build the Lyra integration as the V3 target
whose input/judgment path is a Rust port of dino-shred's architecture — not a repurpose
of the visualization pipeline.**

---

## 2. Pedagogy foundations

### 2.1 Deliberate practice — what it actually requires

Ericsson, Krampe & Tesch-Römer (1993) defined deliberate practice as activity that is
(a) targeted at specific, just-beyond-current-ability weaknesses, (b) effortful rather
than enjoyable, (c) accompanied by immediate informative feedback, and (d) structured
with rest intervals [1]. The follow-up meta-analysis is the part everyone skips:
deliberate practice explains only ~21% of variance in music performance (Macnamara,
Hambrick & Oswald 2014 [2]), which means two things for product design: practice *time*
is a weak metric, and practice *quality/structure* is where a tool can actually move the
needle. A game that logs "30 minutes played" is measuring the wrong variable; a game that
logs "7 focused attempts on bars 9–12 at 80% tempo with declining timing variance" is
measuring practice.

Design consequence: **the unit of the product is the *attempt*, not the session.** Every
mechanic should answer: what was the target, what was the result, what happens next.
dino-shred already has the right primitive — every onset produces a judgment event with
a signed millisecond error — but the game currently discards the pedagogical payload:
no per-section aggregation, no next-attempt recommendation, no "stop here, this section
is your bottleneck" logic.

### 2.2 Chunking and cognitive load

Expert musical performance runs on chunks — a chord shape is one working-memory item for
an expert and four+ for a beginner (Chase & Simon 1973 tradition; for music specifically
see the sight-reading literature, e.g. Kopiez & Lee 2006, and the working-memory review
in Meinz & Hambrick 2010 [3]). Cognitive load theory's implication for a guitar trainer
is severe: a beginner asked to simultaneously track a scrolling highway, decode
fret/string position, execute the fretting-hand shape, time the picking hand, and listen
to the result is in **intrinsic + extraneous overload**. This is precisely the Rocksmith
failure mode for absolute beginners — the highway itself is extraneous load, a second
notation system to decode.

Design consequences:

- **One new demand at a time.** dino-shred V1 strips pitch, fingering, and reading down
  to *timing alone* — this is correct scaffolding. Each subsequent layer should add
  exactly one demand: pitch identity (V2a), then position/fingering (V2b), then
  sequence memory (V3).
- **Representations are scaffolds and must fade.** Show the map; then show less of the
  map; then show a metronome only; then show nothing. A learning product that cannot
  take its display away is a crutch manufacturer.
- The 2025 bi-temporal notation study [4] (musicians process notation on two time axes —
  the score's structural time vs. performance's real time) suggests that *ahead-looking*
  displays (dino-shred's 3 s lookahead, the highway) reduce the bi-temporal conflict for
  beginners, but at the cost of training anticipation-through-eyes instead of
  anticipation-through-ears. Fade the lookahead window as a difficulty axis, not just
  density.

### 2.3 Spacing, interleaving, desirable difficulties

- **Distributed practice beats massed practice** for retention; in musicians
  specifically, Duke, Simmons & Davis (2005) [5] found 24-hour spacing effects, and
  sleep-dependent consolidation is well established. A 2025 melodic-learning study [6]
  found spacing benefits persist for motor sequence learning in music.
- **Interleaving hurts now, helps later.** Interleaving musical interval training
  improved discrimination vs. blocked practice (Wong et al. 2020 [7]); contextual
  interference improved retention in violin practice (Guillaume et al. 2023 [8]). The
  canonical caveat applies: for *initial acquisition* of a motor pattern, blocked
  practice wins; interleaving pays off at the *discrimination and retention* stage
  (Schmidt & Bjork 1992, "desirable difficulties" [9]).
- **Retrieval is the skill.** Testing (playing the section cold, without warm-up or
  display) produces retention that review does not. This is why the mastery gate below
  includes a *cold-open retrieval test*.

Design consequence — the session shape this implies, and the one the product should
literally impose:

```
warm-up (2–3 min, below-ability material, feedback ON)
  → targeted work blocks (5–8 min each, 1–2 weak sections, feedback ON, looped)
  → interleaved rotation (second song/section, moderate difficulty)
  → retrieval test (play the target section cold, feedback OFF, scored)
  → free play (enjoyment, anything — this is not optional; it is the
      part of the session that makes the next session happen)
```

This "work-then-run" alternation mirrors what Chaffin, Logan & Begosh (2011) [10] found
expert concert pianists actually do: they alternate micro-work on difficult segments
with complete runs — the runs are where chunk boundaries get welded into performance
continuity. A pure drill tool that never makes you run the piece end-to-end is
deficient; a pure play-through tool (Rocksmith's default mode) that never isolates is
equally deficient.

### 2.4 Flow and dynamic difficulty

Flow requires challenge ≈ skill (Csikszentmihalyi; for games specifically, Chen 2007).
The empirical knob: hit-rate. An ~80–85% success rate sits in the productive zone —
high enough for engagement, low enough that errors are informative (the "85% rule" from
optimal-learning modeling, Wilson et al. 2019, is a reasonable prior, not a law [11]).
dino-shred's `Spawner` already ramps obstacle density (every-4th-beat → every-2nd →
every beat), which is the right *axis*; what it lacks is feedback *into* the ramp —
difficulty is currently a monotonic schedule, not a controller. The correct V1.5 change
is trivial given existing metrics: regress density when rolling hit-rate < ~75%,
advance when > ~90%. Every incumbent does this (Rocksmith's adaptive difficulty,
Yousician's skill levels); the pedagogy literature says do it on *retention-weighted*
accuracy, not raw hits — a hit that only happens with the click sounding is not the
same hit as one under metronome withdrawal.

---

## 3. Feedback design

### 3.1 Latency budgets — what the research actually says

Three different numbers get conflated in product discussions; keep them separate:

- **Action–sound latency** (strike the string → hear yourself). The NIME community's
  benchmark study (McPherson, Jack & Moro 2016 [12]) sets ~10 ms as the target for
  "feels like an instrument," with degradation reported past ~20 ms for percussion-class
  actions. This matters for Lyra because *any input→monitoring path* must live here —
  and because it explains why guitar-through-computer feels bad even before judgment.
  dino-shred sidesteps it: the player hears the acoustic/pedal chain, not the computer.
  Lyra should do the same — **never put the guitar's own sound through the app** in V1/V2.
- **Judgment/display latency** (strike → score shown). Delayed auditory feedback research
  (Pfordresher & Palmer 2002 [13]; Finney 1997 [14]) shows performance disruption when
  *auditory* feedback is delayed by ~25–30% of the inter-onset interval — but a *visual
  score* arriving one frame later is not part of the motor loop the same way. Musicians'
  JND for the *timing* of an event against a beat is on the order of 10–20 ms for trained
  listeners (Repp & Su 2013 sensorimotor-synchronization review [15]), so the *measured*
  error must be computed from timestamps well inside that, and the *shown* feedback can
  be looser. dino-shred's measured ~15–25 ms string→judgment against the stated ≤40 ms
  budget (`docs/research/2026-07-03-audio-stack-research.md`) is correct engineering for
  exactly this reason: the budget belongs to the *measurement* path, not the pixels.
- **Clock quality.** `docs/learning/01-the-audio-clock.md` already teaches the right
  lesson: frame clocks and wall clocks drift and jitter; only the audio device clock is
  admissible for scoring. The Lyra corollary is non-obvious and worth stating plainly:
  **`VizFrame.seq` is not a clock.** It is a frame counter that stalls when playback
  pauses, is skipped under `try_lock` contention (`lib.rs:567`), and reflects audio that
  may be seconds ahead of what the user hears. The only defensible "musical now" in
  Lyra today is `Engine::position_secs()` — frames actually delivered to the device
  (`OutputTap::fill`, `lib.rs:197–200`). Even better on the HAL path: the IOProc
  trampoline already receives `_in_time`/`_out_time` `AudioTimeStamp`s
  (`crates/lyra-hal/src/lib.rs:217–225`) and ignores them; using `mSampleTime` gives the
  same hardware-anchored clock dino-shred gets from `inputBufferAdcTime`/
  `outputBufferDacTime` for free.

### 3.2 Immediate vs delayed, concurrent vs terminal

The motor-learning feedback literature (Salmoni, Schmidt & Walter 1984; Sigrist et al.
2013 augmented-feedback review [16]; Wulf, Shea & Lewthwaite 2010 [17]) converges on a
pattern every instrument teacher already knows:

- **Concurrent feedback** (judgment during the action) accelerates acquisition but
  produces *guidance dependency* — performance collapses when it is removed.
- **Terminal feedback** (judgment after the action) is fine for discrete events like a
  single onset — and a strum *is* a discrete event, so immediate terminal judgment is
  appropriate and is what dino-shred does.
- **Faded feedback** is the retention mechanism: reduce the bandwidth ("only tell me
  when I'm outside ±60 ms"), then reduce the frequency (judgment shown every Nth
  onset), then summary-only (end-of-phrase report). The guidance-dependency literature
  predicts, correctly, that players trained under always-on concurrent judgment
  underperform on unaided transfer tests — again, the Rocksmith lesson.

Design consequences, all cheap given existing primitives:

- The HUD's early/late bar + grade + histogram (`game/game.py`) is *already the right
  shape* — category (Perfect/Good/OK) for fast uptake, ms readout for the curious. Keep
  both but let the UI hide the ms row for beginners (raw numbers invite
  over-optimization of the wrong thing early).
- Add a **feedback-fade axis** to difficulty in parallel with density: full judgment →
  coarse judgment → no judgment until phrase end → silent run. The measurement schema
  below records which feedback mode each event was produced under, because hit-rate
  under different feedback modes is not comparable.
- A **"ghost mode" / metronome-withdrawal variant**: the click drops out for N bars and
  returns. The metronome-withdrawal literature [18] and every groove teacher agree this
  is where internal pulse is actually built. `ClickScheduler` already renders clicks
  sample-accurately into `outdata`; scheduling silence is free.

### 3.3 Granularity: Perfect/Good/Miss vs milliseconds

Recommendation: **grade in categories, record in milliseconds.** Beginners should see
three buckets and an early/late arrow; the ms readout is a diagnostic for the
measurement store and for advanced players. The ±30/60/100 ms windows in `Judge` are
reasonable starting windows (≈ perceptual-discrimination scales in Repp & Su [15]); what
they lack is *adaptivity* — a beginner at 60 BPM struggling to stay under ±100 ms needs
wider windows early (per the 80–85% success-zone logic above), and the windows should
tighten as a reward for consistency, not as a fixed difficulty ramp. Add "window width"
to the difficulty controller alongside density and feedback fade.

---

## 4. Incumbent analysis

### 4.1 Rocksmith (2011 / 2014)

**What it gets right** — and these are real, earned lessons:

- *Adaptive difficulty* adds/removes note density by observed accuracy — the flow-loop
  literature made product.
- *Riff Repeater* is the single most-copied mechanic because it is the deliberate-
  practice atom: isolate a section, slow it, loop it, score it. Our A–B loop + tempo
  ladder + section mastery is the same idea; the differentiator is what gets *recorded*
  across sessions (§9).
- *Session Mode* (AI band reacting to your playing) is the one feature aimed at
  *musicianship* rather than map-reading — improvisation, dynamics, groove [19].
  Worth stealing much later (V4+): a generative backing loop that reacts to `level`/
  `bass`/onset density is buildable on existing Lyra primitives.
- Real audio input with real latency engineering — the reason it works at all.

**What it gets wrong** (documented criticism, e.g. [20], and consistent with the
transfer literature):

- *Teaches songs, not guitar.* The player learns "the Rocksmith performance of this
  arrangement," including its idiosyncrasies; unaided playing without the screen
  transfers poorly. Nobody at Ubisoft published retention metrics because the product
  cannot survive measuring them.
- *Note-highway literacy ≠ fretboard knowledge.* The highway is a spatial encoding of
  fret×string×time that bypasses note names, intervals, and position logic entirely —
  it trains a reactive skill that doesn't build the fretboard map needed for
  improvisation, transposition, or reading. CAGED/fretboard-fluency approaches [22] do
  the opposite: slow, explicit, transferable.
- *Feedback is a score, not instruction.* "Miss" tells you nothing about *why*
  (early? wrong string? buzzed?). The product can't diagnose because it only measures
  pitch-onset vs. map — it has no model of the player's technique.

**Transferable lesson / anti-pattern pair:** steal Riff Repeater + adaptive density;
refuse "the map is the music." Every map-driven feature must have a defined *fading
schedule* and every curriculum must include map-off assessment.

### 4.2 Yousician

Strengths: structured curriculum with prerequisites (the thing Rocksmith lacks),
bite-size sessions aligned with distributed practice, real-time pitch judgment on
guitar (proof the V2 layer is buildable), strong onboarding. Weaknesses: the score/
streak economy *becomes* the goal — learners grind songs for stars rather than
musicality; the lesson tree is linear where learner needs aren't; and like Rocksmith,
the representation doesn't transfer to reading or fretboard fluency. **Lesson:** a
curriculum graph with prerequisite gates is worth building; a points economy that can
be satisfied without learning is not. Keep streaks/scores instrumental (they gate
content), never terminal (they're not the win condition).

### 4.3 Justin Guitar

Strengths: free, teacher-sequenced curriculum; **One Minute Changes** [21] is the best
public example of a *measurable motor-skill drill* (chord transitions counted per
minute — exactly our per-section hit-rate metric applied to chord changes); explicit
attention to practice routines ("practice like this") rather than just content.
Weakness: no instrument feedback — all self-assessment. **Lesson:** the hybrid is the
point of this product — Justin Guitar's *curriculum structure and drill taxonomy* on
top of dino-shred's *measurement* is more valuable than either alone. Steal the drill
formats: 1-minute changes, strum-patterns-with-accents, "play along at N% speed."

### 4.4 Simply Piano

Strengths: ruthless accessibility — minimal theory up front, instant feedback, short
loops; proof that device-mic pitch judgment is good enough for beginner repertoire.
Weaknesses: screen-reading substitutes for musicianship at scale; thin long-term
transfer evidence. **Lesson for our stack:** onboarding quality is a first-class
feature (dino-shred's calibration flow — strum chugs against a click, median+MAD offset
— is already the right *shape* of a first-run experience; it just needs to teach
something while it measures).

### 4.5 The gap nobody fills

Every commercial incumbent optimizes *engagement metrics on their own maps*. None of
them measure or train unaided retention; none of them fade their notation; none of them
teach fretboard structure. That gap is the product thesis. It is also conveniently the
part the pedagogy literature is most confident about (spacing, retrieval, feedback
fading, interleaving all have decades of replication), so the bet is grounded, not
contrarian.

---

## 5. Rhythm-first vs pitch-first: is V1 defensible?

**Yes — with conditions.** The case for timing-first: (a) it is the lowest-cognitive-
load skill that still produces musical reward; (b) timing error is *objectively
measurable* with the existing stack, so the feedback loop is honest; (c) rhythm/pulse
is a documented prerequisite substrate — syncopation and subdivision training transfer
to ensemble playing in ways pitch drills don't (instrument-specific rhythm-processing
studies, e.g. [28]); (d) the motor pattern being trained (right-hand attack timing) is
the same one every later stage reuses.

The conditions that keep it a foundation rather than a dead end:

1. **Teach pulse explicitly, not implicitly.** V1 currently rewards hitting *detected*
  onsets near beats; add content that teaches the hierarchy — downbeats, subdivisions,
  rests. `Conductor` already gives `beat_time(n)`, `beats_in(start,end)`; a "subdivision
  level" is one parameter of grid density away. Chug-every-beat is the seed; the
  exercises should be: hit only beats 2&4 (backbeat), hit eighths while only quarters
  are scored, syncopated patterns against the same grid.
2. **Metronome withdrawal from day one** (§3.2): internal pulse is the actual skill;
  the click is scaffolding.
3. **A gate that forces progression.** The mastery gate out of V1 should require
   demonstrated internal timing (e.g., ≥85% within ±60 ms over 3 sessions *including*
   withdrawal bars and a next-day cold re-test), not just high streaks. If the player
   camps in rhythm-trainer-land forever, we've built a rhythm game — fine as a game,
   not the goal.

**When does pitch enter?** When the player can hold pulse under withdrawal — i.e., when
timing is automatized enough that adding fretboard demands won't produce cognitive
overload (§2.2). In practice: gate V2 on the V1 mastery criteria. Pitch enters as
*single-note identity in a constrained fretboard region* (one position, 4–5 frets,
2 strings), because polyphonic/chord judgment is a harder DSP problem AND a harder
motor problem — sequence the learner the same way you sequence the engineering.

---

## 6. Map generation constrains pedagogy

The pipeline: FLAC from Lyra's library (`TrackDecoder` decodes any `ByteSource` —
local, SSH, torrent — into interleaved f32, `lyra-formats/src/lib.rs:219`) → offline
analysis → a **song map**: beat/downbeat grid, sections, chord events, note events,
each annotated with confidence.

2026 tooling, honestly assessed:

- **Beats/downbeats:** `beat_this` (ISMIR 2024, [23]) — current SOTA-class, no DBN
  postprocessing, ONNX-able, already flagged as the likely choice in dino-shred's own
  research doc. librosa remains the fallback baseline [24]. madmom is deprioritized —
  it is unmaintained and was broken in the local environment; do not fight it.
- **Notes:** Basic Pitch (Spotify, ICASSP 2023 [25]) — polyphonic, confidence-scored,
  but trained mostly on piano-adjacent data; on guitar expect systematic errors on
  muted chugs, harmonics, slides. CREPE [26] is monophonic pitch-tracking — fine for
  melody, wrong tool for chords. Guitar-specific tablature transcription (fret/string
  assignment, not just pitch) is an active research area with no dominant open tool;
  the string/fret ambiguity is irreducible from audio alone — the same pitch lives at
  up to five positions, and only playability priors disambiguate.
- **Chords:** recognition models are decent on roots/qualities, weaker on extensions
  and guitar voicings; treat as accompaniment-level truth.
- **Stems:** Demucs [27] can isolate the guitar stem — big win for *both* analysis
  (transcribe the stem, not the mix) and UX (drop the stem out so the player replaces
  it — the "karaoke" feature). Cost: heavyweight model, offline-only, keep out of the
  runtime exactly as dino-shred's ADR already decided for torch.

**The rule that makes this safe: confidence gates features, not the other way around.**

| Map layer | Typical confidence | Features allowed to consume it |
|---|---|---|
| Beat/downbeat grid | high | **scored** timing judgment, click sync, highway scroll |
| Section boundaries | medium-high | section looping, practice targets |
| Chord root/quality | medium | display, accompaniment, "play along" — never graded note-by-note |
| Note events (pitch+time) | medium-low on guitar | graded only when confidence ≥ threshold AND string/fret unconstrained |
| String/fret/fingering | low from audio | **never auto-graded** — display only when human-verified or from external tab sources |

And the standing policy: **a generated map is never presented as authoritative
notation.** Confidence-low sections render differently (dimmed highway notes, no
grade), and the UI says why. This is both an honesty constraint and a pedagogical one —
grading a player against a wrong map is the worst feedback a learning product can give.

A clean consequence for V3: prefer *user-supplied or verified* maps (Guitar Pro /
MusicXML import is a solved format problem) for graded note content, and reserve
auto-generated maps for rhythm/section-level features where confidence is high. That
inverts the Rocksmith pipeline (all maps hand-authored, tiny catalog) into something
scalable without inheriting transcription's failure modes.

---

## 7. Learning-loop architecture for this stack

Each recommendation is marked against the real code. **REUSE** names the actual type/
file. **NEW** is a capability that does not exist in either codebase. Three
**ABSENT-BY-DESIGN** traps are called out first because everything else depends on
not stepping in them.

### 7.1 The three traps (read before designing anything)

1. **`VizFrame.beat` is not a beat clock.** `BeatDetect` (`crates/lyra-viz/src/beat.rs`)
   fires a decaying pulse when block energy exceeds ~1.6× its ~1 s EMA, with a 100 ms
   holdoff. That is an *onset-ish transient meter* for visuals. It emits no timestamp,
   no tempo, no beat phase, no downbeat, and it cannot fire faster than 10 Hz — so it
   misses fast subdivisions by construction and fires spuriously on any loud transient.
   Do not judge anything against it; do not even derive obstacle timing from it in a
   learning context. (It is *fine* for the decorative dino-mode that
   `docs/research/dino-shred-integration.md` proposes — a game that surfs the music —
   but that spec was written against the keyboard-only `main.py`, not the PR-1 input
   pipeline, and describes a toy, not a trainer.)
2. **The viz tap is not at the audible position.** `VizTap::push` runs in the decode
   worker post-EQ/pre-ring (`lib.rs:564–569`), so features describe audio up to
   ~1 s @192k … ~4.3 s @44.1k *ahead* of the speakers (RING_SAMPLES). Also, `try_lock`
   means the tap *drops frames* under UI contention — unacceptable for judgment, fine
   for pixels. The audible clock is `OutputTap`'s `position_secs` (delivered frames /
   rate, `lib.rs:197–200`), surfaced as `Engine::position_secs()` →
   `lyra_engine_position` → `LyraPlayer.position`. Note the asymmetry a highway can
   exploit for free: viz features are *lookahead*, position is *now*.
3. **Spectrum bands are not pitch detection, and waveform rings are not events.**
   `SpectrumAnalyzer` (`spectrum.rs`) produces 64 normalized geometric bands with
   attack/decay smoothing — good for confidence meters and energy displays, not note
   identity. `Oscilloscope` (`osc.rs`) decimates into 256-point display rings (~20 ms
   window) with no timestamps. Neither carries input audio anyway: the entire viz
   pipeline taps **playback output**, and there is no input path in the engine at all.

### 7.2 Recommended architecture, piece by piece

**(a) Guitar input path — NEW, and the critical path for everything.**

dino-shred's model: one full-duplex `sounddevice` stream, hardware ADC/DAC timestamps,
5.33 ms blocks @48k/256, onsets emitted through a bounded queue(64), drops counted,
callback does bounded work only. The Lyra port:

- *Compat path:* a `cpal` **input** stream (Lyra already uses cpal output) running the
  EnergyGate port in its callback, timestamping onsets in the device's sample-time
  domain and pushing `OnsetEvent{t, strength}` onto an SPSC ring — mirror of
  dino-shred's `audio/engine.py` + `detect.py`.
- *HAL path:* an IOProc on the input device — the trampoline signature already carries
  `_in_data`, `_in_time`, `_out_time` (`lyra-hal/src/lib.rs:217`); a guitar interface is
  just a second `HalDevice` with input streams. When in/out are different devices, the
  AudioTimeStamp host-time ↔ sample-time conversion reconciles the two clocks — this
  is the macOS-native equivalent of PortAudio's ADC/DAC timestamps.
- *Entitlement:* `com.apple.security.audio` is **absent from `entitlements.plist`**
  (the file's own comment notes audio *output* needs nothing; BLUEPRINT.md confirms
  only `audio-input` is entitled). Sandboxed mic access = plist + TCC prompt. Small
  but real.
- Latency compensation: port `rhythm/calibrate.py` wholesale — strum-to-click, ≥20
  hits, median + MAD rejection, persist per-device offset (it already persists
  device/detector/blocksize metadata; do exactly that in Lyra's store).

**(b) The conductor — REUSE the pattern, NEW in Lyra.**

`Conductor(t0, bpm)`'s drift-free `beat_time(n) = t0 + n·period` is eight lines of math
that any host can port. In Lyra the grid comes from the offline map (`beat_this`), and
the anchor `t0` is a *position in the song*, not a stream time — grid alignment is
`beat_time(n)` in song-seconds vs. `position_secs()` (+ output-latency estimate) as
"now." This is *simpler* than dino-shred's version because the song's grid is fixed
data, not a live BPM. Tempo-scaled playback scales the grid linearly. Missing piece:
`position_secs` is a frame counter on the compat path and unused-timestamp'd on HAL —
promoting it to `AudioTimeStamp`-anchored time is a small engine change, do it.

**(c) The judge — REUSE (port).**

`Judge` (`rhythm/judge.py`): error = onset_t − offset − beat_time(nearest), ±30/60/100
windows, one onset claims at most one beat, `sweep_misses()` for unhit beats, streaks
+ grade histogram + raw errors. Port to Rust in a new `lyra-coach` crate (or Swift —
but Rust keeps the RT-adjacent code in one place and testable headless, exactly like
`VizTap`'s pure-DSP test seam in `crates/lyra-engine/tests/viz_tap.rs`). The judge
consumes two things that already exist conceptually: the input-event ring (new, §a)
and the beat grid (new data, §b). Windows become adaptive (§3.3).

**(d) Click/metronome — REUSE design, small NEW mix point.**

dino-shred's `ClickScheduler` renders clicks sample-accurately into the duplex output.
In Lyra the equivalent is a click synth mixed into `OutputTap::fill` right after
`pop_slice` (`lib.rs:177–196`) — the callback already fills silence on underrun, so
adding a bounded click voice is a few lines inside the existing RT contract (no alloc,
no locks). Accent pattern from the map's downbeat layer. Withdrawal scheduling = the
same synth told to skip bars.

**(e) Practice loop mechanics — mostly REUSE.**

- *A–B section looping:* `Command::Seek` + `TrackDecoder::seek` (accurate seek,
  `lyra-formats:243`) + `position_secs` — a loop is a UI-level "when position ≥ B,
  seek(A)." Pure product feature, no new audio infrastructure.
- *Tempo scaling:* **NEW if real-time, REUSE-ish if offline.** There is no
  time-stretcher in the tree — rubato in the worker is *rate-matching SRC* (changes
  pitch with speed), and `lyra-dsp` is EQ+limiter only. The cheap correct V1/V2 path:
  render time-stretched *copies* at map-generation time (WSOLA/phase-vocoder offline —
  Rubber Band or signalsmith-stretch-class), keep them as practice artifacts; any
  ByteSource decodes, so a stretched artifact is just another track. Real-time stretch
  in the worker loop is a later optimization — do not block practice features on it.
- *Stem drop-out ("play instead of the record"):* NEW; depends on Demucs at map-gen +
  a parallel decode of the minus-guitar stem. The engine already plays arbitrary
  sources; mixing/replacing is worker-loop work.
- *Section selection:* section boundaries come from the map (§6); UI picks them off
  the WaveSeek-style display — `RenderField.swift` already has a waveform seekbar
  pattern to copy.

**(f) The highway and judgment HUD — NEW renderer, REUSE the entire viz pattern.**

A note-highway is exactly a `VizMode` + Canvas renderer + `VizState`-style state bag —
the pattern in `VizMode.swift` / `VizDraw.render` / `Render*.swift` is built for this.
Differences from decorative modes: it consumes the *song map* (timed events) +
judgment events (new FFI function, e.g. `lyra_coach_events` — a second lock+memcpy
snapshot channel next to `lyra_engine_viz_frame`, same shape as `Bridge.swift:170–200`)
rather than `bands`. Judgment presentation per §3: early/late bar, categories, streak;
scroll position driven by `position_secs` + map, not by viz seq. `VizState`'s
beat-edge pattern (`tickFireworks`, `f.beat > 0.55 && prevBeat <= 0.55`) shows the
team already thinks in edge triggers — same technique for "a note just got judged."

**(g) Session/measurement store — NEW, but the substrate exists.**

`lyra-store` already owns a sqlite library DB with incremental sync
(`Bridge.swift:38–89` wraps `lyra_lib_*`). Practice events/attempts/maps are more rows
in the same store: `coach_event`, `coach_attempt`, `song_map`, `practice_session`
tables. Persist across days — spacing/retention math requires it.

**(h) Curriculum/curriculum graph — NEW.** Prerequisite DAG (Justin-Guitar-style), the
mastery gates below, the session-shaper in §2.3. Lives in Swift; consumes the
measurement store. This is the actual product.

### 7.3 Where dino-shred fits

dino-shred remains the **prototype harness**: real input path, working judge, fast
Python iteration — the right place to prove the pedagogy (exercise content, gate
criteria, feedback-fade schedules) before any of it gets ported to Rust/Swift. The
measurement schema in §9 should be defined *once* and emitted by both, so a V1 rhythm
dataset from dino-shred is directly comparable to a V3 Lyra run. Do not maintain two
judges long-term; the Rust port happens when V2 pitch work begins.

---

## 8. Concrete progression design

### V1 — Rhythm trainer (exists; needs pedagogical hardening)

**Content ladder** (all on the existing `Conductor`/`ClickScheduler`/`Judge`/`Spawner`
stack — the ladder is *grid patterns and feedback schedules*, not new engine work):

1. Pulse: chug on the beat, click ON, full judgment. (Current game, formalized.)
2. Subdivision: eighths → sixteenths; click on quarters only.
3. Backbeat/accent: hits required on 2 & 4; ghost hits allowed but unscored —
   `Judge` already distinguishes scored vs. off-grid onsets.
4. Syncopation: sparse off-beat patterns against the click.
5. Withdrawal: click drops N bars, judgment continues silently → report at phrase end.
6. Tempo ladder: same patterns at 70→90→110→130 BPM.

**Mastery gate → V2:** rolling ≥85% onsets within ±60 ms over ≥3 sessions spanning
≥2 days; mean |bias| < 10 ms; ≥80% on withdrawal bars; and a *cold-open test* — first
attempt of a session, no warm-up — within the same bounds. (Cold-open is the retrieval
test; it is supposed to be annoying.)

**Known V1 gaps to fix:** difficulty is open-loop (wire hit-rate → density/window
controller); no per-exercise identity (every run is one anonymous stream — tag events
with exercise ID for the store); calibration UX is correct but unteaching — narrate it
("we're measuring your gear's delay") so first-run also teaches.

### V2 — Pitch (single notes → intervals → chord changes)

Enter when V1 gate passes. **NEW capabilities:** input pitch tracking (YIN/pYIN class
in Rust, or a bundled CREPE-class model — but note ADR 0003's "no torch in runtime"
precedent: prefer DSP or a small ONNX model), a constrained fretboard hypothesis
space (the teacher chooses position; the judge only needs to distinguish the ~10
reachable pitches — this collapses the hard transcription problem into a tractable
classification problem), and fretboard-display UI.

**Ladder:** single notes in one position → two-string alternation → position shifts →
simple intervals → chord *transitions* (One-Minute-Changes mechanic: transitions/minute
is the score) → chord-in-time (strum the right chord on the right beat — fuses V1
timing with V2 pitch).

**Gates:** pitch accuracy ≥85% at slow tempo before speed; chord-change rate targets
(e.g., 30 clean transitions/min before song work). Every pitch judgment still records
timing error — rhythm never stops being measured.

### V3 — Full songs

**NEW:** song maps (§6), section looping UI, tempo variants, stem drop-out, the
highway, the map-off assessment mode.

**Structure per song:** map import/generate → section inventory with per-section
difficulty → learner works weakest-section-first (the measurement store knows exactly
which) at 60–80% tempo → ladder to 100% → complete runs (Chaffin's work/run
alternation, mandatory) → **map-off performance test** (the differentiator — play the
section with the highway blanked, judged from input alone) → song enters the spaced
rotation pool (revisit at +1d/+3d/+7d with cold tests; decay the interval on success).

**Progressive representation policy** (the anti-highway-capture design): each section
is practiced under a representation ladder — highway+tab → highway only → tab/staff
only → fretboard-diagram only → blank. Difficulty credit scales with representation
faded. If a player can only pass with the highway on, the song is not learned; the
product says so.

**Mastery per song:** map-off cold test ≥85% on all sections at full tempo, plus one
complete run. Then the song goes to maintenance (monthly cold checks), and the *next*
song is selected to interleave a different rhythmic/positional demand.

---

## 9. Measurement schema

Defined once, emitted by both codebases. Everything keyed by `(player, exercise_or_map,
rep_id)`; all timestamps in the audio-clock domain.

**Per event** (one row per onset/note attempt):
`event_id, session_id, exercise_id/map_section, beat_index, t_input (device clock),
t_target (grid), error_ms, grade, pitch_target, pitch_detected, pitch_conf,
map_conf, feedback_mode (full/coarse/end-only/silent), representation_mode,
latency_offset_used`

**Per section attempt:** `attempt_id, song_map_id, section, tempo_scale, rep_mode,
hit_rate, grade_hist{perfect,good,ok,miss}, mean_error_ms, sd_error_ms,
mean_bias_ms (early−/late+), max_streak, offgrid_rate, notes_wrong_pitch,
cold_open (bool)`

**Per session:** `session_id, date, duration, exercises_touched, work_blocks vs
run_blocks ratio, warmup_done, retrieval_tests[{exercise, score}], fatigue_marker
(accuracy slope over final 20% of session), calibration_id`

**Per day/week (derived):** retention deltas (cold-open today vs. best-of-yesterday —
*the* spacing metric), interleaving coverage, per-song decay curves, plateau detector.

**Plateau & progression triggers:**
- *Advance difficulty* when: hit-rate ≥90% AND sd_error_ms shrinking over last 3
  sessions AND cold-test passed.
- *Hold/regress* when: hit-rate <75%, or bias drift >15 ms, or accuracy drops under
  feedback-fade (guidance-dependency signature: good scores with feedback, collapse
  without → *increase fading, don't increase feedback*).
- *Plateau* = 5 sessions no improvement in sd or hit-rate at same level → force a
  *different* demand (change representation, change tempo, interleave a second skill)
  rather than more reps — the contextual-interference literature's actual advice.
- *Transfer check* (the metric no incumbent has): periodic map-off/unaided cold tests;
  if in-app mastery climbs while map-off stays flat, the product is teaching the game —
  this flag is the integrity metric for the whole thesis.

---

## 10. Open questions

1. **Input hardware story on macOS.** dino-shred targets an M-Track instrument input.
   Lyra's sandboxed app + TCC mic prompt is fine, but the *user-facing* answer ("plug
   your guitar into X") needs a hardware recommendation and the calibration flow must
   survive device swaps (calibrate.py already keys offsets to device metadata —
   preserve that).
2. **Two-clock reconciliation.** Input device ≠ output device means ADC and DAC clocks
   drift relative to each other; AudioTimeStamp host-time bridging handles it, but the
   calibration loop must also *re-verify* drift over a long session. dino-shred's
   single-duplex-stream dodge isn't available on macOS unless the same device does both.
3. **How honest to be about pitch latency.** pYIN-class trackers need ~40–90 ms of
   window for low E (~82 Hz); pitch judgment will always lag timing judgment by an
   order of magnitude. UX must not pretend otherwise — show pitch verdict when known,
   never block the timing verdict on it.
4. **Map format + storage.** One schema for verified-imported vs. auto-generated maps
   with per-layer confidence (§6 table). Guitar Pro/MusicXML import is likely the V3
   graded-content source of truth — confirm licensing/format story before committing.
5. **Offline tempo-variant storage cost.** Stretched copies of FLACs are big; consider
   stretch-on-demand into a cache, or lossy-but-fine practice variants (Opus) since
   practice audio doesn't need the lossless promise the main library does.
6. **Does stem drop-out need Demucs at all?** For songs where the guitar is
   center-panned or the stems exist (Rocksmith-style multitracks, some archive.org
   sources), simpler EQ/pan tricks may suffice. Evaluate per-source.
7. **Curriculum authoring.** The exercise DAG needs actual content — sequence, gate
   thresholds, repertoire. This is a *teaching* hire problem, not an engineering one;
   the schema above is the delivery mechanism for it.
8. **Evidence gap.** The post-2024 game-based-music-learning reviews [29, 30] are
   honest that long-term transfer evidence is thin and evaluation is non-standardized —
   our measurement schema is designed to *produce* that missing evidence, but no
   published work guarantees the map-off-assessment approach outperforms incumbents.
   It's the best-supported bet, not a proven one.

---

## 11. References

Skill acquisition & practice:

1. Ericsson, Krampe & Tesch-Römer (1993), *The Role of Deliberate Practice in the
   Acquisition of Expert Performance*, Psychological Review 100(3).
   https://doi.org/10.1037/0033-295x.100.3.363
2. Macnamara, Hambrick & Oswald (2014), deliberate-practice meta-analysis,
   Psychological Science. https://doi.org/10.1177/0956797614535810 — and the
   open review: https://pmc.ncbi.nlm.nih.gov/articles/PMC6824411/
3. Meinz & Hambrick (2010), sight-reading & working memory; overview:
   https://www.frontiersin.org/journals/psychology/articles/10.3389/fpsyg.2019.01080/full
4. Bi-temporal processing in music notation (2025):
   https://www.frontiersin.org/journals/cognition/articles/10.3389/fcogn.2025.1689600/full
5. Duke, Simmons & Davis (2005), distributed practice in musicians:
   https://doi.org/10.1177/0022429411424798
6. Spaced melodic motor learning (2025): https://doi.org/10.1177/03057356251401906
7. Wong et al. (2020), interleaved interval training:
   https://journals.sagepub.com/doi/10.1177/0305735620922595
8. Guillaume et al. (2023), contextual interference in violin:
   https://doi.org/10.1177/00224294231222801
9. Schmidt & Bjork (1992), desirable difficulties:
   https://doi.org/10.1207/s15516709cog1602_3
10. Chaffin, Logan & Begosh (2011), how concert pianists actually practice:
    https://musiclab.uconn.edu/wp-content/uploads/sites/290/2019/03/2011-Lisboa-Chaffin-Logan_.pdf —
    and Chaffin & Imreh (2002) expert memorization:
    https://doi.org/10.1111/j.0956-7976.2002.00462.x
11. Wilson et al. (2019), the ~85% optimal-learning rule, Nature Communications:
    https://doi.org/10.1038/s41467-019-12552-4
12. McPherson, Jack & Moro (2016), *Action-Sound Latency: Are Our Tools Fast Enough?*
    NIME. http://www.nime.org/proceedings/2016/nime2016_paper0042.pdf
13. Pfordresher & Palmer (2002), delayed auditory feedback and performance timing:
    https://doi.org/10.1007/s004260100071
14. Finney (1997), auditory feedback in keyboard performance, Music Perception 15(2):
    https://doi.org/10.2307/40285747
15. Repp & Su (2013), sensorimotor synchronization review:
    https://doi.org/10.3758/s13423-013-0332-2
16. Sigrist et al. (2013), augmented feedback review, Neuroscience & Biobehavioral
    Reviews: https://doi.org/10.1016/j.neubiorev.2012.11.006
17. Wulf, Shea & Lewthwaite (2010), motor-skill learning factors:
    https://doi.org/10.1111/j.1467-8721.2010.01721.x
18. Metronome/subdivision pedagogy study: https://doi.org/10.1525/mp.2010.27.5.389

Incumbents:

19. Rocksmith Session Mode: https://www.killscreen.com/can-you-teach-robot-musicians-to-jam/
    and https://gameinformer.com/games/rocksmith_2014/b/xbox360/archive/2013/06/13/artificially-intelligent-jamming.aspx
20. Rocksmith critique ("a terrible way to learn guitar" — take as one pole of the
    debate): https://masters-of-music.com/review-rocksmith-2014-edition-a-terrible-way-to-learn-guitar/
21. JustinGuitar, One Minute Changes:
    https://www.justinguitar.com/guitar-lessons/one-minute-changes-exercise-b1-110
22. CAGED fretboard approach: https://www.premierguitar.com/lessons/guitarists-guide-to-caged

Analysis tooling:

23. beat_this (ISMIR 2024 SOTA beat/downbeat tracker): https://github.com/CPJKU/beat_this
24. librosa: https://doi.org/10.25080/Majora-7b98e3ed-003
25. Basic Pitch (Spotify): https://github.com/spotify/basic-pitch — Bittner et al.,
    ICASSP 2023
26. CREPE: https://github.com/marl/crepe — Kim, Salamon, Li & Bello, ICASSP 2018,
    https://arxiv.org/abs/1802.06182
27. Demucs: https://github.com/facebookresearch/demucs — Défossez et al.,
    https://arxiv.org/abs/1911.13254

Field surveys:

28. Instrument-specific rhythm processing: https://www.frontiersin.org/journals/psychology/articles/10.3389/fpsyg.2016.00069/pdf
29. Digital game-based music education systematic review (2024):
    https://doi.org/10.1177/1321103x241270819
30. Serious games and music learning systematic review (2025):
    https://doi.org/10.1111/jcal.70050

Codebase anchors (all verified in-tree):

- dino-shred PR-1: `dino_shred/audio/{engine,detect,clicks}.py`,
  `dino_shred/rhythm/{conductor,judge,calibrate}.py`, `dino_shred/game/{game,spawner,objects}.py`,
  `docs/decisions/0001–0004`, `docs/learning/01-the-audio-clock.md`,
  `docs/research/2026-07-03-audio-stack-research.md`.
- Lyra: `docs/VIZ-CONTRACT.md`; `crates/lyra-viz/src/{frame,beat,spectrum,level,osc}.rs`
  (`VizFrame`, `BeatDetect`, `SpectrumAnalyzer`, `Levels`, `Oscilloscope`);
  `crates/lyra-engine/src/lib.rs` (`VizTap` decode-worker tap, `OutputTap::fill`
  position clock, `Engine::position_secs`); `crates/lyra-formats/src/lib.rs`
  (`TrackDecoder` over Symphonia, any `ByteSource`); `crates/lyra-hal/src/lib.rs`
  (IOProc with unused `AudioTimeStamp` params); `crates/lyra-ffi/src/lib.rs`
  (`lyra_engine_viz_frame`); `app/Sources/LyraApp/Bridge.swift` + `Viz/{VizSurface,
  VizState,VizFrame,VizMode,VizDraw,Render*}.swift`; `entitlements.plist` (no
  `com.apple.security.audio`); `docs/research/dino-shred-integration.md` (decorative
  dino-mode spec written against `main.py`, not PR-1).
