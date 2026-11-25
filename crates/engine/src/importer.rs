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
//! I file OFX sono letti da `ofx.rs`, le carte in PDF da `card_pdf.rs`.
//!
//! Le righe che non sono movimenti (saldi, totali, righe vuote) vengono saltate.
//! Reimportare lo stesso file non crea doppioni.

pub use crate::card_pdf::{CardRow, Check, CheckKind, Statement};
use crate::db::{Cents, Db, NewTx, Transaction, TxFilter};
use crate::duplicates::{Decision, Deletion, State};
use crate::i18n::{tr, trf};
use crate::money::{DateOrder, parse_amount_with, parse_date, parse_date_as};
use crate::ofx::{self, Correction, Ofx, OfxStatement};
use crate::transfers::near;
use crate::{pdf, xls};
use anyhow::{Context, Result, anyhow, bail};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::Read as _;
use std::path::Path;

/// Dimensione massima di un estratto conto.
pub(crate) const MAX_FILE: u64 = 32 << 20;
/// Largest CSV table.
const MAX_FIELDS: usize = 1_000;
const MAX_CELLS: usize = 2_000_000;
/// How many rows that could not be read are told one by one.
const MAX_PROBLEMS: usize = 100;
/// How many sides of transfers a row is compared with.
const MAX_SIDES: usize = 20;
const EXCEL: &str = "Excel 97-2003";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Mapping {
    pub date: Option<usize>,
    pub value_date: Option<usize>,
    pub payee: Option<usize>,
    pub description: Option<usize>,
    pub extra: Option<usize>,
    pub amount: Option<usize>,
    pub credit: Option<usize>,
    pub debit: Option<usize>,
    /// Says whether the bank has booked the row.
    pub status: Option<usize>,
}

impl Mapping {
    /// Every column that is used.
    pub(crate) fn columns(&self) -> Vec<usize> {
        let all = [
            self.date,
            self.value_date,
            self.payee,
            self.description,
            self.extra,
            self.amount,
            self.credit,
            self.debit,
            self.status,
        ];
        all.into_iter().flatten().collect()
    }
}

/// How the rows of a CSV or Excel file are read.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub mapping: Mapping,
    /// Changes the sign of the amounts.
    pub invert: bool,
    /// Whether the decimal separator is the comma; `None` finds it in each file.
    pub decimal_comma: Option<bool>,
    pub dates: DateOrder,
}

impl Settings {
    pub fn detected(t: &Table) -> Self {
        Settings { mapping: t.mapping.clone(), ..Default::default() }
    }

    /// Those the user confirmed for this layout in this account, or else what was found in the file.
    pub fn of(db: &Db, t: &Table, account: Option<i64>) -> Result<(Self, Option<i64>)> {
        Ok(match db.layout_of(t, account)? {
            Some(l) => (l.settings()?, Some(l.id)),
            None => (Self::detected(t), None),
        })
    }

    /// The decimal separator in use.
    pub fn comma(&self, t: &Table) -> bool {
        self.decimal_comma.unwrap_or(t.decimal_comma)
    }

    /// The decimal separator that the chosen columns seem to have, when it is not the one in use.
    pub fn doubtful_decimals(&self, t: &Table) -> Option<char> {
        let seems = detect_decimal_comma(t, &self.mapping);
        let other = t.has_columns() && self.decimal_comma.is_none() && seems != t.decimal_comma;
        other.then_some(if seems { ',' } else { '.' })
    }
}

/// A row that has something of a transaction and could not be read as one.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Problem {
    pub line: usize,
    pub error: String,
}

/// The rows of a file.
#[derive(Clone, Debug, Default)]
pub struct Read {
    pub rows: Vec<ParsedRow>,
    pub skipped: usize,
    /// The first of those that were skipped and look like transactions.
    pub problems: Vec<Problem>,
    /// How many there are in all.
    pub problem_count: usize,
}

impl Read {
    /// Whether most rows were read: the columns may be remembered.
    pub fn is_convincing(&self) -> bool {
        !self.rows.is_empty() && self.rows.len() >= self.problem_count
    }
}

/// The kind of file that a row comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Format {
    #[default]
    Csv,
    Xls,
    Pdf,
    Ofx,
}

impl Format {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Xls => "xls",
            Format::Pdf => "pdf",
            Format::Ofx => "ofx",
        }
    }

    pub(crate) fn from_code(s: &str) -> Option<Self> {
        [Format::Csv, Format::Xls, Format::Pdf, Format::Ofx].into_iter().find(|f| f.code() == s)
    }
}

/// What only a row of OFX has.
#[derive(Clone, Debug)]
pub struct OfxRowId {
    pub fitid: Option<String>,
    pub corrects: Option<Correction>,
    /// The amount as the file has it.
    pub raw: Cents,
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
    /// A card statement, whose rows are known: no columns to choose.
    pub statement: Option<Statement>,
    /// Where each record starts in the file; empty when it has no lines.
    pub lines: Vec<usize>,
    /// The statements of an OFX file: no columns to choose.
    pub ofx: Option<Ofx>,
}

impl Table {
    pub fn format(&self) -> Format {
        match (&self.statement, &self.ofx) {
            (Some(_), _) => Format::Pdf,
            (_, Some(_)) => Format::Ofx,
            _ if self.encoding == EXCEL => Format::Xls,
            _ => Format::Csv,
        }
    }

    /// Whether there are columns to choose.
    pub fn has_columns(&self) -> bool {
        self.statement.is_none() && self.ofx.is_none()
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ParsedRow {
    pub date: NaiveDate,
    pub amount: Cents,
    pub payee: String,
    pub memo: String,
    /// What tells it from the other rows of the account, instead of its contents.
    #[serde(skip)]
    pub key: Option<String>,
    #[serde(skip)]
    pub format: Format,
    #[serde(skip)]
    pub ofx: Option<OfxRowId>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ImportResult {
    pub read: usize,
    pub added: usize,
    pub duplicates: usize,
    pub skipped: usize,
    /// Rows that the user said are in the books already.
    pub same: usize,
    /// What the bank takes back of what is in the books: nothing was done.
    pub deletions: Vec<Deletion>,
    pub categorized: usize,
    /// Lines that took the place of the added side of a transfer.
    pub adopted: Vec<Adopted>,
    /// Lines that were added, and may be the side of a transfer that is there: the user chooses.
    pub maybe_transfers: Vec<MaybeTransfer>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Adopted {
    pub id: i64,
    pub date: NaiveDate,
    pub payee: String,
    pub amount: Cents,
}

#[derive(Clone, Debug, Serialize)]
pub struct MaybeTransfer {
    pub transaction: Transaction,
    /// The far sides of the transfers it may belong to, for `turn_into_transfer`.
    pub candidates: Vec<Transaction>,
}

/// Legge il file e lo converte in testo (UTF-8 o Windows-1252).
pub fn read_text(path: &Path) -> Result<(String, &'static str)> {
    Ok(decode(&read_bytes(path)?))
}

/// Reads a statement, CSV, Excel 97-2003, OFX or PDF of a card, and finds its columns.
pub fn load(path: &Path) -> Result<Table> {
    let bytes = read_bytes(path)?;
    if xls::is_xls(&bytes) {
        return Ok(table(xls::rows(&bytes)?, b'\t', EXCEL));
    }
    if pdf::is_pdf(&bytes) {
        return Ok(card_table(pdf::statement(&bytes)?));
    }
    if ofx::is_ofx(&bytes) {
        let (file, encoding) = ofx::read(&bytes)?;
        return Ok(ofx_table(file, encoding));
    }
    let (text, encoding) = decode(&bytes);
    analyze(&text, encoding)
}

pub(crate) fn card_table(statement: Statement) -> Table {
    Table {
        delimiter: b'\t',
        encoding: "PDF",
        records: Vec::new(),
        header_row: None,
        headers: Vec::new(),
        mapping: Mapping::default(),
        decimal_comma: true,
        statement: Some(statement),
        lines: Vec::new(),
        ofx: None,
    }
}

pub(crate) fn ofx_table(file: Ofx, encoding: &'static str) -> Table {
    Table {
        delimiter: b'\t',
        encoding,
        records: Vec::new(),
        header_row: None,
        headers: Vec::new(),
        mapping: Mapping::default(),
        decimal_comma: false,
        statement: None,
        lines: Vec::new(),
        ofx: Some(file),
    }
}

fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    let context = || trf("cannot read {path}", &[("path", &path.display())]);
    let mut bytes = Vec::new();
    let file = std::fs::File::open(path).with_context(context)?;
    file.take(MAX_FILE + 1).read_to_end(&mut bytes).with_context(context)?;
    if bytes.len() as u64 > MAX_FILE {
        bail!(trf("{path}: the file is larger than {mb} MB", &[("path", &path.display()), ("mb", &(MAX_FILE >> 20))]));
    }
    Ok(bytes)
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
pub(crate) fn norm(s: &str) -> String {
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

const STATUS_KW: [&str; 3] = ["stato", "stato movimento", "status"];

/// Whether a status says that the row is not booked yet. The bank may still change its text and
/// date, so it would come in twice: it is imported once booked.
fn is_pending(status: &str) -> bool {
    let s = norm(status);
    ["non contabilizzat", "da contabilizzare", "in attesa", "prenotat", "pending", "not booked"]
        .iter()
        .any(|w| s.contains(w))
}

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
    m.status = pick(&STATUS_KW, &no, &mut used);
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
    check_quotes(text, delimiter)?;
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());
    let mut records = Vec::new();
    let mut lines = Vec::new();
    let mut cells = 0;
    // The line at a place of the text. The reader counts from before the empty lines it passes.
    let (mut at, mut line) = (0, 1);
    for r in rdr.records() {
        let r = r.map_err(|e| {
            let line = e.position().map_or(0, |p| p.line());
            anyhow!(trf("line {line}: {error}", &[("line", &line), ("error", &e)]))
        })?;
        let start = r.position().and_then(|p| usize::try_from(p.byte()).ok()).unwrap_or(at);
        let blank = text.get(start..).map_or(0, |t| t.len() - t.trim_start_matches(['\r', '\n']).len());
        line += text.get(at..start + blank).map_or(0, ofx::lines);
        at = start + blank;
        cells += r.len();
        if r.len() > MAX_FIELDS || cells > MAX_CELLS {
            bail!(trf("line {line}: {error}", &[("line", &line), ("error", &tr("the table is too large"))]));
        }
        records.push(r.iter().map(|s| s.trim().to_string()).collect::<Vec<_>>());
        lines.push(line);
    }
    if records.is_empty() {
        bail!(tr("the file is empty or is not a CSV"));
    }
    Ok(Table { lines, ..table(records, delimiter, encoding) })
}

/// A quote that opens a field and is never closed would swallow the rest of the file.
fn check_quotes(text: &str, delimiter: u8) -> Result<()> {
    let mut line = 1;
    let mut opened = 0;
    let (mut start, mut quoted) = (true, false);
    let mut bytes = text.bytes().peekable();
    while let Some(b) = bytes.next() {
        let newline = b == b'\n' || (b == b'\r' && bytes.peek() != Some(&b'\n'));
        line += usize::from(newline);
        if quoted {
            if b == b'"' && bytes.next_if_eq(&b'"').is_none() {
                quoted = false;
            }
            continue;
        }
        if b == b'"' && start {
            (quoted, opened) = (true, line);
        }
        start = b == delimiter || b == b'\r' || b == b'\n';
    }
    if quoted {
        let error = tr("the quotation mark that opens a value is never closed");
        bail!(trf("line {line}: {error}", &[("line", &opened), ("error", &error)]));
    }
    Ok(())
}

/// Finds the headings, the columns and the decimal separator of a statement's rows.
fn table(records: Vec<Vec<String>>, delimiter: u8, encoding: &'static str) -> Table {
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
    let mut t = Table {
        delimiter,
        encoding,
        records,
        header_row,
        headers,
        mapping,
        decimal_comma: true,
        statement: None,
        lines: Vec::new(),
        ofx: None,
    };
    t.decimal_comma = detect_decimal_comma(&t, &t.mapping);
    t
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

fn detect_decimal_comma(t: &Table, m: &Mapping) -> bool {
    let cols: Vec<usize> = [m.amount, m.credit, m.debit].into_iter().flatten().collect();
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
    // Layouts that name the other party between two fixed words, the first one at the start.
    for (start, end) in [("addebito sdd ", " n:"), ("bonifico o/c: ", " abi-cab:"), ("", " operazione carta ")] {
        if let Some(rest) = lower.strip_prefix(start)
            && let Some(n) = rest.find(end)
            && let Some(p) = desc.get(start.len()..start.len() + n)
            && p.trim().len() >= 2
        {
            return collapse(p);
        }
    }
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
/// Those of a card statement are as they are.
pub fn parse_rows(t: &Table, m: &Mapping, invert: bool) -> (Vec<ParsedRow>, usize) {
    let settings =
        Settings { mapping: m.clone(), invert, decimal_comma: Some(t.decimal_comma), dates: DateOrder::Auto };
    let read = read_rows(t, &settings);
    (read.rows, read.skipped)
}

/// The rows of an OFX statement.
fn ofx_rows(s: &OfxStatement, invert: bool) -> Vec<ParsedRow> {
    let mut rows: Vec<ParsedRow> = s
        .rows
        .iter()
        .map(|r| {
            let (payee, memo) = match (r.name.is_empty(), r.memo.is_empty()) {
                (true, _) => {
                    let p = smart_payee(&r.memo);
                    let memo = if p == r.memo { String::new() } else { r.memo.clone() };
                    (p, memo)
                }
                (false, _) if r.memo == r.name => (r.name.clone(), String::new()),
                _ => (r.name.clone(), r.memo.clone()),
            };
            ParsedRow {
                date: r.date,
                amount: if invert { -r.amount } else { r.amount },
                payee,
                memo,
                key: None,
                format: Format::Ofx,
                ofx: Some(OfxRowId { fitid: r.fitid.clone(), corrects: r.corrects.clone(), raw: r.amount }),
            }
        })
        .collect();
    rows.sort_by_key(|r| r.date);
    rows
}

/// [`parse_rows`] with every setting, and what could not be read.
pub fn read_rows(t: &Table, settings: &Settings) -> Read {
    let (m, invert) = (&settings.mapping, settings.invert);
    if let Some(o) = &t.ofx {
        return Read { rows: o.statement().map(|s| ofx_rows(s, invert)).unwrap_or_default(), ..Default::default() };
    }
    if let Some(s) = &t.statement {
        let row = |r: &CardRow| ParsedRow {
            date: r.date,
            amount: r.amount,
            payee: r.payee.clone(),
            memo: r.memo.clone(),
            key: Some(r.key.clone()),
            format: Format::Pdf,
            ofx: None,
        };
        return Read { rows: s.rows.iter().map(row).collect(), skipped: s.unread, ..Default::default() };
    }
    let start = t.header_row.map(|h| h + 1).unwrap_or(0);
    let mut out = Vec::new();
    let mut skipped = 0;
    let mut problems = Vec::new();
    let mut problem_count = 0;
    let dc = Some(settings.comma(t));
    let get = |r: &Vec<String>, c: Option<usize>| -> String { c.and_then(|i| r.get(i)).cloned().unwrap_or_default() };
    let day = |r: &Vec<String>, c| parse_date_as(&get(r, c), settings.dates);
    for (i, r) in t.records.iter().enumerate().skip(start) {
        // A row with neither a date nor an amount is a heading or a note.
        let mut problem = |error: String| {
            problem_count += 1;
            if problems.len() < MAX_PROBLEMS {
                problems.push(Problem { line: t.lines.get(i).copied().unwrap_or(i + 1), error });
            }
        };
        let text = |c| get(r, c).chars().take(60).collect::<String>();
        if r.iter().all(|c| c.is_empty()) {
            continue;
        }
        if is_pending(&get(r, m.status)) {
            skipped += 1;
            continue;
        }
        let Some(date) = day(r, m.date).or_else(|| day(r, m.value_date)) else {
            skipped += 1;
            let has_amount =
                [m.amount, m.credit, m.debit].into_iter().any(|c| parse_amount_with(&get(r, c), dc).is_ok());
            if has_amount && !text(m.date).is_empty() {
                problem(trf("\"{date}\" is not a date", &[("date", &text(m.date))]));
            }
            continue;
        };
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
                (cv, dv) => {
                    skipped += 1;
                    for e in [cv.err(), dv.err()].into_iter().flatten() {
                        problem(e.to_string());
                    }
                    continue;
                }
            }
        } else {
            match parse_amount_with(&get(r, m.amount), dc) {
                Ok(v) => v,
                Err(e) => {
                    skipped += 1;
                    problem(e.to_string());
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
        out.push(ParsedRow { date, amount, payee, memo, key: None, format: t.format(), ofx: None });
    }
    out.sort_by_key(|r| r.date);
    Read { rows: out, skipped, problems, problem_count }
}

pub(crate) fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// What the user chose for an import.
#[derive(Clone, Debug, Default)]
pub struct Choices<'a> {
    /// A card statement that does not add up is imported all the same.
    pub allow_mismatch: bool,
    /// The rows that may be in the books already.
    pub likely: Decision,
    /// How the rows were read, remembered for the next file with this layout in this account.
    pub settings: Option<&'a Settings>,
    /// The user said that the signs of an OFX statement are right, or inverted, whatever its types say.
    pub sign_confirmed: bool,
}

/// [`import_with`] without any choice about duplicates.
pub fn import(
    db: &Db,
    account_id: i64,
    t: &Table,
    rows: &[ParsedRow],
    skipped: usize,
    allow_mismatch: bool,
) -> Result<ImportResult> {
    import_with(db, account_id, t, rows, skipped, &Choices { allow_mismatch, ..Default::default() })
}

/// [`import_rows`] for a whole file. A card statement that does not add up is refused, unless the user
/// asked to import it all the same, and is remembered to find its payment. An OFX statement must be in the
/// currency of the account.
pub fn import_with(
    db: &Db,
    account_id: i64,
    t: &Table,
    rows: &[ParsedRow],
    skipped: usize,
    choices: &Choices,
) -> Result<ImportResult> {
    if let Some(o) = &t.ofx {
        let s = o.statement().ok_or_else(|| choose_statement(o))?;
        return db.atomically(|| {
            let (_, currency) = db.money_ledger(account_id)?;
            if currency != s.currency {
                bail!(trf(
                    "the statement is in {statement} and the account is in {account}: nothing was imported",
                    &[("statement", &s.currency), ("account", &currency)]
                ));
            }
            if s.signs_contradict() && !choices.sign_confirmed {
                bail!(tr("the signs of the amounts contradict the types of the transactions: say whether to invert them; nothing was imported"));
            }
            import_deciding(db, account_id, rows, skipped, &choices.likely)
        });
    }
    let Some(s) = &t.statement else {
        return db.atomically(|| {
            let res = import_deciding(db, account_id, rows, skipped, &choices.likely)?;
            if let Some(settings) = choices.settings {
                db.save_layout(t, account_id, settings)?;
            }
            Ok(res)
        });
    };
    let (Some(closing), true) = (s.closing, s.is_identified()) else {
        bail!(tr("the date or the card number of the statement was not read: nothing was imported"));
    };
    if !s.is_sound() && !choices.allow_mismatch {
        bail!(tr("the rows read do not add up to the totals of the card statement: nothing was imported"));
    }
    db.atomically(|| {
        let (_, currency) = db.money_ledger(account_id)?;
        if currency != "EUR" {
            bail!(tr("the card statement is in euros and the account is not"));
        }
        if let (Some(debit_date), Some(debit)) = (s.debit_date, s.debit) {
            db.record_card_statement(account_id, closing, debit_date, debit, &s.key(closing))?;
        }
        import_deciding(db, account_id, rows, skipped, &choices.likely)
    })
}

pub(crate) fn choose_statement(o: &Ofx) -> anyhow::Error {
    anyhow!(trf("the file has several statements, choose one: {list}", &[("list", &o.list())]))
}

/// Salva i movimenti nel conto, saltando quelli già importati in precedenza.
pub fn import_rows(db: &Db, account_id: i64, rows: &[ParsedRow], skipped: usize) -> Result<ImportResult> {
    import_deciding(db, account_id, rows, skipped, &Decision::default())
}

pub(crate) fn import_deciding(
    db: &Db,
    account_id: i64,
    rows: &[ParsedRow],
    skipped: usize,
    decision: &Decision,
) -> Result<ImportResult> {
    let mut res = ImportResult { read: rows.len(), skipped, ..Default::default() };
    db.atomically(|| {
        let plan = db.plan_import(Some(account_id), rows)?;
        let same = db.decide(&plan, account_id, rows, decision)?;
        res.deletions = db.describe_deletions(&plan, Some(account_id), rows)?;
        let mut fresh: Vec<usize> = Vec::new();
        // The transaction that each row is, and what each transaction is known by from now on.
        let mut place: HashMap<usize, i64> = HashMap::new();
        let mut keys: BTreeMap<i64, Vec<&str>> = BTreeMap::new();
        for (i, p) in plan.rows.iter().enumerate() {
            match (&p.state, same.get(&i)) {
                (State::Known(id), _) => {
                    res.duplicates += 1;
                    place.insert(i, *id);
                }
                (State::Deletes(there), _) => {
                    if let Some(id) = there {
                        let all = std::iter::once(p.hash.as_str()).chain(p.marks.split(' '));
                        keys.entry(*id).or_default().extend(all);
                    }
                }
                (_, Some(there)) => {
                    res.same += 1;
                    place.extend(there.map(|id| (i, id)));
                }
                _ => fresh.push(i),
            }
        }
        // The rows of a card statement are never sides of transfers.
        let fits =
            |row: &ParsedRow, t: &Transaction| row.key.is_none() && row.amount == t.amount && near(row.date, t.date);
        let mut sides: HashMap<Cents, Vec<Transaction>> = HashMap::new();
        for t in db.added_sides(account_id)? {
            sides.entry(t.amount).or_default().push(t);
        }
        // How many rows each side fits.
        let mut dates: HashMap<Cents, BTreeMap<NaiveDate, usize>> = HashMap::new();
        for row in
            fresh.iter().filter_map(|i| rows.get(*i)).filter(|r| r.key.is_none() && sides.contains_key(&r.amount))
        {
            *dates.entry(row.amount).or_default().entry(row.date).or_default() += 1;
        }
        let rows_of = |t: &Transaction| -> usize {
            let near = dates.get(&t.amount).into_iter().flatten().filter(|(d, _)| near(**d, t.date));
            near.map(|(_, n)| n).sum()
        };
        let mut unsure = Vec::new();
        for i in &fresh {
            let (Some(row), Some(p)) = (rows.get(*i), plan.rows.get(*i)) else { continue };
            let mine: Vec<&Transaction> =
                sides.get(&row.amount).into_iter().flatten().filter(|t| fits(row, t)).take(MAX_SIDES).collect();
            // Only when neither the line nor the side could be another one.
            if let [side] = mine.as_slice()
                && rows_of(side) == 1
            {
                db.adopt(side, row.date, &row.payee, &row.memo, &p.hash, &p.marks)?;
                let payee = row.payee.clone();
                res.adopted.push(Adopted { id: side.id, date: row.date, payee, amount: row.amount });
                place.insert(*i, side.id);
                continue;
            }
            let id = db.add_transaction(&NewTx {
                account_id,
                date: row.date,
                amount: row.amount,
                payee: row.payee.clone(),
                memo: row.memo.clone(),
                category_id: None,
                cleared: true,
                import_hash: Some(p.hash.clone()),
                import_marks: Some(p.marks.clone()),
                card_row: row.key.is_some(),
            })?;
            let Some(id) = id else {
                res.duplicates += 1;
                continue;
            };
            place.insert(*i, id);
            res.added += 1;
            if db.category_of(id)?.is_some() {
                res.categorized += 1;
            }
            if !mine.is_empty() {
                unsure.push((id, mine.iter().filter_map(|t| t.transfer_peer).collect::<Vec<_>>()));
            }
        }
        // What the user said is remembered: the same file asks nothing the second time.
        for (i, p) in plan.rows.iter().enumerate().filter(|(i, _)| same.contains_key(i)) {
            let id = place.get(&i).or_else(|| place.get(&p.repeats?));
            if let Some(id) = id {
                keys.entry(*id).or_default().extend(std::iter::once(p.hash.as_str()).chain(p.marks.split(' ')));
            }
        }
        for (id, keys) in &keys {
            db.add_import_keys(*id, keys)?;
        }
        if !unsure.is_empty() {
            let all = db.transactions(&TxFilter::default())?;
            let all: HashMap<i64, Transaction> = all.into_iter().map(|t| (t.id, t)).collect();
            for (id, peers) in unsure {
                let Some(transaction) = all.get(&id).cloned() else { continue };
                let candidates = peers.iter().filter_map(|p| all.get(p).cloned()).collect();
                res.maybe_transfers.push(MaybeTransfer { transaction, candidates });
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
            "status": name(m.status),
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
pub(crate) mod tests {
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

    /// What only `csv` reads, read by it alone: the reader of PDF files is not started.
    fn read_any(bytes: &[u8]) -> Result<Table> {
        if xls::is_xls(bytes) {
            return load_bytes(bytes);
        }
        if ofx::is_ofx(bytes) {
            let (o, e) = ofx::read(bytes)?;
            return Ok(ofx_table(o, e));
        }
        let (text, encoding) = decode(bytes);
        analyze(&text, encoding)
    }

    /// Thousands of damaged files for each reader, the same at every run and for a limited time.
    #[test]
    fn damaged_files_never_panic() {
        use crate::testing::{Seeded, mutate};
        let csv: &[u8] = "Conto;X;;\nData;Descrizione;Importo;Data valuta\n01/02/2025;\"Bar; è \"\"qui\"\"\";-1.234,56;02/02/2025\r\n03/02/2025;PRESSO BAR;1,20;\nTotale;;9,99;\n".as_bytes();
        let ofx = ofx::tests::sgml();
        let two = ofx::tests::xml();
        let book = bper();
        let pieces = [
            "\"",
            ";",
            "\n",
            "\r",
            "<",
            ">",
            "</STMTTRN>",
            "<STMTTRN>",
            "<![CDATA[",
            "&#x10FFFF;",
            "&#0;",
            "&amp;",
            "<MEMO>",
            "İ",
            "\0",
            "99999999999999999999",
            "-0,005",
            "20251399",
            "[+99:X]",
            "<OFX>",
            "]]>",
            "<!--",
            "9223372036854775807",
            "\u{feff}",
            "<CURDEF>USD",
            "01/01/0000",
            "FITID",
        ];
        let start = std::time::Instant::now();
        for (name, good) in [("csv", csv), ("ofx 1", ofx.as_bytes()), ("ofx 2", two.as_bytes()), ("xls", &book)] {
            let mut r = Seeded(0x9E37_79B9_7F4A_7C15);
            let until = start.elapsed() + std::time::Duration::from_secs(3);
            let mut n = 0;
            let mut read = 0;
            while n < 200 || (n < 5_000 && start.elapsed() < until) {
                n += 1;
                let bad = mutate(&mut r, good, &pieces);
                let Ok(mut t) = read_any(&bad) else { continue };
                read += 1;
                if let Some(o) = &mut t.ofx {
                    o.chosen = Some(0);
                }
                for dates in DateOrder::ALL {
                    let s = Settings { dates, invert: true, ..Settings::detected(&t) };
                    let rows = read_rows(&t, &s).rows;
                    assert!(rows.iter().all(|x| x.amount.abs() <= crate::money::MAX_AMOUNT), "{name} {n}");
                }
            }
            assert!(read > 0, "{name}: no damaged file was read at all in {n}");
        }
    }

    /// Damaged files imported for real: the books stay sound.
    #[test]
    fn damaged_files_keep_the_books_sound() {
        use crate::testing::{Seeded, mutate};
        let db = Db::open_in_memory().unwrap();
        let mut r = Seeded(42);
        let mut imported = 0;
        for n in 0..200 {
            let acc = db.add_account(&format!("A{n}"), crate::db::AccountKind::Bank, "EUR", 0, "").unwrap();
            let bad = mutate(&mut r, ofx::tests::sgml().as_bytes(), &["-5,00", "20260107", "<FITID>1"]);
            let Ok(mut t) = read_any(&bad) else { continue };
            if let Some(o) = &mut t.ofx {
                o.chosen = Some(0);
            }
            let rows = read_rows(&t, &Settings::detected(&t)).rows;
            let likely = Decision { skip_same: n % 2 == 0, add_others: true, ..Default::default() };
            let choices = Choices { likely, sign_confirmed: true, ..Default::default() };
            let done = import_with(&db, acc, &t, &rows, 0, &choices);
            imported += usize::from(done.is_ok_and(|d| d.added > 0));
        }
        assert!(imported >= 20, "{imported}");
        crate::ledger::tests::assert_sound(&db, "damaged");
    }

    #[test]
    fn what_cannot_be_read_is_told_with_its_line() {
        use crate::i18n::{Lang, with_lang};
        with_lang(Lang::En, || {
            let text = "Banca\r\n\r\n\r\nData;Descrizione;Importo\r\n01/02/2025;\"a\nb\";-1,20\r\n\n31/02/2025;Bar;-1,20\n01/03/2025;Bar;1x\n01/03/2025;Bar\nTotale;;5,00\nFine;;\n";
            let t = analyze(text, "UTF-8").unwrap();
            assert_eq!(t.header_row, Some(1));
            let read = read_rows(&t, &Settings::detected(&t));
            assert_eq!((read.rows.len(), read.skipped), (1, 5));
            let lines: Vec<_> = read.problems.iter().map(|p| p.line).collect();
            assert_eq!(lines, [8, 9, 10, 11]);
            assert_eq!(read.problems[0].error, "\"31/02/2025\" is not a date");
            let e = analyze("Data;Importo;Note\n01/01/2025;1,00;x\n02/01/2025;2,00;\"y\n03/01/2025;3,00;z\n", "UTF-8");
            assert_eq!(e.unwrap_err().to_string(), "line 3: the quotation mark that opens a value is never closed");
            // A quote in the middle of a value is text.
            let t = analyze("Data;Note;Importo\n01/01/2025;6\" x;1,00\n02/01/2025;\"a\"\"b\";2,00\n", "UTF-8").unwrap();
            assert_eq!(read_rows(&t, &Settings::detected(&t)).rows.len(), 2);
            let wide = "a;".repeat(MAX_FIELDS + 1);
            assert!(analyze(&format!("x;y\n{wide}"), "UTF-8").unwrap_err().to_string().starts_with("line 2: "));
        });
    }

    #[test]
    fn the_columns_are_remembered_for_the_layout() {
        let db = Db::open_in_memory().unwrap();
        let acc = db.add_account("A", crate::db::AccountKind::Bank, "EUR", 0, "").unwrap();
        // The detected columns are wrong for this bank: the amount is "Netto", positive when it goes out.
        let text = |day: &str, v: &str| format!("Data;Nome;Importo;Netto\n{day};Bar;1;{v}\n");
        let t = analyze(&text("02/03/2026", "1.234"), "UTF-8").unwrap();
        let (found, saved) = Settings::of(&db, &t, Some(acc)).unwrap();
        assert_eq!((&found, saved), (&Settings::detected(&t), None));
        let mut s = found.clone();
        s.mapping.amount = Some(3);
        s.invert = true;
        s.decimal_comma = Some(false);
        s.dates = DateOrder::Mdy;
        let rows = read_rows(&t, &s).rows;
        assert_eq!((rows[0].date.to_string(), rows[0].amount), ("2026-02-03".into(), -123));
        let choices = Choices { settings: Some(&s), ..Default::default() };
        // Nothing is remembered of an import that fails.
        assert!(import_with(&db, 999, &t, &rows, 0, &choices).is_err());
        assert!(db.layouts().unwrap().is_empty());
        import_with(&db, acc, &t, &rows, 0, &choices).unwrap();
        // Another file with the same headings, however they are written.
        let next = analyze(&text("12/31/2026", "5.00").replace("Netto", "NETTO "), "UTF-8").unwrap();
        let (again, id) = Settings::of(&db, &next, Some(acc)).unwrap();
        assert_eq!(again, s);
        assert_eq!(read_rows(&next, &again).rows[0].date.to_string(), "2026-12-31");
        // Into another account, or a new one, it starts from what is found: the sign above all.
        let card = db.add_account("Card", crate::db::AccountKind::CreditCard, "EUR", 0, "").unwrap();
        for other in [Some(card), None] {
            let (fresh, id) = Settings::of(&db, &next, other).unwrap();
            assert_eq!((&fresh, id), (&Settings::detected(&next), None));
        }
        import_with(
            &db,
            card,
            &t,
            &read_rows(&t, &found).rows,
            0,
            &Choices { settings: Some(&found), ..Default::default() },
        )
        .unwrap();
        assert!(Settings::of(&db, &next, Some(acc)).unwrap().0.invert);
        assert!(!Settings::of(&db, &next, Some(card)).unwrap().0.invert);
        let shown = db.layouts().unwrap()[0].json();
        assert_eq!(
            (&shown["account"], &shown["invert"], &shown["columns"]["amount"]),
            (&"A".into(), &true.into(), &"Netto".into())
        );
        // Another separator or other headings are another layout; so is a workbook, and so are two signs.
        for other in [text("02/03/2026", "1").replace(';', ","), text("02/03/2026", "1").replace("Nome", "Note")] {
            assert!(Settings::of(&db, &analyze(&other, "UTF-8").unwrap(), Some(acc)).unwrap().1.is_none());
        }
        let of = |text: &str| crate::layouts::signature(&analyze(text, "UTF-8").unwrap());
        let (plus, minus) = (
            "Data;Descrizione;Importo;+;-\n05/01/2026;Bar;1,00;;\n",
            "Data;Descrizione;Importo;-;+\n05/01/2026;Bar;1,00;;\n",
        );
        assert!(of(plus).is_some() && of(plus) != of(minus));
        assert!(crate::layouts::signature(&load_bytes(&bper()).unwrap()).unwrap().starts_with("xls | "));
        // Without headings there is nothing to know it by.
        assert!(of("01/02/2025;1,20;Bar\n").is_none());
        db.forget_layout(id.unwrap()).unwrap();
        assert!(db.forget_layout(id.unwrap()).is_err());
        assert!(Settings::of(&db, &next, Some(acc)).unwrap().1.is_none());
        // One that cannot be read is an error, never the detected columns, and can be seen and forgotten.
        db.save_layout(&t, acc, &s).unwrap();
        let wide =
            serde_json::to_string(&Settings { mapping: Mapping { date: Some(99), ..found.mapping }, ..s }).unwrap();
        for bad in ["{", "{\"mapping\":{\"amount\":-1}}", &wide] {
            db.conn.execute("UPDATE csv_layouts SET settings=?1", [bad]).unwrap();
            assert!(Settings::of(&db, &t, Some(acc)).is_err());
            assert_eq!(db.layouts().unwrap()[0].json()["readable"], false);
        }
        assert_eq!(db.forget_layouts().unwrap(), 2);
        // It goes with its account.
        db.save_layout(&t, card, &Settings::detected(&t)).unwrap();
        db.delete_account(card).unwrap();
        assert!(db.layouts().unwrap().is_empty());
        crate::ledger::tests::assert_sound(&db, "layouts");
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

    /// A workbook laid out as BPER's "Movimenti Conto" export, with invented contents.
    fn bper() -> Vec<u8> {
        let row = |cells: &[&str]| cells.iter().map(|c| c.to_string()).collect::<Vec<String>>();
        let rows = vec![
            row(&["Lunedì,01 dicembre 2025"]),
            row(&[]),
            row(&["CONTO", "", "", "Saldo disponibile", "1.000,00 €"]),
            row(&["IBAN", "IT00X0000000000000000000000", "", "Saldo iniziale", "500,00 €"]),
            row(&[]),
            row(&["Data operazione", "Data valuta", "Descrizione", "Entrate", "Uscite", "Categoria", "Stato"]),
            row(&[
                "28 novembre 2025",
                "29 novembre 2025",
                "Bar Esempio          ",
                " ",
                "-9.9",
                "PAGAMENTO",
                "Contabilizzato",
            ]),
            row(&[
                "27 novembre 2025",
                "27 novembre 2025",
                "BONIFICO DI PROVA",
                "1234.5",
                " ",
                "BONIFICO",
                "Contabilizzato",
            ]),
            row(&[
                "26 novembre 2025",
                "26 novembre 2025",
                "Negozio Esempio",
                " ",
                "-12",
                "PAGAMENTO",
                "Non contabilizzato",
            ]),
            row(&["2 maggio 2025", "2 maggio 2025", "COMMISSIONE", " ", "-0.29", "COMMISSIONE", "Contabilizzato"]),
            row(&[" ", " ", "Totale", "1.234,50", "-22,19", " ", " "]),
            row(&[]),
            row(&["Dati aggiornati al 01 dicembre 2025"]),
        ];
        craft_xls::testing::workbook(&[("Movimenti Conto", rows)]).unwrap()
    }

    pub(crate) fn load_bytes(bytes: &[u8]) -> Result<Table> {
        Ok(table(xls::rows(bytes)?, b'\t', EXCEL))
    }

    #[test]
    fn payees_between_fixed_words() {
        for (desc, payee) in [
            ("BAR ESEMPIO ROMA IT Operazione carta ****0000 del 01.01.2025", "BAR ESEMPIO ROMA IT"),
            ("ADDEBITO SDD Luce Esempio   N: 0000000000/00 ID:X0000 canone", "Luce Esempio"),
            (
                "BONIFICO o/c: DITTA ESEMPIO SRL  ABI-CAB: 00000-00000 a favore di Tizio Caio Num. 0",
                "DITTA ESEMPIO SRL",
            ),
            ("addebito sdd ", "addebito sdd"),
        ] {
            assert_eq!(smart_payee(desc), payee);
        }
    }

    #[test]
    fn bper_statement() {
        let t = load_bytes(&bper()).unwrap();
        let (rows, skipped) = parse_rows(&t, &t.mapping, false);
        let got: Vec<_> = rows.iter().map(|r| (r.date.to_string(), r.amount, r.payee.as_str())).collect();
        assert_eq!(
            got,
            [
                ("2025-05-02".into(), -29, "COMMISSIONE"),
                ("2025-11-27".into(), 123_450, "BONIFICO DI PROVA"),
                ("2025-11-28".into(), -990, "Bar Esempio"),
            ]
        );
        assert!(skipped >= 2, "the pending row and the totals");
        let db = Db::open_in_memory().unwrap();
        let acc = db.add_account("A", crate::db::AccountKind::Bank, "EUR", 0, "").unwrap();
        assert_eq!(import_rows(&db, acc, &rows, skipped).unwrap().added, 3);
        assert_eq!(import_rows(&db, acc, &rows, skipped).unwrap().added, 0);
    }

    #[test]
    fn date_cells_of_a_workbook_are_dates() {
        let rows = vec![
            vec!["Data".into(), "Descrizione".into(), "Importo".into()],
            vec!["date:45989".into(), "Bar".into(), "-1.2".into()],
        ];
        let t = load_bytes(&craft_xls::testing::workbook(&[("a", rows)]).unwrap()).unwrap();
        let (rows, _) = parse_rows(&t, &t.mapping, false);
        assert_eq!(
            rows.iter().map(|r| (r.date.to_string(), r.amount)).collect::<Vec<_>>(),
            [("2025-11-28".into(), -120)]
        );
    }

    #[test]
    fn damaged_workbooks_never_panic() {
        let good = bper();
        for n in (0..good.len()).step_by(37) {
            let _ = load_bytes(&good[..n]);
            let mut bad = good.clone();
            bad[n] ^= 0xFF;
            if let Ok(t) = load_bytes(&bad) {
                parse_rows(&t, &t.mapping, false);
            }
        }
    }
}
