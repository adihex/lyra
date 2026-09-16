//! TAB: constrained DP/Viterbi (string, fret) solver (§3.1).
//!
//! The classical formulation (Hori & Sagayama IOHMM-style, Radisavljevic &
//! Driessen path costs, Parncutt-family ergonomics): position is a hidden
//! state, transitions pay hand-geometry costs. Deterministic, testable,
//! license-free — a few hundred lines of Rust whose honest output is *"a
//! physically plausible way to play these notes,"* not the record's actual
//! fingering. Conditioned on the tuning+capo the tuning stage inferred.
//!
//! Polyphony note: simultaneous notes (within 40 ms) must sit on distinct
//! strings — enforced as an infinite transition cost. True chord-voicing
//! optimization (joint hand-shape states) is P1; P0 solves the sequence
//! with the distinct-string constraint, which covers lines and double-stops.

use crate::map::{Fretting, Tuning};

/// Solver weights — genre-tunable later; the defaults encode "stay in
/// position, move little, don't skip strings, prefer low/reachable
/// positions, open strings are fine but not free."
#[derive(Debug, Clone, Copy)]
pub struct TabOpts {
    pub max_fret: u8,
    pub shift_w: f32,
    pub open_penalty: f32,
    pub string_change: f32,
    pub string_skip: f32,
    pub continuity_bonus: f32,
    pub max_span: u8,
    /// Reach cost per fret — what stops a run climbing one string to the
    /// 12th fret instead of shifting position once. Washes out within
    /// genuinely high passages (all candidates pay it alike).
    pub position_w: f32,
}

impl Default for TabOpts {
    fn default() -> Self {
        Self {
            max_fret: 12,
            shift_w: 1.0,
            open_penalty: 0.4,
            string_change: 0.5,
            string_skip: 1.2,
            continuity_bonus: 0.8,
            max_span: 4,
            position_w: 0.3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TabSolution {
    /// One entry per input note; None = unplayable under the constraints
    /// (kept as pitch-only — notes never depend on the solver).
    pub frettings: Vec<Option<Fretting>>,
    /// Per-note solver margin: cost gap between the chosen assignment and
    /// its runner-up. `sf_conf = margin × note conf` downstream.
    pub margins: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pos {
    string: u8,
    fret: u8,
}

/// Candidate positions for one pitch under a tuning+capo: every string
/// whose (open + capo) sits ≤ pitch ≤ open + capo + max_fret.
fn candidates(midi: f32, tuning: &Tuning, opts: &TabOpts) -> Vec<Pos> {
    let target = midi.round() as i32;
    let mut out = Vec::new();
    for (s, open) in tuning.strings.iter().enumerate() {
        let fret = target - (*open as i32 + tuning.capo as i32);
        if (0..=opts.max_fret as i32).contains(&fret) {
            out.push(Pos {
                string: s as u8,
                fret: fret as u8,
            });
        }
    }
    // Prefer low positions for ties: sort by fret, then string.
    out.sort_by_key(|p| (p.fret, p.string));
    out
}

/// Transition cost between consecutive assignments. `overlap` marks notes
/// within 40 ms (must not share a string); `dt_weight` scales movement
/// cost by inter-onset gap (fast passages forgive shifts less).
fn transition(a: Pos, b: Pos, overlap: bool, opts: &TabOpts) -> f32 {
    if overlap && a.string == b.string {
        return f32::INFINITY; // one string, one note at a time
    }
    // Simultaneous notes must also fit the hand.
    if overlap && a.fret.max(b.fret) - a.fret.min(b.fret) > opts.max_span {
        return f32::INFINITY;
    }
    let mut c = opts.shift_w * (a.fret as f32 - b.fret as f32).abs();
    if a.string != b.string {
        c += opts.string_change;
        let skip = (a.string as i32 - b.string as i32).abs();
        if skip > 1 {
            c += opts.string_skip * (skip - 1) as f32;
        }
    } else if (a.fret as i32 - b.fret as i32).abs() <= 2 {
        c -= opts.continuity_bonus; // phrase continuity: stay in position
    }
    if b.fret == 0 {
        c += opts.open_penalty;
    }
    c += opts.position_w * b.fret as f32;
    c
}

/// Cost of opening a path at `p` (no predecessor): open-string bias +
/// reach cost, so barren starts prefer playable low positions too.
fn entry_cost(p: Pos, opts: &TabOpts) -> f32 {
    (if p.fret == 0 { opts.open_penalty } else { 0.0 }) + opts.position_w * p.fret as f32
}

/// Viterbi over the candidate lattice. `onsets` parallels `midis` and
/// drives the overlap constraint.
pub fn solve_tab(midis: &[f32], onsets: &[f32], tuning: &Tuning, opts: &TabOpts) -> TabSolution {
    let n = midis.len();
    if n == 0 || onsets.len() != n {
        return TabSolution {
            frettings: Vec::new(),
            margins: Vec::new(),
        };
    }
    let cands: Vec<Vec<Pos>> = midis.iter().map(|m| candidates(*m, tuning, opts)).collect();
    // Unplayable notes (outside every string's range) stay None; the DP
    // bridges over them with zero transition cost.
    let mut dp: Vec<Vec<f32>> = Vec::with_capacity(n);
    let mut bt: Vec<Vec<usize>> = Vec::with_capacity(n);
    for (i, c) in cands.iter().enumerate() {
        let mut row = vec![f32::INFINITY; c.len().max(1)];
        let mut brow = vec![0usize; c.len().max(1)];
        if c.is_empty() {
            row[0] = if i == 0 {
                0.0
            } else {
                dp[i - 1].iter().fold(f32::INFINITY, |a, &v| a.min(v))
            };
        } else if i == 0 || dp[i - 1].iter().all(|v| !v.is_finite()) {
            for (j, p) in c.iter().enumerate() {
                row[j] = entry_cost(*p, opts);
            }
        } else {
            let overlap = onsets[i] - onsets[i - 1] < 0.04;
            for (j, &b) in c.iter().enumerate() {
                let mut best = f32::INFINITY;
                let mut bj = 0;
                for (k, &a) in cands[i - 1].iter().enumerate() {
                    // Bridge over unplayable predecessors at no cost.
                    let prev = if cands[i - 1].is_empty() {
                        0.0
                    } else {
                        dp[i - 1][k]
                    };
                    let v = prev + transition(a, b, overlap, opts);
                    if v < best {
                        best = v;
                        bj = k;
                    }
                }
                // Predecessor unplayable → free entry.
                if cands[i - 1].is_empty() {
                    best = entry_cost(b, opts);
                }
                row[j] = best;
                brow[j] = bj.min(row.len().saturating_sub(1));
            }
        }
        dp.push(row);
        bt.push(brow);
    }
    // Backtrack the best path.
    let mut choice: Vec<Option<usize>> = vec![None; n];
    if !cands[n - 1].is_empty() {
        let (j, _) = dp[n - 1]
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        choice[n - 1] = Some(j);
        for i in (1..n).rev() {
            if let Some(j) = choice[i] {
                if !cands[i - 1].is_empty() {
                    choice[i - 1] = Some(bt[i][j].min(cands[i - 1].len() - 1));
                }
            }
        }
    } else if n > 1 {
        // Trailing unplayable notes: backtrack from the last playable row.
        let mut last = None;
        for i in (0..n).rev() {
            if !cands[i].is_empty() {
                last = Some(i);
                break;
            }
        }
        if let Some(l) = last {
            let (j, _) = dp[l]
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .unwrap();
            choice[l] = Some(j);
            for i in (1..=l).rev() {
                if let Some(j) = choice[i] {
                    if !cands[i - 1].is_empty() {
                        choice[i - 1] = Some(bt[i][j].min(cands[i - 1].len() - 1));
                    }
                }
            }
        }
    }

    let frettings: Vec<Option<Fretting>> = cands
        .iter()
        .zip(choice.iter())
        .map(|(c, ch)| {
            ch.and_then(|j| c.get(j)).map(|p| Fretting {
                string: p.string,
                fret: p.fret,
            })
        })
        .collect();
    // Margins: gap between the chosen state's path cost and the best path
    // forced through any other candidate at that note (local re-decode).
    let margins: Vec<f32> = (0..n)
        .map(|i| match (choice[i], cands[i].len()) {
            (Some(j), m) if m > 1 => {
                let chosen = dp[i][j];
                let runner = dp[i]
                    .iter()
                    .enumerate()
                    .filter(|(k, _)| *k != j)
                    .map(|(_, v)| *v)
                    .fold(f32::INFINITY, |a, v| a.min(v));
                if chosen.is_finite() && runner.is_finite() {
                    (runner - chosen).max(0.0)
                } else {
                    0.0
                }
            }
            _ => 0.0,
        })
        .collect();
    TabSolution { frettings, margins }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn std() -> Tuning {
        Tuning::default()
    }

    #[test]
    fn open_strings_solve_to_open_positions() {
        // E major open shape: E2 A2 E3 B3 E4-ish + G#.
        let midis = [40.0, 45.0, 52.0, 59.0, 64.0];
        let onsets = [0.0, 0.5, 1.0, 1.5, 2.0];
        let sol = solve_tab(&midis, &onsets, &std(), &TabOpts::default());
        assert!(sol.frettings.iter().all(|f| f.is_some()));
        // Low E must be string 0 fret 0 (only position for 40).
        assert_eq!(sol.frettings[0], Some(Fretting { string: 0, fret: 0 }));
        // A2 (45): open A string preferred over 5th-fret E.
        assert_eq!(sol.frettings[1], Some(Fretting { string: 1, fret: 0 }));
    }

    #[test]
    fn scale_run_stays_in_position() {
        // A minor pentatonic-ish run, 8th notes: solver should avoid
        // jumping between distant positions per note.
        let midis = [45.0, 48.0, 50.0, 52.0, 53.0, 55.0, 57.0];
        let onsets: Vec<f32> = (0..7).map(|i| i as f32 * 0.25).collect();
        let sol = solve_tab(&midis, &onsets, &std(), &TabOpts::default());
        assert!(sol.frettings.iter().all(|f| f.is_some()));
        let frets: Vec<u8> = sol.frettings.iter().map(|f| f.unwrap().fret).collect();
        let span = frets.iter().max().unwrap() - frets.iter().min().unwrap();
        assert!(span <= 5, "run should sit in one box: {frets:?}");
        assert!(sol.margins.iter().any(|m| *m > 0.0));
    }

    #[test]
    fn simultaneous_notes_split_strings() {
        // Double-stop: two notes 20 ms apart must not share a string.
        let midis = [64.0, 67.0]; // E4 G4
        let onsets = [1.0, 1.02];
        let sol = solve_tab(&midis, &onsets, &std(), &TabOpts::default());
        let (a, b) = (sol.frettings[0].unwrap(), sol.frettings[1].unwrap());
        assert_ne!(a.string, b.string, "same string: {a:?} {b:?}");
    }

    #[test]
    fn unplayable_note_stays_pitch_only() {
        // MIDI 20 is below every open string: None, but the DP bridges.
        let midis = [20.0, 40.0];
        let onsets = [0.0, 0.5];
        let sol = solve_tab(&midis, &onsets, &std(), &TabOpts::default());
        assert_eq!(sol.frettings[0], None);
        assert_eq!(sol.frettings[1], Some(Fretting { string: 0, fret: 0 }));
    }

    #[test]
    fn drop_d_low_d_uses_open_sixth() {
        let tuning = Tuning {
            strings: [38, 45, 50, 55, 59, 64],
            ..Tuning::default()
        };
        let sol = solve_tab(&[38.0], &[0.0], &tuning, &TabOpts::default());
        assert_eq!(sol.frettings[0], Some(Fretting { string: 0, fret: 0 }));
    }
}
