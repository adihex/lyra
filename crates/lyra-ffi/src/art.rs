//! Online artwork fetch — Cover Art Archive via MusicBrainz.
//!
//! `lyra_art_fetch` blocks (network I/O) — the Swift caller runs it on a
//! background queue. The artwork_fetch ledger is the negative cache:
//! not_found rows park for 30d, errors for 1d, 'ok' never refetches.

use std::ffi::{c_char, CStr, CString};

use lyra_search::cover_art::{ArtFetch, CoverArtClient};
use lyra_store::Library;
use serde_json::json;

fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p) }.to_str().ok()
}

fn out(v: serde_json::Value) -> *mut c_char {
    CString::new(v.to_string())
        .unwrap_or_else(|_| CString::new("{}").unwrap())
        .into_raw()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

const DAY: i64 = 86_400;

/// Fetch missing cover art for the track at `path` — applies the result
/// album-wide. Blocking; returns a JSON state object. Free the returned
/// pointer with lyra_string_free.
///
/// States: cached | ok | no_track | no_album | not_due | not_found | error
///
/// # Safety
/// `lib` must be a live library handle or null (null reports an error state).
/// `path` must be null or a valid NUL-terminated C string.
/// Free the non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_art_fetch(lib: *mut Library, path: *const c_char) -> *mut c_char {
    let Some(path) = cstr(path) else {
        return out(json!({"state": "error", "error": "null path"}));
    };
    if lib.is_null() {
        return out(json!({"state": "error", "error": "null lib"}));
    }
    let lib = unsafe { &*lib };

    let Ok(row) = lib.track_art_query(path) else {
        return out(json!({"state": "error", "error": "art query failed"}));
    };
    let Some((album, artist, album_artist, hash, title)) = row else {
        return out(json!({"state": "no_track"}));
    };
    if let Some(h) = hash.as_deref() {
        if !h.is_empty() {
            return out(json!({"state": "cached", "hash": h}));
        }
    }
    let Some(album) = album.filter(|a| !a.is_empty()) else {
        return out(json!({"state": "no_album"}));
    };
    // Album artist when tagged — MB's artist clause wants the release's
    // credited name, which is the album-level credit.
    let artist_key = album_artist
        .as_deref()
        .filter(|a| !a.is_empty())
        .or(artist.as_deref())
        .unwrap_or("");
    if artist_key.is_empty() {
        return out(json!({"state": "no_artist"}));
    }
    let key = Library::album_art_key(album_artist.as_deref(), artist.as_deref(), &album);
    match lib.art_fetch_due(&key) {
        Ok(true) => {}
        Ok(false) => {
            let (_, hash, retry) = lib.art_fetch_row(&key).ok().flatten().unwrap_or_default();
            return out(json!({"state": "not_due", "hash": hash,
                              "next_retry_at": retry}));
        }
        Err(e) => return out(json!({"state": "error", "error": e.to_string()})),
    }

    let client = CoverArtClient::new();
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return out(json!({"state": "error", "error": e.to_string()})),
    };
    match rt.block_on(client.fetch_front(artist_key, &album, title.as_deref())) {
        Ok(art) => {
            let hash = lib.ingest_artwork(&art.bytes, &art.mime, "caa");
            match hash {
                Some(h) => {
                    let applied = lib.set_album_artwork(&album, artist_key, &h).unwrap_or(0);
                    let _ = lib.art_fetch_record(
                        &key,
                        Some(&art.mbid),
                        "ok",
                        Some(200),
                        None,
                        Some(&h),
                    );
                    out(json!({
                        "state": "ok", "hash": h, "mbid": art.mbid,
                        "applied": applied,
                    }))
                }
                None => {
                    let _ = lib.art_fetch_record(
                        &key,
                        Some(&art.mbid),
                        "error",
                        None,
                        Some(now_secs() + DAY),
                        None,
                    );
                    out(json!({"state": "error", "error": "image rejected"}))
                }
            }
        }
        Err(ArtFetch::NotFound) => {
            // Negative cache: don't re-ask MB for a month.
            let _ = lib.art_fetch_record(
                &key,
                None,
                "not_found",
                Some(404),
                Some(now_secs() + 30 * DAY),
                None,
            );
            out(json!({"state": "not_found"}))
        }
        Err(ArtFetch::Http(s)) => {
            let _ = lib.art_fetch_record(
                &key,
                None,
                "error",
                Some(s as i64),
                Some(now_secs() + DAY),
                None,
            );
            out(json!({"state": "error", "error": format!("http {s}")}))
        }
        Err(e) => {
            let _ =
                lib.art_fetch_record(&key, None, "error", None, Some(now_secs() + 6 * 3600), None);
            out(json!({"state": "error", "error": e.to_string()}))
        }
    }
}

/// Same pipeline for rows that aren't in `tracks` — torrent-materialized
/// rows carry synthesized metadata, so the caller passes it explicitly:
/// `{"artist": "Aerosmith", "title": "Dream On", "album": ""}`. Empty
/// album skips the album chain; the recording fallback covers
/// artist+title. The returned hash applies in-memory only — there is no
/// DB row to stamp.
///
/// # Safety
/// `lib` must be a live library handle or null (null reports an error state).
/// `query_json` must be null or a valid NUL-terminated C string.
/// Free the non-null return with `lyra_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lyra_art_fetch_meta(
    lib: *mut Library,
    query_json: *const c_char,
) -> *mut c_char {
    let Some(js) = cstr(query_json) else {
        return out(json!({"state": "error", "error": "null query"}));
    };
    if lib.is_null() {
        return out(json!({"state": "error", "error": "null lib"}));
    }
    let lib = unsafe { &*lib };
    let q: serde_json::Value = match serde_json::from_str(js) {
        Ok(v) => v,
        Err(e) => return out(json!({"state": "error", "error": e.to_string()})),
    };
    let artist = q["artist"].as_str().unwrap_or("");
    let album = q["album"].as_str().unwrap_or("");
    let title = q["title"].as_str().unwrap_or("");
    if artist.is_empty() && title.is_empty() {
        return out(json!({"state": "no_artist"}));
    }
    // Ledger key rides on what was actually queried — album when present,
    // else the title (album chain skipped server-side for empty album).
    let key = if album.is_empty() {
        format!(
            "{}\u{1f}rec:{}",
            artist.to_lowercase(),
            title.to_lowercase()
        )
    } else {
        Library::album_art_key(None, Some(artist), album)
    };
    match lib.art_fetch_due(&key) {
        Ok(true) => {}
        Ok(false) => {
            // The ledger is the only record for metadata-keyed art —
            // hand the stored hash back so torrent tiles re-render after
            // relaunch without another fetch.
            let (_, hash, _) = lib.art_fetch_row(&key).ok().flatten().unwrap_or_default();
            return out(json!({"state": "not_due", "hash": hash}));
        }
        Err(e) => return out(json!({"state": "error", "error": e.to_string()})),
    }
    let client = CoverArtClient::new();
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return out(json!({"state": "error", "error": e.to_string()})),
    };
    match rt.block_on(client.fetch_front(artist, album, (!title.is_empty()).then_some(title))) {
        Ok(art) => match lib.ingest_artwork(&art.bytes, &art.mime, "caa") {
            Some(h) => {
                let _ =
                    lib.art_fetch_record(&key, Some(&art.mbid), "ok", Some(200), None, Some(&h));
                out(json!({"state": "ok", "hash": h, "mbid": art.mbid}))
            }
            None => {
                let _ = lib.art_fetch_record(
                    &key,
                    Some(&art.mbid),
                    "error",
                    None,
                    Some(now_secs() + DAY),
                    None,
                );
                out(json!({"state": "error", "error": "image rejected"}))
            }
        },
        Err(ArtFetch::NotFound) => {
            let _ = lib.art_fetch_record(
                &key,
                None,
                "not_found",
                Some(404),
                Some(now_secs() + 30 * DAY),
                None,
            );
            out(json!({"state": "not_found"}))
        }
        Err(ArtFetch::Http(s)) => {
            let _ = lib.art_fetch_record(
                &key,
                None,
                "error",
                Some(s as i64),
                Some(now_secs() + DAY),
                None,
            );
            out(json!({"state": "error", "error": format!("http {s}")}))
        }
        Err(e) => {
            let _ =
                lib.art_fetch_record(&key, None, "error", None, Some(now_secs() + 6 * 3600), None);
            out(json!({"state": "error", "error": e.to_string()}))
        }
    }
}
