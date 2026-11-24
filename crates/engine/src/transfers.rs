//! Turning a transaction into a transfer and back. The two sides keep the ids, texts, dates and import
//! keys they had: `docs/ledger.md`.

use crate::db::{Cents, Db, Transaction, TxFilter, TxKind, d2s, no_transaction};
use crate::i18n::{tr, trf};
use crate::ledger::{Draft, Line, OPENING, UNCATEGORIZED, split_error, too_big};
use anyhow::{Result, anyhow, bail};
use chrono::NaiveDate;
use serde::Serialize;
use std::ops::RangeInclusive;

/// How far apart the two banks may date the same transfer.
pub const MAX_DAYS: i64 = 10;

/// What to do about the other side of a transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtherSide {
    /// The one transaction that matches, a new one if there is none; refused when there are several.
    Auto,
    /// A new one, even if some match.
    New,
    /// This transaction of the other account.
    Join(i64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Turned {
    /// The transaction that was there, and the other side.
    pub id: i64,
    pub other_id: i64,
    /// Whether the other side was a transaction already.
    pub joined: bool,
}

impl Db {
    /// Whether a transaction can become a transfer: on one other account, which is not of the user nor
    /// was added to the chart by hand.
    fn is_ordinary(&self, t: &Transaction) -> Result<bool> {
        if t.kind != TxKind::Other {
            return Ok(matches!(t.kind, TxKind::Category | TxKind::Uncategorized));
        }
        // Half a transfer of an older file stands against the opening balances.
        let opening = self.system_ledger(OPENING)?;
        Ok(self.load_draft(t.entry_id)?.lines.iter().any(|l| l.account == opening))
    }

    fn check_ordinary(&self, t: &Transaction) -> Result<()> {
        if t.kind == TxKind::Transfer {
            bail!(tr("the transaction is a transfer already"));
        }
        if t.kind == TxKind::Split {
            bail!(tr("the transaction is split over several accounts: it cannot become a transfer"));
        }
        if !self.is_ordinary(t)? {
            bail!(tr("the transaction is on an account added to the chart by hand: it cannot become a transfer"));
        }
        Ok(())
    }

    /// The transactions of `account` that can be the other side of transaction `id`, nearest date first.
    pub fn transfer_candidates(&self, id: i64, account: i64) -> Result<Vec<Transaction>> {
        let t = self.transaction(id)?;
        self.check_ordinary(&t)?;
        if t.account_id == account {
            bail!(tr("the two accounts of the transfer are the same"));
        }
        self.check_transfer(t.account_id, account)?;
        let wanted = t.amount.checked_neg().ok_or_else(too_big)?;
        self.matching(account, wanted, t.date, -MAX_DAYS..=MAX_DAYS)
    }

    /// Ordinary transactions of an account with this amount, within these days of `date`, nearest first.
    fn matching(
        &self,
        account: i64,
        amount: Cents,
        date: NaiveDate,
        near: RangeInclusive<i64>,
    ) -> Result<Vec<Transaction>> {
        let days = |x: &Transaction| (x.date - date).num_days();
        let mut out = Vec::new();
        for x in self.transactions(&TxFilter { account_id: Some(account), ..Default::default() })? {
            if x.amount == amount && near.contains(&days(&x)) && self.is_ordinary(&x)? {
                out.push(x);
            }
        }
        out.sort_by_key(|x| (days(x).abs(), x.id));
        Ok(out)
    }

    /// Turns a transaction into a transfer with `account`. When the other side is a transaction already,
    /// the two become one entry.
    pub fn turn_into_transfer(&self, id: i64, account: i64, other: OtherSide) -> Result<Turned> {
        self.atomically(|| {
            let t = self.transaction(id)?;
            let candidates = self.transfer_candidates(id, account)?;
            let join = match other {
                OtherSide::New => None,
                OtherSide::Join(x) => Some(candidates.iter().find(|c| c.id == x).ok_or_else(|| {
                    anyhow!(trf(
                        "transaction {id} is not the other side of this transfer: it needs the opposite amount, within {days} days, in the other account",
                        &[("id", &x), ("days", &MAX_DAYS)],
                    ))
                })?),
                OtherSide::Auto => match candidates.as_slice() {
                    [] => None,
                    [one] => Some(one),
                    several => {
                        let ids: Vec<String> = several.iter().map(|c| format!("{} ({})", c.id, d2s(c.date))).collect();
                        bail!(trf(
                            "several transactions may be the other side, choose one of them or none: {which}",
                            &[("which", &ids.join(", "))]
                        ));
                    }
                },
            };
            let mut d = self.load_draft(t.entry_id)?;
            let (_, i) = self.kind_of(&d, id)?;
            let (ledger, _) = self.money_ledger(account)?;
            let line = match join {
                Some(x) => {
                    let old = self.load_draft(x.entry_id)?;
                    let mine = old.lines.into_iter().find(|l| l.id == Some(x.id)).ok_or_else(|| no_transaction(x.id))?;
                    self.remove_entry(x.entry_id)?;
                    Line {
                        payee: Some(x.payee.clone()),
                        memo: Some(x.memo.clone()),
                        date: Some(d2s(x.date)),
                        ..mine
                    }
                }
                None => {
                    let payee = if t.amount < 0 {
                        trf("Transfer from {account}", &[("account", &t.account)])
                    } else {
                        trf("Transfer to {account}", &[("account", &t.account)])
                    };
                    let old = d.lines.get(i).cloned().ok_or_else(too_big)?;
                    Line { id: None, account: ledger, payee: Some(payee), cleared: false, import_hash: None, date: None, ..old }
                }
            };
            for l in d.lines.iter_mut().filter(|l| l.id == Some(id)) {
                // Its text stays its own when the other side is edited.
                l.payee = Some(t.payee.clone());
                if join.is_some() {
                    l.memo = Some(t.memo.clone());
                }
                // The entry takes its date.
                d.date = d2s(t.date);
            }
            if let Some(l) = d.lines.get_mut(i) {
                *l = line;
            }
            let (_, ids) = self.save_entry(&d)?;
            let other_id = ids.into_iter().find(|x| *x != id).ok_or_else(|| no_transaction(id))?;
            Ok(Turned { id, other_id, joined: join.is_some() })
        })
    }

    /// Separates a transfer. Both sides become uncategorized transactions; without `keep_other`, the other
    /// side goes.
    pub fn separate_transfer(&self, id: i64, keep_other: bool) -> Result<Option<i64>> {
        self.atomically(|| {
            let t = self.transaction(id)?;
            if t.kind != TxKind::Transfer {
                bail!(tr("the transaction is not a transfer"));
            }
            let d = self.load_draft(t.entry_id)?;
            let (mine, other) = match d.lines.as_slice() {
                [a, b] if a.id == Some(id) => (a, b),
                [a, b] => (b, a),
                _ => return Err(split_error()),
            };
            let spare = self.system_ledger(UNCATEGORIZED)?;
            let last: i64 = self.conn.query_row("SELECT MAX(id) FROM postings", [], |r| r.get(0))?;
            // Ids above every other: that of the side that leaves must not be taken.
            let next = |n: i64| last.checked_add(n).ok_or_else(too_big);
            let alone = |l: &Line, entry: Option<i64>, counter: i64| -> Draft {
                Draft {
                    id: entry,
                    date: l.date.clone().unwrap_or_else(|| d.date.clone()),
                    payee: l.payee.clone().unwrap_or_else(|| d.payee.clone()),
                    memo: l.memo.clone().unwrap_or_else(|| d.memo.clone()),
                    opening: None,
                    lines: vec![
                        Line { payee: None, memo: None, date: None, ..l.clone() },
                        Line {
                            id: Some(counter),
                            account: spare,
                            amount: -l.amount,
                            currency: l.currency.clone(),
                            ..Default::default()
                        },
                    ],
                }
            };
            let (first, second) = (alone(mine, d.id, next(1)?), alone(other, None, next(2)?));
            self.save_entry(&first)?;
            if !keep_other {
                return Ok(None);
            }
            self.save_entry(&second)?;
            Ok(other.id)
        })
    }
}

#[cfg(test)]
mod tests;
