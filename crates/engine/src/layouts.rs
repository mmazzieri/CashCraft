//! The columns the user confirmed for each layout of CSV and Excel files, used again for the next file.

use crate::db::Db;
use crate::i18n::{tr, trf};
use crate::importer::{Settings, Table, norm};
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

/// Since format 7.
pub(crate) const LAYOUT_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS csv_layouts (
    id INTEGER PRIMARY KEY,
    signature TEXT NOT NULL UNIQUE,
    settings TEXT NOT NULL
);
";

/// Longer headings are not remembered.
const MAX_SIGNATURE: usize = 4_000;
/// The layouts used least recently make room.
const MAX_LAYOUTS: i64 = 200;

#[derive(Clone, Debug, Serialize)]
pub struct Layout {
    pub id: i64,
    /// The separator and the headings, in lowercase.
    pub signature: String,
    pub settings: Settings,
}

/// What tells a layout from the others: the separator and the headings. A file without headings has none,
/// for two banks may have as many columns.
pub fn signature(t: &Table) -> Option<String> {
    if t.statement.is_some() || t.ofx.is_some() || t.header_row.is_none() {
        return None;
    }
    let separator = match t.delimiter {
        b'\t' if t.lines.is_empty() => "xls".to_string(),
        b'\t' => "tab".to_string(),
        d => char::from(d).to_string(),
    };
    let names: Vec<String> = t.headers.iter().map(|h| norm(h)).collect();
    let text = format!("{separator} | {}", names.join(" | "));
    (names.iter().any(|n| !n.is_empty()) && text.len() <= MAX_SIGNATURE).then_some(text)
}

impl Db {
    /// The settings remembered for the layout of a file.
    pub fn layout_of(&self, t: &Table) -> Result<Option<Layout>> {
        let Some(signature) = signature(t) else { return Ok(None) };
        let found: Option<(i64, String)> = self
            .conn
            .query_row("SELECT id, settings FROM csv_layouts WHERE signature=?1", [&signature], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let Some((id, text)) = found else { return Ok(None) };
        let settings = read(id, &text)?;
        if settings.mapping.columns().iter().any(|c| *c >= t.headers.len()) {
            bail!(broken(id));
        }
        Ok(Some(Layout { id, signature, settings }))
    }

    pub fn layouts(&self) -> Result<Vec<Layout>> {
        let mut st = self.conn.prepare("SELECT id, signature, settings FROM csv_layouts ORDER BY id")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, String>(2)?)))?;
        rows.map(|r| {
            let (id, signature, text) = r?;
            Ok(Layout { id, signature, settings: read(id, &text)? })
        })
        .collect()
    }

    pub(crate) fn save_layout(&self, t: &Table, settings: &Settings) -> Result<()> {
        let Some(signature) = signature(t) else { return Ok(()) };
        self.atomically(|| {
            // A new id: the highest is the latest used.
            self.conn.execute("DELETE FROM csv_layouts WHERE signature=?1", [&signature])?;
            self.conn.execute(
                "INSERT INTO csv_layouts(signature, settings) VALUES(?1,?2)",
                params![signature, serde_json::to_string(settings)?],
            )?;
            self.conn.execute(
                "DELETE FROM csv_layouts WHERE id NOT IN (SELECT id FROM csv_layouts ORDER BY id DESC LIMIT ?1)",
                [MAX_LAYOUTS],
            )?;
            Ok(())
        })
    }

    /// Goes back to the columns found in the file.
    pub fn forget_layout(&self, id: i64) -> Result<()> {
        if self.conn.execute("DELETE FROM csv_layouts WHERE id=?1", [id])? == 0 {
            bail!(trf("there are no saved columns {id}", &[("id", &id)]));
        }
        Ok(())
    }

    pub fn forget_layouts(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM csv_layouts", [])?)
    }
}

fn broken(id: i64) -> String {
    trf("the saved columns {id} cannot be read: forget them", &[("id", &id)])
}

fn read(id: i64, text: &str) -> Result<Settings> {
    serde_json::from_str(text).with_context(|| broken(id))
}

/// How the settings read, for people.
pub fn describe(s: &Settings) -> String {
    let decimals = match s.decimal_comma {
        None => tr("automatic"),
        Some(true) => ",",
        Some(false) => ".",
    };
    trf("dates {dates}, decimals {decimals}", &[("dates", &s.dates.label()), ("decimals", &decimals)])
}
