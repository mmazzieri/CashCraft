//! Turning a transaction into a transfer and back. The two sides keep the ids, texts, dates and import
//! keys they had: `docs/ledger.md`.

use crate::db::{Cents, Db, Transaction, TxFilter, TxKind, d2s, no_transaction};
use crate::i18n::{tr, trf};
use crate::ledger::{ADDED, Draft, Line, OPENING, UNCATEGORIZED, split_error, too_big};
use crate::money::{fmt_date, fmt_eur};
use crate::rules::Marks;
use anyhow::{Result, anyhow, bail};
use chrono::NaiveDate;
use serde::Serialize;

/// How far apart the two banks may date the same transfer.
pub const MAX_DAYS: i64 = 10;

/// What to do about the other side of a transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtherSide {
    /// A new one; refused when some transactions match, for the user to choose.
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

pub(crate) fn near(a: NaiveDate, b: NaiveDate) -> bool {
    (a - b).num_days().abs() <= MAX_DAYS
}

/// Id, date, text, amount and category, for people to choose.
pub(crate) fn describe(t: &Transaction) -> String {
    format!("{} ({}, {}, {}, {})", t.id, fmt_date(t.date), t.payee, fmt_eur(t.amount), t.category)
}

/// The posting of a transaction, as a side with its own date and texts.
fn own_side(posting: Line, t: &Transaction) -> Line {
    Line { payee: Some(t.payee.clone()), memo: Some(t.memo.clone()), date: Some(d2s(t.date)), origin: None, ..posting }
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

    /// What can be the other side of transaction `id` in `account`, nearest date first: ordinary
    /// transactions, and the far side of transfers whose near side was added and stands for this one.
    pub fn transfer_candidates(&self, id: i64, account: i64) -> Result<Vec<Transaction>> {
        let t = self.transaction(id)?;
        self.check_ordinary(&t)?;
        if t.account_id == account {
            bail!(tr("the two accounts of the transfer are the same"));
        }
        self.check_transfer(t.account_id, account)?;
        let wanted = t.amount.checked_neg().ok_or_else(too_big)?;
        let mut out = self.matching(account, wanted, t.date)?;
        for side in self.added_sides(t.account_id)? {
            if side.amount == t.amount
                && near(side.date, t.date)
                && let Some(peer) = side.transfer_peer
            {
                let peer = self.transaction(peer)?;
                if peer.account_id == account {
                    out.push(peer);
                }
            }
        }
        out.sort_by_key(|x| ((x.date - t.date).num_days().abs(), x.id));
        Ok(out)
    }

    /// Ordinary transactions of an account with this amount near a date, nearest first. Rows of a card
    /// statement are purchases and refunds, never the side of a transfer.
    pub(crate) fn matching(&self, account: i64, amount: Cents, date: NaiveDate) -> Result<Vec<Transaction>> {
        let mut out = Vec::new();
        for x in self.transactions(&TxFilter { account_id: Some(account), ..Default::default() })? {
            if x.amount == amount && near(x.date, date) && !x.card_row && self.is_ordinary(&x)? {
                out.push(x);
            }
        }
        out.sort_by_key(|x| ((x.date - date).num_days().abs(), x.id));
        Ok(out)
    }

    /// The sides of transfers on an account that no statement has brought yet.
    pub(crate) fn added_sides(&self, account: i64) -> Result<Vec<Transaction>> {
        let mut all = self.transactions(&TxFilter { account_id: Some(account), ..Default::default() })?;
        all.retain(|t| t.added);
        Ok(all)
    }

    /// An imported line takes the place of an added side: its date, text and key move onto it.
    pub(crate) fn adopt(
        &self,
        t: &Transaction,
        date: NaiveDate,
        payee: &str,
        memo: &str,
        hash: &str,
        marks: &str,
    ) -> Result<()> {
        self.atomically(|| {
            let mut d = self.load_draft(t.entry_id)?;
            if !d.lines.iter().any(|l| l.id == Some(t.id) && l.origin.as_deref() == Some(ADDED)) {
                bail!(tr("the side of the transfer is on a statement already"));
            }
            for l in d.lines.iter_mut().filter(|l| l.id == Some(t.id)) {
                l.payee = Some(payee.trim().into());
                l.memo = Some(memo.trim().into());
                l.original_payee.clone_from(&l.payee);
                l.original_memo.clone_from(&l.memo);
                // Its tags stay, with what was said of them.
                let m = Marks::read(l.rules.as_deref());
                l.rules = Marks { tags: m.tags, removed: m.removed, ..Default::default() }.write()?;
                l.date = Some(d2s(date));
                l.import_hash = Some(hash.into());
                l.other_hashes = Some(marks.into());
                l.cleared = true;
                l.origin = None;
            }
            self.save_entry(&d)?;
            Ok(())
        })
    }

    /// Turns a transaction into a transfer with `account`. When the other side is a transaction already,
    /// the two become one entry; which one is never guessed.
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
                OtherSide::Auto if candidates.is_empty() => None,
                OtherSide::Auto => {
                    let which: Vec<String> = candidates.iter().map(describe).collect();
                    bail!(trf(
                        "these transactions may be the other side, choose one of them or none: {which}",
                        &[("which", &which.join("; "))]
                    ));
                }
            };
            if let Some(far) = join.filter(|c| c.kind == TxKind::Transfer) {
                // The transfer is there: this line takes the place of its added side.
                let mut d = self.load_draft(far.entry_id)?;
                let own = self.load_draft(t.entry_id)?;
                let mine = own.lines.into_iter().find(|l| l.id == Some(id)).ok_or_else(|| no_transaction(id))?;
                self.remove_entry(t.entry_id)?;
                for l in d.lines.iter_mut().filter(|l| l.id != Some(far.id)) {
                    let mut side = own_side(mine.clone(), &t);
                    side.add_tags_of(l);
                    *l = side;
                }
                self.save_entry(&d)?;
                return Ok(Turned { id, other_id: far.id, joined: true });
            }
            let mut d = self.load_draft(t.entry_id)?;
            let (_, i) = self.kind_of(&d, id)?;
            let (ledger, _) = self.money_ledger(account)?;
            let line = match join {
                Some(x) => {
                    let old = self.load_draft(x.entry_id)?;
                    let mine =
                        old.lines.into_iter().find(|l| l.id == Some(x.id)).ok_or_else(|| no_transaction(x.id))?;
                    self.remove_entry(x.entry_id)?;
                    own_side(mine, x)
                }
                None => {
                    let payee = if t.amount < 0 {
                        trf("Transfer from {account}", &[("account", &t.account)])
                    } else {
                        trf("Transfer to {account}", &[("account", &t.account)])
                    };
                    let old = d.lines.get(i).cloned().ok_or_else(too_big)?;
                    Line {
                        id: None,
                        account: ledger,
                        payee: Some(payee),
                        memo: Some(t.memo.clone()),
                        cleared: false,
                        import_hash: None,
                        other_hashes: None,
                        date: None,
                        origin: Some(ADDED.into()),
                        ..old
                    }
                }
            };
            for l in d.lines.iter_mut().filter(|l| l.id == Some(id)) {
                // Its text stays its own when the other side is edited.
                l.payee = Some(t.payee.clone());
                l.memo = Some(t.memo.clone());
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
            // It pays no statement any more.
            self.conn.execute(
                "UPDATE card_statements SET payment_id=NULL
                 WHERE payment_id IN (SELECT id FROM book_postings WHERE entry_id=?1)",
                [t.entry_id],
            )?;
            let spare = self.system_ledger(UNCATEGORIZED)?;
            let last: i64 = self.conn.query_row("SELECT MAX(id) FROM book_postings", [], |r| r.get(0))?;
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
                        Line {
                            payee: None,
                            memo: None,
                            date: None,
                            origin: l.origin.clone().filter(|o| o != ADDED),
                            ..l.clone()
                        },
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
            let mut stays = mine.clone();
            if !keep_other {
                stays.add_tags_of(other);
            }
            let (first, second) = (alone(&stays, d.id, next(1)?), alone(other, None, next(2)?));
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
