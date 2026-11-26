//! The list of the movements of a current account, printed to PDF by the web site of its bank: a heading
//! of columns (booking date, value date, amount, description), a row for each movement, whose description
//! may go on over several lines and start a little above its date, and the balances at the start and at
//! the end. Its rows become those of a table, as from a CSV file; the balances, where the list has them,
//! prove that every row was read.

use crate::db::Cents;
use crate::money::{checked_total, parse_amount_with, parse_date};
use crate::pdf::{Failure, Line, Page, Word};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// The headings of the table that is made, which the importer recognises.
pub const HEADINGS: [&str; 4] = ["Data contabile", "Data valuta", "Importo", "Descrizione"];
/// The first line of a description may be this far above its date, in points.
const LEAD: f64 = 6.0;
/// Far more than any list has.
const MAX_ROWS: usize = 20_000;
const MAX_TEXT: usize = 2000;

/// The rows of the list, oldest first, as text in the columns of [`HEADINGS`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Movements {
    pub rows: Vec<[String; 4]>,
    /// The balances at the start and at the end, when the list states them.
    pub opening: Option<Cents>,
    pub closing: Option<Cents>,
    /// The day of the closing balance.
    #[serde(default)]
    pub closing_on: Option<NaiveDate>,
    /// The account, as the heading of the list names it: empty when it does not.
    #[serde(default)]
    pub bank: String,
    /// Without spaces, and only when its check digits are right.
    #[serde(default)]
    pub iban: String,
}

impl Movements {
    /// The table that the importer reads: the headings, then the rows.
    pub fn records(&self) -> Vec<Vec<String>> {
        let head = HEADINGS.iter().map(|h| h.to_string()).collect();
        std::iter::once(head).chain(self.rows.iter().map(|r| r.to_vec())).collect()
    }
}

/// An IBAN written with or without spaces, in capitals, when its check digits are right (ISO 13616).
pub fn iban(s: &str) -> Option<String> {
    let c = compact(s);
    let mut chars = c.chars();
    let country = chars.by_ref().take(2).all(|ch| ch.is_ascii_uppercase());
    let digits = chars.take(2).all(|ch| ch.is_ascii_digit());
    if !(15..=34).contains(&c.len()) || !c.is_ascii() || !country || !digits {
        return None;
    }
    let (head, tail) = c.split_at(4);
    let mut rest = 0u32;
    for ch in tail.chars().chain(head.chars()) {
        let v = ch.to_digit(36)?;
        rest = if v < 10 { (rest * 10 + v) % 97 } else { (rest * 100 + v) % 97 };
    }
    (rest == 1).then_some(c)
}

impl Movements {
    /// The account with the IBAN of the list, if there is one that is not archived.
    pub fn account<'a>(&self, accounts: &'a [crate::db::Account]) -> Option<&'a crate::db::Account> {
        let mine = Some(self.iban.as_str()).filter(|i| !i.is_empty());
        accounts.iter().find(|a| !a.archived && iban(&a.iban).as_deref() == mine)
    }
    /// The name proposed for a new account: the bank, or the last digits of the IBAN.
    pub fn account_name(&self) -> Option<String> {
        if !self.bank.is_empty() {
            return Some(self.bank.clone());
        }
        let digits = self.iban.get(self.iban.len().saturating_sub(4)..).filter(|d| !d.is_empty())?;
        Some(crate::i18n::trf("Account {digits}", &[("digits", &digits)]))
    }
    pub fn json(&self, accounts: &[crate::db::Account]) -> serde_json::Value {
        serde_json::json!({"bank": self.bank, "iban": self.iban,
            "opening": self.opening.map(crate::money::fmt_machine), "closing": self.closing.map(crate::money::fmt_machine),
            "closing_on": self.closing_on, "account": self.account(accounts).map(|a| &a.name),
            "new_account_name": self.account_name()})
    }
}

/// What the lines above the table say of the account: the name of the bank, on the first line, and the IBAN.
fn heading(lines: &[Line]) -> (String, String) {
    let text = |l: &Line| l.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
    let bank = lines
        .first()
        .map(text)
        .filter(|t| t.chars().count() <= 60 && !t.chars().any(|c| c.is_ascii_digit()))
        .unwrap_or_default();
    let iban = lines
        .iter()
        .find_map(|l| {
            let (first, rest) = l.words.split_first()?;
            (compact(&first.text) == "IBAN").then(|| rest.iter().map(|w| w.text.as_str()).collect::<String>())
        })
        .and_then(|t| iban(&t))
        .unwrap_or_default();
    (bank.trim().to_string(), iban)
}

/// Letters and digits only, in capitals: spaces may have been lost.
fn compact(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_uppercase).collect()
}

/// Where the columns start, from their headings.
#[derive(Clone, Copy, Debug)]
struct Columns {
    /// Left of the value date.
    value: f64,
    /// Right of the value date's heading: the amounts start after it.
    amount: f64,
    /// Left of the description.
    text: f64,
}

fn columns(line: &Line) -> Option<Columns> {
    let words: Vec<String> = line.words.iter().map(|w| compact(&w.text)).collect();
    let at = |name: &str| words.iter().position(|w| w == name);
    let (contabile, valuta, importo, descrizione) =
        (at("CONTABILE")?, at("VALUTA")?, at("IMPORTO")?, at("DESCRIZIONE")?);
    let word = |i: usize| line.words.get(i);
    // "Data" before "valuta".
    let value = valuta.checked_sub(1).filter(|i| words.get(*i).is_some_and(|w| w == "DATA") && *i > contabile)?;
    let ok = contabile < value && valuta < importo && importo < descrizione;
    ok.then_some(Columns { value: word(value)?.x0, amount: word(valuta)?.x1, text: word(descrizione)?.x0 })
}

/// The words of a line of the body, column by column.
#[derive(Default)]
struct Cells {
    booking: Vec<String>,
    value: Vec<String>,
    amount: Vec<String>,
    text: Vec<String>,
}

fn cells(words: &[Word], c: &Columns) -> Cells {
    let mut out = Cells::default();
    for w in words {
        let cell = if w.x0 >= c.text - 2.0 {
            &mut out.text
        } else if w.x0 < c.value - 5.0 {
            &mut out.booking
        } else if w.x0 < c.amount + 10.0 {
            &mut out.value
        } else {
            &mut out.amount
        };
        cell.push(w.text.clone());
    }
    out
}

/// A description longer than any bank writes is not cut: the list is refused. Counted as it is joined.
fn too_long(text: &[String]) -> bool {
    text.iter().map(|t| t.chars().count() + 1).sum::<usize>() > MAX_TEXT + 1
}

struct Row {
    booking: String,
    value: String,
    amount: Cents,
    /// As the bank wrote it.
    figure: String,
    text: Vec<String>,
}

pub fn parse(pages: &[Page]) -> Result<Movements, Failure> {
    let mut rows: Vec<Row> = Vec::new();
    let (mut opening, mut closing, mut unread, mut found) = (None, None, 0usize, false);
    let (mut closing_on, mut bank, mut iban) = (None, String::new(), String::new());
    for page in pages {
        let Some(start) = page.iter().position(|l| columns(l).is_some()) else { continue };
        let Some(c) = page.get(start).and_then(columns) else { continue };
        if !found {
            (bank, iban) = heading(page.get(..start).unwrap_or_default());
        }
        found = true;
        // A description that starts above its date waits for it.
        let mut waiting: Vec<String> = Vec::new();
        let mut current: Option<usize> = None;
        let body = page.get(start + 1..).unwrap_or_default();
        for (i, l) in body.iter().enumerate() {
            let here = cells(&l.words, &c);
            if here.booking.is_empty() {
                if !here.value.is_empty() || !here.amount.is_empty() {
                    // A movement whose booking date was lost, or the foot of the page.
                    let date = parse_date(&here.value.join(" ")).is_some();
                    if date || parse_amount_with(&here.amount.join(""), Some(true)).is_ok() {
                        unread += 1;
                        waiting.clear();
                        current = None;
                        continue;
                    }
                    break;
                }
                let next = body.get(i + 1);
                let leads = next.is_some_and(|n| n.y - l.y <= LEAD && !cells(&n.words, &c).booking.is_empty());
                match current.and_then(|r| rows.get_mut(r)) {
                    Some(r) if !leads => {
                        r.text.extend(here.text);
                        if too_long(&r.text) {
                            return Err(Failure::TooLarge);
                        }
                    }
                    _ => waiting.extend(here.text),
                }
                continue;
            }
            let booking = here.booking.join(" ");
            let value = here.value.join(" ");
            let figure = here.amount.join("");
            let amount = parse_amount_with(&figure, Some(true)).ok();
            let (Some(on), Some(amount)) = (parse_date(&booking), amount) else {
                unread += 1;
                waiting.clear();
                current = None;
                continue;
            };
            let mut text = std::mem::take(&mut waiting);
            text.extend(here.text);
            if too_long(&text) {
                return Err(Failure::TooLarge);
            }
            let head = compact(&text.join(""));
            if value.is_empty() && head.starts_with("SALDOINIZIALE") {
                opening = Some(amount);
                current = None;
            } else if value.is_empty() && head.starts_with("SALDOFINALE") {
                closing = Some(amount);
                closing_on = Some(on);
                current = None;
            } else {
                if rows.len() >= MAX_ROWS {
                    return Err(Failure::TooLarge);
                }
                rows.push(Row { booking, value, amount, figure, text });
                current = Some(rows.len() - 1);
            }
        }
        // Lines left waiting have no date: they belong to nothing that was read.
        if !waiting.is_empty() {
            unread += 1;
        }
    }
    if !found {
        return Err(Failure::Unknown);
    }
    let total = checked_total(rows.iter().map(|r| r.amount)).ok_or(Failure::Mismatch)?;
    let adds_up = match (opening, closing) {
        (Some(o), Some(c)) => o.checked_add(total) == Some(c),
        _ => true,
    };
    if unread > 0 || !adds_up {
        return Err(Failure::Mismatch);
    }
    let mut out = Movements { rows: Vec::new(), opening, closing, closing_on, bank, iban };
    // The newest comes first in the list.
    for r in rows.into_iter().rev() {
        out.rows.push([r.booking, r.value, r.figure, r.text.join(" ")]);
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests;
