//! lyra-formats: probing, stream info, decoding, tag I/O.
//!
//! Memory-safe by construction — Symphonia for decode, lofty for tags.
//! Where a format has no mature Rust codec (DSD, APE, WavPack today), the
//! plan is a thin C shim behind the same traits, optionally isolated in an
//! XPC worker. See BLUEPRINT.md § formats.

use lyra_core::{AudioFormat, LyraError, StreamInfo, TagMap};
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
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
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
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::io::MediaSourceStream;

    let format = probe(path);
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
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
        codec: format!("{:?}", params.codec),
        sample_rate: params.sample_rate,
        channels: params.channels.as_ref().map(|c| c.count() as u32),
        bits_per_sample: params.bits_per_sample,
        duration_secs,
    })
}

/// Read tags via lofty — the Rust-native TagLib replacement.
pub fn read_tags(path: &Path) -> Result<TagMap, LyraError> {
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

    Ok(TagMap {
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
    })
}
