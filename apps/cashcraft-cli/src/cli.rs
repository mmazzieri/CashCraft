//! The command line. Each command becomes an operation of `ops` (the same as in the MCP server)
//! and prints a readable table, or JSON with `--json`. The help is in English; what is printed is
//! in the language in use.

use anyhow::{Result, anyhow};
use cashcraft_engine::db::Db;
use cashcraft_engine::duplicates::Reason;
use cashcraft_engine::i18n::{Lang, tr, trf, trn, with_lang};
use cashcraft_engine::ledger::Class;
use cashcraft_engine::money::{fmt_date, fmt_eur, fmt_plain, parse_amount_with, parse_date};
use cashcraft_engine::ops;
use cashcraft_engine::store::{self, Home, Shared};
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
    /// Import a statement (CSV, .xls, OFX or the PDF of a credit card) into an account
    #[command(visible_alias = "importa")]
    Import {
        file: PathBuf,
        /// Account to import into (name or id)
        #[arg(long, short)]
        account: String,
        /// Show what would be imported, without saving
        #[arg(long)]
        dry_run: bool,
        /// Import a card statement even if its rows do not add up to its totals
        #[arg(long)]
        allow_mismatch: bool,
        /// Import the statement of a card into an account that is not a credit card
        #[arg(long)]
        any_account: bool,
        /// Rows that are in the books already, among those that may be: numbers, with =id for another transaction
        /// than the proposed one ("2,5=31"), and "same" for those that are surely the same
        #[arg(long)]
        skip_rows: Option<String>,
        /// Rows that are other transactions, among those that may be there: numbers, or "all" that are left
        #[arg(long)]
        add_rows: Option<String>,
        #[command(flatten)]
        cols: ColumnArgs,
    },
    /// Analyze a statement (CSV, .xls, OFX or the PDF of a credit card) without importing it
    #[command(visible_alias = "anteprima")]
    Preview {
        file: PathBuf,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Account to look for likely duplicates in (name or id)
        #[arg(long, short)]
        account: Option<String>,
        #[command(flatten)]
        cols: ColumnArgs,
    },
    /// Columns remembered for each layout of CSV and Excel files
    #[command(subcommand, visible_alias = "colonne")]
    Columns(ColumnsCmd),
    /// Credit card statements and the bank's lines that pay them
    #[command(subcommand, visible_alias = "carte")]
    Cards(CardsCmd),
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
    /// The data file: where it is, synced folders, conflicts
    #[command(subcommand, visible_alias = "archivio")]
    Data(crate::data::DataCmd),
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
    /// Do not invert the sign, whatever is remembered for this layout
    #[arg(long, conflicts_with = "invert")]
    no_invert: bool,
    /// Which statement of an OFX file with several: number from 1, or account number
    #[arg(long)]
    statement: Option<String>,
    /// Decimal separator: ",", "." or "auto"
    #[arg(long)]
    decimals: Option<String>,
    /// Order of the dates: dmy, mdy, ymd or auto
    #[arg(long)]
    dates: Option<String>,
    /// Start from the detected columns, and remember nothing
    #[arg(long)]
    no_saved: bool,
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
        if self.invert || self.no_invert {
            m.insert("invert".into(), json!(self.invert));
        }
        m.insert("use_saved".into(), json!(!self.no_saved));
        for (k, v) in [
            ("date_column", &self.date_col),
            ("amount_column", &self.amount_col),
            ("credit_column", &self.credit_col),
            ("debit_column", &self.debit_col),
            ("description_column", &self.desc_col),
            ("payee_column", &self.payee_col),
            ("statement", &self.statement),
            ("decimal_separator", &self.decimals),
            ("date_format", &self.dates),
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
        /// Only with this tag
        #[arg(long)]
        tag: Option<String>,
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
        /// Tags separated by commas
        #[arg(long)]
        tags: Option<String>,
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
        /// Tags separated by commas, instead of those it has ("" removes them)
        #[arg(long)]
        tags: Option<String>,
        #[arg(long)]
        cleared: Option<bool>,
    },
    /// One transaction: what the bank wrote, and the rules that set something of it
    Show { id: i64 },
    /// List the tags
    Tags,
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
pub enum CardsCmd {
    /// List the imported card statements
    Statements,
    /// List the bank transactions that may pay a statement
    Payments,
    /// Make a bank transaction the payment of a statement: a transfer to the card
    Link {
        statement: i64,
        transaction: i64,
        /// The card's own line of the payment, if it is there (default: its side is added)
        #[arg(long)]
        join: Option<i64>,
    },
    /// Undo `link`
    Unlink { statement: i64 },
}

#[derive(Subcommand, Debug)]
pub enum ColumnsCmd {
    /// List them
    List,
    /// Forget those of a layout, or of all
    Forget {
        #[arg(required_unless_present = "all")]
        id: Option<i64>,
        #[arg(long, conflicts_with = "id")]
        all: bool,
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
    /// Add a rule: keywords separated by | and a category, or the options
    Add {
        keywords: Option<String>,
        category: Option<String>,
        /// Categorize like this transaction; the rule goes first
        #[arg(long)]
        like: Option<i64>,
        /// Before the other rules
        #[arg(long, conflicts_with = "last")]
        first: bool,
        /// After the other rules
        #[arg(long)]
        last: bool,
        #[command(flatten)]
        rule: RuleArgs,
    },
    /// Change a rule ("" removes a condition)
    Edit {
        id: i64,
        #[command(flatten)]
        rule: RuleArgs,
    },
    /// Move a rule: first, up, down or last
    Move { id: i64, to: String },
    /// Which transactions a rule matches; nothing is saved
    Test {
        keywords: Option<String>,
        /// Start from this transaction
        #[arg(long, conflicts_with = "id")]
        like: Option<i64>,
        /// Start from this rule
        #[arg(long)]
        id: Option<i64>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[command(flatten)]
        rule: RuleArgs,
    },
    /// Delete a rule
    Delete { id: i64 },
    /// What apply would change
    Preview {
        #[command(flatten)]
        scope: ScopeArgs,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Apply the rules to transactions without a category
    Apply {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Token of a preview: refused if the changes would be others by now
        #[arg(long)]
        token: Option<String>,
    },
    /// Undo the last apply, by its number (without it, says which it is)
    Undo { run: Option<i64> },
}

#[derive(Args, Debug, Default)]
pub struct ScopeArgs {
    /// Also transactions with a category: tags, payees, and the categories that rules set
    #[arg(long)]
    all: bool,
    /// With --all, also the categories and payees set by hand
    #[arg(long, requires = "all")]
    overwrite: bool,
}

impl ScopeArgs {
    fn pairs(&self) -> [(&'static str, Option<Value>); 2] {
        [("scope", v(if self.all { "all" } else { "uncategorized" })), ("overwrite", v(self.overwrite))]
    }
}

#[derive(Args, Debug, Default)]
pub struct RuleArgs {
    /// Text in the payee or the notes: keywords separated by |, or a regular expression
    #[arg(long = "text")]
    text: Option<String>,
    /// Text in the payee
    #[arg(long)]
    payee: Option<String>,
    /// Text in the notes
    #[arg(long)]
    memo: Option<String>,
    /// The texts are regular expressions (--regex=false: keywords)
    #[arg(long, num_args = 0..=1, default_missing_value = "true", require_equals = true)]
    regex: Option<bool>,
    /// Smallest amount, whatever the sign
    #[arg(long)]
    min: Option<String>,
    /// Largest amount
    #[arg(long)]
    max: Option<String>,
    /// any, in or out
    #[arg(long)]
    sign: Option<String>,
    /// Only transactions of this account
    #[arg(long)]
    account: Option<String>,
    /// Category to give
    #[arg(long = "category")]
    into: Option<String>,
    /// Tags to give, separated by commas
    #[arg(long)]
    tags: Option<String>,
    /// New payee; with --regex $1 is what the first group matched
    #[arg(long)]
    rename: Option<String>,
}

impl RuleArgs {
    fn json(&self, keywords: &Option<String>, category: &Option<String>, others: &[(&str, Option<Value>)]) -> Value {
        let mut all = vec![
            ("keywords", o(if keywords.is_some() { keywords } else { &self.text })),
            ("payee", o(&self.payee)),
            ("memo", o(&self.memo)),
            ("regex", self.regex.map(|r| json!(r))),
            ("min_amount", o(&self.min)),
            ("max_amount", o(&self.max)),
            ("sign", o(&self.sign)),
            ("account", o(&self.account)),
            ("category", o(if category.is_some() { category } else { &self.into })),
            ("tags", o(&self.tags)),
            ("rename_payee", o(&self.rename)),
        ];
        all.extend_from_slice(others);
        args(&all)
    }
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

/// How the data file is open.
pub enum Guard {
    /// Where it is.
    Open(Home, PathBuf),
    Shared(Shared),
}

impl Guard {
    /// Runs an operation. On a shared file it is on what others wrote, and what it changed goes out; nothing is held
    /// when it returns.
    fn call(&self, db: &Db, tool: &str, a: &Value) -> Result<Value> {
        let Guard::Shared(shared) = self else { return ops::call(db, tool, a, None) };
        let (done, notices) = store::around(shared, || ops::call(db, tool, a, None))?;
        for n in notices {
            eprintln!("! {n}");
        }
        done
    }
}

pub fn run(db: &Db, guard: &Guard, cmd: Cmd, as_json: bool) -> Result<()> {
    let (tool, a): (&str, Value) = match cmd {
        Cmd::Mcp { automation_read_root, automation_write_root } => {
            // Agents get English, whatever the language of the interface.
            return with_lang(Lang::En, || {
                let roots = ops::Roots::new(&automation_read_root, &automation_write_root)?;
                crate::mcp::serve(db, guard, &roots)
            });
        }
        // Answered before the data file is opened.
        Cmd::Data(_) => return Ok(()),
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
            let r = guard.call(db, &tool, &a)?;
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
            TxCmd::List { account, category, from, to, search, tag, uncategorized, limit } => (
                "list_transactions",
                args(&[
                    ("account", o(&account)),
                    ("category", o(&category)),
                    ("from", o(&from)),
                    ("to", o(&to)),
                    ("search", o(&search)),
                    ("tag", o(&tag)),
                    ("uncategorized", v(uncategorized)),
                    ("limit", v(limit)),
                ]),
            ),
            TxCmd::Add { account, amount, payee, date, category, memo, tags, cleared } => (
                "add_transaction",
                args(&[
                    ("account", v(account)),
                    ("amount", v(amount)),
                    ("payee", v(payee)),
                    ("date", o(&date)),
                    ("category", o(&category)),
                    ("memo", o(&memo)),
                    ("tags", o(&tags)),
                    ("cleared", v(cleared)),
                ]),
            ),
            TxCmd::Edit { id, amount, payee, date, category, memo, tags, cleared } => (
                "update_transaction",
                args(&[
                    ("id", v(id)),
                    ("amount", o(&amount)),
                    ("payee", o(&payee)),
                    ("date", o(&date)),
                    ("category", o(&category)),
                    ("memo", o(&memo)),
                    ("tags", o(&tags)),
                    ("cleared", cleared.map(|c| json!(c))),
                ]),
            ),
            TxCmd::Show { id } => ("get_transaction", args(&[("id", v(id))])),
            TxCmd::Tags => ("list_tags", json!({})),
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
        Cmd::Import { file, account, dry_run, allow_mismatch, any_account, skip_rows, add_rows, cols } => {
            let mut m = Map::new();
            m.insert("path".into(), json!(file));
            m.insert("account".into(), json!(account));
            m.insert("dry_run".into(), json!(dry_run));
            m.insert("allow_mismatch".into(), json!(allow_mismatch));
            m.insert("any_account".into(), json!(any_account));
            for (k, v) in [("skip_rows", skip_rows), ("add_rows", add_rows)] {
                m.extend(v.map(|v| (k.to_string(), json!(v))));
            }
            cols.put(&mut m);
            ("import_csv", Value::Object(m))
        }
        Cmd::Preview { file, limit, account, cols } => {
            let mut m = Map::new();
            m.insert("path".into(), json!(file));
            m.insert("limit".into(), json!(limit));
            m.extend(account.map(|a| ("account".to_string(), json!(a))));
            cols.put(&mut m);
            ("preview_csv", Value::Object(m))
        }
        Cmd::Columns(ColumnsCmd::List) => ("list_saved_columns", json!({})),
        Cmd::Columns(ColumnsCmd::Forget { id, all }) => {
            ("forget_saved_columns", args(&[("id", id.map(|i| json!(i))), ("all", v(all))]))
        }
        Cmd::Cards(c) => match c {
            CardsCmd::Statements => ("list_card_statements", json!({})),
            CardsCmd::Payments => ("propose_card_payments", json!({})),
            CardsCmd::Link { statement, transaction, join } => (
                "link_card_payment",
                args(&[
                    ("statement_id", v(statement)),
                    ("transaction_id", v(transaction)),
                    ("join", join.map(|j| json!(j))),
                ]),
            ),
            CardsCmd::Unlink { statement } => ("unlink_card_payment", args(&[("statement_id", v(statement))])),
        },
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
            RulesCmd::Add { keywords, category, like, first, last, rule } => {
                let place = if first { Some("first") } else { last.then_some("last") };
                let others = [("from_transaction", like.map(|i| json!(i))), ("position", place.map(|p| json!(p)))];
                ("add_rule", rule.json(&keywords, &category, &others))
            }
            RulesCmd::Edit { id, rule } => ("update_rule", rule.json(&None, &None, &[("id", v(id))])),
            RulesCmd::Move { id, to } => ("move_rule", args(&[("id", v(id)), ("position", v(to))])),
            RulesCmd::Test { keywords, like, id, limit, rule } => {
                let others =
                    [("from_transaction", like.map(|i| json!(i))), ("id", id.map(|i| json!(i))), ("limit", v(limit))];
                ("test_rule", rule.json(&keywords, &None, &others))
            }
            RulesCmd::Delete { id } => ("delete_rule", args(&[("id", v(id))])),
            RulesCmd::Preview { scope, limit } => {
                let [a, b] = scope.pairs();
                ("preview_rules", args(&[a, b, ("limit", v(limit))]))
            }
            RulesCmd::Apply { scope, token } => {
                let [a, b] = scope.pairs();
                ("apply_rules", args(&[a, b, ("token", o(&token))]))
            }
            RulesCmd::Undo { run } => ("undo_rules", args(&[("run", run.map(|r| json!(r)))])),
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
    // A remembered sign is told before anything is written.
    if tool == "import_csv" && !as_json && a.get("dry_run") == Some(&json!(false)) {
        let seen = guard.call(db, "preview_csv", &a)?;
        for n in field(&seen, "notices").as_array().into_iter().flatten() {
            println!("! {}", st(n));
        }
    }
    let r = guard.call(db, tool, &a)?;
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

fn statement_line(s: &Value) -> String {
    format!(
        "{:>4}  {:<20} {}  {}  {:>13}",
        st(field(s, "id")),
        cut(&st(field(s, "account")), 20),
        date(field(s, "closing_date")),
        date(field(s, "debit_date")),
        eur(field(s, "amount"))
    )
}

/// The dates and the totals of a card statement that was read.
fn print_statement(s: &Value) {
    let day = |k: &str| match field(s, k) {
        Value::Null => "—".to_string(),
        d => date(d),
    };
    let cards: Vec<String> = field(s, "cards").as_array().into_iter().flatten().map(st).collect();
    println!(
        "{}",
        trf(
            "Card statement {cards}: up to {closing}, debited on {debit}",
            &[("cards", &cards.join(", ")), ("closing", &day("closing_date")), ("debit", &day("debit_date"))]
        )
    );
    for c in field(s, "checks").as_array().into_iter().flatten() {
        let label = match st(field(c, "check")).as_str() {
            "purchases" => tr("Purchases"),
            "instalments" => tr("Instalments"),
            "debit" => tr("Amount debited"),
            _ => tr("Amount debited, on the first page"),
        };
        let mark = if field(c, "ok").as_bool().unwrap_or(false) { "✔" } else { "✖" };
        let said = match field(c, "expected") {
            Value::Null => "—".to_string(),
            v => exact(v, &json!("EUR")),
        };
        println!(
            "  {mark} {}",
            trf(
                "{what}: {said} on the statement, {read} read",
                &[("what", &label), ("said", &said), ("read", &exact(field(c, "found"), &json!("EUR")))]
            )
        );
    }
    let unread = count(field(s, "unread_rows"));
    if unread > 0 {
        println!("  ✖ {}", trn(unread, "{n} row could not be read", "{n} rows could not be read"));
    }
}

/// The statements of an OFX file, and the balance of the chosen one against the books.
fn print_ofx(ofx: &Value, balance: &Value) {
    for s in field(ofx, "statements").as_array().into_iter().flatten() {
        let mark = if field(s, "n") == field(ofx, "chosen") { "→" } else { " " };
        println!(
            "{mark} {:>2}  {:<8} {:<3}  {:>5}  {} – {}",
            st(field(s, "n")),
            st(field(s, "account")),
            st(field(s, "currency")),
            st(field(s, "transactions")),
            date(field(s, "first_date")),
            date(field(s, "last_date"))
        );
    }
    if !ofx.is_null() && field(ofx, "chosen").is_null() {
        println!("{}", tr("The file has several statements: choose one with --statement"));
    }
    if balance.is_null() {
        return;
    }
    let money = |k: &str| match field(balance, k) {
        Value::Null => "—".to_string(),
        v => st(v),
    };
    println!(
        "{}",
        trf(
            "Balance on {date}: {stated} on the statement, {books} in the books",
            &[("date", &date(field(balance, "date"))), ("stated", &money("stated")), ("books", &money("books"))]
        )
    );
}

/// Rows that may be in the books already.
fn print_likely(rows: &[Value]) {
    for l in rows {
        let reason = Reason::from_code(&st(field(l, "reason"))).map_or("", Reason::label);
        let reason = if field(l, "certain") == &json!(true) { tr("the same date, amount and payee") } else { reason };
        let line = format!(
            "{}  {}  {}  {} ({})",
            st(field(l, "row")),
            date(field(l, "date")),
            st(field(l, "payee")),
            eur(field(l, "amount")),
            reason
        );
        println!("{}", trf("It may be there already (--skip-rows or --add-rows): {line}", &[("line", &line)]));
        for t in field(l, "existing").as_array().into_iter().flatten() {
            let mark = if field(t, "id") == field(l, "proposed") { "→" } else { " " };
            println!(
                " {mark}{:>5}  {}  {:<40} {:>13}",
                st(field(t, "id")),
                date(field(t, "date")),
                cut(&st(field(t, "payee")), 40),
                eur(field(t, "amount"))
            );
        }
    }
}

/// What the bank takes back.
fn print_deletions(list: &Value) {
    for d in list.as_array().into_iter().flatten() {
        let t = field(d, "existing");
        let line = format!(
            "{} {} {} {}",
            st(field(t, "id")),
            date(field(t, "date")),
            st(field(t, "payee")),
            eur(field(t, "amount"))
        );
        println!(
            "! {}",
            trf("the bank deletes a transaction, do it by hand if it is right: {line}", &[("line", &line)])
        );
    }
}

/// What may pay the card statements.
fn print_payments(proposals: &[Value]) {
    for p in proposals {
        println!("{}", trf("Card statement {line}", &[("line", &statement_line(field(p, "statement")))]));
        let candidates = field(p, "candidates").as_array().cloned().unwrap_or_default();
        if candidates.is_empty() {
            println!("  {}", tr("no bank transaction matches"));
        }
        for t in candidates {
            println!(
                "  {:>5}  {}  {:<16} {:<30} {:>13}",
                st(field(&t, "id")),
                date(field(&t, "date")),
                cut(&st(field(&t, "account")), 16),
                cut(&st(field(&t, "payee")), 30),
                eur(field(&t, "amount"))
            );
            for l in field(&t, "card_lines").as_array().into_iter().flatten() {
                let line = format!(
                    "{} {} {} {} {}",
                    st(field(l, "id")),
                    date(field(l, "date")),
                    st(field(l, "payee")),
                    eur(field(l, "amount")),
                    st(field(l, "category"))
                );
                println!("         {}", trf("on the card, perhaps the same (--join): {line}", &[("line", &line)]));
            }
        }
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

/// The tags of a transaction, or what the rules give a row.
fn tags(t: &Value) -> String {
    let all: Vec<String> = field(t, "tags").as_array().into_iter().flatten().map(|x| format!("#{}", st(x))).collect();
    all.join(" ")
}

fn rule_numbers(v: &Value) -> String {
    let all: Vec<String> = field(v, "rules").as_array().into_iter().flatten().map(st).collect();
    trf("rules {ids}", &[("ids", &all.join(", "))])
}

/// What the rules do to a row of a preview.
fn print_ruled(v: &Value) {
    if field(v, "rules").as_array().is_none_or(Vec::is_empty) {
        return;
    }
    let what: Vec<String> =
        [st(field(v, "category")), st(field(v, "payee")), tags(v)].into_iter().filter(|t| !t.is_empty()).collect();
    println!("      → {} ({})", what.join(", "), rule_numbers(v));
}

/// Rules that were skipped.
fn print_broken(v: &Value) {
    for b in field(v, "broken_rules").as_array().into_iter().flatten() {
        let (id, what) = (st(field(b, "id")), st(field(b, "error")));
        println!("! {}", trf("rule {id} was skipped: {what}", &[("id", &id), ("what", &what)]));
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

/// The width of a column of ids: those of a shared data file are long.
fn id_width(rows: &[Value], key: &str, least: usize) -> usize {
    rows.iter().map(|r| st(field(r, key)).len()).max().unwrap_or(0).max(least)
}

fn print_human(tool: &str, r: &Value) {
    let arr = |k: &str| r.get(k).and_then(|x| x.as_array()).cloned().unwrap_or_default();
    match tool {
        "list_accounts" => {
            let w = id_width(&arr("accounts"), "id", 4);
            println!("{:>w$}  {:<28} {:<18} {:>16}", "ID", head(tr("Account")), head(tr("Type")), head(tr("Balance")));
            for a in arr("accounts") {
                let kind = cashcraft_engine::db::AccountKind::from_code(&st(field(&a, "kind")))
                    .map(|k| k.label())
                    .unwrap_or("");
                println!(
                    "{:>w$}  {:<28} {:<18} {:>16}",
                    st(field(&a, "id")),
                    cut(&st(field(&a, "name")), 28),
                    kind,
                    eur(field(&a, "balance"))
                );
            }
            println!("{:>1$}", trf("Net worth: {amount}", &[("amount", &eur(field(r, "net_worth")))]), w + 65);
        }
        "get_transaction" => {
            let t = field(r, "transaction");
            println!(
                "{}  {}  {}  {}  {}",
                st(field(t, "id")),
                date(field(t, "date")),
                st(field(t, "payee")),
                st(field(t, "category")),
                eur(field(t, "amount"))
            );
            for (label, k) in [
                (tr("Notes"), "memo"),
                (tr("Payee written by the bank"), "original_payee"),
                (tr("Notes written by the bank"), "original_memo"),
                (tr("Payee typed before the rules"), "typed_payee"),
                (tr("Notes typed before the rules"), "typed_memo"),
            ] {
                let text = st(field(t, k));
                if !text.is_empty() {
                    println!("  {label}: {text}");
                }
            }
            if !field(field(t, "rules"), "memo").is_null() {
                println!("  {}", tr("The notes are the payee that a rule replaced"));
            }
            if !tags(t).is_empty() {
                println!("  {}: {}", tr("Tags"), tags(t));
            }
            for x in arr("rules") {
                let what = st(field(&x, "summary"));
                println!("  {}", trf("rule {id}: {what}", &[("id", &st(field(&x, "id"))), ("what", &what)]));
            }
        }
        "list_tags" => {
            for t in arr("tags") {
                println!("{:>5}  #{}", st(field(&t, "transactions")), st(field(&t, "tag")));
            }
        }
        "test_rule" => {
            println!("{}", st(field(field(r, "rule"), "summary")));
            for t in arr("transactions") {
                println!(
                    "  {:>5}  {}  {:<40} {:>13}",
                    st(field(&t, "id")),
                    date(field(&t, "date")),
                    cut(&st(field(&t, "payee")), 40),
                    eur(field(&t, "amount"))
                );
            }
            let n = count(field(r, "matches"));
            println!("{}", trn(n, "{n} transaction matches", "{n} transactions match"));
        }
        "preview_rules" => {
            for c in arr("changes") {
                println!(
                    "{:>5}  {}  {:<40} {:>13}",
                    st(field(&c, "id")),
                    date(field(&c, "date")),
                    cut(&st(field(&c, "payee")), 40),
                    eur(field(&c, "amount"))
                );
                let mut what = Vec::new();
                for (old, new) in [("category", "new_category"), ("payee", "new_payee")] {
                    let (old, new) = (st(field(&c, old)), st(field(&c, new)));
                    if !new.is_empty() {
                        what.push(format!("{} ➡ {new}", if old.is_empty() { "—" } else { &old }));
                    }
                }
                let new: Vec<String> = field(&c, "new_tags").as_array().into_iter().flatten().map(st).collect();
                what.extend(new.iter().map(|t| format!("#{t}")));
                println!("       {} ({})", what.join(", "), rule_numbers(&c));
            }
            let n = count(field(r, "count"));
            println!("{}", trn(n, "{n} transaction would change", "{n} transactions would change"));
            println!(
                "{}",
                trf("To apply exactly this: rules apply --token {token}", &[("token", &st(field(r, "token")))])
            );
            print_broken(r);
        }
        "apply_rules" => {
            println!("{}", st(field(r, "message")));
            if !field(r, "run").is_null() {
                println!("{}", trf("To undo it: rules undo {run}", &[("run", &st(field(r, "run")))]));
            }
            print_broken(r);
        }
        "list_transactions" | "transfer_candidates" => {
            let w = id_width(&arr("transactions"), "id", 5);
            println!(
                "{:>w$}  {:<10}  {:<16} {:<30} {:<24} {:>13}",
                "ID",
                head(tr("Date")),
                head(tr("Account")),
                head(tr("Payee")),
                head(tr("Category")),
                head(tr("Amount"))
            );
            for t in arr("transactions") {
                println!(
                    "{:>w$}  {:<10}  {:<16} {:<30} {:<24} {:>13}  {}",
                    st(field(&t, "id")),
                    date(field(&t, "date")),
                    cut(&st(field(&t, "account")), 16),
                    cut(&st(field(&t, "payee")), 30),
                    cut(&st(field(&t, "category")), 24),
                    eur(field(&t, "amount")),
                    tags(&t)
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
        "list_rules" | "move_rule" => {
            for x in arr("rules") {
                println!("{:>4}  {}", st(field(&x, "id")), st(field(&x, "summary")));
                let problem = st(field(&x, "problem"));
                if !problem.is_empty() {
                    println!("      ! {problem}");
                }
            }
        }
        "add_rule" | "update_rule" => {
            println!("{:>4}  {}", st(field(r, "id")), st(field(field(r, "rule"), "summary")));
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
            let w = id_width(&arr("accounts"), "ledger_account_id", 4);
            println!("{:>w$}  {:<8} {:<44} {}", "ID", head(tr("Code")), head(tr("Account")), head(tr("Balance")));
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
                        "{:>w$}  {:<8} {:<44} {}",
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
        "list_card_statements" => {
            for s in arr("statements") {
                let paid = match field(&s, "payment_id") {
                    Value::Null => tr("not linked").to_string(),
                    id if field(&s, "matches") == &json!(false) => {
                        trf("transaction {id} pays another amount", &[("id", id)])
                    }
                    id => trf("paid by transaction {id}", &[("id", id)]),
                };
                println!("{}  {paid}", statement_line(&s));
            }
        }
        "list_saved_columns" => {
            for l in arr("saved_columns") {
                println!(
                    "{:>4}  {}  {}",
                    st(field(&l, "id")),
                    st(field(&l, "account")),
                    cut(&st(field(&l, "layout")), 70)
                );
                if field(&l, "readable") == &json!(false) {
                    println!("      {}", tr("it cannot be read: forget it"));
                    continue;
                }
                let columns = field(&l, "columns").as_object().into_iter().flatten();
                let columns: Vec<String> = columns.map(|(k, c)| format!("{k}={}", st(c))).collect();
                let sign = if field(&l, "invert") == &json!(true) { tr("inverted") } else { tr("as in the file") };
                let text = trf(
                    "{columns}; sign {sign}, dates {dates}, decimals {decimals}",
                    &[
                        ("columns", &columns.join(", ")),
                        ("sign", &sign),
                        ("dates", &st(field(&l, "date_format"))),
                        ("decimals", &st(field(&l, "decimal_separator"))),
                    ],
                );
                println!("      {text}");
            }
        }
        "propose_card_payments" => print_payments(&arr("card_payments")),
        "preview_csv" | "import_csv" => {
            let f = field(r, "format");
            let statement = field(r, "statement");
            if !statement.is_null() {
                print_statement(statement);
            }
            let ofx = field(r, "ofx");
            print_ofx(ofx, field(r, "ledger_balance"));
            if statement.is_null() && ofx.is_null() {
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
            }
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
            if !field(f, "saved_columns").is_null() {
                println!("{}", tr("Columns remembered from the last import of this layout"));
            }
            for p in arr("problems") {
                let (line, error) = (st(field(&p, "line")), st(field(&p, "error")));
                println!("  ! {}", trf("line {line}: {error}", &[("line", &line), ("error", &error)]));
            }
            // Those of an import were told before it.
            for n in arr("notices").iter().filter(|_| r.get("message").is_none()) {
                println!("! {}", st(n));
            }
            print_likely(&arr("likely_duplicates"));
            print_deletions(field(r, "deletions"));
            print_deletions(field(field(r, "result"), "deletions"));
            if let Some(msg) = r.get("message") {
                println!("{}", st(msg));
                print_payments(&arr("card_payments"));
                print_transfers(field(r, "result"));
                print_broken(field(r, "result"));
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
                if !field(r, "refused").is_null() {
                    let error = st(field(r, "refused"));
                    println!("! {}", trf("The import would be refused: {error}", &[("error", &error)]));
                }
                for (i, p) in arr("preview").into_iter().enumerate() {
                    println!(
                        "{:>4}  {}  {:<40} {:>12}",
                        i + 1,
                        date(field(&p, "date")),
                        cut(&st(field(&p, "payee")), 40),
                        eur(field(&p, "amount"))
                    );
                    print_ruled(field(&p, "ruled"));
                }
                print_broken(r);
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
