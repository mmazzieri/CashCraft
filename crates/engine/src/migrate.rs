//! From formats 0 (CashCraft 0.1) and 1 (0.2.0), where money was a table of transactions, to the
//! double-entry ledger of format 2. `docs/ledger.md` describes what becomes what.

use crate::db::{Cents, Db, SCHEMA};
use crate::i18n::{tr, trf};
use crate::ledger::{Draft, LEDGER_SCHEMA, Line, OPENING, UNCATEGORIZED};
use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, OpenFlags};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

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

/// A copy of the old file beside it, with the first free name of `cashcraft.v1-backup.db`,
/// `cashcraft.v1-backup-2.db`… An existing file is never overwritten.
fn backup(path: &Path) -> Result<PathBuf> {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("cashcraft");
    let target = (1..1000)
        .map(|n| match n {
            1 => path.with_file_name(format!("{stem}.v1-backup.db")),
            n => path.with_file_name(format!("{stem}.v1-backup-{n}.db")),
        })
        .find(|p| !p.exists() && std::fs::symlink_metadata(p).is_err())
        .ok_or_else(|| anyhow!(tr("there are too many backup copies")))?;
    let name = target.to_str().ok_or_else(|| anyhow!(tr("the name of the folder is not valid text")))?;
    // Another connection: ours holds the write lock and VACUUM cannot run inside a transaction.
    let source = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    source.busy_timeout(std::time::Duration::from_secs(5))?;
    source.execute("VACUUM INTO ?1", [name])?;
    let count = |c: &Connection| -> Result<(String, i64)> {
        let check: String = c.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        Ok((
            check,
            c.query_row("SELECT (SELECT COUNT(*) FROM accounts) + (SELECT COUNT(*) FROM categories)", [], |r| {
                r.get(0)
            })?,
        ))
    };
    let copy = Connection::open_with_flags(&target, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let (check, rows) = count(&copy)?;
    drop(copy);
    if check != "ok" || rows != count(&source)?.1 {
        bail!(tr("the backup copy is not sound"));
    }
    std::fs::OpenOptions::new().write(true).open(&target)?.sync_all()?;
    Ok(target)
}

impl Db {
    /// Turns the tables of format 0 or 1 into format 2. Runs inside the transaction of `init`, which
    /// holds the write lock: nobody changes the file between the backup and the end.
    pub(crate) fn migrate(&self) -> Result<()> {
        if self.has_table("postings")? {
            return self.restore_version();
        }
        let copy = match &self.path {
            Some(p) => Some(backup(p).with_context(|| {
                tr("the data file must be updated to a new format, but a backup copy of it cannot be made; nothing was changed")
            })?),
            None => None,
        };
        let done = self.convert();
        if done.is_err()
            && let Some(p) = copy
        {
            // Nothing is going to change: the copy just made is of no use.
            let _ = std::fs::remove_file(p);
        }
        done.context(tr("the data file cannot be updated to the new format; nothing was changed"))
    }

    /// An older program can mark a file of format 2 with its own number, and add its empty table, when
    /// both open the file at the same moment.
    fn restore_version(&self) -> Result<()> {
        if self.has_table("transactions")? {
            let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM transactions", [], |r| r.get(0))?;
            if n > 0 {
                bail!(tr("the data file has transactions in both the old and the new format"));
            }
            self.conn.execute_batch("DROP TABLE transactions;")?;
        }
        Ok(())
    }

    fn convert(&self) -> Result<()> {
        // Tables that a file of 0.1 may lack.
        self.conn.execute_batch(SCHEMA)?;
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
                *balance =
                    balance.checked_add(r.amount).ok_or_else(|| anyhow!(tr("the sum of the amounts is too large")))?;
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
                payee: None,
                memo: None,
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
                    });
                }
                None => {
                    // Half a transfer was in no report: its other side is equity, which is in none.
                    let other = match r.peer {
                        Some(_) => opening,
                        None => r.category.and_then(|c| categories.get(&c).copied()).unwrap_or(uncategorized),
                    };
                    let amount =
                        r.amount.checked_neg().ok_or_else(|| anyhow!(tr("the sum of the amounts is too large")))?;
                    next = next.checked_add(1).ok_or_else(|| anyhow!(tr("the sum of the amounts is too large")))?;
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
