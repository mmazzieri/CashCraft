//! Sample data shown the first time the window opens (deleted with a click).

use crate::db::{self, AccountKind, Db, NewScheduled, NewTx};
use crate::i18n::tr;
use anyhow::Result;
use chrono::{Datelike, Duration, Months, NaiveDate};

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % ((hi - lo).max(1) as u64)) as i64
    }
}

/// Fills a new file, in the language its categories were created in.
pub fn seed(db: &Db) -> Result<()> {
    let today = db::today();
    let cat = |p: &str| db.category_by_path(p, false, false).ok();
    let conto = db.add_account(tr("Current account"), AccountKind::Bank, "EUR", 248_000, "")?;
    let carta = db.add_account(tr("Credit card"), AccountKind::CreditCard, "EUR", 0, "")?;
    let cash = db.add_account(tr("Wallet"), AccountKind::Cash, "EUR", 12_000, "")?;
    let risp = db.add_account(tr("Deposit account"), AccountKind::Savings, "EUR", 850_000, "")?;
    let mut rng = Lcg(42);
    let start = crate::ops::months_back(today, 6);
    let add = |acc: i64, d: NaiveDate, amt: i64, payee: &str, memo: &str, c: Option<i64>| -> Result<()> {
        if d <= today {
            db.add_transaction(&NewTx {
                account_id: acc,
                date: d,
                amount: amt,
                payee: payee.into(),
                memo: memo.into(),
                category_id: c,
                cleared: d < today - Duration::days(4),
                import_hash: None,
                card_row: false,
            })?;
        }
        Ok(())
    };
    let mut month = start;
    while month <= today {
        let day = |d: u32| month.with_day(d).unwrap_or(month);
        add(conto, day(1), -75_000, tr("Apartment rent"), tr("Monthly rent"), cat(tr("Home:Rent and mortgage")))?;
        add(conto, day(27), 215_000, tr("Salary"), tr("Salary transfer"), cat(tr("Salary")))?;
        add(
            conto,
            day(5),
            -rng.range(5_500, 9_500),
            tr("Electricity"),
            tr("Bill by direct debit"),
            cat(tr("Home:Utilities")),
        )?;
        add(conto, day(12), -2_990, tr("Home fiber"), tr("Internet subscription"), cat(tr("Subscriptions")))?;
        add(carta, day(8), -1_299, tr("Video streaming"), tr("Monthly subscription"), cat(tr("Subscriptions")))?;
        add(conto, day(28), -350, tr("Account fee"), tr("Account maintenance"), cat(tr("Bank and taxes:Fees")))?;
        if month.month().is_multiple_of(2) {
            add(
                conto,
                day(18),
                -rng.range(4_000, 7_000),
                tr("Gas and water"),
                tr("Two-monthly bill"),
                cat(tr("Home:Utilities")),
            )?;
        }
        for w in 0..4 {
            let d = day(3 + w * 7);
            add(
                carta,
                d,
                -rng.range(4_500, 11_000),
                tr("Downtown Supermarket"),
                tr("Weekly shopping"),
                cat(tr("Groceries")),
            )?;
            add(
                cash,
                d + Duration::days(2),
                -rng.range(250, 900),
                tr("Square Café"),
                tr("Breakfast"),
                cat(tr("Restaurants and bars")),
            )?;
        }
        add(
            carta,
            day(14),
            -rng.range(2_500, 8_000),
            tr("Gino's Pizzeria"),
            tr("Dinner"),
            cat(tr("Restaurants and bars")),
        )?;
        add(carta, day(10), -rng.range(5_000, 7_000), tr("Gas Station"), tr("Refueling"), cat(tr("Transport:Fuel")))?;
        add(carta, day(24), -rng.range(5_000, 7_000), tr("Gas Station"), tr("Refueling"), cat(tr("Transport:Fuel")))?;
        add(conto, day(16), -rng.range(800, 4_000), tr("Town Pharmacy"), "", cat(tr("Health")))?;
        add(carta, day(20), -rng.range(2_000, 9_000), tr("Cinema and books"), "", cat(tr("Leisure")))?;
        if month.month().is_multiple_of(3) {
            add(carta, day(21), -rng.range(6_000, 16_000), tr("Clothes Shop"), "", cat(tr("Clothing")))?;
        }
        add(conto, day(19), -rng.range(1_500, 6_000), tr("Card payment 0412"), "", None)?;
        // Card payment and savings.
        let pay = day(2);
        if pay <= today && month > start {
            db.add_transfer(conto, carta, pay, 42_000, tr("Card statement payment"))?;
        }
        let save = day(28);
        if save <= today {
            db.add_transfer(conto, risp, save, 30_000, tr("Monthly savings"))?;
        }
        if day(9) <= today {
            db.add_transfer(conto, cash, day(9), 10_000, tr("Cash withdrawal"))?;
        }
        let Some(next) = month.checked_add_months(Months::new(1)) else { break };
        month = next;
    }
    add(risp, db::first_of_month(today), 1_240, tr("Interest earned"), "", cat(tr("Interest")))?;

    for (c, amt) in [
        (tr("Groceries"), 40_000),
        (tr("Restaurants and bars"), 15_000),
        (tr("Transport:Fuel"), 14_000),
        (tr("Leisure"), 8_000),
        (tr("Home:Utilities"), 12_000),
        (tr("Subscriptions"), 4_500),
    ] {
        if let Some(id) = cat(c) {
            db.set_budget(id, amt)?;
        }
    }
    let next = |d: u32| {
        let m = today.with_day(d).unwrap_or(today);
        if m > today { m } else { m.checked_add_months(Months::new(1)).unwrap_or(m) }
    };
    let repeats = [
        (tr("Apartment rent"), None, -75_000, tr("Monthly rent"), cat(tr("Home:Rent and mortgage")), 1),
        (tr("Salary"), None, 215_000, "", cat(tr("Salary")), 27),
        (tr("Monthly savings"), Some(risp), 30_000, tr("Monthly savings"), None, 28),
    ];
    for (payee, to_account_id, amount, memo, category_id, day) in repeats {
        db.add_scheduled(&NewScheduled {
            account_id: conto,
            to_account_id,
            amount,
            payee: payee.into(),
            memo: memo.into(),
            category_id,
            next_date: next(day),
            every: 1,
            unit: "month".into(),
        })?;
    }
    db.set_meta("demo", "1")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Lang, with_lang};

    /// The categories of the sample data are found by name: none may be missing in any language.
    #[test]
    fn sample_data_is_complete_in_every_language() {
        let figures = |l| {
            with_lang(l, || {
                let db = Db::open_in_memory().unwrap();
                seed(&db).unwrap();
                for t in db.transactions(&Default::default()).unwrap() {
                    let free = t.transfer_peer.is_some() || t.payee == tr("Card payment 0412");
                    assert_eq!(t.category_id.is_none(), free, "{t:?}");
                }
                let scheduled = db.scheduled().unwrap();
                assert!(scheduled.iter().all(|s| s.to_account_id.is_some() || s.category_id.is_some()));
                let amounts: i64 = db.accounts(true).unwrap().iter().map(|a| a.balance).sum();
                (amounts, db.budget_status(db::today()).unwrap().len(), scheduled.len())
            })
        };
        assert_eq!(figures(Lang::It), figures(Lang::En));
        assert_eq!(figures(Lang::En).1, 6);
    }
}
