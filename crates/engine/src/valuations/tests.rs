use crate::db::{AccountKind, Cents, Db, NewTx, TxEdit};
use crate::ledger::tests::assert_sound;
use crate::money::parse_date;
use chrono::NaiveDate;

fn day(s: &str) -> NaiveDate {
    parse_date(s).unwrap()
}

/// A current account and an investment account with 1,000.00 put in from it on 1 March.
fn books() -> (Db, i64, i64) {
    let db = Db::open_in_memory().unwrap();
    let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 500_000, "").unwrap();
    let fund = db.add_account("Fund", AccountKind::Investment, "EUR", 0, "").unwrap();
    move_money(&db, bank, fund, "2026-03-01", 100_000);
    (db, bank, fund)
}

fn move_money(db: &Db, from: i64, to: i64, date: &str, amount: Cents) -> i64 {
    db.add_transfer(from, to, day(date), amount, "Contribution").unwrap().1
}

fn balance(db: &Db, account: i64) -> Cents {
    db.accounts(true).unwrap().into_iter().find(|a| a.id == account).unwrap().balance
}

#[test]
fn a_value_takes_the_balance_to_it_and_counts_as_a_gain_not_as_income() {
    let (db, _, fund) = books();
    let before = db.monthly_totals(12, day("2026-06-30")).unwrap();
    let v = db.record_valuation(fund, day("2026-03-31"), 103_500).unwrap();
    assert_eq!((v.balance, v.amount), (103_500, 3_500));
    assert_eq!(balance(&db, fund), 103_500);
    let p = db.performance(fund).unwrap();
    assert_eq!((p.invested, p.gain), (100_000, 3_500));
    // Neither income nor spending.
    let after = db.monthly_totals(12, day("2026-06-30")).unwrap();
    assert_eq!(
        before.iter().map(|m| (m.income, m.expense)).collect::<Vec<_>>(),
        after.iter().map(|m| (m.income, m.expense)).collect::<Vec<_>>()
    );
    assert_sound(&db, "valued");
}

#[test]
fn the_net_worth_on_a_day_is_the_sum_of_the_balances_on_that_day() {
    let (db, bank, fund) = books();
    db.record_valuation(fund, day("2026-03-31"), 98_000).unwrap();
    move_money(&db, bank, fund, "2026-04-10", 50_000);
    db.record_valuation(fund, day("2026-04-30"), 151_000).unwrap();
    for date in ["2026-03-15", "2026-03-31", "2026-04-15", "2026-04-30", "2026-05-01"] {
        let sum: Cents = db.accounts(true).unwrap().iter().map(|a| db.balance_on(a.id, day(date)).unwrap()).sum();
        assert_eq!(db.net_worth(Some(day(date))).unwrap(), sum, "{date}");
    }
    assert_eq!(db.balance_on(fund, day("2026-03-31")).unwrap(), 98_000);
    assert_eq!(db.balance_on(fund, day("2026-04-30")).unwrap(), 151_000);
}

#[test]
fn money_moved_before_a_value_later_leaves_every_value_as_it_was_written() {
    let (db, bank, fund) = books();
    db.record_valuation(fund, day("2026-03-31"), 101_000).unwrap();
    db.record_valuation(fund, day("2026-04-30"), 102_500).unwrap();
    // A contribution of March, written down only now.
    let late = move_money(&db, bank, fund, "2026-03-20", 20_000);
    let values = db.valuations(fund).unwrap();
    assert_eq!(values.iter().map(|v| v.balance).collect::<Vec<_>>(), [101_000, 102_500]);
    assert_eq!(db.balance_on(fund, day("2026-03-31")).unwrap(), 101_000);
    assert_eq!(db.balance_on(fund, day("2026-04-30")).unwrap(), 102_500);
    assert_eq!(db.performance(fund).unwrap().invested, 120_000);
    // Changed, then deleted: the same again.
    let edit = TxEdit {
        date: day("2026-03-20"),
        amount: 30_000,
        payee: "Contribution".into(),
        memo: String::new(),
        category_id: None,
        cleared: false,
    };
    db.update_transaction(late, &edit).unwrap();
    assert_eq!(db.balance_on(fund, day("2026-04-30")).unwrap(), 102_500);
    db.delete_transaction(late).unwrap();
    assert_eq!(db.balance_on(fund, day("2026-03-31")).unwrap(), 101_000);
    assert_eq!(db.performance(fund).unwrap().invested, 100_000);
    assert_sound(&db, "moved");
}

#[test]
fn a_value_after_the_last_one_is_not_moved_by_money_after_it() {
    let (db, bank, fund) = books();
    db.record_valuation(fund, day("2026-03-31"), 101_000).unwrap();
    move_money(&db, bank, fund, "2026-04-05", 10_000);
    // The balance today is the last value with the money moved since.
    assert_eq!(balance(&db, fund), 111_000);
    assert_eq!(db.performance(fund).unwrap().gain, 1_000);
}

#[test]
fn a_second_value_of_the_same_day_replaces_the_first_and_deleting_it_takes_the_gain_away() {
    let (db, _, fund) = books();
    db.record_valuation(fund, day("2026-03-31"), 90_000).unwrap();
    db.record_valuation(fund, day("2026-04-30"), 95_000).unwrap();
    db.record_valuation(fund, day("2026-03-31"), 99_000).unwrap();
    let values = db.valuations(fund).unwrap();
    assert_eq!(values.iter().map(|v| (v.balance, v.amount)).collect::<Vec<_>>(), [(99_000, -1_000), (95_000, -4_000)]);
    db.delete_valuation(fund, day("2026-03-31")).unwrap();
    let values = db.valuations(fund).unwrap();
    assert_eq!(values.iter().map(|v| (v.balance, v.amount)).collect::<Vec<_>>(), [(95_000, -5_000)]);
    db.delete_valuation(fund, day("2026-04-30")).unwrap();
    assert_eq!((balance(&db, fund), db.performance(fund).unwrap().gain), (100_000, 0));
    assert!(db.delete_valuation(fund, day("2026-04-30")).is_err());
    assert_sound(&db, "replaced");
}

#[test]
fn the_entry_of_a_value_is_not_changed_by_hand_but_can_be_deleted() {
    let (db, _, fund) = books();
    db.record_valuation(fund, day("2026-03-31"), 104_000).unwrap();
    let t = db.transactions(&crate::db::TxFilter { account_id: Some(fund), ..Default::default() }).unwrap();
    let t = t.iter().find(|t| t.amount == 4_000).unwrap().clone();
    assert!(t.category.contains(crate::i18n::tr("Changes in value")));
    let edit = TxEdit {
        date: t.date,
        amount: 1,
        payee: t.payee.clone(),
        memo: String::new(),
        category_id: None,
        cleared: false,
    };
    assert!(db.update_transaction(t.id, &edit).is_err());
    assert!(db.set_category(t.id, None).is_err());
    assert_eq!(balance(&db, fund), 104_000);
    db.delete_transaction(t.id).unwrap();
    assert!(db.valuations(fund).unwrap().is_empty());
    assert_eq!(balance(&db, fund), 100_000);
    assert_sound(&db, "by hand");
}

#[test]
fn only_investment_accounts_have_values_and_hostile_values_are_refused() {
    let (db, bank, fund) = books();
    assert!(db.record_valuation(bank, day("2026-03-31"), 1).is_err());
    assert!(db.record_valuation(9_999, day("2026-03-31"), 1).is_err());
    for v in [-1, crate::money::MAX_AMOUNT + 1, Cents::MAX, Cents::MIN] {
        assert!(db.record_valuation(fund, day("2026-03-31"), v).is_err(), "{v}");
    }
    assert!(db.valuations(fund).unwrap().is_empty());
    db.record_valuation(fund, day("2026-03-31"), crate::money::MAX_AMOUNT).unwrap();
    assert_eq!(balance(&db, fund), crate::money::MAX_AMOUNT);
    assert_sound(&db, "hostile");
}

#[test]
fn deleting_the_account_takes_its_values_away() {
    let (db, bank, fund) = books();
    db.record_valuation(fund, day("2026-03-31"), 110_000).unwrap();
    db.delete_account(fund).unwrap();
    let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM valuations", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    assert_eq!(balance(&db, bank), 400_000);
    assert_sound(&db, "deleted");
}

#[test]
fn a_new_transaction_without_a_transfer_moves_the_values_too() {
    let (db, _, fund) = books();
    db.record_valuation(fund, day("2026-03-31"), 100_500).unwrap();
    // A coupon paid into the account, as income.
    let tx = NewTx {
        account_id: fund,
        date: day("2026-03-15"),
        amount: 700,
        payee: "Coupon".into(),
        memo: String::new(),
        category_id: None,
        ..Default::default()
    };
    db.add_transaction(&tx).unwrap();
    assert_eq!(db.balance_on(fund, day("2026-03-31")).unwrap(), 100_500);
    assert_eq!(db.performance(fund).unwrap().gain, -200);
    assert_sound(&db, "coupon");
}

#[test]
fn a_scheduled_transaction_whose_values_cannot_be_worked_out_fails_alone() {
    let (db, bank, _) = books();
    // Nothing in it before the value: one cent out would need a change beyond the largest amount.
    let fund = db.add_account("Empty fund", AccountKind::Investment, "EUR", 0, "").unwrap();
    db.record_valuation(fund, day("2026-03-31"), crate::money::MAX_AMOUNT).unwrap();
    let item = |account: i64| crate::db::NewScheduled {
        account_id: account,
        amount: -1,
        payee: "Fee".into(),
        next_date: day("2026-03-10"),
        every: 1,
        unit: "month".into(),
        ..Default::default()
    };
    db.add_scheduled(&item(fund)).unwrap();
    db.add_scheduled(&item(bank)).unwrap();
    let posted = db.post_scheduled(day("2026-03-10")).unwrap();
    assert_eq!((posted.posted, posted.failed.len()), (1, 1));
    assert_eq!(balance(&db, fund), crate::money::MAX_AMOUNT);
    assert_sound(&db, "scheduled");
}

#[test]
fn a_scheduled_transaction_that_would_change_a_reconciled_value_fails_alone() {
    use crate::reconcile::{Source, Start};
    let (db, bank, fund) = books();
    db.record_valuation(fund, day("2026-03-31"), 103_500).unwrap();
    let r = db
        .start_reconciliation(&Start {
            account: fund,
            date: day("2026-03-31"),
            balance: 103_500,
            source: Source::Typed,
            rows: &[],
        })
        .unwrap()
        .id;
    let all: Vec<i64> = db
        .transactions(&crate::db::TxFilter { account_id: Some(fund), ..Default::default() })
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect();
    db.clear_for_reconciliation(r, &all, true).unwrap();
    db.finish_reconciliation(r).unwrap();
    let item = |account: i64| crate::db::NewScheduled {
        account_id: account,
        amount: -100,
        payee: "Fee".into(),
        next_date: day("2026-03-10"),
        every: 1,
        unit: "month".into(),
        ..Default::default()
    };
    db.add_scheduled(&item(fund)).unwrap();
    db.add_scheduled(&item(bank)).unwrap();
    let posted = db.post_scheduled(day("2026-03-10")).unwrap();
    assert_eq!((posted.posted, posted.failed.len()), (1, 1));
    assert_eq!(db.balance_on(fund, day("2026-03-31")).unwrap(), 103_500);
    assert_eq!(db.valuations(fund).unwrap()[0].amount, 3_500);
    assert_sound(&db, "reconciled");
}
