//! What the owner chose about new versions, kept in the updates folder of this computer (never in
//! the data file, which may be shared with other computers): whether to look for them once a day,
//! whether to install them, how often the question was asked, and when a check last got an answer.

use std::fs;
use std::io::Read as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Most starts the question is put at, unanswered, before it is never put again.
pub const MOST_ASKS: u8 = 3;
/// Largest settings file read.
const MAX_FILE: u64 = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Look for new versions once a day: `None` until the owner answers.
    pub check: Option<bool>,
    /// Download and install them, in a copy that updates itself.
    pub install: bool,
    /// The starts the question was put at.
    pub asked: u8,
    /// When a check last got an answer (seconds since 1970).
    pub last_check: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { check: None, install: true, asked: 0, last_check: 0 }
    }
}

impl Settings {
    /// The settings at `path`; the defaults when there are none or they can't be read.
    pub fn load(path: &Path) -> Settings {
        let mut text = String::new();
        let read = fs::File::open(path).and_then(|f| f.take(MAX_FILE).read_to_string(&mut text));
        let mut s: Settings = read.ok().and_then(|_| serde_json::from_str(&text).ok()).unwrap_or_default();
        s.asked = s.asked.min(MOST_ASKS);
        s
    }

    /// Kept at `path`, whole or not at all.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let part = path.with_extension("json.part");
        fs::write(&part, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        fs::rename(&part, path)
    }

    /// Whether a check by itself is due at `now` (seconds since 1970): on, and a day after the
    /// last answer.
    pub fn due(&self, now: i64) -> bool {
        self.check == Some(true) && (now.saturating_sub(self.last_check) >= 86_400 || now < self.last_check)
    }
}
