use super::*;
use crate::db::{AccountKind, Db, TxFilter};
use crate::importer::{self, Table};
use crate::ledger::tests::assert_sound;
use crate::ops;
use crate::pdf;
use crate::pdf::tests::{Font, Item, document};
use serde_json::json;

/// A statement laid out as the real one, with invented contents.
pub fn statement() -> Vec<Item> {
    let mut v: Vec<Item> = Vec::new();
    let mut add = |page, y, words: &[(f64, &str)]| {
        for (x, text) in words {
            v.push(Item { page, x: *x, y, text: text.to_string() });
        }
    };
    add(0, 120.0, &[(40.0, "QUESTO MESE HA SPESO"), (300.0, "QUESTO MESE LE SARANNO ADDEBITATI")]);
    add(0, 140.0, &[(40.0, "Euro 60,10"), (300.0, "Euro 1.163,10")]);
    add(0, 160.0, &[(40.0, "Milano, 1 Dicembre 2026"), (300.0, "In data"), (340.0, "15 Dicembre 2026")]);

    add(1, 60.0, &[(34.0, "RIEPILOGO DEI SUOI MOVIMENTI"), (320.0, "I SUOI PUNTI")]);
    add(1, 80.0, &[(34.0, "Data"), (75.0, "Descrizione"), (230.0, "Importo in Euro"), (320.0, "Saldo punti 1.200")]);
    add(1, 95.0, &[(34.0, "31/10/26"), (75.0, "Debito residuo al mese precedente"), (265.0, "5,00")]);
    add(1, 110.0, &[(75.0, "Totale spese con carte a saldo"), (260.0, "60,10"), (320.0, "Valore 12,00")]);
    add(1, 125.0, &[(75.0, "Totale complessivo finanziamenti per contributi previdenziali")]);
    add(1, 126.0, &[(330.0, "1.100,50")]);
    add(1, 140.0, &[(75.0, "Imposta di bollo"), (265.0, "2,00")]);
    add(1, 155.0, &[(75.0, "TOTALE ADDEBITO SUL SUO C/C"), (253.0, "1.163,10")]);
    add(1, 170.0, &[(75.0, "Debito residuo al 30/11/2026"), (265.0, "4,50")]);
    add(1, 200.0, &[(34.0, "Le ricordiamo che 10,00 punti valgono")]);
    add(1, 300.0, &[(34.0, "CARTA NUMERO ************1234 A SALDO SCADENZA 01/29")]);
    add(1, 320.0, &[(34.0, "DETTAGLIO DEI SUOI MOVIMENTI")]);
    add(
        1,
        338.0,
        &[
            (34.0, "Data"),
            (75.0, "Descrizione"),
            (339.0, "Importo in Euro"),
            (417.0, "Importo in altre valute"),
            (529.0, "Cambio"),
        ],
    );
    add(1, 356.0, &[(34.0, "02/11/26"), (75.0, "BAR DEL MOLO ANCONA ITA"), (370.0, "1,20")]);
    add(1, 370.0, &[(34.0, "02/11/26"), (75.0, "BAR DEL MOLO ANCONA ITA"), (370.0, "1,20")]);
    add(1, 386.0, &[(75.0, "ONLINE SHOP NEW YORK USA")]);
    add(1, 389.5, &[(34.0, "05/11/26"), (366.0, "50,70"), (455.0, "55,00 USD"), (535.0, "1,0848")]);
    add(1, 393.0, &[(75.0, "Commissione di conversione applicata da Nexi: 0,70 Euro*")]);
    add(1, 407.0, &[(75.0, "NEGOZIO CON UN NOME MOLTO LUNGO")]);
    add(1, 410.5, &[(34.0, "09/11/26"), (366.0, "12,00")]);
    add(1, 414.0, &[(75.0, "VIA DELLE ROSE 5 MACERATA")]);
    add(1, 430.0, &[(34.0, "11/11/26"), (75.0, "RIMBORSO NEGOZIO"), (366.0, "-10,00")]);
    add(1, 446.0, &[(34.0, "30/11/26"), (75.0, "EDICOLA CENTRALE"), (370.0, "5,00")]);
    add(1, 462.0, &[(75.0, "TOTALE SPESE"), (366.0, "60,10")]);

    add(2, 60.0, &[(34.0, "CARTA NUMERO ************1234 A SALDO SCADENZA 01/29")]);
    add(2, 80.0, &[(34.0, "DETTAGLIO DEI SUOI MOVIMENTI")]);
    add(
        2,
        98.0,
        &[(34.0, "Data"), (75.0, "Descrizione"), (339.0, "Importo in Euro"), (417.0, "Importo in altre valute")],
    );
    add(2, 115.0, &[(34.0, "*Per rendere confrontabili le commissioni vale 1,00")]);
    add(2, 200.0, &[(34.0, "BOX FINANZIAMENTI PER CONTRIBUTI PREVIDENZIALI")]);
    add(2, 220.0, &[(34.0, "Data"), (75.0, "Descrizione"), (339.0, "Importo in Euro")]);
    for (y, id, amount) in [(238.0, "11", "600,50"), (254.0, "12", "500,00")] {
        let text = format!("Rata finanziamento per contributi previdenziali (Id. finanziamento: {id})");
        add(2, y, &[(34.0, "15/11/26"), (75.0, &text), (362.0, amount)]);
    }
    add(2, 300.0, &[(34.0, "SITUAZIONE CONTABILE SECONDA LINEA DI CREDITO - 9 MESI (11)")]);
    add(2, 320.0, &[(34.0, "Data"), (75.0, "Descrizione"), (339.0, "Importo in Euro")]);
    add(2, 340.0, &[(34.0, "15/11/26"), (75.0, "Importo rata (3/9)(A+B)"), (362.0, "600,50")]);
    add(2, 355.0, &[(34.0, "15/11/26"), (75.0, "Quota interessi (B)"), (370.0, "9,50")]);
    add(2, 400.0, &[(34.0, "CONDIZIONI APPLICATE")]);
    add(2, 420.0, &[(34.0, "15/11/26"), (75.0, "TAN 9,90")]);
    v
}

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

pub fn read(items: &[Item], font: Font) -> Result<Statement> {
    parse(&pdf::pages(&document(font, items))?)
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
            let mut p = pdf::pages(&document(Font::Type3, &statement())).unwrap();
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
    for changes in [
        &[("RIEPILOGO DEI", "ALTRO")][..],
        &[("DETTAGLIO DEI", "ALTRO")],
        &[("Debito residuo al 30/11/2026", "x"), ("15 Dicembre", "x"), ("1 Dicembre", "x")],
    ] {
        assert!(read(&with(changes), Font::Type3).is_err());
    }
    // Text all over the place is never a panic.
    let v = statement();
    for skip in 0..v.len() {
        let mut less = v.clone();
        less.remove(skip);
        let _ = parse(&pdf::pages(&document(Font::Type3, &less)).unwrap());
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
    assert_eq!((&r["result"]["added"], &r["warning"]), (&json!(9), &json!(null)));
    let r = call("import_csv", json!({"path": "bad.pdf", "account": "Bank", "allow_mismatch": true})).unwrap();
    assert_eq!(r["warning"], "the account is not a credit card");
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
