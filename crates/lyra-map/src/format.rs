//! `.lyramap` codec (§4.5): a single-file, content-addressed artifact.
//!
//! ```c
//! struct LyraMapHeader {          // 16 B, little-endian
//!     char  magic[4];             // "LYRM"
//!     u16   format_version;       // = 1
//!     u16   flags;                // bit0: payload is zstd
//!     u64   payload_len;
//! }                               // followed by zstd(serde_json(SongMap))
//! ```
//!
//! `map_id` is the BLAKE3 of the canonical payload bytes: encode serializes
//! once with an empty id, hashes, stamps the id, and serializes again.

use crate::map::SongMap;
use lyra_core::LyraError;

pub const MAGIC: &[u8; 4] = b"LYRM";
pub const FORMAT_VERSION: u16 = 1;
pub const FLAG_ZSTD: u16 = 1;
pub const HEADER_LEN: usize = 16;

fn header_error(msg: &str) -> LyraError {
    LyraError::Decode(format!(".lyramap: {msg}"))
}

/// Encode options. Compressed (zstd) is the on-disk default; the raw-JSON
/// form exists for debugging and byte-level inspection.
#[derive(Debug, Clone, Copy)]
pub struct MapCodec {
    pub compressed: bool,
}

impl Default for MapCodec {
    fn default() -> Self {
        Self { compressed: true }
    }
}

impl MapCodec {
    fn flags(self) -> u16 {
        if self.compressed { FLAG_ZSTD } else { 0 }
    }

    /// Serialize + frame. Stamps `map.map_id` before returning.
    pub fn encode(self, map: &mut SongMap) -> Result<Vec<u8>, LyraError> {
        map.map_id.clear();
        let canonical =
            serde_json::to_vec(map).map_err(|e| LyraError::Decode(e.to_string()))?;
        map.map_id = blake3::hash(&canonical).to_hex().to_string();
        let payload_json =
            serde_json::to_vec(map).map_err(|e| LyraError::Decode(e.to_string()))?;
        let payload = if self.compressed {
            zstd::encode_all(payload_json.as_slice(), 3)
                .map_err(|e| LyraError::Decode(e.to_string()))?
        } else {
            payload_json
        };
        let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&self.flags().to_le_bytes());
        out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        out.extend_from_slice(&payload);
        Ok(out)
    }

    /// Parse + validate + inflate + deserialize.
    pub fn decode(self, bytes: &[u8]) -> Result<SongMap, LyraError> {
        let _ = self;
        decode(bytes)
    }
}

/// Default-codec shorthands used by the pipeline and the CLI.
pub fn encode(map: &mut SongMap) -> Result<Vec<u8>, LyraError> {
    MapCodec::default().encode(map)
}

pub fn decode(bytes: &[u8]) -> Result<SongMap, LyraError> {
    if bytes.len() < HEADER_LEN {
        return Err(header_error("truncated header"));
    }
    if &bytes[..4] != MAGIC {
        return Err(header_error("bad magic"));
    }
    let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
    if version != FORMAT_VERSION {
        return Err(header_error(&format!("unsupported version {version}")));
    }
    let flags = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
    let len = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as usize;
    let payload = bytes.get(HEADER_LEN..HEADER_LEN + len).ok_or_else(|| {
        header_error(&format!(
            "payload_len {len} exceeds file ({} bytes)",
            bytes.len() - HEADER_LEN
        ))
    })?;
    let json = if flags & FLAG_ZSTD != 0 {
        zstd::decode_all(payload).map_err(|e| LyraError::Decode(e.to_string()))?
    } else {
        payload.to_vec()
    };
    serde_json::from_slice(&json).map_err(|e| LyraError::Decode(e.to_string()))
}

/// Test/debug helper: the payload as parsed JSON value.
pub fn decode_value(bytes: &[u8]) -> Result<serde_json::Value, LyraError> {
    if bytes.len() < HEADER_LEN || &bytes[..4] != MAGIC {
        return Err(header_error("bad magic"));
    }
    let flags = u16::from_le_bytes(bytes[6..8].try_into().unwrap());
    let len = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) as usize;
    let payload = bytes
        .get(HEADER_LEN..HEADER_LEN + len)
        .ok_or_else(|| header_error("truncated payload"))?;
    let json = if flags & FLAG_ZSTD != 0 {
        zstd::decode_all(payload).map_err(|e| LyraError::Decode(e.to_string()))?
    } else {
        payload.to_vec()
    };
    serde_json::from_slice(&json).map_err(|e| LyraError::Decode(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::*;

    fn sample_map() -> SongMap {
        let mut m = SongMap::new("abc123".into(), 237.5);
        m.grid.beats = vec![
            BeatPt { t_s: 0.5, conf: 0.9 },
            BeatPt { t_s: 1.0, conf: 0.8 },
        ];
        m.grid.downbeats = vec![0];
        m.notes.push(NoteEvent {
            onset_s: 0.5,
            offset_s: 0.9,
            raw_onset_s: 0.51,
            grid: GridPos { bar: 0, beat: 0, tick: 0 },
            midi: 69.0,
            bend_cents: None,
            techniques: TechFlags(0),
            conf: 0.8,
            source: Provenance::Mix,
            pos: Some(Fretting { string: 1, fret: 5 }),
            sf_conf: Some(0.7),
            layer_mask: 0b11111,
            suspect: false,
        });
        m
    }

    #[test]
    fn roundtrip_compressed_and_raw() {
        for compressed in [true, false] {
            let codec = MapCodec { compressed };
            let mut m = sample_map();
            let bytes = codec.encode(&mut m).unwrap();
            assert_eq!(&bytes[..4], b"LYRM");
            assert!(!m.map_id.is_empty());
            let back = decode(&bytes).unwrap();
            assert_eq!(back.map_id, m.map_id);
            assert_eq!(back.audio_hash, "abc123");
            assert_eq!(back.notes.len(), 1);
            assert_eq!(back.notes[0].midi, 69.0);
            assert_eq!(back.grid.beats.len(), 2);
        }
    }

    #[test]
    fn rejects_bad_magic_version_and_truncation() {
        let mut m = sample_map();
        let bytes = encode(&mut m).unwrap();
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(decode(&bad).is_err());
        let mut bad = bytes.clone();
        bad[4] = 0x2; // version 2
        assert!(decode(&bad).is_err());
        assert!(decode(&bytes[..10]).is_err());
        let mut bad = bytes.clone();
        let n = bad.len();
        bad.truncate(n - 1); // payload_len now exceeds file
        assert!(decode(&bad).is_err());
    }

    #[test]
    fn zstd_actually_compresses_repetitive_maps() {
        let mut m = sample_map();
        for i in 0..500 {
            let mut n = m.notes[0].clone();
            n.onset_s += i as f32 * 0.5;
            m.notes.push(n);
        }
        let raw = MapCodec { compressed: false }.encode(&mut m.clone()).unwrap();
        let z = MapCodec { compressed: true }.encode(&mut m).unwrap();
        assert!(z.len() < raw.len() / 2, "z {} vs raw {}", z.len(), raw.len());
    }
}
