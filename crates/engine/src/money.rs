//! Amounts and dates as people write them, in the language in use.

use crate::db::Cents;
use crate::i18n::{Lang, lang, tr, trf};
use anyhow::{Result, anyhow};
use chrono::{Datelike, NaiveDate};

/// Largest amount of a single transaction: keeps the sums far from the limit of `i64`.
pub const MAX_AMOUNT: Cents = 100_000_000_000_000;

/// "1.234,56 €" in Italian, "€1,234.56" in English, with a minus for money going out.
pub fn fmt_eur(c: Cents) -> String {
    match (lang(), fmt_plain(c)) {
        (Lang::It, n) => format!("{n} €"),
        (Lang::En, n) => match n.strip_prefix('-') {
            Some(n) => format!("-€{n}"),
            None => format!("€{n}"),
        },
    }
}

/// [`fmt_eur`] without the symbol.
pub fn fmt_plain(c: Cents) -> String {
    let (thousands, decimal) = match lang() {
        Lang::It => ('.', ','),
        Lang::En => (',', '.'),
    };
    let a = c.unsigned_abs();
    let digits = (a / 100).to_string();
    let mut grouped = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(thousands);
        }
        grouped.push(ch);
    }
    format!("{}{grouped}{decimal}{:02}", if c < 0 { "-" } else { "" }, a % 100)
}

/// "-1234.56" in every language, for files read by programs.
pub fn fmt_machine(c: Cents) -> String {
    let a = c.unsigned_abs();
    format!("{}{}.{:02}", if c < 0 { "-" } else { "" }, a / 100, a % 100)
}

/// A number for charts and percentages with `decimals` digits after the language's separator.
pub fn fmt_number(x: f64, decimals: usize) -> String {
    let n = format!("{x:.decimals$}");
    match lang() {
        Lang::It => n.replace('.', ","),
        Lang::En => n,
    }
}

/// Reads an amount written the Italian way ("-1.234,56", "12,5 €", "+3") or the English one
/// ("1,234.56", "-12.50"). `decimal_comma` says which separator is the decimal one; `None` guesses
/// it from the text alone, whatever the language: bank statements are read this way.
pub fn parse_amount_with(s: &str, decimal_comma: Option<bool>) -> Result<Cents> {
    let mut t: String =
        s.trim().chars().filter(|c| !c.is_whitespace() && *c != '\u{a0}' && *c != '€' && *c != '\'').collect();
    for unit in ["EUR", "eur", "Eur"] {
        t = t.replace(unit, "");
    }
    if t.is_empty() {
        return Err(anyhow!(tr("the amount is empty")));
    }
    let mut neg = false;
    if let Some(inner) = t.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        neg = true;
        t = inner.to_string();
    }
    if let Some(r) = t.strip_suffix('-') {
        neg = !neg;
        t = r.to_string();
    }
    if let Some(r) = t.strip_prefix('-') {
        neg = !neg;
        t = r.to_string();
    } else if let Some(r) = t.strip_prefix('+') {
        t = r.to_string();
    }
    let last_comma = t.rfind(',');
    let last_dot = t.rfind('.');
    let dec_comma = match decimal_comma {
        Some(v) => v,
        None => match (last_comma, last_dot) {
            (Some(c), Some(d)) => c > d,
            (Some(_), None) => true,
            // Several dots ("1.234.567") are Italian thousands separators.
            (None, Some(_)) => t.matches('.').count() > 1,
            (None, None) => false,
        },
    };
    let (int_part, frac_part) = if dec_comma {
        let clean = t.replace('.', "");
        match clean.rsplit_once(',') {
            Some((i, f)) => (i.to_string(), f.to_string()),
            None => (clean, String::new()),
        }
    } else {
        let clean = t.replace(',', "");
        match clean.rsplit_once('.') {
            Some((i, f)) => (i.to_string(), f.to_string()),
            None => (clean, String::new()),
        }
    };
    if int_part.is_empty() && frac_part.is_empty() {
        return Err(invalid(s));
    }
    if !int_part.chars().all(|c| c.is_ascii_digit()) || !frac_part.chars().all(|c| c.is_ascii_digit()) {
        return Err(invalid(s));
    }
    let units: i64 = if int_part.is_empty() { 0 } else { int_part.parse().map_err(|_| too_large(s))? };
    let mut frac = frac_part.clone();
    frac.truncate(3);
    while frac.len() < 2 {
        frac.push('0');
    }
    // Rounds to the cent.
    let cents_frac: i64 = if frac.len() == 3 {
        let v: i64 = frac.parse()?;
        (v + 5) / 10
    } else {
        frac.parse()?
    };
    let c = units
        .checked_mul(100)
        .and_then(|c| c.checked_add(cents_frac))
        .filter(|c| *c <= MAX_AMOUNT)
        .ok_or_else(|| too_large(s))?;
    Ok(if neg { -c } else { c })
}

fn invalid(s: &str) -> anyhow::Error {
    anyhow!(trf("\"{amount}\" is not a valid amount", &[("amount", &s)]))
}

fn too_large(s: &str) -> anyhow::Error {
    anyhow!(trf("\"{amount}\" is too large", &[("amount", &s)]))
}

/// Whether `digits` is a number split in thousands by `sep`: "1.234", "12.345.678".
fn in_thousands(digits: &str, sep: char) -> bool {
    let mut groups = digits.split(sep);
    let first = groups.next().unwrap_or_default();
    let rest: Vec<&str> = groups.collect();
    (1..=3).contains(&first.len()) && !first.starts_with('0') && !rest.is_empty() && rest.iter().all(|g| g.len() == 3)
}

/// Reads an amount typed by a person. Both ways of writing are understood in every language, which
/// only decides the one case that can be read both ways, a single separator before exactly three
/// digits: "1.234" is one thousand and more in Italian, "1,234" is in English.
pub fn parse_amount(s: &str) -> Result<Cents> {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit() || matches!(c, '.' | ',')).collect();
    let (dots, commas) = (digits.matches('.').count(), digits.matches(',').count());
    let decimal_comma = match (dots, commas) {
        (0, 0) => false,
        (_, 0) => in_thousands(&digits, '.') && (dots > 1 || lang() == Lang::It),
        (0, _) => !(in_thousands(&digits, ',') && (commas > 1 || lang() == Lang::En)),
        _ => digits.rfind(',') > digits.rfind('.'),
    };
    // Repeated separators are never decimal ones.
    let decimals = if decimal_comma { commas } else { dots };
    if decimals > 1 {
        return Err(invalid(s));
    }
    parse_amount_with(s, Some(decimal_comma))
}

/// Dates: 31/12/2024, 31-12-24, 31.12.2024, 2024-12-31, 2024/12/31, with a time after them too.
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim().trim_matches('"');
    let first = s.split([' ', 'T']).next().unwrap_or(s);
    const FMTS: [&str; 9] =
        ["%d/%m/%Y", "%d-%m-%Y", "%d.%m.%Y", "%Y-%m-%d", "%Y/%m/%d", "%d/%m/%y", "%d-%m-%y", "%d.%m.%y", "%Y%m%d"];
    for f in FMTS {
        if let Ok(d) = NaiveDate::parse_from_str(first, f)
            && (1970..=2100).contains(&d.year())
        {
            return Some(d);
        }
    }
    None
}

/// dd/mm/yyyy in Italian, yyyy-mm-dd in English.
pub fn fmt_date(d: NaiveDate) -> String {
    let f = match lang() {
        Lang::It => "%d/%m/%Y",
        Lang::En => "%Y-%m-%d",
    };
    d.format(f).to_string()
}

/// [`fmt_date`] without the year.
pub fn fmt_day(d: NaiveDate) -> String {
    let f = match lang() {
        Lang::It => "%d/%m",
        Lang::En => "%m-%d",
    };
    d.format(f).to_string()
}

pub fn month_name(m: u32) -> &'static str {
    match m {
        2 => tr("February"),
        3 => tr("March"),
        4 => tr("April"),
        5 => tr("May"),
        6 => tr("June"),
        7 => tr("July"),
        8 => tr("August"),
        9 => tr("September"),
        10 => tr("October"),
        11 => tr("November"),
        12 => tr("December"),
        _ => tr("January"),
    }
}

pub fn month_short(m: u32) -> String {
    month_name(m).chars().take(3).collect()
}

/// "October 2026".
pub fn month_and_year(d: NaiveDate) -> String {
    format!("{} {}", month_name(d.month()), d.year())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::with_lang;

    const LANGS: [Lang; 2] = [Lang::It, Lang::En];

    #[test]
    fn amounts() {
        for l in LANGS {
            with_lang(l, || {
                assert_eq!(parse_amount("-1.234,56").unwrap(), -123456);
                assert_eq!(parse_amount("1,234.56").unwrap(), 123456);
                assert_eq!(parse_amount("12,5 €").unwrap(), 1250);
                assert_eq!(parse_amount("-12.50").unwrap(), -1250);
                assert_eq!(parse_amount("-12,50").unwrap(), -1250);
                assert_eq!(parse_amount("1.234.567,00").unwrap(), 123456700);
                assert_eq!(parse_amount("1,234,567.00").unwrap(), 123456700);
                assert_eq!(parse_amount("1.234.567").unwrap(), 123456700);
                assert_eq!(parse_amount("1,234,567").unwrap(), 123456700);
                assert_eq!(parse_amount("100").unwrap(), 10000);
                assert_eq!(parse_amount("+45,00").unwrap(), 4500);
                assert_eq!(parse_amount("45,00-").unwrap(), -4500);
                assert_eq!(parse_amount("(45.00)").unwrap(), -4500);
                assert_eq!(parse_amount("-€1,234.56").unwrap(), -123456);
                assert_eq!(parse_amount("1234,567").unwrap(), 123457);
                assert_eq!(parse_amount("1234.567").unwrap(), 123457);
                assert_eq!(parse_amount("0.500").unwrap(), 50);
                assert_eq!(parse_amount("0,500").unwrap(), 50);
                assert_eq!(parse_amount(".5").unwrap(), 50);
                assert_eq!(parse_amount("5,").unwrap(), 500);
                for wrong in ["", "€", "-", ",", "1,2,3", "1.2.3", "1.234.56", "1,234,56", "1,2.3,4", "12a", "1e3"] {
                    assert!(parse_amount(wrong).is_err(), "{wrong}");
                }
                assert_eq!(parse_amount_with("1.234", Some(true)).unwrap(), 123400);
                assert_eq!(parse_amount_with("1.234", Some(false)).unwrap(), 123);
            });
        }
    }

    /// The only amounts that the language decides.
    #[test]
    fn three_digits_after_one_separator() {
        let read = |l, s| with_lang(l, || parse_amount(s).unwrap());
        for (text, italian, english) in [
            ("1.234", 123400, 123),
            ("1,234", 123, 123400),
            ("-12.345 €", -1234500, -1235),
            ("-12,345 €", -1235, -1234500),
        ] {
            assert_eq!(read(Lang::It, text), italian, "{text}");
            assert_eq!(read(Lang::En, text), english, "{text}");
        }
    }

    #[test]
    fn formats() {
        with_lang(Lang::It, || {
            assert_eq!(fmt_plain(-123456), "-1.234,56");
            assert_eq!(fmt_plain(5), "0,05");
            assert_eq!(fmt_eur(-123456), "-1.234,56 €");
            assert_eq!(fmt_number(12.5, 1), "12,5");
        });
        with_lang(Lang::En, || {
            assert_eq!(fmt_plain(-123456), "-1,234.56");
            assert_eq!(fmt_plain(5), "0.05");
            assert_eq!(fmt_eur(-123456), "-€1,234.56");
            assert_eq!(fmt_eur(123456), "€1,234.56");
            assert_eq!(fmt_number(12.5, 1), "12.5");
        });
        assert_eq!(fmt_machine(-123456), "-1234.56");
        assert_eq!(fmt_machine(-5), "-0.05");
    }

    /// A value shown in a form must be read back as it was, even if the language changes meanwhile.
    #[test]
    fn what_is_shown_is_read_back_the_same() {
        let mut values: Vec<Cents> = (0..=20_000).chain([MAX_AMOUNT, MAX_AMOUNT - 1]).collect();
        let mut x: u64 = 1;
        for _ in 0..20_000 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            values.push(((x >> 1) as i64) % 10_i64.pow((x >> 60) as u32 % 14 + 1));
        }
        for shown_in in LANGS {
            for c in values.iter().flat_map(|c| [*c, -c]) {
                let (plain, eur) = with_lang(shown_in, || (fmt_plain(c), fmt_eur(c)));
                for read_in in LANGS {
                    with_lang(read_in, || {
                        assert_eq!(parse_amount(&plain).unwrap(), c, "{plain}");
                        assert_eq!(parse_amount(&eur).unwrap(), c, "{eur}");
                        assert_eq!(parse_amount(&fmt_machine(c)).unwrap(), c);
                    });
                }
            }
        }
    }

    #[test]
    fn dates_shown_are_read_back_the_same() {
        let mut d = NaiveDate::from_ymd_opt(1999, 12, 1).unwrap();
        for _ in 0..4000 {
            for l in LANGS {
                assert_eq!(parse_date(&with_lang(l, || fmt_date(d))), Some(d));
            }
            d = d.succ_opt().unwrap();
        }
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date("31/12/2024"), NaiveDate::from_ymd_opt(2024, 12, 31));
        assert_eq!(parse_date("2024-12-31 10:00:00"), NaiveDate::from_ymd_opt(2024, 12, 31));
        assert_eq!(parse_date("31/12/24"), NaiveDate::from_ymd_opt(2024, 12, 31));
        assert_eq!(parse_date("Saldo"), None);
    }
}
