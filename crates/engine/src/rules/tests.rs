use super::*;
use crate::db::{AccountKind, NewTx, TxEdit};
use crate::importer::{self, Table};
use crate::ledger::tests::{Lcg, assert_sound, dump};
use crate::transfers::OtherSide;
use crate::{card_pdf, ofx};
use rusqlite::Connection;
use std::path::PathBuf;

fn books() -> (Db, i64) {
    let db = Db::open_in_memory().unwrap();
    db.conn.execute("DELETE FROM transaction_rules", []).unwrap();
    let a = db.add_account("Bank", AccountKind::Bank, "EUR", 0, "").unwrap();
    (db, a)
}

fn day(d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 3, d).unwrap()
}

fn cat(db: &Db, name: &str) -> i64 {
    db.category_by_path(name, false, true).unwrap()
}

fn add(db: &Db, account_id: i64, payee: &str, memo: &str, amount: Cents) -> i64 {
    let tx = NewTx { account_id, date: day(4), amount, payee: payee.into(), memo: memo.into(), ..Default::default() };
    db.add_transaction(&tx).unwrap().unwrap()
}

fn csv(text: &str) -> Table {
    importer::analyze(text, "UTF-8").unwrap()
}

fn import(db: &Db, account: i64, t: &Table) -> importer::ImportResult {
    let (rows, skipped) = importer::parse_rows(t, &t.mapping, false);
    importer::import(db, account, t, &rows, skipped, false).unwrap()
}

fn words(payee: &str, category_id: Option<i64>) -> Rule {
    Rule { payee: payee.into(), category_id, ..Default::default() }
}

fn re(payee: &str, rename: &str) -> Rule {
    Rule { payee: payee.into(), regex: true, rename_payee: rename.into(), ..Default::default() }
}

fn postings(db: &Db) -> String {
    let all = dump(&db.conn);
    let keep = |l: &&str| l.starts_with("book_postings") || l.starts_with("entries");
    all.lines().filter(keep).collect::<Vec<_>>().join("\n")
}

struct Dir(PathBuf);
impl Dir {
    fn new(name: &str) -> Self {
        let d = std::env::temp_dir().join(format!("cashcraft-rules-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// How formats 0 to 7 matched, word for word. Never change this text.
fn old_match<'a>(
    rules: &'a [(i64, String, i64, String)],
    payee: &str,
    memo: &str,
) -> Option<&'a (i64, String, i64, String)> {
    let hay = format!(" {} {} ", payee.to_lowercase(), memo.to_lowercase());
    for r in rules {
        let hit = r.1.to_lowercase().split('|').filter(|k| !k.trim().is_empty()).any(|k| hay.contains(k));
        if hit {
            return Some(r);
        }
    }
    None
}

/// Keyword rules of a file of format 7, with their quirks, match as they did: same rule, category, payee and notes.
#[test]
fn rules_of_format_7_work_as_before() {
    let pieces = [
        "bar", "bar ", " bar", "Gas ", "İ", "ẞ", "K", "σς", "ss", "|", "||", " ", "  ", ".", "(", "[a", "$1", "\\d",
        "*", "caffe", "CAFFÈ", "1", "x y", "r m", "'", "%", "_", "ﬁ", "é", "-", "\t",
    ];
    let mut r = Lcg(7);
    let some = |r: &mut Lcg, n: usize| -> String { (0..r.below(n)).map(|_| pieces[r.below(pieces.len())]).collect() };
    let (mut hits, mut renames) = (0, 0);
    for file in 0..12 {
        let dir = Dir::new(&format!("v7-{file}"));
        let path = dir.0.join("books.db");
        let c = Connection::open(&path).unwrap();
        c.execute_batch(include_str!("../migrate/v7.sql")).unwrap();
        c.execute_batch(
            "INSERT INTO accounts(id,name,kind) VALUES (1,'Bank','bank');
             INSERT INTO categories(id,name) VALUES (1,'A'), (2,'B'), (3,'C');",
        )
        .unwrap();
        let mut old = Vec::new();
        for _ in 0..1 + r.below(12) {
            // As it was saved: trimmed, never blank.
            let pattern = some(&mut r, 6).trim().to_string();
            if pattern.is_empty() {
                continue;
            }
            let rename = if r.below(2) == 0 { some(&mut r, 3).trim().to_string() } else { String::new() };
            let category = 1 + r.below(3) as i64;
            // Ids with holes, and not in the order of the rows.
            let id = r.below(10_000) as i64 + 1;
            if c.execute("INSERT INTO category_rules VALUES(?1,?2,?3,?4)", params![id, pattern, category, rename])
                .is_ok()
            {
                old.push((id, pattern, category, rename));
            }
        }
        old.sort();
        c.pragma_update(None, "user_version", 7).unwrap();
        drop(c);
        let (db, _) = Db::open(&path).unwrap();
        assert!(dir.0.join("books.v7-backup.db").exists());
        let now: Vec<_> = db.rules().unwrap();
        assert!(now.iter().all(|r| r.problem.is_none()), "{now:?}");
        let now: Vec<_> = now.into_iter().map(|r| (r.id, r.pattern, r.category_id.unwrap(), r.rename_payee)).collect();
        assert_eq!(now, old);
        let set = db.ruleset().unwrap();
        let mut texts = Vec::new();
        for _ in 0..4000 {
            let (payee, memo) = (some(&mut r, 6), some(&mut r, 6));
            let o = set.evaluate(&Subject::new(&payee, &memo, r.amount(), Some(1)));
            let hit = old_match(&old, &payee, &memo);
            assert_eq!(o.category, hit.map(|h| (h.0, h.2)), "{payee:?} {memo:?} {old:?}");
            assert_eq!(o.payee, hit.filter(|h| !h.3.is_empty()).map(|h| (h.0, h.3.clone())));
            assert!(o.tags.is_empty());
            hits += usize::from(hit.is_some());
            if texts.len() < 60 {
                texts.push((payee, memo));
            }
        }
        // The same when a transaction is added, and on demand on one that is there.
        for (payee, memo) in texts {
            let new = add(&db, 1, &payee, &memo, -5);
            let later = add(&db, 1, "", "", -5);
            let edit =
                TxEdit { date: day(4), amount: -5, payee: payee.clone(), memo: memo.clone(), ..Default::default() };
            db.update_transaction(later, &edit).unwrap();
            // The payee was typed: a run leaves it, unless the user asks.
            db.apply_rules().unwrap();
            let t = db.transaction(later).unwrap();
            assert_eq!((t.payee.as_str(), t.memo.as_str()), (payee.trim(), memo.trim()));
            db.run_rules(Scope { all: true, overwrite: true }, None).unwrap();
            let hit = old_match(&old, &payee, &memo);
            let rename = hit.map(|h| h.3.clone()).filter(|n| !n.is_empty());
            renames += usize::from(rename.is_some());
            let t = db.transaction(new).unwrap();
            let (p, m) = match &rename {
                Some(n) => (n.clone(), if memo.is_empty() { payee.clone() } else { memo.clone() }),
                None => (payee.clone(), memo.clone()),
            };
            let shown = (p.trim().to_string(), m.trim().to_string());
            assert_eq!((t.category_id, t.payee, t.memo), (hit.map(|h| h.2), shown.0, shown.1));
            // There the texts were trimmed first.
            let (payee, memo) = (payee.trim(), memo.trim());
            let hit = old_match(&old, payee, memo);
            let t = db.transaction(later).unwrap();
            assert_eq!(t.category_id, hit.map(|h| h.2), "{payee:?} {memo:?}");
            if let Some(h) = hit.filter(|h| !h.3.is_empty()) {
                let m = if memo.is_empty() { payee } else { memo };
                assert_eq!((t.payee.as_str(), t.memo.as_str()), (h.3.as_str(), m));
            }
        }
        assert_sound(&db, "rules of format 7");
    }
    assert!(hits > 10_000 && renames > 50, "{hits} {renames}");
}

#[test]
fn every_condition_must_be_met() {
    let (db, a) = books();
    let other = db.add_account("Other", AccountKind::Bank, "EUR", 0, "").unwrap();
    let c = Some(cat(&db, "Groceries"));
    let base = || Rule { category_id: c, ..Default::default() };
    let hit = |r: &Rule, payee: &str, memo: &str, amount: Cents| {
        let compiled = Compiled::new(&db.checked_rule(r).unwrap()).unwrap();
        compiled.matches(&Subject::new(payee, memo, amount, Some(a)))
    };
    let r = Rule { payee: "bar |mercato".into(), ..base() };
    assert!(hit(&r, "Il MERCATO", "", -1) && hit(&r, "Bar", "", -1) && !hit(&r, "Barba", "", -1));
    assert!(!hit(&r, "x", "mercato", -1));
    let r = Rule { memo: "spesa".into(), ..base() };
    assert!(hit(&r, "x", "La spesa", -1) && !hit(&r, "spesa", "", -1));
    // One of the two with a pattern of both.
    let r = Rule { pattern: r"^pos \d+$".into(), regex: true, ..base() };
    assert!(hit(&r, "POS 12", "x", -1) && hit(&r, "x", "pos 3", -1) && !hit(&r, "pos", "12", -1));
    let r = Rule { payee: "^a".into(), memo: "z$".into(), regex: true, ..base() };
    assert!(hit(&r, "Abc", "xyZ", -1) && !hit(&r, "Abc", "zx", -1) && !hit(&r, "bA", "z", -1));
    let r = Rule { payee: "(?-i)^A".into(), regex: true, ..base() };
    assert!(hit(&r, "A", "", -1) && !hit(&r, "a", "", -1));
    let r = Rule { amount_min: Some(100), amount_max: Some(200), ..base() };
    for (amount, met) in [(99, false), (100, true), (-100, true), (-200, true), (201, false), (Cents::MIN, false)] {
        assert_eq!(hit(&r, "", "", amount), met, "{amount}");
    }
    let r = Rule { sign: Sign::Out, ..base() };
    assert!(hit(&r, "", "", -1) && !hit(&r, "", "", 1) && !hit(&r, "", "", 0));
    let r = Rule { sign: Sign::In, ..base() };
    assert!(hit(&r, "", "", 1) && !hit(&r, "", "", -1));
    assert!(hit(&Rule { account_id: Some(a), ..base() }, "", "", 1));
    assert!(!hit(&Rule { account_id: Some(other), ..base() }, "", "", 1));
    // A whole word by its spaces.
    let r = Rule { payee: "x|bar |y".into(), ..base() };
    assert!(hit(&r, "Bar", "", 1) && hit(&r, "bar Roma", "", 1) && !hit(&r, "Barba", "", 1));
}

#[test]
fn what_cannot_work_is_refused_when_it_is_saved() {
    let (db, a) = books();
    let c = Some(cat(&db, "Groceries"));
    let before = dump(&db.conn);
    let long = "x".repeat(MAX_REGEX + 1);
    for bad in [
        Rule::default(),
        Rule { category_id: c, ..Default::default() },
        words("x", None),
        words(" ", c),
        words("| |", c),
        words("x", Some(999)),
        Rule { account_id: Some(99), ..words("x", c) },
        Rule { amount_min: Some(-1), ..words("x", c) },
        Rule { amount_min: Some(5), amount_max: Some(4), ..words("x", c) },
        Rule { amount_max: Some(Cents::MAX), ..words("x", c) },
        Rule { tags: vec!["t".repeat(41)], ..words("x", c) },
        Rule { tags: vec!["a\u{7}".into()], ..words("x", c) },
        Rule { tags: (0..21).map(|i| format!("t{i}")).collect(), ..words("x", c) },
        Rule { rename_payee: "n".repeat(201), ..words("x", c) },
        words(&"w".repeat(MAX_WORDS + 1), c),
        re("(", "x"),
        re("[z-a]", "x"),
        re("(a{999}){999}", "x"),
        re(r"\w{900}", "x"),
        re(&"(".repeat(400), "x"),
        re(&format!("{}a{}", "(".repeat(60), ")".repeat(60)), "x"),
        re(&long, "x"),
        re("(a)", "$2"),
        Rule { regex: false, ..re("a", "$1") },
        Rule { regex: false, ..re("a", "x ${shop}") },
        re("(a)", "${name}"),
        re("(a)", "$"),
        re("(a)", "$1a"),
        re("(a)", "${1"),
        re("(?=a)", "x"),
    ] {
        let e = db.insert_rule(&bad, false);
        assert!(e.is_err(), "{bad:?}");
        assert_eq!(dump(&db.conn), before);
    }
    let e = db.insert_rule(&re("a(b", "x"), false).unwrap_err().to_string();
    assert!(e.contains("not valid") && e.contains("unclosed group"), "{e}");
    let id = db.insert_rule(&words("x", c), false).unwrap();
    assert!(db.update_rule(&Rule { id, regex: true, ..words("(", c) }).is_err());
    assert!(db.update_rule(&Rule { id: 99, ..words("x", c) }).is_err() && db.move_rule(99, Place::Up).is_err());
    let named = db.insert_rule(&re("(a)", "$1"), false).unwrap();
    assert!(db.update_rule(&Rule { id: named, regex: false, ..re("a", "$1") }).is_err());
    assert_eq!(db.rule(id).unwrap().payee, "x");
    // A pattern that would take for ever elsewhere.
    db.insert_rule(&re("^(a+)+$", "x"), false).unwrap();
    let start = std::time::Instant::now();
    add(&db, a, &format!("{}!", "a".repeat(100_000)), "", -1);
    assert!(start.elapsed().as_secs() < 5);
    db.conn.execute("DELETE FROM transaction_rules", []).unwrap();
    for _ in 0..MAX_RULES {
        db.conn.execute("INSERT INTO transaction_rules(position, pattern, category_id) VALUES(1,'x',?1)", [c]).unwrap();
    }
    assert!(db.insert_rule(&words("x", c), false).is_err());
}

#[test]
fn a_damaged_rule_is_reported_and_the_others_work() {
    let (db, a) = books();
    let c = cat(&db, "Groceries");
    db.conn
        .execute_batch(
            "PRAGMA foreign_keys=OFF;
             INSERT INTO transaction_rules(position,pattern,is_regex,category_id) VALUES (1,'(',1,1), (2,'(a{999}){999}',1,1);
             INSERT INTO transaction_rules(position,pattern,category_id) VALUES (3,x'ff',1), (4,'shop',9999), (5,'',1);
             INSERT INTO transaction_rules(position,pattern,sign,category_id) VALUES (6,'shop','sideways',1);
             INSERT INTO transaction_rules(position,pattern,amount_min,category_id) VALUES (7,'shop','much',1);
             INSERT INTO transaction_rules(position,pattern,is_regex,rename_payee) VALUES (8,'shop',1,'$7');
             INSERT INTO transaction_rules(position,pattern,account_id,category_id) VALUES (9,'shop',77,1);
             INSERT INTO transaction_rules(position,pattern,category_id,tags) VALUES ('first','shop',1,x'09');",
        )
        .unwrap();
    db.insert_rule(&words("shop", Some(c)), false).unwrap();
    let rules = db.rules().unwrap();
    assert_eq!(rules.iter().filter(|r| r.problem.is_some()).count(), 10, "{rules:?}");
    let res = import(&db, a, &csv("Data;Descrizione;Importo\n05/03/2026;Shop;-1,20\n"));
    assert_eq!((res.added, res.categorized, res.broken_rules.len()), (1, 1, 10));
    let plan = db.plan_rules(Scope { all: true, overwrite: true }).unwrap();
    assert_eq!(plan.broken.len(), 10);
    assert!(broken_message(&plan.broken).contains("not valid"));
    let t = db.transactions(&TxFilter::default()).unwrap().remove(0);
    assert_eq!(t.category_id, Some(c));
    // Marks that cannot be read mark nothing.
    db.conn.execute("UPDATE book_postings SET rules='{', tags=',,a,' WHERE id=?1", [t.id]).unwrap();
    let t = db.transaction(t.id).unwrap();
    assert!(t.rules.is_empty() && t.tags == ["a"]);
    let id = rules.last().unwrap().id;
    db.move_rule(id, Place::First).unwrap();
    assert_eq!(db.rules().unwrap()[0].id, id);
    for r in rules {
        db.delete_rule(r.id).unwrap();
    }
    assert!(db.rules().unwrap().is_empty());
}

#[test]
fn the_first_rule_with_a_category_decides() {
    let (db, a) = books();
    let (x, y) = (cat(&db, "X"), cat(&db, "Y"));
    let clean =
        db.insert_rule(&Rule { tags: vec!["one".into()], ..re(r"^pos \d+ (?<shop>.+)$", "${shop}") }, false).unwrap();
    let first = db.insert_rule(&Rule { rename_payee: "First".into(), ..words("bar", Some(x)) }, false).unwrap();
    let second =
        Rule { rename_payee: "Second".into(), tags: vec!["ONE".into(), "two".into()], ..words("bar", Some(y)) };
    let second = db.insert_rule(&second, false).unwrap();
    let last = db.insert_rule(&Rule { rename_payee: "Never".into(), ..words("bar", None) }, true).unwrap();
    let order = || db.rules().unwrap().iter().map(|r| r.id).collect::<Vec<_>>();
    assert_eq!(order(), [last, clean, first, second]);
    for (to, expected) in [
        (Place::Down, [clean, last, first, second]),
        (Place::Last, [clean, first, second, last]),
        (Place::Down, [clean, first, second, last]),
        (Place::Up, [clean, first, last, second]),
        (Place::First, [last, clean, first, second]),
        (Place::Up, [last, clean, first, second]),
        (Place::Last, [clean, first, second, last]),
    ] {
        db.move_rule(last, to).unwrap();
        assert_eq!(order(), expected);
    }
    let t = db.transaction(add(&db, a, "POS 12 Bar Roma", "", -5)).unwrap();
    assert_eq!((t.category.as_str(), t.payee.as_str(), t.memo.as_str()), ("X", "Bar Roma", "POS 12 Bar Roma"));
    assert_eq!(t.tags, ["one", "two"]);
    assert_eq!(
        (t.rules.category, t.rules.payee, t.rules.ids()),
        (Some(first), Some(clean), vec![clean, first, second])
    );
    assert_eq!((t.original_payee, t.typed_payee.as_deref()), (None, Some("POS 12 Bar Roma")));
    // A replacement that comes out empty leaves the payee, and one is never longer than it may be.
    db.insert_rule(&re("^zz(y?)", "$1"), true).unwrap();
    db.insert_rule(&re("^(w+)", "$1$1$1 $$"), true).unwrap();
    assert_eq!(db.transaction(add(&db, a, "zz top", "", -5)).unwrap().payee, "zz top");
    let long = db.transaction(add(&db, a, &"w".repeat(90), "", -5)).unwrap().payee;
    assert_eq!(long.chars().count(), MAX_REPLACEMENT);
    db.insert_rule(&re("^(q+)", "$1 $$"), true).unwrap();
    assert_eq!(db.transaction(add(&db, a, "qq", "", -5)).unwrap().payee, "qq $");
    // Another order, another rule.
    db.move_rule(second, Place::First).unwrap();
    assert_eq!(db.transaction(add(&db, a, "Bar", "", -5)).unwrap().category, "Y");
    // A category chosen by hand is not touched, and no rule runs.
    let tx = NewTx {
        account_id: a,
        date: day(1),
        payee: "Bar".into(),
        category_id: Some(x),
        amount: -1,
        ..Default::default()
    };
    let t = db.transaction(db.add_transaction(&tx).unwrap().unwrap()).unwrap();
    assert!(t.category == "X" && t.rules.is_empty() && t.tags.is_empty() && t.payee == "Bar");
}

/// The keys of an import come from the file alone, whatever the rules are and do.
#[test]
fn importing_again_after_a_clean_up_adds_nothing() {
    let (db, bank) = books();
    let card = db.add_account("Card", AccountKind::CreditCard, "EUR", 0, "").unwrap();
    let sheet = db.add_account("Sheet", AccountKind::Bank, "EUR", 0, "").unwrap();
    let money = db.add_account("Ofx", AccountKind::Bank, "EUR", 0, "").unwrap();
    // Without FITID the key has the payee.
    let bare = db.add_account("Bare", AccountKind::Bank, "EUR", 0, "").unwrap();
    let no_ids = ofx::tests::sgml().replace("<FITID>A1", "").replace("<FITID>A2", "");
    let files: Vec<(i64, Table)> = vec![
        (
            bank,
            csv(
                "Data;Descrizione;Importo\n05/03/2026;POS 1 Bar;-1,20\n05/03/2026;POS 1 Bar;-1,20\n06/03/2026;Altro;9,00\n",
            ),
        ),
        (sheet, importer::tests::load_bytes(&importer::tests::bper()).unwrap()),
        (card, card_pdf::tests::table(&card_pdf::tests::statement())),
        (money, importer::ofx_table(ofx::read(ofx::tests::sgml().as_bytes()).unwrap().0, "UTF-8")),
        (bare, importer::ofx_table(ofx::read(no_ids.as_bytes()).unwrap().0, "UTF-8")),
    ];
    let keys = |db: &Db| -> Vec<(i64, Option<String>, Option<String>)> {
        let mut st = db.conn.prepare("SELECT id, import_hash, other_hashes FROM book_postings ORDER BY id").unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
    };
    let again = |db: &Db, what: &str| {
        let before = dump(&db.conn);
        for (account, t) in &files {
            let (rows, skipped) = importer::parse_rows(t, &t.mapping, false);
            let res = importer::import(db, *account, t, &rows, skipped, false).unwrap();
            assert_eq!((res.added, res.duplicates), (0, rows.len()), "{what} {:?}", t.format());
        }
        assert_eq!(dump(&db.conn), before, "{what}");
    };
    let mut added = 0;
    for (account, t) in &files {
        let res = import(&db, *account, t);
        assert!(res.added > 1 && res.categorized == 0, "{:?}", t.format());
        added += res.added;
    }
    let plain = keys(&db);
    again(&db, "no rules");
    // A rule that gives everything another payee, tags and a category.
    let c = cat(&db, "Everything");
    let all = Rule { category_id: Some(c), tags: vec!["seen".into()], ..re("^(.).*$", "$1!") };
    db.insert_rule(&all, true).unwrap();
    again(&db, "a rule");
    assert_eq!(db.apply_rules().unwrap(), added);
    assert!(db.transactions(&TxFilter::default()).unwrap().iter().all(|t| t.payee.ends_with('!') && t.payee.len() < 6));
    assert_eq!(keys(&db), plain);
    again(&db, "applied");
    // Done again it changes nothing: a payee is not cleaned up twice.
    assert_eq!(db.run_rules(Scope { all: true, overwrite: true }, None).unwrap().changed, 0);
    db.undo_rules().unwrap();
    again(&db, "undone");
    // Imported for the first time with the rule, the keys are the same.
    let (fresh, _) = books();
    for (name, kind) in [
        ("Card", AccountKind::CreditCard),
        ("Sheet", AccountKind::Bank),
        ("Ofx", AccountKind::Bank),
        ("Bare", AccountKind::Bank),
    ] {
        fresh.add_account(name, kind, "EUR", 0, "").unwrap();
    }
    fresh.insert_rule(&Rule { category_id: Some(cat(&fresh, "Everything")), ..all }, true).unwrap();
    for (account, t) in &files {
        assert!(import(&fresh, *account, t).categorized > 1);
    }
    let hashes = |k: Vec<(i64, Option<String>, Option<String>)>| k.into_iter().map(|x| (x.1, x.2)).collect::<Vec<_>>();
    assert_eq!(hashes(keys(&fresh)), hashes(plain));
    again(&fresh, "with the rule from the start");
    assert_sound(&fresh, "imports");
}

/// Likely duplicates and the sides of transfers are found on what the bank wrote.
#[test]
fn duplicates_and_transfers_are_found_after_a_clean_up() {
    let (db, a) = books();
    let b = db.add_account("Savings", AccountKind::Savings, "EUR", 0, "").unwrap();
    db.insert_rule(&Rule { rename_payee: "Cosy".into(), ..words("bar|moved", Some(cat(&db, "X"))) }, false).unwrap();
    let memo = "Data;Descrizione;Note;Importo\n05/01/2026;BAR & CAFFE;un caffè;-12,50\n";
    let t = csv(memo);
    assert_eq!(import(&db, a, &t).categorized, 1);
    let first = db.transactions(&TxFilter::default()).unwrap().remove(0);
    assert_eq!(first.payee, "Cosy");
    let file = importer::ofx_table(ofx::read(ofx::tests::sgml().as_bytes()).unwrap().0, "UTF-8");
    let (rows, _) = importer::parse_rows(&file, &file.mapping, false);
    let likely = db.likely_duplicates(Some(a), &rows).unwrap();
    assert_eq!((likely.len(), likely[0].certain, likely[0].proposed), (1, true, Some(first.id)));
    // Also when the user wrote another payee.
    let edit = TxEdit {
        date: first.date,
        amount: first.amount,
        payee: "Mine".into(),
        memo: "Mine".into(),
        category_id: None,
        cleared: true,
    };
    db.update_transaction(first.id, &edit).unwrap();
    assert!(db.likely_duplicates(Some(a), &rows).unwrap()[0].certain);
    // A line that is the side of a transfer takes its place, keeps its text and gets no category.
    let (_, into) = db.add_transfer(a, b, NaiveDate::from_ymd_opt(2026, 3, 4).unwrap(), 5000, "").unwrap();
    let moved = csv("Data;Descrizione;Importo\n05/03/2026;Moved;50,00\n");
    let res = import(&db, b, &moved);
    assert_eq!((res.added, res.adopted.len()), (0, 1));
    let side = db.transaction(into).unwrap();
    assert_eq!((side.payee.as_str(), side.kind), ("Moved", TxKind::Transfer));
    assert_eq!(db.run_rules(Scope { all: true, overwrite: true }, None).unwrap().changed, 1);
    assert_eq!(db.transaction(into).unwrap().payee, "Moved");
    // A cleaned up line can become a transfer, and one that is joined keeps what the bank wrote and its tags.
    let one = add(&db, a, "moved out", "", -700);
    let two = add(&db, b, "moved in", "", 700);
    db.set_tags(two, &["kept".into()]).unwrap();
    assert_eq!(db.transfer_candidates(one, b).unwrap().len(), 1);
    db.turn_into_transfer(one, b, OtherSide::Join(two)).unwrap();
    let t = db.transaction(two).unwrap();
    assert_eq!((t.payee.as_str(), t.typed_payee.as_deref(), t.tags.len()), ("Cosy", Some("moved in"), 1));
    assert!(t.rules.category.is_none());
    db.separate_transfer(one, true).unwrap();
    assert_eq!(db.transaction(two).unwrap().tags, ["kept"]);
    assert_eq!(db.apply_rules().unwrap(), 2);
    assert_sound(&db, "transfers");
}

#[test]
fn what_the_user_set_is_kept_unless_asked() {
    let (db, a) = books();
    let (x, y, z) = (cat(&db, "X"), cat(&db, "Y"), cat(&db, "Z"));
    let t = csv(
        "Data;Descrizione;Importo\n01/03/2026;shop 1;-1\n02/03/2026;shop 2;-1\n03/03/2026;shop 3;-1\n04/03/2026;shop 4;-1\n05/03/2026;shop 5;-1\n",
    );
    import(&db, a, &t);
    let mut ids: Vec<i64> = db.transactions(&TxFilter::default()).unwrap().iter().map(|t| t.id).collect();
    ids.sort_unstable();
    let [by_hand, split, side, written, ruled] = ids[..] else { panic!("{ids:?}") };
    let b = db.add_account("B", AccountKind::Cash, "EUR", 0, "").unwrap();
    db.set_category(by_hand, Some(z)).unwrap();
    db.turn_into_transfer(side, b, OtherSide::Auto).unwrap();
    let entry = db.transaction(split).unwrap().entry_id;
    let mut d = db.load_draft(entry).unwrap();
    d.lines[1].amount = 40;
    d.lines.push(Line { account: d.lines[1].account, amount: 60, currency: "EUR".into(), ..Default::default() });
    db.save_entry(&d).unwrap();
    let w = db.transaction(written).unwrap();
    let edit = TxEdit { date: w.date, amount: w.amount, payee: "My shop".into(), ..Default::default() };
    db.update_transaction(written, &edit).unwrap();
    let rule = Rule { tags: vec!["t".into()], rename_payee: "Shop".into(), ..words("shop", Some(x)) };
    let id = db.insert_rule(&rule, false).unwrap();
    let before = postings(&db);
    let plan = db.plan_rules(Scope::default()).unwrap();
    assert_eq!(postings(&db), before, "a plan changes nothing");
    let mut planned: Vec<i64> = plan.changes.iter().map(|c| c.id).collect();
    planned.sort_unstable();
    assert_eq!(planned, [written, ruled]);
    assert_eq!(db.run_rules(Scope::default(), Some(&plan.token)).unwrap().categorized, 2);
    let get = |id| db.transaction(id).unwrap();
    assert_eq!((get(written).payee.as_str(), get(written).category.as_str()), ("My shop", "X"));
    assert_eq!((get(ruled).payee.as_str(), get(ruled).rules.category), ("Shop", Some(id)));
    assert!(get(split).kind == TxKind::Split && get(split).tags.is_empty() && get(side).tags.is_empty());
    // The token of other changes.
    assert!(db.run_rules(Scope { all: true, overwrite: false }, Some(&plan.token)).is_err());
    // With all: what rules set follows them, what was set by hand does not; a removed tag does not come back.
    db.set_tags(ruled, &[]).unwrap();
    db.update_rule(&Rule { id, category_id: Some(y), ..rule.clone() }).unwrap();
    db.run_rules(Scope { all: true, overwrite: false }, None).unwrap();
    assert_eq!((get(ruled).category.as_str(), get(ruled).tags.len()), ("Y", 0));
    assert_eq!((get(by_hand).category.as_str(), get(by_hand).payee.as_str()), ("Z", "Shop"));
    assert_eq!((get(by_hand).tags, get(by_hand).rules.category), (vec!["t".to_string()], None));
    // By hand again, also to the same category.
    db.set_category(ruled, Some(y)).unwrap();
    assert_eq!(get(ruled).rules.category, None);
    // Other changes do not make it by hand.
    let w = get(written);
    let edit = TxEdit {
        date: w.date,
        amount: -2,
        payee: w.payee,
        memo: "n".into(),
        category_id: w.category_id,
        cleared: true,
    };
    db.update_transaction(written, &edit).unwrap();
    assert_eq!(get(written).rules.category, Some(id));
    db.update_rule(&Rule { id, category_id: Some(x), ..rule }).unwrap();
    db.run_rules(Scope { all: true, overwrite: false }, None).unwrap();
    assert_eq!((get(ruled).category.as_str(), get(written).category.as_str()), ("Y", "X"));
    db.run_rules(Scope { all: true, overwrite: true }, None).unwrap();
    for id in [by_hand, written, ruled] {
        assert_eq!((get(id).category.as_str(), get(id).payee.as_str(), get(id).tags.len()), ("X", "Shop", 1));
    }
    // The books agree with the marks whoever changes the category.
    let other = db.entry(get(ruled).entry_id).unwrap().postings[1].id;
    db.set_posting_category(other, Some(y)).unwrap();
    assert_eq!(get(ruled).rules.category, None);
    db.delete_category(x).unwrap();
    assert!(get(written).category_id.is_none() && get(written).rules.category.is_none());
    assert!(db.rules().unwrap().is_empty(), "the rule goes with its category");
    assert_sound(&db, "kept");
}

#[test]
fn a_run_is_undone_in_one_step() {
    let dir = Dir::new("undo");
    let path = dir.0.join("books.db");
    let (db, _) = Db::open(&path).unwrap();
    db.conn.execute("DELETE FROM transaction_rules", []).unwrap();
    let a = db.add_account("Bank", AccountKind::Bank, "EUR", 0, "").unwrap();
    let (x, z) = (cat(&db, "X"), cat(&db, "Z"));
    let typed: Vec<i64> = (0..6).map(|i| add(&db, a, &format!("shop {i}"), "", -100 - i)).collect();
    import(&db, a, &csv("Data;Descrizione;Importo\n01/03/2026;shop A;-1\n02/03/2026;shop B;-1\n"));
    db.set_category(typed[0], Some(z)).unwrap();
    db.set_tags(typed[1], &["old".into()]).unwrap();
    assert!(db.undo_rules().is_err() && db.last_rule_run().unwrap().is_none());
    let before = postings(&db);
    let rule = Rule { tags: vec!["t".into()], ..re("^(shop) (.)", "$2 $1") };
    db.insert_rule(&Rule { category_id: Some(x), ..rule }, false).unwrap();
    let all = Scope { all: true, overwrite: true };
    assert_eq!(db.run_rules(all, None).unwrap().changed, 8);
    let after = postings(&db);
    assert_ne!(after, before);
    assert_eq!(db.last_rule_run().unwrap().unwrap().changed, 8);
    // A run that changes nothing is not one.
    assert_eq!(db.run_rules(all, None).unwrap().changed, 0);
    drop(db);
    // By another program, later.
    let (db, _) = Db::open(&path).unwrap();
    let done = db.undo_rules().unwrap();
    assert_eq!((done.undone, done.skipped.len()), (8, 0));
    assert_eq!(postings(&db), before);
    assert!(db.undo_rules().is_err());
    assert_sound(&db, "undone");

    // What was changed since stays, and is told.
    db.run_rules(all, None).unwrap();
    db.set_category(typed[1], Some(z)).unwrap();
    db.delete_transaction(typed[2]).unwrap();
    db.set_tags(typed[3], &["mine".into()]).unwrap();
    db.set_cleared(typed[4], true).unwrap();
    let kept = [typed[1], typed[3]].map(|id| format!("{:?}", db.transaction(id).unwrap()));
    let done = db.undo_rules().unwrap();
    assert_eq!(done.undone, 5);
    assert_eq!(done.skipped.iter().map(|s| s.id).collect::<Vec<_>>(), [typed[1], typed[2], typed[3]]);
    assert!(done.message().contains("1 shop") && done.message().contains("deleted"), "{}", done.message());
    assert_eq!([typed[1], typed[3]].map(|id| format!("{:?}", db.transaction(id).unwrap())), kept);
    let t = db.transaction(typed[4]).unwrap();
    assert!(t.cleared && t.payee == "shop 4" && t.category_id.is_none() && t.tags.is_empty());
    assert_eq!(db.transaction(typed[0]).unwrap().category, "Z");
    assert_sound(&db, "undone in part");

    // All changed: refused, and nothing is done.
    db.run_rules(all, None).unwrap();
    for t in db.transactions(&TxFilter::default()).unwrap() {
        db.set_tags(t.id, &[]).unwrap();
    }
    let before = dump(&db.conn);
    let e = db.undo_rules().unwrap_err().to_string();
    assert!(e.contains("nothing was undone") && e.contains("shop"), "{e}");
    assert_eq!(dump(&db.conn), before);

    // A category that went is not replaced by another with its number, nor a transaction by a new one.
    db.conn.execute("DELETE FROM rule_runs", []).unwrap();
    for t in db.transactions(&TxFilter::default()).unwrap() {
        db.delete_transaction(t.id).unwrap();
    }
    let (one, two) = (add(&db, a, "shop Q", "", -5), add(&db, a, "other", "", -5));
    let gone = cat(&db, "Gone");
    db.set_category(one, Some(gone)).unwrap();
    db.insert_rule(&words("other", Some(z)), false).unwrap();
    assert_eq!(db.run_rules(all, None).unwrap().changed, 2);
    db.delete_category(gone).unwrap();
    cat(&db, "Newcomer");
    db.delete_transaction(two).unwrap();
    let again = add(&db, a, "other", "", -5);
    assert_eq!(again, two, "the number is used again");
    let e = db.undo_rules().unwrap_err().to_string();
    assert!(e.contains("category") && e.contains("changed since"), "{e}");
    assert_eq!(db.transaction(one).unwrap().category, "X");
    // Damaged.
    db.conn.execute("UPDATE rule_changes SET after_state='{'", []).unwrap();
    assert!(db.undo_rules().is_err());
    db.conn.execute("UPDATE rule_changes SET after_state=x'ff'", []).unwrap();
    assert!(db.undo_rules().is_err());
    assert_sound(&db, "refused");
}

/// All of it or nothing, for the run and for the undo.
#[test]
fn a_run_that_fails_halfway_changes_nothing() {
    let (db, a) = books();
    let x = cat(&db, "X");
    for i in 0..5 {
        add(&db, a, &format!("shop {i}"), "", -100);
    }
    db.insert_rule(&words("shop", Some(x)), false).unwrap();
    let stop =
        "CREATE TRIGGER stop BEFORE UPDATE ON book_postings WHEN NEW.id=7 BEGIN SELECT RAISE(ABORT,'stop'); END;";
    db.conn.execute_batch(stop).unwrap();
    let before = dump(&db.conn);
    assert!(db.apply_rules().is_err());
    assert_eq!(dump(&db.conn), before);
    db.conn.execute_batch("DROP TRIGGER stop").unwrap();
    assert_eq!(db.apply_rules().unwrap(), 5);
    db.conn.execute_batch(stop).unwrap();
    let before = dump(&db.conn);
    assert!(db.undo_rules().is_err());
    assert_eq!(dump(&db.conn), before);
    db.conn.execute_batch("DROP TRIGGER stop").unwrap();
    assert_eq!(db.undo_rules().unwrap().undone, 5);
}

/// "Always categorise like this": the rule is for what the bank writes, and comes before the others.
#[test]
fn a_rule_from_a_transaction_applies_to_later_imports() {
    let (db, a) = books();
    let (x, y) = (cat(&db, "X"), cat(&db, "Y"));
    db.insert_rule(&words("market", Some(x)), false).unwrap();
    let t = csv("Data;Descrizione;Importo\n01/03/2026;MARKET|BAZAAR $5 (12);-1\n02/03/2026;Market;-1\n");
    import(&db, a, &t);
    let all = db.transactions(&TxFilter::default()).unwrap();
    let t = all.iter().find(|t| t.payee.contains("BAZAAR")).unwrap();
    db.set_category(t.id, Some(y)).unwrap();
    db.set_tags(t.id, &["fun".into()]).unwrap();
    let edit = TxEdit {
        date: t.date,
        amount: t.amount,
        payee: "Big $ Bazaar".into(),
        memo: t.memo.clone(),
        category_id: Some(y),
        cleared: true,
    };
    db.update_transaction(t.id, &edit).unwrap();
    let rule = db.rule_from(t.id).unwrap();
    assert_eq!((rule.category_id, rule.regex, rule.tags.len()), (Some(y), true, 1));
    let all = db.transactions(&TxFilter::default()).unwrap();
    assert_eq!(db.rule_matches(&rule, &all).unwrap(), [t.id]);
    assert_eq!(db.rule_matches(&words("market", Some(y)), &all).unwrap().len(), 2);
    assert!(db.rule_matches(&Rule { payee: "(".into(), ..rule.clone() }, &all).is_err());
    db.insert_rule(&rule, true).unwrap();
    import(&db, a, &csv("Data;Descrizione;Importo\n09/03/2026;MARKET|BAZAAR $5 (12);-7\n"));
    let new = db.transactions(&TxFilter { limit: Some(1), ..Default::default() }).unwrap().remove(0);
    assert_eq!((new.category.as_str(), new.payee.as_str(), new.tags), ("Y", "Big $ Bazaar", vec!["fun".to_string()]));
    // Only the notes, and what rules do not categorize.
    let bare = add(&db, a, "", "Only notes", -5);
    assert_eq!(db.rule_from(bare).unwrap().memo, "Only notes");
    let b = db.add_account("B", AccountKind::Cash, "EUR", 0, "").unwrap();
    let (out, _) = db.add_transfer(a, b, day(1), 5, "").unwrap();
    assert!(db.rule_from(out).is_err() && db.rule_from(9999).is_err());
    let long = add(&db, a, &format!("{}|", "(".repeat(5000)), "", -5);
    db.insert_rule(&Rule { category_id: Some(x), ..db.rule_from(long).unwrap() }, true).unwrap();
    // A rule of an account goes with it, instead of matching everywhere.
    db.insert_rule(&Rule { account_id: Some(b), ..words("x", Some(x)) }, true).unwrap();
    let n = db.rules().unwrap().len();
    db.delete_account(b).unwrap();
    assert_eq!(db.rules().unwrap().len(), n - 1);
}

#[test]
fn tags_are_searched_listed_and_kept() {
    let (db, a) = books();
    let (one, two) = (add(&db, a, "Hotel", "", -5), add(&db, a, "Train", "", -5));
    db.set_tags(one, &["#Summer  trip".into(), "summer trip".into(), " work ".into()]).unwrap();
    db.set_tags(two, &["work".into()]).unwrap();
    assert_eq!(db.transaction(one).unwrap().tags, ["Summer trip", "work"]);
    assert_eq!(db.tags().unwrap(), [("Summer trip".to_string(), 1), ("work".to_string(), 2)]);
    let found = |f: TxFilter| db.transactions(&f).unwrap().iter().map(|t| t.id).collect::<Vec<_>>();
    let search = |s: &str| found(TxFilter { search: s.into(), ..Default::default() });
    assert_eq!((search("summer"), search("#work"), search("#wor x")), (vec![one], vec![two, one], vec![]));
    let tag = |t: &str| found(TxFilter { tag: Some(t.into()), ..Default::default() });
    assert_eq!((tag("WORK"), tag("summer"), tag("summer TRIP")), (vec![two, one], vec![], vec![one]));
    db.set_category(one, Some(cat(&db, "X"))).unwrap();
    let t = db.transaction(one).unwrap();
    let edit = TxEdit { date: t.date, amount: -9, payee: "H".into(), ..Default::default() };
    db.update_transaction(one, &edit).unwrap();
    assert_eq!(db.transaction(one).unwrap().tags.len(), 2);
    for bad in ["x".repeat(41), "a\u{0}".into(), (0..21).map(|i| format!("t{i},")).collect()] {
        assert!(db.set_tags(one, &[bad]).is_err());
    }
    assert!(db.set_tags(9999, &[]).is_err());
    db.set_tags(one, &[]).unwrap();
    assert_eq!(db.tags().unwrap().len(), 1);
    // What the bank wrote is found too.
    db.insert_rule(&Rule { rename_payee: "Rail".into(), ..words("train", None) }, true).unwrap();
    db.run_rules(Scope { all: true, overwrite: true }, None).unwrap();
    db.update_transaction(
        two,
        &TxEdit { date: t.date, amount: -5, payee: "Rail".into(), memo: "".into(), ..Default::default() },
    )
    .unwrap();
    assert_eq!(search("train"), [two]);
}

fn edit_of(t: &Transaction) -> TxEdit {
    TxEdit {
        date: t.date,
        amount: t.amount,
        payee: t.payee.clone(),
        memo: t.memo.clone(),
        category_id: t.category_id,
        cleared: t.cleared,
    }
}

const ALL: Scope = Scope { all: true, overwrite: false };
const OVER: Scope = Scope { all: true, overwrite: true };

fn one(db: &Db, a: i64, row: &str) -> Transaction {
    import(db, a, &csv(&format!("Data;Beneficiario;Descrizione;Importo\n05/03/2026;{row};-1,20\n")));
    db.transactions(&TxFilter { limit: Some(1), ..Default::default() }).unwrap().remove(0)
}

/// As a file of format 7 has it.
fn forget_texts(db: &Db) {
    db.conn.execute("UPDATE book_postings SET original_payee=NULL, original_memo=NULL, rules=NULL", []).unwrap();
}

#[test]
fn what_may_be_the_users_is_never_replaced() {
    let (db, a) = books();
    let s = cat(&db, "Spesa");
    let typed = add(&db, a, "Cena con Bruno", "satispay", -500);
    let t = one(&db, a, "SATISPAY;spesa satispay");
    let old = one(&db, a, "SATISPAY 2;satispay");
    forget_texts(&db);
    // One of an older program is edited, and one was edited there: no key proves its text.
    db.update_transaction(t.id, &TxEdit { payee: "Cena con Anna".into(), category_id: Some(s), ..edit_of(&t) })
        .unwrap();
    assert_eq!(db.transaction(t.id).unwrap().original_payee.as_deref(), Some("SATISPAY"));
    let entry = db.transaction(old.id).unwrap().entry_id;
    db.conn.execute("UPDATE entries SET payee='Cena con Carla' WHERE id=?1", [entry]).unwrap();
    let edited = one(&db, a, "SATISPAY 3;satispay");
    forget_texts(&db);
    db.update_transaction(edited.id, &TxEdit { memo: "satispay, regalo".into(), ..edit_of(&edited) }).unwrap();
    let kept = db.transaction(edited.id).unwrap();
    assert_eq!((kept.original_payee, kept.original_memo.as_deref()), (None, Some("satispay")));
    let plain = one(&db, a, "SATISPAY 4;satispay");
    db.conn
        .execute("UPDATE book_postings SET original_payee=NULL, original_memo=NULL WHERE id=?1", [plain.id])
        .unwrap();
    let rule = Rule { pattern: "satispay".into(), rename_payee: "Satispay".into(), ..Default::default() };
    db.insert_rule(&rule, false).unwrap();
    db.run_rules(ALL, None).unwrap();
    let texts = |id| {
        let t = db.transaction(id).unwrap();
        (t.payee, t.memo)
    };
    let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
    assert_eq!(texts(t.id), pair("Cena con Anna", "spesa satispay"));
    assert_eq!(texts(typed), pair("Cena con Bruno", "satispay"));
    assert_eq!(texts(old.id), pair("Cena con Carla", "satispay"));
    // The payee of these is the bank's: the keys of the import tell.
    assert_eq!(texts(edited.id), pair("Satispay", "satispay, regalo"));
    assert_eq!(texts(plain.id), pair("Satispay", "satispay"));
    let x = db.transaction(plain.id).unwrap();
    assert_eq!((x.original_payee.as_deref(), x.typed_payee), (Some("SATISPAY 4"), None));
    // Asked for, what was typed is kept, and never called the bank's.
    db.run_rules(OVER, None).unwrap();
    for (id, text) in [(typed, "Cena con Bruno"), (old.id, "Cena con Carla")] {
        let x = db.transaction(id).unwrap();
        assert_eq!(
            (x.payee.as_str(), x.original_payee.as_deref(), x.typed_payee.as_deref()),
            ("Satispay", None, Some(text))
        );
    }
    // Nor is it compared as such with another statement.
    let file = importer::ofx_table(
        ofx::read(
            ofx::tests::sgml()
                .replace("20260105", "20260305")
                .replace("-12,50", "-1,20")
                .replace("BAR &amp; CAFFE", "Cena con Carla")
                .as_bytes(),
        )
        .unwrap()
        .0,
        "UTF-8",
    );
    let (rows, _) = importer::parse_rows(&file, &file.mapping, false);
    let likely = db.likely_duplicates(Some(a), &rows).unwrap();
    assert!(!likely.is_empty() && likely.iter().all(|l| !l.certain), "{likely:?}");
    assert_sound(&db, "typed");
}

/// The text that an older program moved into the notes is proved too, and that of an OFX file.
#[test]
fn the_keys_of_an_import_prove_the_text_of_the_bank() {
    let (db, a) = books();
    let id = db.insert_rule(&Rule { rename_payee: "Shop".into(), ..words("pos", None) }, true).unwrap();
    let t = one(&db, a, "POS 1;");
    assert_eq!((t.payee.as_str(), t.memo.as_str(), t.rules.memo), ("Shop", "POS 1", Some(id)));
    forget_texts(&db);
    let file = importer::ofx_table(ofx::read(ofx::tests::sgml().as_bytes()).unwrap().0, "UTF-8");
    let b = db.add_account("B", AccountKind::Bank, "EUR", 0, "").unwrap();
    import(&db, b, &file);
    db.conn.execute("UPDATE book_postings SET original_payee=NULL, original_memo=NULL", []).unwrap();
    // Nothing says who wrote "Shop": it stays, and the notes are the bank's payee.
    let y = cat(&db, "Y");
    db.insert_rule(&Rule { memo: "pos".into(), category_id: Some(y), ..Default::default() }, true).unwrap();
    db.insert_rule(&Rule { rename_payee: "Bar".into(), ..words("bar &", None) }, true).unwrap();
    db.run_rules(ALL, None).unwrap();
    let x = db.transaction(t.id).unwrap();
    assert_eq!((x.payee.as_str(), x.category_id), ("Shop", None));
    let found = db.transactions(&TxFilter { account_id: Some(b), ..Default::default() }).unwrap();
    assert!(found.iter().any(|t| t.payee == "Bar" && t.original_payee.as_deref() == Some("BAR & CAFFE")), "{found:?}");
    db.update_transaction(t.id, &TxEdit { payee: "Mine".into(), ..edit_of(&x) }).unwrap();
    let x = db.transaction(t.id).unwrap();
    assert_eq!((x.original_payee.as_deref(), x.memo.as_str()), (Some("POS 1"), "POS 1"));
    db.run_rules(ALL, None).unwrap();
    assert_eq!(db.transaction(t.id).unwrap().category_id, None, "its notes are not the user's");
}

#[test]
fn a_category_confirmed_is_by_hand() {
    let (db, a) = books();
    let (x, y) = (cat(&db, "X"), cat(&db, "Y"));
    let id = db.insert_rule(&words("shop", Some(x)), false).unwrap();
    let t = add(&db, a, "shop", "", -5);
    let update = |args| crate::ops::call(&db, "update_transaction", &args, None).unwrap();
    update(serde_json::json!({"id": t, "memo": "n"}));
    assert_eq!(db.transaction(t).unwrap().rules.category, Some(id));
    update(serde_json::json!({"id": t, "category": "X"}));
    assert_eq!(db.transaction(t).unwrap().rules.category, None);
    db.update_rule(&Rule { id, ..words("shop", Some(y)) }).unwrap();
    db.run_rules(ALL, None).unwrap();
    assert_eq!(db.transaction(t).unwrap().category, "X");
    // And an undo leaves it.
    let t = add(&db, a, "other", "", -5);
    db.insert_rule(&words("other", Some(x)), false).unwrap();
    db.run_rules(Scope::default(), None).unwrap();
    update(serde_json::json!({"id": t, "category": "X"}));
    assert!(db.undo_rules().is_err());
    assert_eq!(db.transaction(t).unwrap().category, "X");
}

/// Running twice is running once, whatever the rules, the files and what the user did between.
#[test]
fn a_second_run_changes_nothing() {
    let text = ["pos", "bar", "zoo", "pos 1", "bar zoo", "", "shop", "a", "Shop!"];
    let mut r = Lcg(21);
    let mut changed = 0;
    for round in 0..400 {
        let (db, a) = books();
        let cats: Vec<i64> = ["X", "Y", "Z"].iter().map(|c| cat(&db, c)).collect();
        let mut rules = Vec::new();
        let some_rule = |r: &mut Lcg| {
            let mut w = |on: bool| if on { r.pick(&text).unwrap().to_string() } else { String::new() };
            let (p, q, m) = (w(true), w(true), w(true));
            let regex = r.below(3) == 0;
            Rule {
                pattern: if r.below(2) == 0 { p } else { String::new() },
                payee: if r.below(2) == 0 { q } else { String::new() },
                memo: if r.below(2) == 0 { m } else { String::new() },
                regex,
                category_id: if r.below(2) == 0 { r.pick(&cats) } else { None },
                tags: if r.below(3) == 0 { vec![format!("t{}", r.below(30))] } else { vec![] },
                rename_payee: match r.below(4) {
                    0 => "Shop!".into(),
                    1 if regex => "$0 $0".into(),
                    2 => "bar".into(),
                    _ => String::new(),
                },
                ..Default::default()
            }
        };
        for step in 0..6 {
            let rule = some_rule(&mut r);
            rules.extend(db.insert_rule(&rule, r.below(2) == 0).ok());
            let (p, m) = (r.pick(&text).unwrap(), r.pick(&text).unwrap());
            let id = if r.below(2) == 0 {
                let row = format!("{p} ;{m}");
                let t =
                    csv(&format!("Data;Beneficiario;Descrizione;Importo\n0{}/03/2026;{row};-1,2{step}\n", step + 1));
                import(&db, a, &t);
                db.transactions(&TxFilter { limit: Some(1), ..Default::default() }).unwrap().first().map(|t| t.id)
            } else {
                Some(add(&db, a, p, m, -120))
            };
            if r.below(3) == 0 {
                forget_texts(&db);
            }
            let Some(id) = id else { continue };
            for _ in 0..r.below(3) {
                let t = db.transaction(id).unwrap();
                let w = r.pick(&text).unwrap().to_string();
                match r.below(6) {
                    0 => db.update_transaction(id, &TxEdit { payee: w, ..edit_of(&t) }).unwrap(),
                    1 => db.update_transaction(id, &TxEdit { memo: w, ..edit_of(&t) }).unwrap(),
                    2 => db.set_category(id, r.pick(&cats)).unwrap(),
                    3 => db.set_tags(id, &[w.replace('!', "")]).unwrap(),
                    4 => db.set_tags(id, &[]).unwrap(),
                    _ => db.set_category(id, None).unwrap(),
                }
            }
            for scope in [Scope::default(), ALL, OVER] {
                if scope == OVER && r.below(3) > 0 {
                    continue;
                }
                let before: Vec<_> = db.transactions(&TxFilter::default()).unwrap();
                changed += db.run_rules(scope, None).unwrap().changed;
                let again = db.plan_rules(scope).unwrap().changes;
                assert!(again.is_empty(), "{round} {scope:?} {again:#?} {:#?}", db.rules().unwrap());
                if scope != OVER {
                    for t in before.iter().filter(|t| t.marks.own_payee || t.keys.is_empty() && t.rules.payee.is_none())
                    {
                        assert_eq!(db.transaction(t.id).unwrap().payee, t.payee);
                    }
                }
            }
        }
        assert_sound(&db, "twice");
    }
    assert!(changed > 700, "{changed}");
}

#[test]
fn notes_are_matched_only_as_the_user_wrote_them() {
    let (db, a) = books();
    let (g, s) = (cat(&db, "Regali"), cat(&db, "Spesa"));
    let gift =
        Rule { memo: "regalo".into(), category_id: Some(g), rename_payee: "Regalo".into(), ..Default::default() };
    db.insert_rule(&gift, false).unwrap();
    db.insert_rule(&words("pos", Some(s)), false).unwrap();
    let t = one(&db, a, "POS 1;");
    db.update_transaction(t.id, &TxEdit { memo: "regalo Anna".into(), ..edit_of(&t) }).unwrap();
    assert_eq!(db.run_rules(ALL, None).unwrap().changed, 1);
    assert_eq!(db.run_rules(ALL, None).unwrap().changed, 0);
    let x = db.transaction(t.id).unwrap();
    assert_eq!((x.category.as_str(), x.payee.as_str(), x.memo.as_str()), ("Regali", "Regalo", "regalo Anna"));
    // A payee that was moved into the notes is not a note, also when the user then writes another payee.
    db.insert_rule(&Rule { rename_payee: "Shop".into(), ..words("card", Some(s)) }, true).unwrap();
    let t = one(&db, a, "CARD 9;");
    assert_eq!((t.memo.as_str(), t.rules.memo.is_some()), ("CARD 9", true));
    db.update_transaction(t.id, &TxEdit { payee: "Mio".into(), ..edit_of(&t) }).unwrap();
    db.insert_rule(&Rule { memo: "card".into(), category_id: Some(g), ..Default::default() }, true).unwrap();
    db.run_rules(ALL, None).unwrap();
    assert_eq!(db.transaction(t.id).unwrap().category, "Spesa");
    // Written by the user, they are.
    db.update_transaction(t.id, &TxEdit { memo: "card of Anna".into(), ..edit_of(&db.transaction(t.id).unwrap()) })
        .unwrap();
    db.run_rules(ALL, None).unwrap();
    let x = db.transaction(t.id).unwrap();
    assert_eq!((x.category.as_str(), x.payee.as_str(), x.rules.memo), ("Regali", "Mio", None));
    // Put back by hand to what the bank wrote, the payee stays, and so do notes that were emptied.
    let t = one(&db, a, "CARD 10;");
    let edit = TxEdit { payee: "CARD 10".into(), memo: String::new(), ..edit_of(&t) };
    db.update_transaction(t.id, &edit).unwrap();
    db.run_rules(ALL, None).unwrap();
    let x = db.transaction(t.id).unwrap();
    assert_eq!((x.payee.as_str(), x.memo.as_str()), ("CARD 10", ""));
    db.run_rules(OVER, None).unwrap();
    let x = db.transaction(t.id).unwrap();
    assert_eq!((x.payee.as_str(), x.memo.as_str()), ("Shop", ""));
}

#[test]
fn tags_are_lost_by_no_join_and_come_back_by_no_rule() {
    let (db, a) = books();
    let b = db.add_account("B", AccountKind::Bank, "EUR", 0, "").unwrap();
    let tags = |id: i64| db.transaction(id).unwrap().tags;
    let (_, into) = db.add_transfer(a, b, day(4), 5000, "").unwrap();
    db.set_tags(into, &["vacanze".into(), "both".into()]).unwrap();
    let row = add(&db, b, "Bonifico", "", 5000);
    db.set_tags(row, &["mine".into(), "Both".into()]).unwrap();
    let far = db.transfer_candidates(row, a).unwrap()[0].id;
    db.turn_into_transfer(row, a, OtherSide::Join(far)).unwrap();
    assert_eq!(tags(row), ["mine", "Both", "vacanze"]);
    // Two lines that are joined keep their own; one that goes leaves its tags.
    let (x, y) = (add(&db, a, "x", "", -70), add(&db, b, "y", "", 70));
    db.set_tags(x, &["x".into()]).unwrap();
    db.set_tags(y, &["y".into()]).unwrap();
    db.turn_into_transfer(x, b, OtherSide::Join(y)).unwrap();
    assert_eq!((tags(x), tags(y)), (vec!["x".to_string()], vec!["y".to_string()]));
    db.separate_transfer(x, false).unwrap();
    assert_eq!(tags(x), ["x", "y"]);
    // A line of the bank that takes the place of a side keeps the tags of the side.
    let (_, side) = db.add_transfer(a, b, day(4), 900, "").unwrap();
    db.set_tags(side, &["kept".into()]).unwrap();
    assert_eq!(import(&db, b, &csv("Data;Descrizione;Importo\n05/03/2026;Moved;9,00\n")).adopted.len(), 1);
    assert_eq!(tags(side), ["kept"]);
    // Removed by hand, whoever had put it there.
    let t = add(&db, a, "pos", "", -1);
    db.set_tags(t, &["Fun".into()]).unwrap();
    db.insert_rule(&Rule { tags: vec!["fun".into()], ..words("pos", None) }, true).unwrap();
    assert_eq!(db.run_rules(ALL, None).unwrap().changed, 0);
    db.set_tags(t, &[]).unwrap();
    assert_eq!(db.run_rules(ALL, None).unwrap().changed, 0);
    db.set_tags(t, &["fun".into()]).unwrap();
    assert!(db.transaction(t).unwrap().rules.is_empty());
    // Those of the rules make room for those given.
    let many: Vec<String> = (0..20).map(|i| format!("t{i}")).collect();
    db.insert_rule(&Rule { tags: many.clone(), ..words("shop", None) }, true).unwrap();
    let given = serde_json::json!({"account": "Bank", "payee": "shop", "amount": -5, "tags": "mine, T3"});
    let v = crate::ops::call(&db, "add_transaction", &given, None).unwrap();
    let got = tags(v["id"].as_i64().unwrap());
    assert!(got.len() == MAX_TAGS && got[..2] == ["mine", "T3"], "{got:?}");
    assert_eq!(db.run_rules(ALL, None).unwrap().changed, 0);
    assert_sound(&db, "tags");
}

#[test]
fn a_rule_from_a_transaction_can_always_be_saved() {
    let (db, a) = books();
    let x = cat(&db, "X");
    let pieces = ["$1", "${a}", "$", "|", "(", "Tutto a ", " ", "x", "é", "\\", "[", ",", "#"];
    let mut r = Lcg(5);
    for i in 0..400 {
        let some = |r: &mut Lcg| (0..r.below(8)).map(|_| pieces[r.below(pieces.len())]).collect::<String>();
        let (p, m, n) = (some(&mut r), some(&mut r), some(&mut r).repeat(1 + r.below(60)));
        let t = add(&db, a, &p.repeat(1 + r.below(60)), &m, -(i + 1));
        db.update_transaction(t, &TxEdit { payee: n, category_id: Some(x), ..edit_of(&db.transaction(t).unwrap()) })
            .unwrap();
        if r.below(2) == 0 {
            db.conn.execute("UPDATE book_postings SET original_payee=?2 WHERE id=?1", params![t, p]).unwrap();
        }
        let rule = db.rule_from(t).unwrap();
        let id = db.insert_rule(&rule, true).unwrap_or_else(|e| panic!("{rule:?} {e:#}"));
        let all = db.transactions(&TxFilter::default()).unwrap();
        assert!(db.rule_matches(&rule, &all).unwrap().contains(&t), "{rule:?}");
        db.delete_rule(id).unwrap();
        db.delete_transaction(t).unwrap();
    }
}

#[test]
fn rules_are_shown_only_for_rows_that_would_be_added() {
    let (db, a) = books();
    let b = db.add_account("B", AccountKind::Bank, "EUR", 0, "").unwrap();
    db.insert_rule(&words("shop", Some(cat(&db, "X"))), true).unwrap();
    db.add_transfer(b, a, day(4), 900, "").unwrap();
    import(&db, a, &csv("Data;Descrizione;Importo\n01/03/2026;shop known;-1,00\n"));
    let t = csv(
        "Data;Descrizione;Importo\n01/03/2026;shop known;-1,00\n02/03/2026;shop new;-2,00\n05/03/2026;shop side;9,00\n",
    );
    let (rows, _) = importer::parse_rows(&t, &t.mapping, false);
    let add = |account, d: &crate::duplicates::Decision| {
        let mut v: Vec<usize> = importer::rows_to_add(&db, account, &rows, d).unwrap().into_iter().collect();
        v.sort_unstable();
        v
    };
    assert_eq!(add(Some(a), &Default::default()), [1]);
    assert_eq!(add(None, &Default::default()), [0, 1, 2]);
    let res = import(&db, a, &t);
    assert_eq!((res.added, res.duplicates, res.adopted.len()), (1, 1, 1));
    // One that may be there, once it is said that it is another.
    let file = importer::ofx_table(
        ofx::read(ofx::tests::sgml().replace("20260105", "20260302").replace("-12,50", "-2,00").as_bytes()).unwrap().0,
        "UTF-8",
    );
    let (rows, _) = importer::parse_rows(&file, &file.mapping, false);
    let of = |d: &crate::duplicates::Decision| importer::rows_to_add(&db, Some(a), &rows, d).unwrap().len();
    let mut d = crate::duplicates::Decision::default();
    assert_eq!(of(&d), 1);
    d.add_others = true;
    assert_eq!(of(&d), 2);
}

#[test]
fn what_goes_with_an_account_or_a_category_is_counted() {
    let (db, a) = books();
    let (x, below) = (cat(&db, "X"), cat(&db, "X:Below"));
    let other = cat(&db, "Other");
    for (account, category) in [(Some(a), x), (Some(a), other), (None, below), (None, other)] {
        db.insert_rule(&Rule { account_id: account, ..words("w", Some(category)) }, false).unwrap();
    }
    assert_eq!((db.rules_of_account(a).unwrap(), db.rules_of_category(x).unwrap()), (2, 2));
    let v = crate::ops::call(&db, "delete_category", &serde_json::json!({"category": "X"}), None).unwrap();
    assert!(v["rules_deleted"] == 2 && v["message"].as_str().unwrap().contains("2 rules"), "{v}");
    assert_eq!(db.rules().unwrap().len(), 2);
    let v = crate::ops::call(&db, "delete_account", &serde_json::json!({"account": "Bank"}), None).unwrap();
    assert!(v["rules_deleted"] == 1 && v["message"].as_str().unwrap().contains("1 rule "), "{v}");
    assert_eq!(db.rules().unwrap().len(), 1);
}

#[test]
fn a_count_stops_when_it_is_told_or_takes_too_long() {
    let (db, a) = books();
    for i in 0..50 {
        add(&db, a, &format!("shop {i}"), "", -1);
    }
    let all = db.transactions(&TxFilter::default()).unwrap();
    let go = AtomicBool::new(false);
    let hour = Duration::from_secs(3600);
    assert_eq!(count_matches(&words("shop", None), &all, hour, &go).unwrap(), (50, true));
    assert_eq!(count_matches(&words("shop", None), &all, Duration::ZERO, &go).unwrap(), (0, false));
    go.store(true, Ordering::Relaxed);
    assert_eq!(count_matches(&words("shop", None), &all, hour, &go).unwrap(), (0, false));
    assert!(count_matches(&re("(", ""), &all, hour, &go).is_err());
}
