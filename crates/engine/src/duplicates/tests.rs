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
    fn full(&self, t: &Table) -> Vec<LikelyDuplicate> {
        self.db.likely_duplicates(Some(self.account), &self.rows(t)).unwrap()
    }
    /// The first transaction of a day.
    fn id(&self, date: &str) -> i64 {
        let mut all = self.db.transactions(&TxFilter::default()).unwrap();
        all.reverse();
        all.iter().find(|t| t.date.to_string() == date).unwrap().id
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

fn ask() -> Decision {
    Decision::default()
}

fn add_all() -> Decision {
    Decision { add_others: true, ..Default::default() }
}

/// These rows are the transactions proposed, the others are added.
fn skip(rows: &[usize]) -> Decision {
    let each = rows.iter().map(|i| (*i, Choice::Skip(None))).collect();
    Decision { each, ..add_all() }
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
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (6, 0, 0));
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (0, 6, 0));
    // The bank writes the text in another way: the id is enough.
    let again = ofx_file(&[("A", "20260105", "-1.20", "BAR ROMA")]);
    assert_eq!(counts(b.import(&again, ask()).unwrap()), (0, 1, 0));
}

#[test]
fn an_id_used_again_drops_nothing() {
    let b = Books::new();
    b.import(&ofx_file(&[("1", "20260105", "-10", "Shop")]), ask()).unwrap();
    // Another date and amount: asked, and added when the user says so.
    let t = ofx_file(&[("1", "20260205", "-25", "Other")]);
    assert_eq!(b.likely(&t), [(0, Reason::SameId, 1)]);
    assert!(b.import(&t, ask()).is_err());
    assert_eq!(b.count(), 1);
    assert_eq!(counts(b.import(&t, add_all()).unwrap()), (1, 0, 0));
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (0, 1, 0));
    // By now the id tells nothing: no more questions.
    assert!(b.likely(&ofx_file(&[("1", "20260305", "-7", "Third")])).is_empty());
    // Corrected by the bank: skipped and remembered.
    b.import(&ofx_file(&[("2", "20260401", "-30", "Hotel")]), ask()).unwrap();
    let t = ofx_file(&[("2", "20260404", "-31", "Hotel")]);
    assert_eq!(counts(b.import(&t, skip(&[0])).unwrap()), (0, 0, 1));
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (0, 1, 0));
}

#[test]
fn what_came_from_csv_is_not_doubled_by_ofx() {
    let b = Books::new();
    let csv = csv_file(&[
        ("05/01/2026", "-12,50", "BAR"),
        ("09/01/2026", "-3,00", "Edicola"),
        ("12/01/2026", "-12,50", "BAR"),
    ]);
    b.import(&csv, ask()).unwrap();
    let t = ofx_file(&[
        ("A", "20260106", "-12.50", "Bar Roma"),
        ("B", "20260107", "-99", "New"),
        ("C", "20260115", "-3", "Far"),
    ]);
    assert_eq!(b.likely(&t), [(0, Reason::SameAmount, 1)]);
    let refused = b.import(&t, ask()).unwrap_err().to_string();
    assert!(refused.contains("BAR"), "{refused}");
    assert_eq!(b.count(), 3);
    let (no, yes) = (Choice::Add, Choice::Skip(None));
    for wrong in
        [vec![], vec![(1, yes)], vec![(0, yes), (1, no)], vec![(0, yes), (99, no)], vec![(0, Choice::Skip(Some(2)))]]
    {
        let wrong = Decision { each: wrong.into_iter().collect(), ..Default::default() };
        assert!(b.import(&t, wrong).is_err());
        assert_eq!(b.count(), 3);
    }
    assert_eq!(counts(b.import(&t, skip(&[0])).unwrap()), (2, 0, 1));
    // Either file again: nothing, and nothing is asked.
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (0, 3, 0));
    assert_eq!(counts(b.import(&csv, ask()).unwrap()), (0, 3, 0));
    // The key stays when the transaction becomes a transfer and back.
    let other = b.db.add_account("B", AccountKind::Savings, "EUR", 0, "").unwrap();
    let id = b.db.transactions(&TxFilter { search: "BAR".into(), ..Default::default() }).unwrap();
    let id = id.iter().find(|t| t.date.to_string() == "2026-01-05").unwrap().id;
    b.db.turn_into_transfer(id, other, crate::transfers::OtherSide::New).unwrap();
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (0, 3, 0));
    b.db.separate_transfer(id, false).unwrap();
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (0, 3, 0));
}

#[test]
fn what_came_from_ofx_is_not_doubled_by_csv() {
    let b = Books::new();
    let t = ofx_file(&[("A", "20260105", "-12.50", "Bar")]);
    b.import(&t, ask()).unwrap();
    let csv = csv_file(&[("04/01/2026", "-12,50", "PAGAMENTO BAR"), ("12/01/2026", "-12,50", "PAGAMENTO BAR")]);
    assert_eq!(b.likely(&csv), [(0, Reason::SameAmount, 1)]);
    assert!(b.import(&csv, ask()).is_err());
    assert_eq!(counts(b.import(&csv, skip(&[0])).unwrap()), (1, 0, 1));
    assert_eq!(counts(b.import(&csv, ask()).unwrap()), (0, 2, 0));
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (0, 1, 0));
    // Two CSV files are compared as they always were.
    let next = csv_file(&[("09/01/2026", "-12,50", "PAGAMENTO BAR")]);
    assert!(b.likely(&next).is_empty());
}

#[test]
fn more_rows_than_transactions_cannot_all_be_the_same() {
    let b = Books::new();
    b.import(&csv_file(&[("05/01/2026", "-1,20", "BAR"), ("06/01/2026", "-1,20", "BAR")]), ask()).unwrap();
    let t = ofx_file(&[
        ("A", "20260105", "-1.20", "Bar"),
        ("B", "20260106", "-1.20", "Bar"),
        ("C", "20260107", "-1.20", "Bar"),
    ]);
    assert_eq!(b.likely(&t).len(), 3);
    assert!(b.import(&t, skip(&[0, 1, 2])).is_err());
    let all = b.full(&t);
    assert_eq!(all.iter().map(|l| l.certain).collect::<Vec<_>>(), [true, true, false]);
    assert_eq!(
        (all[0].proposed, all[1].proposed, all[2].proposed),
        (Some(b.id("2026-01-05")), Some(b.id("2026-01-06")), None)
    );
    // Nor can two rows be one transaction.
    let one = Choice::Skip(all[0].proposed);
    let each = BTreeMap::from([(0, one), (1, Choice::Skip(None)), (2, one)]);
    assert!(b.import(&t, Decision { each, ..Default::default() }).is_err());
    assert_eq!(b.count(), 2);
    assert_eq!(counts(b.import(&t, skip(&[0, 1])).unwrap()), (1, 0, 2));
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (0, 3, 0));
}

/// Random rows: the same date and payee come first, then the same date, and never less of them than can be;
/// what is left over had nothing left to be paired with.
#[test]
fn the_likeliest_pairs_come_first() {
    let mut r = crate::testing::Seeded(7);
    for round in 0..60 {
        let b = Books::new();
        let mut make =
            |n: usize| -> Vec<(usize, &str)> { (0..n).map(|_| (r.next(8), ["alpha", "beta"][r.next(2)])).collect() };
        let (there, mut new) = (make(4), make(4));
        new.sort();
        let dates: Vec<String> = there.iter().map(|(d, _)| format!("{:02}/01/2026", 1 + d)).collect();
        let lines: Vec<_> = dates.iter().zip(&there).map(|(d, t)| (d.as_str(), "-1,00", t.1)).collect();
        b.import(&csv_file(&lines), ask()).unwrap();
        let dates: Vec<(String, String)> =
            new.iter().enumerate().map(|(i, (d, _))| (format!("k{i}"), format!("202601{:02}", 1 + d))).collect();
        let lines: Vec<_> = dates.iter().zip(&new).map(|((i, d), t)| (i.as_str(), d.as_str(), "-1", t.1)).collect();
        let t = ofx_file(&lines);
        let rows = b.rows(&t);
        let got = b.full(&t);
        let ids: Vec<i64> = got.iter().filter_map(|l| l.proposed).collect();
        assert_eq!(ids.len(), ids.iter().collect::<HashSet<_>>().len(), "{round}: one transaction twice");
        let count = |list: &[(usize, &str)], x: &(usize, &str)| list.iter().filter(|y| *y == x).count();
        let days = |list: &[(usize, &str)], d: usize| list.iter().filter(|y| y.0 == d).count();
        let same: usize = new.iter().collect::<HashSet<_>>().iter().map(|x| count(&new, x).min(count(&there, x))).sum();
        let dated: usize = (0..8).map(|d| days(&new, d).min(days(&there, d))).sum();
        let pair = |l: &LikelyDuplicate| l.existing.iter().find(|t| Some(t.id) == l.proposed).cloned();
        let is_dated = |l: &&LikelyDuplicate| pair(l).is_some_and(|t| t.date == rows[l.index].date);
        let is_same = |l: &&LikelyDuplicate| is_dated(l) && pair(l).is_some_and(|t| t.payee == rows[l.index].payee);
        assert_eq!(got.iter().filter(is_same).count(), same, "{round}: {there:?} {new:?}");
        assert_eq!(got.iter().filter(is_dated).count(), dated, "{round}: {there:?} {new:?}");
        for l in &got {
            assert!(l.proposed.is_some() || l.existing.iter().all(|t| ids.contains(&t.id)), "{round}: left over");
            let x = (new[l.index].0, new[l.index].1);
            assert_eq!(l.certain, count(&new, &x) == 1 && count(&there, &x) == 1, "{round}: {there:?} {new:?}");
            // The likeliest first.
            let far: Vec<bool> = l.existing.iter().map(|t| t.date != rows[l.index].date).collect();
            assert!(far.windows(2).all(|w| w[0] <= w[1]), "{round}");
        }
    }
}

/// A bus every day, and the last day of the books is the first of the file.
#[test]
fn rows_of_another_day_are_never_skipped_together() {
    let b = Books::new();
    b.import(
        &csv_file(&[
            ("30/01/2026", "-1,30", "Bus"),
            ("31/01/2026", "-1,30", "Bus"),
            ("31/01/2026", "-3,00", "Edicola"),
        ]),
        ask(),
    )
    .unwrap();
    let t = ofx_file(&[
        ("A", "20260131", "-1.30", "BUS"),
        ("B", "20260131", "-3", "EDICOLA"),
        ("C", "20260201", "-1.30", "BUS"),
    ]);
    let all = b.full(&t);
    assert_eq!(all.iter().map(|l| (l.index, l.certain)).collect::<Vec<_>>(), [(0, true), (1, true), (2, false)]);
    assert_eq!(all[0].proposed, Some(b.id("2026-01-31")));
    assert_eq!(all[2].proposed, Some(b.id("2026-01-30")));
    let same = Decision { skip_same: true, ..Default::default() };
    assert!(b.import(&t, same.clone()).is_err(), "the third row is not decided");
    assert_eq!(counts(b.import(&t, Decision { add_others: true, ..same }).unwrap()), (1, 0, 2));
    assert_eq!(b.count(), 4);
}

/// Same date comes before same payee, and the payee before the distance; the user can choose another.
#[test]
fn the_user_sees_and_chooses_the_transaction() {
    let b = Books::new();
    b.import(
        &csv_file(&[
            ("05/01/2026", "-10", "Alimentari Rossi"),
            ("05/01/2026", "-10", "Ferramenta"),
            ("06/01/2026", "-10", "Cinema"),
        ]),
        ask(),
    )
    .unwrap();
    let t = ofx_file(&[("A", "20260105", "-10", "ROSSI ALIMENTARI SRL"), ("B", "20260104", "-10", "CINEMA")]);
    let all = b.full(&t);
    let payees = |l: &LikelyDuplicate| l.existing.iter().map(|t| t.payee.clone()).collect::<Vec<_>>();
    // Rows are by date.
    assert_eq!(payees(&all[1]), ["Alimentari Rossi", "Ferramenta", "Cinema"]);
    assert_eq!(payees(&all[0]), ["Cinema", "Alimentari Rossi", "Ferramenta"]);
    assert_eq!((all[0].proposed, all[1].proposed), (Some(all[0].existing[0].id), Some(all[1].existing[0].id)));
    assert!(all.iter().all(|l| !l.certain));
    let chosen = all[1].existing[1].id;
    let each = BTreeMap::from([(0, Choice::Add), (1, Choice::Skip(Some(chosen)))]);
    assert_eq!(counts(b.import(&t, Decision { each, ..Default::default() }).unwrap()), (1, 0, 1));
    // The one chosen has the key now.
    let keys: String =
        b.db.conn.query_row("SELECT other_hashes FROM journal_postings WHERE id=?1", [chosen], |r| r.get(0)).unwrap();
    assert!(keys.contains("ofx:") && keys.contains("src:csv") && keys.contains("src:ofx"));
}

#[test]
fn five_days_apart_is_asked_and_six_is_not() {
    for (day, asked) in [("10", 1), ("11", 0)] {
        let b = Books::new();
        b.import(&csv_file(&[("05/01/2026", "-12,50", "BAR")]), ask()).unwrap();
        assert_eq!(b.likely(&ofx_file(&[("A", &format!("202601{day}"), "-12.5", "x")])).len(), asked);
    }
    // Two OFX files tell their transactions apart: one day.
    for (day, asked) in [("06", 1), ("07", 0)] {
        let b = Books::new();
        b.import(&ofx_file(&[("A", "20260105", "-12.5", "x")]), ask()).unwrap();
        assert_eq!(b.likely(&ofx_file(&[("B", &format!("202601{day}"), "-12.5", "y")])).len(), asked);
    }
}

/// CSV, Excel, PDF and OFX of the same account, one after the other.
#[test]
fn every_format_is_compared_with_the_others() {
    let pdf = |date: &str, cents: i64| {
        let key = format!("card|{date}|{cents}");
        let date = crate::money::parse_date(date).unwrap();
        let row = crate::importer::CardRow { date, amount: cents, payee: "SHOP SRL".into(), memo: String::new(), key };
        crate::importer::card_table(crate::importer::Statement {
            closing: None,
            debit_date: None,
            debit: None,
            residual: None,
            cards: vec![],
            rows: vec![row],
            unread: 0,
            unread_card: false,
            checks: vec![],
        })
    };
    let xls = |date: &str, amount: &str| {
        let row = |c: [&str; 3]| c.map(str::to_string).to_vec();
        let rows = vec![row(["Data", "Descrizione", "Importo"]), row([date, "Negozio", amount])];
        crate::importer::tests::load_bytes(&craft_xls::testing::workbook(&[("a", rows)]).unwrap()).unwrap()
    };
    let files: [(&str, Table); 4] = [
        ("csv", csv_file(&[("05/01/2026", "-9,99", "POS Shop")])),
        ("xls", xls("06/01/2026", "-9.99")),
        ("pdf", pdf("2026-01-03", -999)),
        ("ofx", ofx_file(&[("A", "20260107", "-9.99", "SHOP")])),
    ];
    for first in 0..4 {
        for second in (0..4).filter(|x| *x != first) {
            let b = Books::new();
            let import = |t: &Table, d: Decision| {
                let rows = b.rows(t);
                crate::importer::import_deciding(&b.db, b.account, &rows, 0, &d)
            };
            import(&files[first].1, ask()).unwrap();
            let names = (files[first].0, files[second].0);
            assert_eq!(b.likely(&files[second].1).len(), 1, "{names:?}");
            import(&files[second].1, skip(&[0])).unwrap();
            // And the third, and the fourth.
            for (name, t) in files.iter().enumerate().filter(|(i, _)| *i != first && *i != second).map(|(_, f)| f) {
                assert_eq!(b.likely(t).len(), 1, "{names:?} {name}");
                import(t, skip(&[0])).unwrap();
            }
            assert_eq!(b.count(), 1, "{names:?}");
            for (name, t) in &files {
                assert_eq!(counts(import(t, ask()).unwrap()), (0, 1, 0), "{names:?} {name}");
            }
        }
    }
}

/// What an older version imported has no mark of its file.
#[test]
fn rows_of_older_versions_are_compared() {
    let b = Books::new();
    let csv = csv_file(&[("05/01/2026", "-9,99", "POS Shop")]);
    b.import(&csv, ask()).unwrap();
    b.db.conn.execute("UPDATE journal_postings SET other_hashes=NULL", []).unwrap();
    assert!(b.likely(&csv_file(&[("06/01/2026", "-9,99", "POS Shop")])).is_empty());
    let t = ofx_file(&[("A", "20260107", "-9.99", "SHOP")]);
    assert_eq!(b.likely(&t).len(), 1);
    b.import(&t, skip(&[0])).unwrap();
    assert!(b.likely(&csv_file(&[("06/01/2026", "-9,99", "POS Shop")])).is_empty(), "still of a CSV file");
    assert_eq!(counts(b.import(&csv, ask()).unwrap()), (0, 1, 0));
}

/// A bank that numbers the transactions of each file from one.
#[test]
fn ids_that_start_again_are_not_believed() {
    let b = Books::new();
    b.import(
        &ofx_file(&[
            ("1", "20260601", "-10", "RENT"),
            ("2", "20260610", "-20", "GAS"),
            ("3", "20260615", "-33", "SHOE"),
        ]),
        ask(),
    )
    .unwrap();
    let t = ofx_file(&[("1", "20260615", "-33", "SHOE"), ("2", "20260620", "-7", "BAR")]);
    assert!(b.likely(&t).is_empty());
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (1, 1, 0));
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (0, 2, 0));
    // Not in order, and two others than they were.
    let t = ofx_file(&[("3", "20260701", "-1", "X"), ("1", "20260702", "-2", "Y"), ("77", "20260703", "-3", "Z")]);
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (3, 0, 0));
}

#[test]
fn an_id_for_everything_is_not_believed() {
    let b = Books::new();
    b.import(&ofx_file(&[("0", "20260107", "-5", "BAR"), ("0", "20260101", "-6", "X")]), ask()).unwrap();
    let t = ofx_file(&[("0", "20260107", "-5", "SHOP"), ("0", "20260107", "-5", "BAR"), ("0", "20260101", "-6", "X")]);
    // The shop has the amount of the bar, which is another row of the file: it is added.
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (1, 2, 0));
    assert_eq!(b.count(), 3);
}

/// What is exactly the same comes first, whatever the id says.
#[test]
fn the_exact_one_comes_before_the_one_of_the_id() {
    let b = Books::new();
    b.import(&ofx_file(&[("X", "20260601", "-10", "RENT"), ("Y", "20260615", "-33", "SHOE")]), ask()).unwrap();
    let t = ofx_file(&[("X", "20260615", "-33", "SHOE")]);
    let all = b.full(&t);
    assert_eq!(
        (all[0].reason, all[0].existing[0].payee.as_str(), all[0].existing[1].payee.as_str()),
        (Reason::SameId, "SHOE", "RENT")
    );
    assert_eq!((all[0].proposed, all[0].certain), (Some(all[0].existing[0].id), false));
    // Without it, the one of the id; a row of the same amount that is not exact takes nothing from it.
    let t = ofx_file(&[("X", "20260616", "-33", "OTHER")]);
    let all = b.full(&t);
    assert_eq!(all[0].existing.iter().find(|t| Some(t.id) == all[0].proposed).unwrap().payee, "RENT");
    // The transaction of an id that a row of the file is for sure is not proposed.
    let t = ofx_file(&[("X", "20260601", "-10", "RENT"), ("X2", "20260602", "-10", "RENT")]);
    assert!(b.likely(&t).is_empty());
}

#[test]
fn a_row_twice_in_the_file_is_asked() {
    let b = Books::new();
    let t = ofx_file(&[("A", "20260105", "-1.20", "Bar"), ("A", "20260105", "-1.20", "Bar")]);
    assert_eq!(b.likely(&t), [(1, Reason::Repeated, 0)]);
    assert!(b.db.likely_duplicates(None, &b.rows(&t)).unwrap().len() == 1, "also for a new account");
    assert!(b.import(&t, ask()).is_err());
    assert_eq!(counts(b.import(&t, skip(&[1])).unwrap()), (1, 0, 1));
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (0, 2, 0));
    let other = Books::new();
    assert_eq!(counts(other.import(&t, add_all()).unwrap()), (2, 0, 0));
    assert_eq!(counts(other.import(&t, ask()).unwrap()), (0, 2, 0));
}

#[test]
fn a_correction_is_asked() {
    let b = Books::new();
    b.import(&ofx_file(&[("A", "20260105", "-10", "Shop")]), ask()).unwrap();
    let t = ofx_file(&[("B<CORRECTFITID>A<CORRECTACTION>REPLACE", "20260120", "-11", "Shop")]);
    assert_eq!(b.likely(&t), [(0, Reason::Corrects, 1)]);
    assert_eq!(counts(b.import(&t, skip(&[0])).unwrap()), (0, 0, 1));
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (0, 1, 0));
    // One that is not there corrects nothing here: the new one is added.
    let unknown = ofx_file(&[("C<CORRECTFITID>Z<CORRECTACTION>REPLACE", "20260220", "-12", "Shop")]);
    assert!(b.likely(&unknown).is_empty());
    let t = ofx_file(&[("D<CORRECTFITID>A<CORRECTACTION>REPLACE", "20260221", "-13", "Shop")]);
    assert_eq!(counts(b.import(&t, add_all()).unwrap()), (1, 0, 0));
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (0, 1, 0));
}

/// What the bank deletes is never added, and said once.
#[test]
fn a_deletion_is_never_added() {
    let b = Books::new();
    b.import(&ofx_file(&[("A", "20260105", "-10", "Shop")]), ask()).unwrap();
    let t = ofx_file(&[
        ("B<CORRECTFITID>A<CORRECTACTION>DELETE", "20260105", "-10", "Shop"),
        ("C", "20260301", "-1", "New"),
    ]);
    assert!(b.likely(&t).is_empty());
    assert_eq!(b.db.deletions(Some(b.account), &b.rows(&t)).unwrap()[0].existing.payee, "Shop");
    let done = b.import(&t, ask()).unwrap();
    assert_eq!((done.added, done.deletions.len(), b.count()), (1, 1, 2));
    let done = b.import(&t, ask()).unwrap();
    assert_eq!((done.added, done.deletions.len(), b.count()), (0, 0, 2));
    let unknown = ofx_file(&[("D<CORRECTFITID>Z<CORRECTACTION>DELETE", "20260105", "-9", "X")]);
    assert_eq!(counts(b.import(&unknown, add_all()).unwrap()), (0, 0, 0));
    assert_eq!(b.count(), 2);
}

/// A side of a transfer that is twice in the file: what the user says is remembered on the side.
#[test]
fn a_repeated_line_of_a_transfer_is_remembered() {
    let b = Books::new();
    let other = b.db.add_account("B", AccountKind::Savings, "EUR", 0, "").unwrap();
    b.db.add_transfer(b.account, other, crate::money::parse_date("2026-01-05").unwrap(), 5000, "").unwrap();
    let t = ofx_file(&[("A", "20260107", "-50", "GIRO"), ("A", "20260107", "-50", "GIRO")]);
    let r = b.import(&t, skip(&[1])).unwrap();
    assert_eq!((r.added, r.adopted.len(), r.same), (0, 1, 1));
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (0, 2, 0));
}

#[test]
fn signs_that_contradict_the_types_are_confirmed() {
    let file = |types: [&str; 3]| {
        let one = |(i, t): (usize, &&str)| {
            format!("<STMTTRN><TRNTYPE>{t}<DTPOSTED>2026010{}<TRNAMT>10<FITID>{i}<NAME>n{i}</STMTTRN>", i + 1)
        };
        let list: String = types.iter().enumerate().map(one).collect();
        let text = format!(
            "<OFX><CCSTMTRS><CURDEF>EUR<CCACCTFROM><ACCTID>1</CCACCTFROM><BANKTRANLIST>{list}</BANKTRANLIST></CCSTMTRS></OFX>"
        );
        let (o, e) = ofx::read(text.as_bytes()).unwrap();
        ofx_table(o, e)
    };
    for (types, wrong) in [
        (["DEBIT", "DEBIT", "CREDIT"], true),
        (["DEBIT", "CREDIT", "CREDIT"], false),
        (["DEBIT", "CREDIT", "OTHER"], false),
        (["POS", "XFER", "FEE"], true),
        (["POS", "XFER", "INT"], false),
    ] {
        let b = Books::new();
        let t = file(types);
        assert_eq!(t.ofx.as_ref().unwrap().statement().unwrap().signs_contradict(), wrong, "{types:?}");
        assert_eq!(b.import(&t, ask()).is_err(), wrong, "{types:?}");
        if wrong {
            let settings = Settings { invert: true, ..Settings::detected(&t) };
            let rows = read_rows(&t, &settings).rows;
            let c = Choices { sign_confirmed: true, ..Default::default() };
            import_with(&b.db, b.account, &t, &rows, 0, &c).unwrap();
            assert_eq!(b.db.find_account("A").unwrap().balance, -3000);
            // The file is the same whatever the sign it is read with.
            let again = import_with(&b.db, b.account, &t, &b.rows(&t), 0, &c).unwrap();
            assert_eq!(counts(again), (0, 3, 0));
        }
    }
}

/// A line of OFX confirms the added side of a transfer, as one of CSV does.
#[test]
fn ofx_lines_take_the_place_of_added_sides() {
    let b = Books::new();
    let other = b.db.add_account("B", AccountKind::Savings, "EUR", 0, "").unwrap();
    b.db.add_transfer(b.account, other, crate::money::parse_date("2026-01-05").unwrap(), 5000, "").unwrap();
    let t = ofx_file(&[("A", "20260107", "-50", "GIROCONTO")]);
    let r = b.import(&t, ask()).unwrap();
    assert_eq!((r.added, r.adopted.len()), (0, 1));
    assert_eq!(counts(b.import(&t, ask()).unwrap()), (0, 1, 0));
}

#[test]
fn the_currency_must_be_that_of_the_account() {
    let b = Books::new();
    let mut t = ofx_file(&[("A", "20260105", "-10", "Shop")]);
    t.ofx.as_mut().unwrap().statements[0].currency = "USD".into();
    assert!(b.import(&t, ask()).is_err());
    assert_eq!(b.count(), 0);
    t.ofx.as_mut().unwrap().chosen = None;
    assert!(b.import(&t, ask()).is_err());
    assert!(b.rows(&t).is_empty());
}

#[test]
fn the_balance_of_a_day() {
    let b = Books::new();
    b.db.add_account("C", AccountKind::Bank, "EUR", 777, "").unwrap();
    b.import(&ofx_file(&[("A", "20260105", "-10", "x"), ("B", "20260107", "3", "y")]), ask()).unwrap();
    let on = |d: &str| b.db.balance_on(b.account, crate::money::parse_date(d).unwrap()).unwrap();
    assert_eq!((on("2026-01-04"), on("2026-01-05"), on("2026-01-06"), on("2026-12-31")), (0, -1000, -1000, -700));
    let c = b.db.find_account("C").unwrap().id;
    assert_eq!(b.db.balance_on(c, crate::money::parse_date("2026-01-04").unwrap()).unwrap(), 777);
}

/// Twice the rows, about twice the time: thousands of each kind.
#[test]
fn large_files_take_linear_time() {
    let run = |n: usize| {
        let b = Books::new();
        let start = std::time::Instant::now();
        // Skipped one by one onto transactions of CSV.
        let days: Vec<String> = (0..n).map(|i| format!("{:02}", 1 + i % 28)).collect();
        let (dates, amounts): (Vec<String>, Vec<String>) =
            days.iter().enumerate().map(|(i, d)| (format!("{d}/01/2026"), format!("-{},00", i + 1))).unzip();
        let lines: Vec<_> = dates.iter().zip(&amounts).map(|(d, a)| (d.as_str(), a.as_str(), "X")).collect();
        b.import(&csv_file(&lines), ask()).unwrap();
        let own: Vec<(String, String, String)> = days
            .iter()
            .enumerate()
            .map(|(i, d)| (format!("k{i}"), format!("202601{d}"), format!("-{}", i + 1)))
            .collect();
        let lines: Vec<_> = own.iter().map(|(k, d, a)| (k.as_str(), d.as_str(), a.as_str(), "X")).collect();
        let t = ofx_file(&lines);
        assert!(b.full(&t).iter().all(|l| l.certain));
        let same = Decision { skip_same: true, ..Default::default() };
        assert_eq!(counts(b.import(&t, same).unwrap()), (0, 0, n));
        // One row again and again, and rows that are all alike.
        let repeated = ofx_file(&vec![("R", "20260301", "-1.5", "Y"); n]);
        let every: Vec<usize> = (1..n).collect();
        assert_eq!(counts(b.import(&repeated, skip(&every)).unwrap()), (1, 0, n - 1));
        assert_eq!(counts(b.import(&repeated, ask()).unwrap()), (0, n, 0));
        let alike: Vec<String> = (0..n).map(|i| format!("q{i}")).collect();
        let alike: Vec<_> = alike.iter().map(|i| (i.as_str(), "20260401", "-2.5", "Z")).collect();
        b.import(&ofx_file(&alike), add_all()).unwrap();
        let constant =
            ofx_file(&[vec![("0", "20260402", "-2.5", "W"); n], vec![("0", "20260403", "-2.5", "W")]].concat());
        assert_eq!(b.likely(&constant).len(), n);
        start.elapsed().as_secs_f64()
    };
    let (small, large) = (run(500), run(2_000));
    assert!(large < small * 10.0 && large < 40.0, "{small} {large}");
}

/// Many sides of transfers, and as many lines.
#[test]
fn many_sides_of_transfers_take_linear_time() {
    let run = |n: i64| {
        let b = Books::new();
        let other = b.db.add_account("B", AccountKind::Savings, "EUR", 0, "").unwrap();
        let date = crate::money::parse_date("2026-01-05").unwrap();
        b.db.atomically(|| {
            for i in 1..=n {
                b.db.add_transfer(b.account, other, date, i * 100, "")?;
            }
            Ok(())
        })
        .unwrap();
        let amounts: Vec<(String, String)> = (1..=n).map(|i| (format!("k{i}"), format!("-{i}"))).collect();
        let t = ofx_file(&amounts.iter().map(|(k, a)| (k.as_str(), "20260106", a.as_str(), "G")).collect::<Vec<_>>());
        let start = std::time::Instant::now();
        assert_eq!(b.import(&t, ask()).unwrap().adopted.len(), amounts.len());
        start.elapsed().as_secs_f64()
    };
    let (small, large) = (run(300), run(1_200));
    assert!(large < small * 10.0 && large < 20.0, "{small} {large}");
}

/// One transaction of the books is one row of the file, whatever says so.
#[test]
fn a_transaction_is_never_two_rows() {
    let b = Books::new();
    b.import(&ofx_file(&[("A", "20260105", "-1.20", "Bar")]), ask()).unwrap();
    // The row without an id is not the one that has it.
    let t = ofx_file(&[("", "20260105", "-1.20", "Bar"), ("A", "20260105", "-1.20", "Bar")]);
    assert_eq!(counts(b.import(&t, add_all()).unwrap()), (1, 1, 0));
    assert_eq!(b.count(), 2);
    // What a row is by its payee is not what another has the id of.
    let b = Books::new();
    b.import(&ofx_file(&[("X", "20260105", "-1.20", "Bar")]), ask()).unwrap();
    let t = ofx_file(&[("", "20260105", "-1.20", "Bar"), ("X", "20260301", "-9", "Other")]);
    assert!(b.likely(&t).is_empty());
    // Two that replace the same one.
    let t = ofx_file(&[("B<CORRECTFITID>X", "20260402", "-2", "Bar"), ("C<CORRECTFITID>X", "20260403", "-3", "Bar")]);
    let all = b.full(&t);
    assert_eq!(all.iter().map(|l| l.proposed.is_some()).collect::<Vec<_>>(), [true, false]);
}

#[test]
fn the_same_payee_comes_before_one_alike_and_the_nearest_day_first() {
    let b = Books::new();
    b.import(
        &csv_file(&[("05/01/2026", "-1", "BAR ROMA CENTRO"), ("02/03/2026", "-7", "X"), ("04/03/2026", "-7", "X")]),
        ask(),
    )
    .unwrap();
    let t = ofx_file(&[
        ("A", "20260105", "-1", "BAR ROMA"),
        ("B", "20260105", "-1", "BAR ROMA CENTRO"),
        ("C", "20260303", "-7", "Y"),
        ("D", "20260305", "-7", "Y"),
    ]);
    let got: Vec<_> = b.full(&t).iter().map(|l| l.proposed).collect();
    let id = |d: &str| Some(b.id(d));
    assert_eq!(got, [None, id("2026-01-05"), id("2026-03-02"), id("2026-03-04")]);
}

#[test]
fn rows_without_a_believed_id_are_known_by_their_payee() {
    let b = Books::new();
    b.import(&ofx_file(&[("0", "20260107", "-5", "BAR"), ("0", "20260101", "-6", "X")]), ask()).unwrap();
    // Alone in the file, the id is the one that the books have on two.
    assert_eq!(counts(b.import(&ofx_file(&[("0", "20260107", "-5", "BAR")]), ask()).unwrap()), (0, 1, 0));
    // A number that is taken is not given again.
    let t = ofx_file(&[("", "20260201", "-1", "Bar"), ("", "20260201", "-1", "Bar")]);
    b.import(&t, ask()).unwrap();
    b.db.delete_transaction(b.id("2026-02-01")).unwrap();
    assert_eq!(counts(b.import(&t, add_all()).unwrap()), (1, 1, 0));
    assert_eq!(b.count(), 4);
}

/// The marks of a card's row of an older version, and of one of CSV.
#[test]
fn the_source_of_older_rows_is_kept() {
    for (card, source) in [(true, "src:pdf"), (false, "src:csv src:xls")] {
        let b = Books::new();
        b.import(&csv_file(&[("05/01/2026", "-1", "Bar")]), ask()).unwrap();
        let origin = card.then_some("card");
        b.db.conn
            .execute("UPDATE journal_postings SET other_hashes=NULL, origin=?1 WHERE import_hash IS NOT NULL", [origin])
            .unwrap();
        b.import(&ofx_file(&[("A", "20260105", "-1", "Bar")]), skip(&[0])).unwrap();
        let sql = "SELECT other_hashes FROM journal_postings WHERE import_hash IS NOT NULL";
        let marks: String = b.db.conn.query_row(sql, [], |r| r.get(0)).unwrap();
        assert!(marks.starts_with(source) && marks.contains("src:ofx"), "{marks}");
    }
}
