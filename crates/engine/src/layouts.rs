//! The columns the user confirmed for each layout of CSV and Excel files in each account, used again for the
//! next file. The sign is a thing of the account: a card's statement may have it inverted and a bank's not.

use crate::db::Db;
use crate::i18n::{tr, trf};
use crate::importer::{Format, Settings, Table, column_label};
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

/// Since format 7.
pub(crate) const LAYOUT_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS csv_layouts (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    signature TEXT NOT NULL,
    headers TEXT NOT NULL,
    settings TEXT NOT NULL,
    UNIQUE (account_id, signature)
);
";

/// Longer headings are not remembered.
const MAX_SIGNATURE: usize = 4_000;
/// The layouts used least recently make room.
const MAX_LAYOUTS: i64 = 200;

#[derive(Clone, Debug)]
pub struct Layout {
    pub id: i64,
    pub account: String,
    /// The separator and the headings, in lowercase.
    pub signature: String,
    headers: Vec<String>,
    settings: Option<Settings>,
}

impl Layout {
    /// What was remembered; an error when it cannot be read.
    pub fn settings(&self) -> Result<Settings> {
        self.settings
            .clone()
            .with_context(|| trf("the saved columns {id} cannot be read: forget them", &[("id", &self.id)]))
    }

    /// For the command line and MCP: the columns, the formats and the sign.
    pub fn json(&self) -> Value {
        let mut out = json!({"id": self.id, "account": self.account, "layout": self.signature,
                             "readable": self.settings.is_some()});
        if let (Some(s), Some(o)) = (&self.settings, out.as_object_mut()) {
            let m = &s.mapping;
            let name = |c: Option<usize>| c.and_then(|i| column_label(&self.headers, i));
            let columns = [
                ("date", m.date),
                ("value_date", m.value_date),
                ("payee", m.payee),
                ("description", m.description),
                ("extra", m.extra),
                ("amount", m.amount),
                ("credit", m.credit),
                ("debit", m.debit),
                ("status", m.status),
            ];
            let columns: serde_json::Map<String, Value> =
                columns.into_iter().filter_map(|(k, c)| Some((k.to_string(), json!(name(c)?)))).collect();
            let decimals = match s.decimal_comma {
                None => "auto",
                Some(true) => ",",
                Some(false) => ".",
            };
            o.insert("columns".into(), Value::Object(columns));
            o.insert("invert".into(), json!(s.invert));
            o.insert("decimal_separator".into(), json!(decimals));
            o.insert("date_format".into(), json!(s.dates.code()));
        }
        out
    }
}

/// What tells a layout from the others: the separator and the headings. A file without headings has none,
/// for two banks may have as many columns.
pub fn signature(t: &Table) -> Option<String> {
    if !t.has_columns() || t.header_row.is_none() {
        return None;
    }
    let separator = match (t.format(), t.delimiter) {
        (Format::Xls, _) => "xls".to_string(),
        (_, b'\t') => "tab".to_string(),
        (_, d) => char::from(d).to_string(),
    };
    // Signs stay: "+" and "-" are two headings.
    let tidy = |h: &String| h.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ").replace('|', "||");
    let names: Vec<String> = t.headers.iter().map(tidy).collect();
    let text = format!("{separator} | {}", names.join(" | "));
    (names.iter().any(|n| !n.is_empty()) && text.len() <= MAX_SIGNATURE).then_some(text)
}

type Row = (i64, String, String, String, String);

fn layout((id, account, signature, headers, settings): Row) -> Layout {
    let headers: Vec<String> = serde_json::from_str(&headers).unwrap_or_default();
    let settings: Option<Settings> = serde_json::from_str(&settings).ok();
    let fits = |s: &Settings| s.mapping.columns().iter().all(|c| *c < headers.len());
    Layout { id, account, signature, settings: settings.filter(fits), headers }
}

const SELECT: &str = "SELECT l.id, a.name, l.signature, l.headers, l.settings
                      FROM csv_layouts l JOIN accounts a ON a.id=l.account_id";

impl Db {
    /// What is remembered for the layout of a file in an account.
    pub fn layout_of(&self, t: &Table, account: Option<i64>) -> Result<Option<Layout>> {
        let (Some(signature), Some(account)) = (signature(t), account) else { return Ok(None) };
        let sql = format!("{SELECT} WHERE l.signature=?1 AND l.account_id=?2");
        let read = |r: &rusqlite::Row| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?));
        Ok(self.conn.query_row(&sql, params![signature, account], read).optional()?.map(layout))
    }

    pub fn layouts(&self) -> Result<Vec<Layout>> {
        let mut st = self.conn.prepare(&format!("{SELECT} ORDER BY l.id"))?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<Row>, _>>()?.into_iter().map(layout).collect())
    }

    pub(crate) fn save_layout(&self, t: &Table, account: i64, settings: &Settings) -> Result<()> {
        let Some(signature) = signature(t) else { return Ok(()) };
        self.atomically(|| {
            // A new id: the highest is the latest used.
            self.conn
                .execute("DELETE FROM csv_layouts WHERE signature=?1 AND account_id=?2", params![signature, account])?;
            self.conn.execute(
                "INSERT INTO csv_layouts(account_id, signature, headers, settings) VALUES(?1,?2,?3,?4)",
                params![account, signature, serde_json::to_string(&t.headers)?, serde_json::to_string(settings)?],
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

/// What a file is read with and the user may not expect: the sign remembered, and the decimal separator.
pub fn notices(t: &Table, s: &Settings, remembered_sign: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    if let (Some(account), true) = (remembered_sign, s.invert) {
        out.push(trf(
            "the sign of the amounts is inverted, as in the last import of this layout into \"{account}\"",
            &[("account", &account)],
        ));
    }
    if let Some(c) = s.doubtful_decimals(t) {
        out.push(trf(
            "the chosen columns seem to have \"{separator}\" before the decimals, and are read with the other one: say which it is",
            &[("separator", &c)],
        ));
    }
    if t.ofx.as_ref().and_then(|o| o.statement()).is_some_and(|s| s.signs_contradict()) {
        out.push(tr("the signs of the amounts contradict the types of the transactions").into());
    }
    out
}
