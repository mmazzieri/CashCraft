//! Card statements that were imported, and the bank's line that pays each: a transfer, not an expense.

use crate::db::{AccountKind, Cents, Db, Transaction, TxFilter, TxKind, d2s, s2d};
use crate::i18n::{tr, trf};
use crate::transfers::OtherSide;
use anyhow::{Result, anyhow, bail};
use chrono::NaiveDate;
use rusqlite::params;
use serde::Serialize;
use std::ops::RangeInclusive;

/// Days from the debit date on which the bank may have booked the payment.
const NEAR: RangeInclusive<i64> = -3..=10;

/// Format 4.
pub(crate) const CARD_SCHEMA: &str = "
CREATE TABLE card_statements (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    closing_date TEXT NOT NULL,
    debit_date TEXT NOT NULL,
    amount INTEGER NOT NULL,
    import_key TEXT NOT NULL,
    payment_id INTEGER REFERENCES postings(id) ON DELETE SET NULL,
    payment_joined INTEGER NOT NULL DEFAULT 0,
    UNIQUE (account_id, import_key)
);
CREATE INDEX card_statement_payment ON card_statements(payment_id);
";

#[derive(Clone, Debug, Serialize)]
pub struct CardStatement {
    pub id: i64,
    pub account_id: i64,
    pub account: String,
    pub closing_date: NaiveDate,
    pub debit_date: NaiveDate,
    /// What the bank is debited, positive.
    pub amount: Cents,
    /// The bank's transaction linked to it.
    pub payment_id: Option<i64>,
    #[serde(skip)]
    joined: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PaymentProposal {
    pub statement: CardStatement,
    /// Bank transactions that may be the payment, nearest date first.
    pub candidates: Vec<Transaction>,
}

impl Db {
    /// Remembers an imported statement, once.
    pub(crate) fn record_card_statement(
        &self,
        account: i64,
        closing: NaiveDate,
        debit_date: NaiveDate,
        amount: Cents,
        key: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO card_statements(account_id,closing_date,debit_date,amount,import_key) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(account_id,import_key) DO NOTHING",
            params![account, d2s(closing), d2s(debit_date), amount, key],
        )?;
        Ok(())
    }

    /// The statements of the credit cards, newest first.
    pub fn card_statements(&self) -> Result<Vec<CardStatement>> {
        // A payment counts while it is a transfer to the card.
        let mut st = self.conn.prepare(
            "SELECT s.id, s.account_id, a.name, s.closing_date, s.debit_date, s.amount, s.payment_joined,
                    (SELECT p.id FROM postings p JOIN postings o ON o.entry_id=p.entry_id
                     JOIN ledger_accounts l ON l.id=o.ledger_account_id
                     WHERE p.id=s.payment_id AND l.money_account_id=s.account_id)
             FROM card_statements s JOIN accounts a ON a.id=s.account_id WHERE a.kind='credit_card'
             ORDER BY s.closing_date DESC, s.id DESC",
        )?;
        let rows = st.query_map([], |r| {
            Ok(CardStatement {
                id: r.get(0)?,
                account_id: r.get(1)?,
                account: r.get(2)?,
                closing_date: s2d(&r.get::<_, String>(3)?),
                debit_date: s2d(&r.get::<_, String>(4)?),
                amount: r.get(5)?,
                joined: r.get::<_, i64>(6)? != 0,
                payment_id: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn card_statement(&self, id: i64) -> Result<CardStatement> {
        let found = self.card_statements()?.into_iter().find(|s| s.id == id);
        found.ok_or_else(|| anyhow!(trf("card statement {id} not found", &[("id", &id)])))
    }

    /// Bank transactions that may pay a statement.
    fn payment_candidates(&self, s: &CardStatement) -> Result<Vec<Transaction>> {
        let mut out = Vec::new();
        let (_, currency) = self.money_ledger(s.account_id)?;
        let Some(wanted) = s.amount.checked_neg().filter(|a| *a < 0) else { return Ok(out) };
        for a in self.accounts(true)?.into_iter().filter(|a| a.kind == AccountKind::Bank) {
            if self.money_ledger(a.id)?.1 == currency {
                out.extend(self.matching(a.id, wanted, s.debit_date, NEAR)?);
            }
        }
        out.sort_by_key(|t| ((t.date - s.debit_date).num_days().abs(), t.id));
        Ok(out)
    }

    /// Statements without a payment, each with the bank transactions that may be it. One already paid by a
    /// transfer of the same amount is left out.
    pub fn propose_card_payments(&self) -> Result<Vec<PaymentProposal>> {
        let all = self.card_statements()?;
        let used: Vec<i64> = all.iter().filter_map(|s| s.payment_id).collect();
        let mut taken: Vec<i64> = Vec::new();
        let mut out = Vec::new();
        for statement in all.iter().filter(|s| s.payment_id.is_none()) {
            let f = TxFilter { account_id: Some(statement.account_id), ..Default::default() };
            let paid = self.transactions(&f)?.into_iter().find(|t| {
                t.kind == TxKind::Transfer
                    && t.amount == statement.amount
                    && NEAR.contains(&(t.date - statement.debit_date).num_days())
                    && t.transfer_peer.is_some_and(|p| !used.contains(&p) && !taken.contains(&p))
            });
            match paid {
                Some(t) => taken.extend(t.transfer_peer),
                None => {
                    let candidates = self.payment_candidates(statement)?;
                    out.push(PaymentProposal { statement: statement.clone(), candidates });
                }
            }
        }
        Ok(out)
    }

    /// Makes a bank transaction the payment of a statement: a transfer to the card, joined with the card's
    /// own line when there is one. Doing it again changes nothing.
    pub fn link_card_payment(&self, statement: i64, transaction: i64) -> Result<()> {
        self.atomically(|| {
            let s = self.card_statement(statement)?;
            match s.payment_id {
                Some(p) if p == transaction => return Ok(()),
                Some(_) => bail!(tr("the card statement has a payment already: unlink it first")),
                None => {}
            }
            if !self.payment_candidates(&s)?.iter().any(|c| c.id == transaction) {
                bail!(trf(
                    "transaction {id} cannot be the payment of this statement: it must be of a bank account, for the amount debited, near the debit date, and not a transfer or split",
                    &[("id", &transaction)]
                ));
            }
            let other = match self.matching(s.account_id, s.amount, s.debit_date, NEAR)?.as_slice() {
                [] => OtherSide::New,
                [one] => OtherSide::Join(one.id),
                several => {
                    let ids: Vec<String> = several.iter().map(|t| format!("{} ({})", t.id, d2s(t.date))).collect();
                    bail!(trf(
                        "the card has several payments of this amount: turn the bank's line into a transfer with one of them by hand: {which}",
                        &[("which", &ids.join(", "))]
                    ));
                }
            };
            let done = self.turn_into_transfer(transaction, s.account_id, other)?;
            self.conn.execute(
                "UPDATE card_statements SET payment_id=?2, payment_joined=?3 WHERE id=?1",
                params![s.id, done.id, done.joined as i64],
            )?;
            Ok(())
        })
    }

    /// Undoes `link_card_payment`: the bank's line becomes an uncategorized transaction. Doing it again
    /// changes nothing.
    pub fn unlink_card_payment(&self, statement: i64) -> Result<()> {
        self.atomically(|| {
            let s = self.card_statement(statement)?;
            if let Some(p) = s.payment_id {
                self.separate_transfer(p, s.joined)?;
            }
            self.conn.execute("UPDATE card_statements SET payment_id=NULL, payment_joined=0 WHERE id=?1", [s.id])?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
