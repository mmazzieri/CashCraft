//! Which rows of a statement are in the books already. One with the same key is skipped; one that only looks
//! the same is never guessed: the user says what it is. `docs/import.md` has the rules.

use crate::db::{Cents, Db, Transaction, TxFilter, s2d};
use crate::i18n::{tr, trf};
use crate::importer::{Format, ParsedRow, fnv, norm};
use crate::ledger::CARD_ROW;
use crate::money::{fmt_date, fmt_plain};
use anyhow::{Result, anyhow, bail};
use chrono::{Days, NaiveDate};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// How far apart two statements may date the same transaction: the day of the purchase and that of the bank
/// are up to a weekend with a holiday apart.
const FAR: i64 = 5;
/// The same between two OFX files, which tell transactions apart by themselves.
const FAR_OFX: i64 = 1;
/// How many transactions are shown for a row.
const SHOWN: usize = 8;
/// How many rows an error lists.
const LISTED: usize = 20;

/// What a row that may be in the books is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Another transaction.
    Add,
    /// This transaction, or the one proposed.
    Skip(Option<i64>),
}

/// What the user said of the rows that may be in the books. The import is refused while one is left.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decision {
    /// By the place in the rows.
    pub each: BTreeMap<usize, Choice>,
    /// Those that are surely the same are skipped.
    pub skip_same: bool,
    /// Those that are left are added.
    pub add_others: bool,
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
    /// The bank says that it replaces another one.
    Corrects,
}

impl Reason {
    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s {
            "same_amount" => Reason::SameAmount,
            "same_id" => Reason::SameId,
            "repeated" => Reason::Repeated,
            "corrects" => Reason::Corrects,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Reason::SameAmount => tr("same amount, almost the same date"),
            Reason::SameId => tr("same bank id, another date or amount"),
            Reason::Repeated => tr("twice in the file"),
            Reason::Corrects => {
                tr("the bank replaces a transaction with this one: if you import it, delete the old one by hand")
            }
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
    /// What is in the books already, the likeliest first.
    pub existing: Vec<Transaction>,
    /// Which of them skipping the row means, unless the user chooses another.
    pub proposed: Option<i64>,
    /// Same date, amount and payee, and no other row or transaction like it.
    pub certain: bool,
}

/// The bank takes a transaction back. Nothing is done but saying it.
#[derive(Clone, Debug, Serialize)]
pub struct Deletion {
    pub date: NaiveDate,
    pub payee: String,
    pub amount: Cents,
    pub existing: Transaction,
}

struct Imported {
    id: i64,
    date: NaiveDate,
    amount: Cents,
    texts: [String; 2],
    keys: Vec<String>,
    formats: Vec<Format>,
}

pub(crate) enum State {
    Known(i64),
    New,
    Likely {
        reason: Reason,
        candidates: Vec<i64>,
        proposed: Option<i64>,
        certain: bool,
    },
    /// Never added; the transaction it is about, when it is there.
    Deletes(Option<i64>),
}

pub(crate) struct Planned {
    /// The key of the row, unique in its account.
    pub hash: String,
    /// What else it is known by.
    pub marks: String,
    pub state: State,
    /// The first row of the file that this one repeats.
    pub repeats: Option<usize>,
}

pub(crate) struct Plan {
    pub rows: Vec<Planned>,
}

const OFX: &str = "ofx:";
const ID: &str = "id:";
const PAYEE: &str = "pay:";
const SOURCE: &str = "src:";

/// The contents that a FITID stands for.
type Contents = (NaiveDate, Cents);

fn raw(row: &ParsedRow) -> Cents {
    row.ofx.as_ref().map_or(row.amount, |o| o.raw)
}

fn fitid(row: &ParsedRow) -> Option<&str> {
    row.ofx.as_ref()?.fitid.as_deref()
}

/// 0 the same, 1 alike, 2 others.
fn unlike(text: &str, t: &Imported) -> u8 {
    let word = |w: &str| w.chars().count() >= 4;
    let alike = |a: &str, b: &str| {
        (a.len() >= 3 && b.contains(a))
            || (b.len() >= 3 && a.contains(b))
            || a.split(' ').any(|w| word(w) && b.split(' ').any(|x| x == w))
    };
    if t.texts.iter().any(|x| x == text) {
        0
    } else if t.texts.iter().any(|x| !x.is_empty() && !text.is_empty() && alike(text, x)) {
        1
    } else {
        2
    }
}

fn shifted(date: NaiveDate, days: i64) -> NaiveDate {
    let n = Days::new(days.unsigned_abs());
    if days < 0 { date.checked_sub_days(n) } else { date.checked_add_days(n) }.unwrap_or(date)
}

fn days_around() -> impl Iterator<Item = i64> {
    std::iter::once(0).chain((1..=FAR).flat_map(|d| [-d, d]))
}

/// Imported transactions that may be rows of the file.
#[derive(Default)]
struct Free {
    of: HashMap<i64, usize>,
    by_text: HashMap<(Cents, NaiveDate, String), Vec<usize>>,
    by_date: HashMap<(Cents, NaiveDate), Vec<usize>>,
}

impl Free {
    fn add(&mut self, at: usize, t: &Imported) {
        self.of.insert(t.id, at);
        self.by_date.entry((t.amount, t.date)).or_default().push(at);
        for (i, text) in t.texts.iter().enumerate() {
            if i == 0 || t.texts.first() != Some(text) {
                self.by_text.entry((t.amount, t.date, text.clone())).or_default().push(at);
            }
        }
    }
}

/// How far from a row of the file a transaction may be; `None` when it cannot be one.
fn reach(format: Format, t: &Imported) -> Option<i64> {
    match (t.formats.contains(&format), format) {
        (false, _) => Some(FAR),
        (true, Format::Ofx) => Some(FAR_OFX),
        // Two files of one format are compared by key alone.
        (true, _) => None,
    }
}

impl Db {
    fn imported(&self, account: Option<i64>) -> Result<Vec<Imported>> {
        let Some(a) = account else { return Ok(Vec::new()) };
        let (ledger, _) = self.money_ledger(a)?;
        let mut st = self.conn.prepare(
            "SELECT p.id, p.amount, COALESCE(p.date, e.date), p.import_hash, COALESCE(p.other_hashes,''), p.origin,
                    COALESCE(p.original_payee, p.payee, e.payee), COALESCE(p.original_memo, p.memo, e.memo)
             FROM book_postings p JOIN entries e ON e.id=p.entry_id
             WHERE p.ledger_account_id=?1 AND (p.import_hash IS NOT NULL OR p.other_hashes IS NOT NULL)
             ORDER BY p.id",
        )?;
        let mut found = st.query([ledger])?;
        let mut out = Vec::new();
        while let Some(r) = found.next()? {
            let first: Option<String> = r.get(3)?;
            let others: String = r.get(4)?;
            let card = r.get::<_, Option<String>>(5)?.as_deref() == Some(CARD_ROW);
            let mut formats: Vec<Format> = others
                .split_whitespace()
                .filter_map(|k| k.strip_prefix(SOURCE))
                .filter_map(Format::from_code)
                .collect();
            if formats.is_empty() {
                // Before format 7 the files left no mark.
                formats = if card { vec![Format::Pdf] } else { vec![Format::Csv, Format::Xls] };
            }
            let texts = [r.get::<_, String>(6)?, r.get::<_, String>(7)?].map(|t| norm(&t));
            out.push(Imported {
                id: r.get(0)?,
                amount: r.get(1)?,
                date: s2d(&r.get::<_, String>(2)?),
                texts,
                keys: first.into_iter().chain(others.split_whitespace().map(str::to_string)).collect(),
                formats,
            });
        }
        Ok(out)
    }

    /// Compares the rows with the books; `None` is an account that is not there yet.
    pub(crate) fn plan_import(&self, account: Option<i64>, rows: &[ParsedRow]) -> Result<Plan> {
        Ok(Planner::new(self.imported(account)?, rows).plan())
    }

    /// The rows that may be in the books already, for the user to say.
    pub fn likely_duplicates(&self, account: Option<i64>, rows: &[ParsedRow]) -> Result<Vec<LikelyDuplicate>> {
        let plan = self.plan_import(account, rows)?;
        self.describe_likely(&plan, account, rows)
    }

    /// What the bank takes back of what is in the books.
    pub fn deletions(&self, account: Option<i64>, rows: &[ParsedRow]) -> Result<Vec<Deletion>> {
        let plan = self.plan_import(account, rows)?;
        self.describe_deletions(&plan, account, rows)
    }

    fn all_of(&self, account: Option<i64>) -> Result<HashMap<i64, Transaction>> {
        if account.is_none() {
            return Ok(HashMap::new());
        }
        let all = self.transactions(&TxFilter { account_id: account, ..Default::default() })?;
        Ok(all.into_iter().map(|t| (t.id, t)).collect())
    }

    pub(crate) fn describe_deletions(
        &self,
        plan: &Plan,
        account: Option<i64>,
        rows: &[ParsedRow],
    ) -> Result<Vec<Deletion>> {
        if !plan.rows.iter().any(|p| matches!(p.state, State::Deletes(Some(_)))) {
            return Ok(Vec::new());
        }
        let all = self.all_of(account)?;
        let one = |(row, p): (&ParsedRow, &Planned)| {
            let State::Deletes(Some(id)) = p.state else { return None };
            let existing = all.get(&id)?.clone();
            Some(Deletion { date: row.date, payee: row.payee.clone(), amount: row.amount, existing })
        };
        Ok(rows.iter().zip(&plan.rows).filter_map(one).collect())
    }

    fn describe_likely(&self, plan: &Plan, account: Option<i64>, rows: &[ParsedRow]) -> Result<Vec<LikelyDuplicate>> {
        if !plan.rows.iter().any(|p| matches!(p.state, State::Likely { .. })) {
            return Ok(Vec::new());
        }
        let all = self.all_of(account)?;
        let mut out = Vec::new();
        for (index, (row, p)) in rows.iter().zip(&plan.rows).enumerate() {
            let State::Likely { reason, candidates, proposed, certain } = &p.state else { continue };
            out.push(LikelyDuplicate {
                index,
                reason: *reason,
                date: row.date,
                payee: row.payee.clone(),
                amount: row.amount,
                existing: candidates.iter().filter_map(|i| all.get(i).cloned()).collect(),
                proposed: *proposed,
                certain: *certain,
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
        let mut same: HashMap<usize, Option<i64>> = HashMap::new();
        let mut by_posting: HashMap<i64, usize> = HashMap::new();
        let mut undecided = false;
        for (i, p) in plan.rows.iter().enumerate() {
            let State::Likely { reason, candidates, proposed, certain } = &p.state else {
                if decision.each.contains_key(&i) {
                    bail!(trf("row {n} is not among those that may be there already", &[("n", &(i + 1))]));
                }
                continue;
            };
            let choice = match decision.each.get(&i) {
                Some(c) => *c,
                None if *certain && decision.skip_same => Choice::Skip(None),
                None if decision.add_others => Choice::Add,
                None => {
                    undecided = true;
                    continue;
                }
            };
            let Choice::Skip(chosen) = choice else { continue };
            if let Some(c) = chosen
                && !candidates.contains(&c)
            {
                bail!(trf("transaction {id} is not one that row {n} may be", &[("id", &c), ("n", &(i + 1))]));
            }
            let posting = chosen.or(*proposed);
            match posting {
                None if *reason != Reason::Repeated => {
                    bail!(trf("nothing was imported: say which transaction row {n} is, or add it", &[("n", &(i + 1))]));
                }
                None => {}
                Some(id) => {
                    if let Some(other) = by_posting.insert(id, i) {
                        bail!(trf(
                            "nothing was imported: rows {a} and {b} cannot both be transaction {id}",
                            &[("a", &(other + 1)), ("b", &(i + 1)), ("id", &id)]
                        ));
                    }
                }
            }
            same.insert(i, posting);
        }
        if let Some(i) = decision.each.keys().find(|i| **i >= plan.rows.len()) {
            bail!(trf("row {n} is not among those that may be there already", &[("n", &(i + 1))]));
        }
        if undecided {
            return Err(undecided_error(&self.describe_likely(plan, Some(account), rows)?));
        }
        Ok(same)
    }

    /// A transaction is also known by the keys of other statements.
    pub(crate) fn add_import_keys(&self, posting: i64, keys: &[&str]) -> Result<()> {
        let entry = self.entry_of_transaction(posting)?.ok_or_else(|| crate::db::no_transaction(posting))?;
        let mut d = self.load_draft(entry)?;
        for l in d.lines.iter_mut().filter(|l| l.id == Some(posting)) {
            let old = l.other_hashes.take().unwrap_or_default();
            // What was imported before format 7 says no more where it came from.
            let source = match (old.contains(SOURCE), l.import_hash.is_some(), l.origin.as_deref()) {
                (true, _, _) | (_, false, _) => "",
                (_, _, Some(CARD_ROW)) => "src:pdf",
                _ => "src:csv src:xls",
            };
            let mut seen = HashSet::new();
            let all = old.split_whitespace().chain(source.split_whitespace()).chain(keys.iter().copied());
            let all: Vec<&str> = all.filter(|k| seen.insert(*k)).collect();
            l.other_hashes = Some(all.join(" "));
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

struct Planner<'a> {
    rows: &'a [ParsedRow],
    imported: Vec<Imported>,
    texts: Vec<String>,
    /// Transactions by key, by FITID and by date, amount and payee.
    known: HashMap<String, usize>,
    holders: HashMap<String, Vec<usize>>,
    by_payee: HashMap<String, VecDeque<usize>>,
}

impl<'a> Planner<'a> {
    fn new(imported: Vec<Imported>, rows: &'a [ParsedRow]) -> Self {
        let mut known = HashMap::new();
        let mut holders: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_payee: HashMap<String, VecDeque<usize>> = HashMap::new();
        for (at, t) in imported.iter().enumerate() {
            for k in &t.keys {
                if k.starts_with(ID) {
                    let all = holders.entry(k.clone()).or_default();
                    // Two are enough to say that the FITID tells nothing.
                    if all.len() < 2 {
                        all.push(at);
                    }
                } else if k.starts_with(PAYEE) {
                    by_payee.entry(k.clone()).or_default().push_back(at);
                } else if !k.starts_with(SOURCE) {
                    known.entry(k.clone()).or_insert(at);
                }
            }
        }
        let texts = rows.iter().map(|r| norm(&r.payee)).collect();
        Planner { rows, imported, texts, known, holders, by_payee }
    }

    fn holder(&self, id: &str) -> Option<usize> {
        match self.holders.get(&id_mark(id)).map(Vec::as_slice) {
            Some([one]) => Some(*one),
            _ => None,
        }
    }

    /// The FITIDs that tell a transaction from the others. Not those that the file or the books have on several
    /// contents; none when the bank numbers the transactions of each file again.
    fn trusted(&self) -> HashSet<&'a str> {
        let mut uses: HashMap<&str, (Contents, bool)> = HashMap::new();
        for r in self.rows {
            if let Some(id) = fitid(r) {
                let c = (r.date, raw(r));
                let first = uses.entry(id).or_insert((c, true));
                first.1 &= first.0 == c;
            }
        }
        let many = |id: &str| self.holders.get(&id_mark(id)).is_some_and(|h| h.len() > 1);
        let mut out: HashSet<&str> = uses.iter().filter(|(id, u)| u.1 && !many(id)).map(|(id, _)| *id).collect();
        let (mut met, mut other) = (0, 0);
        for id in &out {
            if let (Some(at), Some((c, _))) = (self.holder(id), uses.get(id)) {
                met += 1;
                let key = id_key(id, *c, 1);
                let same = self.imported.get(at).is_some_and(|t| t.keys.contains(&key));
                other += usize::from(!same);
            }
        }
        let mut numbers: Vec<u64> = uses.keys().filter_map(|id| id.parse().ok()).collect();
        numbers.sort_unstable();
        let in_order = numbers.len() > 1
            && numbers.len() == uses.len()
            && numbers.windows(2).all(|w| matches!(w, [a, b] if a.checked_add(1) == Some(*b)));
        if (in_order && other > 0) || (other > 1 && other * 2 >= met) {
            out.clear();
        }
        out
    }

    fn plan(mut self) -> Plan {
        let trusted = self.trusted();
        let mut seen: HashMap<String, usize> = HashMap::new();
        // What a FITID says comes before what a payee says.
        let mut taken: HashSet<usize> = HashSet::new();
        for row in self.rows {
            if let Some(id) = fitid(row).filter(|id| trusted.contains(id)) {
                let c = (row.date, raw(row));
                let n = seen.entry(id_key(id, c, 0)).or_default();
                *n += 1;
                taken.extend(self.known.get(&id_key(id, c, *n)).copied());
            }
        }
        seen.clear();
        let mut out: Vec<Planned> = Vec::new();
        let mut first_of: HashMap<(&str, &str, &str), usize> = HashMap::new();
        // How many of each payee key there are, to number those that are added.
        let mut numbers: HashMap<String, usize> = HashMap::new();
        for (index, row) in self.rows.iter().enumerate() {
            let mut nth = |key: &str| {
                let n = seen.entry(key.to_string()).or_default();
                *n += 1;
                *n
            };
            let source = format!("{SOURCE}{}", row.format.code());
            let mut repeats = None;
            let (hash, marks, at) = match &row.ofx {
                None => {
                    let key = row.key.clone().unwrap_or_else(|| {
                        format!("{}|{}|{}|{}", row.date, row.amount, row.payee.to_lowercase(), row.memo.to_lowercase())
                    });
                    let hash = format!("{:016x}", fnv(&format!("{key}|{}", nth(&key))));
                    let at = self.known.get(&hash).copied();
                    (hash, source, at)
                }
                Some(o) => {
                    let c = (row.date, raw(row));
                    let payee =
                        format!("{PAYEE}{:016x}", fnv(&format!("{}|{}|{}", c.0, c.1, row.payee.to_lowercase())));
                    let mut marks = format!("{source} {payee}");
                    if let Some(id) = &o.fitid {
                        marks = format!("{marks} {}", id_mark(id));
                    }
                    match o.fitid.as_deref().filter(|id| trusted.contains(id)) {
                        Some(id) => {
                            match first_of.get(&(id, row.payee.as_str(), row.memo.as_str())) {
                                Some(i) => repeats = Some(*i),
                                None => {
                                    first_of.insert((id, &row.payee, &row.memo), index);
                                }
                            }
                            let n = nth(&id_key(id, c, 0));
                            let hash = id_key(id, c, n);
                            let at = self.known.get(&hash).copied();
                            (hash, marks, at)
                        }
                        None => {
                            let there = self.by_payee.get_mut(&payee);
                            let at = there.and_then(|all| {
                                loop {
                                    let at = all.pop_front()?;
                                    if !taken.contains(&at) {
                                        break Some(at);
                                    }
                                }
                            });
                            // A key that nothing has.
                            let n = numbers.entry(payee.clone()).or_default();
                            let hash = loop {
                                *n += 1;
                                let hash = format!("{OFX}:{:016x}", fnv(&format!("{payee}|{n}")));
                                if !self.known.contains_key(&hash) {
                                    break hash;
                                }
                            };
                            (hash, marks, at)
                        }
                    }
                }
            };
            let state = match at.and_then(|at| Some((at, self.imported.get(at)?.id))) {
                Some((at, id)) => {
                    taken.insert(at);
                    State::Known(id)
                }
                None => State::New,
            };
            out.push(Planned { hash, marks, state, repeats });
        }
        let Some(format) = self.rows.first().map(|r| r.format) else { return Plan { rows: out } };
        let mut free = [Free::default(), Free::default()];
        for (at, t) in self.imported.iter().enumerate().filter(|(at, _)| !taken.contains(at)) {
            let [near, far] = &mut free;
            match reach(format, t) {
                Some(FAR) => far.add(at, t),
                Some(_) => near.add(at, t),
                None => {}
            }
        }
        let mut ask: Vec<usize> = Vec::new();
        let mut strict: HashSet<usize> = HashSet::new();
        for (i, (row, p)) in self.rows.iter().zip(&mut out).enumerate() {
            if !matches!(p.state, State::New) {
                continue;
            }
            let correction = row.ofx.as_ref().and_then(|o| o.corrects.as_ref());
            let corrected = correction.and_then(|c| self.holder(&c.fitid));
            let id_of = |at: usize| self.imported.get(at).map(|t| t.id);
            if correction.is_some_and(|c| c.delete) {
                p.state = State::Deletes(corrected.and_then(id_of));
                continue;
            }
            let mut near = self.candidates(i, row, &free);
            let id = fitid(row).filter(|id| trusted.contains(id));
            let held = id.and_then(|id| self.holder(id)).filter(|at| !taken.contains(at));
            let reason = match (corrected, held) {
                (Some(_), _) => Reason::Corrects,
                (_, Some(_)) => Reason::SameId,
                _ if p.repeats.is_some() => Reason::Repeated,
                _ if !near.is_empty() => Reason::SameAmount,
                _ => continue,
            };
            // After what is exactly the same.
            if let Some(at) = corrected.or(held)
                && !near.contains(&at)
            {
                let exact = near.iter().take_while(|n| self.is_exact(i, row, **n)).count();
                near.insert(exact, at);
            }
            near.truncate(SHOWN);
            let candidates = near.into_iter().filter_map(id_of).collect();
            p.state = State::Likely { reason, candidates, proposed: None, certain: false };
            ask.push(i);
            if matches!(reason, Reason::Corrects | Reason::SameId) {
                strict.insert(i);
            }
        }
        let pairs = self.pair(&ask, &strict, &free);
        let mut rows_like: HashMap<(Cents, NaiveDate, &str), usize> = HashMap::new();
        for i in &ask {
            if let (Some(r), Some(t)) = (self.rows.get(*i), self.texts.get(*i)) {
                *rows_like.entry((r.amount, r.date, t)).or_default() += 1;
            }
        }
        let mut given: HashSet<i64> = pairs.values().filter_map(|at| self.imported.get(*at)).map(|t| t.id).collect();
        for i in ask {
            let (Some(row), Some(text), Some(p)) = (self.rows.get(i), self.texts.get(i), out.get_mut(i)) else {
                continue;
            };
            let State::Likely { reason, candidates, proposed, certain } = &mut p.state else { continue };
            let pair = pairs.get(&i).copied();
            if let Some(t) = pair.and_then(|at| self.imported.get(at)) {
                *proposed = Some(t.id);
                if !candidates.contains(&t.id) {
                    candidates.insert(0, t.id);
                    candidates.truncate(SHOWN);
                }
                let like = |f: &Free| f.by_text.get(&(row.amount, row.date, text.clone())).map_or(0, Vec::len);
                *certain = *reason == Reason::SameAmount
                    && pair.is_some_and(|at| self.is_exact(i, row, at))
                    && rows_like.get(&(row.amount, row.date, text)) == Some(&1)
                    && free.iter().map(like).sum::<usize>() == 1;
            } else if matches!(reason, Reason::Corrects | Reason::SameId)
                && let Some(id) = candidates.iter().find(|id| given.insert(**id))
            {
                *proposed = Some(*id);
            }
        }
        Plan { rows: out }
    }

    fn is_exact(&self, i: usize, row: &ParsedRow, at: usize) -> bool {
        let (Some(t), Some(text)) = (self.imported.get(at), self.texts.get(i)) else { return false };
        t.amount == row.amount && t.date == row.date && unlike(text, t) == 0
    }

    /// What a row may be, the likeliest first: the same date, the payee, the distance.
    fn candidates(&self, i: usize, row: &ParsedRow, free: &[Free; 2]) -> Vec<usize> {
        let Some(text) = self.texts.get(i) else { return Vec::new() };
        let mut found: Vec<usize> = Vec::new();
        for (f, far) in free.iter().zip([FAR_OFX, FAR]) {
            for d in days_around().filter(|d| d.abs() <= far) {
                let date = shifted(row.date, d);
                let exact = f.by_text.get(&(row.amount, date, text.clone())).into_iter().flatten().take(SHOWN);
                let others = f.by_date.get(&(row.amount, date)).into_iter().flatten().take(SHOWN);
                found.extend(exact.chain(others));
            }
        }
        let rank = |at: &usize| {
            let t = self.imported.get(*at)?;
            let days = (t.date - row.date).num_days().abs();
            Some((days != 0, unlike(text, t), days, t.id))
        };
        found.sort_by_key(rank);
        found.dedup();
        found.truncate(SHOWN * 2);
        found
    }

    /// Gives as many rows as it can a transaction of their own, in this order: the same date, with the same
    /// payee, then one alike, then any; other dates, the same way and the nearest first. What comes later never
    /// takes what comes before. `strict` rows are paired only with what is exactly the same.
    fn pair(&self, ask: &[usize], strict: &HashSet<usize>, free: &[Free; 2]) -> HashMap<usize, usize> {
        let mut rows: Vec<usize> = ask.to_vec();
        rows.sort_by_key(|i| (self.rows.get(*i).map(|r| r.date), *i));
        let mut p = Pairing { planner: self, free, pairs: HashMap::new(), used: HashSet::new(), read: HashMap::new() };
        p.step(&rows, Payee::Same, 0);
        rows.retain(|i| !strict.contains(i));
        p.step(&rows, Payee::Alike, 0);
        p.step(&rows, Payee::Any, 0);
        for payee in [Payee::Same, Payee::Alike, Payee::Any] {
            for days in 1..=FAR {
                p.step(&rows, payee, days);
            }
        }
        p.pairs
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Payee {
    Same,
    Alike,
    Any,
}

struct Pairing<'a> {
    planner: &'a Planner<'a>,
    free: &'a [Free; 2],
    pairs: HashMap<usize, usize>,
    used: HashSet<usize>,
    /// How far each list was read: which, amount, date and payee.
    read: HashMap<(usize, Cents, NaiveDate, &'a str), usize>,
}

impl Pairing<'_> {
    /// Pairs the rows, by date, with transactions some days before, or else after.
    fn step(&mut self, rows: &[usize], payee: Payee, days: i64) {
        let around = if days == 0 { vec![0] } else { vec![-days, days] };
        for i in rows.iter().filter(|i| !self.pairs.contains_key(i)).copied().collect::<Vec<_>>() {
            if let Some(at) = around.iter().find_map(|d| self.find(i, payee, *d)) {
                self.used.insert(at);
                self.pairs.insert(i, at);
            }
        }
    }

    fn find(&mut self, i: usize, payee: Payee, days: i64) -> Option<usize> {
        let (row, text) = (self.planner.rows.get(i)?, self.planner.texts.get(i)?);
        let date = shifted(row.date, days);
        for (n, (f, far)) in self.free.iter().zip([FAR_OFX, FAR]).enumerate() {
            if days.abs() > far {
                continue;
            }
            let list = match payee {
                Payee::Same => f.by_text.get(&(row.amount, date, text.clone())),
                _ => f.by_date.get(&(row.amount, date)),
            };
            let Some(list) = list else { continue };
            let found = if payee == Payee::Alike {
                // Only the first few are looked at.
                let alike = |at: &&usize| self.planner.imported.get(**at).is_some_and(|t| unlike(text, t) <= 1);
                list.iter().take(SHOWN * 4).filter(|at| !self.used.contains(at)).find(alike).copied()
            } else {
                // An empty payee is not that of all.
                let key = if payee == Payee::Same { (0, text.as_str()) } else { (2, "") };
                let at = self.read.entry((n + key.0, row.amount, date, key.1)).or_default();
                let mut found = None;
                while let Some(x) = list.get(*at) {
                    *at += 1;
                    if !self.used.contains(x) {
                        found = Some(*x);
                        break;
                    }
                }
                found
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }
}

fn id_mark(id: &str) -> String {
    format!("{ID}{:016x}", fnv(id))
}

fn id_key(id: &str, c: Contents, n: usize) -> String {
    format!("{OFX}{:016x}:{:016x}", fnv(id), fnv(&format!("{}|{}|{n}", c.0, c.1)))
}

fn undecided_error(likely: &[LikelyDuplicate]) -> anyhow::Error {
    let mut lines: Vec<String> = likely
        .iter()
        .take(LISTED)
        .map(|l| {
            let there: Vec<String> = l
                .existing
                .iter()
                .map(|t| {
                    let mark = if l.proposed == Some(t.id) { "→ " } else { "" };
                    format!("{mark}{} ({}, {})", t.id, fmt_date(t.date), t.payee)
                })
                .collect();
            let what = format!("{}, {}, {}", fmt_date(l.date), l.payee, fmt_plain(l.amount));
            format!("{}: {what}, {} [{}]", l.index + 1, l.reason.label(), there.join(", "))
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

#[cfg(test)]
mod tests;
