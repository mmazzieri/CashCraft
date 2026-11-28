//! What an investment account is worth. The user writes down, on a day, the value that the bank or the platform shows;
//! an entry against *Changes in value* (`system = market_value`, equity) takes the balance of the account to it, so the
//! net worth, the registers and every report go on being sums of postings. The value is kept with the entry: when a
//! transaction of the account before that day is added, changed or deleted, every entry of the values is worked out
//! again, in order of date, at the end of the same change, so the balance on each of those days is the value written.
//! `docs/investments.md` has the rules.

use crate::db::{AccountKind, Cents, Db, d2s, s2d};
use crate::i18n::{tr, trf};
use crate::ledger::{Draft, Line, too_big};
use crate::money::MAX_AMOUNT;
use anyhow::{Result, anyhow, bail};
use chrono::NaiveDate;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

/// Key of the account of the chart that takes the changes of value.
pub const MARKET_VALUE: &str = "market_value";

/// The values written by the user (format 11), each with its entry, which goes with it.
pub(crate) const VALUATION_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS valuations (
    entry_id INTEGER PRIMARY KEY REFERENCES entries(id) ON DELETE CASCADE,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    value INTEGER NOT NULL CHECK (typeof(value) = 'integer')
);
CREATE INDEX IF NOT EXISTS valuation_account ON valuations(account_id);
INSERT INTO ledger_accounts(class, name, system)
SELECT 'equity', 'Changes in value', 'market_value'
WHERE NOT EXISTS (SELECT 1 FROM ledger_accounts WHERE system = 'market_value');
";

/// Most values of one account.
pub const MAX_VALUATIONS: usize = 10_000;

/// The value of an account on a day, as the user wrote it.
#[derive(Clone, Debug, Serialize)]
pub struct Valuation {
    pub entry_id: i64,
    pub account_id: i64,
    pub date: NaiveDate,
    /// What the account was worth at the end of the day.
    pub balance: Cents,
    /// The change of value since the value before, without the money moved in or out: the posting of the entry.
    pub amount: Cents,
}

/// An investment account: what it is worth, what was put in and what it gained.
#[derive(Clone, Debug, Serialize)]
pub struct Performance {
    pub account_id: i64,
    pub account: String,
    pub currency: String,
    /// Its balance in the books today: the last value, with the money moved since.
    pub balance: Cents,
    /// The money put in less the money taken out, with the opening balance.
    pub invested: Cents,
    /// Every change of value: balance less invested.
    pub gain: Cents,
    /// The last value written, if any.
    pub last: Option<Valuation>,
    pub valuations: Vec<Valuation>,
}

fn no_valuation(account: &str, date: NaiveDate) -> anyhow::Error {
    anyhow!(trf(
        "no value of \"{account}\" on {date}",
        &[("account", &account), ("date", &crate::money::fmt_date(date))]
    ))
}

impl Db {
    /// The values of an account, by date.
    pub fn valuations(&self, account: i64) -> Result<Vec<Valuation>> {
        let (ledger, _) = self.money_ledger(account)?;
        let mut st = self.conn.prepare_cached(
            "SELECT v.entry_id, e.date, v.value,
                    COALESCE((SELECT SUM(p.amount) FROM journal_postings p WHERE p.entry_id=v.entry_id
                              AND p.ledger_account_id=?2),0)
             FROM valuations v JOIN entries e ON e.id=v.entry_id WHERE v.account_id=?1 ORDER BY e.date, v.entry_id",
        )?;
        let rows = st.query_map(params![account, ledger], |r| {
            Ok(Valuation {
                entry_id: r.get(0)?,
                account_id: account,
                date: s2d(&r.get::<_, String>(1)?),
                balance: r.get(2)?,
                amount: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// What an account is worth and what it gained.
    pub fn performance(&self, account: i64) -> Result<Performance> {
        let a = self
            .accounts(true)?
            .into_iter()
            .find(|a| a.id == account)
            .ok_or_else(|| anyhow!(trf("account \"{name}\" not found", &[("name", &account)])))?;
        let (ledger, _) = self.money_ledger(account)?;
        // Balance and gain in one statement, so that both are of the same state of the file.
        let (balance, gain): (Cents, Cents) = self
            .conn
            .prepare_cached(
                "SELECT COALESCE((SELECT SUM(amount) FROM journal_postings WHERE ledger_account_id=?1),0),
                    COALESCE((SELECT SUM(p.amount) FROM journal_postings p JOIN valuations v ON v.entry_id=p.entry_id
                              WHERE p.ledger_account_id=?1 AND v.account_id=?2),0)",
            )?
            .query_row(params![ledger, account], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let valuations = self.valuations(account)?;
        Ok(Performance {
            account_id: a.id,
            account: a.name,
            currency: a.currency,
            balance,
            invested: balance.checked_sub(gain).ok_or_else(too_big)?,
            gain,
            last: valuations.last().cloned(),
            valuations,
        })
    }

    /// Writes down what an investment account is worth at the end of a day; the value of that day, if there is one,
    /// is replaced. Returns the value as it is in the books.
    pub fn record_valuation(&self, account: i64, date: NaiveDate, value: Cents) -> Result<Valuation> {
        if value < 0 {
            bail!(tr("the value of an account cannot be below zero"));
        }
        if value > MAX_AMOUNT {
            bail!(tr("the amount is too large"));
        }
        let a = self
            .accounts(true)?
            .into_iter()
            .find(|a| a.id == account)
            .ok_or_else(|| anyhow!(trf("account \"{name}\" not found", &[("name", &account)])))?;
        if a.kind != AccountKind::Investment {
            bail!(trf(
                "\"{account}\" is not an investment account: only those have a value of their own",
                &[("account", &a.name)]
            ));
        }
        let entry = self.atomically(|| {
            if let Some(entry) = self.valuation_on(account, date)? {
                self.conn.execute("UPDATE valuations SET value=?2 WHERE entry_id=?1", params![entry, value])?;
                self.moved.borrow_mut().insert(self.money_ledger(account)?.0);
                // Now, so that what is returned is in the books also inside a change that goes on.
                self.revalue_moved()?;
                return Ok(entry);
            }
            if self.valuations(account)?.len() >= MAX_VALUATIONS {
                bail!(trf("an account has at most {n} values", &[("n", &MAX_VALUATIONS)]));
            }
            let (ledger, currency) = self.money_ledger(account)?;
            let d = Draft {
                id: None,
                date: d2s(date),
                payee: tr("Value of the account").into(),
                memo: String::new(),
                opening: None,
                lines: vec![
                    Line { account: ledger, currency: currency.clone(), ..Default::default() },
                    Line { account: self.system_ledger(MARKET_VALUE)?, currency, ..Default::default() },
                ],
            };
            let (entry, _) = self.save_entry(&d)?;
            self.conn.execute(
                "INSERT INTO valuations(entry_id, account_id, value) VALUES(?1,?2,?3)",
                params![entry, account, value],
            )?;
            self.revalue_moved()?;
            Ok(entry)
        })?;
        self.valuations(account)?.into_iter().find(|v| v.entry_id == entry).ok_or_else(|| no_valuation(&a.name, date))
    }

    /// Takes away the value of an account on a day; the values after it are worked out again.
    pub fn delete_valuation(&self, account: i64, date: NaiveDate) -> Result<()> {
        let name = self.accounts(true)?.into_iter().find(|a| a.id == account).map(|a| a.name).unwrap_or_default();
        self.atomically(|| {
            let entry = self.valuation_on(account, date)?.ok_or_else(|| no_valuation(&name, date))?;
            self.remove_entry(entry)
        })
    }

    /// Whether an entry is the value of an account, which is changed only as a value.
    pub(crate) fn is_valuation(&self, entry: i64) -> Result<bool> {
        Ok(self
            .conn
            .prepare_cached("SELECT 1 FROM valuations WHERE entry_id=?1")?
            .query_row([entry], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// Refuses to change by hand the entry of a value: the value is changed instead, or deleted.
    pub(crate) fn refuse_valuation(&self, entry: i64) -> Result<()> {
        if self.has_table("valuations")? && self.is_valuation(entry)? {
            bail!(tr("this is the value of an investment account on a day: write the new value, or delete it"));
        }
        Ok(())
    }

    fn valuation_on(&self, account: i64, date: NaiveDate) -> Result<Option<i64>> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT v.entry_id FROM valuations v JOIN entries e ON e.id=v.entry_id
                 WHERE v.account_id=?1 AND e.date=?2 ORDER BY v.entry_id LIMIT 1",
            )?
            .query_row(params![account, d2s(date)], |r| r.get(0))
            .optional()?)
    }

    /// At the end of a change: the values of the accounts that it moved are worked out again.
    pub(crate) fn revalue_moved(&self) -> Result<()> {
        let moved: Vec<i64> = std::mem::take(&mut *self.moved.borrow_mut()).into_iter().collect();
        if moved.is_empty() || !self.has_table("valuations")? {
            return Ok(());
        }
        let mut accounts = Vec::new();
        for ledger in moved {
            let account: Option<i64> = self
                .conn
                .prepare_cached(
                    "SELECT l.money_account_id FROM ledger_accounts l
                     WHERE l.id=?1 AND EXISTS (SELECT 1 FROM valuations v WHERE v.account_id=l.money_account_id)",
                )?
                .query_row([ledger], |r| r.get(0))
                .optional()?;
            accounts.extend(account);
        }
        accounts.sort_unstable();
        accounts.dedup();
        for account in accounts {
            self.revalue(account)?;
        }
        self.moved.borrow_mut().clear();
        Ok(())
    }

    /// Gives each entry of the values of an account the amount that takes its balance, on its day, to the value.
    fn revalue(&self, account: i64) -> Result<()> {
        let (ledger, currency) = self.money_ledger(account)?;
        let market = self.system_ledger(MARKET_VALUE)?;
        let rows: Vec<(i64, Cents)> = {
            let mut st = self.conn.prepare_cached(
                "SELECT v.entry_id, v.value FROM valuations v JOIN entries e ON e.id=v.entry_id
                 WHERE v.account_id=?1 ORDER BY e.date, v.entry_id",
            )?;
            st.query_map([account], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?
        };
        let mut changes: Cents = 0;
        for (entry, value) in rows {
            let mut d = self.load_draft(entry)?;
            // Everything else on the account up to the end of the day, as its register has it.
            let others: Cents = self
                .conn
                .prepare_cached(
                    "SELECT COALESCE(SUM(p.amount),0) FROM journal_postings p JOIN entries e ON e.id=p.entry_id
                 WHERE p.ledger_account_id=?1 AND COALESCE(p.date, e.date)<=?2
                   AND p.entry_id NOT IN (SELECT entry_id FROM valuations WHERE account_id=?3)",
                )?
                .query_row(params![ledger, d.date, account], |r| r.get(0))?;
            let change = value.checked_sub(others).and_then(|c| c.checked_sub(changes)).ok_or_else(too_big)?;
            changes = changes.checked_add(change).ok_or_else(too_big)?;
            let negated = change.checked_neg().ok_or_else(too_big)?;
            let mine = d.lines.iter().find(|l| l.account == ledger).and_then(|l| l.id);
            let theirs = d.lines.iter().find(|l| l.account != ledger).and_then(|l| l.id);
            let sound = d.lines.len() == 2
                && d.lines.iter().any(|l| l.account == ledger && l.amount == change && l.currency == currency)
                && d.lines.iter().any(|l| l.account == market && l.amount == negated && l.currency == currency);
            if sound {
                continue;
            }
            let keep = |id: Option<i64>| d.lines.iter().find(|l| l.id == id && id.is_some()).cloned();
            let mut mine_line = keep(mine).unwrap_or_default();
            let mut market_line = keep(theirs).unwrap_or_default();
            (mine_line.id, mine_line.account, mine_line.amount, mine_line.currency) =
                (mine, ledger, change, currency.clone());
            (market_line.id, market_line.account, market_line.amount, market_line.currency) =
                (theirs, market, negated, currency.clone());
            mine_line.date = None;
            market_line.date = None;
            d.lines = vec![mine_line, market_line];
            self.save_entry(&d)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
