use super::*;
use crate::db::{AccountKind, NewScheduled, NewTx};
use crate::ledger::tests::assert_sound;
use crate::money::parse_date;

fn day(s: &str) -> NaiveDate {
    parse_date(s).unwrap()
}

fn tx(db: &Db, account_id: i64, date: &str, amount: Cents, payee: &str) {
    db.add_transaction(&NewTx { account_id, date: day(date), amount, payee: payee.into(), ..Default::default() })
        .unwrap()
        .unwrap();
}

fn every_month(db: &Db, account_id: i64, to: Option<i64>, amount: Cents, payee: &str, next: &str) -> i64 {
    db.add_scheduled(&NewScheduled {
        account_id,
        to_account_id: to,
        amount,
        payee: payee.into(),
        memo: String::new(),
        category_id: None,
        next_date: day(next),
        every: 1,
        unit: "month".into(),
    })
    .unwrap()
}

#[test]
fn a_monthly_payment_shows_the_first_day_below_zero_and_what_is_needed() {
    let db = Db::open_in_memory().unwrap();
    let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 100_000, "").unwrap();
    every_month(&db, bank, None, -40_000, "Rent", "2026-01-31");
    let f = db.forecast(bank, day("2026-01-15"), 90).unwrap();
    // 31 Jan, end of February, 31 March: three rents.
    let rents: Vec<NaiveDate> = f.items.iter().map(|i| i.date).collect();
    assert_eq!(rents.len(), 3);
    assert!(rents.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(f.minimum, Some(0));
    let s = f.shortfall.clone().unwrap();
    assert_eq!(s.date, rents[2]);
    assert_eq!(s.balance, f.lowest);
    assert_eq!(f.lowest + s.needed, 0);
    assert!(f.message().contains("below zero"));
    assert_sound(&db, "forecast");
}

#[test]
fn a_transfer_counts_on_both_accounts() {
    let db = Db::open_in_memory().unwrap();
    let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 10_000, "").unwrap();
    let savings = db.add_account("Savings", AccountKind::Savings, "EUR", 0, "").unwrap();
    every_month(&db, bank, Some(savings), 3_000, "To savings", "2026-02-01");
    let today = day("2026-01-20");
    let out = db.forecast(bank, today, 20).unwrap();
    let into = db.forecast(savings, today, 20).unwrap();
    assert!(out.end_balance < out.balance);
    assert!(into.end_balance > into.balance);
    assert_eq!(out.balance - out.end_balance, into.end_balance - into.balance);
    assert_sound(&db, "transfer");
}

#[test]
fn an_overdue_occurrence_counts_today() {
    let db = Db::open_in_memory().unwrap();
    let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 1_000, "").unwrap();
    every_month(&db, bank, None, -5_000, "Insurance", "2026-01-05");
    let today = day("2026-01-20");
    let f = db.forecast(bank, today, 5).unwrap();
    let first = f.items.first().unwrap();
    assert!(first.overdue);
    assert_eq!(first.date, today);
    assert_eq!(f.shortfall.unwrap().date, today);
}

#[test]
fn a_minimum_is_crossed_or_not() {
    let db = Db::open_in_memory().unwrap();
    let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 50_000, "").unwrap();
    every_month(&db, bank, None, -10_000, "Loan", "2026-02-10");
    let today = day("2026-01-20");
    assert!(db.forecast(bank, today, 30).unwrap().shortfall.is_none());
    db.set_minimum_balance(bank, Some(45_000)).unwrap();
    let f = db.forecast(bank, today, 30).unwrap();
    let s = f.shortfall.clone().unwrap();
    assert_eq!(s.date, day("2026-02-10"));
    assert_eq!(f.lowest + s.needed, 45_000);
    assert!(f.message().contains("minimum"));
    db.set_minimum_balance(bank, None).unwrap();
    assert_eq!(db.minimum_balance(bank), None);
    assert!(db.set_minimum_balance(bank + 99, Some(1)).is_err());
}

#[test]
fn a_booked_transaction_with_a_later_date_is_projected_on_its_day() {
    let db = Db::open_in_memory().unwrap();
    let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 0, "").unwrap();
    tx(&db, bank, "2026-01-02", 20_000, "Salary");
    tx(&db, bank, "2026-01-25", -30_000, "Car");
    let today = day("2026-01-10");
    let f = db.forecast(bank, today, 30).unwrap();
    // Today's balance leaves out the later transaction, which comes on its day.
    assert!(f.balance > f.end_balance);
    assert_eq!(f.items.len(), 1);
    assert_eq!(f.shortfall.unwrap().date, day("2026-01-25"));
    assert_sound(&db, "later");
}

#[test]
fn a_card_has_no_minimum_unless_one_is_set() {
    let db = Db::open_in_memory().unwrap();
    let card = db.add_account("Card", AccountKind::CreditCard, "EUR", -10_000, "").unwrap();
    every_month(&db, card, None, -5_000, "Shop", "2026-02-01");
    let today = day("2026-01-20");
    let f = db.forecast(card, today, 30).unwrap();
    assert_eq!(f.minimum, None);
    assert!(f.shortfall.is_none());
    assert!(f.message().contains("lowest"));
    db.set_minimum_balance(card, Some(-12_000)).unwrap();
    assert!(db.forecast(card, today, 30).unwrap().shortfall.is_some());
}

#[test]
fn hostile_days_are_refused() {
    let db = Db::open_in_memory().unwrap();
    let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 0, "").unwrap();
    for d in [0, -1, MAX_DAYS + 1, i64::MAX, i64::MIN] {
        assert!(db.forecast(bank, day("2026-01-01"), d).is_err(), "{d}");
    }
    assert!(db.forecast(bank, NaiveDate::MAX, 1).is_err());
    assert!(db.forecast(bank + 99, day("2026-01-01"), 1).is_err());
}

#[test]
fn the_daily_balances_follow_the_items() {
    let db = Db::open_in_memory().unwrap();
    let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 1_000, "").unwrap();
    every_month(&db, bank, None, -100, "Fee", "2026-01-03");
    let f = db.forecast(bank, day("2026-01-01"), 10).unwrap();
    let daily = f.daily();
    assert_eq!(daily.len(), 11);
    assert_eq!(daily.first().unwrap().1, f.balance);
    assert_eq!(daily.last().unwrap().1, f.end_balance);
    assert!(daily.iter().any(|&(_, b)| b == f.lowest));
}

#[test]
fn deleting_an_account_forgets_its_minimum() {
    let db = Db::open_in_memory().unwrap();
    let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 0, "").unwrap();
    db.set_minimum_balance(bank, Some(500)).unwrap();
    db.delete_account(bank).unwrap();
    assert_eq!(db.meta(&format!("minimum_balance:{bank}")), None);
    assert_sound(&db, "deleted");
}

#[test]
fn every_account_in_use_has_a_forecast() {
    let db = Db::open_in_memory().unwrap();
    db.add_account("A", AccountKind::Bank, "EUR", 0, "").unwrap();
    db.add_account("B", AccountKind::Cash, "EUR", 0, "").unwrap();
    assert_eq!(db.forecasts(day("2026-01-01"), 30).unwrap().len(), 2);
}
