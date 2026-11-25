use super::*;
use crate::db::{AccountKind, Db, TxFilter};
use crate::importer::{self, Table};
use crate::ledger::tests::assert_sound;
use crate::ops;
use crate::pdf::tests::{Font, document, read_pages};
pub use crate::testing::{Item, statement};
use serde_json::json;

/// The statement with some text replaced.
pub fn with(changes: &[(&str, &str)]) -> Vec<Item> {
    let mut v = statement();
    for (from, to) in changes {
        assert!(v.iter().any(|i| i.text.contains(from)), "{from}");
        for i in v.iter_mut() {
            i.text = i.text.replace(from, to);
        }
    }
    v
}

pub fn bytes(items: &[Item]) -> Vec<u8> {
    document(Font::TrueType, items)
}

pub fn read(items: &[Item], font: Font) -> Result<Statement, Failure> {
    parse(&read_pages(&document(font, items))?)
}

pub fn table(items: &[Item]) -> Table {
    importer::card_table(read(items, Font::Type3).unwrap())
}

#[test]
fn a_statement_is_read_with_its_dates_signs_and_totals() {
    for font in [Font::Type3, Font::TrueType] {
        let s = read(&statement(), font).unwrap();
        let d = |s: &str| parse_date(s);
        assert_eq!((s.closing, s.debit_date, s.debit), (d("2026-11-30"), d("2026-12-15"), Some(116_310)));
        assert_eq!(s.cards, ["1234"]);
        let checks: Vec<_> = s.checks.iter().map(|c| (c.kind, c.expected, c.found)).collect();
        assert_eq!(
            checks,
            [
                (CheckKind::Purchases, Some(6_010), 6_010),
                (CheckKind::Instalments, Some(110_050), 110_050),
                (CheckKind::Debit, Some(116_310), 116_310),
                (CheckKind::FirstPage, Some(116_310), 116_310),
            ]
        );
        assert!(s.is_sound() && s.unread == 0, "{font:?}");
        let rows: Vec<_> =
            s.rows.iter().map(|r| (r.date.to_string(), r.amount, r.payee.as_str(), r.memo.as_str())).collect();
        let rata = |n| format!("Rata finanziamento per contributi previdenziali (Id. finanziamento: {n})");
        assert_eq!(
            rows,
            [
                ("2026-11-02".into(), -120, "BAR DEL MOLO ANCONA ITA", ""),
                ("2026-11-02".into(), -120, "BAR DEL MOLO ANCONA ITA", ""),
                (
                    "2026-11-05".into(),
                    -5_070,
                    "ONLINE SHOP NEW YORK USA",
                    "55,00 USD, cambio 1,0848 — Commissione di conversione applicata da Nexi: 0,70 Euro"
                ),
                ("2026-11-09".into(), -1_200, "NEGOZIO CON UN NOME MOLTO LUNGO VIA DELLE ROSE 5 MACERATA", ""),
                ("2026-11-11".into(), 1_000, "RIMBORSO NEGOZIO", ""),
                ("2026-11-15".into(), -60_050, rata(11).as_str(), ""),
                ("2026-11-15".into(), -50_000, rata(12).as_str(), ""),
                ("2026-11-30".into(), -500, "EDICOLA CENTRALE", ""),
                ("2026-11-30".into(), -200, "Imposta di bollo", ""),
            ]
        );
    }
}

#[test]
fn a_refund_may_have_its_minus_on_either_side() {
    for minus in ["10,00-", "- 10,00", "\u{2013}10,00", "10,00 -"] {
        // The PDF of the tests writes ASCII.
        let pages = {
            let mut p = read_pages(&document(Font::Type3, &statement())).unwrap();
            for w in p.iter_mut().flatten().flat_map(|l| l.words.iter_mut()).filter(|w| w.text == "-10,00") {
                w.text = minus.into();
            }
            // A minus on its own is a word.
            for l in p.iter_mut().flatten() {
                if let Some(i) = l.words.iter().position(|w| w.text.contains(' ')) {
                    let w = l.words.remove(i);
                    let (a, b) = w.text.split_once(' ').unwrap();
                    l.words.insert(i, Word { x0: w.x0 + 10.0, text: b.into(), ..w.clone() });
                    l.words.insert(i, Word { x1: w.x0 + 5.0, text: a.into(), ..w });
                }
            }
            p
        };
        let s = parse(&pages).unwrap();
        assert!(s.is_sound(), "{minus}: {:?}", s.checks);
        let r = s.rows.iter().find(|r| r.payee.starts_with("RIMBORSO")).unwrap();
        assert_eq!((r.amount, r.payee.as_str()), (1_000, "RIMBORSO NEGOZIO"), "{minus}");
    }
}

#[test]
fn movements_may_go_on_on_the_next_page() {
    let mut v = statement();
    for i in v.iter_mut().filter(|i| i.page == 1 && i.y > 440.0) {
        // The last purchase and the total.
        (i.page, i.y) = (2, if i.y < 450.0 { 106.0 } else { 111.0 + 11.0 });
    }
    v.retain(|i| !i.text.starts_with("*Per rendere"));
    let s = read(&v, Font::Type3).unwrap();
    assert!(s.is_sound(), "{:?}", s.checks);
    assert_eq!(s.rows.len(), 9);
}

#[test]
fn figures_that_do_not_add_up_are_reported() {
    let failing = |changes: &[(&str, &str)]| -> Vec<CheckKind> {
        let s = read(&with(changes), Font::Type3).unwrap();
        assert!(!s.is_sound());
        s.checks.iter().filter(|c| !c.ok()).map(|c| c.kind).collect()
    };
    assert_eq!(failing(&[("12,00", "13,00")]), [CheckKind::Purchases, CheckKind::Debit]);
    assert_eq!(failing(&[("500,00", "501,00")]), [CheckKind::Instalments, CheckKind::Debit]);
    assert_eq!(failing(&[("4,50", "4,60")]), [CheckKind::Debit]);
    assert_eq!(failing(&[("Euro 1.163,10", "Euro 1.163,20")]), [CheckKind::FirstPage]);
    // The amount is that under its own heading, not any of the page.
    let swapped = [("Euro 1.163,10", "Euro 9.999,99"), ("Euro 60,10", "Euro 1.163,10")];
    assert_eq!(failing(&swapped), [CheckKind::FirstPage]);
    assert_eq!(failing(&[("TOTALE ADDEBITO", "ALTRO")]), [CheckKind::Debit]);
    // A row that cannot be read.
    let s = read(&with(&[("09/11/26", "39/11/26")]), Font::Type3).unwrap();
    assert!(!s.is_sound() && s.unread > 0);
    let s = read(&with(&[("Rata finanziamento", "Altro finanziamento")]), Font::Type3).unwrap();
    assert!(!s.is_sound() && s.unread == 2);

    let db = Db::open_in_memory().unwrap();
    let card = db.add_account("Card", AccountKind::CreditCard, "EUR", 0, "").unwrap();
    let t = table(&with(&[("12,00", "13,00")]));
    let (rows, skipped) = importer::parse_rows(&t, &t.mapping, false);
    assert!(importer::import(&db, card, &t, &rows, skipped, false).is_err());
    assert!(db.transactions(&TxFilter::default()).unwrap().is_empty() && db.card_statements().unwrap().is_empty());
    assert_eq!(importer::import(&db, card, &t, &rows, skipped, true).unwrap().added, 9);
    assert_sound(&db, "mismatch");
}

#[test]
fn other_documents_are_refused() {
    for changes in [&[("RIEPILOGO DEI", "ALTRO")][..], &[("DETTAGLIO DEI", "ALTRO")]] {
        assert!(read(&with(changes), Font::Type3).is_err());
    }
    // Text all over the place is never a panic.
    let v = statement();
    for skip in 0..v.len() {
        let mut less = v.clone();
        less.remove(skip);
        let _ = parse(&read_pages(&document(Font::Type3, &less)).unwrap());
    }
    for t in ["-", "--1,00", ",00", "1,0", "1,000", "a,00", "..,00", "99999999999999999999,00", "€", "1.,00-"] {
        let _ = amount(t);
    }
    assert_eq!((amount("1.234,56"), amount("-1,00"), amount("1,00-")), (Some(123_456), Some(-100), Some(-100)));
    assert_eq!((amount("1,00*"), amount("1,0"), amount("12"), amount("")), (Some(100), None, None, None));
    for t in ["99/99/99", "1/2", "1/1/1/1", "01/01/123", "//", "a/b/cd"] {
        assert_eq!(day(t), None, "{t}");
    }
    assert_eq!(day("01/02/70"), NaiveDate::from_ymd_opt(2070, 2, 1));
}

/// Rows equal to some of the month before are other purchases; the same statement adds nothing.
#[test]
fn a_statement_is_imported_once_and_the_next_one_whole() {
    let db = Db::open_in_memory().unwrap();
    let card = db.add_account("Card", AccountKind::CreditCard, "EUR", 0, "").unwrap();
    let import = |items: &[Item]| {
        let t = table(items);
        let (rows, skipped) = importer::parse_rows(&t, &t.mapping, false);
        importer::import(&db, card, &t, &rows, skipped, false).unwrap().added
    };
    assert_eq!(import(&statement()), 9);
    assert_eq!(import(&statement()), 0);
    let next = with(&[("30/11/2026", "31/12/2026"), ("15 Dicembre 2026", "15 Gennaio 2027")]);
    assert_eq!(import(&next), 9);
    assert_eq!(import(&next), 0);
    assert_eq!(import(&statement()), 0);
    assert_eq!(db.card_statements().unwrap().len(), 2);
    assert_sound(&db, "two months");
}

#[test]
fn operations_preview_and_import_a_pdf() {
    let dir = std::env::temp_dir().join(format!("cashcraft-card-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let roots = ops::Roots::new(&dir, &dir).unwrap();
    std::fs::write(dir.join("good.pdf"), bytes(&statement())).unwrap();
    std::fs::write(dir.join("bad.pdf"), bytes(&with(&[("12,00", "13,00")]))).unwrap();
    let db = Db::open_in_memory().unwrap();
    db.add_account("Card", AccountKind::CreditCard, "EUR", 0, "").unwrap();
    db.add_account("Bank", AccountKind::Bank, "EUR", 0, "").unwrap();
    db.add_account("Dollars", AccountKind::CreditCard, "USD", 0, "").unwrap();
    let call = |name: &str, a: serde_json::Value| ops::call(&db, name, &a, Some(&roots));
    let p = call("preview_csv", json!({"path": "good.pdf"})).unwrap();
    assert_eq!((&p["rows_found"], &p["statement"]["sound"]), (&json!(9), &json!(true)));
    assert_eq!(p["statement"]["debit"], "1163.10");
    let p = call("preview_csv", json!({"path": "bad.pdf"})).unwrap();
    let check = json!({"check": "purchases", "expected": "60.10", "found": "61.10", "ok": false});
    assert_eq!(p["statement"]["checks"][0], check);
    assert_eq!(p["statement"]["sound"], false);
    assert!(call("import_csv", json!({"path": "bad.pdf", "account": "Card"})).is_err());
    assert!(call("import_csv", json!({"path": "good.pdf", "account": "Dollars"})).is_err());
    for wrong in [json!({"invert": true}), json!({"amount_column": "1"})] {
        let mut a = json!({"path": "good.pdf", "account": "Card"});
        a.as_object_mut().unwrap().extend(wrong.as_object().unwrap().clone());
        assert!(call("import_csv", a).is_err());
    }
    let r = call("import_csv", json!({"path": "good.pdf", "account": "Card"})).unwrap();
    assert_eq!(r["result"]["added"], 9);
    // The warning comes before anything is written.
    assert!(call("import_csv", json!({"path": "bad.pdf", "account": "Bank", "allow_mismatch": true})).is_err());
    assert_eq!(db.transactions(&TxFilter::default()).unwrap().len(), 9);
    let sure = json!({"path": "bad.pdf", "account": "Bank", "allow_mismatch": true, "any_account": true});
    assert_eq!(call("import_csv", sure).unwrap()["result"]["added"], 9);
    // Only those of credit cards.
    assert_eq!(call("list_card_statements", json!({})).unwrap()["statements"].as_array().unwrap().len(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The real statement has no gap between the word and the date.
#[test]
fn the_closing_date_is_read_when_glued_to_its_label() {
    let items = with(&[("Debito residuo al 30/11/2026", "Debito residuo al30/11/2026")]);
    let s = read(&items, Font::Type3).unwrap();
    assert_eq!(s.closing, NaiveDate::from_ymd_opt(2026, 11, 30));
}

/// Without what tells it from the others, a statement is not imported even if the user insists.
#[test]
fn a_statement_that_is_not_identified_is_refused() {
    let import = |items: &[Item], force: bool| {
        let db = Db::open_in_memory().unwrap();
        let card = db.add_account("Card", AccountKind::CreditCard, "EUR", 0, "").unwrap();
        let t = table(items);
        assert!(!t.statement.as_ref().unwrap().is_sound());
        let (rows, skipped) = importer::parse_rows(&t, &t.mapping, false);
        importer::import(&db, card, &t, &rows, skipped, force).is_ok()
    };
    for changes in [
        &[("Debito residuo al 30/11/2026", "Debito rimasto al 30/11/2026")][..],
        &[("************1234", "************")],
        &[("************1234", "**** 12")],
    ] {
        for force in [false, true] {
            assert!(!import(&with(changes), force), "{changes:?}");
        }
    }
    // The date of the debit is in no key: the user may insist.
    let v = with(&[("15 Dicembre 2026", "più avanti"), ("1 Dicembre 2026", "oggi")]);
    assert!(!import(&v, false) && import(&v, true));
}

#[test]
fn the_number_of_the_card_is_read_however_it_is_spaced() {
    for (written, card) in [("************1234", "1234"), ("**** **** **** 9876", "9876"), ("**** 5555", "5555")] {
        let s = read(&with(&[("************1234", written)]), Font::Type3).unwrap();
        assert_eq!((s.cards.as_slice(), s.is_sound()), (&[card.to_string()][..], true), "{written}");
    }
    // Two cards on one account, with equal rows: both are kept.
    let db = Db::open_in_memory().unwrap();
    let account = db.add_account("Card", AccountKind::CreditCard, "EUR", 0, "").unwrap();
    for (written, added) in [("**** 1234", 9), ("**** 9876", 9), ("**** 1234", 0)] {
        let t = table(&with(&[("************1234", written)]));
        let (rows, skipped) = importer::parse_rows(&t, &t.mapping, false);
        assert_eq!(importer::import(&db, account, &t, &rows, skipped, false).unwrap().added, added);
    }
    assert_eq!(db.card_statements().unwrap().len(), 2);
}

/// Rows of two months that are equal in everything: only the statement tells them.
#[test]
fn equal_rows_of_two_statements_are_kept() {
    let next = with(&[("30/11/2026", "01/12/2026")]);
    let rows = |v: &[Item]| read(v, Font::Type3).unwrap().rows;
    let (a, b) = (rows(&statement()), rows(&next));
    let key = |r: &CardRow| r.key.clone();
    assert!(a.iter().map(key).all(|k| !b.iter().map(key).any(|x| x == k)));
}

/// Totals that add up are not enough: a row that was not read, or a card without its number.
#[test]
fn what_was_not_read_is_never_sound() {
    let item = |page, x, y, text: &str| Item { page, x, y, text: text.into() };
    let mut v = statement();
    v.extend([item(1, 34.0, 454.0, "31/11/26"), item(1, 75.0, 454.0, "X"), item(1, 370.0, 454.0, "0,00")]);
    let s = read(&v, Font::Type3).unwrap();
    assert!(s.checks.iter().all(Check::ok) && s.unread == 1 && !s.is_sound());
    let mut v = statement();
    for i in v.iter_mut().filter(|i| i.page == 2) {
        i.text = i.text.replace("************1234", "************");
    }
    let s = read(&v, Font::Type3).unwrap();
    assert!(s.checks.iter().all(Check::ok) && s.cards == ["1234"] && !s.is_identified() && !s.is_sound());
}

#[test]
fn the_figure_of_another_heading_is_not_that_of_the_debit() {
    let mut v = statement();
    // Nearer, but below the heading on the left, and one further right.
    v.push(Item { page: 0, x: 40.0, y: 130.0, text: "Euro 1,00".into() });
    v.push(Item { page: 0, x: 520.0, y: 130.0, text: "2,00".into() });
    let s = read(&v, Font::Type3).unwrap();
    assert!(s.is_sound(), "{:?}", s.checks);
    // Without a figure of its own there is nothing to compare.
    v.retain(|i| i.page != 0 || i.x < 200.0 || !i.text.starts_with("Euro"));
    v.retain(|i| i.page != 0 || i.text != "2,00");
    let s = read(&v, Font::Type3).unwrap();
    assert!(s.checks.iter().all(|c| c.kind != CheckKind::FirstPage) && s.is_sound());
}
