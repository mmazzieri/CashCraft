//! Importi e date nel formato italiano.

use crate::db::Cents;
use anyhow::{anyhow, Result};
use chrono::NaiveDate;

/// "1.234,56 €" (con segno meno per le uscite).
pub fn fmt_eur(c: Cents) -> String {
    format!("{} €", fmt_plain(c))
}

/// "1.234,56" senza simbolo.
pub fn fmt_plain(c: Cents) -> String {
    let neg = c < 0;
    let a = c.unsigned_abs();
    let units = a / 100;
    let dec = a % 100;
    let digits = units.to_string();
    let mut grouped = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push('.');
        }
        grouped.push(ch);
    }
    format!("{}{},{:02}", if neg { "-" } else { "" }, grouped, dec)
}

/// Legge un importo scritto all'italiana ("-1.234,56", "12,5 €", "+3")
/// o all'inglese ("1,234.56", "-12.50").
/// `decimal_comma`: Some(true/false) forza il separatore decimale, None lo indovina.
pub fn parse_amount_with(s: &str, decimal_comma: Option<bool>) -> Result<Cents> {
    let mut t: String = s
        .trim()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{a0}' && *c != '€' && *c != '\'')
        .collect();
    for unit in ["EUR", "eur", "Eur"] {
        t = t.replace(unit, "");
    }
    if t.is_empty() {
        return Err(anyhow!("importo vuoto"));
    }
    let mut neg = false;
    if t.starts_with('(') && t.ends_with(')') {
        neg = true;
        t = t[1..t.len() - 1].to_string();
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
            // Più punti ("1.234.567") sono separatori delle migliaia all'italiana.
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
        return Err(anyhow!("importo \"{s}\" non valido"));
    }
    if !int_part.chars().all(|c| c.is_ascii_digit())
        || !frac_part.chars().all(|c| c.is_ascii_digit())
    {
        return Err(anyhow!("importo \"{s}\" non valido"));
    }
    let units: i64 = if int_part.is_empty() {
        0
    } else {
        int_part.parse()?
    };
    let mut frac = frac_part.clone();
    frac.truncate(3);
    while frac.len() < 2 {
        frac.push('0');
    }
    // Arrotonda al centesimo.
    let cents_frac: i64 = if frac.len() == 3 {
        let v: i64 = frac.parse()?;
        (v + 5) / 10
    } else {
        frac.parse()?
    };
    let c = units * 100 + cents_frac;
    Ok(if neg { -c } else { c })
}

pub fn parse_amount(s: &str) -> Result<Cents> {
    parse_amount_with(s, None)
}

/// Date: 31/12/2024, 31-12-24, 31.12.2024, 2024-12-31, 2024/12/31, anche con orario.
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim().trim_matches('"');
    let first = s.split(|c: char| c == ' ' || c == 'T').next().unwrap_or(s);
    const FMTS: [&str; 9] = [
        "%d/%m/%Y", "%d-%m-%Y", "%d.%m.%Y", "%Y-%m-%d", "%Y/%m/%d", "%d/%m/%y", "%d-%m-%y",
        "%d.%m.%y", "%Y%m%d",
    ];
    for f in FMTS {
        if let Ok(d) = NaiveDate::parse_from_str(first, f) {
            if (1970..=2100).contains(&chrono::Datelike::year(&d)) {
                return Some(d);
            }
        }
    }
    None
}

pub fn fmt_date(d: NaiveDate) -> String {
    d.format("%d/%m/%Y").to_string()
}

pub const MONTHS_IT: [&str; 12] = [
    "gennaio",
    "febbraio",
    "marzo",
    "aprile",
    "maggio",
    "giugno",
    "luglio",
    "agosto",
    "settembre",
    "ottobre",
    "novembre",
    "dicembre",
];

pub fn month_name(m: u32) -> &'static str {
    MONTHS_IT[((m.max(1) - 1) % 12) as usize]
}

pub fn month_short(m: u32) -> String {
    month_name(m).chars().take(3).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts() {
        assert_eq!(parse_amount("-1.234,56").unwrap(), -123456);
        assert_eq!(parse_amount("1,234.56").unwrap(), 123456);
        assert_eq!(parse_amount("12,5 €").unwrap(), 1250);
        assert_eq!(parse_amount("-12.50").unwrap(), -1250);
        assert_eq!(parse_amount("1.234.567,00").unwrap(), 123456700);
        assert_eq!(parse_amount("100").unwrap(), 10000);
        assert_eq!(parse_amount("+45,00").unwrap(), 4500);
        assert_eq!(parse_amount("45,00-").unwrap(), -4500);
        assert_eq!(parse_amount_with("1.234", Some(true)).unwrap(), 123400);
        assert_eq!(fmt_plain(-123456), "-1.234,56");
        assert_eq!(fmt_plain(5), "0,05");
    }

    #[test]
    fn dates() {
        assert_eq!(
            parse_date("31/12/2024"),
            NaiveDate::from_ymd_opt(2024, 12, 31)
        );
        assert_eq!(
            parse_date("2024-12-31 10:00:00"),
            NaiveDate::from_ymd_opt(2024, 12, 31)
        );
        assert_eq!(
            parse_date("31/12/24"),
            NaiveDate::from_ymd_opt(2024, 12, 31)
        );
        assert_eq!(parse_date("Saldo"), None);
    }
}
