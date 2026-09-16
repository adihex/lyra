//! [`MockDispatcher`]: in-memory player backend for tests and offline CLI work.
//! Implements the full op table with revision counters so conflict, job and
//! subscription behavior can be tested on Linux with no engine.

use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::Duration;

use crate::dispatcher::{ApiError, DispatchCtx, Dispatcher};

#[derive(Debug, Clone)]
struct Track {
    id: String,
    title: String,
    artist: String,
    album: String,
}

impl Track {
    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "artist": self.artist,
            "album": self.album,
            "source": "library",
            "provider_meta": {},
        })
    }
}

struct State {
    revision: i64,
    playlist_revision: i64,
    playing: bool,
    position: f64,
    duration: f64,
    volume: f64,
    speed: f64,
    shuffle: bool,
    repeat: String,
    track: Option<Track>,
    queue: Vec<Track>,
    queue_index: usize,
    bands: Vec<f64>,
    preamp: f64,
    device: String,
    library: Vec<Track>,
    scan_count: u64,
    enqueued_keys: std::collections::HashSet<String>,
}

impl State {
    fn new() -> Self {
        let library = vec![
            Track {
                id: "t-aphex".into(),
                title: "Windowlicker".into(),
                artist: "Aphex Twin".into(),
                album: "Windowlicker".into(),
            },
            Track {
                id: "t-boards".into(),
                title: "Dayvan Cowboy".into(),
                artist: "Boards of Canada".into(),
                album: "The Campfire Headphase".into(),
            },
            Track {
                id: "t-coltrane".into(),
                title: "Naima".into(),
                artist: "John Coltrane".into(),
                album: "Giant Steps".into(),
            },
        ];
        Self {
            revision: 1,
            playlist_revision: 1,
            playing: false,
            position: 0.0,
            duration: 183.0,
            volume: 0.8,
            speed: 1.0,
            shuffle: false,
            repeat: "off".into(),
            track: None,
            queue: Vec::new(),
            queue_index: 0,
            bands: vec![0.0; 10],
            preamp: 0.0,
            device: "default".into(),
            library,
            scan_count: 0,
            enqueued_keys: Default::default(),
        }
    }

    fn bump(&mut self) {
        self.revision += 1;
    }

    fn bump_playlist(&mut self) {
        self.playlist_revision += 1;
        self.bump();
    }

    fn snapshot(&self) -> Value {
        json!({
            "revision": self.revision,
            "playlist_revision": self.playlist_revision,
            "state": if self.playing { "playing" } else { "paused" },
            "position": self.position,
            "duration": self.duration,
            "seekable": true,
            "volume": self.volume,
            "speed": self.speed,
            "shuffle": self.shuffle,
            "repeat": self.repeat,
            "track": self.track.as_ref().map(|t| t.json()),
            "queue": {"length": self.queue.len(), "index": self.queue_index},
            "eq": {"bands": self.bands, "preamp": self.preamp},
            "viz": {"bands": vec![0.0; 32]},
            "audio": {"device": self.device, "sample_rate": 48000},
            "stream_error": Value::Null,
        })
    }
}

pub struct MockDispatcher {
    state: Mutex<State>,
    /// Simulated duration of async ops (scan/torrent) so job polling and
    /// cancel tests can observe `queued`/`running`.
    pub job_work: Duration,
}

impl Default for MockDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl MockDispatcher {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State::new()),
            job_work: Duration::from_millis(300),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn get_num(params: &Value, key: &str) -> Option<f64> {
        params.get(key)?.as_f64()
    }

    fn get_int(params: &Value, key: &str) -> Option<i64> {
        params.get(key)?.as_i64()
    }

    fn get_str<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
        params.get(key)?.as_str()
    }
}

impl Dispatcher for MockDispatcher {
    fn snapshot(&self) -> Value {
        self.lock().snapshot()
    }

    fn call(&self, ctx: &DispatchCtx, method: &str, params: &Value) -> Result<Value, ApiError> {
        let mut st = self.lock();
        match method {
            "play" => {
                st.playing = true;
                st.bump();
                Ok(json!({"state": "playing"}))
            }
            "pause" => {
                st.playing = false;
                st.bump();
                Ok(json!({"state": "paused"}))
            }
            "toggle" => {
                st.playing = !st.playing;
                st.bump();
                Ok(json!({"state": if st.playing { "playing" } else { "paused" }}))
            }
            "stop" => {
                st.playing = false;
                st.position = 0.0;
                st.bump();
                Ok(json!({"state": "stopped"}))
            }
            "next" | "prev" => {
                if !st.queue.is_empty() {
                    if method == "next" {
                        st.queue_index = (st.queue_index + 1) % st.queue.len();
                    } else {
                        st.queue_index = (st.queue_index + st.queue.len() - 1) % st.queue.len();
                    }
                    st.track = Some(st.queue[st.queue_index].clone());
                    st.position = 0.0;
                }
                st.playing = true;
                st.bump();
                ctx.emit("runtime.playback", st.snapshot());
                Ok(json!({"state": "playing"}))
            }
            "seek.absolute" => {
                let pos = Self::get_num(params, "position_s")
                    .ok_or_else(|| ApiError::invalid_param("seek.absolute needs position_s"))?;
                st.position = pos.clamp(0.0, st.duration);
                st.bump();
                Ok(json!({"position": st.position}))
            }
            "seek.relative" => {
                let d = Self::get_num(params, "delta_s")
                    .ok_or_else(|| ApiError::invalid_param("seek.relative needs delta_s"))?;
                st.position = (st.position + d).clamp(0.0, st.duration);
                st.bump();
                Ok(json!({"position": st.position}))
            }
            "volume" | "volume.set" => {
                let v = Self::get_num(params, "volume")
                    .ok_or_else(|| ApiError::invalid_param("volume.set needs volume"))?;
                if !(0.0..=1.0).contains(&v) {
                    return Err(ApiError::invalid_param("volume must be 0.0..=1.0"));
                }
                st.volume = v;
                st.bump();
                Ok(json!({"volume": st.volume}))
            }
            "speed" => {
                let s = Self::get_num(params, "speed")
                    .ok_or_else(|| ApiError::invalid_param("speed needs speed"))?;
                st.speed = s;
                st.bump();
                Ok(json!({"speed": st.speed}))
            }
            "shuffle" => {
                let e = params
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| ApiError::invalid_param("shuffle needs enabled"))?;
                st.shuffle = e;
                st.bump();
                Ok(json!({"shuffle": st.shuffle}))
            }
            "repeat" => {
                let m = Self::get_str(params, "mode")
                    .ok_or_else(|| ApiError::invalid_param("repeat needs mode"))?;
                if !["off", "all", "one"].contains(&m) {
                    return Err(ApiError::invalid_param("repeat mode must be off|all|one"));
                }
                st.repeat = m.to_string();
                st.bump();
                Ok(json!({"repeat": st.repeat}))
            }
            "eq.get" => Ok(json!({"bands": st.bands, "preamp": st.preamp})),
            "eq.set" => {
                if let Some(bands) = params.get("bands").and_then(|b| b.as_array()) {
                    st.bands = bands.iter().map(|b| b.as_f64().unwrap_or(0.0)).collect();
                }
                if let Some(p) = params.get("preamp").and_then(Value::as_f64) {
                    st.preamp = p;
                }
                st.bump();
                Ok(json!({"bands": st.bands, "preamp": st.preamp}))
            }
            "eq.band.set" => {
                let band = Self::get_int(params, "band")
                    .ok_or_else(|| ApiError::invalid_param("eq.band.set needs band"))?;
                let gain = Self::get_num(params, "gain_db")
                    .ok_or_else(|| ApiError::invalid_param("eq.band.set needs gain_db"))?;
                let i = usize::try_from(band).map_err(|_| ApiError::invalid_param("band < 0"))?;
                if i >= st.bands.len() {
                    return Err(ApiError::invalid_param("band out of range"));
                }
                st.bands[i] = gain;
                st.bump();
                Ok(json!({"band": band, "gain_db": gain}))
            }
            "queue.list" => Ok(json!({
                "index": st.queue_index,
                "tracks": st.queue.iter().map(Track::json).collect::<Vec<_>>(),
            })),
            "queue.play" => {
                let i = Self::get_int(params, "index")
                    .ok_or_else(|| ApiError::invalid_param("queue.play needs index"))?;
                let i = usize::try_from(i).map_err(|_| ApiError::invalid_param("index < 0"))?;
                if i >= st.queue.len() {
                    return Err(ApiError::not_found("queue index out of range"));
                }
                st.queue_index = i;
                st.track = Some(st.queue[i].clone());
                st.position = 0.0;
                st.playing = true;
                st.bump();
                ctx.emit("runtime.playback", st.snapshot());
                Ok(json!({"index": i}))
            }
            "queue.enqueue" => {
                // Idempotency key: double-submits dedupe (§6 rule 3).
                if let Some(key) = Self::get_str(params, "idempotency_key") {
                    if st.enqueued_keys.contains(key) {
                        return Ok(json!({"deduped": true, "length": st.queue.len()}));
                    }
                    st.enqueued_keys.insert(key.to_string());
                }
                let track = if let Some(id) = Self::get_str(params, "track_id") {
                    st.library
                        .iter()
                        .find(|t| t.id == id)
                        .cloned()
                        .ok_or_else(|| ApiError::not_found(format!("track {id}")))?
                } else if let Some(q) = Self::get_str(params, "query") {
                    let q = q.to_lowercase();
                    st.library
                        .iter()
                        .find(|t| {
                            t.title.to_lowercase().contains(&q)
                                || t.artist.to_lowercase().contains(&q)
                        })
                        .cloned()
                        .ok_or_else(|| ApiError::not_found(format!("no match for {q}")))?
                } else {
                    return Err(ApiError::invalid_param(
                        "queue.enqueue needs track_id or query",
                    ));
                };
                if let Some(pos) = Self::get_int(params, "position") {
                    let i = usize::try_from(pos)
                        .unwrap_or(st.queue.len())
                        .min(st.queue.len());
                    st.queue.insert(i, track);
                } else {
                    st.queue.push(track);
                }
                st.bump_playlist();
                let snap = st.snapshot();
                drop(st);
                ctx.emit("queue", snap.clone());
                Ok(json!({"length": snap["queue"]["length"].clone()}))
            }
            "queue.remove" => {
                let i = Self::get_int(params, "index")
                    .ok_or_else(|| ApiError::invalid_param("queue.remove needs index"))?;
                let i = usize::try_from(i).map_err(|_| ApiError::invalid_param("index < 0"))?;
                if i >= st.queue.len() {
                    return Err(ApiError::not_found("queue index out of range"));
                }
                st.queue.remove(i);
                st.bump_playlist();
                Ok(json!({"length": st.queue.len()}))
            }
            "queue.move" => {
                let from = Self::get_int(params, "from")
                    .ok_or_else(|| ApiError::invalid_param("queue.move needs from"))?;
                let to = Self::get_int(params, "to")
                    .ok_or_else(|| ApiError::invalid_param("queue.move needs to"))?;
                let (from, to) = (
                    usize::try_from(from).unwrap_or(0),
                    usize::try_from(to).unwrap_or(0),
                );
                if from >= st.queue.len() || to >= st.queue.len() {
                    return Err(ApiError::not_found("queue index out of range"));
                }
                let t = st.queue.remove(from);
                st.queue.insert(to, t);
                st.bump_playlist();
                Ok(json!({"from": from, "to": to}))
            }
            "queue.clear" => {
                st.queue.clear();
                st.queue_index = 0;
                st.bump_playlist();
                Ok(json!({"length": 0}))
            }
            "library.search" => {
                let q = Self::get_str(params, "q")
                    .ok_or_else(|| ApiError::invalid_param("library.search needs q"))?
                    .to_lowercase();
                let limit = Self::get_int(params, "limit").unwrap_or(20).max(1) as usize;
                let hits: Vec<Value> = st
                    .library
                    .iter()
                    .filter(|t| {
                        t.title.to_lowercase().contains(&q)
                            || t.artist.to_lowercase().contains(&q)
                            || t.album.to_lowercase().contains(&q)
                    })
                    .take(limit)
                    .map(Track::json)
                    .collect();
                Ok(json!({"results": hits}))
            }
            "library.stats" => Ok(json!({
                "tracks": st.library.len(),
                "scans": st.scan_count,
            })),
            "library.scan" => {
                // Async op: runs on the job worker thread.
                std::thread::sleep(self.job_work);
                st.scan_count += 1;
                st.bump();
                let snap = st.snapshot();
                drop(st);
                ctx.emit(
                    "library.scan",
                    json!({"state": "succeeded", "scans": snap["revision"]}),
                );
                Ok(json!({"scanned": 3, "tracks": 3}))
            }
            "track.play" => {
                let id = Self::get_str(params, "track_id")
                    .ok_or_else(|| ApiError::invalid_param("track.play needs track_id"))?;
                let track = st
                    .library
                    .iter()
                    .find(|t| t.id == id)
                    .cloned()
                    .ok_or_else(|| ApiError::not_found(format!("track {id}")))?;
                st.track = Some(track);
                st.position = 0.0;
                st.playing = true;
                st.bump();
                let snap = st.snapshot();
                drop(st);
                ctx.emit("runtime.playback", snap);
                Ok(json!({"state": "playing"}))
            }
            "track.queue" => {
                let id = Self::get_str(params, "track_id")
                    .ok_or_else(|| ApiError::invalid_param("track.queue needs track_id"))?;
                let track = st
                    .library
                    .iter()
                    .find(|t| t.id == id)
                    .cloned()
                    .ok_or_else(|| ApiError::not_found(format!("track {id}")))?;
                st.queue.push(track);
                st.bump_playlist();
                Ok(json!({"length": st.queue.len()}))
            }
            "url.load" => {
                let url = Self::get_str(params, "url")
                    .ok_or_else(|| ApiError::invalid_param("url.load needs url"))?;
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return Err(ApiError::invalid_param("only http(s) urls"));
                }
                st.playing = true;
                st.bump();
                Ok(json!({"state": "playing", "url": url}))
            }
            "torrent.add" => {
                std::thread::sleep(self.job_work);
                st.bump_playlist();
                Ok(json!({"infohash": "mock", "queued": true}))
            }
            "lyrics.get" => Ok(json!({"lines": [], "synced": false})),
            "spectrum.get" => Ok(json!({"bands": vec![0.0; 32]})),
            "device.list" => Ok(json!({"devices": ["default", "mock-dac"], "current": st.device})),
            "device.set" => {
                let d = Self::get_str(params, "device")
                    .ok_or_else(|| ApiError::invalid_param("device.set needs device"))?;
                st.device = d.to_string();
                st.bump();
                Ok(json!({"device": st.device}))
            }
            "plugin.call" => {
                let plugin = Self::get_str(params, "plugin").unwrap_or("unknown");
                let command = Self::get_str(params, "command").unwrap_or("unknown");
                let out = json!({"plugin": plugin, "command": command, "ok": true});
                let topic = format!("plugin.{plugin}");
                drop(st);
                ctx.emit(&topic, out.clone());
                Ok(out)
            }
            other => Err(ApiError::new(
                crate::protocol::ErrorCode::UnknownMethod,
                format!("unknown method: {other}"),
            )),
        }
    }
}
