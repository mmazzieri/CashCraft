//! Interfaccia a riga di comando. Ogni comando si traduce in un'operazione di `ops`
//! (le stesse del server MCP) e stampa una tabella leggibile, o JSON con `--json`.

use anyhow::{Result, anyhow};
use cashcraft_engine::db::Db;
use cashcraft_engine::money::fmt_eur;
use cashcraft_engine::ops;
use clap::{Args, Subcommand};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Avvia il server MCP su stdin/stdout. I file CSV si leggono solo dalla cartella
    /// di lettura e le esportazioni si scrivono solo in quella di scrittura.
    Mcp {
        /// Cartella da cui l'agente può leggere i CSV da importare
        #[arg(long)]
        automation_read_root: PathBuf,
        /// Cartella in cui l'agente può scrivere le esportazioni
        #[arg(long)]
        automation_write_root: PathBuf,
    },
    /// Conti
    #[command(subcommand, visible_alias = "conti")]
    Accounts(AccountsCmd),
    /// Movimenti
    #[command(subcommand, visible_alias = "movimenti")]
    Tx(TxCmd),
    /// Giroconto tra due conti
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
    /// Importa un CSV bancario in un conto
    #[command(visible_alias = "importa")]
    Import {
        file: PathBuf,
        /// Conto di destinazione (nome o id)
        #[arg(long, short)]
        account: String,
        /// Mostra cosa verrebbe importato senza salvare
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        cols: ColumnArgs,
    },
    /// Analizza un CSV senza importarlo
    #[command(visible_alias = "anteprima")]
    Preview {
        file: PathBuf,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[command(flatten)]
        cols: ColumnArgs,
    },
    /// Categorie
    #[command(subcommand, visible_alias = "categorie")]
    Categories(CatCmd),
    /// Budget mensili
    #[command(subcommand)]
    Budget(BudgetCmd),
    /// Regole di categorizzazione automatica
    #[command(subcommand, visible_alias = "regole")]
    Rules(RulesCmd),
    /// Report
    #[command(subcommand)]
    Report(ReportCmd),
    /// Movimenti pianificati (ricorrenti)
    #[command(subcommand, visible_alias = "pianificati")]
    Scheduled(SchedCmd),
    /// Esporta i movimenti in CSV
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
    /// Elenca le operazioni disponibili (le stesse del server MCP)
    Tools,
    /// Esegue un'operazione con argomenti JSON, es.: call list_transactions '{"limit":5}'
    Call { tool: String, args: Option<String> },
}

#[derive(Args, Debug, Default)]
pub struct ColumnArgs {
    /// Inverte il segno degli importi (es. estratti carta con spese positive)
    #[arg(long)]
    invert: bool,
    /// Colonna della data (nome o numero da 1)
    #[arg(long)]
    date_col: Option<String>,
    /// Colonna dell'importo con segno
    #[arg(long)]
    amount_col: Option<String>,
    /// Colonna delle entrate
    #[arg(long)]
    credit_col: Option<String>,
    /// Colonna delle uscite
    #[arg(long)]
    debit_col: Option<String>,
    /// Colonna della descrizione
    #[arg(long)]
    desc_col: Option<String>,
    /// Colonna del beneficiario
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
    /// Elenca i conti con i saldi
    List {
        #[arg(long)]
        all: bool,
    },
    /// Crea un conto
    Add {
        name: String,
        /// bank, cash, credit_card, savings, investment, loan
        #[arg(long, default_value = "bank")]
        kind: String,
        #[arg(long, allow_hyphen_values = true, default_value = "0")]
        opening: String,
        #[arg(long)]
        iban: Option<String>,
    },
    /// Elimina un conto e i suoi movimenti
    Delete { account: String },
}

#[derive(Subcommand, Debug)]
pub enum TxCmd {
    /// Elenca i movimenti
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
    /// Registra un movimento (importo negativo = uscita)
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
    /// Modifica un movimento
    Edit {
        id: i64,
        #[arg(long, allow_hyphen_values = true)]
        amount: Option<String>,
        #[arg(long)]
        payee: Option<String>,
        #[arg(long)]
        date: Option<String>,
        /// Categoria ("" per toglierla)
        #[arg(long)]
        category: Option<String>,
        #[arg(long)]
        memo: Option<String>,
        #[arg(long)]
        cleared: Option<bool>,
    },
    /// Elimina un movimento
    Delete { id: i64 },
}

#[derive(Subcommand, Debug)]
pub enum CatCmd {
    List,
    /// Crea una categoria ("Padre:Figlio")
    Add {
        path: String,
        #[arg(long)]
        income: bool,
    },
    Delete {
        category: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum BudgetCmd {
    /// Stato del budget di un mese (AAAA-MM)
    Status { month: Option<String> },
    /// Imposta il budget mensile di una categoria (0 per toglierlo)
    Set { category: String, amount: String },
}

#[derive(Subcommand, Debug)]
pub enum RulesCmd {
    List,
    /// Aggiunge una regola: parole chiave separate da |
    Add {
        keywords: String,
        category: String,
        #[arg(long)]
        rename: Option<String>,
    },
    Delete {
        id: i64,
    },
    /// Applica le regole ai movimenti senza categoria
    Apply,
}

#[derive(Subcommand, Debug)]
pub enum ReportCmd {
    /// Totali per categoria (predefinito: mese corrente)
    Categories {
        #[arg(long)]
        month: Option<String>,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
    },
    /// Entrate e uscite per mese
    Monthly {
        #[arg(long, default_value_t = 12)]
        months: u32,
    },
    /// Patrimonio netto
    Networth {
        #[arg(long, default_value_t = 12)]
        months: u32,
    },
}

#[derive(Subcommand, Debug)]
pub enum SchedCmd {
    List,
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
        /// giorno, settimana, mese, anno
        #[arg(long, default_value = "mese")]
        unit: String,
        #[arg(long, short)]
        category: Option<String>,
        #[arg(long)]
        to_account: Option<String>,
    },
    Delete {
        id: i64,
    },
    /// Registra i pianificati scaduti
    Post {
        #[arg(long)]
        until: Option<String>,
    },
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
            let roots = ops::Roots::new(&automation_read_root, &automation_write_root)?;
            return crate::mcp::serve(db, &roots);
        }
        Cmd::Tools => {
            for t in ops::tools() {
                println!("{:<22} {}", t.name, t.description);
            }
            return Ok(());
        }
        Cmd::Call { tool, args } => {
            let a: Value = match args {
                Some(s) => serde_json::from_str(&s).map_err(|e| anyhow!("argomenti JSON non validi: {e}"))?,
                None => json!({}),
            };
            let r = ops::call(db, &tool, &a, None)?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            return Ok(());
        }
        Cmd::Accounts(c) => match c {
            AccountsCmd::List { all } => ("list_accounts", args(&[("include_archived", v(all))])),
            AccountsCmd::Add { name, kind, opening, iban } => (
                "create_account",
                args(&[("name", v(name)), ("kind", v(kind)), ("opening_balance", v(opening)), ("iban", o(&iban))]),
            ),
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

fn st(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n - 1).collect::<String>()) }
}

fn it_date(v: &Value) -> String {
    cashcraft_engine::money::parse_date(&st(v)).map(cashcraft_engine::money::fmt_date).unwrap_or_else(|| st(v))
}

fn print_human(tool: &str, r: &Value) {
    let arr = |k: &str| r.get(k).and_then(|x| x.as_array()).cloned().unwrap_or_default();
    match tool {
        "list_accounts" => {
            println!("{:>4}  {:<28} {:<18} {:>16}", "ID", "CONTO", "TIPO", "SALDO");
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
            println!("{:>69}", format!("Patrimonio netto: {}", eur(field(r, "net_worth"))));
        }
        "list_transactions" => {
            println!(
                "{:>5}  {:<10}  {:<16} {:<30} {:<24} {:>13}",
                "ID", "DATA", "CONTO", "BENEFICIARIO", "CATEGORIA", "IMPORTO"
            );
            for t in arr("transactions") {
                println!(
                    "{:>5}  {:<10}  {:<16} {:<30} {:<24} {:>13}",
                    st(field(&t, "id")),
                    it_date(field(&t, "date")),
                    cut(&st(field(&t, "account")), 16),
                    cut(&st(field(&t, "payee")), 30),
                    cut(&st(field(&t, "category")), 24),
                    eur(field(&t, "amount"))
                );
            }
            println!("{} movimenti", st(field(r, "count")));
        }
        "spending_by_category" => {
            println!("Dal {} al {}", it_date(field(r, "from")), it_date(field(r, "to")));
            for c in arr("categories") {
                println!("  {:<36} {:>14}", cut(&st(field(&c, "category")), 36), eur(field(&c, "total")));
            }
            println!("  {:<36} {:>14}", "Entrate", eur(field(r, "income")));
            println!("  {:<36} {:>14}", "Uscite", eur(field(r, "expense")));
            println!("  {:<36} {:>14}", "Risparmio", eur(field(r, "net")));
        }
        "monthly_summary" => {
            println!("{:<9} {:>14} {:>14} {:>14}", "MESE", "ENTRATE", "USCITE", "RISPARMIO");
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
            println!("Patrimonio netto attuale: {}", eur(field(r, "net_worth")));
        }
        "budget_status" => {
            println!("Budget di {}", st(field(r, "month")));
            println!("  {:<30} {:>12} {:>12} {:>12}", "CATEGORIA", "BUDGET", "SPESO", "RESTANO");
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
                let kind = if field(&c, "is_income").as_bool().unwrap_or(false) { "entrata" } else { "uscita" };
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
                    it_date(field(&x, "next_date")),
                    cut(&st(field(&x, "account")), 16),
                    cut(&st(field(&x, "payee")), 28),
                    eur(field(&x, "amount")),
                    cashcraft_engine::db::unit_label(&st(field(&x, "unit")), field(&x, "every").as_i64().unwrap_or(1))
                );
            }
        }
        "preview_csv" | "import_csv" => {
            let f = field(r, "format");
            println!(
                "Formato: separatore '{}', codifica {}, decimali '{}', intestazione alla riga {}",
                st(field(f, "delimiter")),
                st(field(f, "encoding")),
                st(field(f, "decimal_separator")),
                st(field(f, "header_row"))
            );
            let m = field(f, "mapping");
            for (k, label) in [
                ("date", "Data"),
                ("amount", "Importo"),
                ("credit", "Entrate"),
                ("debit", "Uscite"),
                ("payee", "Beneficiario"),
                ("description", "Descrizione"),
                ("extra", "Dettagli"),
            ] {
                if !m[k].is_null() {
                    println!("  {:<13} ← \"{}\"", label, st(&m[k]));
                }
            }
            if let Some(msg) = r.get("message") {
                println!("{}", st(msg));
            } else {
                println!(
                    "{} movimenti trovati ({} righe saltate), dal {} al {}, somma {}",
                    st(field(r, "rows_found")),
                    st(field(r, "rows_skipped")),
                    it_date(field(r, "first_date")),
                    it_date(field(r, "last_date")),
                    eur(field(r, "sum"))
                );
                for p in arr("preview") {
                    println!(
                        "  {}  {:<40} {:>12}",
                        it_date(field(&p, "date")),
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
