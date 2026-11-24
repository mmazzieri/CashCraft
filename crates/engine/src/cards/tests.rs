use super::*;
use crate::card_pdf::tests::{statement, table, with};
use crate::db::{NewTx, month_range};
use crate::importer;
use crate::ledger::tests::{assert_sound, dump};
use crate::money::parse_date;
use crate::ops;
use serde_json::json;

const BANK: &str =
    "Data;Descrizione;Importo\n16/12/2026;ADDEBITO CARTA;-1.163,10\n16/12/2026;BAR;-1,20\n20/12/2026;SPESA;-30,00\n";

struct Books {
    db: Db,
    bank: i64,
    card: i64,
}

impl Books {
    fn new() -> Self {
        let db = Db::open_in_memory().unwrap();
        let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 1_000_000, "").unwrap();
        let card = db.add_account("Card", AccountKind::CreditCard, "EUR", -500, "").unwrap();
        Books { db, bank, card }
    }
    fn import_bank(&self) -> usize {
        let t = importer::analyze(BANK, "UTF-8").unwrap();
        let (rows, skipped) = importer::parse_rows(&t, &t.mapping, false);
        importer::import(&self.db, self.bank, &t, &rows, skipped, false).unwrap().added
    }
    fn import_card(&self) -> usize {
        let t = table(&statement());
        let (rows, skipped) = importer::parse_rows(&t, &t.mapping, false);
        importer::import(&self.db, self.card, &t, &rows, skipped, false).unwrap().added
    }
    fn add(&self, account_id: i64, date: &str, amount: Cents) -> i64 {
        let date = parse_date(date).unwrap();
        let tx = NewTx { account_id, date, amount, payee: "x".into(), ..Default::default() };
        self.db.add_transaction(&tx).unwrap().unwrap()
    }
    fn expense(&self, month: &str) -> (Cents, Cents) {
        let (from, to) = month_range(parse_date(month).unwrap());
        let totals = self.db.category_totals(from, to).unwrap();
        crate::db::CategoryTotal::income_and_expense(&totals).unwrap()
    }
    fn balance(&self, account: i64) -> Cents {
        self.db.accounts(true).unwrap().iter().find(|a| a.id == account).unwrap().balance
    }
    fn only(&self) -> (i64, Vec<i64>) {
        let p = self.db.propose_card_payments().unwrap();
        assert_eq!(p.len(), 1);
        (p[0].statement.id, p[0].candidates.iter().map(|c| c.id).collect())
    }
}

#[test]
fn the_debit_becomes_a_transfer_whatever_is_imported_first() {
    for bank_first in [true, false] {
        let b = Books::new();
        if bank_first {
            b.import_bank();
            assert!(b.db.propose_card_payments().unwrap().is_empty());
            b.import_card();
        } else {
            b.import_card();
            assert!(b.only().1.is_empty());
            b.import_bank();
        }
        // December has the debit as an expense, on top of the purchases of November.
        assert_eq!(b.expense("2026-11-01"), (0, 116_260));
        assert_eq!(b.expense("2026-12-01"), (0, 116_310 + 120 + 3_000));
        let (s, candidates) = b.only();
        let [payment] = candidates[..] else { panic!("{candidates:?}") };
        let before = b.db.transaction(payment).unwrap();
        b.db.link_card_payment(s, payment).unwrap();
        let linked = dump(&b.db.conn);
        b.db.link_card_payment(s, payment).unwrap();
        assert_eq!(dump(&b.db.conn), linked, "twice");
        let t = b.db.transaction(payment).unwrap();
        assert_eq!((t.kind, t.date, &t.payee, t.cleared), (TxKind::Transfer, before.date, &before.payee, true));
        assert_eq!(b.expense("2026-11-01"), (0, 116_260));
        assert_eq!(b.expense("2026-12-01"), (0, 3_120));
        // What is left on the card is the new residual debt.
        assert_eq!(b.balance(b.card), -450);
        assert!(b.db.propose_card_payments().unwrap().is_empty());
        assert_eq!((b.import_bank(), b.import_card()), (0, 0));
        assert_eq!(b.db.card_statements().unwrap().len(), 1);
        assert_sound(&b.db, "linked");
        b.db.unlink_card_payment(s).unwrap();
        let undone = dump(&b.db.conn);
        b.db.unlink_card_payment(s).unwrap();
        assert_eq!(dump(&b.db.conn), undone, "twice");
        let t = b.db.transaction(payment).unwrap();
        assert_eq!((t.kind, t.date, &t.payee), (TxKind::Uncategorized, before.date, &before.payee));
        assert_eq!((b.import_bank(), b.balance(b.card)), (0, -450 - 116_310));
        assert_eq!(b.only(), (s, vec![payment]));
        assert_sound(&b.db, "unlinked");
    }
}

#[test]
fn only_the_banks_ordinary_lines_near_the_date_are_proposed() {
    let b = Books::new();
    b.import_card();
    let savings = b.db.add_account("Savings", AccountKind::Savings, "EUR", 0, "").unwrap();
    let dollars = b.db.add_account("Dollars", AccountKind::Bank, "USD", 0, "").unwrap();
    let near: Vec<i64> = ["2026-12-15", "2026-12-12", "2026-12-25"].map(|d| b.add(b.bank, d, -116_310)).to_vec();
    b.add(b.bank, "2026-12-11", -116_310);
    b.add(b.bank, "2026-12-26", -116_310);
    b.add(b.bank, "2026-12-15", 116_310);
    b.add(b.bank, "2026-12-15", -116_311);
    b.add(savings, "2026-12-15", -116_310);
    b.add(dollars, "2026-12-15", -116_310);
    let moved = b.add(b.bank, "2026-12-15", -116_310);
    b.db.turn_into_transfer(moved, savings, OtherSide::New).unwrap();
    let (s, candidates) = b.only();
    assert_eq!(candidates, near, "nearest first, none chosen");
    let before = dump(&b.db.conn);
    for (statement, t) in [(s, moved), (s, 1), (s, 999_999), (999, near[0]), (s, moved + 2)] {
        assert!(b.db.link_card_payment(statement, t).is_err());
        assert_eq!(dump(&b.db.conn), before);
    }
    b.db.link_card_payment(s, near[1]).unwrap();
    assert!(b.db.link_card_payment(s, near[0]).is_err(), "one payment");
    // The link goes with the bank's line, and when the transfer is separated in the register.
    b.db.delete_transaction(near[1]).unwrap();
    assert_eq!(b.only().1, [near[0], near[2]]);
    b.db.link_card_payment(s, near[0]).unwrap();
    let other = b.db.transaction(near[0]).unwrap().transfer_peer.unwrap();
    b.db.separate_transfer(other, true).unwrap();
    assert_eq!(b.only().1, [near[0], near[2]]);
    assert_sound(&b.db, "proposals");
}

#[test]
fn the_cards_own_line_of_the_payment_is_joined() {
    let b = Books::new();
    b.import_card();
    b.import_bank();
    let own = b.add(b.card, "2026-12-15", 116_310);
    let balance = b.balance(b.card);
    let (s, candidates) = b.only();
    b.db.link_card_payment(s, candidates[0]).unwrap();
    assert_eq!(b.balance(b.card), balance);
    assert_eq!(b.db.transaction(candidates[0]).unwrap().transfer_peer, Some(own));
    b.db.unlink_card_payment(s).unwrap();
    assert_eq!((b.balance(b.card), b.db.transaction(own).unwrap().amount), (balance, 116_310));
    // With two of them, nobody chooses.
    b.add(b.card, "2026-12-16", 116_310);
    let before = dump(&b.db.conn);
    assert!(b.db.link_card_payment(s, candidates[0]).is_err());
    assert_eq!(dump(&b.db.conn), before);
    assert_sound(&b.db, "joined");
}

#[test]
fn a_transfer_made_by_hand_pays_the_statement() {
    let b = Books::new();
    b.import_card();
    b.db.add_transfer(b.bank, b.card, parse_date("2026-12-16").unwrap(), 116_310, "").unwrap();
    assert!(b.db.propose_card_payments().unwrap().is_empty());
    // It pays one statement, not two.
    let t = table(&with(&[("30/11/2026", "29/11/2026")]));
    let (rows, skipped) = importer::parse_rows(&t, &t.mapping, false);
    importer::import(&b.db, b.card, &t, &rows, skipped, false).unwrap();
    assert_eq!(b.db.propose_card_payments().unwrap().len(), 1);
}

#[test]
fn operations_link_and_unlink() {
    let b = Books::new();
    b.import_card();
    b.import_bank();
    let call = |name: &str, a: serde_json::Value| ops::call(&b.db, name, &a, None);
    let p = call("propose_card_payments", json!({})).unwrap();
    let (s, t) = (&p["card_payments"][0]["statement"]["id"], &p["card_payments"][0]["candidates"][0]["id"]);
    assert_eq!(p["card_payments"][0]["statement"]["amount"], 1163.1);
    let done = call("link_card_payment", json!({"statement_id": s, "transaction_id": t})).unwrap();
    assert_eq!(done["transaction"]["kind"], "transfer");
    let all = call("list_card_statements", json!({})).unwrap();
    assert_eq!(&all["statements"][0]["payment_id"], t);
    call("unlink_card_payment", json!({"statement_id": s})).unwrap();
    assert!(call("unlink_card_payment", json!({"statement_id": 99})).is_err());
    b.db.delete_account(b.card).unwrap();
    assert!(b.db.card_statements().unwrap().is_empty());
    assert_sound(&b.db, "operations");
}
