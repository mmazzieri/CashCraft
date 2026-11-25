//! What is in a data file: whether it is whole, and whether two hold the same.

use anyhow::{Result, bail};
use rusqlite::backup::{Backup, StepResult};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// Opens a file that is there.
pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(conn)
}

pub fn is_sound(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))? == "ok")
}

pub fn format(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

pub fn has_accounts(conn: &Connection) -> Result<bool> {
    let sql = "SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name='accounts'";
    Ok(conn.query_row(sql, [], |r| r.get::<_, i64>(0))? > 0)
}

fn number(bytes: &[u8], at: usize) -> Option<u64> {
    let four: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(u64::from(u32::from_be_bytes(four)))
}

/// Whether the bytes are those of a database with all its pages. A download that has not ended is not.
pub fn is_whole(bytes: &[u8]) -> bool {
    if !bytes.starts_with(b"SQLite format 3\0") {
        return false;
    }
    let two = bytes.get(16..18).map(|b| b.iter().fold(0u64, |n, b| n * 256 + u64::from(*b)));
    let page = match two {
        Some(1) => 65536,
        Some(n) if n >= 512 && n.is_power_of_two() => n,
        _ => return false,
    };
    let (Some(pages), len) = (number(bytes, 28), bytes.len() as u64) else { return false };
    // The number of pages counts only when it was written with the last change.
    let counted = number(bytes, 24) == number(bytes, 92);
    len >= page && len % page == 0 && (!counted || len >= pages * page)
}

/// Whether a file says that its changes go through a write-ahead log.
pub fn uses_log(bytes: &[u8]) -> bool {
    bytes.get(18) == Some(&2)
}

/// A hash of all that the database holds, whatever the pages it lies on.
pub fn fingerprint(conn: &Connection) -> Result<String> {
    let tx = conn.unchecked_transaction()?;
    fingerprint_in(&tx)
}

/// [`fingerprint`] inside a transaction.
pub fn fingerprint_in(tx: &Connection) -> Result<String> {
    let mut hash = Sha256::new();
    let put = |hash: &mut Sha256, kind: u8, bytes: &[u8]| {
        hash.update([kind]);
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    };
    hash.update(format(tx)?.to_be_bytes());
    let objects: Vec<(String, String, String)> = tx
        .prepare(
            "SELECT type, name, COALESCE(sql,'') FROM sqlite_schema WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
             ORDER BY type, name",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (kind, name, sql) in objects {
        put(&mut hash, b'o', name.as_bytes());
        put(&mut hash, b's', sql.as_bytes());
        if kind != "table" {
            continue;
        }
        let from = format!("SELECT * FROM \"{}\"", name.replace('"', "\"\""));
        let columns = tx.prepare(&from)?.column_count();
        let order: Vec<String> = (1..=columns).map(|n| n.to_string()).collect();
        let mut st = tx.prepare(&format!("{from} ORDER BY {}", order.join(",")))?;
        let mut rows = st.query([])?;
        while let Some(row) = rows.next()? {
            hash.update(b"r");
            for i in 0..columns {
                match row.get_ref(i)? {
                    ValueRef::Null => hash.update(b"n"),
                    ValueRef::Integer(n) => put(&mut hash, b'i', &n.to_be_bytes()),
                    ValueRef::Real(x) => put(&mut hash, b'f', &x.to_be_bytes()),
                    ValueRef::Text(t) => put(&mut hash, b't', t),
                    ValueRef::Blob(b) => put(&mut hash, b'b', b),
                }
            }
        }
    }
    Ok(hex(&hash.finalize()))
}

/// Copies a whole database into another one, in one transaction of that one.
pub fn copy_into(from: &Connection, to: &mut Connection) -> Result<()> {
    let backup = Backup::new(from, to)?;
    for _ in 0..200 {
        match backup.step(-1)? {
            StepResult::Done => return Ok(()),
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    bail!(crate::i18n::tr("the data file is busy: try again"))
}

/// The path of a file that SQLite keeps beside a database.
pub fn log(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

pub fn remove_with_logs(path: &Path) {
    for p in [path.to_path_buf(), log(path, "-wal"), log(path, "-shm"), log(path, "-journal")] {
        let _ = std::fs::remove_file(p);
    }
}

/// One file, with nothing beside it, whose bytes are all there and on the disk.
pub fn settle(conn: Connection, path: &Path) -> Result<()> {
    let _: String = conn.query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))?;
    if !is_sound(&conn)? {
        bail!(crate::i18n::tr("the data file is damaged"));
    }
    conn.close().map_err(|(_, e)| e)?;
    std::fs::OpenOptions::new().write(true).open(path)?.sync_all()?;
    Ok(())
}

/// Whether every row of `old` is in `new`. Totals that triggers keep are not compared.
pub fn contains(new: &Connection, old: &Connection) -> Result<bool> {
    let names = |c: &Connection| -> Result<Vec<String>> {
        let sql = "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
                   AND name<>'ledger_turnover' ORDER BY name";
        Ok(c.prepare(sql)?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    };
    let tables = names(old)?;
    if format(new)? != format(old)? || names(new)? != tables {
        return Ok(false);
    }
    for table in tables {
        let mut counts = std::collections::HashMap::<String, i64>::new();
        for (conn, by) in [(new, 1), (old, -1)] {
            let mut st = conn.prepare(&format!("SELECT * FROM \"{}\"", table.replace('"', "\"\"")))?;
            let columns = st.column_count();
            let mut rows = st.query([])?;
            while let Some(row) = rows.next()? {
                let mut key = String::new();
                for i in 0..columns {
                    key.push_str(&format!("{:?};", row.get_ref(i)?));
                }
                *counts.entry(key).or_insert(0) += by;
            }
        }
        if counts.values().any(|n| *n < 0) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Fails if any other program of this computer has the file open, and changes nothing of it: a log that lies beside it
/// is not brought in.
pub fn prove_alone(path: &Path) -> Result<()> {
    let conn = open(path)?;
    conn.busy_timeout(Duration::from_millis(500))?;
    conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)?;
    let _: String = conn.query_row("PRAGMA locking_mode = EXCLUSIVE", [], |r| r.get(0))?;
    conn.execute_batch("BEGIN EXCLUSIVE; ROLLBACK;")?;
    Ok(())
}
