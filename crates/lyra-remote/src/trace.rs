//! Request/operation ID propagation for lyra-remote.
//!
//! HTTP services carry this concept as the `X-Request-ID` header. This
//! crate's transport is length-prefixed Noise-encrypted TCP frames, so the
//! same idea rides *inside* the encrypted JSON envelope instead:
//!
//! ```json
//! { "rid": "a3f9c1e07b2d44aa", "cmd": { "command": "toggle" } }
//! ```
//!
//! - [`new_request_id`] mints an ID; [`normalize_request_id`] validates a
//!   caller-supplied one (the `X-Request-ID` handling: a well-formed ID is
//!   honored, anything else is replaced, never rejected).
//! - [`encode_command`] wraps a command with its ID; [`decode_command`]
//!   accepts the envelope *and* the legacy bare command (older clients
//!   keep working — the server mints an ID for them).
//! - Every hop logs inside a `tracing` span carrying `request_id`, so a
//!   phone tap → host → engine round-trip is greppable as one operation.

use lyra_core::{LyraError, PlayerCommand};
use rand::Rng;

/// Mint a fresh request/operation ID: 16 lowercase hex chars (64 bits from
/// the OS RNG — unique enough for correlation, not a secret).
#[must_use]
pub fn new_request_id() -> String {
    format!("{:016x}", rand::thread_rng().gen::<u64>())
}

/// IDs are 1–64 chars of `[A-Za-z0-9-_]`: safe for log grep, filenames,
/// and future HTTP `X-Request-ID` interop.
#[must_use]
pub fn is_valid_request_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Honor a caller-supplied ID when it is well-formed, else mint one.
/// Never fails — a bad ID must not fail the operation it labels.
#[must_use]
pub fn normalize_request_id(raw: Option<&str>) -> String {
    match raw {
        Some(s) if is_valid_request_id(s) => s.to_string(),
        _ => new_request_id(),
    }
}

/// Serialize a command into its encrypted-frame payload with the request ID
/// attached. The bytes still go through the Noise transport unchanged.
#[must_use]
pub fn encode_command(cmd: &PlayerCommand, request_id: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({ "rid": request_id, "cmd": cmd }))
        .unwrap_or_else(|_| b"{}".to_vec())
}

/// Split a decrypted frame into `(request_id, command)`.
///
/// Accepts the [`encode_command`] envelope; a bare [`PlayerCommand`] (no
/// `cmd` key) is accepted for backward compatibility and assigned a fresh
/// ID so every operation still traces end to end.
pub fn decode_command(bytes: &[u8]) -> Result<(String, PlayerCommand), LyraError> {
    let bad = || LyraError::Remote("bad_command".into());
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| bad())?;
    if let Some(cmd) = v.get("cmd") {
        let rid = v
            .get("rid")
            .and_then(|r| r.as_str())
            .map_or_else(new_request_id, |s| normalize_request_id(Some(s)));
        let cmd: PlayerCommand = serde_json::from_value(cmd.clone()).map_err(|_| bad())?;
        return Ok((rid, cmd));
    }
    let cmd: PlayerCommand = serde_json::from_value(v).map_err(|_| bad())?;
    Ok((new_request_id(), cmd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_hex16() {
        let a = new_request_id();
        let b = new_request_id();
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn normalize_honors_valid_rejects_rest() {
        assert_eq!(normalize_request_id(Some("abc-123_X")), "abc-123_X");
        // Empty, too long, and weird chars all get replaced — never fail.
        for bad in [Some(""), Some("has space"), Some("semi;colon"), None] {
            let id = normalize_request_id(bad);
            assert!(is_valid_request_id(&id), "bad input {bad:?} gave {id}");
        }
        assert!(normalize_request_id(Some(&"x".repeat(65))).len() <= 64);
    }

    #[test]
    fn envelope_round_trip() {
        let cmd = PlayerCommand::Volume { value: 0.5 };
        let bytes = encode_command(&cmd, "req-1");
        let (rid, back) = decode_command(&bytes).unwrap();
        assert_eq!(rid, "req-1");
        assert!(matches!(back, PlayerCommand::Volume { .. }));
    }

    #[test]
    fn bare_command_still_decodes_with_minted_id() {
        let bytes = serde_json::to_vec(&PlayerCommand::Toggle).unwrap();
        let (rid, back) = decode_command(&bytes).unwrap();
        assert!(is_valid_request_id(&rid));
        assert!(matches!(back, PlayerCommand::Toggle));
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode_command(b"not json").is_err());
        assert!(decode_command(b"{\"rid\":\"x\"}").is_err());
    }
}
