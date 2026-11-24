//! The command line. Each command becomes an operation of `ops` (the same as in the MCP server)
//! and prints a readable table, or JSON with `--json`. The help is in English; what is printed is
//! in the language in use.

use anyhow::{Result, anyhow};
use cashcraft_engine::db::Db;
use cashcraft_engine::i18n::{Lang, tr, trf, trn, with_lang};
use cashcraft_engine::ledger::Class;
use cashcraft_engine::money::{fmt_date, fmt_eur, fmt_plain, parse_amount_with, parse_date};
use cashcraft_engine::ops;
use clap::{Args, Subcommand};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Start the MCP server on stdin/stdout. CSV files are read only from the read folder and
    /// exports are written only in the write folder.
    Mcp {
        /// Folder the agent may read CSV files from
        #[arg(long)]
        automation_read_root: PathBuf,
        /// Folder the agent may write exports to
        #[arg(long)]
        automation_write_root: PathBuf,
    },
    /// Accounts
    #[command(subcommand, visible_alias = "conti")]
    Accounts(AccountsCmd),
    /// Transactions
    #[command(subcommand, visible_aliases = ["transactions", "movimenti"])]
    Tx(TxCmd),
    /// Transfer between two accounts
    #[command(visible_alias = "giroconto")]
    Transfer {
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long, allow_hyphen_values = true)]
        amount: String,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        memo: Option<String>,
    },
    /// Import a bank statement (CSV or .xls) into an account
    #[command(visible_alias = "importa")]
    Import {
        file: PathBuf,
        /// Account to import into (name or id)
        #[arg(long, short)]
        account: String,
        /// Show what would be imported, without saving
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        cols: ColumnArgs,
    },
    /// Analyze a statement (CSV or .xls) without importing it
    #[command(visible_alias = "anteprima")]
    Preview {
        file: PathBuf,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[command(flatten)]
        cols: ColumnArgs,
    },
    /// Categories
    #[command(subcommand, visible_alias = "categorie")]
    Categories(CatCmd),
    /// Monthly budgets
    #[command(subcommand)]
    Budget(BudgetCmd),
    /// Automatic categorization rules
    #[command(subcommand, visible_alias = "regole")]
    Rules(RulesCmd),
    /// Reports
    #[command(subcommand)]
    Report(ReportCmd),
    /// Scheduled (recurring) transactions
    #[command(subcommand, visible_alias = "pianificati")]
    Scheduled(SchedCmd),
    /// Double-entry ledger: chart of accounts, entries, trial balance
    #[command(subcommand, visible_alias = "contabilita")]
    Ledger(LedgerCmd),
    /// Export transactions to CSV, in the layout of the language in use
    #[command(visible_alias = "esporta")]
    Export {
        file: PathBuf,
        #[arg(long)]
        account: Option<String>,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
    },
    /// List the available operations (the same as in the MCP server)
    Tools,
    /// Run an operation with JSON arguments, e.g.: call list_transactions '{"limit":5}'
    Call { tool: String, args: Option<String> },
}

#[derive(Args, Debug, Default)]
pub struct ColumnArgs {
    /// Invert the sign of the amounts (e.g. card statements with positive expenses)
    #[arg(long)]
    invert: bool,
    /// Date column (name, or number from 1)
    #[arg(long)]
    date_col: Option<String>,
    /// Signed amount column
    #[arg(long)]
    amount_col: Option<String>,
    /// Money-in column
    #[arg(long)]
    credit_col: Option<String>,
    /// Money-out column
    #[arg(long)]
    debit_col: Option<String>,
    /// Description column
    #[arg(long)]
    desc_col: Option<String>,
    /// Payee column
    #[arg(long)]
    payee_col: Option<String>,
}

impl ColumnArgs {
    fn put(&self, m: &mut Map<String, Value>) {
        m.insert("invert".into(), json!(self.invert));
        for (k, v) in [
            ("date_column", &self.date_col),
            ("amount_column", &self.amount_col),
            ("credit_column", &self.credit_col),
            ("debit_column", &self.debit_col),
            ("description_column", &self.desc_col),
            ("payee_column", &self.payee_col),
        ] {
            if let Some(v) = v {
                m.insert(k.into(), json!(v));
            }
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum AccountsCmd {
    /// List the accounts with their balance
    List {
        #[arg(long)]
        all: bool,
    },
    /// Create an account
    Add {
        name: String,
        /// bank, cash, credit_card, savings, investment, loan
        #[arg(long, default_value = "bank")]
        kind: String,
        #[arg(long, allow_hyphen_values = true, default_value = "0")]
        opening: String,
        #[arg(long)]
        iban: Option<String>,
        /// Three-letter code
        #[arg(long, default_value = "EUR")]
        currency: String,
    },
    /// Change the currency of an account, amounts unchanged
    Currency { account: String, currency: String },
    /// Delete an account and its transactions
    Delete { account: String },
}

#[derive(Subcommand, Debug)]
pub enum TxCmd {
    /// List transactions
    List {
        #[arg(long, short)]
        account: Option<String>,
        #[arg(long, short)]
        category: Option<String>,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
        #[arg(long, short)]
        search: Option<String>,
        #[arg(long)]
        uncategorized: bool,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Record a transaction (negative amount = money out)
    Add {
        #[arg(long, short)]
        account: String,
        #[arg(long, allow_hyphen_values = true)]
        amount: String,
        #[arg(long, short)]
        payee: String,
        #[arg(long)]
        date: Option<String>,
        #[arg(long, short)]
        category: Option<String>,
        #[arg(long)]
        memo: Option<String>,
        #[arg(long)]
        cleared: bool,
    },
    /// Change a transaction
    Edit {
        id: i64,
        #[arg(long, allow_hyphen_values = true)]
        amount: Option<String>,
        #[arg(long)]
        payee: Option<String>,
        #[arg(long)]
        date: Option<String>,
        /// Category ("" removes it)
        #[arg(long)]
        category: Option<String>,
        #[arg(long)]
        memo: Option<String>,
        #[arg(long)]
        cleared: Option<bool>,
    },
    /// Delete a transaction
    Delete { id: i64 },
    /// Turn a transaction into a transfer with another account
    ToTransfer {
        id: i64,
        /// The other account (name or id)
        account: String,
        /// Transaction to join as the other side
        #[arg(long, conflicts_with = "no_match")]
        join: Option<i64>,
        /// Add the other side even if some transactions match
        #[arg(long)]
        no_match: bool,
    },
    /// List what may be the other side of a transaction in another account
    Matches { id: i64, account: String },
    /// Separate a transfer into two transactions
    Separate {
        id: i64,
        /// Remove the other side
        #[arg(long)]
        drop_other: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum CatCmd {
    /// List the categories
    List,
    /// Create a category ("Parent:Child")
    Add {
        path: String,
        #[arg(long)]
        income: bool,
    },
    /// Delete a category (its transactions stay)
    Delete { category: String },
}

#[derive(Subcommand, Debug)]
pub enum BudgetCmd {
    /// Budget of a month (YYYY-MM)
    Status { month: Option<String> },
    /// Set the monthly budget of a category (0 removes it)
    Set { category: String, amount: String },
}

#[derive(Subcommand, Debug)]
pub enum RulesCmd {
    /// List the rules
    List,
    /// Add a rule: keywords separated by |
    Add {
        keywords: String,
        category: String,
        #[arg(long)]
        rename: Option<String>,
    },
    /// Delete a rule
    Delete { id: i64 },
    /// Apply the rules to transactions without a category
    Apply,
}

#[derive(Subcommand, Debug)]
pub enum ReportCmd {
    /// Totals by category (default: current month)
    Categories {
        #[arg(long)]
        month: Option<String>,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
    },
    /// Income and expenses by month
    Monthly {
        #[arg(long, default_value_t = 12)]
        months: u32,
    },
    /// Net worth
    Networth {
        #[arg(long, default_value_t = 12)]
        months: u32,
    },
}

#[derive(Subcommand, Debug)]
pub enum SchedCmd {
    /// List the scheduled transactions
    List,
    /// Add a recurring transaction, or a transfer with --to-account
    Add {
        #[arg(long, short)]
        account: String,
        #[arg(long, allow_hyphen_values = true)]
        amount: String,
        #[arg(long, short)]
        payee: String,
        #[arg(long)]
        next: String,
        #[arg(long, default_value_t = 1)]
        every: i64,
        /// day, week, month, year
        #[arg(long, default_value = "month")]
        unit: String,
        #[arg(long, short)]
        category: Option<String>,
        #[arg(long)]
        to_account: Option<String>,
    },
    /// Delete a scheduled transaction
    Delete { id: i64 },
    /// Record the scheduled transactions that are due
    Post {
        #[arg(long)]
        until: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum LedgerCmd {
    /// List the chart of accounts with the balances
    Accounts,
    /// Add an account to the chart (income and expense accounts are categories)
    Add {
        name: String,
        /// asset, liability, equity, income, expense (default: that of the parent)
        #[arg(long)]
        class: Option<String>,
        /// Account above it: code, id, path or name
        #[arg(long)]
        parent: Option<String>,
        #[arg(long)]
        code: Option<String>,
        #[arg(long)]
        currency: Option<String>,
    },
    /// Rename an account added by hand
    Rename { account: String, name: String },
    /// Delete an account added by hand, when nothing is on it
    Remove { account: String },
    /// Give a posting of an entry another category (none: uncategorized)
    Categorize { posting_id: i64, category: Option<String> },
    /// Post an entry; debits (positive) and credits (negative) must be equal
    Post {
        /// "Account=amount", at least twice
        #[arg(long = "line", short, required = true)]
        lines: Vec<String>,
        #[arg(long, short)]
        payee: Option<String>,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        memo: Option<String>,
    },
    /// Show an entry
    Show { id: i64 },
    /// List the entries, newest first
    Entries {
        #[arg(long, short)]
        account: Option<String>,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// How many to skip
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Delete an entry, every side of it
    Delete { id: i64 },
    /// Trial balance, up to a date
    #[command(visible_alias = "bilancio")]
    TrialBalance {
        #[arg(long)]
        date: Option<String>,
    },
    /// Check that the books are sound
    Verify,
}

/// Postings written as "Account=amount".
fn posting_args(lines: &[String]) -> Result<Value> {
    lines
        .iter()
        .map(|l| {
            let (account, amount) = l
                .rsplit_once('=')
                .ok_or_else(|| anyhow!(trf("\"{line}\" is not written as Account=amount", &[("line", l)])))?;
            Ok(json!({"account": account, "amount": amount}))
        })
        .collect()
}

fn args(pairs: &[(&str, Option<Value>)]) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        if let Some(v) = v {
            m.insert(k.to_string(), v.clone());
        }
    }
    Value::Object(m)
}

fn o(s: &Option<String>) -> Option<Value> {
    s.as_ref().map(|v| json!(v))
}

/// A field of a JSON object, or null.
fn field<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.get(key).unwrap_or(&Value::Null)
}

fn v<T: serde::Serialize>(x: T) -> Option<Value> {
    Some(json!(x))
}

pub fn run(db: &Db, cmd: Cmd, as_json: bool) -> Result<()> {
    let (tool, a): (&str, Value) = match cmd {
        Cmd::Mcp { automation_read_root, automation_write_root } => {
            // Agents get English, whatever the language of the interface.
            return with_lang(Lang::En, || {
                let roots = ops::Roots::new(&automation_read_root, &automation_write_root)?;
                crate::mcp::serve(db, &roots)
            });
        }
        Cmd::Tools => {
            for t in ops::tools() {
                println!("{:<22} {}", t.name, t.description);
            }
            return Ok(());
        }
        Cmd::Call { tool, args } => {
            let a: Value = match args {
                Some(s) => serde_json::from_str(&s)
                    .map_err(|e| anyhow!(trf("the JSON arguments are not valid: {error}", &[("error", &e)])))?,
                None => json!({}),
            };
            let r = ops::call(db, &tool, &a, None)?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            return Ok(());
        }
        Cmd::Accounts(c) => match c {
            AccountsCmd::List { all } => ("list_accounts", args(&[("include_archived", v(all))])),
            AccountsCmd::Add { name, kind, opening, iban, currency } => (
                "create_account",
                args(&[
                    ("name", v(name)),
                    ("kind", v(kind)),
                    ("opening_balance", v(opening)),
                    ("iban", o(&iban)),
                    ("currency", v(currency)),
                ]),
            ),
            AccountsCmd::Currency { account, currency } => {
                ("set_account_currency", args(&[("account", v(account)), ("currency", v(currency))]))
            }
            AccountsCmd::Delete { account } => ("delete_account", args(&[("account", v(account))])),
        },
        Cmd::Tx(c) => match c {
            TxCmd::List { account, category, from, to, search, uncategorized, limit } => (
                "list_transactions",
                args(&[
                    ("account", o(&account)),
                    ("category", o(&category)),
                    ("from", o(&from)),
                    ("to", o(&to)),
                    ("search", o(&search)),
                    ("uncategorized", v(uncategorized)),
                    ("limit", v(limit)),
                ]),
            ),
            TxCmd::Add { account, amount, payee, date, category, memo, cleared } => (
                "add_transaction",
                args(&[
                    ("account", v(account)),
                    ("amount", v(amount)),
                    ("payee", v(payee)),
                    ("date", o(&date)),
                    ("category", o(&category)),
                    ("memo", o(&memo)),
                    ("cleared", v(cleared)),
                ]),
            ),
            TxCmd::Edit { id, amount, payee, date, category, memo, cleared } => (
                "update_transaction",
                args(&[
                    ("id", v(id)),
                    ("amount", o(&amount)),
                    ("payee", o(&payee)),
                    ("date", o(&date)),
                    ("category", o(&category)),
                    ("memo", o(&memo)),
                    ("cleared", cleared.map(|c| json!(c))),
                ]),
            ),
            TxCmd::Delete { id } => ("delete_transaction", args(&[("id", v(id))])),
            TxCmd::ToTransfer { id, account, join, no_match } => (
                "turn_into_transfer",
                args(&[
                    ("id", v(id)),
                    ("account", v(account)),
                    ("match", if no_match { v("none") } else { join.map(|j| json!(j)) }),
                ]),
            ),
            TxCmd::Matches { id, account } => ("transfer_candidates", args(&[("id", v(id)), ("account", v(account))])),
            TxCmd::Separate { id, drop_other } => {
                ("separate_transfer", args(&[("id", v(id)), ("keep_other", v(!drop_other))]))
            }
        },
        Cmd::Transfer { from, to, amount, date, memo } => (
            "add_transfer",
            args(&[("from", v(from)), ("to", v(to)), ("amount", v(amount)), ("date", o(&date)), ("memo", o(&memo))]),
        ),
        Cmd::Import { file, account, dry_run, cols } => {
            let mut m = Map::new();
            m.insert("path".into(), json!(file));
            m.insert("account".into(), json!(account));
            m.insert("dry_run".into(), json!(dry_run));
            cols.put(&mut m);
            ("import_csv", Value::Object(m))
        }
        Cmd::Preview { file, limit, cols } => {
            let mut m = Map::new();
            m.insert("path".into(), json!(file));
            m.insert("limit".into(), json!(limit));
            cols.put(&mut m);
            ("preview_csv", Value::Object(m))
        }
        Cmd::Categories(c) => match c {
            CatCmd::List => ("list_categories", json!({})),
            CatCmd::Add { path, income } => ("create_category", args(&[("path", v(path)), ("income", v(income))])),
            CatCmd::Delete { category } => ("delete_category", args(&[("category", v(category))])),
        },
        Cmd::Budget(c) => match c {
            BudgetCmd::Status { month } => ("budget_status", args(&[("month", o(&month))])),
            BudgetCmd::Set { category, amount } => {
                ("set_budget", args(&[("category", v(category)), ("monthly_amount", v(amount))]))
            }
        },
        Cmd::Rules(c) => match c {
            RulesCmd::List => ("list_rules", json!({})),
            RulesCmd::Add { keywords, category, rename } => (
                "add_rule",
                args(&[("keywords", v(keywords)), ("category", v(category)), ("rename_payee", o(&rename))]),
            ),
            RulesCmd::Delete { id } => ("delete_rule", args(&[("id", v(id))])),
            RulesCmd::Apply => ("apply_rules", json!({})),
        },
        Cmd::Report(c) => match c {
            ReportCmd::Categories { month, from, to } => {
                ("spending_by_category", args(&[("month", o(&month)), ("from", o(&from)), ("to", o(&to))]))
            }
            ReportCmd::Monthly { months } => ("monthly_summary", args(&[("months", v(months))])),
            ReportCmd::Networth { months } => ("net_worth", args(&[("months", v(months))])),
        },
        Cmd::Scheduled(c) => match c {
            SchedCmd::List => ("list_scheduled", json!({})),
            SchedCmd::Add { account, amount, payee, next, every, unit, category, to_account } => (
                "add_scheduled",
                args(&[
                    ("account", v(account)),
                    ("amount", v(amount)),
                    ("payee", v(payee)),
                    ("next_date", v(next)),
                    ("every", v(every)),
                    ("unit", v(unit)),
                    ("category", o(&category)),
                    ("to_account", o(&to_account)),
                ]),
            ),
            SchedCmd::Delete { id } => ("delete_scheduled", args(&[("id", v(id))])),
            SchedCmd::Post { until } => ("post_scheduled", args(&[("until", o(&until))])),
        },
        Cmd::Ledger(c) => match c {
            LedgerCmd::Accounts => ("list_ledger_accounts", json!({})),
            LedgerCmd::Add { name, class, parent, code, currency } => (
                "create_ledger_account",
                args(&[
                    ("name", v(name)),
                    ("class", o(&class)),
                    ("parent", o(&parent)),
                    ("code", o(&code)),
                    ("currency", o(&currency)),
                ]),
            ),
            LedgerCmd::Post { lines, payee, date, memo } => (
                "post_entry",
                args(&[
                    ("postings", Some(posting_args(&lines)?)),
                    ("payee", o(&payee)),
                    ("date", o(&date)),
                    ("memo", o(&memo)),
                ]),
            ),
            LedgerCmd::Show { id } => ("get_entry", args(&[("id", v(id))])),
            LedgerCmd::Entries { account, from, to, limit, offset } => (
                "list_entries",
                args(&[
                    ("ledger_account", o(&account)),
                    ("from", o(&from)),
                    ("to", o(&to)),
                    ("limit", v(limit)),
                    ("offset", v(offset)),
                ]),
            ),
            LedgerCmd::Rename { account, name } => {
                ("rename_ledger_account", args(&[("ledger_account", v(account)), ("name", v(name))]))
            }
            LedgerCmd::Remove { account } => ("delete_ledger_account", args(&[("ledger_account", v(account))])),
            LedgerCmd::Categorize { posting_id, category } => {
                ("set_posting_category", args(&[("posting_id", v(posting_id)), ("category", o(&category))]))
            }
            LedgerCmd::Delete { id } => ("delete_entry", args(&[("id", v(id))])),
            LedgerCmd::TrialBalance { date } => ("trial_balance", args(&[("date", o(&date))])),
            LedgerCmd::Verify => ("verify_ledger", json!({})),
        },
        Cmd::Export { file, account, from, to } => {
            ("export_csv", args(&[("path", v(file)), ("account", o(&account)), ("from", o(&from)), ("to", o(&to))]))
        }
    };
    let r = ops::call(db, tool, &a, None)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else {
        print_human(tool, &r);
    }
    Ok(())
}

fn eur(v: &Value) -> String {
    v.as_f64().map(|f| fmt_eur((f * 100.0).round() as i64)).unwrap_or_default()
}

/// An exact amount of the ledger, written as in the language in use, with its currency.
fn exact(v: &Value, currency: &Value) -> String {
    let n = parse_amount_with(&st(v), Some(false)).map(fmt_plain).unwrap_or_else(|_| st(v));
    format!("{n} {}", st(currency))
}

fn print_entry(e: &Value) {
    let when = if field(e, "opening").as_bool().unwrap_or(false) {
        tr("Opening balance").to_string()
    } else {
        date(field(e, "date"))
    };
    println!("{:>5}  {}  {}", st(field(e, "id")), when, st(field(e, "payee")));
    for p in field(e, "postings").as_array().into_iter().flatten() {
        println!(
            "  {:>5} {:<44} {:>20}",
            st(field(p, "id")),
            cut(&st(field(p, "account")), 44),
            exact(field(p, "amount"), field(p, "currency"))
        );
    }
}

/// Imported lines that are sides of transfers, or may be.
fn print_transfers(result: &Value) {
    let line = |t: &Value| {
        format!(
            "{:>5}  {}  {:<40} {:>13}",
            st(field(t, "id")),
            date(field(t, "date")),
            cut(&st(field(t, "payee")), 40),
            eur(field(t, "amount"))
        )
    };
    for t in field(result, "adopted").as_array().into_iter().flatten() {
        println!("{}", trf("Side of a transfer that was there: {line}", &[("line", &line(t))]));
    }
    for m in field(result, "maybe_transfers").as_array().into_iter().flatten() {
        println!(
            "{}",
            trf(
                "It may be the side of a transfer (tx to-transfer --join): {line}",
                &[("line", &line(field(m, "transaction")))]
            )
        );
        for c in field(m, "candidates").as_array().into_iter().flatten() {
            println!("  {}", line(c));
        }
    }
}

fn st(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A column heading.
fn head(s: &str) -> String {
    s.to_uppercase()
}

fn count(v: &Value) -> usize {
    v.as_u64().and_then(|n| usize::try_from(n).ok()).unwrap_or(0)
}

fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}

fn date(v: &Value) -> String {
    parse_date(&st(v)).map(fmt_date).unwrap_or_else(|| st(v))
}

fn print_human(tool: &str, r: &Value) {
    let arr = |k: &str| r.get(k).and_then(|x| x.as_array()).cloned().unwrap_or_default();
    match tool {
        "list_accounts" => {
            println!("{:>4}  {:<28} {:<18} {:>16}", "ID", head(tr("Account")), head(tr("Type")), head(tr("Balance")));
            for a in arr("accounts") {
                let kind = cashcraft_engine::db::AccountKind::from_code(&st(field(&a, "kind")))
                    .map(|k| k.label())
                    .unwrap_or("");
                println!(
                    "{:>4}  {:<28} {:<18} {:>16}",
                    st(field(&a, "id")),
                    cut(&st(field(&a, "name")), 28),
                    kind,
                    eur(field(&a, "balance"))
                );
            }
            println!("{:>69}", trf("Net worth: {amount}", &[("amount", &eur(field(r, "net_worth")))]));
        }
        "list_transactions" | "transfer_candidates" => {
            println!(
                "{:>5}  {:<10}  {:<16} {:<30} {:<24} {:>13}",
                "ID",
                head(tr("Date")),
                head(tr("Account")),
                head(tr("Payee")),
                head(tr("Category")),
                head(tr("Amount"))
            );
            for t in arr("transactions") {
                println!(
                    "{:>5}  {:<10}  {:<16} {:<30} {:<24} {:>13}",
                    st(field(&t, "id")),
                    date(field(&t, "date")),
                    cut(&st(field(&t, "account")), 16),
                    cut(&st(field(&t, "payee")), 30),
                    cut(&st(field(&t, "category")), 24),
                    eur(field(&t, "amount"))
                );
            }
            let n = count(field(r, "count"));
            println!("{}", trn(n, "{n} transaction", "{n} transactions"));
        }
        "spending_by_category" => {
            println!(
                "{}",
                trf("From {from} to {to}", &[("from", &date(field(r, "from"))), ("to", &date(field(r, "to")))])
            );
            for c in arr("categories") {
                println!("  {:<36} {:>14}", cut(&st(field(&c, "category")), 36), eur(field(&c, "total")));
            }
            println!("  {:<36} {:>14}", tr("Income"), eur(field(r, "income")));
            println!("  {:<36} {:>14}", tr("Expenses"), eur(field(r, "expense")));
            println!("  {:<36} {:>14}", tr("Saved"), eur(field(r, "net")));
        }
        "monthly_summary" => {
            println!(
                "{:<9} {:>14} {:>14} {:>14}",
                head(tr("Month")),
                head(tr("Income")),
                head(tr("Expenses")),
                head(tr("Saved"))
            );
            for m in arr("months") {
                println!(
                    "{:<9} {:>14} {:>14} {:>14}",
                    st(field(&m, "month")),
                    eur(field(&m, "income")),
                    eur(field(&m, "expense")),
                    eur(field(&m, "net"))
                );
            }
        }
        "net_worth" => {
            for m in arr("history") {
                println!("{:<9} {:>16}", st(field(&m, "month")), eur(field(&m, "net_worth")));
            }
            println!("{}", trf("Current net worth: {amount}", &[("amount", &eur(field(r, "net_worth")))]));
        }
        "budget_status" => {
            println!("{}", trf("Budget of {month}", &[("month", &st(field(r, "month")))]));
            println!(
                "  {:<30} {:>12} {:>12} {:>12}",
                head(tr("Category")),
                head(tr("Budget")),
                head(tr("Spent")),
                head(tr("Left"))
            );
            for l in arr("lines") {
                println!(
                    "  {:<30} {:>12} {:>12} {:>12}",
                    cut(&st(field(&l, "category")), 30),
                    eur(field(&l, "budget")),
                    eur(field(&l, "spent")),
                    eur(field(&l, "remaining"))
                );
            }
        }
        "list_categories" => {
            for c in arr("categories") {
                let kind = if field(&c, "is_income").as_bool().unwrap_or(false) { tr("income") } else { tr("expense") };
                println!("{:>4}  {:<36} {}", st(field(&c, "id")), st(field(&c, "path")), kind);
            }
        }
        "list_rules" => {
            for x in arr("rules") {
                println!(
                    "{:>4}  {:<50} → {}",
                    st(field(&x, "id")),
                    cut(&st(field(&x, "pattern")), 50),
                    st(field(&x, "category"))
                );
            }
        }
        "list_scheduled" => {
            for x in arr("scheduled") {
                println!(
                    "{:>4}  {}  {:<16} {:<28} {:>12}  {}",
                    st(field(&x, "id")),
                    date(field(&x, "next_date")),
                    cut(&st(field(&x, "account")), 16),
                    cut(&st(field(&x, "payee")), 28),
                    eur(field(&x, "amount")),
                    cashcraft_engine::db::unit_label(&st(field(&x, "unit")), field(&x, "every").as_i64().unwrap_or(1))
                );
            }
        }
        "list_ledger_accounts" => {
            println!("{:>4}  {:<8} {:<44} {}", "ID", head(tr("Code")), head(tr("Account")), head(tr("Balance")));
            let all = arr("accounts");
            for class in Class::ALL {
                println!("{}", class.label());
                for a in all.iter().filter(|a| st(field(a, "class")) == class.code()) {
                    let balances: Vec<String> = field(a, "balances")
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|b| exact(field(b, "amount"), field(b, "currency")))
                        .collect();
                    println!(
                        "{:>4}  {:<8} {:<44} {}",
                        st(field(a, "ledger_account_id")),
                        cut(&st(field(a, "code")), 8),
                        cut(&st(field(a, "path")), 44),
                        balances.join(", ")
                    );
                }
            }
        }
        "get_entry" => print_entry(field(r, "entry")),
        "list_entries" => arr("entries").iter().for_each(print_entry),
        "trial_balance" => {
            println!("{:<44} {:>20} {:>20}", head(tr("Account")), head(tr("Debit")), head(tr("Credit")));
            let total = tr("Total").to_string();
            for l in arr("lines").iter().chain(&arr("totals")) {
                let name = l.get("account").map_or_else(|| total.clone(), st);
                let cur = field(l, "currency");
                println!(
                    "{:<44} {:>20} {:>20}",
                    cut(&name, 44),
                    exact(field(l, "debit"), cur),
                    exact(field(l, "credit"), cur)
                );
            }
        }
        "verify_ledger" => {
            println!("{}", st(field(r, "message")));
            for p in arr("problems") {
                println!("  {}", st(&p));
            }
        }
        "preview_csv" | "import_csv" => {
            let f = field(r, "format");
            println!(
                "{}",
                trf(
                    "Format: separator '{separator}', encoding {encoding}, decimals '{decimal}', header on row {row}",
                    &[
                        ("separator", &st(field(f, "delimiter"))),
                        ("encoding", &st(field(f, "encoding"))),
                        ("decimal", &st(field(f, "decimal_separator"))),
                        ("row", &st(field(f, "header_row")))
                    ]
                )
            );
            let m = field(f, "mapping");
            for (k, label) in [
                ("date", tr("Date")),
                ("amount", tr("Amount")),
                ("credit", tr("Income")),
                ("debit", tr("Expenses")),
                ("payee", tr("Payee")),
                ("description", tr("Description")),
                ("extra", tr("Details")),
            ] {
                if !m[k].is_null() {
                    println!("  {:<13} ← \"{}\"", label, st(&m[k]));
                }
            }
            if let Some(msg) = r.get("message") {
                println!("{}", st(msg));
                print_transfers(field(r, "result"));
            } else {
                println!(
                    "{}",
                    trf(
                        "Transactions found: {found} (rows skipped: {skipped}), from {from} to {to}, sum {sum}",
                        &[
                            ("found", &st(field(r, "rows_found"))),
                            ("skipped", &st(field(r, "rows_skipped"))),
                            ("from", &date(field(r, "first_date"))),
                            ("to", &date(field(r, "last_date"))),
                            ("sum", &eur(field(r, "sum")))
                        ]
                    )
                );
                for p in arr("preview") {
                    println!(
                        "  {}  {:<40} {:>12}",
                        date(field(&p, "date")),
                        cut(&st(field(&p, "payee")), 40),
                        eur(field(&p, "amount"))
                    );
                }
            }
        }
        _ => {
            if let Some(msg) = r.get("message") {
                println!("{}", st(msg));
            } else {
                println!("{}", serde_json::to_string_pretty(r).unwrap_or_default());
            }
        }
    }
}
