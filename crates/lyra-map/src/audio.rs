//! Offline decode + resample (§4.1 head): `TrackDecoder` over any
//! `ByteSource` → the two analysis buses — mono f32 @ 22050 Hz
//! (basic-pitch input) and the 44100 Hz stereo bus (stems, chroma).
//!
//! Resampling is plain linear interpolation: analysis-grade, dependency-free,
//! and deterministic. (The RT path's rubato SRC stays where it belongs —
//! this crate never touches the playback chain.)

use lyra_core::LyraError;
use lyra_formats::TrackDecoder;
use lyra_fs::{ByteSource, SourceMediaSource};
use std::sync::Arc;

/// Target rates for the two analysis buses.
pub const NOTE_RATE: u32 = 22_050;
pub const CHROMA_RATE: u32 = 44_100;

/// Fully decoded analysis input. A 4-minute stereo song is ~40 MB here —
/// fine for an offline batch stage, never for the RT path.
pub struct AudioBuses {
    /// Mono mix at 22050 Hz — note-transcription input.
    pub mono_22050: Vec<f32>,
    /// Interleaved stereo at 44100 Hz — chroma/stem input.
    pub stereo_44100: Vec<f32>,
    pub duration_s: f32,
    pub src_rate: u32,
    pub src_channels: usize,
}

/// "Decode whole file to mono f32 fast" — the helper §4.6 asks for.
/// Lives here (not in lyra-formats) so the decoder stays playback-agnostic.
pub struct OfflineDecode;

impl OfflineDecode {
    pub fn decode(
        source: Arc<dyn ByteSource>,
        extension_hint: Option<&str>,
    ) -> Result<AudioBuses, LyraError> {
        let media = SourceMediaSource::new(source);
        let mut dec = TrackDecoder::open(media, extension_hint)?;
        let src_rate = dec.sample_rate;
        let src_channels = dec.channels;
        if src_rate == 0 || src_channels == 0 {
            return Err(LyraError::Decode("empty stream info".into()));
        }

        let mut interleaved = Vec::new();
        while let Some(block) = dec.next_block()? {
            interleaved.extend_from_slice(&block);
        }
        if interleaved.is_empty() {
            return Err(LyraError::Decode("no audio frames decoded".into()));
        }
        let frames = interleaved.len() / src_channels;
        let duration_s = frames as f32 / src_rate as f32;

        let mono = mixdown_mono(&interleaved, src_channels);
        let (mono_22050, _) = resample_linear(&mono, src_rate, NOTE_RATE, 1);
        let (stereo_44100, _) = if src_channels == 2 {
            resample_linear(&interleaved, src_rate, CHROMA_RATE, 2)
        } else {
            // Upmix anything non-stereo to stereo so downstream stages see
            // one bus shape: mono→dup, >2ch→L/R fold-down.
            let stereo = to_stereo(&interleaved, src_channels);
            resample_linear(&stereo, src_rate, CHROMA_RATE, 2)
        };

        Ok(AudioBuses {
            mono_22050,
            stereo_44100,
            duration_s,
            src_rate,
            src_channels,
        })
    }
}

/// BLAKE3 of the source bytes — the content-addressed join key (§4.5):
/// the same recording shares one map across local/torrent/remote copies.
/// Streams in 1 MiB reads; a short read ends the hash.
pub fn hash_source(source: &dyn ByteSource) -> Result<String, LyraError> {
    let mut hasher = blake3::Hasher::new();
    let mut offset = 0u64;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = source.read_at(offset, &mut buf).map_err(LyraError::Io)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        offset += n as u64;
        if offset >= source.len() {
            break;
        }
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Average channels → mono. Silent-channel-safe (divides by channel count,
/// not by active count — a dead right channel halves level, honestly).
pub fn mixdown_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|f| f.iter().sum::<f32>() / channels as f32)
        .collect()
}

fn to_stereo(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels == 2 {
        return interleaved.to_vec();
    }
    let mut out = Vec::with_capacity(interleaved.len() * 2 / channels.max(1));
    for f in interleaved.chunks_exact(channels) {
        let (l, r) = if channels == 1 {
            (f[0], f[0])
        } else {
            // >2ch fold-down: odd channels → L, even → R.
            let mut l = 0.0;
            let mut r = 0.0;
            for (i, s) in f.iter().enumerate() {
                if i % 2 == 0 {
                    l += *s;
                } else {
                    r += *s;
                }
            }
            let n = (channels as f32 / 2.0).ceil().max(1.0);
            (l / n, r / n)
        };
        out.push(l);
        out.push(r);
    }
    out
}

/// Per-channel linear resample. `input` is interleaved with `channels`
/// channels at `src_rate`; output is interleaved at `dst_rate`.
pub fn resample_linear(
    input: &[f32],
    src_rate: u32,
    dst_rate: u32,
    channels: usize,
) -> (Vec<f32>, usize) {
    if src_rate == dst_rate || input.is_empty() {
        return (input.to_vec(), channels);
    }
    let in_frames = input.len() / channels.max(1);
    let out_frames = ((in_frames as u64 * dst_rate as u64) / src_rate as u64) as usize;
    let mut out = vec![0.0f32; out_frames * channels];
    let step = src_rate as f64 / dst_rate as f64;
    for ch in 0..channels {
        for i in 0..out_frames {
            let pos = i as f64 * step;
            let j = pos.floor() as usize;
            let frac = (pos - j as f64) as f32;
            let a = input.get(j * channels + ch).copied().unwrap_or(0.0);
            let b = input.get((j + 1) * channels + ch).copied().unwrap_or(a);
            out[i * channels + ch] = a + frac * (b - a);
        }
    }
    (out, channels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn wav_bytes(rate: u32, channels: u16, secs: f32, freq: f32) -> Vec<u8> {
        let n = (rate as f32 * secs) as u32;
        let data_len = n * channels as u32 * 2;
        let mut b = Vec::with_capacity(44 + data_len as usize);
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
        b.extend_from_slice(&(channels * 2).to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..n {
            let s = (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin();
            let v = ((s * 32767.0) as i16).to_le_bytes();
            for _ in 0..channels {
                b.extend_from_slice(&v);
            }
        }
        b
    }

    struct MemSource {
        data: Vec<u8>,
        desc: String,
    }
    impl ByteSource for MemSource {
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
            let off = offset as usize;
            if off >= self.data.len() {
                return Ok(0);
            }
            let n = (self.data.len() - off).min(buf.len());
            buf[..n].copy_from_slice(&self.data[off..off + n]);
            Ok(n)
        }
        fn len(&self) -> u64 {
            self.data.len() as u64
        }
        fn describe(&self) -> String {
            self.desc.clone()
        }
    }

    fn mem_wav(rate: u32, ch: u16) -> Arc<MemSource> {
        Arc::new(MemSource {
            data: wav_bytes(rate, ch, 1.0, 440.0),
            desc: "mem".into(),
        })
    }

    #[test]
    fn decodes_to_both_buses() {
        let buses = OfflineDecode::decode(mem_wav(44_100, 2), Some("wav")).unwrap();
        assert!((buses.duration_s - 1.0).abs() < 0.05);
        assert_eq!(buses.mono_22050.len(), NOTE_RATE as usize, "1 s @ 22050");
        assert_eq!(buses.stereo_44100.len(), CHROMA_RATE as usize * 2);
        let peak = buses.mono_22050.iter().fold(0f32, |a, s| a.max(s.abs()));
        assert!(peak > 0.9, "sine near full scale, got {peak}");
    }

    #[test]
    fn mono_upmix_and_odd_rate_resample() {
        // 8 kHz mono → exercises both the upmix and a non-integer ratio.
        let buses = OfflineDecode::decode(mem_wav(8_000, 1), Some("wav")).unwrap();
        assert_eq!(buses.mono_22050.len(), NOTE_RATE as usize);
        assert_eq!(buses.stereo_44100.len(), CHROMA_RATE as usize * 2);
        // Upmixed stereo channels must agree for mono input.
        assert!((buses.stereo_44100[0] - buses.stereo_44100[1]).abs() < 1e-5);
    }

    #[test]
    fn hash_is_stable_and_content_addressed() {
        let a = mem_wav(44_100, 2);
        let b = mem_wav(44_100, 2);
        assert_eq!(
            hash_source(a.as_ref()).unwrap(),
            hash_source(b.as_ref()).unwrap()
        );
        let mut other = wav_bytes(44_100, 2, 1.0, 880.0);
        other.push(0);
        let c = Arc::new(MemSource {
            data: other,
            desc: "x".into(),
        });
        assert_ne!(
            hash_source(a.as_ref()).unwrap(),
            hash_source(c.as_ref()).unwrap()
        );
    }

    #[test]
    fn resample_identity_and_length_math() {
        let x: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let (same, _) = resample_linear(&x, 44100, 44100, 1);
        assert_eq!(same, x);
        let (up, _) = resample_linear(&x, 22050, 44100, 1);
        assert_eq!(up.len(), 200);
        assert!((up[0] - 0.0).abs() < 1e-6 && (up[198] - 99.0).abs() < 1e-3);
    }

    #[test]
    fn decode_roundtrip_through_temp_file() {
        // Independence from the in-memory path: real file on disk.
        let p = std::env::temp_dir().join("lyra_map_decode_test.wav");
        std::fs::File::create(&p)
            .unwrap()
            .write_all(&wav_bytes(48_000, 2, 0.5, 220.0))
            .unwrap();
        let buses =
            OfflineDecode::decode(Arc::new(lyra_fs::LocalFile::open(&p).unwrap()), Some("wav"))
                .unwrap();
        assert!((buses.duration_s - 0.5).abs() < 0.05);
        let _ = std::fs::remove_file(&p);
    }
}
