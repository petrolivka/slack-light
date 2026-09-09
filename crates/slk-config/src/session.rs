//! What the window looked like last time.
//!
//! Separate from the config on purpose. The config is a file people write;
//! this one is a file the program writes, and mixing the two means an
//! ordinary resize rewrites — and reformats, and strips the comments from —
//! something somebody hand-edited. `[window]` in the config stays the
//! *default*; this is where it was actually left.
//!
//! Read once before the window exists and written once as it closes, so
//! neither touches the running main loop (CONTRIBUTING rule 3).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub width: i32,
    pub height: i32,
    pub sidebar_width: i32,
    /// The side pane, when one was open.
    pub thread_width: i32,
    /// Which workspace, by its domain — an index would point at a different
    /// workspace the moment one is added or removed.
    pub workspace: String,
    /// The thread that was open, as `channel/ts`.
    pub thread: String,
}

impl Session {
    pub fn path() -> std::path::PathBuf {
        crate::state_dir().join("session.toml")
    }

    /// Whatever was left, or nothing. A malformed file is nothing: this is a
    /// convenience, and refusing to start over it would be absurd.
    pub fn load() -> Session {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(body) = toml::to_string_pretty(self) {
            let _ = std::fs::write(p, body);
        }
    }
}
