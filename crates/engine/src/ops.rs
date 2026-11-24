//! Operazioni comuni a CLI e server MCP: ogni operazione riceve argomenti JSON
//! e restituisce JSON. Gli importi in uscita sono in euro (numeri con due decimali).

use crate::db::{self, AccountKind, Cents, Db, NewScheduled, NewTx, TxEdit, TxFilter};
use crate::importer;
use crate::money::{fmt_eur, parse_amount, parse_date};
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

const AMOUNT: &str =
    "Importo in euro: numero (-12.5) o testo all'italiana (\"-12,50\"). Negativo = uscita, positivo = entrata.";
const DATE: &str = "Data: AAAA-MM-GG o GG/MM/AAAA (predefinita: oggi).";
const ACCOUNT: &str = "Conto: id numerico o nome.";
const CATEGORY: &str = "Categoria: id, nome o percorso \"Padre:Figlio\" (es. \"Casa:Bollette\").";

pub fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "list_accounts",
            description: "Elenca i conti con saldo attuale e patrimonio netto totale.",
            schema: obj(json!({"include_archived": b("Includi i conti archiviati.")}), &[]),
        },
        ToolDef {
            name: "create_account",
            description: "Crea un nuovo conto (bank, cash, credit_card, savings, investment, loan).",
            schema: obj(
                json!({"name": s("Nome del conto."), "kind": s("Tipo: bank, cash, credit_card, savings, investment, loan."),
                "opening_balance": n("Saldo iniziale in euro."), "currency": s("Valuta (predefinita EUR)."), "iban": s("IBAN (facoltativo).")}),
                &["name"],
            ),
        },
        ToolDef {
            name: "delete_account",
            description: "Elimina un conto e tutti i suoi movimenti.",
            schema: obj(json!({"account": s(ACCOUNT)}), &["account"]),
        },
        ToolDef {
            name: "list_transactions",
            description: "Elenca i movimenti (dal più recente), con filtri facoltativi.",
            schema: obj(
                json!({"account": s(ACCOUNT), "category": s(CATEGORY), "from": s("Dal giorno (incluso)."), "to": s("Al giorno (incluso)."),
                "search": s("Testo da cercare in beneficiario, note, categoria."), "uncategorized": b("Solo movimenti senza categoria."),
                "limit": n("Numero massimo di movimenti (predefinito 100).")}),
                &[],
            ),
        },
        ToolDef {
            name: "add_transaction",
            description: "Registra un movimento. Se non si indica la categoria vengono applicate le regole automatiche.",
            schema: obj(
                json!({"account": s(ACCOUNT), "amount": s(AMOUNT), "payee": s("Beneficiario o descrizione."), "date": s(DATE),
                "category": s(CATEGORY), "memo": s("Note."), "cleared": b("Movimento già verificato sull'estratto conto.")}),
                &["account", "amount", "payee"],
            ),
        },
        ToolDef {
            name: "update_transaction",
            description: "Modifica un movimento esistente; i campi non indicati restano invariati. category vuota = senza categoria.",
            schema: obj(
                json!({"id": n("Id del movimento."), "date": s(DATE), "amount": s(AMOUNT), "payee": s("Beneficiario."), "memo": s("Note."),
                "category": s(CATEGORY), "cleared": b("Verificato.")}),
                &["id"],
            ),
        },
        ToolDef {
            name: "delete_transaction",
            description: "Elimina un movimento (per i giroconti elimina entrambe le parti).",
            schema: obj(json!({"id": n("Id del movimento.")}), &["id"]),
        },
        ToolDef {
            name: "add_transfer",
            description: "Registra un giroconto tra due conti.",
            schema: obj(
                json!({"from": s("Conto di origine."), "to": s("Conto di destinazione."), "amount": s("Importo positivo in euro."),
                "date": s(DATE), "memo": s("Note.")}),
                &["from", "to", "amount"],
            ),
        },
        ToolDef {
            name: "preview_csv",
            description: "Analizza un CSV bancario senza importarlo: separatore, colonne riconosciute e prime righe.",
            schema: obj(
                json!({"path": s("Percorso del file CSV."), "invert": b("Inverti il segno degli importi."), "limit": n("Righe di anteprima (predefinito 10)."),
                "date_column": s("Colonna data (nome o numero da 1)."), "amount_column": s("Colonna importo."), "credit_column": s("Colonna entrate."),
                "debit_column": s("Colonna uscite."), "description_column": s("Colonna descrizione."), "payee_column": s("Colonna beneficiario.")}),
                &["path"],
            ),
        },
        ToolDef {
            name: "import_csv",
            description: "Importa un CSV esportato da una banca italiana in un conto. Colonne, separatore, codifica e formato degli importi sono riconosciuti automaticamente; i movimenti già importati vengono saltati; le regole assegnano le categorie.",
            schema: obj(
                json!({"path": s("Percorso del file CSV."), "account": s(ACCOUNT), "invert": b("Inverti il segno (es. estratti carta di credito con spese positive)."),
                "dry_run": b("Solo prova: non salva nulla."), "date_column": s("Colonna data (nome o numero da 1)."), "amount_column": s("Colonna importo."),
                "credit_column": s("Colonna entrate."), "debit_column": s("Colonna uscite."), "description_column": s("Colonna descrizione."),
                "payee_column": s("Colonna beneficiario.")}),
                &["path", "account"],
            ),
        },
        ToolDef {
            name: "list_categories",
            description: "Elenca le categorie di entrata e di uscita.",
            schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "create_category",
            description: "Crea una categoria (anche annidata, es. \"Casa:Giardino\").",
            schema: obj(
                json!({"path": s("Nome o percorso Padre:Figlio."), "income": b("Categoria di entrata (predefinito: uscita).")}),
                &["path"],
            ),
        },
        ToolDef {
            name: "delete_category",
            description: "Elimina una categoria (i movimenti restano, senza categoria).",
            schema: obj(json!({"category": s(CATEGORY)}), &["category"]),
        },
        ToolDef {
            name: "spending_by_category",
            description: "Totali per categoria in un periodo (predefinito: mese corrente). Uscite negative.",
            schema: obj(
                json!({"from": s("Dal giorno."), "to": s("Al giorno."), "month": s("Mese AAAA-MM (alternativo a from/to).")}),
                &[],
            ),
        },
        ToolDef {
            name: "monthly_summary",
            description: "Entrate, uscite e risparmio per mese.",
            schema: obj(json!({"months": n("Quanti mesi (predefinito 12).")}), &[]),
        },
        ToolDef {
            name: "net_worth",
            description: "Patrimonio netto attuale e andamento a fine mese.",
            schema: obj(json!({"months": n("Quanti mesi di storico (predefinito 12).")}), &[]),
        },
        ToolDef {
            name: "set_budget",
            description: "Imposta il budget mensile di una categoria (0 per rimuoverlo).",
            schema: obj(
                json!({"category": s(CATEGORY), "monthly_amount": s("Importo mensile in euro.")}),
                &["category", "monthly_amount"],
            ),
        },
        ToolDef {
            name: "budget_status",
            description: "Budget del mese: previsto, speso e rimanente per categoria.",
            schema: obj(json!({"month": s("Mese AAAA-MM (predefinito: corrente).")}), &[]),
        },
        ToolDef {
            name: "list_rules",
            description: "Elenca le regole di categorizzazione automatica.",
            schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "add_rule",
            description: "Aggiunge una regola: se il beneficiario o la descrizione contengono una delle parole chiave (separate da |), assegna la categoria.",
            schema: obj(
                json!({"keywords": s("Parole chiave separate da | (es. \"conad|coop\")."), "category": s(CATEGORY),
                "rename_payee": s("Nuovo nome del beneficiario (facoltativo).")}),
                &["keywords", "category"],
            ),
        },
        ToolDef {
            name: "delete_rule",
            description: "Elimina una regola.",
            schema: obj(json!({"id": n("Id della regola.")}), &["id"]),
        },
        ToolDef {
            name: "apply_rules",
            description: "Applica le regole a tutti i movimenti ancora senza categoria.",
            schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "list_scheduled",
            description: "Elenca i movimenti pianificati (ricorrenti).",
            schema: obj(json!({}), &[]),
        },
        ToolDef {
            name: "add_scheduled",
            description: "Aggiunge un movimento ricorrente (o un giroconto ricorrente con to_account).",
            schema: obj(
                json!({"account": s(ACCOUNT), "amount": s(AMOUNT), "payee": s("Beneficiario."), "next_date": s("Prossima scadenza."),
                "every": n("Ogni quante unità (predefinito 1)."), "unit": s("giorno, settimana, mese, anno (predefinito mese)."),
                "category": s(CATEGORY), "memo": s("Note."), "to_account": s("Conto di destinazione per un giroconto.")}),
                &["account", "amount", "payee", "next_date"],
            ),
        },
        ToolDef {
            name: "delete_scheduled",
            description: "Elimina un movimento pianificato.",
            schema: obj(json!({"id": n("Id.")}), &["id"]),
        },
        ToolDef {
            name: "post_scheduled",
            description: "Registra i movimenti pianificati scaduti fino alla data indicata (predefinita: oggi).",
            schema: obj(json!({"until": s("Fino al giorno.")}), &[]),
        },
        ToolDef {
            name: "export_csv",
            description: "Esporta i movimenti in un file CSV (separatore ;, importi all'italiana).",
            schema: obj(
                json!({"path": s("File da scrivere."), "account": s(ACCOUNT), "from": s("Dal giorno."), "to": s("Al giorno.")}),
                &["path"],
            ),
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

fn req_str(a: &Value, k: &str) -> Result<String> {
    opt_str(a, k).filter(|s| !s.trim().is_empty()).ok_or_else(|| anyhow!("manca il parametro \"{k}\""))
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
    opt_i64(a, k).ok_or_else(|| anyhow!("manca il parametro numerico \"{k}\""))
}

fn amount_of(v: &Value) -> Result<Cents> {
    match v {
        // The decimal text of the number, so that no amount goes through floating point.
        Value::Number(n) => crate::money::parse_amount_with(&n.to_string(), Some(false)),
        Value::String(s) => parse_amount(s),
        _ => bail!("importo non valido"),
    }
}

fn opt_amount(a: &Value, k: &str) -> Result<Option<Cents>> {
    match a.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => amount_of(v).map(Some),
    }
}

fn req_amount(a: &Value, k: &str) -> Result<Cents> {
    opt_amount(a, k)?.ok_or_else(|| anyhow!("manca il parametro \"{k}\""))
}

pub fn date_arg(s: &str) -> Result<NaiveDate> {
    let t = s.trim().to_lowercase();
    match t.as_str() {
        "" | "oggi" | "today" => return Ok(db::today()),
        "ieri" | "yesterday" => return Ok(db::today().pred_opt().unwrap_or_default()),
        _ => {}
    }
    parse_date(s).ok_or_else(|| anyhow!("data \"{s}\" non valida (usa AAAA-MM-GG o GG/MM/AAAA)"))
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
        .map_err(|_| anyhow!("mese \"{s}\" non valido (usa AAAA-MM)"))
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
                *slot =
                    Some(importer::column_index(t, &v).ok_or_else(|| anyhow!("colonna \"{v}\" non trovata nel file"))?);
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
        let canon = |p: &Path, what: &str| -> Result<PathBuf> {
            let c = p.canonicalize().map_err(|e| anyhow!("cartella {what} \"{}\" non valida: {e}", p.display()))?;
            if !c.is_dir() {
                bail!("\"{}\" non è una cartella", p.display());
            }
            Ok(c)
        };
        Ok(Roots { read: canon(read, "di lettura")?, write: canon(write, "di scrittura")? })
    }

    fn read_path(&self, p: &str) -> Result<PathBuf> {
        let full = self.read.join(p);
        let c = full.canonicalize().map_err(|e| anyhow!("file \"{p}\" non leggibile: {e}"))?;
        if !c.starts_with(&self.read) {
            bail!("accesso negato: \"{p}\" è fuori dalla cartella di lettura consentita");
        }
        Ok(c)
    }

    fn write_path(&self, p: &str) -> Result<PathBuf> {
        let full = self.write.join(p);
        let name = full.file_name().ok_or_else(|| anyhow!("percorso \"{p}\" senza nome di file"))?.to_owned();
        let parent = full.parent().ok_or_else(|| anyhow!("percorso \"{p}\" non valido"))?;
        let dir = parent.canonicalize().map_err(|e| anyhow!("cartella di \"{p}\" non valida: {e}"))?;
        let target = dir.join(name);
        // Un collegamento già esistente non deve portare fuori dalla cartella.
        let resolved = if target.exists() { target.canonicalize()? } else { target.clone() };
        if !dir.starts_with(&self.write) || !resolved.starts_with(&self.write) {
            bail!("accesso negato: \"{p}\" è fuori dalla cartella di scrittura consentita");
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
                Some(k) => AccountKind::from_code(&k).ok_or_else(|| anyhow!("tipo di conto \"{k}\" sconosciuto"))?,
                None => AccountKind::Bank,
            };
            let id = db.add_account(
                &req_str(a, "name")?,
                kind,
                &opt_str(a, "currency").unwrap_or_else(|| "EUR".into()),
                opt_amount(a, "opening_balance")?.unwrap_or(0),
                &opt_str(a, "iban").unwrap_or_default(),
            )?;
            json!({"id": id, "message": "conto creato"})
        }
        "delete_account" => {
            let acc = db.find_account(&req_str(a, "account")?)?;
            db.delete_account(acc.id)?;
            json!({"message": format!("conto \"{}\" eliminato", acc.name)})
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
            let id = id.ok_or_else(|| anyhow!("movimento non salvato"))?;
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
            )?;
            json!({"transaction": db.transaction(id)?})
        }
        "delete_transaction" => {
            let id = req_i64(a, "id")?;
            db.delete_transaction(id)?;
            json!({"message": format!("movimento {id} eliminato")})
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
            json!({"ids": [x, y], "message": "giroconto registrato"})
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
                bail!("non trovo la colonna della data: indicala con date_column");
            }
            if m.amount.is_none() && m.credit.is_none() && m.debit.is_none() {
                bail!("non trovo la colonna dell'importo: indicala con amount_column o credit_column/debit_column");
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
                       "message": format!("{} movimenti importati, {} già presenti, {} categorizzati automaticamente", res.added, res.duplicates, res.categorized)})
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
            json!({"message": format!("categoria \"{}\" eliminata", c.path)})
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
            json!({"message": format!("budget di \"{}\": {} al mese", c.path, fmt_eur(amt.abs()))})
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
            json!({"message": "regola eliminata"})
        }
        "apply_rules" => {
            let n = db.apply_rules()?;
            json!({"categorized": n, "message": format!("{n} movimenti categorizzati")})
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
            json!({"message": "pianificato eliminato"})
        }
        "post_scheduled" => {
            let until = opt_date(a, "until")?.unwrap_or_else(db::today);
            let n = db.post_scheduled(until)?;
            json!({"posted": n, "message": format!("{n} movimenti registrati")})
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
        other => bail!("operazione \"{other}\" sconosciuta"),
    })
}

pub fn export_csv(db: &Db, f: &TxFilter, path: &std::path::Path) -> Result<usize> {
    let mut txs = db.transactions(f)?;
    txs.reverse();
    let mut w = csv::WriterBuilder::new().delimiter(b';').from_path(path)?;
    w.write_record(["Data", "Conto", "Beneficiario", "Categoria", "Note", "Importo", "Verificato"])?;
    for t in &txs {
        w.write_record([
            t.date.format("%d/%m/%Y").to_string(),
            t.account.clone(),
            t.payee.clone(),
            t.category.clone(),
            t.memo.clone(),
            crate::money::fmt_plain(t.amount),
            if t.cleared { "sì".into() } else { "no".into() },
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
            let db = Db::open_in_memory().unwrap();
            crate::demo::seed(&db).unwrap();
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
                        "next_date": "2025-01-31", "id": 1, "path": "out.csv", "pattern": "x", "unit": "month"});
                    a[hostile.as_str()] = v.clone();
                    let _ = call(&db, tool.name, &a, Some(&roots));
                }
            }
            let _ = call(&db, tool.name, &json!("not an object"), Some(&roots));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
