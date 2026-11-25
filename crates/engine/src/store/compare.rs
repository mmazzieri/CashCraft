//! What differs between two data files, for who has to choose.

use super::{Home, content};
use crate::db::Db;
use crate::ledger::EntryFilter;
use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// An entry as people know it: its ids are not compared, for each file gives its own.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Line {
    pub date: NaiveDate,
    pub payee: String,
    pub memo: String,
    /// Account and amount of each posting.
    pub postings: Vec<(String, i64)>,
}

impl Line {
    /// The amounts on the accounts, in one line.
    pub fn amounts(&self) -> String {
        let each = |(a, c): &(String, i64)| format!("{a} {}", crate::money::fmt_plain(*c));
        self.postings.iter().map(each).collect::<Vec<_>>().join(", ")
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Differences {
    /// Only in the first file.
    pub first: Vec<Line>,
    pub second: Vec<Line>,
    /// The entries are the same, and something else is not: names, budgets, rules…
    pub others: bool,
}

/// The entries of a data file, read from a private copy of it.
fn entries(home: &Home, path: &Path) -> Result<(BTreeMap<Line, usize>, String)> {
    let dir = home.scratch()?;
    let copy = dir.0.join("compare.db");
    std::fs::copy(path, &copy).with_context(|| path.display().to_string())?;
    let print = content::fingerprint(&content::open(&copy)?)?;
    let (db, _) = Db::open(&copy)?;
    let mut out = BTreeMap::new();
    for e in db.entries(&EntryFilter::default())? {
        let mut postings: Vec<_> = e.postings.into_iter().map(|p| (p.account, p.amount)).collect();
        postings.sort();
        *out.entry(Line { date: e.date, payee: e.payee, memo: e.memo, postings }).or_insert(0) += 1;
    }
    Ok((out, print))
}

/// The entries that only one of two files has.
pub fn differences(home: &Home, first: &Path, second: &Path) -> Result<Differences> {
    let (mut a, print_a) = entries(home, first)?;
    let (b, print_b) = entries(home, second)?;
    let mut out = Differences::default();
    for (line, n) in b {
        let there = a.remove(&line).unwrap_or(0);
        out.second.extend(std::iter::repeat_n(line.clone(), n.saturating_sub(there)));
        out.first.extend(std::iter::repeat_n(line, there.saturating_sub(n)));
    }
    for (line, n) in a {
        out.first.extend(std::iter::repeat_n(line, n));
    }
    out.first.sort();
    out.others = out.first.is_empty() && out.second.is_empty() && print_a != print_b;
    Ok(out)
}
