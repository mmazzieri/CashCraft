//! The double-entry ledger: the chart of accounts, the entries and their postings.
//!
//! Every amount of money in the data file is a posting. Debit is positive and credit negative, so a
//! posting on an account of the user keeps the sign people expect (negative = money out). Postings
//! are written by [`Db::save_entry`] alone, which refuses an entry that does not balance.

use crate::db::{Cents, Db, TxEdit, TxKind, d2s, s2d};
use crate::i18n::{tr, trf};
use crate::money::{MAX_AMOUNT, fmt_machine};
use anyhow::{Context, Result, anyhow, bail};
use chrono::NaiveDate;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Keys of the accounts every data file has.
pub const OPENING: &str = "opening_balances";
pub const UNCATEGORIZED: &str = "uncategorized";

/// Date of the opening balances: before every transaction, so that they count in any period.
pub const OPENING_DATE: &str = "0001-01-01";

/// Most postings of one entry.
pub const MAX_LINES: usize = 1000;

/// Largest balance of an account, either side.
pub const MAX_BALANCE: Cents = 1_000_000_000_000_000;

/// Largest sum of all postings of the file, whatever their sign. No sum of some of them, at any date, can
/// be larger, so every total and the difference of two fit in an `i64`.
pub const MAX_TURNOVER: Cents = 4_000_000_000_000_000_000;

const MAX_DEPTH: usize = 16;

/// The ledger of format 2. A row linked to an account of the user or to a category has no name of
/// its own, so that names are in one place; triggers keep exactly one row for each of them.
pub(crate) const LEDGER_SCHEMA: &str = r#"
CREATE TABLE ledger_accounts (
    id INTEGER PRIMARY KEY,
    class TEXT NOT NULL CHECK (class IN ('asset','liability','equity','income','expense')),
    parent_id INTEGER REFERENCES ledger_accounts(id) ON DELETE SET NULL,
    name TEXT NOT NULL DEFAULT '',
    code TEXT NOT NULL DEFAULT '',
    currency TEXT,
    money_account_id INTEGER UNIQUE REFERENCES accounts(id),
    category_id INTEGER UNIQUE REFERENCES categories(id),
    system TEXT UNIQUE
);
CREATE INDEX ledger_parent ON ledger_accounts(parent_id);
CREATE UNIQUE INDEX ledger_code ON ledger_accounts(code) WHERE code <> '';
CREATE TABLE entries (
    id INTEGER PRIMARY KEY,
    date TEXT NOT NULL,
    payee TEXT NOT NULL DEFAULT '',
    memo TEXT NOT NULL DEFAULT '',
    opening_account_id INTEGER UNIQUE REFERENCES accounts(id)
);
CREATE INDEX entry_date ON entries(date);
CREATE TABLE postings (
    id INTEGER PRIMARY KEY,
    entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    ledger_account_id INTEGER NOT NULL REFERENCES ledger_accounts(id),
    amount INTEGER NOT NULL CHECK (typeof(amount) = 'integer'),
    currency TEXT NOT NULL,
    cleared INTEGER NOT NULL DEFAULT 0,
    import_hash TEXT,
    payee TEXT,
    memo TEXT
);
CREATE INDEX posting_entry ON postings(entry_id);
CREATE INDEX posting_account ON postings(ledger_account_id);
CREATE UNIQUE INDEX posting_import_hash ON postings(ledger_account_id, import_hash) WHERE import_hash IS NOT NULL;
CREATE TABLE ledger_turnover (id INTEGER PRIMARY KEY CHECK (id = 1), total INTEGER NOT NULL);
INSERT INTO ledger_turnover VALUES (1, 0);
CREATE TRIGGER posting_added AFTER INSERT ON postings BEGIN
    UPDATE ledger_turnover SET total = total + abs(NEW.amount);
END;
CREATE TRIGGER posting_changed AFTER UPDATE OF amount ON postings BEGIN
    UPDATE ledger_turnover SET total = total - abs(OLD.amount) + abs(NEW.amount);
END;
CREATE TRIGGER posting_removed AFTER DELETE ON postings BEGIN
    UPDATE ledger_turnover SET total = total - abs(OLD.amount);
END;
CREATE TRIGGER account_gets_ledger AFTER INSERT ON accounts BEGIN
    INSERT INTO ledger_accounts(class, currency, money_account_id)
    VALUES (CASE WHEN NEW.kind IN ('credit_card','loan') THEN 'liability' ELSE 'asset' END,
            CASE WHEN trim(NEW.currency) = '' THEN 'EUR' ELSE upper(trim(NEW.currency)) END, NEW.id);
END;
CREATE TRIGGER account_kind_sets_class AFTER UPDATE OF kind ON accounts BEGIN
    UPDATE ledger_accounts
    SET class = CASE WHEN NEW.kind IN ('credit_card','loan') THEN 'liability' ELSE 'asset' END
    WHERE money_account_id = NEW.id;
END;
CREATE TRIGGER category_gets_ledger AFTER INSERT ON categories BEGIN
    INSERT INTO ledger_accounts(class, parent_id, category_id)
    VALUES (CASE WHEN NEW.is_income THEN 'income' ELSE 'expense' END,
            (SELECT id FROM ledger_accounts WHERE category_id = NEW.parent_id), NEW.id);
END;
INSERT INTO ledger_accounts(class, system) VALUES ('equity', 'opening_balances'), ('expense', 'uncategorized');
"#;

/// A posting counts as income or expense when its account (`l`) is of one of those classes.
pub(crate) const PROFIT_AND_LOSS: &str = "l.class IN ('income','expense')";

/// The postings that count from ?1 to ?2, each with how many of them its entry has (`n`).
pub(crate) const COUNTED: &str = "WITH counted AS (
    SELECT l.category_id, l.class, p.amount, COUNT(*) OVER (PARTITION BY p.entry_id) AS n
    FROM postings p JOIN entries e ON e.id=p.entry_id JOIN ledger_accounts l ON l.id=p.ledger_account_id
    WHERE l.class IN ('income','expense') AND e.date>=?1 AND e.date<=?2)";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Asset,
    Liability,
    Equity,
    Income,
    Expense,
}

impl Class {
    pub const ALL: [Class; 5] = [Class::Asset, Class::Liability, Class::Equity, Class::Income, Class::Expense];

    pub fn code(self) -> &'static str {
        match self {
            Class::Asset => "asset",
            Class::Liability => "liability",
            Class::Equity => "equity",
            Class::Income => "income",
            Class::Expense => "expense",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s.trim().to_lowercase().as_str() {
            "asset" | "assets" | "attività" | "attivo" => Class::Asset,
            "liability" | "liabilities" | "passività" | "passivo" => Class::Liability,
            "equity" | "patrimonio netto" => Class::Equity,
            "income" | "entrata" | "entrate" | "ricavo" | "ricavi" => Class::Income,
            "expense" | "expenses" | "uscita" | "uscite" | "costo" | "costi" => Class::Expense,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Class::Asset => tr("Assets"),
            Class::Liability => tr("Liabilities"),
            Class::Equity => tr("Equity"),
            Class::Income => tr("Income"),
            Class::Expense => tr("Expenses"),
        }
    }

    /// Whether a growing balance of this class is a credit.
    pub fn grows_by_credit(self) -> bool {
        matches!(self, Class::Liability | Class::Equity | Class::Income)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Balance {
    pub currency: String,
    /// Debit positive.
    pub amount: Cents,
}

#[derive(Clone, Debug, Serialize)]
pub struct LedgerAccount {
    pub id: i64,
    pub class: Class,
    pub parent_id: Option<i64>,
    pub name: String,
    /// The name after those of the accounts above it: "Parent:Child".
    pub path: String,
    pub code: String,
    /// `None`: any currency, that of the other postings of the entry.
    pub currency: Option<String>,
    pub money_account_id: Option<i64>,
    pub category_id: Option<i64>,
    pub system: Option<String>,
    /// Its own balances, without the accounts under it.
    pub balances: Vec<Balance>,
}

#[derive(Clone, Debug, Default)]
pub struct NewEntry {
    pub date: NaiveDate,
    pub payee: String,
    pub memo: String,
    pub lines: Vec<NewLine>,
}

#[derive(Clone, Debug, Default)]
pub struct NewLine {
    pub ledger_account_id: i64,
    /// Debit positive.
    pub amount: Cents,
    /// Needed only on an account of any currency in an entry with several currencies.
    pub currency: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub id: i64,
    pub date: NaiveDate,
    pub payee: String,
    pub memo: String,
    /// The opening balance of an account.
    pub opening: bool,
    pub postings: Vec<Posting>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Posting {
    pub id: i64,
    pub account_id: i64,
    pub account: String,
    pub amount: Cents,
    pub currency: String,
    pub cleared: bool,
}

#[derive(Clone, Debug, Default)]
pub struct EntryFilter {
    pub ledger_account_id: Option<i64>,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    pub offset: usize,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct TrialLine {
    pub account_id: i64,
    pub account: String,
    pub code: String,
    pub class: Class,
    pub currency: String,
    /// Debit positive.
    pub amount: Cents,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrialTotal {
    pub currency: String,
    pub debit: Cents,
    pub credit: Cents,
}

#[derive(Clone, Debug, Default)]
pub struct TrialBalance {
    pub lines: Vec<TrialLine>,
    pub totals: Vec<TrialTotal>,
}

impl TrialBalance {
    pub fn is_zero(&self) -> bool {
        self.totals.iter().all(|t| t.debit == t.credit)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Verification {
    pub entries: usize,
    pub postings: usize,
    pub problems: Vec<String>,
}

/// An entry as it is written: every posting, in order.
#[derive(Clone, Debug)]
pub(crate) struct Draft {
    /// `None` for a new entry.
    pub id: Option<i64>,
    pub date: String,
    pub payee: String,
    pub memo: String,
    /// The account this entry is the opening balance of.
    pub opening: Option<i64>,
    pub lines: Vec<Line>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Line {
    /// The id to keep, or to give to the posting of a new entry; `None` for the next free one.
    pub id: Option<i64>,
    pub account: i64,
    pub amount: Cents,
    pub currency: String,
    pub cleared: bool,
    pub import_hash: Option<String>,
    /// The text of this side alone (the two sides of a transfer), instead of the entry's.
    pub payee: Option<String>,
    pub memo: Option<String>,
}

/// What an account of the chart is, for the checks.
#[derive(Clone, Debug)]
pub(crate) struct LedgerRow {
    pub class: Class,
    pub currency: Option<String>,
    pub money: Option<i64>,
    pub category: Option<i64>,
    pub system: Option<String>,
}

/// In capitals without spaces around; the ways people write the euro become its code.
pub fn normalize_currency(s: &str) -> String {
    let t = s.trim().to_uppercase();
    if matches!(t.as_str(), "" | "€" | "EURO" | "EUROS") { "EUR".into() } else { t }
}

/// A currency for something new: a code of three letters.
pub fn check_currency(s: &str) -> Result<String> {
    let c = normalize_currency(s);
    if c.len() != 3 || !c.chars().all(|x| x.is_ascii_uppercase()) {
        bail!(trf("the currency \"{currency}\" is not a code of three letters, like EUR", &[("currency", &s)]));
    }
    Ok(c)
}

fn no_ledger_account(key: &str) -> anyhow::Error {
    anyhow!(trf("account \"{name}\" is not in the chart of accounts", &[("name", &key)]))
}

fn no_posting(id: i64) -> anyhow::Error {
    anyhow!(trf("posting {id} not found", &[("id", &id)]))
}

fn no_entry(id: i64) -> anyhow::Error {
    anyhow!(trf("entry {id} not found", &[("id", &id)]))
}

pub(crate) fn split_error() -> anyhow::Error {
    anyhow!(tr("the transaction is split over several accounts: its amount and category cannot be changed here"))
}

pub(crate) fn too_big() -> anyhow::Error {
    anyhow!(tr("the sum of the amounts is too large"))
}

/// Finds one of the accounts of the chart by "ledger:id", "code:…", "system:…", code, system key, path or
/// name. A number alone is refused: it could be the id of something else.
pub fn find_ledger_account(all: &[LedgerAccount], key: &str) -> Result<LedgerAccount> {
    let key = key.trim();
    let lower = key.to_lowercase();
    if !key.is_empty() && key.chars().all(|c| c.is_ascii_digit()) {
        bail!(trf(
            "\"{key}\" may be several things: write ledger:{key} for the id in the chart of accounts, code:{key} for the code",
            &[("key", &key)]
        ));
    }
    let only = |test: &dyn Fn(&LedgerAccount) -> bool| -> Result<Option<LedgerAccount>> {
        let hits: Vec<&LedgerAccount> = all.iter().filter(|a| test(a)).collect();
        match hits.as_slice() {
            [] => Ok(None),
            [one] => Ok(Some((*one).clone())),
            _ => {
                let ids: Vec<String> = hits.iter().map(|a| format!("ledger:{} ({})", a.id, a.path)).collect();
                bail!(trf(
                    "several accounts of the chart are called \"{name}\": {which}",
                    &[("name", &key), ("which", &ids.join(", "))]
                ))
            }
        }
    };
    let found = if let Some(id) = lower.strip_prefix("ledger:") {
        let id = id.trim().parse::<i64>().ok();
        only(&|a| Some(a.id) == id)?
    } else if let Some(code) = key.strip_prefix("code:") {
        only(&|a| !a.code.is_empty() && a.code == code.trim())?
    } else if let Some(system) = lower.strip_prefix("system:") {
        only(&|a| a.system.as_deref() == Some(system.trim()))?
    } else {
        let exact = only(&|a| {
            (!a.code.is_empty() && a.code == key)
                || a.system.as_deref() == Some(lower.as_str())
                || a.path.to_lowercase() == lower
        })?;
        match exact {
            None => only(&|a| a.name.to_lowercase() == lower)?,
            found => found,
        }
    };
    found.ok_or_else(|| no_ledger_account(key))
}

impl Db {
    // ------------------------------------------------------------- writing

    /// Writes an entry and gives back its id and those of its postings, in order. The only code that
    /// inserts or changes postings: an entry that is not sound is refused and nothing is written.
    pub(crate) fn save_entry(&self, d: &Draft) -> Result<(i64, Vec<i64>)> {
        self.check_draft(d)?;
        self.atomically(|| {
            let mut touched: HashSet<i64> = d.lines.iter().map(|l| l.account).collect();
            let entry = match d.id {
                Some(id) => {
                    touched.extend(self.load_draft(id)?.lines.iter().map(|l| l.account));
                    let n = self
                        .conn
                        .prepare_cached("UPDATE entries SET date=?2, payee=?3, memo=?4 WHERE id=?1")?
                        .execute(params![id, d.date, d.payee, d.memo])?;
                    if n != 1 {
                        return Err(no_entry(id));
                    }
                    let kept: HashSet<i64> = d.lines.iter().filter_map(|l| l.id).collect();
                    for old in self.posting_ids(id)? {
                        if !kept.contains(&old) {
                            self.conn.execute("DELETE FROM postings WHERE id=?1", [old])?;
                        }
                    }
                    id
                }
                None => {
                    self.conn
                        .prepare_cached("INSERT INTO entries(date,payee,memo,opening_account_id) VALUES(?1,?2,?3,?4)")?
                        .execute(params![d.date, d.payee, d.memo, d.opening])?;
                    self.conn.last_insert_rowid()
                }
            };
            let mut ids = Vec::with_capacity(d.lines.len());
            for l in &d.lines {
                let id = match l.id {
                    Some(id) if d.id.is_some() => {
                        let n = self
                            .conn
                            .prepare_cached(
                                "UPDATE postings SET ledger_account_id=?3, amount=?4, currency=?5, cleared=?6,
                                        import_hash=?7, payee=?8, memo=?9 WHERE id=?1 AND entry_id=?2",
                            )?
                            .execute(params![
                                id,
                                entry,
                                l.account,
                                l.amount,
                                l.currency,
                                l.cleared as i64,
                                l.import_hash,
                                l.payee,
                                l.memo
                            ])?;
                        if n != 1 {
                            return Err(no_entry(entry));
                        }
                        id
                    }
                    wanted => {
                        self.conn
                            .prepare_cached(
                                "INSERT INTO postings(id,entry_id,ledger_account_id,amount,currency,cleared,
                                                      import_hash,payee,memo)
                                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                            )?
                            .execute(params![
                                wanted,
                                entry,
                                l.account,
                                l.amount,
                                l.currency,
                                l.cleared as i64,
                                l.import_hash,
                                l.payee,
                                l.memo
                            ])?;
                        self.conn.last_insert_rowid()
                    }
                };
                ids.push(id);
            }
            self.check_bounds(&touched)?;
            Ok((entry, ids))
        })
    }

    /// Refuses what would make some sum too large to be read back.
    fn check_bounds(&self, touched: &HashSet<i64>) -> Result<()> {
        let turnover: Cents = self.conn.query_row("SELECT total FROM ledger_turnover", [], |r| r.get(0))?;
        if turnover > MAX_TURNOVER {
            bail!(tr("the amounts in the data file are too large"));
        }
        if turnover <= MAX_BALANCE {
            return Ok(());
        }
        let mut st = self.conn.prepare_cached(
            "SELECT 1 FROM postings WHERE ledger_account_id=?1 GROUP BY currency HAVING SUM(amount) NOT BETWEEN -?2 AND ?2",
        )?;
        for a in touched {
            if st.exists(params![a, MAX_BALANCE])? {
                bail!(tr("the balance of an account would be too large"));
            }
        }
        Ok(())
    }

    fn check_draft(&self, d: &Draft) -> Result<()> {
        if d.lines.len() < 2 {
            bail!(tr("an entry needs at least two postings"));
        }
        if d.lines.len() > MAX_LINES {
            bail!(trf("an entry has at most {n} postings", &[("n", &MAX_LINES)]));
        }
        let mut sums: BTreeMap<&str, (Cents, Cents)> = BTreeMap::new();
        for l in &d.lines {
            let row = self.ledger_row(l.account)?;
            if row.currency.as_ref().is_some_and(|c| *c != l.currency) {
                bail!(trf("a posting in {currency} on an account in another currency", &[("currency", &l.currency)]));
            }
            if l.amount.unsigned_abs() > MAX_AMOUNT.unsigned_abs() {
                bail!(tr("the amount is too large"));
            }
            let (debit, credit) = sums.entry(&l.currency).or_default();
            let side = if l.amount > 0 { debit } else { credit };
            *side = side.checked_add(l.amount.abs()).ok_or_else(too_big)?;
        }
        let several = sums.len() > 1;
        for (currency, (debit, credit)) in sums {
            if debit != credit {
                let (debit, credit) = (fmt_machine(debit), fmt_machine(credit));
                if several {
                    bail!(trf(
                        "entries across currencies are not supported: in {currency} debits are {debit} and credits {credit}",
                        &[("currency", &currency), ("debit", &debit), ("credit", &credit)]
                    ));
                }
                bail!(trf(
                    "the entry does not balance: debits {debit}, credits {credit} ({currency})",
                    &[("debit", &debit), ("credit", &credit), ("currency", &currency)]
                ));
            }
        }
        Ok(())
    }

    fn posting_ids(&self, entry: i64) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare_cached("SELECT id FROM postings WHERE entry_id=?1 ORDER BY id")?;
        Ok(st.query_map([entry], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?)
    }

    pub(crate) fn ledger_row(&self, id: i64) -> Result<LedgerRow> {
        let row = self
            .conn
            .prepare_cached(
                "SELECT class, currency, money_account_id, category_id, system FROM ledger_accounts WHERE id=?1",
            )?
            .query_row([id], |r| {
                Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, Option<String>>(4)?))
            })
            .optional()?;
        let (class, currency, money, category, system) =
            row.ok_or_else(|| anyhow!(trf("account {id} of the chart of accounts not found", &[("id", &id)])))?;
        let class = Class::from_code(&class).unwrap_or(Class::Expense);
        Ok(LedgerRow { class, currency, money, category, system })
    }

    pub(crate) fn load_draft(&self, id: i64) -> Result<Draft> {
        let mut d = self
            .conn
            .prepare_cached("SELECT date, payee, memo, opening_account_id FROM entries WHERE id=?1")?
            .query_row([id], |r| {
                Ok(Draft {
                    id: Some(id),
                    date: r.get(0)?,
                    payee: r.get(1)?,
                    memo: r.get(2)?,
                    opening: r.get(3)?,
                    lines: Vec::new(),
                })
            })
            .optional()?
            .ok_or_else(|| no_entry(id))?;
        let mut st = self.conn.prepare_cached(
            "SELECT id, ledger_account_id, amount, currency, cleared, import_hash, payee, memo
             FROM postings WHERE entry_id=?1 ORDER BY id",
        )?;
        let lines = st.query_map([id], |r| {
            Ok(Line {
                id: r.get(0)?,
                account: r.get(1)?,
                amount: r.get(2)?,
                currency: r.get(3)?,
                cleared: r.get::<_, i64>(4)? != 0,
                import_hash: r.get(5)?,
                payee: r.get(6)?,
                memo: r.get(7)?,
            })
        })?;
        for l in lines {
            d.lines.push(l?);
        }
        Ok(d)
    }

    /// The entry of a transaction: a posting on an account of the user, not an opening balance.
    pub(crate) fn entry_of_transaction(&self, posting: i64) -> Result<Option<i64>> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT e.id FROM postings p JOIN entries e ON e.id=p.entry_id
                 JOIN ledger_accounts l ON l.id=p.ledger_account_id
                 WHERE p.id=?1 AND l.money_account_id IS NOT NULL AND e.opening_account_id IS NULL",
            )?
            .query_row([posting], |r| r.get(0))
            .optional()?)
    }

    /// What the transaction is, and the index of the posting on the other side when there is one.
    fn kind_of(&self, d: &Draft, posting: i64) -> Result<(TxKind, usize)> {
        let [a, b] = d.lines.as_slice() else { return Ok((TxKind::Split, 0)) };
        let (me, other, i) = if a.id == Some(posting) { (a, b, 1) } else { (b, a, 0) };
        let row = self.ledger_row(other.account)?;
        let kind = if row.money.is_some() {
            if me.account == other.account { TxKind::Split } else { TxKind::Transfer }
        } else if row.category.is_some() {
            TxKind::Category
        } else if row.system.as_deref() == Some(UNCATEGORIZED) {
            TxKind::Uncategorized
        } else {
            TxKind::Other
        };
        Ok((kind, i))
    }

    /// The account of the chart that stands for a category, or for "uncategorized".
    pub(crate) fn category_ledger(&self, category: Option<i64>) -> Result<i64> {
        match category {
            None => self.system_ledger(UNCATEGORIZED),
            Some(c) => self
                .conn
                .prepare_cached("SELECT id FROM ledger_accounts WHERE category_id=?1")?
                .query_row([c], |r| r.get(0))
                .optional()?
                .ok_or_else(|| anyhow!(trf("category \"{name}\" not found", &[("name", &c)]))),
        }
    }

    pub(crate) fn system_ledger(&self, key: &str) -> Result<i64> {
        self.conn
            .prepare_cached("SELECT id FROM ledger_accounts WHERE system=?1")?
            .query_row([key], |r| r.get(0))
            .optional()?
            .ok_or_else(|| no_ledger_account(key))
    }

    /// The account of the chart that stands for an account of the user, with its currency.
    pub(crate) fn money_ledger(&self, account: i64) -> Result<(i64, String)> {
        self.conn
            .prepare_cached("SELECT id, currency FROM ledger_accounts WHERE money_account_id=?1")?
            .query_row([account], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
            .ok_or_else(|| anyhow!(trf("account \"{name}\" not found", &[("name", &account)])))
    }

    pub(crate) fn set_opening(&self, account: i64, opening: Cents) -> Result<()> {
        let old: Option<i64> = self
            .conn
            .query_row("SELECT id FROM entries WHERE opening_account_id=?1", [account], |r| r.get(0))
            .optional()?;
        if opening == 0 {
            if let Some(id) = old {
                self.remove_entry(id)?;
            }
            return Ok(());
        }
        let credit = opening.checked_neg().ok_or_else(too_big)?;
        let (ledger, currency) = self.money_ledger(account)?;
        let mut d = match old {
            Some(id) => self.load_draft(id)?,
            None => Draft {
                id: None,
                date: OPENING_DATE.into(),
                payee: String::new(),
                memo: String::new(),
                opening: Some(account),
                lines: vec![
                    Line { account: ledger, currency: currency.clone(), ..Default::default() },
                    Line { account: self.system_ledger(OPENING)?, currency, ..Default::default() },
                ],
            },
        };
        for l in &mut d.lines {
            l.amount = if l.account == ledger { opening } else { credit };
        }
        self.save_entry(&d)?;
        Ok(())
    }

    pub(crate) fn edit_transaction(&self, id: i64, tx: &TxEdit) -> Result<bool> {
        let Some(entry) = self.entry_of_transaction(id)? else { return Ok(false) };
        let mut d = self.load_draft(entry)?;
        let (kind, other) = self.kind_of(&d, id)?;
        let negated = tx.amount.checked_neg().ok_or_else(too_big)?;
        let unchanged = d.lines.iter().any(|l| l.id == Some(id) && l.amount == tx.amount);
        if kind == TxKind::Split && (!unchanged || tx.category_id.is_some()) {
            return Err(split_error());
        }
        let other_account = match (kind, tx.category_id) {
            (TxKind::Category | TxKind::Uncategorized, c) => Some(self.category_ledger(c)?),
            (TxKind::Other, Some(c)) => Some(self.category_ledger(Some(c))?),
            _ => None,
        };
        let (payee, memo) = (tx.payee.trim().to_string(), tx.memo.trim().to_string());
        d.date = d2s(tx.date);
        for (i, l) in d.lines.iter_mut().enumerate() {
            let mine = l.id == Some(id);
            if mine {
                l.cleared = tx.cleared;
            }
            if kind == TxKind::Transfer {
                // Each side has its own payee.
                if mine {
                    l.payee = Some(payee.clone());
                }
                l.memo = None;
            } else if mine {
                (l.payee, l.memo) = (None, None);
            }
            if kind != TxKind::Split {
                l.amount = if mine { tx.amount } else { negated };
                if i == other
                    && let Some(a) = other_account
                {
                    l.account = a;
                }
            }
        }
        if kind != TxKind::Transfer {
            d.payee = payee;
        }
        d.memo = memo;
        self.save_entry(&d)?;
        Ok(true)
    }

    /// Moves a transaction to a category; `false` when it is not there or is a transfer.
    pub(crate) fn move_to_category(&self, id: i64, category: Option<i64>) -> Result<bool> {
        let Some(entry) = self.entry_of_transaction(id)? else { return Ok(false) };
        let mut d = self.load_draft(entry)?;
        match self.kind_of(&d, id)? {
            (TxKind::Transfer, _) => return Ok(false),
            (TxKind::Split, _) => return Err(split_error()),
            (_, other) => {
                let account = self.category_ledger(category)?;
                if let Some(l) = d.lines.get_mut(other) {
                    l.account = account;
                }
            }
        }
        self.save_entry(&d)?;
        Ok(true)
    }

    /// Replaces a category with "uncategorized" in every entry, then drops its account.
    pub(crate) fn drop_category_ledger(&self, category: i64) -> Result<()> {
        let Ok(ledger) = self.category_ledger(Some(category)) else { return Ok(()) };
        let spare = self.system_ledger(UNCATEGORIZED)?;
        for entry in self.entries_on(ledger)? {
            let mut d = self.load_draft(entry)?;
            for l in d.lines.iter_mut().filter(|l| l.account == ledger) {
                l.account = spare;
            }
            self.save_entry(&d)?;
        }
        self.conn.execute("DELETE FROM ledger_accounts WHERE id=?1", [ledger])?;
        Ok(())
    }

    /// Deletes the entries of an account of the user, then its account in the chart. An entry that
    /// also touches another account with a balance of its own (the other side of a transfer) stays,
    /// with the deleted side on the opening balances: no other balance changes, and nothing becomes
    /// income or an expense.
    pub(crate) fn drop_money_ledger(&self, account: i64) -> Result<()> {
        let (ledger, _) = self.money_ledger(account)?;
        let spare = self.system_ledger(OPENING)?;
        for entry in self.entries_on(ledger)? {
            let mut d = self.load_draft(entry)?;
            let mut stays = false;
            for l in d.lines.iter().filter(|l| l.account != ledger) {
                let row = self.ledger_row(l.account)?;
                stays |= !matches!(row.class, Class::Income | Class::Expense) && row.system.is_none();
            }
            if !stays || d.opening == Some(account) {
                self.remove_entry(entry)?;
                continue;
            }
            for l in d.lines.iter_mut().filter(|l| l.account == ledger) {
                *l = Line {
                    id: l.id,
                    account: spare,
                    amount: l.amount,
                    currency: l.currency.clone(),
                    ..Default::default()
                };
            }
            self.save_entry(&d)?;
        }
        self.conn.execute("DELETE FROM ledger_accounts WHERE id=?1", [ledger])?;
        Ok(())
    }

    fn entries_on(&self, ledger: i64) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare("SELECT DISTINCT entry_id FROM postings WHERE ledger_account_id=?1")?;
        Ok(st.query_map([ledger], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?)
    }

    /// Posts an entry written by hand. Refused, with nothing written, when it does not balance.
    pub fn post_entry(&self, e: &NewEntry) -> Result<i64> {
        self.atomically(|| {
            let mut lines = Vec::new();
            let mut fixed: HashSet<String> = HashSet::new();
            for l in &e.lines {
                let own = self.ledger_row(l.ledger_account_id)?.currency;
                let asked = match l.currency.as_deref().map(normalize_currency) {
                    Some(c) if own.as_ref() != Some(&c) => Some(check_currency(&c)?),
                    c => c,
                };
                fixed.extend(own.clone().or_else(|| asked.clone()));
                lines.push((l, asked.or(own)));
            }
            let mut all = fixed.into_iter();
            let common = match (all.next(), all.next()) {
                (None, _) => Some("EUR".to_string()),
                (Some(c), None) => Some(c),
                _ => None,
            };
            let lines = lines
                .into_iter()
                .map(|(l, currency)| {
                    let currency = currency.or_else(|| common.clone()).ok_or_else(|| {
                        anyhow!(tr("an entry in several currencies needs the currency of each posting"))
                    })?;
                    Ok(Line { account: l.ledger_account_id, amount: l.amount, currency, ..Default::default() })
                })
                .collect::<Result<_>>()?;
            let d = Draft {
                id: None,
                date: d2s(e.date),
                payee: e.payee.trim().into(),
                memo: e.memo.trim().into(),
                opening: None,
                lines,
            };
            Ok(self.save_entry(&d)?.0)
        })
    }

    /// Deletes a whole entry, every side of it.
    pub fn delete_entry(&self, id: i64) -> Result<()> {
        if self.load_draft(id)?.opening.is_some() {
            bail!(tr("the opening balance is changed in the account"));
        }
        self.remove_entry(id)
    }

    /// Every side of an entry goes, so the books stay balanced.
    fn remove_entry(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM entries WHERE id=?1", [id])?;
        Ok(())
    }

    /// Gives a posting on income or expenses to another category, or to none.
    pub fn set_posting_category(&self, posting: i64, category: Option<i64>) -> Result<()> {
        self.atomically(|| {
            let entry: Option<i64> =
                self.conn.query_row("SELECT entry_id FROM postings WHERE id=?1", [posting], |r| r.get(0)).optional()?;
            let mut d = self.load_draft(entry.ok_or_else(|| no_posting(posting))?)?;
            let account = self.category_ledger(category)?;
            for l in d.lines.iter_mut().filter(|l| l.id == Some(posting)) {
                let row = self.ledger_row(l.account)?;
                if row.category.is_none() && row.system.as_deref() != Some(UNCATEGORIZED) {
                    bail!(tr("only a posting on a category can be given another one"));
                }
                l.account = account;
            }
            self.save_entry(&d)?;
            Ok(())
        })
    }

    /// Changes the currency of an account of the user, with its side of every entry; refused when that
    /// leaves one unbalanced.
    pub fn set_account_currency(&self, account: i64, currency: &str) -> Result<()> {
        let currency = check_currency(currency)?;
        self.atomically(|| {
            let (ledger, old) = self.money_ledger(account)?;
            if old == currency {
                return Ok(());
            }
            self.conn.execute("UPDATE accounts SET currency=?2 WHERE id=?1", params![account, currency])?;
            self.conn.execute("UPDATE ledger_accounts SET currency=?2 WHERE id=?1", params![ledger, currency])?;
            for entry in self.entries_on(ledger)? {
                let mut d = self.load_draft(entry)?;
                for l in d.lines.iter_mut().filter(|l| l.currency == old) {
                    // The other side follows when it takes any currency.
                    if l.account == ledger || self.ledger_row(l.account)?.currency.is_none() {
                        l.currency.clone_from(&currency);
                    }
                }
                self.save_entry(&d).with_context(|| {
                    trf(
                        "the currency cannot be changed: entry {id} of {date} also moves another account",
                        &[("id", &entry), ("date", &d.date)],
                    )
                })?;
            }
            Ok(())
        })
    }

    // -------------------------------------------------- chart of accounts

    /// Every account of the chart, by class and then by code and name.
    pub fn ledger_accounts(&self) -> Result<Vec<LedgerAccount>> {
        let mut balances: HashMap<i64, Vec<Balance>> = HashMap::new();
        for (account, currency, amount) in self.ledger_sums(None)? {
            balances.entry(account).or_default().push(Balance { currency, amount });
        }
        let mut st = self.conn.prepare(
            "SELECT l.id, l.class, l.parent_id, COALESCE(a.name, c.name, l.name), l.code, l.currency,
                    l.money_account_id, l.category_id, l.system
             FROM ledger_accounts l LEFT JOIN accounts a ON a.id=l.money_account_id
             LEFT JOIN categories c ON c.id=l.category_id",
        )?;
        let rows = st.query_map([], |r| {
            let system: Option<String> = r.get(8)?;
            let name: String = match system.as_deref() {
                Some(OPENING) => tr("Opening balances").into(),
                Some(UNCATEGORIZED) => tr("Uncategorized").into(),
                _ => r.get(3)?,
            };
            Ok(LedgerAccount {
                id: r.get(0)?,
                class: Class::from_code(&r.get::<_, String>(1)?).unwrap_or(Class::Expense),
                parent_id: r.get(2)?,
                path: name.clone(),
                name,
                code: r.get(4)?,
                currency: r.get(5)?,
                money_account_id: r.get(6)?,
                category_id: r.get(7)?,
                system,
                balances: Vec::new(),
            })
        })?;
        let mut out: Vec<LedgerAccount> = rows.collect::<std::result::Result<_, _>>()?;
        let names: HashMap<i64, (String, Option<i64>)> =
            out.iter().map(|a| (a.id, (a.name.clone(), a.parent_id))).collect();
        for a in &mut out {
            a.balances = balances.remove(&a.id).unwrap_or_default();
            let mut above = a.parent_id;
            for _ in 0..MAX_DEPTH {
                let Some((name, next)) = above.and_then(|p| names.get(&p)) else { break };
                a.path = format!("{name}:{}", a.path);
                above = *next;
            }
        }
        out.sort_by_cached_key(|a| (a.class, a.system.is_some(), a.code.clone(), a.path.to_lowercase(), a.id));
        Ok(out)
    }

    /// Finds an account of the chart by code, system key, path, name or id.
    pub fn find_ledger_account(&self, key: &str) -> Result<LedgerAccount> {
        find_ledger_account(&self.ledger_accounts()?, key)
    }

    /// Adds an account to the chart, under `parent` or at the top of `class`. An income or expense
    /// account is a category, so that budgets and reports see it.
    pub fn create_ledger_account(
        &self,
        name: &str,
        class: Option<Class>,
        parent: Option<i64>,
        code: &str,
        currency: Option<&str>,
    ) -> Result<i64> {
        let (name, code) = (name.trim(), code.trim());
        if name.is_empty() {
            bail!(tr("the account needs a name"));
        }
        if name.contains(':') {
            bail!(tr("the name of an account cannot contain a colon"));
        }
        self.atomically(|| {
            let all = self.ledger_accounts()?;
            let above = match parent {
                Some(p) => Some(all.iter().find(|a| a.id == p).ok_or_else(|| no_ledger_account(&p.to_string()))?),
                None => None,
            };
            if above.is_some_and(|p| p.system.is_some()) {
                bail!(tr("this account cannot have accounts under it"));
            }
            if above.is_some_and(|p| p.path.matches(':').count() + 2 > MAX_DEPTH) {
                bail!(tr("there are too many levels of accounts"));
            }
            let class = match (class, above) {
                (Some(c), Some(p)) if c != p.class => {
                    bail!(tr("an account has the class of the one above it"))
                }
                (Some(c), _) => c,
                (None, Some(p)) => p.class,
                (None, None) => bail!(tr("the account needs a class or an account above it")),
            };
            let lower = name.to_lowercase();
            if all.iter().any(|a| a.parent_id == parent && a.class == class && a.name.to_lowercase() == lower) {
                bail!(trf("is there already an account named \"{name}\"?", &[("name", &name)]));
            }
            if !code.is_empty() && all.iter().any(|a| a.code == code) {
                bail!(trf("the code \"{code}\" is already in use", &[("code", &code)]));
            }
            let id = if matches!(class, Class::Income | Class::Expense) {
                if currency.is_some_and(|c| !c.trim().is_empty()) {
                    bail!(tr("an income or expense account has no currency of its own"));
                }
                self.conn.execute(
                    "INSERT INTO categories(name,parent_id,is_income) VALUES(?1,?2,?3)",
                    params![name, above.and_then(|p| p.category_id), (class == Class::Income) as i64],
                )?;
                self.category_ledger(Some(self.conn.last_insert_rowid()))?
            } else {
                let inherited = above.and_then(|p| p.currency.clone());
                let currency = match currency.filter(|c| !c.trim().is_empty()) {
                    Some(c) => check_currency(c)?,
                    None => check_currency(&inherited.unwrap_or_default())?,
                };
                self.conn.execute(
                    "INSERT INTO ledger_accounts(class,parent_id,name,currency) VALUES(?1,?2,?3,?4)",
                    params![class.code(), parent, name, currency],
                )?;
                self.conn.last_insert_rowid()
            };
            self.conn.execute("UPDATE ledger_accounts SET code=?2 WHERE id=?1", params![id, code])?;
            Ok(id)
        })
    }

    /// An account added by hand, the only kind that is changed in the chart.
    fn hand_made(&self, id: i64) -> Result<()> {
        let row = self.ledger_row(id)?;
        if row.money.is_some() || row.category.is_some() || row.system.is_some() {
            bail!(tr("only accounts added to the chart by hand can be changed here"));
        }
        Ok(())
    }

    pub fn rename_ledger_account(&self, id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            bail!(tr("the account needs a name"));
        }
        if name.contains(':') {
            bail!(tr("the name of an account cannot contain a colon"));
        }
        self.atomically(|| {
            self.hand_made(id)?;
            let all = self.ledger_accounts()?;
            let lower = name.to_lowercase();
            if let Some(me) = all.iter().find(|a| a.id == id)
                && all.iter().any(|a| {
                    a.id != id && a.parent_id == me.parent_id && a.class == me.class && a.name.to_lowercase() == lower
                })
            {
                bail!(trf("is there already an account named \"{name}\"?", &[("name", &name)]));
            }
            self.conn.execute("UPDATE ledger_accounts SET name=?2 WHERE id=?1", params![id, name])?;
            Ok(())
        })
    }

    /// Deletes an account added by hand, when nothing is on it or under it.
    pub fn delete_ledger_account(&self, id: i64) -> Result<()> {
        self.atomically(|| {
            self.hand_made(id)?;
            let used: bool = self.conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM postings WHERE ledger_account_id=?1)
                     OR EXISTS (SELECT 1 FROM ledger_accounts WHERE parent_id=?1)",
                [id],
                |r| r.get(0),
            )?;
            if used {
                bail!(tr("the account is in use: it has entries or accounts under it"));
            }
            self.conn.execute("DELETE FROM ledger_accounts WHERE id=?1", [id])?;
            Ok(())
        })
    }

    // ------------------------------------------------------------- reading

    /// Sums of the postings up to a date, by account and currency.
    fn ledger_sums(&self, until: Option<NaiveDate>) -> Result<Vec<(i64, String, Cents)>> {
        let mut st = self.conn.prepare(
            "SELECT p.ledger_account_id, p.currency, SUM(p.amount) FROM postings p JOIN entries e ON e.id=p.entry_id
             WHERE ?1 IS NULL OR e.date<=?1 GROUP BY p.ledger_account_id, p.currency ORDER BY 1, 2",
        )?;
        let rows = st.query_map([until.map(d2s)], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// The balance of every account up to a date. Debits and credits are equal in each currency.
    pub fn trial_balance(&self, until: Option<NaiveDate>) -> Result<TrialBalance> {
        let sums = self.ledger_sums(until)?;
        let mut out = TrialBalance::default();
        for a in self.ledger_accounts()? {
            for (_, currency, amount) in sums.iter().filter(|s| s.0 == a.id && s.2 != 0) {
                out.lines.push(TrialLine {
                    account_id: a.id,
                    account: a.path.clone(),
                    code: a.code.clone(),
                    class: a.class,
                    currency: currency.clone(),
                    amount: *amount,
                });
            }
        }
        let mut totals: BTreeMap<&str, (Cents, Cents)> = BTreeMap::new();
        for l in &out.lines {
            let (debit, credit) = totals.entry(&l.currency).or_default();
            let side = if l.amount > 0 { debit } else { credit };
            *side = l.amount.checked_abs().and_then(|a| side.checked_add(a)).ok_or_else(too_big)?;
        }
        out.totals = totals
            .into_iter()
            .map(|(currency, (debit, credit))| TrialTotal { currency: currency.into(), debit, credit })
            .collect();
        Ok(out)
    }

    /// Entries, newest first.
    pub fn entries(&self, f: &EntryFilter) -> Result<Vec<Entry>> {
        let mut st = self.conn.prepare(
            "SELECT id FROM entries e
             WHERE (?1 IS NULL OR EXISTS (SELECT 1 FROM postings p WHERE p.entry_id=e.id AND p.ledger_account_id=?1))
               AND (?2 IS NULL OR date>=?2) AND (?3 IS NULL OR date<=?3)
             ORDER BY date DESC, id DESC LIMIT ?4 OFFSET ?5",
        )?;
        let limit = f.limit.map_or(-1, |n| i64::try_from(n).unwrap_or(-1));
        let offset = i64::try_from(f.offset).unwrap_or(i64::MAX);
        let ids: Vec<i64> = st
            .query_map(params![f.ledger_account_id, f.from.map(d2s), f.to.map(d2s), limit, offset], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let paths: HashMap<i64, String> = self.ledger_accounts()?.into_iter().map(|a| (a.id, a.path)).collect();
        ids.into_iter().map(|id| self.entry_with(id, &paths)).collect()
    }

    pub fn entry(&self, id: i64) -> Result<Entry> {
        let paths = self.ledger_accounts()?.into_iter().map(|a| (a.id, a.path)).collect();
        self.entry_with(id, &paths)
    }

    fn entry_with(&self, id: i64, paths: &HashMap<i64, String>) -> Result<Entry> {
        let d = self.load_draft(id)?;
        let postings = d
            .lines
            .into_iter()
            .map(|l| Posting {
                id: l.id.unwrap_or_default(),
                account_id: l.account,
                account: paths.get(&l.account).cloned().unwrap_or_default(),
                amount: l.amount,
                currency: l.currency,
                cleared: l.cleared,
            })
            .collect();
        Ok(Entry { id, date: s2d(&d.date), payee: d.payee, memo: d.memo, opening: d.opening.is_some(), postings })
    }

    /// Checks the books: what is wrong, one sentence each.
    pub fn verify_ledger(&self) -> Result<Verification> {
        let mut problems = Vec::new();
        let ids = |sql: &str| -> Result<Vec<i64>> {
            let mut st = self.conn.prepare(sql)?;
            Ok(st.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?)
        };
        for id in ids("SELECT e.id FROM entries e WHERE (SELECT COUNT(*) FROM postings p WHERE p.entry_id=e.id) < 2")? {
            problems.push(trf("entry {id} has fewer than two postings", &[("id", &id)]));
        }
        for id in ids("SELECT DISTINCT entry_id FROM
                       (SELECT entry_id FROM postings GROUP BY entry_id, currency HAVING SUM(amount) <> 0)")?
        {
            problems.push(trf("entry {id} does not balance", &[("id", &id)]));
        }
        for id in ids("SELECT p.id FROM postings p
                       LEFT JOIN entries e ON e.id=p.entry_id LEFT JOIN ledger_accounts l ON l.id=p.ledger_account_id
                       WHERE e.id IS NULL OR l.id IS NULL")?
        {
            problems.push(trf("posting {id} has no entry or no account", &[("id", &id)]));
        }
        for id in ids("SELECT p.id FROM postings p JOIN ledger_accounts l ON l.id=p.ledger_account_id
                       WHERE l.currency IS NOT NULL AND l.currency <> p.currency")?
        {
            problems.push(trf("posting {id} is not in the currency of its account", &[("id", &id)]));
        }
        for id in ids("SELECT e.id FROM entries e WHERE e.opening_account_id IS NOT NULL AND
                       (SELECT COUNT(*) FROM postings p WHERE p.entry_id=e.id) <>
                       (SELECT COUNT(*) FROM postings p JOIN ledger_accounts l ON l.id=p.ledger_account_id
                        WHERE p.entry_id=e.id
                          AND (l.money_account_id=e.opening_account_id OR l.system='opening_balances'))")?
        {
            problems.push(trf("entry {id} is not a sound opening balance", &[("id", &id)]));
        }
        for id in ids("SELECT a.id FROM accounts a LEFT JOIN ledger_accounts l ON l.money_account_id=a.id
                       WHERE l.id IS NULL OR l.currency IS NULL OR
                             l.class <> CASE WHEN a.kind IN ('credit_card','loan') THEN 'liability' ELSE 'asset' END")?
        {
            problems.push(trf("account {id} has no proper account in the chart", &[("id", &id)]));
        }
        for id in ids("SELECT c.id FROM categories c LEFT JOIN ledger_accounts l ON l.category_id=c.id
                       LEFT JOIN ledger_accounts u ON u.category_id=c.parent_id
                       WHERE l.id IS NULL OR l.parent_id IS NOT u.id OR
                             l.class <> CASE WHEN c.is_income THEN 'income' ELSE 'expense' END")?
        {
            problems.push(trf("category {id} has no proper account in the chart", &[("id", &id)]));
        }
        for id in ids("SELECT l.id FROM ledger_accounts l
                       LEFT JOIN accounts a ON a.id=l.money_account_id LEFT JOIN categories c ON c.id=l.category_id
                       WHERE (l.money_account_id IS NOT NULL AND a.id IS NULL)
                          OR (l.category_id IS NOT NULL AND c.id IS NULL)
                          OR (l.money_account_id IS NOT NULL) + (l.category_id IS NOT NULL) + (l.system IS NOT NULL) > 1")?
        {
            problems.push(trf("account {id} of the chart of accounts stands for nothing", &[("id", &id)]));
        }
        for _ in ids("SELECT id FROM ledger_turnover WHERE
                      total <> (SELECT COALESCE(SUM(abs(amount)),0) FROM postings)")?
        {
            problems.push(tr("the sum of the postings is not the one on record").into());
        }
        for key in [OPENING, UNCATEGORIZED] {
            if self.system_ledger(key).is_err() {
                problems.push(no_ledger_account(key).to_string());
            }
        }
        let count = |sql: &str| -> Result<usize> {
            Ok(usize::try_from(self.conn.query_row(sql, [], |r| r.get::<_, i64>(0))?).unwrap_or(0))
        };
        Ok(Verification {
            entries: count("SELECT COUNT(*) FROM entries")?,
            postings: count("SELECT COUNT(*) FROM postings")?,
            problems,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests;
