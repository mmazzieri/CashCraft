//! A small file beside the shared one says that a computer has it open. Only a courtesy: nothing depends on it.
//! Each computer writes its own, so that no sync app ever finds two versions of one.

use super::{Home, parent};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How often it is renewed.
pub const RENEW: Duration = Duration::from_secs(120);
/// One that nobody renews for this long is of a computer that is gone.
const EXPIRES: Duration = Duration::from_secs(360);
/// One so old is removed by whoever finds it.
const ABANDONED_SECONDS: i64 = 24 * 3600;
const SUFFIX: &str = ".cashcraft-open";

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Lease {
    computer: String,
    /// Grows at each renewal: a clock that is wrong does not matter to who watches.
    beat: u64,
    /// Seconds of the clock of that computer.
    at: i64,
}

/// What was seen of one, and when it was last seen to change.
struct Seen {
    beat: u64,
    changed: Option<Instant>,
}

pub struct Presence {
    shared: PathBuf,
    id: String,
    computer: String,
    beat: u64,
    seen: HashMap<String, Seen>,
}

fn prefix(shared: &Path) -> String {
    format!(".{}.", shared.file_name().unwrap_or_default().to_string_lossy())
}

fn read(path: &Path) -> Option<Lease> {
    let mut text = String::new();
    std::fs::File::open(path).ok()?.take(4096).read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

impl Presence {
    pub fn new(home: &Home, shared: &Path) -> Presence {
        Presence {
            shared: shared.to_path_buf(),
            id: home.id(),
            computer: home.computer.clone(),
            beat: 0,
            seen: HashMap::new(),
        }
    }

    fn own(&self) -> PathBuf {
        parent(&self.shared).join(format!("{}{}{SUFFIX}", prefix(&self.shared), self.id))
    }

    /// Says that this computer has the file open.
    pub fn renew(&mut self, now: i64) {
        self.beat += 1;
        let lease = Lease { computer: self.computer.clone(), beat: self.beat, at: now };
        if self.shared.exists()
            && let Ok(text) = serde_json::to_string(&lease)
        {
            // It may be a folder that cannot be written.
            let _ = std::fs::write(self.own(), text);
        }
    }

    pub fn leave(&self) {
        let _ = std::fs::remove_file(self.own());
    }

    /// The names of the other computers that have the file open.
    pub fn others(&mut self, now: i64, instant: Instant) -> Vec<String> {
        let prefix = prefix(&self.shared);
        let mut out = Vec::new();
        let Ok(dir) = std::fs::read_dir(parent(&self.shared)) else { return out };
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_prefix(&prefix).and_then(|n| n.strip_suffix(SUFFIX)) else { continue };
            if id == self.id {
                continue;
            }
            let Some(lease) = read(&entry.path()) else { continue };
            let age = now.saturating_sub(lease.at);
            let live = match self.seen.get_mut(id) {
                Some(seen) => {
                    if seen.beat != lease.beat {
                        *seen = Seen { beat: lease.beat, changed: Some(instant) };
                    }
                    seen.changed.is_some_and(|c| instant.duration_since(c) < EXPIRES)
                }
                None => {
                    // Of one seen for the first time only its clock can tell.
                    let recent = age.unsigned_abs() < EXPIRES.as_secs();
                    self.seen.insert(id.to_string(), Seen { beat: lease.beat, changed: recent.then_some(instant) });
                    recent
                }
            };
            if live {
                out.push(super::clean_name(&lease.computer));
            } else if age > ABANDONED_SECONDS {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        out.sort();
        out.dedup();
        out
    }
}
