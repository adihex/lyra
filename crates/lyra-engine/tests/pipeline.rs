//! End-to-end smoke: real device output isn't assertable headless, but the
//! decode→DSP→ring→callback path is — Engine::new grabs the default output,
//! play a generated WAV, confirm position advances and state flips.

use lyra_engine::Engine;
use lyra_fs::LocalFile;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn wav(path: &std::path::Path, rate: u32, dur_s: f32) {
    let n = (rate as f32 * dur_s) as u32;
    let data_len = n * 4; // stereo 16-bit
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes()); // stereo
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..n {
        let s = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate as f32).sin();
        let v = (s * 16000.0) as i16;
        b.extend_from_slice(&v.to_le_bytes());
        b.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::File::create(path).unwrap().write_all(&b).unwrap();
}

#[test]
fn plays_and_reports_position() {
    let path = std::env::temp_dir().join("lyra_engine_smoke.wav");
    wav(&path, 48_000, 2.0);

    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing_subscriber::filter::LevelFilter::DEBUG)
        .try_init();
    let e = match Engine::new() {
        Ok(e) => e,
        Err(err) => { eprintln!("engine init failed: {err}"); return; }
    };
    let src: Arc<dyn lyra_fs::ByteSource> =
        lyra_fs::CachingSource::wrap(LocalFile::open(&path).unwrap());
    e.play(src, Some("wav"));

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut advanced = false;
    while Instant::now() < deadline {
        eprintln!("playing={} pos={:.2}", e.is_playing(), e.position_secs());
        if e.position_secs() > 0.5 {
            advanced = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(advanced, "position never advanced — pipeline stalled");

    e.pause();
    let t0 = Instant::now();
    while e.is_playing() && t0.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(1));
    }
    eprintln!("pause→playing=false took {:?}", t0.elapsed());
    assert!(!e.is_playing());
    assert!(e.can_resume());
    e.resume();
    let t0 = Instant::now();
    while !e.is_playing() && t0.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(e.is_playing(), "resume never took effect");
    e.stop();
    e.shutdown();
}
