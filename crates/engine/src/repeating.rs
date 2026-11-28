//! A new movement that repeats: the one of its day written down and the next ones scheduled, in one change, so that
//! the first is not counted twice.

use crate::db::{Db, NewScheduled, NewTx, advance, normalize_unit};
use anyhow::{Result, anyhow};
use chrono::NaiveDate;
use serde::Serialize;

/// What [`Db::add_repeating`] did.
#[derive(Clone, Debug, Serialize)]
pub struct Repeated {
    /// The movement written now: one id, or the two sides of a transfer; none when its day is still to come.
    pub ids: Vec<i64>,
    /// The scheduled movement.
    pub scheduled: i64,
    /// Its next due date: one period after the movement written, or the movement's own day when it is to come.
    pub next_date: NaiveDate,
}

impl Db {
    /// Writes the movement of `s.next_date` and schedules the ones after it, every `s.every` `s.unit`, all or
    /// nothing. A movement whose day comes after `today` is not written: the schedule starts on that day. A transfer
    /// has `s.to_account_id` and a positive amount; tags and `cleared` are for the movement written now.
    pub fn add_repeating(
        &self,
        s: &NewScheduled,
        tags: &[String],
        cleared: bool,
        today: NaiveDate,
    ) -> Result<Repeated> {
        let unit = normalize_unit(&s.unit)?;
        self.atomically(|| {
            let mut ids = Vec::new();
            let mut next_date = s.next_date;
            if s.next_date <= today {
                match s.to_account_id {
                    Some(to) => {
                        let (out, back) = self.add_transfer(s.account_id, to, s.next_date, s.amount, &s.memo)?;
                        ids.extend([out, back]);
                    }
                    None => {
                        let id = self
                            .add_transaction(&NewTx {
                                account_id: s.account_id,
                                date: s.next_date,
                                amount: s.amount,
                                payee: s.payee.clone(),
                                memo: s.memo.clone(),
                                category_id: s.category_id,
                                cleared,
                                import_hash: None,
                                import_marks: None,
                                card_row: false,
                            })?
                            .ok_or_else(|| anyhow!(crate::i18n::tr("the transaction was not saved")))?;
                        if !tags.is_empty() {
                            self.add_tags(id, tags)?;
                        }
                        ids.push(id);
                    }
                }
                next_date = advance(s.next_date, s.every, unit);
            }
            let scheduled = self.add_scheduled(&NewScheduled { next_date, ..s.clone() })?;
            Ok(Repeated { ids, scheduled, next_date })
        })
    }
}

#[cfg(test)]
mod tests;
