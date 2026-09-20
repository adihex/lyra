//! HAL exclusive output: real device, hog mode + IOProc, real WAV decode.
//! macOS-only — plays a 440Hz sine through the speakers for ~2s.

#![cfg(target_os = "macos")]

use lyra_engine::{Engine, OutputMode};
use lyra_fs::LocalFile;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn wav(path: &std::path::Path, rate: u32, dur_s: f32) {
    let n = (rate as f32 * dur_s) as u32;
    let data_len = n * 4;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
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
fn hal_exclusive_plays() {
    let path = std::env::temp_dir().join("lyra_hal_smoke.wav");
    wav(&path, 44_100, 2.0);

    let e = match Engine::with_output(OutputMode::HalExclusive) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("HAL engine init failed (device busy?): {err}");
            return;
        }
    };
    let src: Arc<dyn lyra_fs::ByteSource> =
        lyra_fs::CachingSource::wrap(LocalFile::open(&path).unwrap());
    e.play(src, Some("wav"));

    let deadline = Instant::now() + Duration::from_secs(4);
    let mut advanced = false;
    while Instant::now() < deadline {
        if e.position_secs() > 0.5 {
            advanced = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(advanced, "HAL path: position never advanced");
    eprintln!(
        "HAL exclusive: pos={:.2}s, playing={}",
        e.position_secs(),
        e.is_playing()
    );
    e.stop();
    e.shutdown();
}
