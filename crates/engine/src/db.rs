//! Archivio dati di CashCraft (SQLite): conti, movimenti, categorie, budget,
//! regole di categorizzazione automatica e movimenti pianificati.
//! Gli importi sono sempre in centesimi (i64): negativi = uscite, positivi = entrate.

use crate::cards::CARD_SCHEMA;
use crate::envelopes::ENVELOPE_SCHEMA;
use crate::i18n::{Lang, lang, tr, trf, trn, with_lang};
use crate::layouts::LAYOUT_SCHEMA;
use crate::ledger::{self, COUNTED, Draft, LEDGER_SCHEMA, Line, PROFIT_AND_LOSS, UNCATEGORIZED};
use crate::money::checked_total;
use crate::reconcile::{POSTING_IDS, RECONCILE_SCHEMA};
use crate::rules::{Marks, RULE_SCHEMA, RuleUse, Ruleset, Subject, has_tag, stored_tags};
use anyhow::{Context, Result, anyhow, bail};
use chrono::{Datelike, Days, Local, Months, NaiveDate};
use rusqlite::{Connection, OptionalExtension, Transaction as SqlTransaction, TransactionBehavior, params};
use serde::Serialize;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub type Cents = i64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    Bank,
    Cash,
    CreditCard,
    Savings,
    Investment,
    Loan,
}

impl AccountKind {
    pub const ALL: [AccountKind; 6] = [
        AccountKind::Bank,
        AccountKind::Cash,
        AccountKind::CreditCard,
        AccountKind::Savings,
        AccountKind::Investment,
        AccountKind::Loan,
    ];

    pub fn code(self) -> &'static str {
        match self {
            AccountKind::Bank => "bank",
            AccountKind::Cash => "cash",
            AccountKind::CreditCard => "credit_card",
            AccountKind::Savings => "savings",
            AccountKind::Investment => "investment",
            AccountKind::Loan => "loan",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase();
        Some(match s.as_str() {
            "bank" | "banca" | "conto" | "conto corrente" | "checking" | "current account" => AccountKind::Bank,
            "cash" | "contanti" | "portafoglio" | "wallet" => AccountKind::Cash,
            "credit_card" | "credit card" | "carta" | "carta di credito" | "card" => AccountKind::CreditCard,
            "savings" | "savings account" | "deposit" | "risparmio" | "risparmi" | "deposito" | "conto deposito" => {
                AccountKind::Savings
            }
            "investment" | "investments" | "investimenti" | "investimento" | "titoli" => AccountKind::Investment,
            "loan" | "mortgage" | "prestito" | "mutuo" | "debito" => AccountKind::Loan,
            _ => return None,
        })
    }

    /// Title of the group in the sidebar.
    pub fn group_label(self) -> &'static str {
        match self {
            AccountKind::Bank => tr("Current accounts"),
            AccountKind::Cash => tr("Cash"),
            AccountKind::CreditCard => tr("Credit cards"),
            AccountKind::Savings => tr("Savings"),
            AccountKind::Investment => tr("Investments"),
            AccountKind::Loan => tr("Loans and mortgages"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AccountKind::Bank => tr("Current account"),
            AccountKind::Cash => tr("Cash"),
            AccountKind::CreditCard => tr("Credit card"),
            AccountKind::Savings => tr("Savings account"),
            AccountKind::Investment => tr("Investment"),
            AccountKind::Loan => tr("Loan / mortgage"),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Account {
    pub id: i64,
    pub name: String,
    pub kind: AccountKind,
    pub currency: String,
    pub opening_balance: Cents,
    pub iban: String,
    pub archived: bool,
    pub balance: Cents,
}

#[derive(Clone, Debug, Serialize)]
pub struct Category {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub is_income: bool,
    /// Nome completo "Padre:Figlio".
    pub path: String,
}

/// What is on the other side of a transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TxKind {
    Category,
    Uncategorized,
    /// Another account of the user.
    Transfer,
    /// An account of the chart that is neither an account of the user nor a category.
    Other,
    /// Several postings.
    Split,
}

/// A posting on an account of the user. The opening balance is not one.
#[derive(Clone, Debug, Serialize)]
pub struct Transaction {
    /// The id of the posting.
    pub id: i64,
    pub entry_id: i64,
    pub kind: TxKind,
    pub account_id: i64,
    pub account: String,
    pub date: NaiveDate,
    pub amount: Cents,
    pub payee: String,
    pub memo: String,
    pub category_id: Option<i64>,
    pub category: String,
    pub cleared: bool,
    /// Checked against a statement by a reconciliation that is finished: see `docs/reconcile.md`.
    pub reconciled: bool,
    pub transfer_peer: Option<i64>,
    /// A side of a transfer that was added and not seen on a statement yet.
    pub added: bool,
    /// A row of a card statement.
    #[serde(skip)]
    pub card_row: bool,
    pub tags: Vec<String>,
    /// What the bank wrote, when the payee or the notes are others.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_payee: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_memo: Option<String>,
    /// The rules that set its category, payee or tags, by id.
    #[serde(skip_serializing_if = "RuleUse::is_empty")]
    pub rules: RuleUse,
    /// What was typed before a rule changed the payee or the notes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typed_payee: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typed_memo: Option<String>,
    #[serde(skip)]
    pub(crate) marks: Marks,
    /// The texts of before, whoever wrote them, and the import keys.
    #[serde(skip)]
    pub(crate) source_payee: Option<String>,
    #[serde(skip)]
    pub(crate) source_memo: Option<String>,
    #[serde(skip)]
    pub(crate) keys: String,
    /// Saldo progressivo (solo quando si filtra per un conto).
    pub balance: Option<Cents>,
}

#[derive(Clone, Debug, Default)]
pub struct NewTx {
    pub account_id: i64,
    pub date: NaiveDate,
    pub amount: Cents,
    pub payee: String,
    pub memo: String,
    pub category_id: Option<i64>,
    pub cleared: bool,
    pub import_hash: Option<String>,
    /// What else the import knows it by, separated by spaces.
    pub import_marks: Option<String>,
    /// A row of a card statement.
    pub card_row: bool,
}

/// The editable part of a transaction.
#[derive(Clone, Debug, Default)]
pub struct TxEdit {
    pub date: NaiveDate,
    pub amount: Cents,
    pub payee: String,
    pub memo: String,
    pub category_id: Option<i64>,
    pub cleared: bool,
}

#[derive(Clone, Debug, Default)]
pub struct NewScheduled {
    pub account_id: i64,
    pub to_account_id: Option<i64>,
    pub amount: Cents,
    pub payee: String,
    pub memo: String,
    pub category_id: Option<i64>,
    pub next_date: NaiveDate,
    pub every: i64,
    pub unit: String,
}

#[derive(Clone, Debug, Default)]
pub struct TxFilter {
    pub account_id: Option<i64>,
    pub category_id: Option<i64>,
    pub uncategorized: bool,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    pub search: String,
    /// Only transactions with this tag.
    pub tag: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Scheduled {
    pub id: i64,
    pub account_id: i64,
    pub account: String,
    pub to_account_id: Option<i64>,
    pub amount: Cents,
    pub payee: String,
    pub memo: String,
    pub category_id: Option<i64>,
    pub category: String,
    pub next_date: NaiveDate,
    pub every: i64,
    pub unit: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CategoryTotal {
    pub category_id: Option<i64>,
    pub category: String,
    pub is_income: bool,
    pub total: Cents,
    /// The part of it in entries with several categories.
    #[serde(skip)]
    pub shared: Cents,
}

impl CategoryTotal {
    /// Income and expense: a category counts in the side its total falls on; what is in entries with
    /// several categories stays in the side of its class, so that moving money between two categories
    /// is neither.
    pub fn income_and_expense(totals: &[CategoryTotal]) -> Result<(Cents, Cents)> {
        let (mut income, mut expense): (Cents, Cents) = (0, 0);
        for t in totals {
            let own = t.total.checked_sub(t.shared).ok_or_else(ledger::too_big)?;
            let class = t.is_income && t.category_id.is_some();
            for (c, is_income) in [(own, own > 0), (t.shared, class)] {
                let side = if is_income { &mut income } else { &mut expense };
                let c = if is_income { Some(c) } else { c.checked_neg() };
                *side = c.and_then(|c| side.checked_add(c)).ok_or_else(ledger::too_big)?;
            }
        }
        Ok((income, expense))
    }
}

/// What posting the scheduled transactions did.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Posted {
    pub posted: usize,
    pub failed: Vec<FailedScheduled>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FailedScheduled {
    pub id: i64,
    pub payee: String,
    pub error: String,
}

impl Posted {
    /// What to tell people.
    pub fn message(&self) -> String {
        let mut out = trn(self.posted, "{n} transaction recorded", "{n} transactions recorded");
        for f in &self.failed {
            out.push_str("; ");
            out.push_str(&trf("\"{name}\" not recorded: {error}", &[("name", &f.payee), ("error", &f.error)]));
        }
        out
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct MonthTotal {
    pub month: String,
    pub income: Cents,
    pub expense: Cents,
}

#[derive(Clone, Debug, Serialize)]
pub struct BudgetLine {
    pub category_id: i64,
    pub category: String,
    pub budget: Cents,
    pub spent: Cents,
}

pub struct Db {
    pub conn: Connection,
    pub path: Option<PathBuf>,
    /// Whether the ids of new rows are unlike those of any other computer: see [`Db::new_id`].
    pub(crate) spread_ids: bool,
    /// The postings that the transaction in progress changes or deletes, with the rows of the reconciliations that
    /// they agreed with when it touched them first: see [`Db::check_reconciled`].
    pub(crate) touched: RefCell<HashMap<i64, Vec<i64>>>,
    /// A change to reconciled transactions undoes their reconciliation instead of being refused.
    pub(crate) unlocking: Cell<bool>,
}

/// Version of the data file format, kept in `PRAGMA user_version` (0 is CashCraft 0.1, 1 is 0.2.0 with
/// the same tables, 2 the double-entry ledger, 3 adds dates of single postings, 4 was never
/// released, 5 tells the added sides of transfers, 6 the card statements, 7 the saved columns of CSV
/// files and the other import keys of a posting, in a table
/// with another name so that an older program fails to write, 8 the rules with conditions and order, the
/// tags and the texts of the bank, in tables with other names, 9 the reconciliations with the bank, with the postings
/// in a table with another name again, 10 the envelopes: see `docs/ledger.md`, `docs/reconcile.md` and `docs/envelopes.md`).
pub const SCHEMA_VERSION: i64 = 10;

/// Tables of formats 0 and 1 with their name in 2. The rules have another one since 8. A window of an older program that has the file open
/// fails at once when it writes to one, instead of doing half of what it was asked.
pub(crate) const RENAMED: [(&str, &str); 3] =
    [("budgets", "category_budgets"), ("rules", "category_rules"), ("scheduled", "scheduled_items")];

/// The tables beside the ledger. Formats 0 and 1 also had `accounts.opening_balance`.
pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS accounts (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    kind TEXT NOT NULL,
    currency TEXT NOT NULL DEFAULT 'EUR',
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
CREATE TABLE IF NOT EXISTS category_budgets (
    category_id INTEGER PRIMARY KEY REFERENCES categories(id) ON DELETE CASCADE,
    monthly INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS scheduled_items (
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

pub fn default_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("CASHCRAFT_DB")
        && !p.trim().is_empty()
    {
        return PathBuf::from(p);
    }
    private_db_path()
}

/// The data file of a user who chose none.
pub(crate) fn private_db_path() -> PathBuf {
    crate::paths::data_dir().join("CashCraft").join("cashcraft.db")
}

pub(crate) fn no_transaction(id: i64) -> anyhow::Error {
    anyhow!(trf("transaction {id} not found", &[("id", &id)]))
}

pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

pub(crate) fn d2s(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

pub(crate) fn s2d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap_or_else(|_| today())
}

impl Db {
    /// Opens the data file, or creates it. Also returns whether the file is new.
    pub fn open(path: &Path) -> Result<(Db, bool)> {
        Self::open_seeding(path, lang())
    }

    /// [`Db::open`] with the language of the categories and rules of a new file.
    pub fn open_seeding(path: &Path, seed: Lang) -> Result<(Db, bool)> {
        Self::open_with(path, seed, false)
    }

    /// The working copy of a file that several computers change.
    pub fn open_copy(path: &Path, seed: Lang) -> Result<(Db, bool)> {
        Self::open_with(path, seed, true)
    }

    fn open_with(path: &Path, seed: Lang, spread_ids: bool) -> Result<(Db, bool)> {
        let is_new = !path.exists();
        if let Some(dir) = path.parent()
            && !dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(dir).with_context(|| trf("cannot create {path}", &[("path", &dir.display())]))?;
        }
        let conn = Connection::open(path).with_context(|| trf("cannot open {path}", &[("path", &path.display())]))?;
        let db = Db::new(conn, Some(path.to_path_buf()), spread_ids);
        db.init()?;
        if is_new {
            with_lang(seed, || db.seed_defaults())?;
        }
        Ok((db, is_new))
    }

    pub(crate) fn new(conn: Connection, path: Option<PathBuf>, spread_ids: bool) -> Db {
        Db { conn, path, spread_ids, touched: RefCell::default(), unlocking: Cell::new(false) }
    }

    pub fn open_in_memory() -> Result<Db> {
        let db = Db::new(Connection::open_in_memory()?, None, false);
        db.init()?;
        db.seed_defaults()?;
        Ok(db)
    }

    pub(crate) fn init(&self) -> Result<()> {
        self.conn.busy_timeout(std::time::Duration::from_secs(5))?;
        self.conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let _ = self.conn.execute_batch("PRAGMA journal_mode = WAL;");
        if self.format()? == SCHEMA_VERSION {
            return Ok(());
        }
        // One process at a time: the version is read again once the file is ours.
        self.atomically(|| {
            let version = self.format()?;
            if version == SCHEMA_VERSION {
                return Ok(());
            }
            if version > SCHEMA_VERSION {
                bail!(trf(
                    "the data file was written by a newer version of CashCraft (format {version})",
                    &[("version", &version)]
                ));
            }
            let mut old = self.has_table("accounts")? || self.has_table("transactions")?;
            for t in RENAMED.iter().map(|t| t.0).chain(["categories", "meta"]) {
                old = old || self.has_table(t)?;
            }
            if old {
                self.migrate(version)?;
            } else {
                self.conn.execute_batch(SCHEMA)?;
                self.conn.execute_batch(LEDGER_SCHEMA)?;
                self.conn.execute_batch(CARD_SCHEMA)?;
                self.conn.execute_batch(LAYOUT_SCHEMA)?;
                self.conn.execute_batch(RULE_SCHEMA)?;
                self.conn.execute_batch(RECONCILE_SCHEMA)?;
                self.conn.execute_batch(POSTING_IDS)?;
                self.conn.execute_batch(ENVELOPE_SCHEMA)?;
            }
            self.conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            Ok(())
        })
    }

    fn format(&self) -> Result<i64> {
        Ok(self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    pub(crate) fn has_table(&self, name: &str) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name=?1 COLLATE NOCASE",
            [name],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// The id of a new row of a table, for its `INSERT`: `None` lets SQLite give the next number. In a file that several
    /// computers change, a number that none of them gives, so that an id which somebody remembers is of the same row or
    /// of none. It grows, as ids are the order in which rows were written: minutes since 2026, then eight random digits.
    pub(crate) fn new_id(&self, table: &str) -> Result<Option<i64>> {
        /// What a number of JSON holds exactly.
        const MOST: i64 = 9_000_000_000_000_000;
        const RANDOM: i64 = 100_000_000;
        if !self.spread_ids {
            return Ok(None);
        }
        let (last, random): (i64, i64) = self
            .conn
            .prepare_cached(&format!("SELECT COALESCE(MAX(id),0), random() FROM {table}"))?
            .query_row([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let minutes = (chrono::Utc::now().timestamp() - 1_767_225_600).max(60) / 60;
        let digits = random.rem_euclid(RANDOM);
        let id = minutes.saturating_mul(RANDOM).saturating_add(digits).max(last.saturating_add(1 + digits));
        if id <= MOST {
            return Ok(Some(id));
        }
        // A clock, or a file, that went too far: any number that is free.
        let mut id = random;
        loop {
            id = id.rem_euclid(MOST);
            let sql = format!("SELECT COUNT(*) FROM {table} WHERE id=?1");
            if id >= RANDOM && self.conn.query_row(&sql, [id], |r| r.get::<_, i64>(0))? == 0 {
                return Ok(Some(id));
            }
            id = self.conn.query_row("SELECT random()", [], |r| r.get(0))?;
        }
    }

    /// A new account or category, whose row in the chart a trigger has just added, gets its id there.
    pub(crate) fn place_in_chart(&self, column: &str, id: i64) -> Result<()> {
        if let Some(chart) = self.new_id("ledger_accounts")? {
            self.conn.execute(&format!("UPDATE ledger_accounts SET id=?1 WHERE {column}=?2"), params![chart, id])?;
        }
        Ok(())
    }

    /// Adds a category.
    pub(crate) fn insert_category(&self, name: &str, parent: Option<i64>, income: bool) -> Result<i64> {
        self.atomically(|| {
            self.conn.execute(
                "INSERT INTO categories(id,name,parent_id,is_income) VALUES(?1,?2,?3,?4)",
                params![self.new_id("categories")?, name, parent, income as i64],
            )?;
            let id = self.conn.last_insert_rowid();
            self.place_in_chart("category_id", id)?;
            Ok(id)
        })
    }

    /// Cambia ogni volta che un altro processo (CLI, MCP) modifica l'archivio.
    pub fn data_version(&self) -> i64 {
        self.conn.query_row("PRAGMA data_version", [], |r| r.get(0)).unwrap_or(0)
    }

    pub fn meta(&self, key: &str) -> Option<String> {
        self.conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0)).optional().ok().flatten()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    fn seed_defaults(&self) -> Result<()> {
        let expense: &[&str] = &[
            tr("Home:Rent and mortgage"),
            tr("Home:Utilities"),
            tr("Home:Maintenance"),
            tr("Groceries"),
            tr("Restaurants and bars"),
            tr("Transport:Fuel"),
            tr("Transport:Public transport"),
            tr("Transport:Car"),
            tr("Health"),
            tr("Leisure"),
            tr("Clothing"),
            tr("Travel"),
            tr("Subscriptions"),
            tr("Bank and taxes:Fees"),
            tr("Bank and taxes:Taxes"),
            tr("Education"),
            tr("Gifts"),
            tr("Other expenses"),
        ];
        let income: &[&str] = &[tr("Salary"), tr("Refunds"), tr("Interest"), tr("Other income")];
        for p in expense {
            self.category_by_path(p, false, true)?;
        }
        for p in income {
            self.category_by_path(p, true, true)?;
        }
        // Italian keywords in every language: they match the wording of Italian banks.
        let rules: &[(&str, &str)] = &[
            ("supermercato|alimentari|market|discount|ipermercato", tr("Groceries")),
            ("ristorante|pizzeria|bar |trattoria|osteria|caffe", tr("Restaurants and bars")),
            ("carburante|benzina|distributore|rifornimento", tr("Transport:Fuel")),
            ("treno|autobus|metro|biglietto|trasporto pubblico", tr("Transport:Public transport")),
            ("farmacia|medico|ospedale|ticket sanitario", tr("Health")),
            ("energia|luce e gas|gas |acqua|bolletta|utenza", tr("Home:Utilities")),
            ("affitto|canone locazione|rata mutuo", tr("Home:Rent and mortgage")),
            ("commissione|commissioni|canone conto|spese tenuta", tr("Bank and taxes:Fees")),
            ("imposta di bollo|f24|imposta", tr("Bank and taxes:Taxes")),
            ("stipendio|emolumenti|retribuzione", tr("Salary")),
            ("interessi creditori|interessi", tr("Interest")),
            ("abbonamento|streaming", tr("Subscriptions")),
        ];
        for (pat, cat) in rules {
            let cid = self.category_by_path(cat, false, false)?;
            self.add_rule(pat, cid, "")?;
        }
        self.set_meta("created", &d2s(today()))?;
        Ok(())
    }

    // ---------------------------------------------------------------- conti

    pub fn accounts(&self, include_archived: bool) -> Result<Vec<Account>> {
        let mut st = self.conn.prepare(
            "SELECT a.id, a.name, a.kind, a.currency,
                    COALESCE((SELECT SUM(p.amount) FROM journal_postings p JOIN entries e ON e.id=p.entry_id
                              WHERE p.ledger_account_id=l.id AND e.opening_account_id=a.id),0),
                    a.iban, a.archived,
                    COALESCE((SELECT SUM(p.amount) FROM journal_postings p WHERE p.ledger_account_id=l.id),0)
             FROM accounts a LEFT JOIN ledger_accounts l ON l.money_account_id=a.id
             ORDER BY a.name COLLATE NOCASE",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Account {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: AccountKind::from_code(&r.get::<_, String>(2)?).unwrap_or(AccountKind::Bank),
                currency: r.get(3)?,
                opening_balance: r.get(4)?,
                iban: r.get(5)?,
                archived: r.get::<_, i64>(6)? != 0,
                balance: r.get(7)?,
            })
        })?;
        let mut out = Vec::new();
        for a in rows {
            let a = a?;
            if include_archived || !a.archived {
                out.push(a);
            }
        }
        Ok(out)
    }

    pub fn add_account(
        &self,
        name: &str,
        kind: AccountKind,
        currency: &str,
        opening: Cents,
        iban: &str,
    ) -> Result<i64> {
        let name = name.trim();
        if name.is_empty() {
            bail!(tr("the account needs a name"));
        }
        let currency = ledger::check_currency(currency)?;
        self.atomically(|| {
            self.conn
                .execute(
                    "INSERT INTO accounts(id,name,kind,currency,iban) VALUES(?1,?2,?3,?4,?5)",
                    params![self.new_id("accounts")?, name, kind.code(), currency, iban],
                )
                .with_context(|| trf("is there already an account named \"{name}\"?", &[("name", &name)]))?;
            let id = self.conn.last_insert_rowid();
            self.place_in_chart("money_account_id", id)?;
            self.set_opening(id, opening)?;
            Ok(id)
        })
    }

    pub fn update_account(
        &self,
        id: i64,
        name: &str,
        kind: AccountKind,
        opening: Cents,
        iban: &str,
        archived: bool,
    ) -> Result<()> {
        self.atomically(|| {
            self.conn.execute(
                "UPDATE accounts SET name=?2, kind=?3, iban=?4, archived=?5 WHERE id=?1",
                params![id, name.trim(), kind.code(), iban, archived as i64],
            )?;
            self.set_opening(id, opening)
        })
    }

    /// Gives an account an IBAN, and changes nothing else of it.
    pub fn set_account_iban(&self, id: i64, iban: &str) -> Result<()> {
        self.conn.execute("UPDATE accounts SET iban=?2 WHERE id=?1", params![id, iban.trim()])?;
        Ok(())
    }

    /// Deletes an account and its transactions. The other side of its transfers stays in the other
    /// account, against the opening balances.
    pub fn delete_account(&self, id: i64) -> Result<()> {
        self.atomically(|| {
            self.drop_money_ledger(id)?;
            self.conn.execute("DELETE FROM accounts WHERE id=?1", [id])?;
            Ok(())
        })
    }

    /// Trova un conto per id o per nome (senza distinguere maiuscole).
    pub fn find_account(&self, key: &str) -> Result<Account> {
        let key = key.trim();
        let all = self.accounts(true)?;
        if let Ok(id) = key.parse::<i64>()
            && let Some(a) = all.iter().find(|a| a.id == id)
        {
            return Ok(a.clone());
        }
        let lk = key.to_lowercase();
        if let Some(a) = all.iter().find(|a| a.name.to_lowercase() == lk) {
            return Ok(a.clone());
        }
        let partial: Vec<_> = all.iter().filter(|a| a.name.to_lowercase().contains(&lk)).collect();
        if let [only] = partial.as_slice() {
            return Ok((*only).clone());
        }
        Err(anyhow!(trf("account \"{name}\" not found", &[("name", &key)])))
    }

    // ------------------------------------------------------------ categorie

    pub fn categories(&self) -> Result<Vec<Category>> {
        let mut st = self.conn.prepare("SELECT id, name, parent_id, is_income FROM categories")?;
        let raw: Vec<(i64, String, Option<i64>, bool)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? != 0)))?
            .collect::<std::result::Result<_, _>>()?;
        let names: HashMap<i64, (String, Option<i64>)> =
            raw.iter().map(|(id, n, p, _)| (*id, (n.clone(), *p))).collect();
        let mut out: Vec<Category> = raw
            .into_iter()
            .map(|(id, name, parent_id, is_income)| {
                let mut path = name.clone();
                let mut cur = parent_id;
                let mut guard = 0;
                while let Some(p) = cur {
                    guard += 1;
                    if guard > 16 {
                        break;
                    }
                    if let Some((pn, pp)) = names.get(&p) {
                        path = format!("{pn}:{path}");
                        cur = *pp;
                    } else {
                        break;
                    }
                }
                Category { id, name, parent_id, is_income, path }
            })
            .collect();
        out.sort_by_key(|a| (a.is_income, a.path.to_lowercase()));
        Ok(out)
    }

    /// Restituisce l'id della categoria "Padre:Figlio", creandola se serve e se `create`.
    pub fn category_by_path(&self, path: &str, is_income: bool, create: bool) -> Result<i64> {
        let mut parent: Option<i64> = None;
        for part in path.split(':').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            let found: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM categories WHERE name = ?1 COLLATE NOCASE AND parent_id IS ?2",
                    params![part, parent],
                    |r| r.get(0),
                )
                .optional()?;
            parent = Some(match found {
                Some(id) => id,
                None if create => self.insert_category(part, parent, is_income)?,
                None => bail!(trf("category \"{name}\" not found", &[("name", &path)])),
            });
        }
        parent.ok_or_else(|| anyhow!(tr("the category needs a name")))
    }

    /// Trova una categoria per id, percorso completo o nome (anche parziale se univoco).
    pub fn find_category(&self, key: &str) -> Result<Category> {
        let key = key.trim();
        let all = self.categories()?;
        if let Ok(id) = key.parse::<i64>()
            && let Some(c) = all.iter().find(|c| c.id == id)
        {
            return Ok(c.clone());
        }
        let lk = key.to_lowercase();
        if let Some(c) = all.iter().find(|c| c.path.to_lowercase() == lk) {
            return Ok(c.clone());
        }
        let by_name: Vec<_> = all.iter().filter(|c| c.name.to_lowercase() == lk).collect();
        if let [only] = by_name.as_slice() {
            return Ok((*only).clone());
        }
        let partial: Vec<_> = all.iter().filter(|c| c.path.to_lowercase().contains(&lk)).collect();
        if let [only] = partial.as_slice() {
            return Ok((*only).clone());
        }
        Err(anyhow!(trf("category \"{name}\" not found", &[("name", &key)])))
    }

    pub fn rename_category(&self, id: i64, name: &str) -> Result<()> {
        self.conn.execute("UPDATE categories SET name=?2 WHERE id=?1", params![id, name.trim()])?;
        Ok(())
    }

    /// Deletes a category and those under it; their transactions become uncategorized.
    pub fn delete_category(&self, id: i64) -> Result<()> {
        self.atomically(|| {
            let mut st = self.conn.prepare(
                "WITH RECURSIVE sub(id) AS (SELECT ?1 UNION SELECT c.id FROM categories c JOIN sub ON c.parent_id=sub.id)
                 SELECT id FROM sub",
            )?;
            let all: Vec<i64> = st.query_map([id], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
            for c in all {
                self.drop_category_ledger(c)?;
            }
            self.conn.execute("DELETE FROM categories WHERE id=?1", [id])?;
            Ok(())
        })
    }

    // ------------------------------------------------------------ movimenti

    pub fn transactions(&self, f: &TxFilter) -> Result<Vec<Transaction>> {
        let cats: HashMap<i64, String> = self.categories()?.into_iter().map(|c| (c.id, c.path)).collect();
        let mut uncategorized: HashSet<i64> = HashSet::new();
        if f.uncategorized {
            let spare = self.system_ledger(UNCATEGORIZED)?;
            let mut st = self.conn.prepare("SELECT entry_id FROM journal_postings WHERE ledger_account_id=?1")?;
            uncategorized = st.query_map([spare], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
        }
        let mut entry_categories: HashMap<i64, Vec<i64>> = HashMap::new();
        if f.category_id.is_some() {
            let mut st = self.conn.prepare(
                "SELECT p.entry_id, l.category_id FROM journal_postings p JOIN ledger_accounts l ON l.id=p.ledger_account_id
                 WHERE l.category_id IS NOT NULL",
            )?;
            for row in st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))? {
                let (entry, category) = row?;
                entry_categories.entry(entry).or_default().push(category);
            }
        }
        let opening = match f.account_id {
            Some(a) => self.accounts(true)?.iter().find(|x| x.id == a).map_or(0, |x| x.opening_balance),
            None => 0,
        };
        let mut sql = format!(
            "SELECT p.id, a.id, a.name, COALESCE(p.date, e.date), p.amount, COALESCE(p.payee, e.payee), COALESCE(p.memo, e.memo),
                    ol.category_id, p.cleared, o.id, e.id, oa.id, oa.name, ol.system, ol.name, p.origin,
                    p.original_payee, p.original_memo, p.tags, p.rules, ol.id,
                    p.import_hash, p.other_hashes, {LIVE}
             FROM journal_postings p JOIN entries e ON e.id=p.entry_id
             JOIN ledger_accounts l ON l.id=p.ledger_account_id JOIN accounts a ON a.id=l.money_account_id
             LEFT JOIN journal_postings o ON o.entry_id=p.entry_id AND o.id<>p.id
                  AND (SELECT COUNT(*) FROM journal_postings x WHERE x.entry_id=p.entry_id)=2
             LEFT JOIN ledger_accounts ol ON ol.id=o.ledger_account_id
             LEFT JOIN accounts oa ON oa.id=ol.money_account_id
             WHERE +e.opening_account_id IS NULL",
            LIVE = crate::reconcile::LIVE
        );
        // The plus keeps away from the index of the opening balances, which has every other entry.
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(a) = f.account_id {
            sql.push_str(" AND a.id=?");
            args.push(a.into());
        }
        sql.push_str(" ORDER BY COALESCE(p.date, e.date) ASC, p.id ASC");
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(args), |r| {
            let account_id: i64 = r.get(1)?;
            let category_id: Option<i64> = r.get(7)?;
            let peer: Option<i64> = r.get(9)?;
            let peer_account: Option<i64> = r.get(11)?;
            let peer_name: Option<String> = r.get(12)?;
            let system: Option<String> = r.get(13)?;
            let other_name: Option<String> = r.get(14)?;
            let kind = match (peer, peer_account, category_id) {
                (None, _, _) => TxKind::Split,
                (_, Some(a), _) if a == account_id => TxKind::Split,
                (_, Some(_), _) => TxKind::Transfer,
                (_, None, Some(_)) => TxKind::Category,
                _ if system.as_deref() == Some(UNCATEGORIZED) => TxKind::Uncategorized,
                _ => TxKind::Other,
            };
            let (payee, memo): (String, String) = (r.get(5)?, r.get(6)?);
            let tags = stored_tags(r.get::<_, Option<String>>(18)?.as_deref());
            let marks = Marks::read(r.get::<_, Option<String>>(19)?.as_deref());
            let (source_payee, source_memo): (Option<String>, Option<String>) = (r.get(16)?, r.get(17)?);
            let others = [source_payee.clone().filter(|p| *p != payee), source_memo.clone().filter(|m| *m != memo)];
            let ([original_payee, original_memo], [typed_payee, typed_memo]) =
                if marks.typed { ([None, None], others) } else { (others, [None, None]) };
            let keys = [r.get::<_, Option<String>>(21)?, r.get::<_, Option<String>>(22)?];
            Ok(Transaction {
                original_payee,
                original_memo,
                typed_payee,
                typed_memo,
                source_payee,
                source_memo,
                keys: keys.into_iter().flatten().collect::<Vec<_>>().join(" "),
                rules: RuleUse::of(&marks, &tags, r.get(20)?),
                tags,
                marks,
                id: r.get(0)?,
                entry_id: r.get(10)?,
                kind,
                account_id,
                account: r.get(2)?,
                date: s2d(&r.get::<_, String>(3)?),
                amount: r.get(4)?,
                payee,
                memo,
                category_id,
                category: match kind {
                    TxKind::Transfer => trf("[Transfer] {account}", &[("account", &peer_name.unwrap_or_default())]),
                    TxKind::Category => category_id.and_then(|c| cats.get(&c)).cloned().unwrap_or_default(),
                    TxKind::Uncategorized => String::new(),
                    TxKind::Other if system.is_some() => format!("[{}]", tr("Opening balances")),
                    TxKind::Other => format!("[{}]", other_name.unwrap_or_default()),
                    TxKind::Split => tr("Split").into(),
                },
                cleared: r.get::<_, i64>(8)? != 0,
                reconciled: r.get(23)?,
                transfer_peer: peer.filter(|_| kind == TxKind::Transfer),
                added: kind == TxKind::Transfer && r.get::<_, Option<String>>(15)?.as_deref() == Some(ledger::ADDED),
                card_row: r.get::<_, Option<String>>(15)?.as_deref() == Some(ledger::CARD_ROW),
                balance: None,
            })
        })?;
        let mut out = Vec::new();
        let mut running: Option<Cents> = None;
        let needle = f.search.trim().to_lowercase();
        for row in rows {
            let mut t = row?;
            if f.account_id.is_some() {
                let b = running.unwrap_or(opening).checked_add(t.amount).ok_or_else(ledger::too_big)?;
                running = Some(b);
                t.balance = Some(b);
            }
            if let Some(from) = f.from
                && t.date < from
            {
                continue;
            }
            if let Some(to) = f.to
                && t.date > to
            {
                continue;
            }
            if f.uncategorized && !uncategorized.contains(&t.entry_id) {
                continue;
            }
            if let Some(c) = f.category_id {
                // Include le sottocategorie.
                let path = cats.get(&c).cloned().unwrap_or_default();
                let below = format!("{path}:");
                let ok = entry_categories
                    .get(&t.entry_id)
                    .is_some_and(|l| l.iter().any(|x| *x == c || cats.get(x).is_some_and(|p| p.starts_with(&below))));
                if !ok {
                    continue;
                }
            }
            if f.tag.as_ref().is_some_and(|tag| !has_tag(&t.tags, tag)) {
                continue;
            }
            if !needle.is_empty() {
                let tags: Vec<String> = t.tags.iter().map(|x| format!("#{}", x.to_lowercase())).collect();
                let original = t.source_payee.iter().chain(&t.source_memo);
                let hay = format!(
                    "{} {} {} {} {} {}",
                    t.payee.to_lowercase(),
                    t.memo.to_lowercase(),
                    t.category.to_lowercase(),
                    crate::money::fmt_plain(t.amount),
                    tags.join(" "),
                    original.map(|o| o.to_lowercase()).collect::<Vec<_>>().join(" ")
                );
                if !hay.contains(&needle) {
                    continue;
                }
            }
            out.push(t);
        }
        out.reverse(); // dal più recente
        if let Some(l) = f.limit {
            out.truncate(l);
        }
        Ok(out)
    }

    pub fn transaction(&self, id: i64) -> Result<Transaction> {
        let acc: Option<i64> = self
            .conn
            .query_row(
                "SELECT l.money_account_id FROM journal_postings p JOIN ledger_accounts l ON l.id=p.ledger_account_id
                 WHERE p.id=?1 AND l.money_account_id IS NOT NULL",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        let acc = acc.ok_or_else(|| no_transaction(id))?;
        let all = self.transactions(&TxFilter { account_id: Some(acc), ..Default::default() })?;
        all.into_iter().find(|t| t.id == id).ok_or_else(|| no_transaction(id))
    }

    /// Inserisce un movimento. Se non ha categoria applica le regole.
    /// Restituisce `None` se era un duplicato di un import precedente.
    pub fn add_transaction(&self, tx: &NewTx) -> Result<Option<i64>> {
        match tx.category_id {
            Some(_) => self.add_with_rules(tx, None),
            None => self.add_with_rules(tx, Some(&self.ruleset()?)),
        }
    }

    /// [`Db::add_transaction`] with rules that are ready: many transactions read them once.
    pub(crate) fn add_with_rules(&self, tx: &NewTx, rules: Option<&Ruleset>) -> Result<Option<i64>> {
        let mut tx = tx.clone();
        // The keys of an import are of the row: the rules change none.
        let (original_payee, original_memo) = (tx.payee.trim().to_string(), tx.memo.trim().to_string());
        let (mut tags, mut marks) = (None, None);
        if let (None, Some(set)) = (tx.category_id, rules) {
            let s = Subject::new(&tx.payee, &tx.memo, tx.amount, Some(tx.account_id));
            let done = self.apply_to_new(set, &tx.payee, &tx.memo, &s, tx.import_hash.is_none())?;
            (tx.payee, tx.memo, tx.category_id) = (done.payee, done.memo, done.category);
            (tags, marks) = (done.tags, done.rules);
        }
        let keep = tx.import_hash.is_some() || tx.payee.trim() != original_payee;
        let credit = tx.amount.checked_neg().ok_or_else(|| anyhow!(tr("the amount is too large")))?;
        self.atomically(|| {
            let (account, currency) = self.money_ledger(tx.account_id)?;
            if let Some(hash) = &tx.import_hash {
                let known: i64 = self.conn.query_row(
                    "SELECT COUNT(*) FROM journal_postings WHERE ledger_account_id=?1 AND import_hash=?2",
                    params![account, hash],
                    |r| r.get(0),
                )?;
                if known > 0 {
                    return Ok(None);
                }
            }
            let (_, ids) = self.save_entry(&Draft {
                id: None,
                date: d2s(tx.date),
                payee: tx.payee.trim().into(),
                memo: tx.memo.trim().into(),
                opening: None,
                lines: vec![
                    Line {
                        account,
                        amount: tx.amount,
                        currency: currency.clone(),
                        cleared: tx.cleared,
                        import_hash: tx.import_hash.clone(),
                        other_hashes: tx.import_marks.clone(),
                        origin: tx.card_row.then(|| ledger::CARD_ROW.into()),
                        original_payee: keep.then(|| original_payee.clone()),
                        original_memo: keep.then(|| original_memo.clone()),
                        tags: tags.clone(),
                        rules: marks.clone(),
                        ..Default::default()
                    },
                    Line {
                        account: self.category_ledger(tx.category_id)?,
                        amount: credit,
                        currency,
                        ..Default::default()
                    },
                ],
            })?;
            Ok(ids.first().copied())
        })
    }

    /// The category of a transaction that has one only.
    pub(crate) fn category_of(&self, id: i64) -> Result<Option<i64>> {
        Ok(self.transaction_side(id)?.and_then(|r| r.category))
    }

    fn transaction_side(&self, id: i64) -> Result<Option<ledger::LedgerRow>> {
        let Some(entry) = self.entry_of_transaction(id)? else { return Ok(None) };
        let d = self.load_draft(entry)?;
        let others: Vec<&Line> = d.lines.iter().filter(|l| l.id != Some(id)).collect();
        match others.as_slice() {
            [one] => self.ledger_row(one.account).map(Some),
            _ => Ok(None),
        }
    }

    /// Changes a transaction, on both accounts when it is a transfer. The amount and the category of
    /// a split one are refused; `category_id: None` leaves an account of the chart as it is.
    pub fn update_transaction(&self, id: i64, tx: &TxEdit) -> Result<()> {
        if self.atomically(|| self.edit_transaction(id, tx))? { Ok(()) } else { Err(no_transaction(id)) }
    }

    pub fn set_category(&self, id: i64, category_id: Option<i64>) -> Result<()> {
        if !self.atomically(|| self.move_to_category(id, category_id))? {
            bail!(trf("transaction {id} not found (or it is a transfer)", &[("id", &id)]));
        }
        Ok(())
    }

    pub fn set_cleared(&self, id: i64, cleared: bool) -> Result<()> {
        self.atomically(|| {
            let Some(entry) = self.entry_of_transaction(id)? else { return Ok(()) };
            let mut d = self.load_draft(entry)?;
            for l in d.lines.iter_mut().filter(|l| l.id == Some(id)) {
                l.cleared = cleared;
            }
            self.save_entry(&d)?;
            Ok(())
        })
    }

    /// Deletes a transaction with every other side of its entry.
    pub fn delete_transaction(&self, id: i64) -> Result<()> {
        self.atomically(|| {
            let entry = self.entry_of_transaction(id)?.ok_or_else(|| no_transaction(id))?;
            self.delete_entry(entry)
        })
    }

    /// The two accounts of a transfer must have the same currency.
    pub(crate) fn check_transfer(&self, from: i64, to: i64) -> Result<()> {
        let ((_, a), (_, b)) = (self.money_ledger(from)?, self.money_ledger(to)?);
        if a != b {
            bail!(trf(
                "a transfer between accounts in different currencies ({from} and {to}) is not supported",
                &[("from", &a), ("to", &b)]
            ));
        }
        Ok(())
    }

    /// Transfer: out of `from`, into `to`. Returns the ids (out, in).
    pub fn add_transfer(&self, from: i64, to: i64, date: NaiveDate, amount: Cents, memo: &str) -> Result<(i64, i64)> {
        if from == to {
            bail!(tr("the two accounts of the transfer are the same"));
        }
        let too_large = || anyhow!(tr("the amount is too large"));
        let amount = amount.checked_abs().ok_or_else(too_large)?;
        let names: HashMap<i64, String> = self.accounts(true)?.into_iter().map(|a| (a.id, a.name)).collect();
        let to_name = names.get(&to).ok_or_else(|| anyhow!(tr("the account to transfer to does not exist")))?;
        let from_name = names.get(&from).ok_or_else(|| anyhow!(tr("the account to transfer from does not exist")))?;
        self.check_transfer(from, to)?;
        let (out_text, in_text) = (
            trf("Transfer to {account}", &[("account", to_name)]),
            trf("Transfer from {account}", &[("account", from_name)]),
        );
        let side = |account: i64, amount: Cents, payee: &str| -> Result<Line> {
            let (account, currency) = self.money_ledger(account)?;
            Ok(Line {
                account,
                amount,
                currency,
                payee: Some(payee.into()),
                origin: Some(ledger::ADDED.into()),
                ..Default::default()
            })
        };
        let (_, ids) = self.save_entry(&Draft {
            id: None,
            date: d2s(date),
            payee: out_text.clone(),
            memo: memo.into(),
            opening: None,
            lines: vec![side(from, -amount, &out_text)?, side(to, amount, &in_text)?],
        })?;
        match ids.as_slice() {
            [a, b] => Ok((*a, *b)),
            _ => Err(anyhow!(tr("the transaction was not saved"))),
        }
    }

    // --------------------------------------------------------------- budget

    pub fn set_budget(&self, category_id: i64, monthly: Cents) -> Result<()> {
        if monthly == 0 {
            self.conn.execute("DELETE FROM category_budgets WHERE category_id=?1", [category_id])?;
        } else {
            self.conn.execute(
                "INSERT INTO category_budgets(category_id,monthly) VALUES(?1,?2)
                 ON CONFLICT(category_id) DO UPDATE SET monthly=excluded.monthly",
                params![category_id, monthly.saturating_abs()],
            )?;
        }
        Ok(())
    }

    /// Budget del mese indicato: importo previsto e speso (incluse le sottocategorie).
    pub fn budget_status(&self, month: NaiveDate) -> Result<Vec<BudgetLine>> {
        let (from, to) = month_range(month);
        let cats = self.categories()?;
        let totals = self.category_totals(from, to)?;
        let mut st = self.conn.prepare("SELECT category_id, monthly FROM category_budgets")?;
        let budgets: Vec<(i64, i64)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?;
        let mut out = Vec::new();
        for (cid, monthly) in budgets {
            let Some(c) = cats.iter().find(|c| c.id == cid) else {
                continue;
            };
            let prefix = format!("{}:", c.path);
            let below = totals
                .iter()
                .filter(|t| t.category_id.is_some() && (t.category == c.path || t.category.starts_with(&prefix)));
            let total = checked_total(below.map(|t| t.total)).ok_or_else(ledger::too_big)?;
            let spent = if c.is_income { total } else { total.saturating_neg() };
            out.push(BudgetLine { category_id: cid, category: c.path.clone(), budget: monthly, spent });
        }
        out.sort_by_key(|a| a.category.to_lowercase());
        Ok(out)
    }

    // --------------------------------------------------------------- report

    /// Totali per categoria nel periodo (giroconti esclusi). Le uscite sono negative.
    pub fn category_totals(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<CategoryTotal>> {
        let cats: HashMap<i64, Category> = self.categories()?.into_iter().map(|c| (c.id, c)).collect();
        let mut st = self.conn.prepare(&format!(
            "{COUNTED} SELECT c.category_id, -SUM(c.amount), -COALESCE(SUM(CASE WHEN c.n>1 THEN c.amount END),0)
                 FROM counted c GROUP BY c.category_id"
        ))?;
        let rows = st.query_map(params![d2s(from), d2s(to)], |r| {
            Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (cid, total, shared) = row?;
            let (name, is_income) = match cid.and_then(|c| cats.get(&c)) {
                Some(c) => (c.path.clone(), c.is_income),
                None => (tr("Uncategorized").to_string(), total > 0),
            };
            out.push(CategoryTotal { category_id: cid, category: name, is_income, total, shared });
        }
        out.sort_by_key(|c| c.total);
        Ok(out)
    }

    /// Entrate e uscite per mese degli ultimi `months` mesi (giroconti esclusi).
    pub fn monthly_totals(&self, months: u32, until: NaiveDate) -> Result<Vec<MonthTotal>> {
        let mut out = Vec::new();
        let first = first_of_month(until);
        for i in (0..months).rev() {
            let m = first.checked_sub_months(Months::new(i)).unwrap_or(first);
            let (from, to) = month_range(m);
            let (inc, exp): (i64, i64) = self.conn.query_row(
                &format!(
                    "{COUNTED} SELECT
                        COALESCE(SUM(CASE WHEN CASE c.n WHEN 1 THEN c.amount<0 ELSE c.class='income' END
                                     THEN -c.amount END),0),
                        COALESCE(SUM(CASE WHEN CASE c.n WHEN 1 THEN c.amount>0 ELSE c.class='expense' END
                                     THEN c.amount END),0)
                     FROM counted c"
                ),
                params![d2s(from), d2s(to)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            out.push(MonthTotal { month: m.format("%Y-%m").to_string(), income: inc, expense: exp });
        }
        Ok(out)
    }

    /// Income and expense of the transactions shown: those of the whole entry, once, when it is split.
    /// With a category, only what is in it or under it.
    pub fn register_totals(&self, txs: &[Transaction], category: Option<i64>) -> Result<(Cents, Cents)> {
        let wanted: Option<HashSet<i64>> = match category {
            None => None,
            Some(c) => {
                let all = self.categories()?;
                let path = all.iter().find(|x| x.id == c).map(|x| x.path.clone()).unwrap_or_default();
                let below = format!("{path}:");
                let found = all.iter().filter(|x| x.id == c || x.path.starts_with(&below)).map(|x| x.id);
                Some(found.collect())
            }
        };
        let (mut income, mut expense): (Cents, Cents) = (0, 0);
        let mut add = |is_income: bool, money_in: Cents| -> Result<()> {
            let side = if is_income { &mut income } else { &mut expense };
            let c = if is_income { Some(money_in) } else { money_in.checked_neg() };
            *side = c.and_then(|c| side.checked_add(c)).ok_or_else(ledger::too_big)?;
            Ok(())
        };
        let mut st = self.conn.prepare(&format!(
            "SELECT l.class='income', l.category_id, p.amount FROM journal_postings p
             JOIN ledger_accounts l ON l.id=p.ledger_account_id WHERE p.entry_id=?1 AND {PROFIT_AND_LOSS}"
        ))?;
        let mut seen = HashSet::new();
        for t in txs {
            match t.kind {
                TxKind::Category | TxKind::Uncategorized => add(t.amount > 0, t.amount)?,
                TxKind::Split if seen.insert(t.entry_id) => {
                    let rows: Vec<(bool, Option<i64>, Cents)> = st
                        .query_map([t.entry_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                        .collect::<std::result::Result<_, _>>()?;
                    for (class, c, amount) in &rows {
                        if wanted.as_ref().is_some_and(|w| !c.is_some_and(|c| w.contains(&c))) {
                            continue;
                        }
                        add(if rows.len() > 1 { *class } else { *amount < 0 }, amount.saturating_neg())?;
                    }
                }
                _ => {}
            }
        }
        Ok((income, expense))
    }

    /// Assets minus debts: every account of the chart of those classes, up to a date.
    pub fn net_worth(&self, until: Option<NaiveDate>) -> Result<Cents> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(SUM(p.amount),0) FROM journal_postings p JOIN entries e ON e.id=p.entry_id
             JOIN ledger_accounts l ON l.id=p.ledger_account_id
             WHERE l.class IN ('asset','liability') AND (?1 IS NULL OR e.date<=?1)",
            [until.map(d2s)],
            |r| r.get(0),
        )?)
    }

    /// Patrimonio netto alla fine di ogni mese degli ultimi `months` mesi.
    pub fn net_worth_history(&self, months: u32, until: NaiveDate) -> Result<Vec<(String, Cents)>> {
        let first = first_of_month(until);
        let mut out = Vec::new();
        for i in (0..months).rev() {
            let m = first.checked_sub_months(Months::new(i)).unwrap_or(first);
            let (_, to) = month_range(m);
            out.push((m.format("%Y-%m").to_string(), self.net_worth(Some(to))?));
        }
        Ok(out)
    }

    // ---------------------------------------------------------- pianificati

    pub fn scheduled(&self) -> Result<Vec<Scheduled>> {
        let cats: HashMap<i64, String> = self.categories()?.into_iter().map(|c| (c.id, c.path)).collect();
        let mut st = self.conn.prepare(
            "SELECT s.id, s.account_id, a.name, s.to_account_id, s.amount, s.payee, s.memo, s.category_id,
                    s.next_date, s.every, s.unit
             FROM scheduled_items s JOIN accounts a ON a.id=s.account_id ORDER BY s.next_date",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Scheduled {
                id: r.get(0)?,
                account_id: r.get(1)?,
                account: r.get(2)?,
                to_account_id: r.get(3)?,
                amount: r.get(4)?,
                payee: r.get(5)?,
                memo: r.get(6)?,
                category_id: r.get(7)?,
                category: String::new(),
                next_date: s2d(&r.get::<_, String>(8)?),
                every: r.get(9)?,
                unit: r.get(10)?,
            })
        })?;
        let mut out = Vec::new();
        for s in rows {
            let mut s = s?;
            s.category = s.category_id.and_then(|c| cats.get(&c).cloned()).unwrap_or_default();
            out.push(s);
        }
        Ok(out)
    }

    pub fn add_scheduled(&self, s: &NewScheduled) -> Result<i64> {
        let unit = normalize_unit(&s.unit)?;
        if !(1..=1000).contains(&s.every) {
            bail!(tr("the repetition goes from 1 to 1000"));
        }
        if let Some(to) = s.to_account_id {
            self.check_transfer(s.account_id, to)?;
        }
        self.conn.execute(
            "INSERT INTO scheduled_items(account_id,to_account_id,amount,payee,memo,category_id,next_date,every,unit,id)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                s.account_id,
                s.to_account_id,
                s.amount,
                s.payee,
                s.memo,
                s.category_id,
                d2s(s.next_date),
                s.every,
                unit,
                self.new_id("scheduled_items")?
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_scheduled(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM scheduled_items WHERE id=?1", [id])?;
        Ok(())
    }

    /// Registra tutti i movimenti pianificati con scadenza fino a `until`. One that fails records
    /// nothing, stays due and is reported; the others are recorded.
    pub fn post_scheduled(&self, until: NaiveDate) -> Result<Posted> {
        self.atomically(|| self.post_scheduled_until(until))
    }

    /// Runs `f` in one database transaction: all of it, or nothing. A nested call that fails undoes its
    /// own part, even when the outer one goes on. Takes the write lock at once, so that what `f` reads
    /// cannot change before it writes.
    pub fn atomically<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        if !self.conn.is_autocommit() {
            self.conn.execute_batch("SAVEPOINT nested")?;
            let out = f();
            let end = if out.is_ok() { "RELEASE nested" } else { "ROLLBACK TO nested; RELEASE nested" };
            let ended = self.conn.execute_batch(end);
            if out.is_ok() {
                ended?;
            }
            return out;
        }
        let tx = SqlTransaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        self.touched.borrow_mut().clear();
        let out = f()?;
        self.check_reconciled()?;
        tx.commit()?;
        Ok(out)
    }

    fn post_scheduled_until(&self, until: NaiveDate) -> Result<Posted> {
        let mut out = Posted::default();
        for s in self.scheduled()? {
            match self.atomically(|| self.post_one_scheduled(&s, until)) {
                Ok(n) => out.posted += n,
                Err(e) => {
                    out.failed.push(FailedScheduled { id: s.id, payee: s.payee, error: format!("{e:#}") });
                }
            }
        }
        Ok(out)
    }

    fn post_one_scheduled(&self, s: &Scheduled, until: NaiveDate) -> Result<usize> {
        let mut n = 0;
        let mut date = s.next_date;
        let mut guard = 0;
        while date <= until && guard < 500 {
            guard += 1;
            match s.to_account_id {
                Some(to) => {
                    self.add_transfer(s.account_id, to, date, s.amount, &s.memo)?;
                }
                None => {
                    self.add_transaction(&NewTx {
                        account_id: s.account_id,
                        date,
                        amount: s.amount,
                        payee: s.payee.clone(),
                        memo: s.memo.clone(),
                        category_id: s.category_id,
                        cleared: false,
                        import_hash: None,
                        import_marks: None,
                        card_row: false,
                    })?;
                }
            }
            n += 1;
            date = advance(date, s.every, &s.unit);
        }
        if date != s.next_date {
            self.conn.execute("UPDATE scheduled_items SET next_date=?2 WHERE id=?1", params![s.id, d2s(date)])?;
        }
        Ok(n)
    }

    /// Cancella conti, movimenti e pianificati (tiene categorie e regole, tranne quelle di un conto).
    pub fn clear_accounts(&self) -> Result<()> {
        self.atomically(|| {
            self.conn.execute_batch(
                "DELETE FROM scheduled_items; DELETE FROM rule_runs; DELETE FROM entries;
                 DELETE FROM ledger_accounts WHERE money_account_id IS NOT NULL;
                 DELETE FROM accounts; DELETE FROM category_budgets; DELETE FROM envelope_changes; DELETE FROM envelopes;",
            )?;
            self.set_meta("demo", "0")
        })
    }
}

pub fn normalize_unit(u: &str) -> Result<&'static str> {
    Ok(match u.trim().to_lowercase().as_str() {
        "day" | "days" | "giorno" | "giorni" | "d" => "day",
        "week" | "weeks" | "settimana" | "settimane" | "w" => "week",
        "month" | "months" | "mese" | "mesi" | "m" => "month",
        "year" | "years" | "anno" | "anni" | "y" => "year",
        other => {
            bail!(trf("unknown repetition unit \"{unit}\" (day, week, month, year)", &[("unit", &other)]))
        }
    })
}

pub fn unit_label(unit: &str, every: i64) -> String {
    let n = usize::try_from(every).unwrap_or(1).max(1);
    match unit {
        "day" => trn(n, "every day", "every {n} days"),
        "week" => trn(n, "every week", "every {n} weeks"),
        "year" => trn(n, "every year", "every {n} years"),
        _ => trn(n, "every month", "every {n} months"),
    }
}

pub fn advance(d: NaiveDate, every: i64, unit: &str) -> NaiveDate {
    let every = every.clamp(1, 1000) as u32;
    let next = match unit {
        "day" => d.checked_add_days(Days::new(every.into())),
        "week" => d.checked_add_days(Days::new(u64::from(every) * 7)),
        "year" => d.checked_add_months(Months::new(12 * every)),
        _ => d.checked_add_months(Months::new(every)),
    };
    next.unwrap_or(d)
}

pub fn first_of_month(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap_or(d)
}

pub fn month_range(d: NaiveDate) -> (NaiveDate, NaiveDate) {
    let first = first_of_month(d);
    let last = first.checked_add_months(Months::new(1)).and_then(|n| n.pred_opt()).unwrap_or(first);
    (first, last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_of_a_newer_version_is_refused_and_left_alone() {
        let path = std::env::temp_dir().join(format!("cashcraft-newer-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        drop(Db::open(&path).unwrap());
        let newer = SCHEMA_VERSION + 1;
        Connection::open(&path).unwrap().pragma_update(None, "user_version", newer).unwrap();
        assert!(Db::open(&path).is_err());
        let v: i64 = Connection::open(&path).unwrap().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, newer);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_new_file_gets_categories_and_rules_in_its_language() {
        let categories = |l| {
            with_lang(l, || {
                let db = Db::open_in_memory().unwrap();
                let patterns: Vec<String> = db.rules().unwrap().into_iter().map(|r| r.pattern).collect();
                // Italian statements are categorized in every language.
                let acc = db.add_account("A", AccountKind::Bank, "EUR", 0, "").unwrap();
                let tx =
                    NewTx { account_id: acc, payee: "FARMACIA COMUNALE".into(), amount: -100, ..Default::default() };
                let id = db.add_transaction(&tx).unwrap().unwrap();
                let names: Vec<String> = db.categories().unwrap().into_iter().map(|c| c.path).collect();
                (names, patterns, db.transaction(id).unwrap().category)
            })
        };
        let (en, en_rules, en_hit) = categories(Lang::En);
        let (it, it_rules, it_hit) = categories(Lang::It);
        assert!(en.contains(&"Home:Utilities".to_string()) && it.contains(&"Casa:Bollette".to_string()));
        assert_eq!(en.len(), it.len());
        assert_eq!(en_rules, it_rules);
        assert_eq!((en_hit.as_str(), it_hit.as_str()), ("Health", "Salute"));
    }

    #[test]
    fn what_the_user_wrote_is_never_translated() {
        let db = with_lang(Lang::It, Db::open_in_memory).unwrap();
        let names = |l| with_lang(l, || db.categories().unwrap().into_iter().map(|c| c.path).collect::<Vec<_>>());
        assert_eq!(names(Lang::It), names(Lang::En));
    }

    #[test]
    fn a_failure_halfway_leaves_nothing_behind() {
        let db = Db::open_in_memory().unwrap();
        let r: Result<()> = db.atomically(|| {
            db.add_account("A", AccountKind::Bank, "EUR", 0, "")?;
            bail!("stop")
        });
        assert!(r.is_err());
        assert!(db.accounts(true).unwrap().is_empty());
    }
}
