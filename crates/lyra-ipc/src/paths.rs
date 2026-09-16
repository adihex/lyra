//! Socket location + discovery precedence (§7):
//! 1. `LYRA_SOCKET` env var. 2. Well-known path (group container on macOS,
//!    `$XDG_RUNTIME_DIR/lyra` on Linux). 3. Legacy app-container fallback
//!    (macOS only). 4. Sidecar `control.sock.json` liveness check.

use std::path::PathBuf;

pub const SOCKET_NAME: &str = "control.sock";
pub const SIDECAR_NAME: &str = "control.sock.json";

/// macOS group-container team scope. Parametrized so the app target can
/// override it at wiring time without touching this crate.
pub const TEAM_SCOPE: &str = "TEAMID.lyra";
pub const BUNDLE_ID: &str = "app.lyra.player";

/// Explicit override (tests, weird setups). Checked first.
pub fn env_override() -> Option<PathBuf> {
    std::env::var_os("LYRA_SOCKET").map(PathBuf::from)
}

/// Default socket path for this platform.
pub fn default_socket_path() -> PathBuf {
    if let Some(p) = env_override() {
        return p;
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library/Group Containers")
                .join(TEAM_SCOPE)
                .join(SOCKET_NAME);
        }
    }
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime).join("lyra").join(SOCKET_NAME);
    }
    let uid = uid_fallback();
    PathBuf::from(format!("/tmp/lyra-{uid}")).join(SOCKET_NAME)
}

/// Last-resort tmp path needs a per-user directory.
/// No libc dep: honor `UID`, else fall back to a shared tmp dir.
fn uid_fallback() -> u32 {
    std::env::var("UID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Sidecar path for a socket path (`control.sock` → `control.sock.json`).
pub fn sidecar_path(socket: &std::path::Path) -> PathBuf {
    socket.with_extension("sock.json")
}

/// Candidate paths in discovery order (§7 rules 1–3).
pub fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(p) = env_override() {
        out.push(p);
        return out;
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            out.push(
                home.join("Library/Group Containers")
                    .join(TEAM_SCOPE)
                    .join(SOCKET_NAME),
            );
            out.push(
                home.join("Library/Containers")
                    .join(BUNDLE_ID)
                    .join("Data/Library/Application Support/Lyra")
                    .join(SOCKET_NAME),
            );
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        out.push(default_socket_path());
    }
    out
}

/// Read the sidecar liveness file, if present and valid.
pub fn read_sidecar(socket: &std::path::Path) -> Option<crate::protocol::Sidecar> {
    let raw = std::fs::read(sidecar_path(socket)).ok()?;
    serde_json::from_slice(&raw).ok()
}
