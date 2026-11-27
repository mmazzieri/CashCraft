//! The monthly statement of a credit card, from the text of its PDF. Columns come from the headings of
//! each page, figures from their labels; what is read is checked against the totals of the statement.

use crate::db::Cents;
use crate::i18n::tr;
use crate::money::{checked_total, fmt_machine, parse_amount_with, parse_date};
use crate::pdf::{Failure, Line, Page, Word};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// Lines of one row are at most this far from its date, in points.
const ROW_REACH: f64 = 10.0;
/// Far more than a month has.
const MAX_ROWS: usize = 5000;
const MAX_TEXT: usize = 1000;
/// What ends a list of movements.
const SECTIONS: [&str; 7] = [
    "PERRENDERE",
    "BOXFINANZIAMENTI",
    "SITUAZIONECONTABILE",
    "CONDIZIONIAPPLICATE",
    "RIEPILOGODEISUOI",
    "CARTANUMERO",
    "DETTAGLIODEISUOI",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckKind {
    Purchases,
    Instalments,
    Debit,
    FirstPage,
}

/// A total of the statement against what was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub kind: CheckKind,
    /// `None` when the statement does not say it.
    pub expected: Option<Cents>,
    pub found: Cents,
}

impl Check {
    pub fn ok(&self) -> bool {
        self.expected == Some(self.found)
    }

    pub fn code(&self) -> &'static str {
        match self.kind {
            CheckKind::Purchases => "purchases",
            CheckKind::Instalments => "instalments",
            CheckKind::Debit => "debit",
            CheckKind::FirstPage => "first_page",
        }
    }

    pub fn label(&self) -> &'static str {
        match self.kind {
            CheckKind::Purchases => tr("Purchases"),
            CheckKind::Instalments => tr("Instalments"),
            CheckKind::Debit => tr("Amount debited"),
            CheckKind::FirstPage => tr("Amount debited, on the first page"),
        }
    }
}

/// A row as it is imported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CardRow {
    pub date: NaiveDate,
    /// Negative for a purchase.
    pub amount: Cents,
    pub payee: String,
    pub memo: String,
    /// What tells it from the other rows of the account.
    pub key: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Statement {
    /// Last day of the statement.
    pub closing: Option<NaiveDate>,
    /// When the bank is debited.
    pub debit_date: Option<NaiveDate>,
    pub debit: Option<Cents>,
    /// The debt left after the debit, as the statement says it ("Debito residuo al" its last day).
    #[serde(default)]
    pub residual: Option<Cents>,
    /// Last digits of the cards.
    pub cards: Vec<String>,
    /// A heading of a card whose number could not be read.
    pub unread_card: bool,
    pub rows: Vec<CardRow>,
    /// Rows that look like movements but could not be read.
    pub unread: usize,
    pub checks: Vec<Check>,
}

impl Statement {
    /// Everything was read and adds up.
    pub fn is_sound(&self) -> bool {
        self.is_identified() && self.debit_date.is_some() && self.unread == 0 && self.checks.iter().all(Check::ok)
    }

    /// Whether what tells it from the other statements was read.
    pub fn is_identified(&self) -> bool {
        self.closing.is_some() && !self.cards.is_empty() && !self.unread_card
    }

    pub(crate) fn key(&self, closing: NaiveDate) -> String {
        format!("{closing}|{}", self.cards.join(","))
    }

    pub fn json(&self) -> serde_json::Value {
        let checks: Vec<_> = self
            .checks
            .iter()
            .map(|c| {
                serde_json::json!({"check": c.code(), "expected": c.expected.map(fmt_machine),
                                   "found": fmt_machine(c.found), "ok": c.ok()})
            })
            .collect();
        serde_json::json!({"closing_date": self.closing, "debit_date": self.debit_date,
            "debit": self.debit.map(fmt_machine), "cards": self.cards, "unread_rows": self.unread,
            "checks": checks, "identified": self.is_identified(), "sound": self.is_sound()})
    }
}

/// Letters and digits only, in capitals: spaces may have been lost.
fn compact(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_uppercase).collect()
}

fn line_text(words: &[Word]) -> String {
    words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
}

/// Where a label is in a line: its first word and the one after it.
fn find_label(words: &[Word], label: &str) -> Option<(usize, usize)> {
    for start in 0..words.len() {
        let mut text = String::new();
        // No label has more words than letters.
        for (i, w) in words.iter().enumerate().skip(start).take(label.len()) {
            let part = compact(&w.text);
            if i == start && part.is_empty() {
                break;
            }
            text.push_str(&part);
            if text.starts_with(label) {
                return Some((start, i + 1));
            }
            if !label.starts_with(&text) {
                break;
            }
        }
    }
    None
}

fn has(line: &Line, label: &str) -> bool {
    find_label(&line.words, label).is_some()
}

/// "1.234,56", with a minus before or after.
fn amount(text: &str) -> Option<Cents> {
    let t = text.replace(['\u{2212}', '\u{2013}'], "-");
    let t = t.trim_matches('*').trim_start_matches('€').trim_end_matches('€');
    let bare = t.strip_prefix('-').or_else(|| t.strip_suffix('-')).unwrap_or(t);
    let (units, cents) = bare.split_once(',')?;
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '.');
    if cents.len() != 2 || !cents.chars().all(|c| c.is_ascii_digit()) || !digits(units) || units.starts_with('.') {
        return None;
    }
    parse_amount_with(t, Some(true)).ok()
}

fn is_minus(w: &Word) -> bool {
    matches!(w.text.as_str(), "-" | "\u{2212}" | "\u{2013}")
}

/// The amounts of some words; the minus may be a word of its own.
fn amounts(words: &[Word]) -> Vec<(&Word, Cents)> {
    let near = |a: &Word, b: &Word| b.x0 - a.x1 < 12.0;
    let mut out = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let Some(mut v) = amount(&w.text) else { continue };
        let before = i.checked_sub(1).and_then(|p| words.get(p)).is_some_and(|p| is_minus(p) && near(p, w));
        let after = words.get(i + 1).is_some_and(|n| is_minus(n) && near(w, n));
        if before || after {
            v = v.saturating_neg();
        }
        out.push((w, v));
    }
    out
}

/// "24/11/26": the century is this one.
fn day(text: &str) -> Option<NaiveDate> {
    let mut parts = text.split('/');
    let (d, m, y) = (parts.next()?, parts.next()?, parts.next()?);
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if parts.next().is_some() || !matches!(y.len(), 2 | 4) || ![d, m, y].iter().all(|s| digits(s)) {
        return None;
    }
    let year: i32 = y.parse().ok()?;
    NaiveDate::from_ymd_opt(if y.len() == 2 { 2000 + year } else { year }, m.parse().ok()?, d.parse().ok()?)
}

fn looks_like_day(text: &str) -> bool {
    text.matches('/').count() == 2 && text.chars().all(|c| c.is_ascii_digit() || c == '/')
}

/// Where the columns of a list start.
struct Columns {
    date: f64,
    description: f64,
    amount: f64,
    foreign: f64,
    rate: f64,
    bottom: f64,
}

/// The headings of a list, in the lines after `from`.
fn columns(page: &Page, from: usize) -> Option<Columns> {
    for line in page.iter().skip(from).take(8) {
        let find = |label: &str, after: f64| {
            // A heading may be on two lines.
            let mut x = None;
            for l in page.iter().skip(from).filter(|l| (l.y - line.y).abs() < 7.0) {
                for w in l.words.iter().filter(|w| w.x0 > after && compact(&w.text).starts_with(label)) {
                    x = Some(x.map_or(w.x0, |o: f64| o.min(w.x0)));
                }
            }
            x
        };
        let Some(date) = line.words.iter().find(|w| compact(&w.text).starts_with("DATA")).map(|w| w.x0) else {
            continue;
        };
        let Some(description) = find("DESCRIZIONE", date) else { continue };
        let Some(amount) = find("IMPORTO", description) else { continue };
        let foreign = find("IMPORTO", amount + 20.0).unwrap_or(f64::INFINITY);
        let rate = find("CAMBIO", amount).unwrap_or(f64::INFINITY);
        return Some(Columns { date, description, amount, foreign, rate, bottom: line.y + 4.0 });
    }
    None
}

struct Row {
    date: NaiveDate,
    /// As written: purchases are positive.
    amount: Cents,
    description: String,
    note: String,
}

/// The rows of a list: a date and an amount on one line, with the lines just above and below.
fn rows(lines: &[&Line], c: &Columns) -> (Vec<Row>, usize) {
    let euro = |l: &Line| -> Option<(f64, Cents)> {
        let found = amounts(&l.words);
        let mut hits = found.iter().filter(|(w, _)| w.x1 > c.amount && w.x0 < c.foreign - 5.0);
        hits.next_back().map(|(w, v)| (w.x0, *v))
    };
    let date_word = |l: &Line| {
        l.words.iter().find(|w| (w.x0 - c.date).abs() <= 10.0 && looks_like_day(&w.text)).map(|w| w.text.clone())
    };
    let mut unread = 0;
    let mut anchors: Vec<(usize, NaiveDate, f64, Cents)> = Vec::new();
    let mut loose: Vec<usize> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        match (date_word(l), euro(l)) {
            (Some(d), Some((x, v))) => match day(&d) {
                Some(d) => anchors.push((i, d, x, v)),
                None => unread += 1,
            },
            (Some(_), None) => unread += 1,
            _ => loose.push(i),
        }
    }
    let y = |i: usize| lines.get(i).map_or(0.0, |l| l.y);
    let mut own: Vec<Vec<usize>> = anchors.iter().map(|a| vec![a.0]).collect();
    for i in loose {
        let nearest = anchors
            .iter()
            .enumerate()
            .map(|(n, a)| (n, (y(a.0) - y(i)).abs()))
            .filter(|(_, d)| *d <= ROW_REACH)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        match nearest.and_then(|(n, _)| own.get_mut(n)) {
            Some(o) => o.push(i),
            None => unread += usize::from(lines.get(i).is_some_and(|l| euro(l).is_some())),
        }
    }
    let mut out = Vec::new();
    for ((anchor, date, amount_x, value), mut mine) in anchors.into_iter().zip(own) {
        mine.sort_unstable();
        let (mut description, mut note) = (Vec::new(), Vec::new());
        for i in mine {
            let Some(l) = lines.get(i) else { continue };
            let end = if i == anchor { amount_x } else { c.foreign };
            let mut words: Vec<Word> =
                l.words.iter().filter(|w| w.x0 >= c.description - 6.0 && w.x0 < end).cloned().collect();
            while i == anchor && words.last().is_some_and(is_minus) {
                words.pop();
            }
            let text = line_text(&words);
            if compact(&text).starts_with("COMMISSIONE") && i != anchor {
                note.push(text.trim_end_matches('*').to_string());
            } else if !text.is_empty() {
                if i != anchor && euro(l).is_some() {
                    // An amount without a date is not part of a description.
                    unread += 1;
                }
                description.push(text);
            }
            if i == anchor {
                let part = |from: f64, to: f64| {
                    let w: Vec<Word> =
                        l.words.iter().filter(|w| w.x0 >= from - 5.0 && w.x0 < to - 5.0).cloned().collect();
                    line_text(&w)
                };
                let (foreign, rate) = (part(c.foreign, c.rate), part(c.rate, f64::INFINITY));
                if !foreign.is_empty() {
                    note.insert(0, if rate.is_empty() { foreign } else { format!("{foreign}, cambio {rate}") });
                }
            }
        }
        out.push(Row { date, amount: value, description: description.join(" "), note: note.join(" — ") });
    }
    (out, unread)
}

/// The lines of a page after a list's headings, up to what follows the list.
fn body<'a>(page: &'a Page, from: usize, c: &Columns, ends: &[&str]) -> Vec<&'a Line> {
    page.iter()
        .skip(from)
        .filter(|l| l.y > c.bottom)
        .take_while(|l| !SECTIONS.iter().chain(ends).any(|s| has(l, s)))
        .collect()
}

/// The figure beside a label of the summary.
fn figure(summary: &[&Line], label: &str) -> Option<Cents> {
    for l in summary {
        let Some((_, next)) = find_label(&l.words, label) else { continue };
        if let Some((_, v)) = amounts(l.words.get(next..).unwrap_or_default()).first() {
            return Some(*v);
        }
        // The baseline of the figure may be another one.
        let end = l.words.get(next - 1).map_or(0.0, |w| w.x1);
        let beside = summary.iter().filter(|o| (o.y - l.y).abs() < 5.0).find_map(|o| {
            let right: Vec<Word> = o.words.iter().filter(|w| w.x0 > end).cloned().collect();
            amounts(&right).first().map(|a| a.1)
        });
        if beside.is_some() {
            return beside;
        }
    }
    None
}

/// The worded date nearest to "In data", beside it or below.
fn debit_date(page: &Page) -> Option<NaiveDate> {
    let mut best: Option<(f64, NaiveDate)> = None;
    for l in page {
        let Some(label) = find_label(&l.words, "INDATA").and_then(|(i, _)| l.words.get(i)) else { continue };
        for o in page.iter().filter(|o| o.y > l.y - 6.0 && o.y < l.y + 80.0) {
            for three in o.words.windows(3) {
                let [first, ..] = three else { continue };
                if let Some(d) = parse_date(&line_text(three)) {
                    let distance = (o.y - l.y).abs() * 4.0 + (first.x0 - label.x0).abs();
                    if best.is_none_or(|b| distance < b.0) {
                        best = Some((distance, d));
                    }
                }
            }
        }
    }
    best.map(|b| b.1)
}

/// On the first page, the amount under the heading of the debit, or beside it.
fn headed_figure(page: &Page) -> Option<Cents> {
    let (y, x0, x1) = page.iter().find_map(|l| {
        let (from, to) = find_label(&l.words, "QUESTOMESELESARANNOADDEBITATI")?;
        Some((l.y, l.words.get(from)?.x0, l.words.get(to - 1)?.x1))
    })?;
    let mut best: Option<(f64, Cents)> = None;
    for l in page.iter().filter(|l| l.y > y - 4.0 && l.y < y + 120.0) {
        for (w, v) in amounts(&l.words) {
            let beside = (l.y - y).abs() < 4.0 && w.x0 > x1;
            let below = l.y >= y + 4.0 && w.x1 > x0 - 20.0 && w.x0 < x1 + 20.0;
            let distance = (l.y - y).abs() + if beside { w.x0 - x1 } else { 0.0 };
            if (beside || below) && best.is_none_or(|b| distance < b.0) {
                best = Some((distance, v));
            }
        }
    }
    best.map(|b| b.1)
}

pub fn parse(pages: &[Page]) -> Result<Statement, Failure> {
    let sum = |v: &[Cents]| checked_total(v.iter().copied()).ok_or(Failure::Unknown);
    let mut s = Statement::default();
    // The summary ends where the list of the movements starts.
    let mut summary: Vec<&Line> = Vec::new();
    let mut found_summary = false;
    'pages: for page in pages {
        for l in page {
            if found_summary && (has(l, "CARTANUMERO") || has(l, "DETTAGLIODEISUOIMOVIMENTI")) {
                break 'pages;
            }
            found_summary |= has(l, "RIEPILOGODEISUOIMOVIMENTI");
            if found_summary {
                summary.push(l);
            }
        }
    }
    let detailed = pages.iter().flatten().any(|l| has(l, "DETTAGLIODEISUOIMOVIMENTI"));
    if !found_summary || !detailed {
        return Err(Failure::Unknown);
    }
    for l in &summary {
        // The date can be glued to the word before it.
        if let Some((start, _)) = find_label(&l.words, "DEBITORESIDUOAL") {
            let text: String = l.words.iter().skip(start).map(|w| w.text.as_str()).collect();
            let date = text.char_indices().find(|(_, c)| c.is_ascii_digit()).and_then(|(i, _)| text.get(i..i + 10));
            if let Some(d) = date.filter(|d| d.matches('/').count() == 2).and_then(crate::money::parse_date) {
                s.closing = Some(d);
            }
        }
    }
    let closing_label = s.closing.map(|d| format!("DEBITORESIDUOAL{}", d.format("%d%m%Y")));
    let previous = figure(&summary, "DEBITORESIDUOALMESEPRECEDENTE").unwrap_or(0);
    s.residual = closing_label.and_then(|l| figure(&summary, &l));
    let residual = s.residual.unwrap_or(0);
    let duty = figure(&summary, "IMPOSTADIBOLLO").unwrap_or(0);
    s.debit = figure(&summary, "TOTALEADDEBITO");
    let first = pages.first();
    s.debit_date = pages.first().and_then(debit_date).filter(|d| {
        let Some(closing) = s.closing else { return true };
        (-5..=70).contains(&(*d - closing).num_days())
    });

    let (mut purchases, mut instalments, mut totals) = (Vec::new(), Vec::new(), Vec::new());
    for page in pages {
        for (i, l) in page.iter().enumerate() {
            if let Some((_, next)) = find_label(&l.words, "CARTANUMERO")
                && l.words.iter().any(|w| w.text.contains('*') || compact(&w.text).starts_with("SCADENZA"))
            {
                // The stars may be words of their own.
                let rest = l.words.get(next - 1..).unwrap_or_default().iter();
                let text: String = rest
                    .map(|w| compact(&w.text))
                    .take_while(|w| !w.starts_with("ASALDO") && w != "A" && !w.starts_with("SCADENZA"))
                    .collect();
                let number: String = text.chars().skip_while(|c| !c.is_ascii_digit()).collect();
                let digits = number.chars().take_while(char::is_ascii_digit).count();
                match number.get(digits.saturating_sub(4)..digits).filter(|n| n.len() == 4) {
                    Some(n) if s.cards.iter().any(|c| c == n) => {}
                    Some(n) => s.cards.push(n.into()),
                    None => s.unread_card = true,
                }
            }
            if has(l, "DETTAGLIODEISUOIMOVIMENTI")
                && let Some(c) = columns(page, i + 1)
            {
                let lines = body(page, i + 1, &c, &["TOTALESPESE"]);
                let end = page.iter().skip(i + 1).filter(|l| l.y > c.bottom).nth(lines.len());
                if let Some(e) = end
                    && let Some(v) = figure(&[e], "TOTALESPESE")
                {
                    totals.push(v);
                }
                let (found, unread) = rows(&lines, &c);
                purchases.extend(found);
                s.unread += unread;
            }
            if has(l, "BOXFINANZIAMENTI")
                && let Some(c) = columns(page, i + 1)
            {
                let (found, unread) = rows(&body(page, i + 1, &c, &["TOTALE"]), &c);
                s.unread += unread;
                for r in found {
                    if compact(&r.description).starts_with("RATA") {
                        instalments.push(r);
                    } else {
                        s.unread += 1;
                    }
                }
            }
        }
    }
    let spent = sum(&purchases.iter().map(|r| r.amount).collect::<Vec<_>>())?;
    let financed = sum(&instalments.iter().map(|r| r.amount).collect::<Vec<_>>())?;
    let said = match totals.as_slice() {
        [] => figure(&summary, "TOTALESPESECONCARTEASALDO"),
        t => Some(sum(t)?),
    };
    s.checks.push(Check { kind: CheckKind::Purchases, expected: said, found: spent });
    let said = figure(&summary, "TOTALECOMPLESSIVOFINANZIAMENTI");
    if said.is_some() || financed != 0 {
        s.checks.push(Check { kind: CheckKind::Instalments, expected: said, found: financed });
    }
    let found = sum(&[spent, financed, duty, previous, residual.saturating_neg()])?;
    s.checks.push(Check { kind: CheckKind::Debit, expected: s.debit, found });
    if let (Some(said), Some(debit)) = (first.and_then(headed_figure), s.debit) {
        s.checks.push(Check { kind: CheckKind::FirstPage, expected: Some(debit), found: said });
    }
    // Without a date nothing is imported: this one only lets the rows be shown.
    let date = s.closing.or(s.debit_date).unwrap_or_default();
    if duty != 0 {
        purchases.push(Row { date, amount: duty, description: "Imposta di bollo".into(), note: String::new() });
    }
    if purchases.len() + instalments.len() > MAX_ROWS {
        return Err(Failure::TooLarge);
    }
    let of = format!("card|{date}|{}", s.cards.join(","));
    let short = |t: &str| t.chars().take(MAX_TEXT).collect::<String>();
    for r in purchases.into_iter().chain(instalments) {
        // Without spaces, so that reading them better one day does not change the key.
        let key = short(&format!("{of}|{}|{}|{}", r.date, r.amount, compact(&r.description).to_lowercase()));
        let amount = r.amount.saturating_neg();
        let (payee, memo) = (short(&r.description), short(&r.note));
        s.rows.push(CardRow { date: r.date, amount, payee, memo, key });
    }
    if s.rows.is_empty() && s.debit.is_none() {
        return Err(Failure::Unknown);
    }
    s.rows.sort_by_key(|r| r.date);
    Ok(s)
}

#[cfg(test)]
pub(crate) mod tests;
