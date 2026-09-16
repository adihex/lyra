//! lyra-formats: probing, stream info, decoding, tag I/O.
//!
//! Memory-safe by construction — Symphonia for decode, lofty for tags.
//! Where a format has no mature Rust codec (DSD, APE, WavPack today), the
//! plan is a thin C shim behind the same traits, optionally isolated in an
//! XPC worker. See BLUEPRINT.md § formats.

use lyra_core::{AudioFormat, LibraryTrack, LyraError, StreamInfo, TagMap};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Probe a file's container format. Magic bytes first, extension fallback —
/// BitMuse's changelog shows extension-trusting caused their M4A/AAC bug.
pub fn probe(path: &Path) -> AudioFormat {
    if let Ok(fmt) = probe_magic(path) {
        return fmt;
    }
    probe_ext(path)
}

fn probe_magic(path: &Path) -> Result<AudioFormat, LyraError> {
    let mut buf = [0u8; 12];
    let mut f = File::open(path)?;
    let n = f.read(&mut buf)?;
    let b = &buf[..n];
    Ok(match b {
        [b'f', b'L', b'a', b'C', ..] => AudioFormat::Flac,
        [b'D', b'S', b'D', b' ', ..] => AudioFormat::Dsf,
        [b'F', b'R', b'M', b'8', ..] => AudioFormat::Dff,
        [b'F', b'O', b'R', b'M', ..] => AudioFormat::Aiff,
        [b'R', b'I', b'F', b'F', ..] => AudioFormat::Wav,
        [b'O', b'g', b'g', b'S', ..] => AudioFormat::Ogg,
        [b'M', b'A', b'C', b' ', ..] => AudioFormat::Ape,      // Monkey's Audio
        [b'w', b'v', b'p', b'k', ..] => AudioFormat::WavPack,
        [_, _, _, _, b'f', b't', b'y', b'p', ..] => AudioFormat::M4a,
        [b'I', b'D', b'3', ..] => AudioFormat::Mp3,
        _ => return Err(LyraError::UnsupportedFormat("no magic".into())),
    })
}

fn probe_ext(path: &Path) -> AudioFormat {
    format_from_ext(
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default(),
    )
}

/// Extension → container guess (no magic sniff — for stream sources).
pub fn format_from_ext(ext: &str) -> AudioFormat {
    match ext.to_ascii_lowercase().as_str() {
        "flac" => AudioFormat::Flac,
        "aiff" | "aif" | "aifc" => AudioFormat::Aiff,
        "wav" | "wave" => AudioFormat::Wav,
        "m4a" | "mp4" | "alac" | "aac" => AudioFormat::M4a,
        "mp3" => AudioFormat::Mp3,
        "ogg" | "oga" | "opus" => AudioFormat::Ogg,
        "dsf" => AudioFormat::Dsf,
        "dff" => AudioFormat::Dff,
        "ape" => AudioFormat::Ape,
        "wv" => AudioFormat::WavPack,
        "cue" => AudioFormat::Cue,
        _ => AudioFormat::Unknown,
    }
}

/// Container/codec info without decoding — feeds the library scanner.
pub fn stream_info(path: &Path) -> Result<StreamInfo, LyraError> {
    let format = probe(path);
    let file = File::open(path)?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    stream_info_media(file, format, ext)
}

/// Same probe over any `MediaSource` — torrent files, SSH streams, caches.
/// `format` should come from `probe_ext`/`format_from_ext` (no path here).
pub fn stream_info_media(
    source: impl symphonia::core::io::MediaSource + 'static,
    format: AudioFormat,
    ext: &str,
) -> Result<StreamInfo, LyraError> {
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::io::MediaSourceStream;

    let mss = MediaSourceStream::new(Box::new(source), Default::default());
    let mut hint = Hint::new();
    if !ext.is_empty() {
        hint.with_extension(ext);
    }

    // Symphonia 0.6: Probe::probe returns the FormatReader directly.
    let reader = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            symphonia::core::formats::FormatOptions::default(),
            symphonia::core::meta::MetadataOptions::default(),
        )
        .map_err(|e| LyraError::Decode(e.to_string()))?;

    let track = reader
        .default_track(symphonia::core::formats::TrackType::Audio)
        .ok_or_else(|| LyraError::Decode("no default track".into()))?;

    let params = match track.codec_params.as_ref() {
        Some(symphonia::core::codecs::CodecParameters::Audio(a)) => a,
        _ => return Err(LyraError::Decode("no audio codec params".into())),
    };

    let duration_secs = track
        .num_frames
        .zip(track.time_base)
        .and_then(|(f, tb)| tb.calc_duration(f.into()))
        .map(|t| t.as_nanos() as f64 / 1e9);

    Ok(StreamInfo {
        format,
        codec: codec_name(params.codec),
        sample_rate: params.sample_rate,
        channels: params.channels.as_ref().map(|c| c.count() as u32),
        bits_per_sample: params.bits_per_sample,
        duration_secs,
    })
}

/// Human-readable codec name (AudioCodecId debug is `AudioCodecId(266)` —
/// useless in a UI column).
fn codec_name(id: symphonia::core::codecs::audio::AudioCodecId) -> String {
    use symphonia::core::codecs::audio::well_known as C;
    match id {
        C::CODEC_ID_FLAC => "FLAC",
        C::CODEC_ID_MP3 => "MP3",
        C::CODEC_ID_AAC => "AAC",
        C::CODEC_ID_ALAC => "ALAC",
        C::CODEC_ID_VORBIS => "Vorbis",
        C::CODEC_ID_OPUS => "Opus",
        C::CODEC_ID_WAVPACK => "WavPack",
        C::CODEC_ID_MONKEYS_AUDIO => "APE",
        C::CODEC_ID_MUSEPACK => "Musepack",
        C::CODEC_ID_DCA => "DTS",
        C::CODEC_ID_TRUEHD => "TrueHD",
        // common PCM variants (the PCM id space is a whole range)
        C::CODEC_ID_PCM_S16LE | C::CODEC_ID_PCM_S16BE | C::CODEC_ID_PCM_S24LE
        | C::CODEC_ID_PCM_S24BE | C::CODEC_ID_PCM_S32LE | C::CODEC_ID_PCM_S32BE
        | C::CODEC_ID_PCM_F32LE | C::CODEC_ID_PCM_F32BE | C::CODEC_ID_PCM_U8
            => "PCM",
        _ => return format!("{id}"), // Display renders the hex codec id
    }
    .into()
}

/// Live decoder: wraps a `MediaSource` (local file, SSH stream, torrent
/// stream — anything) into "give me interleaved f32 blocks". The engine
/// drives this; Symphonia owns the demux/decode.
pub struct TrackDecoder {
    reader: Box<dyn symphonia::core::formats::FormatReader + 'static>,
    decoder: Box<dyn symphonia::core::codecs::audio::AudioDecoder>,
    track_id: u32,
    pub sample_rate: u32,
    pub channels: usize,
}

impl TrackDecoder {
    /// Probe + build a decoder for the default audio track.
    pub fn open(
        source: impl symphonia::core::io::MediaSource + 'static,
        extension_hint: Option<&str>,
    ) -> Result<Self, LyraError> {
        use symphonia::core::formats::probe::Hint;
        use symphonia::core::io::MediaSourceStream;

        let mss = MediaSourceStream::new(Box::new(source), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = extension_hint {
            hint.with_extension(ext);
        }
        let reader = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                symphonia::core::formats::FormatOptions::default(),
                symphonia::core::meta::MetadataOptions::default(),
            )
            .map_err(|e| LyraError::Decode(e.to_string()))?;

        let track = reader
            .default_track(symphonia::core::formats::TrackType::Audio)
            .ok_or_else(|| LyraError::Decode("no audio track".into()))?;
        let track_id = track.id;
        let params = match track.codec_params.as_ref() {
            Some(symphonia::core::codecs::CodecParameters::Audio(a)) => a,
            _ => return Err(LyraError::Decode("no audio codec params".into())),
        };
        let sample_rate = params
            .sample_rate
            .ok_or_else(|| LyraError::Decode("no sample rate".into()))?;
        let channels = params
            .channels
            .as_ref()
            .map(|c| c.count())
            .unwrap_or(2);

        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(
                params,
                &symphonia::core::codecs::audio::AudioDecoderOptions::default(),
            )
            .map_err(|e| LyraError::Decode(e.to_string()))?;

        Ok(Self { reader, decoder, track_id, sample_rate, channels })
    }

    /// Next decoded block as interleaved f32 (native channel count).
    /// `Ok(None)` = clean EOF. Corrupt packets are skipped, not fatal.
    pub fn next_block(&mut self) -> Result<Option<Vec<f32>>, LyraError> {
        loop {
            let packet = match self.reader.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => return Ok(None),
                Err(e) => return Err(LyraError::Decode(e.to_string())),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(buf) => {
                    let frames = buf.frames();
                    let ch = buf.spec().channels().count();
                    let mut out = vec![0f32; frames * ch];
                    buf.copy_to_slice_interleaved(&mut out);
                    return Ok(Some(out));
                }
                Err(_) => continue, // skip corrupt packets — never fatal to playback
            }
        }
    }

    /// Seek to seconds. Resets the decoder state machine.
    pub fn seek(&mut self, seconds: f64) -> Result<(), LyraError> {
        let time = symphonia::core::units::Time::try_from_secs_f64(seconds)
            .ok_or_else(|| LyraError::Decode("bad seek time".into()))?;
        self.reader
            .seek(
                symphonia::core::formats::SeekMode::Accurate,
                symphonia::core::formats::SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| LyraError::Decode(e.to_string()))?;
        self.decoder.reset();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Minimal PCM16 WAV bytes: mono, `rate` Hz, `dur_s` seconds of a sine.
    fn wav_bytes(rate: u32, dur_s: f32, freq: f32) -> Vec<u8> {
        let n = (rate as f32 * dur_s) as u32;
        let data_len = n * 2;
        let mut b = Vec::with_capacity(44 + data_len as usize);
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes()); // PCM
        b.extend_from_slice(&1u16.to_le_bytes()); // mono
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * 2).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..n {
            let s = (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin();
            b.extend_from_slice(&((s * 32767.0) as i16).to_le_bytes());
        }
        b
    }

    #[test]
    fn decodes_wav_end_to_end() {
        let rate = 44_100u32;
        let path = std::env::temp_dir().join("lyra_test_tone.wav");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&wav_bytes(rate, 1.0, 440.0))
            .unwrap();

        let mut d = TrackDecoder::open(std::fs::File::open(&path).unwrap(), Some("wav")).unwrap();
        assert_eq!(d.sample_rate, rate);
        assert_eq!(d.channels, 1);

        let mut total = 0usize;
        let mut peak = 0f32;
        while let Some(block) = d.next_block().unwrap() {
            peak = peak.max(block.iter().fold(0f32, |a, s| a.max(s.abs())));
            total += block.len();
        }
        assert_eq!(total, rate as usize); // exactly 1s of mono frames
        assert!(peak > 0.9, "sine should decode near full scale, got {peak}");

        // seek back to 0 and confirm re-decode works
        d.seek(0.0).unwrap();
        assert!(d.next_block().unwrap().is_some());
    }

    #[test]
    fn scan_real_library_dir() {
        // Present on the dev machine (generated by `say`+afconvert).
        let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap())
            .join("Music/Lyra-Test");
        if !dir.exists() {
            return;
        }
        let tracks = scan_dir(&dir).unwrap();
        assert_eq!(tracks.len(), 3);
        for t in &tracks {
            eprintln!("{} — {} ({:?}, {:?}s)", t.path, t.codec, t.format, t.duration_secs);
        }
        assert!(tracks.iter().all(|t| t.duration_secs.unwrap_or(0.0) > 0.0));
    }
}

/// Scan a folder recursively → LibraryTrack rows. v1: synchronous walk +
/// per-file probe/tags — fast enough for the scaffold; the real scanner
/// adds incremental mtime diffing + remote find-mode (BLUEPRINT §lyra-fs).
pub fn scan_dir(dir: &Path) -> Result<Vec<LibraryTrack>, LyraError> {
    let mut tracks = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            if !is_audio_ext(&ext) {
                continue;
            }
            tracks.push(probe_track(&p));
        }
    }
    tracks.sort_by(|a, b| {
        (a.album.as_deref().unwrap_or(""), a.track_number.unwrap_or(0))
            .cmp(&(b.album.as_deref().unwrap_or(""), b.track_number.unwrap_or(0)))
    });
    Ok(tracks)
}

/// Probe one file → a library row (format probe + stream info + tags).
/// Single source for both scan_dir and the store's incremental sync.
pub fn probe_track(p: &Path) -> LibraryTrack {
    probe_track_full(p).0
}

/// probe_track plus the embedded cover bytes (one file open — the tag
/// parse is shared). Art extraction never fails the probe.
pub fn probe_track_full(p: &Path) -> (LibraryTrack, Option<EmbeddedArt>) {
    let format = probe(p);
    let stream = stream_info(p).ok();
    let (tags, art) = read_tagged(p).unwrap_or_default();
    (LibraryTrack {
        path: p.display().to_string(),
        title: tags.title,
        artist: tags.artist,
        album: tags.album,
        album_artist: tags.album_artist,
        genre: tags.genre,
        year: tags.year,
        track_number: tags.track_number,
        duration_secs: stream.as_ref().and_then(|s| s.duration_secs),
        codec: stream.as_ref().map(|s| s.codec.clone()).unwrap_or_default(),
        sample_rate: stream.as_ref().and_then(|s| s.sample_rate),
        channels: stream.as_ref().and_then(|s| s.channels),
        bits_per_sample: stream.as_ref().and_then(|s| s.bits_per_sample),
        format: stream.map(|s| s.format).unwrap_or(format),
        artwork_hash: None, // set by the store once bytes are cached
    }, art)
}

/// Extensions the scanner considers audio candidates.
pub fn is_audio_ext(ext: &str) -> bool {
    matches!(
        ext,
        "flac" | "wav" | "aiff" | "aif" | "m4a" | "mp4" | "mp3" | "ogg"
            | "opus" | "dsf" | "dff" | "ape" | "wv" | "wvc"
    )
}

/// Embedded cover bytes, sniffed — declared MIME lies in the wild.
pub struct EmbeddedArt {
    pub data: Vec<u8>,
    pub mime: &'static str,
}

/// Magic-byte sniff → canonical mime; None means "not a usable cover"
/// (also catches ID3v2 APICs whose data is a `-->` URL, not pixels).
fn sniff_mime(d: &[u8]) -> Option<&'static str> {
    Some(match d {
        [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [b'R', b'I', b'F', b'F', ..] => "image/webp", // WEBP rides RIFF
        [0x42, 0x4D, ..] => "image/bmp",
        _ => return None,
    })
}

/// Cover pick: front cover first, else any picture with real image bytes.
/// Icons (32×32 junk) and non-image payloads are skipped.
fn pick_cover(pics: &[lofty::picture::Picture]) -> Option<EmbeddedArt> {
    use lofty::picture::PictureType;
    let usable = |p: &lofty::picture::Picture| {
        !matches!(p.pic_type(), PictureType::Icon | PictureType::OtherIcon)
            && sniff_mime(p.data()).is_some()
    };
    let pic = pics
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront && usable(p))
        .or_else(|| pics.iter().find(|p| usable(p)))?;
    Some(EmbeddedArt {
        mime: sniff_mime(pic.data())?,
        data: pic.data().to_vec(),
    })
}

/// Read tags via lofty — the Rust-native TagLib replacement.
pub fn read_tags(path: &Path) -> Result<TagMap, LyraError> {
    read_tagged(path).map(|(t, _)| t)
}

/// Just the embedded cover — art-only re-checks on rows that predate
/// artwork support (no stream probe).
pub fn read_art(path: &Path) -> Option<EmbeddedArt> {
    read_tagged(path).ok()?.1
}

/// Tags + embedded art in a single file open. Art failures never fail
/// the tag read — a corrupt APIC must not drop the title.
fn read_tagged(path: &Path) -> Result<(TagMap, Option<EmbeddedArt>), LyraError> {
    use lofty::prelude::{Accessor, TaggedFileExt};

    let tagged = lofty::probe::Probe::open(path)
        .map_err(|e| LyraError::Tag(e.to_string()))?
        .guess_file_type()
        .map_err(|e| LyraError::Tag(e.to_string()))?
        .read()
        .map_err(|e| LyraError::Tag(e.to_string()))?;

    let tag = tagged
        .primary_tag()
        .or_else(|| tagged.first_tag())
        .ok_or_else(|| LyraError::Tag("no tags".into()))?;

    let art = pick_cover(tag.pictures());
    Ok((TagMap {
        title: tag.title().map(|s| s.into_owned()),
        artist: tag.artist().map(|s| s.into_owned()),
        album: tag.album().map(|s| s.into_owned()),
        album_artist: tag
            .get_string(lofty::tag::ItemKey::AlbumArtist)
            .map(String::from),
        genre: tag.genre().map(|s| s.into_owned()),
        year: tag
            .get_string(lofty::tag::ItemKey::Year)
            .and_then(|s| s.parse().ok()),
        track_number: tag.track(),
    }, art))
}
