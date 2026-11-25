//! Rules that categorize, tag and clean up transactions: what they match, in which order, and the undo of the
//! last run. `docs/rules.md` has the semantics.

use crate::db::{Cents, Db, Transaction, TxFilter, TxKind, no_transaction, s2d};
use crate::i18n::{tr, trf, trn};
use crate::importer::fnv;
use crate::ledger::{Draft, Line, UNCATEGORIZED};
use crate::money::{MAX_AMOUNT, fmt_date, fmt_eur};
use anyhow::{Context, Result, anyhow, bail};
use chrono::{Local, NaiveDate};
use regex::{Captures, Regex, RegexBuilder};
use rusqlite::types::ValueRef;
use rusqlite::{OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Since format 8. An account or category that goes takes its rules with it: without the condition a rule
/// would match more than the user wanted.
pub(crate) const RULE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS transaction_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    position INTEGER NOT NULL,
    pattern TEXT NOT NULL DEFAULT '',
    payee TEXT NOT NULL DEFAULT '',
    memo TEXT NOT NULL DEFAULT '',
    is_regex INTEGER NOT NULL DEFAULT 0,
    amount_min INTEGER,
    amount_max INTEGER,
    sign TEXT NOT NULL DEFAULT 'any',
    account_id INTEGER REFERENCES accounts(id) ON DELETE CASCADE,
    category_id INTEGER REFERENCES categories(id) ON DELETE CASCADE,
    tags TEXT NOT NULL DEFAULT '',
    rename_payee TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS rule_runs (id INTEGER PRIMARY KEY AUTOINCREMENT, made_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS rule_changes (
    run_id INTEGER NOT NULL REFERENCES rule_runs(id) ON DELETE CASCADE,
    posting_id INTEGER NOT NULL,
    before_state TEXT NOT NULL,
    after_state TEXT NOT NULL,
    was_categorized INTEGER NOT NULL,
    before_category INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    PRIMARY KEY (run_id, posting_id)
);
";

/// Most rules that can be added.
pub const MAX_RULES: usize = 500;
const MAX_REGEX: usize = 500;
const MAX_WORDS: usize = 2_000;
const MAX_REPLACEMENT: usize = 200;
/// Of a compiled regular expression, in bytes.
const REGEX_SIZE: usize = 256 * 1024;
/// How much of a text a regular expression looks at, in characters.
const MAX_TEXT: usize = 2_000;
const MAX_TAG: usize = 40;
/// Most tags of a rule, and of a transaction.
const MAX_TAGS: usize = 20;
/// How many transactions an error lists.
const LISTED: usize = 10;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Sign {
    #[default]
    Any,
    In,
    Out,
}

impl Sign {
    pub const ALL: [Sign; 3] = [Sign::Any, Sign::Out, Sign::In];

    pub fn code(self) -> &'static str {
        match self {
            Sign::Any => "any",
            Sign::In => "in",
            Sign::Out => "out",
        }
    }

    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s.trim().to_lowercase().as_str() {
            "" | "any" => Sign::Any,
            "in" => Sign::In,
            "out" => Sign::Out,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Sign::Any => tr("money in or out"),
            Sign::In => tr("money in"),
            Sign::Out => tr("money out"),
        }
    }
}

/// A rule matches when all the conditions it has are met.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Rule {
    pub id: i64,
    /// Keywords separated by `|`, or a regular expression, in the payee or in the notes.
    pub pattern: String,
    /// The same in the payee alone, and in the notes alone.
    pub payee: String,
    pub memo: String,
    pub regex: bool,
    /// Whatever the sign, included.
    pub amount_min: Option<Cents>,
    pub amount_max: Option<Cents>,
    pub sign: Sign,
    pub account_id: Option<i64>,
    pub account: String,
    pub category_id: Option<i64>,
    pub category: String,
    pub tags: Vec<String>,
    pub rename_payee: String,
    /// Why the rule is skipped.
    pub problem: Option<String>,
}

impl Rule {
    /// The conditions, for people.
    pub fn conditions(&self) -> String {
        let mut out = Vec::new();
        for (text, label) in [(&self.pattern, ""), (&self.payee, tr("payee")), (&self.memo, tr("notes"))] {
            if text.is_empty() {
                continue;
            }
            let text = if self.regex { format!("/{text}/") } else { text.replace('|', " · ") };
            out.push(if label.is_empty() { text } else { format!("{label}: {text}") });
        }
        match (self.amount_min, self.amount_max) {
            (Some(a), Some(b)) if a == b => out.push(fmt_eur(a)),
            (Some(a), Some(b)) => out.push(format!("{} – {}", fmt_eur(a), fmt_eur(b))),
            (Some(a), None) => out.push(format!("≥ {}", fmt_eur(a))),
            (None, Some(b)) => out.push(format!("≤ {}", fmt_eur(b))),
            (None, None) => {}
        }
        if self.sign != Sign::Any {
            out.push(self.sign.label().into());
        }
        if self.account_id.is_some() {
            out.push(trf("account {name}", &[("name", &self.account)]));
        }
        out.join(", ")
    }

    /// What it does, for people.
    pub fn actions(&self) -> String {
        let mut out = Vec::new();
        if self.category_id.is_some() {
            out.push(self.category.clone());
        }
        out.extend(self.tags.iter().map(|t| format!("#{t}")));
        if !self.rename_payee.is_empty() {
            out.push(trf("payee \"{name}\"", &[("name", &self.rename_payee)]));
        }
        out.join(", ")
    }

    pub fn summary(&self) -> String {
        format!("{} ➡ {}", self.conditions(), self.actions())
    }
}

/// Where a rule goes among the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    First,
    Up,
    Down,
    Last,
}

impl Place {
    pub fn from_code(s: &str) -> Option<Self> {
        Some(match s.trim().to_lowercase().as_str() {
            "first" => Place::First,
            "up" => Place::Up,
            "down" => Place::Down,
            "last" => Place::Last,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Broken {
    pub id: i64,
    pub error: String,
}

/// What the rules did to a transaction: kept with it. A category by hand takes the mark away.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Marks {
    /// The rule, and the account of the chart it chose: the mark counts only while that is the category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<(i64, i64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payee: Option<i64>,
    /// By tag, in lowercase. One that the user removed stays here, and is not added again.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, i64>,
    /// The run on demand, so that what an undo looks for is in one transaction only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<i64>,
}

impl Marks {
    /// What cannot be read marks nothing: the transaction counts as changed by hand.
    pub fn read(text: Option<&str>) -> Self {
        text.and_then(|t| serde_json::from_str(t).ok()).unwrap_or_default()
    }

    pub fn write(&self) -> Result<Option<String>> {
        Ok(if *self == Marks::default() { None } else { Some(serde_json::to_string(self)?) })
    }
}

/// The rules that set something of a transaction, for people and agents.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct RuleUse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payee: Option<i64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, i64>,
}

impl RuleUse {
    pub fn is_empty(&self) -> bool {
        *self == RuleUse::default()
    }

    /// The rules, each once.
    pub fn ids(&self) -> Vec<i64> {
        let mut out: Vec<i64> =
            self.category.into_iter().chain(self.payee).chain(self.tags.values().copied()).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    pub(crate) fn of(marks: &Marks, tags: &[String], other_account: Option<i64>) -> Self {
        let there = |t: &&String| tags.iter().any(|x| x.to_lowercase() == **t);
        RuleUse {
            category: marks.category.filter(|(_, a)| Some(*a) == other_account).map(|(r, _)| r),
            payee: marks.payee,
            tags: marks.tags.iter().filter(|(t, _)| there(t)).map(|(t, r)| (t.clone(), *r)).collect(),
        }
    }
}

impl Line {
    /// What the user changes by hand is no more the work of a rule.
    pub(crate) fn changed_by_hand(&mut self, category: bool, payee: bool) -> Result<()> {
        if self.rules.is_none() {
            return Ok(());
        }
        let mut m = Marks::read(self.rules.as_deref());
        if category {
            m.category = None;
        }
        if payee {
            m.payee = None;
        }
        self.rules = m.write()?;
        Ok(())
    }
}

// ------------------------------------------------------------------ tags

/// Tags separated by commas, each once; what cannot be a tag is refused.
pub fn parse_tags(text: &str) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for part in text.split(',') {
        let tag = part.trim().trim_start_matches('#').split_whitespace().collect::<Vec<_>>().join(" ");
        if tag.is_empty() {
            continue;
        }
        if tag.chars().count() > MAX_TAG || tag.chars().any(char::is_control) {
            bail!(trf("\"{tag}\" cannot be a tag: at most {n} characters", &[("tag", &cut(&tag)), ("n", &MAX_TAG)]));
        }
        if !has_tag(&out, &tag) {
            out.push(tag);
        }
    }
    if out.len() > MAX_TAGS {
        bail!(trf("there are too many tags: at most {n}", &[("n", &MAX_TAGS)]));
    }
    Ok(out)
}

/// As they are in the file.
pub(crate) fn stored_tags(text: Option<&str>) -> Vec<String> {
    text.unwrap_or_default().split(',').filter(|t| !t.is_empty()).map(str::to_string).collect()
}

fn store_tags(tags: &[String]) -> Option<String> {
    (!tags.is_empty()).then(|| tags.join(","))
}

pub(crate) fn has_tag(tags: &[String], tag: &str) -> bool {
    let tag = tag.to_lowercase();
    tags.iter().any(|t| t.to_lowercase() == tag)
}

fn cut(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(MAX_TAG).collect()
}

// -------------------------------------------------------------- matching

/// What the rules look at: the text of the bank, and what the user wrote instead.
pub struct Subject {
    payee: String,
    memo: String,
    amount: Cents,
    account: Option<i64>,
    both_lower: String,
    payee_lower: String,
    memo_lower: String,
    written: Option<Box<Subject>>,
}

impl Subject {
    pub fn new(payee: &str, memo: &str, amount: Cents, account: Option<i64>) -> Self {
        let clip = |s: &str| s.chars().take(MAX_TEXT).collect::<String>();
        let (p, m) = (payee.to_lowercase(), memo.to_lowercase());
        Subject {
            payee: clip(payee),
            memo: clip(memo),
            amount,
            account,
            both_lower: format!(" {p} {m} "),
            payee_lower: format!(" {p} "),
            memo_lower: format!(" {m} "),
            written: None,
        }
    }

    /// Of a transaction. The payee that a rule wrote is not what the bank wrote.
    pub fn of(t: &Transaction) -> Self {
        let account = Some(t.account_id);
        let payee = t.original_payee.as_deref().unwrap_or(&t.payee);
        let memo = t.original_memo.as_deref().unwrap_or(&t.memo);
        let mut out = Subject::new(payee, memo, t.amount, account);
        if t.rules.payee.is_none() && (t.original_payee.is_some() || t.original_memo.is_some()) {
            out.written = Some(Box::new(Subject::new(&t.payee, &t.memo, t.amount, account)));
        }
        out
    }
}

enum Matcher {
    /// No condition on this text.
    Absent,
    Words(Vec<String>),
    Regex(Box<Regex>),
}

impl Matcher {
    fn new(text: &str, regex: bool) -> Result<Self> {
        if text.is_empty() {
            return Ok(Matcher::Absent);
        }
        if !regex {
            let lower = text.to_lowercase();
            // The spaces around a keyword are part of it.
            return Ok(Matcher::Words(lower.split('|').filter(|k| !k.trim().is_empty()).map(str::to_string).collect()));
        }
        if text.chars().count() > MAX_REGEX {
            bail!(trf("the regular expression is too long: at most {n} characters", &[("n", &MAX_REGEX)]));
        }
        let built = RegexBuilder::new(text)
            .case_insensitive(true)
            .size_limit(REGEX_SIZE)
            .dfa_size_limit(REGEX_SIZE)
            .nest_limit(50)
            .build();
        match built {
            Ok(r) => Ok(Matcher::Regex(Box::new(r))),
            Err(regex::Error::CompiledTooBig(_)) => bail!(tr("the regular expression is too large")),
            Err(e) => {
                let e = e.to_string();
                // Its last line says what is wrong; those before draw the place.
                let what = e.lines().last().unwrap_or_default().trim().trim_start_matches("error: ");
                bail!(trf("the regular expression is not valid: {error}", &[("error", &what)]))
            }
        }
    }

    fn is_met(&self, lower: &str, texts: &[&str]) -> bool {
        match self {
            Matcher::Absent => true,
            Matcher::Words(words) => words.iter().any(|k| lower.contains(k)),
            Matcher::Regex(r) => texts.iter().any(|t| r.is_match(t)),
        }
    }

    fn captures<'a>(&self, texts: &[&'a str]) -> Option<Captures<'a>> {
        let Matcher::Regex(r) = self else { return None };
        texts.iter().find_map(|t| r.captures(t))
    }
}

enum Part {
    Text(String),
    Group(usize),
}

/// `$1`, `${1}`, `$name`, `${name}` and `$$`. Groups that are not there are refused: they would be empty.
fn parse_replacement(text: &str, regex: &Regex) -> Result<Vec<Part>> {
    let mut out = Vec::new();
    let mut literal = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        let (before, after) = rest.split_at(at);
        literal.push_str(before);
        let after = after.strip_prefix('$').unwrap_or(after);
        if let Some(r) = after.strip_prefix('$') {
            literal.push('$');
            rest = r;
            continue;
        }
        let (name, r) = match after.strip_prefix('{').and_then(|a| a.split_once('}')) {
            Some((name, r)) => (name, r),
            None => {
                let end = after.find(|c: char| !c.is_ascii_alphanumeric() && c != '_').unwrap_or(after.len());
                after.split_at(end)
            }
        };
        let group = match name.parse::<usize>() {
            Ok(n) => (n < regex.captures_len()).then_some(n),
            Err(_) => regex.capture_names().position(|n| n == Some(name)),
        };
        let group = group.ok_or_else(|| {
            anyhow!(trf("the regular expression has no group \"{name}\" ($$ is a dollar)", &[("name", &name)]))
        })?;
        if !literal.is_empty() {
            out.push(Part::Text(std::mem::take(&mut literal)));
        }
        out.push(Part::Group(group));
        rest = r;
    }
    literal.push_str(rest);
    if !literal.is_empty() {
        out.push(Part::Text(literal));
    }
    Ok(out)
}

struct Compiled {
    rule: Rule,
    pattern: Matcher,
    payee: Matcher,
    memo: Matcher,
    replacement: Option<Vec<Part>>,
}

impl Compiled {
    fn new(rule: &Rule) -> Result<Self> {
        if let Some(p) = &rule.problem {
            bail!(p.clone());
        }
        let texts = [&rule.pattern, &rule.payee, &rule.memo];
        if texts.iter().all(|t| t.is_empty())
            && rule.amount_min.is_none()
            && rule.amount_max.is_none()
            && rule.sign == Sign::Any
            && rule.account_id.is_none()
        {
            bail!(tr("the rule needs a keyword or another condition"));
        }
        let of = |t: &str| Matcher::new(t, rule.regex);
        let (pattern, payee, memo) = (of(&rule.pattern)?, of(&rule.payee)?, of(&rule.memo)?);
        let source = [&payee, &pattern, &memo].into_iter().find_map(|m| match m {
            Matcher::Regex(r) => Some(r),
            _ => None,
        });
        let replacement = match source {
            Some(r) if !rule.rename_payee.is_empty() => Some(parse_replacement(&rule.rename_payee, r)?),
            _ => None,
        };
        Ok(Compiled { rule: rule.clone(), pattern, payee, memo, replacement })
    }

    /// The texts that match, if any do.
    fn met<'a>(&self, s: &'a Subject) -> Option<&'a Subject> {
        std::iter::once(s).chain(s.written.as_deref()).find(|s| self.matches(s))
    }

    fn matches(&self, s: &Subject) -> bool {
        let r = &self.rule;
        let size = s.amount.unsigned_abs();
        let sign = match r.sign {
            Sign::Any => true,
            Sign::In => s.amount > 0,
            Sign::Out => s.amount < 0,
        };
        sign && r.amount_min.is_none_or(|a| size >= a.unsigned_abs())
            && r.amount_max.is_none_or(|a| size <= a.unsigned_abs())
            && r.account_id.is_none_or(|a| s.account == Some(a))
            && self.pattern.is_met(&s.both_lower, &[&s.payee, &s.memo])
            && self.payee.is_met(&s.payee_lower, &[&s.payee])
            && self.memo.is_met(&s.memo_lower, &[&s.memo])
    }

    /// The new payee, when there is one.
    fn new_payee(&self, s: &Subject) -> Option<String> {
        let Some(parts) = &self.replacement else {
            return Some(self.rule.rename_payee.clone()).filter(|p| !p.is_empty());
        };
        let found = match (&self.payee, &self.pattern) {
            (Matcher::Regex(_), _) => self.payee.captures(&[&s.payee]),
            (_, Matcher::Regex(_)) => self.pattern.captures(&[&s.payee, &s.memo]),
            _ => self.memo.captures(&[&s.memo]),
        }?;
        let mut out = String::new();
        let mut left = MAX_REPLACEMENT;
        for p in parts {
            let text = match p {
                Part::Text(t) => t.as_str(),
                Part::Group(g) => found.get(*g).map_or("", |m| m.as_str()),
            };
            for c in text.chars().take(left) {
                out.push(c);
                left -= 1;
            }
        }
        let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
        (!out.is_empty()).then_some(out)
    }
}

/// What the rules say of a transaction. Each thing with the rule that decided it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub category: Option<(i64, i64)>,
    pub payee: Option<(i64, String)>,
    pub tags: Vec<(i64, String)>,
}

impl Outcome {
    /// The rules, in order, each once.
    pub fn ids(&self) -> Vec<i64> {
        let mut out = Vec::new();
        let all = self.payee.iter().map(|p| p.0).chain(self.category.iter().map(|c| c.0));
        for id in all.chain(self.tags.iter().map(|t| t.0)) {
            if !out.contains(&id) {
                out.push(id);
            }
        }
        out
    }
}

/// The rules of the file, in order and ready to match.
pub struct Ruleset {
    rules: Vec<Compiled>,
    /// The rules that are skipped, and why.
    pub broken: Vec<Broken>,
}

impl Ruleset {
    /// The rules in order: the first with a category decides it. The payee is that of the first with a
    /// replacement up to there; tags are those of every rule that matches.
    pub fn evaluate(&self, s: &Subject) -> Outcome {
        let mut out = Outcome::default();
        for c in &self.rules {
            let Some(s) = c.met(s) else { continue };
            let r = &c.rule;
            if out.category.is_none() {
                if out.payee.is_none() {
                    out.payee = c.new_payee(s).map(|p| (r.id, p));
                }
                out.category = r.category_id.map(|c| (r.id, c));
            }
            for t in &r.tags {
                if !out.tags.iter().any(|(_, x)| x.to_lowercase() == t.to_lowercase()) {
                    out.tags.push((r.id, t.clone()));
                }
            }
        }
        out
    }

    pub fn rule(&self, id: i64) -> Option<&Rule> {
        self.rules.iter().map(|c| &c.rule).find(|r| r.id == id)
    }

    /// For the command line and MCP.
    pub fn json(&self, o: &Outcome) -> serde_json::Value {
        let category = o.category.and_then(|(r, _)| self.rule(r)).map(|r| r.category.as_str());
        let tags: Vec<&str> = o.tags.iter().map(|t| t.1.as_str()).collect();
        serde_json::json!({"category": category, "category_rule": o.category.map(|c| c.0),
                           "payee": o.payee.as_ref().map(|p| &p.1), "payee_rule": o.payee.as_ref().map(|p| p.0),
                           "tags": tags, "rules": o.ids()})
    }

    /// For people: what is set, and the rules with their conditions.
    pub fn explain(&self, o: &Outcome) -> String {
        let one = |id: i64| self.rule(id).map(|r| trf("rule {id}: {what}", &[("id", &id), ("what", &r.summary())]));
        o.ids().into_iter().filter_map(one).collect::<Vec<_>>().join("\n")
    }
}

// ------------------------------------------------------------------ runs

/// Which transactions a run on demand touches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    /// Also those that have a category. Without it, only those without.
    pub all: bool,
    /// Also categories and payees that the user wrote.
    pub overwrite: bool,
}

/// What a run on demand changes of a transaction.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Change {
    pub id: i64,
    pub date: NaiveDate,
    pub account: String,
    pub amount: Cents,
    pub payee: String,
    pub category: String,
    #[serde(skip)]
    pub category_id: Option<i64>,
    pub new_category: Option<String>,
    #[serde(skip)]
    pub new_category_id: Option<i64>,
    pub new_payee: Option<String>,
    pub new_tags: Vec<String>,
    /// The rules that change something.
    pub rules: Vec<i64>,
    #[serde(skip)]
    entry: i64,
    #[serde(skip)]
    memo: String,
    #[serde(skip)]
    category_rule: Option<i64>,
    #[serde(skip)]
    payee_rule: Option<i64>,
    #[serde(skip)]
    tag_rules: Vec<i64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Plan {
    pub changes: Vec<Change>,
    /// Stands for the changes: a run that is given it is refused when there would be others.
    pub token: String,
    /// Rules that are skipped.
    pub broken: Vec<Broken>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Run {
    pub changed: usize,
    pub categorized: usize,
    pub broken: Vec<Broken>,
}

/// The run that can be undone.
#[derive(Clone, Debug, Serialize)]
pub struct LastRun {
    pub made_at: String,
    pub changed: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Skipped {
    pub id: i64,
    /// The transaction as it is now, for people; empty when it is not there.
    pub transaction: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Undone {
    pub undone: usize,
    pub skipped: Vec<Skipped>,
}

/// What a run changes of an entry, before and after it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct State {
    /// The account of the chart on the other side.
    other: i64,
    payee: String,
    memo: String,
    own_payee: Option<String>,
    own_memo: Option<String>,
    original_payee: Option<String>,
    original_memo: Option<String>,
    tags: Option<String>,
    rules: Option<String>,
}

impl State {
    fn of(d: &Draft, posting: i64) -> Option<Self> {
        let (mine, other) = match d.lines.as_slice() {
            [a, b] if a.id == Some(posting) => (a, b),
            [b, a] if a.id == Some(posting) => (a, b),
            _ => return None,
        };
        Some(State {
            other: other.account,
            payee: d.payee.clone(),
            memo: d.memo.clone(),
            own_payee: mine.payee.clone(),
            own_memo: mine.memo.clone(),
            original_payee: mine.original_payee.clone(),
            original_memo: mine.original_memo.clone(),
            tags: mine.tags.clone(),
            rules: mine.rules.clone(),
        })
    }

    fn put(self, d: &mut Draft, posting: i64) {
        (d.payee, d.memo) = (self.payee, self.memo);
        for l in &mut d.lines {
            if l.id != Some(posting) {
                l.account = self.other;
                continue;
            }
            l.payee.clone_from(&self.own_payee);
            l.memo.clone_from(&self.own_memo);
            l.original_payee.clone_from(&self.original_payee);
            l.original_memo.clone_from(&self.original_memo);
            l.tags.clone_from(&self.tags);
            l.rules.clone_from(&self.rules);
        }
    }
}

/// What the rules give a transaction that is added.
pub(crate) struct Applied {
    pub payee: String,
    pub memo: String,
    pub tags: Option<String>,
    pub rules: Option<String>,
    pub category: Option<i64>,
}

impl Db {
    // ------------------------------------------------------------- the rules

    /// The rules in order, broken ones included.
    pub fn rules(&self) -> Result<Vec<Rule>> {
        let mut all = self.load_rules()?;
        for r in &mut all {
            r.problem = Compiled::new(r).err().map(|e| format!("{e:#}"));
        }
        Ok(all)
    }

    pub fn ruleset(&self) -> Result<Ruleset> {
        let mut out = Ruleset { rules: Vec::new(), broken: Vec::new() };
        for r in self.load_rules()? {
            match Compiled::new(&r) {
                Ok(c) => out.rules.push(c),
                Err(e) => out.broken.push(Broken { id: r.id, error: format!("{e:#}") }),
            }
        }
        Ok(out)
    }

    fn load_rules(&self) -> Result<Vec<Rule>> {
        let cats: HashMap<i64, String> = self.categories()?.into_iter().map(|c| (c.id, c.path)).collect();
        let accounts: HashMap<i64, String> = self.accounts(true)?.into_iter().map(|a| (a.id, a.name)).collect();
        let mut st = self.conn.prepare(
            "SELECT id, pattern, payee, memo, is_regex, amount_min, amount_max, sign, account_id, category_id, tags,
                    rename_payee
             FROM transaction_rules ORDER BY position, id",
        )?;
        let mut found = st.query([])?;
        let mut out = Vec::new();
        while let Some(row) = found.next()? {
            let mut sound = true;
            let mut rule = Rule {
                id: row.get(0)?,
                pattern: text(row, 1, &mut sound),
                payee: text(row, 2, &mut sound),
                memo: text(row, 3, &mut sound),
                regex: number(row, 4, &mut sound).is_some_and(|n| n != 0),
                amount_min: number(row, 5, &mut sound),
                amount_max: number(row, 6, &mut sound),
                account_id: number(row, 8, &mut sound),
                category_id: number(row, 9, &mut sound),
                rename_payee: text(row, 11, &mut sound),
                ..Default::default()
            };
            let sign = Sign::from_code(&text(row, 7, &mut sound));
            rule.sign = sign.unwrap_or_default();
            rule.tags = stored_tags(Some(&text(row, 10, &mut sound)));
            rule.account = rule.account_id.and_then(|a| accounts.get(&a).cloned()).unwrap_or_default();
            rule.category = rule.category_id.and_then(|c| cats.get(&c).cloned()).unwrap_or_default();
            let gone = rule.account_id.is_some_and(|a| !accounts.contains_key(&a))
                || rule.category_id.is_some_and(|c| !cats.contains_key(&c));
            if !sound || sign.is_none() {
                rule.problem = Some(tr("the rule is damaged in the data file").into());
            } else if gone {
                rule.problem = Some(tr("the category or the account of the rule is not there").into());
            }
            out.push(rule);
        }
        Ok(out)
    }

    /// The rule as it will be saved; an error when it cannot be.
    fn checked_rule(&self, r: &Rule) -> Result<Rule> {
        let tidy = |s: &str| {
            if s.trim().is_empty() {
                String::new()
            } else if r.regex {
                s.to_string()
            } else {
                s.trim().to_string()
            }
        };
        let rule = Rule {
            pattern: tidy(&r.pattern),
            payee: tidy(&r.payee),
            memo: tidy(&r.memo),
            rename_payee: r.rename_payee.trim().to_string(),
            tags: parse_tags(&r.tags.join(","))?,
            problem: None,
            ..r.clone()
        };
        for t in [&rule.pattern, &rule.payee, &rule.memo].into_iter().filter(|t| !t.is_empty()) {
            if !rule.regex && t.split('|').all(|k| k.trim().is_empty()) {
                bail!(tr("the rule needs a keyword"));
            }
            if t.chars().count() > MAX_WORDS {
                bail!(trf("the keywords are too long: at most {n} characters", &[("n", &MAX_WORDS)]));
            }
        }
        if rule.rename_payee.chars().count() > MAX_REPLACEMENT {
            bail!(trf("the new payee is too long: at most {n} characters", &[("n", &MAX_REPLACEMENT)]));
        }
        let limits = [rule.amount_min, rule.amount_max];
        if limits.iter().flatten().any(|a| !(0..=MAX_AMOUNT).contains(a))
            || matches!(limits, [Some(a), Some(b)] if a > b)
        {
            bail!(tr("the range of amounts is not valid: two amounts without sign, the smaller first"));
        }
        if rule.category_id.is_none() && rule.tags.is_empty() && rule.rename_payee.is_empty() {
            bail!(tr("the rule does nothing: give it a category, tags or a new payee"));
        }
        if let Some(c) = rule.category_id {
            self.category_ledger(Some(c))?;
        }
        if let Some(a) = rule.account_id {
            self.money_ledger(a)?;
        }
        Compiled::new(&rule)?;
        Ok(rule)
    }

    /// A keyword rule after the others.
    pub fn add_rule(&self, pattern: &str, category_id: i64, rename_payee: &str) -> Result<i64> {
        if pattern.trim().is_empty() {
            bail!(tr("the rule needs a keyword"));
        }
        let rule = Rule {
            pattern: pattern.into(),
            category_id: Some(category_id),
            rename_payee: rename_payee.into(),
            ..Default::default()
        };
        self.insert_rule(&rule, false)
    }

    /// Adds a rule, before the others or after them. The id of `rule` is not used.
    pub fn insert_rule(&self, rule: &Rule, first: bool) -> Result<i64> {
        self.atomically(|| {
            let r = self.checked_rule(rule)?;
            let order = self.rule_order()?;
            if order.len() >= MAX_RULES {
                bail!(trf("there are too many rules: at most {n}", &[("n", &MAX_RULES)]));
            }
            self.conn.execute(
                "INSERT INTO transaction_rules(position,pattern,payee,memo,is_regex,amount_min,amount_max,sign,
                                               account_id,category_id,tags,rename_payee)
                 VALUES(0,?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![
                    r.pattern,
                    r.payee,
                    r.memo,
                    r.regex,
                    r.amount_min,
                    r.amount_max,
                    r.sign.code(),
                    r.account_id,
                    r.category_id,
                    r.tags.join(","),
                    r.rename_payee
                ],
            )?;
            let id = self.conn.last_insert_rowid();
            let mut order = order;
            order.insert(if first { 0 } else { order.len() }, id);
            self.set_rule_order(&order)?;
            Ok(id)
        })
    }

    pub fn update_rule(&self, rule: &Rule) -> Result<()> {
        self.atomically(|| {
            let r = self.checked_rule(rule)?;
            let n = self.conn.execute(
                "UPDATE transaction_rules SET pattern=?2, payee=?3, memo=?4, is_regex=?5, amount_min=?6, amount_max=?7,
                        sign=?8, account_id=?9, category_id=?10, tags=?11, rename_payee=?12 WHERE id=?1",
                params![
                    r.id,
                    r.pattern,
                    r.payee,
                    r.memo,
                    r.regex,
                    r.amount_min,
                    r.amount_max,
                    r.sign.code(),
                    r.account_id,
                    r.category_id,
                    r.tags.join(","),
                    r.rename_payee
                ],
            )?;
            if n == 0 { Err(no_rule(r.id)) } else { Ok(()) }
        })
    }

    pub fn delete_rule(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM transaction_rules WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn rule(&self, id: i64) -> Result<Rule> {
        self.rules()?.into_iter().find(|r| r.id == id).ok_or_else(|| no_rule(id))
    }

    fn rule_order(&self) -> Result<Vec<i64>> {
        let mut st = self.conn.prepare("SELECT id FROM transaction_rules ORDER BY position, id")?;
        Ok(st.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?)
    }

    fn set_rule_order(&self, ids: &[i64]) -> Result<()> {
        let mut st = self.conn.prepare("UPDATE transaction_rules SET position=?2 WHERE id=?1")?;
        for (position, id) in (1_i64..).zip(ids) {
            st.execute([*id, position])?;
        }
        Ok(())
    }

    pub fn move_rule(&self, id: i64, to: Place) -> Result<()> {
        self.atomically(|| {
            let mut order = self.rule_order()?;
            let at = order.iter().position(|x| *x == id).ok_or_else(|| no_rule(id))?;
            order.remove(at);
            let target = match to {
                Place::First => 0,
                Place::Up => at.saturating_sub(1),
                Place::Down => (at + 1).min(order.len()),
                Place::Last => order.len(),
            };
            order.insert(target, id);
            self.set_rule_order(&order)
        })
    }

    /// The rule that categorizes others like a transaction, for the user to see and change.
    pub fn rule_from(&self, id: i64) -> Result<Rule> {
        let t = self.transaction(id)?;
        if !matches!(t.kind, TxKind::Category | TxKind::Uncategorized) {
            bail!(tr("rules do not categorize transfers, splits and entries of the chart of accounts"));
        }
        let payee = t.original_payee.as_deref().unwrap_or(&t.payee).trim();
        let memo = t.original_memo.as_deref().unwrap_or(&t.memo).trim();
        let mut rule = Rule { category_id: t.category_id, category: t.category.clone(), ..Default::default() };
        let (text, of_payee) = if payee.is_empty() { (memo, false) } else { (payee, true) };
        // In a keyword it would separate two.
        rule.regex = text.contains('|');
        let text: String = text.chars().take(MAX_REPLACEMENT).collect();
        let text = if rule.regex { regex::escape(&text) } else { text };
        if of_payee {
            rule.payee = text;
        } else {
            rule.memo = text;
        }
        rule.tags = t.tags.into_iter().take(MAX_TAGS).collect();
        if t.payee.trim() != payee && !payee.is_empty() {
            // A name that was cut or has a $ is written again.
            let name: String = t.payee.trim().chars().take(MAX_REPLACEMENT).collect();
            rule.rename_payee = if rule.regex { name.replace('$', "$$") } else { name };
        }
        Ok(rule)
    }

    /// The transactions that a rule not saved yet matches, newest first, whatever the other rules say.
    pub fn rule_matches(&self, rule: &Rule, among: &[Transaction]) -> Result<Vec<i64>> {
        let compiled = Compiled::new(&Rule { problem: None, ..rule.clone() })?;
        let fits = |t: &&Transaction| is_for_rules(t) && compiled.met(&Subject::of(t)).is_some();
        Ok(among.iter().filter(fits).map(|t| t.id).collect())
    }

    // --------------------------------------------------- on what is added

    /// What the rules give a transaction that has no category.
    pub(crate) fn apply_to_new(&self, set: &Ruleset, payee: &str, memo: &str, s: &Subject) -> Result<Applied> {
        let o = set.evaluate(s);
        let mut marks = Marks::default();
        if let Some((rule, category)) = o.category {
            marks.category = Some((rule, self.category_ledger(Some(category))?));
        }
        let (mut payee, mut memo) = (payee.to_string(), memo.to_string());
        if let Some((rule, name)) = o.payee {
            if memo.is_empty() {
                memo = payee;
            }
            payee = name;
            marks.payee = Some(rule);
        }
        let tags: Vec<String> = o.tags.iter().take(MAX_TAGS).map(|t| t.1.clone()).collect();
        marks.tags = o.tags.iter().map(|(r, t)| (t.to_lowercase(), *r)).collect();
        Ok(Applied { payee, memo, tags: store_tags(&tags), rules: marks.write()?, category: o.category.map(|c| c.1) })
    }

    // ------------------------------------------------------------- on demand

    /// What a run on demand would change.
    pub fn plan_rules(&self, scope: Scope) -> Result<Plan> {
        let set = self.ruleset()?;
        let mut changes = Vec::new();
        for t in self.transactions(&TxFilter { uncategorized: !scope.all, ..Default::default() })? {
            // Transfers, splits and entries of the chart are left to people.
            if !is_for_rules(&t) || (!scope.all && t.kind != TxKind::Uncategorized) {
                continue;
            }
            changes.extend(self.change(&set, &t, scope));
        }
        let token = format!("{:016x}", fnv(&format!("{changes:?}")));
        Ok(Plan { changes, token, broken: set.broken })
    }

    fn change(&self, set: &Ruleset, t: &Transaction, scope: Scope) -> Option<Change> {
        let o = set.evaluate(&Subject::of(t));
        let by_hand = t.category_id.is_some() && t.rules.category.is_none();
        let category = o.category.filter(|(_, c)| (!by_hand || scope.overwrite) && t.category_id != Some(*c));
        // The payee that the user wrote: neither that of the bank nor that of a rule.
        let written = t.original_payee.is_some() && t.rules.payee.is_none();
        let payee = o.payee.filter(|(_, p)| {
            (!written || scope.overwrite) && (*p != t.payee || (category.is_some() && t.memo.is_empty()))
        });
        let wanted =
            |tag: &str| !has_tag(&t.tags, tag) && (scope.overwrite || !t.marks.tags.contains_key(&tag.to_lowercase()));
        let room = MAX_TAGS.saturating_sub(t.tags.len());
        let tags: Vec<(i64, String)> = o.tags.into_iter().filter(|(_, tag)| wanted(tag)).take(room).collect();
        if category.is_none() && payee.is_none() && tags.is_empty() {
            return None;
        }
        let done = Outcome { category, payee, tags };
        let rules = done.ids();
        Some(Change {
            id: t.id,
            date: t.date,
            account: t.account.clone(),
            amount: t.amount,
            payee: t.payee.clone(),
            category: t.category.clone(),
            category_id: t.category_id,
            new_category: done.category.and_then(|(r, _)| set.rule(r)).map(|r| r.category.clone()),
            new_category_id: done.category.map(|c| c.1),
            new_payee: done.payee.as_ref().map(|p| p.1.clone()),
            new_tags: done.tags.iter().map(|t| t.1.clone()).collect(),
            rules,
            entry: t.entry_id,
            memo: t.memo.clone(),
            category_rule: done.category.map(|c| c.0),
            payee_rule: done.payee.map(|p| p.0),
            tag_rules: done.tags.iter().map(|t| t.0).collect(),
        })
    }

    /// Applies the rules to the transactions without a category. Returns how many changed.
    pub fn apply_rules(&self) -> Result<usize> {
        Ok(self.run_rules(Scope::default(), None)?.changed)
    }

    /// Applies the rules, all of it or nothing, and remembers how to undo it. With the token of a plan, only
    /// when the changes are still those.
    pub fn run_rules(&self, scope: Scope, token: Option<&str>) -> Result<Run> {
        self.atomically(|| {
            let plan = self.plan_rules(scope)?;
            if token.is_some_and(|t| t != plan.token) {
                bail!(tr("the books or the rules have changed meanwhile: look at the changes again; nothing was done"));
            }
            let mut out = Run { broken: plan.broken, ..Default::default() };
            if plan.changes.is_empty() {
                return Ok(out);
            }
            self.conn.execute("DELETE FROM rule_runs", [])?;
            self.conn.execute(
                "INSERT INTO rule_runs(made_at) VALUES(?1)",
                [Local::now().format("%Y-%m-%d %H:%M:%S").to_string()],
            )?;
            let run = self.conn.last_insert_rowid();
            for c in &plan.changes {
                self.make_change(run, c)?;
                out.changed += 1;
                out.categorized += usize::from(c.new_category_id.is_some());
            }
            Ok(out)
        })
    }

    fn make_change(&self, run: i64, c: &Change) -> Result<()> {
        let mut d = self.load_draft(c.entry)?;
        let before = State::of(&d, c.id).ok_or_else(|| no_transaction(c.id))?;
        let account = c.new_category_id.map(|id| self.category_ledger(Some(id))).transpose()?;
        for l in &mut d.lines {
            if l.id != Some(c.id) {
                l.account = account.unwrap_or(l.account);
                continue;
            }
            let mut marks = Marks::read(l.rules.as_deref());
            marks.run = Some(run);
            if let (Some(rule), Some(account)) = (c.category_rule, account) {
                marks.category = Some((rule, account));
            }
            if c.new_payee.is_some() {
                marks.payee = c.payee_rule;
                l.original_payee.get_or_insert_with(|| c.payee.clone());
                l.original_memo.get_or_insert_with(|| c.memo.clone());
                (l.payee, l.memo) = (None, None);
            }
            let mut tags = stored_tags(l.tags.as_deref());
            for (rule, tag) in c.tag_rules.iter().zip(&c.new_tags) {
                marks.tags.insert(tag.to_lowercase(), *rule);
                tags.push(tag.clone());
            }
            l.tags = store_tags(&tags);
            l.rules = marks.write()?;
        }
        if let Some(p) = &c.new_payee {
            d.memo = if c.memo.is_empty() { c.payee.clone() } else { c.memo.clone() };
            d.payee.clone_from(p);
        }
        self.save_entry(&d)?;
        let after = State::of(&d, c.id).ok_or_else(|| no_transaction(c.id))?;
        self.conn
            .prepare_cached(
                "INSERT INTO rule_changes(run_id,posting_id,before_state,after_state,was_categorized,before_category)
             VALUES(?1,?2,?3,?4,?5,?6)",
            )?
            .execute(params![
                run,
                c.id,
                serde_json::to_string(&before)?,
                serde_json::to_string(&after)?,
                c.category_id.is_some(),
                c.category_id
            ])?;
        Ok(())
    }

    /// The run on demand that can be undone.
    pub fn last_rule_run(&self) -> Result<Option<LastRun>> {
        let found = self
            .conn
            .query_row(
                "SELECT made_at, (SELECT COUNT(*) FROM rule_changes c WHERE c.run_id=r.id)
                 FROM rule_runs r ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;
        Ok(found.map(|(made_at, n)| LastRun { made_at, changed: usize::try_from(n).unwrap_or(0) }))
    }

    /// Undoes the last run on demand. A transaction that was changed since stays as it is and is listed; when
    /// all were, nothing is done and the run can still be undone.
    pub fn undo_rules(&self) -> Result<Undone> {
        self.atomically(|| {
            let run: Option<i64> = self
                .conn
                .query_row("SELECT id FROM rule_runs ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
                .optional()?;
            let run = run.ok_or_else(|| anyhow!(tr("there is no run of the rules to undo")))?;
            let mut st = self.conn.prepare(
                "SELECT posting_id, before_state, after_state, was_categorized, before_category
                 FROM rule_changes WHERE run_id=?1 ORDER BY posting_id",
            )?;
            type Saved = (i64, String, String, bool, Option<i64>);
            let saved: Vec<Saved> = st
                .query_map([run], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
                .collect::<std::result::Result<_, _>>()
                .with_context(|| tr("what is needed to undo is damaged in the data file"))?;
            let mut out = Undone::default();
            for (id, before, after, was_categorized, category) in saved {
                match self.undo_change(id, &before, &after, was_categorized, category)? {
                    None => out.undone += 1,
                    Some(reason) => {
                        out.skipped.push(Skipped { id, transaction: self.line_of(id)?, reason: reason.into() });
                    }
                }
            }
            if out.undone == 0 {
                bail!(trf("nothing was undone, every transaction was changed since: {list}", &[("list", &out.list())]));
            }
            self.conn.execute("DELETE FROM rule_runs", [])?;
            Ok(out)
        })
    }

    /// Why the transaction stays as it is, or nothing when it was put back.
    fn undo_change(
        &self,
        id: i64,
        before: &str,
        after: &str,
        was_categorized: bool,
        category: Option<i64>,
    ) -> Result<Option<&'static str>> {
        let Some(entry) = self.entry_of_transaction(id)? else { return Ok(Some(tr("it was deleted"))) };
        let states = serde_json::from_str::<State>(before).ok().zip(serde_json::from_str::<State>(after).ok());
        let Some((mut before, after)) = states else {
            return Ok(Some(tr("what is needed to undo is damaged in the data file")));
        };
        let mut d = self.load_draft(entry)?;
        if State::of(&d, id).as_ref() != Some(&after) {
            return Ok(Some(tr("it was changed since")));
        }
        // The number of an account of the chart may be that of another one by now.
        before.other = match (was_categorized, category) {
            (false, _) => self.system_ledger(UNCATEGORIZED)?,
            (true, Some(c)) => self.category_ledger(Some(c))?,
            (true, None) => return Ok(Some(tr("the category it had was deleted"))),
        };
        before.put(&mut d, id);
        self.save_entry(&d)?;
        Ok(None)
    }

    /// Date, text and amount of a transaction, when it is there.
    fn line_of(&self, id: i64) -> Result<String> {
        let found = self
            .conn
            .query_row(
                "SELECT COALESCE(p.date, e.date), COALESCE(p.payee, e.payee), p.amount
                 FROM book_postings p JOIN entries e ON e.id=p.entry_id WHERE p.id=?1",
                [id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Cents>(2)?)),
            )
            .optional()?;
        Ok(found.map(|(d, p, a)| format!("{}, {p}, {}", fmt_date(s2d(&d)), fmt_eur(a))).unwrap_or_default())
    }

    // ------------------------------------------------------------------ tags

    /// Gives a transaction these tags instead of those it has.
    pub fn set_tags(&self, id: i64, tags: &[String]) -> Result<()> {
        let all = store_tags(&parse_tags(&tags.join(","))?);
        self.atomically(|| {
            let entry = self.entry_of_transaction(id)?.ok_or_else(|| no_transaction(id))?;
            let mut d = self.load_draft(entry)?;
            for l in d.lines.iter_mut().filter(|l| l.id == Some(id)) {
                l.tags.clone_from(&all);
            }
            self.save_entry(&d)?;
            Ok(())
        })
    }

    /// Every tag, with how many transactions have it.
    pub fn tags(&self) -> Result<Vec<(String, usize)>> {
        let mut st = self.conn.prepare("SELECT tags FROM book_postings WHERE tags IS NOT NULL ORDER BY id")?;
        let mut all: BTreeMap<String, (String, usize)> = BTreeMap::new();
        for t in st.query_map([], |r| r.get::<_, String>(0))? {
            for tag in stored_tags(Some(&t?)) {
                all.entry(tag.to_lowercase()).or_insert((tag, 0)).1 += 1;
            }
        }
        Ok(all.into_values().collect())
    }
}

impl Undone {
    fn list(&self) -> String {
        let one = |s: &Skipped| {
            format!(
                "{} ({})",
                if s.transaction.is_empty() { s.id.to_string() } else { s.transaction.clone() },
                s.reason
            )
        };
        let mut out: Vec<String> = self.skipped.iter().take(LISTED).map(one).collect();
        if self.skipped.len() > LISTED {
            out.push("…".into());
        }
        out.join("; ")
    }

    /// What to tell people.
    pub fn message(&self) -> String {
        let mut out = trn(self.undone, "{n} transaction is as before", "{n} transactions are as before");
        if !self.skipped.is_empty() {
            out = format!("{out}; {}", trf("left as they are: {list}", &[("list", &self.list())]));
        }
        out
    }
}

/// What to tell people of the rules that were skipped.
pub fn broken_message(broken: &[Broken]) -> String {
    let all: Vec<String> =
        broken.iter().map(|b| trf("rule {id}: {what}", &[("id", &b.id), ("what", &b.error)])).collect();
    if all.is_empty() { String::new() } else { trf("rules skipped: {list}", &[("list", &all.join("; "))]) }
}

fn is_for_rules(t: &Transaction) -> bool {
    matches!(t.kind, TxKind::Category | TxKind::Uncategorized)
}

fn no_rule(id: i64) -> anyhow::Error {
    anyhow!(trf("rule {id} not found", &[("id", &id)]))
}

fn text(r: &Row, i: usize, sound: &mut bool) -> String {
    match r.get_ref(i) {
        Ok(ValueRef::Text(t)) => match std::str::from_utf8(t) {
            Ok(t) => return t.to_string(),
            Err(_) => *sound = false,
        },
        _ => *sound = false,
    }
    String::new()
}

fn number(r: &Row, i: usize, sound: &mut bool) -> Option<i64> {
    match r.get_ref(i) {
        Ok(ValueRef::Integer(n)) => return Some(n),
        Ok(ValueRef::Null) => {}
        _ => *sound = false,
    }
    None
}

#[cfg(test)]
mod tests;
