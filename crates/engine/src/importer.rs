//! Importazione dei file CSV esportati dalle banche italiane.
//!
//! Le banche usano formati diversi; invece di un elenco rigido di formati,
//! l'importatore riconosce da solo:
//! - la codifica (UTF-8 o Windows-1252/ISO-8859-1, tipica dei file italiani),
//! - il separatore (`;`, `,`, tabulazione, `|`),
//! - le righe di intestazione iniziali (nome banca, IBAN, periodo...) da saltare,
//! - le colonne per nome ("Data contabile", "Data valuta", "Descrizione", "Causale",
//!   "Importo", "Entrate"/"Uscite", "Accrediti"/"Addebiti", "Dare"/"Avere"...),
//! - importi all'italiana (1.234,56) e date gg/mm/aaaa.
//!
//! Le righe che non sono movimenti (saldi, totali, righe vuote) vengono saltate.
//! Reimportare lo stesso file non crea doppioni.

use crate::db::{Cents, Db, NewTx};
use crate::i18n::{tr, trf};
use crate::money::{parse_amount_with, parse_date};
use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// Dimensione massima di un estratto conto.
const MAX_FILE: u64 = 32 << 20;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Mapping {
    pub date: Option<usize>,
    pub value_date: Option<usize>,
    pub payee: Option<usize>,
    pub description: Option<usize>,
    pub extra: Option<usize>,
    pub amount: Option<usize>,
    pub credit: Option<usize>,
    pub debit: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Table {
    pub delimiter: u8,
    pub encoding: &'static str,
    pub records: Vec<Vec<String>>,
    pub header_row: Option<usize>,
    pub headers: Vec<String>,
    pub mapping: Mapping,
    pub decimal_comma: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ParsedRow {
    pub date: NaiveDate,
    pub amount: Cents,
    pub payee: String,
    pub memo: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ImportResult {
    pub read: usize,
    pub added: usize,
    pub duplicates: usize,
    pub skipped: usize,
    pub categorized: usize,
}

/// Legge il file e lo converte in testo (UTF-8 o Windows-1252).
pub fn read_text(path: &Path) -> Result<(String, &'static str)> {
    let context = || trf("cannot read {path}", &[("path", &path.display())]);
    let mut bytes = Vec::new();
    let file = std::fs::File::open(path).with_context(context)?;
    file.take(MAX_FILE + 1).read_to_end(&mut bytes).with_context(context)?;
    if bytes.len() as u64 > MAX_FILE {
        bail!(trf("{path}: the file is larger than {mb} MB", &[("path", &path.display()), ("mb", &(MAX_FILE >> 20))]));
    }
    Ok(decode(&bytes))
}

pub fn decode(bytes: &[u8]) -> (String, &'static str) {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (String::from_utf8_lossy(rest).into_owned(), "UTF-8");
    }
    if let Some((enc, _)) = encoding_rs::Encoding::for_bom(bytes) {
        let (s, _, _) = enc.decode(bytes);
        return (s.into_owned(), "UTF-16");
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_string(), "UTF-8"),
        Err(_) => {
            let (s, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
            (s.into_owned(), "Windows-1252")
        }
    }
}

fn detect_delimiter(text: &str) -> u8 {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).take(40).collect();
    let mut best = (b';', 0usize);
    for d in *b";,\t|" {
        // Conta le righe che hanno lo stesso numero (>0) di separatori più frequente.
        let mut freq: HashMap<usize, usize> = HashMap::new();
        for l in &lines {
            let n = count_outside_quotes(l, d as char);
            if n > 0 {
                *freq.entry(n).or_default() += 1;
            }
        }
        let score = freq.values().copied().max().unwrap_or(0);
        if score > best.1 {
            best = (d, score);
        }
    }
    best.0
}

fn count_outside_quotes(line: &str, d: char) -> usize {
    let mut q = false;
    let mut n = 0;
    for c in line.chars() {
        if c == '"' {
            q = !q;
        } else if c == d && !q {
            n += 1;
        }
    }
    n
}

/// Testo in minuscolo, senza accenti e punteggiatura, con spazi singoli.
fn norm(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        let c = match c {
            'à' | 'á' | 'â' => 'a',
            'è' | 'é' | 'ê' => 'e',
            'ì' | 'í' => 'i',
            'ò' | 'ó' => 'o',
            'ù' | 'ú' => 'u',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        };
        out.push(c);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_word(h: &str, kw: &str) -> bool {
    format!(" {h} ").contains(&format!(" {kw} "))
}

const DATE_KW: [&str; 12] = [
    "data contabile",
    "data operazione",
    "data registrazione",
    "data movimento",
    "data op",
    "data transazione",
    "data contabilizzazione",
    "booking date",
    "completed date",
    "started date",
    "date",
    "data",
];
const CREDIT_KW: [&str; 8] = ["accrediti", "accredito", "entrate", "entrata", "avere", "paid in", "credit", "credito"];
const DEBIT_KW: [&str; 8] = ["addebiti", "addebito", "uscite", "uscita", "dare", "paid out", "debit", "debito"];
const AMOUNT_KW: [&str; 7] = ["importo", "amount", "ammontare", "movimento", "valore", "importo eur", "totale"];
const PAYEE_KW: [&str; 10] = [
    "beneficiario",
    "controparte",
    "esercente",
    "payee",
    "merchant",
    "ordinante",
    "counterparty",
    "destinatario",
    "nome",
    "name",
];
const DESC_KW: [&str; 8] = [
    "descrizione operazione",
    "descrizione",
    "causale",
    "description",
    "dettagli",
    "operazione",
    "riferimento",
    "reference",
];
const EXTRA_KW: [&str; 6] = ["descrizione estesa", "descrizione completa", "dettaglio", "note", "memo", "causale"];

/// Sceglie le colonne in base ai nomi dell'intestazione.
pub fn map_headers(headers: &[String]) -> Mapping {
    let n: Vec<String> = headers.iter().map(|h| norm(h)).collect();
    let mut used: Vec<usize> = Vec::new();
    let pick = |kws: &[&str], skip: &dyn Fn(&str) -> bool, used: &mut Vec<usize>| -> Option<usize> {
        for kw in kws {
            for (i, h) in n.iter().enumerate() {
                if !used.contains(&i) && !skip(h) && (h == kw || has_word(h, kw)) {
                    used.push(i);
                    return Some(i);
                }
            }
        }
        None
    };
    let no = |_: &str| false;
    let is_valuta = |h: &str| h.contains("valuta");
    let credit = pick(&CREDIT_KW, &no, &mut used);
    let debit = pick(&DEBIT_KW, &no, &mut used);
    let date = pick(&DATE_KW, &is_valuta, &mut used);
    let mut m = Mapping { credit, debit, date, ..Default::default() };
    m.value_date = pick(&["data valuta", "valuta", "value date"], &no, &mut used);
    if m.date.is_none() {
        m.date = m.value_date.take();
    }
    if m.credit.is_none() || m.debit.is_none() {
        // Una sola colonna di entrate/uscite: meglio un importo unico se esiste.
        let amount = pick(&AMOUNT_KW, &is_valuta, &mut used);
        if amount.is_some() {
            if let Some(c) = m.credit.take() {
                used.retain(|x| *x != c);
            }
            if let Some(d) = m.debit.take() {
                used.retain(|x| *x != d);
            }
            m.amount = amount;
        }
    }
    m.payee = pick(&PAYEE_KW, &no, &mut used);
    m.description = pick(&DESC_KW, &|h: &str| h.contains("estesa") || h.contains("completa"), &mut used);
    m.extra = pick(&EXTRA_KW, &no, &mut used);
    m
}

fn looks_like_header(row: &[String]) -> bool {
    let joined: Vec<String> = row.iter().map(|c| norm(c)).collect();
    let has_date = joined.iter().any(|h| DATE_KW.iter().any(|k| has_word(h, k)) || h.contains("valuta"));
    let has_amount =
        joined.iter().any(|h| AMOUNT_KW.iter().chain(CREDIT_KW.iter()).chain(DEBIT_KW.iter()).any(|k| has_word(h, k)));
    has_date && has_amount
}

/// Analizza il testo CSV: separatore, intestazione e colonne.
pub fn analyze(text: &str, encoding: &'static str) -> Result<Table> {
    let delimiter = detect_delimiter(text);
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());
    let mut records = Vec::new();
    for r in rdr.records() {
        match r {
            Ok(r) => records.push(r.iter().map(|s| s.trim().to_string()).collect::<Vec<_>>()),
            Err(_) => continue,
        }
    }
    if records.is_empty() {
        bail!(tr("the file is empty or is not a CSV"));
    }
    let header_row = records.iter().take(60).position(|r| looks_like_header(r));
    let (headers, mapping) = match header_row {
        Some(h) => {
            let headers = records.get(h).cloned().unwrap_or_default();
            let rows = records.get(h + 1..).unwrap_or_default();
            let mut m = map_headers(&headers);
            if m.date.is_none() || (m.amount.is_none() && m.credit.is_none() && m.debit.is_none()) {
                let g = guess_by_content(rows);
                m.date = m.date.or(g.date);
                if m.amount.is_none() && m.credit.is_none() && m.debit.is_none() {
                    m.amount = g.amount;
                }
            }
            if m.payee.is_none() && m.description.is_none() {
                m.description = guess_by_content(rows).description;
            }
            (headers, m)
        }
        None => {
            let width = records.iter().map(|r| r.len()).max().unwrap_or(0);
            // Unnamed: `column_label` numbers them in the language of whoever reads.
            let headers = vec![String::new(); width];
            (headers, guess_by_content(&records))
        }
    };
    let mut t = Table { delimiter, encoding, records, header_row, headers, mapping, decimal_comma: true };
    t.decimal_comma = detect_decimal_comma(&t);
    Ok(t)
}

/// Senza intestazione: la colonna con più date, quella con più importi, il testo più lungo.
fn guess_by_content(rows: &[Vec<String>]) -> Mapping {
    let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let sample: Vec<&Vec<String>> = rows.iter().take(200).collect();
    let mut date_score = vec![0usize; width];
    let mut amount_score = vec![0usize; width];
    let mut text_len = vec![0usize; width];
    let add = |v: &mut Vec<usize>, i: usize, n: usize| {
        if let Some(s) = v.get_mut(i) {
            *s += n;
        }
    };
    for r in &sample {
        for (i, c) in r.iter().enumerate() {
            if parse_date(c).is_some() {
                add(&mut date_score, i, 1);
            } else if c.chars().any(|ch| ch.is_ascii_digit()) && parse_amount_with(c, None).is_ok() {
                add(&mut amount_score, i, 1);
            } else {
                add(&mut text_len, i, c.len());
            }
        }
    }
    let best = |v: &[usize], exclude: &[Option<usize>]| -> Option<usize> {
        v.iter()
            .enumerate()
            .filter(|(i, s)| **s > 0 && !exclude.contains(&Some(*i)))
            .max_by_key(|(i, s)| (**s, usize::MAX - *i))
            .map(|(i, _)| i)
    };
    let date = best(&date_score, &[]);
    let amount = best(&amount_score, &[date]);
    let description = best(&text_len, &[date, amount]);
    Mapping { date, amount, description, ..Default::default() }
}

fn detect_decimal_comma(t: &Table) -> bool {
    let cols: Vec<usize> = [t.mapping.amount, t.mapping.credit, t.mapping.debit].into_iter().flatten().collect();
    let start = t.header_row.map(|h| h + 1).unwrap_or(0);
    let (mut comma, mut dot) = (0, 0);
    for r in t.records.iter().skip(start).take(300) {
        for &c in &cols {
            let Some(v) = r.get(c) else { continue };
            let (lc, ld) = (v.rfind(','), v.rfind('.'));
            match (lc, ld) {
                (Some(a), Some(b)) => {
                    if a > b {
                        comma += 1
                    } else {
                        dot += 1
                    }
                }
                (Some(a), None) => {
                    // "1,234" è ambiguo: non conta
                    if v.len() - a - 1 != 3 || a > 3 {
                        comma += 1
                    }
                }
                (None, Some(b)) if v.len() - b - 1 != 3 => dot += 1,
                _ => {}
            }
        }
    }
    comma >= dot
}

/// Estrae il nome dell'esercente dalle descrizioni tipiche ("PAGAMENTO POS ... PRESSO NEGOZIO").
pub fn smart_payee(desc: &str) -> String {
    // Solo ASCII: le posizioni restano quelle del testo originale.
    let lower = desc.to_ascii_lowercase();
    for marker in ["presso ", "c/o ", "a favore di ", "beneficiario: ", "beneficiario ", "ordinante: ", "ordinante "] {
        if let Some(i) = lower.find(marker) {
            let Some(rest) = desc.get(i + marker.len()..) else { continue };
            let rest_l = rest.to_ascii_lowercase();
            let mut end = rest.len();
            for stop in [" carta", " del ", " data ", " ore ", " cro", " trn", " iban", " causale", " - ", " rif"] {
                if let Some(j) = rest_l.find(stop) {
                    end = end.min(j);
                }
            }
            let p = rest.get(..end).unwrap_or(rest).trim().trim_matches(|c: char| c == ':' || c == '-').trim();
            if p.len() >= 2 {
                return collapse(p);
            }
        }
    }
    collapse(desc)
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Converte le righe del file in movimenti secondo la mappatura.
/// `invert` cambia il segno (utile per gli estratti delle carte di credito).
pub fn parse_rows(t: &Table, m: &Mapping, invert: bool) -> (Vec<ParsedRow>, usize) {
    let start = t.header_row.map(|h| h + 1).unwrap_or(0);
    let mut out = Vec::new();
    let mut skipped = 0;
    let get = |r: &Vec<String>, c: Option<usize>| -> String { c.and_then(|i| r.get(i)).cloned().unwrap_or_default() };
    for r in t.records.iter().skip(start) {
        if r.iter().all(|c| c.is_empty()) {
            continue;
        }
        let Some(date) = parse_date(&get(r, m.date)).or_else(|| parse_date(&get(r, m.value_date))) else {
            skipped += 1;
            continue;
        };
        let dc = Some(t.decimal_comma);
        let amount = if m.credit.is_some() || m.debit.is_some() {
            let c = get(r, m.credit);
            let d = get(r, m.debit);
            let cv = if c.is_empty() { Ok(0) } else { parse_amount_with(&c, dc) };
            let dv = if d.is_empty() { Ok(0) } else { parse_amount_with(&d, dc) };
            match (cv, dv) {
                (Ok(cv), Ok(dv)) => {
                    let mut a = cv.abs() - dv.abs();
                    // Alcune banche usano un'unica colonna "Importo" più "Dare/Avere" come segno.
                    if let Some(am) = m.amount
                        && let Ok(v) = parse_amount_with(&get(r, Some(am)), dc)
                    {
                        a = v;
                    }
                    a
                }
                _ => {
                    skipped += 1;
                    continue;
                }
            }
        } else {
            match parse_amount_with(&get(r, m.amount), dc) {
                Ok(v) => v,
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            }
        };
        let amount = if invert { -amount } else { amount };
        let desc = collapse(&get(r, m.description));
        let extra = collapse(&get(r, m.extra));
        let payee_col = collapse(&get(r, m.payee));
        let (payee, memo) = if !payee_col.is_empty() {
            let memo = [desc.as_str(), extra.as_str()]
                .iter()
                .filter(|s| !s.is_empty() && **s != payee_col)
                .copied()
                .collect::<Vec<_>>()
                .join(" — ");
            (payee_col, memo)
        } else {
            let full = if extra.is_empty() || extra == desc {
                desc.clone()
            } else if desc.is_empty() {
                extra.clone()
            } else {
                format!("{desc} — {extra}")
            };
            let p = smart_payee(if extra.len() > desc.len() { &extra } else { &desc });
            let p = if p.is_empty() { desc.clone() } else { p };
            let memo = if full == p { String::new() } else { full };
            (p, memo)
        };
        out.push(ParsedRow { date, amount, payee, memo });
    }
    out.sort_by_key(|r| r.date);
    (out, skipped)
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Salva i movimenti nel conto, saltando quelli già importati in precedenza.
pub fn import_rows(db: &Db, account_id: i64, rows: &[ParsedRow], skipped: usize) -> Result<ImportResult> {
    let mut res = ImportResult { read: rows.len(), skipped, ..Default::default() };
    let mut seen: HashMap<String, usize> = HashMap::new();
    db.atomically(|| {
        for row in rows {
            let key = format!("{}|{}|{}|{}", row.date, row.amount, row.payee.to_lowercase(), row.memo.to_lowercase());
            let n = seen.entry(key.clone()).or_default();
            *n += 1;
            let hash = format!("{:016x}", fnv(&format!("{key}|{n}")));
            let id = db.add_transaction(&NewTx {
                account_id,
                date: row.date,
                amount: row.amount,
                payee: row.payee.clone(),
                memo: row.memo.clone(),
                category_id: None,
                cleared: true,
                import_hash: Some(hash),
            })?;
            match id {
                Some(id) => {
                    res.added += 1;
                    let cat: Option<i64> =
                        db.conn.query_row("SELECT category_id FROM transactions WHERE id=?1", [id], |r| r.get(0))?;
                    if cat.is_some() {
                        res.categorized += 1;
                    }
                }
                None => res.duplicates += 1,
            }
        }
        Ok(())
    })?;
    Ok(res)
}

/// The name of a column, or its number when it has none.
pub fn column_label(headers: &[String], i: usize) -> Option<String> {
    headers.get(i).map(|h| if h.is_empty() { trf("Column {n}", &[("n", &(i + 1))]) } else { h.clone() })
}

/// The mapping in readable form, for the command line and MCP.
pub fn describe(t: &Table, m: &Mapping) -> serde_json::Value {
    let name = |c: Option<usize>| c.and_then(|i| column_label(&t.headers, i));
    let columns: Vec<_> = (0..t.headers.len()).filter_map(|i| column_label(&t.headers, i)).collect();
    serde_json::json!({
        "encoding": t.encoding,
        "delimiter": match t.delimiter { b'\t' => "tab".to_string(), d => (d as char).to_string() },
        "header_row": t.header_row.map(|h| h + 1),
        "decimal_separator": if t.decimal_comma { "," } else { "." },
        "columns": columns,
        "mapping": {
            "date": name(m.date),
            "value_date": name(m.value_date),
            "payee": name(m.payee),
            "description": name(m.description),
            "extra": name(m.extra),
            "amount": name(m.amount),
            "credit": name(m.credit),
            "debit": name(m.debit),
        }
    })
}

/// Finds a column by name (ignoring case) or by number (from 1).
pub fn column_index(t: &Table, key: &str) -> Option<usize> {
    let k = key.trim();
    if let Ok(n) = k.parse::<usize>()
        && n >= 1
        && n <= t.headers.len().max(1)
    {
        return Some(n - 1);
    }
    let nk = norm(k);
    if let Some(i) = t.headers.iter().position(|h| !nk.is_empty() && norm(h) == nk) {
        return Some(i);
    }
    // The label of an unnamed column, in either language.
    let n: usize = nk.strip_prefix("column ").or_else(|| nk.strip_prefix("colonna "))?.parse().ok()?;
    n.checked_sub(1).filter(|i| t.headers.get(*i).is_some_and(String::is_empty))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separate_columns_with_preamble() {
        let csv = "Estratto conto;;;\nIBAN IT00X0000000000000000000000;;;\n\nData contabile;Data valuta;Descrizione;Accrediti;Addebiti\n02/01/2025;02/01/2025;PAGAMENTO POS 23,40 EUR DEL 01/01/2025 PRESSO SUPERMERCATO ROSSI;;23,40\n27/01/2025;27/01/2025;BONIFICO STIPENDIO;2.150,00;\nSaldo finale;;;;\n";
        let t = analyze(csv, "UTF-8").unwrap();
        assert_eq!(t.delimiter, b';');
        assert_eq!(t.headers[0], "Data contabile");
        let (rows, skipped) = parse_rows(&t, &t.mapping, false);
        assert_eq!(rows.len(), 2);
        assert_eq!(skipped, 1);
        assert_eq!(rows[0].amount, -2340);
        assert_eq!(rows[0].payee, "SUPERMERCATO ROSSI");
        assert_eq!(rows[1].amount, 215000);
    }

    #[test]
    fn single_amount_column() {
        let csv = "Data Registrazione;Data valuta;Descrizione;Importo (EUR)\n03/02/2025;03/02/2025;ADDEBITO DIRETTO ENERGIA;-85,12\n";
        let t = analyze(csv, "UTF-8").unwrap();
        let (rows, _) = parse_rows(&t, &t.mapping, false);
        assert_eq!(rows[0].amount, -8512);
    }

    #[test]
    fn english_comma_file() {
        let csv = "\"Date\",\"Payee\",\"Amount (EUR)\"\n\"2025-03-01\",\"Bar Centrale\",\"-3.50\"\n";
        let t = analyze(csv, "UTF-8").unwrap();
        let (rows, _) = parse_rows(&t, &t.mapping, false);
        assert_eq!(rows[0].amount, -350);
        assert_eq!(rows[0].payee, "Bar Centrale");
    }

    /// Statements are read the same whatever the language of the interface.
    #[test]
    fn the_language_does_not_change_the_import() {
        use crate::i18n::{Lang, with_lang};
        let files = [
            "Data;Descrizione;Importo\n01/02/2025;Auto;-1.234\n02/02/2025;Bar;-1,234\n03/02/2025;Bar;1.234,56\n",
            "Date,Payee,Amount\n2025-03-01,Car,\"-1,234\"\n2025-03-02,Bar,-1.234\n2025-03-03,Bar,\"1,234.56\"\n",
            "01/02/2025;1.234;Auto\n02/02/2025;1,20;Bar\n",
        ];
        for text in files {
            let read = |l| {
                with_lang(l, || {
                    let t = analyze(text, "UTF-8").unwrap();
                    let (rows, skipped) = parse_rows(&t, &t.mapping, false);
                    (format!("{rows:?} {:?}", t.mapping), skipped)
                })
            };
            assert_eq!(read(Lang::It), read(Lang::En));
        }
    }

    #[test]
    fn unnamed_columns_have_numbers() {
        use crate::i18n::{Lang, with_lang};
        let t = analyze("01/02/2025;1,20;Bar\n02/02/2025;3,20;Bar\n", "UTF-8").unwrap();
        assert_eq!(with_lang(Lang::It, || column_label(&t.headers, 1)).as_deref(), Some("Colonna 2"));
        assert_eq!(with_lang(Lang::En, || column_label(&t.headers, 1)).as_deref(), Some("Column 2"));
        for key in ["2", "Colonna 2", "column 2"] {
            assert_eq!(column_index(&t, key), Some(1), "{key}");
        }
        for key in ["", "!", "Column 0", "Column 4"] {
            assert_eq!(column_index(&t, key), None, "{key}");
        }
    }

    /// Every input that once could crash a reader, plus broken and oversized ones.
    const HOSTILE: &[&str] = &[
        "",
        "\0\0\0",
        ";;;;\n;;;;",
        "\"unclosed;quote\nData;Importo\n",
        "Data;Importo;Descrizione\n01/01/2025;99999999999999999999999;x",
        "Data;Importo;Descrizione\n01/01/2025;9223372036854775807;x",
        "Data;Importo;Descrizione\n01/01/2025;(;x\n01/01/2025;();x\n01/01/2025;-;x\n01/01/2025;€;x",
        "Data;Importo;Descrizione\n99/99/9999;1,00;x\n01/01/2025;1,00;İİİİ presso İİ carta",
        "Data;Importo;Descrizione\n01/01/2025;1,00;PRESSO ",
        "Data\nImporto",
    ];

    #[test]
    fn hostile_statements_never_panic() {
        for text in HOSTILE {
            if let Ok(t) = analyze(text, "UTF-8") {
                for m in [t.mapping.clone(), Mapping { date: Some(99), amount: Some(99), ..Default::default() }] {
                    parse_rows(&t, &m, true);
                }
            }
            for n in 0..text.len() {
                decode(text.as_bytes().get(..n).unwrap_or_default());
            }
        }
    }

    #[test]
    fn importing_twice_adds_nothing_the_second_time() {
        let db = Db::open_in_memory().unwrap();
        let acc = db.add_account("A", crate::db::AccountKind::Bank, "EUR", 0, "").unwrap();
        // Two identical purchases on the same day are two transactions.
        let t = analyze("Data;Descrizione;Importo\n01/02/2025;Bar;-1,20\n01/02/2025;Bar;-1,20\n", "UTF-8").unwrap();
        let (rows, skipped) = parse_rows(&t, &t.mapping, false);
        assert_eq!(import_rows(&db, acc, &rows, skipped).unwrap().added, 2);
        let again = import_rows(&db, acc, &rows, skipped).unwrap();
        assert_eq!((again.added, again.duplicates), (0, 2));
    }
}
