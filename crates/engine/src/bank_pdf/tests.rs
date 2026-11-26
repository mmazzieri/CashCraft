use super::*;
use crate::db::{AccountKind, Db, TxFilter};
use crate::importer::{self, Table};
use crate::ledger::tests::assert_sound;
use crate::pdf::tests::{Font, document, read_pages};
use crate::testing::{Item, Seeded, helvetica, movements};

fn file(items: &[Item], font: Font) -> Vec<u8> {
    match font {
        // As banks print it.
        Font::NotEmbedded => crate::testing::document(items, helvetica(), 7, 278),
        f => document(f, items),
    }
}

fn read(items: &[Item], font: Font) -> Result<Movements, Failure> {
    crate::pdf::parse(&read_pages(&file(items, font))?).and_then(|d| match d {
        crate::pdf::Document::Movements(m) => Ok(m),
        crate::pdf::Document::Card(_) => Err(Failure::Unknown),
    })
}

/// The list with some text replaced.
fn with(changes: &[(&str, &str)]) -> Vec<Item> {
    let mut v = movements();
    for (from, to) in changes {
        assert!(v.iter().any(|i| i.text.contains(from)), "{from}");
        for i in v.iter_mut() {
            i.text = i.text.replace(from, to);
        }
    }
    v
}

fn table(items: &[Item]) -> Table {
    importer::movements_table(&read(items, Font::NotEmbedded).unwrap())
}

#[test]
fn the_rows_are_read_with_their_descriptions_and_balances() {
    for font in [Font::NotEmbedded, Font::Type3, Font::TrueType] {
        let m = read(&movements(), font).unwrap();
        assert_eq!((m.opening, m.closing), (Some(10_000), Some(99_600)), "{font:?}");
        let got: Vec<[&str; 4]> = m.rows.iter().map(|r| [&*r[0], &*r[1], &*r[2], &*r[3]]).collect();
        let expected = [
            [
                "28/08/2026",
                "25/08/2026",
                "-79,00",
                "Operazione POS Eurozona Del 25.08.26 10:44 Carta *0000 OFFICINA ESEMPIO ROMA IT",
            ],
            ["18/11/2026", "18/11/2026", "-1.011,65", "Utilizzo carta di credito ****000 EMITTENTE"],
            [
                "20/11/2026",
                "20/11/2026",
                "2.000,00",
                "Bonifico a vs favore *INST 14:14 DITTA ESEMPIO Giroconto Info aggiuntive: Ragione sociale ordinante: DITTA ESEMPIO SRL ID_BONIFICO: 0000",
            ],
            ["23/11/2026", "23/11/2026", "-2,00", "Comm. richiesta incasso SEPA B2C"],
            ["23/11/2026", "23/11/2026", "-2,00", "Comm. richiesta incasso SEPA B2C"],
            [
                "25/11/2026",
                "21/11/2026",
                "-9,35",
                "Operazione POS Eurozona Del 21.11.26 18:19 Carta *0000 NEGOZIO ESEMPIO ROMA IT",
            ],
        ];
        assert_eq!(got, expected, "{font:?}");
    }
}

#[test]
fn the_rows_are_imported_as_those_of_a_csv_once() {
    let t = table(&movements());
    assert!(t.has_columns() && t.format() == importer::Format::Pdf);
    let (rows, skipped) = importer::parse_rows(&t, &t.mapping, false);
    assert_eq!((rows.len(), skipped), (6, 0));
    let payees: Vec<&str> = rows.iter().map(|r| r.payee.as_str()).collect();
    assert!(payees.contains(&"NEGOZIO ESEMPIO ROMA IT") && payees.contains(&"DITTA ESEMPIO SRL"), "{payees:?}");
    assert_eq!(rows.iter().map(|r| r.amount).sum::<Cents>(), 89_600);
    let db = Db::open_in_memory().unwrap();
    let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 0, "").unwrap();
    let import = |t: &Table| {
        let (rows, skipped) = importer::parse_rows(t, &t.mapping, false);
        importer::import(&db, bank, t, &rows, skipped, false).unwrap().added
    };
    assert_eq!(import(&t), 6);
    assert_eq!(import(&t), 0);
    // The next list repeats the last rows: only the new one is added.
    let next = with(&[("Utilizzo carta di credito ****000 EMITTENTE", "Utilizzo carta di credito ****000 EMITTENTE")]);
    let mut next = next;
    next.push(Item { page: 0, x: 54.0, y: 386.6, text: "17/11/2026".into() });
    next.push(Item { page: 0, x: 144.0, y: 386.6, text: "17/11/2026".into() });
    next.push(Item { page: 0, x: 276.8, y: 386.6, text: "-1,00".into() });
    next.push(Item { page: 0, x: 303.0, y: 386.6, text: "Commissioni su bonifico".into() });
    let next = next
        .into_iter()
        .map(|mut i| {
            i.text = i.text.replace("100,00", "101,00");
            i
        })
        .collect::<Vec<_>>();
    assert_eq!(import(&table(&next)), 1);
    assert_eq!(db.transactions(&TxFilter::default()).unwrap().len(), 7);
    assert_sound(&db, "two lists");
}

#[test]
fn rows_that_do_not_add_up_or_cannot_be_read_are_refused() {
    // A row whose booking date is lost, in a list without balances.
    let mut lost_date = movements();
    lost_date.retain(|i| !matches!((i.page, i.y), (0, 278.5) | (1, 296.5)));
    lost_date.retain(|i| !(i.page == 0 && i.y == 311.4 && i.x == 54.0));
    assert_eq!(read(&lost_date, Font::NotEmbedded), Err(Failure::Mismatch));
    let long = "x".repeat(MAX_TEXT + 1);
    assert_eq!(read(&with(&[("EMITTENTE", &long)]), Font::Type3), Err(Failure::TooLarge));
    // Also when the long text is a line below the date.
    assert_eq!(read(&with(&[("ID_BONIFICO: 0000", &long)]), Font::Type3), Err(Failure::TooLarge));
    for change in [("-9,35", "-9,36"), ("-9,35", "-9,3x"), ("18/11/2026", "18/1x/2026"), ("996,00", "995,00")] {
        assert_eq!(read(&with(&[change]), Font::NotEmbedded), Err(Failure::Mismatch), "{change:?}");
    }
    // A line of text that belongs to no row.
    let mut lost = movements();
    lost.push(Item { page: 1, x: 303.0, y: 310.0, text: "Testo senza data".into() });
    assert_eq!(read(&lost, Font::NotEmbedded), Err(Failure::Mismatch));
}

#[test]
fn a_list_without_balances_is_read() {
    let mut items = movements();
    items.retain(|i| !matches!((i.page, i.y), (0, 278.5) | (1, 296.5)));
    let m = read(&items, Font::NotEmbedded).unwrap();
    assert_eq!((m.rows.len(), m.opening, m.closing), (6, None, None));
}

#[test]
fn other_documents_are_not_lists() {
    let items = with(&[("Descrizione", "Causale")]);
    assert_eq!(read(&items, Font::NotEmbedded), Err(Failure::Unknown));
    // A card statement stays one.
    let card = read_pages(&document(Font::Type3, &crate::testing::statement())).unwrap();
    assert!(matches!(crate::pdf::parse(&card), Ok(crate::pdf::Document::Card(_))));
}

/// Words anywhere, of any text: the reader answers, whatever it is.
#[test]
fn hostile_pages_never_panic() {
    let pieces = [
        "Data",
        "contabile",
        "valuta",
        "Importo",
        "Descrizione",
        "01/02/2026",
        "-1.234,56",
        "Saldo",
        "iniziale",
        "finale",
        "x",
        "9".repeat(40).leak(),
    ];
    let mut r = Seeded(0xBA4C);
    let heading: Line = Line {
        y: 0.0,
        words: [
            (46.7, "Data"),
            (66.2, "contabile"),
            (142.7, "Data"),
            (162.2, "valuta"),
            (265.2, "Importo"),
            (303.0, "Descrizione"),
        ]
        .iter()
        .map(|(x, t)| Word { x0: *x, x1: x + 20.0, text: (*t).into() })
        .collect(),
    };
    for _ in 0..2000 {
        let mut page: Page = vec![heading.clone()];
        for n in 0..r.next(30) {
            let words = (0..r.next(8))
                .map(|_| {
                    let x = r.next(600) as f64 - 20.0;
                    Word { x0: x, x1: x + r.next(80) as f64, text: pieces[r.next(pieces.len())].to_string() }
                })
                .collect();
            page.push(Line { y: n as f64 * r.next(15) as f64, words });
        }
        let _ = parse(&[page.clone(), page]);
    }
}
