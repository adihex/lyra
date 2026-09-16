//! QUANTIZE + LAYERS (§4.1, §4.4): snap onsets to the grid (keeping raw
//! AND grid positions), vote the subdivision (2-vs-3), measure swing, flag
//! rubato — then assign the per-note difficulty `layer_mask` density ladder:
//! L0 downbeat anchors → L1 chord-stab additions → L2 eighth skeleton →
//! L3 melody above conf τ → L4 full density (confident notes only).

use crate::grid::{grid_pos_at, nearest_beat_idx};
use crate::map::{
    BeatGrid, ChordEvent, GridPos, NoteEvent, TAU_LO, L0_ANCHOR, L1_STAB,
    L2_SKELETON, L3_MELODY, L4_FULL, TICKS_PER_BEAT,
};

/// Quantized placement of one onset: grid position, grid-derived time,
/// and two residuals — sub-tick (placement precision) and distance to the
/// nearest 16th-note line (musical slop; drives the rubato flag).
#[derive(Debug, Clone, Copy)]
pub struct QuantizedNote {
    pub grid: GridPos,
    pub quantized_s: f32,
    pub residual_s: f32,
    pub grid_residual_s: f32,
}

/// Snap `t` to the grid. Without beats the time passes through untouched
/// (grid 0:0:0) — the caller flags rubato, the note is never warped.
pub fn quantize_time(grid: &BeatGrid, t: f32) -> QuantizedNote {
    let grid_pos = grid_pos_at(grid, t);
    if grid.beats.is_empty() {
        return QuantizedNote {
            grid: grid_pos,
            quantized_s: t,
            residual_s: 0.0,
            grid_residual_s: 0.0,
        };
    }
    let i = nearest_beat_idx(&grid.beats, t);
    let b0 = grid.beats[i].t_s;
    let b1 = grid.beats.get(i + 1).map(|b| b.t_s).unwrap_or(b0 + 0.5);
    // Re-anchor backwards snaps to the previous beat (matches grid_pos_at).
    let (base, span) = if t < b0 && i > 0 {
        (grid.beats[i - 1].t_s, (b0 - grid.beats[i - 1].t_s).max(1e-3))
    } else {
        (b0, (b1 - b0).max(1e-3))
    };
    let phase = ((t - base) / span).clamp(0.0, 1.0);
    let tick = (phase * TICKS_PER_BEAT as f32).round() as u16;
    let quantized_s = base + tick as f32 / TICKS_PER_BEAT as f32 * span;
    // Musical slop: distance to the nearest 16th line — 4 per beat, since
    // one beat is a quarter note (tick resolution is ~1 ms — everything
    // snaps, so it can't measure rubato).
    let k = (phase * 4.0).round();
    let grid_residual_s = t - (base + k / 4.0 * span);
    QuantizedNote { grid: grid_pos, quantized_s, residual_s: t - quantized_s, grid_residual_s }
}

/// Subdivision vote from tick residues: are off-beat onsets duple (8ths,
/// tick ≈ 240) or triple (shuffle, tick ≈ 160/320)? Returns (2|3, conf).
pub fn subdivision_vote(quantized: &[QuantizedNote]) -> (u8, f32) {
    let mut duple = 0u32;
    let mut triple = 0u32;
    for q in quantized {
        let tick = (q.grid.tick % TICKS_PER_BEAT) as i32;
        if tick < 40 || (tick - 480).abs() < 40 {
            continue; // on-beat: no vote
        }
        let d_duple = (tick - 240).abs().min(tick.min(480 - tick));
        let d_trip = (tick - 160).abs().min((tick - 320).abs());
        if d_duple <= d_trip {
            duple += 1;
        } else {
            triple += 1;
        }
    }
    let total = duple + triple;
    if total < 4 {
        return (2, 0.0); // too few off-beats to call it
    }
    if triple * 2 > total {
        (3, triple as f32 / total as f32)
    } else {
        (2, duple as f32 / total as f32)
    }
}

/// Swing ratio from off-beat eighth residues: 0.5 = straight, ~0.67 =
/// triplet feel. None when fewer than 8 usable off-beats exist.
pub fn swing_ratio(quantized: &[QuantizedNote]) -> Option<f32> {
    let mut sum = 0.0;
    let mut n = 0u32;
    for q in quantized {
        let tick = (q.grid.tick % TICKS_PER_BEAT) as i32;
        // Off-beat eighths live in the middle third of the beat.
        if (120..360).contains(&tick) {
            sum += tick as f32 / TICKS_PER_BEAT as f32;
            n += 1;
        }
    }
    if n < 8 {
        return None;
    }
    Some((sum / n as f32).clamp(0.33, 0.75))
}

/// Rubato test: median |16th-residual| above ~35 ms means the performance
/// isn't playing the grid — widen windows / suspend grading (§5.2).
pub fn is_rubato(quantized: &[QuantizedNote]) -> bool {
    if quantized.len() < 8 {
        return false;
    }
    let mut res: Vec<f32> =
        quantized.iter().map(|q| q.grid_residual_s.abs()).collect();
    res.sort_by(|a, b| a.partial_cmp(b).unwrap());
    res[res.len() / 2] > 0.035
}

/// Assign the cumulative difficulty mask (§4.4). A note's max level sets it
/// and every level below (NLD subsets); ghosts (conf < τ_lo) stay out of
/// every level — ghosts stay ghosts.
pub fn assign_layers(
    notes: &mut [NoteEvent],
    grid: &BeatGrid,
    chords: &[ChordEvent],
    tau: f32,
) {
    let bounds: Vec<f32> = chords.iter().map(|c| c.t0).collect();
    for n in notes.iter_mut() {
        let top = max_layer(n, grid, &bounds, tau);
        n.layer_mask = match top {
            0 => L0_ANCHOR | L1_STAB | L2_SKELETON | L3_MELODY | L4_FULL,
            1 => L1_STAB | L2_SKELETON | L3_MELODY | L4_FULL,
            2 => L2_SKELETON | L3_MELODY | L4_FULL,
            3 => L3_MELODY | L4_FULL,
            4 => L4_FULL,
            _ => 0,
        };
    }
}

fn max_layer(n: &NoteEvent, grid: &BeatGrid, bounds: &[f32], tau: f32) -> u8 {
    if n.conf < TAU_LO {
        return 99; // ghost: no level
    }
    // L0: within 80 ms of a downbeat.
    let near_downbeat = grid
        .downbeats
        .iter()
        .filter_map(|&d| grid.beats.get(d as usize))
        .any(|b| (b.t_s - n.onset_s).abs() < 0.08);
    if near_downbeat {
        return 0;
    }
    // L1: within 120 ms of a chord change.
    if bounds.iter().any(|b| (b - n.onset_s).abs() < 0.12) {
        return 1;
    }
    // L2: on the eighth-note skeleton.
    let tick = (n.grid.tick % TICKS_PER_BEAT) as i32;
    let on_8th = tick < 50 || (tick - 240).abs() < 50 || tick > 430;
    if on_8th {
        return 2;
    }
    // L3: melody/riff above τ. L4: everything confident left.
    if n.conf >= tau {
        return 3;
    }
    4
}

/// Why a note sits outside every level (for the "why dimmed" explainer).
pub fn dim_reason(n: &NoteEvent) -> Option<&'static str> {
    if n.layer_mask != 0 {
        return None;
    }
    if n.conf < TAU_LO {
        return Some("low-confidence ghost: shown for context, never graded");
    }
    Some("below difficulty threshold")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::bar_of;
    use crate::map::{BeatPt, Provenance, StageStatus, TechFlags};

    fn grid_120() -> BeatGrid {
        let mut g = BeatGrid::empty(StageStatus::Degraded);
        for i in 0..32 {
            g.beats.push(BeatPt { t_s: i as f32 * 0.5, conf: 1.0 });
        }
        g.downbeats = vec![0, 4, 8, 12, 16, 20, 24, 28];
        g
    }

    fn note_at(t: f32, conf: f32) -> NoteEvent {
        NoteEvent {
            onset_s: t,
            offset_s: t + 0.4,
            raw_onset_s: t,
            grid: GridPos { bar: 0, beat: 0, tick: 0 },
            midi: 69.0,
            bend_cents: None,
            techniques: TechFlags(0),
            conf,
            source: Provenance::Mix,
            pos: None,
            sf_conf: None,
            layer_mask: 0,
            suspect: false,
        }
    }

    #[test]
    fn snap_roundtrip_and_residual() {
        let g = grid_120();
        let q = quantize_time(&g, 1.02); // 20 ms late on beat 2
        assert_eq!((q.grid.bar, q.grid.beat), (0, 2));
        // Snapped to the tick grid: sub-millisecond tick residual, but the
        // full 20 ms of musical slop is preserved in grid_residual_s.
        assert!(q.residual_s.abs() < 1e-3, "{}", q.residual_s);
        assert!((q.grid_residual_s - 0.02).abs() < 1e-3, "{}", q.grid_residual_s);
        assert!((q.quantized_s - 1.0198).abs() < 1e-3, "{}", q.quantized_s);
        // Eighth off-beat → tick ≈ 240.
        let q = quantize_time(&g, 1.25);
        assert!((q.grid.tick as i32 - 240).abs() < 5, "{}", q.grid.tick);
    }

    #[test]
    fn swing_and_subdivision_votes() {
        let g = grid_120();
        // Straight eighths.
        let qs: Vec<_> = (0..16).map(|i| quantize_time(&g, i as f32 * 0.25)).collect();
        let (sub, _) = subdivision_vote(&qs);
        assert_eq!(sub, 2);
        let swing = swing_ratio(&qs).unwrap();
        assert!((swing - 0.5).abs() < 0.05, "{swing}");
        // Shuffled: off-beats at 2/3 of the beat.
        let qs: Vec<_> = (0..16)
            .map(|i| {
                let t = i as f32 * 0.5 + if i % 2 == 1 { 0.5 * 0.667 } else { 0.0 };
                quantize_time(&g, t)
            })
            .collect();
        let (sub, conf) = subdivision_vote(&qs);
        assert_eq!(sub, 3, "conf {conf}");
        let swing = swing_ratio(&qs).unwrap();
        assert!((swing - 0.667).abs() < 0.06, "{swing}");
        // Metronomic quarter-notes: no rubato.
        let qs: Vec<_> = (0..16).map(|i| quantize_time(&g, i as f32 * 0.5)).collect();
        assert!(!is_rubato(&qs));
        // Sloppy ±50 ms timing: rubato.
        let qs: Vec<_> = (0..16)
            .map(|i| quantize_time(&g, i as f32 * 0.5 + if i % 2 == 0 { 0.05 } else { -0.05 }))
            .collect();
        assert!(is_rubato(&qs));
    }

    #[test]
    fn layers_are_cumulative_subsets() {
        let g = grid_120();
        let chords = vec![ChordEvent {
            t0: 4.5,
            t1: 8.0,
            grid0: GridPos { bar: 2, beat: 1, tick: 0 },
            grid1: GridPos { bar: 4, beat: 0, tick: 0 },
            root: Some(9),
            quality: crate::map::ChordQuality::Maj,
            bass: Some(9),
            conf: 0.8,
            alt: None,
        }];
        let mut notes = vec![
            note_at(0.0, 0.9),  // downbeat anchor → L0 (all bits)
            note_at(4.52, 0.9), // chord change, off the downbeat → L1+
            note_at(1.25, 0.9), // eighth skeleton → L2+
            note_at(1.1, 0.65), // confident melody → L3+
            note_at(1.37, 0.5), // weak but above ghost → L4 only
            note_at(2.0, 0.2),  // ghost → no level
        ];
        // Pipeline order: quantize first so ticks are real, then layers.
        for n in notes.iter_mut() {
            let q = quantize_time(&g, n.onset_s);
            n.grid = q.grid;
            n.onset_s = q.quantized_s;
        }
        assign_layers(&mut notes, &g, &chords, 0.6);
        assert_eq!(notes[0].layer_mask, 0b11111);
        assert_eq!(notes[1].layer_mask & L0_ANCHOR, 0);
        assert_ne!(notes[1].layer_mask & L1_STAB, 0);
        assert_ne!(notes[2].layer_mask & L2_SKELETON, 0);
        assert_ne!(notes[3].layer_mask & L3_MELODY, 0);
        assert_eq!(notes[4].layer_mask, L4_FULL);
        assert_eq!(notes[5].layer_mask, 0);
        assert!(dim_reason(&notes[5]).is_some());
        assert!(dim_reason(&notes[0]).is_none());
        // NLD subset property: chart(k) ⊆ chart(k+1) — a note playable at
        // level k is playable at every higher level (density only grows).
        for k in 0..4 {
            let bit = 1 << k;
            let next = 1 << (k + 1);
            for n in &notes {
                if n.layer_mask & bit != 0 {
                    assert!(n.layer_mask & next != 0, "NLD violated");
                }
            }
        }
    }

    #[test]
    fn bar_of_counts_downbeats() {
        let g = grid_120();
        assert_eq!(bar_of(&g, 5), (1, 1));
        assert_eq!(bar_of(&g, 0), (0, 0));
    }
}
