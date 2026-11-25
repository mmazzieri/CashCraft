use super::*;
use crate::db::{AccountKind, NewTx, month_range};
use crate::importer;
use crate::ledger::tests::{assert_sound, dump};
use crate::money::parse_date;
use crate::ops;
use serde_json::json;

const BANK: &str = "Data;Descrizione;Importo\n31/01/2026;BONIFICO A RISPARMIO;-500,00\n02/02/2026;BAR;-1,20\n";
const SAVINGS: &str = "Data;Descrizione;Importo\n02/02/2026;BONIFICO RICEVUTO;500,00\n03/02/2026;INTERESSI;3,00\n";

fn import(db: &Db, account: i64, text: &str) -> usize {
    let t = importer::analyze(text, "UTF-8").unwrap();
    let (rows, skipped) = importer::parse_rows(&t, &t.mapping, false);
    importer::import_rows(db, account, &rows, skipped).unwrap().added
}

fn books() -> (Db, i64, i64) {
    let db = Db::open_in_memory().unwrap();
    let a = db.add_account("Bank", AccountKind::Bank, "EUR", 100_000, "").unwrap();
    let b = db.add_account("Savings", AccountKind::Savings, "EUR", 0, "").unwrap();
    (db, a, b)
}

fn txs(db: &Db, account: i64) -> Vec<Transaction> {
    db.transactions(&TxFilter { account_id: Some(account), ..Default::default() }).unwrap()
}

fn find(db: &Db, account: i64, payee: &str) -> Transaction {
    txs(db, account).into_iter().find(|t| t.payee == payee).unwrap()
}

/// Income and expenses of January and February.
fn months(db: &Db) -> Vec<(Cents, Cents)> {
    let m = db.monthly_totals(2, parse_date("2026-02-28").unwrap()).unwrap();
    m.into_iter().map(|m| (m.income, m.expense)).collect()
}

fn balances(db: &Db) -> Vec<(String, Cents)> {
    db.accounts(true).unwrap().into_iter().map(|a| (a.name, a.balance)).collect()
}

/// Date, payee, memo, amount and running balance of a register.
fn register(db: &Db, account: i64) -> Vec<(i64, String, String, String, Cents, Option<Cents>)> {
    txs(db, account).into_iter().map(|t| (t.id, d2s(t.date), t.payee, t.memo, t.amount, t.balance)).collect()
}

#[test]
fn an_expense_becomes_a_transfer() {
    let (db, bank, savings) = books();
    import(&db, bank, BANK);
    assert_eq!(months(&db), [(0, 50_000), (0, 120)]);
    let t = find(&db, bank, "BONIFICO A RISPARMIO");
    let done = db.turn_into_transfer(t.id, savings, OtherSide::Auto).unwrap();
    assert!(!done.joined);
    assert_eq!(months(&db), [(0, 0), (0, 120)]);
    assert_eq!(balances(&db), [("Bank".into(), 49_880), ("Savings".into(), 50_000)]);
    let (a, b) = (db.transaction(t.id).unwrap(), db.transaction(done.other_id).unwrap());
    assert_eq!((a.kind, a.date, a.payee), (TxKind::Transfer, t.date, t.payee.clone()));
    assert_eq!((b.payee.as_str(), b.date, b.amount), ("Transfer from Bank", t.date, 50_000));
    assert_eq!(import(&db, bank, BANK), 0);
    assert!(db.turn_into_transfer(t.id, savings, OtherSide::Auto).is_err());
    // Back, without the side that was added.
    assert_eq!(db.separate_transfer(t.id, false).unwrap(), None);
    assert_eq!(months(&db), [(0, 50_000), (0, 120)]);
    assert_eq!(balances(&db), [("Bank".into(), 49_880), ("Savings".into(), 0)]);
    assert_eq!(import(&db, bank, BANK), 0);
    assert_sound(&db, "expense");
}

#[test]
fn both_sides_imported_are_joined_and_separated() {
    for from_savings in [false, true] {
        let (db, bank, savings) = books();
        import(&db, bank, BANK);
        import(&db, savings, SAVINGS);
        let (before, registers) = (balances(&db), [register(&db, bank), register(&db, savings)]);
        let (x, y) = (find(&db, bank, "BONIFICO A RISPARMIO"), find(&db, savings, "BONIFICO RICEVUTO"));
        let (from, to, account) = if from_savings { (&y, &x, bank) } else { (&x, &y, savings) };
        let found = db.transfer_candidates(from.id, account).unwrap();
        assert!(db.turn_into_transfer(from.id, account, OtherSide::Auto).is_err(), "never guessed");
        assert_eq!(found.iter().map(|c| c.id).collect::<Vec<_>>(), [to.id]);
        let done = db.turn_into_transfer(from.id, account, OtherSide::Join(to.id)).unwrap();
        assert!(done.joined && done.other_id == to.id);
        // Every register shows its own dates, texts and running balances.
        assert_eq!(balances(&db), before);
        assert_eq!([register(&db, bank), register(&db, savings)], registers);
        assert_eq!(db.transaction(x.id).unwrap().entry_id, db.transaction(y.id).unwrap().entry_id);
        assert_eq!(months(&db), [(0, 0), (300, 120)]);
        assert_eq!((import(&db, bank, BANK), import(&db, savings, SAVINGS)), (0, 0));
        // At every date the books balance.
        for day in ["2026-01-30", "2026-01-31", "2026-02-01", "2026-02-02", "2026-02-28"] {
            assert!(db.trial_balance(parse_date(day)).unwrap().is_zero(), "{day}");
        }
        let in_january = TxFilter {
            account_id: Some(savings),
            to: Some(month_range(parse_date("2026-01-01").unwrap()).1),
            ..Default::default()
        };
        assert!(db.transactions(&in_january).unwrap().is_empty());
        assert_sound(&db, "joined");
        // Editing one side keeps the date of the other.
        let edit = |t: &Transaction, date: &str| crate::db::TxEdit {
            date: parse_date(date).unwrap(),
            amount: t.amount,
            payee: t.payee.clone(),
            memo: t.memo.clone(),
            category_id: None,
            cleared: true,
        };
        db.update_transaction(x.id, &edit(&x, "2026-01-30")).unwrap();
        assert_eq!(db.transaction(x.id).unwrap().date, parse_date("2026-01-30").unwrap());
        assert_eq!(db.transaction(y.id).unwrap().date, y.date);
        db.update_transaction(x.id, &edit(&x, "2026-01-31")).unwrap();
        db.separate_transfer(to.id, true).unwrap();
        assert_eq!(balances(&db), before);
        assert_eq!([register(&db, bank), register(&db, savings)], registers);
        for t in [&x, &y] {
            assert_eq!(db.transaction(t.id).unwrap().kind, TxKind::Uncategorized);
        }
        assert_eq!((import(&db, bank, BANK), import(&db, savings, SAVINGS)), (0, 0));
        assert_sound(&db, "separated");
    }
}

#[test]
fn several_candidates_are_never_chosen_for_the_user() {
    let (db, bank, savings) = books();
    import(&db, bank, BANK);
    let t = find(&db, bank, "BONIFICO A RISPARMIO");
    let mut ids = Vec::new();
    for day in ["2026-01-21", "2026-02-10", "2026-02-11", "2026-01-20"] {
        let date = parse_date(day).unwrap();
        let tx = NewTx { account_id: savings, date, amount: 50_000, payee: day.into(), ..Default::default() };
        ids.push(db.add_transaction(&tx).unwrap().unwrap());
    }
    let found = db.transfer_candidates(t.id, savings).unwrap();
    assert_eq!(found.iter().map(|c| c.id).collect::<Vec<_>>(), ids[..2], "ten days, either way");
    let before = dump(&db.conn);
    for other in [OtherSide::Auto, OtherSide::Join(ids[2]), OtherSide::Join(t.id), OtherSide::Join(-1)] {
        assert!(db.turn_into_transfer(t.id, savings, other).is_err());
        assert_eq!(dump(&db.conn), before);
    }
    let args = json!({"id": t.id, "account": "Savings", "match": "none"});
    let done = ops::call(&db, "turn_into_transfer", &args, None).unwrap();
    assert_eq!(done["joined"], false);
    assert_eq!(txs(&db, savings).len(), 5);
    assert_sound(&db, "none of these");
}

#[test]
fn what_cannot_be_a_transfer_is_refused() {
    let (db, bank, savings) = books();
    let dollars = db.add_account("Dollars", AccountKind::Bank, "USD", 0, "").unwrap();
    import(&db, bank, BANK);
    let t = find(&db, bank, "BAR");
    let id = |k: &str| db.find_ledger_account(k).unwrap().id;
    let hand = db.create_ledger_account("Loan to a friend", Some(crate::ledger::Class::Asset), None, "", None).unwrap();
    let post = |lines: &[(i64, Cents)]| {
        let lines = lines
            .iter()
            .map(|l| crate::ledger::NewLine { ledger_account_id: l.0, amount: l.1, currency: None })
            .collect();
        let e = crate::ledger::NewEntry { date: t.date, payee: "e".into(), memo: String::new(), lines };
        let e = db.post_entry(&e).unwrap();
        txs(&db, bank).into_iter().find(|t| t.entry_id == e).unwrap().id
    };
    let split = post(&[(id("Bank"), -30), (id("system:uncategorized"), 10), (id("Health"), 20)]);
    let other = post(&[(id("Bank"), -40), (hand, 40)]);
    let half = post(&[(id("Bank"), -50), (id("system:opening_balances"), 50)]);
    let (x, _) = db.add_transfer(bank, savings, t.date, 7, "").unwrap();
    let before = dump(&db.conn);
    for (tx, account) in
        [(split, savings), (other, savings), (x, savings), (t.id, dollars), (t.id, bank), (t.id, 999), (999, savings)]
    {
        assert!(db.turn_into_transfer(tx, account, OtherSide::Auto).is_err(), "{tx} {account}");
        assert!(db.transfer_candidates(tx, account).is_err(), "{tx} {account}");
        assert_eq!(dump(&db.conn), before);
    }
    for tx in [split, other, half, t.id, 999] {
        assert!(db.separate_transfer(tx, true).is_err());
        assert_eq!(dump(&db.conn), before);
    }
    // Half a transfer of an older file becomes a whole one.
    db.turn_into_transfer(half, savings, OtherSide::Auto).unwrap();
    assert_sound(&db, "half");
}

/// A side with the date of the entry keeps it when the other side is given another.
#[test]
fn a_joined_side_keeps_its_date() {
    let (db, bank, savings) = books();
    let date = parse_date("2026-01-31").unwrap();
    let add = |account_id, amount| {
        db.add_transaction(&NewTx { account_id, date, amount, payee: "x".into(), ..Default::default() })
            .unwrap()
            .unwrap()
    };
    let (x, y) = (add(bank, -5), add(savings, 5));
    db.turn_into_transfer(x, savings, OtherSide::Join(y)).unwrap();
    let edit = crate::db::TxEdit { date: parse_date("2026-01-20").unwrap(), amount: -5, ..Default::default() };
    db.update_transaction(x, &edit).unwrap();
    assert_eq!((db.transaction(x).unwrap().date, db.transaction(y).unwrap().date), (edit.date, date));
    assert_sound(&db, "dates");
}

/// Accounts deleted afterwards leave the other side where it was.
#[test]
fn a_joined_side_outlives_the_other_account() {
    let (db, bank, savings) = books();
    import(&db, bank, BANK);
    import(&db, savings, SAVINGS);
    let x = find(&db, bank, "BONIFICO A RISPARMIO");
    let y = find(&db, savings, "BONIFICO RICEVUTO");
    db.turn_into_transfer(x.id, savings, OtherSide::Join(y.id)).unwrap();
    let before = register(&db, savings);
    db.delete_account(bank).unwrap();
    assert_eq!(register(&db, savings), before);
    assert_eq!(import(&db, savings, SAVINGS), 0);
    assert_sound(&db, "account gone");
}

/// A line imported later takes the place of the side that was added for it.
#[test]
fn an_added_side_is_confirmed_by_the_line_of_its_statement() {
    let (db, bank, savings) = books();
    import(&db, bank, BANK);
    let t = find(&db, bank, "BONIFICO A RISPARMIO");
    let done = db.turn_into_transfer(t.id, savings, OtherSide::Auto).unwrap();
    assert!(db.transaction(done.other_id).unwrap().added && !db.transaction(t.id).unwrap().added);
    let table = importer::analyze(SAVINGS, "UTF-8").unwrap();
    let (rows, _) = importer::parse_rows(&table, &table.mapping, false);
    let res = importer::import_rows(&db, savings, &rows, 0).unwrap();
    assert_eq!((res.added, res.adopted.len(), res.duplicates), (1, 1, 0));
    assert_eq!((res.adopted[0].id, res.adopted[0].payee.as_str()), (done.other_id, "BONIFICO RICEVUTO"));
    assert!(res.maybe_transfers.is_empty());
    let side = db.transaction(done.other_id).unwrap();
    assert_eq!((side.payee.as_str(), d2s(side.date), side.added), ("BONIFICO RICEVUTO", "2026-02-02".into(), false));
    assert_eq!(db.transaction(t.id).unwrap().date, t.date);
    assert_eq!(balances(&db), [("Bank".into(), 49_880), ("Savings".into(), 50_300)]);
    assert_eq!((import(&db, bank, BANK), import(&db, savings, SAVINGS)), (0, 0));
    assert!(db.adopt(&db.transaction(done.other_id).unwrap(), t.date, "x", "", "h", "m").is_err(), "once");
    assert_sound(&db, "adopted");
}

#[test]
fn both_sides_of_a_transfer_made_by_hand_are_confirmed() {
    let (db, bank, savings) = books();
    db.add_transfer(bank, savings, parse_date("2026-02-01").unwrap(), 50_000, "").unwrap();
    let before = balances(&db);
    import(&db, bank, BANK);
    import(&db, savings, SAVINGS);
    assert_eq!(balances(&db), [("Bank".into(), before[0].1 - 120), ("Savings".into(), before[1].1 + 300)]);
    let x = find(&db, bank, "BONIFICO A RISPARMIO");
    assert_eq!(x.transfer_peer, Some(find(&db, savings, "BONIFICO RICEVUTO").id));
    assert_eq!((import(&db, bank, BANK), import(&db, savings, SAVINGS)), (0, 0));
    assert_sound(&db, "by hand");
}

#[test]
fn what_could_be_several_is_left_to_the_user() {
    // Two lines for one side, one line for two sides, a line too far away.
    let line = |day: &str, payee: &str| format!("{day};{payee};500,00\n");
    let head = "Data;Descrizione;Importo\n";
    let hand = |db: &Db, bank, savings, day: &str| {
        db.add_transfer(bank, savings, parse_date(day).unwrap(), 50_000, "").unwrap()
    };
    let read = |db: &Db, account, text: &str| {
        let t = importer::analyze(text, "UTF-8").unwrap();
        let (rows, _) = importer::parse_rows(&t, &t.mapping, false);
        importer::import_rows(db, account, &rows, 0).unwrap()
    };
    let (db, bank, savings) = books();
    let (x, y) = hand(&db, bank, savings, "2026-02-01");
    let text = format!("{head}{}{}", line("02/02/2026", "A"), line("03/02/2026", "B"));
    let res = read(&db, savings, &text);
    assert_eq!((res.added, res.adopted.len(), res.maybe_transfers.len()), (2, 0, 2));
    let m = &res.maybe_transfers[0];
    assert_eq!(m.candidates.iter().map(|c| c.id).collect::<Vec<_>>(), [x]);
    // The choice: the line takes the place of the added side.
    let before = balances(&db);
    let found = db.transfer_candidates(m.transaction.id, bank).unwrap();
    assert_eq!(found.iter().map(|c| (c.id, c.kind)).collect::<Vec<_>>(), [(x, TxKind::Transfer)]);
    let done = db.turn_into_transfer(m.transaction.id, bank, OtherSide::Join(x)).unwrap();
    assert_eq!(done, Turned { id: m.transaction.id, other_id: x, joined: true });
    assert_eq!(balances(&db)[0], before[0]);
    assert_eq!(balances(&db)[1].1, before[1].1 - 50_000);
    assert!(db.transaction(y).is_err());
    let t = db.transaction(m.transaction.id).unwrap();
    assert_eq!((t.payee.as_str(), d2s(t.date), t.added, t.transfer_peer), ("A", "2026-02-02".into(), false, Some(x)));
    assert_eq!(read(&db, savings, &text).added, 0);
    // The other line has nothing left to stand for.
    assert!(db.transfer_candidates(res.maybe_transfers[1].transaction.id, bank).unwrap().is_empty());
    assert_sound(&db, "two lines");

    let (db, bank, savings) = books();
    hand(&db, bank, savings, "2026-02-01");
    hand(&db, bank, savings, "2026-02-04");
    let res = read(&db, savings, &format!("{head}{}", line("02/02/2026", "A")));
    assert_eq!((res.adopted.len(), res.maybe_transfers[0].candidates.len()), (0, 2));

    let (db, bank, savings) = books();
    hand(&db, bank, savings, "2026-02-01");
    let res = read(&db, savings, &format!("{head}{}", line("12/02/2026", "A")));
    assert_eq!((res.added, res.adopted.len(), res.maybe_transfers.len()), (1, 0, 0));
}

/// When the other account goes, the date of the side that stays is that of its entry, in the reports too.
#[test]
fn the_date_of_a_side_left_alone_is_the_entrys() {
    let (db, bank, savings) = books();
    let add = |account_id, date: &str, amount| {
        let date = parse_date(date).unwrap();
        db.add_transaction(&NewTx { account_id, date, amount, payee: "x".into(), ..Default::default() })
            .unwrap()
            .unwrap()
    };
    let (x, y) = (add(savings, "2026-01-28", 50_000), add(bank, "2026-02-03", -50_000));
    db.turn_into_transfer(x, bank, OtherSide::Join(y)).unwrap();
    db.delete_account(savings).unwrap();
    db.set_category(y, Some(db.categories().unwrap()[0].id)).unwrap();
    assert_eq!(months(&db), [(0, 0), (0, 50_000)]);
    let t = db.transaction(y).unwrap();
    assert_eq!(db.entry(t.entry_id).unwrap().postings[0].date, None);
    let edit = crate::db::TxEdit {
        date: parse_date("2026-01-10").unwrap(),
        amount: -50_000,
        category_id: t.category_id,
        ..Default::default()
    };
    db.update_transaction(y, &edit).unwrap();
    assert_eq!(months(&db), [(0, 50_000), (0, 0)]);
    assert_sound(&db, "left alone");
}

#[test]
fn editing_a_side_leaves_the_text_of_the_other() {
    let (db, bank, savings) = books();
    let date = parse_date("2026-01-31").unwrap();
    let add = |account_id, amount, memo: &str| {
        let tx = NewTx { account_id, date, amount, payee: memo.into(), memo: memo.into(), ..Default::default() };
        db.add_transaction(&tx).unwrap().unwrap()
    };
    let (x, y) = (add(bank, -5, "bank"), add(savings, 5, "savings"));
    db.turn_into_transfer(x, savings, OtherSide::Join(y)).unwrap();
    for (mine, other, kept) in [(x, y, "savings"), (y, x, "new 0")] {
        let memo = if mine == x { "new 0" } else { "new 1" };
        let edit = crate::db::TxEdit {
            date,
            amount: if mine == x { -5 } else { 5 },
            payee: memo.into(),
            memo: memo.into(),
            ..Default::default()
        };
        db.update_transaction(mine, &edit).unwrap();
        let (m, o) = (db.transaction(mine).unwrap(), db.transaction(other).unwrap());
        assert_eq!((m.memo.as_str(), m.payee.as_str(), o.memo.as_str(), o.payee.as_str()), (memo, memo, kept, kept));
    }
    // A side that was added, too.
    let z = add(bank, -9, "line");
    let done = db.turn_into_transfer(z, savings, OtherSide::New).unwrap();
    let edit = crate::db::TxEdit { date, amount: 9, payee: "p".into(), memo: "m".into(), ..Default::default() };
    db.update_transaction(done.other_id, &edit).unwrap();
    assert_eq!(db.transaction(z).unwrap().memo, "line");
    assert_sound(&db, "texts");
}

#[test]
fn a_parameter_that_is_not_a_boolean_is_refused() {
    let (db, bank, savings) = books();
    let (x, _) = db.add_transfer(bank, savings, parse_date("2026-01-31").unwrap(), 7, "").unwrap();
    let before = dump(&db.conn);
    for v in [json!("y"), json!("true"), json!(1), json!([]), json!({})] {
        assert!(ops::call(&db, "separate_transfer", &json!({"id": x, "keep_other": v}), None).is_err(), "{v}");
        assert!(ops::call(&db, "update_transaction", &json!({"id": x, "cleared": v}), None).is_err(), "{v}");
        assert!(ops::call(&db, "list_accounts", &json!({"include_archived": v}), None).is_err(), "{v}");
        assert_eq!(dump(&db.conn), before);
    }
    let done = ops::call(&db, "separate_transfer", &json!({"id": x, "keep_other": null}), None).unwrap();
    assert!(done["other_id"].is_i64());
}
