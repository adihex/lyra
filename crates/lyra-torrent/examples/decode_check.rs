use lyra_fs::SourceMediaSource;
use std::sync::Arc;
fn main() {
    let eng = Arc::new(
        lyra_torrent::TorrentEngine::new(std::env::temp_dir().join("lyra-decodecheck")).unwrap(),
    );
    let spec = std::env::args().nth(1).unwrap();
    let idx: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2);
    let seek_secs: f64 = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let id = eng.add(&spec).expect("add failed");
    let files = eng.files(id).unwrap();
    let f = &files[idx];
    println!("decoding [{}] {} ({} bytes)", f.index, f.path, f.len);
    let src = eng.open_file(id, idx).expect("open_file failed");
    let media = SourceMediaSource::new(lyra_fs::CachingSource::wrap(src));
    let mut dec =
        lyra_formats::TrackDecoder::open(media, Some("flac")).expect("decoder open failed");
    println!("decoder open: {}Hz {}ch", dec.sample_rate, dec.channels);
    if seek_secs > 0.0 {
        dec.seek(seek_secs).expect("seek failed");
        println!("seeked to {seek_secs}s");
    }
    let mut total = 0usize;
    let mut peak = 0f32;
    let target = (dec.sample_rate as usize) * dec.channels * 12; // ~12s
    while total < target {
        match dec.next_block() {
            Ok(Some(pcm)) => {
                total += pcm.len();
                for &s in &pcm {
                    if s.abs() > peak {
                        peak = s.abs();
                    }
                }
            }
            Ok(None) => break,
            Err(e) => {
                println!("decode error after {total} samples: {e}");
                std::process::exit(1);
            }
        }
    }
    println!(
        "decoded {total} samples ({:.2}s), peak={peak:.4}",
        total as f32 / dec.channels as f32 / dec.sample_rate as f32
    );
    assert!(total > 0, "no samples decoded");
}
