//! Operations shared by the command line and the MCP server: each takes JSON arguments and
//! returns JSON. Amounts come out in euros (numbers with two decimals). The descriptions of the
//! operations are English in every language; messages and errors are in the language in use.

use crate::db::{self, AccountKind, Cents, Db, NewScheduled, NewTx, TxEdit, TxFilter, TxKind};
use crate::i18n::{Lang, lang, tr, trf, trn};
use crate::importer;
use crate::ledger::{Class, Entry, EntryFilter, MAX_LINES, NewEntry, NewLine, TrialBalance, find_ledger_account};
use crate::money::{fmt_date, fmt_eur, fmt_machine, fmt_plain, parse_amount, parse_date};
use anyhow::{Result, anyhow, bail};
use chrono::{Datelike, Months, NaiveDate};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

fn s(desc: &str) -> Value {
    json!({"type": "string", "description": desc})
}
fn n(desc: &str) -> Value {
    json!({"type": "number", "description": desc})
}
fn b(desc: &str) -> Value {
    json!({"type": "boolean", "description": desc})
}
fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

const AMOUNT: &str = "Amount in euros: a number (-12.5), or text in the English (\"-1,234.50\") or Italian (\"-1.234,50\") style. Negative = money out, positive = money in. Prefer numbers: text with one separator before exactly three digits (\"1,234\" or \"1.234\") is read in the language in use, English over MCP.";
const DATE: &str = "Date: YYYY-MM-DD or DD/MM/YYYY (default: today).";
const ACCOUNT: &str = "Account: numeric id or name.";
const LEDGER_ACCOUNT: &str = "Account of the chart of accounts: code, id of list_ledger_accounts, path \"Parent:Child\" or name; \"opening_balances\" and \"uncategorized\" name the two accounts every file has.";
const CATEGORY: &str = "Category: id, name or path \"Parent:Child\" (e.g. \"Home:Utilities\").";

pub fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "list_accounts",
            description: "Lists the accounts with their balance and the total net worth.",
            schema: obj(json!({"include_archived": b("Include archived accounts.")}), &[]),
        },
        ToolDef {
            name: "create_account",
            description: "Creates an account (bank, cash, credit_card, savings, investment, loan).",
            schema: obj(
                json!({"name": s("Name of the account."), "kind": s("Type: bank, cash, credit_card, savings, investment, loan."),
                "opening_balance": n("Opening balance in euros."), "currency": s("Currency (default EUR)."), "iban": s("IBAN (optional).")}),
                &["name"],
            ),
        },
        ToolDef {
            name: "delete_account",
            description: "Deletes an account and all its transactions.",
            schema: obj(json!({"account": s(ACCOUNT)}), &["account"]),
        },
        ToolDef {
            name: "list_transactions",
            description: "Lists transactions, newest first, with optional filters.",
            schema: obj(
                json!({"account": s(ACCOUNT), "category": s(CATEGORY), "from": s("From this day (included)."), "to": s("To this day (included)."),
                "search": s("Text to look for in payee, notes and category."), "uncategorized": b("Only transactions without a category."),
                "limit": n("Maximum number of transactions (default 100).")}),
                &[],
            ),
        },
        ToolDef {
            name: "add_transaction",
            description: "Records a transaction. Without a category, the automatic rules are applied.",
            schema: obj(
                json!({"account": s(ACCOUNT), "amount": s(AMOUNT), "payee": s("Payee or description."), "date": s(DATE),
                "category": s(CATEGORY), "memo": s("Notes."), "cleared": b("Already checked against the bank statement.")}),
                &["account", "amount", "payee"],
            ),
        },
        ToolDef {
            name: "update_transaction",
            description: "Changes a transaction; fields left out stay as they are. Empty category = no category.",
            schema: obj(
                json!({"id": n("Id of the transaction."), "date": s(DATE), "amount": s(AMOUNT), "payee": s("Payee."), "memo": s("Notes."),
                "category": s(CATEGORY), "cleared": b("Cleared.")}),
                &["id"],
            ),
        },
        ToolDef {
            name: "delete_transaction",
            description: "Deletes a transaction (both sides of a transfer).",
            schema: obj(json!({"id": n("Id of the transaction.")}), &["id"]),
        },
        ToolDef {
            name: "add_transfer",
            description: "Records a transfer between two accounts.",
            schema: obj(
                json!({"from": s("Account to transfer from."), "to": s("Account to transfer to."), "amount": s("Positive amount in euros."),
                "date": s(DATE), "memo": s("Notes.")}),
                &["from", "to", "amount"],
            ),
        },
        ToolDef {
            name: "preview_csv",
            description: "Analyzes a bank CSV without importing it: separator, recognized columns and first rows.",
            schema: obj(
                json!({"path": s("Path of the CSV file."), "invert": b("Invert the sign of the amounts."), "limit": n("Preview rows (default 10)."),
                "date_column": s("Date column (name, or number from 1)."), "amount_column": s("Amount column."), "credit_column": s("Money-in column."),
                "debit_column": s("Money-out column."), "description_column": s("Description column."), "payee_column": s("Payee column.")}),
                &["path"],
            ),
        },
        ToolDef {
            name: "import_csv",
            description: "Imports a CSV exported by an Italian bank into an account. Columns, separator, encoding and amount format are detected; transactions already imported are skipped; rules assign the categories.",
            schema: obj(
                json!({"path": s("Path of the CSV file."), "account": s(ACCOUNT), "invert": b("Invert the sign (e.g. credit card statements with positive expenses)."),
                "dry_run": b("Trial run: saves nothing."), "date_column": s("Date column (name, or number from 1)."), "amount_column": s("Amount column."),
                "credit_column": s("Money-in column."), "debit_column": s("Money-out column."), "description_column": s("Description column."),
                "payee_column": s("Payee column.")}),
                &["path", "account"],
            ),
        },
        ToolDef {
            name: "list_categories",
            description: "Lists the income and expense categories.",
            schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "create_category",
            description: "Creates a category (nested ones too, e.g. \"Home:Garden\").",
            schema: obj(
                json!({"path": s("Name or path Parent:Child."), "income": b("Income category (default: expense).")}),
                &["path"],
            ),
        },
        ToolDef {
            name: "delete_category",
            description: "Deletes a category (its transactions stay, without a category).",
            schema: obj(json!({"category": s(CATEGORY)}), &["category"]),
        },
        ToolDef {
            name: "spending_by_category",
            description: "Totals by category in a period (default: current month). Expenses are negative.",
            schema: obj(
                json!({"from": s("From this day."), "to": s("To this day."), "month": s("Month YYYY-MM (instead of from/to).")}),
                &[],
            ),
        },
        ToolDef {
            name: "monthly_summary",
            description: "Income, expenses and savings by month.",
            schema: obj(json!({"months": n("How many months (default 12).")}), &[]),
        },
        ToolDef {
            name: "net_worth",
            description: "Current net worth and its value at the end of each month.",
            schema: obj(json!({"months": n("How many months of history (default 12).")}), &[]),
        },
        ToolDef {
            name: "set_budget",
            description: "Sets the monthly budget of a category (0 removes it).",
            schema: obj(
                json!({"category": s(CATEGORY), "monthly_amount": s("Monthly amount in euros.")}),
                &["category", "monthly_amount"],
            ),
        },
        ToolDef {
            name: "budget_status",
            description: "Budget of the month: planned, spent and left by category.",
            schema: obj(json!({"month": s("Month YYYY-MM (default: current).")}), &[]),
        },
        ToolDef {
            name: "list_rules",
            description: "Lists the automatic categorization rules.",
            schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "add_rule",
            description: "Adds a rule: when the payee or the description contains one of the keywords (separated by |), it assigns the category.",
            schema: obj(
                json!({"keywords": s("Keywords separated by | (e.g. \"bakery|panificio\")."), "category": s(CATEGORY),
                "rename_payee": s("New name of the payee (optional).")}),
                &["keywords", "category"],
            ),
        },
        ToolDef {
            name: "delete_rule",
            description: "Deletes a rule.",
            schema: obj(json!({"id": n("Id of the rule.")}), &["id"]),
        },
        ToolDef {
            name: "apply_rules",
            description: "Applies the rules to every transaction without a category.",
            schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "list_scheduled",
            description: "Lists the scheduled (recurring) transactions.",
            schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "add_scheduled",
            description: "Adds a recurring transaction (or a recurring transfer, with to_account).",
            schema: obj(
                json!({"account": s(ACCOUNT), "amount": s(AMOUNT), "payee": s("Payee."), "next_date": s("Next due date."),
                "every": n("Every how many units (default 1)."), "unit": s("day, week, month, year (default month)."),
                "category": s(CATEGORY), "memo": s("Notes."), "to_account": s("Account to transfer to, for a transfer.")}),
                &["account", "amount", "payee", "next_date"],
            ),
        },
        ToolDef {
            name: "delete_scheduled",
            description: "Deletes a scheduled transaction.",
            schema: obj(json!({"id": n("Id.")}), &["id"]),
        },
        ToolDef {
            name: "post_scheduled",
            description: "Records the scheduled transactions due up to a date (default: today).",
            schema: obj(json!({"until": s("Up to this day.")}), &[]),
        },
        ToolDef {
            name: "export_csv",
            description: "Exports transactions to a CSV file. Over MCP, and in English: comma separator, amounts as 1234.56, dates as YYYY-MM-DD. In the Italian command line: semicolon, 1.234,56, DD/MM/YYYY.",
            schema: obj(
                json!({"path": s("File to write."), "account": s(ACCOUNT), "from": s("From this day."), "to": s("To this day.")}),
                &["path"],
            ),
        },
        ToolDef {
            name: "list_ledger_accounts",
            description: "Lists the chart of accounts of the double-entry ledger, with balances (exact decimal text, debit positive). Every account and every category is in it.",
            schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "create_ledger_account",
            description: "Adds an account to the chart of accounts. An income or expense account is a category.",
            schema: obj(
                json!({"name": s("Name, without colons."), "class": s("asset, liability, equity, income or expense (default: that of the parent)."),
                "parent": s("Account above it, of the same class (optional). Code, id, path or name."), "code": s("Code, unique (optional)."),
                "currency": s("Currency (default: that of the parent, or EUR); not for income and expense.")}),
                &["name"],
            ),
        },
        ToolDef {
            name: "post_entry",
            description: "Posts a double entry. Debits (positive) and credits (negative) must be equal in each currency, or it is refused and nothing is written.",
            schema: obj(
                json!({"date": s(DATE), "payee": s("Payee or description."), "memo": s("Notes."),
                "postings": {"type": "array", "description": "At least two.", "items": obj(
                    json!({"account": s(LEDGER_ACCOUNT), "amount": s("Exact amount with at most two decimals, a number or text like \"-12.50\": debit positive, credit negative."),
                    "currency": s("Only for entries in several currencies.")}),
                    &["account", "amount"],
                )}}),
                &["postings"],
            ),
        },
        ToolDef {
            name: "get_entry",
            description: "One entry of the ledger with its postings.",
            schema: obj(json!({"id": n("Id of the entry (entry_id of a transaction).")}), &["id"]),
        },
        ToolDef {
            name: "list_entries",
            description: "Lists the entries of the ledger with their postings, newest first, opening balances included (dated 0001-01-01).",
            schema: obj(
                json!({"ledger_account": s(LEDGER_ACCOUNT), "from": s("From this date."), "to": s("Up to this date."),
                "limit": n("Maximum number of entries (default 100).")}),
                &[],
            ),
        },
        ToolDef {
            name: "delete_entry",
            description: "Deletes a whole entry, every side of it. Opening balances are changed in their account instead.",
            schema: obj(json!({"id": n("Id of the entry.")}), &["id"]),
        },
        ToolDef {
            name: "trial_balance",
            description: "Trial balance: the debit or credit balance of every account, with equal totals.",
            schema: obj(json!({"date": s("Up to this date, included (default: everything).")}), &[]),
        },
        ToolDef {
            name: "verify_ledger",
            description: "Checks that the books are sound: every entry balances, every posting has its account. Changes nothing.",
            schema: obj(json!({}), &[]),
        },
    ]
}

// ----------------------------------------------------------- argomenti

fn opt_str(a: &Value, k: &str) -> Option<String> {
    match a.get(k)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn missing(k: &str) -> anyhow::Error {
    anyhow!(trf("the parameter \"{name}\" is missing", &[("name", &k)]))
}

fn req_str(a: &Value, k: &str) -> Result<String> {
    opt_str(a, k).filter(|s| !s.trim().is_empty()).ok_or_else(|| missing(k))
}

fn opt_bool(a: &Value, k: &str) -> bool {
    match a.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.to_lowercase().as_str(), "true" | "1" | "si" | "sì" | "yes"),
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        _ => false,
    }
}

fn opt_i64(a: &Value, k: &str) -> Option<i64> {
    match a.get(k)? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn req_i64(a: &Value, k: &str) -> Result<i64> {
    opt_i64(a, k).ok_or_else(|| anyhow!(trf("the numeric parameter \"{name}\" is missing", &[("name", &k)])))
}

fn amount_of(v: &Value) -> Result<Cents> {
    match v {
        // The decimal text of the number, so that no amount goes through floating point.
        Value::Number(n) => crate::money::parse_amount_with(&n.to_string(), Some(false)),
        Value::String(s) => parse_amount(s),
        _ => bail!(tr("the amount is not valid")),
    }
}

/// [`amount_of`] without rounding: more than two decimals are refused.
fn exact_amount(v: &Value) -> Result<Cents> {
    let c = amount_of(v)?;
    let text = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    crate::money::check_exact(&text, c)?;
    Ok(c)
}

/// An account of the chart, given as text or as a whole number.
fn ledger_account_arg(db: &Db, a: &Value, k: &str) -> Result<Option<i64>> {
    match opt_str(a, k) {
        Some(key) if !key.trim().is_empty() => Ok(Some(db.find_ledger_account(&key)?.id)),
        _ => Ok(None),
    }
}

fn entry_json(e: &Entry) -> Value {
    let postings: Vec<Value> = e
        .postings
        .iter()
        .map(|p| {
            json!({"id": p.id, "account_id": p.account_id, "account": p.account, "amount": fmt_machine(p.amount),
                   "currency": p.currency, "cleared": p.cleared})
        })
        .collect();
    json!({"id": e.id, "date": e.date, "payee": e.payee, "memo": e.memo, "opening": e.opening, "postings": postings})
}

fn trial_json(t: &TrialBalance, until: Option<NaiveDate>) -> Value {
    let side = |c: Cents, debit: bool| if (c > 0) == debit { fmt_machine(c.saturating_abs()) } else { fmt_machine(0) };
    let lines: Vec<Value> = t
        .lines
        .iter()
        .map(|l| {
            json!({"account_id": l.account_id, "account": l.account, "code": l.code, "class": l.class,
                   "currency": l.currency, "debit": side(l.amount, true), "credit": side(l.amount, false)})
        })
        .collect();
    let totals: Vec<Value> = t
        .totals
        .iter()
        .map(|x| {
            json!({"currency": x.currency, "debit": fmt_machine(x.debit), "credit": fmt_machine(x.credit),
                   "difference": fmt_machine(x.debit.saturating_sub(x.credit))})
        })
        .collect();
    json!({"date": until, "lines": lines, "totals": totals, "balanced": t.is_zero()})
}

fn opt_amount(a: &Value, k: &str) -> Result<Option<Cents>> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => amount_of(v).map(Some),
    }
}

fn req_amount(a: &Value, k: &str) -> Result<Cents> {
    opt_amount(a, k)?.ok_or_else(|| missing(k))
}

pub fn date_arg(s: &str) -> Result<NaiveDate> {
    let t = s.trim().to_lowercase();
    match t.as_str() {
        "" | "oggi" | "today" => return Ok(db::today()),
        "ieri" | "yesterday" => return Ok(db::today().pred_opt().unwrap_or_default()),
        _ => {}
    }
    parse_date(s)
        .ok_or_else(|| anyhow!(trf("\"{date}\" is not a valid date (use YYYY-MM-DD or DD/MM/YYYY)", &[("date", &s)])))
}

fn opt_date(a: &Value, k: &str) -> Result<Option<NaiveDate>> {
    match opt_str(a, k) {
        Some(s) if !s.trim().is_empty() => date_arg(&s).map(Some),
        _ => Ok(None),
    }
}

fn month_arg(s: &str) -> Result<NaiveDate> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(db::first_of_month(db::today()));
    }
    NaiveDate::parse_from_str(&format!("{t}-01"), "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(&format!("01/{t}"), "%d/%m/%Y"))
        .map_err(|_| anyhow!(trf("\"{month}\" is not a valid month (use YYYY-MM)", &[("month", &s)])))
}

/// Converte ricorsivamente i campi in centesimi in euro.
fn to_eur(v: Value) -> Value {
    const KEYS: [&str; 12] = [
        "amount",
        "balance",
        "opening_balance",
        "total",
        "income",
        "expense",
        "budget",
        "spent",
        "remaining",
        "net",
        "net_worth",
        "sum",
    ];
    match v {
        Value::Object(m) => {
            let mut out = Map::new();
            for (k, val) in m {
                let val = if KEYS.contains(&k.as_str()) {
                    match val.as_i64() {
                        Some(c) => json!((c as f64) / 100.0),
                        None => to_eur(val),
                    }
                } else {
                    to_eur(val)
                };
                out.insert(k, val);
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.into_iter().map(to_eur).collect()),
        other => other,
    }
}

fn mapping_overrides(t: &importer::Table, a: &Value) -> Result<importer::Mapping> {
    let mut m = t.mapping.clone();
    let set = |k: &str, slot: &mut Option<usize>| -> Result<()> {
        if let Some(v) = opt_str(a, k) {
            if v.trim().is_empty() || v.trim() == "-" {
                *slot = None;
            } else {
                *slot = Some(
                    importer::column_index(t, &v)
                        .ok_or_else(|| anyhow!(trf("column \"{name}\" is not in the file", &[("name", &v)])))?,
                );
            }
        }
        Ok(())
    };
    set("date_column", &mut m.date)?;
    set("description_column", &mut m.description)?;
    set("payee_column", &mut m.payee)?;
    set("amount_column", &mut m.amount)?;
    set("credit_column", &mut m.credit)?;
    set("debit_column", &mut m.debit)?;
    if a.get("amount_column").is_some() && a.get("credit_column").is_none() && a.get("debit_column").is_none() {
        m.credit = None;
        m.debit = None;
    }
    if (a.get("credit_column").is_some() || a.get("debit_column").is_some()) && a.get("amount_column").is_none() {
        m.amount = None;
    }
    Ok(m)
}

// ----------------------------------------------------------- esecuzione

/// Cartelle a cui un agente (MCP) può accedere: legge i CSV solo da `read`
/// e scrive le esportazioni solo in `write`. I percorsi relativi partono da lì.
pub struct Roots {
    read: PathBuf,
    write: PathBuf,
}

impl Roots {
    pub fn new(read: &Path, write: &Path) -> Result<Self> {
        let canon = |p: &Path| -> Result<PathBuf> {
            let c = p.canonicalize().map_err(|e| {
                anyhow!(trf("the folder \"{path}\" cannot be used: {error}", &[("path", &p.display()), ("error", &e)]))
            })?;
            if !c.is_dir() {
                bail!(trf("\"{path}\" is not a folder", &[("path", &p.display())]));
            }
            Ok(c)
        };
        Ok(Roots { read: canon(read)?, write: canon(write)? })
    }

    fn read_path(&self, p: &str) -> Result<PathBuf> {
        let full = self.read.join(p);
        let c = full
            .canonicalize()
            .map_err(|e| anyhow!(trf("the file \"{path}\" cannot be read: {error}", &[("path", &p), ("error", &e)])))?;
        if !c.starts_with(&self.read) {
            bail!(trf("access denied: \"{path}\" is outside the allowed read folder", &[("path", &p)]));
        }
        Ok(c)
    }

    fn write_path(&self, p: &str) -> Result<PathBuf> {
        let full = self.write.join(p);
        let name = full
            .file_name()
            .ok_or_else(|| anyhow!(trf("the path \"{path}\" has no file name", &[("path", &p)])))?
            .to_owned();
        let parent = full.parent().ok_or_else(|| anyhow!(trf("the path \"{path}\" is not valid", &[("path", &p)])))?;
        let dir = parent.canonicalize().map_err(|e| {
            anyhow!(trf("the folder of \"{path}\" cannot be used: {error}", &[("path", &p), ("error", &e)]))
        })?;
        let target = dir.join(name);
        // Un collegamento già esistente non deve portare fuori dalla cartella.
        let resolved = if target.exists() { target.canonicalize()? } else { target.clone() };
        if !dir.starts_with(&self.write) || !resolved.starts_with(&self.write) {
            bail!(trf("access denied: \"{path}\" is outside the allowed write folder", &[("path", &p)]));
        }
        Ok(target)
    }
}

/// Esegue un'operazione. `roots` limita l'accesso ai file (obbligatorio per MCP);
/// `None` è per la CLI, dove decide chi la usa.
pub fn call(db: &Db, name: &str, a: &Value, roots: Option<&Roots>) -> Result<Value> {
    let v = call_raw(db, name, a, roots)?;
    Ok(to_eur(v))
}

fn call_raw(db: &Db, name: &str, a: &Value, roots: Option<&Roots>) -> Result<Value> {
    Ok(match name {
        "list_accounts" => {
            let accts = db.accounts(opt_bool(a, "include_archived"))?;
            let total: Cents = accts.iter().map(|a| a.balance).sum();
            json!({"accounts": accts, "net_worth": total})
        }
        "create_account" => {
            let kind = match opt_str(a, "kind") {
                Some(k) => AccountKind::from_code(&k)
                    .ok_or_else(|| anyhow!(trf("unknown account type \"{kind}\"", &[("kind", &k)])))?,
                None => AccountKind::Bank,
            };
            let id = db.add_account(
                &req_str(a, "name")?,
                kind,
                &opt_str(a, "currency").unwrap_or_else(|| "EUR".into()),
                opt_amount(a, "opening_balance")?.unwrap_or(0),
                &opt_str(a, "iban").unwrap_or_default(),
            )?;
            json!({"id": id, "message": tr("account created")})
        }
        "delete_account" => {
            let acc = db.find_account(&req_str(a, "account")?)?;
            db.delete_account(acc.id)?;
            json!({"message": trf("account \"{name}\" deleted", &[("name", &acc.name)])})
        }
        "list_transactions" => {
            let mut f = TxFilter {
                from: opt_date(a, "from")?,
                to: opt_date(a, "to")?,
                search: opt_str(a, "search").unwrap_or_default(),
                uncategorized: opt_bool(a, "uncategorized"),
                limit: Some(opt_i64(a, "limit").unwrap_or(100).max(1) as usize),
                ..Default::default()
            };
            if let Some(acc) = opt_str(a, "account") {
                f.account_id = Some(db.find_account(&acc)?.id);
            }
            if let Some(c) = opt_str(a, "category") {
                f.category_id = Some(db.find_category(&c)?.id);
            }
            let txs = db.transactions(&f)?;
            json!({"count": txs.len(), "transactions": txs})
        }
        "add_transaction" => {
            let acc = db.find_account(&req_str(a, "account")?)?;
            let category_id = match opt_str(a, "category") {
                Some(c) if !c.trim().is_empty() => Some(db.find_category(&c)?.id),
                _ => None,
            };
            let id = db.add_transaction(&NewTx {
                account_id: acc.id,
                date: opt_date(a, "date")?.unwrap_or_else(db::today),
                amount: req_amount(a, "amount")?,
                payee: req_str(a, "payee")?,
                memo: opt_str(a, "memo").unwrap_or_default(),
                category_id,
                cleared: opt_bool(a, "cleared"),
                import_hash: None,
            })?;
            let id = id.ok_or_else(|| anyhow!(tr("the transaction was not saved")))?;
            json!({"id": id, "transaction": db.transaction(id)?})
        }
        "update_transaction" => {
            let id = req_i64(a, "id")?;
            let t = db.transaction(id)?;
            let category_id = match opt_str(a, "category") {
                Some(c) if c.trim().is_empty() => None,
                Some(c) => Some(db.find_category(&c)?.id),
                None => t.category_id,
            };
            db.atomically(|| {
                if opt_str(a, "category").is_some() && matches!(t.kind, TxKind::Other | TxKind::Split) {
                    // Also without a category: asked for, it is not left as it is.
                    db.set_category(id, category_id)?;
                }
                db.update_transaction(
                    id,
                    &TxEdit {
                        date: opt_date(a, "date")?.unwrap_or(t.date),
                        amount: opt_amount(a, "amount")?.unwrap_or(t.amount),
                        payee: opt_str(a, "payee").unwrap_or(t.payee),
                        memo: opt_str(a, "memo").unwrap_or(t.memo),
                        category_id,
                        cleared: if a.get("cleared").is_some() { opt_bool(a, "cleared") } else { t.cleared },
                    },
                )
            })?;
            json!({"transaction": db.transaction(id)?})
        }
        "delete_transaction" => {
            let id = req_i64(a, "id")?;
            db.delete_transaction(id)?;
            json!({"message": trf("transaction {id} deleted", &[("id", &id)])})
        }
        "add_transfer" => {
            let from = db.find_account(&req_str(a, "from")?)?;
            let to = db.find_account(&req_str(a, "to")?)?;
            let (x, y) = db.add_transfer(
                from.id,
                to.id,
                opt_date(a, "date")?.unwrap_or_else(db::today),
                req_amount(a, "amount")?,
                &opt_str(a, "memo").unwrap_or_default(),
            )?;
            json!({"ids": [x, y], "message": tr("transfer recorded")})
        }
        "preview_csv" | "import_csv" => {
            let raw = req_str(a, "path")?;
            let path = match roots {
                Some(r) => r.read_path(&raw)?,
                None => PathBuf::from(raw),
            };
            let (text, enc) = importer::read_text(&path)?;
            let t = importer::analyze(&text, enc)?;
            let m = mapping_overrides(&t, a)?;
            if m.date.is_none() {
                bail!(tr("the date column was not found: give it with date_column"));
            }
            if m.amount.is_none() && m.credit.is_none() && m.debit.is_none() {
                bail!(tr("the amount column was not found: give it with amount_column or credit_column/debit_column"));
            }
            let (rows, skipped) = importer::parse_rows(&t, &m, opt_bool(a, "invert"));
            let info = importer::describe(&t, &m);
            if name == "preview_csv" || opt_bool(a, "dry_run") {
                let limit = opt_i64(a, "limit").unwrap_or(10).max(1) as usize;
                let total: Cents = rows.iter().map(|r| r.amount).sum();
                json!({"format": info, "rows_found": rows.len(), "rows_skipped": skipped, "sum": total,
                       "first_date": rows.first().map(|r| r.date), "last_date": rows.last().map(|r| r.date),
                       "preview": rows.iter().take(limit).collect::<Vec<_>>()})
            } else {
                let acc = db.find_account(&req_str(a, "account")?)?;
                let res = importer::import_rows(db, acc.id, &rows, skipped)?;
                json!({"account": acc.name, "format": info, "result": res,
                       "message": trf("{added} imported, {present} already present, {categorized} categorized automatically", &[("added", &res.added), ("present", &res.duplicates), ("categorized", &res.categorized)])})
            }
        }
        "list_categories" => json!({"categories": db.categories()?}),
        "create_category" => {
            let id = db.category_by_path(&req_str(a, "path")?, opt_bool(a, "income"), true)?;
            json!({"id": id})
        }
        "delete_category" => {
            let c = db.find_category(&req_str(a, "category")?)?;
            db.delete_category(c.id)?;
            json!({"message": trf("category \"{name}\" deleted", &[("name", &c.path)])})
        }
        "spending_by_category" => {
            let (from, to) = match opt_str(a, "month") {
                Some(m) => db::month_range(month_arg(&m)?),
                None => {
                    let (f0, t0) = db::month_range(db::today());
                    (opt_date(a, "from")?.unwrap_or(f0), opt_date(a, "to")?.unwrap_or(t0))
                }
            };
            let totals = db.category_totals(from, to)?;
            let income: Cents = totals.iter().filter(|t| t.total > 0).map(|t| t.total).sum();
            let expense: Cents = -totals.iter().filter(|t| t.total < 0).map(|t| t.total).sum::<Cents>();
            json!({"from": from, "to": to, "income": income, "expense": expense, "net": income - expense, "categories": totals})
        }
        "monthly_summary" => {
            let months = opt_i64(a, "months").unwrap_or(12).clamp(1, 120) as u32;
            let rows: Vec<Value> = db
                .monthly_totals(months, db::today())?
                .into_iter()
                .map(|m| json!({"month": m.month, "income": m.income, "expense": m.expense, "net": m.income - m.expense}))
                .collect();
            json!({"months": rows})
        }
        "net_worth" => {
            let months = opt_i64(a, "months").unwrap_or(12).clamp(1, 120) as u32;
            let accts = db.accounts(true)?;
            let total: Cents = accts.iter().map(|a| a.balance).sum();
            let hist: Vec<Value> = db
                .net_worth_history(months, db::today())?
                .into_iter()
                .map(|(m, v)| json!({"month": m, "net_worth": v}))
                .collect();
            json!({"net_worth": total, "history": hist})
        }
        "set_budget" => {
            let c = db.find_category(&req_str(a, "category")?)?;
            let amt = req_amount(a, "monthly_amount")?;
            db.set_budget(c.id, amt)?;
            json!({"message": trf("budget of \"{category}\": {amount} a month", &[("category", &c.path), ("amount", &fmt_eur(amt.abs()))])})
        }
        "budget_status" => {
            let m = month_arg(&opt_str(a, "month").unwrap_or_default())?;
            let lines: Vec<Value> = db
                .budget_status(m)?
                .into_iter()
                .map(|l| json!({"category": l.category, "budget": l.budget, "spent": l.spent, "remaining": l.budget - l.spent}))
                .collect();
            json!({"month": m.format("%Y-%m").to_string(), "lines": lines})
        }
        "list_rules" => json!({"rules": db.rules()?}),
        "add_rule" => {
            let c = db.find_category(&req_str(a, "category")?)?;
            let id = db.add_rule(&req_str(a, "keywords")?, c.id, &opt_str(a, "rename_payee").unwrap_or_default())?;
            json!({"id": id})
        }
        "delete_rule" => {
            db.delete_rule(req_i64(a, "id")?)?;
            json!({"message": tr("rule deleted")})
        }
        "apply_rules" => {
            let n = db.apply_rules()?;
            json!({"categorized": n, "message": trn(n, "{n} transaction categorized", "{n} transactions categorized")})
        }
        "list_scheduled" => json!({"scheduled": db.scheduled()?}),
        "add_scheduled" => {
            let acc = db.find_account(&req_str(a, "account")?)?;
            let to = match opt_str(a, "to_account") {
                Some(t) if !t.trim().is_empty() => Some(db.find_account(&t)?.id),
                _ => None,
            };
            let cat = match opt_str(a, "category") {
                Some(c) if !c.trim().is_empty() => Some(db.find_category(&c)?.id),
                _ => None,
            };
            let id = db.add_scheduled(&NewScheduled {
                account_id: acc.id,
                to_account_id: to,
                amount: req_amount(a, "amount")?,
                payee: req_str(a, "payee")?,
                memo: opt_str(a, "memo").unwrap_or_default(),
                category_id: cat,
                next_date: date_arg(&req_str(a, "next_date")?)?,
                every: opt_i64(a, "every").unwrap_or(1),
                unit: opt_str(a, "unit").unwrap_or_else(|| "month".into()),
            })?;
            json!({"id": id})
        }
        "delete_scheduled" => {
            db.delete_scheduled(req_i64(a, "id")?)?;
            json!({"message": tr("scheduled transaction deleted")})
        }
        "post_scheduled" => {
            let until = opt_date(a, "until")?.unwrap_or_else(db::today);
            let n = db.post_scheduled(until)?;
            json!({"posted": n, "message": trn(n, "{n} transaction recorded", "{n} transactions recorded")})
        }
        "export_csv" => {
            let mut f = TxFilter { from: opt_date(a, "from")?, to: opt_date(a, "to")?, ..Default::default() };
            if let Some(acc) = opt_str(a, "account") {
                f.account_id = Some(db.find_account(&acc)?.id);
            }
            let raw = req_str(a, "path")?;
            let path = match roots {
                Some(r) => r.write_path(&raw)?,
                None => PathBuf::from(raw),
            };
            let n = export_csv(db, &f, &path)?;
            json!({"exported": n, "path": path})
        }
        "list_ledger_accounts" => {
            let accounts: Vec<Value> = db
                .ledger_accounts()?
                .iter()
                .map(|l| {
                    let balances: Vec<Value> = l
                        .balances
                        .iter()
                        .map(|b| json!({"currency": b.currency, "amount": fmt_machine(b.amount)}))
                        .collect();
                    json!({"id": l.id, "class": l.class, "parent_id": l.parent_id, "name": l.name, "path": l.path,
                           "code": l.code, "currency": l.currency, "money_account_id": l.money_account_id,
                           "category_id": l.category_id, "system": l.system, "balances": balances})
                })
                .collect();
            json!({"accounts": accounts})
        }
        "create_ledger_account" => {
            let class = match opt_str(a, "class") {
                Some(c) if !c.trim().is_empty() => Some(
                    Class::from_code(&c)
                        .ok_or_else(|| anyhow!(trf("unknown class of account \"{class}\"", &[("class", &c)])))?,
                ),
                _ => None,
            };
            let id = db.create_ledger_account(
                &req_str(a, "name")?,
                class,
                ledger_account_arg(db, a, "parent")?,
                &opt_str(a, "code").unwrap_or_default(),
                opt_str(a, "currency").as_deref(),
            )?;
            json!({"id": id, "message": tr("account created")})
        }
        "post_entry" => {
            let postings = a.get("postings").and_then(Value::as_array).ok_or_else(|| missing("postings"))?;
            if postings.len() > MAX_LINES {
                bail!(trf("an entry has at most {n} postings", &[("n", &MAX_LINES)]));
            }
            let chart = db.ledger_accounts()?;
            let account = |p: &Value| match opt_str(p, "account") {
                Some(k) if !k.trim().is_empty() => Ok(find_ledger_account(&chart, &k)?.id),
                _ => Err(missing("account")),
            };
            let lines = postings
                .iter()
                .map(|p| {
                    Ok(NewLine {
                        ledger_account_id: account(p)?,
                        amount: exact_amount(p.get("amount").ok_or_else(|| missing("amount"))?)?,
                        currency: opt_str(p, "currency").filter(|c| !c.trim().is_empty()),
                    })
                })
                .collect::<Result<_>>()?;
            let id = db.post_entry(&NewEntry {
                date: opt_date(a, "date")?.unwrap_or_else(db::today),
                payee: opt_str(a, "payee").unwrap_or_default(),
                memo: opt_str(a, "memo").unwrap_or_default(),
                lines,
            })?;
            json!({"id": id, "entry": entry_json(&db.entry(id)?), "message": tr("entry posted")})
        }
        "get_entry" => json!({"entry": entry_json(&db.entry(req_i64(a, "id")?)?)}),
        "list_entries" => {
            let entries = db.entries(&EntryFilter {
                ledger_account_id: ledger_account_arg(db, a, "ledger_account")?,
                from: opt_date(a, "from")?,
                to: opt_date(a, "to")?,
                limit: Some(opt_i64(a, "limit").unwrap_or(100).max(1) as usize),
            })?;
            json!({"count": entries.len(), "entries": entries.iter().map(entry_json).collect::<Vec<_>>()})
        }
        "delete_entry" => {
            let id = req_i64(a, "id")?;
            db.delete_entry(id)?;
            json!({"message": trf("entry {id} deleted", &[("id", &id)])})
        }
        "trial_balance" => {
            let until = opt_date(a, "date")?;
            trial_json(&db.trial_balance(until)?, until)
        }
        "verify_ledger" => {
            let v = db.verify_ledger()?;
            let message = if v.problems.is_empty() {
                trf(
                    "The books are sound: {entries} entries, {postings} postings",
                    &[("entries", &v.entries), ("postings", &v.postings)],
                )
            } else {
                trn(v.problems.len(), "{n} problem found in the books", "{n} problems found in the books")
            };
            json!({"ok": v.problems.is_empty(), "entries": v.entries, "postings": v.postings,
                   "problems": v.problems, "message": message})
        }
        other => bail!(trf("unknown operation \"{name}\"", &[("name", &other)])),
    })
}

/// Writes transactions to a CSV file that follows the language: in Italian `;`, `1.234,56` and
/// dd/mm/yyyy; in English `,`, `1234.56` and yyyy-mm-dd. README documents both.
pub fn export_csv(db: &Db, f: &TxFilter, path: &std::path::Path) -> Result<usize> {
    let (delimiter, amount): (u8, fn(Cents) -> String) = match lang() {
        Lang::It => (b';', fmt_plain),
        Lang::En => (b',', fmt_machine),
    };
    let mut txs = db.transactions(f)?;
    txs.reverse();
    let mut w = csv::WriterBuilder::new().delimiter(delimiter).from_path(path)?;
    w.write_record([tr("Date"), tr("Account"), tr("Payee"), tr("Category"), tr("Notes"), tr("Amount"), tr("Cleared")])?;
    for t in &txs {
        w.write_record([
            fmt_date(t.date),
            t.account.clone(),
            t.payee.clone(),
            t.category.clone(),
            t.memo.clone(),
            amount(t.amount),
            if t.cleared { tr("yes").into() } else { tr("no").into() },
        ])?;
    }
    w.flush()?;
    Ok(txs.len())
}

/// Periodo "mese precedente" ecc. usato dalla finestra.
pub fn months_back(d: NaiveDate, n: u32) -> NaiveDate {
    db::first_of_month(d).checked_sub_months(Months::new(n)).unwrap_or(d)
}

pub fn year_start(d: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(d.year(), 1, 1).unwrap_or(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::with_lang;

    /// A fresh file in memory with one account, "A".
    fn one_account() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.add_account("A", AccountKind::Bank, "EUR", 0, "").unwrap();
        db
    }

    #[test]
    fn exports_follow_the_language_and_can_be_imported_back() {
        let export = |l| {
            with_lang(l, || {
                let db = one_account();
                for (amount, payee, cleared) in [(-123456, "Casa, dolce; casa", true), (100000, "Rossi \"SpA\"", false)]
                {
                    db.add_transaction(&NewTx {
                        account_id: 1,
                        date: NaiveDate::from_ymd_opt(2026, 3, 4).unwrap(),
                        amount,
                        payee: payee.into(),
                        cleared,
                        ..Default::default()
                    })
                    .unwrap();
                }
                let file = format!("cashcraft-export-{}-{}.csv", l.code(), std::process::id());
                let path = std::env::temp_dir().join(file);
                export_csv(&db, &TxFilter::default(), &path).unwrap();
                let text = std::fs::read_to_string(&path).unwrap();
                std::fs::remove_file(&path).unwrap();
                text
            })
        };
        let (it, en) = (export(Lang::It), export(Lang::En));
        assert_eq!(
            it,
            "Data;Conto;Beneficiario;Categoria;Note;Importo;Verificato\n\
             04/03/2026;A;\"Casa, dolce; casa\";;;-1.234,56;sì\n\
             04/03/2026;A;\"Rossi \"\"SpA\"\"\";;;1.000,00;no\n"
        );
        assert_eq!(
            en,
            "Date,Account,Payee,Category,Notes,Amount,Cleared\n\
             2026-03-04,A,\"Casa, dolce; casa\",,,-1234.56,yes\n\
             2026-03-04,A,\"Rossi \"\"SpA\"\"\",,,1000.00,no\n"
        );
        for text in [it, en] {
            let t = importer::analyze(&text, "UTF-8").unwrap();
            let (rows, skipped) = importer::parse_rows(&t, &t.mapping, false);
            let read: Vec<_> = rows.iter().map(|r| (r.amount, r.payee.as_str())).collect();
            assert_eq!(read, [(-123456, "Casa, dolce; casa"), (100000, "Rossi \"SpA\"")]);
            assert_eq!(skipped, 0);
        }
    }

    /// JSON numbers mean the same in every language; text is read as a form would.
    #[test]
    fn amounts_of_operations() {
        for (l, text, cents) in [
            (Lang::It, json!(1.234), 123),
            (Lang::En, json!(1.234), 123),
            (Lang::It, json!("1.234"), 123400),
            (Lang::En, json!("1.234"), 123),
            (Lang::En, json!("1,234"), 123400),
            (Lang::It, json!("-12,50"), -1250),
            (Lang::En, json!("-12,50"), -1250),
        ] {
            assert_eq!(with_lang(l, || amount_of(&text)).unwrap(), cents, "{text} {l:?}");
        }
    }

    #[test]
    fn keys_do_not_change_with_the_language() {
        let keys = |l| {
            with_lang(l, || {
                let v = call(&one_account(), "create_account", &json!({"name": "B"}), None).unwrap();
                (v.as_object().unwrap().keys().cloned().collect::<Vec<_>>(), v["message"].to_string())
            })
        };
        let (it, en) = (keys(Lang::It), keys(Lang::En));
        assert_eq!(it.0, en.0);
        assert_eq!((it.1.as_str(), en.1.as_str()), ("\"conto creato\"", "\"account created\""));
    }

    #[test]
    fn roots_keep_agent_file_access_inside_the_allowed_folders() {
        let base = std::env::temp_dir().join(format!("cashcraft-roots-{}", std::process::id()));
        let (read, write) = (base.join("in"), base.join("out"));
        std::fs::create_dir_all(&read).unwrap();
        std::fs::create_dir_all(&write).unwrap();
        std::fs::write(read.join("ok.csv"), "x").unwrap();
        std::fs::write(base.join("secret.csv"), "x").unwrap();
        let r = Roots::new(&read, &write).unwrap();
        assert!(r.read_path("ok.csv").is_ok());
        assert!(r.read_path("../secret.csv").is_err());
        assert!(r.read_path(base.join("secret.csv").to_str().unwrap()).is_err());
        assert!(r.write_path("export.csv").is_ok());
        assert!(r.write_path("../export.csv").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn hostile_params_never_panic() {
        let dir = std::env::temp_dir().join(format!("cashcraft-hostile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let roots = Roots::new(&dir, &dir).unwrap();
        let values = [
            json!(null),
            json!(true),
            json!(-1),
            json!(i64::MAX),
            json!(i64::MIN),
            json!(u64::MAX),
            json!(1e300),
            json!(-0.005),
            json!(""),
            json!("\0"),
            json!("İ"),
            json!("99999999999999999999,99"),
            json!("../../../etc/passwd"),
            json!("2025-13-99"),
            json!("9999-12"),
            json!([]),
            json!({"a": [1]}),
        ];
        for tool in tools() {
            let db = with_lang(Lang::It, || {
                let db = Db::open_in_memory().unwrap();
                crate::demo::seed(&db).unwrap();
                db
            });
            let names: Vec<String> = tool
                .schema
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|o| o.keys().cloned())
                .collect();
            for v in &values {
                // All parameters hostile, then one at a time among plausible ones.
                let all: Map<String, Value> = names.iter().map(|n| (n.clone(), v.clone())).collect();
                let _ = call(&db, tool.name, &Value::Object(all), Some(&roots));
                for hostile in &names {
                    let mut a = json!({"account": "Conto corrente", "from": "Conto corrente", "to": "Portafoglio",
                        "from_account": "Conto corrente", "to_account": "Portafoglio", "name": "x", "payee": "x",
                        "category": "Svago", "amount": 1, "monthly_amount": 1, "date": "2025-01-31",
                        "next_date": "2025-01-31", "id": 1, "class": "asset",
                        "parent": "Conto corrente", "ledger_account": "Conto corrente", "code": "1",
                        "postings": [{"account": "Conto corrente", "amount": "1,00"}, {"account": "Svago", "amount": -1}],
                        "path": "out.csv", "pattern": "x", "unit": "month"});
                    a[hostile.as_str()] = v.clone();
                    let _ = call(&db, tool.name, &a, Some(&roots));
                }
            }
            let _ = call(&db, tool.name, &json!("not an object"), Some(&roots));
            assert!(db.verify_ledger().unwrap().problems.is_empty(), "{}", tool.name);
            // Hostile postings, alone and in pairs.
            for v in &values {
                for p in [
                    json!([v]),
                    json!([{"account": v, "amount": v, "currency": v}, {"account": "Svago", "amount": v}]),
                    json!([{"account": "Conto corrente", "amount": v, "currency": v}, {"account": "Svago", "amount": -1}]),
                ] {
                    let _ = call(&db, tool.name, &json!({"postings": p}), Some(&roots));
                }
            }
            let many: Vec<Value> = vec![json!({"account": "Svago", "amount": 1_000_000_000_000_i64}); 2000];
            let _ = call(&db, tool.name, &json!({"postings": many}), Some(&roots));
            assert!(db.verify_ledger().unwrap().problems.is_empty(), "{}", tool.name);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
