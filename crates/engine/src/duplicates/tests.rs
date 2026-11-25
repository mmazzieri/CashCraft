use super::*;
use crate::db::AccountKind;
use crate::importer::{Choices, ImportResult, Settings, Table, analyze, import_with, ofx_table, read_rows};
use crate::ledger::tests::assert_sound;
use crate::ofx;
use std::fmt::Write as _;

/// FITID, date, amount, name.
type Line<'a> = (&'a str, &'a str, &'a str, &'a str);

fn ofx_file(lines: &[Line]) -> Table {
    let mut list = String::new();
    for (id, date, amount, name) in lines {
        let id = if id.is_empty() { String::new() } else { format!("<FITID>{id}") };
        let _ = write!(list, "<STMTTRN><DTPOSTED>{date}<TRNAMT>{amount}{id}<NAME>{name}</STMTTRN>");
    }
    let text = format!(
        "<OFX><STMTRS><CURDEF>EUR<BANKACCTFROM><ACCTID>1</BANKACCTFROM><BANKTRANLIST>{list}</BANKTRANLIST></STMTRS></OFX>"
    );
    let (o, encoding) = ofx::read(text.as_bytes()).unwrap();
    ofx_table(o, encoding)
}

fn csv_file(lines: &[(&str, &str, &str)]) -> Table {
    let mut text = "Data;Descrizione;Importo\n".to_string();
    for (date, amount, name) in lines {
        let _ = writeln!(text, "{date};{name};{amount}");
    }
    analyze(&text, "UTF-8").unwrap()
}

struct Books {
    db: Db,
    account: i64,
}

impl Books {
    fn new() -> Self {
        let db = Db::open_in_memory().unwrap();
        let account = db.add_account("A", AccountKind::Bank, "EUR", 0, "").unwrap();
        Books { db, account }
    }
    fn import(&self, t: &Table, likely: Decision) -> Result<ImportResult> {
        let rows = self.rows(t);
        import_with(&self.db, self.account, t, &rows, 0, &Choices { likely, ..Default::default() })
    }
    fn rows(&self, t: &Table) -> Vec<ParsedRow> {
        read_rows(t, &Settings::detected(t)).rows
    }
    fn likely(&self, t: &Table) -> Vec<(usize, Reason, usize)> {
        let l = self.db.likely_duplicates(Some(self.account), &self.rows(t)).unwrap();
        l.iter().map(|l| (l.index, l.reason, l.existing.len())).collect()
    }
    fn count(&self) -> usize {
        self.db.transactions(&TxFilter::default()).unwrap().len()
    }
}

impl Drop for Books {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert_sound(&self.db, "duplicates");
        }
    }
}

fn counts(r: ImportResult) -> (usize, usize, usize) {
    (r.added, r.duplicates, r.same)
}

#[test]
fn the_same_ofx_file_twice_adds_nothing() {
    let b = Books::new();
    // With ids, without, and an id that the bank uses for everything.
    let t = ofx_file(&[
        ("A", "20260105", "-1.20", "Bar"),
        ("", "20260105", "-1.20", "Bar"),
        ("", "20260105", "-1.20", "Bar"),
        ("0", "20260106", "-5", "One"),
        ("0", "20260107", "-6", "Two"),
        ("0", "20260107", "-6", "Two"),
    ]);
    assert!(b.likely(&t).is_empty());
    assert_eq!(counts(b.import(&t, Decision::Ask).unwrap()), (6, 0, 0));
    assert_eq!(counts(b.import(&t, Decision::Ask).unwrap()), (0, 6, 0));
    // The bank writes the text in another way: the id is enough.
    let again = ofx_file(&[("A", "20260105", "-1.20", "BAR ROMA")]);
    assert_eq!(counts(b.import(&again, Decision::Ask).unwrap()), (0, 1, 0));
}

#[test]
fn an_id_used_again_drops_nothing() {
    let b = Books::new();
    b.import(&ofx_file(&[("1", "20260105", "-10", "Shop")]), Decision::Ask).unwrap();
    // Another date and amount: asked, and added when the user says so.
    let t = ofx_file(&[("1", "20260205", "-25", "Other")]);
    assert_eq!(b.likely(&t), [(0, Reason::SameId, 1)]);
    assert!(b.import(&t, Decision::Ask).is_err());
    assert_eq!(b.count(), 1);
    assert_eq!(counts(b.import(&t, Decision::AddAll).unwrap()), (1, 0, 0));
    assert_eq!(counts(b.import(&t, Decision::Ask).unwrap()), (0, 1, 0));
    // By now the id tells nothing: no more questions.
    assert!(b.likely(&ofx_file(&[("1", "20260305", "-7", "Third")])).is_empty());
    // Corrected by the bank: skipped and remembered.
    b.import(&ofx_file(&[("2", "20260401", "-30", "Hotel")]), Decision::Ask).unwrap();
    let t = ofx_file(&[("2", "20260404", "-31", "Hotel")]);
    assert_eq!(counts(b.import(&t, Decision::SkipAll).unwrap()), (0, 0, 1));
    assert_eq!(counts(b.import(&t, Decision::Ask).unwrap()), (0, 1, 0));
}

#[test]
fn what_came_from_csv_is_not_doubled_by_ofx() {
    let b = Books::new();
    let csv = csv_file(&[
        ("05/01/2026", "-12,50", "BAR"),
        ("09/01/2026", "-3,00", "Edicola"),
        ("20/01/2026", "-12,50", "BAR"),
    ]);
    b.import(&csv, Decision::Ask).unwrap();
    let t = ofx_file(&[
        ("A", "20260106", "-12.50", "Bar Roma"),
        ("B", "20260107", "-99", "New"),
        ("C", "20260111", "-3", "Far"),
    ]);
    assert_eq!(b.likely(&t), [(0, Reason::SameAmount, 1)]);
    let refused = b.import(&t, Decision::Ask).unwrap_err().to_string();
    assert!(refused.contains("BAR"), "{refused}");
    assert_eq!(b.count(), 3);
    for wrong in [vec![], vec![(1, true)], vec![(0, true), (1, false)], vec![(0, true), (99, true)]] {
        assert!(b.import(&t, Decision::Each(wrong.into_iter().collect())).is_err());
        assert_eq!(b.count(), 3);
    }
    assert_eq!(counts(b.import(&t, Decision::Each(BTreeMap::from([(0, true)]))).unwrap()), (2, 0, 1));
    // Either file again: nothing, and nothing is asked.
    assert_eq!(counts(b.import(&t, Decision::Ask).unwrap()), (0, 3, 0));
    assert_eq!(counts(b.import(&csv, Decision::Ask).unwrap()), (0, 3, 0));
    // The key stays when the transaction becomes a transfer and back.
    let other = b.db.add_account("B", AccountKind::Savings, "EUR", 0, "").unwrap();
    let id = b.db.transactions(&TxFilter { search: "BAR".into(), ..Default::default() }).unwrap();
    let id = id.iter().find(|t| t.date.to_string() == "2026-01-05").unwrap().id;
    b.db.turn_into_transfer(id, other, crate::transfers::OtherSide::New).unwrap();
    assert_eq!(counts(b.import(&t, Decision::Ask).unwrap()), (0, 3, 0));
    b.db.separate_transfer(id, false).unwrap();
    assert_eq!(counts(b.import(&t, Decision::Ask).unwrap()), (0, 3, 0));
}

#[test]
fn what_came_from_ofx_is_not_doubled_by_csv() {
    let b = Books::new();
    let t = ofx_file(&[("A", "20260105", "-12.50", "Bar")]);
    b.import(&t, Decision::Ask).unwrap();
    let csv = csv_file(&[("04/01/2026", "-12,50", "PAGAMENTO BAR"), ("08/01/2026", "-12,50", "PAGAMENTO BAR")]);
    assert_eq!(b.likely(&csv), [(0, Reason::SameAmount, 1)]);
    assert!(b.import(&csv, Decision::Ask).is_err());
    assert_eq!(counts(b.import(&csv, Decision::SkipAll).unwrap()), (1, 0, 1));
    assert_eq!(counts(b.import(&csv, Decision::Ask).unwrap()), (0, 2, 0));
    assert_eq!(counts(b.import(&t, Decision::Ask).unwrap()), (0, 1, 0));
    // Two CSV files are compared as they always were.
    let next = csv_file(&[("09/01/2026", "-12,50", "PAGAMENTO BAR")]);
    assert!(b.likely(&next).is_empty());
}

#[test]
fn more_rows_than_transactions_cannot_all_be_the_same() {
    let b = Books::new();
    b.import(&csv_file(&[("05/01/2026", "-1,20", "BAR"), ("06/01/2026", "-1,20", "BAR")]), Decision::Ask).unwrap();
    let t = ofx_file(&[
        ("A", "20260105", "-1.20", "Bar"),
        ("B", "20260106", "-1.20", "Bar"),
        ("C", "20260107", "-1.20", "Bar"),
    ]);
    assert_eq!(b.likely(&t).len(), 3);
    assert!(b.import(&t, Decision::SkipAll).is_err());
    assert_eq!(b.count(), 2);
    let each = Decision::Each(BTreeMap::from([(0, true), (1, true), (2, false)]));
    assert_eq!(counts(b.import(&t, each).unwrap()), (1, 0, 2));
    assert_eq!(counts(b.import(&t, Decision::Ask).unwrap()), (0, 3, 0));
}

/// Pairing as many rows as can be, against trying every way.
#[test]
fn rows_are_paired_as_many_as_can_be() {
    let mut r = crate::testing::Seeded(7);
    for _ in 0..40 {
        let b = Books::new();
        let day = |n: usize| format!("{:02}/01/2026", 1 + n);
        let there: Vec<usize> = (0..1 + r.next(4)).map(|_| r.next(6)).collect();
        let new: Vec<usize> = (0..1 + r.next(4)).map(|_| r.next(6)).collect();
        let dates: Vec<String> = there.iter().map(|n| day(*n)).collect();
        let lines: Vec<_> = dates.iter().map(|d| (d.as_str(), "-1,00", "x")).collect();
        b.import(&csv_file(&lines), Decision::Ask).unwrap();
        let mut sorted = new.clone();
        sorted.sort();
        let near = |n: usize| there.iter().any(|t| t.abs_diff(n) <= 1);
        let asked: Vec<usize> = sorted.iter().copied().filter(|n| near(*n)).collect();
        fn all_fit(rows: &[usize], there: &[usize]) -> bool {
            let Some((first, rest)) = rows.split_first() else { return true };
            (0..there.len()).filter(|i| there[*i].abs_diff(*first) <= 1).any(|i| {
                let mut left = there.to_vec();
                left.remove(i);
                all_fit(rest, &left)
            })
        }
        let dates: Vec<(String, String)> =
            sorted.iter().enumerate().map(|(i, n)| (format!("id{i}"), format!("202601{:02}", 1 + n))).collect();
        let lines: Vec<_> = dates.iter().map(|(i, d)| (i.as_str(), d.as_str(), "-1", "y")).collect();
        let t = ofx_file(&lines);
        assert_eq!(b.likely(&t).len(), asked.len());
        assert_eq!(b.import(&t, Decision::SkipAll).is_ok(), all_fit(&asked, &there), "{there:?} {sorted:?}");
    }
}

#[test]
fn a_row_twice_in_the_file_is_asked() {
    let b = Books::new();
    let t = ofx_file(&[("A", "20260105", "-1.20", "Bar"), ("A", "20260105", "-1.20", "Bar")]);
    assert_eq!(b.likely(&t), [(1, Reason::Repeated, 0)]);
    assert!(b.db.likely_duplicates(None, &b.rows(&t)).unwrap().len() == 1, "also for a new account");
    assert!(b.import(&t, Decision::Ask).is_err());
    assert_eq!(counts(b.import(&t, Decision::SkipAll).unwrap()), (1, 0, 1));
    assert_eq!(counts(b.import(&t, Decision::Ask).unwrap()), (0, 2, 0));
    let other = Books::new();
    assert_eq!(counts(other.import(&t, Decision::AddAll).unwrap()), (2, 0, 0));
    assert_eq!(counts(other.import(&t, Decision::Ask).unwrap()), (0, 2, 0));
}

#[test]
fn a_correction_is_asked() {
    let b = Books::new();
    b.import(&ofx_file(&[("A", "20260105", "-10", "Shop")]), Decision::Ask).unwrap();
    let t = ofx_file(&[("B<CORRECTFITID>A<CORRECTACTION>REPLACE", "20260120", "-11", "Shop")]);
    assert_eq!(b.likely(&t), [(0, Reason::Corrects, 1)]);
    assert_eq!(counts(b.import(&t, Decision::SkipAll).unwrap()), (0, 0, 1));
    assert_eq!(counts(b.import(&t, Decision::Ask).unwrap()), (0, 1, 0));
    let unknown = ofx_file(&[("C<CORRECTFITID>Z<CORRECTACTION>DELETE", "20260120", "-12", "Shop")]);
    assert_eq!(b.likely(&unknown), [(0, Reason::Corrects, 0)]);
    assert_eq!(counts(b.import(&unknown, Decision::SkipAll).unwrap()), (0, 0, 1));
}

/// A line of OFX confirms the added side of a transfer, as one of CSV does.
#[test]
fn ofx_lines_take_the_place_of_added_sides() {
    let b = Books::new();
    let other = b.db.add_account("B", AccountKind::Savings, "EUR", 0, "").unwrap();
    b.db.add_transfer(b.account, other, crate::money::parse_date("2026-01-05").unwrap(), 5000, "").unwrap();
    let t = ofx_file(&[("A", "20260107", "-50", "GIROCONTO")]);
    let r = b.import(&t, Decision::Ask).unwrap();
    assert_eq!((r.added, r.adopted.len()), (0, 1));
    assert_eq!(counts(b.import(&t, Decision::Ask).unwrap()), (0, 1, 0));
}

#[test]
fn the_currency_must_be_that_of_the_account() {
    let b = Books::new();
    let mut t = ofx_file(&[("A", "20260105", "-10", "Shop")]);
    t.ofx.as_mut().unwrap().statements[0].currency = "USD".into();
    assert!(b.import(&t, Decision::Ask).is_err());
    assert_eq!(b.count(), 0);
    t.ofx.as_mut().unwrap().chosen = None;
    assert!(b.import(&t, Decision::Ask).is_err());
    assert!(b.rows(&t).is_empty());
}

#[test]
fn the_balance_of_a_day() {
    let b = Books::new();
    b.db.add_account("C", AccountKind::Bank, "EUR", 777, "").unwrap();
    b.import(&ofx_file(&[("A", "20260105", "-10", "x"), ("B", "20260107", "3", "y")]), Decision::Ask).unwrap();
    let on = |d: &str| b.db.balance_on(b.account, crate::money::parse_date(d).unwrap()).unwrap();
    assert_eq!((on("2026-01-04"), on("2026-01-05"), on("2026-01-06"), on("2026-12-31")), (0, -1000, -1000, -700));
    let c = b.db.find_account("C").unwrap().id;
    assert_eq!(b.db.balance_on(c, crate::money::parse_date("2026-01-04").unwrap()).unwrap(), 777);
}

/// A bank that gives everything one id, and a file that is all alike: quickly.
#[test]
fn many_equal_rows_are_quick() {
    let b = Books::new();
    let lines: Vec<(String, String)> =
        (0..1500).map(|i| (format!("{:02}/01/2026", 1 + i % 28), format!("n{i}"))).collect();
    let csv: Vec<_> = lines.iter().map(|(d, n)| (d.as_str(), "-1,00", n.as_str())).collect();
    b.import(&csv_file(&csv), Decision::Ask).unwrap();
    let start = std::time::Instant::now();
    let ids: Vec<String> = (0..20_000).map(|i| format!("202601{:02}", 1 + i % 28)).collect();
    let t = ofx_file(&ids.iter().map(|d| ("0", d.as_str(), "-1", "z")).collect::<Vec<_>>());
    assert_eq!(b.likely(&t).len(), 20_000);
    assert!(b.import(&t, Decision::SkipAll).is_err());
    assert!(start.elapsed().as_secs() < 20);
}
