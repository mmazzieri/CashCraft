//! Reconciling an account with the statement of its bank: the balance the bank states on a day against the
//! cleared balance of the books, and the lines of the statement against the transactions. A finished
//! reconciliation locks what it checked. `docs/reconcile.md` has the rules.

use crate::db::{AccountKind, Cents, Db, NewTx, Transaction, TxFilter, d2s, s2d};
use crate::duplicates::{self, State};
use crate::i18n::{tr, trf, trn};
use crate::importer::{Format, ParsedRow, Table};
use crate::ledger::{ADDED, too_big};
use crate::money::{fmt_date, parse_amount, parse_date};
use anyhow::{Result, anyhow, bail};
use chrono::NaiveDate;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fmt;

pub(crate) const RECONCILE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS reconciliations (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    statement_date TEXT NOT NULL,
    statement_balance INTEGER NOT NULL,
    source TEXT NOT NULL DEFAULT 'typed',
    state TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open','finished','undone')),
    started_at TEXT NOT NULL,
    finished_at TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS reconciliation_open ON reconciliations(account_id) WHERE state = 'open';
CREATE TABLE IF NOT EXISTS reconciliation_lines (
    id INTEGER PRIMARY KEY,
    reconciliation_id INTEGER NOT NULL REFERENCES reconciliations(id) ON DELETE CASCADE,
    line INTEGER NOT NULL,
    date TEXT NOT NULL,
    amount INTEGER NOT NULL,
    payee TEXT NOT NULL DEFAULT '',
    memo TEXT NOT NULL DEFAULT '',
    hash TEXT NOT NULL,
    marks TEXT NOT NULL DEFAULT '',
    card_row INTEGER NOT NULL DEFAULT 0,
    known_id INTEGER,
    posting_id INTEGER,
    was_cleared INTEGER,
    was_added INTEGER,
    added_keys TEXT,
    added_here INTEGER NOT NULL DEFAULT 0,
    UNIQUE (reconciliation_id, line)
);
CREATE INDEX IF NOT EXISTS reconciliation_line_posting ON reconciliation_lines(posting_id);
CREATE TABLE IF NOT EXISTS reconciled_postings (
    id INTEGER PRIMARY KEY,
    reconciliation_id INTEGER NOT NULL REFERENCES reconciliations(id) ON DELETE CASCADE,
    posting_id INTEGER NOT NULL,
    date TEXT NOT NULL,
    amount INTEGER NOT NULL,
    ledger_account_id INTEGER NOT NULL,
    released INTEGER NOT NULL DEFAULT 0,
    UNIQUE (reconciliation_id, posting_id)
);
CREATE INDEX IF NOT EXISTS reconciled_posting ON reconciled_postings(posting_id);
";

/// Whether the posting `p` of the entry `e` is as a finished reconciliation found it: reconciled.
pub(crate) const LIVE: &str =
    "EXISTS(SELECT 1 FROM reconciled_postings rp JOIN reconciliations rc ON rc.id=rp.reconciliation_id
    WHERE rp.posting_id=p.id AND rc.state='finished' AND rp.released=0 AND rp.amount=p.amount
      AND rp.ledger_account_id=p.ledger_account_id AND rp.date=COALESCE(p.date,e.date) AND p.cleared=1)";

/// The same, for the row `rp` of a reconciliation.
const ROW_LIVE: &str = "EXISTS(SELECT 1 FROM journal_postings p JOIN entries e ON e.id=p.entry_id
    WHERE p.id=rp.posting_id AND rp.amount=p.amount AND rp.ledger_account_id=p.ledger_account_id
      AND rp.date=COALESCE(p.date,e.date) AND p.cleared=1)";

/// The lines of a statement that a reconciliation keeps.
pub const MAX_LINES: usize = 20_000;

/// Changing these transactions would undo their reconciliation: the user says whether to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconciled {
    pub transactions: Vec<i64>,
    pub reconciliations: Vec<i64>,
}

impl fmt::Display for Reconciled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ids = self.transactions.iter().map(i64::to_string).collect::<Vec<_>>().join(", ");
        let text = if self.transactions.len() == 1 {
            trf(
                "transaction {ids} is reconciled: changing its amount, date or account, unclearing or deleting it undoes its reconciliation",
                &[("ids", &ids)],
            )
        } else {
            trf(
                "transactions {ids} are reconciled: changing their amount, date or account, unclearing or deleting them undoes their reconciliation",
                &[("ids", &ids)],
            )
        };
        f.write_str(&text)
    }
}

impl std::error::Error for Reconciled {}

/// The reconciled transactions that an error says a change would touch.
pub fn reconciled_in(e: &anyhow::Error) -> Option<&Reconciled> {
    e.downcast_ref::<Reconciled>()
}

/// Where the balance of the statement came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Typed,
    Csv,
    Xls,
    Ofx,
    Pdf,
}

impl Source {
    pub fn code(self) -> &'static str {
        match self {
            Source::Typed => "typed",
            Source::Csv => "csv",
            Source::Xls => "xls",
            Source::Ofx => "ofx",
            Source::Pdf => "pdf",
        }
    }

    fn from_code(s: &str) -> Self {
        [Source::Csv, Source::Xls, Source::Ofx, Source::Pdf]
            .into_iter()
            .find(|x| x.code() == s)
            .unwrap_or(Source::Typed)
    }

    pub fn of(format: Format) -> Self {
        match format {
            Format::Csv => Source::Csv,
            Format::Xls => Source::Xls,
            Format::Ofx => Source::Ofx,
            Format::Pdf => Source::Pdf,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Being done.
    Open,
    /// Finished, and the books still agree with it.
    Finished,
    /// Finished, but a transaction it checked was changed since, or the cleared balance on its day is another.
    Broken,
    /// Broken, and a later reconciliation of the account has been finished.
    Redone,
    Undone,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Open => tr("open"),
            Status::Finished => tr("finished"),
            Status::Broken => tr("broken: to be done again"),
            Status::Redone => tr("done again later"),
            Status::Undone => tr("undone"),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Reconciliation {
    pub id: i64,
    pub account_id: i64,
    pub account: String,
    pub currency: String,
    /// A credit card or a loan: a debt is a negative balance.
    pub liability: bool,
    pub statement_date: NaiveDate,
    /// As the books have it: a debt is negative.
    pub statement_balance: Cents,
    pub source: Source,
    pub status: Status,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// Of the books on the statement date: the opening balance and the cleared transactions up to that day.
    pub cleared_balance: Cents,
    /// Statement less books: zero to finish.
    pub difference: Cents,
    /// The transactions it checked, once finished.
    pub reconciled: usize,
    /// Those of them that are not as it found them any more.
    pub changed: Vec<i64>,
}

/// A line of the statement and the transaction it is.
#[derive(Clone, Debug, Serialize)]
pub struct LineView {
    /// Its number in the file, as the preview of the import has it.
    pub line: i64,
    pub date: NaiveDate,
    pub amount: Cents,
    pub payee: String,
    pub memo: String,
    /// The transaction it was matched with.
    pub matched: Option<Transaction>,
    /// The transaction that came from this very line, by its import key.
    pub exact: Option<Transaction>,
    /// The transaction of the same amount, near the date, that is proposed: the user confirms it.
    pub proposed: Option<Transaction>,
    /// The others it may be, the likeliest first.
    pub candidates: Vec<Transaction>,
    /// Same date, amount and payee, and nothing else like it.
    pub certain: bool,
    /// The transaction of this line was reconciled by another reconciliation.
    pub reconciled_before: Option<i64>,
    /// The reconciliation added it to the books.
    pub added: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Preview {
    pub reconciliation: Reconciliation,
    pub lines: Vec<LineView>,
    /// The transactions of the books up to the statement date that are not reconciled and that no line is: those
    /// that are cleared count in the balance.
    pub books_only: Vec<Transaction>,
    pub unmatched_lines: usize,
    /// Lines whose own transaction is there and not matched yet: they can be matched together.
    pub exact_pending: usize,
}

/// The balance that a statement states, as found in the file: a proposal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Stated {
    /// As the books have it: a debt is negative.
    pub balance: Cents,
    pub date: Option<NaiveDate>,
    /// What it was read from.
    pub label: String,
}

/// What a new reconciliation starts from.
#[derive(Clone, Debug)]
pub struct Start<'a> {
    pub account: i64,
    pub date: NaiveDate,
    pub balance: Cents,
    pub source: Source,
    /// The lines of the statement, in the order of the file; none when the balance is typed.
    pub rows: &'a [ParsedRow],
}

/// What [`Db::start_reconciliation`] did.
#[derive(Clone, Debug, Serialize)]
pub struct Started {
    pub id: i64,
    /// Lines dated after the statement date, left out.
    pub later_lines: usize,
    /// Lines that the bank takes back, left out.
    pub deletions: usize,
}

/// The cells of a table that may state the balance, best first.
const BALANCE_LABELS: [&str; 5] = ["saldo contabile", "saldo finale", "saldo al", "saldo disponibile", "saldo"];

/// The balance an OFX, a card statement or an Excel or CSV file states, if it does.
pub fn stated_balance(t: &Table, invert: bool, rows: &[ParsedRow]) -> Option<Stated> {
    let sign = |c: Cents| if invert { c.checked_neg() } else { Some(c) };
    if let Some((amount, date)) = t.ofx.as_ref().and_then(|o| o.statement()).and_then(|s| s.balance) {
        return Some(Stated { balance: sign(amount)?, date, label: "LEDGERBAL".into() });
    }
    if let Some(s) = &t.statement {
        // What is owed at the end of the statement: what the bank debits next, and what is left after it.
        let owed = s.debit?.checked_add(s.residual.unwrap_or(0))?;
        return Some(Stated {
            balance: owed.checked_neg()?,
            date: s.closing,
            label: tr("debt at the end of the statement").into(),
        });
    }
    let header = t.header_row.unwrap_or(0);
    let mut best: Option<(usize, Cents, String)> = None;
    let mut date = None;
    for (i, record) in t.records.iter().enumerate() {
        // Rows of transactions are not read: one may have "saldo" in its text.
        if t.header_row.is_some() && i > header && i < header.saturating_add(1).saturating_add(rows.len()) {
            continue;
        }
        for (c, cell) in record.iter().enumerate() {
            let text = cell.trim().to_lowercase();
            if date.is_none() {
                date = date_in(&text);
            }
            if !text.starts_with("saldo") || ["iniziale", "precedente", "punti"].iter().any(|w| text.contains(w)) {
                continue;
            }
            let Some(rank) = BALANCE_LABELS.iter().position(|l| text.starts_with(l)) else { continue };
            let figure = record.iter().skip(c + 1).map(|x| x.trim()).find(|x| !x.is_empty());
            let Some(amount) = figure.and_then(|x| parse_amount(&x.replace('€', "")).ok()) else { continue };
            if best.as_ref().is_none_or(|(r, _, _)| rank < *r) {
                best = Some((rank, amount, cell.trim().to_string()));
            }
        }
    }
    let (_, amount, label) = best?;
    Some(Stated { balance: sign(amount)?, date: date.or_else(|| rows.iter().map(|r| r.date).max()), label })
}

/// A date at the end of a text such as "Dati aggiornati al 01 dicembre 2025" or "Lunedì,01 dicembre 2025".
fn date_in(text: &str) -> Option<NaiveDate> {
    let tail = text.rsplit([',', ':']).next().unwrap_or(text);
    let tail = tail.rsplit_once(" al ").map_or(tail, |(_, t)| t);
    let tail = tail.trim();
    (tail.chars().any(|c| c.is_ascii_digit()) && tail.len() <= 30).then(|| parse_date(tail)).flatten()
}

/// A row of `reconciliations`.
struct Row {
    id: i64,
    account: i64,
    date: NaiveDate,
    balance: Cents,
    source: Source,
    state: String,
    started_at: String,
    finished_at: Option<String>,
}

/// A row of `reconciliation_lines`.
#[derive(Clone)]
struct Line {
    id: i64,
    line: i64,
    date: NaiveDate,
    amount: Cents,
    payee: String,
    memo: String,
    hash: String,
    marks: String,
    card_row: bool,
    known: Option<i64>,
    posting: Option<i64>,
    was_cleared: Option<bool>,
    was_added: Option<bool>,
    added_keys: Option<String>,
    added_here: bool,
}

fn no_reconciliation(id: i64) -> anyhow::Error {
    anyhow!(trf("reconciliation {id} not found", &[("id", &id)]))
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

impl Db {
    /// Remembers the reconciled postings that a change touches, before it touches them.
    pub(crate) fn touch(&self, postings: &[i64]) -> Result<()> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT rp.id FROM reconciled_postings rp JOIN reconciliations rc ON rc.id=rp.reconciliation_id
             WHERE rp.posting_id=?1 AND rc.state='finished' AND rp.released=0 AND {ROW_LIVE}"
        ))?;
        for id in postings {
            if self.touched.borrow().contains_key(id) {
                continue;
            }
            let rows: Vec<i64> = st.query_map([id], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
            self.touched.borrow_mut().insert(*id, rows);
        }
        Ok(())
    }

    /// Refuses a change that leaves a reconciled transaction other than its reconciliation found it, unless the
    /// user said so ([`Db::unreconciling`]): then the reconciliation no longer counts it, and shows as broken.
    pub(crate) fn check_reconciled(&self) -> Result<()> {
        let touched = std::mem::take(&mut *self.touched.borrow_mut());
        let rows: Vec<i64> = touched.into_values().flatten().collect();
        if rows.is_empty() {
            return Ok(());
        }
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT rp.posting_id, rp.reconciliation_id FROM reconciled_postings rp WHERE rp.id=?1 AND NOT {ROW_LIVE}"
        ))?;
        let mut gone = Vec::new();
        for row in &rows {
            if let Some(found) = st.query_row([row], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).optional()? {
                gone.push((*row, found));
            }
        }
        if gone.is_empty() {
            return Ok(());
        }
        if self.unlocking.get() {
            for (row, _) in &gone {
                self.conn.execute("UPDATE reconciled_postings SET released=1 WHERE id=?1", [row])?;
            }
            return Ok(());
        }
        let mut transactions: Vec<i64> = gone.iter().map(|(_, (p, _))| *p).collect();
        let mut reconciliations: Vec<i64> = gone.iter().map(|(_, (_, r))| *r).collect();
        for v in [&mut transactions, &mut reconciliations] {
            v.sort_unstable();
            v.dedup();
        }
        Err(Reconciled { transactions, reconciliations }.into())
    }

    /// Runs `f` letting it change reconciled transactions: their reconciliations no longer count them. The user
    /// has said so, after [`Reconciled`].
    pub fn unreconciling<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let before = self.unlocking.replace(true);
        let out = if self.conn.is_autocommit() {
            self.atomically(f)
        } else {
            f().and_then(|v| self.check_reconciled().map(|()| v))
        };
        self.unlocking.set(before);
        out
    }

    fn reconciliation_row(&self, id: i64) -> Result<Row> {
        self.conn
            .query_row(
                "SELECT id, account_id, statement_date, statement_balance, source, state, started_at, finished_at
                 FROM reconciliations WHERE id=?1",
                [id],
                |r| {
                    Ok(Row {
                        id: r.get(0)?,
                        account: r.get(1)?,
                        date: s2d(&r.get::<_, String>(2)?),
                        balance: r.get(3)?,
                        source: Source::from_code(&r.get::<_, String>(4)?),
                        state: r.get(5)?,
                        started_at: r.get(6)?,
                        finished_at: r.get(7)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| no_reconciliation(id))
    }

    fn open_row(&self, id: i64) -> Result<Row> {
        let row = self.reconciliation_row(id)?;
        if row.state != "open" {
            bail!(trf("reconciliation {id} is not open", &[("id", &id)]));
        }
        Ok(row)
    }

    /// The opening balance and the cleared transactions of an account up to the end of a day.
    pub fn cleared_balance(&self, account: i64, date: NaiveDate) -> Result<Cents> {
        let (ledger, _) = self.money_ledger(account)?;
        let sum: Option<i64> = self.conn.query_row(
            "SELECT SUM(p.amount) FROM journal_postings p JOIN entries e ON e.id=p.entry_id
             WHERE p.ledger_account_id=?1 AND (p.cleared=1 OR e.opening_account_id IS NOT NULL)
               AND COALESCE(p.date,e.date) <= ?2",
            params![ledger, d2s(date)],
            |r| r.get(0),
        )?;
        Ok(sum.unwrap_or(0))
    }

    fn lines(&self, id: i64) -> Result<Vec<Line>> {
        let mut st = self.conn.prepare(
            "SELECT id, line, date, amount, payee, memo, hash, marks, card_row, known_id, posting_id, was_cleared,
                    was_added, added_keys, added_here
             FROM reconciliation_lines WHERE reconciliation_id=?1 ORDER BY line",
        )?;
        let rows = st.query_map([id], |r| {
            Ok(Line {
                id: r.get(0)?,
                line: r.get(1)?,
                date: s2d(&r.get::<_, String>(2)?),
                amount: r.get(3)?,
                payee: r.get(4)?,
                memo: r.get(5)?,
                hash: r.get(6)?,
                marks: r.get(7)?,
                card_row: r.get(8)?,
                known: r.get(9)?,
                posting: r.get(10)?,
                was_cleared: r.get(11)?,
                was_added: r.get(12)?,
                added_keys: r.get(13)?,
                added_here: r.get(14)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// What a reconciliation is, with its balances now.
    fn describe(&self, row: &Row) -> Result<Reconciliation> {
        let acc =
            self.accounts(true)?.into_iter().find(|a| a.id == row.account).ok_or_else(|| no_reconciliation(row.id))?;
        let cleared = self.cleared_balance(row.account, row.date)?;
        let difference = row.balance.checked_sub(cleared).ok_or_else(too_big)?;
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT rp.posting_id, rp.released=0 AND {ROW_LIVE} FROM reconciled_postings rp WHERE rp.reconciliation_id=?1"
        ))?;
        let snapshot: Vec<(i64, bool)> =
            st.query_map([row.id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?;
        let changed: Vec<i64> = snapshot.iter().filter(|(_, live)| !live).map(|(p, _)| *p).collect();
        let status = match row.state.as_str() {
            "open" => Status::Open,
            "undone" => Status::Undone,
            _ if changed.is_empty() && difference == 0 => Status::Finished,
            _ => {
                let later: bool = self.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM reconciliations WHERE account_id=?1 AND state='finished'
                                   AND statement_date >= ?2 AND id > ?3)",
                    params![row.account, d2s(row.date), row.id],
                    |r| r.get(0),
                )?;
                if later { Status::Redone } else { Status::Broken }
            }
        };
        Ok(Reconciliation {
            id: row.id,
            account_id: row.account,
            account: acc.name,
            currency: acc.currency,
            liability: matches!(acc.kind, AccountKind::CreditCard | AccountKind::Loan),
            statement_date: row.date,
            statement_balance: row.balance,
            source: row.source,
            status,
            started_at: row.started_at.clone(),
            finished_at: row.finished_at.clone(),
            cleared_balance: cleared,
            difference,
            reconciled: snapshot.len(),
            changed,
        })
    }

    pub fn reconciliation(&self, id: i64) -> Result<Reconciliation> {
        self.describe(&self.reconciliation_row(id)?)
    }

    /// The reconciliations, of one account or of all, the latest first.
    pub fn reconciliations(&self, account: Option<i64>) -> Result<Vec<Reconciliation>> {
        let mut st = self.conn.prepare(
            "SELECT id FROM reconciliations WHERE ?1 IS NULL OR account_id=?1 ORDER BY statement_date DESC, id DESC",
        )?;
        let ids: Vec<i64> = st.query_map([account], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
        ids.into_iter().map(|id| self.reconciliation(id)).collect()
    }

    /// The reconciliation of an account that is being done.
    pub fn open_reconciliation(&self, account: i64) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row("SELECT id FROM reconciliations WHERE account_id=?1 AND state='open'", [account], |r| r.get(0))
            .optional()?)
    }

    /// The last finished reconciliation of an account: the one that can be undone.
    fn last_finished(&self, account: i64) -> Result<Option<(i64, NaiveDate)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, statement_date FROM reconciliations WHERE account_id=?1 AND state='finished'
                 ORDER BY statement_date DESC, id DESC LIMIT 1",
                [account],
                |r| Ok((r.get(0)?, s2d(&r.get::<_, String>(1)?))),
            )
            .optional()?)
    }

    /// Starts reconciling an account with a statement. One at a time for each account, on the day of the last
    /// finished one or later.
    pub fn start_reconciliation(&self, s: &Start) -> Result<Started> {
        if s.rows.len() > MAX_LINES {
            bail!(trf("a reconciliation has at most {n} lines", &[("n", &MAX_LINES)]));
        }
        if s.balance.unsigned_abs() > crate::ledger::MAX_BALANCE.unsigned_abs() {
            bail!(tr("the amount is too large"));
        }
        self.atomically(|| {
            self.money_ledger(s.account)?;
            if let Some(open) = self.open_reconciliation(s.account)? {
                bail!(trf(
                    "the account is being reconciled already (reconciliation {id}): finish or undo it first",
                    &[("id", &open)]
                ));
            }
            if let Some((_, last)) = self.last_finished(s.account)?
                && s.date < last
            {
                bail!(trf(
                    "the account was reconciled up to {date}: a new reconciliation starts on that day or later",
                    &[("date", &fmt_date(last))]
                ));
            }
            self.conn.execute(
                "INSERT INTO reconciliations(id, account_id, statement_date, statement_balance, source, started_at)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![self.new_id("reconciliations")?, s.account, d2s(s.date), s.balance, s.source.code(), now()],
            )?;
            let id = self.conn.last_insert_rowid();
            let plan = self.plan_import(Some(s.account), s.rows)?;
            let mut out = Started { id, later_lines: 0, deletions: 0 };
            let mut st = self.conn.prepare_cached(
                "INSERT INTO reconciliation_lines(id, reconciliation_id, line, date, amount, payee, memo, hash, marks,
                                                  card_row, known_id)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            )?;
            for (i, (row, p)) in s.rows.iter().zip(&plan.rows).enumerate() {
                if row.date > s.date {
                    out.later_lines += 1;
                    continue;
                }
                let known = match p.state {
                    State::Deletes(_) => {
                        out.deletions += 1;
                        continue;
                    }
                    State::Known(id) => Some(id),
                    _ => None,
                };
                st.execute(params![
                    self.new_id("reconciliation_lines")?,
                    id,
                    i + 1,
                    d2s(row.date),
                    row.amount,
                    row.payee,
                    row.memo,
                    p.hash,
                    p.marks,
                    row.key.is_some(),
                    known
                ])?;
            }
            Ok(out)
        })
    }

    /// The reconciliation with its lines, what they may be, and the transactions that no line is.
    pub fn preview_reconciliation(&self, id: i64) -> Result<Preview> {
        let row = self.reconciliation_row(id)?;
        let reconciliation = self.describe(&row)?;
        let lines = self.lines(id)?;
        let all = self.transactions(&TxFilter { account_id: Some(row.account), ..Default::default() })?;
        let by_id: HashMap<i64, &Transaction> = all.iter().map(|t| (t.id, t)).collect();
        let get = |id: Option<i64>| id.and_then(|i| by_id.get(&i)).map(|t| (*t).clone());
        let open = row.state == "open";
        let matched: HashSet<i64> = lines.iter().filter_map(|l| l.posting).collect();
        // Each transaction by the keys of the lines it came from.
        let mut by_key: HashMap<&str, i64> = HashMap::new();
        for t in all.iter().rev() {
            for k in t.keys.split_whitespace() {
                by_key.entry(k).or_insert(t.id);
            }
        }
        let mut claimed: HashSet<i64> = HashSet::new();
        let mut views = Vec::with_capacity(lines.len());
        let mut exact_of: Vec<(Option<i64>, Option<i64>)> = Vec::with_capacity(lines.len());
        for l in &lines {
            let own = by_key.get(l.hash.as_str()).copied().or(l.known).filter(|i| by_id.contains_key(i));
            let (mut exact, mut before) = (None, None);
            if l.posting.is_none()
                && let Some(t) = own.and_then(|i| by_id.get(&i))
            {
                if t.reconciled {
                    before = Some(t.id);
                } else if !matched.contains(&t.id) && t.date <= row.date && claimed.insert(t.id) {
                    exact = Some(t.id);
                }
            }
            exact_of.push((exact, before));
        }
        // The other lines, against the transactions that are free, as an import pairs them.
        let free: Vec<Transaction> = all
            .iter()
            .filter(|t| !t.reconciled && t.date <= row.date && !matched.contains(&t.id) && !claimed.contains(&t.id))
            .cloned()
            .collect();
        let rows: Vec<ParsedRow> = lines
            .iter()
            .map(|l| ParsedRow {
                date: l.date,
                amount: l.amount,
                payee: l.payee.clone(),
                memo: l.memo.clone(),
                ..Default::default()
            })
            .collect();
        let ask: Vec<usize> = lines
            .iter()
            .zip(&exact_of)
            .enumerate()
            .filter(|(_, (l, (e, b)))| open && l.posting.is_none() && e.is_none() && b.is_none())
            .map(|(i, _)| i)
            .collect();
        let proposals = duplicates::propose(&rows, &ask, &free);
        for (i, (l, (exact, before))) in lines.iter().zip(&exact_of).enumerate() {
            let p = proposals.get(&i);
            views.push(LineView {
                line: l.line,
                date: l.date,
                amount: l.amount,
                payee: l.payee.clone(),
                memo: l.memo.clone(),
                matched: get(l.posting),
                exact: get(*exact),
                proposed: get(p.and_then(|p| p.proposed)),
                candidates: p.map(|p| p.candidates.iter().filter_map(|c| get(Some(*c))).collect()).unwrap_or_default(),
                certain: p.is_some_and(|p| p.certain),
                reconciled_before: *before,
                added: l.added_here,
            });
        }
        // What is proposed for a line is shown there.
        let proposed: HashSet<i64> = proposals.values().filter_map(|p| p.proposed).collect();
        let books_only = if open {
            let mut b: Vec<Transaction> = all
                .iter()
                .filter(|t| !t.reconciled && t.date <= row.date && !matched.contains(&t.id) && !claimed.contains(&t.id))
                .filter(|t| !proposed.contains(&t.id))
                .cloned()
                .collect();
            b.sort_by_key(|t| (t.date, t.id));
            b
        } else {
            Vec::new()
        };
        let unmatched_lines = views.iter().filter(|v| v.matched.is_none() && v.reconciled_before.is_none()).count();
        let exact_pending = views.iter().filter(|v| v.exact.is_some()).count();
        Ok(Preview { reconciliation, lines: views, books_only, unmatched_lines, exact_pending })
    }

    /// Says that lines of the statement are these transactions: each is cleared, and keeps the keys of its line so
    /// that the statement imported later adds nothing. Pairs of line number and transaction id.
    pub fn match_lines(&self, id: i64, pairs: &[(i64, i64)]) -> Result<usize> {
        self.atomically(|| {
            let row = self.open_row(id)?;
            let lines = self.lines(id)?;
            let (ledger, _) = self.money_ledger(row.account)?;
            let mut used: HashSet<i64> = lines.iter().filter_map(|l| l.posting).collect();
            let mut done: HashSet<i64> = HashSet::new();
            for (number, posting) in pairs {
                let l = lines
                    .iter()
                    .find(|l| l.line == *number)
                    .ok_or_else(|| anyhow!(trf("line {n} is not on the statement", &[("n", number)])))?;
                if l.posting.is_some() || !done.insert(*number) {
                    bail!(trf("line {n} is matched already", &[("n", number)]));
                }
                let t = self.transaction(*posting)?;
                if t.account_id != row.account {
                    bail!(trf("transaction {id} is not of the account", &[("id", posting)]));
                }
                if t.reconciled {
                    bail!(trf("transaction {id} is reconciled already", &[("id", posting)]));
                }
                if t.date > row.date {
                    bail!(trf("transaction {id} is dated after the statement", &[("id", posting)]));
                }
                if t.amount != l.amount {
                    bail!(trf("transaction {id} has another amount than line {n}", &[("id", posting), ("n", number)]));
                }
                if !used.insert(*posting) {
                    bail!(trf("transaction {id} is matched with another line", &[("id", posting)]));
                }
                // The keys it does not have yet: those alone go again when the match is undone.
                let has: HashSet<&str> = t.keys.split_whitespace().collect();
                let mut seen = HashSet::new();
                let keys: Vec<&str> = std::iter::once(l.hash.as_str())
                    .chain(l.marks.split_whitespace())
                    .filter(|k| !has.contains(k) && seen.insert(*k))
                    .collect();
                if !keys.is_empty() {
                    self.add_import_keys(*posting, &keys)?;
                }
                let was_added = self.set_seen(*posting, ledger, true, None)?;
                self.conn.execute(
                    "UPDATE reconciliation_lines SET posting_id=?2, was_cleared=?3, was_added=?4, added_keys=?5
                     WHERE id=?1",
                    params![l.id, posting, t.cleared, was_added, (!keys.is_empty()).then(|| keys.join(" "))],
                )?;
            }
            Ok(pairs.len())
        })
    }

    /// Clears a transaction and takes away the mark of an added side of a transfer, or puts both back. Whether it
    /// had the mark.
    fn set_seen(&self, posting: i64, ledger: i64, cleared: bool, added: Option<bool>) -> Result<bool> {
        let entry = self.entry_of_transaction(posting)?.ok_or_else(|| crate::db::no_transaction(posting))?;
        let mut d = self.load_draft(entry)?;
        let mut was = false;
        for l in d.lines.iter_mut().filter(|l| l.id == Some(posting) && l.account == ledger) {
            was = l.origin.as_deref() == Some(ADDED);
            l.cleared = cleared;
            match added {
                None if was => l.origin = None,
                Some(true) if l.origin.is_none() => l.origin = Some(ADDED.into()),
                _ => {}
            }
        }
        self.save_entry(&d)?;
        Ok(was)
    }

    /// Undoes matches: what they changed of each transaction is put back, if it is still as they left it.
    pub fn unmatch_lines(&self, id: i64, numbers: &[i64]) -> Result<usize> {
        self.atomically(|| {
            self.open_row(id)?;
            let lines = self.lines(id)?;
            for n in numbers {
                let l = lines
                    .iter()
                    .find(|l| l.line == *n)
                    .ok_or_else(|| anyhow!(trf("line {n} is not on the statement", &[("n", n)])))?;
                if l.posting.is_none() {
                    bail!(trf("line {n} is not matched", &[("n", n)]));
                }
                self.put_back(l)?;
            }
            Ok(numbers.len())
        })
    }

    /// What a match changed of its transaction, back as it was, and the line free.
    fn put_back(&self, l: &Line) -> Result<()> {
        if let Some(posting) = l.posting
            && let Ok(t) = self.transaction(posting)
            && !l.added_here
        {
            if let Some(keys) = &l.added_keys {
                self.remove_import_keys(posting, &keys.split_whitespace().collect::<Vec<_>>())?;
            }
            let (ledger, _) = self.money_ledger(t.account_id)?;
            if t.cleared && !t.reconciled {
                self.set_seen(posting, ledger, l.was_cleared.unwrap_or(true), Some(l.was_added.unwrap_or(false)))?;
            }
        }
        self.conn.execute(
            "UPDATE reconciliation_lines SET posting_id=NULL, was_cleared=NULL, was_added=NULL, added_keys=NULL,
                    added_here=0 WHERE id=?1",
            [l.id],
        )?;
        Ok(())
    }

    /// Adds to the books lines of the statement that are not there, cleared and with the keys of the import, so that
    /// the statement imported later adds them no more. The rules run on them as on an import.
    pub fn add_lines(&self, id: i64, numbers: &[i64]) -> Result<Vec<i64>> {
        self.atomically(|| {
            let row = self.open_row(id)?;
            let lines = self.lines(id)?;
            let rules = self.ruleset()?;
            let mut out = Vec::new();
            let mut done = HashSet::new();
            for n in numbers {
                let l = lines
                    .iter()
                    .find(|l| l.line == *n)
                    .ok_or_else(|| anyhow!(trf("line {n} is not on the statement", &[("n", n)])))?;
                if l.posting.is_some() || !done.insert(*n) {
                    bail!(trf("line {n} is matched already", &[("n", n)]));
                }
                let new = NewTx {
                    account_id: row.account,
                    date: l.date,
                    amount: l.amount,
                    payee: l.payee.clone(),
                    memo: l.memo.clone(),
                    category_id: None,
                    cleared: true,
                    import_hash: Some(l.hash.clone()),
                    import_marks: Some(l.marks.clone()),
                    card_row: l.card_row,
                };
                let Some(posting) = self.add_with_rules(&new, Some(&rules))? else {
                    bail!(trf("line {n} is in the books already: match it", &[("n", n)]));
                };
                self.conn.execute(
                    "UPDATE reconciliation_lines SET posting_id=?2, was_cleared=1, was_added=0, added_here=1 WHERE id=?1",
                    params![l.id, posting],
                )?;
                out.push(posting);
            }
            Ok(out)
        })
    }

    /// Clears transactions of the account up to the statement date, or not: for a statement without lines this is
    /// how the books reach its balance.
    pub fn clear_for_reconciliation(&self, id: i64, postings: &[i64], cleared: bool) -> Result<usize> {
        self.atomically(|| {
            let row = self.open_row(id)?;
            let (ledger, _) = self.money_ledger(row.account)?;
            for posting in postings {
                let t = self.transaction(*posting)?;
                if t.account_id != row.account {
                    bail!(trf("transaction {id} is not of the account", &[("id", posting)]));
                }
                if t.reconciled {
                    bail!(trf("transaction {id} is reconciled already", &[("id", posting)]));
                }
                if t.date > row.date {
                    bail!(trf("transaction {id} is dated after the statement", &[("id", posting)]));
                }
                let entry = self.entry_of_transaction(*posting)?.ok_or_else(|| crate::db::no_transaction(*posting))?;
                let mut d = self.load_draft(entry)?;
                for l in d.lines.iter_mut().filter(|l| l.id == Some(*posting) && l.account == ledger) {
                    l.cleared = cleared;
                }
                self.save_entry(&d)?;
            }
            Ok(postings.len())
        })
    }

    /// Finishes a reconciliation whose difference is zero: the cleared transactions up to its day that no other
    /// reconciliation has are reconciled, and locked.
    pub fn finish_reconciliation(&self, id: i64) -> Result<Reconciliation> {
        self.atomically(|| {
            let row = self.open_row(id)?;
            let r = self.describe(&row)?;
            if r.difference != 0 {
                bail!(trf(
                    "the statement and the books differ by {amount}: the reconciliation cannot be finished",
                    &[("amount", &crate::money::fmt_plain(r.difference))]
                ));
            }
            // A line ticked on the side of the books has not the keys of its line: the statement imported later
            // would add it again.
            let left = self.preview_reconciliation(id)?.unmatched_lines;
            if left > 0 {
                bail!(trn(
                    left,
                    "{n} line of the statement is not matched: match it, or add it to the books",
                    "{n} lines of the statement are not matched: match them, or add them to the books"
                ));
            }
            let (ledger, _) = self.money_ledger(row.account)?;
            let mut st = self.conn.prepare(&format!(
                "SELECT p.id, COALESCE(p.date,e.date), p.amount, p.origin FROM journal_postings p JOIN entries e ON e.id=p.entry_id
                 WHERE p.ledger_account_id=?1 AND p.cleared=1 AND e.opening_account_id IS NULL
                   AND COALESCE(p.date,e.date) <= ?2 AND NOT {LIVE} ORDER BY p.id"
            ))?;
            let found: Vec<(i64, String, Cents, Option<String>)> = st
                .query_map(params![ledger, d2s(row.date)], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<std::result::Result<_, _>>()?;
            for (posting, date, amount, origin) in &found {
                // On a statement now: no line imported later takes its place.
                if origin.as_deref() == Some(ADDED) {
                    self.set_seen(*posting, ledger, true, None)?;
                }
                self.conn.execute(
                    "INSERT INTO reconciled_postings(id, reconciliation_id, posting_id, date, amount, ledger_account_id)
                     VALUES(?1,?2,?3,?4,?5,?6)",
                    params![self.new_id("reconciled_postings")?, id, posting, date, amount, ledger],
                )?;
            }
            self.conn.execute(
                "UPDATE reconciliations SET state='finished', finished_at=?2 WHERE id=?1",
                params![id, now()],
            )?;
            self.reconciliation(id)
        })
    }

    /// Undoes a reconciliation. One that is open goes, and what its matches changed is put back where it is as they
    /// left it; the transactions it added stay. A finished one must be the last of its account: its transactions
    /// are no longer reconciled and stay cleared, and it stays in the list as undone.
    pub fn undo_reconciliation(&self, id: i64) -> Result<String> {
        self.atomically(|| {
            let row = self.reconciliation_row(id)?;
            match row.state.as_str() {
                "open" => {
                    let lines = self.lines(id)?;
                    let added = lines.iter().filter(|l| l.added_here).count();
                    for l in lines.iter().filter(|l| l.posting.is_some()) {
                        self.put_back(l)?;
                    }
                    self.conn.execute("DELETE FROM reconciliations WHERE id=?1", [id])?;
                    let mut message = tr("the reconciliation was discarded").to_string();
                    if added > 0 {
                        let kept = trn(added, "{n} transaction it added stays", "{n} transactions it added stay");
                        message = format!("{message}; {kept}");
                    }
                    Ok(message)
                }
                "finished" => {
                    if self.open_reconciliation(row.account)?.is_some() {
                        bail!(tr("the account is being reconciled: finish or undo that reconciliation first"));
                    }
                    if self.last_finished(row.account)?.map(|(last, _)| last) != Some(id) {
                        bail!(tr("only the last reconciliation of an account can be undone"));
                    }
                    self.conn.execute("UPDATE reconciliations SET state='undone' WHERE id=?1", [id])?;
                    Ok(tr("the reconciliation was undone: its transactions are no longer reconciled").into())
                }
                _ => bail!(trf("reconciliation {id} is undone already", &[("id", &id)])),
            }
        })
    }
}

#[cfg(test)]
mod tests;
