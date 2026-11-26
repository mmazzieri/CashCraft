use super::*;
use crate::db::{AccountKind, TxEdit};
use crate::importer;
use crate::ledger::tests::{assert_sound, dump};
use crate::ledger::{NewEntry, NewLine};
use crate::money::parse_date;
use crate::ops;
use crate::testing::Seeded;
use serde_json::{Value, json};

const STATEMENT: &str =
    "Data;Descrizione;Importo\n02/01/2026;BAR;-1,20\n10/01/2026;STIPENDIO;1500,00\n20/01/2026;SUPERMERCATO;-80,30\n";

fn day(s: &str) -> NaiveDate {
    parse_date(s).unwrap()
}

fn rows(text: &str) -> Vec<ParsedRow> {
    let t = importer::analyze(text, "UTF-8").unwrap();
    importer::parse_rows(&t, &t.mapping, false).0
}

fn import(db: &Db, account: i64, text: &str) -> usize {
    importer::import_rows(db, account, &rows(text), 0).unwrap().added
}

fn bank() -> (Db, i64) {
    let db = Db::open_in_memory().unwrap();
    let a = db.add_account("Bank", AccountKind::Bank, "EUR", 100_000, "").unwrap();
    (db, a)
}

fn typed(db: &Db, account: i64, date: &str, amount: Cents, payee: &str, cleared: bool) -> i64 {
    let tx = NewTx { account_id: account, date: day(date), amount, payee: payee.into(), cleared, ..Default::default() };
    db.add_transaction(&tx).unwrap().unwrap()
}

fn start(db: &Db, account: i64, date: &str, balance: Cents, lines: &[ParsedRow]) -> i64 {
    let s = Start { account, date: day(date), balance, source: Source::Csv, rows: lines };
    db.start_reconciliation(&s).unwrap().id
}

fn exact_pairs(p: &Preview) -> Vec<(i64, i64)> {
    p.lines.iter().filter_map(|l| l.exact.as_ref().map(|t| (l.line, t.id))).collect()
}

fn edit(db: &Db, id: i64, f: impl FnOnce(&mut TxEdit)) -> Result<()> {
    let t = db.transaction(id)?;
    let mut e = TxEdit {
        date: t.date,
        amount: t.amount,
        payee: t.payee,
        memo: t.memo,
        category_id: t.category_id,
        cleared: t.cleared,
    };
    f(&mut e);
    db.update_transaction(id, &e)
}

fn status(db: &Db, id: i64) -> Status {
    db.reconciliation(id).unwrap().status
}

/// The statement imported, then reconciled from the same file: every line is exact, and the difference is 0.00.
fn reconciled_bank() -> (Db, i64, i64) {
    let (db, a) = bank();
    assert_eq!(import(&db, a, STATEMENT), 3);
    let lines = rows(STATEMENT);
    let r = start(&db, a, "2026-01-31", 100_000 - 120 + 150_000 - 8030, &lines);
    let p = db.preview_reconciliation(r).unwrap();
    assert_eq!((p.exact_pending, p.unmatched_lines), (3, 3));
    assert_eq!(db.match_lines(r, &exact_pairs(&p)).unwrap(), 3);
    let p = db.preview_reconciliation(r).unwrap();
    assert_eq!((p.reconciliation.difference, p.unmatched_lines, p.books_only.len()), (0, 0, 0));
    let done = db.finish_reconciliation(r).unwrap();
    assert_eq!((done.status, done.reconciled, done.difference), (Status::Finished, 3, 0));
    (db, a, r)
}

#[test]
fn a_statement_fully_matched_closes_with_difference_zero() {
    let (db, a, r) = reconciled_bank();
    let all = db.transactions(&TxFilter { account_id: Some(a), ..Default::default() }).unwrap();
    assert!(all.iter().all(|t| t.reconciled && t.cleared));
    assert_eq!(db.cleared_balance(a, day("2026-01-31")).unwrap(), db.reconciliation(r).unwrap().statement_balance);
    let v = ops::call(&db, "list_reconciliations", &json!({}), None).unwrap();
    assert_eq!(v["reconciliations"][0]["difference"], "0.00");
    assert_eq!(v["reconciliations"][0]["status"], "finished");
    assert_sound(&db, "reconciled");
}

#[test]
fn the_difference_is_shown_until_zero_and_finishing_is_refused_before() {
    let (db, a) = bank();
    let bar = typed(&db, a, "2026-01-02", -120, "Bar", false);
    let rent = typed(&db, a, "2026-01-05", -50_000, "Rent", false);
    let later = typed(&db, a, "2026-02-05", -900, "Later", true);
    let r = start(&db, a, "2026-01-31", 100_000 - 120 - 50_000, &[]);
    let p = db.preview_reconciliation(r).unwrap();
    assert_eq!(p.reconciliation.difference, -50_120);
    // What comes after the statement is not there.
    assert_eq!(p.books_only.iter().map(|t| t.id).collect::<Vec<_>>(), [bar, rent]);
    assert!(db.finish_reconciliation(r).is_err());
    assert!(db.clear_for_reconciliation(r, &[later], true).is_err());
    db.clear_for_reconciliation(r, &[bar], true).unwrap();
    assert_eq!(db.reconciliation(r).unwrap().difference, -50_000);
    db.clear_for_reconciliation(r, &[rent], true).unwrap();
    assert_eq!(db.reconciliation(r).unwrap().difference, 0);
    let done = db.finish_reconciliation(r).unwrap();
    assert_eq!(done.reconciled, 2);
    assert!(!db.transaction(later).unwrap().reconciled);
    assert!(db.transaction(rent).unwrap().reconciled);
    assert_sound(&db, "typed");
}

#[test]
fn editing_a_reconciled_transaction_asks_for_confirmation_and_unreconciles_it() {
    let (db, a, r) = reconciled_bank();
    let bar = db.transactions(&TxFilter { account_id: Some(a), ..Default::default() }).unwrap();
    let bar = bar.iter().find(|t| t.payee == "BAR").unwrap().id;
    let before = dump(&db.conn);
    for change in [
        edit(&db, bar, |e| e.amount = -130),
        edit(&db, bar, |e| e.date = day("2026-01-03")),
        edit(&db, bar, |e| e.cleared = false),
        db.delete_transaction(bar),
        db.set_cleared(bar, false),
    ] {
        let e = change.unwrap_err();
        let locked = reconciled_in(&e).unwrap();
        assert_eq!((locked.transactions.clone(), locked.reconciliations.clone()), (vec![bar], vec![r]));
        assert_eq!(dump(&db.conn), before);
    }
    // The category, the tags, the notes and the payee are not checked against the bank.
    let food = db.category_by_path("Food", false, true).unwrap();
    db.set_category(bar, Some(food)).unwrap();
    db.set_tags(bar, &["coffee".into()]).unwrap();
    edit(&db, bar, |e| {
        e.memo = "with Anna".into();
        e.payee = "Bar Roma".into();
    })
    .unwrap();
    assert!(db.transaction(bar).unwrap().reconciled);
    assert_eq!(status(&db, r), Status::Finished);
    // Confirmed: the change is made and the reconciliation no longer counts it.
    db.unreconciling(|| edit(&db, bar, |e| e.amount = -130)).unwrap();
    let t = db.transaction(bar).unwrap();
    assert!(!t.reconciled && t.amount == -130);
    let broken = db.reconciliation(r).unwrap();
    assert_eq!((broken.status, broken.changed.as_slice(), broken.difference), (Status::Broken, [bar].as_slice(), 10));
    // Back as it was, it stays out until it is reconciled again.
    edit(&db, bar, |e| e.amount = -120).unwrap();
    assert!(!db.transaction(bar).unwrap().reconciled);
    assert_eq!(status(&db, r), Status::Broken);
    // Done again on the same day.
    let again = start(&db, a, "2026-01-31", broken.statement_balance, &[]);
    assert_eq!(db.preview_reconciliation(again).unwrap().books_only.iter().map(|t| t.id).collect::<Vec<_>>(), [bar]);
    assert_eq!(db.finish_reconciliation(again).unwrap().reconciled, 1);
    assert_eq!(status(&db, r), Status::Redone);
    assert!(db.transaction(bar).unwrap().reconciled);
    // Deleting asks too.
    let e = db.delete_transaction(bar).unwrap_err();
    assert_eq!(reconciled_in(&e).unwrap().reconciliations, [again]);
    db.unreconciling(|| db.delete_transaction(bar)).unwrap();
    assert_eq!(status(&db, again), Status::Broken);
    assert_sound(&db, "edited");
}

#[test]
fn the_operations_ask_before_changing_reconciled_transactions() {
    let (db, a, r) = reconciled_bank();
    let id = db.transactions(&TxFilter { account_id: Some(a), ..Default::default() }).unwrap()[0].id;
    let before = dump(&db.conn);
    let e = ops::call(&db, "update_transaction", &json!({"id": id, "amount": -5}), None).unwrap_err();
    assert!(format!("{e:#}").contains("unreconcile: true"), "{e:#}");
    assert!(ops::call(&db, "delete_transaction", &json!({"id": id}), None).is_err());
    assert_eq!(dump(&db.conn), before);
    ops::call(&db, "update_transaction", &json!({"id": id, "memo": "note"}), None).unwrap();
    ops::call(&db, "update_transaction", &json!({"id": id, "amount": -5, "unreconcile": true}), None).unwrap();
    assert_eq!(status(&db, r), Status::Broken);
    assert!(ops::call(&db, "undo_reconciliation", &json!({"id": r}), None).is_ok());
    assert_eq!(status(&db, r), Status::Undone);
    assert_sound(&db, "operations");
}

#[test]
fn lines_are_matched_exactly_first_then_by_amount_and_date_and_never_guessed() {
    let (db, a) = bank();
    // Typed by hand: no key. The bar twice, two days before the bank; the salary on its day.
    let bar1 = typed(&db, a, "2025-12-31", -120, "Bar", false);
    let bar2 = typed(&db, a, "2026-01-01", -120, "Bar", false);
    let salary = typed(&db, a, "2026-01-10", 150_000, "STIPENDIO", false);
    let lines = rows(STATEMENT);
    let r = start(&db, a, "2026-01-31", 100_000 - 120 + 150_000 - 8030, &lines);
    let p = db.preview_reconciliation(r).unwrap();
    assert_eq!(p.exact_pending, 0);
    let [bar, pay, shop] = p.lines.as_slice() else { panic!("{:?}", p.lines) };
    // The nearest is proposed, the other shown; neither is sure.
    assert_eq!(bar.proposed.as_ref().map(|t| t.id), Some(bar2));
    assert!(bar.candidates.iter().any(|t| t.id == bar1) && !bar.certain);
    assert_eq!(pay.proposed.as_ref().map(|t| t.id), Some(salary));
    assert!(pay.certain);
    assert!(shop.proposed.is_none() && shop.candidates.is_empty());
    // Nothing is matched by itself, and a wrong amount is refused.
    assert!(p.lines.iter().all(|l| l.matched.is_none()));
    assert!(db.match_lines(r, &[(1, salary)]).is_err());
    assert!(db.match_lines(r, &[(1, bar1), (2, bar1)]).is_err());
    assert!(db.match_lines(r, &[(9, bar1)]).is_err());
    db.match_lines(r, &[(1, bar1), (2, salary)]).unwrap();
    assert!(db.match_lines(r, &[(1, bar2)]).is_err());
    // The line missing in the books is added; the other bar stays, not cleared, on the side of the books.
    let added = db.add_lines(r, &[3]).unwrap();
    let p = db.preview_reconciliation(r).unwrap();
    assert_eq!(p.books_only.iter().map(|t| t.id).collect::<Vec<_>>(), [bar2]);
    assert!(!p.books_only.iter().any(|t| t.cleared));
    assert_eq!((p.unmatched_lines, p.reconciliation.difference), (0, 0));
    assert!(p.lines.last().unwrap().added);
    db.finish_reconciliation(r).unwrap();
    assert!(db.transaction(added[0]).unwrap().reconciled && !db.transaction(bar2).unwrap().reconciled);
    // The statement imported after that adds nothing and asks nothing.
    let again = importer::import_rows(&db, a, &lines, 0).unwrap();
    assert_eq!((again.added, again.duplicates), (0, 3));
    assert_eq!(status(&db, r), Status::Finished);
    assert_sound(&db, "matched");
}

#[test]
fn undoing_a_match_or_an_open_reconciliation_puts_back_what_it_changed() {
    let (db, a) = bank();
    let bar = typed(&db, a, "2026-01-01", -120, "Bar", false);
    let lines = rows(STATEMENT);
    let before = dump(&db.conn);
    let r = start(&db, a, "2026-01-31", 0, &lines);
    db.match_lines(r, &[(1, bar)]).unwrap();
    let t = db.transaction(bar).unwrap();
    assert!(t.cleared && !t.keys.is_empty());
    db.unmatch_lines(r, &[1]).unwrap();
    let t = db.transaction(bar).unwrap();
    assert!(!t.cleared && t.keys.is_empty());
    assert!(db.unmatch_lines(r, &[1]).is_err());
    db.match_lines(r, &[(1, bar)]).unwrap();
    assert!(
        db.start_reconciliation(&Start {
            account: a,
            date: day("2026-02-01"),
            balance: 0,
            source: Source::Typed,
            rows: &[]
        })
        .is_err()
    );
    db.undo_reconciliation(r).unwrap();
    assert_eq!(dump(&db.conn), before);
    assert!(db.reconciliation(r).is_err());
}

#[test]
fn only_the_last_finished_reconciliation_of_an_account_is_undone() {
    let (db, a) = bank();
    typed(&db, a, "2026-01-05", -100, "One", true);
    let first = start(&db, a, "2026-01-31", 99_900, &[]);
    db.finish_reconciliation(first).unwrap();
    typed(&db, a, "2026-02-05", -100, "Two", true);
    // Not before the last one.
    let early = Start { account: a, date: day("2026-01-30"), balance: 0, source: Source::Typed, rows: &[] };
    assert!(db.start_reconciliation(&early).is_err());
    let second = start(&db, a, "2026-02-28", 99_800, &[]);
    assert_eq!(db.finish_reconciliation(second).unwrap().reconciled, 1);
    assert!(db.undo_reconciliation(first).is_err());
    db.undo_reconciliation(second).unwrap();
    assert!(db.undo_reconciliation(second).is_err());
    let two = db.transactions(&TxFilter { account_id: Some(a), ..Default::default() }).unwrap();
    assert_eq!(
        two.iter().map(|t| (t.payee.as_str(), t.reconciled, t.cleared)).collect::<Vec<_>>(),
        [("Two", false, true), ("One", true, true)]
    );
    db.undo_reconciliation(first).unwrap();
    let list = db.reconciliations(Some(a)).unwrap();
    assert_eq!(list.iter().map(|r| r.status).collect::<Vec<_>>(), [Status::Undone, Status::Undone]);
}

#[test]
fn a_transfer_is_reconciled_on_each_account_by_itself() {
    let (db, a) = bank();
    let s = db.add_account("Savings", AccountKind::Savings, "EUR", 0, "").unwrap();
    let (out, into) = db.add_transfer(a, s, day("2026-01-10"), 20_000, "").unwrap();
    db.set_cleared(out, true).unwrap();
    db.set_cleared(into, true).unwrap();
    let r = start(&db, a, "2026-01-31", 80_000, &[]);
    db.finish_reconciliation(r).unwrap();
    assert!(db.transaction(out).unwrap().reconciled && !db.transaction(into).unwrap().reconciled);
    // Made by hand, the two sides have the date of the entry: changing that of the savings changes both. The
    // amount is of both too; the notes are not checked.
    for change in [edit(&db, into, |e| e.date = day("2026-01-11")), edit(&db, into, |e| e.amount = 30_000)] {
        assert_eq!(reconciled_in(&change.unwrap_err()).unwrap().transactions, [out]);
    }
    edit(&db, into, |e| e.memo = "savings".into()).unwrap();
    assert!(db.transaction(out).unwrap().reconciled);
    // Separating it would give the side of the bank another entry, with the same date and amount: it stays.
    db.separate_transfer(out, true).unwrap();
    assert!(db.transaction(out).unwrap().reconciled);
    // The savings reconciled too: a change of the amount touches both.
    let (x, y) = db.add_transfer(a, s, day("2026-02-10"), 1_000, "").unwrap();
    db.set_cleared(x, true).unwrap();
    db.set_cleared(y, true).unwrap();
    let ra = start(&db, a, "2026-02-28", 79_000, &[]);
    db.finish_reconciliation(ra).unwrap();
    let rs = start(&db, s, "2026-02-28", 21_000, &[]);
    db.finish_reconciliation(rs).unwrap();
    let e = edit(&db, x, |e| e.amount = -2_000).unwrap_err();
    let locked = reconciled_in(&e).unwrap();
    assert_eq!((locked.transactions.len(), locked.reconciliations.as_slice()), (2, [ra, rs].as_slice()));
    assert_sound(&db, "transfers");
}

#[test]
fn a_split_and_a_card_are_reconciled_with_their_signs() {
    let db = Db::open_in_memory().unwrap();
    let card = db.add_account("Card", AccountKind::CreditCard, "EUR", 0, "").unwrap();
    let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 100_000, "").unwrap();
    let (card_ledger, _) = db.money_ledger(card).unwrap();
    let food = db.category_ledger(Some(db.category_by_path("Food", false, true).unwrap())).unwrap();
    let home = db.category_ledger(Some(db.category_by_path("Home", false, true).unwrap())).unwrap();
    let line = |id, amount| NewLine { ledger_account_id: id, amount, currency: None };
    let split = db
        .post_entry(&NewEntry {
            date: day("2026-01-03"),
            payee: "Shop".into(),
            memo: String::new(),
            lines: vec![line(card_ledger, -6_010), line(food, 4_000), line(home, 2_010)],
        })
        .unwrap();
    let posting = db.entry(split).unwrap().postings.iter().find(|p| p.account_id == card_ledger).unwrap().id;
    assert_eq!(db.transaction(posting).unwrap().kind, crate::db::TxKind::Split);
    db.set_cleared(posting, true).unwrap();
    // The first statement is paid in February: the debt at its end is all of it.
    let t = importer::card_table(crate::importer::Statement {
        closing: Some(day("2026-01-31")),
        debit: Some(6_010),
        residual: Some(0),
        ..Default::default()
    });
    let stated = stated_balance(&t, false, &[]).unwrap();
    assert_eq!((stated.balance, stated.date), (-6_010, Some(day("2026-01-31"))));
    let r = start(&db, card, "2026-01-31", stated.balance, &[]);
    assert_eq!(db.finish_reconciliation(r).unwrap().reconciled, 1);
    let (pay, side) = db.add_transfer(bank, card, day("2026-02-15"), 6_010, "").unwrap();
    db.set_cleared(side, true).unwrap();
    let second = typed(&db, card, "2026-02-20", -3_000, "Fuel", true);
    let t = importer::card_table(crate::importer::Statement {
        closing: Some(day("2026-02-28")),
        debit: Some(3_000),
        residual: Some(0),
        ..Default::default()
    });
    let r2 = start(&db, card, "2026-02-28", stated_balance(&t, false, &[]).unwrap().balance, &[]);
    assert_eq!(db.reconciliation(r2).unwrap().difference, 0);
    db.finish_reconciliation(r2).unwrap();
    assert!(db.transaction(second).unwrap().reconciled && !db.transaction(pay).unwrap().reconciled);
    // The categories of the split are not checked against the bank.
    let other = db.entry(split).unwrap().postings.iter().find(|p| p.account_id == food).unwrap().id;
    db.set_posting_category(other, None).unwrap();
    assert!(db.transaction(posting).unwrap().reconciled);
    assert_sound(&db, "card");
}

#[test]
fn each_account_reconciles_in_its_currency() {
    let db = Db::open_in_memory().unwrap();
    let usd = db.add_account("Dollars", AccountKind::Bank, "USD", 1_000, "").unwrap();
    typed(&db, usd, "2026-01-05", 2_500, "Refund", true);
    let r = start(&db, usd, "2026-01-31", 3_500, &[]);
    let done = db.finish_reconciliation(r).unwrap();
    assert_eq!((done.currency.as_str(), done.difference), ("USD", 0));
    // An OFX in euros is refused for it.
    let dir = std::env::temp_dir().join(format!("cashcraft-reconcile-ofx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("eur.ofx");
    std::fs::write(&path, crate::ofx::tests::sgml()).unwrap();
    let e =
        ops::call(&db, "start_reconciliation", &json!({"account": "Dollars", "path": path.to_str().unwrap()}), None);
    assert!(e.is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_ofx_statement_gives_its_balance_and_lines() {
    let (db, a) = bank();
    let dir = std::env::temp_dir().join(format!("cashcraft-reconcile-sgml-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("in.ofx");
    std::fs::write(&path, crate::ofx::tests::sgml()).unwrap();
    let file = path.to_str().unwrap();
    ops::call(&db, "import_csv", &json!({"account": "Bank", "path": file}), None).unwrap();
    let t = importer::load(&path).unwrap();
    let (stated, on) = t.ofx.as_ref().and_then(|o| o.statement()).and_then(|s| s.balance).unwrap();
    let v = ops::call_in_cents(&db, "start_reconciliation", &json!({"account": "Bank", "path": file})).unwrap();
    assert_eq!(v["stated"]["balance"], json!(crate::money::fmt_machine(stated)));
    assert_eq!(v["stated"]["label"], "LEDGERBAL");
    let id = v["reconciliation"]["id"].as_i64().unwrap();
    let r = db.reconciliation(id).unwrap();
    assert_eq!((r.statement_balance, Some(r.statement_date)), (stated, on));
    assert_eq!(r.source, Source::Ofx);
    let lines = v["lines"].as_array().unwrap();
    assert!(!lines.is_empty() && lines.iter().all(|l| l["exact"].is_object()), "{v:#}");
    std::fs::remove_dir_all(&dir).unwrap();
    let _ = a;
}

#[test]
fn bper_states_its_balance_in_the_heading() {
    let t = importer::tests::load_bytes(&importer::tests::bper()).unwrap();
    let stated = stated_balance(&t, false, &[]).unwrap();
    assert_eq!((stated.balance, stated.label.as_str()), (100_000, "Saldo disponibile"));
    assert_eq!(stated.date, Some(day("2025-12-01")));
    let csv = importer::analyze(
        "Data;Descrizione;Importo\n02/01/2026;SALDO ZUCCHERO;-1,20\nSaldo contabile al 31/01/2026;;1.234,56\n",
        "UTF-8",
    )
    .unwrap();
    let r = importer::parse_rows(&csv, &csv.mapping, false).0;
    let stated = stated_balance(&csv, true, &r).unwrap();
    assert_eq!((stated.balance, stated.date), (-123_456, Some(day("2026-01-31"))));
    assert!(stated_balance(&importer::analyze(STATEMENT, "UTF-8").unwrap(), false, &[]).is_none());
}

/// What a program of format 8 that has the file open writes is not checked: the reconciliation shows it.
#[test]
fn a_change_made_without_asking_shows_as_broken() {
    let (db, a, r) = reconciled_bank();
    let id = db.transactions(&TxFilter { account_id: Some(a), ..Default::default() }).unwrap()[0].id;
    db.conn.execute("UPDATE journal_postings SET amount=amount-1 WHERE id=?1", [id]).unwrap();
    db.conn.execute("UPDATE journal_postings SET amount=amount+1 WHERE entry_id=(SELECT entry_id FROM journal_postings WHERE id=?1) AND id<>?1", [id]).unwrap();
    assert!(!db.transaction(id).unwrap().reconciled);
    assert_eq!(status(&db, r), Status::Broken);
    // What it changed is not asked again, and the rest goes on.
    edit(&db, id, |e| e.memo = "seen".into()).unwrap();
    edit(&db, id, |e| e.amount -= 1).unwrap();
    typed(&db, a, "2026-02-01", -1, "After", true);
    assert_sound(&db, "outside");
}

/// A finished reconciliation agrees with the books, and is broken once they no longer do, whatever happens.
#[test]
fn a_finished_reconciliation_has_the_balance_of_the_statement() {
    for seed in 1..40_u64 {
        let mut rng = Seeded(seed * 7919);
        let (db, a) = bank();
        let s = db.add_account("Other", AccountKind::Bank, "EUR", 0, "").unwrap();
        let mut ids = Vec::new();
        for _ in 0..rng.next(25) + 1 {
            let date = format!("2026-01-{:02}", rng.next(31) + 1);
            let amount = rng.next(20_000) as Cents - 10_000;
            let id = match rng.next(4) {
                0 => db.add_transfer(a, s, day(&date), amount.abs() + 1, "").unwrap().0,
                _ => typed(&db, a, &date, amount, "x", rng.next(2) == 0),
            };
            if rng.next(3) == 0 {
                db.set_cleared(id, true).unwrap();
            }
            ids.push(id);
        }
        let date = format!("2026-01-{:02}", rng.next(31) + 1);
        let balance = db.cleared_balance(a, day(&date)).unwrap();
        let r = start(&db, a, &date, balance + 1, &[]);
        assert!(db.finish_reconciliation(r).is_err(), "{seed}");
        db.undo_reconciliation(r).unwrap();
        let r = start(&db, a, &date, balance, &[]);
        let done = db.finish_reconciliation(r).unwrap();
        assert_eq!((done.cleared_balance, done.difference, done.status), (balance, 0, Status::Finished), "{seed}");
        for _ in 0..6 {
            let id = ids[rng.next(ids.len())];
            let Ok(t) = db.transaction(id) else { continue };
            let attempt = match rng.next(5) {
                0 => edit(&db, id, |e| e.amount += 1),
                1 => edit(&db, id, |e| e.date = day("2026-02-01")),
                2 => db.set_cleared(id, !t.cleared),
                3 => db.delete_transaction(id),
                _ => Ok(()),
            };
            match attempt {
                Err(e) => {
                    assert!(t.reconciled, "{seed}: {e:#}");
                    assert!(reconciled_in(&e).is_some(), "{seed}");
                }
                Ok(()) => {
                    let r = db.reconciliation(r).unwrap();
                    assert_eq!(r.status == Status::Finished, r.difference == 0 && r.changed.is_empty(), "{seed}");
                }
            }
            if rng.next(3) == 0 {
                // Asked and confirmed.
                let _ = db.unreconciling(|| edit(&db, id, |e| e.amount -= 2));
            }
            // A cleared transaction of before, typed now: the books no longer agree.
            if rng.next(4) == 0 {
                typed(&db, a, "2026-01-01", 5, "late", true);
            }
            let now = db.reconciliation(r).unwrap();
            let agrees = db.cleared_balance(a, day(&date)).unwrap() == balance;
            assert_eq!(now.status == Status::Finished, agrees && now.changed.is_empty(), "{seed}");
            assert_eq!(now.difference == 0, agrees, "{seed}");
        }
        assert_sound(&db, "random");
    }
}

#[test]
fn hostile_parameters_change_nothing() {
    let (db, a) = bank();
    let other = db.add_account("Other", AccountKind::Bank, "EUR", 0, "").unwrap();
    let theirs = typed(&db, other, "2026-01-02", -120, "BAR", false);
    let v =
        ops::call(&db, "start_reconciliation", &json!({"account": "Bank", "balance": 999, "date": "2026-01-31"}), None)
            .unwrap();
    let id = v["reconciliation"]["id"].as_i64().unwrap();
    let before = dump(&db.conn);
    let values = [
        json!(""),
        json!("x"),
        json!("1="),
        json!("=1"),
        json!("1=2=3"),
        json!("-1=-1"),
        json!(format!("1={theirs}")),
        json!("99999999999999999999=1"),
        json!(i64::MAX),
        json!(-1),
        json!([]),
        json!(["1=x"]),
        json!("1,".repeat(30_000)),
    ];
    for tool in [
        "match_reconciliation",
        "unmatch_reconciliation",
        "add_reconciliation_lines",
        "clear_reconciliation_transactions",
        "finish_reconciliation",
        "preview_reconciliation",
    ] {
        for v in &values {
            for args in [
                json!({"id": id, "pairs": v, "lines": v, "transactions": v}),
                json!({"id": v, "pairs": "1=1", "lines": "1", "transactions": "1"}),
                json!({"id": i64::MAX, "pairs": "1=1", "lines": "1", "transactions": format!("{theirs}")}),
                json!({"id": id, "transactions": format!("{theirs}")}),
            ] {
                let _ = ops::call(&db, tool, &args, None);
                assert_eq!(dump(&db.conn), before, "{tool} {args}");
            }
        }
    }
    for args in [
        json!({"account": "Bank"}),
        json!({"account": "Bank", "balance": "1e999", "date": "2026-01-31"}),
        json!({"account": "Bank", "balance": i64::MAX, "date": "2026-01-31"}),
        json!({"account": "Other", "balance": 1, "date": "x"}),
        json!({"account": "Other", "balance": 1, "path": "/nothing/here.csv"}),
        json!({"account": "Bank", "balance": 1, "date": "2026-01-31"}),
    ] {
        assert!(ops::call(&db, "start_reconciliation", &args, None).is_err(), "{args}");
        assert_eq!(dump(&db.conn), before, "{args}");
    }
    let v: Value = ops::call(&db, "list_reconciliations", &json!({"account": "Bank"}), None).unwrap();
    assert_eq!(v["reconciliations"].as_array().unwrap().len(), 1);
    let _ = a;
}
