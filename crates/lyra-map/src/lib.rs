//! lyra-map: offline FLAC→song-map transcription pipeline.
//!
//! Batch analysis only — nothing here runs on the RT path. Stages follow
//! `docs/research/transcription-pipeline.md` §4.1 in order:
//! decode → grid → sections → chords → notes → tuning → quantize → tab →
//! layers → quality rollup → `.lyramap` + store registry.
//!
//! Every stage is independently optional and confidence-carrying: a stage
//! may fail (or be skipped for missing weights) without sinking the layers
//! below it. A grid-only map is a valid artifact (`status = "grid"`).

mod audio;
mod chords;
mod format;
mod grid;
mod map;
mod notes;
mod pipeline;
mod quantize;
mod sections;
mod tab;
mod tuning;

pub use audio::{
    hash_source, mixdown_mono, resample_linear, AudioBuses, OfflineDecode, CHROMA_RATE, NOTE_RATE,
};
pub use chords::{chroma_fps, transcribe_chords, ChordTrack};
pub use format::{decode as decode_map_bytes, decode_value, encode as encode_map, MapCodec};
#[cfg(feature = "onnx")]
pub use grid::OnnxBeatTracker;
pub use grid::{
    grid_pos_at, models_dir as grid_models_dir, nearest_beat_idx, BeatTracker, GridEstimate,
    NullBeatTracker,
};
pub use map::*;
#[cfg(feature = "onnx")]
pub use notes::OnnxNoteTranscriber;
pub use notes::{
    assemble_notes, fuse_notes, AssemblyOpts, NoteSet, NoteTranscriber, NullNoteTranscriber,
    RawNote,
};
pub use pipeline::{record_for, registry_status, MapGen, MapOptions, MapRecord, Stage, StageSet};
pub use quantize::{assign_layers, dim_reason, quantize_time, QuantizedNote};
pub use sections::{segment_sections, SectionTrack};
pub use tab::{solve_tab, TabOpts, TabSolution};
pub use tuning::{estimate_tuning, TUNING_CANDIDATES};
