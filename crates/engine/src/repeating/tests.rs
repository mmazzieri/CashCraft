use crate::db::{AccountKind, Cents, Db, NewScheduled, TxFilter};
use crate::ledger::tests::assert_sound;
use crate::money::parse_date;
use chrono::NaiveDate;

fn day(s: &str) -> NaiveDate {
    parse_date(s).unwrap()
}

fn books() -> (Db, i64, i64) {
    let db = Db::open_in_memory().unwrap();
    let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 500_000, "").unwrap();
    let fund = db.add_account("Fund", AccountKind::Investment, "EUR", 0, "").unwrap();
    (db, bank, fund)
}

fn monthly(account: i64, to: Option<i64>, date: &str, amount: Cents) -> NewScheduled {
    NewScheduled {
        account_id: account,
        to_account_id: to,
        amount,
        payee: "Plan".into(),
        memo: String::new(),
        category_id: None,
        next_date: day(date),
        every: 1,
        unit: "month".into(),
    }
}

fn count(db: &Db, account: i64) -> usize {
    db.transactions(&TxFilter { account_id: Some(account), ..Default::default() }).unwrap().len()
}

fn balance(db: &Db, account: i64) -> Cents {
    db.accounts(true).unwrap().into_iter().find(|a| a.id == account).unwrap().balance
}

#[test]
fn a_repeating_transfer_is_written_once_and_the_next_ones_start_a_period_later() {
    let (db, bank, fund) = books();
    let done =
        db.add_repeating(&monthly(bank, Some(fund), "2026-01-31", 10_000), &[], false, day("2026-01-31")).unwrap();
    assert_eq!(done.ids.len(), 2);
    assert_eq!(done.next_date, day("2026-02-28"));
    assert_eq!(balance(&db, fund), 10_000);
    // Posting what is due up to the next date adds only that one.
    db.post_scheduled(day("2026-02-28")).unwrap();
    assert_eq!(count(&db, fund), 2);
    assert_eq!(balance(&db, fund), 20_000);
    assert_sound(&db, "transfer");
}

#[test]
fn a_repeating_transaction_keeps_its_tags_and_its_check_mark() {
    let (db, bank, _) = books();
    let tags = vec!["home".to_string()];
    let done = db.add_repeating(&monthly(bank, None, "2026-03-10", -2_500), &tags, true, day("2026-03-15")).unwrap();
    let t = db.transaction(done.ids[0]).unwrap();
    assert!(t.cleared);
    assert!(t.tags.contains(&"home".to_string()));
    let s = db.scheduled().unwrap();
    assert_eq!((s.len(), s[0].next_date, s[0].amount), (1, day("2026-04-10"), -2_500));
}

#[test]
fn a_day_still_to_come_is_only_scheduled() {
    let (db, bank, fund) = books();
    let done =
        db.add_repeating(&monthly(bank, Some(fund), "2026-05-01", 10_000), &[], false, day("2026-04-20")).unwrap();
    assert!(done.ids.is_empty());
    assert_eq!(done.next_date, day("2026-05-01"));
    assert_eq!(count(&db, fund), 0);
}

#[test]
fn when_the_schedule_cannot_be_made_the_movement_is_not_written_either() {
    let (db, bank, fund) = books();
    let mut bad = monthly(bank, Some(fund), "2026-01-31", 10_000);
    bad.every = 0;
    assert!(db.add_repeating(&bad, &[], false, day("2026-01-31")).is_err());
    bad.every = 1;
    bad.unit = "fortnight".into();
    assert!(db.add_repeating(&bad, &[], false, day("2026-01-31")).is_err());
    assert_eq!((count(&db, fund), db.scheduled().unwrap().len()), (0, 0));
    assert_sound(&db, "refused");
}

#[test]
fn a_repetition_still_to_come_refuses_what_it_could_not_keep() {
    let (db, bank, _) = books();
    let today = day("2026-04-20");
    // A transfer from an account to itself would fail at every posting run: refused when it is made.
    assert!(db.add_repeating(&monthly(bank, Some(bank), "2026-05-01", 10_000), &[], false, today).is_err());
    // A scheduled item has no tags: they are refused rather than dropped.
    let tags = ["home".to_string()];
    assert!(db.add_repeating(&monthly(bank, None, "2026-05-01", -2_500), &tags, false, today).is_err());
    assert!(db.scheduled().unwrap().is_empty());
}
