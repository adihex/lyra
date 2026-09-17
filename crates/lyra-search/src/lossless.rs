//! Lossless classification — the one place format decisions live.
//! Inputs are either filenames ("gd77d1t01.flac") or provider format
//! labels ("24bit Flac", "VBR MP3"); both funnel through `classify`.

use crate::SearchQuery;

/// Codec ids Lyra treats as lossless — the default query family.
/// shn/tta are legacy-etree codecs; both genuinely lossless.
pub const FAMILY: &[&str] = &["flac", "alac", "wav", "aiff", "wv", "ape", "tta", "shn"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioClass {
    pub codec: &'static str,
    pub lossless: bool,
    pub bit_depth: Option<u32>,
    pub sample_rate: Option<u32>,
}

const fn class(codec: &'static str, lossless: bool) -> AudioClass {
    AudioClass { codec, lossless, bit_depth: None, sample_rate: None }
}

/// Classify a filename or format label → audio codec. None = not audio
/// (or unrecognized — callers decide whether that's rejectable).
pub fn classify(label: &str) -> Option<AudioClass> {
    let l = label.trim().to_lowercase();
    if l.is_empty() {
        return None;
    }
    if l.contains('.') {
        // Filename — decide by extension only; substrings in names lie
        // ("gd77.flac16_archive.torrent" contains "flac").
        return l.rsplit('.').next().and_then(by_ext).map(|c| with_depth(c, &l));
    }
    by_label(&l).map(|c| with_depth(c, &l))
}

fn by_ext(ext: &str) -> Option<AudioClass> {
    Some(match ext {
        "flac" => class("flac", true),
        "wav" => class("wav", true),
        "aif" | "aiff" | "aifc" => class("aiff", true),
        "wv" => class("wv", true),
        "ape" => class("ape", true),
        "tta" => class("tta", true),
        "shn" => class("shn", true),
        // m4a container: ALAC vs AAC indistinguishable by ext — lossy.
        "m4a" => class("m4a", false),
        "mp3" => class("mp3", false),
        "ogg" | "oga" => class("ogg", false),
        "opus" => class("opus", false),
        "aac" => class("aac", false),
        "wma" => class("wma", false),
        _ => return None,
    })
}

/// Classify a free-text label that isn't a filename — torrent titles,
/// provider tags. Skips the filename dot-branch so "R.A.M. [FLAC]"
/// still matches flac.
pub(crate) fn classify_title(label: &str) -> Option<AudioClass> {
    let l = label.trim().to_lowercase();
    if l.is_empty() {
        return None;
    }
    by_label(&l).map(|c| with_depth(c, &l))
}

/// Provider format strings (IA "format" field values).
fn by_label(l: &str) -> Option<AudioClass> {
    Some(match l {
        // "Archive BitTorrent" contains "tta" — exclude before audio match.
        s if s.contains("bittorrent") || s.contains("bit torrent") => return None,
        s if s.contains("flac") => class("flac", true),
        s if s.contains("apple lossless") || s.contains("alac") => class("alac", true),
        s if s.contains("wavpack") => class("wv", true),
        s if s.contains("monkey") => class("ape", true),
        s if s.contains("wave") || s == "wav" => class("wav", true),
        s if s.contains("aiff") => class("aiff", true),
        s if s.contains("tta") => class("tta", true),
        s if s.contains("shorten") => class("shn", true),
        s if s.contains("mp3") => class("mp3", false),
        s if s.contains("vorbis") || s.contains("ogg") => class("ogg", false),
        s if s.contains("opus") => class("opus", false),
        s if s.contains("aac") || s.contains("m4a") => class("aac", false),
        s if s.contains("wma") => class("wma", false),
        _ => return None,
    })
}

/// Pull "24bit"/"96kHz" hints out of labels like "24bit Flac".
fn with_depth(mut c: AudioClass, l: &str) -> AudioClass {
    for tok in l.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '.') {
        if let Some(b) = tok.strip_suffix("bit") {
            if let Ok(v) = b.parse::<u32>() {
                c.bit_depth = Some(v);
            }
        }
        if let Some(k) = tok.strip_suffix("khz") {
            if let Ok(v) = k.parse::<f32>() {
                c.sample_rate = Some((v * 1000.0) as u32);
            }
        }
    }
    c
}

/// The codec set a query accepts: explicit `formats` or FAMILY.
pub fn acceptable(q: &SearchQuery) -> Vec<&str> {
    if q.formats.is_empty() {
        FAMILY.to_vec()
    } else {
        q.formats.iter().map(String::as_str).collect()
    }
}

/// Fold per-file classes into result fields. `None` classes (non-audio
/// files) are skipped by the caller. Empty input ⇒ Some(false):
/// a known file list with no lossless audio is verified-lossy.
pub fn summarize<'a>(
    classes: impl Iterator<Item = &'a AudioClass>,
) -> (Vec<String>, Option<bool>, Option<u32>, Option<u32>) {
    let mut formats: Vec<String> = Vec::new();
    let (mut lossless, mut depth, mut rate) = (false, None, None);
    for c in classes {
        if !formats.iter().any(|f| f == c.codec) {
            formats.push(c.codec.to_string());
        }
        if c.lossless {
            lossless = true;
            depth = depth.max(c.bit_depth);
            rate = rate.max(c.sample_rate);
        }
    }
    (formats, Some(lossless), depth, rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_lossless_family() {
        for (input, codec) in [
            ("gd1977-05-08d1t01.flac", "flac"),
            ("show.WAV", "wav"),
            ("x.aiff", "aiff"),
            ("x.wv", "wv"),
            ("x.ape", "ape"),
            ("x.tta", "tta"),
            ("gd73.shn", "shn"),
            ("Flac", "flac"),
            ("24bit Flac", "flac"),
            ("Apple Lossless", "alac"),
            ("WAVE", "wav"),
            ("Monkey's Audio", "ape"),
            ("Shorten", "shn"),
        ] {
            let c = classify(input).unwrap_or_else(|| panic!("{input}"));
            assert_eq!(c.codec, codec, "{input}");
            assert!(c.lossless, "{input}");
        }
    }

    #[test]
    fn classify_lossy_and_non_audio() {
        for (input, codec) in [
            ("x.mp3", "mp3"),
            ("VBR MP3", "mp3"),
            ("64Kbps MP3", "mp3"),
            ("x.ogg", "ogg"),
            ("Ogg Vorbis", "ogg"),
            ("x.opus", "opus"),
            ("x.m4a", "m4a"),
            ("x.aac", "aac"),
        ] {
            let c = classify(input).unwrap_or_else(|| panic!("{input}"));
            assert_eq!(c.codec, codec, "{input}");
            assert!(!c.lossless, "{input}");
        }
        for non in ["cover.jpg", "notes.txt", "checksums.md5", "f.fp", "x.torrent", "Metadata", "PNG"] {
            assert!(classify(non).is_none(), "{non}");
        }
    }

    #[test]
    fn bit_depth_from_label() {
        assert_eq!(classify("24bit Flac").unwrap().bit_depth, Some(24));
        assert_eq!(classify("16Bit FLAC").unwrap().bit_depth, Some(16));
        assert_eq!(classify("Flac").unwrap().bit_depth, None);
    }

    #[test]
    fn summarize_formats_and_lossless() {
        let mixed = [classify("a.flac").unwrap(), classify("a.mp3").unwrap()];
        let (formats, lossless, _, _) = summarize(mixed.iter());
        assert_eq!(formats, ["flac", "mp3"]);
        assert_eq!(lossless, Some(true));

        let lossy = [classify("a.mp3").unwrap(), classify("b.ogg").unwrap()];
        assert_eq!(summarize(lossy.iter()).1, Some(false));

        let none: [AudioClass; 0] = [];
        assert_eq!(summarize(none.iter()).1, Some(false));
    }

    #[test]
    fn acceptable_defaults_to_family() {
        let q = SearchQuery::text("x");
        assert_eq!(acceptable(&q), FAMILY.to_vec());
        let q2 = SearchQuery { formats: vec!["flac".into()], ..SearchQuery::text("x") };
        assert_eq!(acceptable(&q2), ["flac"]);
    }
}
