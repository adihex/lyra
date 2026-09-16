//! SECTIONS: Foote checkerboard novelty on a chroma self-similarity
//! matrix + repetition grouping → labeled segments snapped to bars.
//!
//! Pure Rust on the chroma the chord stage already computes (validated in
//! the Python lane against msaf). Boundaries snap to the nearest bar; labels
//! are repetition-ranked (most-repeated → Chorus) with short edge segments
//! called Intro/Outro. No timbre feature exists at P0, so Solo is never
//! emitted — a wrong label is worse than a coarse one.

use crate::grid::grid_pos_at;
use crate::map::{BeatGrid, Section, SectionLabel, StageStatus};

/// Labeled-section pass. Needs a usable grid (beat-sync chroma); without
/// beats this stage honestly reports Failed instead of guessing on time.
pub struct SectionTrack {
    pub sections: Vec<Section>,
    pub status: StageStatus,
    pub conf: f32,
}

pub fn segment_sections(chroma: &[[f32; 12]], hop_s: f32, grid: &BeatGrid) -> SectionTrack {
    if grid.beats.len() < 8 || chroma.is_empty() {
        return SectionTrack {
            sections: Vec::new(),
            status: StageStatus::Failed,
            conf: 0.0,
        };
    }
    // 1. Beat-synchronous chroma: mean-pool frames into beat intervals.
    let beat_chroma = beat_sync(chroma, hop_s, grid);
    if beat_chroma.len() < 8 {
        return SectionTrack {
            sections: Vec::new(),
            status: StageStatus::Failed,
            conf: 0.0,
        };
    }
    // 2–3. Self-similarity + checkerboard novelty.
    let ssm = self_similarity(&beat_chroma);
    let kernel = (beat_chroma.len() / 8).clamp(4, 16);
    let novelty = foote_novelty(&ssm, kernel);
    // 4. Peak-pick boundaries (beat indices), min 8 beats apart.
    let mut bounds = vec![0usize];
    bounds.extend(pick_peaks(&novelty, 8));
    bounds.push(beat_chroma.len());
    bounds.dedup();
    // 5. Snap to bars: nearest downbeat at or before each bound.
    let snapped: Vec<usize> = bounds
        .iter()
        .map(|&b| snap_to_bar(grid, b))
        .collect::<Vec<_>>();
    let mut snapped = snapped;
    snapped.dedup();
    if snapped.len() < 2 {
        snapped = vec![0, beat_chroma.len()];
    }
    // 6. Repetition grouping → labels.
    let sections = label_segments(&beat_chroma, &snapped, grid);
    let conf = if sections.is_empty() {
        0.0
    } else {
        sections.iter().map(|s| s.conf).sum::<f32>() / sections.len() as f32
    };
    SectionTrack {
        sections,
        status: StageStatus::Ok,
        conf,
    }
}

fn beat_sync(chroma: &[[f32; 12]], hop_s: f32, grid: &BeatGrid) -> Vec<[f32; 12]> {
    let mut out = Vec::with_capacity(grid.beats.len());
    for (i, b) in grid.beats.iter().enumerate() {
        let t0 = b.t_s;
        let t1 = grid.beats.get(i + 1).map(|n| n.t_s).unwrap_or(t0 + 0.5);
        let f0 = (t0 / hop_s) as usize;
        let f1 = ((t1 / hop_s) as usize).max(f0 + 1).min(chroma.len());
        if f0 >= chroma.len() {
            break;
        }
        let mut acc = [0f32; 12];
        for f in &chroma[f0..f1] {
            for pc in 0..12 {
                acc[pc] += f[pc];
            }
        }
        let n = (f1 - f0) as f32;
        for v in acc.iter_mut() {
            *v /= n;
        }
        out.push(acc);
    }
    out
}

fn cosine(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na < 1e-9 || nb < 1e-9 {
        return 0.0;
    }
    dot / (na * nb)
}

fn self_similarity(beat_chroma: &[[f32; 12]]) -> Vec<Vec<f32>> {
    let n = beat_chroma.len();
    let mut ssm = vec![vec![0.0; n]; n];
    for i in 0..n {
        ssm[i][i] = 1.0;
        for j in (i + 1)..n {
            let v = cosine(&beat_chroma[i], &beat_chroma[j]);
            ssm[i][j] = v;
            ssm[j][i] = v;
        }
    }
    ssm
}

/// Foote novelty: checkerboard kernel (±K blocks) correlated along the SSM
/// diagonal — high where the past-K and next-K windows disagree. Only
/// full-context indices (K..N-K) are scored; the edges carry no contrast
/// information and would otherwise dominate the peak threshold.
fn foote_novelty(ssm: &[Vec<f32>], k: usize) -> Vec<f32> {
    let n = ssm.len();
    let mut nov = vec![0.0; n];
    if n < 2 * k + 1 {
        return nov;
    }
    for i in k..n - k {
        let mut v = 0.0;
        for x in i - k..i {
            for y in i..i + k {
                v += ssm[x][y];
            }
            for y in i - k..i {
                v -= ssm[x][y];
            }
        }
        for x in i..i + k {
            for y in i..i + k {
                v -= ssm[x][y];
            }
        }
        nov[i] = -v / (k * k) as f32;
    }
    nov
}

fn pick_peaks(novelty: &[f32], min_gap: usize) -> Vec<usize> {
    if novelty.len() < 3 {
        return Vec::new();
    }
    let mean = novelty.iter().sum::<f32>() / novelty.len() as f32;
    let std =
        (novelty.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / novelty.len() as f32).sqrt();
    let thresh = mean + 0.5 * std;
    let mut peaks = Vec::new();
    let mut last = 0usize.wrapping_sub(min_gap);
    for i in 1..novelty.len() - 1 {
        if novelty[i] > thresh
            && novelty[i] >= novelty[i - 1]
            && novelty[i] >= novelty[i + 1]
            && i.wrapping_sub(last) >= min_gap
        {
            peaks.push(i);
            last = i;
        }
    }
    peaks
}

/// Nearest downbeat beat-index at or before `b` (bar-snapped boundaries).
/// The track end (`b == beats.len()`) passes through as the end sentinel
/// so the last section keeps full coverage.
fn snap_to_bar(grid: &BeatGrid, b: usize) -> usize {
    if b >= grid.beats.len() {
        return grid.beats.len();
    }
    let mut best = 0usize;
    for &db in &grid.downbeats {
        if (db as usize) <= b {
            best = db as usize;
        } else {
            break;
        }
    }
    best.min(grid.beats.len().saturating_sub(1))
}

fn label_segments(beat_chroma: &[[f32; 12]], bounds: &[usize], grid: &BeatGrid) -> Vec<Section> {
    // Mean chroma per segment → greedy repetition clusters (cos > 0.85).
    let mut centroids: Vec<[f32; 12]> = Vec::new();
    let mut seg_cluster: Vec<usize> = Vec::new();
    let mut seg_mean: Vec<[f32; 12]> = Vec::new();
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mut m = [0f32; 12];
        for beat in &beat_chroma[a..b.min(beat_chroma.len())] {
            for pc in 0..12 {
                m[pc] += beat[pc];
            }
        }
        let n = (b - a).max(1) as f32;
        for v in m.iter_mut() {
            *v /= n;
        }
        let c = centroids
            .iter()
            .position(|cen| cosine(&m, cen) > 0.85)
            .unwrap_or_else(|| {
                centroids.push(m);
                centroids.len() - 1
            });
        seg_cluster.push(c);
        seg_mean.push(m);
    }
    // Rank clusters by total beat count: most-repeated → Chorus.
    let mut mass = vec![0usize; centroids.len()];
    for (i, &c) in seg_cluster.iter().enumerate() {
        mass[c] += bounds[i + 1] - bounds[i];
    }
    let mut rank: Vec<usize> = (0..mass.len()).collect();
    rank.sort_by(|a, b| mass[*b].cmp(&mass[*a]));
    let rank_of = |c: usize| rank.iter().position(|&r| r == c).unwrap_or(99);

    let n_seg = seg_cluster.len();
    bounds
        .windows(2)
        .enumerate()
        .map(|(i, w)| {
            let (a, b) = (w[0], w[1]);
            let t0 = grid.beats[a].t_s;
            let t1 = grid
                .beats
                .get(b)
                .map(|x| x.t_s)
                .unwrap_or_else(|| grid.beats.last().map(|x| x.t_s + 0.5).unwrap_or(0.0));
            let dur = t1 - t0;
            let label = if i == 0 && dur < 15.0 {
                SectionLabel::Intro
            } else if i + 1 == n_seg && dur < 20.0 && n_seg > 1 {
                SectionLabel::Outro
            } else {
                match rank_of(seg_cluster[i]) {
                    0 => SectionLabel::Chorus,
                    1 => SectionLabel::Verse,
                    _ => SectionLabel::Bridge,
                }
            };
            // Cohesion = mean cosine to the cluster centroid.
            let cen = centroids[seg_cluster[i]];
            let mut coh = 0.0;
            let mut cnt = 0;
            for beat in &beat_chroma[a..b.min(beat_chroma.len())] {
                coh += cosine(beat, &cen);
                cnt += 1;
            }
            Section {
                t0,
                t1,
                grid0: grid_pos_at(grid, t0),
                grid1: grid_pos_at(grid, t1),
                label,
                conf: if cnt == 0 {
                    0.3
                } else {
                    (coh / cnt as f32).clamp(0.0, 1.0)
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{BeatPt, StageStatus};

    /// 64 beats @0.5 s; chroma texture A for beats 0-16+32-48, B elsewhere.
    fn abab_fixture() -> (Vec<[f32; 12]>, f32, BeatGrid) {
        let mut chroma = Vec::new();
        for i in 0..320 {
            // 5 frames/beat at hop 0.1 s.
            let beat = i / 5;
            let in_a = (0..16).contains(&beat) || (32..48).contains(&beat);
            let mut c = [0.02f32; 12];
            if in_a {
                for pc in [0, 4, 7] {
                    c[pc] = 1.0;
                }
            } else {
                for pc in [5, 9, 2] {
                    c[pc] = 1.0;
                }
            }
            chroma.push(c);
        }
        let mut grid = BeatGrid::empty(StageStatus::Degraded);
        for i in 0..64 {
            grid.beats.push(BeatPt {
                t_s: i as f32 * 0.5,
                conf: 1.0,
            });
        }
        grid.downbeats = (0..64u32).step_by(4).collect();
        (chroma, 0.1, grid)
    }

    #[test]
    fn finds_repeating_sections() {
        let (chroma, hop, grid) = abab_fixture();
        let track = segment_sections(&chroma, hop, &grid);
        assert_eq!(track.status, StageStatus::Ok);
        assert!(track.sections.len() >= 3, "got {:?}", track.sections.len());
        // A-texture (32 beats) outranks B (32 beats tie → first-seen wins
        // rank 0); either way both labels appear.
        let labels: Vec<SectionLabel> = track.sections.iter().map(|s| s.label).collect();
        assert!(
            labels.contains(&SectionLabel::Chorus) && labels.contains(&SectionLabel::Verse),
            "labels: {labels:?}"
        );
        // Boundaries snapped to bars.
        for s in &track.sections {
            assert_eq!(s.grid0.beat, 0, "section at {} not bar-snapped", s.t0);
        }
        // Coverage: first starts at 0, last ends at the final beat.
        assert_eq!(track.sections[0].t0, 0.0);
        assert!((track.sections.last().unwrap().t1 - 32.0).abs() < 0.01);
    }

    #[test]
    fn no_grid_no_sections() {
        let (chroma, hop, _) = abab_fixture();
        let grid = BeatGrid::empty(StageStatus::Failed);
        let track = segment_sections(&chroma, hop, &grid);
        assert_eq!(track.status, StageStatus::Failed);
        assert!(track.sections.is_empty());
    }
}
