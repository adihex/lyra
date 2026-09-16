//! TUNING + CAPO (§3.4): latent fretboard variables estimated jointly
//! from the note multiset — global cent offset, then a candidate-tuning
//! dictionary score, then capo inference.
//!
//! Pure algorithm on pitch lists: fully unit-testable on synthetic notes.
//! Two hypotheses can fit identically — the map carries alternatives and
//! the UI offers them instead of silently committing.

use crate::map::{Tuning, TuningCandidate};

/// Candidate fretboards, low-E → high-E open-string MIDI.
pub const TUNING_CANDIDATES: &[(&str, [i8; 6])] = &[
    ("standard", [40, 45, 50, 55, 59, 64]),
    ("half-step-down", [39, 44, 49, 54, 58, 63]),
    ("drop-d", [38, 45, 50, 55, 59, 64]),
    ("dadgad", [38, 45, 50, 55, 57, 62]),
    ("open-g", [38, 43, 50, 55, 59, 62]),
    ("open-d", [38, 45, 50, 54, 57, 62]),
];

const MAX_FRET: i32 = 15;

/// Estimate tuning from detected (continuous-MIDI) pitches.
/// Empty input → default standard tuning at conf 0 (honest unknown).
pub fn estimate_tuning(midis: &[f32]) -> Tuning {
    if midis.is_empty() {
        return Tuning::default();
    }
    let global_cents = global_offset_cents(midis);
    // Score every candidate at every capo 0..=5; keep the best two.
    let mut scored: Vec<(f32, usize, u8)> = Vec::new();
    for (ci, (_, strings)) in TUNING_CANDIDATES.iter().enumerate() {
        for capo in 0u8..=5 {
            scored.push((score_fretboard(midis, strings, capo), ci, capo));
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let (best_score, best_ci, best_capo) = scored[0];
    let (name, strings) = TUNING_CANDIDATES[best_ci];
    let conf = (best_score).clamp(0.0, 1.0);
    let mut alternatives: Vec<TuningCandidate> = scored
        .iter()
        .skip(1)
        // Distinct fretboards only — capo variants of the winner are the
        // same hypothesis family, not an alternative worth offering.
        .filter(|(_, ci, _)| *ci != best_ci)
        .take(2)
        .map(|(s, ci, capo)| {
            let (n, st) = TUNING_CANDIDATES[*ci];
            TuningCandidate {
                name: n.into(),
                strings: st,
                capo: *capo,
                conf: s.clamp(0.0, 1.0),
            }
        })
        .collect();
    // Capo is only meaningful when it buys open strings: if the winner
    // needs no capo to explain the notes, prefer capo 0 at equal score.
    let (strings, capo, name) = if best_capo > 0 {
        let plain = score_fretboard(midis, &strings, 0);
        if plain >= best_score - 0.02 {
            (strings, 0, name)
        } else {
            (strings, best_capo, name)
        }
    } else {
        (strings, 0, name)
    };
    let _ = name;
    let mut tuning = Tuning {
        strings,
        global_cents,
        capo,
        conf,
        alternatives: Vec::new(),
    };
    // Re-anchor alternatives' conf relative to the winner for display.
    for a in alternatives.iter_mut() {
        a.conf = (a.conf / conf.max(0.05)).clamp(0.0, 0.99);
    }
    tuning.alternatives = alternatives;
    tuning
}

/// Circular-mean deviation of stable pitches from 12-TET, in cents.
/// Many records sit ±20¢ (or a half-step down — handled by candidates).
fn global_offset_cents(midis: &[f32]) -> f32 {
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    for m in midis {
        let dev = ((m - m.round()) * 100.0).clamp(-50.0, 50.0);
        let a = dev * std::f32::consts::PI / 50.0; // ±50¢ → ±π
        sx += a.cos();
        sy += a.sin();
    }
    (sy.atan2(sx)) * 50.0 / std::f32::consts::PI
}

/// Fretboard score: open-string pitch-class coverage minus fret-span cost.
/// A note is "explained" if some string frets it within 0..=MAX_FRET of the
/// (capo-shifted) open string; open-string hits score double.
fn score_fretboard(midis: &[f32], strings: &[i8; 6], capo: u8) -> f32 {
    let mut hit = 0u32;
    let mut open = 0u32;
    let mut span_cost = 0.0f32;
    for m in midis {
        let target = m.round() as i32;
        let mut best: Option<i32> = None;
        for s in strings {
            let fret = target - (*s as i32 + capo as i32);
            if (0..=MAX_FRET).contains(&fret) && best.map_or(true, |b| fret < b) {
                best = Some(fret);
            }
        }
        match best {
            Some(0) => {
                hit += 1;
                open += 1;
            }
            Some(f) => {
                hit += 1;
                span_cost += f as f32 / MAX_FRET as f32;
            }
            None => {}
        }
    }
    if midis.is_empty() {
        return 0.0;
    }
    let coverage = hit as f32 / midis.len() as f32;
    let openness = open as f32 / midis.len() as f32;
    let span = span_cost / midis.len() as f32;
    coverage * 0.6 + openness * 0.5 - span * 0.3 - capo as f32 * 0.02
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_chords_detect_standard() {
        // Dm7–G7–Cmaj7–A7 (ii–V–I–VI with chromatic extensions): eight
        // distinct pitch classes across two keys — no single open tuning
        // covers them, so standard wins on span + openness. Single-key
        // fixtures are genuinely ambiguous (open tunings are built for
        // I–IV–V in their key); multi-key content is the honest test.
        let midis = vec![
            50.0, 53.0, 57.0, 60.0, // Dm7
            43.0, 47.0, 50.0, 53.0, // G7
            48.0, 52.0, 55.0, 59.0, // Cmaj7
            45.0, 61.0, 64.0, 67.0, // A7 (A2 C#4 E4 G4)
        ];
        let t = estimate_tuning(&midis);
        assert_eq!(t.strings, [40, 45, 50, 55, 59, 64], "got capo {}", t.capo);
        assert_eq!(t.capo, 0);
        assert!(t.conf > 0.5, "{}", t.conf);
        // The near-twin (drop-D = standard minus one string) must survive
        // as an offered alternative — underdetermination made visible.
        assert!(!t.alternatives.is_empty());
    }

    #[test]
    fn low_d_riff_detects_drop_d() {
        // D2 pedal + B3/E4 open content: drop-D voices all three open;
        // DADGAD frets B and E; standard can't play D2 at all.
        let midis = vec![
            38.0, 38.0, 59.0, 64.0, 59.0, 64.0, 38.0, 55.0, 57.0, 59.0, 62.0, 64.0,
        ];
        let t = estimate_tuning(&midis);
        assert_eq!(
            t.strings,
            [38, 45, 50, 55, 59, 64],
            "alternatives: {:?}",
            t.alternatives
        );
        assert!(!t.alternatives.is_empty());
    }

    #[test]
    fn transposed_open_shape_infers_capo() {
        // G-shape (open strings ring) transposed +2 semitones = capo 2 shape.
        let g_shape = vec![40.0, 45.0, 50.0, 55.0, 59.0, 64.0, 43.0, 47.0, 50.0];
        let transposed: Vec<f32> = g_shape.iter().map(|m| m + 2.0).collect();
        let t = estimate_tuning(&transposed);
        assert_eq!(t.capo, 2, "strings {:?} capo {}", t.strings, t.capo);
    }

    #[test]
    fn global_offset_measures_detune() {
        let sharp: Vec<f32> = vec![40.2, 45.2, 50.2, 55.2];
        let t = estimate_tuning(&sharp);
        assert!((t.global_cents - 20.0).abs() < 3.0, "{}", t.global_cents);
    }

    #[test]
    fn empty_is_honest_unknown() {
        let t = estimate_tuning(&[]);
        assert_eq!(t.conf, 0.0);
        assert_eq!(t.strings, [40, 45, 50, 55, 59, 64]);
    }
}
