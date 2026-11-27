//! Zero-based budgeting with envelopes, beside the monthly budgets of the categories. Every euro that arrives waits in
//! *to be assigned* until it is given to an envelope; an envelope has what was assigned to it, what its categories
//! spent and what is left, which goes on to the next month. Nothing of a month is stored but the changes the user made:
//! every figure is computed again from the ledger, from the first month of the envelopes on, so a change to the past
//! is seen in every month after it. `docs/envelopes.md` has the rules.

use crate::db::{Category, Cents, Db, first_of_month, today};
use crate::i18n::{tr, trf};
use crate::ledger::{MAX_BALANCE, MAX_TURNOVER, too_big};
use crate::money::fmt_eur;
use anyhow::{Result, anyhow, bail};
use chrono::{Datelike, Months, NaiveDate};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

/// The tables of the envelopes (format 10). A category is in one envelope at most. The changes are only ever added:
/// an undo is a change too, with the opposite amounts, so the history stays as it was made.
pub(crate) const ENVELOPE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS envelopes (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    overspending TEXT NOT NULL DEFAULT 'carry' CHECK (overspending IN ('carry','next_month'))
);
CREATE TABLE IF NOT EXISTS envelope_categories (
    category_id INTEGER PRIMARY KEY REFERENCES categories(id) ON DELETE CASCADE,
    envelope_id INTEGER NOT NULL REFERENCES envelopes(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS envelope_category_envelope ON envelope_categories(envelope_id);
CREATE TABLE IF NOT EXISTS envelope_changes (
    id INTEGER PRIMARY KEY,
    group_id INTEGER NOT NULL,
    month TEXT NOT NULL CHECK (length(month) = 7),
    kind TEXT NOT NULL CHECK (kind IN ('assign','move')),
    from_envelope_id INTEGER REFERENCES envelopes(id) ON DELETE SET NULL,
    to_envelope_id INTEGER REFERENCES envelopes(id) ON DELETE SET NULL,
    amount INTEGER NOT NULL CHECK (typeof(amount) = 'integer'),
    made_at TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    undoes INTEGER
);
CREATE INDEX IF NOT EXISTS envelope_change_month ON envelope_changes(month);
CREATE INDEX IF NOT EXISTS envelope_change_group ON envelope_changes(group_id);
";

/// How many months after the first one can be asked for: a hundred years.
pub const MAX_MONTHS: u32 = 1200;
/// How many envelopes a file may have.
pub const MAX_ENVELOPES: usize = 500;
/// The longest name and reason.
pub const MAX_TEXT: usize = 200;

/// What the budget screen of a file shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// A limit a month for each category, as before the envelopes.
    Monthly,
    Envelopes,
}

impl Mode {
    pub fn code(self) -> &'static str {
        match self {
            Mode::Monthly => "monthly",
            Mode::Envelopes => "envelopes",
        }
    }

    pub fn from_code(s: &str) -> Option<Mode> {
        match s.trim().to_lowercase().as_str() {
            "monthly" | "simple" | "mensile" => Some(Mode::Monthly),
            "envelopes" | "envelope" | "buste" => Some(Mode::Envelopes),
            _ => None,
        }
    }
}

/// What happens to an envelope that ends a month below zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Overspending {
    /// It starts the next month below zero, until money is moved into it.
    Carry,
    /// It starts the next month at zero, and what it lacked is taken from the next month's *to be assigned*.
    NextMonth,
}

impl Overspending {
    pub fn code(self) -> &'static str {
        match self {
            Overspending::Carry => "carry",
            Overspending::NextMonth => "next_month",
        }
    }

    pub fn from_code(s: &str) -> Option<Overspending> {
        match s.trim().to_lowercase().as_str() {
            "carry" => Some(Overspending::Carry),
            "next_month" | "next-month" | "next" => Some(Overspending::NextMonth),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Overspending::Carry => tr("Carry it into the next month"),
            Overspending::NextMonth => tr("Take it from next month's money to assign"),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Envelope {
    pub id: i64,
    pub name: String,
    pub overspending: Overspending,
    pub category_ids: Vec<i64>,
}

/// An envelope in a month.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EnvelopeLine {
    pub id: i64,
    pub name: String,
    pub overspending: Overspending,
    pub categories: Vec<String>,
    /// The monthly budgets of its categories, as a guide to what to assign.
    pub budget: Cents,
    /// What was left at the end of the month before; below zero only with [`Overspending::Carry`].
    pub carried: Cents,
    pub assigned: Cents,
    /// Spending of its categories and of those under them; refunds take it down.
    pub spent: Cents,
    /// `carried + assigned - spent`.
    pub available: Cents,
}

/// One change of the assigned amounts: an amount given to an envelope (from *to be assigned*, or back to it when
/// negative), moved from one to another, or the undoing of another change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Change {
    pub id: i64,
    /// The changes made together, undone together.
    pub group_id: i64,
    pub month: String,
    pub kind: ChangeKind,
    /// None: *to be assigned*, or an envelope that was deleted.
    pub from: Option<String>,
    pub to: Option<String>,
    pub amount: Cents,
    pub made_at: String,
    pub reason: String,
    /// The group this one undoes.
    pub undoes: Option<i64>,
    /// Undone by a later change.
    pub undone: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Assign,
    Move,
    Undo,
}

/// A month of the envelopes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MonthView {
    pub mode: Mode,
    /// `YYYY-MM`.
    pub month: String,
    /// The first month of the envelopes.
    pub start: String,
    /// The month comes before the envelopes: everything is zero.
    pub before_start: bool,
    /// The month is over: a change to it is seen in every month after it.
    pub past: bool,
    /// What *to be assigned* had at the end of the month before (below zero when more was assigned than there was).
    pub left_over: Cents,
    /// Income of the month: the income categories, refunds of income taken off.
    pub income: Cents,
    /// Spending of categories in no envelope; it is taken from *to be assigned*.
    pub outside: Cents,
    /// Uncategorized money, out minus in; it is taken from *to be assigned* too.
    pub uncategorized: Cents,
    /// What envelopes with [`Overspending::NextMonth`] lacked at the end of the month before.
    pub overspent_before: Cents,
    /// Assigned this month, moves between envelopes cancelling out.
    pub assigned: Cents,
    /// Must be zero: below zero is more assigned than there is.
    pub to_be_assigned: Cents,
    pub envelopes: Vec<EnvelopeLine>,
    /// Postings in other currencies, which the envelopes do not count.
    pub other_currencies: i64,
    /// The changes made to this month, last first.
    pub changes: Vec<Change>,
}

impl MonthView {
    pub fn overspent(&self) -> impl Iterator<Item = &EnvelopeLine> {
        self.envelopes.iter().filter(|e| e.available < 0)
    }
}

/// The figures of one month that the next one needs, and those that tell whether a change moved something.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Computed {
    month: NaiveDate,
    left_over: i128,
    income: i128,
    outside: i128,
    uncategorized: i128,
    overspent_before: i128,
    assigned: i128,
    to_be_assigned: i128,
    other_currencies: i64,
    /// By envelope: carried, assigned, spent, available.
    lines: BTreeMap<i64, [i128; 4]>,
}

pub fn month_key(d: NaiveDate) -> String {
    d.format("%Y-%m").to_string()
}

pub fn parse_month(s: &str) -> Option<NaiveDate> {
    let t = s.trim();
    if t.len() != 7 {
        return None;
    }
    NaiveDate::parse_from_str(&format!("{t}-01"), "%Y-%m-%d").ok()
}

fn cents(v: i128) -> Result<Cents> {
    Cents::try_from(v).map_err(|_| too_big())
}

fn now_text() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Text of the user, trimmed; `long` is the error when it is too long.
fn clean_text(s: &str, long: &str) -> Result<String> {
    let t = s.trim();
    if t.chars().count() > MAX_TEXT {
        bail!(long.to_string());
    }
    Ok(t.to_string())
}

/// The postings of a month on a category, or on *Uncategorized*: whether it is income, the category, whether in euros,
/// their sum and how many.
type Flow = (bool, Option<i64>, bool, i64, i64);
/// A row of a change: from, to, amount.
type Moved = (Option<i64>, Option<i64>, i64);

const ASSIGN: &str = "assign";
const MOVE: &str = "move";

/// One row of a change: `amount` leaves `from` and reaches `to`, where None is *to be assigned*.
#[derive(Clone, Copy)]
struct Row {
    month: NaiveDate,
    from: Option<i64>,
    to: Option<i64>,
    amount: Cents,
    /// `assign` or `move`; an undo has the kind of what it undoes.
    kind: &'static str,
}

fn months_between(from: NaiveDate, to: NaiveDate) -> i64 {
    i64::from(to.year() - from.year()) * 12 + i64::from(to.month()) - i64::from(from.month())
}

impl Db {
    pub fn budget_mode(&self) -> Mode {
        self.meta("budget_mode").and_then(|m| Mode::from_code(&m)).unwrap_or(Mode::Monthly)
    }

    /// The first month of the envelopes: the one chosen, else this one.
    pub fn envelopes_start(&self) -> NaiveDate {
        self.meta("envelopes_from").and_then(|m| parse_month(&m)).unwrap_or_else(|| first_of_month(today()))
    }

    /// Chooses what the budget screen shows. When envelopes are chosen and there are none, and no change was ever
    /// made, each category with a monthly budget gets one, with that budget assigned in the first month (this one,
    /// unless another was chosen): the number of envelopes made.
    pub fn set_budget_mode(&self, mode: Mode) -> Result<usize> {
        self.atomically(|| {
            let mut made = 0;
            if mode == Mode::Envelopes {
                if self.meta("envelopes_from").is_none() {
                    self.set_meta("envelopes_from", &month_key(first_of_month(today())))?;
                }
                let changed: bool =
                    self.conn.query_row("SELECT EXISTS (SELECT 1 FROM envelope_changes)", [], |r| r.get(0))?;
                if self.envelopes()?.is_empty() && !changed {
                    made = self.envelopes_from_budgets(self.envelopes_start())?;
                }
            }
            self.set_meta("budget_mode", mode.code())?;
            Ok(made)
        })
    }

    /// Changes the first month: every month after it is computed again from there.
    pub fn set_envelopes_start(&self, month: NaiveDate) -> Result<()> {
        let month = first_of_month(month);
        let far = first_of_month(today()).checked_add_months(Months::new(MAX_MONTHS)).ok_or_else(too_big)?;
        if month.year() < 1900 || month > far {
            bail!(tr("the first month of the envelopes is out of range"));
        }
        self.set_meta("envelopes_from", &month_key(month))
    }

    /// An envelope for each expense category with a monthly budget, linked to it, with the budget assigned in `month`.
    fn envelopes_from_budgets(&self, month: NaiveDate) -> Result<usize> {
        let cats = self.categories()?;
        let mut st = self.conn.prepare("SELECT category_id, monthly FROM category_budgets ORDER BY category_id")?;
        let budgets: Vec<(i64, i64)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?;
        let mut made = 0;
        let mut group = None;
        for (cid, monthly) in budgets {
            let Some(c) = cats.iter().find(|c| c.id == cid && !c.is_income) else {
                continue;
            };
            if self.envelopes()?.iter().any(|e| e.name.to_lowercase() == c.path.to_lowercase()) {
                continue;
            }
            let id = self.add_envelope(&c.path, &[cid], Overspending::Carry)?;
            made += 1;
            let amount = monthly.clamp(0, MAX_BALANCE);
            if amount != 0 {
                let g = self.write_change(
                    group,
                    Row { month, from: None, to: Some(id), amount, kind: ASSIGN },
                    tr("From the monthly budget"),
                    None,
                )?;
                group = Some(g);
            }
        }
        Ok(made)
    }

    pub fn envelopes(&self) -> Result<Vec<Envelope>> {
        let mut st =
            self.conn.prepare("SELECT id, name, overspending FROM envelopes ORDER BY name COLLATE NOCASE, id")?;
        let rows: Vec<(i64, String, String)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<std::result::Result<_, _>>()?;
        let mut links: HashMap<i64, Vec<i64>> = HashMap::new();
        let mut st =
            self.conn.prepare("SELECT category_id, envelope_id FROM envelope_categories ORDER BY category_id")?;
        for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
            let (c, e) = row?;
            links.entry(e).or_default().push(c);
        }
        Ok(rows
            .into_iter()
            .map(|(id, name, o)| Envelope {
                id,
                name,
                overspending: Overspending::from_code(&o).unwrap_or(Overspending::Carry),
                category_ids: links.remove(&id).unwrap_or_default(),
            })
            .collect())
    }

    /// An envelope by id or name.
    pub fn find_envelope(&self, key: &str) -> Result<Envelope> {
        let key = key.trim();
        let all = self.envelopes()?;
        if let Ok(id) = key.parse::<i64>()
            && let Some(e) = all.iter().find(|e| e.id == id)
        {
            return Ok(e.clone());
        }
        let lk = key.to_lowercase();
        if let Some(e) = all.iter().find(|e| e.name.to_lowercase() == lk) {
            return Ok(e.clone());
        }
        let partial: Vec<&Envelope> = all.iter().filter(|e| e.name.to_lowercase().contains(&lk)).collect();
        if let ([only], false) = (partial.as_slice(), lk.is_empty()) {
            return Ok((*only).clone());
        }
        Err(anyhow!(trf("envelope \"{name}\" not found", &[("name", &key)])))
    }

    fn check_name(&self, name: &str, other_than: Option<i64>) -> Result<String> {
        let name = clean_text(name, tr("the name is too long"))?;
        if name.is_empty() {
            bail!(tr("the envelope needs a name"));
        }
        if self.envelopes()?.iter().any(|e| Some(e.id) != other_than && e.name.to_lowercase() == name.to_lowercase()) {
            bail!(trf("there is already an envelope \"{name}\"", &[("name", &name)]));
        }
        Ok(name)
    }

    /// Links the categories to an envelope, in place of those it had. Income categories are refused, and so is
    /// a category of another envelope.
    fn link_categories(&self, envelope: i64, ids: &[i64]) -> Result<()> {
        let cats = self.categories()?;
        if ids.len() > 1000 {
            bail!(tr("too many categories"));
        }
        let mut st = self.conn.prepare(
            "SELECT e.name FROM envelope_categories c JOIN envelopes e ON e.id=c.envelope_id
                                         WHERE c.category_id=?1 AND c.envelope_id<>?2",
        )?;
        for id in ids {
            let c = cats
                .iter()
                .find(|c| c.id == *id)
                .ok_or_else(|| anyhow!(trf("category \"{name}\" not found", &[("name", id)])))?;
            if c.is_income {
                bail!(trf(
                    "\"{category}\" is an income category: its money is to be assigned",
                    &[("category", &c.path)]
                ));
            }
            if let Some(other) = st.query_row(params![id, envelope], |r| r.get::<_, String>(0)).optional()? {
                bail!(trf(
                    "\"{category}\" is already in the envelope \"{name}\"",
                    &[("category", &c.path), ("name", &other)]
                ));
            }
        }
        self.conn.execute("DELETE FROM envelope_categories WHERE envelope_id=?1", [envelope])?;
        for id in ids {
            self.conn.execute(
                "INSERT OR IGNORE INTO envelope_categories(category_id, envelope_id) VALUES (?1, ?2)",
                params![id, envelope],
            )?;
        }
        Ok(())
    }

    pub fn add_envelope(&self, name: &str, categories: &[i64], overspending: Overspending) -> Result<i64> {
        self.atomically(|| {
            let name = self.check_name(name, None)?;
            if self.envelopes()?.len() >= MAX_ENVELOPES {
                bail!(tr("there are too many envelopes"));
            }
            self.conn.execute(
                "INSERT INTO envelopes(id, name, overspending) VALUES (?1, ?2, ?3)",
                params![self.new_id("envelopes")?, name, overspending.code()],
            )?;
            let id = self.conn.last_insert_rowid();
            self.link_categories(id, categories)?;
            Ok(id)
        })
    }

    /// Changes what is given; `None` leaves it as it is.
    pub fn edit_envelope(
        &self,
        id: i64,
        name: Option<&str>,
        categories: Option<&[i64]>,
        overspending: Option<Overspending>,
    ) -> Result<()> {
        self.atomically(|| {
            if !self.envelopes()?.iter().any(|e| e.id == id) {
                bail!(trf("envelope \"{name}\" not found", &[("name", &id)]));
            }
            if let Some(n) = name {
                let n = self.check_name(n, Some(id))?;
                self.conn.execute("UPDATE envelopes SET name=?2 WHERE id=?1", params![id, n])?;
            }
            if let Some(c) = categories {
                self.link_categories(id, c)?;
            }
            if let Some(o) = overspending {
                self.conn.execute("UPDATE envelopes SET overspending=?2 WHERE id=?1", params![id, o.code()])?;
            }
            Ok(())
        })
    }

    /// Deletes an envelope. Every month is then as if it had never been: what was assigned to it is to be assigned
    /// again, and the spending of its categories, which are in no envelope any more, is taken from there.
    pub fn delete_envelope(&self, id: i64) -> Result<()> {
        if self.conn.execute("DELETE FROM envelopes WHERE id=?1", [id])? == 0 {
            bail!(trf("envelope \"{name}\" not found", &[("name", &id)]));
        }
        Ok(())
    }

    /// Adds one row of a change; the first row of a group gives the group its id.
    fn write_change(&self, group: Option<i64>, row: Row, reason: &str, undoes: Option<i64>) -> Result<i64> {
        let Row { month, from, to, amount, kind } = row;
        if !(-MAX_BALANCE..=MAX_BALANCE).contains(&amount) {
            bail!(trf("\"{amount}\" is too large", &[("amount", &fmt_eur(amount))]));
        }
        let used: i64 =
            self.conn.query_row("SELECT COALESCE(SUM(abs(amount)),0) FROM envelope_changes", [], |r| r.get(0))?;
        if used.checked_add(amount.abs()).is_none_or(|t| t > MAX_TURNOVER) {
            bail!(tr("the envelopes have had too many changes"));
        }
        self.conn.execute(
            "INSERT INTO envelope_changes(id, group_id, month, kind, from_envelope_id, to_envelope_id, amount, made_at, reason, undoes)
             VALUES (?1, COALESCE(?2, 0), ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![self.new_id("envelope_changes")?, group, month_key(month), kind, from, to, amount, now_text(), reason, undoes],
        )?;
        let id = self.conn.last_insert_rowid();
        if group.is_none() {
            self.conn.execute("UPDATE envelope_changes SET group_id=?1 WHERE id=?1", [id])?;
        }
        Ok(group.unwrap_or(id))
    }

    fn check_month(&self, month: NaiveDate) -> Result<NaiveDate> {
        let month = first_of_month(month);
        let start = self.envelopes_start();
        if month < start {
            bail!(trf("the envelopes start in {month}", &[("month", &month_key(start))]));
        }
        if months_between(start, month) > i64::from(MAX_MONTHS) {
            bail!(tr("the month is too far from the first month of the envelopes"));
        }
        Ok(month)
    }

    /// Sets what is assigned to an envelope in a month; the difference comes from, or goes back to, *to be assigned*.
    /// None when it was that already.
    pub fn assign(&self, envelope: i64, month: NaiveDate, amount: Cents, reason: &str) -> Result<Option<i64>> {
        self.atomically(|| {
            let month = self.check_month(month)?;
            let reason = clean_text(reason, tr("the reason is too long"))?;
            if !(-MAX_BALANCE..=MAX_BALANCE).contains(&amount) {
                bail!(trf("\"{amount}\" is too large", &[("amount", &fmt_eur(amount))]));
            }
            if !self.envelopes()?.iter().any(|e| e.id == envelope) {
                bail!(trf("envelope \"{name}\" not found", &[("name", &envelope)]));
            }
            let now = self.assigned_in(envelope, month)?;
            let delta = cents(i128::from(amount) - now)?;
            if delta == 0 {
                return Ok(None);
            }
            self.write_change(
                None,
                Row { month, from: None, to: Some(envelope), amount: delta, kind: ASSIGN },
                &reason,
                None,
            )
            .map(Some)
        })
    }

    /// What is assigned to an envelope in a month.
    fn assigned_in(&self, envelope: i64, month: NaiveDate) -> Result<i128> {
        let mut st = self.conn.prepare(
            "SELECT amount, to_envelope_id=?1, from_envelope_id=?1 FROM envelope_changes WHERE month=?2
             AND (to_envelope_id=?1 OR from_envelope_id=?1)",
        )?;
        let mut total = 0i128;
        for row in st.query_map(params![envelope, month_key(month)], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<bool>>(1)?, r.get::<_, Option<bool>>(2)?))
        })? {
            let (a, to, from) = row?;
            if to == Some(true) {
                total += i128::from(a);
            }
            if from == Some(true) {
                total -= i128::from(a);
            }
        }
        Ok(total)
    }

    /// Moves money from one envelope to another in a month, such as to cover what one overspent.
    pub fn move_money(&self, from: i64, to: i64, month: NaiveDate, amount: Cents, reason: &str) -> Result<i64> {
        self.atomically(|| {
            let month = self.check_month(month)?;
            let reason = clean_text(reason, tr("the reason is too long"))?;
            if from == to {
                bail!(tr("money is moved between two different envelopes"));
            }
            if amount <= 0 || amount > MAX_BALANCE {
                bail!(tr("the amount to move must be more than zero"));
            }
            let all = self.envelopes()?;
            for id in [from, to] {
                if !all.iter().any(|e| e.id == id) {
                    bail!(trf("envelope \"{name}\" not found", &[("name", &id)]));
                }
            }
            self.write_change(None, Row { month, from: Some(from), to: Some(to), amount, kind: MOVE }, &reason, None)
        })
    }

    /// Assigns to every envelope with a monthly budget among its categories the sum of those budgets. The changes
    /// are one group, undone together; None when nothing changed.
    pub fn assign_budgets(&self, month: NaiveDate) -> Result<Option<i64>> {
        self.atomically(|| {
            let month = self.check_month(month)?;
            let budgets = self.category_budget_map()?;
            let mut group = None;
            for e in self.envelopes()? {
                let mut target = 0i128;
                let mut any = false;
                for c in &e.category_ids {
                    if let Some(b) = budgets.get(c) {
                        target += i128::from(*b);
                        any = true;
                    }
                }
                if !any {
                    continue;
                }
                let delta = cents(target.clamp(0, i128::from(MAX_BALANCE)) - self.assigned_in(e.id, month)?)?;
                if delta != 0 {
                    let row = Row { month, from: None, to: Some(e.id), amount: delta, kind: ASSIGN };
                    group = Some(self.write_change(group, row, tr("From the monthly budget"), None)?);
                }
            }
            Ok(group)
        })
    }

    fn category_budget_map(&self) -> Result<HashMap<i64, i64>> {
        let mut st = self.conn.prepare("SELECT category_id, monthly FROM category_budgets")?;
        let out = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?;
        Ok(out)
    }

    /// Undoes a group of changes, or without one the last group that is not undone and undoes nothing. Its rows
    /// are written again with the opposite amounts; the id of the new group.
    pub fn undo_envelope_change(&self, group: Option<i64>) -> Result<i64> {
        self.atomically(|| {
            let changes = self.changes(None)?;
            let undone: Vec<i64> = changes.iter().filter_map(|c| c.undoes).collect();
            let target = match group {
                Some(g) => {
                    let Some(c) = changes.iter().find(|c| c.group_id == g || c.id == g) else {
                        bail!(trf("change {id} not found", &[("id", &g)]));
                    };
                    if c.undoes.is_some() {
                        bail!(tr("an undo cannot be undone: make the change again"));
                    }
                    c.group_id
                }
                None => changes
                    .iter()
                    .find(|c| c.undoes.is_none() && !undone.contains(&c.group_id))
                    .map(|c| c.group_id)
                    .ok_or_else(|| anyhow!(tr("there is no change of the envelopes to undo")))?,
            };
            if undone.contains(&target) {
                bail!(tr("that change was undone already"));
            }
            let mut st = self.conn.prepare(
                "SELECT month, from_envelope_id, to_envelope_id, amount, kind='move' FROM envelope_changes WHERE group_id=?1 ORDER BY id",
            )?;
            let rows: Vec<(String, Option<i64>, Option<i64>, i64, bool)> = st
                .query_map([target], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
                .collect::<std::result::Result<_, _>>()?;
            let mut group = None;
            for (month, from, to, amount, moved) in rows {
                let month = parse_month(&month).ok_or_else(|| anyhow!(tr("the month of a change cannot be read")))?;
                let reason = trf("undoes change {id}", &[("id", &target)]);
                let row = Row { month, from, to, amount: amount.saturating_neg(), kind: if moved { MOVE } else { ASSIGN } };
                group = Some(self.write_change(group, row, &reason, Some(target))?);
            }
            group.ok_or_else(|| anyhow!(tr("there is no change of the envelopes to undo")))
        })
    }

    /// The changes of a month, or of every month, last first.
    pub fn changes(&self, month: Option<NaiveDate>) -> Result<Vec<Change>> {
        let names: HashMap<i64, String> = self.envelopes()?.into_iter().map(|e| (e.id, e.name)).collect();
        let mut st = self.conn.prepare(
            "SELECT c.id, c.group_id, c.month, c.from_envelope_id, c.to_envelope_id, c.amount, c.made_at, c.reason, c.undoes, c.kind='move',
                    EXISTS (SELECT 1 FROM envelope_changes u WHERE u.undoes = c.group_id)
             FROM envelope_changes c WHERE ?1 IS NULL OR c.month = ?1 ORDER BY c.id DESC",
        )?;
        let rows = st.query_map([month.map(month_key)], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, Option<i64>>(8)?,
                (r.get::<_, bool>(9)?, r.get::<_, bool>(10)?),
            ))
        })?;
        let mut out = Vec::new();
        let deleted = || tr("deleted envelope").to_string();
        let name = |e: i64| names.get(&e).cloned().unwrap_or_else(deleted);
        for row in rows {
            let (id, group_id, month, from, to, amount, made_at, reason, undoes, (moved, undone)) = row?;
            let kind = match (undoes, moved) {
                (Some(_), _) => ChangeKind::Undo,
                (None, true) => ChangeKind::Move,
                (None, false) => ChangeKind::Assign,
            };
            // Only a move comes from an envelope; every change goes to one.
            let from = match (from, moved) {
                (Some(e), _) => Some(name(e)),
                (None, true) => Some(deleted()),
                (None, false) => None,
            };
            let to = Some(to.map_or_else(deleted, name));
            out.push(Change { id, group_id, month, kind, from, to, amount, made_at, reason, undoes, undone });
        }
        Ok(out)
    }

    /// The envelopes in a month.
    pub fn envelope_month(&self, month: NaiveDate) -> Result<MonthView> {
        let month = first_of_month(month);
        let start = self.envelopes_start();
        let envelopes = self.envelopes()?;
        let cats = self.categories()?;
        let budgets = self.category_budget_map()?;
        let paths: HashMap<i64, &Category> = cats.iter().map(|c| (c.id, c)).collect();
        let line = |e: &Envelope, f: [i128; 4]| -> Result<EnvelopeLine> {
            let mut categories: Vec<String> =
                e.category_ids.iter().filter_map(|c| paths.get(c).map(|c| c.path.clone())).collect();
            categories.sort_by_key(|c| c.to_lowercase());
            let budget = e.category_ids.iter().filter_map(|c| budgets.get(c)).map(|b| i128::from(*b)).sum::<i128>();
            Ok(EnvelopeLine {
                id: e.id,
                name: e.name.clone(),
                overspending: e.overspending,
                categories,
                budget: cents(budget)?,
                carried: cents(f[0])?,
                assigned: cents(f[1])?,
                spent: cents(f[2])?,
                available: cents(f[3])?,
            })
        };
        let this = first_of_month(today());
        let mut view = MonthView {
            mode: self.budget_mode(),
            month: month_key(month),
            start: month_key(start),
            before_start: month < start,
            past: month < this,
            left_over: 0,
            income: 0,
            outside: 0,
            uncategorized: 0,
            overspent_before: 0,
            assigned: 0,
            to_be_assigned: 0,
            envelopes: Vec::new(),
            other_currencies: 0,
            changes: Vec::new(),
        };
        if month < start {
            view.envelopes = envelopes.iter().map(|e| line(e, [0; 4])).collect::<Result<_>>()?;
            return Ok(view);
        }
        let all = self.compute(month)?;
        let c = all.last().ok_or_else(|| anyhow!(tr("the month cannot be computed")))?;
        view.left_over = cents(c.left_over)?;
        view.income = cents(c.income)?;
        view.outside = cents(c.outside)?;
        view.uncategorized = cents(c.uncategorized)?;
        view.overspent_before = cents(c.overspent_before)?;
        view.assigned = cents(c.assigned)?;
        view.to_be_assigned = cents(c.to_be_assigned)?;
        view.other_currencies = c.other_currencies;
        view.envelopes =
            envelopes.iter().map(|e| line(e, c.lines.get(&e.id).copied().unwrap_or([0; 4]))).collect::<Result<_>>()?;
        view.changes = self.changes(Some(month))?;
        Ok(view)
    }

    /// The months after `month`, up to this one, whose figures differ between two computations.
    pub fn months_changed_after(&self, month: NaiveDate, before: &[(NaiveDate, String)]) -> Result<Vec<String>> {
        let after = self.fingerprints(month)?;
        Ok(after
            .iter()
            .filter(|(m, f)| *m > month && before.iter().find(|(b, _)| b == m).is_none_or(|(_, g)| g != f))
            .map(|(m, _)| month_key(*m))
            .collect())
    }

    /// The figures of each month from the first to this one, or to `month` when that is later, in a form that can be
    /// compared: for [`Db::months_changed_after`].
    pub fn fingerprints(&self, month: NaiveDate) -> Result<Vec<(NaiveDate, String)>> {
        let last = first_of_month(today()).max(first_of_month(month));
        if last < self.envelopes_start() || self.check_month(last).is_err() {
            return Ok(Vec::new());
        }
        Ok(self.compute(last)?.into_iter().map(|c| (c.month, format!("{c:?}"))).collect())
    }

    /// Every month from the first of the envelopes to `until`, in order.
    fn compute(&self, until: NaiveDate) -> Result<Vec<Computed>> {
        let start = self.envelopes_start();
        let until = self.check_month(until)?;
        let (_, end) = crate::db::month_range(until);
        let envelopes = self.envelopes()?;
        let parents: HashMap<i64, Option<i64>> = self.categories()?.into_iter().map(|c| (c.id, c.parent_id)).collect();
        let linked: HashMap<i64, i64> =
            envelopes.iter().flat_map(|e| e.category_ids.iter().map(|c| (*c, e.id))).collect();
        // The nearest category, itself or above it, that is in an envelope.
        let envelope_of = |mut c: i64| -> Option<i64> {
            for _ in 0..64 {
                if let Some(e) = linked.get(&c) {
                    return Some(*e);
                }
                c = (*parents.get(&c)?)?;
            }
            None
        };
        // Postings by month: (class is income, category, in euros) -> (sum, count).
        let mut flows: HashMap<String, Vec<Flow>> = HashMap::new();
        let mut st = self.conn.prepare(
            "SELECT substr(e.date,1,7), l.class='income', l.category_id, p.currency='EUR', SUM(p.amount), COUNT(*)
             FROM journal_postings p JOIN entries e ON e.id=p.entry_id JOIN ledger_accounts l ON l.id=p.ledger_account_id
             WHERE l.class IN ('income','expense') AND e.date>=?1 AND e.date<=?2
             GROUP BY 1, 2, 3, 4",
        )?;
        let rows = st.query_map(params![crate::db::d2s(start), crate::db::d2s(end)], |r| {
            Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
        })?;
        for row in rows {
            let (m, f) = row?;
            flows.entry(m).or_default().push(f);
        }
        let mut changes: HashMap<String, Vec<Moved>> = HashMap::new();
        let mut st =
            self.conn.prepare("SELECT month, from_envelope_id, to_envelope_id, amount FROM envelope_changes")?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))? {
            let (m, c) = row?;
            changes.entry(m).or_default().push(c);
        }
        let mut out: Vec<Computed> = Vec::new();
        let mut month = start;
        loop {
            let key = month_key(month);
            let mut c = Computed {
                month,
                left_over: 0,
                income: 0,
                outside: 0,
                uncategorized: 0,
                overspent_before: 0,
                assigned: 0,
                to_be_assigned: 0,
                other_currencies: 0,
                lines: envelopes.iter().map(|e| (e.id, [0; 4])).collect(),
            };
            if let Some(prev) = out.last() {
                c.left_over = prev.to_be_assigned;
                for e in &envelopes {
                    let before = prev.lines.get(&e.id).map_or(0, |l| l[3]);
                    let carried = if before < 0 && e.overspending == Overspending::NextMonth {
                        c.overspent_before -= before;
                        0
                    } else {
                        before
                    };
                    if let Some(l) = c.lines.get_mut(&e.id) {
                        l[0] = carried;
                    }
                }
            }
            for (income, category, euros, sum, count) in flows.get(&key).map(Vec::as_slice).unwrap_or_default() {
                if !euros {
                    c.other_currencies += count;
                    continue;
                }
                let sum = i128::from(*sum);
                match (income, category.and_then(envelope_of)) {
                    (true, _) => c.income -= sum,
                    (false, Some(e)) => {
                        if let Some(l) = c.lines.get_mut(&e) {
                            l[2] += sum;
                        }
                    }
                    (false, None) if category.is_some() => c.outside += sum,
                    (false, None) => c.uncategorized += sum,
                }
            }
            for (from, to, amount) in changes.get(&key).map(Vec::as_slice).unwrap_or_default() {
                for (e, sign) in [(from, -1), (to, 1)] {
                    if let Some(l) = e.and_then(|e| c.lines.get_mut(&e)) {
                        l[1] += sign * i128::from(*amount);
                    }
                }
            }
            for l in c.lines.values_mut() {
                l[3] = l[0] + l[1] - l[2];
                c.assigned += l[1];
            }
            c.to_be_assigned = c.left_over + c.income - c.outside - c.uncategorized - c.overspent_before - c.assigned;
            out.push(c);
            if month >= until {
                break;
            }
            month = month.checked_add_months(Months::new(1)).ok_or_else(too_big)?;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
