//! Thin safe wrappers over the `lyra-ffi` C ABI — the same functions the
//! SwiftUI app calls, invoked from Rust over the rlib. CString/malloc
//! discipline lives here so the panes stay in safe Rust.

// The wrapper layer mirrors the full FFI surface the app uses; panes only
// exercise the subset they need today.
#![allow(dead_code)]

use lyra_ffi::host::{cstr, take_string};
use serde_json::Value;
use std::path::Path;

fn cs(s: &str) -> std::ffi::CString {
    std::ffi::CString::new(s).unwrap_or_default()
}

fn json_call(p: *mut std::ffi::c_char) -> Value {
    match unsafe { take_string(p) } {
        Some(s) => serde_json::from_str(&s).unwrap_or(Value::Null),
        None => Value::Null,
    }
}

// ── engine ──────────────────────────────────────────────────────────────

pub fn engine() -> *mut lyra_engine::Engine {
    lyra_ffi::lyra_engine_current()
}

pub fn play_file(e: *mut lyra_engine::Engine, path: &str) -> bool {
    !e.is_null() && unsafe { lyra_ffi::lyra_engine_play_file(e, cs(path).as_ptr()) } == 0
}

pub fn play_remote(e: *mut lyra_engine::Engine, profile: &Value, remote_path: &str) -> bool {
    !e.is_null()
        && unsafe {
            lyra_ffi::remote_fs::lyra_engine_play_remote(
                e,
                cs(&profile.to_string()).as_ptr(),
                cs(remote_path).as_ptr(),
            )
        } == 0
}

pub fn play_torrent(e: *mut lyra_engine::Engine, id: i32, file_idx: i32) -> bool {
    !e.is_null() && unsafe { lyra_ffi::lyra_engine_play_torrent(e, id, file_idx) } == 0
}

pub fn pause(e: *mut lyra_engine::Engine) {
    unsafe { lyra_ffi::lyra_engine_pause(e) }
}
pub fn resume(e: *mut lyra_engine::Engine) {
    unsafe { lyra_ffi::lyra_engine_resume(e) }
}
pub fn stop(e: *mut lyra_engine::Engine) {
    unsafe { lyra_ffi::lyra_engine_stop(e) }
}
pub fn seek(e: *mut lyra_engine::Engine, secs: f64) {
    unsafe { lyra_ffi::lyra_engine_seek(e, secs) }
}
pub fn set_volume(e: *mut lyra_engine::Engine, v: f32) {
    unsafe { lyra_ffi::lyra_engine_set_volume(e, v) }
}
pub fn position(e: *mut lyra_engine::Engine) -> f64 {
    if e.is_null() {
        0.0
    } else {
        unsafe { lyra_ffi::lyra_engine_position(e) }
    }
}
pub fn is_playing(e: *mut lyra_engine::Engine) -> bool {
    !e.is_null() && unsafe { lyra_ffi::lyra_engine_is_playing(e) } != 0
}
pub fn can_resume(e: *mut lyra_engine::Engine) -> bool {
    !e.is_null() && unsafe { lyra_ffi::lyra_engine_can_resume(e) } != 0
}
pub fn output_mode() -> i32 {
    lyra_ffi::lyra_engine_output_mode()
}
pub fn set_output_mode(mode: i32) -> bool {
    lyra_ffi::lyra_engine_set_output_mode(mode) == 0
}
pub fn set_band(e: *mut lyra_engine::Engine, band: i32, freq: f32, q: f32, gain: f32, peak: bool) {
    if !e.is_null() {
        unsafe { lyra_ffi::lyra_engine_set_band(e, band, freq, q, gain, i32::from(peak)) }
    }
}

/// Latest viz frame; seq==0/unset fields when no engine.
pub fn viz_frame(e: *mut lyra_engine::Engine) -> lyra_engine::VizFrame {
    let mut f = lyra_engine::VizFrame::default();
    if !e.is_null() {
        unsafe {
            lyra_ffi::lyra_engine_viz_frame(e, &mut f);
        }
    }
    f
}

/// EQ magnitude response {freqs:[…], db:[…]} — same curve the app draws.
pub fn eq_response(e: *mut lyra_engine::Engine) -> Value {
    if e.is_null() {
        return Value::Null;
    }
    json_call(unsafe { lyra_ffi::lyra_engine_eq_response(e) })
}

// ── library ─────────────────────────────────────────────────────────────

/// Open a throwaway handle for a blocking call on a worker thread —
/// rusqlite connections aren't Send, so each worker opens its own.
pub struct LibHandle(*mut lyra_store::Library);
unsafe impl Send for LibHandle {}

impl LibHandle {
    pub fn open(db: &Path) -> Option<Self> {
        let p = unsafe { lyra_ffi::lyra_lib_open(cstr(db).as_ptr()) };
        (!p.is_null()).then_some(Self(p))
    }
    pub fn sync_dir(&self, dir: &Path) -> Value {
        json_call(unsafe { lyra_ffi::lyra_lib_sync_dir(self.0, cstr(dir).as_ptr()) })
    }
    pub fn tracks(&self) -> Value {
        json_call(unsafe { lyra_ffi::lyra_lib_tracks(self.0) })
    }
}

impl Drop for LibHandle {
    fn drop(&mut self) {
        unsafe { lyra_ffi::lyra_lib_free(self.0) }
    }
}

/// Stateless probe — used by the torrent duration prober.
pub fn probe(path: &Path) -> Value {
    json_call(unsafe { lyra_ffi::lyra_probe(cstr(path).as_ptr()) })
}

// ── artwork (blocking network — call from worker threads) ───────────────

pub fn art_fetch(db: &Path, path: &str) -> Value {
    LibHandle::open(db)
        .map(|l| json_call(unsafe { lyra_ffi::art::lyra_art_fetch(l.0, cs(path).as_ptr()) }))
        .unwrap_or(Value::Null)
}

// ── torrents ────────────────────────────────────────────────────────────

pub fn torrent_add(spec: &str) -> i32 {
    unsafe { lyra_ffi::lyra_torrent_add(cs(spec).as_ptr()) }
}
pub fn torrent_files(id: i32) -> Value {
    json_call(lyra_ffi::lyra_torrent_files(id))
}
pub fn torrent_stats(id: i32) -> Value {
    json_call(lyra_ffi::lyra_torrent_stats(id))
}
pub fn torrent_remove(id: i32, delete_files: bool) -> bool {
    lyra_ffi::lyra_torrent_remove(id, i32::from(delete_files)) == 0
}
pub fn torrent_list() -> Value {
    json_call(lyra_ffi::lyra_torrent_list())
}
pub fn torrent_orphans() -> Value {
    json_call(lyra_ffi::lyra_torrent_orphans())
}
pub fn torrent_purge_orphans() -> Value {
    json_call(lyra_ffi::lyra_torrent_purge_orphans())
}
pub fn torrent_probe(id: i32, file_idx: i32) -> Value {
    json_call(lyra_ffi::lyra_torrent_probe(id, file_idx))
}

// ── discover (lyra-search; blocking — worker threads) ───────────────────

pub struct SearchHandle(*mut lyra_ffi::LyraSearch);
unsafe impl Send for SearchHandle {}

impl SearchHandle {
    /// `data_dir` backs provider caches; null when init fails.
    pub fn open(data_dir: &Path) -> Option<Self> {
        let p = unsafe { lyra_ffi::lyra_search_new(cstr(data_dir).as_ptr()) };
        (!p.is_null()).then_some(Self(p))
    }
    pub fn search(&self, query_json: &str) -> Value {
        json_call(unsafe { lyra_ffi::lyra_search(self.0, cs(query_json).as_ptr()) })
    }
    pub fn resolve(&self, result_json: &str) -> Value {
        json_call(unsafe { lyra_ffi::lyra_search_resolve(self.0, cs(result_json).as_ptr()) })
    }
}

impl Drop for SearchHandle {
    fn drop(&mut self) {
        unsafe { lyra_ffi::lyra_search_free(self.0) }
    }
}

// ── remote library (SFTP) ───────────────────────────────────────────────

pub fn remlib_test(profile: &Value) -> Value {
    json_call(unsafe { lyra_ffi::remote_fs::lyra_remlib_test(cs(&profile.to_string()).as_ptr()) })
}
pub fn remlib_scan(db: &Path, profile: &Value) -> Value {
    LibHandle::open(db)
        .map(|l| {
            json_call(unsafe {
                lyra_ffi::remote_fs::lyra_remlib_scan(l.0, cs(&profile.to_string()).as_ptr())
            })
        })
        .unwrap_or(Value::Null)
}

// ── LAN remote ──────────────────────────────────────────────────────────

pub fn remote_init(e: *mut lyra_engine::Engine, key_path: &Path) -> bool {
    !e.is_null() && unsafe { lyra_ffi::lyra_remote_init(e, cstr(key_path).as_ptr()) } == 0
}
pub fn remote_start(port: u16) -> bool {
    lyra_ffi::lyra_remote_start(port) == 0
}
pub fn remote_open_pairing() -> Value {
    json_call(lyra_ffi::lyra_remote_open_pairing())
}
pub fn remote_paired_count() -> i32 {
    lyra_ffi::lyra_remote_paired_count()
}
pub fn remote_devices() -> Value {
    json_call(lyra_ffi::lyra_remote_devices())
}
pub fn remote_revoke(id: &str) -> i32 {
    unsafe { lyra_ffi::lyra_remote_revoke(cs(id).as_ptr()) }
}

// ── coach ───────────────────────────────────────────────────────────────

pub fn coach_new(chart_json: &str, config_json: &str) -> bool {
    (unsafe { lyra_ffi::coach::lyra_coach_new(cs(chart_json).as_ptr(), cs(config_json).as_ptr()) })
        == 0
}
pub fn coach_push(samples: &[f32], t_first: f64) -> bool {
    (unsafe { lyra_ffi::coach::lyra_coach_push(samples.as_ptr(), samples.len(), t_first) }) == 0
}
pub fn coach_events() -> Value {
    json_call(lyra_ffi::coach::lyra_coach_events())
}
pub fn coach_score() -> Value {
    json_call(lyra_ffi::coach::lyra_coach_score())
}
pub fn coach_start_calibration(t0: f64, bpm: f64) -> bool {
    lyra_ffi::coach::lyra_coach_start_calibration(t0, bpm) == 0
}
pub fn coach_complete_calibration() -> Value {
    json_call(lyra_ffi::coach::lyra_coach_complete_calibration())
}
pub fn coach_count_in(first_index: u64) -> bool {
    lyra_ffi::coach::lyra_coach_count_in(first_index as usize) == 0
}
pub fn coach_feedback(mode: &str) -> bool {
    (unsafe { lyra_ffi::coach::lyra_coach_feedback(cs(mode).as_ptr()) }) == 0
}
pub fn coach_stop() {
    lyra_ffi::coach::lyra_coach_stop()
}

// ── map (offline analysis; blocking — worker threads) ───────────────────

pub fn map_analyze(db: &Path, path: &str, maps_dir: &Path, stages: Option<&str>) -> Value {
    let stages_c = stages.map(cs);
    LibHandle::open(db)
        .map(|l| {
            json_call(unsafe {
                lyra_ffi::map::lyra_map_analyze(
                    l.0,
                    cs(path).as_ptr(),
                    cstr(maps_dir).as_ptr(),
                    stages_c.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                )
            })
        })
        .unwrap_or(Value::Null)
}

/// Cached map for a track, if analysis already ran.
pub fn map_for_track(db: &Path, path: &str) -> Value {
    LibHandle::open(db)
        .map(|l| json_call(unsafe { lyra_ffi::map::lyra_map_for_track(l.0, cs(path).as_ptr()) }))
        .unwrap_or(Value::Null)
}
