//! Models + small persistence layer. The macOS app keeps prefs in
//! UserDefaults and remote-source passwords in the Keychain; the GTK
//! port stores the same state as JSON under the data dir — passwords are
//! intentionally not persisted (no Keychain here), the field is
//! session-only.

use lyra_core::LibraryTrack;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

/// Display-facing view of a LibraryTrack — matches the app's fallbacks.
#[derive(Clone, Debug)]
pub struct Track {
    pub id: String, // path (or torrent://id/idx for torrent rows)
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    pub codec: String,
    pub track_no: u32,
    pub artwork_hash: Option<String>,
    pub source: Source,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    File,
    Torrent {
        id: i32,
        file_idx: i32,
    },
    /// sftp://host[:port]/… — resolved against saved remote profiles.
    Remote,
}

impl Track {
    pub fn from_library(t: &LibraryTrack) -> Self {
        let path = t.path.clone();
        let stem = Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            id: path.clone(),
            title: t.title.clone().unwrap_or(stem),
            artist: t.artist.clone().unwrap_or_else(|| "Unknown Artist".into()),
            album: t.album.clone().unwrap_or_else(|| "Unknown Album".into()),
            duration: t.duration_secs.unwrap_or(0.0),
            codec: t.codec.clone(),
            track_no: t.track_number.unwrap_or(0),
            artwork_hash: t.artwork_hash.clone(),
            path: path.clone(),
            source: if path.starts_with("sftp://") {
                Source::Remote
            } else {
                Source::File
            },
        }
    }

    /// Extensions symphonia decodes — filters nfo/txt/jpg out of torrent
    /// file lists (same set as the app's Track.audioExts).
    pub fn is_audio(&self) -> bool {
        const EXT: [&str; 17] = [
            "flac", "mp3", "m4a", "aac", "aiff", "aif", "wav", "wave", "ogg", "oga", "opus", "wv",
            "ape", "shn", "alac", "mp4", "mka",
        ];
        EXT.contains(&self.codec.to_lowercase().as_str())
    }

    /// A file inside an active torrent — "01. Artist - Title" names get
    /// the same parse the app's Track.init(torrentId:) does.
    pub fn from_torrent(torrent_id: i32, f: &Value) -> Option<Self> {
        let idx = f.get("index")?.as_i64()? as i32;
        let name = f.get("path")?.as_str()?;
        let stem = Path::new(name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("file {idx}"));
        let stem = stem
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start_matches(['.', ')', '-', '–', '_', ' '])
            .to_string();
        let (artist, title) = match stem.split_once(" - ") {
            Some((a, t)) => (a.to_string(), t.to_string()),
            None => (format!("torrent #{torrent_id}"), stem),
        };
        let ext = Path::new(name)
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_default();
        Some(Self {
            id: format!("torrent://{torrent_id}/{idx}"),
            path: format!("torrent://{torrent_id}/{idx}"),
            title,
            artist,
            album: String::new(),
            duration: 0.0,
            codec: ext,
            track_no: (idx + 1) as u32,
            artwork_hash: None,
            source: Source::Torrent {
                id: torrent_id,
                file_idx: idx,
            },
        })
    }
}

pub fn fmt_dur(s: f64) -> String {
    let t = s.max(0.0) as u64;
    format!("{}:{:02}", t / 60, t % 60)
}

/// UI prefs — UserDefaults equivalents that matter on Linux.
#[derive(Serialize, Deserialize, Clone)]
pub struct Prefs {
    pub volume: f64,
    pub exclusive_output: bool,
    pub remote_port: u16,
    pub viz_mode: i32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            volume: 0.8,
            exclusive_output: false,
            remote_port: 9600,
            viz_mode: 0,
        }
    }
}

impl Prefs {
    pub fn load(data_dir: &Path) -> Self {
        std::fs::read_to_string(data_dir.join("prefs.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, data_dir: &Path) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(data_dir.join("prefs.json"), s);
        }
    }
}

/// SSH/SFTP library root — same shape as the app's RemoteSource minus the
/// security-scoped bookmark (key_path is a plain path on Linux).
#[derive(Serialize, Deserialize, Clone)]
pub struct RemoteSource {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub root_path: String,
    pub key_path: String,
}

impl RemoteSource {
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            &self.host
        } else {
            &self.name
        }
    }
    /// The profile JSON remlib expects: {host,port,user,key_path,root_path}.
    /// `password` is session-scoped — never written to disk.
    pub fn profile_json(&self, password: Option<&str>) -> Value {
        let mut d = serde_json::json!({
            "host": self.host, "port": self.port, "root_path": self.root_path,
        });
        if !self.user.is_empty() {
            d["user"] = self.user.clone().into();
        }
        if !self.key_path.is_empty() {
            d["key_path"] = self.key_path.clone().into();
        }
        if let Some(pw) = password.filter(|p| !p.is_empty()) {
            d["password"] = pw.into();
        }
        d
    }
    /// sftp:// prefix remote rows are keyed under.
    pub fn uri_prefix(&self) -> String {
        if self.port == 22 {
            format!("sftp://{}", self.host)
        } else {
            format!("sftp://{}:{}", self.host, self.port)
        }
    }
}

pub fn load_remotes(data_dir: &Path) -> Vec<RemoteSource> {
    std::fs::read_to_string(data_dir.join("remotes.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_remotes(data_dir: &Path, rs: &[RemoteSource]) {
    if let Ok(s) = serde_json::to_string_pretty(rs) {
        let _ = std::fs::write(data_dir.join("remotes.json"), s);
    }
}

pub const EQ_FREQS: [f32; 10] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];
