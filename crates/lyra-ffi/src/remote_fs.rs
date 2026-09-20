//! Remote-library FFI: scan/probe/pin/play over SSH+SFTP. Profiles arrive
//! as JSON ({host,port,user,key_path,root_path,password?}); `password`
//! isn't a RemoteProfile field — it selects AuthMethod::Password over the
//! profile's implied auth (key file, else agent).

use std::ffi::{c_char, c_int, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lyra_fs::{
    AuthMethod, ByteSource, CachingSource, ExecOpen, ExecWalk, HeaderProbe, RemoteProfile,
    RemoteScanner, ScanOptions, SftpOpener, SftpSource, SftpWalk, Ssh2Backend, SshExecFile,
};
use serde_json::{json, Value};

fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p) }.to_str().ok()
}

/// Transport pref from the profile JSON: "sftp" | "exec" | "auto".
/// Exec exists for sshds with no sftp subsystem (dropbear, restricted
/// shells). "auto" tries SFTP and falls back — the scan is resumable so
/// a failed first pass costs one connect, not the walk.
enum Transport {
    Sftp,
    Exec,
    Auto,
}

fn parse_profile(js: &str) -> Option<(RemoteProfile, AuthMethod, Transport)> {
    let v: Value = serde_json::from_str(js).ok()?;
    let profile: RemoteProfile = serde_json::from_value(v.clone()).ok()?;
    let auth = match v.get("password").and_then(Value::as_str) {
        Some(pw) if !pw.is_empty() => AuthMethod::Password(pw.to_string()),
        _ => profile.default_auth(),
    };
    let t = match v.get("transport").and_then(Value::as_str) {
        Some("sftp") => Transport::Sftp,
        Some("exec") => Transport::Exec,
        _ => Transport::Auto,
    };
    Some((profile, auth, t))
}

fn out(v: Value) -> *mut c_char {
    CString::new(v.to_string()).unwrap_or_default().into_raw()
}

/// Stream-play `remote_path` on the profile's host: SFTP random-access
/// under the 1 MiB block cache, same shape as torrent streaming.
/// 0 = engine accepted; 1 = open failed; 2 = bad args.
///
/// # Safety
/// `e` must be a live engine handle or null (null returns 2).
/// `profile_json` and `remote_path` may be null (returns 2); otherwise valid NUL-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn lyra_engine_play_remote(
    e: *mut lyra_engine::Engine,
    profile_json: *const c_char,
    remote_path: *const c_char,
) -> c_int {
    if e.is_null() {
        return 2;
    }
    let (Some(pj), Some(path)) = (cstr(profile_json), cstr(remote_path)) else {
        return 2;
    };
    let Some((profile, auth, t)) = parse_profile(pj) else {
        return 2;
    };
    let src: Arc<dyn ByteSource> = if matches!(t, Transport::Exec) {
        match SshExecFile::open_profile(&profile, path) {
            Ok(s) => Arc::new(s),
            Err(e) => {
                tracing::warn!("play_remote exec open {}:{path}: {e}", profile.host);
                return 1;
            }
        }
    } else {
        match SftpSource::<Ssh2Backend>::open_with_auth(&profile, path, &auth) {
            Ok(s) => Arc::new(s),
            Err(e) if matches!(t, Transport::Auto) => {
                tracing::info!("play_remote sftp failed ({e}) — exec fallback");
                match SshExecFile::open_profile(&profile, path) {
                    Ok(s) => Arc::new(s),
                    Err(e2) => {
                        tracing::warn!("play_remote exec open {}:{path}: {e2}", profile.host);
                        return 1;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("play_remote open {}:{path}: {e}", profile.host);
                return 1;
            }
        }
    };
    let ext = Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .map(String::from);
    let cached: Arc<dyn ByteSource> = CachingSource::wrap(src);
    unsafe { &*e }.play(cached, ext.as_deref());
    0
}

/// Remote scan into the library DB — SFTP walk + header probes, resumable
/// via the store's scan cursor. Blocks; call off the main thread.
/// Returns ScanStats JSON; null on failure.
///
/// # Safety
/// `l` must be a live library handle or null (null returns null).
/// `profile_json` may be null (returns null); otherwise a valid NUL-terminated C string.
/// Free the non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_remlib_scan(
    l: *mut lyra_store::Library,
    profile_json: *const c_char,
) -> *mut c_char {
    if l.is_null() {
        return std::ptr::null_mut();
    }
    let Some(pj) = cstr(profile_json) else {
        return std::ptr::null_mut();
    };
    let Some((profile, auth, t)) = parse_profile(pj) else {
        return std::ptr::null_mut();
    };
    let cancel = lyra_fs::Cancel::new();
    let opts = ScanOptions::default();
    let lib = unsafe { &*l };
    let result = if matches!(t, Transport::Exec) {
        RemoteScanner::new(
            ExecWalk::new(&profile),
            ExecOpen::new(&profile),
            HeaderProbe::default(),
            &profile,
        )
        .scan(lib, &cancel, &opts)
    } else {
        let r = RemoteScanner::new(
            SftpWalk::new(&profile, &auth),
            SftpOpener::new(&profile, &auth),
            HeaderProbe::default(),
            &profile,
        )
        .scan(lib, &cancel, &opts);
        match (r, t) {
            (Ok(s), _) => Ok(s),
            (Err(e), Transport::Auto) => {
                tracing::info!("sftp scan failed ({e}) — exec fallback");
                RemoteScanner::new(
                    ExecWalk::new(&profile),
                    ExecOpen::new(&profile),
                    HeaderProbe::default(),
                    &profile,
                )
                .scan(lib, &cancel, &opts)
            }
            (Err(e), _) => Err(e),
        }
    };
    match result {
        Ok(s) => out(json!({
            "walked": s.walked, "probed": s.probed, "skipped": s.skipped,
            "pruned": s.pruned, "cancelled": s.cancelled,
        })),
        Err(e) => out(json!({"error": e.to_string()})),
    }
}

/// Cheap health check — ssh `find` count over the root (no probing).
/// Returns {ok:true, files:N, elapsed_ms} or {ok:false, error}.
///
/// # Safety
/// `profile_json` may be null (returns null); otherwise a valid NUL-terminated C string.
/// Free the non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_remlib_test(profile_json: *const c_char) -> *mut c_char {
    let Some(pj) = cstr(profile_json) else {
        return std::ptr::null_mut();
    };
    let Some((profile, _, _)) = parse_profile(pj) else {
        return std::ptr::null_mut();
    };
    let t0 = std::time::Instant::now();
    match lyra_fs::scan(&profile, &profile.root_path) {
        Ok(entries) => out(json!({
            "ok": true,
            "files": entries.len(),
            "elapsed_ms": t0.elapsed().as_millis() as u64,
        })),
        Err(e) => out(json!({"ok": false, "error": e.to_string()})),
    }
}

/// Pin a remote subtree locally via rsync (delta + resume). `local_dir`
/// must be inside the app's writable scope. 0 ok.
///
/// # Safety
/// Each argument may be null (returns 2); otherwise valid NUL-terminated C strings.
#[no_mangle]
pub unsafe extern "C" fn lyra_remlib_pin(
    profile_json: *const c_char,
    remote_dir: *const c_char,
    local_dir: *const c_char,
) -> c_int {
    let (Some(pj), Some(rdir), Some(ldir)) =
        (cstr(profile_json), cstr(remote_dir), cstr(local_dir))
    else {
        return 2;
    };
    let Some((profile, _, _)) = parse_profile(pj) else {
        return 2;
    };
    match lyra_fs::pin(&profile, rdir, &PathBuf::from(ldir)) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{CStr, CString};

    /// Live-network smoke: jiopc over the tailnet. Ignored by default —
    /// run `cargo test -p lyra-ffi remfs -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn remfs_test_and_scan_jiopc() {
        let pj = serde_json::json!({
            "host": "100.122.67.74",
            "port": 2225,
            "user": "001201575025_0",
            "root_path": "/home/001201575025_0/Music",
        });
        let pj = CString::new(pj.to_string()).unwrap();
        let raw = unsafe { super::lyra_remlib_test(pj.as_ptr()) };
        assert!(!raw.is_null());
        let s = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
        unsafe { super::super::lyra_string_free(raw) };
        println!("test: {s}");
        assert!(s.contains("\"ok\":true"), "test failed: {s}");

        // SFTP walk + probe into a temp library — the real scan path.
        let db = std::env::temp_dir().join(format!("lyra-remfs-{}.db", std::process::id()));
        let dbc = CString::new(db.to_str().unwrap()).unwrap();
        let lib = unsafe { super::super::lyra_lib_open(dbc.as_ptr()) };
        assert!(!lib.is_null());
        let raw = unsafe { super::lyra_remlib_scan(lib, pj.as_ptr()) };
        assert!(!raw.is_null());
        let s = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
        unsafe { super::super::lyra_string_free(raw) };
        println!("scan: {s}");
        assert!(!s.contains("error"), "scan failed: {s}");
        let rows = unsafe { super::super::lyra_lib_tracks(lib) };
        let tracks = unsafe { CStr::from_ptr(rows) }
            .to_str()
            .unwrap()
            .to_string();
        unsafe { super::super::lyra_string_free(rows) };
        println!("tracks: {}", &tracks[..tracks.len().min(300)]);
        assert!(tracks.contains("sftp://"), "no sftp rows: {tracks}");
        unsafe { super::super::lyra_lib_free(lib) };

        // Engine open — SFTP session + stat must succeed; decode happens
        // on the worker, so accept is the assertable edge headless.
        let e = super::super::lyra_engine_new();
        assert!(!e.is_null());
        let path = CString::new("/home/001201575025_0/Music/lyra-remote-demo.flac").unwrap();
        let rc = unsafe { super::lyra_engine_play_remote(e, pj.as_ptr(), path.as_ptr()) };
        assert_eq!(rc, 0, "play_remote rc={rc}");
        std::thread::sleep(std::time::Duration::from_secs(3));
        assert_eq!(
            unsafe { super::super::lyra_engine_is_playing(e) },
            1,
            "not playing"
        );
        unsafe { super::super::lyra_engine_free(e) };
        let _ = std::fs::remove_file(&db);
    }
}
