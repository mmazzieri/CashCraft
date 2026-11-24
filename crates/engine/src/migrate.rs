//! From formats 0 (CashCraft 0.1) and 1 (0.2.0), where money was a table of transactions, to the
//! double-entry ledger of format 2, and from that to the present one. `docs/ledger.md` describes what becomes what.

use crate::cards::CARD_SCHEMA;
use crate::db::{Cents, Db, RENAMED, SCHEMA, SCHEMA_VERSION};
use crate::i18n::{tr, trf};
use crate::ledger::{Draft, LEDGER_SCHEMA, Line, OPENING, UNCATEGORIZED, normalize_currency, too_big};
use anyhow::{Context, Result, bail};
use rusqlite::backup::Backup;
use rusqlite::{Connection, OpenFlags, params};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A row of the table `transactions` of format 1.
struct Row {
    id: i64,
    account: i64,
    date: String,
    amount: Cents,
    payee: String,
    memo: String,
    category: Option<i64>,
    cleared: bool,
    peer: Option<i64>,
    import_hash: Option<String>,
}

/// Removes a file that must not stay when something fails.
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_stem().unwrap_or(OsStr::new("cashcraft")).to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// The file of the first of these names that is not there, created empty.
fn create_free(path: &Path, from: i64) -> Result<PathBuf> {
    for n in 0..1000 {
        let p = beside(path, &format!(".v{from}-backup.{}-{n}.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(&p) {
            Ok(_) => return Ok(p),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    bail!(tr("there are too many backup copies"))
}

/// A copy of the old file, of format `from`, beside it, with the first free name of `cashcraft.v1-backup.db`,
/// `cashcraft.v1-backup-2.db`… The name appears only on a complete copy; an existing file is never
/// overwritten.
fn backup(path: &Path, from: i64) -> Result<PathBuf> {
    let part = Temporary(create_free(path, from)?);
    // Another connection: ours holds the write lock.
    let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    source.busy_timeout(Duration::from_secs(5))?;
    let mut copy = Connection::open(&part.0)?;
    Backup::new(&source, &mut copy)?.run_to_completion(1000, Duration::from_millis(50), None)?;
    // One file, without those of the write-ahead log.
    copy.query_row("PRAGMA journal_mode = DELETE", [], |_| Ok(()))?;
    let check: String = copy.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    if check != "ok" || table_sizes(&copy)? != table_sizes(&source)? {
        bail!(tr("the backup copy is not sound"));
    }
    copy.close().map_err(|(_, e)| e)?;
    std::fs::set_permissions(&part.0, std::fs::metadata(path)?.permissions())?;
    OpenOptions::new().read(true).open(&part.0)?.sync_all()?;
    for n in 1..1000 {
        let target = match n {
            1 => beside(path, &format!(".v{from}-backup.db")),
            n => beside(path, &format!(".v{from}-backup-{n}.db")),
        };
        // A link never replaces a file.
        match std::fs::hard_link(&part.0, &target) {
            Ok(()) => return Ok(target),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            // A file system without links.
            Err(_) if std::fs::symlink_metadata(&target).is_err() => {
                std::fs::rename(&part.0, &target)?;
                return Ok(target);
            }
            Err(_) => {}
        }
    }
    bail!(tr("there are too many backup copies"))
}

/// How many rows each table has.
fn table_sizes(c: &Connection) -> Result<Vec<(String, i64)>> {
    let mut st = c.prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")?;
    let names: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
    names
        .into_iter()
        .map(|n| {
            let sql = format!("SELECT COUNT(*) FROM \"{}\"", n.replace('"', "\"\""));
            let rows = c.query_row(&sql, [], |r| r.get(0))?;
            Ok((n, rows))
        })
        .collect()
}

impl Db {
    /// Turns the tables of an older format into the present one. Runs inside the transaction of `init`, which
    /// holds the write lock: nobody changes the file between the backup and the end.
    pub(crate) fn migrate(&self, version: i64) -> Result<()> {
        let again = self.has_table("postings")?;
        let dated = again && self.has_posting_dates()?;
        let current = dated && self.has_table("card_statements")?;
        if current && !self.has_leftovers()? {
            return self.drop_leftovers();
        }
        // An older program may have written a lower number on a newer file.
        let from = match (again, dated, current) {
            (false, _, _) => 1,
            (_, false, _) => 2,
            (_, _, false) => 3,
            _ => SCHEMA_VERSION.max(version),
        };
        let copy = match &self.path {
            Some(p) => Some(backup(p, from).with_context(|| {
                tr("the data file must be updated to a new format, but a backup copy of it cannot be made; nothing was changed")
            })?),
            None => None,
        };
        let done = (|| -> Result<()> {
            if !again {
                self.convert()?;
            } else {
                if !dated {
                    self.add_posting_dates()?;
                }
                if self.has_leftovers()? {
                    self.merge_leftovers()?;
                } else {
                    self.drop_leftovers()?;
                }
            }
            if !current {
                self.conn.execute_batch(CARD_SCHEMA)?;
            }
            Ok(())
        })();
        if done.is_err()
            && let Some(p) = copy
        {
            // Nothing is going to change: the copy just made is of no use.
            let _ = std::fs::remove_file(p);
        }
        done.context(tr("the data file cannot be updated to the new format; nothing was changed"))
    }

    /// Format 3.
    fn add_posting_dates(&self) -> Result<()> {
        Ok(self.conn.execute_batch("ALTER TABLE postings ADD COLUMN date TEXT")?)
    }

    fn has_posting_dates(&self) -> Result<bool> {
        Ok(self.conn.prepare("SELECT 1 FROM pragma_table_info('postings') WHERE name='date'")?.exists([])?)
    }

    /// An older program can mark a newer file with its own number, and add its tables, when both
    /// open the file at the same moment. Whether it wrote in them.
    fn has_leftovers(&self) -> Result<bool> {
        for t in RENAMED.iter().map(|t| t.0).chain(["transactions"]) {
            if self.has_table(t)? && self.conn.prepare(&format!("SELECT 1 FROM {t}"))?.exists([])? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn drop_leftovers(&self) -> Result<()> {
        for t in RENAMED.iter().map(|t| t.0).chain(["transactions"]) {
            self.conn.execute_batch(&format!("DROP TABLE IF EXISTS {t}"))?;
        }
        Ok(())
    }

    /// Adds to the ledger what an older program wrote in its own tables after the update.
    fn merge_leftovers(&self) -> Result<()> {
        let mut rows = self.old_rows()?;
        // Their ids are taken by now.
        let used: i64 = self.conn.query_row("SELECT COALESCE(MAX(id),0) FROM postings", [], |r| r.get(0))?;
        for r in &mut rows {
            r.id = r.id.checked_add(used).ok_or_else(too_big)?;
            r.peer = r.peer.map(|p| p.checked_add(used).ok_or_else(too_big)).transpose()?;
        }
        // A statement imported by both is in the ledger already.
        let mut known = self.conn.prepare(
            "SELECT 1 FROM postings p JOIN ledger_accounts l ON l.id=p.ledger_account_id
             WHERE l.money_account_id=?1 AND p.import_hash=?2",
        )?;
        let mut kept = Vec::new();
        for r in rows {
            if r.import_hash.is_none() || !known.exists(params![r.account, r.import_hash])? {
                kept.push(r);
            }
        }
        self.convert_rows(&kept)?;
        for (old, new) in RENAMED {
            if self.has_table(old)? {
                let columns = match old {
                    "budgets" => "category_id, monthly",
                    "rules" => "pattern, category_id, rename_payee",
                    _ => "account_id, to_account_id, amount, payee, memo, category_id, next_date, every, unit",
                };
                self.conn.execute_batch(&format!(
                    "INSERT OR IGNORE INTO {new}({columns}) SELECT {columns} FROM {old} ORDER BY rowid"
                ))?;
            }
        }
        self.drop_leftovers()?;
        match self.verify_ledger()?.problems.into_iter().next() {
            Some(p) => bail!(p),
            None => Ok(()),
        }
    }

    fn convert(&self) -> Result<()> {
        for (old, new) in RENAMED {
            if self.has_table(old)? {
                self.conn.execute_batch(&format!("ALTER TABLE {old} RENAME TO {new}"))?;
            }
        }
        // Tables that a file of 0.1 may lack.
        self.conn.execute_batch(SCHEMA)?;
        if !self.conn.prepare("SELECT 1 FROM pragma_table_info('accounts') WHERE name='opening_balance'")?.exists([])? {
            self.conn.execute_batch("ALTER TABLE accounts ADD COLUMN opening_balance INTEGER NOT NULL DEFAULT 0")?;
        }
        self.normalize_currencies()?;
        self.conn.execute_batch(LEDGER_SCHEMA)?;
        // As the triggers do for new accounts and categories.
        self.conn.execute_batch(
            "INSERT INTO ledger_accounts(class, currency, money_account_id)
             SELECT CASE WHEN kind IN ('credit_card','loan') THEN 'liability' ELSE 'asset' END,
                    CASE WHEN trim(currency) = '' THEN 'EUR' ELSE upper(trim(currency)) END, id
             FROM accounts ORDER BY id;
             INSERT INTO ledger_accounts(class, category_id)
             SELECT CASE WHEN is_income THEN 'income' ELSE 'expense' END, id FROM categories ORDER BY id;
             UPDATE ledger_accounts SET parent_id =
                 (SELECT u.id FROM categories c JOIN ledger_accounts u ON u.category_id = c.parent_id
                  WHERE c.id = ledger_accounts.category_id)
             WHERE category_id IS NOT NULL;",
        )?;
        let rows = self.old_rows()?;
        let before = self.old_balances(&rows)?;
        self.convert_rows(&rows)?;
        for (account, (opening, _)) in &before {
            self.set_opening(*account, *opening)?;
        }
        for a in self.accounts(true)? {
            if before.get(&a.id).map(|b| b.1) != Some(a.balance) {
                bail!(trf("the balance of \"{name}\" would change", &[("name", &a.name)]));
            }
        }
        if let Some(p) = self.verify_ledger()?.problems.into_iter().next() {
            bail!(p);
        }
        self.conn
            .execute_batch("DROP TABLE IF EXISTS transactions; ALTER TABLE accounts DROP COLUMN opening_balance;")?;
        Ok(())
    }

    fn normalize_currencies(&self) -> Result<()> {
        let mut st = self.conn.prepare("SELECT id, currency FROM accounts")?;
        let all: Vec<(i64, String)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?;
        for (id, c) in all {
            self.conn.execute("UPDATE accounts SET currency=?2 WHERE id=?1", params![id, normalize_currency(&c)])?;
        }
        Ok(())
    }

    fn old_rows(&self) -> Result<Vec<Row>> {
        if !self.has_table("transactions")? {
            return Ok(Vec::new());
        }
        let mut st = self.conn.prepare(
            "SELECT id, account_id, date, amount, payee, memo, category_id, cleared, transfer_peer, import_hash
             FROM transactions ORDER BY id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Row {
                id: r.get(0)?,
                account: r.get(1)?,
                date: r.get(2)?,
                amount: r.get(3)?,
                payee: r.get(4)?,
                memo: r.get(5)?,
                category: r.get(6)?,
                cleared: r.get::<_, i64>(7)? != 0,
                peer: r.get(8)?,
                import_hash: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Opening and current balance of each account in the old tables.
    fn old_balances(&self, rows: &[Row]) -> Result<BTreeMap<i64, (Cents, Cents)>> {
        let mut st = self.conn.prepare("SELECT id, opening_balance FROM accounts")?;
        let mut out: BTreeMap<i64, (Cents, Cents)> = BTreeMap::new();
        for row in st.query_map([], |r| Ok((r.get(0)?, r.get::<_, Cents>(1)?)))? {
            let (id, opening) = row?;
            out.insert(id, (opening, opening));
        }
        for r in rows {
            if let Some((_, balance)) = out.get_mut(&r.account) {
                *balance = balance.checked_add(r.amount).ok_or_else(too_big)?;
            }
        }
        Ok(out)
    }

    fn convert_rows(&self, rows: &[Row]) -> Result<()> {
        let mut money: HashMap<i64, (i64, String)> = HashMap::new();
        for a in self.accounts(true)? {
            money.insert(a.id, self.money_ledger(a.id)?);
        }
        let mut categories: HashMap<i64, i64> = HashMap::new();
        for c in self.categories()? {
            categories.insert(c.id, self.category_ledger(Some(c.id))?);
        }
        let (opening, uncategorized) = (self.system_ledger(OPENING)?, self.system_ledger(UNCATEGORIZED)?);
        let by_id: HashMap<i64, &Row> = rows.iter().map(|r| (r.id, r)).collect();
        // The two sides of a transfer that can be one balanced entry.
        let partner = |r: &Row| -> Option<&Row> {
            let p = by_id.get(&r.peer?).copied()?;
            let sound = p.peer == Some(r.id)
                && p.account != r.account
                && p.date == r.date
                && p.amount.checked_neg() == Some(r.amount)
                && money.get(&p.account)?.1 == money.get(&r.account)?.1;
            sound.then_some(p)
        };
        // Old ids are kept by the postings on the accounts of the user; the others come after them.
        let mut next = rows.iter().map(|r| r.id).max().unwrap_or(0).max(0);
        let mut done: HashSet<i64> = HashSet::new();
        for r in rows {
            if !done.insert(r.id) {
                continue;
            }
            // A row of an account that is not there was in no balance: it is kept, against equity.
            let (account, currency) = money.get(&r.account).cloned().unwrap_or((opening, "EUR".into()));
            let here = money.contains_key(&r.account);
            let mut lines = vec![Line {
                id: Some(r.id),
                account,
                amount: r.amount,
                currency: currency.clone(),
                cleared: r.cleared,
                import_hash: r.import_hash.clone().filter(|_| here),
                ..Default::default()
            }];
            match partner(r) {
                Some(p) => {
                    done.insert(p.id);
                    let (account, currency) = money.get(&p.account).cloned().unwrap_or((opening, currency));
                    for l in &mut lines {
                        l.payee = Some(r.payee.clone());
                    }
                    lines.push(Line {
                        id: Some(p.id),
                        account,
                        amount: p.amount,
                        currency,
                        cleared: p.cleared,
                        import_hash: p.import_hash.clone(),
                        payee: Some(p.payee.clone()),
                        memo: Some(p.memo.clone()).filter(|m| *m != r.memo),
                        date: None,
                    });
                }
                None => {
                    // Half a transfer was in no report: its other side is equity, which is in none.
                    let other = match r.peer {
                        Some(_) => opening,
                        None => r.category.and_then(|c| categories.get(&c).copied()).unwrap_or(uncategorized),
                    };
                    let amount = r.amount.checked_neg().ok_or_else(too_big)?;
                    next = next.checked_add(1).ok_or_else(too_big)?;
                    lines.push(Line { id: Some(next), account: other, amount, currency, ..Default::default() });
                }
            }
            self.save_entry(&Draft {
                id: None,
                date: r.date.clone(),
                payee: r.payee.clone(),
                memo: r.memo.clone(),
                opening: None,
                lines,
            })
            .with_context(|| trf("transaction {id}", &[("id", &r.id)]))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
