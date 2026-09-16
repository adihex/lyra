//! Lyra core: domain model shared by every layer.
//! Owns the vocabulary — formats, tracks, player/scan events — that the
//! FFI boundary, the remote protocol, and the library DB all speak.

use serde::{Deserialize, Serialize};

/// Audio formats Lyra can probe/decode. Superset of BitMuse's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    Flac,
    Aiff,
    Wav,
    M4a,   // AAC or ALAC inside — codec distinguishes
    Mp3,
    Ogg,   // Vorbis or Opus inside
    Dsf,
    Dff,
    Ape,
    WavPack,
    Cue,   // container-adjacent: cue sheets enumerate tracks
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrackId(pub u64);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamInfo {
    pub format: AudioFormat,
    pub codec: String,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub bits_per_sample: Option<u32>,
    pub duration_secs: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TagMap {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<u32>,
    pub track_number: Option<u32>,
}

/// One scanned library row — path + tags + stream info. What the library
/// table, queue, and remote API all share.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryTrack {
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<u32>,
    pub track_number: Option<u32>,
    pub duration_secs: Option<f64>,
    pub format: AudioFormat,
    pub codec: String,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub bits_per_sample: Option<u32>,
    /// sha256 of the cached artwork row (artwork table); Swift composes
    /// cache paths from it — no pixel data crosses FFI.
    pub artwork_hash: Option<String>,
}

/// Commands the UI / remote can send to the engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "camelCase")]
pub enum PlayerCommand {
    Play { track: Option<TrackId> },
    Toggle,
    Next,
    Prev,
    Seek { position_secs: f64 },
    Volume { value: f32 },
    Mute { on: bool },
    StopAfterCurrent,
    QueueAdd { track: TrackId },
}

/// Events the engine pushes to subscribers (UI, remote WS clients).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum PlayerEvent {
    State { playing: bool, position_secs: f64, track: Option<TrackId> },
    TrackChanged { track: TrackId },
    QueueChanged,
    Error { message: String },
}

/// Scanner progress, surfaced to UI + remote.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum ScanEvent {
    Started { folder: String },
    Progress { scanned: u64, total_hint: Option<u64> },
    TrackFound { path: String },
    Finished { scanned: u64, elapsed_secs: f64 },
    Failed { folder: String, message: String },
}

#[derive(Debug, thiserror::Error)]
pub enum LyraError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    #[error("decode: {0}")]
    Decode(String),
    #[error("tag: {0}")]
    Tag(String),
    #[error("remote: {0}")]
    Remote(String),
    #[cfg(feature = "db")]
    #[error("db: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("audio: {0}")]
    Audio(String),
}
