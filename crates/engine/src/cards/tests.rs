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
    fn csv(&self, account: i64, text: &str) -> importer::ImportResult {
        let t = importer::analyze(text, "UTF-8").unwrap();
        let (rows, skipped) = importer::parse_rows(&t, &t.mapping, false);
        importer::import(&self.db, account, &t, &rows, skipped, false).unwrap()
    }
    fn only(&self) -> (i64, Vec<i64>) {
        let p = self.db.propose_card_payments().unwrap();
        assert_eq!(p.len(), 1);
        (p[0].statement.id, p[0].candidates.iter().map(|c| c.transaction.id).collect())
    }
}

fn txs(b: &Books, account: i64) -> Vec<Transaction> {
    b.db.transactions(&TxFilter { account_id: Some(account), ..Default::default() }).unwrap()
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
        b.db.link_card_payment(s, payment, None).unwrap();
        let linked = dump(&b.db.conn);
        b.db.link_card_payment(s, payment, None).unwrap();
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
    b.add(b.bank, "2026-12-04", -116_310);
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
        assert!(b.db.link_card_payment(statement, t, None).is_err());
        assert_eq!(dump(&b.db.conn), before);
    }
    b.db.link_card_payment(s, near[1], None).unwrap();
    assert!(b.db.link_card_payment(s, near[0], None).is_err(), "one payment");
    // The link goes with the bank's line, and when the transfer is separated in the register.
    b.db.delete_transaction(near[1]).unwrap();
    assert_eq!(b.only().1, [near[0], near[2]]);
    b.db.link_card_payment(s, near[0], None).unwrap();
    let other = b.db.transaction(near[0]).unwrap().transfer_peer.unwrap();
    b.db.separate_transfer(other, true).unwrap();
    assert_eq!(b.only().1, [near[0], near[2]]);
    assert_sound(&b.db, "proposals");
}

/// Joining is the user's choice: without it the side of the card is added, whatever is there.
#[test]
fn the_cards_own_line_is_joined_only_when_asked() {
    let b = Books::new();
    b.import_card();
    b.import_bank();
    let own = b.add(b.card, "2026-12-15", 116_310);
    let balance = b.balance(b.card);
    let (s, candidates) = b.only();
    let proposal = &b.db.propose_card_payments().unwrap()[0];
    assert_eq!(proposal.candidates[0].card_lines.iter().map(|t| t.id).collect::<Vec<_>>(), [own]);
    b.db.link_card_payment(s, candidates[0], None).unwrap();
    assert_eq!(b.balance(b.card), balance + 116_310);
    b.db.unlink_card_payment(s).unwrap();
    assert_eq!((b.balance(b.card), b.only().1), (balance, candidates.clone()));
    assert!(b.db.link_card_payment(s, candidates[0], Some(candidates[0])).is_err());
    b.db.link_card_payment(s, candidates[0], Some(own)).unwrap();
    assert_eq!(b.balance(b.card), balance);
    assert_eq!(b.db.transaction(candidates[0]).unwrap().transfer_peer, Some(own));
    b.db.unlink_card_payment(s).unwrap();
    assert_eq!((b.balance(b.card), b.db.transaction(own).unwrap().amount), (balance, 116_310));
    assert_sound(&b.db, "joined");
}

/// A refund of the same amount on a statement is neither proposed nor taken for the payment.
#[test]
fn rows_of_a_card_statement_are_never_the_payment() {
    let refund = || crate::importer::ParsedRow {
        date: parse_date("2026-12-13").unwrap(),
        amount: 116_310,
        payee: "RIMBORSO".into(),
        memo: String::new(),
        key: Some("card|2026-12-31|1234|2026-12-13|-116310|rimborso".into()),
        ..Default::default()
    };
    let import = |b: &Books| crate::importer::import_rows(&b.db, b.card, &[refund()], 0).unwrap();
    let b = Books::new();
    b.import_bank();
    b.import_card();
    assert_eq!(import(&b).added, 1);
    let (s, c) = b.only();
    assert!(b.db.propose_card_payments().unwrap()[0].candidates[0].card_lines.is_empty());
    let t = txs(&b, b.card).into_iter().find(|t| t.payee == "RIMBORSO").unwrap();
    assert!(b.db.link_card_payment(s, c[0], Some(t.id)).is_err());
    // The refund stays what it is.
    let net = |b: &Books| {
        let (income, expense) = b.expense("2026-12-01");
        income - expense
    };
    let (card, before) = (b.balance(b.card), net(&b));
    b.db.link_card_payment(s, c[0], None).unwrap();
    assert_eq!(b.balance(b.card), card + 116_310);
    assert_eq!(net(&b), before + 116_310);
    assert_eq!(b.db.transaction(t.id).unwrap().kind, TxKind::Uncategorized);
    // Nor does one imported afterwards take the place of the side that was added.
    let b = Books::new();
    b.import_bank();
    b.import_card();
    let (s, c) = b.only();
    b.db.link_card_payment(s, c[0], None).unwrap();
    let res = import(&b);
    assert_eq!((res.added, res.adopted.len(), res.maybe_transfers.len()), (1, 0, 0));
    // A line of the payment does.
    let card = b.balance(b.card);
    let csv = "Data;Descrizione;Importo\n16/12/2026;PAGAMENTO RICEVUTO;1.163,10\n";
    assert_eq!(b.csv(b.card, csv).adopted.len(), 1);
    assert_eq!((b.balance(b.card), b.csv(b.card, csv).duplicates), (card, 1));
    // It came from a statement: unlinking keeps it.
    b.db.unlink_card_payment(s).unwrap();
    assert_eq!(b.balance(b.card), card);
    assert!(txs(&b, b.card).iter().any(|t| t.payee == "PAGAMENTO RICEVUTO" && t.kind == TxKind::Uncategorized));
    assert_sound(&b.db, "refund");
}

/// What the books say, not what was once stored, tells whether a statement is paid and how.
#[test]
fn the_books_tell_what_is_paid() {
    let b = Books::new();
    b.import_bank();
    b.import_card();
    let (s, c) = b.only();
    b.db.link_card_payment(s, c[0], None).unwrap();
    b.db.separate_transfer(c[0], false).unwrap();
    let stored = |b: &Books| -> Option<i64> {
        b.db.conn.query_row("SELECT payment_id FROM card_statements", [], |r| r.get(0)).unwrap()
    };
    assert_eq!((stored(&b), b.only().0), (None, s));
    b.csv(b.card, "Data;Descrizione;Importo\n16/12/2026;PAGAMENTO RICEVUTO;1.163,10\n");
    let own = txs(&b, b.card).into_iter().find(|t| t.payee == "PAGAMENTO RICEVUTO").unwrap().id;
    let before = b.balance(b.card);
    b.db.turn_into_transfer(c[0], b.card, OtherSide::Join(own)).unwrap();
    b.db.unlink_card_payment(s).unwrap();
    assert_eq!((b.balance(b.card), b.db.transaction(own).unwrap().amount), (before, 116_310));
    // By hand, it is paid as well: a transfer of that amount.
    assert!(b.db.propose_card_payments().unwrap().is_empty());
    b.db.separate_transfer(own, true).unwrap();
    b.db.link_card_payment(s, c[0], None).unwrap();
    // A link to what is no longer a transfer to this card does not count.
    let other = b.db.add_account("Other card", AccountKind::CreditCard, "EUR", 0, "").unwrap();
    let t = b.db.transaction(c[0]).unwrap();
    let mut d = b.db.load_draft(t.entry_id).unwrap();
    let (from, to) = (b.db.money_ledger(b.card).unwrap().0, b.db.money_ledger(other).unwrap().0);
    d.lines.iter_mut().filter(|l| l.account == from).for_each(|l| l.account = to);
    b.db.save_entry(&d).unwrap();
    assert_eq!(b.db.card_statements().unwrap()[0].payment_id, None);
    assert_sound(&b.db, "books");
}

#[test]
fn a_payment_whose_amount_was_changed_does_not_match() {
    let b = Books::new();
    b.import_bank();
    b.import_card();
    let (s, c) = b.only();
    b.db.link_card_payment(s, c[0], None).unwrap();
    assert!(b.db.card_statements().unwrap()[0].matches);
    let t = b.db.transaction(c[0]).unwrap();
    let edit = crate::db::TxEdit { date: t.date, amount: -1, payee: t.payee, ..Default::default() };
    b.db.update_transaction(c[0], &edit).unwrap();
    let now = &b.db.card_statements().unwrap()[0];
    assert_eq!((now.payment_id, now.matches), (Some(c[0]), false));
    b.db.unlink_card_payment(s).unwrap();
    assert!(!b.db.card_statements().unwrap()[0].matches);
}

/// The same window, and the same lines, for what is proposed, what can be linked and what can be joined.
#[test]
fn proposals_links_and_joins_agree() {
    for day in 1..=28 {
        let b = Books::new();
        b.import_card();
        let date = |d: i64| (parse_date("2026-12-15").unwrap() + chrono::Duration::days(d - 14)).to_string();
        let bank = b.add(b.bank, &date(day), -116_310);
        let own = b.add(b.card, "2026-12-11", 116_310);
        let p = b.db.propose_card_payments().unwrap().remove(0);
        let proposed = p.candidates.iter().find(|c| c.transaction.id == bank);
        assert_eq!(proposed.is_some(), (day - 14).abs() <= 10, "{day}");
        assert_eq!(b.db.link_card_payment(p.statement.id, bank, None).is_ok(), proposed.is_some(), "{day}");
        if let Some(c) = proposed {
            let joins: Vec<i64> = c.card_lines.iter().map(|t| t.id).collect();
            b.db.unlink_card_payment(p.statement.id).unwrap();
            let found = b.db.transfer_candidates(bank, b.card).unwrap();
            assert_eq!(found.iter().map(|t| t.id).collect::<Vec<_>>(), joins, "{day}");
            let linked = b.db.link_card_payment(p.statement.id, bank, Some(own));
            assert_eq!(linked.is_ok(), joins.contains(&own), "{day}");
        }
    }
}

/// A transfer made by hand before the bank's line is the one that line confirms.
#[test]
fn the_banks_line_confirms_a_transfer_made_by_hand() {
    let b = Books::new();
    b.import_card();
    b.db.add_transfer(b.bank, b.card, parse_date("2026-12-15").unwrap(), 116_310, "").unwrap();
    let before = b.balance(b.bank);
    b.import_bank();
    assert_eq!(b.balance(b.bank), before - 3_120);
    assert_eq!(b.expense("2026-12-01"), (0, 3_120));
    assert!(b.db.propose_card_payments().unwrap().is_empty());
    assert_eq!(b.import_bank(), 0);
    assert_sound(&b.db, "by hand");
}

#[test]
fn only_one_statement_is_paid_by_a_payment() {
    let b = Books::new();
    b.import_bank();
    b.import_card();
    let other = table(&with(&[("30/11/2026", "29/11/2026")]));
    let (rows, skipped) = importer::parse_rows(&other, &other.mapping, false);
    importer::import(&b.db, b.card, &other, &rows, skipped, false).unwrap();
    let p = b.db.propose_card_payments().unwrap();
    assert_eq!(p.len(), 2);
    let paid = p[0].candidates[0].transaction.id;
    b.db.link_card_payment(p[0].statement.id, paid, None).unwrap();
    // The transfer that pays one is not a payment made by hand of the other.
    let left = b.db.propose_card_payments().unwrap();
    assert_eq!((left.len(), left[0].statement.id), (1, p[1].statement.id));
    assert!(b.db.link_card_payment(p[1].statement.id, paid, None).is_err());
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
