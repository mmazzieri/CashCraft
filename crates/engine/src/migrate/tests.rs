use crate::db::*;
use crate::importer;
use crate::ledger::tests::{Lcg, assert_sound, dump};
use crate::ledger::{Class, EntryFilter};
use rusqlite::{Connection, params};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The tables of formats 0 and 1, as CashCraft 0.2.0 created them. Never change this text.
const V1_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS accounts (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    kind TEXT NOT NULL,
    currency TEXT NOT NULL DEFAULT 'EUR',
    opening_balance INTEGER NOT NULL DEFAULT 0,
    iban TEXT NOT NULL DEFAULT '',
    archived INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS categories (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    parent_id INTEGER REFERENCES categories(id) ON DELETE CASCADE,
    is_income INTEGER NOT NULL DEFAULT 0,
    UNIQUE(name, parent_id)
);
CREATE TABLE IF NOT EXISTS transactions (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    date TEXT NOT NULL,
    amount INTEGER NOT NULL,
    payee TEXT NOT NULL DEFAULT '',
    memo TEXT NOT NULL DEFAULT '',
    category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    cleared INTEGER NOT NULL DEFAULT 0,
    transfer_peer INTEGER,
    import_hash TEXT
);
CREATE INDEX IF NOT EXISTS tx_account_date ON transactions(account_id, date);
CREATE UNIQUE INDEX IF NOT EXISTS tx_import_hash ON transactions(account_id, import_hash) WHERE import_hash IS NOT NULL;
CREATE TABLE IF NOT EXISTS budgets (
    category_id INTEGER PRIMARY KEY REFERENCES categories(id) ON DELETE CASCADE,
    monthly INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS rules (
    id INTEGER PRIMARY KEY,
    pattern TEXT NOT NULL,
    category_id INTEGER NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
    rename_payee TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS scheduled (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    to_account_id INTEGER REFERENCES accounts(id) ON DELETE CASCADE,
    amount INTEGER NOT NULL,
    payee TEXT NOT NULL DEFAULT '',
    memo TEXT NOT NULL DEFAULT '',
    category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    next_date TEXT NOT NULL,
    every INTEGER NOT NULL DEFAULT 1,
    unit TEXT NOT NULL DEFAULT 'month'
);
"#;

/// A folder of its own for each test.
struct Dir(PathBuf);
impl Dir {
    fn new(name: &str) -> Self {
        let d = std::env::temp_dir().join(format!("cashcraft-migrate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }
    fn file(&self) -> PathBuf {
        self.0.join("books.db")
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const STATEMENT: &str = "Data;Descrizione;Importo\n01/02/2026;BAR ROMA;-1,20\n01/02/2026;BAR ROMA;-1,20\n03/02/2026;STIPENDIO MARZO;1.500,00\n";

/// The statement as 0.2.0 stored it: the rows with the hashes of the importer.
fn imported() -> Vec<(String, i64, String, String)> {
    let db = Db::open_in_memory().unwrap();
    let a = db.add_account("A", AccountKind::Bank, "EUR", 0, "").unwrap();
    let t = importer::analyze(STATEMENT, "UTF-8").unwrap();
    let (rows, skipped) = importer::parse_rows(&t, &t.mapping, false);
    importer::import_rows(&db, a, &rows, skipped).unwrap();
    let mut st = db
        .conn
        .prepare(
            "SELECT e.date, p.amount, e.payee, p.import_hash FROM book_postings p JOIN entries e ON e.id=p.entry_id
             WHERE p.import_hash IS NOT NULL ORDER BY p.id",
        )
        .unwrap();
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
    rows.map(Result::unwrap).collect()
}

fn create(path: &Path, version: i64) -> Connection {
    let c = Connection::open(path).unwrap();
    let _ = c.execute_batch("PRAGMA journal_mode=WAL;");
    c.execute_batch(V1_SCHEMA).unwrap();
    c.pragma_update(None, "user_version", version).unwrap();
    // 0.2.0 kept the keys consistent; the hostile rows below need it off.
    c.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
    c
}

struct Tx<'a> {
    account: i64,
    date: &'a str,
    amount: i64,
    category: Option<i64>,
    peer: Option<i64>,
}

fn add(c: &Connection, t: Tx) -> i64 {
    c.execute(
        "INSERT INTO transactions(account_id,date,amount,payee,memo,category_id,cleared,transfer_peer)
         VALUES(?1,?2,?3,'Shop '||?1,'memo '||?3,?4,abs(?3)%2,?5)",
        params![t.account, t.date, t.amount, t.category, t.peer],
    )
    .unwrap();
    c.last_insert_rowid()
}

fn link(c: &Connection, a: i64, b: i64) {
    c.execute("UPDATE transactions SET transfer_peer=?2, category_id=NULL WHERE id=?1", [a, b]).unwrap();
}

/// A file of 0.2.0 after a year of use, with what crashes and other programs may have left.
fn seed_v1(path: &Path, version: i64, seed: u64) -> Connection {
    let c = create(path, version);
    c.execute_batch(
        "INSERT INTO accounts(id,name,kind,currency,opening_balance,archived) VALUES
            (1,'Bank','bank','EUR',248000,0), (2,'Card','credit_card','EUR',-31050,0), (3,'Wallet','cash','eur',0,0),
            (4,'Deposit','savings','EUR',850000,0), (7,'Mortgage','loan','',-9000000,0), (9,'Dollars','bank','USD',5000,1),
            (12,'Old','investment','EUR',777,1);
         INSERT INTO categories(id,name,parent_id,is_income) VALUES
            (1,'Home',NULL,0), (2,'Utilities',1,0), (3,'Groceries',NULL,0), (4,'Salary',NULL,1), (5,'Refunds',4,1),
            (6,'Gas',2,0), (8,'Never used',NULL,0);
         INSERT INTO budgets VALUES (1,20000), (3,40000), (4,100000), (8,1);
         INSERT INTO rules(pattern,category_id) VALUES ('supermarket',3);
         INSERT INTO scheduled(account_id,to_account_id,amount,payee,category_id,next_date) VALUES
            (1,NULL,-75000,'Rent',1,'2099-01-01'), (1,4,30000,'Savings',NULL,'2099-01-01');
         INSERT INTO meta VALUES ('created','2025-01-01');",
    )
    .unwrap();
    for (date, amount, payee, hash) in imported() {
        c.execute(
            "INSERT INTO transactions(account_id,date,amount,payee,cleared,import_hash) VALUES(1,?1,?2,?3,1,?4)",
            params![date, amount, payee, hash],
        )
        .unwrap();
    }
    let mut r = Lcg(seed);
    let accounts = [1, 2, 3, 4, 7, 9, 12];
    let categories = [None, Some(1), Some(2), Some(3), Some(4), Some(5), Some(6)];
    let date = |r: &mut Lcg| format!("{}-{:02}-{:02}", 2025 + r.below(2), 1 + r.below(12), 1 + r.below(28));
    for _ in 0..400 {
        let d = date(&mut r);
        let account = r.pick(&accounts).unwrap();
        if r.below(5) == 0 {
            let (from, to) = (account, r.pick(&[1, 2, 3, 4, 7]).unwrap());
            if from == to {
                continue;
            }
            let amount = r.amount();
            let a = add(&c, Tx { account: from, date: &d, amount: -amount, category: None, peer: None });
            let b = add(&c, Tx { account: to, date: &d, amount, category: None, peer: Some(a) });
            link(&c, a, b);
            c.execute("UPDATE transactions SET memo='m', payee='To '||account_id WHERE id IN (?1,?2)", [a, b]).unwrap();
        } else {
            let category = r.pick(&categories).unwrap();
            let amount = if r.below(30) == 0 { 0 } else { r.amount() };
            let id = add(&c, Tx { account, date: &d, amount, category, peer: None });
            if r.below(3) == 0 {
                c.execute("UPDATE transactions SET import_hash=?2 WHERE id=?1", params![id, format!("h{id}")]).unwrap();
            }
        }
    }
    let one = |account, date, amount, peer| add(&c, Tx { account, date, amount, category: None, peer });
    // A peer that is not there, one that does not point back, itself, different amounts, different
    // dates, different currencies, the same account, three in a ring, and a category on half a transfer.
    one(1, "2026-03-01", -1111, Some(999_999));
    let a = one(1, "2026-03-02", -2222, None);
    one(4, "2026-03-02", 2222, Some(a));
    let a = one(2, "2026-03-03", -3333, None);
    link(&c, a, a);
    for (x, y) in [
        ((1, "2026-03-04", -4444), (3, "2026-03-04", 4443)),
        ((1, "2026-03-05", -5555), (3, "2026-04-05", 5555)),
        ((1, "2026-03-06", -6666), (9, "2026-03-06", 6666)),
        ((1, "2026-03-07", -7777), (1, "2026-03-07", 7777)),
    ] {
        let a = one(x.0, x.1, x.2, None);
        let b = one(y.0, y.1, y.2, Some(a));
        link(&c, a, b);
    }
    let (p, q, s) = (one(1, "2026-03-08", -1, None), one(3, "2026-03-08", 1, None), one(4, "2026-03-08", 1, None));
    link(&c, p, q);
    link(&c, q, s);
    link(&c, s, p);
    add(&c, Tx { account: 1, date: "2026-03-09", amount: -88, category: Some(3), peer: Some(999_998) });
    c
}

/// Transaction, account, date, amount, payee, memo, category, cleared, running balance.
type RegisterRow = (i64, i64, String, i64, String, String, Option<i64>, bool, i64);

/// What people see, read with the queries of 0.2.0.
#[derive(Debug, PartialEq, Eq)]
struct Figures {
    accounts: Vec<(i64, i64, i64)>,
    register: Vec<RegisterRow>,
    hashes: Vec<(i64, i64, String)>,
    months: Vec<(String, i64, i64)>,
    categories: BTreeMap<String, Vec<(Option<i64>, i64)>>,
    budgets: BTreeMap<String, Vec<(i64, i64, i64)>>,
    net_worth: Vec<(String, i64)>,
    uncategorized: Vec<i64>,
}

const UNTIL: &str = "2026-12-31";

fn months() -> Vec<(String, String, String)> {
    let last = chrono::NaiveDate::parse_from_str(UNTIL, "%Y-%m-%d").unwrap();
    (0..30)
        .rev()
        .map(|i| {
            let (a, b) = month_range(crate::ops::months_back(last, i));
            (a.format("%Y-%m").to_string(), d2s(a), d2s(b))
        })
        .collect()
}

fn rows<T>(
    c: &Connection,
    sql: &str,
    p: impl rusqlite::Params,
    f: impl Fn(&rusqlite::Row) -> rusqlite::Result<T>,
) -> Vec<T> {
    let mut st = c.prepare(sql).unwrap();
    st.query_map(p, |r| f(r)).unwrap().map(Result::unwrap).collect()
}

fn v1_figures(c: &Connection) -> Figures {
    let accounts = rows(
        c,
        "SELECT a.id, a.opening_balance,
                a.opening_balance + COALESCE((SELECT SUM(amount) FROM transactions t WHERE t.account_id=a.id),0)
         FROM accounts a ORDER BY a.id",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    );
    let mut register = Vec::new();
    for (account, opening, _) in &accounts {
        let mut running: i64 = *opening;
        for (id, date, amount, payee, memo, category, cleared, peer) in rows(
            c,
            "SELECT t.id, t.date, t.amount, t.payee, t.memo, t.category_id, t.cleared, t.transfer_peer
             FROM transactions t JOIN accounts a ON a.id=t.account_id WHERE t.account_id=?1
             ORDER BY t.date ASC, t.id ASC",
            [account],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, i64>(6)? != 0,
                    r.get::<_, Option<i64>>(7)?,
                ))
            },
        ) {
            running += amount;
            // Half a transfer has no category any more.
            let category = category.filter(|_| peer.is_none());
            register.push((id, *account, date, amount, payee, memo, category, cleared, running));
        }
    }
    let opening: i64 = c.query_row("SELECT COALESCE(SUM(opening_balance),0) FROM accounts", [], |r| r.get(0)).unwrap();
    let parents: BTreeMap<i64, Option<i64>> =
        rows(c, "SELECT id, parent_id FROM categories", [], |r| Ok((r.get(0)?, r.get(1)?))).into_iter().collect();
    let under = |mut x: i64, top: i64| loop {
        if x == top {
            return true;
        }
        match parents.get(&x).copied().flatten() {
            Some(p) => x = p,
            None => return false,
        }
    };
    let income: Vec<i64> = rows(c, "SELECT id FROM categories WHERE is_income", [], |r| r.get(0));
    let (mut months_out, mut categories, mut budgets, mut net_worth) =
        (Vec::new(), BTreeMap::new(), BTreeMap::new(), Vec::new());
    for (m, from, to) in months() {
        let (inc, exp): (i64, i64) = c
            .query_row(
                "SELECT COALESCE(SUM(CASE WHEN amount>0 THEN amount END),0),
                        COALESCE(SUM(CASE WHEN amount<0 THEN amount END),0)
                 FROM transactions WHERE transfer_peer IS NULL AND date>=?1 AND date<=?2",
                params![from, to],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        months_out.push((m.clone(), inc, -exp));
        let totals: Vec<(Option<i64>, i64)> = rows(
            c,
            "SELECT category_id, SUM(amount) FROM transactions
             WHERE transfer_peer IS NULL AND date>=?1 AND date<=?2 GROUP BY category_id",
            params![from, to],
            |r| Ok((r.get(0)?, r.get(1)?)),
        );
        let lines = rows(c, "SELECT category_id, monthly FROM budgets ORDER BY 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .into_iter()
            .map(|(top, monthly): (i64, i64)| {
                let total: i64 = totals.iter().filter(|t| t.0.is_some_and(|x| under(x, top))).map(|t| t.1).sum();
                (top, monthly, if income.contains(&top) { total } else { -total })
            })
            .collect();
        budgets.insert(m.clone(), lines);
        categories.insert(m.clone(), totals);
        let sum: i64 = c
            .query_row("SELECT COALESCE(SUM(amount),0) FROM transactions WHERE date<=?1", [&to], |r| r.get(0))
            .unwrap();
        net_worth.push((m, opening + sum));
    }
    Figures {
        accounts,
        register,
        hashes: rows(
            c,
            "SELECT id, account_id, import_hash FROM transactions WHERE import_hash IS NOT NULL ORDER BY id",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ),
        months: months_out,
        categories,
        budgets,
        net_worth,
        uncategorized: rows(
            c,
            "SELECT id FROM transactions WHERE category_id IS NULL AND transfer_peer IS NULL ORDER BY id",
            [],
            |r| r.get(0),
        ),
    }
}

fn v2_figures(db: &Db) -> Figures {
    let mut accounts: Vec<_> =
        db.accounts(true).unwrap().iter().map(|a| (a.id, a.opening_balance, a.balance)).collect();
    accounts.sort();
    let mut register = Vec::new();
    for (account, _, _) in &accounts {
        let mut txs = db.transactions(&TxFilter { account_id: Some(*account), ..Default::default() }).unwrap();
        txs.reverse();
        for t in txs {
            let balance = t.balance.unwrap();
            register.push((t.id, *account, d2s(t.date), t.amount, t.payee, t.memo, t.category_id, t.cleared, balance));
        }
    }
    let until = chrono::NaiveDate::parse_from_str(UNTIL, "%Y-%m-%d").unwrap();
    let (mut categories, mut budgets) = (BTreeMap::new(), BTreeMap::new());
    for (m, from, to) in months() {
        let (from, to) = (s2d(&from), s2d(&to));
        let mut totals: Vec<_> =
            db.category_totals(from, to).unwrap().into_iter().map(|c| (c.category_id, c.total)).collect();
        totals.sort();
        categories.insert(m.clone(), totals);
        let mut lines: Vec<_> =
            db.budget_status(from).unwrap().into_iter().map(|b| (b.category_id, b.budget, b.spent)).collect();
        lines.sort();
        budgets.insert(m, lines);
    }
    let mut uncategorized: Vec<i64> = db
        .transactions(&TxFilter { uncategorized: true, ..Default::default() })
        .unwrap()
        .iter()
        .map(|t| t.id)
        .collect();
    uncategorized.sort();
    Figures {
        accounts,
        register,
        hashes: rows(
            &db.conn,
            "SELECT p.id, l.money_account_id, p.import_hash FROM book_postings p
             JOIN ledger_accounts l ON l.id=p.ledger_account_id WHERE p.import_hash IS NOT NULL ORDER BY p.id",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ),
        months: db.monthly_totals(30, until).unwrap().into_iter().map(|m| (m.month, m.income, m.expense)).collect(),
        categories,
        budgets,
        net_worth: db.net_worth_history(30, until).unwrap(),
        uncategorized,
    }
}

#[test]
fn every_figure_is_the_same_after_the_migration() {
    for (version, seed) in [(0, 1), (1, 2), (1, 3), (1, 4)] {
        let dir = Dir::new(&format!("figures-{seed}"));
        let old = seed_v1(&dir.file(), version, seed);
        let before = v1_figures(&old);
        let old_dump = dump(&old);
        drop(old);
        assert!(before.register.len() > 350 && before.hashes.len() > 50 && before.uncategorized.len() > 20);
        let (db, is_new) = Db::open(&dir.file()).unwrap();
        assert!(!is_new);
        assert_eq!(v2_figures(&db), before);
        assert_sound(&db, "migrated");
        assert_eq!(db.conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), SCHEMA_VERSION);
        // One source of truth.
        assert!(!db.has_table("transactions").unwrap());
        assert!(db.conn.prepare("SELECT opening_balance FROM accounts").is_err());
        // The same classes as for new accounts.
        for a in db.ledger_accounts().unwrap() {
            let liability = matches!(a.money_account_id, Some(2 | 7));
            assert_eq!(a.class == Class::Liability, liability, "{a:?}");
        }
        // The copy is the old file, and opens again as such.
        let copy = dir.0.join("books.v1-backup.db");
        assert_eq!(dump(&Connection::open(&copy).unwrap()), old_dump);
        // Opening again changes nothing.
        let after = dump(&db.conn);
        drop(db);
        let (db, _) = Db::open(&dir.file()).unwrap();
        assert_eq!(dump(&db.conn), after);
        assert_eq!(
            std::fs::read_dir(&dir.0)
                .unwrap()
                .filter(|e| { e.as_ref().unwrap().file_name().to_str().unwrap().contains("backup") })
                .count(),
            1
        );
        // And it works as before.
        for t in db.transactions(&TxFilter::default()).unwrap().iter().take(60) {
            db.set_cleared(t.id, !t.cleared).unwrap();
            let _ = db.set_category(t.id, Some(3));
        }
        let t = importer::analyze(STATEMENT, "UTF-8").unwrap();
        let (rows, skipped) = importer::parse_rows(&t, &t.mapping, false);
        let again = importer::import_rows(&db, 1, &rows, skipped).unwrap();
        assert_eq!((again.added, again.duplicates), (0, 3));
        for a in [2, 1] {
            db.delete_account(a).unwrap();
            assert_sound(&db, "account of a migrated file");
        }
        for c in [1, 4, 3] {
            db.delete_category(c).unwrap();
            assert_sound(&db, "category of a migrated file");
        }
        db.clear_accounts().unwrap();
        assert_sound(&db, "cleared");
    }
}

#[test]
fn sound_transfers_become_one_entry_and_the_others_go_against_equity() {
    let dir = Dir::new("transfers");
    let c = create(&dir.file(), 1);
    c.execute_batch("INSERT INTO accounts(id,name,kind,opening_balance) VALUES (1,'A','bank',10), (2,'B','cash',0);")
        .unwrap();
    let t = |account, amount, peer| add(&c, Tx { account, date: "2026-01-01", amount, category: None, peer });
    let a = t(1, -500, None);
    let b = t(2, 500, Some(a));
    link(&c, a, b);
    let orphan = t(1, -70, Some(12345));
    drop(c);
    let (db, _) = Db::open(&dir.file()).unwrap();
    let (x, y) = (db.transaction(a).unwrap(), db.transaction(b).unwrap());
    assert_eq!((x.transfer_peer, y.transfer_peer, x.entry_id), (Some(b), Some(a), y.entry_id));
    let o = db.transaction(orphan).unwrap();
    assert_eq!((o.kind, o.transfer_peer, o.balance), (TxKind::Other, None, Some(10 - 500 - 70)));
    let entries = db.entries(&EntryFilter::default()).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries.iter().filter(|e| e.opening).count(), 1);
}

/// 0.2.0 counted rows of a missing account in the reports and in the history of the net worth, but in
/// no balance. They stay in the reports; the history now agrees with the balances.
#[test]
fn rows_of_a_missing_account_or_category_are_kept() {
    let dir = Dir::new("missing");
    let c = create(&dir.file(), 1);
    c.execute_batch(
        "INSERT INTO accounts(id,name,kind,opening_balance) VALUES (1,'A','bank',100);
         INSERT INTO categories(id,name) VALUES (1,'C');",
    )
    .unwrap();
    for (account, category, peer) in [(5, Some(1), None), (1, Some(99), None), (5, None, Some(1))] {
        add(&c, Tx { account, date: "2026-12-01", amount: -30, category, peer });
    }
    let before = v1_figures(&c);
    drop(c);
    let (db, _) = Db::open(&dir.file()).unwrap();
    let after = v2_figures(&db);
    assert_eq!((&after.accounts, &after.months, &after.budgets), (&before.accounts, &before.months, &before.budgets));
    assert_eq!(after.categories["2026-12"], [(None, -30), (Some(1), -30)]);
    assert_eq!(before.net_worth.last().unwrap().1, 10);
    assert_eq!(after.net_worth.last().unwrap().1, 70);
    assert_sound(&db, "missing rows");
}

#[test]
fn a_file_without_transactions_and_an_empty_file_open() {
    let dir = Dir::new("bare");
    let c = Connection::open(dir.file()).unwrap();
    c.execute_batch(
        "CREATE TABLE accounts (id INTEGER PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL,
            currency TEXT NOT NULL, opening_balance INTEGER NOT NULL, iban TEXT NOT NULL, archived INTEGER NOT NULL);
         CREATE TABLE categories (id INTEGER PRIMARY KEY, name TEXT NOT NULL, parent_id INTEGER, is_income INTEGER NOT NULL);
         INSERT INTO accounts VALUES (1,'A','bank','EUR',5,'',0);",
    )
    .unwrap();
    drop(c);
    let (db, _) = Db::open(&dir.file()).unwrap();
    assert_eq!(db.find_account("A").unwrap().balance, 5);
    assert_sound(&db, "bare");
    let empty = dir.0.join("empty.db");
    std::fs::write(&empty, "").unwrap();
    assert_sound(&Db::open(&empty).unwrap().0, "empty");
}

#[test]
fn files_of_formats_2_to_7_open() {
    ledger_file_opens(2, include_str!("v2.sql"));
    ledger_file_opens(3, include_str!("v3.sql"));
    ledger_file_opens(4, include_str!("v4.sql"));
    ledger_file_opens(5, include_str!("v5.sql"));
    ledger_file_opens(6, include_str!("v6.sql"));
    ledger_file_opens(7, include_str!("v7.sql"));
}

/// What is new is added, after a copy; everything else stays as it was.
fn ledger_file_opens(version: i64, schema: &str) {
    let dir = Dir::new(&format!("format-{version}"));
    let old = Connection::open(dir.file()).unwrap();
    let table = if version >= 7 { "ledger_postings" } else { "postings" };
    let run = |sql: &str| old.execute_batch(&sql.replace("postings", table)).unwrap();
    old.execute_batch(schema).unwrap();
    run("INSERT INTO accounts(id,name,kind) VALUES (1,'Bank','bank'), (2,'Card','credit_card');
         INSERT INTO categories(id,name) VALUES (1,'Home');
         INSERT INTO entries(id,date,payee) VALUES (1,'2026-01-05','Shop'), (2,'2026-01-06','Moved');
         INSERT INTO postings(id,entry_id,ledger_account_id,amount,currency,import_hash)
         SELECT 1,1,id,-500,'EUR','h' FROM ledger_accounts WHERE money_account_id=1 UNION ALL
         SELECT 2,1,id,500,'EUR',NULL FROM ledger_accounts WHERE category_id=1 UNION ALL
         SELECT 3,2,id,-900,'EUR',NULL FROM ledger_accounts WHERE money_account_id=1 UNION ALL
         SELECT 4,2,id,900,'EUR',NULL FROM ledger_accounts WHERE money_account_id=2;
         UPDATE postings SET payee='side', import_hash='k' WHERE id=3;
         UPDATE postings SET payee='side' WHERE id=4;");
    old.pragma_update(None, "user_version", version).unwrap();
    if (3..5).contains(&version) {
        // What format 3 left of a transfer whose other account was deleted.
        run("UPDATE postings SET date='2026-02-01' WHERE id=1");
    }
    if version == 4 {
        run(
            "INSERT INTO card_statements VALUES (1,2,'2026-01-01','2026-01-05',900,'2026-01-01|2026-01-05|900|1234',3,1);
             INSERT INTO entries(id,date,payee) VALUES (3,'2026-01-02','Refund');
             INSERT INTO postings(id,entry_id,ledger_account_id,amount,currency,import_hash)
             SELECT 5,3,id,70,'EUR','r' FROM ledger_accounts WHERE money_account_id=2 UNION ALL
             SELECT 6,3,id,-70,'EUR',NULL FROM ledger_accounts WHERE category_id=1;",
        );
    }
    if version >= 5 {
        run("UPDATE postings SET origin='added' WHERE id=4");
    }
    if version >= 6 {
        run("INSERT INTO card_statements VALUES (1,2,'2026-01-01','2026-01-05',900,'2026-01-01|1234',3);");
    }
    let before = dump(&old);
    let (db, _) = Db::open(&dir.file()).unwrap();
    assert_sound(&db, "opened");
    if version == 4 {
        let s = &db.card_statements().unwrap()[0];
        assert_eq!((s.payment_id, s.matches, db.transaction(5).unwrap().card_row), (Some(3), true, true));
        let key: String = db.conn.query_row("SELECT import_key FROM card_statements", [], |r| r.get(0)).unwrap();
        assert_eq!(key, "2026-01-01|1234");
        db.conn.execute("DELETE FROM card_statements", []).unwrap();
        db.delete_transaction(5).unwrap();
    }
    if version >= 6 {
        let s = &db.card_statements().unwrap()[0];
        assert_eq!((s.payment_id, s.matches), (Some(3), true));
        db.conn.execute("DELETE FROM card_statements", []).unwrap();
    }
    assert!(db.layouts().unwrap().is_empty());
    let t = db.transaction(1).unwrap();
    let date = if (3..5).contains(&version) { "2026-02-01" } else { "2026-01-05" };
    assert_eq!((d2s(t.date), d2s(db.entry(1).unwrap().date)), (date.into(), date.into()));
    assert_eq!((db.transaction(3).unwrap().added, db.transaction(4).unwrap().added), (false, true));
    let figures = v2_figures(&db);
    assert_eq!(figures.register.len(), 3);
    assert_eq!(figures.accounts.iter().map(|a| a.2).collect::<Vec<_>>(), [-1400, 900]);
    db.separate_transfer(3, true).unwrap();
    assert_sound(&db, "changed");
    let copy = Connection::open(dir.0.join(format!("books.v{version}-backup.db"))).unwrap();
    assert_eq!(dump(&copy), before);
    drop(db);
    Db::open(&dir.file()).unwrap();
    assert!(!dir.0.join(format!("books.v{version}-backup-2.db")).exists());
    // A number written by an older program on a present file changes nothing.
    old.pragma_update(None, "user_version", 1).unwrap();
    let (db, _) = Db::open(&dir.file()).unwrap();
    assert_sound(&db, "stamped");
    assert_eq!(db.conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), SCHEMA_VERSION);
}

/// A window of 0.3.0 that has the file open can write no posting any more, and nothing of what it tries stays.
#[test]
fn a_window_of_format_6_with_the_file_open_is_kept_out() {
    let dir = Dir::new("format-6-open");
    let old = Connection::open(dir.file()).unwrap();
    old.execute_batch(include_str!("v6.sql")).unwrap();
    old.execute_batch(
        "PRAGMA foreign_keys=ON;
         INSERT INTO accounts(id,name,kind) VALUES (1,'Bank','bank'), (2,'Card','credit_card');
         INSERT INTO entries(id,date,payee) VALUES (1,'2026-01-05','Shop');
         INSERT INTO postings(id,entry_id,ledger_account_id,amount,currency,import_hash)
         SELECT 1,1,id,-500,'EUR','h' FROM ledger_accounts WHERE money_account_id=1 UNION ALL
         SELECT 2,1,id,500,'EUR',NULL FROM ledger_accounts WHERE system='uncategorized';",
    )
    .unwrap();
    old.pragma_update(None, "user_version", 6).unwrap();
    // Its statements are ready, as those of an open window.
    let mut ready = old.prepare("SELECT id FROM postings WHERE entry_id=?1").unwrap();
    assert!(ready.exists([1]).unwrap());
    let (db, _) = Db::open(&dir.file()).unwrap();
    db.conn.execute("UPDATE book_postings SET other_hashes='src:csv' WHERE id=1", []).unwrap();
    let before = dump(&db.conn);
    // Every change of 0.3.0 starts by reading the postings of what it changes, or adds some.
    assert!(ready.exists([1]).is_err());
    for sql in [
        "INSERT INTO postings(entry_id,ledger_account_id,amount,currency) VALUES(1,1,5,'EUR')",
        "UPDATE postings SET amount=1 WHERE id=1",
        "UPDATE postings SET import_hash=NULL WHERE id=1 AND entry_id=1",
        "DELETE FROM postings WHERE id=1",
        "SELECT e.id FROM postings p JOIN entries e ON e.id=p.entry_id",
        "BEGIN; INSERT INTO entries(date) VALUES('2026-01-01');
         INSERT INTO postings(entry_id,ledger_account_id,amount,currency) VALUES(last_insert_rowid(),1,5,'EUR'); COMMIT;",
    ] {
        assert!(old.execute_batch(sql).is_err(), "{sql}");
        if !old.is_autocommit() {
            old.execute_batch("ROLLBACK").unwrap();
        }
        assert_eq!(dump(&db.conn), before, "{sql}");
    }
    // What a deleted entry takes with it is whole.
    old.execute("DELETE FROM entries WHERE id=1", []).unwrap();
    assert_sound(&db, "kept out");
}

#[test]
fn a_backup_is_never_overwritten() {
    let dir = Dir::new("backup");
    let first = dir.0.join("books.v1-backup.db");
    std::fs::write(&first, "precious").unwrap();
    drop(seed_v1(&dir.file(), 1, 7));
    Db::open(&dir.file()).unwrap();
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "precious");
    let c = Connection::open(dir.0.join("books.v1-backup-2.db")).unwrap();
    assert_eq!(c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
}

#[test]
fn without_a_backup_nothing_is_migrated() {
    let dir = Dir::new("no-backup");
    let old = seed_v1(&dir.file(), 1, 8);
    let before = dump(&old);
    // The copy would go to a folder that is not there.
    let db = Db { conn: Connection::open(dir.file()).unwrap(), path: Some(dir.0.join("gone").join("books.db")) };
    assert!(db.init().is_err());
    drop(db);
    assert_eq!(dump(&old), before);
}

#[test]
fn a_file_that_cannot_be_migrated_is_left_as_it_was() {
    let dir = Dir::new("refused");
    let old = seed_v1(&dir.file(), 1, 9);
    old.execute("UPDATE transactions SET amount=?1 WHERE id=5", [crate::money::MAX_AMOUNT + 1]).unwrap();
    let before = dump(&old);
    assert!(Db::open(&dir.file()).is_err());
    assert_eq!(dump(&old), before);
    assert!(!dir.0.join("books.v1-backup.db").exists());
    old.execute("UPDATE transactions SET amount=1 WHERE id=5", []).unwrap();
    assert_sound(&Db::open(&dir.file()).unwrap().0, "mended");
}

/// A window of 0.2.0 that has the file open can write nothing any more, so it breaks nothing.
#[test]
fn an_older_program_with_the_file_open_is_kept_out() {
    let dir = Dir::new("older-open");
    let old = seed_v1(&dir.file(), 1, 10);
    old.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    old.execute("INSERT OR IGNORE INTO budgets VALUES (1,5)", []).unwrap();
    let n: i64 = old.query_row("SELECT COUNT(*) FROM transactions", [], |r| r.get(0)).unwrap();
    assert!(n > 0);
    let (db, _) = Db::open(&dir.file()).unwrap();
    let before = dump(&db.conn);
    for sql in [
        "INSERT INTO transactions(account_id,date,amount) VALUES(1,'2026-01-01',5)",
        "UPDATE transactions SET amount=1 WHERE id=1",
        "DELETE FROM transactions WHERE id=1",
        "UPDATE accounts SET name='x', kind='bank', opening_balance=1, iban='', archived=0 WHERE id=1",
        "INSERT INTO accounts(name,kind,currency,opening_balance,iban) VALUES('n','bank','EUR',5,'')",
        "DELETE FROM accounts WHERE id=1",
        "DELETE FROM categories WHERE id=1",
        "DELETE FROM scheduled WHERE id=1",
        "INSERT INTO budgets(category_id,monthly) VALUES(1,5)",
        "DELETE FROM budgets WHERE category_id=1",
        "INSERT INTO rules(pattern,category_id,rename_payee) VALUES('x',1,'')",
        "DELETE FROM rules WHERE id=1",
        "DELETE FROM scheduled; DELETE FROM transactions; DELETE FROM accounts; DELETE FROM budgets;",
    ] {
        // As 0.2.0 does it: each statement commits on its own, `clear_accounts` too.
        assert!(old.execute_batch(sql).is_err(), "{sql}");
        assert_eq!(dump(&db.conn), before, "{sql}");
    }
    assert_eq!(dump(&db.conn), before);
    // What it can still write stays sound.
    old.execute("INSERT INTO categories(name,parent_id,is_income) VALUES('New',1,0)", []).unwrap();
    old.execute("UPDATE categories SET name='House' WHERE id=1", []).unwrap();
    assert_sound(&db, "category of an older program");
    assert!(db.find_ledger_account("House:New").is_ok());
}

/// 0.2.0 starting at the very moment of the migration can write its number on the new file.
#[test]
fn the_number_written_by_an_older_program_is_put_right() {
    let dir = Dir::new("stamped");
    drop(seed_v1(&dir.file(), 1, 11));
    let before = v2_figures(&Db::open(&dir.file()).unwrap().0);
    let old = Connection::open(dir.file()).unwrap();
    old.execute_batch(V1_SCHEMA).unwrap();
    old.pragma_update(None, "user_version", 1).unwrap();
    let (db, _) = Db::open(&dir.file()).unwrap();
    assert_eq!(v2_figures(&db), before);
    assert!(!db.has_table("transactions").unwrap());
    drop(db);
    old.execute_batch(V1_SCHEMA).unwrap();
    old.pragma_update(None, "user_version", 1).unwrap();
    // What it wrote in its own tables is added to the ledger, and nothing twice.
    let x = add(&old, Tx { account: 1, date: "2026-01-01", amount: -5, category: Some(3), peer: None });
    let y = add(&old, Tx { account: 2, date: "2026-01-01", amount: 5, category: None, peer: Some(x) });
    let z = add(&old, Tx { account: 1, date: "2026-01-01", amount: -5, category: None, peer: Some(y) });
    link(&old, y, z);
    old.execute("UPDATE transactions SET category_id=3 WHERE id=?1", [x]).unwrap();
    let (_, amount, _, hash) = imported().remove(0);
    old.execute(
        "INSERT INTO transactions(account_id,date,amount,import_hash) VALUES(1,'2026-02-01',?1,?2)",
        params![amount, hash],
    )
    .unwrap();
    old.execute("INSERT INTO rules(pattern,category_id) VALUES('late',3)", []).unwrap();
    old.execute_batch("INSERT INTO budgets VALUES (3,1), (2,7)").unwrap();
    let backups = || {
        std::fs::read_dir(&dir.0)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_str().unwrap().contains("backup"))
            .count()
    };
    assert_eq!(backups(), 1);
    let (db, _) = Db::open(&dir.file()).unwrap();
    assert_eq!(backups(), 2);
    assert_sound(&db, "leftovers");
    let now = v2_figures(&db);
    assert_eq!(now.register.len(), before.register.len() + 3);
    assert_eq!(now.accounts[0].2, before.accounts[0].2 - 10);
    assert_eq!(now.accounts[1].2, before.accounts[1].2 + 5);
    assert!(db.rules().unwrap().iter().any(|r| r.pattern == "late"));
    assert!(now.budgets["2026-01"].iter().any(|b| (b.0, b.1) == (2, 7)));
    assert!(now.budgets["2026-01"].iter().any(|b| (b.0, b.1) == (3, 40_000)));
    assert!(!db.has_table("rules").unwrap() && !db.has_table("transactions").unwrap());
}

#[test]
fn programs_that_open_the_file_together_migrate_it_once() {
    let dir = Dir::new("together");
    let old = seed_v1(&dir.file(), 1, 12);
    let before = v1_figures(&old);
    drop(old);
    let path = dir.file();
    std::thread::scope(|s| {
        for _ in 0..6 {
            s.spawn(|| Db::open(&path).unwrap());
        }
    });
    assert_eq!(v2_figures(&Db::open(&path).unwrap().0), before);
    assert!(dir.0.join("books.v1-backup.db").exists());
    assert!(!dir.0.join("books.v1-backup-2.db").exists());
}

/// 0.2.0 let people schedule a transfer between currencies.
#[test]
fn a_scheduled_transfer_between_currencies_does_not_stop_the_others() {
    let dir = Dir::new("scheduled");
    let old = seed_v1(&dir.file(), 1, 13);
    old.execute_batch(
        "INSERT INTO scheduled(id,account_id,to_account_id,amount,payee,next_date) VALUES
            (90,1,9,100,'To dollars','2026-01-01'), (91,1,NULL,-5,'Fee','2026-01-01');",
    )
    .unwrap();
    drop(old);
    let (db, _) = Db::open(&dir.file()).unwrap();
    let p = db.post_scheduled(s2d("2026-01-02")).unwrap();
    assert_eq!((p.posted, p.failed.len(), p.failed[0].payee.as_str()), (1, 1, "To dollars"));
    assert_sound(&db, "scheduled");
}

#[test]
fn a_file_without_some_of_its_tables_opens() {
    for (n, gone) in
        ["categories", "budgets", "rules", "scheduled", "meta", "transactions", "accounts"].iter().enumerate()
    {
        for version in [0, 1] {
            let dir = Dir::new(&format!("gone-{n}-{version}"));
            let c = create(&dir.file(), version);
            c.execute_batch(
                "INSERT INTO accounts(id,name,kind,opening_balance) VALUES (1,'A','bank',10);
                 INSERT INTO transactions(account_id,date,amount) VALUES (1,'2026-01-01',5);",
            )
            .unwrap();
            c.execute_batch(&format!("DROP TABLE {gone}")).unwrap();
            drop(c);
            let (db, _) = Db::open(&dir.file()).unwrap_or_else(|e| panic!("{gone}: {e:#}"));
            assert_sound(&db, gone);
            let balance = db.find_account("A").map(|a| a.balance).unwrap_or(0);
            assert_eq!(
                balance,
                match *gone {
                    "accounts" => 0,
                    "transactions" => 10,
                    _ => 15,
                },
                "{gone}"
            );
            db.budget_status(today()).unwrap();
            assert!(db.rules().unwrap().is_empty() && db.scheduled().unwrap().is_empty());
        }
    }
}

#[test]
fn currencies_are_normalized() {
    let dir = Dir::new("currencies");
    let c = create(&dir.file(), 1);
    for (id, cur) in ["€", " euro ", "EUROS", "Euro", "", "eur", "$", "usd"].iter().enumerate() {
        c.execute(
            "INSERT INTO accounts(id,name,kind,currency) VALUES(?1,?2,'bank',?3)",
            params![id as i64 + 1, format!("A{id}"), cur],
        )
        .unwrap();
    }
    let a = add(&c, Tx { account: 1, date: "2026-01-01", amount: -500, category: None, peer: None });
    let b = add(&c, Tx { account: 4, date: "2026-01-01", amount: 500, category: None, peer: Some(a) });
    link(&c, a, b);
    drop(c);
    let (db, _) = Db::open(&dir.file()).unwrap();
    let mut all: Vec<_> = db.accounts(true).unwrap().into_iter().map(|a| (a.id, a.currency)).collect();
    all.sort();
    let all: Vec<_> = all.into_iter().map(|a| a.1).collect();
    assert_eq!(all, ["EUR", "EUR", "EUR", "EUR", "EUR", "EUR", "$", "USD"]);
    assert_eq!(db.transaction(a).unwrap().transfer_peer, Some(b));
    // What cannot be guessed is put right by hand.
    db.set_account_currency(7, "USD").unwrap();
    assert_sound(&db, "currencies");
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> =
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.retain(|n| !n.ends_with("-wal") && !n.ends_with("-shm"));
    v.sort();
    v
}

#[test]
fn a_backup_that_fails_leaves_no_file() {
    let dir = Dir::new("backup-fails");
    let c = Connection::open(dir.file()).unwrap();
    c.execute_batch(V1_SCHEMA).unwrap();
    for i in 0..3000 {
        c.execute("INSERT INTO categories(name) VALUES(?1)", [format!("{i:0200}")]).unwrap();
    }
    c.execute_batch("INSERT INTO accounts(id,name,kind) VALUES(1,'A','bank')").unwrap();
    drop(c);
    // A page in the middle is spoiled: the copy is made but is not sound.
    let mut bytes = std::fs::read(dir.file()).unwrap();
    let at = bytes.len() / 2 / 4096 * 4096;
    bytes[at..at + 4096].fill(0xff);
    std::fs::write(dir.file(), &bytes).unwrap();
    assert!(Db::open(&dir.file()).is_err());
    assert_eq!(names(&dir.0), ["books.db"]);
}

#[cfg(unix)]
#[test]
fn a_backup_has_the_permissions_of_the_file() {
    use std::os::unix::fs::PermissionsExt;
    for mode in [0o600, 0o640] {
        let dir = Dir::new(&format!("mode-{mode:o}"));
        drop(seed_v1(&dir.file(), 1, 14));
        std::fs::set_permissions(dir.file(), std::fs::Permissions::from_mode(mode)).unwrap();
        Db::open(&dir.file()).unwrap();
        assert_eq!(names(&dir.0), ["books.db", "books.v1-backup.db"]);
        let m = std::fs::metadata(dir.0.join("books.v1-backup.db")).unwrap().permissions().mode();
        assert_eq!(m & 0o777, mode);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_path_that_is_not_utf8_is_migrated() {
    use std::os::unix::ffi::OsStrExt;
    let dir = Dir::new("bytes");
    let folder = dir.0.join(std::ffi::OsStr::from_bytes(b"caf\xe9"));
    std::fs::create_dir(&folder).unwrap();
    let file = folder.join(std::ffi::OsStr::from_bytes(b"b\xff.db"));
    let old = seed_v1(&file, 1, 15);
    let before = v1_figures(&old);
    let old_dump = dump(&old);
    drop(old);
    assert_eq!(v2_figures(&Db::open(&file).unwrap().0), before);
    let copy = folder.join(std::ffi::OsStr::from_bytes(b"b\xff.v1-backup.db"));
    assert_eq!(dump(&Connection::open(copy).unwrap()), old_dump);
}

/// A window of format 7 that has the file open can write no posting and apply no rule any more.
#[test]
fn a_window_of_format_7_with_the_file_open_is_kept_out() {
    let dir = Dir::new("format-7-open");
    let old = Connection::open(dir.file()).unwrap();
    old.execute_batch(include_str!("v7.sql")).unwrap();
    old.execute_batch(
        "PRAGMA foreign_keys=ON;
         INSERT INTO accounts(id,name,kind) VALUES (1,'Bank','bank');
         INSERT INTO categories(id,name) VALUES (1,'Home'), (2,'Food');
         INSERT INTO category_rules VALUES (9,'late',1,''), (3,'shop|bar ',2,'Shop');
         INSERT INTO csv_layouts(account_id,signature,headers,settings) VALUES (1,'s','[]','{}');
         INSERT INTO entries(id,date,payee) VALUES (1,'2026-01-05','Shop');
         INSERT INTO ledger_postings(id,entry_id,ledger_account_id,amount,currency,import_hash,other_hashes)
         SELECT 1,1,id,-500,'EUR','h','src:csv' FROM ledger_accounts WHERE money_account_id=1 UNION ALL
         SELECT 2,1,id,500,'EUR',NULL,NULL FROM ledger_accounts WHERE system='uncategorized';",
    )
    .unwrap();
    old.pragma_update(None, "user_version", 7).unwrap();
    let file = dump(&old);
    let mut ready = old.prepare("SELECT id FROM ledger_postings WHERE entry_id=?1").unwrap();
    let mut rules = old.prepare("SELECT id, pattern, category_id, rename_payee FROM category_rules").unwrap();
    assert!(ready.exists([1]).unwrap() && rules.exists([]).unwrap());
    let (db, _) = Db::open(&dir.file()).unwrap();
    assert_eq!(dump(&Connection::open(dir.0.join("books.v7-backup.db")).unwrap()), file);
    let order: Vec<_> =
        db.rules().unwrap().into_iter().map(|r| (r.id, r.pattern, r.category, r.rename_payee)).collect();
    let rule = |id, p: &str, c: &str, n: &str| (id, p.to_string(), c.to_string(), n.to_string());
    assert_eq!(order, [rule(3, "shop|bar ", "Food", "Shop"), rule(9, "late", "Home", "")]);
    assert_eq!(db.layouts().unwrap().len(), 1);
    db.apply_rules().unwrap();
    db.set_tags(1, &["tag".into()]).unwrap();
    let before = dump(&db.conn);
    assert!(ready.exists([1]).is_err() && rules.exists([]).is_err());
    for sql in [
        "INSERT INTO ledger_postings(entry_id,ledger_account_id,amount,currency) VALUES(1,1,5,'EUR')",
        "UPDATE ledger_postings SET payee='x' WHERE id=1",
        "DELETE FROM ledger_postings WHERE id=1",
        "INSERT INTO category_rules(pattern,category_id,rename_payee) VALUES('x',1,'')",
        "DELETE FROM category_rules WHERE id=3",
        "SELECT id FROM ledger_postings p JOIN entries e ON e.id=p.entry_id",
        "BEGIN; INSERT INTO entries(date) VALUES('2026-01-01');
         INSERT INTO ledger_postings(entry_id,ledger_account_id,amount,currency) VALUES(last_insert_rowid(),1,5,'EUR'); COMMIT;",
    ] {
        assert!(old.execute_batch(sql).is_err(), "{sql}");
        if !old.is_autocommit() {
            old.execute_batch("ROLLBACK").unwrap();
        }
        assert_eq!(dump(&db.conn), before, "{sql}");
    }
    drop((ready, rules));
    // It cannot open the file again, and the categories it can still change stay sound.
    old.execute("INSERT INTO categories(name) VALUES('New')", []).unwrap();
    old.execute("DELETE FROM entries WHERE id=1", []).unwrap();
    assert_sound(&db, "kept out");
}

/// The tables of a new file are those of a file that was migrated.
#[test]
fn a_migrated_file_is_like_a_new_one() {
    let columns = |db: &Db| -> Vec<String> {
        let sql =
            "SELECT m.name || '.' || c.name || ' ' || c.type || ' ' || c.[notnull] || ' ' || COALESCE(c.dflt_value,'')
                   FROM sqlite_schema m JOIN pragma_table_info(m.name) c
                   WHERE m.type='table' AND m.name NOT LIKE 'sqlite%' ORDER BY 1";
        let mut st = db.conn.prepare(sql).unwrap();
        st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    let new = columns(&Db::open_in_memory().unwrap());
    for (version, schema) in [
        (2, include_str!("v2.sql")),
        (5, include_str!("v5.sql")),
        (6, include_str!("v6.sql")),
        (7, include_str!("v7.sql")),
    ] {
        let dir = Dir::new(&format!("like-new-{version}"));
        let old = Connection::open(dir.file()).unwrap();
        old.execute_batch(schema).unwrap();
        old.pragma_update(None, "user_version", version).unwrap();
        drop(old);
        assert_eq!(columns(&Db::open(&dir.file()).unwrap().0), new, "{version}");
    }
}
