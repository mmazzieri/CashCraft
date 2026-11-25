//! Rows of an Excel 97-2003 statement, as text. Some banks offer no other file.

use crate::i18n::tr;
use anyhow::{Result, anyhow, bail};
use chrono::{Days, NaiveDate};
use craft_xls::{Limits, Value, Workbook};

/// A statement is a short table: far below what a spreadsheet application would accept.
const LIMITS: Limits = Limits { max_records: 2_000_000, max_cells: 500_000, max_text: 32 << 20 };
/// Columns past this one are not read.
const MAX_COLUMNS: usize = 64;

pub fn is_xls(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1])
}

/// The first worksheet that has any cell.
pub fn rows(bytes: &[u8]) -> Result<Vec<Vec<String>>> {
    let read = craft_xls::read_with(bytes, LIMITS).map_err(|e| anyhow!(e))?;
    let book = &read.workbook;
    let Some(sheet) = book.sheets.iter().find(|s| !s.cells.is_empty()) else {
        bail!(tr("the workbook is empty"));
    };
    let mut rows: Vec<Vec<String>> = Vec::new();
    for (at, cell) in &sheet.cells {
        let (r, c) = (at.row as usize, at.col as usize);
        if c >= MAX_COLUMNS {
            continue;
        }
        if rows.len() <= r {
            rows.resize(r + 1, Vec::new());
        }
        if let Some(row) = rows.get_mut(r) {
            if row.len() <= c {
                row.resize(c + 1, String::new());
            }
            if let Some(slot) = row.get_mut(c) {
                *slot = text(book, &cell.value, cell.format);
            }
        }
    }
    Ok(rows)
}

fn text(book: &Workbook, value: &Value, format: Option<u16>) -> String {
    match value {
        Value::Text(s) => s.trim().to_string(),
        Value::Number(n) if is_date(book, format) => date(*n, book.date_1904).unwrap_or_default(),
        Value::Number(n) => number(*n),
        _ => String::new(),
    }
}

/// The number as Italian statements write it, rounded to the cent that the cell shows.
fn number(n: f64) -> String {
    if !n.is_finite() {
        return String::new();
    }
    if n.fract() == 0.0 { format!("{n:.0}") } else { format!("{n:.2}").replace('.', ",") }
}

/// Whether the cell's number format shows a day: it has a `d` and a `y` outside quoted text.
fn is_date(book: &Workbook, format: Option<u16>) -> bool {
    let Some(f) = format.and_then(|i| book.formats.get(usize::from(i))) else { return false };
    let mut quoted = false;
    let mut code = String::new();
    for c in f.number_format.to_ascii_lowercase().chars() {
        match c {
            '"' => quoted = !quoted,
            c if !quoted => code.push(c),
            _ => {}
        }
    }
    code.contains('d') && code.contains('y')
}

fn date(serial: f64, from_1904: bool) -> Option<String> {
    if !(1.0..=3_000_000.0).contains(&serial) {
        return None;
    }
    let first = if from_1904 { NaiveDate::from_ymd_opt(1904, 1, 1)? } else { NaiveDate::from_ymd_opt(1899, 12, 30)? };
    Some(first.checked_add_days(Days::new(serial as u64))?.to_string())
}
