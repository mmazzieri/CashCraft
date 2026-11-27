//! Zero-based budgeting with envelopes, beside the monthly budgets of the categories. Every euro that arrives waits in
//! *to be assigned* until it is given to an envelope; an envelope has what was assigned to it, what its categories
//! spent and what is left, which goes on to the next month. Nothing of a month is stored but the changes the user made:
//! every figure is computed again from the ledger, from the first month of the envelopes on, so a change to the past
//! is seen in every month after it. `docs/envelopes.md` has the rules.

use crate::db::{Category, Cents, Db, first_of_month, today};
use crate::i18n::{tr, trf};
use crate::ledger::{MAX_BALANCE, too_big};
use crate::money::fmt_eur;
use anyhow::{Result, anyhow, bail};
use chrono::{Datelike, Months, NaiveDate};
use rusqlite::params;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

/// The tables of the envelopes (format 10). An envelope is never deleted, so its id is never given again: deleting it
/// is a change, which can be undone. The changes are only ever added: an undo is a change that names the group it
/// undoes, so the history stays as it was made. Which envelope a category is in is a change too, from a month on.
pub(crate) const ENVELOPE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS envelopes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    overspending TEXT NOT NULL DEFAULT 'carry' CHECK (overspending IN ('carry','next_month'))
);
CREATE TABLE IF NOT EXISTS envelope_changes (
    id INTEGER PRIMARY KEY,
    group_id INTEGER NOT NULL,
    month TEXT NOT NULL CHECK (length(month) = 7),
    kind TEXT NOT NULL CHECK (kind IN ('assign','move','link','delete','undo')),
    from_envelope_id INTEGER REFERENCES envelopes(id),
    to_envelope_id INTEGER REFERENCES envelopes(id),
    category_id INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    amount INTEGER NOT NULL DEFAULT 0 CHECK (typeof(amount) = 'integer'),
    made_at TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    undoes INTEGER
);
CREATE INDEX IF NOT EXISTS envelope_change_month ON envelope_changes(month);
CREATE INDEX IF NOT EXISTS envelope_change_group ON envelope_changes(group_id);
CREATE INDEX IF NOT EXISTS envelope_change_undoes ON envelope_changes(undoes);
CREATE INDEX IF NOT EXISTS envelope_change_to ON envelope_changes(to_envelope_id, month);
CREATE INDEX IF NOT EXISTS envelope_change_from ON envelope_changes(from_envelope_id, month);
CREATE INDEX IF NOT EXISTS envelope_change_category ON envelope_changes(category_id);
";

/// How many months from this one the first month can be, and after the first one a month can be: a hundred years.
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

/// An envelope that is not deleted, with its categories in this month.
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
    /// Its categories in this month.
    pub categories: Vec<String>,
    pub category_ids: Vec<i64>,
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

/// One change: an amount given to an envelope (from *to be assigned*, or back to it when negative), moved from one
/// to another, a category put in an envelope or taken out from a month on, an envelope deleted, or the undoing of
/// another change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Change {
    pub id: i64,
    /// The changes made together, undone together.
    pub group_id: i64,
    pub month: String,
    pub kind: ChangeKind,
    /// None: *to be assigned*.
    pub from: Option<String>,
    /// None: *to be assigned*, or no envelope for a category taken out.
    pub to: Option<String>,
    /// The category of a link.
    pub category: Option<String>,
    pub amount: Cents,
    pub made_at: String,
    pub reason: String,
    /// The group this one undoes.
    pub undoes: Option<i64>,
    /// Undone by a later change.
    pub undone: bool,
    /// What it did, in words.
    pub text: String,
}

impl Change {
    fn describe(&self) -> String {
        let none = || tr("To be assigned").to_string();
        match self.kind {
            ChangeKind::Link => match &self.to {
                Some(name) => trf(
                    "{category} in {name}",
                    &[("category", &self.category.clone().unwrap_or_default()), ("name", name)],
                ),
                None => {
                    trf("{category} out of the envelopes", &[("category", &self.category.clone().unwrap_or_default())])
                }
            },
            ChangeKind::Delete => trf("{name} deleted", &[("name", &self.to.clone().unwrap_or_default())]),
            ChangeKind::Undo => tr("Undo").to_string(),
            ChangeKind::Assign | ChangeKind::Move => format!(
                "{} → {} {}",
                self.from.clone().unwrap_or_else(none),
                self.to.clone().unwrap_or_else(none),
                fmt_eur(self.amount)
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Assign,
    Move,
    Link,
    Delete,
    Undo,
}

impl ChangeKind {
    fn from_code(s: &str) -> ChangeKind {
        match s {
            "move" => ChangeKind::Move,
            "link" => ChangeKind::Link,
            "delete" => ChangeKind::Delete,
            "undo" => ChangeKind::Undo,
            _ => ChangeKind::Assign,
        }
    }
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

/// The figures of an envelope in a month.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Figures {
    carried: i128,
    assigned: i128,
    spent: i128,
    available: i128,
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
    lines: BTreeMap<i64, Figures>,
    /// Which envelope each linked category is in.
    links: BTreeMap<i64, i64>,
}

/// The figures of every month, to tell which a change moved.
pub type Fingerprints = Vec<(NaiveDate, String)>;

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

fn not_found(key: &dyn std::fmt::Display) -> anyhow::Error {
    anyhow!(trf("envelope \"{name}\" not found", &[("name", key)]))
}

/// The postings of a month on a category, or on *Uncategorized*: whether it is income, the category, whether in euros,
/// their sum and how many.
type Flow = (bool, Option<i64>, bool, i64, i64);

/// Money of a change: from, to, amount.
type Moved = (Option<i64>, Option<i64>, i64);

/// One row of a change.
#[derive(Clone, Copy)]
struct Row {
    month: NaiveDate,
    kind: &'static str,
    from: Option<i64>,
    to: Option<i64>,
    category: Option<i64>,
    amount: Cents,
}

impl Row {
    fn money(month: NaiveDate, from: Option<i64>, to: Option<i64>, amount: Cents) -> Row {
        let kind = if from.is_some() { "move" } else { "assign" };
        Row { month, kind, from, to, category: None, amount }
    }

    fn link(month: NaiveDate, category: i64, to: Option<i64>) -> Row {
        Row { month, kind: "link", from: None, to, category: Some(category), amount: 0 }
    }
}

/// What the changes say, read once: which envelopes are deleted, which category is in which envelope from which month,
/// and the money given and moved in each month. Undone groups are left out.
#[derive(Default)]
struct State {
    /// Every envelope, deleted or not: id, name, overspending.
    all: Vec<(i64, String, Overspending)>,
    deleted: HashSet<i64>,
    /// Links by month and order: month, category, envelope (None: taken out).
    links: Vec<(NaiveDate, i64, Option<i64>)>,
    /// By month: from, to, amount.
    money: HashMap<NaiveDate, Vec<Moved>>,
    /// The earliest month with money.
    first_money: Option<NaiveDate>,
    last_month: Option<NaiveDate>,
}

impl State {
    fn live(&self) -> impl Iterator<Item = &(i64, String, Overspending)> {
        self.all.iter().filter(|e| !self.deleted.contains(&e.0))
    }

    /// Which envelope each linked category is in, in a month; deleted envelopes are none.
    fn links_at(&self, month: NaiveDate) -> BTreeMap<i64, i64> {
        let mut out: BTreeMap<i64, Option<i64>> = BTreeMap::new();
        for (_, c, e) in self.links.iter().take_while(|l| l.0 <= month) {
            out.insert(*c, e.filter(|e| !self.deleted.contains(e)));
        }
        out.into_iter().filter_map(|(c, e)| e.map(|e| (c, e))).collect()
    }
}

/// The first months that may be chosen: up to [`MAX_MONTHS`] before or after this one.
fn start_range(this: NaiveDate) -> (NaiveDate, NaiveDate) {
    let span = Months::new(MAX_MONTHS);
    (this.checked_sub_months(span).unwrap_or(this), this.checked_add_months(span).unwrap_or(this))
}

fn months_between(from: NaiveDate, to: NaiveDate) -> i64 {
    i64::from(to.year() - from.year()) * 12 + i64::from(to.month()) - i64::from(from.month())
}

impl Db {
    pub fn budget_mode(&self) -> Mode {
        self.meta("budget_mode").and_then(|m| Mode::from_code(&m)).unwrap_or(Mode::Monthly)
    }

    /// The first month chosen, if any.
    fn chosen_start(&self) -> Option<NaiveDate> {
        let (oldest, latest) = start_range(first_of_month(today()));
        self.meta("envelopes_from").and_then(|m| parse_month(&m)).map(|m| m.clamp(oldest, latest))
    }

    /// The first month of the envelopes. It is fixed by the first change of the envelopes, whoever makes it, and
    /// moves only when the user moves it; until then it is this month. One further than [`MAX_MONTHS`] from this
    /// month, which only a file changed by hand or a clock gone wrong could have, is taken as the furthest that is not,
    /// so this month can always be shown.
    pub fn envelopes_start(&self) -> NaiveDate {
        self.chosen_start().unwrap_or_else(|| first_of_month(today()))
    }

    /// Fixes the first month, if nothing did yet, at `month` or this one, whichever comes first.
    fn fix_start(&self, month: NaiveDate) -> Result<()> {
        if self.meta("envelopes_from").and_then(|m| parse_month(&m)).is_none() {
            let m = first_of_month(month).min(first_of_month(today()));
            self.set_meta("envelopes_from", &month_key(m))?;
        }
        Ok(())
    }

    /// Chooses what the budget screen shows. When envelopes are chosen and there are none, and no change was ever
    /// made, each category with a monthly budget gets one, with that budget assigned in the first month (this one,
    /// unless another was chosen): the number of envelopes made.
    pub fn set_budget_mode(&self, mode: Mode) -> Result<usize> {
        self.atomically(|| {
            let mut made = 0;
            if mode == Mode::Envelopes {
                self.fix_start(first_of_month(today()))?;
                let changed: bool =
                    self.conn.query_row("SELECT EXISTS (SELECT 1 FROM envelope_changes)", [], |r| r.get(0))?;
                let any: bool = self.conn.query_row("SELECT EXISTS (SELECT 1 FROM envelopes)", [], |r| r.get(0))?;
                if !any && !changed {
                    made = self.envelopes_from_budgets(self.envelopes_start())?;
                }
            }
            self.set_meta("budget_mode", mode.code())?;
            Ok(made)
        })
    }

    /// Changes the first month: every month from there is computed again. A later month would leave out money given
    /// or moved before it: refused, unless `confirm`.
    pub fn set_envelopes_start(&self, month: NaiveDate, confirm: bool) -> Result<()> {
        let month = first_of_month(month);
        let (oldest, latest) = start_range(first_of_month(today()));
        if month < oldest || month > latest {
            bail!(tr("the first month of the envelopes is out of range"));
        }
        self.atomically(|| {
            let state = self.state()?;
            let dropped: usize = state.money.iter().filter(|(m, _)| **m < month).map(|(_, r)| r.len()).sum();
            if dropped > 0 && !confirm {
                bail!(trf(
                    "{n} changes of the envelopes come before {month} and would no longer count: confirm to start there",
                    &[("n", &dropped), ("month", &month_key(month))]
                ));
            }
            self.set_meta("envelopes_from", &month_key(month))
        })
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
            // Named after the category, or after its path when another envelope has that name.
            let taken = |n: &str| -> Result<bool> {
                Ok(self.envelopes()?.iter().any(|e| e.name.to_lowercase() == n.to_lowercase()))
            };
            let name = if taken(&c.name)? { c.path.clone() } else { c.name.clone() };
            if taken(&name)? {
                continue;
            }
            let id = self.add_envelope(&name, &[cid], Overspending::Carry, month)?;
            made += 1;
            let amount = monthly.clamp(0, MAX_BALANCE);
            if amount != 0 {
                let row = Row::money(month, None, Some(id), amount);
                group = Some(self.write_change(group, row, tr("From the monthly budget"), None)?);
            }
        }
        Ok(made)
    }

    /// Reads the changes, once.
    fn state(&self) -> Result<State> {
        let mut s = State::default();
        let mut st = self.conn.prepare("SELECT id, name, overspending FROM envelopes ORDER BY id")?;
        for row in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))? {
            let (id, name, o) = row?;
            s.all.push((id, name, Overspending::from_code(&o).unwrap_or(Overspending::Carry)));
        }
        let mut st = self.conn.prepare("SELECT undoes FROM envelope_changes WHERE undoes IS NOT NULL")?;
        let undone: HashSet<i64> = st.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
        let mut st = self.conn.prepare(
            "SELECT group_id, month, kind, from_envelope_id, to_envelope_id, category_id, amount FROM envelope_changes
             WHERE kind <> 'undo' ORDER BY month, id",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<i64>>(4)?,
                (r.get::<_, Option<i64>>(5)?, r.get::<_, i64>(6)?),
            ))
        })?;
        for row in rows {
            let (group, month, kind, from, to, (category, amount)) = row?;
            if undone.contains(&group) {
                continue;
            }
            let Some(month) = parse_month(&month) else { continue };
            s.last_month = s.last_month.max(Some(month));
            match kind.as_str() {
                "link" => {
                    if let Some(c) = category {
                        s.links.push((month, c, to));
                    }
                }
                "delete" => {
                    if let Some(e) = to {
                        s.deleted.insert(e);
                    }
                }
                _ => {
                    s.first_money = Some(s.first_money.map_or(month, |f| f.min(month)));
                    s.money.entry(month).or_default().push((from, to, amount));
                }
            }
        }
        Ok(s)
    }

    /// The envelopes that are not deleted, with their categories in this month.
    pub fn envelopes(&self) -> Result<Vec<Envelope>> {
        let state = self.state()?;
        let links = state.links_at(first_of_month(today()).max(self.envelopes_start()));
        let mut out: Vec<Envelope> = state
            .live()
            .map(|(id, name, o)| Envelope {
                id: *id,
                name: name.clone(),
                overspending: *o,
                category_ids: links.iter().filter(|(_, e)| *e == id).map(|(c, _)| *c).collect(),
            })
            .collect();
        out.sort_by_key(|e| (e.name.to_lowercase(), e.id));
        Ok(out)
    }

    /// An envelope that is not deleted, by its id or by its whole name, whatever the case. A text that is both is
    /// refused.
    pub fn find_envelope(&self, key: &str) -> Result<Envelope> {
        let key = key.trim();
        let all = self.envelopes()?;
        let by_id = key.parse::<i64>().ok().and_then(|id| all.iter().find(|e| e.id == id));
        let by_name = all.iter().find(|e| e.name.to_lowercase() == key.to_lowercase());
        match (by_id, by_name) {
            (Some(a), Some(b)) if a.id != b.id => {
                bail!(trf(
                    "\"{name}\" is the id of one envelope and the name of another: use the id",
                    &[("name", &key)]
                ))
            }
            (Some(e), _) | (None, Some(e)) => Ok(e.clone()),
            (None, None) => Err(not_found(&key)),
        }
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

    /// Puts the categories, and those alone, in an envelope from `month` on: the months before keep what they had.
    /// Income categories are refused, and so is a category of another envelope in that month. None when nothing
    /// changed, else the group of the change.
    fn link_categories(&self, envelope: i64, ids: &[i64], month: NaiveDate) -> Result<Option<i64>> {
        let month = self.check_month(month)?;
        let cats = self.categories()?;
        if ids.len() > 1000 {
            bail!(tr("too many categories"));
        }
        let state = self.state()?;
        let now = state.links_at(month);
        let names: HashMap<i64, &str> = state.live().map(|e| (e.0, e.1.as_str())).collect();
        let mut wanted: Vec<i64> = Vec::new();
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
            if let Some(other) = now.get(id).filter(|e| **e != envelope) {
                bail!(trf(
                    "\"{category}\" is already in the envelope \"{name}\"",
                    &[("category", &c.path), ("name", &names.get(other).copied().unwrap_or(""))]
                ));
            }
            if !wanted.contains(id) {
                wanted.push(*id);
            }
        }
        let mut group = None;
        let reason = trf("from {month}", &[("month", &month_key(month))]);
        for c in now.iter().filter(|(c, e)| **e == envelope && !wanted.contains(c)).map(|(c, _)| *c) {
            group = Some(self.write_change(group, Row::link(month, c, None), &reason, None)?);
        }
        for c in wanted.into_iter().filter(|c| now.get(c) != Some(&envelope)) {
            group = Some(self.write_change(group, Row::link(month, c, Some(envelope)), &reason, None)?);
        }
        Ok(group)
    }

    /// A new envelope, with its categories from `month` on.
    pub fn add_envelope(
        &self,
        name: &str,
        categories: &[i64],
        overspending: Overspending,
        month: NaiveDate,
    ) -> Result<i64> {
        self.atomically(|| {
            let name = self.check_name(name, None)?;
            if self.envelopes()?.len() >= MAX_ENVELOPES {
                bail!(tr("there are too many envelopes"));
            }
            self.fix_start(month)?;
            self.conn.execute(
                "INSERT INTO envelopes(id, name, overspending) VALUES (?1, ?2, ?3)",
                params![self.new_id("envelopes")?, name, overspending.code()],
            )?;
            let id = self.conn.last_insert_rowid();
            self.link_categories(id, categories, month)?;
            Ok(id)
        })
    }

    /// Changes what is given; `None` leaves it as it is. The categories are those of the envelope from their month on.
    pub fn edit_envelope(
        &self,
        id: i64,
        name: Option<&str>,
        categories: Option<(&[i64], NaiveDate)>,
        overspending: Option<Overspending>,
    ) -> Result<()> {
        self.atomically(|| {
            if !self.envelopes()?.iter().any(|e| e.id == id) {
                return Err(not_found(&id));
            }
            if let Some(n) = name {
                let n = self.check_name(n, Some(id))?;
                self.conn.execute("UPDATE envelopes SET name=?2 WHERE id=?1", params![id, n])?;
            }
            if let Some((c, month)) = categories {
                self.link_categories(id, c, month)?;
            }
            if let Some(o) = overspending {
                self.conn.execute("UPDATE envelopes SET overspending=?2 WHERE id=?1", params![id, o.code()])?;
            }
            Ok(())
        })
    }

    /// Deletes an envelope, as a change that can be undone. Every month is then as if it had never been: what was
    /// assigned to it is to be assigned again, and the spending of its categories, which are in no envelope any more,
    /// is taken from there. The change is kept in `month`, where it is listed. The group of the change.
    pub fn delete_envelope(&self, id: i64, month: NaiveDate) -> Result<i64> {
        self.atomically(|| {
            if !self.envelopes()?.iter().any(|e| e.id == id) {
                return Err(not_found(&id));
            }
            let month = self.check_month(month)?;
            let row = Row { month, kind: "delete", from: None, to: Some(id), category: None, amount: 0 };
            self.write_change(None, row, "", None)
        })
    }

    /// Adds one row of a change; the first row of a group gives the group its id.
    fn write_change(&self, group: Option<i64>, row: Row, reason: &str, undoes: Option<i64>) -> Result<i64> {
        let Row { month, kind, from, to, category, amount } = row;
        // An assignment goes from one amount allowed to another: at most twice as far.
        let far = MAX_BALANCE.saturating_mul(2);
        if !(-far..=far).contains(&amount) {
            bail!(trf("\"{amount}\" is too large", &[("amount", &fmt_eur(amount))]));
        }
        self.fix_start(month)?;
        self.conn.execute(
            "INSERT INTO envelope_changes(id, group_id, month, kind, from_envelope_id, to_envelope_id, category_id, amount,
                                          made_at, reason, undoes)
             VALUES (?1, COALESCE(?2, 0), ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                self.new_id("envelope_changes")?,
                group,
                month_key(month),
                kind,
                from,
                to,
                category,
                amount,
                now_text(),
                reason,
                undoes
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        if group.is_none() {
            self.conn.execute("UPDATE envelope_changes SET group_id=?1 WHERE id=?1", [id])?;
        }
        Ok(group.unwrap_or(id))
    }

    /// A month in which a change can be made: not before the first month, nor too far from it. Before the first
    /// change, the first month is not fixed yet and that change fixes it.
    fn check_month(&self, month: NaiveDate) -> Result<NaiveDate> {
        let month = first_of_month(month);
        match self.chosen_start() {
            Some(start) if month < start => {
                bail!(trf("the envelopes start in {month}", &[("month", &month_key(start))]))
            }
            Some(start) if months_between(start, month) > i64::from(MAX_MONTHS) => {
                bail!(tr("the month is too far from the first month of the envelopes"))
            }
            None => {
                let (oldest, latest) = start_range(first_of_month(today()));
                if month < oldest || month > latest {
                    bail!(tr("the month is too far from the first month of the envelopes"));
                }
            }
            _ => {}
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
                return Err(not_found(&envelope));
            }
            let delta = cents(i128::from(amount) - self.assigned_in(envelope, month)?)?;
            if delta == 0 {
                return Ok(None);
            }
            self.write_change(None, Row::money(month, None, Some(envelope), delta), &reason, None).map(Some)
        })
    }

    /// What is assigned to an envelope in a month, changes undone left out.
    fn assigned_in(&self, envelope: i64, month: NaiveDate) -> Result<i128> {
        let mut st = self.conn.prepare_cached(
            "SELECT amount, to_envelope_id IS ?1 FROM envelope_changes c
             WHERE month=?2 AND kind IN ('assign','move') AND (to_envelope_id=?1 OR from_envelope_id=?1)
               AND NOT EXISTS (SELECT 1 FROM envelope_changes u WHERE u.undoes = c.group_id)",
        )?;
        let mut total = 0i128;
        for row in
            st.query_map(params![envelope, month_key(month)], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, bool>(1)?)))?
        {
            let (a, to) = row?;
            total += if to { i128::from(a) } else { -i128::from(a) };
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
                    return Err(not_found(&id));
                }
            }
            // What each is assigned stays within what one change may give.
            let limit = i128::from(MAX_BALANCE);
            if self.assigned_in(from, month)? - i128::from(amount) < -limit
                || self.assigned_in(to, month)? + i128::from(amount) > limit
            {
                bail!(trf("\"{amount}\" is too large", &[("amount", &fmt_eur(amount))]));
            }
            self.write_change(None, Row::money(month, Some(from), Some(to), amount), &reason, None)
        })
    }

    /// Assigns to every envelope with a monthly budget among its categories the sum of those budgets. The changes
    /// are one group, undone together; None when nothing changed.
    pub fn assign_budgets(&self, month: NaiveDate) -> Result<Option<i64>> {
        self.atomically(|| {
            let month = self.check_month(month)?;
            let budgets = self.category_budget_map()?;
            let state = self.state()?;
            let links = state.links_at(month);
            let mut group = None;
            for (id, _, _) in state.live() {
                let mut target = 0i128;
                let mut any = false;
                for (c, _) in links.iter().filter(|(_, e)| *e == id) {
                    if let Some(b) = budgets.get(c) {
                        target += i128::from(*b);
                        any = true;
                    }
                }
                if !any {
                    continue;
                }
                let delta = cents(target.clamp(0, i128::from(MAX_BALANCE)) - self.assigned_in(*id, month)?)?;
                if delta != 0 {
                    let row = Row::money(month, None, Some(*id), delta);
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

    /// The last group of a month, or of any month, that can be undone: not an undo, and not undone.
    pub fn last_undoable(&self, month: Option<NaiveDate>) -> Result<Option<i64>> {
        let mut st = self.conn.prepare_cached(
            "SELECT c.group_id FROM envelope_changes c
             WHERE c.kind <> 'undo' AND (?1 IS NULL OR c.month = ?1)
               AND NOT EXISTS (SELECT 1 FROM envelope_changes u WHERE u.undoes = c.group_id)
             ORDER BY c.id DESC LIMIT 1",
        )?;
        let mut rows = st.query([month.map(month_key)])?;
        Ok(match rows.next()? {
            Some(r) => Some(r.get(0)?),
            None => None,
        })
    }

    /// The earliest month of a group of changes.
    pub fn group_month(&self, group: i64) -> Result<Option<NaiveDate>> {
        let m: Option<String> = self.conn.query_row(
            "SELECT MIN(month) FROM envelope_changes WHERE group_id=?1 OR id=?1",
            [group],
            |r| r.get(0),
        )?;
        Ok(m.and_then(|m| parse_month(&m)))
    }

    /// Undoes a group of changes, or without one the last that can be undone. An envelope whose deletion is undone
    /// comes back, unless another one has its name. The id of the undo.
    pub fn undo_envelope_change(&self, group: Option<i64>) -> Result<i64> {
        self.atomically(|| {
            let target = match group {
                Some(g) => {
                    let found: Option<(i64, String)> = {
                        let mut st = self.conn.prepare(
                            "SELECT group_id, kind FROM envelope_changes WHERE group_id=?1 OR id=?1 LIMIT 1",
                        )?;
                        let mut rows = st.query([g])?;
                        match rows.next()? {
                            Some(r) => Some((r.get(0)?, r.get(1)?)),
                            None => None,
                        }
                    };
                    let Some((g, kind)) = found else {
                        bail!(trf("change {id} not found", &[("id", &g)]));
                    };
                    if kind == "undo" {
                        bail!(tr("an undo cannot be undone: make the change again"));
                    }
                    g
                }
                None => self
                    .last_undoable(None)?
                    .ok_or_else(|| anyhow!(tr("there is no change of the envelopes to undo")))?,
            };
            let undone: bool = self.conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM envelope_changes WHERE undoes=?1)",
                [target],
                |r| r.get(0),
            )?;
            if undone {
                bail!(tr("that change was undone already"));
            }
            // A deleted envelope that comes back needs its name.
            let mut st = self.conn.prepare(
                "SELECT e.id, e.name FROM envelope_changes c JOIN envelopes e ON e.id=c.to_envelope_id
                          WHERE c.group_id=?1 AND c.kind='delete'",
            )?;
            let back: Vec<(i64, String)> =
                st.query_map([target], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?;
            for (id, name) in back {
                self.check_name(&name, Some(id))?;
            }
            let month = self.group_month(target)?.unwrap_or_else(|| first_of_month(today()));
            let row = Row { month, kind: "undo", from: None, to: None, category: None, amount: 0 };
            let reason = trf("undoes change {id}", &[("id", &target)]);
            self.write_change(None, row, &reason, Some(target))
        })
    }

    /// The changes of a month, or of every month, last first.
    pub fn changes(&self, month: Option<NaiveDate>) -> Result<Vec<Change>> {
        let names: HashMap<i64, String> = {
            let mut st = self.conn.prepare("SELECT id, name FROM envelopes")?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?
        };
        let cats: HashMap<i64, String> = self.categories()?.into_iter().map(|c| (c.id, c.path)).collect();
        let mut st = self.conn.prepare(
            "SELECT c.id, c.group_id, c.month, c.kind, c.from_envelope_id, c.to_envelope_id, c.category_id, c.amount,
                    c.made_at, c.reason, c.undoes, EXISTS (SELECT 1 FROM envelope_changes u WHERE u.undoes = c.group_id)
             FROM envelope_changes c WHERE ?1 IS NULL OR c.month = ?1 ORDER BY c.id DESC",
        )?;
        let rows = st.query_map([month.map(month_key)], |r| {
            Ok((
                (r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?),
                (r.get::<_, Option<i64>>(4)?, r.get::<_, Option<i64>>(5)?, r.get::<_, Option<i64>>(6)?),
                (r.get::<_, i64>(7)?, r.get::<_, String>(8)?, r.get::<_, String>(9)?),
                (r.get::<_, Option<i64>>(10)?, r.get::<_, bool>(11)?),
            ))
        })?;
        let name = |e: Option<i64>| e.map(|e| names.get(&e).cloned().unwrap_or_default());
        let mut out = Vec::new();
        for row in rows {
            let ((id, group_id, month, kind), (from, to, category), (amount, made_at, reason), (undoes, undone)) = row?;
            let mut c = Change {
                id,
                group_id,
                month,
                kind: ChangeKind::from_code(&kind),
                from: name(from),
                to: name(to),
                category: category.map(|c| cats.get(&c).cloned().unwrap_or_default()),
                amount,
                made_at,
                reason,
                undoes,
                undone,
                text: String::new(),
            };
            c.text = c.describe();
            out.push(c);
        }
        Ok(out)
    }

    /// The envelopes in a month.
    pub fn envelope_month(&self, month: NaiveDate) -> Result<MonthView> {
        let month = first_of_month(month);
        let start = self.envelopes_start();
        let state = self.state()?;
        let cats = self.categories()?;
        let budgets = self.category_budget_map()?;
        let paths: HashMap<i64, &Category> = cats.iter().map(|c| (c.id, c)).collect();
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
        let (lines, links) = if month < start {
            (BTreeMap::new(), state.links_at(month))
        } else {
            let all = self.compute(&state, month)?;
            let c = all.last().ok_or_else(|| anyhow!(tr("the month cannot be computed")))?;
            view.left_over = cents(c.left_over)?;
            view.income = cents(c.income)?;
            view.outside = cents(c.outside)?;
            view.uncategorized = cents(c.uncategorized)?;
            view.overspent_before = cents(c.overspent_before)?;
            view.assigned = cents(c.assigned)?;
            view.to_be_assigned = cents(c.to_be_assigned)?;
            view.other_currencies = c.other_currencies;
            view.changes = self.changes(Some(month))?;
            (c.lines.clone(), c.links.clone())
        };
        for (id, name, o) in state.live() {
            let f = lines.get(id).copied().unwrap_or_default();
            let category_ids: Vec<i64> = links.iter().filter(|(_, e)| *e == id).map(|(c, _)| *c).collect();
            let mut categories: Vec<String> =
                category_ids.iter().filter_map(|c| paths.get(c).map(|c| c.path.clone())).collect();
            categories.sort_by_key(|c| c.to_lowercase());
            let budget = category_ids.iter().filter_map(|c| budgets.get(c)).map(|b| i128::from(*b)).sum::<i128>();
            view.envelopes.push(EnvelopeLine {
                id: *id,
                name: name.clone(),
                overspending: *o,
                categories,
                category_ids,
                budget: cents(budget)?,
                carried: cents(f.carried)?,
                assigned: cents(f.assigned)?,
                spent: cents(f.spent)?,
                available: cents(f.available)?,
            });
        }
        view.envelopes.sort_by_key(|e| (e.name.to_lowercase(), e.id));
        Ok(view)
    }

    /// The months whose figures differ from those of before, in order.
    pub fn months_changed(&self, before: &Fingerprints) -> Result<Vec<String>> {
        let after = self.fingerprints()?;
        let old: HashMap<&NaiveDate, &String> = before.iter().map(|(m, f)| (m, f)).collect();
        let new: HashMap<&NaiveDate, &String> = after.iter().map(|(m, f)| (m, f)).collect();
        let mut months: Vec<NaiveDate> = old.keys().chain(new.keys()).map(|m| **m).collect();
        months.sort();
        months.dedup();
        Ok(months.into_iter().filter(|m| old.get(m) != new.get(m)).map(month_key).collect())
    }

    /// The figures of each month from the first to this one, or to the last with a change when that is later, in a
    /// form that can be compared: for [`Db::months_changed`].
    pub fn fingerprints(&self) -> Result<Fingerprints> {
        let state = self.state()?;
        let start = self.envelopes_start();
        let last = first_of_month(today()).max(state.last_month.unwrap_or(start));
        if last < start || self.check_month(last).is_err() {
            return Ok(Vec::new());
        }
        let mut names: Vec<(i64, String)> = state.live().map(|e| (e.0, e.1.clone())).collect();
        names.sort();
        Ok(self.compute(&state, last)?.into_iter().map(|c| (c.month, format!("{c:?}{names:?}"))).collect())
    }

    /// Every month from the first of the envelopes to `until`, in order.
    fn compute(&self, state: &State, until: NaiveDate) -> Result<Vec<Computed>> {
        let start = self.envelopes_start();
        let until = first_of_month(until);
        if months_between(start, until) > i64::from(MAX_MONTHS) {
            bail!(tr("the month is too far from the first month of the envelopes"));
        }
        let (_, end) = crate::db::month_range(until);
        let live: Vec<(i64, Overspending)> = state.live().map(|e| (e.0, e.2)).collect();
        let parents: HashMap<i64, Option<i64>> = self.categories()?.into_iter().map(|c| (c.id, c.parent_id)).collect();
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
        let mut out: Vec<Computed> = Vec::new();
        let mut month = start;
        loop {
            let links = state.links_at(month);
            // The nearest category, itself or above it, that is in an envelope.
            let envelope_of = |mut c: i64| -> Option<i64> {
                for _ in 0..64 {
                    if let Some(e) = links.get(&c) {
                        return Some(*e);
                    }
                    c = (*parents.get(&c)?)?;
                }
                None
            };
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
                lines: live.iter().map(|e| (e.0, Figures::default())).collect(),
                links: links.clone(),
            };
            if let Some(prev) = out.last() {
                c.left_over = prev.to_be_assigned;
                for (id, o) in &live {
                    let before = prev.lines.get(id).map_or(0, |l| l.available);
                    let carried = if before < 0 && *o == Overspending::NextMonth {
                        c.overspent_before -= before;
                        0
                    } else {
                        before
                    };
                    if let Some(l) = c.lines.get_mut(id) {
                        l.carried = carried;
                    }
                }
            }
            for (income, category, euros, sum, count) in
                flows.get(&month_key(month)).map(Vec::as_slice).unwrap_or_default()
            {
                if !euros {
                    c.other_currencies += count;
                    continue;
                }
                let sum = i128::from(*sum);
                match (income, category.and_then(envelope_of)) {
                    (true, _) => c.income -= sum,
                    (false, Some(e)) => {
                        if let Some(l) = c.lines.get_mut(&e) {
                            l.spent += sum;
                        }
                    }
                    (false, None) if category.is_some() => c.outside += sum,
                    (false, None) => c.uncategorized += sum,
                }
            }
            for (from, to, amount) in state.money.get(&month).map(Vec::as_slice).unwrap_or_default() {
                for (e, sign) in [(from, -1), (to, 1)] {
                    if let Some(l) = e.and_then(|e| c.lines.get_mut(&e)) {
                        l.assigned += sign * i128::from(*amount);
                    }
                }
            }
            for l in c.lines.values_mut() {
                l.available = l.carried + l.assigned - l.spent;
                c.assigned += l.assigned;
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

    /// The changes in force, as people know them, for comparing two files: `month what`, in order.
    pub fn live_changes(&self) -> Result<Vec<String>> {
        let mut out: Vec<String> = self
            .changes(None)?
            .into_iter()
            .filter(|c| !c.undone && c.kind != ChangeKind::Undo)
            .map(|c| format!("{} {}", c.month, c.text))
            .collect();
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
