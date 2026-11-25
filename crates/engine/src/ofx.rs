//! Bank and credit card statements in OFX, written from the public specification: 1.x is SGML, where the
//! end tag of a value may be missing, 2.x is XML. One reader for both, tolerant of what banks leave out
//! and strict about money: what is wrong with a transaction refuses the file, with the line.

use crate::db::Cents;
use crate::i18n::{tr, trf};
use crate::importer::decode;
use crate::money::{MAX_AMOUNT, fmt_machine};
use anyhow::{Result, anyhow, bail};
use chrono::NaiveDate;
use encoding_rs::{Encoding, UTF_8, WINDOWS_1252};
use serde::Serialize;
use std::collections::HashMap;
use std::fmt::Display;

const MAX_STATEMENTS: usize = 100;
const MAX_TRANSACTIONS: usize = 100_000;
/// Elements inside one another.
const MAX_DEPTH: usize = 200;
/// Characters of a text that is kept.
const MAX_TEXT: usize = 2_000;
/// Where the declarations of the file are.
const HEAD: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Bank,
    Savings,
    CreditCard,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OfxRow {
    pub line: usize,
    pub date: NaiveDate,
    pub amount: Cents,
    pub name: String,
    pub memo: String,
    pub fitid: Option<String>,
    pub corrects: Option<Correction>,
    /// What its type says of the sign, when it says something: whether money goes out.
    pub goes_out: Option<bool>,
}

/// The FITID of the transaction that this one replaces, or deletes.
#[derive(Clone, Debug, PartialEq)]
pub struct Correction {
    pub fitid: String,
    pub delete: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OfxStatement {
    pub kind: Kind,
    pub account: String,
    /// In capitals, as it is written: three letters.
    pub currency: String,
    /// What the bank says the balance is, and on which day. Nothing but a check.
    pub balance: Option<(Cents, Option<NaiveDate>)>,
    pub rows: Vec<OfxRow>,
}

impl OfxStatement {
    /// The account as it is shown: its last digits.
    pub fn account_label(&self) -> String {
        let n = self.account.chars().count();
        if n <= 4 {
            return self.account.clone();
        }
        format!("…{}", self.account.chars().skip(n - 4).collect::<String>())
    }

    /// Whether most of the types say the opposite of the signs: the bank may write expenses as positive.
    pub fn signs_contradict(&self) -> bool {
        let said = self.rows.iter().filter(|r| r.amount != 0).filter_map(|r| Some(r.goes_out? == (r.amount < 0)));
        let (agree, not): (Vec<bool>, Vec<bool>) = said.partition(|agrees| *agrees);
        not.len() > agree.len()
    }

    pub fn first_date(&self) -> Option<NaiveDate> {
        self.rows.iter().map(|r| r.date).min()
    }

    pub fn last_date(&self) -> Option<NaiveDate> {
        self.rows.iter().map(|r| r.date).max()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ofx {
    pub statements: Vec<OfxStatement>,
    /// The statement to read. When there are several the user chooses.
    pub chosen: Option<usize>,
}

impl Ofx {
    pub fn statement(&self) -> Option<&OfxStatement> {
        self.statements.get(self.chosen?)
    }

    /// Chooses a statement by the number of its account, when only one has it, or by its place, from 1.
    pub fn choose(&mut self, key: &str) -> Result<()> {
        let key = key.trim();
        let mut same = self.statements.iter().enumerate().filter(|(_, s)| s.account == key).map(|(i, _)| i);
        let place = || key.parse::<usize>().ok()?.checked_sub(1).filter(|i| *i < self.statements.len());
        let found = match (same.next(), same.next()) {
            (Some(i), None) => Some(i),
            (None, _) => place(),
            _ => None,
        };
        if found.is_none() {
            bail!(trf(
                "\"{key}\" is not one of the statements of the file: {list}",
                &[("key", &key), ("list", &self.list())]
            ));
        }
        self.chosen = found;
        Ok(())
    }

    /// The statements, for people to choose.
    pub fn list(&self) -> String {
        let one = |(i, s): (usize, &OfxStatement)| {
            let range = match (s.first_date(), s.last_date()) {
                (Some(a), Some(b)) => format!(", {a} – {b}"),
                _ => String::new(),
            };
            format!("{}: {} {} ({}{range})", i + 1, s.account_label(), s.currency, s.rows.len())
        };
        self.statements.iter().enumerate().map(one).collect::<Vec<_>>().join("; ")
    }

    pub fn json(&self) -> serde_json::Value {
        let one = |(i, s): (usize, &OfxStatement)| {
            let balance = s.balance.map(|(c, d)| serde_json::json!({"stated": fmt_machine(c), "date": d}));
            serde_json::json!({"n": i + 1, "kind": s.kind, "account": s.account_label(), "currency": s.currency,
                "transactions": s.rows.len(), "first_date": s.first_date(), "last_date": s.last_date(),
                "ledger_balance": balance, "signs_contradict_types": s.signs_contradict()})
        };
        let all: Vec<_> = self.statements.iter().enumerate().map(one).collect();
        serde_json::json!({"statements": all, "chosen": self.chosen.map(|i| i + 1)})
    }
}

fn starts_with(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len()).is_some_and(|t| t.eq_ignore_ascii_case(prefix))
}

/// The declarations at the start of the file, whatever its encoding, and whether a mark gives the encoding.
fn head(bytes: &[u8]) -> (String, bool) {
    let first = bytes.get(..HEAD).unwrap_or(bytes);
    match Encoding::for_bom(bytes) {
        Some((e, _)) => (e.decode(first).0.trim_start().to_string(), true),
        None => (first.iter().map(|b| char::from(*b)).collect::<String>().trim_start().to_string(), false),
    }
}

pub fn is_ofx(bytes: &[u8]) -> bool {
    let (head, _) = head(bytes);
    let upper = head.to_ascii_uppercase();
    let root =
        |t: &str| t.strip_prefix("<OFX").is_some_and(|r| r.starts_with('>') || r.starts_with(char::is_whitespace));
    let anywhere = || upper.match_indices("<OFX").any(|(i, _)| upper.get(i..).is_some_and(root));
    starts_with(&upper, "OFXHEADER")
        || root(&upper)
        || starts_with(&upper, "<?OFX")
        || (upper.starts_with("<?XML") && (upper.contains("<?OFX") || anywhere()))
}

/// What follows `key` and the sign in the declarations: `CHARSET:1252`, `encoding="UTF-8"`.
fn declared(head: &str, key: &str) -> Option<String> {
    let upper = head.to_ascii_uppercase();
    let at = upper.find(key)?;
    let rest = upper.get(at + key.len()..)?.trim_start_matches([' ', '=', ':', '"', '\'']);
    let end = rest.find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '?' | '<')).unwrap_or(rest.len());
    // Some banks write the header in one line, without spaces.
    let mut value = rest.get(..end)?;
    for next in ["CHARSET:", "COMPRESSION:", "OLDFILEUID:", "NEWFILEUID:"] {
        value = value.split(next).next().unwrap_or(value);
    }
    Some(value.to_string())
}

/// The text of the file. A character set that is declared and does not fit is not believed.
fn text_of(bytes: &[u8]) -> (String, &'static str) {
    let (head, marked) = head(bytes);
    if marked {
        return decode(bytes);
    }
    let end = head.find("<OFX>").unwrap_or(head.len());
    let head = head.get(..end).unwrap_or(&head);
    let wanted = if starts_with(head, "<?xml") {
        declared(head, "ENCODING").and_then(|l| Encoding::for_label(l.as_bytes()))
    } else {
        let utf8 = declared(head, "ENCODING:").is_some_and(|e| e.starts_with("UTF") || e == "UNICODE");
        match declared(head, "CHARSET:").as_deref() {
            _ if utf8 => Some(UTF_8),
            Some("1252" | "8859-1" | "ISO-8859-1" | "WINDOWS-1252") => Some(WINDOWS_1252),
            Some(label) => Encoding::for_label(label.as_bytes())
                .or_else(|| Encoding::for_label(format!("windows-{label}").as_bytes())),
            None => None,
        }
    };
    // Bytes beyond ASCII that make sense as UTF-8 are not of a set of one byte.
    let utf8 = !bytes.is_ascii() && std::str::from_utf8(bytes).is_ok();
    match wanted {
        // Without a mark at the start the file cannot be in 16 bits: its tags were read.
        Some(e) if !utf8 && e != UTF_8 && e.is_ascii_compatible() => {
            let (text, e, _) = e.decode(bytes);
            (text.into_owned(), e.name())
        }
        _ => decode(bytes),
    }
}

fn at_line(line: usize, text: impl Display) -> anyhow::Error {
    anyhow!(trf("line {line}: {error}", &[("line", &line), ("error", &text)]))
}

enum Token<'a> {
    Open(&'a str),
    Close(&'a str),
    Text(&'a str, bool),
    /// An element without a value.
    Empty,
    End,
}

/// Tags and text, with the line where each starts. Nothing is expanded here.
struct Tokens<'a> {
    rest: &'a str,
    line: usize,
}

impl<'a> Tokens<'a> {
    fn take(&mut self, n: usize) -> &'a str {
        let (taken, rest) = self.rest.split_at_checked(n).unwrap_or((self.rest, ""));
        self.rest = rest;
        self.line = self.line.saturating_add(lines(taken));
        taken
    }

    /// What is before `end`, which is passed over.
    fn until(&mut self, end: &str) -> Result<&'a str> {
        let Some(n) = self.rest.find(end) else {
            return Err(at_line(self.line, trf("\"{end}\" is missing", &[("end", &end)])));
        };
        let body = self.take(n);
        self.take(end.len());
        Ok(body)
    }

    fn next(&mut self) -> Result<(usize, Token<'a>)> {
        loop {
            let line = self.line;
            if self.rest.is_empty() {
                return Ok((line, Token::End));
            }
            if self.rest.starts_with("<?") {
                self.until("?>")?;
            } else if self.rest.starts_with("<!--") {
                self.until("-->")?;
            } else if self.rest.starts_with("<![CDATA[") {
                self.take("<![CDATA[".len());
                return Ok((line, Token::Text(self.until("]]>")?, true)));
            } else if self.rest.starts_with("<!") {
                return Err(at_line(line, tr("declarations are not accepted in an OFX file")));
            } else if is_tag(self.rest) {
                self.take(1);
                let body = self.until(">")?;
                if body.contains('<') {
                    return Err(at_line(line, trf("\"{end}\" is missing", &[("end", &">")])));
                }
                let name = |s: &'a str| s.split(char::is_whitespace).next().unwrap_or(s);
                return Ok((
                    line,
                    match body.strip_prefix('/') {
                        Some(n) => Token::Close(name(n)),
                        None if body.ends_with('/') => Token::Empty,
                        None => Token::Open(name(body)),
                    },
                ));
            } else {
                // Up to the next tag: a "<" that starts none is text.
                let mut from = self.rest.chars().next().map_or(1, char::len_utf8);
                let n = loop {
                    let Some(i) = self.rest.get(from..).and_then(|r| r.find('<')) else { break self.rest.len() };
                    let at = from + i;
                    if self.rest.get(at..).is_some_and(|r| is_tag(r) || r.starts_with("<!") || r.starts_with("<?")) {
                        break at;
                    }
                    from = at + 1;
                };
                return Ok((line, Token::Text(self.take(n), false)));
            }
        }
    }
}

/// Whether a tag starts here. A "<" in a text is told from one: a name that is not of OFX, followed by
/// something other than its end or an attribute, is text.
fn is_tag(s: &str) -> bool {
    let Some(body) = s.strip_prefix('<') else { return false };
    let body = body.strip_prefix('/').unwrap_or(body);
    if !body.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return false;
    }
    let end =
        body.find(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '.' | '_' | '-' | ':')).unwrap_or(body.len());
    let (name, rest) = body.split_at_checked(end).unwrap_or((body, ""));
    if rest.starts_with('>') || rest.starts_with("/>") {
        return true;
    }
    if name.len() <= 20 && is_name(&name.to_ascii_uppercase()) {
        return true;
    }
    if !rest.starts_with(char::is_whitespace) {
        return false;
    }
    let rest = rest.trim_start();
    let end =
        rest.find(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '.' | '_' | '-' | ':')).unwrap_or(rest.len());
    rest.starts_with('>') || rest.starts_with("/>") || (end > 0 && rest.get(end..).is_some_and(|r| r.starts_with('=')))
}

fn is_name(name: &str) -> bool {
    BLOCKS.contains(&name) || OF_TRANSACTION.iter().chain(&OF_STATEMENT).any(|(_, n, _)| *n == name)
}

/// Line ends, however they are written.
pub(crate) fn lines(s: &str) -> usize {
    let mut n = 0;
    let mut bytes = s.bytes().peekable();
    while let Some(b) = bytes.next() {
        match b {
            b'\n' => n += 1,
            b'\r' if bytes.peek() != Some(&b'\n') => n += 1,
            _ => {}
        }
    }
    n
}

/// The standard entities and the numeric ones, once; anything else stays as it is.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        let (before, from) = rest.split_at(i);
        out.push_str(before);
        let end = from.bytes().take(12).position(|b| b == b';');
        let name = end.and_then(|e| from.get(1..e));
        let c = match name {
            Some("amp") => Some('&'),
            Some("lt") => Some('<'),
            Some("gt") => Some('>'),
            Some("quot") => Some('"'),
            Some("apos") => Some('\''),
            Some("nbsp") => Some(' '),
            Some(n) => n.strip_prefix('#').and_then(|n| match n.strip_prefix(['x', 'X']) {
                Some(h) => u32::from_str_radix(h, 16).ok().and_then(char::from_u32),
                None => n.parse().ok().and_then(char::from_u32),
            }),
            None => None,
        };
        match (c, end) {
            (Some(c), Some(e)) => {
                out.push(c);
                rest = from.get(e + 1..).unwrap_or_default();
            }
            _ => {
                out.push('&');
                rest = from.get(1..).unwrap_or_default();
            }
        }
    }
    out.push_str(rest);
    out
}

/// A text on one line, without control characters, of a length that is capped.
fn tidy(s: &str) -> String {
    let clean: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    clean.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_TEXT).collect()
}

/// What an error quotes of the file.
fn quoted(s: &str) -> String {
    tidy(s).chars().take(60).collect()
}

/// An amount as the specification has it: a sign, digits, one point or comma. Nothing is rounded.
pub(crate) fn amount(s: &str) -> Result<Cents> {
    let s = s.trim();
    let wrong = || anyhow!(trf("\"{amount}\" is not a valid amount", &[("amount", &quoted(s))]));
    let large = || anyhow!(trf("\"{amount}\" is too large", &[("amount", &quoted(s))]));
    let (negative, digits) = match s.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (units, decimals) = digits.split_once(['.', ',']).unwrap_or((digits, ""));
    if decimals.contains(['.', ',']) {
        return Err(wrong());
    }
    // A thousand or a thousandth? Banks write both.
    if decimals.len() == 3 && decimals.ends_with('0') && decimals.bytes().all(|b| b.is_ascii_digit()) {
        bail!(trf("\"{amount}\" may be read in two ways: three digits after the separator", &[("amount", &quoted(s))]));
    }
    if units.len() + decimals.len() == 0 || !units.bytes().chain(decimals.bytes()).all(|b| b.is_ascii_digit()) {
        return Err(wrong());
    }
    let (cents, beyond) = decimals.split_at(decimals.len().min(2));
    if beyond.bytes().any(|b| b != b'0') {
        bail!(trf("\"{amount}\" has more than two decimals", &[("amount", &quoted(s))]));
    }
    let units = units.trim_start_matches('0');
    let text = format!("{units}{cents:0<2}");
    let c: Cents = text.parse().map_err(|_| large())?;
    if c > MAX_AMOUNT {
        return Err(large());
    }
    Ok(if negative { -c } else { c })
}

/// The day of `YYYYMMDD[HHMM[SS[.XXX]]][[offset:zone]]`, as the bank writes it: moving it to another time
/// zone would move some transactions to another day.
pub(crate) fn date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    let (stamp, zone) = match s.split_once('[') {
        Some((stamp, zone)) => (stamp.trim_end(), Some(zone.strip_suffix(']')?)),
        None => (s, None),
    };
    if let Some(z) = zone {
        // "+1:CET", "-0500:EST", "-03.30", "GMT", "GMT+1".
        let offset = z.split(':').next().unwrap_or_default().trim();
        let named = offset.trim_start_matches(|c: char| c.is_ascii_alphabetic());
        let number = named.strip_prefix(['+', '-']).unwrap_or(named);
        let (h, m) = match number.split_once('.') {
            Some(parts) => parts,
            None if number.len() == 4 && number.is_ascii() => number.split_at(2),
            None => (number, "0"),
        };
        let digits = |t: &str| (1..=2).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_digit());
        let in_day = digits(h) && digits(m) && h.parse::<u8>().is_ok_and(|h| h <= 14);
        if offset.is_empty() || !(in_day || named.is_empty()) {
            return None;
        }
    }
    let (whole, thousandths) = stamp.split_once('.').unwrap_or((stamp, ""));
    let number = |t: &str| t.bytes().all(|b| b.is_ascii_digit());
    if !matches!(whole.len(), 8 | 10 | 12 | 14) || !number(whole) || !number(thousandths) || thousandths.len() > 6 {
        return None;
    }
    let (day, time) = whole.split_at(8);
    let mut parts = time.as_bytes().chunks(2).map(|c| std::str::from_utf8(c).ok()?.parse::<u8>().ok());
    for most in [23, 59, 60] {
        if let Some(p) = parts.next()
            && p.is_none_or(|p| p > most)
        {
            return None;
        }
    }
    let d = NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    let year = chrono::Datelike::year(&d);
    (1970..=2100).contains(&year).then_some(d)
}

/// The elements that hold others and matter here: each must end. Those that are not known are passed
/// through, and what is read belongs to the nearest one that is.
const BLOCKS: [&str; 18] = [
    "OFX",
    "STMTTRNRS",
    "CCSTMTTRNRS",
    "STMTRS",
    "CCSTMTRS",
    "BANKACCTFROM",
    "CCACCTFROM",
    "BANKTRANLIST",
    "STMTTRN",
    "PAYEE",
    "BANKACCTTO",
    "CCACCTTO",
    "CURRENCY",
    "ORIGCURRENCY",
    "LEDGERBAL",
    "AVAILBAL",
    "BALLIST",
    "IMAGEDATA",
];

/// What is kept of a transaction and of a statement: the block it is in ("": the one itself), the element, and
/// the name it is kept by.
const OF_TRANSACTION: [(&str, &str, &str); 11] = [
    ("", "TRNTYPE", "TRNTYPE"),
    ("", "DTPOSTED", "DTPOSTED"),
    ("", "TRNAMT", "TRNAMT"),
    ("", "FITID", "FITID"),
    ("", "CORRECTFITID", "CORRECTFITID"),
    ("", "CORRECTACTION", "CORRECTACTION"),
    ("", "NAME", "NAME"),
    ("PAYEE", "NAME", "PAYEE.NAME"),
    ("", "EXTDNAME", "EXTDNAME"),
    ("", "MEMO", "MEMO"),
    ("CURRENCY", "CURSYM", "CURRENCY.CURSYM"),
];
const OF_STATEMENT: [(&str, &str, &str); 6] = [
    ("", "CURDEF", "CURDEF"),
    ("BANKACCTFROM", "ACCTID", "BANKACCTFROM.ACCTID"),
    ("BANKACCTFROM", "ACCTTYPE", "BANKACCTFROM.ACCTTYPE"),
    ("CCACCTFROM", "ACCTID", "CCACCTFROM.ACCTID"),
    ("LEDGERBAL", "BALAMT", "LEDGERBAL.BALAMT"),
    ("LEDGERBAL", "DTASOF", "LEDGERBAL.DTASOF"),
];

/// A transaction or a statement while it is read: the value of each element, with its line.
#[derive(Default)]
struct Fields(HashMap<&'static str, (usize, String)>);

impl Fields {
    fn get(&self, key: &str) -> Option<(usize, &str)> {
        self.0.get(key).map(|(l, v)| (*l, v.as_str())).filter(|(_, v)| !v.is_empty())
    }

    fn text(&self, key: &str) -> String {
        self.get(key).map(|(_, v)| v.to_string()).unwrap_or_default()
    }

    /// The value of an element, or of the rest of one that is in pieces.
    fn put(&mut self, key: &'static str, line: usize, value: &str, again: bool) -> Result<()> {
        const MANY: [&str; 4] = ["MEMO", "NAME", "EXTDNAME", "PAYEE.NAME"];
        match self.0.get_mut(key) {
            Some((_, old)) if again || MANY.contains(&key) => {
                if !again {
                    old.push(' ');
                }
                old.push_str(value);
            }
            Some(_) => return Err(at_line(line, trf("{tag} is there twice", &[("tag", &key)]))),
            None => {
                self.0.insert(key, (line, value.to_string()));
            }
        }
        Ok(())
    }
}

fn required<'a>(f: &'a Fields, key: &str, line: usize) -> Result<(usize, &'a str)> {
    f.get(key).ok_or_else(|| at_line(line, trf("{tag} is missing", &[("tag", &key)])))
}

fn row(f: &Fields, line: usize, currency: &str) -> Result<OfxRow> {
    let (l, v) = required(f, "DTPOSTED", line)?;
    let date = date(v).ok_or_else(|| at_line(l, trf("\"{date}\" is not a valid OFX date", &[("date", &quoted(v))])))?;
    let (l, v) = required(f, "TRNAMT", line)?;
    let amount = amount(v).map_err(|e| at_line(l, e))?;
    if let Some((l, v)) = f.get("CURRENCY.CURSYM")
        && !v.eq_ignore_ascii_case(currency)
    {
        return Err(at_line(
            l,
            trf("the transaction is in {currency}, the statement is not", &[("currency", &quoted(v))]),
        ));
    }
    let first = ["NAME", "PAYEE.NAME", "EXTDNAME"].map(|k| tidy(&f.text(k))).into_iter().find(|t| !t.is_empty());
    Ok(OfxRow {
        line,
        date,
        amount,
        name: first.unwrap_or_default(),
        memo: tidy(&f.text("MEMO")),
        fitid: f.get("FITID").map(|(_, v)| v.to_string()),
        corrects: f.get("CORRECTFITID").map(|(_, v)| Correction {
            fitid: v.to_string(),
            delete: f.text("CORRECTACTION").eq_ignore_ascii_case("DELETE"),
        }),
        // The other types say nothing of the sign.
        goes_out: match f.text("TRNTYPE").to_ascii_uppercase().as_str() {
            "DEBIT" | "FEE" | "SRVCHG" | "CHECK" | "CASH" | "DIRECTDEBIT" | "REPEATPMT" => Some(true),
            "CREDIT" | "DEP" | "DIRECTDEP" => Some(false),
            _ => None,
        },
    })
}

/// A statement being read: its fields are those of every block in it, by the name of the block.
struct Open {
    line: usize,
    card: bool,
    fields: Fields,
    rows: Vec<(usize, Fields)>,
    listed: bool,
}

fn statement(o: Open) -> Result<OfxStatement> {
    let (l, currency) = required(&o.fields, "CURDEF", o.line)?;
    let currency = currency.to_ascii_uppercase();
    if currency.len() != 3 || !currency.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(at_line(l, trf("\"{currency}\" is not a currency code", &[("currency", &quoted(&currency))])));
    }
    let (_, account) = required(&o.fields, if o.card { "CCACCTFROM.ACCTID" } else { "BANKACCTFROM.ACCTID" }, o.line)?;
    let savings = o
        .fields
        .get("BANKACCTFROM.ACCTTYPE")
        .is_some_and(|(_, t)| ["SAVINGS", "MONEYMRKT", "CD"].iter().any(|k| t.eq_ignore_ascii_case(k)));
    let kind = match (o.card, savings) {
        (true, _) => Kind::CreditCard,
        (false, true) => Kind::Savings,
        _ => Kind::Bank,
    };
    let stated = o.fields.get("LEDGERBAL.BALAMT").and_then(|(_, v)| amount(v).ok());
    let on = o.fields.get("LEDGERBAL.DTASOF").and_then(|(_, v)| date(v));
    let rows = o.rows.iter().map(|(l, f)| row(f, *l, &currency)).collect::<Result<Vec<_>>>()?;
    Ok(OfxStatement { kind, account: tidy(account), currency, balance: stated.map(|s| (s, on)), rows })
}

/// Reads every statement of a file.
pub fn read(bytes: &[u8]) -> Result<(Ofx, &'static str)> {
    let (text, encoding) = text_of(bytes);
    Ok((parse(&text)?, encoding))
}

fn parse(text: &str) -> Result<Ofx> {
    let mut t = Tokens { rest: text, line: 1 };
    let mut open: Vec<(usize, String)> = Vec::new();
    let mut statements = Vec::new();
    let mut now: Option<Open> = None;
    let mut transactions = 0;
    let mut ended = false;
    // The element that a text is the value of, if it is kept; whether some of it was read.
    let mut leaf: Option<(Option<&'static str>, bool)> = None;
    loop {
        let (line, token) = t.next()?;
        match token {
            Token::End => break,
            Token::Empty => leaf = None,
            Token::Text(s, raw) => {
                let value = if raw { s.to_string() } else { unescape(s) };
                let Some((Some(key), again)) = &mut leaf else { continue };
                if !*again && value.trim().is_empty() {
                    continue;
                }
                let value = if *again { value.trim_end() } else { value.trim() };
                if let Some(now) = &mut now {
                    let in_transaction = open.iter().any(|(_, n)| n == "STMTTRN");
                    let fields = match now.rows.last_mut() {
                        Some((_, f)) if in_transaction => f,
                        _ => &mut now.fields,
                    };
                    fields.put(key, line, value, *again)?;
                }
                *again = true;
            }
            Token::Open(name) => {
                leaf = None;
                let name = name.to_ascii_uppercase();
                if ended {
                    return Err(at_line(line, tr("there is something after the end of the OFX file")));
                }
                let parent = open.iter().rev().find(|(_, n)| BLOCKS.contains(&n.as_str())).map(|(_, n)| n.as_str());
                if name == "OFX" && parent.is_some() {
                    return Err(at_line(line, trf("{tag} is there twice", &[("tag", &name)])));
                }
                if matches!(name.as_str(), "STMTRS" | "CCSTMTRS") {
                    if now.is_some() || open.iter().any(|(_, n)| n == "STMTRS" || n == "CCSTMTRS") {
                        return Err(at_line(line, tr("a statement is inside another one")));
                    }
                    if statements.len() >= MAX_STATEMENTS {
                        return Err(at_line(line, tr("the file has too many statements")));
                    }
                    let card = name == "CCSTMTRS";
                    now = Some(Open { line, card, fields: Fields::default(), rows: Vec::new(), listed: false });
                }
                if let Some(now) = &mut now {
                    match name.as_str() {
                        "BANKTRANLIST" if now.listed => {
                            return Err(at_line(line, trf("{tag} is there twice", &[("tag", &name)])));
                        }
                        "BANKTRANLIST" => now.listed = true,
                        "STMTTRN" => {
                            if parent != Some("BANKTRANLIST") {
                                return Err(at_line(
                                    line,
                                    tr("a transaction is outside the list of transactions, or inside another one"),
                                ));
                            }
                            transactions += 1;
                            if transactions > MAX_TRANSACTIONS {
                                return Err(at_line(line, tr("the file has too many transactions")));
                            }
                            now.rows.push((line, Fields::default()));
                        }
                        _ => {}
                    }
                }
                if BLOCKS.contains(&name.as_str()) {
                    if open.len() >= MAX_DEPTH {
                        return Err(at_line(line, tr("the elements of the file are nested too deep")));
                    }
                    open.push((line, name));
                } else {
                    let block = match parent {
                        Some("STMTTRN" | "STMTRS" | "CCSTMTRS") | None => "",
                        Some(b) => b,
                    };
                    let in_transaction = open.iter().any(|(_, n)| n == "STMTTRN");
                    let all = if in_transaction { OF_TRANSACTION.as_slice() } else { &OF_STATEMENT };
                    let kept = all.iter().find(|(b, n, _)| *b == block && *n == name).map(|(_, _, k)| *k);
                    leaf = Some((kept, false));
                }
            }
            Token::Close(name) => {
                leaf = None;
                let name = name.to_ascii_uppercase();
                if !BLOCKS.contains(&name.as_str()) {
                    continue;
                }
                match open.pop() {
                    Some((_, n)) if n == name => {}
                    Some((l, n)) => {
                        let error = trf(
                            "{tag} ends here, and {inner} of line {opened} has not ended",
                            &[("tag", &name), ("inner", &n), ("opened", &l)],
                        );
                        return Err(at_line(line, error));
                    }
                    None => return Err(at_line(line, trf("{tag} ends without a start", &[("tag", &name)]))),
                }
                match name.as_str() {
                    "STMTRS" | "CCSTMTRS" => {
                        if let Some(o) = now.take() {
                            statements.push(statement(o)?);
                        }
                    }
                    "OFX" => ended = true,
                    _ => {}
                }
            }
        }
    }
    if let Some((l, n)) = open.pop() {
        return Err(at_line(l, trf("{tag} never ends: the file is not complete", &[("tag", &n)])));
    }
    if !ended {
        bail!(tr("the file is not a complete OFX file"));
    }
    if statements.is_empty() {
        bail!(tr("the OFX file has no statement of a bank account or of a credit card"));
    }
    let chosen = (statements.len() == 1).then_some(0);
    Ok(Ofx { statements, chosen })
}

#[cfg(test)]
pub(crate) mod tests;
