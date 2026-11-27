//! Operations shared by the command line and the MCP server: each takes JSON arguments and
//! returns JSON. Amounts come out in euros (numbers with two decimals). The descriptions of the
//! operations are English in every language; messages and errors are in the language in use.

use crate::db::{self, AccountKind, CategoryTotal, Cents, Db, NewScheduled, NewTx, TxEdit, TxFilter, TxKind};
use crate::duplicates::{Choice, Decision};
use crate::i18n::{Lang, lang, tr, trf, trn};
use crate::importer::{self, Settings};
use crate::layouts;
use crate::ledger::{
    Class, Entry, EntryFilter, MAX_LINES, NewEntry, NewLine, TrialBalance, find_ledger_account, too_big,
};
use crate::money::{DateOrder, checked_total, fmt_date, fmt_eur, fmt_machine, fmt_plain, parse_amount, parse_date};
use crate::rules::{self, Place, Rule, Scope, Sign};
use crate::transfers::OtherSide;
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
/// A whole number.
fn n(desc: &str) -> Value {
    json!({"type": "integer", "description": desc})
}
/// Text or a number: an amount, or an id that may be a name.
fn k(desc: &str) -> Value {
    json!({"type": ["string", "number"], "description": desc})
}
fn rows(desc: &str) -> Value {
    json!({"type": ["string", "number", "array"], "description": desc})
}
fn list(desc: &str) -> Value {
    json!({"type": ["string", "array"], "description": desc})
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
const LEDGER_ACCOUNT: &str = "Account of the chart of accounts: path \"Parent:Child\", name, \"ledger:5\" for ledger_account_id 5, \"code:3010\" for the code, \"system:opening_balances\" or \"system:uncategorized\". A number alone is refused.";
const STATEMENT: &str =
    "Which statement of an OFX file that has several: its number from 1, or the number of the account.";
const DECIMALS: &str = "Decimal separator of the file: \",\", \".\" or \"auto\".";
const DATES: &str = "Order of the dates of the file: dmy, mdy, ymd or auto.";
const SAVED: &str = "Use and remember the columns of this layout in this account (default true); false starts from those detected and remembers nothing.";
const TAGS: &str = "Tags separated by commas.";
const SCOPE: &str = "uncategorized (default): transactions without a category; all: the others too, for tags, payees and the categories that rules set.";
const OVERWRITE: &str = "With scope all, also categories and payees set by hand; only if the user asked.";
const CATEGORY: &str = "Category: id, name or path \"Parent:Child\" (e.g. \"Home:Utilities\").";

/// The parameters of a rule, with others.
fn rule_properties(mut others: Value) -> Value {
    let rule = json!({"keywords": s("Text to find in the payee or the notes: keywords separated by | (e.g. \"bakery|panificio\")."),
        "payee": s("The same, in the payee alone."), "memo": s("The same, in the notes alone."),
        "regex": b("The texts are regular expressions (whatever the case) instead of keywords."),
        "min_amount": k("Smallest amount, whatever the sign, included."), "max_amount": k("Largest amount, included."),
        "sign": s("any, in (money in) or out (money out)."), "account": k("Only transactions of this account: numeric id or name."),
        "category": k(CATEGORY), "tags": list(TAGS),
        "rename_payee": s("New name of the payee; with regex $1 or ${name} is what a group matched (of the payee's expression, else of the keywords', else of the notes'), $$ a dollar.")});
    if let (Some(o), Value::Object(r)) = (others.as_object_mut(), rule) {
        o.extend(r);
    }
    others
}

/// A rule with what the call gives.
fn rule_arg(db: &Db, mut r: Rule, a: &Value) -> Result<Rule> {
    for (k, slot) in [
        ("keywords", &mut r.pattern),
        ("payee", &mut r.payee),
        ("memo", &mut r.memo),
        ("rename_payee", &mut r.rename_payee),
    ] {
        if let Some(t) = opt_str(a, k) {
            *slot = t;
        }
    }
    if let Some(x) = given_bool(a, "regex")? {
        r.regex = x;
    }
    for (k, slot) in [("min_amount", &mut r.amount_min), ("max_amount", &mut r.amount_max)] {
        match a.get(k) {
            None | Some(Value::Null) => {}
            Some(Value::String(t)) if t.trim().is_empty() => *slot = None,
            Some(v) => *slot = Some(exact_amount(v)?),
        }
    }
    if let Some(t) = opt_str(a, "sign") {
        r.sign = Sign::from_code(&t)
            .ok_or_else(|| anyhow!(trf("\"{text}\" is not a sign: use any, in or out", &[("text", &t)])))?;
    }
    if let Some(t) = opt_str(a, "account") {
        let found = if t.trim().is_empty() { None } else { Some(db.find_account(&t)?) };
        r.account = found.as_ref().map(|x| x.name.clone()).unwrap_or_default();
        r.account_id = found.map(|x| x.id);
    }
    if let Some(t) = opt_str(a, "category") {
        let found = if t.trim().is_empty() { None } else { Some(db.find_category(&t)?) };
        r.category = found.as_ref().map(|x| x.path.clone()).unwrap_or_default();
        r.category_id = found.map(|x| x.id);
    }
    if let Some(tags) = tags_arg(a)? {
        r.tags = tags;
    }
    Ok(r)
}

/// Text with commas, or a list.
fn tags_arg(a: &Value) -> Result<Option<Vec<String>>> {
    let wrong = || anyhow!(trf("the parameter \"{name}\" must be text", &[("name", &"tags")]));
    let text = match a.get("tags") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(t)) => t.clone(),
        Some(Value::Array(l)) => {
            let all: Option<Vec<&str>> = l.iter().map(Value::as_str).collect();
            all.ok_or_else(wrong)?.join(",")
        }
        Some(_) => return Err(wrong()),
    };
    rules::parse_tags(&text).map(Some)
}

/// A rule to start from: one that is there, one like a transaction, or an empty one.
fn rule_base(db: &Db, a: &Value) -> Result<Rule> {
    match (opt_i64_strict(a, "id")?, opt_i64_strict(a, "from_transaction")?) {
        (Some(id), None) => db.rule(id),
        (None, Some(t)) => db.rule_from(t),
        (None, None) => Ok(Rule::default()),
        _ => bail!(tr("give the rule or the transaction to start from, not both")),
    }
}

/// Given after a transaction, the keywords take the place of its payee.
fn rule_from_call(db: &Db, a: &Value) -> Result<Rule> {
    let mut base = rule_base(db, a)?;
    let given = |k: &str| opt_str(a, k).is_some_and(|t| !t.trim().is_empty());
    if opt_i64_strict(a, "from_transaction")?.is_some() && ["keywords", "payee", "memo"].iter().any(|k| given(k)) {
        (base.payee, base.memo, base.regex) = (String::new(), String::new(), false);
    }
    rule_arg(db, base, a)
}

/// With the amounts as exact decimal text.
fn rule_json(r: &Rule) -> Value {
    json!({"id": r.id, "pattern": r.pattern, "payee": r.payee, "memo": r.memo, "regex": r.regex,
           "min_amount": r.amount_min.map(fmt_machine), "max_amount": r.amount_max.map(fmt_machine), "sign": r.sign,
           "account_id": r.account_id, "account": r.account, "category_id": r.category_id, "category": r.category,
           "tags": r.tags, "rename_payee": r.rename_payee, "problem": r.problem, "summary": r.summary()})
}

fn scope_arg(a: &Value) -> Result<Scope> {
    let all = match opt_str(a, "scope").unwrap_or_default().trim() {
        "" | "uncategorized" => false,
        "all" => true,
        x => bail!(trf("\"{text}\" is not a scope: use uncategorized or all", &[("text", &x)])),
    };
    let overwrite = opt_bool(a, "overwrite")?;
    if overwrite && !all {
        bail!(tr("what was set by hand is overwritten only with all the transactions"));
    }
    Ok(Scope { all, overwrite })
}

fn with_rules(message: String, gone: usize) -> String {
    if gone == 0 {
        message
    } else {
        format!("{message}; {}", trn(gone, "{n} rule went with it", "{n} rules went with it"))
    }
}

/// A transaction with the rules it comes from.
fn transaction_json(db: &Db, id: i64) -> Result<Value> {
    let t = db.transaction(id)?;
    let all = db.rules()?;
    let used: Vec<Value> = t
        .rules
        .ids()
        .into_iter()
        .map(|id| match all.iter().find(|r| r.id == id) {
            Some(r) => json!({"id": id, "summary": r.summary()}),
            None => json!({"id": id, "summary": tr("deleted")}),
        })
        .collect();
    Ok(json!({"transaction": t, "rules": used}))
}

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
                "opening_balance": k("Opening balance in euros."), "currency": s("Currency (default EUR)."), "iban": s("IBAN (optional).")}),
                &["name"],
            ),
        },
        ToolDef {
            name: "delete_account",
            description: "Deletes an account and all its transactions.",
            schema: obj(json!({"account": k(ACCOUNT)}), &["account"]),
        },
        ToolDef {
            name: "list_transactions",
            description: "Lists transactions, newest first, with optional filters.",
            schema: obj(
                json!({"account": k(ACCOUNT), "category": k(CATEGORY), "from": s("From this day (included)."), "to": s("To this day (included)."),
                "search": s("Text to look for in payee, notes, category, #tags, amount and the text of the bank."), "tag": s("Only transactions with this tag."), "uncategorized": b("Only transactions without a category."),
                "limit": n("Maximum number of transactions (default 100).")}),
                &[],
            ),
        },
        ToolDef {
            name: "add_transaction",
            description: "Records a transaction. Without a category, the automatic rules are applied.",
            schema: obj(
                json!({"account": k(ACCOUNT), "amount": k(AMOUNT), "payee": s("Payee or description."), "date": s(DATE),
                "category": k(CATEGORY), "memo": s("Notes."), "tags": list(TAGS), "cleared": b("Already checked against the bank statement.")}),
                &["account", "amount", "payee"],
            ),
        },
        ToolDef {
            name: "update_transaction",
            description: "Changes a transaction; fields left out stay as they are. Empty category = no category.",
            schema: obj(
                json!({"id": n("Id of the transaction."), "date": s(DATE), "amount": k(AMOUNT), "payee": s("Payee."), "memo": s("Notes."),
                "category": k(CATEGORY), "tags": list("Tags separated by commas, instead of those it has; empty removes them."), "cleared": b("Cleared.")}),
                &["id"],
            ),
        },
        ToolDef {
            name: "get_transaction",
            description: "One transaction, with what the bank wrote and the rules that set its category, payee or tags.",
            schema: obj(json!({"id": n("Id of the transaction.")}), &["id"]),
        },
        ToolDef {
            name: "list_tags",
            description: "Lists the tags of the transactions, with how many have each.",
            schema: obj(json!({}), &[]),
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
                json!({"from": k("Account to transfer from."), "to": k("Account to transfer to."), "amount": k("Positive amount in euros."),
                "date": s(DATE), "memo": s("Notes.")}),
                &["from", "to", "amount"],
            ),
        },
        ToolDef {
            name: "transfer_candidates",
            description: "Lists the transactions of another account that may be the other side of a transaction (opposite amount, within 10 days, nearest first). One of kind transfer is a transfer whose other side this transaction would confirm.",
            schema: obj(
                json!({"id": n("Id of the transaction."), "account": k("The other account: numeric id or name.")}),
                &["id", "account"],
            ),
        },
        ToolDef {
            name: "turn_into_transfer",
            description: "Turns a transaction into a transfer with another account. If transactions there may be the other side already, the call is refused and lists them: ask the user, then pass one as match, or none. Ids, texts, dates and import keys are kept.",
            schema: obj(
                json!({"id": n("Id of the transaction."), "account": k("The other account: numeric id or name."),
                "match": k("\"auto\" (default), \"none\" to add the other side anyway, or the id of the transaction to join.")}),
                &["id", "account"],
            ),
        },
        ToolDef {
            name: "separate_transfer",
            description: "Separates a transfer: both sides become uncategorized transactions.",
            schema: obj(
                json!({"id": n("Id of one side, which stays."), "keep_other": b("Keep the other side too (default true).")}),
                &["id"],
            ),
        },
        ToolDef {
            name: "preview_csv",
            description: "Analyzes a statement (CSV, Excel 97 .xls, OFX, the PDF of a credit card, or a list of an account's movements printed to PDF by a bank, which is read as a CSV and refused when its rows do not add up to its balances) without importing it: separator, recognized columns, first rows and the lines that could not be read; for a card statement its dates and whether the rows add up to its totals; for OFX its statements and the balance they state, a check only; for a list of movements, list: the bank, the IBAN, the opening and closing balances, the account that has that IBAN, and the name proposed for a new one (create_account with that IBAN and the opening balance, when none has it and the user agrees). With an account, also likely_duplicates: rows that may be there already, each with the transactions it may be and the one proposed (certain: same date, amount and payee, and only one).",
            schema: obj(
                json!({"path": s("Path of the file."), "account": k("Account to look for likely duplicates and remembered columns in: numeric id or name (optional)."),
                "statement": k(STATEMENT), "invert": b("Invert the sign of the amounts."), "limit": n("Preview rows (default 10)."),
                "date_column": k("Date column (name, or number from 1)."), "amount_column": k("Amount column."), "credit_column": k("Money-in column."),
                "debit_column": k("Money-out column."), "description_column": k("Description column."), "payee_column": k("Payee column."),
                "decimal_separator": s(DECIMALS), "date_format": s(DATES), "use_saved": b(SAVED)}),
                &["path"],
            ),
        },
        ToolDef {
            name: "import_csv",
            description: "Imports a CSV or Excel 97 .xls file exported by a bank, an OFX statement in the currency of the account, a list of an account's movements printed to PDF by a bank, or the PDF statement of a credit card (into a credit_card account), into an account. Columns, separator, encoding and amount format are detected, or are those of the last import into this account of a file with the same headings; transactions already imported are skipped; rules assign the categories. When rows may be there already from another statement the call is refused and lists them: ask the user, then pass skip_rows and add_rows.",
            schema: obj(
                json!({"path": s("Path of the file."), "account": k(ACCOUNT), "statement": k(STATEMENT), "invert": b("Invert the sign (e.g. credit card statements with positive expenses), or not. When it is not given the sign is that of the last import into this account. It also confirms the sign of an OFX statement that contradicts its types."),
                "dry_run": b("Trial run: the same checks, saves nothing; refused says why the import would be refused."), "date_column": k("Date column (name, or number from 1)."), "amount_column": k("Amount column."),
                "credit_column": k("Money-in column."), "debit_column": k("Money-out column."), "description_column": k("Description column."),
                "payee_column": k("Payee column."), "decimal_separator": s(DECIMALS), "date_format": s(DATES), "use_saved": b(SAVED),
                "skip_rows": rows("Rows of likely_duplicates that the user says are in the books already: numbers separated by commas, each with =id for a transaction other than the proposed one (\"2,5=31\"), and \"same\" for all those that are certain."),
                "add_rows": rows("Rows of likely_duplicates that the user says are other transactions: numbers separated by commas, or \"all\" for those that are left."),
                "allow_mismatch": b("Import a card statement even if its rows do not add up to its totals; only after the user agreed."),
                "limit": n("Preview rows of a trial (default 10)."), "any_account": b("Import the statement of a card into an account that is not a credit card; only after the user agreed.")}),
                &["path", "account"],
            ),
        },
        ToolDef {
            name: "list_saved_columns",
            description: "Lists the columns, formats and sign remembered for each layout of CSV and Excel files in each account.",
            schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "forget_saved_columns",
            description: "Forgets the columns remembered for a layout, so that they are detected again.",
            schema: obj(json!({"id": n("Id of the saved columns."), "all": b("Forget those of every layout.")}), &[]),
        },
        ToolDef {
            name: "list_card_statements",
            description: "Lists the imported credit card statements, newest first: closing date, debit date, amount and the bank transaction linked as payment.",
            schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "propose_card_payments",
            description: "For each card statement without a payment, the bank transactions that may be its monthly debit (same amount, near the debit date). Changes nothing: ask the user, then link_card_payment.",
            schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "link_card_payment",
            description: "Makes a bank transaction the payment of a card statement: it becomes a transfer to the card, no longer an expense. The bank's line keeps its id, date, text and import key.",
            schema: obj(
                json!({"statement_id": n("Id of the card statement."), "transaction_id": n("Id of the bank transaction, one of those proposed."),
                "join": n("Id of the card's own line of the payment, one of its card_lines, if the user says it is the same (default: the side of the card is added).")}),
                &["statement_id", "transaction_id"],
            ),
        },
        ToolDef {
            name: "unlink_card_payment",
            description: "Undoes link_card_payment: the bank's line becomes an uncategorized transaction.",
            schema: obj(json!({"statement_id": n("Id of the card statement.")}), &["statement_id"]),
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
            schema: obj(json!({"category": k(CATEGORY)}), &["category"]),
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
                json!({"category": k(CATEGORY), "monthly_amount": k("Monthly amount in euros.")}),
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
            description: "Lists the automatic rules in the order they run; problem says why one is skipped.",
            schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "add_rule",
            description: "Adds a rule: when all its conditions are met it gives a transaction a category, tags or a cleaner payee. The first rule that matches decides the category. With from_transaction, what is not given is taken from that transaction.",
            schema: obj(
                rule_properties(json!({"from_transaction": n("Id of a transaction to categorize others like."),
                    "position": s("Where: last (default), or first (default with from_transaction).")})),
                &[],
            ),
        },
        ToolDef {
            name: "update_rule",
            description: "Changes a rule; what is left out stays, empty text removes it.",
            schema: obj(rule_properties(json!({"id": n("Id of the rule.")})), &["id"]),
        },
        ToolDef {
            name: "move_rule",
            description: "Moves a rule among the others.",
            schema: obj(
                json!({"id": n("Id of the rule."), "position": s("first, up, down or last.")}),
                &["id", "position"],
            ),
        },
        ToolDef {
            name: "test_rule",
            description: "How many transactions a rule would match, whatever the other rules say, and the newest of them. Takes the parameters of add_rule, or the id of a rule too, and saves nothing.",
            schema: obj(
                rule_properties(json!({"id": n("Id of a rule to start from."),
                    "from_transaction": n("Id of a transaction to start from."),
                    "limit": n("How many transactions to list (default 10).")})),
                &[],
            ),
        },
        ToolDef {
            name: "delete_rule",
            description: "Deletes a rule. What it did to transactions stays.",
            schema: obj(json!({"id": n("Id of the rule.")}), &["id"]),
        },
        ToolDef {
            name: "preview_rules",
            description: "What apply_rules would change, transaction by transaction, with the rules; changes nothing. Show it to the user, then pass its token to apply_rules.",
            schema: obj(
                json!({"scope": s(SCOPE), "overwrite": b(OVERWRITE), "limit": n("How many changes to list (default 100).")}),
                &[],
            ),
        },
        ToolDef {
            name: "apply_rules",
            description: "Applies the rules to the transactions without a category, or to all. Transfers and splits are left alone. undo_rules undoes the last time.",
            schema: obj(
                json!({"scope": s(SCOPE), "overwrite": b(OVERWRITE),
                "token": s("Token of preview_rules: refused if the changes would be others by now.")}),
                &[],
            ),
        },
        ToolDef {
            name: "undo_rules",
            description: "Undoes the last apply_rules that changed something. Transactions changed since stay as they are and are listed in skipped. Ask the user first, saying which run it is.",
            schema: obj(json!({"run": n("The run: from apply_rules, or can_undo of list_rules.")}), &["run"]),
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
                json!({"account": k(ACCOUNT), "amount": k(AMOUNT), "payee": s("Payee."), "next_date": s("Next due date."),
                "every": n("Every how many units (default 1)."), "unit": s("day, week, month, year (default month)."),
                "category": k(CATEGORY), "memo": s("Notes."), "to_account": k("Account to transfer to, for a transfer.")}),
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
                json!({"path": s("File to write."), "account": k(ACCOUNT), "from": s("From this day."), "to": s("To this day.")}),
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
                "parent": k("Account above it, of the same class (optional); written as in post_entry."), "code": k("Code, unique (optional)."),
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
                    json!({"account": k(LEDGER_ACCOUNT), "amount": k("Exact amount with at most two decimals, a number or text like \"-12.50\": debit positive, credit negative."),
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
                json!({"ledger_account": k(LEDGER_ACCOUNT), "from": s("From this date."), "to": s("Up to this date."),
                "limit": n("Maximum number of entries (default 100, at most 1000)."), "offset": n("How many to skip, for the next page.")}),
                &[],
            ),
        },
        ToolDef {
            name: "delete_entry",
            description: "Deletes a whole entry, every side of it. Opening balances are changed in their account instead.",
            schema: obj(json!({"id": n("Id of the entry.")}), &["id"]),
        },
        ToolDef {
            name: "set_posting_category",
            description: "Gives one posting on a category (or uncategorized) of an entry to another category; the way to re-categorize part of a split.",
            schema: obj(
                json!({"posting_id": n("Id of the posting, from get_entry."), "category": k("Category: id, name or path (empty: uncategorized).")}),
                &["posting_id"],
            ),
        },
        ToolDef {
            name: "rename_ledger_account",
            description: "Renames an account added to the chart by hand.",
            schema: obj(
                json!({"ledger_account": k(LEDGER_ACCOUNT), "name": s("New name.")}),
                &["ledger_account", "name"],
            ),
        },
        ToolDef {
            name: "delete_ledger_account",
            description: "Deletes an account added to the chart by hand, when it has no entries and no accounts under it.",
            schema: obj(json!({"ledger_account": k(LEDGER_ACCOUNT)}), &["ledger_account"]),
        },
        ToolDef {
            name: "set_account_currency",
            description: "Changes the currency of an account and of its entries, amounts unchanged. Refused when an entry also moves another account.",
            schema: obj(
                json!({"account": k(ACCOUNT), "currency": s("Three-letter code, like EUR.")}),
                &["account", "currency"],
            ),
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

/// Refuses a call with a parameter of another type than the one declared: ignored, it would do something else
/// than what was asked. Null is a parameter that is not given.
fn check_types(schema: &Value, a: &Value) -> Result<()> {
    let given = match a {
        Value::Null => return Ok(()),
        Value::Object(o) => o,
        _ => bail!(tr("the parameters must be an object")),
    };
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else { return Ok(()) };
    for (name, value) in given {
        let Some(p) = properties.get(name).filter(|_| !value.is_null()) else { continue };
        let types: Vec<&str> = match p.get("type") {
            Some(Value::String(t)) => vec![t],
            Some(Value::Array(l)) => l.iter().filter_map(Value::as_str).collect(),
            _ => continue,
        };
        if !types.iter().any(|t| is_of_type(value, t)) {
            let wanted = types.join(", ");
            bail!(trf("the parameter \"{name}\" must be of type {type}", &[("name", name), ("type", &wanted)]));
        }
        if let (Some(items), Value::Array(all)) = (p.get("items"), value) {
            for x in all {
                if !x.is_object() {
                    bail!(trf(
                        "the parameter \"{name}\" must be of type {type}",
                        &[("name", name), ("type", &"object")]
                    ));
                }
                check_types(items, x)?;
            }
        }
    }
    Ok(())
}

fn is_of_type(v: &Value, t: &str) -> bool {
    match t {
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => whole(v).is_some(),
        "boolean" => v.is_boolean(),
        "array" => v.is_array(),
        _ => v.is_object(),
    }
}

/// Text or a number; `call` has refused the rest.
fn opt_str(a: &Value, k: &str) -> Option<String> {
    match a.get(k)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The arguments, but one.
fn without(a: &Value, k: &str) -> Value {
    let mut a = a.clone();
    if let Some(o) = a.as_object_mut() {
        o.remove(k);
    }
    a
}

fn missing(k: &str) -> anyhow::Error {
    anyhow!(trf("the parameter \"{name}\" is missing", &[("name", &k)]))
}

fn req_str(a: &Value, k: &str) -> Result<String> {
    opt_str(a, k).filter(|s| !s.trim().is_empty()).ok_or_else(|| missing(k))
}

/// `false` when it is not given; anything but a boolean is an error.
fn opt_bool(a: &Value, k: &str) -> Result<bool> {
    Ok(given_bool(a, k)?.unwrap_or(false))
}

fn given_bool(a: &Value, k: &str) -> Result<Option<bool>> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => bail!(trf("the parameter \"{name}\" must be true or false", &[("name", &k)])),
    }
}

/// A whole number, also written 5.0.
fn whole(v: &Value) -> Option<i64> {
    let exact = |f: &f64| f.fract() == 0.0 && f.abs() <= 9_007_199_254_740_992.0;
    v.as_i64().or_else(|| v.as_f64().filter(|_| v.is_f64()).filter(exact).map(|f| f as i64))
}

fn opt_i64(a: &Value, k: &str) -> Option<i64> {
    whole(a.get(k)?)
}

/// `None` when it is not given; anything but a whole number is an error.
fn opt_i64_strict(a: &Value, k: &str) -> Result<Option<i64>> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => req_i64(a, k).map(Some),
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
            json!({"id": p.id, "ledger_account_id": p.account_id, "account": p.account, "amount": fmt_machine(p.amount),
                   "currency": p.currency, "cleared": p.cleared, "date": p.date})
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
            json!({"ledger_account_id": l.account_id, "account": l.account, "code": l.code, "class": l.class,
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

fn mapping_overrides(t: &importer::Table, saved: &importer::Mapping, a: &Value) -> Result<importer::Mapping> {
    let mut m = saved.clone();
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

/// How to read a file: the settings remembered or detected, with what the call changes.
fn file_settings(db: &Db, t: &importer::Table, account: Option<i64>, a: &Value) -> Result<(Settings, Option<i64>)> {
    let (mut s, saved) = match given_bool(a, "use_saved")? {
        Some(false) => (Settings::detected(t), None),
        _ => Settings::of(db, t, account)?,
    };
    s.mapping = mapping_overrides(t, &s.mapping, a)?;
    if let Some(i) = given_bool(a, "invert")? {
        s.invert = i;
    }
    match opt_str(a, "decimal_separator").as_deref().map(str::trim) {
        None | Some("") => {}
        Some(",") => s.decimal_comma = Some(true),
        Some(".") => s.decimal_comma = Some(false),
        Some("auto") => s.decimal_comma = None,
        Some(x) => bail!(trf("\"{text}\" is not a decimal separator: use , . or auto", &[("text", &x)])),
    }
    if let Some(d) = opt_str(a, "date_format").filter(|d| !d.trim().is_empty()) {
        s.dates = DateOrder::from_code(&d).ok_or_else(|| {
            anyhow!(trf("\"{text}\" is not an order of dates: use dmy, mdy, ymd or auto", &[("text", &d)]))
        })?;
    }
    if t.ofx.is_some() && s != (Settings { invert: s.invert, ..Settings::detected(t) }) {
        bail!(tr("an OFX statement has no columns to change"));
    }
    if t.statement.is_some() && s != Settings::detected(t) {
        bail!(tr("the columns and the sign of a card statement cannot be changed"));
    }
    if s.mapping.date.is_none() && t.has_columns() {
        bail!(tr("the date column was not found: give it with date_column"));
    }
    let m = &s.mapping;
    if m.amount.is_none() && m.credit.is_none() && m.debit.is_none() && t.has_columns() {
        bail!(tr("the amount column was not found: give it with amount_column or credit_column/debit_column"));
    }
    Ok((s, saved))
}

/// The parts of a parameter with rows: text, a number, or a list of them.
fn rows_arg(a: &Value, k: &str) -> Result<Vec<String>> {
    let wrong = || anyhow!(trf("the parameter \"{name}\" must have row numbers", &[("name", &k)]));
    let one = |v: &Value| match v {
        Value::String(t) => Ok(t.clone()),
        Value::Number(n) => Ok(n.to_string()),
        _ => Err(wrong()),
    };
    let text = match a.get(k) {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(l)) => l.iter().map(one).collect::<Result<Vec<_>>>()?.join(","),
        Some(v) => one(v)?,
    };
    Ok(text.split([',', ' ']).filter(|p| !p.is_empty()).map(str::to_lowercase).collect())
}

/// What the call says of the rows that may be there already.
fn decision(a: &Value) -> Result<Decision> {
    let mut d = Decision::default();
    for (k, skip) in [("skip_rows", true), ("add_rows", false)] {
        let wrong = || anyhow!(trf("the parameter \"{name}\" must have row numbers", &[("name", &k)]));
        for part in rows_arg(a, k)? {
            match (part.as_str(), skip) {
                ("same", true) => d.skip_same = true,
                ("all", false) => d.add_others = true,
                ("all", true) => {
                    bail!(tr(
                        "rows that may be there are skipped one by one, or with \"same\" when they are surely the same"
                    ))
                }
                (part, _) => {
                    let (row, id) = match part.split_once('=') {
                        Some((r, id)) if skip => (r, Some(id.parse::<i64>().map_err(|_| wrong())?)),
                        Some(_) => return Err(wrong()),
                        None => (part, None),
                    };
                    let row = row.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).ok_or_else(wrong)?;
                    let choice = if skip { Choice::Skip(id) } else { Choice::Add };
                    if d.each.insert(row, choice).is_some_and(|old| old != choice) {
                        bail!(tr("the same rows cannot be both skipped and added"));
                    }
                }
            }
        }
    }
    Ok(d)
}

/// The balance that an OFX statement or a list of movements states, against the books.
fn ledger_balance(db: &Db, account: Option<i64>, t: &importer::Table) -> Result<Value> {
    let Some((stated, on)) = t.stated_balance() else {
        return Ok(Value::Null);
    };
    let books = match (account, on) {
        (Some(a), Some(d)) => Some(db.balance_on(a, d)?),
        _ => None,
    };
    Ok(json!({"stated": fmt_machine(stated), "date": on, "books": books.map(fmt_machine),
              "agrees": books.map(|b| b == stated)}))
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
    if let Some(t) = tools().iter().find(|t| t.name == name) {
        check_types(&t.schema, a)?;
    }
    let v = call_raw(db, name, a, roots)?;
    Ok(to_eur(v))
}

/// [`call`] with amounts in whole cents: exact, whatever reads the JSON. No file is reached by path
/// except by the operations that take one.
pub fn call_in_cents(db: &Db, name: &str, a: &Value) -> Result<Value> {
    call_raw(db, name, a, None)
}

fn call_raw(db: &Db, name: &str, a: &Value, roots: Option<&Roots>) -> Result<Value> {
    Ok(match name {
        "list_accounts" => {
            let accts = db.accounts(opt_bool(a, "include_archived")?)?;
            json!({"accounts": accts, "net_worth": db.net_worth(None)?})
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
            let gone = db.rules_of_account(acc.id)?;
            db.delete_account(acc.id)?;
            let message = trf("account \"{name}\" deleted", &[("name", &acc.name)]);
            json!({"message": with_rules(message, gone), "rules_deleted": gone})
        }
        "list_transactions" => {
            let mut f = TxFilter {
                from: opt_date(a, "from")?,
                to: opt_date(a, "to")?,
                search: opt_str(a, "search").unwrap_or_default(),
                uncategorized: opt_bool(a, "uncategorized")?,
                tag: opt_str(a, "tag")
                    .filter(|t| !t.trim().is_empty())
                    .map(|t| t.trim().trim_start_matches('#').into()),
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
            let tx = NewTx {
                account_id: acc.id,
                date: opt_date(a, "date")?.unwrap_or_else(db::today),
                amount: req_amount(a, "amount")?,
                payee: req_str(a, "payee")?,
                memo: opt_str(a, "memo").unwrap_or_default(),
                category_id,
                cleared: opt_bool(a, "cleared")?,
                import_hash: None,
                import_marks: None,
                card_row: false,
            };
            db.atomically(|| {
                let id = db.add_transaction(&tx)?.ok_or_else(|| anyhow!(tr("the transaction was not saved")))?;
                if let Some(tags) = tags_arg(a)? {
                    db.add_tags(id, &tags)?;
                }
                Ok(json!({"id": id, "transaction": db.transaction(id)?}))
            })?
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
                if opt_str(a, "category").is_some() && t.kind != TxKind::Transfer {
                    // Also without a category, or the one it has: asked for, it is the user's choice.
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
                        cleared: given_bool(a, "cleared")?.unwrap_or(t.cleared),
                    },
                )?;
                if let Some(tags) = tags_arg(a)? {
                    db.set_tags(id, &tags)?;
                }
                Ok(json!({"transaction": db.transaction(id)?}))
            })?
        }
        "get_transaction" => transaction_json(db, req_i64(a, "id")?)?,
        "list_tags" => {
            let all: Vec<Value> = db.tags()?.into_iter().map(|(t, n)| json!({"tag": t, "transactions": n})).collect();
            json!({"tags": all})
        }
        "delete_transaction" => {
            let id = req_i64(a, "id")?;
            let t = db.transaction(id)?;
            let lines = db.entry(t.entry_id)?.postings.len();
            db.delete_transaction(id)?;
            let mut message = trf("transaction {id} deleted", &[("id", &id)]);
            if t.kind == TxKind::Split {
                message = format!("{message} ({})", trn(lines, "{n} line of its entry", "{n} lines of its entry"));
            }
            json!({"message": message, "lines": lines})
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
        "transfer_candidates" => {
            let acc = db.find_account(&req_str(a, "account")?)?;
            let found = db.transfer_candidates(req_i64(a, "id")?, acc.id)?;
            json!({"count": found.len(), "transactions": found})
        }
        "turn_into_transfer" => {
            let id = req_i64(a, "id")?;
            let acc = db.find_account(&req_str(a, "account")?)?;
            let other = match opt_str(a, "match").unwrap_or_default().trim() {
                "" | "auto" => OtherSide::Auto,
                "none" => OtherSide::New,
                x => OtherSide::Join(x.parse().map_err(|_| {
                    anyhow!(trf("\"{match}\" is neither auto, none nor the id of a transaction", &[("match", &x)]))
                })?),
            };
            let done = db.turn_into_transfer(id, acc.id, other)?;
            let message =
                if done.joined { tr("the two transactions are one transfer now") } else { tr("transfer recorded") };
            json!({"ids": [done.id, done.other_id], "joined": done.joined, "message": message})
        }
        "separate_transfer" => {
            let id = req_i64(a, "id")?;
            let keep = given_bool(a, "keep_other")?.unwrap_or(true);
            let other = db.separate_transfer(id, keep)?;
            json!({"id": id, "other_id": other, "message": tr("transfer separated")})
        }
        "preview_csv" | "import_csv" => {
            let raw = req_str(a, "path")?;
            let path = match roots {
                Some(r) => r.read_path(&raw)?,
                None => PathBuf::from(raw),
            };
            let mut t = importer::load(&path)?;
            if let Some(key) = opt_str(a, "statement").filter(|k| !k.trim().is_empty()) {
                match &mut t.ofx {
                    Some(o) => o.choose(&key)?,
                    None => bail!(tr("only an OFX file has statements to choose")),
                }
            }
            let dry = name == "preview_csv" || opt_bool(a, "dry_run")?;
            let account = match opt_str(a, "account").filter(|k| !k.trim().is_empty()) {
                Some(k) => Some(db.find_account(&k)?),
                None if name == "import_csv" => return Err(missing("account")),
                None => None,
            };
            let id = account.as_ref().map(|x| x.id);
            let (settings, saved) = file_settings(db, &t, id, a)?;
            let remembered = saved.is_some() && given_bool(a, "invert")?.is_none();
            let sign_of = account.as_ref().filter(|_| remembered).map(|x| x.name.as_str());
            let notices = layouts::notices(&t, &settings, sign_of);
            let read = importer::read_rows(&t, &settings);
            let rows = &read.rows;
            let set = db.ruleset()?;
            let new = importer::rows_to_add(db, id, rows, &decision(a)?)?;
            let ruled = |(i, r): (usize, &importer::ParsedRow)| -> Result<Value> {
                let mut row = serde_json::to_value(r)?;
                let mut outcome = rules::Outcome::default();
                if new.contains(&i) {
                    outcome = set.evaluate(&rules::Subject::new(&r.payee, &r.memo, r.amount, id));
                }
                if let Some(o) = row.as_object_mut() {
                    o.insert("ruled".into(), set.json(&outcome));
                }
                Ok(row)
            };
            let mut info = importer::describe(&t, &settings.mapping);
            if let Some(i) = info.as_object_mut() {
                i.insert("invert".into(), json!(settings.invert));
                if t.has_columns() {
                    i.insert("decimal_separator".into(), json!(if settings.comma(&t) { "," } else { "." }));
                    i.insert("date_format".into(), json!(settings.dates.code()));
                    i.insert("saved_columns".into(), json!(saved));
                }
            }
            let ofx = t.ofx.as_ref().map(crate::ofx::Ofx::json);
            let statement = t.statement.as_ref().map(importer::Statement::json);
            let import = || -> Result<(importer::ImportResult, Value)> {
                let acc = account.as_ref().ok_or_else(|| missing("account"))?;
                if let Some(o) = t.ofx.as_ref().filter(|o| o.chosen.is_none()) {
                    return Err(importer::choose_statement(o));
                }
                let of_card = t.statement.is_some()
                    || t.ofx
                        .as_ref()
                        .and_then(|o| o.statement())
                        .is_some_and(|s| s.kind == crate::ofx::Kind::CreditCard);
                if of_card && acc.kind != AccountKind::CreditCard && !opt_bool(a, "any_account")? {
                    bail!(trf(
                        "\"{name}\" is not a credit card: nothing was imported (any_account to import all the same)",
                        &[("name", &acc.name)]
                    ));
                }
                let remember = given_bool(a, "use_saved")?.unwrap_or(true) && read.is_convincing();
                let choices = importer::Choices {
                    allow_mismatch: opt_bool(a, "allow_mismatch")?,
                    likely: decision(a)?,
                    settings: remember.then_some(&settings),
                    sign_confirmed: given_bool(a, "invert")?.is_some(),
                };
                db.atomically(|| {
                    let res = importer::import_with(db, acc.id, &t, rows, read.skipped, &choices)?;
                    Ok((res, ledger_balance(db, Some(acc.id), &t)?))
                })
            };
            if dry {
                let accounts = db.accounts(false)?;
                let limit = opt_i64(a, "limit").unwrap_or(10).max(1) as usize;
                let total = checked_total(rows.iter().map(|r| r.amount)).ok_or_else(too_big)?;
                let likely: Vec<Value> = db
                    .likely_duplicates(id, rows)?
                    .iter()
                    .map(|l| {
                        json!({"row": l.index + 1, "reason": l.reason, "date": l.date, "payee": l.payee,
                               "amount": l.amount, "existing": l.existing, "proposed": l.proposed,
                               "certain": l.certain})
                    })
                    .collect();
                // The import itself, undone: its checks are the same.
                let mut refused = None;
                let mut result = None;
                if name == "import_csv" {
                    let undone: Result<()> = db.atomically(|| {
                        result = Some(import()?.0);
                        bail!("undone")
                    });
                    if let (Err(e), None) = (undone, &result) {
                        refused = Some(format!("{e:#}"));
                    }
                }
                let counts = result.map(|r| {
                    json!({"added": r.added, "duplicates": r.duplicates, "same": r.same, "adopted": r.adopted.len()})
                });
                json!({"format": info, "rows_found": rows.len(), "rows_skipped": read.skipped, "sum": total,
                       "first_date": rows.first().map(|r| r.date), "last_date": rows.last().map(|r| r.date),
                       "preview": rows.iter().enumerate().take(limit).map(ruled).collect::<Result<Vec<_>>>()?,
                       "broken_rules": set.broken, "problems": read.problems,
                       "likely_duplicates": likely, "deletions": db.deletions(id, rows)?, "notices": notices,
                       "refused": refused, "would_import": counts, "statement": statement, "ofx": ofx,
                       "list": t.list.as_ref().map(|l| l.json(&accounts)),
                       "ledger_balance": ledger_balance(db, id, &t)?})
            } else {
                let (res, balance) = import()?;
                let proposals: Vec<_> =
                    db.propose_card_payments()?.into_iter().filter(|p| !p.candidates.is_empty()).collect();
                let present = res.duplicates + res.same;
                json!({"account": account.as_ref().map(|x| &x.name), "format": info, "result": res, "card_payments": proposals,
                       "statement": statement, "ofx": ofx, "problems": read.problems, "notices": notices,
                       "ledger_balance": balance,
                       "message": trf("{added} imported, {present} already present, {categorized} categorized automatically", &[("added", &res.added), ("present", &present), ("categorized", &res.categorized)])})
            }
        }
        "list_saved_columns" => {
            let all: Vec<Value> = db.layouts()?.iter().map(layouts::Layout::json).collect();
            json!({"saved_columns": all})
        }
        "forget_saved_columns" => {
            let n = match (opt_bool(a, "all")?, opt_i64_strict(a, "id")?) {
                (true, None) => db.forget_layouts()?,
                (false, Some(id)) => {
                    db.forget_layout(id)?;
                    1
                }
                _ => bail!(tr("give the id of the saved columns, or all")),
            };
            json!({"forgotten": n, "message": tr("the saved columns were forgotten")})
        }
        "list_card_statements" => json!({"statements": db.card_statements()?}),
        "propose_card_payments" => json!({"card_payments": db.propose_card_payments()?}),
        "link_card_payment" => {
            let (statement, transaction) = (req_i64(a, "statement_id")?, req_i64(a, "transaction_id")?);
            db.link_card_payment(statement, transaction, opt_i64_strict(a, "join")?)?;
            json!({"statement_id": statement, "transaction": db.transaction(transaction)?,
                   "message": tr("the bank's line is now the transfer that pays the card statement")})
        }
        "unlink_card_payment" => {
            let statement = req_i64(a, "statement_id")?;
            db.unlink_card_payment(statement)?;
            json!({"statement_id": statement, "message": tr("card payment unlinked")})
        }
        "list_categories" => json!({"categories": db.categories()?}),
        "create_category" => {
            let id = db.category_by_path(&req_str(a, "path")?, opt_bool(a, "income")?, true)?;
            json!({"id": id})
        }
        "delete_category" => {
            let c = db.find_category(&req_str(a, "category")?)?;
            let gone = db.rules_of_category(c.id)?;
            db.delete_category(c.id)?;
            let message = trf("category \"{name}\" deleted", &[("name", &c.path)]);
            json!({"message": with_rules(message, gone), "rules_deleted": gone})
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
            let (income, expense) = CategoryTotal::income_and_expense(&totals)?;
            json!({"from": from, "to": to, "income": income, "expense": expense, "net": income.saturating_sub(expense), "categories": totals})
        }
        "monthly_summary" => {
            let months = opt_i64(a, "months").unwrap_or(12).clamp(1, 120) as u32;
            let rows: Vec<Value> = db
                .monthly_totals(months, db::today())?
                .into_iter()
                .map(|m| json!({"month": m.month, "income": m.income, "expense": m.expense, "net": m.income.saturating_sub(m.expense)}))
                .collect();
            json!({"months": rows})
        }
        "net_worth" => {
            let months = opt_i64(a, "months").unwrap_or(12).clamp(1, 120) as u32;
            let hist: Vec<Value> = db
                .net_worth_history(months, db::today())?
                .into_iter()
                .map(|(m, v)| json!({"month": m, "net_worth": v}))
                .collect();
            json!({"net_worth": db.net_worth(None)?, "history": hist})
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
                .map(|l| json!({"category": l.category, "budget": l.budget, "spent": l.spent, "remaining": l.budget.saturating_sub(l.spent)}))
                .collect();
            json!({"month": m.format("%Y-%m").to_string(), "lines": lines})
        }
        "list_rules" => {
            let last = db.last_rule_run()?;
            json!({"rules": db.rules()?.iter().map(rule_json).collect::<Vec<_>>(), "can_undo": last})
        }
        "add_rule" => {
            let from = opt_i64_strict(a, "from_transaction")?.is_some();
            let first = match opt_str(a, "position").map(|p| Place::from_code(&p)) {
                None => from,
                Some(Some(Place::First)) => true,
                Some(Some(Place::Last)) => false,
                Some(_) => bail!(tr("a new rule goes first or last")),
            };
            let rule = rule_from_call(db, &without(a, "id"))?;
            db.atomically(|| {
                let id = db.insert_rule(&rule, first)?;
                Ok(json!({"id": id, "rule": rule_json(&db.rule(id)?)}))
            })?
        }
        "update_rule" => {
            let id = req_i64(a, "id")?;
            let rule = rule_arg(db, db.rule(id)?, a)?;
            db.update_rule(&rule)?;
            json!({"id": id, "rule": rule_json(&db.rule(id)?), "message": tr("rule changed")})
        }
        "move_rule" => {
            let place = req_str(a, "position")?;
            let place = Place::from_code(&place).ok_or_else(|| {
                anyhow!(trf("\"{text}\" is not a place: use first, up, down or last", &[("text", &place)]))
            })?;
            db.move_rule(req_i64(a, "id")?, place)?;
            json!({"rules": db.rules()?.iter().map(rule_json).collect::<Vec<_>>()})
        }
        "test_rule" => {
            let rule = rule_from_call(db, a)?;
            let all = db.transactions(&TxFilter::default())?;
            let found = db.rule_matches(&rule, &all)?;
            let limit = usize::try_from(opt_i64(a, "limit").unwrap_or(10).max(0)).unwrap_or(0);
            let shown: Vec<&db::Transaction> =
                found.iter().take(limit).filter_map(|id| all.iter().find(|t| t.id == *id)).collect();
            json!({"matches": found.len(), "rule": rule_json(&rule), "transactions": shown})
        }
        "delete_rule" => {
            db.delete_rule(req_i64(a, "id")?)?;
            json!({"message": tr("rule deleted")})
        }
        "preview_rules" => {
            let plan = db.plan_rules(scope_arg(a)?)?;
            let limit = usize::try_from(opt_i64(a, "limit").unwrap_or(100).max(0)).unwrap_or(0);
            let (count, token, broken) = (plan.changes.len(), plan.token, plan.broken);
            json!({"count": count, "changes": plan.changes.into_iter().take(limit).collect::<Vec<_>>(),
                   "token": token, "broken_rules": broken})
        }
        "apply_rules" => {
            let run = db.run_rules(scope_arg(a)?, opt_str(a, "token").as_deref())?;
            let n = run.changed;
            json!({"run": run.run, "categorized": run.categorized, "changed": n, "broken_rules": run.broken,
                   "message": trn(n, "{n} transaction changed by the rules", "{n} transactions changed by the rules")})
        }
        "undo_rules" => {
            let Some(run) = opt_i64_strict(a, "run")? else {
                let last = db.last_rule_run()?.ok_or_else(|| anyhow!(tr("there is no run of the rules to undo")))?;
                bail!(trf("say which run to undo: {what}", &[("what", &last.text())]));
            };
            let done = db.undo_run(run)?;
            json!({"undone": done.undone, "skipped": done.skipped, "message": done.message()})
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
            let p = db.post_scheduled(until)?;
            json!({"posted": p.posted, "failed": p.failed, "message": p.message()})
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
                    json!({"ledger_account_id": l.id, "class": l.class, "parent_id": l.parent_id, "name": l.name, "path": l.path,
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
            json!({"ledger_account_id": id, "message": tr("account created")})
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
            let entry = NewEntry {
                date: opt_date(a, "date")?.unwrap_or_else(db::today),
                payee: opt_str(a, "payee").unwrap_or_default(),
                memo: opt_str(a, "memo").unwrap_or_default(),
                lines,
            };
            db.atomically(|| {
                let id = db.post_entry(&entry)?;
                Ok(json!({"id": id, "entry": entry_json(&db.entry(id)?), "message": tr("entry posted")}))
            })?
        }
        "get_entry" => json!({"entry": entry_json(&db.entry(req_i64(a, "id")?)?)}),
        "list_entries" => {
            let entries = db.entries(&EntryFilter {
                ledger_account_id: ledger_account_arg(db, a, "ledger_account")?,
                from: opt_date(a, "from")?,
                to: opt_date(a, "to")?,
                offset: usize::try_from(opt_i64(a, "offset").unwrap_or(0)).unwrap_or(0),
                limit: Some(opt_i64(a, "limit").unwrap_or(100).clamp(1, 1000) as usize),
            })?;
            json!({"count": entries.len(), "entries": entries.iter().map(entry_json).collect::<Vec<_>>()})
        }
        "set_posting_category" => {
            let id = req_i64(a, "posting_id")?;
            let category = match opt_str(a, "category") {
                Some(c) if !c.trim().is_empty() => Some(db.find_category(&c)?.id),
                _ => None,
            };
            db.set_posting_category(id, category)?;
            json!({"message": trf("posting {id} categorized", &[("id", &id)])})
        }
        "rename_ledger_account" => {
            let id = ledger_account_arg(db, a, "ledger_account")?.ok_or_else(|| missing("ledger_account"))?;
            db.rename_ledger_account(id, &req_str(a, "name")?)?;
            json!({"ledger_account_id": id, "message": tr("account renamed")})
        }
        "delete_ledger_account" => {
            let id = ledger_account_arg(db, a, "ledger_account")?.ok_or_else(|| missing("ledger_account"))?;
            db.delete_ledger_account(id)?;
            json!({"message": tr("account of the chart deleted")})
        }
        "set_account_currency" => {
            let acc = db.find_account(&req_str(a, "account")?)?;
            db.set_account_currency(acc.id, &req_str(a, "currency")?)?;
            json!({"message": trf("currency of \"{name}\" changed", &[("name", &acc.name)])})
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
    w.write_record([
        tr("Date"),
        tr("Account"),
        tr("Payee"),
        tr("Category"),
        tr("Notes"),
        tr("Amount"),
        tr("Cleared"),
        tr("Tags"),
    ])?;
    for t in &txs {
        w.write_record([
            fmt_date(t.date),
            t.account.clone(),
            t.payee.clone(),
            t.category.clone(),
            t.memo.clone(),
            amount(t.amount),
            if t.cleared { tr("yes").into() } else { tr("no").into() },
            t.tags.join(", "),
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
            "Data;Conto;Beneficiario;Categoria;Note;Importo;Verificato;Etichette\n\
             04/03/2026;A;\"Casa, dolce; casa\";;;-1.234,56;sì;\n\
             04/03/2026;A;\"Rossi \"\"SpA\"\"\";;;1.000,00;no;\n"
        );
        assert_eq!(
            en,
            "Date,Account,Payee,Category,Notes,Amount,Cleared,Tags\n\
             2026-03-04,A,\"Casa, dolce; casa\",,,-1234.56,yes,\n\
             2026-03-04,A,\"Rossi \"\"SpA\"\"\",,,1000.00,no,\n"
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
    fn statements_are_imported_as_the_user_decides() {
        with_lang(Lang::En, || {
            let dir = std::env::temp_dir().join(format!("cashcraft-ops-ofx-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let roots = Roots::new(&dir, &dir).unwrap();
            std::fs::write(dir.join("one.ofx"), crate::ofx::tests::sgml()).unwrap();
            std::fs::write(dir.join("two.ofx"), crate::ofx::tests::xml()).unwrap();
            let csv = "Data;Nome;Importo;Netto\n04/01/2026;Bar;1;12.50\n20/02/2026;?;1;-\n";
            std::fs::write(dir.join("a.csv"), csv).unwrap();
            let db = one_account();
            let run = |name: &str, a: Value| call(&db, name, &a, Some(&roots));
            let v = run("preview_csv", json!({"path": "a.csv", "amount_column": "Netto", "invert": true})).unwrap();
            // Read as it always was for these arguments, and said.
            assert_eq!((&v["format"]["decimal_separator"], &v["preview"][0]["amount"]), (&json!(","), &json!(-1250.0)));
            assert!(v["notices"][0].as_str().unwrap().contains("decimals"), "{v}");
            let v = run(
                "preview_csv",
                json!({"path": "a.csv", "amount_column": "Netto", "invert": true, "decimal_separator": "."}),
            )
            .unwrap();
            assert_eq!((&v["format"]["decimal_separator"], &v["problems"][0]["line"]), (&json!("."), &json!(3)));
            let how = json!({"path": "a.csv", "account": "A", "amount_column": "Netto", "invert": true,
                             "date_format": "dmy", "decimal_separator": "."});
            // Not remembered when the columns are not used, and not read either.
            let mut alone = how.clone();
            alone["use_saved"] = json!(false);
            alone["dry_run"] = json!(true);
            assert_eq!(run("import_csv", alone.clone()).unwrap()["would_import"]["added"], 1);
            assert!(db.transactions(&TxFilter::default()).unwrap().is_empty(), "a trial");
            alone["dry_run"] = json!(false);
            assert_eq!(run("import_csv", alone).unwrap()["result"]["added"], 1);
            assert!(db.layouts().unwrap().is_empty());
            assert_eq!(run("import_csv", how).unwrap()["result"]["duplicates"], 1);
            // Remembered for this account: no columns are given this time, and the sign is said.
            let v = run("preview_csv", json!({"path": "a.csv", "account": "A"})).unwrap();
            assert_eq!(
                (&v["format"]["mapping"]["amount"], &v["preview"][0]["amount"], &v["format"]["invert"]),
                (&json!("Netto"), &json!(-12.5), &json!(true))
            );
            assert!(v["notices"][0].as_str().unwrap().contains("\"A\""), "{v}");
            assert_eq!(v["format"]["saved_columns"], 1);
            // What is given wins, and then it is not a remembered sign.
            let v = run("preview_csv", json!({"path": "a.csv", "account": "A", "invert": false})).unwrap();
            assert_eq!((&v["preview"][0]["amount"], &v["notices"]), (&json!(12.5), &json!([])));
            run("create_account", json!({"name": "Other"})).unwrap();
            for other in [
                json!({"path": "a.csv"}),
                json!({"path": "a.csv", "account": "Other"}),
                json!({"path": "a.csv", "account": "A", "use_saved": false}),
            ] {
                let v = run("preview_csv", other).unwrap();
                assert_eq!(
                    (&v["format"]["mapping"]["amount"], &v["format"]["invert"]),
                    (&json!("Importo"), &json!(false))
                );
            }
            // Columns of which most rows make no sense are not remembered.
            std::fs::write(
                dir.join("odd.csv"),
                "Data;Nome;Importo\n04/02/2026;a;7,00\n05/02/2026;b;x\n06/02/2026;c;y\n",
            )
            .unwrap();
            assert_eq!(
                run("import_csv", json!({"path": "odd.csv", "account": "Other"})).unwrap()["result"]["added"],
                1
            );
            std::fs::write(dir.join("none.csv"), "Data;Nome;Importo;X\n04/02/2026;a;;\n").unwrap();
            assert_eq!(
                run("import_csv", json!({"path": "none.csv", "account": "Other"})).unwrap()["result"]["added"],
                0
            );
            assert_eq!(db.layouts().unwrap().len(), 1);
            let list = run("list_saved_columns", json!({})).unwrap();
            let one = &list["saved_columns"][0];
            assert_eq!(
                (&one["account"], &one["invert"], &one["columns"]["amount"]),
                (&json!("A"), &json!(true), &json!("Netto"))
            );
            assert_eq!((&one["date_format"], &one["decimal_separator"]), (&json!("dmy"), &json!(".")));
            assert!(run("forget_saved_columns", json!({})).is_err());
            assert!(run("forget_saved_columns", json!({"id": 99})).is_err());
            run("forget_saved_columns", json!({"id": 1})).unwrap();
            for bad in [json!({"date_format": "x"}), json!({"decimal_separator": ";"}), json!({"statement": 1})] {
                let mut a = bad;
                a["path"] = json!("a.csv");
                assert!(run("preview_csv", a).is_err());
            }
            let v = run("preview_csv", json!({"path": "one.ofx", "account": "A"})).unwrap();
            assert_eq!((&v["rows_found"], &v["likely_duplicates"][0]["row"]), (&json!(2), &json!(1)));
            let there = &v["likely_duplicates"][0];
            assert_eq!((&there["existing"][0]["payee"], &there["certain"]), (&json!("Bar"), &json!(false)));
            assert_eq!(there["existing"][0]["id"], there["proposed"]);
            let bar = there["proposed"].as_i64().unwrap();
            assert_eq!(
                v["ledger_balance"],
                json!({"stated": "1487.50", "date": "2026-01-31", "books": "-12.50", "agrees": false})
            );
            let import = |extra: Value| {
                let mut a = json!({"path": "one.ofx", "account": "A"});
                for (k, v) in extra.as_object().unwrap() {
                    a[k] = v.clone();
                }
                run("import_csv", a)
            };
            let refused = import(json!({})).unwrap_err().to_string();
            assert!(
                refused.contains("1: 2026-01-05, BAR & CAFFE, -12.50") && refused.contains(&format!("→ {bar} ")),
                "{refused}"
            );
            let before = db.transactions(&TxFilter::default()).unwrap().len();
            for bad in [
                json!({"skip_rows": [2]}),
                json!({"skip_rows": [1], "add_rows": "1"}),
                json!({"skip_rows": "all"}),
                json!({"skip_rows": "same"}),
                json!({"skip_rows": [0]}),
                json!({"skip_rows": {}}),
                json!({"skip_rows": "x"}),
                json!({"skip_rows": ["x"]}),
                json!({"skip_rows": [1.5]}),
                json!({"skip_rows": [true]}),
                json!({"skip_rows": "1=99"}),
                json!({"skip_rows": "1=x"}),
                json!({"add_rows": format!("1={bar}")}),
                json!({"add_rows": "same"}),
                json!({"date_column": "1", "skip_rows": "1"}),
                json!({"statement": "7", "skip_rows": "1"}),
            ] {
                assert!(import(bad.clone()).is_err(), "{bad}");
                // A trial refuses the same, and says it.
                let mut trial = bad.clone();
                trial["dry_run"] = json!(true);
                let seen = import(trial);
                assert!(seen.is_err() || !seen.unwrap()["refused"].is_null(), "{bad}");
                assert_eq!(db.transactions(&TxFilter::default()).unwrap().len(), before);
            }
            let trial = import(json!({"skip_rows": ["1"], "dry_run": true})).unwrap();
            assert_eq!((&trial["refused"], &trial["would_import"]["same"]), (&Value::Null, &json!(1)));
            assert_eq!(db.transactions(&TxFilter::default()).unwrap().len(), before);
            let v = import(json!({"skip_rows": [format!("1={bar}")]})).unwrap();
            assert_eq!(
                (&v["result"]["added"], &v["result"]["same"], &v["ledger_balance"]["agrees"]),
                (&json!(1), &json!(1), &json!(true))
            );
            assert_eq!(import(json!({})).unwrap()["result"]["duplicates"], 2);
            // Signs that contradict the types.
            std::fs::write(
                dir.join("sign.ofx"),
                crate::ofx::tests::sgml().replace("-12,50", "12,50").replace("+1500", "-1500"),
            )
            .unwrap();
            let sign = |extra: Value| {
                let mut a = json!({"path": "sign.ofx", "account": "Other"});
                a.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
                run("import_csv", a)
            };
            assert!(sign(json!({})).unwrap_err().to_string().contains("contradict"));
            assert!(sign(json!({"dry_run": true})).unwrap()["refused"].as_str().unwrap().contains("contradict"));
            assert_eq!(sign(json!({"invert": true, "dry_run": true})).unwrap()["preview"][0]["amount"], -12.5);
            assert_eq!(sign(json!({"invert": false})).unwrap()["result"]["added"], 2);
            // Several statements: none is taken without a choice, nor in another currency or kind of account.
            let v = run("preview_csv", json!({"path": "two.ofx"})).unwrap();
            assert_eq!((&v["rows_found"], &v["ofx"]["chosen"]), (&json!(0), &Value::Null));
            assert_eq!(v["ofx"]["statements"][1]["account"], "…3333");
            let two = |a: Value| run("import_csv", a);
            assert!(two(json!({"path": "two.ofx", "account": "A"})).unwrap_err().to_string().contains("choose one"));
            run("create_account", json!({"name": "Card", "kind": "credit_card"})).unwrap();
            run("create_account", json!({"name": "Dollars", "kind": "credit_card", "currency": "USD"})).unwrap();
            for account in ["A", "Card"] {
                assert!(two(json!({"path": "two.ofx", "account": account, "statement": 2})).is_err());
            }
            let v = two(json!({"path": "two.ofx", "account": "Dollars", "statement": "0000111122223333"})).unwrap();
            assert_eq!(v["result"]["added"], 1);
            let v = two(json!({"path": "two.ofx", "account": "A", "statement": 1})).unwrap();
            assert_eq!(v["result"]["added"], 1);
            crate::ledger::tests::assert_sound(&db, "ops");
            std::fs::remove_dir_all(&dir).unwrap();
        });
    }

    #[test]
    fn rules_are_run_and_undone_by_operations() {
        with_lang(Lang::En, || {
            let dir = std::env::temp_dir().join(format!("cashcraft-ops-rules-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let roots = Roots::new(&dir, &dir).unwrap();
            let db = one_account();
            db.conn.execute("DELETE FROM transaction_rules", []).unwrap();
            let run = |name: &str, a: Value| call(&db, name, &a, Some(&roots));
            let add = |payee: &str, amount: &str| {
                let text = format!("Data;Descrizione;Importo\n05/01/2026;{payee};{amount}\n");
                std::fs::write(dir.join("row.csv"), text).unwrap();
                run("import_csv", json!({"path": "row.csv", "account": "A", "use_saved": false})).unwrap();
                let id = run("list_transactions", json!({"search": payee})).unwrap()["transactions"][0]["id"].clone();
                run("update_transaction", json!({"id": id, "tags": ["x"]})).unwrap();
                id.as_i64().unwrap()
            };
            let (one, two) = (add("POS 77 Zoo", "-12,50"), add("POS 78 Zoo", "-90,00"));
            run("update_transaction", json!({"id": one, "category": "Leisure", "tags": "fun, x"})).unwrap();
            let v = run("test_rule", json!({"from_transaction": one})).unwrap();
            assert_eq!(
                (&v["matches"], &v["rule"]["category"], &v["rule"]["tags"]),
                (&json!(1), &json!("Leisure"), &json!(["fun", "x"]))
            );
            let rule = json!({"from_transaction": one, "payee": r"^pos \d+ (.+)$", "regex": true, "rename_payee": "$1!",
                              "min_amount": "10.00", "max_amount": 80.5, "sign": "out", "account": "A"});
            assert_eq!(run("test_rule", rule.clone()).unwrap()["matches"], 1);
            let v = run("add_rule", rule).unwrap();
            assert_eq!((&v["rule"]["min_amount"], &v["rule"]["max_amount"]), (&json!("10.00"), &json!("80.50")));
            let id = v["id"].as_i64().unwrap();
            let last = run("add_rule", json!({"keywords": "zoo|park", "category": "Travel"})).unwrap()["id"].clone();
            for bad in [
                json!({"keywords": "x"}),
                json!({"keywords": "(", "regex": true, "category": "Travel"}),
                json!({"keywords": "x", "category": "Travel", "position": "up"}),
                json!({"keywords": "x", "category": "Travel", "sign": "both"}),
                json!({"keywords": "x", "category": "Travel", "min_amount": 1.234}),
                json!({"keywords": "x", "category": "Travel", "regex": "yes"}),
                json!({"keywords": "x", "category": "Travel", "tags": [1]}),
            ] {
                assert!(run("add_rule", bad.clone()).is_err(), "{bad}");
            }
            assert!(run("test_rule", json!({"id": id, "from_transaction": one})).is_err());
            let v = run("update_rule", json!({"id": id, "max_amount": ""})).unwrap();
            assert_eq!((&v["rule"]["max_amount"], &v["rule"]["rename_payee"]), (&Value::Null, &json!("$1!")));
            let v = run("move_rule", json!({"id": id, "position": "last"})).unwrap();
            assert_eq!(v["rules"][0]["id"], last);
            run("move_rule", json!({"id": id, "position": "up"})).unwrap();
            assert_eq!(run("list_rules", json!({})).unwrap()["rules"][0]["id"], id);
            let v = run("preview_rules", json!({})).unwrap();
            assert_eq!(
                (&v["count"], &v["changes"][0]["id"], &v["changes"][0]["rules"]),
                (&json!(1), &json!(two), &json!([id]))
            );
            assert_eq!(v["changes"][0]["amount"], -90.0);
            for bad in [json!({"overwrite": true}), json!({"scope": "some"}), json!({"token": "other"})] {
                assert!(run("apply_rules", bad).is_err());
            }
            let v = run("apply_rules", json!({"token": v["token"]})).unwrap();
            assert_eq!(v["changed"], 1);
            let done = v["run"].as_i64().unwrap();
            let v = run("get_transaction", json!({"id": two})).unwrap();
            assert_eq!(
                (&v["transaction"]["payee"], &v["transaction"]["original_payee"]),
                (&json!("Zoo!"), &json!("POS 78 Zoo"))
            );
            assert_eq!((&v["rules"][0]["id"], &v["transaction"]["rules"]["category"]), (&json!(id), &json!(id)));
            assert_eq!(run("list_transactions", json!({"tag": "#FUN"})).unwrap()["count"], 2);
            assert_eq!(run("list_tags", json!({})).unwrap()["tags"][1], json!({"tag": "x", "transactions": 2}));
            run("export_csv", json!({"path": "out.csv"})).unwrap();
            let text = std::fs::read_to_string(dir.join("out.csv")).unwrap();
            assert!(text.contains(",Zoo!,Leisure,POS 78 Zoo,-90.00,yes,\"x, fun\""), "{text}");
            std::fs::write(dir.join("in.csv"), "Data;Descrizione;Importo\n05/01/2026;POS 1 Zoo;-12,00\n").unwrap();
            let v = run("preview_csv", json!({"path": "in.csv", "account": "A"})).unwrap();
            let ruled = &v["preview"][0]["ruled"];
            assert_eq!(
                (&ruled["category"], &ruled["payee"], &ruled["rules"]),
                (&json!("Leisure"), &json!("Zoo!"), &json!([id]))
            );
            // The rule is of that account.
            let v = run("preview_csv", json!({"path": "in.csv"})).unwrap();
            assert_eq!(v["preview"][0]["ruled"]["category"], "Travel");
            let e = run("undo_rules", json!({})).unwrap_err().to_string();
            assert!(e.contains(&format!("{done}")) && e.contains("1 transaction"), "{e}");
            assert!(run("undo_rules", json!({"run": 99})).is_err());
            assert_eq!(run("undo_rules", json!({"run": done})).unwrap()["undone"], 1);
            assert!(run("undo_rules", json!({"run": done})).is_err());
            assert_eq!(db.transaction(two).unwrap().payee, "POS 78 Zoo");
            std::fs::remove_dir_all(&dir).unwrap();
        });
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
            json!("(a{999}){999}"),
            json!("(((("),
            json!("${x}$99$"),
            json!("2025-13-99"),
            json!("9999-12"),
            json!([]),
            json!({"a": [1]}),
        ];
        std::fs::write(dir.join("in.csv"), "Data;Descrizione;Importo\n01/01/2026;Bar;-1,20\nx;y;2,00\n").unwrap();
        std::fs::write(dir.join("in.ofx"), crate::ofx::tests::sgml()).unwrap();
        std::fs::write(dir.join("two.ofx"), crate::ofx::tests::xml()).unwrap();
        // What is not of the declared type is refused, alone or among plausible parameters.
        for tool in tools() {
            let db = one_account();
            let properties = tool.schema["properties"].as_object().unwrap();
            for (name, p) in properties {
                let types: Vec<&str> = match &p["type"] {
                    Value::String(t) => vec![t.as_str()],
                    l => l.as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect(),
                };
                for (kind, v) in [
                    ("string", json!("1")),
                    ("number", json!(1.5)),
                    ("boolean", json!(true)),
                    ("array", json!(["1"])),
                    ("array", json!([])),
                    ("object", json!({"a": 1})),
                    ("object", json!({})),
                    ("number", json!(u64::MAX)),
                ] {
                    if types.contains(&kind) {
                        continue;
                    }
                    let before = crate::ledger::tests::dump(&db.conn);
                    for mut a in [
                        json!({}),
                        json!({"account": "A", "path": "in.csv", "keywords": "x", "category": "Leisure",
                                                    "payee": "x", "amount": 1, "name": "x", "id": 1}),
                    ] {
                        a[name.as_str()] = v.clone();
                        let e = call(&db, tool.name, &a, Some(&roots));
                        let e = e.expect_err(&format!("{} {name} {v}", tool.name)).to_string();
                        assert!(e.contains(name.as_str()), "{} {name} {v}: {e}", tool.name);
                    }
                    assert_eq!(crate::ledger::tests::dump(&db.conn), before, "{} {name} {v}", tool.name);
                }
            }
            for a in [json!([]), json!("x"), json!(1), json!(true)] {
                assert!(call(&db, tool.name, &a, Some(&roots)).is_err(), "{}", tool.name);
            }
        }
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
                        "posting_id": 2, "statement_id": 1, "transaction_id": 1, "currency": "USD",
                        "parent": "Conto corrente", "match": "none", "ledger_account": "Conto corrente", "code": "1",
                        "postings": [{"account": "Conto corrente", "amount": "1,00"}, {"account": "Svago", "amount": -1}],
                        "path": "out.csv", "keywords": "a|b", "regex": true, "min_amount": 1, "max_amount": "900",
                        "sign": "out", "tags": "x, y", "rename_payee": "$0", "position": "first", "scope": "all", "unit": "month"});
                    a[hostile.as_str()] = v.clone();
                    let _ = call(&db, tool.name, &a, Some(&roots));
                    // With files that are there.
                    if names.iter().any(|n| n == "statement") && hostile != "path" {
                        for file in ["in.csv", "in.ofx", "two.ofx"] {
                            a["path"] = json!(file);
                            let _ = call(&db, tool.name, &a, Some(&roots));
                        }
                    }
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
            // Balanced and as large as they can be, again and again until refused; then everything still reads.
            let huge = json!({"postings": [{"account": "Conto corrente", "amount": "1000000000000"},
                                           {"account": "Svago", "amount": "-1000000000000"}]});
            let posted = (0..50).take_while(|_| call(&db, "post_entry", &huge, Some(&roots)).is_ok()).count();
            assert!(posted < 50, "{}", tool.name);
            let mut reads = vec![];
            for t in tools() {
                if t.name.starts_with("list_") {
                    reads.push(t.name);
                }
            }
            reads.extend(["spending_by_category", "monthly_summary", "net_worth", "budget_status", "trial_balance"]);
            for t in reads {
                call(&db, t, &json!({}), Some(&roots)).unwrap_or_else(|e| panic!("{t}: {e:#}"));
            }
            let many: Vec<Value> = vec![json!({"account": "Svago", "amount": 1_000_000_000_000_i64}); 2000];
            let _ = call(&db, tool.name, &json!({"postings": many}), Some(&roots));
            assert!(db.verify_ledger().unwrap().problems.is_empty(), "{}", tool.name);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
