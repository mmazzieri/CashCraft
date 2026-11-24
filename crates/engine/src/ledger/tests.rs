use crate::db::*;
use crate::ledger::*;
use crate::ops;
use crate::transfers::OtherSide;
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
fn deleting_an_account_leaves_the_other_side_of_its_transfers_out_of_the_reports() {
    let (db, a, b) = books();
    let (_, y) = db.add_transfer(a, b, day("2026-03-04"), 5_000, "").unwrap();
    let before = db.find_account("Card").unwrap().balance;
    db.delete_account(a).unwrap();
    assert_eq!(db.find_account("Card").unwrap().balance, before);
    let t = db.transaction(y).unwrap();
    assert_eq!((t.kind, t.transfer_peer), (TxKind::Other, None));
    let m = db.monthly_totals(1, day("2026-03-31")).unwrap().remove(0);
    assert_eq!((m.income, m.expense), (0, 0));
    assert!(db.category_totals(day("2026-03-01"), day("2026-03-31")).unwrap().is_empty());
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
    for key in ["code:3010", "capital:reserves", "Reserves", &format!("ledger:{sub}")] {
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

/// The books kept in a second, simpler way: each entry is a list of accounts and amounts. Everything that is
/// neither an account of the user nor an asset, liability or equity account added by hand is account 0.
#[derive(Default)]
struct Model {
    entries: std::collections::BTreeMap<i64, Vec<(i64, Cents)>>,
    /// Opening balances, by account of the user.
    openings: std::collections::BTreeMap<i64, Cents>,
    hashes: std::collections::HashSet<(i64, String)>,
    /// The keys of an entry: a transfer may have two.
    hash_of: Vec<(i64, (i64, String))>,
}

impl Model {
    fn balances(&self) -> std::collections::BTreeMap<i64, Cents> {
        let mut out = std::collections::BTreeMap::new();
        for (a, v) in self.openings.iter().chain(self.entries.values().flatten().map(|(a, v)| (a, v))) {
            *out.entry(*a).or_default() += *v;
        }
        out.remove(&0);
        out.retain(|_, v| *v != 0);
        out
    }

    fn remove(&mut self, entry: i64) {
        self.entries.remove(&entry);
        self.hash_of.retain(|(e, h)| {
            if *e == entry {
                self.hashes.remove(h);
            }
            *e != entry
        });
    }
}

/// Accounts that the model follows: positive is an account of the user, negative one of the chart.
fn followed(a: &LedgerAccount) -> i64 {
    match a.money_account_id {
        Some(m) => m,
        None if a.system.is_none() && a.category_id.is_none() => -a.id,
        None => 0,
    }
}

/// One random operation, sound or not, on the books and on the model; what it was.
fn step(db: &Db, m: &mut Model, r: &mut Lcg, n: usize) -> (String, anyhow::Result<()>) {
    let accounts: Vec<i64> = db.accounts(true).unwrap().iter().map(|a| a.id).collect();
    let categories: Vec<i64> = db.categories().unwrap().iter().map(|c| c.id).collect();
    let chart: Vec<(i64, i64)> = db.ledger_accounts().unwrap().iter().map(|a| (a.id, followed(a))).collect();
    let txs = db.transactions(&TxFilter::default()).unwrap();
    let date = day("2025-01-01") + chrono::Days::new(r.below(700) as u64);
    let category = if r.below(3) == 0 { None } else { r.pick(&categories) };
    let account = r.pick(&accounts).unwrap_or(0);
    let tx = if txs.is_empty() { None } else { Some(&txs[r.below(txs.len())]) };
    let (tx_id, tx_entry) = tx.map_or((0, 0), |t| (t.id, t.entry_id));
    let what = r.below(29);
    let done: anyhow::Result<()> = (|| {
        match what {
            0 => {
                let kind = AccountKind::ALL[r.below(6)];
                let currency = if r.below(4) == 0 { "USD" } else { "EUR" };
                let opening = r.amount();
                let id = db.add_account(&format!("A{n}"), kind, currency, opening, "")?;
                m.openings.insert(id, opening);
            }
            1 => {
                db.delete_account(account)?;
                m.openings.remove(&account);
                let mut gone = Vec::new();
                for (id, lines) in &mut m.entries {
                    if !lines.iter().any(|l| l.0 == account) {
                        continue;
                    }
                    for l in lines.iter_mut().filter(|l| l.0 == account) {
                        l.0 = 0;
                    }
                    if lines.iter().all(|l| l.0 == 0) {
                        gone.push(*id);
                    }
                }
                for id in gone {
                    m.remove(id);
                }
                m.hashes.retain(|h| h.0 != account);
                m.hash_of.retain(|(_, h)| h.0 != account);
            }
            2 => {
                let opening = if r.below(3) == 0 { 0 } else { r.amount() };
                db.update_account(account, &format!("B{n}"), AccountKind::ALL[r.below(6)], opening, "", false)?;
                if accounts.contains(&account) {
                    m.openings.insert(account, opening);
                }
            }
            3..=7 => {
                let amount = r.amount();
                let hash = if r.below(4) == 0 { Some(format!("h{}", r.below(20))) } else { None };
                let known = hash.clone().is_some_and(|h| m.hashes.contains(&(account, h)));
                let id = db.add_transaction(&NewTx {
                    account_id: account,
                    date,
                    amount,
                    payee: ["FARMACIA", "shop", "bar "][r.below(3)].into(),
                    category_id: category,
                    import_hash: hash.clone(),
                    ..Default::default()
                })?;
                assert_eq!(id.is_none(), known);
                if let Some(id) = id {
                    let entry = db.transaction(id)?.entry_id;
                    m.entries.insert(entry, vec![(account, amount), (0, -amount)]);
                    if let Some(h) = hash {
                        m.hashes.insert((account, h.clone()));
                        m.hash_of.push((entry, (account, h)));
                    }
                }
            }
            8 | 9 => {
                let (to, amount) = (r.pick(&accounts).unwrap_or(0), r.amount());
                let (x, _) = db.add_transfer(account, to, date, amount, "t")?;
                let amount = amount.abs();
                m.entries.insert(db.transaction(x)?.entry_id, vec![(account, -amount), (to, amount)]);
            }
            10 | 11 => {
                let amount = r.amount();
                let edit = TxEdit { date, amount, payee: "e".into(), category_id: category, ..Default::default() };
                db.update_transaction(tx_id, &edit)?;
                let t = tx.unwrap();
                let lines = m.entries.get_mut(&tx_entry).unwrap();
                if t.kind != TxKind::Split {
                    assert_eq!(lines.len(), 2);
                    for l in lines {
                        l.1 = if l.0 == t.account_id { amount } else { -amount };
                    }
                    if t.kind == TxKind::Other && category.is_some() {
                        let account = t.account_id;
                        m.entries.insert(tx_entry, vec![(account, amount), (0, -amount)]);
                    }
                }
            }
            12 => {
                db.set_category(tx_id, category)?;
                for l in m.entries.get_mut(&tx_entry).unwrap() {
                    if l.0 != tx.unwrap().account_id {
                        l.0 = 0;
                    }
                }
            }
            13 => db.set_cleared(tx_id, r.below(2) == 0)?,
            14 => {
                db.delete_transaction(tx_id)?;
                m.remove(tx_entry);
            }
            15 => db.delete_category(r.pick(&categories).unwrap_or(0))?,
            16 => {
                db.category_by_path(&format!("C{}:D{n}", r.below(3)), r.below(2) == 0, true)?;
            }
            17 => {
                db.apply_rules()?;
            }
            18 => {
                let class = if r.below(2) == 0 { Some(Class::ALL[r.below(5)]) } else { None };
                let parent = if r.below(2) == 0 { r.pick(&chart).map(|c| c.0) } else { None };
                db.create_ledger_account(&format!("L{n}"), class, parent, "", None)?;
            }
            19 => {
                let entries: Vec<i64> = db.entries(&EntryFilter::default())?.iter().map(|e| e.id).collect();
                let id = r.pick(&entries).unwrap_or(0);
                db.delete_entry(id)?;
                m.remove(id);
            }
            20 => {
                let c = r.pick(&chart).unwrap_or((0, 0)).0;
                if r.below(2) == 0 {
                    db.delete_ledger_account(c)?;
                } else {
                    db.rename_ledger_account(c, &format!("R{n}"))?;
                }
            }
            21 => db.set_account_currency(account, ["USD", "EUR"][r.below(2)])?,
            22 => {
                let postings: Vec<i64> =
                    db.entries(&EntryFilter::default())?.iter().flat_map(|e| e.postings.iter().map(|p| p.id)).collect();
                db.set_posting_category(r.pick(&postings).unwrap_or(0), category)?;
            }
            25 | 26 => {
                // Half of them have something to join.
                let t = tx.ok_or_else(|| anyhow::anyhow!("none"))?;
                let mut target = account;
                if what == 26 {
                    let opposite = txs.iter().find(|x| x.amount == -t.amount && x.account_id != t.account_id);
                    target = match opposite {
                        Some(x) => x.account_id,
                        None => {
                            db.transfer_candidates(tx_id, target)?;
                            let day = t.date + chrono::Days::new(r.below(10) as u64);
                            let tx = NewTx { account_id: target, date: day, amount: -t.amount, ..Default::default() };
                            let id = db.add_transaction(&tx)?.unwrap();
                            m.entries.insert(db.transaction(id)?.entry_id, vec![(target, -t.amount), (0, t.amount)]);
                            target
                        }
                    };
                }
                let candidates = db.transfer_candidates(tx_id, target)?;
                let other = match r.pick(&candidates.iter().map(|c| (c.id, c.entry_id)).collect::<Vec<_>>()) {
                    Some((id, entry)) => {
                        m.entries.remove(&entry);
                        for (e, _) in m.hash_of.iter_mut().filter(|h| h.0 == entry) {
                            *e = tx_entry;
                        }
                        OtherSide::Join(id)
                    }
                    None => OtherSide::Auto,
                };
                let done = db.turn_into_transfer(tx_id, target, other)?;
                m.entries.insert(tx_entry, vec![(t.account_id, t.amount), (target, -t.amount)]);
                let (a, b) = (db.transaction(done.id)?, db.transaction(done.other_id)?);
                assert_eq!((a.date, a.payee, a.memo), (t.date, t.payee.clone(), t.memo.clone()));
                if let Some(c) = candidates.iter().find(|c| c.id == b.id) {
                    assert_eq!((b.date, b.payee, b.memo), (c.date, c.payee.clone(), c.memo.clone()));
                }
            }
            27 | 28 => {
                let t = txs.iter().find(|t| t.kind == TxKind::Transfer).ok_or_else(|| anyhow::anyhow!("none"))?;
                let peer = db.transaction(t.transfer_peer.unwrap())?;
                let keep = r.below(2) == 0;
                let other = db.separate_transfer(t.id, keep)?;
                assert_eq!(other, t.transfer_peer.filter(|_| keep));
                m.entries.insert(t.entry_id, vec![(t.account_id, t.amount), (0, -t.amount)]);
                let moved = match other {
                    Some(id) => db.transaction(id)?.entry_id,
                    None => -1,
                };
                for (e, h) in m.hash_of.iter_mut() {
                    if *e == t.entry_id && h.0 == peer.account_id {
                        *e = moved;
                    }
                }
                match other {
                    Some(id) => {
                        let now = db.transaction(id)?;
                        assert_eq!((now.date, now.payee, now.memo), (peer.date, peer.payee, peer.memo));
                        m.entries.insert(moved, vec![(peer.account_id, peer.amount), (0, -peer.amount)]);
                    }
                    None => m.remove(-1),
                }
            }
            _ => {
                // Balanced but for the last case.
                let mut lines: Vec<(i64, i64, Cents)> = (0..1 + r.below(5))
                    .map(|_| {
                        let (id, key) = r.pick(&chart).unwrap();
                        (id, key, r.amount())
                    })
                    .collect();
                let sum: Cents = lines.iter().map(|l| l.2).sum();
                let off = if what == 24 { 1 } else { 0 };
                let (id, key) = r.pick(&chart).unwrap();
                lines.push((id, key, off - sum));
                let new = lines.iter().map(|l| line(l.0, l.2)).collect();
                let done = db.post_entry(&NewEntry { date, payee: "p".into(), memo: String::new(), lines: new });
                assert!(what != 24 || done.is_err());
                m.entries.insert(done?, lines.iter().map(|l| (l.1, l.2)).collect());
            }
        }
        Ok(())
    })();
    (format!("step {n}, operation {what}, account {account}"), done)
}

#[test]
fn random_operations_keep_the_books_balanced_and_as_the_model() {
    let mut worked = [0; 29];
    for seed in 1..=25 {
        let mut r = Lcg(seed);
        let (db, a, b) = books();
        let mut model = Model::default();
        model.openings.extend([(a, 100_000), (b, -5_000)]);
        for n in 0..120 {
            let before = dump(&db.conn);
            let (what, done) = step(&db, &mut model, &mut r, n);
            let what = format!("seed {seed}, {what}");
            match done {
                Ok(()) => {
                    let op = what.split("operation ").nth(1).unwrap().split(',').next().unwrap();
                    worked[op.parse::<usize>().unwrap()] += 1;
                }
                Err(_) => assert_eq!(dump(&db.conn), before, "{what}"),
            }
            assert_sound(&db, &what);
            let mut real = std::collections::BTreeMap::new();
            for a in db.ledger_accounts().unwrap() {
                let sum: Cents = a.balances.iter().map(|b| b.amount).sum();
                if followed(&a) != 0 && sum != 0 {
                    real.insert(followed(&a), sum);
                }
            }
            assert_eq!(real, model.balances(), "{what}");
            // The register and the net worth agree with the balances.
            for a in db.accounts(true).unwrap() {
                let txs = db.transactions(&TxFilter { account_id: Some(a.id), ..Default::default() }).unwrap();
                assert_eq!(txs.first().and_then(|t| t.balance).unwrap_or(a.opening_balance), a.balance, "{what}");
            }
            let owned: Cents = db
                .ledger_accounts()
                .unwrap()
                .iter()
                .filter(|a| matches!(a.class, Class::Asset | Class::Liability))
                .flat_map(|a| a.balances.iter().map(|b| b.amount))
                .sum();
            assert_eq!(db.net_worth(None).unwrap(), owned, "{what}");
        }
    }
    // Every operation was tried and some of each succeeded, but for the one that is never balanced.
    assert!(worked.iter().enumerate().all(|(i, n)| (*n > 0) == (i != 24)), "{worked:?}");
    assert!(worked.iter().sum::<i32>() > 1500, "{worked:?}");
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
                    for table in ["postings", "entries"] {
                        for _ in text.matches(&format!("{verb} {table}")) {
                            out.push(format!("{name}: {verb} {table}"));
                        }
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
    let expected = [
        // Every account goes.
        "db.rs: delete from entries",
        // `remove_entry`.
        "ledger.rs: delete from entries",
        "ledger.rs: delete from postings",
        "ledger.rs: insert into entries",
        "ledger.rs: insert into postings",
        "ledger.rs: update entries",
        "ledger.rs: update postings",
    ];
    assert_eq!(found, expected);
}

/// S1: what would make a sum too large is refused, and everything still reads.
#[test]
fn balanced_huge_entries_are_refused_before_any_sum_overflows() {
    let (db, _, _) = books();
    let (bank, health) = (ledger(&db, "Bank"), ledger(&db, "Health"));
    let max = crate::money::MAX_AMOUNT;
    let post = |lines: Vec<NewLine>| {
        let before = dump(&db.conn);
        let done = db.post_entry(&entry(lines));
        if done.is_err() {
            assert_eq!(dump(&db.conn), before);
        }
        done.is_ok()
    };
    // One account, always on the same side.
    let mut n = 0;
    while post(vec![line(bank, max), line(health, -max)]) {
        n += 1;
        assert!(n < 100);
    }
    assert!(n >= 5);
    // The other side is still open.
    assert!(post(vec![line(bank, -max), line(health, max)]));
    // No account grows, but their movements do: sums of one sign, or up to a date, must fit too.
    let lines: Vec<NewLine> =
        (0..MAX_LINES as i64).map(|i| line(health, if i % 2 == 0 { max } else { -max })).collect();
    let mut n = 0;
    while post(lines.clone()) {
        n += 1;
        assert!(n < 100);
    }
    assert!(n >= 10);
    assert_sound(&db, "huge");
    let all = json!({"account": "Bank", "month": "2026-03", "from": "2026-01-01", "to": "2026-12-31", "id": 1});
    for tool in [
        "list_accounts",
        "list_transactions",
        "spending_by_category",
        "monthly_summary",
        "net_worth",
        "budget_status",
        "list_ledger_accounts",
        "list_entries",
        "trial_balance",
        "verify_ledger",
    ] {
        let a = if tool == "list_transactions" { json!({"account": "Bank"}) } else { all.clone() };
        ops::call(&db, tool, &a, None).unwrap_or_else(|e| panic!("{tool}: {e:#}"));
    }
    let txs = db.transactions(&TxFilter::default()).unwrap();
    db.register_totals(&txs, None).unwrap();
    // What is there can always be taken away.
    for e in db.entries(&EntryFilter::default()).unwrap() {
        if !e.opening {
            db.delete_entry(e.id).unwrap();
        }
    }
    assert_sound(&db, "emptied");
}

/// S2 and S4: what a split counts for, in the register, by month and by category.
#[test]
fn a_split_counts_its_categories_once() {
    let (db, bank_id, _) = books();
    let id = |k: &str| ledger(&db, k);
    let cash = db.add_account("Cash", AccountKind::Cash, "EUR", 0, "").unwrap();
    let (from, to) = (day("2026-03-01"), day("2026-03-31"));
    let check = |lines: Vec<NewLine>, income: Cents, expense: Cents, what: &str| {
        let e = db.post_entry(&entry(lines)).unwrap();
        let m = db.monthly_totals(1, to).unwrap().remove(0);
        assert_eq!((m.income, m.expense), (income, expense), "{what}: month");
        let totals = db.category_totals(from, to).unwrap();
        assert_eq!(CategoryTotal::income_and_expense(&totals).unwrap(), (income, expense), "{what}: categories");
        let txs = db.transactions(&TxFilter::default()).unwrap();
        if !txs.is_empty() {
            assert_eq!(db.register_totals(&txs, None).unwrap(), (income, expense), "{what}: register");
        }
        db.delete_entry(e).unwrap();
    };
    check(vec![line(id("Bank"), 150), line(id("Bank and taxes:Taxes"), 50), line(id("Salary"), -200)], 200, 50, "pay");
    check(vec![line(id("Bank"), -60), line(id("Card"), -40), line(id("Groceries"), 100)], 0, 100, "paid twice");
    check(vec![line(id("Bank"), -60), line(id("Card"), -40), line(id("Cash"), 100)], 0, 0, "own accounts");
    // A refund of two categories is a smaller expense; money back on one is income, as in 0.2.0.
    check(vec![line(id("Bank"), 30), line(id("Groceries"), -10), line(id("Health"), -20)], 0, -30, "refunds");
    check(vec![line(id("Bank"), 30), line(id("Groceries"), -30)], 30, 0, "refund");
    check(vec![line(id("Groceries"), 100), line(id("Health"), -100)], 0, 0, "between expenses");
    check(vec![line(id("Salary"), 100), line(id("Groceries"), -100)], -100, -100, "between classes");
    // Filtered by a category, only that part of it; in the register of one account, once.
    db.post_entry(&entry(vec![line(id("Bank"), -60), line(id("Groceries"), 40), line(id("Health"), 20)])).unwrap();
    let health = db.find_category("Health").unwrap().id;
    let txs = db.transactions(&TxFilter { category_id: Some(health), ..Default::default() }).unwrap();
    assert_eq!(db.register_totals(&txs, Some(health)).unwrap(), (0, 20));
    let txs = db.transactions(&TxFilter { account_id: Some(bank_id), ..Default::default() }).unwrap();
    assert_eq!(db.register_totals(&txs, None).unwrap(), (0, 60));
    let txs = db.transactions(&TxFilter { account_id: Some(cash), ..Default::default() }).unwrap();
    assert_eq!(db.register_totals(&txs, None).unwrap(), (0, 0));
}

/// S3: no income or expense appears when an account goes, not even from a split.
#[test]
fn deleting_an_account_never_makes_income_or_expense() {
    let (db, a, _) = books();
    let id = |k: &str| ledger(&db, k);
    let capital = db.create_ledger_account("Capital", Some(Class::Equity), None, "", None).unwrap();
    db.post_entry(&entry(vec![line(id("Bank"), 900), line(capital, -900)])).unwrap();
    db.post_entry(&entry(vec![line(id("Bank"), -60), line(id("Card"), -40), line(id("Groceries"), 100)])).unwrap();
    db.add_transfer(a, db.find_account("Card").unwrap().id, day("2026-03-04"), 700, "").unwrap();
    let figures = || {
        let m = db.monthly_totals(1, day("2026-03-31")).unwrap().remove(0);
        (m.income, m.expense, db.find_account("Card").unwrap().balance)
    };
    let before = figures();
    db.delete_account(a).unwrap();
    assert_eq!(figures(), before);
    assert_sound(&db, "deleted");
}

/// S5: part of a split can be categorized again, and is found.
#[test]
fn part_of_a_split_is_categorized_again() {
    let (db, _, _) = books();
    let id = |k: &str| ledger(&db, k);
    let e =
        db.post_entry(&entry(vec![line(id("Bank"), -60), line(id("Groceries"), 40), line(id("Health"), 20)])).unwrap();
    db.delete_category(db.find_category("Health").unwrap().id).unwrap();
    let uncategorized = || db.transactions(&TxFilter { uncategorized: true, ..Default::default() }).unwrap();
    assert_eq!(uncategorized().len(), 1);
    // A rule never touches a split.
    db.add_rule("x", db.find_category("Travel").unwrap().id, "").unwrap();
    let before = dump(&db.conn);
    assert_eq!(db.apply_rules().unwrap(), 0);
    assert_eq!(dump(&db.conn), before);
    let find = |name: &str| db.entry(e).unwrap().postings.into_iter().find(|p| p.account == name).unwrap().id;
    let posting = find("Uncategorized");
    ops::call(&db, "set_posting_category", &json!({"posting_id": posting, "category": "Travel"}), None).unwrap();
    assert!(uncategorized().is_empty());
    assert_eq!(find("Travel"), posting);
    let before = dump(&db.conn);
    for bad in [find("Bank"), 999_999] {
        assert!(db.set_posting_category(bad, None).is_err());
    }
    assert!(db.set_posting_category(posting, Some(999_999)).is_err());
    assert_eq!(dump(&db.conn), before);
    assert_sound(&db, "categorized");
}

/// S5: accounts added by hand.
#[test]
fn hand_made_accounts_are_renamed_and_deleted_when_unused() {
    let (db, _, _) = books();
    let top = db.create_ledger_account("Capital", Some(Class::Equity), None, "", None).unwrap();
    let sub = db.create_ledger_account("Reserves", None, Some(top), "", None).unwrap();
    db.create_ledger_account("Other", Some(Class::Equity), None, "", None).unwrap();
    let before = dump(&db.conn);
    for (id, name) in [(top, ""), (top, "a:b"), (top, "OTHER"), (ledger(&db, "Bank"), "x"), (999_999, "x")] {
        assert!(db.rename_ledger_account(id, name).is_err(), "{name}");
    }
    for id in [top, ledger(&db, "Bank"), ledger(&db, "Health"), ledger(&db, "system:uncategorized"), 999_999] {
        assert!(db.delete_ledger_account(id).is_err());
    }
    assert_eq!(dump(&db.conn), before);
    let a = json!({"ledger_account": "Capital", "name": "Equity"});
    ops::call(&db, "rename_ledger_account", &a, None).unwrap();
    let e = db.post_entry(&entry(vec![line(ledger(&db, "Bank"), 5), line(sub, -5)])).unwrap();
    assert!(db.delete_ledger_account(sub).is_err());
    db.delete_entry(e).unwrap();
    for key in ["Equity:Reserves", "Equity"] {
        ops::call(&db, "delete_ledger_account", &json!({"ledger_account": key}), None).unwrap();
    }
    assert!(db.find_ledger_account("Equity").is_err());
    assert_sound(&db, "hand made");
}

/// S6: currencies are written one way, and can be put right.
#[test]
fn currencies_are_normalized_and_can_be_changed() {
    let (db, a, b) = books();
    for euro in ["", " eur ", "€", "Euro", "EUROS"] {
        assert_eq!(check_currency(euro).unwrap(), "EUR", "{euro}");
    }
    assert_eq!(check_currency("usd").unwrap(), "USD");
    for bad in ["$", "EU", "EURR", "E1R", "ÀBC", "é"] {
        assert!(check_currency(bad).is_err(), "{bad}");
        assert!(db.add_account("X", AccountKind::Bank, bad, 0, "").is_err(), "{bad}");
        assert!(db.create_ledger_account("X", Some(Class::Asset), None, "", Some(bad)).is_err(), "{bad}");
    }
    let c = db.add_account("E", AccountKind::Bank, "Euro", 0, "").unwrap();
    assert_eq!(db.find_account("E").unwrap().currency, "EUR");
    db.delete_account(c).unwrap();
    let health = db.find_category("Health").unwrap().id;
    let tx = NewTx { account_id: a, amount: -100, payee: "p".into(), category_id: Some(health), ..Default::default() };
    db.add_transaction(&tx).unwrap();
    ops::call(&db, "set_account_currency", &json!({"account": "Bank", "currency": "usd"}), None).unwrap();
    let bank = db.find_account("Bank").unwrap();
    assert_eq!((bank.currency.as_str(), bank.balance, bank.opening_balance), ("USD", 99_900, 100_000));
    assert_sound(&db, "dollars");
    assert!(db.entries(&EntryFilter::default()).unwrap().iter().flat_map(|e| &e.postings).any(|p| p.currency == "USD"));
    db.set_account_currency(a, "EUR").unwrap();
    db.add_transfer(a, b, day("2026-03-04"), 5, "").unwrap();
    let before = dump(&db.conn);
    assert!(db.set_account_currency(a, "USD").is_err());
    assert!(db.set_account_currency(a, "$").is_err());
    assert_eq!(dump(&db.conn), before);
}

/// M5: a number is never guessed, and neither is a name that two accounts have.
#[test]
fn lookups_in_the_chart_are_never_guesses() {
    let (db, _, _) = books();
    let bank = ledger(&db, "Bank");
    let e = db.find_ledger_account(&bank.to_string()).unwrap_err().to_string();
    assert!(e.contains(&format!("ledger:{bank}")), "{e}");
    let two = |account: serde_json::Value| {
        let postings = json!([{"account": account, "amount": 1}, {"account": "Health", "amount": -1}]);
        ops::call(&db, "post_entry", &json!({"postings": postings}), None)
    };
    assert!(two(json!(bank)).is_err() && two(json!(bank.to_string())).is_err());
    let v = two(json!(format!("ledger:{bank}"))).unwrap();
    assert_eq!(v["entry"]["postings"][0]["ledger_account_id"], bank);
    let all = ops::call(&db, "list_ledger_accounts", &json!({}), None).unwrap();
    assert!(all["accounts"][0]["ledger_account_id"].is_i64() && all["accounts"][0].get("id").is_none());
    // A category with the name of an account every file has.
    for name in ["Uncategorized", "opening_balances"] {
        db.category_by_path(name, false, true).unwrap();
        let e = db.find_ledger_account(name).unwrap_err().to_string();
        assert!(e.contains("ledger:"), "{e}");
        assert!(db.find_ledger_account(&format!("system:{}", name.to_lowercase())).unwrap().system.is_some());
    }
}

/// M7: what is owned or owed counts, whoever made the account.
#[test]
fn the_net_worth_has_every_asset_and_liability() {
    let (db, _, _) = books();
    let car = db.create_ledger_account("Car", Some(Class::Asset), None, "", None).unwrap();
    let debt = db.create_ledger_account("Debt", Some(Class::Liability), None, "", None).unwrap();
    db.post_entry(&entry(vec![line(car, 9_000), line(debt, -9_000)])).unwrap();
    db.post_entry(&entry(vec![line(car, 500), line(ledger(&db, "system:opening_balances"), -500)])).unwrap();
    assert_eq!(db.net_worth(None).unwrap(), 95_500);
    assert_eq!(db.net_worth_history(1, day("2026-03-31")).unwrap()[0].1, 95_500);
    assert_eq!(db.net_worth(Some(day("2026-03-03"))).unwrap(), 95_000);
    for tool in ["net_worth", "list_accounts"] {
        assert_eq!(ops::call(&db, tool, &json!({}), None).unwrap()["net_worth"], json!(955.0));
    }
}

/// S6: one that fails does not stop the others.
#[test]
fn a_scheduled_transaction_that_fails_does_not_stop_the_others() {
    let (db, a, b) = books();
    let item = |payee: &str, to, amount| NewScheduled {
        account_id: a,
        to_account_id: to,
        amount,
        payee: payee.into(),
        next_date: day("2026-01-01"),
        every: 1,
        unit: "month".into(),
        ..Default::default()
    };
    db.add_scheduled(&item("Bad", Some(b), 5)).unwrap();
    db.add_scheduled(&item("Good", None, -7)).unwrap();
    // The card becomes a dollar one after that.
    db.set_account_currency(b, "USD").unwrap();
    let v = ops::call(&db, "post_scheduled", &json!({"until": "2026-02-15"}), None).unwrap();
    assert_eq!((v["posted"].clone(), v["failed"][0]["payee"].clone()), (json!(2), json!("Bad")));
    assert!(v["message"].as_str().unwrap().contains("Bad"));
    let dates: Vec<_> = db.scheduled().unwrap().iter().map(|s| (s.payee.clone(), d2s(s.next_date))).collect();
    assert_eq!(dates, [("Bad".into(), "2026-01-01".to_string()), ("Good".into(), "2026-03-01".to_string())]);
    assert_eq!(db.transactions(&TxFilter::default()).unwrap().len(), 2);
}

/// Pages of the journal.
#[test]
fn entries_come_in_pages() {
    let (db, _, _) = books();
    let (bank, health) = (ledger(&db, "Bank"), ledger(&db, "Health"));
    for _ in 0..7 {
        db.post_entry(&entry(vec![line(bank, -1), line(health, 1)])).unwrap();
    }
    let page = |offset| {
        let f = EntryFilter { ledger_account_id: Some(health), offset, limit: Some(3), ..Default::default() };
        db.entries(&f).unwrap().iter().map(|e| e.id).collect::<Vec<_>>()
    };
    let all: Vec<i64> = [page(0), page(3), page(6), page(9)].concat();
    assert_eq!(all.len(), 7);
    assert!(all.windows(2).all(|w| w[0] > w[1]));
}
