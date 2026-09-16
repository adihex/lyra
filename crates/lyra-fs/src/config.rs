//! Remote connection profiles — the one serializable struct the app layer
//! fills in. Every remote backend (SFTP reads, rsync staging, the scanner,
//! and the v0 `ssh(1)` bootstrap) consumes it, so host/port/user/key logic
//! lives here instead of being copied per backend.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

/// Which machine + tree a remote backend talks to. The app layer supplies
/// instances (built from its own settings UI / bookmark resolution).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoteProfile {
    /// Anything `ssh(1)` resolves (config alias, tailnet name) or a raw
    /// hostname/IP for direct-TCP backends like SFTP.
    pub host: String,
    /// SSH port. 22 = default.
    pub port: u16,
    /// Remote login name. `None` = ssh-config / current-user default.
    pub user: Option<String>,
    /// Explicit identity file (sandbox-safe: the app passes a
    /// security-scoped path, never `~/.ssh`). `None` = agent/default keys.
    pub key_path: Option<PathBuf>,
    /// Remote library root, e.g. `/mnt/music/flac`.
    pub root_path: String,
}

impl RemoteProfile {
    pub fn new(host: impl Into<String>, root_path: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port: 22,
            user: None,
            key_path: None,
            root_path: root_path.into(),
        }
    }

    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    pub fn with_key(mut self, key: &Path) -> Self {
        self.key_path = Some(key.to_path_buf());
        self
    }

    /// `user@host` for `ssh(1)`-family CLIs, or bare `host`.
    pub fn ssh_target(&self) -> String {
        match &self.user {
            Some(u) => format!("{u}@{}", self.host),
            None => self.host.clone(),
        }
    }

    /// Library-row identity for a remote file: `sftp://host[:port]/path`.
    pub fn source_uri(&self, remote_path: &str) -> String {
        if self.port == 22 {
            format!("sftp://{}{remote_path}", self.host)
        } else {
            format!("sftp://{}:{}{remote_path}", self.host, self.port)
        }
    }

    /// Join `root_path` and a walk-relative path without double slashes.
    pub fn join_root(&self, rel: &str) -> String {
        let root = self.root_path.trim_end_matches('/');
        match rel.strip_prefix('/') {
            Some(r) => format!("{root}/{r}"),
            None if rel.is_empty() => root.to_string(),
            None => format!("{root}/{rel}"),
        }
    }

    /// Auth implied by the profile alone: explicit key file wins,
    /// otherwise the agent (with `~/.ssh` default-key fallback).
    pub fn default_auth(&self) -> AuthMethod {
        match &self.key_path {
            Some(p) => AuthMethod::KeyFile {
                path: p.clone(),
                passphrase: None,
            },
            None => AuthMethod::Agent,
        }
    }
}

/// How an SFTP session proves who it is. Secrets never hit `Debug`.
#[derive(Clone)]
pub enum AuthMethod {
    /// ssh-agent, then `~/.ssh/id_ed25519`/`id_rsa`/`id_ecdsa` fallback.
    Agent,
    /// Explicit identity file; the app resolves sandbox access.
    KeyFile {
        path: PathBuf,
        passphrase: Option<String>,
    },
    /// Password auth with the secret inline (short-lived profiles).
    Password(String),
    /// Password auth where the secret is pulled on demand — the app shows
    /// its own prompt UI behind this. Called at most once per connect.
    PasswordCallback(AuthCallback),
}

#[derive(Clone)]
pub struct AuthCallback(pub Arc<dyn Fn() -> Option<String> + Send + Sync>);

impl std::fmt::Debug for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Agent => write!(f, "Agent"),
            Self::KeyFile { path, .. } => {
                write!(f, "KeyFile({})", path.display())
            }
            Self::Password(_) => write!(f, "Password(<redacted>)"),
            Self::PasswordCallback(_) => write!(f, "PasswordCallback(<redacted>)"),
        }
    }
}

/// `ssh [-p port] target` — shared by the exec bootstrap and rsync's `-e`.
pub(crate) fn ssh_cmd(target: &str, port: u16) -> Command {
    let mut c = Command::new("ssh");
    if port != 22 {
        c.args(["-p", &port.to_string()]);
    }
    c.arg(target);
    c
}

/// Extra `ssh` flags for `rsync -e "..."`, derived from a profile.
pub(crate) fn rsync_ssh(profile: &RemoteProfile) -> String {
    let mut s = String::from("ssh");
    if profile.port != 22 {
        s.push_str(&format!(" -p {}", profile.port));
    }
    if let Some(k) = &profile.key_path {
        s.push_str(&format!(
            " -i '{}'",
            k.display().to_string().replace('\'', "'\\''")
        ));
    }
    s
}

pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_and_uri_shapes() {
        let p = RemoteProfile::new("adi-linux", "/mnt/music");
        assert_eq!(p.ssh_target(), "adi-linux");
        assert_eq!(
            p.source_uri("/mnt/music/a.flac"),
            "sftp://adi-linux/mnt/music/a.flac"
        );

        let p = RemoteProfile::new("h", "/m")
            .with_user("adi")
            .with_port(2222);
        assert_eq!(p.ssh_target(), "adi@h");
        assert_eq!(p.source_uri("/x"), "sftp://h:2222/x");
    }

    #[test]
    fn join_root_no_doubles() {
        let p = RemoteProfile::new("h", "/mnt/music/");
        assert_eq!(p.join_root("a/b.flac"), "/mnt/music/a/b.flac");
        assert_eq!(p.join_root("/a.flac"), "/mnt/music/a.flac");
        assert_eq!(p.join_root(""), "/mnt/music");
    }

    #[test]
    fn default_auth_follows_key() {
        let p = RemoteProfile::new("h", "/m");
        assert!(matches!(p.default_auth(), AuthMethod::Agent));
        let p = p.with_key(Path::new("/k/id"));
        assert!(matches!(p.default_auth(), AuthMethod::KeyFile { .. }));
    }

    #[test]
    fn auth_debug_redacts() {
        assert_eq!(
            format!("{:?}", AuthMethod::Password("s3cret".into())),
            "Password(<redacted>)"
        );
        assert!(!format!("{:?}", AuthMethod::Password("s3cret".into())).contains("s3cret"));
    }

    #[test]
    fn rsync_ssh_flags() {
        let p = RemoteProfile::new("h", "/m");
        assert_eq!(rsync_ssh(&p), "ssh");
        let p = p.with_port(2222).with_key(Path::new("/k/id"));
        assert_eq!(rsync_ssh(&p), "ssh -p 2222 -i '/k/id'");
    }
}
