//! Which rows of a statement are in the books already. One with the same key is skipped; one that only looks
//! the same is never guessed: the user says what it is.

use crate::db::{Cents, Db, Transaction, TxFilter, s2d};
use crate::i18n::{tr, trf};
use crate::importer::{ParsedRow, fnv};
use crate::money::{fmt_date, fmt_plain};
use anyhow::{Result, anyhow, bail};
use chrono::{Days, NaiveDate};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// How far apart two statements may date the same transaction.
const MAX_DAYS: u64 = 1;
/// How many transactions are shown for a row.
const SHOWN: usize = 5;
/// How many rows an error lists.
const LISTED: usize = 20;

/// What to do with the rows that may be in the books.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Decision {
    /// The import is refused and they are listed.
    #[default]
    Ask,
    /// They are the same: none is added.
    SkipAll,
    /// They are others: all are added.
    AddAll,
    /// For each, by its place in the rows: whether it is the same and is skipped.
    Each(BTreeMap<usize, bool>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// A transaction of another statement has this amount and almost this date.
    SameAmount,
    /// The bank gave its FITID to another date or amount before.
    SameId,
    /// The file has it twice, FITID and all.
    Repeated,
    /// The bank says that it corrects another one.
    Corrects,
}

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Reason::SameAmount => tr("same amount, almost the same date"),
            Reason::SameId => tr("same bank id, another date or amount"),
            Reason::Repeated => tr("twice in the file"),
            Reason::Corrects => tr("corrects another transaction"),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LikelyDuplicate {
    /// Its place in the rows, from 0.
    pub index: usize,
    pub reason: Reason,
    pub date: NaiveDate,
    pub payee: String,
    pub amount: Cents,
    /// What is in the books already, the first few.
    pub existing: Vec<Transaction>,
}

struct Imported {
    id: i64,
    keys: Vec<String>,
    card: bool,
}

/// What each row is.
pub(crate) enum State {
    Known,
    New,
    Likely(Reason, Vec<i64>),
}

pub(crate) struct Planned {
    pub hash: String,
    pub state: State,
    /// The first row of the file that this one repeats.
    pub repeats: Option<usize>,
}

pub(crate) struct Plan {
    pub rows: Vec<Planned>,
    /// Imported transactions by amount and date, without those that a row of the file is for sure.
    free: HashMap<(Cents, NaiveDate), Vec<i64>>,
    known: HashMap<String, i64>,
}

const OFX: &str = "ofx:";

fn id_part(fitid: &str) -> String {
    format!("{OFX}{:016x}:", fnv(fitid))
}

fn around(date: NaiveDate) -> impl Iterator<Item = NaiveDate> {
    let first = date.checked_sub_days(Days::new(MAX_DAYS)).unwrap_or(date);
    first.iter_days().take(2 * MAX_DAYS as usize + 1)
}

/// The key of each row: what tells it from the others of the account. The FITID alone would drop the
/// transactions of banks that use it twice, so the date and the amount are in it too.
fn hashes(rows: &[ParsedRow]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    rows.iter()
        .map(|row| {
            let (prefix, key) = match (&row.key, row.ofx, &row.fitid) {
                (Some(k), _, _) => (String::new(), k.clone()),
                (None, true, Some(id)) => (id_part(id), format!("{}|{}", row.date, row.amount)),
                (None, true, None) => {
                    (format!("{OFX}:"), format!("{}|{}|{}", row.date, row.amount, row.payee.to_lowercase()))
                }
                (None, false, _) => (
                    String::new(),
                    format!("{}|{}|{}|{}", row.date, row.amount, row.payee.to_lowercase(), row.memo.to_lowercase()),
                ),
            };
            let n = seen.entry(format!("{prefix}{key}")).or_default();
            *n += 1;
            format!("{prefix}{:016x}", fnv(&format!("{key}|{n}")))
        })
        .collect()
}

impl Db {
    /// Compares the rows with the books; `None` is an account that is not there yet.
    pub(crate) fn plan_import(&self, account: Option<i64>, rows: &[ParsedRow]) -> Result<Plan> {
        let mut imported = Vec::new();
        let mut known: HashMap<String, i64> = HashMap::new();
        let mut places: Vec<(Cents, NaiveDate)> = Vec::new();
        if let Some(a) = account {
            let (ledger, _) = self.money_ledger(a)?;
            let mut st = self.conn.prepare(
                "SELECT p.id, p.amount, COALESCE(p.date, e.date), p.import_hash, COALESCE(p.other_hashes,''), p.origin
                 FROM postings p JOIN entries e ON e.id=p.entry_id
                 WHERE p.ledger_account_id=?1 AND (p.import_hash IS NOT NULL OR p.other_hashes IS NOT NULL)
                 ORDER BY p.id",
            )?;
            let mut found = st.query([ledger])?;
            while let Some(r) = found.next()? {
                let id: i64 = r.get(0)?;
                let first: Option<String> = r.get(3)?;
                let others: String = r.get(4)?;
                let keys: Vec<String> =
                    first.into_iter().chain(others.split_whitespace().map(str::to_string)).collect();
                for k in &keys {
                    known.entry(k.clone()).or_insert(id);
                }
                places.push((r.get(1)?, s2d(&r.get::<_, String>(2)?)));
                let card = r.get::<_, Option<String>>(5)?.as_deref() == Some(crate::ledger::CARD_ROW);
                imported.push(Imported { id, keys, card });
            }
        }
        let hashes = hashes(rows);
        let taken: HashSet<i64> = hashes.iter().filter_map(|h| known.get(h).copied()).collect();
        // Rows of OFX are compared with every other; those of other files only with what came from OFX alone.
        let ofx = rows.iter().any(|r| r.ofx);
        let mut free: HashMap<(Cents, NaiveDate), Vec<i64>> = HashMap::new();
        let mut by_id: HashMap<&str, Vec<i64>> = HashMap::new();
        for (i, place) in imported.iter().zip(&places) {
            for k in i.keys.iter().filter(|k| k.starts_with(OFX)) {
                if let Some((prefix, _)) = k.rsplit_once(':') {
                    let ids = by_id.entry(prefix).or_default();
                    // Two are enough to say that the FITID tells nothing.
                    if ids.len() < 2 && !ids.contains(&i.id) {
                        ids.push(i.id);
                    }
                }
            }
            if !taken.contains(&i.id) && (ofx || (!i.card && i.keys.iter().all(|k| k.starts_with(OFX)))) {
                free.entry(*place).or_default().push(i.id);
            }
        }
        // A bank that uses a FITID for several transactions does not tell them apart by it.
        let mut uses: HashMap<&str, (NaiveDate, Cents, bool)> = HashMap::new();
        for r in rows {
            if let Some(id) = &r.fitid {
                let first = uses.entry(id).or_insert((r.date, r.amount, true));
                first.2 &= (first.0, first.1) == (r.date, r.amount);
            }
        }
        let trusted = |id: &str| uses.get(id).is_some_and(|u| u.2);
        let holder = |id: &str| match by_id.get(id_part(id).trim_end_matches(':')).map(Vec::as_slice) {
            Some([one]) => Some(*one),
            _ => None,
        };
        let mut first_of: HashMap<(&str, &str, &str), usize> = HashMap::new();
        let mut out = Vec::new();
        for (index, (row, hash)) in rows.iter().zip(hashes).enumerate() {
            let mut repeats = None;
            if let Some(id) = row.fitid.as_deref().filter(|id| trusted(id)) {
                match first_of.get(&(id, row.payee.as_str(), row.memo.as_str())) {
                    Some(i) => repeats = Some(*i),
                    None => {
                        first_of.insert((id, &row.payee, &row.memo), index);
                    }
                }
            }
            let state = if known.contains_key(&hash) {
                State::Known
            } else if let Some(c) = &row.corrects {
                State::Likely(Reason::Corrects, holder(c).into_iter().collect())
            } else if let Some(p) =
                row.fitid.as_deref().filter(|id| trusted(id)).and_then(holder).filter(|p| !taken.contains(p))
            {
                State::Likely(Reason::SameId, vec![p])
            } else if repeats.is_some() {
                State::Likely(Reason::Repeated, Vec::new())
            } else {
                let near: Vec<i64> = around(row.date)
                    .filter_map(|d| free.get(&(row.amount, d)))
                    .flat_map(|ids| ids.iter().copied().take(SHOWN))
                    .collect();
                if near.is_empty() { State::New } else { State::Likely(Reason::SameAmount, near) }
            };
            out.push(Planned { hash, state, repeats });
        }
        Ok(Plan { rows: out, free, known })
    }

    /// The rows that may be in the books already, for the user to say.
    pub fn likely_duplicates(&self, account: Option<i64>, rows: &[ParsedRow]) -> Result<Vec<LikelyDuplicate>> {
        let plan = self.plan_import(account, rows)?;
        self.describe_likely(&plan, account, rows)
    }

    fn describe_likely(&self, plan: &Plan, account: Option<i64>, rows: &[ParsedRow]) -> Result<Vec<LikelyDuplicate>> {
        if !plan.rows.iter().any(|p| matches!(p.state, State::Likely(..))) {
            return Ok(Vec::new());
        }
        let all: HashMap<i64, Transaction> = match account {
            Some(_) => self
                .transactions(&TxFilter { account_id: account, ..Default::default() })?
                .into_iter()
                .map(|t| (t.id, t))
                .collect(),
            None => HashMap::new(),
        };
        let mut out = Vec::new();
        for (index, (row, p)) in rows.iter().zip(&plan.rows).enumerate() {
            let State::Likely(reason, ids) = &p.state else { continue };
            let existing = ids.iter().filter_map(|i| all.get(i).cloned()).take(SHOWN).collect();
            out.push(LikelyDuplicate {
                index,
                reason: *reason,
                date: row.date,
                payee: row.payee.clone(),
                amount: row.amount,
                existing,
            });
        }
        Ok(out)
    }

    /// The rows that the user said are the same: for each the transaction it is, when it is in the books.
    pub(crate) fn decide(
        &self,
        plan: &Plan,
        account: i64,
        rows: &[ParsedRow],
        decision: &Decision,
    ) -> Result<HashMap<usize, Option<i64>>> {
        let likely: Vec<usize> = plan
            .rows
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(p.state, State::Likely(..)))
            .map(|(i, _)| i)
            .collect();
        let mine: HashSet<usize> = likely.iter().copied().collect();
        if let Decision::Each(each) = decision
            && let Some(i) = each.keys().find(|i| !mine.contains(i))
        {
            bail!(trf("row {n} is not among those that may be there already", &[("n", &(i + 1))]));
        }
        let mut skip = Vec::new();
        let mut undecided = false;
        for i in likely {
            match decision {
                Decision::Ask => undecided = true,
                Decision::SkipAll => skip.push(i),
                Decision::AddAll => {}
                Decision::Each(each) => match each.get(&i) {
                    Some(true) => skip.push(i),
                    Some(false) => {}
                    None => undecided = true,
                },
            }
        }
        if undecided {
            return Err(undecided_error(&self.describe_likely(plan, Some(account), rows)?));
        }
        pair(plan, rows, &skip)
    }

    /// A transaction is also known by the key of another statement.
    pub(crate) fn add_import_key(&self, posting: i64, hash: &str) -> Result<()> {
        let entry = self.entry_of_transaction(posting)?.ok_or_else(|| crate::db::no_transaction(posting))?;
        let mut d = self.load_draft(entry)?;
        for l in d.lines.iter_mut().filter(|l| l.id == Some(posting)) {
            let old = l.other_hashes.take().unwrap_or_default();
            l.other_hashes = Some(format!("{old} {hash}").trim().to_string());
        }
        self.save_entry(&d)?;
        Ok(())
    }

    /// The balance of an account in the books at the end of a day.
    pub fn balance_on(&self, account: i64, date: NaiveDate) -> Result<Cents> {
        let all = self.transactions(&TxFilter { account_id: Some(account), ..Default::default() })?;
        if let Some(t) = all.iter().find(|t| t.date <= date) {
            return Ok(t.balance.unwrap_or(0));
        }
        let accounts = self.accounts(true)?;
        Ok(accounts.iter().find(|a| a.id == account).map_or(0, |a| a.opening_balance))
    }
}

fn undecided_error(likely: &[LikelyDuplicate]) -> anyhow::Error {
    let mut lines: Vec<String> = likely
        .iter()
        .take(LISTED)
        .map(|l| {
            let there: Vec<String> =
                l.existing.iter().map(|t| format!("{} ({}, {})", t.id, fmt_date(t.date), t.payee)).collect();
            let there = if there.is_empty() { String::new() } else { format!(" → {}", there.join(", ")) };
            let what = format!("{}, {}, {}", fmt_date(l.date), l.payee, fmt_plain(l.amount));
            format!("{}: {what}, {}{there}", l.index + 1, l.reason.label())
        })
        .collect();
    if likely.len() > LISTED {
        lines.push("…".into());
    }
    anyhow!(trf(
        "nothing was imported: these rows may be there already, say for each whether to skip or add it: {rows}",
        &[("rows", &lines.join("; "))]
    ))
}

/// Gives each row that is skipped a transaction of its own. The windows of dates are all as wide, so taking
/// the earliest transaction for the earliest row pairs as many as can be.
fn pair(plan: &Plan, rows: &[ParsedRow], skip: &[usize]) -> Result<HashMap<usize, Option<i64>>> {
    let mut out: HashMap<usize, Option<i64>> = HashMap::new();
    let mut used: HashSet<i64> = HashSet::new();
    let mut by_amount: Vec<(NaiveDate, usize)> = Vec::new();
    for i in skip {
        let (Some(p), Some(row)) = (plan.rows.get(*i), rows.get(*i)) else { continue };
        match &p.state {
            // A correction may be of a transaction that was skipped or deleted.
            State::Likely(Reason::Corrects | Reason::SameId, ids) => {
                let id = ids.first().copied();
                if let Some(id) = id.filter(|_| matches!(p.state, State::Likely(Reason::SameId, _)))
                    && !used.insert(id)
                {
                    return Err(too_few(&[*i]));
                }
                out.insert(*i, id);
            }
            State::Likely(Reason::Repeated, _) => {
                out.insert(*i, None);
            }
            State::Likely(Reason::SameAmount, _) => by_amount.push((row.date, *i)),
            State::Known | State::New => {}
        }
    }
    by_amount.sort();
    let mut free: HashMap<(Cents, NaiveDate), VecDeque<i64>> = HashMap::new();
    let mut left = Vec::new();
    for (date, i) in by_amount {
        let Some(row) = rows.get(i) else { continue };
        let mut found = None;
        for d in around(date) {
            let key = (row.amount, d);
            let Some(ids) = plan.free.get(&key) else { continue };
            let ids = free.entry(key).or_insert_with(|| ids.iter().copied().collect());
            while let Some(id) = ids.pop_front() {
                if used.insert(id) {
                    found = Some(id);
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        match found {
            Some(id) => {
                out.insert(i, Some(id));
            }
            None => left.push(i),
        }
    }
    if left.is_empty() { Ok(out) } else { Err(too_few(&left)) }
}

fn too_few(rows: &[usize]) -> anyhow::Error {
    let list: Vec<String> = rows.iter().take(LISTED).map(|i| (i + 1).to_string()).collect();
    anyhow!(trf(
        "nothing was imported: more rows are skipped than there are transactions like them; add some of them (rows {rows})",
        &[("rows", &list.join(", "))]
    ))
}

impl Plan {
    /// The transaction that a key stands for.
    pub(crate) fn posting(&self, hash: &str) -> Option<i64> {
        self.known.get(hash).copied()
    }
}

#[cfg(test)]
mod tests;
