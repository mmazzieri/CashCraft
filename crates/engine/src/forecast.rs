//! The balance an account will have in the coming days: today's balance, then what is already in the
//! books with a later date and every occurrence of the scheduled transactions that touch it, day by
//! day. It says on which day the balance first goes below the account's minimum and how much must
//! come in before then to stay above it to the end. Only what is in the books or scheduled is
//! projected.

use crate::db::{Account, AccountKind, Cents, Db, Scheduled, TxFilter, advance};
use crate::i18n::{tr, trf};
use crate::money::{fmt_date, fmt_eur};
use anyhow::{Result, anyhow, bail};
use chrono::{Days, NaiveDate};
use serde::Serialize;

/// The horizon when none is asked for, in days.
pub const DEFAULT_DAYS: i64 = 90;
/// The longest horizon, in days.
pub const MAX_DAYS: i64 = 730;
/// The most occurrences of one scheduled transaction counted in a projection.
const MAX_OCCURRENCES: usize = 1000;

/// Something that changes the balance in the projection.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Item {
    pub date: NaiveDate,
    pub payee: String,
    pub amount: Cents,
    /// The balance after it.
    pub balance: Cents,
    /// The scheduled transaction it comes from; none for a transaction already in the books.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_id: Option<i64>,
    /// A scheduled occurrence whose date is past and that is not recorded yet: counted today.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub overdue: bool,
}

/// The first day below the minimum.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Shortfall {
    pub date: NaiveDate,
    /// The balance on that day.
    pub balance: Cents,
    /// What must come in before that day for the balance to stay at the minimum or above to the end
    /// of the horizon.
    pub needed: Cents,
}

#[derive(Clone, Debug, Serialize)]
pub struct Forecast {
    pub account_id: i64,
    pub account: String,
    pub currency: String,
    pub from: NaiveDate,
    pub until: NaiveDate,
    /// The balance at the end of `from`, before anything later.
    pub balance: Cents,
    /// The minimum it is compared with; none for a card or a loan without one.
    pub minimum: Option<Cents>,
    pub items: Vec<Item>,
    /// The lowest balance and its first day (`from` when nothing lowers it).
    pub lowest: Cents,
    pub lowest_date: NaiveDate,
    pub end_balance: Cents,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortfall: Option<Shortfall>,
}

impl Forecast {
    /// The balance at the end of each day from `from` to `until`, for a chart.
    pub fn daily(&self) -> Vec<(NaiveDate, Cents)> {
        let mut out = Vec::new();
        let mut balance = self.balance;
        let mut items = self.items.iter().peekable();
        let mut day = self.from;
        while day <= self.until {
            while let Some(i) = items.next_if(|i| i.date <= day) {
                balance = i.balance;
            }
            out.push((day, balance));
            let Some(next) = day.checked_add_days(Days::new(1)) else { break };
            day = next;
        }
        out
    }

    /// One line that says how the account goes.
    pub fn message(&self) -> String {
        match (&self.shortfall, self.minimum) {
            (Some(s), Some(0)) => trf(
                "{account}: below zero on {date}; {needed} must come in before then",
                &[("account", &self.account), ("date", &fmt_date(s.date)), ("needed", &fmt_eur(s.needed))],
            ),
            (Some(s), _) => trf(
                "{account}: below its minimum on {date}; {needed} must come in before then",
                &[("account", &self.account), ("date", &fmt_date(s.date)), ("needed", &fmt_eur(s.needed))],
            ),
            _ => trf(
                "{account}: lowest balance {lowest} on {date}",
                &[("account", &self.account), ("lowest", &fmt_eur(self.lowest)), ("date", &fmt_date(self.lowest_date))],
            ),
        }
    }
}

/// The meta key of an account's minimum.
fn minimum_key(account_id: i64) -> String {
    format!("minimum_balance:{account_id}")
}

fn is_debt(kind: AccountKind) -> bool {
    matches!(kind, AccountKind::CreditCard | AccountKind::Loan)
}

impl Db {
    /// The minimum the user set for an account, if any.
    pub fn minimum_balance(&self, account_id: i64) -> Option<Cents> {
        self.meta(&minimum_key(account_id)).and_then(|v| v.trim().parse().ok())
    }

    /// Sets the minimum of an account, or takes it away with `None`.
    pub fn set_minimum_balance(&self, account_id: i64, minimum: Option<Cents>) -> Result<()> {
        if !self.accounts(true)?.iter().any(|a| a.id == account_id) {
            bail!(tr("the account does not exist"));
        }
        let key = minimum_key(account_id);
        match minimum {
            Some(m) => self.set_meta(&key, &m.to_string()),
            None => {
                self.conn.execute("DELETE FROM meta WHERE key=?1", [key])?;
                Ok(())
            }
        }
    }

    /// The minimum the projection uses: the one set, else zero for an account that holds money and
    /// none for a card or a loan, which are below zero by their nature.
    pub fn effective_minimum(&self, a: &Account) -> Option<Cents> {
        self.minimum_balance(a.id).or(if is_debt(a.kind) { None } else { Some(0) })
    }

    /// The projection of an account from `today` for `days` days (1 to [`MAX_DAYS`]).
    pub fn forecast(&self, account_id: i64, today: NaiveDate, days: i64) -> Result<Forecast> {
        let a = self
            .accounts(true)?
            .into_iter()
            .find(|a| a.id == account_id)
            .ok_or_else(|| anyhow!(tr("the account does not exist")))?;
        let scheduled = self.scheduled()?;
        self.forecast_of(&a, &scheduled, today, days)
    }

    /// The projections of the accounts in use.
    pub fn forecasts(&self, today: NaiveDate, days: i64) -> Result<Vec<Forecast>> {
        let scheduled = self.scheduled()?;
        self.accounts(false)?.iter().map(|a| self.forecast_of(a, &scheduled, today, days)).collect()
    }

    fn forecast_of(&self, a: &Account, scheduled: &[Scheduled], today: NaiveDate, days: i64) -> Result<Forecast> {
        if !(1..=MAX_DAYS).contains(&days) {
            bail!(trf("the days go from 1 to {max}", &[("max", &MAX_DAYS)]));
        }
        let too_large = || anyhow!(tr("the amount is too large"));
        let until = today
            .checked_add_days(Days::new(days.unsigned_abs()))
            .ok_or_else(|| anyhow!(tr("the date is not valid")))?;
        let mut items: Vec<Item> = Vec::new();
        // What is in the books with a later date: out of today's balance, into the days ahead.
        let later =
            self.transactions(&TxFilter { account_id: Some(a.id), from: today.succ_opt(), ..Default::default() })?;
        let mut balance = a.balance;
        for t in later.iter().filter(|t| t.date > today) {
            balance = balance.checked_sub(t.amount).ok_or_else(too_large)?;
            if t.date <= until {
                items.push(Item {
                    date: t.date,
                    payee: t.payee.clone(),
                    amount: t.amount,
                    balance: 0,
                    scheduled_id: None,
                    overdue: false,
                });
            }
        }
        for s in scheduled {
            let amount = match s.to_account_id {
                None if s.account_id == a.id => s.amount,
                Some(_) if s.account_id == a.id => {
                    s.amount.checked_abs().and_then(|x| x.checked_neg()).ok_or_else(too_large)?
                }
                Some(to) if to == a.id => s.amount.checked_abs().ok_or_else(too_large)?,
                _ => continue,
            };
            let mut date = s.next_date;
            for _ in 0..MAX_OCCURRENCES {
                if date > until {
                    break;
                }
                items.push(Item {
                    date: date.max(today),
                    payee: s.payee.clone(),
                    amount,
                    balance: 0,
                    scheduled_id: Some(s.id),
                    overdue: date < today,
                });
                let next = advance(date, s.every, &s.unit);
                if next <= date {
                    break;
                }
                date = next;
            }
        }
        // A day counts with its balance at its end; within it, money in comes first.
        items.sort_by(|x, y| x.date.cmp(&y.date).then(y.amount.cmp(&x.amount)));
        let minimum = self.effective_minimum(a);
        let (mut lowest, mut lowest_date, mut running) = (Cents::MAX, today, balance);
        let mut first_below = None;
        let mut i = 0;
        let mut day = today;
        loop {
            while let Some(item) = items.get_mut(i).filter(|x| x.date == day) {
                running = running.checked_add(item.amount).ok_or_else(too_large)?;
                item.balance = running;
                i += 1;
            }
            if running < lowest {
                (lowest, lowest_date) = (running, day);
            }
            if first_below.is_none() && minimum.is_some_and(|m| running < m) {
                first_below = Some((day, running));
            }
            match items.get(i) {
                Some(next) => day = next.date,
                None => break,
            }
        }
        let shortfall = match (first_below, minimum) {
            (Some((date, at)), Some(m)) => {
                Some(Shortfall { date, balance: at, needed: m.checked_sub(lowest).ok_or_else(too_large)? })
            }
            _ => None,
        };
        Ok(Forecast {
            account_id: a.id,
            account: a.name.clone(),
            currency: a.currency.clone(),
            from: today,
            until,
            balance,
            minimum,
            items,
            lowest,
            lowest_date,
            end_balance: running,
            shortfall,
        })
    }
}

#[cfg(test)]
mod tests;
