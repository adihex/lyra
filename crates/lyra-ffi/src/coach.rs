//! Coach FFI: one live Session behind a lock. The app feeds mono f32
//! blocks from an AVAudioEngine input tap (t_first = stream-clock secs of
//! sample 0); events are staged inside `push_samples`' borrowing drain, so
//! they collect into a queue the UI thread polls.

use std::ffi::{c_char, c_int, CStr, CString};
use std::sync::{Mutex, OnceLock};

use lyra_coach::{
    BeatGrid, CoachEvent, ExpectedEvent, FeedbackMode, Grade, NotePolicy, Session, SessionConfig,
};
use serde_json::{json, Value};

struct Live {
    session: Session,
    events: Vec<Value>,
}

// SAFETY: `Session` holds `Rc<RefCell>` DSP scratch (`!Send`), but every
// access — construction, push, drain, drop — happens under `LIVE`'s mutex.
// No `Rc` clone ever leaves the struct, so refcounts stay single-threaded.
unsafe impl Send for Live {}

static LIVE: OnceLock<Mutex<Option<Live>>> = OnceLock::new();

fn live() -> &'static Mutex<Option<Live>> {
    LIVE.get_or_init(|| Mutex::new(None))
}

fn ev_json(e: &CoachEvent) -> Value {
    match e {
        CoachEvent::Onset { t, strength } => {
            json!({"type": "onset", "t": t, "strength": strength})
        }
        CoachEvent::Verdict(j) => json!({
            "type": "verdict",
            "grade": match j.grade {
                Grade::Perfect => "perfect",
                Grade::Good => "good",
                Grade::Ok => "ok",
                Grade::Miss => "miss",
                Grade::OffGrid => "off_grid",
                Grade::Ghost => "ghost",
            },
            "expected_index": j.expected_index,
            "error_ms": j.error_s.map(|s| s * 1000.0),
            "pitch_target": j.pitch_target,
            "pitch_detected": j.pitch_detected,
            "pitch_conf": j.pitch_conf,
        }),
        CoachEvent::Position { matched, next_t } => {
            json!({"type": "position", "matched": matched, "next_t": next_t})
        }
        CoachEvent::CalibrationComplete { offset_s, mad } => {
            json!({"type": "calibration", "offset_ms": offset_s * 1000.0, "mad_ms": mad * 1000.0})
        }
    }
}

/// Start a session. `chart_json`: `[{t_secs, midi?, policy?}]` —
/// policy "graded"|"advisory"|"ghost" (default graded).
/// `config_json` optional: `{sample_rate, latency_offset_s, wait_for_me}`.
/// Replaces any live session. 0 ok.
///
/// # Safety
/// `chart_json` must be non-null and point to valid NUL-terminated JSON.
/// `config_json` may be null; otherwise it must point to valid NUL-terminated JSON.
#[no_mangle]
pub unsafe extern "C" fn lyra_coach_new(
    chart_json: *const c_char,
    config_json: *const c_char,
) -> c_int {
    if chart_json.is_null() {
        return 2;
    }
    let chart_s = match unsafe { CStr::from_ptr(chart_json) }.to_str() {
        Ok(s) => s,
        Err(_) => return 2,
    };
    let chart_v: Vec<Value> = match serde_json::from_str(chart_s) {
        Ok(v) => v,
        Err(_) => return 2,
    };
    let mut chart = Vec::with_capacity(chart_v.len());
    for e in &chart_v {
        let Some(t) = e
            .get("t_secs")
            .or_else(|| e.get("t"))
            .and_then(Value::as_f64)
        else {
            return 2;
        };
        let midi = e.get("midi").and_then(Value::as_f64).map(|m| m as f32);
        let policy = match e.get("policy").and_then(Value::as_str) {
            Some("advisory") => NotePolicy::Advisory,
            Some("ghost") => NotePolicy::Ghost,
            _ => NotePolicy::Graded,
        };
        chart.push(ExpectedEvent {
            t_secs: t,
            midi,
            policy,
        });
    }
    let mut config = SessionConfig::default();
    if !config_json.is_null() {
        if let Ok(s) = unsafe { CStr::from_ptr(config_json) }.to_str() {
            if let Ok(v) = serde_json::from_str::<Value>(s) {
                if let Some(r) = v.get("sample_rate").and_then(Value::as_u64) {
                    config.sample_rate = r as u32;
                }
                if let Some(o) = v.get("latency_offset_s").and_then(Value::as_f64) {
                    config.latency_offset_s = o;
                }
                if let Some(w) = v.get("wait_for_me").and_then(Value::as_bool) {
                    config.wait_for_me = w;
                }
            }
        }
    }
    *live().lock().unwrap() = Some(Live {
        session: Session::new(chart, config),
        events: Vec::new(),
    });
    0
}

/// Feed one mono f32 block; `t_first` = stream-clock seconds of sample 0.
/// Called from the audio tap thread — the lock is held for one hop's
/// worth of DSP, no allocation after init.
///
/// # Safety
/// `samples` must be null or point to `len` readable `f32` samples (null returns 2).
#[no_mangle]
pub unsafe extern "C" fn lyra_coach_push(samples: *const f32, len: usize, t_first: f64) -> c_int {
    if samples.is_null() || len == 0 {
        return 2;
    }
    let mut g = live().lock().unwrap();
    let Some(l) = g.as_mut() else { return 1 };
    let buf = unsafe { std::slice::from_raw_parts(samples, len) };
    for e in l.session.push_samples(buf, t_first) {
        l.events.push(ev_json(&e));
    }
    0
}

/// Drain staged events → JSON array (null when empty or no session).
#[no_mangle]
pub extern "C" fn lyra_coach_events() -> *mut c_char {
    let mut g = live().lock().unwrap();
    let Some(l) = g.as_mut() else {
        return std::ptr::null_mut();
    };
    if l.events.is_empty() {
        return std::ptr::null_mut();
    }
    let out = Value::Array(std::mem::take(&mut l.events));
    CString::new(out.to_string()).unwrap_or_default().into_raw()
}

/// Live score snapshot — null when no session.
#[no_mangle]
pub extern "C" fn lyra_coach_score() -> *mut c_char {
    let g = live().lock().unwrap();
    let Some(l) = g.as_ref() else {
        return std::ptr::null_mut();
    };
    let s = &l.session;
    let v = json!({
        "accuracy": s.accuracy(),
        "streak": s.streak(),
        "best_streak": s.best_streak(),
        "position": s.chart_position(),
        "latency_ms": s.latency_offset() * 1000.0,
        "calibrating": s.calibrating(),
        "calibration_hits": s.calibration_hits(),
    });
    CString::new(v.to_string()).unwrap_or_default().into_raw()
}

/// Begin input-latency calibration against a fixed click grid
/// ("strum on the click"). `bpm` at `t0` seconds. 0 ok, 1 no session.
#[no_mangle]
pub extern "C" fn lyra_coach_start_calibration(t0: f64, bpm: f64) -> c_int {
    let mut g = live().lock().unwrap();
    let Some(l) = g.as_mut() else { return 1 };
    l.session.start_calibration(BeatGrid::fixed(t0, bpm));
    0
}

/// Finish calibration — applies the measured offset. Returns
/// `{offset_ms, mad_ms}` JSON, or null if not calibrating / too few hits.
#[no_mangle]
pub extern "C" fn lyra_coach_complete_calibration() -> *mut c_char {
    let mut g = live().lock().unwrap();
    let Some(l) = g.as_mut() else {
        return std::ptr::null_mut();
    };
    match l.session.complete_calibration() {
        Some((offset, mad)) => {
            CString::new(json!({"offset_ms": offset * 1000.0, "mad_ms": mad * 1000.0}).to_string())
                .unwrap_or_default()
                .into_raw()
        }
        None => std::ptr::null_mut(),
    }
}

/// Skip the first `n` chart events (count-in). Call before audio flows.
#[no_mangle]
pub extern "C" fn lyra_coach_count_in(first_index: usize) -> c_int {
    let mut g = live().lock().unwrap();
    let Some(l) = g.as_mut() else { return 1 };
    l.session.set_count_in(first_index);
    0
}

/// Set feedback verbosity: "full" | "coarse" | "end_of_phrase" | "silent".
///
/// # Safety
/// `mode` may be null (returns 2); otherwise it must point to a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn lyra_coach_feedback(mode: *const c_char) -> c_int {
    if mode.is_null() {
        return 2;
    }
    let s = match unsafe { CStr::from_ptr(mode) }.to_str() {
        Ok(s) => s,
        Err(_) => return 2,
    };
    let m = match s {
        "coarse" => FeedbackMode::Coarse,
        "end_of_phrase" => FeedbackMode::EndOfPhrase,
        "silent" => FeedbackMode::Silent,
        _ => FeedbackMode::Full,
    };
    let mut g = live().lock().unwrap();
    let Some(l) = g.as_mut() else { return 1 };
    l.session.set_feedback_mode(m);
    0
}

/// Drop the live session.
#[no_mangle]
pub extern "C" fn lyra_coach_stop() {
    *live().lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use std::ffi::{CStr, CString};

    /// One test for the whole surface — `LIVE` is a process-global, so
    /// parallel cases would race the session.
    #[test]
    fn coach_lifecycle() {
        // Null guards before any session exists.
        assert_eq!(
            unsafe { super::lyra_coach_new(std::ptr::null(), std::ptr::null()) },
            2
        );
        assert_eq!(
            unsafe { super::lyra_coach_push(std::ptr::null(), 8, 0.0) },
            2
        );
        assert_eq!(unsafe { super::lyra_coach_feedback(std::ptr::null()) }, 2);
        assert!(super::lyra_coach_events().is_null());
        assert!(super::lyra_coach_score().is_null());
        assert_eq!(super::lyra_coach_count_in(4), 1);

        // Chart: one graded chug at t=0.5. Stream: silence, a 0.5-amp 440 Hz
        // burst starting one block after 0.5, silence again past the sweep
        // window — the gate opens at −30 dB and the judge grades the hit.
        let chart = CString::new(r#"[{"t_secs":0.5}]"#).unwrap();
        let cfg = CString::new(r#"{"sample_rate":48000}"#).unwrap();
        assert_eq!(
            unsafe { super::lyra_coach_new(chart.as_ptr(), cfg.as_ptr()) },
            0
        );
        assert_eq!(unsafe { super::lyra_coach_feedback(c"full".as_ptr()) }, 0);

        const SR: usize = 48_000;
        const N: usize = 1024;
        for i in 0..64usize {
            let t = (i * N) as f64 / SR as f64;
            let amp = if (24..40).contains(&i) { 0.5f32 } else { 0.0 };
            let buf: Vec<f32> = (0..N)
                .map(|k| {
                    ((t + k as f64 / SR as f64) * 440.0 * 2.0 * std::f64::consts::PI).sin() as f32
                        * amp
                })
                .collect();
            assert_eq!(unsafe { super::lyra_coach_push(buf.as_ptr(), N, t) }, 0);
        }

        let raw = super::lyra_coach_events();
        assert!(!raw.is_null(), "no events staged");
        let ev = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
        unsafe { super::super::lyra_string_free(raw) };
        assert!(ev.contains("\"onset\""), "no onset: {ev}");
        assert!(ev.contains("\"verdict\""), "no verdict: {ev}");

        let raw = super::lyra_coach_score();
        assert!(!raw.is_null());
        let s = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
        unsafe { super::super::lyra_string_free(raw) };
        for k in [
            "accuracy",
            "streak",
            "best_streak",
            "position",
            "latency_ms",
        ] {
            assert!(s.contains(k), "score missing {k}: {s}");
        }

        super::lyra_coach_stop();
        assert!(super::lyra_coach_events().is_null());
        assert!(super::lyra_coach_score().is_null());
    }
}
