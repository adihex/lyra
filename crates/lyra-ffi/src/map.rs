//! Map FFI: run the analysis pipeline on a track and serve `.lyramap`
//! artifacts as JSON. `lyra_map_analyze` is the heavy call (decode + all
//! enabled stages) — callers run it off the UI thread. Registry rows land
//! in `track_maps` when a lib handle is passed.

use std::ffi::{c_char, CStr, CString};
use std::path::{Path, PathBuf};

use lyra_map::{encode_map, record_for, MapGen, MapOptions, StageSet};
use serde_json::json;

fn opt_str(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p) }
        .to_str()
        .ok()
        .map(str::to_owned)
}

fn into_raw(s: String) -> *mut c_char {
    CString::new(s).unwrap_or_default().into_raw()
}

fn err_json(msg: impl std::fmt::Display) -> *mut c_char {
    into_raw(json!({"error": msg.to_string()}).to_string())
}

/// Analyse `path` through the map pipeline. `maps_dir` receives
/// `<audio_hash>.lyramap` (created if missing); NULL falls back to
/// `<db-dir>/maps` isn't knowable here, so NULL → per-track `maps/` beside
/// nothing — return an error instead. `stages`: NULL → all; else a
/// comma list per `StageSet::parse`.
/// Returns `{map_path, status, overall_conf, audio_hash, beats, sections,
/// chords, strums, notes, tab}` or `{error}`.
///
/// # Safety
/// `lib` may be null (registry write skipped) or a live library handle.
/// `path`, `maps_dir` and `stages` may be null (null path/maps_dir yields error JSON; null stages means all stages).
/// Non-null string args must be valid NUL-terminated C strings.
/// Free the non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_map_analyze(
    lib: *mut lyra_store::Library,
    path: *const c_char,
    maps_dir: *const c_char,
    stages: *const c_char,
) -> *mut c_char {
    let Some(path) = opt_str(path) else {
        return err_json("null path");
    };
    let Some(maps_dir) = opt_str(maps_dir).map(PathBuf::from) else {
        return err_json("null maps_dir");
    };
    let stage_set = opt_str(stages).map_or_else(StageSet::default, |s| StageSet::parse(&s));
    if let Err(e) = std::fs::create_dir_all(&maps_dir) {
        return err_json(format!("maps_dir: {e}"));
    }

    let gen = MapGen::new(MapOptions {
        models_dir: lyra_map::grid_models_dir(),
        stages: stage_set,
        maps_dir: Some(maps_dir.clone()),
        ..MapOptions::default()
    });
    let mut map = match gen.for_path(Path::new(&path)) {
        Ok(m) => m,
        Err(e) => return err_json(format!("pipeline: {e}")),
    };
    let bytes = match encode_map(&mut map) {
        Ok(b) => b,
        Err(e) => return err_json(format!("encode: {e}")),
    };
    let out = maps_dir.join(format!("{}.lyramap", map.audio_hash));
    if let Err(e) = std::fs::write(&out, &bytes) {
        return err_json(format!("write: {e}"));
    }

    if !lib.is_null() {
        let lib = unsafe { &*lib };
        let rec = record_for(&map, &out.display().to_string());
        let _ = lib
            .upsert_map(&lyra_store::TrackMapRow {
                audio_hash: rec.audio_hash.clone(),
                map_path: rec.map_path.clone(),
                pipeline_ver: rec.pipeline_ver.clone(),
                status: rec.status.clone(),
                overall_conf: rec.overall_conf,
                updated_at: rec.updated_at,
            })
            .and_then(|_| lib.set_track_audio_hash(&path, &rec.audio_hash));
    }

    let status = lyra_map::registry_status(&map);
    into_raw(
        json!({
            "map_path": out.display().to_string(),
            "audio_hash": map.audio_hash,
            "status": status.as_str(),
            "overall_conf": map.quality.overall,
            "beats": map.grid.beats.len(),
            "sections": map.sections.len(),
            "chords": map.chords.len(),
            "strums": map.strums.len(),
            "notes": map.notes.len(),
            "tab": map.tab.len(),
        })
        .to_string(),
    )
}

/// Decode a `.lyramap` file → full SongMap JSON.
///
/// # Safety
/// `map_path` may be null (error JSON); otherwise it must be a valid NUL-terminated C string.
/// Free the non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_map_load(map_path: *const c_char) -> *mut c_char {
    let Some(p) = opt_str(map_path) else {
        return err_json("null map_path");
    };
    let bytes = match std::fs::read(&p) {
        Ok(b) => b,
        Err(e) => return err_json(format!("read: {e}")),
    };
    match lyra_map::decode_map_bytes(&bytes) {
        Ok(map) => into_raw(serde_json::to_string(&map).unwrap_or_default()),
        Err(e) => err_json(format!("decode: {e}")),
    }
}

/// Resolve a track path → its SongMap JSON via `tracks.audio_hash` →
/// `track_maps`. `{status:"none"}` when never analysed.
///
/// # Safety
/// `lib` may be null (error JSON) or a live library handle.
/// `path` may be null (error JSON); otherwise a valid NUL-terminated C string.
/// Free the non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_map_for_track(
    lib: *mut lyra_store::Library,
    path: *const c_char,
) -> *mut c_char {
    if lib.is_null() {
        return err_json("null lib");
    }
    let Some(path) = opt_str(path) else {
        return err_json("null path");
    };
    let lib = unsafe { &*lib };
    let hash = match lib.track_audio_hash(&path) {
        Ok(Some(h)) => h,
        _ => return into_raw(json!({"status": "none"}).to_string()),
    };
    let row = match lib.map_for_hash(&hash) {
        Ok(Some(r)) => r,
        _ => return into_raw(json!({"status": "none"}).to_string()),
    };
    let bytes = match std::fs::read(&row.map_path) {
        Ok(b) => b,
        Err(_) => return into_raw(json!({"status": "missing_artifact"}).to_string()),
    };
    match lyra_map::decode_map_bytes(&bytes) {
        Ok(map) => into_raw(serde_json::to_string(&map).unwrap_or_default()),
        Err(e) => err_json(format!("decode: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    /// Synthesize a 2 s 440 Hz WAV, run the pipeline, reload the artifact.
    /// ONNX-less stages report Skipped/Failed rather than erroring the run.
    #[test]
    fn analyze_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("lyra-map-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("tone.wav");
        // 2 s mono 16-bit 44.1 kHz sine with 4 loud beats at 0.5 s spacing.
        let sr = 44_100usize;
        let mut pcm = Vec::with_capacity(sr * 2);
        for i in 0..sr * 2 {
            let t = i as f64 / sr as f64;
            let beat = (t % 0.5) < 0.08;
            let s = (t * 440.0 * 2.0 * std::f64::consts::PI).sin() as f32
                * if beat { 0.9 } else { 0.15 };
            pcm.extend_from_slice(
                &((s * i16::MAX as f32).clamp(-32768.0, 32767.0) as i16).to_le_bytes(),
            );
        }
        // Minimal WAV header.
        let mut f = Vec::new();
        f.extend_from_slice(b"RIFF");
        f.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
        f.extend_from_slice(b"WAVEfmt ");
        f.extend_from_slice(&16u32.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&(sr as u32).to_le_bytes());
        f.extend_from_slice(&(sr as u32 * 2).to_le_bytes());
        f.extend_from_slice(&2u16.to_le_bytes());
        f.extend_from_slice(&16u16.to_le_bytes());
        f.extend_from_slice(b"data");
        f.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        f.extend_from_slice(&pcm);
        std::fs::write(&wav, &f).unwrap();

        let maps = dir.join("maps");
        let rc = unsafe {
            lyra_map_analyze(
                std::ptr::null_mut(),
                CString::new(wav.to_str().unwrap()).unwrap().as_ptr(),
                CString::new(maps.to_str().unwrap()).unwrap().as_ptr(),
                std::ptr::null(),
            )
        };
        let s = unsafe { CStr::from_ptr(rc) }.to_str().unwrap().to_string();
        unsafe { crate::lyra_string_free(rc) };
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert!(v.get("error").is_none(), "analyze failed: {s}");
        assert!(v["map_path"].as_str().unwrap().ends_with(".lyramap"));

        let mp = CString::new(v["map_path"].as_str().unwrap()).unwrap();
        let raw = unsafe { lyra_map_load(mp.as_ptr()) };
        let js = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
        unsafe { crate::lyra_string_free(raw) };
        let map: serde_json::Value = serde_json::from_str(&js).unwrap();
        assert_eq!(map["duration_s"].as_f64().unwrap().round() as i64, 2);
        assert!(map["grid"].is_object() && map["quality"].is_object());

        // Null-path guards.
        assert!(unsafe { CStr::from_ptr(lyra_map_load(std::ptr::null())) }
            .to_str()
            .unwrap()
            .contains("error"));
        assert!(unsafe {
            CStr::from_ptr(lyra_map_analyze(
                std::ptr::null_mut(),
                std::ptr::null(),
                mp.as_ptr(),
                std::ptr::null(),
            ))
        }
        .to_str()
        .unwrap()
        .contains("error"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
