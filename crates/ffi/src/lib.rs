//! CashCraft's operations for the phone apps.
//!
//! The apps call one function, [`call`], with a request in JSON and get an answer in JSON:
//! `{"ok": …}` or `{"error": "…"}`. A phone reaches the shared data file only through the system, so
//! it works on a private copy and carries whole files. The operations are those of
//! `cashcraft_engine::ops`. The requests are in [`Request`]; `docs/mobile.md` says how they are used.
//!
//! Only the modules that make [`call`] reachable from Java (`android`) and from C and Swift (`c`)
//! use `unsafe`.

#[cfg(target_os = "android")]
mod android;
#[cfg(not(target_os = "android"))]
pub mod c;

use anyhow::{Context, Result, bail};
use cashcraft_engine::db::Db;
use cashcraft_engine::i18n::{Lang, set_lang};
use cashcraft_engine::ops;
use serde::Deserialize;
use serde_json::{Value, json};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

/// The operations a phone may ask for. The others read or write files by path, or start a program.
const ALLOWED: &[&str] = &[
    "list_accounts",
    "create_account",
    "list_transactions",
    "add_transaction",
    "update_transaction",
    "delete_transaction",
    "add_transfer",
    "list_categories",
    "spending_by_category",
    "net_worth",
    "set_budget",
    "budget_status",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// The language of messages and of amounts typed: `it` or `en`.
    Lang { lang: String },
    /// Makes a new data file at `path`, which must not be there.
    New { path: PathBuf },
    /// Checks the file just fetched, `from`, and puts it in the place of the private copy.
    Adopt { from: PathBuf, path: PathBuf },
    /// Ends whatever a call that stopped left open in the private copy.
    Settle { path: PathBuf },
    /// Runs an operation on the private copy, which is whole again when it ends.
    Call { path: PathBuf, tool: String, args: Value },
}

/// Opens, uses and closes the private copy. The last connection that closes leaves no log beside
/// it: the file alone holds everything.
fn with_db<T>(path: &Path, body: impl FnOnce(&Db) -> Result<T>) -> Result<T> {
    if !path.is_file() {
        bail!("there is no data file on the phone");
    }
    let (db, _) = Db::open(path)?;
    let out = body(&db)?;
    let Db { conn, .. } = db;
    conn.close().map_err(|(_, e)| e).context("the data file could not be closed")?;
    if beside(path, "-wal").exists() {
        bail!("the data file is in use");
    }
    Ok(out)
}

fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn remove_logs(path: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        match std::fs::remove_file(beside(path, suffix)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    Ok(())
}

/// Refuses what is not a whole CashCraft file, before the engine opens it: the engine would make
/// an empty file into a new one.
fn check(path: &Path) -> Result<()> {
    use std::io::Read;
    let mut head = [0u8; 16];
    let whole = std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok();
    if !whole || &head != b"SQLite format 3\0" {
        bail!("the file is not a CashCraft data file");
    }
    let conn = rusqlite::Connection::open(path)?;
    let tables: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name IN ('categories','meta')",
        [],
        |r| r.get(0),
    )?;
    if tables != 2 {
        bail!("the file is not a CashCraft data file");
    }
    let state: String = conn.query_row("PRAGMA quick_check(1)", [], |r| r.get(0))?;
    if state != "ok" {
        bail!("the data file is damaged or not all here yet");
    }
    conn.close().map_err(|(_, e)| e)?;
    Ok(())
}

fn run(request: Request) -> Result<Value> {
    match request {
        Request::Lang { lang } => set_lang(if lang.starts_with("it") { Lang::It } else { Lang::En }),
        Request::New { path } => {
            if path.symlink_metadata().is_ok() {
                bail!("the file is there already");
            }
            let (db, _) = Db::open(&path)?;
            db.conn.close().map_err(|(_, e)| e)?;
        }
        Request::Adopt { from, path } => {
            remove_logs(&from)?;
            check(&from)?;
            // A newer format is refused and an older one brought up to date, on the copy.
            with_db(&from, |_| Ok(()))?;
            remove_logs(&path)?;
            std::fs::rename(&from, &path)?;
        }
        Request::Settle { path } => with_db(&path, |_| Ok(()))?,
        Request::Call { path, tool, args } => {
            if !ALLOWED.contains(&tool.as_str()) {
                bail!("unknown operation {tool}");
            }
            return with_db(&path, |db| ops::call(db, &tool, &args, None));
        }
    }
    Ok(Value::Null)
}

/// Runs one request. Never panics: a request that cannot be read, or a fault inside, comes back
/// as `{"error": …}`.
pub fn call(input: &str) -> String {
    let answer = catch_unwind(AssertUnwindSafe(|| {
        let request: Request =
            serde_json::from_str(input).map_err(|e| format!("the request could not be read: {e}"))?;
        run(request).map_err(|e| format!("{e:#}"))
    }))
    .unwrap_or_else(|_| Err("CashCraft's engine stopped on an internal error".to_owned()));
    let value = match answer {
        Ok(v) => json!({ "ok": v }),
        Err(e) => json!({ "error": e }),
    };
    serde_json::to_string(&value).unwrap_or_else(|_| "{\"error\":\"the answer could not be written\"}".to_owned())
}

#[cfg(test)]
mod tests;
