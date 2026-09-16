//! Pipeline orchestrator (§4.1): runs the stages in order, each writing
//! into the SongMap + setting status flags. A stage may fail without
//! sinking earlier layers — the error is recorded in the layer status and
//! the map keeps whatever it already earned. Incremental write-back after
//! each layer lands in `maps_dir`, so grid-only maps are valid artifacts.
//!
//! Public API: `MapGen::for_track(ByteSource) -> SongMap` (+ progress).

use crate::audio::{hash_source, mixdown_mono, OfflineDecode, CHROMA_RATE};
use crate::chords::{chroma_track, transcribe_chords};
use crate::format::encode as encode_map;
use crate::grid::{apply_estimate, BeatTracker, NullBeatTracker};
use crate::map::{MapStatus, NoteEvent, Provenance, SongMap, StageStatus, TabEvent};
use crate::notes::{fuse_notes, NoteTranscriber, NullNoteTranscriber};
use crate::quantize::{assign_layers, is_rubato, quantize_time, subdivision_vote, swing_ratio};
use crate::sections::segment_sections;
use crate::tab::{solve_tab, TabOpts};
use crate::tuning::estimate_tuning;
use lyra_core::LyraError;
use lyra_fs::{ByteSource, LocalFile};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Pipeline stages, in §4.1 run order — also the progress-callback unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Decode,
    Grid,
    Sections,
    Chords,
    Notes,
    Tuning,
    Quantize,
    Tab,
    Layers,
    Write,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Decode => "decode",
            Stage::Grid => "grid",
            Stage::Sections => "sections",
            Stage::Chords => "chords",
            Stage::Notes => "notes",
            Stage::Tuning => "tuning",
            Stage::Quantize => "quantize",
            Stage::Tab => "tab",
            Stage::Layers => "layers",
            Stage::Write => "write",
        }
    }

    fn order(self) -> usize {
        match self {
            Stage::Decode => 0,
            Stage::Grid => 1,
            Stage::Sections => 2,
            Stage::Chords => 3,
            Stage::Notes => 4,
            Stage::Tuning => 5,
            Stage::Quantize => 6,
            Stage::Tab => 7,
            Stage::Layers => 8,
            Stage::Write => 9,
        }
    }
}

/// Stage enablement. Tuning + quantize run whenever notes + grid exist
/// (they're the glue between them, not separately gated); layers always run.
#[derive(Debug, Clone)]
pub struct StageSet {
    pub grid: bool,
    pub sections: bool,
    pub chords: bool,
    pub notes: bool,
    pub tab: bool,
}

impl Default for StageSet {
    fn default() -> Self {
        Self {
            grid: true,
            sections: true,
            chords: true,
            notes: true,
            tab: true,
        }
    }
}

impl StageSet {
    /// Parse `--stages grid,chords,…` / `all` (CLI spelling).
    pub fn parse(s: &str) -> Self {
        if s.trim().eq_ignore_ascii_case("all") {
            return Self::default();
        }
        let mut set = Self {
            grid: false,
            sections: false,
            chords: false,
            notes: false,
            tab: false,
        };
        for part in s.split(',') {
            match part.trim().to_ascii_lowercase().as_str() {
                "grid" => set.grid = true,
                "sections" => set.sections = true,
                "chords" => set.chords = true,
                "notes" => set.notes = true,
                "tab" => set.tab = true,
                _ => {}
            }
        }
        set
    }
}

#[derive(Debug, Clone)]
pub struct MapOptions {
    pub models_dir: Option<PathBuf>,
    pub stages: StageSet,
    /// Incremental write-back target (`maps/<hash>.lyramap` per layer).
    pub maps_dir: Option<PathBuf>,
    /// L3 melody threshold (default 0.6).
    pub tau_melody: f32,
}

impl Default for MapOptions {
    fn default() -> Self {
        Self {
            models_dir: None,
            stages: StageSet::default(),
            maps_dir: None,
            tau_melody: 0.6,
        }
    }
}

/// Registry row for `track_maps` (§4.5) — written by the CLI, not the
/// pipeline (the pipeline stays DB-free; the bin owns the Library).
#[derive(Debug, Clone)]
pub struct MapRecord {
    pub audio_hash: String,
    pub map_path: String,
    pub pipeline_ver: String,
    pub status: String,
    pub overall_conf: Option<f32>,
    pub updated_at: i64,
}

pub struct MapGen {
    opts: MapOptions,
    progress: Option<Arc<dyn Fn(Stage, f32) + Send + Sync>>,
}

impl MapGen {
    pub fn new(opts: MapOptions) -> Self {
        Self {
            opts,
            progress: None,
        }
    }

    pub fn on_progress(mut self, f: impl Fn(Stage, f32) + Send + Sync + 'static) -> Self {
        self.progress = Some(Arc::new(f));
        self
    }

    fn emit(&self, stage: Stage) {
        if let Some(p) = &self.progress {
            p(stage, (stage.order() + 1) as f32 / 10.0);
        }
    }

    /// Convenience over local files (also what `lyra-mapgen` drives).
    pub fn for_path(&self, path: &Path) -> Result<SongMap, LyraError> {
        let ext = path.extension().and_then(|e| e.to_str()).map(str::to_owned);
        let src: Arc<dyn ByteSource> = Arc::new(LocalFile::open(path)?);
        self.for_track(src, ext.as_deref())
    }

    /// Full stage graph over any ByteSource (local, cached-remote, torrent).
    pub fn for_track(
        &self,
        source: Arc<dyn ByteSource>,
        ext: Option<&str>,
    ) -> Result<SongMap, LyraError> {
        self.emit(Stage::Decode);
        let audio_hash = hash_source(source.as_ref())?;
        let buses = OfflineDecode::decode(source, ext)?;
        let mut map = SongMap::new(audio_hash, buses.duration_s);
        self.write_back(&mut map);

        // GRID — Null fallback today; ONNX when the adapter + weights land.
        self.emit(Stage::Grid);
        if self.opts.stages.grid {
            match self.track_grid(&buses.mono_22050) {
                Ok(est) => {
                    apply_estimate(&mut map.grid, est);
                    map.quality.layers.grid = map.grid.status;
                }
                Err(_) => {
                    map.grid.status = StageStatus::Failed;
                    map.quality.layers.grid = StageStatus::Failed;
                }
            }
        }
        self.write_back(&mut map);

        // Shared chroma for sections + chords (one STFT over the track).
        let mono44: Vec<f32> = mixdown_mono(&buses.stereo_44100, 2);
        let (chroma, _bass, hop_s) = if self.opts.stages.sections || self.opts.stages.chords {
            chroma_track(&mono44)
        } else {
            (Vec::new(), Vec::new(), HOP_FALLBACK)
        };

        self.emit(Stage::Sections);
        if self.opts.stages.sections && !chroma.is_empty() {
            let track = segment_sections(&chroma, hop_s, &map.grid);
            map.quality.layers.sections = track.status;
            map.sections = track.sections;
        }
        self.write_back(&mut map);

        self.emit(Stage::Chords);
        if self.opts.stages.chords {
            let track = transcribe_chords(&buses.stereo_44100, &map.grid);
            map.quality.layers.chords = track.status;
            map.chords = track.segments;
            map.strums = track.strums;
        }
        self.write_back(&mut map);

        self.emit(Stage::Notes);
        if self.opts.stages.notes {
            match self.transcribe_notes(&buses.mono_22050) {
                Ok(fused) => {
                    map.quality.layers.notes = if fused.is_empty() {
                        StageStatus::Failed
                    } else {
                        StageStatus::Ok
                    };
                    map.notes = fused
                        .into_iter()
                        .map(|(r, source)| NoteEvent {
                            onset_s: r.onset_s,
                            offset_s: r.offset_s,
                            raw_onset_s: r.onset_s,
                            grid: crate::map::GridPos {
                                bar: 0,
                                beat: 0,
                                tick: 0,
                            },
                            midi: r.midi,
                            bend_cents: r.bend_cents,
                            techniques: crate::map::TechFlags(0),
                            conf: r.conf,
                            source,
                            pos: None,
                            sf_conf: None,
                            layer_mask: 0,
                            suspect: false,
                        })
                        .collect();
                }
                Err(_) => map.quality.layers.notes = StageStatus::Failed,
            }
        }
        self.write_back(&mut map);

        // TUNING from the note multiset (default standard at conf 0 — the
        // solver's prior, never presented as a measurement).
        self.emit(Stage::Tuning);
        if !map.notes.is_empty() {
            let midis: Vec<f32> = map.notes.iter().map(|n| n.midi).collect();
            map.tuning = estimate_tuning(&midis);
        }

        // QUANTIZE: snap to grid, subdivision vote, swing, rubato flag.
        self.emit(Stage::Quantize);
        if !map.notes.is_empty() && !map.grid.beats.is_empty() {
            let qs: Vec<_> = map
                .notes
                .iter()
                .map(|n| quantize_time(&map.grid, n.raw_onset_s))
                .collect();
            for (n, q) in map.notes.iter_mut().zip(qs.iter()) {
                n.grid = q.grid;
                n.onset_s = q.quantized_s;
            }
            let (_sub, _sub_conf) = subdivision_vote(&qs);
            map.grid.swing = swing_ratio(&qs);
            map.grid.rubato = is_rubato(&qs);
        }

        // TAB: position solver conditioned on tuning+capo.
        self.emit(Stage::Tab);
        if self.opts.stages.tab && !map.notes.is_empty() {
            let midis: Vec<f32> = map.notes.iter().map(|n| n.midi).collect();
            let onsets: Vec<f32> = map.notes.iter().map(|n| n.onset_s).collect();
            let sol = solve_tab(&midis, &onsets, &map.tuning, &TabOpts::default());
            map.tab = Vec::with_capacity(map.notes.len());
            for (i, (n, (f, m))) in map
                .notes
                .iter_mut()
                .zip(sol.frettings.iter().zip(sol.margins.iter()))
                .enumerate()
            {
                if let Some(fr) = f {
                    n.pos = Some(*fr);
                    // Margin → [0,1) confidence weight (gap 2 ≈ 0.5).
                    let w = m / (m + 2.0);
                    n.sf_conf = Some(n.conf * w);
                    map.tab.push(TabEvent {
                        note_idx: i,
                        string: fr.string,
                        fret: fr.fret,
                        margin: *m,
                    });
                }
            }
            map.quality.layers.tab = if map.tab.is_empty() {
                StageStatus::Failed
            } else {
                StageStatus::Ok
            };
        }

        // LAYERS + QUALITY rollup.
        self.emit(Stage::Layers);
        assign_layers(&mut map.notes, &map.grid, &map.chords, self.opts.tau_melody);
        rollup_quality(&mut map);

        self.emit(Stage::Write);
        self.write_back(&mut map);
        Ok(map)
    }

    fn track_grid(&self, mono_22050: &[f32]) -> Result<crate::grid::GridEstimate, LyraError> {
        #[cfg(feature = "onnx")]
        if let Some(onnx) = crate::grid::OnnxBeatTracker::discover(self.opts.models_dir.clone()) {
            match onnx.track(mono_22050) {
                Ok(est) => return Ok(est),
                Err(_) => {} // P1 adapter: fall through to Null, status-marked.
            }
        }
        let _ = &self.opts.models_dir;
        NullBeatTracker.track(mono_22050)
    }

    fn transcribe_notes(
        &self,
        mono_22050: &[f32],
    ) -> Result<Vec<(crate::notes::RawNote, Provenance)>, LyraError> {
        #[cfg(feature = "onnx")]
        if let Some(onnx) =
            crate::notes::OnnxNoteTranscriber::discover(self.opts.models_dir.clone())
        {
            match onnx.transcribe(mono_22050) {
                Ok(set) => return Ok(fuse_notes(set.notes, Vec::new())),
                Err(_) => {}
            }
        }
        let set = NullNoteTranscriber.transcribe(mono_22050)?;
        // P0 has no stem stage: fuse mix-only (honest 0.6 single-source).
        Ok(fuse_notes(set.notes, Vec::new()))
    }

    /// Incremental write-back: every layer lands on disk as it completes.
    fn write_back(&self, map: &mut SongMap) {
        let Some(dir) = &self.opts.maps_dir else {
            return;
        };
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join(format!("{}.lyramap", map.audio_hash));
        if let Ok(bytes) = encode_map(map) {
            let _ = std::fs::write(&path, bytes);
        }
    }
}

const HOP_FALLBACK: f32 = 2048.0 / CHROMA_RATE as f32;

/// Section quality = median member-note/chord conf; overall = p25 across
/// sections (worst-section-weighted, §5.1). Empty sections → global mean.
fn rollup_quality(map: &mut SongMap) {
    let mut per_section = Vec::with_capacity(map.sections.len());
    for s in &map.sections {
        let mut confs: Vec<f32> = map
            .notes
            .iter()
            .filter(|n| n.onset_s >= s.t0 && n.onset_s < s.t1)
            .map(|n| n.conf)
            .collect();
        confs.extend(
            map.chords
                .iter()
                .filter(|c| c.t0 < s.t1 && c.t1 > s.t0)
                .map(|c| c.conf),
        );
        confs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        per_section.push(if confs.is_empty() {
            0.0
        } else {
            confs[confs.len() / 2]
        });
    }
    if per_section.is_empty() {
        let mut all: Vec<f32> = map.notes.iter().map(|n| n.conf).collect();
        all.extend(map.chords.iter().map(|c| c.conf));
        all.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let overall = if all.is_empty() {
            0.0
        } else {
            all[all.len() / 4]
        };
        map.quality.overall = overall;
    } else {
        let mut sorted = per_section.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        map.quality.overall = sorted[sorted.len() / 4];
        map.quality.per_section = per_section;
    }
}

/// Registry status from the layer outcomes (the tiered-delivery ladder).
pub fn registry_status(map: &SongMap) -> MapStatus {
    use StageStatus::*;
    if map.duration_s < 3.0 {
        return MapStatus::Unsupported;
    }
    match (
        map.quality.layers.grid,
        map.quality.layers.chords,
        map.quality.layers.notes,
        map.quality.layers.tab,
    ) {
        (Failed, Failed, _, _) => MapStatus::Failed,
        (_, _, Ok, Ok) | (_, _, Ok, Degraded) => MapStatus::Done,
        (_, _, Ok, _) => MapStatus::Notes,
        (_, Ok, _, _) | (_, Degraded, _, _) => MapStatus::Chords,
        (Ok, _, _, _) | (Degraded, _, _, _) => MapStatus::Grid,
        _ => MapStatus::Pending,
    }
}

/// Build the `track_maps` row for a finished map + its on-disk path.
pub fn record_for(map: &SongMap, map_path: &str) -> MapRecord {
    MapRecord {
        audio_hash: map.audio_hash.clone(),
        map_path: map_path.into(),
        pipeline_ver: map.pipeline_ver.clone(),
        status: registry_status(map).as_str().into(),
        overall_conf: Some(map.quality.overall),
        updated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 8 s FLAC-ish? WAV suffices — decoder path is identical (symphonia).
    /// 120 BPM click + A-major pad so grid/chords/notes all fire.
    fn fixture_wav(path: &Path) {
        let sr = 44_100u32;
        let n = (sr as f32 * 8.0) as usize;
        let mut pcm = vec![0i16; n * 2];
        for i in 0..n {
            let t = i as f32 / sr as f32;
            // Click every 0.5 s.
            let click = if (t % 0.5) < 0.01 { 0.9 } else { 0.0 };
            // A-major pad (110/164.81/220/277.18/329.63).
            let pad: f32 = [110.0, 164.81, 220.0, 277.18, 329.63]
                .iter()
                .map(|f| (2.0 * std::f32::consts::PI * f * t).sin())
                .sum();
            let s = ((click + 0.25 * pad / 5.0) * 20000.0) as i16;
            pcm[i * 2] = s;
            pcm[i * 2 + 1] = s;
        }
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + pcm.len() as u32 * 2).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&sr.to_le_bytes());
        b.extend_from_slice(&(sr * 4).to_le_bytes());
        b.extend_from_slice(&4u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(pcm.len() as u32 * 2).to_le_bytes());
        for s in pcm {
            b.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::File::create(path).unwrap().write_all(&b).unwrap();
    }

    #[test]
    fn end_to_end_fixture_map() {
        let dir = std::env::temp_dir().join("lyra_map_pipe");
        let _ = std::fs::create_dir_all(&dir);
        let wav = dir.join("fix.wav");
        fixture_wav(&wav);

        let maps = dir.join("maps");
        let gen = MapGen::new(MapOptions {
            maps_dir: Some(maps.clone()),
            ..MapOptions::default()
        });
        let map = gen.for_path(&wav).unwrap();
        assert!(!map.audio_hash.is_empty());
        assert!((map.duration_s - 8.0).abs() < 0.2);
        assert!(!map.grid.beats.is_empty(), "grid must fire on clicks");
        assert!(!map.chords.is_empty(), "chords must fire on the pad");
        assert!(!map.notes.is_empty(), "null follower must find the pad");
        assert!(!map.tab.is_empty(), "solver must place the notes");
        assert!(map.quality.overall > 0.0);
        // Incremental write-back landed a valid artifact.
        let written = maps.join(format!("{}.lyramap", map.audio_hash));
        assert!(written.is_file());
        let back = crate::format::decode(&std::fs::read(&written).unwrap()).unwrap();
        assert_eq!(back.audio_hash, map.audio_hash);
        // Registry row carries the tiered status.
        let rec = record_for(&map, &written.display().to_string());
        assert!(!rec.map_path.is_empty());
        assert!(["grid", "chords", "notes", "done"].contains(&rec.status.as_str()));
    }

    #[test]
    fn stage_subset_produces_partial_map() {
        let dir = std::env::temp_dir().join("lyra_map_pipe");
        let _ = std::fs::create_dir_all(&dir);
        let wav = dir.join("fix.wav");
        if !wav.is_file() {
            fixture_wav(&wav);
        }
        let gen = MapGen::new(MapOptions {
            stages: StageSet::parse("grid,chords"),
            ..MapOptions::default()
        });
        let map = gen.for_path(&wav).unwrap();
        assert!(!map.grid.beats.is_empty());
        assert!(!map.chords.is_empty());
        assert!(map.notes.is_empty());
        assert_eq!(map.quality.layers.notes, StageStatus::Skipped);
        assert_eq!(registry_status(&map), MapStatus::Chords);
    }

    #[test]
    fn stage_set_parsing() {
        let all = StageSet::parse("all");
        assert!(all.grid && all.tab);
        let some = StageSet::parse("grid, chords,notes");
        assert!(some.grid && some.chords && some.notes && !some.tab);
    }
}
