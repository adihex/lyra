//! Minimal feature-flag infrastructure.
//!
//! Each flag has a compiled-in default ([`KNOWN_FLAGS`]) that can be
//! overridden at runtime without rebuilding:
//!
//! 1. a JSON file (`{"extended_lossless": false}`) pointed to by the
//!    `LYRA_FEATURE_FILE` environment variable, then
//! 2. individual environment variables `LYRA_FEATURE_<NAME>` — e.g.
//!    `LYRA_FEATURE_CUE_SHEET_TRACKS=0` — which win over the file.
//!
//! [`FeatureFlags::load`] applies exactly that precedence
//! (defaults < file < env). Unknown flag names fail closed
//! ([`FeatureFlags::is_enabled`] returns `false`).
//!
//! The typed accessors ([`FeatureFlags::extended_lossless`],
//! [`FeatureFlags::cue_sheet_tracks`],
//! [`FeatureFlags::strict_format_validation`]) gate real behavior today
//! via [`classify_extension`] / [`resolve_format`]. Planned integration
//! points for future flags (engine decoder registry, library scanner,
//! remote protocol) should read through these accessors rather than
//! branching on raw strings, so a rollout is one default flip plus tests.

use std::collections::HashMap;
use std::path::Path;

use crate::AudioFormat;

/// (name, compiled-in default) for every known flag.
///
/// Environment override: `LYRA_FEATURE_<NAME.to_uppercase()>=1`.
const KNOWN_DEFAULTS: &[(&str, bool)] = &[
    // Niche lossless decoders (DSD, APE, WavPack). Disabling demotes
    // those extensions to `AudioFormat::Unknown` so scanners and the
    // engine skip files they cannot play cheaply.
    ("extended_lossless", true),
    // Cue sheets enumerate tracks without audio of their own. Disabling
    // demotes `.cue` to `Unknown` for importers that only want audio.
    ("cue_sheet_tracks", true),
    // Reject unrecognized extensions outright (`None`) instead of
    // surfacing them as `Unknown` for the caller to tolerate.
    ("strict_format_validation", false),
];

/// Prefix for per-flag environment overrides, e.g.
/// `LYRA_FEATURE_EXTENDED_LOSSLESS=0`.
pub const ENV_PREFIX: &str = "LYRA_FEATURE_";

/// Environment variable naming a JSON flag file, e.g.
/// `LYRA_FEATURE_FILE=/etc/lyra/flags.json`.
pub const FILE_ENV_VAR: &str = "LYRA_FEATURE_FILE";

/// Errors loading flags from a file or JSON text.
#[derive(Debug, thiserror::Error)]
pub enum FlagError {
    /// The flag file could not be read.
    #[error("read flag file {path}: {source}")]
    Io {
        /// File that could not be read.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// The flag file was not valid JSON / flag values.
    #[error("parse flag file {path}: {message}")]
    Parse {
        /// File that could not be parsed (`"<inline>"` for strings).
        path: String,
        /// What was wrong with it.
        message: String,
    },
}

/// Runtime feature flags: compiled-in defaults plus file/env overrides.
#[derive(Debug, Clone)]
pub struct FeatureFlags {
    flags: HashMap<String, bool>,
}

impl Default for FeatureFlags {
    /// Compiled-in defaults, no overrides applied.
    fn default() -> Self {
        Self::defaults()
    }
}

impl FeatureFlags {
    /// Flags at their compiled-in defaults.
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            flags: KNOWN_DEFAULTS
                .iter()
                .map(|(name, default)| ((*name).to_string(), *default))
                .collect(),
        }
    }

    /// Load with production precedence: defaults, then the JSON file named
    /// by `LYRA_FEATURE_FILE` (if set and present), then `LYRA_FEATURE_*`
    /// environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`FlagError`] if the file named by `LYRA_FEATURE_FILE`
    /// exists but cannot be read or parsed. A missing file is not an
    /// error — the deployment simply has no file overrides.
    pub fn load() -> Result<Self, FlagError> {
        let mut flags = Self::defaults();
        if let Some(path) = std::env::var_os(FILE_ENV_VAR) {
            let path = Path::new(&path);
            if path.exists() {
                flags.layer_file(path)?;
            }
        }
        flags.layer_env(ENV_PREFIX);
        Ok(flags)
    }

    /// Build from defaults plus `prefix_*` environment variables.
    ///
    /// The prefix exists so tests can use a unique namespace per case
    /// (process env is global); production code should prefer
    /// [`FeatureFlags::load`].
    #[must_use]
    pub fn from_env_with(prefix: &str) -> Self {
        let mut flags = Self::defaults();
        flags.layer_env(prefix);
        flags
    }

    /// Parse flags from a JSON object string, e.g.
    /// `{"extended_lossless": false}`.
    ///
    /// Unknown keys are ignored (forward compatibility); a non-boolean
    /// value for a known flag is an error.
    ///
    /// # Errors
    ///
    /// Returns [`FlagError::Parse`] if the text is not a JSON object or a
    /// known flag maps to a non-boolean.
    pub fn from_json_str(json: &str) -> Result<Self, FlagError> {
        let mut flags = Self::defaults();
        flags.layer_json_str(json, "<inline>")?;
        Ok(flags)
    }

    /// Parse flags from a JSON object file with the same rules as
    /// [`FeatureFlags::from_json_str`].
    ///
    /// # Errors
    ///
    /// Returns [`FlagError::Io`] if the file cannot be read and
    /// [`FlagError::Parse`] if it is not a JSON flag object.
    pub fn from_json_file(path: &Path) -> Result<Self, FlagError> {
        let text = std::fs::read_to_string(path).map_err(|source| FlagError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let mut flags = Self::defaults();
        flags.layer_json_str(&text, &path.display().to_string())?;
        Ok(flags)
    }

    /// Override one flag at runtime (tests, admin endpoints).
    pub fn set(&mut self, name: &str, enabled: bool) {
        self.flags.insert(name.to_string(), enabled);
    }

    /// `true` iff `name` is a known enabled flag. Unknown names fail
    /// closed (`false`) so a typo can never enable behavior.
    #[must_use]
    pub fn is_enabled(&self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }

    /// `true` iff `name` is a flag this build knows about.
    #[must_use]
    pub fn is_known(name: &str) -> bool {
        KNOWN_DEFAULTS.iter().any(|(known, _)| *known == name)
    }

    /// Niche lossless families (DSD/APE/WavPack) stay decodable.
    #[must_use]
    pub fn extended_lossless(&self) -> bool {
        self.is_enabled("extended_lossless")
    }

    /// Cue sheets enumerate as tracks.
    #[must_use]
    pub fn cue_sheet_tracks(&self) -> bool {
        self.is_enabled("cue_sheet_tracks")
    }

    /// Unrecognized extensions are rejected, not tolerated.
    #[must_use]
    pub fn strict_format_validation(&self) -> bool {
        self.is_enabled("strict_format_validation")
    }

    /// Apply `prefix_*` env vars over the current map. Values parse via
    /// [`parse_env_bool`]; unparseable values leave the default in place
    /// (a typo must not silently flip a rollout).
    fn layer_env(&mut self, prefix: &str) {
        for (name, _) in KNOWN_DEFAULTS {
            let var: String = format!("{prefix}{}", name.to_uppercase());
            if let Some(raw) = std::env::var_os(var) {
                if let Some(enabled) = parse_env_bool(&raw.to_string_lossy()) {
                    self.flags.insert((*name).to_string(), enabled);
                }
            }
        }
    }

    /// Apply a JSON flag file over the current map.
    fn layer_file(&mut self, path: &Path) -> Result<(), FlagError> {
        let text = std::fs::read_to_string(path).map_err(|source| FlagError::Io {
            path: path.display().to_string(),
            source,
        })?;
        self.layer_json_str(&text, &path.display().to_string())
    }

    /// Apply a JSON flag object over the current map.
    fn layer_json_str(&mut self, json: &str, origin: &str) -> Result<(), FlagError> {
        let parse_err = |message: String| FlagError::Parse {
            path: origin.to_string(),
            message,
        };
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|e| parse_err(e.to_string()))?;
        let object = value
            .as_object()
            .ok_or_else(|| parse_err("top level must be a JSON object".to_string()))?;
        for (key, value) in object {
            if !Self::is_known(key) {
                continue;
            }
            let enabled = value
                .as_bool()
                .ok_or_else(|| parse_err(format!("flag {key} must be a boolean")))?;
            self.flags.insert(key.clone(), enabled);
        }
        Ok(())
    }
}

/// Parse a human env-var boolean: `1/true/yes/on` → true,
/// `0/false/no/off` → false (case-insensitive, surrounding whitespace
/// ignored). Anything else → `None` (caller keeps the default).
#[must_use]
pub fn parse_env_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Classify a file extension under the given flags.
///
/// Pure [`AudioFormat::from_extension`] mapping, except flag-gated
/// families demote to [`AudioFormat::Unknown`] when disabled:
/// DSD/APE/WavPack need [`FeatureFlags::extended_lossless`], cue sheets
/// need [`FeatureFlags::cue_sheet_tracks`].
#[must_use]
pub fn classify_extension(ext: &str, flags: &FeatureFlags) -> AudioFormat {
    let format = AudioFormat::from_extension(ext);
    match format {
        AudioFormat::Dsf | AudioFormat::Dff | AudioFormat::Ape | AudioFormat::WavPack
            if !flags.extended_lossless() =>
        {
            AudioFormat::Unknown
        }
        AudioFormat::Cue if !flags.cue_sheet_tracks() => AudioFormat::Unknown,
        other => other,
    }
}

/// Resolve an extension to a playable format under the given flags.
///
/// Returns `None` when the extension is not usable: unknown extensions
/// under [`FeatureFlags::strict_format_validation`], or flag-disabled
/// families (which [`classify_extension`] already demotes to `Unknown`).
/// Callers that must reject unsupported files (import, remote upload)
/// use this; tolerant callers (library scan display) use
/// [`classify_extension`].
#[must_use]
pub fn resolve_format(ext: &str, flags: &FeatureFlags) -> Option<AudioFormat> {
    let format = classify_extension(ext, flags);
    if format == AudioFormat::Unknown && flags.strict_format_validation() {
        return None;
    }
    // A disabled family demotes to Unknown: reject it even when the
    // caller is not otherwise strict — the flag says "do not play".
    if format == AudioFormat::Unknown && AudioFormat::from_extension(ext) != AudioFormat::Unknown {
        return None;
    }
    Some(format)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique env prefix per test: process env is shared across parallel
    /// test threads, so each case gets its own namespace.
    fn unique_prefix(case: &str) -> String {
        format!(
            "LYRA_TEST_{}_{}_",
            case.to_uppercase(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        )
    }

    #[test]
    fn defaults_match_compiled_in_table() {
        let flags = FeatureFlags::defaults();
        assert!(flags.extended_lossless());
        assert!(flags.cue_sheet_tracks());
        assert!(!flags.strict_format_validation());
    }

    #[test]
    fn unknown_flag_names_fail_closed() {
        let flags = FeatureFlags::defaults();
        assert!(!flags.is_enabled("does_not_exist"));
        assert!(!FeatureFlags::is_known("does_not_exist"));
        assert!(FeatureFlags::is_known("extended_lossless"));
    }

    #[test]
    fn env_override_disables_a_default_on_flag() {
        let prefix = unique_prefix("env_off");
        std::env::set_var(format!("{prefix}EXTENDED_LOSSLESS"), "0");
        let flags = FeatureFlags::from_env_with(&prefix);
        assert!(!flags.extended_lossless());
        assert!(flags.cue_sheet_tracks());
    }

    #[test]
    fn env_override_enables_a_default_off_flag() {
        let prefix = unique_prefix("env_on");
        std::env::set_var(format!("{prefix}STRICT_FORMAT_VALIDATION"), "yes");
        let flags = FeatureFlags::from_env_with(&prefix);
        assert!(flags.strict_format_validation());
    }

    #[test]
    fn env_override_accepts_bool_spellings() {
        for (raw, expected) in [
            ("1", true),
            ("TRUE", true),
            (" yes ", true),
            ("On", true),
            ("0", false),
            ("False", false),
            ("no", false),
            ("OFF", false),
        ] {
            assert_eq!(parse_env_bool(raw), Some(expected), "{raw}");
        }
        assert_eq!(parse_env_bool("maybe"), None);
        assert_eq!(parse_env_bool(""), None);
    }

    #[test]
    fn unparseable_env_value_keeps_default() {
        let prefix = unique_prefix("env_bad");
        std::env::set_var(format!("{prefix}EXTENDED_LOSSLESS"), "sometimes");
        let flags = FeatureFlags::from_env_with(&prefix);
        assert!(flags.extended_lossless());
    }

    #[test]
    fn json_file_gates_behavior() {
        let flags = FeatureFlags::from_json_str(
            r#"{"extended_lossless": false, "strict_format_validation": true}"#,
        )
        .expect("valid flag json");
        assert!(!flags.extended_lossless());
        assert!(flags.strict_format_validation());
        // Untouched flags keep defaults.
        assert!(flags.cue_sheet_tracks());
    }

    #[test]
    fn json_ignores_unknown_keys_but_rejects_non_bools() {
        let flags = FeatureFlags::from_json_str(r#"{"future_flag": true}"#)
            .expect("unknown keys are forward-compatible");
        assert!(flags.extended_lossless());

        let err = FeatureFlags::from_json_str(r#"{"extended_lossless": "yes"}"#)
            .expect_err("non-bool must fail");
        assert!(matches!(err, FlagError::Parse { .. }), "{err:?}");

        assert!(matches!(
            FeatureFlags::from_json_str(r#"[true]"#).expect_err("array must fail"),
            FlagError::Parse { .. }
        ));
    }

    #[test]
    fn json_file_round_trip_from_disk() {
        let path = std::env::temp_dir().join(format!(
            "lyra-flags-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&path, r#"{"cue_sheet_tracks": false}"#).expect("write temp flags");
        let flags = FeatureFlags::from_json_file(&path).expect("read temp flags");
        assert!(!flags.cue_sheet_tracks());
        std::fs::remove_file(&path).expect("remove temp flags");

        assert!(matches!(
            FeatureFlags::from_json_file(&path).expect_err("missing file must fail"),
            FlagError::Io { .. }
        ));
    }

    #[test]
    fn disabled_extended_lossless_demotes_niche_formats() {
        let on = FeatureFlags::defaults();
        assert_eq!(classify_extension("dsf", &on), AudioFormat::Dsf);
        assert_eq!(classify_extension("dff", &on), AudioFormat::Dff);
        assert_eq!(classify_extension("ape", &on), AudioFormat::Ape);
        assert_eq!(classify_extension(".wv", &on), AudioFormat::WavPack);

        let mut off = FeatureFlags::defaults();
        off.set("extended_lossless", false);
        for ext in ["dsf", "dff", "ape", "wv"] {
            assert_eq!(classify_extension(ext, &off), AudioFormat::Unknown, "{ext}");
        }
        // Mainstream formats are unaffected by the flag.
        assert_eq!(classify_extension("flac", &off), AudioFormat::Flac);
        assert_eq!(classify_extension("mp3", &off), AudioFormat::Mp3);
    }

    #[test]
    fn disabled_cue_flag_demotes_cue_sheets_only() {
        let mut flags = FeatureFlags::defaults();
        flags.set("cue_sheet_tracks", false);
        assert_eq!(classify_extension("cue", &flags), AudioFormat::Unknown);
        assert_eq!(classify_extension("flac", &flags), AudioFormat::Flac);
    }

    #[test]
    fn strict_mode_rejects_unknown_extensions() {
        let lenient = FeatureFlags::defaults();
        assert_eq!(resolve_format("xyz", &lenient), Some(AudioFormat::Unknown));
        assert_eq!(resolve_format("flac", &lenient), Some(AudioFormat::Flac));

        let mut strict = FeatureFlags::defaults();
        strict.set("strict_format_validation", true);
        assert_eq!(resolve_format("xyz", &strict), None);
        assert_eq!(resolve_format("flac", &strict), Some(AudioFormat::Flac));
    }

    #[test]
    fn disabled_families_rejected_even_when_not_strict() {
        let mut flags = FeatureFlags::defaults();
        flags.set("extended_lossless", false);
        assert_eq!(resolve_format("dsf", &flags), None);
        assert_eq!(resolve_format("flac", &flags), Some(AudioFormat::Flac));
    }

    #[test]
    fn set_overrides_any_name() {
        let mut flags = FeatureFlags::defaults();
        flags.set("extended_lossless", false);
        assert!(!flags.is_enabled("extended_lossless"));
        flags.set("extended_lossless", true);
        assert!(flags.extended_lossless());
    }
}
