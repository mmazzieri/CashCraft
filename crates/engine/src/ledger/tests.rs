use crate::db::*;
use crate::ledger::*;
use crate::ops;
use rusqlite::Connection;
use serde_json::json;
use std::path::Path;

pub struct Lcg(pub u64);
impl Lcg {
    pub fn below(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as usize % n.max(1)
    }
    pub fn amount(&mut self) -> Cents {
        let a = self.below(200_000) as Cents + 1;
        if self.below(3) == 0 { a } else { -a }
    }
    pub fn pick<T: Copy>(&mut self, v: &[T]) -> Option<T> {
        if v.is_empty() { None } else { Some(v[self.below(v.len())]) }
    }
}

/// Every row of every table, and the format: two equal dumps are two equal files.
pub fn dump(conn: &Connection) -> String {
    let mut st = conn.prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name").unwrap();
    let tables: Vec<String> = st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    let mut out = format!("format {version}\n");
    for t in tables {
        let mut st = conn.prepare(&format!("SELECT * FROM \"{t}\" ORDER BY rowid")).unwrap();
        let n = st.column_count();
        let mut rows = st.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            let cells: Vec<String> =
                (0..n).map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap())).collect();
            out.push_str(&format!("{t}: {}\n", cells.join("|")));
        }
    }
    out
}

pub fn assert_sound(db: &Db, what: &str) {
    let t = db.trial_balance(None).unwrap();
    assert!(t.is_zero(), "{what}: {:?}", t.totals);
    let sum: Cents = t.lines.iter().map(|l| l.amount).sum();
    assert_eq!(sum, 0, "{what}");
    let v = db.verify_ledger().unwrap();
    assert!(v.problems.is_empty(), "{what}: {:?}", v.problems);
}

fn day(s: &str) -> chrono::NaiveDate {
    crate::money::parse_date(s).unwrap()
}

fn books() -> (Db, i64, i64) {
    let db = Db::open_in_memory().unwrap();
    let a = db.add_account("Bank", AccountKind::Bank, "EUR", 100_000, "").unwrap();
    let b = db.add_account("Card", AccountKind::CreditCard, "EUR", -5_000, "").unwrap();
    (db, a, b)
}

fn ledger(db: &Db, name: &str) -> i64 {
    db.find_ledger_account(name).unwrap().id
}

fn line(account: i64, amount: Cents) -> NewLine {
    NewLine { ledger_account_id: account, amount, currency: None }
}

fn entry(lines: Vec<NewLine>) -> NewEntry {
    NewEntry { date: day("2026-03-04"), payee: "x".into(), memo: "m".into(), lines }
}

#[test]
fn an_unbalanced_entry_is_refused_and_the_books_do_not_change() {
    let (db, _, _) = books();
    let (bank, health, card) = (ledger(&db, "Bank"), ledger(&db, "Health"), ledger(&db, "Card"));
    let usd = db.create_ledger_account("Dollars", Some(Class::Asset), None, "", Some("usd")).unwrap();
    db.post_entry(&entry(vec![line(bank, -100), line(health, 100)])).unwrap();
    let before = dump(&db.conn);
    let mut wrong = vec![
        vec![],
        vec![line(bank, 100)],
        vec![line(bank, 0)],
        vec![line(bank, 100), line(health, -99)],
        vec![line(bank, 100), line(health, 100)],
        vec![line(bank, 100), line(999_999, -100)],
        vec![line(bank, i64::MIN), line(health, i64::MIN)],
        vec![line(bank, i64::MAX), line(health, 1), line(card, i64::MIN)],
        vec![line(bank, crate::money::MAX_AMOUNT + 1), line(health, -crate::money::MAX_AMOUNT - 1)],
        // Different currencies never balance each other.
        vec![line(bank, 100), line(usd, -100)],
        vec![line(bank, 100), line(usd, -100), line(health, 0)],
        vec![line(bank, 100), NewLine { currency: Some("USD".into()), ..line(bank, -100) }],
        vec![line(bank, 100), NewLine { currency: Some("USD".into()), ..line(health, -100) }],
    ];
    wrong.push(vec![line(bank, 0); MAX_LINES + 1]);
    for lines in wrong {
        assert!(db.post_entry(&entry(lines.clone())).is_err(), "{lines:?}");
        assert_eq!(dump(&db.conn), before);
        // Not even when the caller goes on inside a transaction of its own.
        db.atomically(|| {
            assert!(db.post_entry(&entry(lines)).is_err());
            Ok(())
        })
        .unwrap();
        assert_eq!(dump(&db.conn), before);
    }
    assert_sound(&db, "refused");
}

#[test]
fn several_currencies_are_fine_when_each_balances() {
    let (db, _, _) = books();
    let usd = db.create_ledger_account("Dollars", Some(Class::Asset), None, "", Some("usd")).unwrap();
    let (bank, travel) = (ledger(&db, "Bank"), ledger(&db, "Travel"));
    let in_usd = |a, c| NewLine { currency: Some("usd".into()), ..line(a, c) };
    let in_eur = |a, c| NewLine { currency: Some("EUR".into()), ..line(a, c) };
    db.post_entry(&entry(vec![line(usd, -700), in_usd(travel, 700), line(bank, -300), in_eur(travel, 300)])).unwrap();
    assert_sound(&db, "two currencies");
    let t = db.trial_balance(None).unwrap();
    assert_eq!(t.totals.iter().map(|t| t.currency.as_str()).collect::<Vec<_>>(), ["EUR", "USD"]);
}

#[test]
fn the_sample_file_balances() {
    for l in [crate::i18n::Lang::It, crate::i18n::Lang::En] {
        crate::i18n::with_lang(l, || {
            let db = Db::open_in_memory().unwrap();
            crate::demo::seed(&db).unwrap();
            assert_sound(&db, "sample");
            let t = db.trial_balance(None).unwrap();
            assert!(t.lines.len() > 15);
            assert!(t.totals[0].debit > 0);
            // At any date.
            for m in 0..12 {
                assert!(db.trial_balance(Some(ops::months_back(today(), m))).unwrap().is_zero());
            }
            db.clear_accounts().unwrap();
            assert_sound(&db, "cleared");
            assert!(db.trial_balance(None).unwrap().lines.is_empty());
        });
    }
}

#[test]
fn a_salary_with_taxes_is_split_and_cannot_be_broken() {
    let (db, bank_account, _) = books();
    let (bank, salary, taxes) = (ledger(&db, "Bank"), ledger(&db, "Salary"), ledger(&db, "Bank and taxes:Taxes"));
    let e = db.post_entry(&entry(vec![line(bank, 150_000), line(taxes, 50_000), line(salary, -200_000)])).unwrap();
    let t = db.transactions(&TxFilter::default()).unwrap().remove(0);
    assert_eq!((t.kind, t.entry_id, t.category_id, t.transfer_peer), (TxKind::Split, e, None, None));
    for (l, label) in [(crate::i18n::Lang::En, "Split"), (crate::i18n::Lang::It, "Suddiviso")] {
        crate::i18n::with_lang(l, || assert_eq!(db.transaction(t.id).unwrap().category, label));
    }
    let before = dump(&db.conn);
    let edit = TxEdit {
        date: t.date,
        amount: t.amount,
        payee: t.payee.clone(),
        memo: t.memo.clone(),
        category_id: None,
        cleared: false,
    };
    let health = db.find_category("Health").unwrap().id;
    assert!(db.update_transaction(t.id, &TxEdit { amount: 1, ..edit.clone() }).is_err());
    assert!(db.update_transaction(t.id, &TxEdit { category_id: Some(health), ..edit.clone() }).is_err());
    assert!(db.set_category(t.id, Some(health)).is_err());
    assert!(db.set_category(t.id, None).is_err());
    for a in [json!({"id": t.id, "category": ""}), json!({"id": t.id, "category": "Health"})] {
        assert!(ops::call(&db, "update_transaction", &a, None).is_err());
    }
    assert_eq!(db.apply_rules().unwrap(), 0);
    assert_eq!(dump(&db.conn), before);
    // The rest of it can be changed.
    db.update_transaction(t.id, &TxEdit { payee: "Salary".into(), date: day("2026-03-05"), ..edit }).unwrap();
    db.set_cleared(t.id, true).unwrap();
    let after = db.transaction(t.id).unwrap();
    assert_eq!((after.payee.as_str(), after.amount, after.cleared), ("Salary", 150_000, true));
    // Reports see each category with its own amount, and the filters find the transaction in both.
    let totals = db.category_totals(day("2026-03-01"), day("2026-03-31")).unwrap();
    let mut seen: Vec<_> = totals.iter().map(|c| (c.category.as_str(), c.total)).collect();
    seen.sort();
    assert_eq!(seen, [("Bank and taxes:Taxes", -50_000), ("Salary", 200_000)]);
    let m = db.monthly_totals(1, day("2026-03-31")).unwrap().remove(0);
    assert_eq!((m.income, m.expense), (200_000, 50_000));
    for c in ["Bank and taxes", "Salary"] {
        let f = TxFilter { category_id: Some(db.find_category(c).unwrap().id), ..Default::default() };
        assert_eq!(db.transactions(&f).unwrap().len(), 1, "{c}");
    }
    assert!(db.transactions(&TxFilter { uncategorized: true, ..Default::default() }).unwrap().is_empty());
    // Without one of its categories it stays balanced and split.
    db.delete_category(db.find_category("Bank and taxes").unwrap().id).unwrap();
    assert_sound(&db, "split without a category");
    assert_eq!(db.transaction(t.id).unwrap().kind, TxKind::Split);
    db.delete_transaction(t.id).unwrap();
    assert!(db.entry(e).is_err());
    assert_eq!(db.accounts(true).unwrap().iter().find(|a| a.id == bank_account).unwrap().balance, 100_000);
}

#[test]
fn money_from_equity_is_not_income_and_keeps_its_account() {
    let (db, _, _) = books();
    let capital = db.create_ledger_account("Capital", Some(Class::Equity), None, "3000", None).unwrap();
    db.post_entry(&entry(vec![line(ledger(&db, "Bank"), 500), line(capital, -500)])).unwrap();
    let t = db.transactions(&TxFilter::default()).unwrap().remove(0);
    assert_eq!((t.kind, t.category.as_str()), (TxKind::Other, "[Capital]"));
    let m = db.monthly_totals(1, day("2026-03-31")).unwrap().remove(0);
    assert_eq!((m.income, m.expense), (0, 0));
    let edit = TxEdit { date: t.date, amount: 700, payee: "p".into(), ..Default::default() };
    db.update_transaction(t.id, &edit).unwrap();
    let t = db.transaction(t.id).unwrap();
    assert_eq!((t.kind, t.amount), (TxKind::Other, 700));
    ops::call(&db, "update_transaction", &json!({"id": t.id, "category": ""}), None).unwrap();
    assert_eq!(db.transaction(t.id).unwrap().kind, TxKind::Uncategorized);
    assert_sound(&db, "equity");
}

#[test]
fn a_transfer_keeps_both_texts_and_ids() {
    let (db, a, b) = books();
    let (x, y) = db.add_transfer(a, b, day("2026-03-04"), 5_000, "memo").unwrap();
    let (tx, ty) = (db.transaction(x).unwrap(), db.transaction(y).unwrap());
    assert_eq!((tx.transfer_peer, ty.transfer_peer), (Some(y), Some(x)));
    assert_eq!((tx.payee.as_str(), ty.payee.as_str()), ("Transfer to Card", "Transfer from Bank"));
    let edit =
        TxEdit { date: day("2026-03-06"), amount: 900, payee: "Back".into(), memo: "n".into(), ..Default::default() };
    db.update_transaction(y, &edit).unwrap();
    let (tx, ty) = (db.transaction(x).unwrap(), db.transaction(y).unwrap());
    assert_eq!((tx.amount, ty.amount, tx.date), (-900, 900, ty.date));
    assert_eq!((tx.payee.as_str(), ty.payee.as_str(), tx.memo.as_str()), ("Transfer to Card", "Back", "n"));
    assert!(db.set_category(x, None).is_err());
    db.delete_transaction(y).unwrap();
    assert!(db.transaction(x).is_err());
    assert_sound(&db, "transfer");
}

#[test]
fn deleting_an_account_leaves_the_other_side_of_its_transfers_uncategorized() {
    let (db, a, b) = books();
    let (_, y) = db.add_transfer(a, b, day("2026-03-04"), 5_000, "").unwrap();
    let before = db.find_account("Card").unwrap().balance;
    db.delete_account(a).unwrap();
    assert_eq!(db.find_account("Card").unwrap().balance, before);
    let t = db.transaction(y).unwrap();
    assert_eq!((t.kind, t.transfer_peer), (TxKind::Uncategorized, None));
    assert_eq!(db.monthly_totals(1, day("2026-03-31")).unwrap()[0].income, 5_000);
    assert_sound(&db, "account deleted");
    assert!(db.find_ledger_account("Bank").is_err());
}

#[test]
fn transfers_between_currencies_are_refused() {
    let (db, a, _) = books();
    let usd = db.add_account("Dollars", AccountKind::Bank, " usd ", 1_000, "").unwrap();
    let before = dump(&db.conn);
    let e = db.add_transfer(a, usd, day("2026-03-04"), 100, "").unwrap_err().to_string();
    assert!(e.contains("EUR") && e.contains("USD"), "{e}");
    let s = NewScheduled {
        account_id: a,
        to_account_id: Some(usd),
        amount: 1,
        payee: "x".into(),
        next_date: today(),
        every: 1,
        unit: "month".into(),
        ..Default::default()
    };
    assert!(db.add_scheduled(&s).is_err());
    assert_eq!(dump(&db.conn), before);
    // A category takes the currency of the account.
    let health = db.find_category("Health").unwrap().id;
    let tx =
        NewTx { account_id: usd, amount: -100, payee: "p".into(), category_id: Some(health), ..Default::default() };
    db.add_transaction(&tx).unwrap();
    assert_sound(&db, "currencies");
    assert_eq!(db.find_account("Dollars").unwrap().balance, 900);
}

#[test]
fn opening_balances_are_entries_and_stay_editable() {
    let (db, a, _) = books();
    let opening = |v| {
        db.update_account(a, "Bank", AccountKind::Loan, v, "", false).unwrap();
        let acc = db.find_account("Bank").unwrap();
        assert_eq!((acc.opening_balance, acc.balance), (v, v));
        assert_eq!(db.find_ledger_account("Bank").unwrap().class, Class::Liability);
        assert_sound(&db, "opening");
        assert!(db.transactions(&TxFilter { account_id: Some(a), ..Default::default() }).unwrap().is_empty());
    };
    for v in [5, -7, 0, 12] {
        opening(v);
    }
    let e = db.entries(&EntryFilter::default()).unwrap().into_iter().find(|e| e.opening).unwrap();
    assert!(db.delete_entry(e.id).is_err());
    for p in e.postings {
        assert!(db.transaction(p.id).is_err());
        assert!(db.delete_transaction(p.id).is_err());
        assert!(db.set_category(p.id, None).is_err());
    }
    // Counted before any transaction.
    assert_eq!(db.net_worth_history(1, day("1970-01-31")).unwrap()[0].1, 12 - 5_000);
}

#[test]
fn the_chart_of_accounts_grows() {
    let (db, _, _) = books();
    let top = db.create_ledger_account("Capital", Some(Class::Equity), None, "3000", None).unwrap();
    let sub = db.create_ledger_account("Reserves", None, Some(top), "3010", None).unwrap();
    let all = db.ledger_accounts().unwrap();
    let s = all.iter().find(|a| a.id == sub).unwrap();
    assert_eq!((s.class, s.path.as_str(), s.parent_id), (Class::Equity, "Capital:Reserves", Some(top)));
    for key in ["3010", "capital:reserves", "Reserves", &sub.to_string()] {
        assert_eq!(db.find_ledger_account(key).unwrap().id, sub, "{key}");
    }
    let before = dump(&db.conn);
    for (name, class, parent, code, currency) in [
        ("", Some(Class::Equity), None, "", None),
        ("a:b", Some(Class::Equity), None, "", None),
        ("x", None, None, "", None),
        ("x", Some(Class::Asset), Some(top), "", None),
        ("x", None, Some(999_999), "", None),
        ("CAPITAL", Some(Class::Equity), None, "", None),
        ("x", Some(Class::Equity), None, "3010", None),
        ("x", Some(Class::Expense), None, "", Some("USD")),
        ("x", None, Some(ledger(&db, OPENING)), "", None),
        ("x", None, Some(ledger(&db, UNCATEGORIZED)), "", None),
    ] {
        assert!(db.create_ledger_account(name, class, parent, code, currency).is_err(), "{name} {code}");
    }
    assert_eq!(dump(&db.conn), before);
    // An account of the bank can have accounts under it; an expense account is a category.
    db.create_ledger_account("Reserved", None, Some(ledger(&db, "Bank")), "", None).unwrap();
    let dentist = db.create_ledger_account("Dentist", None, Some(ledger(&db, "Health")), "", None).unwrap();
    assert_eq!(db.find_category("Health:Dentist").unwrap().id, {
        let all = db.ledger_accounts().unwrap();
        all.iter().find(|a| a.id == dentist).unwrap().category_id.unwrap()
    });
    db.delete_account(db.find_account("Bank").unwrap().id).unwrap();
    assert_eq!(db.find_ledger_account("Reserved").unwrap().parent_id, None);
    assert_sound(&db, "chart");
}

#[test]
fn verification_finds_what_is_wrong() {
    let (db, _, _) = books();
    let tamper = |sql: &str| {
        db.conn.execute_batch("PRAGMA foreign_keys=OFF; BEGIN;").unwrap();
        db.conn.execute_batch(sql).unwrap();
        let n = db.verify_ledger().unwrap().problems.len();
        db.conn.execute_batch("ROLLBACK; PRAGMA foreign_keys=ON;").unwrap();
        assert!(n > 0, "{sql}");
    };
    tamper("UPDATE postings SET amount=amount+1 WHERE id=1");
    tamper("DELETE FROM postings WHERE id=1");
    tamper("DELETE FROM entries");
    tamper("DELETE FROM ledger_accounts WHERE money_account_id=1");
    tamper("DELETE FROM ledger_accounts WHERE category_id=1");
    tamper("DELETE FROM ledger_accounts WHERE system='uncategorized'");
    tamper("DELETE FROM accounts WHERE id=1");
    tamper("UPDATE ledger_accounts SET class='asset' WHERE category_id=1");
    tamper("UPDATE ledger_accounts SET currency='USD' WHERE money_account_id=1");
    tamper("UPDATE postings SET ledger_account_id=(SELECT id FROM ledger_accounts WHERE system='uncategorized')");
    assert_sound(&db, "untouched");
}

#[test]
fn operations_take_exact_amounts() {
    let (db, _, _) = books();
    let post = |amount: serde_json::Value| {
        let a = json!({"date": "2026-01-31", "payee": "p", "postings": [
            {"account": "Bank", "amount": amount}, {"account": "Health", "amount": "-12.30"}]});
        ops::call(&db, "post_entry", &a, None)
    };
    for bad in [json!("12.301"), json!(12.301), json!("12.299"), json!(0.1 + 0.2), json!("12,3049"), json!(null)] {
        assert!(post(bad.clone()).is_err(), "{bad}");
    }
    for good in [json!("12.30"), json!(12.3), json!("12,3"), json!("€ 12.30")] {
        let v = post(good).unwrap();
        assert_eq!(v["entry"]["postings"][0]["amount"], "12.30");
        assert_eq!(v["entry"]["postings"][1]["amount"], "-12.30");
    }
    for (big, text) in [("1,000", "1000.00"), ("1000000000000", "1000000000000.00"), ("0.000", "0.00")] {
        let a = json!({"postings": [{"account": "Bank", "amount": big}, {"account": "Health", "amount": format!("-{big}")}]});
        assert_eq!(ops::call(&db, "post_entry", &a, None).unwrap()["entry"]["postings"][0]["amount"], text);
    }
    let t = ops::call(&db, "trial_balance", &json!({"date": "2026-01-31"}), None).unwrap();
    assert_eq!(t["balanced"], true);
    assert_eq!(t["totals"][0]["difference"], "0.00");
    let v = ops::call(&db, "verify_ledger", &json!({}), None).unwrap();
    assert_eq!(v["ok"], true);
    let l = ops::call(&db, "list_entries", &json!({"ledger_account": "Health", "limit": 2}), None).unwrap();
    assert_eq!(l["count"], 2);
    let id = l["entries"][0]["id"].clone();
    assert_eq!(ops::call(&db, "get_entry", &json!({"id": id}), None).unwrap()["entry"]["id"], id);
    ops::call(&db, "delete_entry", &json!({"id": id}), None).unwrap();
    assert!(ops::call(&db, "get_entry", &json!({"id": id}), None).is_err());
}

/// One random operation, sound or not; what it was.
fn step(db: &Db, r: &mut Lcg, n: usize) -> (String, anyhow::Result<()>) {
    let accounts: Vec<i64> = db.accounts(true).unwrap().iter().map(|a| a.id).collect();
    let categories: Vec<i64> = db.categories().unwrap().iter().map(|c| c.id).collect();
    let chart: Vec<i64> = db.ledger_accounts().unwrap().iter().map(|a| a.id).collect();
    let txs: Vec<i64> = db.transactions(&TxFilter::default()).unwrap().iter().map(|t| t.id).collect();
    let date = day("2025-01-01") + chrono::Days::new(r.below(700) as u64);
    let category = if r.below(3) == 0 { None } else { r.pick(&categories) };
    let account = r.pick(&accounts).unwrap_or(0);
    let tx = r.pick(&txs).unwrap_or(0);
    let what = r.below(24);
    let done = match what {
        0 => {
            let kind = AccountKind::ALL[r.below(6)];
            let currency = if r.below(4) == 0 { "USD" } else { "EUR" };
            db.add_account(&format!("A{n}"), kind, currency, r.amount(), "").map(|_| ())
        }
        1 => db.delete_account(account),
        2 => db.update_account(
            account,
            &format!("B{n}"),
            AccountKind::ALL[r.below(6)],
            if r.below(3) == 0 { 0 } else { r.amount() },
            "",
            false,
        ),
        3..=7 => db
            .add_transaction(&NewTx {
                account_id: account,
                date,
                amount: r.amount(),
                payee: ["FARMACIA", "shop", "bar "][r.below(3)].into(),
                category_id: category,
                import_hash: if r.below(4) == 0 { Some(format!("h{}", r.below(20))) } else { None },
                ..Default::default()
            })
            .map(|_| ()),
        8 | 9 => db.add_transfer(account, r.pick(&accounts).unwrap_or(0), date, r.amount(), "t").map(|_| ()),
        10 | 11 => db.update_transaction(
            tx,
            &TxEdit { date, amount: r.amount(), payee: "e".into(), category_id: category, ..Default::default() },
        ),
        12 => db.set_category(tx, category),
        13 => db.set_cleared(tx, r.below(2) == 0),
        14 => db.delete_transaction(tx),
        15 => db.delete_category(r.pick(&categories).unwrap_or(0)),
        16 => db.category_by_path(&format!("C{}:D{n}", r.below(3)), r.below(2) == 0, true).map(|_| ()),
        17 => db.apply_rules().map(|_| ()),
        18 => {
            let class = if r.below(2) == 0 { Some(Class::ALL[r.below(5)]) } else { None };
            let parent = if r.below(2) == 0 { r.pick(&chart) } else { None };
            db.create_ledger_account(&format!("L{n}"), class, parent, "", None).map(|_| ())
        }
        19 => {
            let entries: Vec<i64> = db.entries(&EntryFilter::default()).unwrap().iter().map(|e| e.id).collect();
            db.delete_entry(r.pick(&entries).unwrap_or(0))
        }
        _ => {
            // Balanced but for the last case.
            let mut lines: Vec<NewLine> =
                (0..1 + r.below(5)).map(|_| line(r.pick(&chart).unwrap_or(0), r.amount())).collect();
            let sum: Cents = lines.iter().map(|l| l.amount).sum();
            let off = if what == 23 { 1 } else { 0 };
            lines.push(line(r.pick(&chart).unwrap_or(0), off - sum));
            let done = db.post_entry(&NewEntry { date, payee: "p".into(), memo: String::new(), lines }).map(|_| ());
            assert!(what != 23 || done.is_err());
            done
        }
    };
    (format!("step {n}, operation {what}, account {account}"), done)
}

#[test]
fn random_operations_keep_the_books_balanced() {
    let mut worked = 0;
    for seed in 1..=25 {
        let mut r = Lcg(seed);
        let (db, _, _) = books();
        for n in 0..120 {
            let before = dump(&db.conn);
            let balances: Vec<(i64, Cents)> = db.accounts(true).unwrap().iter().map(|a| (a.id, a.balance)).collect();
            let (what, done) = step(&db, &mut r, n);
            let what = format!("seed {seed}, {what}");
            match done {
                Ok(()) => worked += 1,
                Err(_) => assert_eq!(dump(&db.conn), before, "{what}"),
            }
            assert_sound(&db, &what);
            if what.contains("operation 1,") || what.contains("operation 15,") {
                for a in db.accounts(true).unwrap() {
                    assert!(balances.contains(&(a.id, a.balance)), "{what}");
                }
            }
            // The register agrees with the balance.
            for a in db.accounts(true).unwrap() {
                let txs = db.transactions(&TxFilter { account_id: Some(a.id), ..Default::default() }).unwrap();
                assert_eq!(txs.first().and_then(|t| t.balance).unwrap_or(a.opening_balance), a.balance, "{what}");
            }
        }
    }
    assert!(worked > 1500, "{worked}");
}

/// Postings are written in one place: anything else could leave an entry unbalanced.
#[test]
fn only_save_entry_writes_postings() {
    fn scan(dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_str().unwrap().to_string();
            if p.is_dir() {
                scan(&p, out);
            } else if name.ends_with(".rs") && !name.starts_with("tests") {
                let text = std::fs::read_to_string(&p).unwrap().to_lowercase();
                for verb in ["insert into", "insert or ignore into", "insert or replace into", "update", "delete from"]
                {
                    for _ in text.matches(&format!("{verb} postings")) {
                        out.push(format!("{name}: {verb}"));
                    }
                }
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut found = Vec::new();
    for dir in ["crates", "apps"] {
        scan(&root.join(dir), &mut found);
    }
    found.sort();
    assert_eq!(found, ["ledger.rs: delete from", "ledger.rs: insert into", "ledger.rs: update"]);
}
