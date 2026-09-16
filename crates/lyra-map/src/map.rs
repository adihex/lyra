//! SongMap contract: the versioned artifact every downstream consumer
//! (highway renderer, judge, difficulty system, measurement schema) reads.
//! See `docs/research/transcription-pipeline.md` §1 and the §8 sketch.
//!
//! Dual clocks by design: every timed event carries audio seconds (the
//! judge's clock) AND a grid position (bar:beat:tick, the renderer's clock),
//! so the highway scrolls at constant musical speed through tempo drift
//! while judgment stays in stream-clock seconds.

use serde::{Deserialize, Serialize};

/// Version tag stamped on every map; a bump forces regeneration, not migration.
pub const PIPELINE_VERSION: &str = "mapgen-0.1|grid-null-0.1|chroma-hmm-0.1|notes-null-0.1";
/// Ticks per beat in grid positions (MIDI PPQ convention).
pub const TICKS_PER_BEAT: u16 = 480;
/// Judge-policy thresholds (§5.2): graded ≥ HI, advisory LO..HI, ghost < LO.
pub const TAU_HI: f32 = 0.7;
pub const TAU_LO: f32 = 0.4;
/// Difficulty-layer bits (§4.4, Rocksmith NLD-style: lower levels are the
/// same chart with notes removed, so membership is cumulative downward).
pub const L0_ANCHOR: u8 = 1 << 0;
pub const L1_STAB: u8 = 1 << 1;
pub const L2_SKELETON: u8 = 1 << 2;
pub const L3_MELODY: u8 = 1 << 3;
pub const L4_FULL: u8 = 1 << 4;

/// Per-layer outcome. A layer may fail without sinking the ones below it;
/// `Skipped` means the stage was disabled or had no input (e.g. no notes
/// for the tab solver), not an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StageStatus {
    Ok,
    Degraded,
    Failed,
    Skipped,
}

/// Registry status for `track_maps.status` (§4.5) — the tiered-delivery
/// ladder. The UI offers strum mode as soon as status reaches chords.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MapStatus {
    Pending,
    Grid,
    Chords,
    Notes,
    Done,
    Unsupported,
    Failed,
}

impl MapStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            MapStatus::Pending => "pending",
            MapStatus::Grid => "grid",
            MapStatus::Chords => "chords",
            MapStatus::Notes => "notes",
            MapStatus::Done => "done",
            MapStatus::Unsupported => "unsupported",
            MapStatus::Failed => "failed",
        }
    }
}

/// Grid position: the renderer's clock. `beat` is 0-indexed within the bar,
/// `tick` subdivides the beat (`TICKS_PER_BEAT` per beat).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridPos {
    pub bar: u32,
    pub beat: u8,
    pub tick: u16,
}

/// One beat: audio time plus the tracker's activation confidence.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BeatPt {
    pub t_s: f32,
    pub conf: f32,
}

/// Meter change effective from `bar` (supports mid-song changes).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MeterChange {
    pub bar: u32,
    pub beats_per_bar: u8,
}

/// Local tempo effective from `bar`, from inter-beat intervals — never a
/// global constant, so rubato/accelerando stay representable.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TempoMark {
    pub bar: u32,
    pub bpm: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeatGrid {
    pub beats: Vec<BeatPt>,
    /// Indices into `beats` marking bar starts (= downbeats).
    pub downbeats: Vec<u32>,
    pub meter: Vec<MeterChange>,
    pub tempi: Vec<TempoMark>,
    /// 0.5 = straight .. ~0.67 = triplet feel; None when unmeasurable.
    pub swing: Option<f32>,
    /// True when onsets resist quantization (free-time sections) — the
    /// grid itself is uncertain there, so grading suspends (§5.2).
    pub rubato: bool,
    pub status: StageStatus,
    pub conf: f32,
}

impl BeatGrid {
    pub fn empty(status: StageStatus) -> Self {
        Self {
            beats: Vec::new(),
            downbeats: Vec::new(),
            meter: Vec::new(),
            tempi: Vec::new(),
            swing: None,
            rubato: false,
            status,
            conf: 0.0,
        }
    }

    /// Audio time of beat `n` — a table lookup, never a BPM projection.
    pub fn beat_time(&self, n: usize) -> Option<f32> {
        self.beats.get(n).map(|b| b.t_s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SectionLabel {
    Intro,
    Verse,
    Chorus,
    Bridge,
    Solo,
    Interlude,
    Outro,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub t0: f32,
    pub t1: f32,
    pub grid0: GridPos,
    pub grid1: GridPos,
    pub label: SectionLabel,
    pub conf: f32,
}

/// P0 vocabulary: 12 roots × {maj, min} + N.C. Extended qualities (7/sus/…)
/// are a P1 conformer upgrade — same decoder, more states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChordQuality {
    Maj,
    Min,
    Nc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChordEvent {    pub t0: f32,
    pub t1: f32,
    pub grid0: GridPos,
    pub grid1: GridPos,
    /// Pitch class 0..12 (C..B); None = N.C. (no chord).
    pub root: Option<u8>,
    pub quality: ChordQuality,
    pub bass: Option<u8>,
    /// Posterior margin (top1 − top2): ambiguity reads as low margin.
    pub conf: f32,
    /// Runner-up quality when the margin is thin — never force an exotic label.
    pub alt: Option<ChordQuality>,
}

/// Strum onset inside a chord segment — the strum-chart event ("strum on
/// beat 2-and"), aligned to the grid at detection time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrumEvent {
    pub t: f32,
    pub grid: GridPos,
    pub conf: f32,
}

/// Technique flags on a note — best-effort markers, never silent (§3.5).
/// A muted chug (onset, no resolvable pitch) is a rhythm event, not dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TechFlags(pub u8);

impl TechFlags {
    pub const NONE: u8 = 0;
    pub const PALM_MUTE: u8 = 1 << 0;
    pub const SLIDE: u8 = 1 << 1;
    pub const BEND: u8 = 1 << 2;
    pub const VIBRATO: u8 = 1 << 3;
    pub const HARMONIC: u8 = 1 << 4;
    pub const CHUG: u8 = 1 << 5;
    pub const FX: u8 = 1 << 6;

    pub fn has(self, flag: u8) -> bool {
        self.0 & flag != 0
    }
}

/// Where a note came from — mix/stem agreement is a model-free confidence
/// signal (§5.1), and verified sources outrank generated ones (§3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provenance {
    Mix,
    Stem,
    Fused,
    Imported,
    HumanEdited,
}

/// Physical playing position: 0 = low-E string … 5 = high-E.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fretting {
    pub string: u8,
    pub fret: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteEvent {
    /// Quantized onset (grid-snapped audio time) — the judge's clock.
    pub onset_s: f32,
    pub offset_s: f32,
    /// Pre-quantization onset — kept so re-quantization never needs re-decode.
    pub raw_onset_s: f32,
    /// Grid position — the renderer's clock.
    pub grid: GridPos,
    /// Continuous MIDI (bend-aware center pitch).
    pub midi: f32,
    pub bend_cents: Option<i16>,
    pub techniques: TechFlags,
    /// Composed: onset_strength × frame_strength × source_agreement (§5.1).
    pub conf: f32,
    pub source: Provenance,
    /// Solver's playable hypothesis + its margin — a suggestion, never
    /// presented as the recording's actual fingering (§3.6).
    pub pos: Option<Fretting>,
    pub sf_conf: Option<f32>,
    /// Difficulty-layer membership (§4.4), cumulative downward.
    pub layer_mask: u8,
    /// Trust-the-player flag (§5.3): aggregate performance disputes this note.
    pub suspect: bool,
}

/// Solver output log, parallel to `notes[].pos` with per-note margins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabEvent {
    /// Index into `SongMap.notes`.
    pub note_idx: usize,
    pub string: u8,
    pub fret: u8,
    /// Cost gap to the runner-up assignment — a confident note can still
    /// have an arbitrary fingering.
    pub margin: f32,
}

/// One alternate fretboard hypothesis (§3.4) — the UI offers the top
/// alternates instead of silently committing ("detected drop-D · also
/// possible: standard").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuningCandidate {
    pub name: String,
    pub strings: [i8; 6],
    pub capo: u8,
    pub conf: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tuning {
    /// Open-string MIDI, low-E → high-E (standard: [40,45,50,55,59,64]).
    pub strings: [i8; 6],
    /// Global deviation from concert pitch, cents.
    pub global_cents: f32,
    pub capo: u8,
    pub conf: f32,
    pub alternatives: Vec<TuningCandidate>,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            strings: [40, 45, 50, 55, 59, 64],
            global_cents: 0.0,
            capo: 0,
            conf: 0.0,
            alternatives: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerStatus {
    pub grid: StageStatus,
    pub sections: StageStatus,
    pub chords: StageStatus,
    pub notes: StageStatus,
    pub tab: StageStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quality {
    /// p25 of section confs — worst-section-weighted, not mean (§5.1).
    pub overall: f32,
    pub per_section: Vec<f32>,
    pub layers: LayerStatus,
}

/// The artifact. Loaded whole into memory per session — a few hundred KB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongMap {
    /// BLAKE3 of the canonical payload bytes (set at encode time).
    pub map_id: String,
    /// BLAKE3 of the source audio bytes — join key to tracks across
    /// local/torrent/remote copies of the same recording.
    pub audio_hash: String,
    pub pipeline_ver: String,
    pub duration_s: f32,
    pub tuning: Tuning,
    pub grid: BeatGrid,
    pub sections: Vec<Section>,
    pub chords: Vec<ChordEvent>,
    pub strums: Vec<StrumEvent>,
    pub notes: Vec<NoteEvent>,
    pub tab: Vec<TabEvent>,
    pub quality: Quality,
}

impl SongMap {
    /// A grid-only map is already a valid artifact (tiered delivery, §4.3):
    /// strum-along mode needs nothing more than beats + chords.
    pub fn new(audio_hash: String, duration_s: f32) -> Self {
        Self {
            map_id: String::new(),
            audio_hash,
            pipeline_ver: PIPELINE_VERSION.into(),
            duration_s,
            tuning: Tuning::default(),
            grid: BeatGrid::empty(StageStatus::Skipped),
            sections: Vec::new(),
            chords: Vec::new(),
            strums: Vec::new(),
            notes: Vec::new(),
            tab: Vec::new(),
            quality: Quality {
                overall: 0.0,
                per_section: Vec::new(),
                layers: LayerStatus {
                    grid: StageStatus::Skipped,
                    sections: StageStatus::Skipped,
                    chords: StageStatus::Skipped,
                    notes: StageStatus::Skipped,
                    tab: StageStatus::Skipped,
                },
            },
        }
    }
}
