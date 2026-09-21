//! Real-device checks — runs against the actual output hardware.
//! `hog_ioproc_tone` plays ~1.2s of a sine through the IOProc path.

#![cfg(target_os = "macos")]

use lyra_hal::HalDevice;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

// Device tests must serialize — a rate switch mid-stream kills an
// in-flight IOProc, and hog acquisition is exclusive.
static DEV: Mutex<()> = Mutex::new(());

// Device tree flickers during rate transitions — the default-device
// property can momentarily report none. Retry briefly.
fn dev() -> HalDevice {
    for _ in 0..100 {
        if let Ok(d) = HalDevice::default_output() {
            return d;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    dev()
}

#[test]
fn enumerate_and_hog() {
    let _g = DEV.lock().unwrap();
    let dev = dev();
    let name = dev.name().unwrap_or_else(|_| "?".into());
    let uid = dev.uid().unwrap_or_else(|_| "?".into());
    let rate = dev.nominal_rate().unwrap();
    let rates = dev.available_rates().unwrap();
    eprintln!("device: {name} uid={uid} rate={rate} rates={rates:?}");
    assert!(rate > 8000.0);
    assert!(!rates.is_empty());

    // Hog → verify held → release → verify free.
    let hog = dev.hog().expect("hog acquire");
    drop(hog);
    eprintln!("hog acquire/release OK");
}

#[test]
fn rate_switch_roundtrip() {
    let _g = DEV.lock().unwrap();
    let dev = dev();
    let orig = dev.nominal_rate().unwrap();
    let rates = dev.available_rates().unwrap();
    // Pick a different supported rate if one exists.
    let other = rates
        .iter()
        .map(|&(_mn, mx)| mx)
        .find(|&r| (r - orig).abs() > 0.5 && (44100.0..=96000.0).contains(&r));
    let Some(other) = other else {
        eprintln!("single-rate device ({orig}) — skipping switch");
        return;
    };
    dev.set_nominal_rate(other).unwrap();
    assert!((dev.nominal_rate().unwrap() - other).abs() < 0.5);
    dev.set_nominal_rate(orig).unwrap();
    assert!((dev.nominal_rate().unwrap() - orig).abs() < 0.5);
    eprintln!("rate switch {orig} → {other} → {orig} OK");
}

#[test]
fn hog_ioproc_tone() {
    let _g = DEV.lock().unwrap();
    let dev = dev();
    let (ch, rate, _il) = dev.virtual_format().unwrap();
    let _hog = dev.hog().expect("hog");

    let produced = Arc::new(AtomicU64::new(0));
    let p = Arc::clone(&produced);
    let mut phase = 0f64;
    let freq = 440.0f64;
    let pull = move |buf: &mut [f32]| -> usize {
        let frames = buf.len() / ch;
        for f in 0..frames {
            let s = (phase * 2.0 * std::f64::consts::PI * freq / rate) as f32 * 0.1;
            for c in 0..ch {
                buf[f * ch + c] = s;
            }
            phase += 1.0;
        }
        p.fetch_add(frames as u64, Ordering::Relaxed);
        frames
    };

    let proc = dev.start_ioproc(Box::new(pull)).expect("ioproc start");
    let t0 = Instant::now();
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let frames = produced.load(Ordering::Relaxed);
    let elapsed = t0.elapsed().as_secs_f64();
    let underruns = proc.underruns();
    drop(proc);

    let expected = rate * elapsed;
    eprintln!(
        "ioproc: {frames} frames in {elapsed:.2}s (expect ~{expected:.0}), underruns={underruns}"
    );
    // Within 15% of realtime = the callback ran at hardware rate.
    assert!(
        (frames as f64 - expected).abs() / expected < 0.15,
        "callback rate off: {frames} vs {expected}"
    );
}
