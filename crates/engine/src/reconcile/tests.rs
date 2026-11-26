use super::*;
use crate::db::{AccountKind, TxEdit};
use crate::importer;
use crate::ledger::tests::{assert_sound, dump};
use crate::ledger::{NewEntry, NewLine};
use crate::money::parse_date;
use crate::ops;
use crate::testing::Seeded;
use serde_json::{Value, json};
use std::collections::HashSet;

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
    let done = db.reconciliation(r).unwrap();
    assert_eq!(done.books_balance, done.statement_balance);
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
    assert_eq!(
        p.books_only.iter().map(|b| (b.transaction.id, b.ticked)).collect::<Vec<_>>(),
        [(bar, false), (rent, false)]
    );
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
    // It is out of the set: the books lack it.
    assert_eq!((broken.status, broken.changed.as_slice(), broken.difference), (Status::Broken, [bar].as_slice(), -120));
    // Back as it was, it stays out until it is reconciled again.
    edit(&db, bar, |e| e.amount = -120).unwrap();
    assert!(!db.transaction(bar).unwrap().reconciled);
    assert_eq!(status(&db, r), Status::Broken);
    // Done again on the same day.
    let again = start(&db, a, "2026-01-31", broken.statement_balance, &[]);
    let ticked = db.preview_reconciliation(again).unwrap().books_only;
    assert_eq!(ticked.iter().map(|b| (b.transaction.id, b.ticked)).collect::<Vec<_>>(), [(bar, true)]);
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
    assert_eq!(p.books_only.iter().map(|b| b.transaction.id).collect::<Vec<_>>(), [bar2]);
    assert!(!p.books_only.iter().any(|b| b.transaction.cleared || b.ticked));
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
    let stated = stated_balance(&t, &importer::Settings::detected(&t)).unwrap();
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
    let r2 =
        start(&db, card, "2026-02-28", stated_balance(&t, &importer::Settings::detected(&t)).unwrap().balance, &[]);
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

/// The available balance of BPER is a proposal with a notice: it may count what is not booked yet. A booked one
/// is preferred; the day is that the file says the balance is at, else the last movement, never the first date.
#[test]
fn the_balance_of_a_spreadsheet_is_a_proposal() {
    let t = importer::tests::load_bytes(&importer::tests::bper()).unwrap();
    let stated = stated_balance(&t, &importer::Settings::detected(&t)).unwrap();
    assert_eq!(stated.label, "Saldo disponibile");
    assert!(stated.notice.is_some());
    // "Dati aggiornati al 01 dicembre 2025": not "Lunedì,01 dicembre 2025", which only looks like it.
    assert_eq!(stated.date, Some(day("2025-12-01")));
    let csv = |text: &str| importer::analyze(text, "UTF-8").unwrap();
    let both = csv(
        "Saldo disponibile;;9.999,00\nData;Descrizione;Importo\n02/01/2026;SALDO ZUCCHERO;-1,20\nSaldo contabile al 31/01/2026;;1.234,56\n",
    );
    let mut settings = importer::Settings::detected(&both);
    settings.invert = true;
    let stated = stated_balance(&both, &settings).unwrap();
    assert_eq!((stated.balance, stated.date, stated.notice), (-123_456, Some(day("2026-01-31")), None));
    // Without a day of its own: the last movement, not the first date of the file.
    let later = csv("Data;Descrizione;Importo\n02/01/2026;BAR;-1,20\n20/01/2026;SHOP;-2,00\nSaldo finale;;500,00\n");
    let stated = stated_balance(&later, &importer::Settings::detected(&later)).unwrap();
    assert_eq!((stated.balance, stated.date), (50_000, Some(day("2026-01-20"))));
    let none = csv(STATEMENT);
    assert!(stated_balance(&none, &importer::Settings::detected(&none)).is_none());
}

/// A movement whose text begins with "Saldo" is a movement, wherever it is, also after a row that was skipped.
#[test]
fn a_movement_is_never_read_as_the_balance() {
    let csv = |text: &str| importer::analyze(text, "UTF-8").unwrap();
    let t = csv("Data;Descrizione;Importo\n05/10/2025;BAR;-50,00\n;storno;\n20/10/2025;Saldo carta;-10,00\n");
    assert!(stated_balance(&t, &importer::Settings::detected(&t)).is_none());
    let t = csv(
        "Data;Descrizione;Importo\n05/10/2025;BAR;-50,00\n;storno;\n20/10/2025;Saldo carta;-10,00\nSaldo finale;;940,00\n",
    );
    assert_eq!(stated_balance(&t, &importer::Settings::detected(&t)).map(|s| s.balance), Some(94_000));
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

/// A finished reconciliation has the balance of its statement: the opening balance and the amounts of the
/// transactions it took, counted here by the test itself. Whatever happens next, it is finished exactly while
/// none of those was changed with the user's yes; transactions dated before and added later are not of it.
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
                _ => typed(&db, a, &date, amount, "x", false),
            };
            ids.push((id, day(&date)));
        }
        let date = format!("2026-01-{:02}", rng.next(31) + 1);
        let end = day(&date);
        // The transactions the user ticks, and the balance the statement has for them.
        let mut taken: Vec<(i64, Cents)> = Vec::new();
        for (id, d) in &ids {
            if *d <= end && rng.next(2) == 0 {
                taken.push((*id, db.transaction(*id).unwrap().amount));
            }
        }
        let balance = 100_000 + taken.iter().map(|(_, c)| c).sum::<Cents>();
        let all: Vec<i64> = taken.iter().map(|(id, _)| *id).collect();
        let r = start(&db, a, &date, balance + 1, &[]);
        if !all.is_empty() {
            db.clear_for_reconciliation(r, &all, true).unwrap();
        }
        assert!(db.finish_reconciliation(r).is_err(), "{seed}");
        db.undo_reconciliation(r).unwrap();
        let r = start(&db, a, &date, balance, &[]);
        if !all.is_empty() {
            db.clear_for_reconciliation(r, &all, true).unwrap();
        }
        let done = db.finish_reconciliation(r).unwrap();
        let expected = (balance, 0, Status::Finished, all.len());
        assert_eq!((done.books_balance, done.difference, done.status, done.reconciled), expected, "{seed}");
        let mut released: HashSet<i64> = HashSet::new();
        for _ in 0..6 {
            let (id, _) = ids[rng.next(ids.len())];
            let Ok(t) = db.transaction(id) else { continue };
            let attempt = match rng.next(5) {
                0 => edit(&db, id, |e| e.amount += 1),
                1 => edit(&db, id, |e| e.date = day("2026-02-01")),
                2 => db.set_cleared(id, !t.cleared),
                3 => db.delete_transaction(id),
                _ => Ok(()),
            };
            if let Err(e) = attempt {
                assert!(all.contains(&id) && !released.contains(&id), "{seed}: {e:#}");
                assert!(reconciled_in(&e).is_some(), "{seed}");
                if rng.next(2) == 0 {
                    // Asked and confirmed.
                    db.unreconciling(|| edit(&db, id, |e| e.amount -= 2)).unwrap();
                    released.insert(id);
                }
            }
            // Transactions dated before, typed, cleared or imported later, are not of it.
            if rng.next(3) == 0 {
                typed(&db, a, "2026-01-01", 5, "late", true);
            }
            let now = db.reconciliation(r).unwrap();
            let kept: Cents = taken.iter().filter(|(id, _)| !released.contains(id)).map(|(_, c)| c).sum();
            assert_eq!(now.books_balance, 100_000 + kept, "{seed}");
            assert_eq!(now.status == Status::Finished, released.is_empty(), "{seed}");
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

/// A line ticked on the side of the books instead of matched would be imported again: finishing waits for it.
#[test]
fn every_line_is_matched_before_finishing() {
    let (db, a) = bank();
    let bar = typed(&db, a, "2026-01-02", -120, "Bar", false);
    let salary = typed(&db, a, "2026-01-10", 150_000, "Salary", false);
    let shop = typed(&db, a, "2026-01-20", -8_030, "Shop", false);
    let lines = rows(STATEMENT);
    let r = start(&db, a, "2026-01-31", 100_000 - 120 + 150_000 - 8030, &lines);
    db.clear_for_reconciliation(r, &[bar, salary, shop], true).unwrap();
    assert_eq!(db.reconciliation(r).unwrap().difference, 0);
    let e = db.finish_reconciliation(r).unwrap_err();
    assert!(format!("{e:#}").contains('3'), "{e:#}");
    db.match_lines(r, &[(1, bar), (2, salary), (3, shop)]).unwrap();
    db.finish_reconciliation(r).unwrap();
    let again = importer::import_rows(&db, a, &lines, 0).unwrap();
    assert_eq!((again.added, again.duplicates), (0, 3));
    assert_eq!(status(&db, r), Status::Finished);
}

/// Deleting the other account of a transfer whose sides have their own dates leaves the reconciled side as it was.
#[test]
fn deleting_the_other_account_of_a_transfer_leaves_the_side_reconciled() {
    let (db, a) = bank();
    let s = db.add_account("Savings", AccountKind::Savings, "EUR", 0, "").unwrap();
    import(&db, a, "Data;Descrizione;Importo\n10/01/2026;GIRO;-200,00\n");
    import(&db, s, "Data;Descrizione;Importo\n12/01/2026;GIRO;200,00\n");
    let out = db.transactions(&TxFilter { account_id: Some(a), ..Default::default() }).unwrap()[0].id;
    let into = db.transactions(&TxFilter { account_id: Some(s), ..Default::default() }).unwrap()[0].id;
    db.turn_into_transfer(into, a, crate::transfers::OtherSide::Join(out)).unwrap();
    let r = start(&db, a, "2026-01-31", 80_000, &[]);
    db.finish_reconciliation(r).unwrap();
    let before = db.transaction(out).unwrap().date;
    // The side that stays takes the entry's date, which is its own: it stays reconciled.
    db.delete_account(s).unwrap();
    let t = db.transaction(out).unwrap();
    assert!(t.date == before && t.reconciled);
    assert_eq!(status(&db, r), Status::Finished);
    assert_sound(&db, "deleted");
}

/// An id that a posting had is never given to another: a reconciliation that remembers it finds that posting or none.
#[test]
fn the_id_of_a_posting_is_never_given_again() {
    let (db, a) = bank();
    let first = typed(&db, a, "2026-01-02", -120, "Bar", true);
    db.delete_transaction(first).unwrap();
    let second = typed(&db, a, "2026-01-02", -120, "Bar", true);
    assert!(second > first, "{second} {first}");
    let mut ids: Vec<i64> =
        db.entry(db.transaction(second).unwrap().entry_id).unwrap().postings.iter().map(|p| p.id).collect();
    ids.sort_unstable();
    assert!(ids.iter().all(|id| *id > first + 1), "{ids:?}");
}

/// A line whose transaction goes is a line to match again: added again, or its match undone.
#[test]
fn a_line_whose_transaction_goes_is_unmatched_again() {
    let (db, a) = bank();
    let bar = typed(&db, a, "2026-01-02", -120, "Bar", false);
    let lines = rows(STATEMENT);
    let r = start(&db, a, "2026-01-31", 100_000 - 120 + 150_000 - 8030, &lines);
    db.match_lines(r, &[(1, bar)]).unwrap();
    let added = db.add_lines(r, &[2, 3]).unwrap();
    db.delete_transaction(bar).unwrap();
    db.delete_transaction(added[1]).unwrap();
    let p = db.preview_reconciliation(r).unwrap();
    assert!(p.lines.iter().map(|l| l.matched.is_some()).eq([false, true, false]));
    assert!(!p.lines[2].added && p.unmatched_lines == 2);
    assert!(db.finish_reconciliation(r).is_err());
    db.unmatch_lines(r, &[1]).unwrap();
    db.add_lines(r, &[1, 3]).unwrap();
    assert_eq!(db.finish_reconciliation(r).unwrap().reconciled, 3);
    assert_sound(&db, "gone");
}

/// Undoing a match leaves alone a transaction that is no longer the one matched.
#[test]
fn undoing_a_match_touches_only_the_transaction_it_matched() {
    let (db, a) = bank();
    let bar = typed(&db, a, "2026-01-02", -120, "Bar", false);
    let r = start(&db, a, "2026-01-31", 0, &rows(STATEMENT));
    db.match_lines(r, &[(1, bar)]).unwrap();
    let keys = db.transaction(bar).unwrap().keys;
    edit(&db, bar, |e| e.date = day("2026-01-03")).unwrap();
    db.unmatch_lines(r, &[1]).unwrap();
    let t = db.transaction(bar).unwrap();
    assert!(t.cleared && t.keys == keys);
}

/// Finishing checks each match again: a transaction changed since has not the amount of its line any more.
#[test]
fn finishing_checks_each_match_again() {
    let (db, a) = bank();
    let lines = rows("Data;Descrizione;Importo\n02/01/2026;BAR;-1,20\n");
    let bar = typed(&db, a, "2026-01-02", -120, "Bar", false);
    let tip = typed(&db, a, "2026-01-02", 10, "Tip", false);
    let r = start(&db, a, "2026-01-31", 100_000 - 120, &lines);
    db.match_lines(r, &[(1, bar)]).unwrap();
    edit(&db, bar, |e| e.amount = -130).unwrap();
    db.clear_for_reconciliation(r, &[tip], true).unwrap();
    assert_eq!(db.reconciliation(r).unwrap().difference, 0);
    let e = db.finish_reconciliation(r).unwrap_err();
    assert!(format!("{e:#}").contains("line 1"), "{e:#}");
}

/// An added side of a transfer ticked without a statement stays one: the bank's line imported later takes its
/// place, and the date the reconciliation checked stays.
#[test]
fn an_added_side_reconciled_without_lines_is_taken_by_the_import() {
    let (db, a) = bank();
    let s = db.add_account("Savings", AccountKind::Savings, "EUR", 0, "").unwrap();
    let (_, side) = db.add_transfer(a, s, day("2026-01-10"), 20_000, "").unwrap();
    db.set_cleared(side, true).unwrap();
    let r = start(&db, s, "2026-01-31", 20_000, &[]);
    db.finish_reconciliation(r).unwrap();
    assert!(db.transaction(side).unwrap().added);
    let done =
        importer::import_rows(&db, s, &rows("Data;Descrizione;Importo\n13/01/2026;GIRO DA BANK;200,00\n"), 0).unwrap();
    assert_eq!((done.added, done.adopted.len()), (0, 1));
    let t = db.transaction(side).unwrap();
    assert!(t.reconciled && !t.added && t.date == day("2026-01-10"));
    assert_eq!(status(&db, r), Status::Finished);
    assert_sound(&db, "adopted");
}

/// A cheque written in October and cashed in November belongs to November; what is dated in October and comes
/// later leaves October as it was.
#[test]
fn a_transaction_belongs_to_the_reconciliation_that_takes_it() {
    let (db, a) = bank();
    import(&db, a, "Data;Descrizione;Importo\n05/10/2025;BAR;-50,00\n");
    let oct = rows("Data;Descrizione;Importo\n05/10/2025;BAR;-50,00\n");
    let october = start(&db, a, "2025-10-31", 95_000, &oct);
    let p = db.preview_reconciliation(october).unwrap();
    db.match_lines(october, &exact_pairs(&p)).unwrap();
    db.finish_reconciliation(october).unwrap();
    let cheque = typed(&db, a, "2025-10-28", -3_000, "Cheque 123", false);
    typed(&db, a, "2025-10-15", -700, "Fee typed late", true);
    import(&db, a, "Data;Descrizione;Importo\n20/10/2025;LATE LINE;-1,00\n");
    assert_eq!(status(&db, october), Status::Finished);
    let nov = rows("Data;Descrizione;Importo\n02/11/2025;PAGAMENTO ASSEGNO N.123;-30,00\n");
    let november = start(&db, a, "2025-11-30", 92_000, &nov);
    let p = db.preview_reconciliation(november).unwrap();
    assert_eq!(p.lines[0].proposed.as_ref().map(|t| t.id), Some(cheque));
    db.match_lines(november, &[(1, cheque)]).unwrap();
    assert_eq!(db.finish_reconciliation(november).unwrap().books_balance, 92_000);
    assert_eq!((status(&db, october), status(&db, november)), (Status::Finished, Status::Finished));
    assert_eq!(db.reconciliation(october).unwrap().books_balance, 95_000);
}

/// The opening balance and the currency of a reconciled account are asked about too.
#[test]
fn the_opening_balance_and_the_currency_of_a_reconciled_account_ask_too() {
    let (db, a, r) = reconciled_bank();
    let before = dump(&db.conn);
    let e = db.update_account(a, "Bank", AccountKind::Bank, 90_000, "", false).unwrap_err();
    assert!(reconciled_in(&e).is_some_and(|l| l.transactions.is_empty() && l.reconciliations == [r]));
    assert!(reconciled_in(&db.set_account_currency(a, "USD").unwrap_err()).is_some());
    assert!(ops::call(&db, "set_account_currency", &json!({"account": "Bank", "currency": "USD"}), None).is_err());
    assert_eq!(dump(&db.conn), before);
    // The rest of the account changes freely.
    db.update_account(a, "Main", AccountKind::Bank, 100_000, "IT00", false).unwrap();
    assert_eq!(status(&db, r), Status::Finished);
    db.unreconciling(|| db.update_account(a, "Main", AccountKind::Bank, 90_000, "", false)).unwrap();
    assert_eq!(status(&db, r), Status::Broken);
    db.unreconciling(|| db.update_account(a, "Main", AccountKind::Bank, 100_000, "", false)).unwrap();
    ops::call(&db, "set_account_currency", &json!({"account": "Main", "currency": "USD", "unreconcile": true}), None)
        .unwrap();
    assert_eq!(status(&db, r), Status::Broken);
}

/// Discarding a reconciliation puts back the cleared marks its ticks set.
#[test]
fn discarding_puts_back_the_ticks() {
    let (db, a) = bank();
    let bar = typed(&db, a, "2026-01-02", -120, "Bar", false);
    let fee = typed(&db, a, "2026-01-03", -50, "Fee", true);
    let before = dump(&db.conn);
    let r = start(&db, a, "2026-01-31", 0, &[]);
    db.clear_for_reconciliation(r, &[bar], true).unwrap();
    assert!(db.transaction(bar).unwrap().cleared);
    db.clear_for_reconciliation(r, &[fee], false).unwrap();
    db.clear_for_reconciliation(r, &[fee], true).unwrap();
    db.undo_reconciliation(r).unwrap();
    assert_eq!(dump(&db.conn), before);
}

/// Every key a match gives, the marks of the file of an older import too, goes when it is undone.
#[test]
fn undoing_a_match_takes_back_every_key_it_gave() {
    let (db, a) = bank();
    let tx = NewTx {
        account_id: a,
        date: day("2026-01-02"),
        amount: -120,
        payee: "Bar old".into(),
        import_hash: Some("old".into()),
        ..Default::default()
    };
    let bar = db.add_transaction(&tx).unwrap().unwrap();
    let before = dump(&db.conn);
    let r = start(&db, a, "2026-01-31", 0, &rows(STATEMENT));
    db.match_lines(r, &[(1, bar)]).unwrap();
    assert!(db.transaction(bar).unwrap().keys.contains("src:"));
    db.undo_reconciliation(r).unwrap();
    assert_eq!(dump(&db.conn), before);
}
