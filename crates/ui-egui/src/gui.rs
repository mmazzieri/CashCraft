//! Finestra di CashCraft (eframe/egui): avvio, stato, stile e widget comuni.
//! Le singole schermate sono in `views.rs`, le finestre di dialogo in `dialogs.rs`.

use crate::datafile::{self, DataFile, Start};
use crate::ledger::{EntryForm, LedgerAccountForm, LedgerData};
use crate::rules::{RuleForm, RunForm};
use anyhow::Result;
use cashcraft_engine::cards::{CardStatement, PaymentProposal};
use cashcraft_engine::db::{self, Account, AccountKind, Category, Cents, Db, Scheduled, Transaction, TxFilter, TxKind};
use cashcraft_engine::duplicates::{Choice, Decision, Deletion, LikelyDuplicate};
use cashcraft_engine::i18n::{self, Lang, lang, tr, trf};
use cashcraft_engine::importer::{self, ImportResult, Mapping, ParsedRow, Problem, Settings, Table};

use cashcraft_engine::money::{DateOrder, fmt_date, fmt_eur, fmt_number, month_short, total};
use cashcraft_engine::rules::{LastRun, Rule};
use cashcraft_engine::store::{Home, Opened, Use};
use cashcraft_engine::transfers::OtherSide;
use cashcraft_engine::{demo, ops, rules};
use chrono::{Datelike, NaiveDate};
use egui::{self, Align2, Color32, CornerRadius, FontId, Frame, Margin, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

/// How many rows of a file are shown before it is imported.
pub const PREVIEWED: usize = 50;

// ------------------------------------------------------------------ colori

/// The colours of one theme. The window reads them through [`pal`], which follows the theme
/// egui draws with (the system's, or the one chosen in the sidebar).
pub struct Palette {
    pub accent: Color32,
    /// Behind white text: buttons, the selected segment.
    pub accent_fill: Color32,
    pub accent_soft: Color32,
    pub sidebar: Color32,
    pub bg: Color32,
    pub card: Color32,
    /// Text fields and other sunken areas.
    pub field: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub green: Color32,
    /// Behind white text: the income segment, the reconciled mark.
    pub green_fill: Color32,
    pub red: Color32,
    /// Behind white text: the delete buttons.
    pub red_fill: Color32,
    pub amber: Color32,
    pub blue: Color32,
    pub zebra: Color32,
    pub track: Color32,
    pub hover: Color32,
    /// The grid lines of the charts.
    pub grid: Color32,
    /// The header of a card with a table.
    pub header: Color32,
    pub row_selected: Color32,
    pub check_border: Color32,
    pub warn_bg: Color32,
    pub warn_text: Color32,
}

pub static LIGHT: Palette = Palette {
    // Ochre, the colour of the icon's tile, darkened to carry white text.
    accent: Color32::from_rgb(0x8E, 0x5F, 0x0E),
    accent_fill: Color32::from_rgb(0x8E, 0x5F, 0x0E),
    accent_soft: Color32::from_rgb(0xF6, 0xEC, 0xD6),
    sidebar: Color32::from_rgb(0xF2, 0xF4, 0xF3),
    bg: Color32::from_rgb(0xFA, 0xFB, 0xFB),
    card: Color32::WHITE,
    field: Color32::WHITE,
    border: Color32::from_rgb(0xE1, 0xE5, 0xE4),
    text: Color32::from_rgb(0x22, 0x2B, 0x2A),
    muted: Color32::from_rgb(0x60, 0x6B, 0x69),
    green: Color32::from_rgb(0x1B, 0x80, 0x47),
    green_fill: Color32::from_rgb(0x1B, 0x80, 0x47),
    red: Color32::from_rgb(0xC6, 0x3B, 0x34),
    red_fill: Color32::from_rgb(0xC6, 0x3B, 0x34),
    amber: Color32::from_rgb(0xD9, 0x8E, 0x1C),
    blue: Color32::from_rgb(0x3B, 0x6E, 0xC4),
    zebra: Color32::from_rgb(0xF6, 0xF8, 0xF8),
    track: Color32::from_rgb(0xEC, 0xEF, 0xEE),
    hover: Color32::from_rgb(0xEB, 0xE6, 0xDC),
    grid: Color32::from_rgb(0xEE, 0xF1, 0xF0),
    header: Color32::from_rgb(0xF4, 0xF6, 0xF5),
    row_selected: Color32::from_rgb(0xF7, 0xF2, 0xE7),
    check_border: Color32::from_rgb(0xB8, 0xC0, 0xBE),
    warn_bg: Color32::from_rgb(0xFF, 0xF4, 0xDC),
    warn_text: Color32::from_rgb(0x7A, 0x55, 0x10),
};

pub static DARK: Palette = Palette {
    accent: Color32::from_rgb(0xE0, 0xAE, 0x4A),
    accent_fill: Color32::from_rgb(0x8E, 0x5F, 0x0E),
    accent_soft: Color32::from_rgb(0x3A, 0x30, 0x1A),
    sidebar: Color32::from_rgb(0x1A, 0x20, 0x1F),
    bg: Color32::from_rgb(0x14, 0x19, 0x18),
    card: Color32::from_rgb(0x1E, 0x25, 0x24),
    field: Color32::from_rgb(0x11, 0x15, 0x14),
    border: Color32::from_rgb(0x34, 0x3E, 0x3C),
    text: Color32::from_rgb(0xE3, 0xE8, 0xE7),
    muted: Color32::from_rgb(0x9C, 0xA8, 0xA6),
    green: Color32::from_rgb(0x55, 0xC7, 0x84),
    green_fill: Color32::from_rgb(0x1B, 0x76, 0x41),
    red: Color32::from_rgb(0xF2, 0x7A, 0x71),
    red_fill: Color32::from_rgb(0xB0, 0x34, 0x2E),
    amber: Color32::from_rgb(0xE8, 0xAA, 0x45),
    blue: Color32::from_rgb(0x74, 0xA1, 0xEA),
    zebra: Color32::from_rgb(0x22, 0x29, 0x28),
    track: Color32::from_rgb(0x2B, 0x33, 0x32),
    hover: Color32::from_rgb(0x3A, 0x38, 0x31),
    grid: Color32::from_rgb(0x2A, 0x32, 0x31),
    header: Color32::from_rgb(0x25, 0x2D, 0x2C),
    row_selected: Color32::from_rgb(0x2E, 0x2B, 0x22),
    check_border: Color32::from_rgb(0x5A, 0x66, 0x64),
    warn_bg: Color32::from_rgb(0x3A, 0x2F, 0x14),
    warn_text: Color32::from_rgb(0xF0, 0xC8, 0x70),
};

/// Whether the window is drawn dark: set at the start of each frame.
static DARK_MODE: AtomicBool = AtomicBool::new(false);

/// The colours of the theme the window is drawn with now.
pub fn pal() -> &'static Palette {
    if DARK_MODE.load(Ordering::Relaxed) { &DARK } else { &LIGHT }
}

pub fn set_dark(dark: bool) {
    DARK_MODE.store(dark, Ordering::Relaxed);
}

pub const PALETTE: [Color32; 10] = [
    Color32::from_rgb(0x1F, 0x7A, 0x6A),
    Color32::from_rgb(0xE0, 0x8A, 0x2E),
    Color32::from_rgb(0x3B, 0x6E, 0xC4),
    Color32::from_rgb(0xC4, 0x4D, 0x6B),
    Color32::from_rgb(0x7A, 0x5C, 0xC4),
    Color32::from_rgb(0x4F, 0xA3, 0x4F),
    Color32::from_rgb(0xC9, 0xB0, 0x2B),
    Color32::from_rgb(0x2E, 0xA6, 0xB8),
    Color32::from_rgb(0x8C, 0x6A, 0x4F),
    Color32::from_rgb(0x9A, 0xA3, 0xA1),
];

/// The languages to choose from: the system's when `None`. Languages have their own names.
pub const LANGUAGES: [Option<Lang>; 3] = [None, Some(Lang::En), Some(Lang::It)];

pub fn language_name(l: Option<Lang>) -> &'static str {
    match l {
        None => tr("System"),
        Some(Lang::En) => "English",
        Some(Lang::It) => "Italiano",
    }
}

/// The colours of the window: the system's (the default), or always light or dark.
/// Kept in the data file next to the language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    System,
    Light,
    Dark,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::System, Theme::Light, Theme::Dark];
    pub const META_KEY: &str = "theme";

    pub fn from_setting(s: Option<&str>) -> Self {
        match s {
            Some("light") => Theme::Light,
            Some("dark") => Theme::Dark,
            _ => Theme::System,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Theme::System => tr("System"),
            Theme::Light => tr("Light"),
            Theme::Dark => tr("Dark"),
        }
    }

    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Theme::System => egui::ThemePreference::System,
            Theme::Light => egui::ThemePreference::Light,
            Theme::Dark => egui::ThemePreference::Dark,
        }
    }
}

pub fn error_text(e: &anyhow::Error) -> String {
    trf("Error: {error}", &[("error", &format_args!("{e:#}"))])
}

/// The value, or an empty one with the error shown: a failed read must not look like "no data".
pub fn or_report<T: Default>(r: Result<T>, toast: &mut Option<(String, f64)>) -> T {
    r.unwrap_or_else(|e| {
        *toast = Some((format!("{e:#}"), f64::INFINITY));
        T::default()
    })
}

/// Whether a transaction has one category, or may be given one.
pub fn has_category(t: &Transaction) -> bool {
    matches!(t.kind, TxKind::Category | TxKind::Uncategorized)
}

/// The colour of the `i`-th series of a chart.
pub fn palette(i: usize) -> Color32 {
    PALETTE.get(i % PALETTE.len()).copied().unwrap_or(pal().accent)
}

pub fn amount_color(c: Cents) -> Color32 {
    if c < 0 {
        pal().red
    } else if c > 0 {
        pal().green
    } else {
        pal().muted
    }
}

pub fn kind_icon(k: AccountKind) -> &'static str {
    match k {
        AccountKind::Bank => "🏦",
        AccountKind::Cash => "💶",
        AccountKind::CreditCard => "💳",
        AccountKind::Savings => "🐖",
        AccountKind::Investment => "📈",
        AccountKind::Loan => "📄",
    }
}

// ------------------------------------------------------------------ stato

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Dashboard,
    Register(Option<i64>),
    Reports,
    Budget,
    Categories,
    Rules,
    Scheduled,
    Ledger,
    /// The reconciliation of an account with its bank statement.
    Reconcile(i64),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Period {
    ThisMonth,
    LastMonth,
    Last3,
    ThisYear,
    Last12,
    All,
}

impl Period {
    pub const ALL: [Period; 6] =
        [Period::ThisMonth, Period::LastMonth, Period::Last3, Period::ThisYear, Period::Last12, Period::All];
    pub fn label(self) -> &'static str {
        match self {
            Period::ThisMonth => tr("This month"),
            Period::LastMonth => tr("Last month"),
            Period::Last3 => tr("Last 3 months"),
            Period::ThisYear => tr("This year"),
            Period::Last12 => tr("Last 12 months"),
            Period::All => tr("All"),
        }
    }
    pub fn range(self) -> (Option<NaiveDate>, Option<NaiveDate>) {
        let t = db::today();
        match self {
            Period::ThisMonth => (Some(db::first_of_month(t)), None),
            Period::LastMonth => {
                let (a, b) = db::month_range(ops::months_back(t, 1));
                (Some(a), Some(b))
            }
            Period::Last3 => (Some(ops::months_back(t, 2)), None),
            Period::ThisYear => (Some(ops::year_start(t)), None),
            Period::Last12 => (Some(ops::months_back(t, 11)), None),
            Period::All => (None, None),
        }
    }
}

#[derive(Clone, Default)]
pub struct TxForm {
    pub id: Option<i64>,
    pub account: Option<i64>,
    pub date: String,
    pub amount: String,
    pub expense: bool,
    pub payee: String,
    pub memo: String,
    pub category: Option<i64>,
    pub cleared: bool,
    pub is_transfer: bool,
    /// Whether the category can be chosen.
    pub has_category: bool,
    /// The user chose a category, maybe the one it had.
    pub category_chosen: bool,
    pub is_split: bool,
    pub tags: String,
    /// The tags it had.
    pub old_tags: String,
    /// What the bank wrote, and the rules that set something of it.
    pub detail: String,
    pub error: String,
}

#[derive(Clone, Default)]
pub struct TransferForm {
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub date: String,
    pub amount: String,
    pub memo: String,
    pub error: String,
}

/// A transaction that becomes a transfer.
pub struct TurnForm {
    pub tx: Transaction,
    pub account: Option<i64>,
    /// What may be the other side in that account.
    pub candidates: Vec<Transaction>,
    /// `Auto` is not chosen yet.
    pub other: OtherSide,
    pub error: String,
}

impl TurnForm {
    pub fn reload(&mut self, db: &Db) {
        self.error.clear();
        self.candidates.clear();
        self.other = OtherSide::New;
        let Some(account) = self.account else { return };
        match db.transfer_candidates(self.tx.id, account) {
            Ok(c) => {
                self.other = if c.is_empty() { OtherSide::New } else { OtherSide::Auto };
                self.candidates = c;
            }
            Err(e) => self.error = format!("{e:#}"),
        }
    }
}

#[derive(Clone)]
pub struct AccountForm {
    pub id: Option<i64>,
    pub name: String,
    pub kind: AccountKind,
    pub opening: String,
    pub iban: String,
    pub currency: String,
    /// The currency it has now.
    pub old_currency: String,
    pub archived: bool,
    pub error: String,
}

#[derive(Clone, Default)]
pub struct SchedForm {
    pub account: Option<i64>,
    pub to_account: Option<i64>,
    pub transfer: bool,
    pub amount: String,
    pub expense: bool,
    pub payee: String,
    pub memo: String,
    pub category: Option<i64>,
    pub date: String,
    pub every: String,
    pub unit: String,
    /// The date of the transaction it is made from, and the due date the form wrote: while the due
    /// date is still that one, it follows the repetition when that changes.
    pub from: Option<(NaiveDate, String)>,
    pub error: String,
}

#[derive(Default)]
pub struct ImportState {
    pub path_text: String,
    pub table: Option<Table>,
    pub mapping: Mapping,
    pub invert: bool,
    /// `None`: the separator is found in the file.
    pub decimal_comma: Option<bool>,
    pub dates: DateOrder,
    /// The id of the saved columns in use.
    pub saved: Option<i64>,
    /// The id of saved columns that cannot be read: forgotten first.
    pub broken: Option<i64>,
    /// The account whose settings were loaded, and those settings.
    pub loaded_for: Option<Option<i64>>,
    pub loaded: Settings,
    /// Whether the user chose the sign.
    pub sign_by_hand: bool,
    pub account: Option<i64>,
    pub new_account: String,
    pub rows: Vec<ParsedRow>,
    pub skipped: usize,
    pub problems: Vec<Problem>,
    /// Whether most rows were read.
    pub convincing: bool,
    /// What the rules give the rows that are shown, with the rules.
    pub ruled: HashMap<usize, (String, String)>,
    pub broken_rules: String,
    /// What the user had said of the rows when the rules were looked at.
    pub ruled_for: Option<BTreeMap<usize, Choice>>,
    /// Rows that may be in the books already, and what the user said of each, for every account.
    pub likely: Vec<LikelyDuplicate>,
    pub decided: BTreeMap<usize, Choice>,
    pub said: HashMap<Option<i64>, BTreeMap<usize, Choice>>,
    pub deletions: Vec<Deletion>,
    /// The account that the rows were compared with.
    pub compared: Option<Option<i64>>,
    /// The balance in the books on the day of the one that the statement states.
    pub books: Option<Cents>,
    pub error: String,
    pub result: Option<ImportResult>,
    /// The user wants a card statement that does not add up.
    pub allow_mismatch: bool,
    pub cards: CardPayments,
    pub loading: Option<mpsc::Receiver<Result<Table>>>,
    /// A join that would change reconciled transactions, to be asked.
    pub pending: Option<Box<(cashcraft_engine::reconcile::Reconciled, crate::reconcile::Unlock)>>,
}

/// Card statements, and what may pay those that are not paid.
#[derive(Default)]
pub struct CardPayments {
    pub statements: Vec<CardStatement>,
    pub proposals: Vec<PaymentProposal>,
    /// The line of the card to join for a candidate, instead of adding its side.
    pub join: HashMap<i64, i64>,
    /// The statement whose link is about to be undone.
    pub unlinking: Option<i64>,
    pub error: String,
    /// A link or unlink that would change reconciled transactions, to be asked.
    pub pending: Option<Box<(cashcraft_engine::reconcile::Reconciled, crate::reconcile::Unlock)>>,
}

impl CardPayments {
    pub fn reload(&mut self, db: &Db) {
        self.error.clear();
        self.join.clear();
        self.unlinking = None;
        match db.card_statements().and_then(|s| Ok((s, db.propose_card_payments()?))) {
            Ok((statements, proposals)) => (self.statements, self.proposals) = (statements, proposals),
            Err(e) => self.error = format!("{e:#}"),
        }
    }
}

impl ImportState {
    /// Reads the file beside the window, which goes on drawing.
    pub fn load(&mut self) {
        self.error.clear();
        self.result = None;
        self.table = None;
        self.rows.clear();
        self.allow_mismatch = false;
        self.reparse();
        let path = PathBuf::from(self.path_text.trim());
        let language = lang();
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            // Nobody may be waiting any more.
            let _ = send.send(i18n::with_lang(language, || importer::load(&path)));
        });
        self.loading = Some(receive);
    }
    /// Takes what was read, once it is there; whether it has just come.
    pub fn poll(&mut self) -> bool {
        let Some(receive) = &self.loading else { return false };
        let read = match receive.try_recv() {
            Err(mpsc::TryRecvError::Empty) => return false,
            Ok(read) => read,
            Err(e) => Err(e.into()),
        };
        self.loading = None;
        match read {
            Ok(t) => {
                self.use_settings(Settings::detected(&t), None);
                self.loaded = self.settings();
                self.loaded_for = None;
                self.sign_by_hand = false;
                self.broken = None;
                self.table = Some(t);
                self.reparse();
            }
            Err(e) => self.error = format!("{e:#}"),
        }
        true
    }
    pub fn can_import(&self) -> bool {
        let statement = self.table.as_ref().and_then(|t| t.statement.as_ref());
        statement.is_none_or(|s| s.is_sound() || (s.is_identified() && self.allow_mismatch))
            && !self.rows.is_empty()
            && self.broken.is_none()
            && (self.sign_by_hand || !self.signs_contradict())
            && self.likely.iter().all(|l| self.decided.contains_key(&l.index))
            && (self.account.is_some() || !self.new_account.trim().is_empty())
    }
    pub fn reparse(&mut self) {
        if let Some(t) = &self.table {
            let read = importer::read_rows(t, &self.settings());
            self.convincing = read.is_convincing();
            self.rows = read.rows;
            self.skipped = read.skipped;
            self.problems = read.problems;
        }
        self.likely.clear();
        self.deletions.clear();
        self.decided.clear();
        self.said.clear();
        self.compared = None;
        self.books = None;
        self.ruled.clear();
        self.ruled_for = None;
    }
    pub fn settings(&self) -> Settings {
        Settings {
            mapping: self.mapping.clone(),
            invert: self.invert,
            decimal_comma: self.decimal_comma,
            dates: self.dates,
        }
    }
    pub fn use_settings(&mut self, s: Settings, saved: Option<i64>) {
        (self.mapping, self.invert, self.decimal_comma, self.dates) = (s.mapping, s.invert, s.decimal_comma, s.dates);
        self.saved = saved;
    }
    /// The account the rows go into, if it is there already.
    pub fn target(&self) -> Option<i64> {
        self.account.filter(|_| self.new_account.trim().is_empty())
    }
    pub fn signs_contradict(&self) -> bool {
        let statement = self.table.as_ref().and_then(|t| t.ofx.as_ref()).and_then(|o| o.statement());
        statement.is_some_and(|s| s.signs_contradict())
    }
    /// Loads what was remembered for the account, when it changed. What the user changed by hand stays, and a
    /// sign that was remembered for another account does not.
    pub fn load_saved(&mut self, db: &Db) -> Result<()> {
        let target = self.target();
        let Some(t) = &self.table else { return Ok(()) };
        if self.loaded_for == Some(target) {
            return Ok(());
        }
        self.loaded_for = Some(target);
        let before = self.settings();
        let layout = db.layout_of(t, target)?;
        let mut settings = Settings::detected(t);
        self.broken = None;
        let mut saved = None;
        if let Some(l) = layout {
            match l.settings() {
                Ok(s) => {
                    settings = s;
                    saved = Some(l.id);
                }
                Err(_) => self.broken = Some(l.id),
            }
        }
        let remembered = settings.clone();
        let edited = before != self.loaded;
        if edited {
            let invert = if self.sign_by_hand { before.invert } else { settings.invert };
            settings = Settings { invert, ..before.clone() };
        }
        let changed = settings != before;
        self.use_settings(settings, saved);
        self.loaded = remembered;
        if changed {
            self.reparse();
        }
        Ok(())
    }
    /// Looks again for the rows that may be in the books, when the account or the rows changed.
    pub fn compare(&mut self, db: &Db) -> Result<()> {
        let target = self.target();
        if self.table.is_none() || self.compared == Some(target) {
            return Ok(());
        }
        // What was said of another account says nothing of this one, and is there when the user comes back.
        if let Some(old) = self.compared {
            let said = std::mem::take(&mut self.decided);
            self.said.insert(old, said);
            self.decided = self.said.remove(&target).unwrap_or_default();
        }
        self.compared = Some(target);
        self.likely.clear();
        self.deletions.clear();
        self.likely = db.likely_duplicates(target, &self.rows)?;
        self.deletions = db.deletions(target, &self.rows)?;
        self.ruled_for = None;
        let likely: HashMap<usize, &LikelyDuplicate> = self.likely.iter().map(|l| (l.index, l)).collect();
        self.decided.retain(|i, c| {
            likely.get(i).is_some_and(|l| match c {
                Choice::Skip(Some(id)) => l.existing.iter().any(|t| t.id == *id),
                _ => true,
            })
        });
        self.read_balance(db)
    }
    /// What the rules give the rows that are shown and would be added, when what is known of them changed.
    pub fn look_at_rules(&mut self, db: &Db) -> Result<()> {
        if self.table.is_none() || self.ruled_for.as_ref() == Some(&self.decided) {
            return Ok(());
        }
        self.ruled_for = Some(self.decided.clone());
        self.ruled.clear();
        let target = self.target();
        let set = db.ruleset()?;
        self.broken_rules = rules::broken_message(&set.broken);
        let decision = Decision { each: self.decided.clone(), ..Default::default() };
        let new = importer::rows_to_add(db, target, &self.rows, &decision)?;
        for (i, r) in self.rows.iter().enumerate().rev().take(PREVIEWED).filter(|(i, _)| new.contains(i)) {
            let o = set.evaluate(&rules::Subject::new(&r.payee, &r.memo, r.amount, target));
            let category = o.category.and_then(|(id, _)| set.rule(id)).map(|r| r.category.clone());
            let payee = o.payee.as_ref().map(|p| format!("\"{}\"", p.1));
            let mut what: Vec<String> = category.into_iter().chain(payee).collect();
            what.extend(o.tags.iter().map(|t| format!("#{}", t.1)));
            if !what.is_empty() {
                self.ruled.insert(i, (what.join(", "), set.explain(&o)));
            }
        }
        Ok(())
    }
    /// The account the rows go into: a new one when it has a name, made from what the file says of it. Runs inside
    /// the import's `atomically`.
    pub fn destination(&self, db: &Db, accounts: &[Account], card: bool) -> Result<i64> {
        let list = self.table.as_ref().and_then(|t| t.list.as_ref());
        let iban = list.map_or("", |l| l.iban.as_str());
        if self.new_account.trim().is_empty() {
            let id = self.account.ok_or_else(|| anyhow::anyhow!(tr("choose an account")))?;
            // An account without an IBAN takes that of the list, to be found by it the next time.
            if let Some(a) =
                accounts.iter().find(|a| a.id == id && a.iban.trim().is_empty()).filter(|_| !iban.is_empty())
            {
                db.set_account_iban(a.id, iban)?;
            }
            return Ok(id);
        }
        let ofx = self.table.as_ref().and_then(|t| t.ofx.as_ref()).and_then(|o| o.statement());
        let kind = match ofx.map(|s| s.kind) {
            Some(cashcraft_engine::ofx::Kind::CreditCard) => AccountKind::CreditCard,
            Some(cashcraft_engine::ofx::Kind::Savings) => AccountKind::Savings,
            Some(cashcraft_engine::ofx::Kind::Bank) => AccountKind::Bank,
            None if card => AccountKind::CreditCard,
            None => AccountKind::Bank,
        };
        let currency = ofx.map_or("EUR", |s| s.currency.as_str());
        // A list of movements says where the account starts.
        let opening = list.and_then(|l| l.opening).unwrap_or(0);
        db.add_account(self.new_account.trim(), kind, currency, opening, iban)
    }
    pub fn read_balance(&mut self, db: &Db) -> Result<()> {
        let stated = self.table.as_ref().and_then(|t| t.stated_balance());
        self.books = match (self.target(), stated) {
            (Some(a), Some((_, Some(d)))) => Some(db.balance_on(a, d)?),
            _ => None,
        };
        Ok(())
    }
}

pub enum Dialog {
    Tx(TxForm),
    Transfer(TransferForm),
    TurnIntoTransfer(Box<TurnForm>),
    /// A side of a transfer, and whether the other one stays.
    SeparateTransfer(Box<Transaction>, bool),
    Account(AccountForm),
    Import(Box<ImportState>),
    Scheduled(SchedForm),
    Entry(EntryForm),
    LedgerAccount(LedgerAccountForm),
    RenameLedgerAccount(i64, String, String),
    ConfirmDeleteLedgerAccount(i64, String),
    /// An entry with its postings.
    EntryDetail(i64),
    /// The entry, and how many lines it has.
    ConfirmDeleteEntry(i64, usize),
    /// With how many rules go.
    ConfirmDeleteAccount(i64, String, usize),
    ConfirmClearDemo,
    Rule(Box<RuleForm>),
    RunRules(RunForm),
    ConfirmUndoRules(LastRun),
    /// A category with rules, its name and how many go.
    ConfirmDeleteCategory(i64, String, usize),
    /// A change to reconciled transactions, and how to make it once the user says yes.
    /// The dialog to go back to afterwards, if any.
    ConfirmUnreconcile(Box<(cashcraft_engine::reconcile::Reconciled, crate::reconcile::Unlock, Option<Dialog>)>),
    ConfirmReconcile(crate::reconcile::Step),
}

pub struct Dash {
    pub month_income: Cents,
    pub month_expense: Cents,
    pub monthly: Vec<db::MonthTotal>,
    pub top_expenses: Vec<(String, Cents)>,
    pub recent: Vec<Transaction>,
    pub upcoming: Vec<Scheduled>,
    pub budgets: Vec<db::BudgetLine>,
    pub uncategorized: usize,
}

pub struct ReportData {
    pub totals: Vec<db::CategoryTotal>,
    pub monthly: Vec<db::MonthTotal>,
    pub net_worth: Vec<(String, Cents)>,
}

pub struct App {
    pub db: Db,
    pub view: View,
    pub accounts: Vec<Account>,
    pub categories: Vec<Category>,
    pub txs: Vec<Transaction>,
    /// Income and expense of `txs`.
    pub tx_totals: (Cents, Cents),
    pub net_worth: Cents,
    pub period: Period,
    pub search: String,
    pub cat_filter: Option<i64>,
    pub only_uncategorized: bool,
    pub selected: Option<i64>,
    pub dialog: Option<Dialog>,
    pub toast: Option<(String, f64)>,
    pub report_period: Period,
    pub budget_month: NaiveDate,
    pub budget_edit: Option<(i64, String)>,
    pub new_budget: (Option<i64>, String),
    /// The rules, and the run that can be undone.
    pub rules: Option<(Vec<Rule>, Option<LastRun>)>,
    /// The screen of the reconciliation of an account.
    pub reconcile: Option<crate::reconcile::ReconcileState>,
    /// The budget made of envelopes.
    pub envelopes: crate::envelopes::EnvelopeState,
    pub new_cat: (String, bool, Option<i64>),
    pub rename_cat: Option<(i64, String)>,
    pub dash: Option<Dash>,
    pub report: Option<ReportData>,
    pub ledger: Option<LedgerData>,
    /// The trial balance up to this date, as it is typed.
    pub ledger_date: String,
    /// The account of the chart whose entries are shown, and the page.
    pub journal: Option<(i64, usize)>,
    /// The language chosen in the window, `None` for the system's.
    pub language: Option<Lang>,
    pub theme: Theme,
    /// Where the data is.
    pub file: DataFile,
    pub(crate) data_version: i64,
    last_check: f64,
    pub demo: bool,
    screenshot: Option<PathBuf>,
    frame_count: u32,
    shot_requested: bool,
    shot_done: Arc<AtomicBool>,
    /// New versions: `None` until the first frame of a window on a file, and in a picture.
    pub updates: Option<crate::updates::Updates>,
}

// ------------------------------------------------------------------ avvio

/// Opens the window on a data file, which must be there or is made.
pub fn run(home: Home, (path, must_exist): (PathBuf, bool), asked: Use, screenshot: Option<PathBuf>) -> Result<()> {
    // The language of a new file and of the errors while opening; then the one the file asks for.
    i18n::set_lang(Lang::at_start(None));
    // A picture is taken of what is there at once; a window opens first, and the file beside it.
    let opened = if screenshot.is_none() {
        None
    } else if !path.exists() {
        // Sample data in memory, no file created.
        let d = Db::open_in_memory()?;
        demo::seed(&d)?;
        Some(Opened { db: d, is_new: false, shared: None, notices: Vec::new() })
    } else {
        Some(datafile::open(&home, &path, asked, must_exist)?)
    };
    if let Some(o) = &opened {
        i18n::set_lang(Lang::at_start(o.db.meta(i18n::META_KEY).as_deref()));
    }
    let shooting = screenshot.is_some();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("CashCraft")
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([900.0, 560.0])
        .with_icon({
            let img = cashcraft_icon::render(256);
            egui::IconData { rgba: img.into_raw(), width: 256, height: 256 }
        });
    if shooting {
        viewport = viewport.with_active(false);
        #[cfg(target_os = "windows")]
        {
            viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "CashCraft",
        options,
        Box::new(move |cc| {
            setup_style(&cc.egui_ctx);
            let mut start = Start::new(home, path, must_exist, asked);
            match opened {
                None => start.open(|_, _| Ok(())),
                Some(o) => {
                    let (home, path) = (start.home.clone(), start.path.clone());
                    let mut app = App::of(o, home, path, asked, &cc.egui_ctx);
                    app.screenshot = screenshot;
                    start.app = Some(Box::new(app));
                }
            }
            Ok(Box::new(start))
        }),
    )
    .map_err(|e| anyhow::anyhow!(trf("the window cannot be opened: {error}", &[("error", &e)])))
}

fn setup_style(ctx: &egui::Context) {
    for (theme, p) in [(egui::Theme::Light, &LIGHT), (egui::Theme::Dark, &DARK)] {
        ctx.set_visuals_of(theme, visuals(theme, p));
    }
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 4.0);
        style.spacing.interact_size.y = 26.0;
        use egui::TextStyle::*;
        style.text_styles.insert(Body, FontId::proportional(14.0));
        style.text_styles.insert(Button, FontId::proportional(14.0));
        style.text_styles.insert(Heading, FontId::proportional(22.0));
        style.text_styles.insert(Small, FontId::proportional(11.5));
    });
}

/// egui's look for one theme, from its palette.
pub fn visuals(theme: egui::Theme, p: &Palette) -> egui::Visuals {
    let mut v = if theme == egui::Theme::Dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.extreme_bg_color = p.field;
    v.faint_bg_color = p.zebra;
    v.selection.bg_fill = p.accent_soft;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.hyperlink_color = p.accent;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(6);
    }
    v.widgets.inactive.bg_fill = p.track;
    v.widgets.inactive.weak_bg_fill = p.track;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.hovered.weak_bg_fill = p.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent);
    v
}

fn save_png(path: &std::path::Path, img: &egui::ColorImage) -> Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    let [w, h] = img.size;
    let mut bytes = Vec::with_capacity(w * h * 4);
    for p in &img.pixels {
        bytes.extend_from_slice(&[p.r(), p.g(), p.b(), 255]);
    }
    let buf = image::RgbaImage::from_raw(w as u32, h as u32, bytes)
        .ok_or_else(|| anyhow::anyhow!(tr("the picture is not valid")))?;
    buf.save(path)?;
    Ok(())
}

// ------------------------------------------------------------------ App

impl App {
    pub fn new(db: Db, screenshot: Option<PathBuf>) -> Self {
        let demo = db.meta("demo").as_deref() == Some("1");
        let language = Lang::from_setting(db.meta(i18n::META_KEY).as_deref());
        // For screenshots, CASHCRAFT_THEME=light|dark overrides the file's choice.
        let theme = Theme::from_setting(
            std::env::var("CASHCRAFT_THEME").ok().as_deref().or(db.meta(Theme::META_KEY).as_deref()),
        );
        let mut app = App {
            db,
            view: View::Dashboard,
            accounts: vec![],
            categories: vec![],
            txs: vec![],
            tx_totals: (0, 0),
            net_worth: 0,
            period: Period::Last3,
            search: String::new(),
            cat_filter: None,
            only_uncategorized: false,
            selected: None,
            dialog: None,
            toast: None,
            report_period: Period::ThisYear,
            budget_month: db::first_of_month(db::today()),
            budget_edit: None,
            new_budget: (None, String::new()),
            rules: None,
            reconcile: None,
            envelopes: Default::default(),
            new_cat: (String::new(), false, None),
            rename_cat: None,
            dash: None,
            report: None,
            ledger: None,
            ledger_date: String::new(),
            journal: None,
            language,
            theme,
            file: DataFile::default(),
            data_version: 0,
            last_check: 0.0,
            demo,
            screenshot,
            frame_count: 0,
            shot_requested: false,
            shot_done: Arc::new(AtomicBool::new(false)),
            updates: None,
        };
        app.data_version = app.db.data_version();
        app.reload();
        // Optional first screen, for screenshots: anything else ("overview") is the overview.
        if let Ok(v) = std::env::var("CASHCRAFT_START_VIEW") {
            let first = app.accounts.first().map(|a| a.id);
            app.view = match v.as_str() {
                "register" | "movimenti" | "selected" | "transfer" => View::Register(first),
                "all" | "tutti" => View::Register(None),
                "reports" | "report" => View::Reports,
                "budget" | "envelopes" | "buste" => View::Budget,
                "categories" | "categorie" => View::Categories,
                "rules" | "regole" => View::Rules,
                "scheduled" | "pianificati" => View::Scheduled,
                "ledger" | "entry" | "journal" => View::Ledger,
                "reconcile" | "riconcilia" => first.map_or(View::Dashboard, View::Reconcile),
                _ => View::Dashboard,
            };
            app.reload_txs();
            if v == "import" || v == "importa" {
                // Optional file to read, for screenshots.
                app.open_import(std::env::var_os("CASHCRAFT_START_FILE").map(PathBuf::from));
            }
            if v == "selected" || v == "transfer" {
                let t = app.txs.iter().find(|t| has_category(t) && t.amount < 0).cloned();
                app.selected = t.as_ref().map(|t| t.id);
                if let Some(t) = t.filter(|_| v == "transfer") {
                    app.open_turn(&t);
                    if let Some(Dialog::TurnIntoTransfer(f)) = &mut app.dialog {
                        f.account = app.accounts.iter().find(|a| a.id != t.account_id).map(|a| a.id);
                        f.reload(&app.db);
                    }
                }
            }
            if v == "entry" {
                app.open_entry();
            }
            app.sample_envelopes(&v);
            if v == "journal" {
                let all = or_report(app.db.ledger_accounts(), &mut app.toast);
                app.journal = all.first().map(|l| (l.id, 0));
            }
        }
        app
    }

    /// For the screenshot of the envelopes: the sample budgets in memory become envelopes, as a file of the user's
    /// would. A real file keeps its mode.
    pub fn sample_envelopes(&mut self, start_view: &str) {
        if (start_view == "envelopes" || start_view == "buste") && self.db.path.is_none() {
            let r = self.db.set_budget_mode(cashcraft_engine::envelopes::Mode::Envelopes);
            or_report(r.map(|_| ()), &mut self.toast);
        }
    }

    /// Changes the language of the window, and of the command line on this data file.
    /// Whether this window only takes a picture of itself (`--screenshot`).
    pub(crate) fn screenshot_mode(&self) -> bool {
        self.screenshot.is_some()
    }

    pub fn choose_language(&mut self, ui: &Ui, l: Option<Lang>) {
        let r = self.db.set_meta(i18n::META_KEY, l.map_or("system", Lang::code));
        if r.is_ok() {
            self.language = l;
            i18n::set_lang(l.unwrap_or_else(Lang::from_system));
            // An amount being typed could mean something else in the new language.
            self.budget_edit = None;
            self.new_budget.1.clear();
        }
        self.act(ui, r.map(|_| String::new()));
    }

    /// Changes the colours of the window.
    pub fn choose_theme(&mut self, ui: &Ui, t: Theme) {
        let r = self.db.set_meta(Theme::META_KEY, t.code());
        if r.is_ok() {
            self.theme = t;
            ui.ctx().set_theme(t.preference());
        }
        self.act(ui, r.map(|_| String::new()));
    }

    pub fn reload(&mut self) {
        self.accounts = or_report(self.db.accounts(true), &mut self.toast);
        self.net_worth = or_report(self.db.net_worth(None), &mut self.toast);
        self.categories = or_report(self.db.categories(), &mut self.toast);
        self.demo = self.db.meta("demo").as_deref() == Some("1");
        self.reload_txs();
        self.dash = None;
        self.report = None;
        self.ledger = None;
        self.rules = None;
        if let Some(r) = self.reconcile.as_mut() {
            r.stale = true;
        }
        self.envelopes.stale = true;
    }

    pub fn reload_txs(&mut self) {
        let (from, to) = self.period.range();
        let account_id = match self.view {
            View::Register(a) => a,
            _ => None,
        };
        let f = TxFilter {
            account_id,
            category_id: self.cat_filter,
            uncategorized: self.only_uncategorized,
            from,
            to,
            search: self.search.clone(),
            tag: None,
            limit: None,
        };
        self.txs = or_report(self.db.transactions(&f), &mut self.toast);
        self.tx_totals = or_report(self.db.register_totals(&self.txs, self.cat_filter), &mut self.toast);
    }

    pub fn notify(&mut self, ui: &Ui, msg: impl Into<String>) {
        self.toast = Some((msg.into(), ui.input(|i| i.time) + 4.0));
    }

    /// Mostra l'esito di un'azione e ricarica i dati.
    pub fn act(&mut self, ui: &Ui, r: Result<String>) {
        match r {
            Ok(m) => {
                if !m.is_empty() {
                    self.notify(ui, m);
                }
            }
            Err(e) => self.notify(ui, error_text(&e)),
        }
        self.reload();
    }

    pub fn go(&mut self, v: View) {
        if self.view != v {
            self.view = v;
            self.envelopes.leave();
            self.selected = None;
            if matches!(v, View::Register(_)) {
                self.reload_txs();
            }
        }
    }

    pub fn account_name(&self, id: i64) -> String {
        self.accounts.iter().find(|a| a.id == id).map(|a| a.name.clone()).unwrap_or_default()
    }

    pub fn default_account(&self) -> Option<i64> {
        if let View::Register(Some(a)) = self.view {
            return Some(a);
        }
        self.accounts.iter().find(|a| !a.archived).map(|a| a.id)
    }

    pub fn open_new_tx(&mut self) {
        self.dialog = Some(Dialog::Tx(TxForm {
            account: self.default_account(),
            date: fmt_date(db::today()),
            expense: true,
            has_category: true,
            ..Default::default()
        }));
    }

    pub fn open_edit_tx(&mut self, t: &Transaction) {
        self.dialog = Some(Dialog::Tx(TxForm {
            id: Some(t.id),
            account: Some(t.account_id),
            date: fmt_date(t.date),
            amount: cashcraft_engine::money::fmt_plain(t.amount.abs()),
            expense: t.amount < 0,
            payee: t.payee.clone(),
            memo: t.memo.clone(),
            category: t.category_id,
            cleared: t.cleared,
            is_transfer: t.transfer_peer.is_some(),
            has_category: has_category(t),
            category_chosen: false,
            is_split: t.kind == TxKind::Split,
            tags: t.tags.join(", "),
            old_tags: t.tags.join(", "),
            detail: self.detail_of(t),
            error: String::new(),
        }));
    }

    fn detail_of(&self, t: &Transaction) -> String {
        let mut out = Vec::new();
        let texts = [
            (tr("Payee written by the bank"), &t.original_payee),
            (tr("Notes written by the bank"), &t.original_memo),
            (tr("Payee typed before the rules"), &t.typed_payee),
            (tr("Notes typed before the rules"), &t.typed_memo),
        ];
        for (label, text) in texts {
            out.extend(text.as_ref().filter(|x| !x.is_empty()).map(|x| format!("{label}: {x}")));
        }
        if t.rules.memo.is_some() {
            out.push(tr("The notes are the payee that a rule replaced").into());
        }
        out.push(self.rules_of(t).unwrap_or_else(|e| error_text(&e)));
        out.join("\n").trim().to_string()
    }

    /// Asks for the other account of a transaction, or whether to separate a transfer.
    /// The form of a scheduled transaction, filled in from `t`.
    pub fn open_recurring(&mut self, t: &Transaction) -> Result<()> {
        let s = self.db.scheduled_from(t.id)?;
        // The date as the file has it now, not as the register read it.
        let date = self.db.transaction(t.id)?.date;
        self.dialog = Some(Dialog::Scheduled(SchedForm {
            account: Some(s.account_id),
            to_account: s.to_account_id,
            transfer: s.to_account_id.is_some(),
            amount: cashcraft_engine::money::fmt_plain(s.amount.saturating_abs()),
            expense: s.amount < 0,
            payee: s.payee,
            memo: s.memo,
            category: s.category_id,
            date: fmt_date(s.next_date),
            every: s.every.to_string(),
            unit: s.unit,
            from: Some((date, fmt_date(s.next_date))),
            error: String::new(),
        }));
        Ok(())
    }

    pub fn open_turn(&mut self, t: &Transaction) {
        self.dialog = Some(if t.kind == TxKind::Transfer {
            // Both added by hand: none is the one that should go.
            let added = |id| self.db.transaction(id).is_ok_and(|t| t.added);
            let spare = !t.added && t.transfer_peer.is_some_and(added);
            Dialog::SeparateTransfer(Box::new(t.clone()), !spare)
        } else {
            Dialog::TurnIntoTransfer(Box::new(TurnForm {
                tx: t.clone(),
                account: None,
                candidates: vec![],
                other: OtherSide::New,
                error: String::new(),
            }))
        });
    }

    pub fn open_import(&mut self, path: Option<PathBuf>) {
        let mut st = ImportState { account: self.default_account(), ..Default::default() };
        st.cards.reload(&self.db);
        if let Some(p) = path {
            st.path_text = p.display().to_string();
            st.load();
        }
        self.dialog = Some(Dialog::Import(Box::new(st)));
    }

    /// A card statement goes to a credit card, a new one if there is none.
    pub(crate) fn card_account(&self, st: &mut ImportState) {
        let Some(s) = st.table.as_ref().and_then(|t| t.statement.as_ref()) else { return };
        let cards: Vec<&Account> =
            self.accounts.iter().filter(|a| a.kind == AccountKind::CreditCard && !a.archived).collect();
        if cards.iter().any(|a| Some(a.id) == st.account) {
            return;
        }
        st.account = match cards.as_slice() {
            [one] => Some(one.id),
            _ => None,
        };
        if cards.is_empty() && st.new_account.is_empty() {
            st.new_account = match s.cards.first() {
                Some(c) => trf("Card {digits}", &[("digits", c)]),
                None => tr("Credit card").into(),
            };
        }
    }

    /// A list of movements goes to the account with its IBAN; when none has it, to a new account named after the
    /// bank, which starts with the list's opening balance.
    pub(crate) fn list_account(&self, st: &mut ImportState) {
        let Some(l) = st.table.as_ref().and_then(|t| t.list.as_ref()) else { return };
        if let Some(a) = l.account(&self.accounts) {
            st.account = Some(a.id);
            st.new_account.clear();
            return;
        }
        if let Some(name) = l.account_name().filter(|_| !l.iban.is_empty() && st.new_account.is_empty()) {
            st.new_account = name;
        }
    }

    pub fn ensure_dash(&mut self) {
        if self.dash.is_some() {
            return;
        }
        let t = db::today();
        let (from, to) = db::month_range(t);
        let totals = or_report(self.db.category_totals(from, to), &mut self.toast);
        let (month_income, month_expense) = or_report(db::CategoryTotal::income_and_expense(&totals), &mut self.toast);
        let mut top: Vec<(String, Cents)> =
            totals.iter().filter(|c| c.total < 0).map(|c| (c.category.clone(), -c.total)).collect();
        top.sort_by_key(|x| -x.1);
        top.truncate(6);
        let uncategorized =
            self.db.transactions(&TxFilter { uncategorized: true, ..Default::default() }).map(|v| v.len()).unwrap_or(0);
        self.dash = Some(Dash {
            month_income,
            month_expense,
            monthly: or_report(self.db.monthly_totals(6, t), &mut self.toast),
            top_expenses: top,
            recent: or_report(
                self.db.transactions(&TxFilter { limit: Some(8), ..Default::default() }),
                &mut self.toast,
            ),
            upcoming: or_report(self.db.scheduled(), &mut self.toast).into_iter().take(5).collect(),
            budgets: or_report(self.db.budget_status(t), &mut self.toast),
            uncategorized,
        });
    }

    pub fn ensure_report(&mut self) {
        if self.report.is_some() {
            return;
        }
        let t = db::today();
        let (from, to) = self.report_period.range();
        let from = from.unwrap_or(NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or_default());
        let to = to.unwrap_or(t);
        let months = match self.report_period {
            Period::ThisMonth | Period::LastMonth | Period::Last3 => 6,
            Period::ThisYear => t.month().max(3),
            _ => 12,
        };
        self.report = Some(ReportData {
            totals: or_report(self.db.category_totals(from, to), &mut self.toast),
            monthly: or_report(self.db.monthly_totals(months, to), &mut self.toast),
            net_worth: or_report(self.db.net_worth_history(12, t), &mut self.toast),
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        set_dark(ui.visuals().dark_mode);

        let steady = self.file_before(ui);
        if !steady {
            ui.disable();
        }

        // Foto di controllo (--screenshot): ~120 fotogrammi, poi salva ed esce.
        if let Some(path) = self.screenshot.clone() {
            self.frame_count += 1;
            ctx.request_repaint();
            if self.frame_count >= 120 && !self.shot_requested {
                self.shot_requested = true;
                let done = self.shot_done.clone();
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                    if let Err(e) = save_png(&path, &img) {
                        eprintln!(
                            "{}",
                            trf("The picture cannot be saved: {error}", &[("error", &format_args!("{e:#}"))])
                        );
                    }
                    done.store(true, Ordering::SeqCst);
                })));
            }
            if self.shot_done.load(Ordering::SeqCst) || self.frame_count > 2000 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        // Modifiche fatte da CLI o MCP mentre la finestra è aperta.
        let now = ui.input(|i| i.time);
        if now - self.last_check > 1.0 {
            self.last_check = now;
            let v = self.db.data_version();
            if v != self.data_version {
                self.data_version = v;
                self.reload();
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(1000));

        // CSV trascinati sulla finestra.
        let dropped: Vec<PathBuf> = ui.input(|i| {
            i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect()
        });
        if let Some(p) = dropped.into_iter().next().filter(|_| steady && self.file.window.is_none()) {
            self.open_import(Some(p));
        }

        // Scorciatoie.
        if steady && self.dialog.is_none() && self.file.window.is_none() {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::N)) {
                self.open_new_tx();
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::J)) {
                self.open_entry();
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::I)) {
                self.open_import(None);
            }
            if let View::Register(Some(a)) = self.view
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::R))
            {
                self.go(View::Reconcile(a));
            }
            if matches!(self.view, View::Register(_))
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::T))
                && let Some(t) = self.txs.iter().find(|t| Some(t.id) == self.selected).cloned()
            {
                self.open_turn(&t);
            }
        }

        self.sidebar(ui);
        self.toolbar(ui);
        self.status_bar(ui);
        self.updates_tick(ui);
        egui::CentralPanel::default().frame(Frame::new().fill(pal().bg).inner_margin(Margin::symmetric(22, 16))).show(
            ui,
            |ui| match self.view {
                View::Dashboard => self.dashboard(ui),
                View::Register(a) => self.register(ui, a),
                View::Reports => self.reports(ui),
                View::Budget => self.budget(ui),
                View::Categories => self.categories_view(ui),
                View::Rules => self.rules_view(ui),
                View::Scheduled => self.scheduled_view(ui),
                View::Ledger => self.ledger_view(ui),
                View::Reconcile(a) => self.reconcile_view(ui, a),
            },
        );
        self.dialogs(&ctx);
        self.file_window(&ctx);
        self.draw_toast(&ctx);
        self.file_after(ui);
    }

    fn on_exit(&mut self) {
        self.updates_on_exit();
        self.file_close();
    }
}

// ------------------------------------------------------------------ widget

pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(pal().card)
        .stroke(Stroke::new(1.0, pal().border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| ui.vertical(add).inner)
        .inner
}

pub fn section_title(ui: &mut Ui, s: &str) {
    ui.label(RichText::new(s).size(15.0).strong().color(pal().text));
}

pub fn small_caps(ui: &mut Ui, s: &str) {
    ui.label(RichText::new(s.to_uppercase()).size(11.0).color(pal().muted).strong());
}

pub fn money(c: Cents) -> RichText {
    RichText::new(fmt_eur(c)).color(amount_color(c))
}

pub fn primary_button(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
            .fill(pal().accent_fill)
            .corner_radius(CornerRadius::same(6)),
    )
}

/// Voce della barra laterale, con un testo opzionale a destra (il saldo).
pub fn nav_item(
    ui: &mut Ui,
    selected: bool,
    icon: &str,
    label: &str,
    right: Option<(String, Color32)>,
) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 28.0), Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, CornerRadius::same(6), pal().accent_soft);
        p.rect_filled(
            Rect::from_min_size(rect.min + Vec2::new(0.0, 5.0), Vec2::new(3.0, rect.height() - 10.0)),
            CornerRadius::same(2),
            pal().accent,
        );
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(6), pal().hover);
    }
    let color = if selected { pal().accent } else { pal().text };
    p.text(rect.left_center() + Vec2::new(10.0, 0.0), Align2::LEFT_CENTER, icon, FontId::proportional(14.0), color);
    let right_w = if let Some((t, c)) = &right {
        let g = p.layout_no_wrap(t.clone(), FontId::proportional(12.5), *c);
        let w = g.size().x;
        p.galley(Pos2::new(rect.right() - 8.0 - w, rect.center().y - g.size().y / 2.0), g, *c);
        w + 12.0
    } else {
        0.0
    };
    let label_rect =
        Rect::from_min_max(rect.min + Vec2::new(34.0, 0.0), Pos2::new(rect.right() - right_w - 4.0, rect.bottom()));
    p.with_clip_rect(label_rect).text(
        label_rect.left_center(),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.0),
        color,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn category_combo(ui: &mut Ui, id: &str, cats: &[Category], value: &mut Option<i64>, none_label: &str) -> bool {
    let mut changed = false;
    let text = value
        .and_then(|v| cats.iter().find(|c| c.id == v))
        .map(|c| c.path.clone())
        .unwrap_or_else(|| none_label.to_string());
    egui::ComboBox::from_id_salt(id).selected_text(text).width(220.0).height(420.0).show_ui(ui, |ui| {
        if ui.selectable_label(value.is_none(), none_label).clicked() {
            *value = None;
            changed = true;
        }
        let mut last_income = None;
        for c in cats {
            if last_income != Some(c.is_income) {
                last_income = Some(c.is_income);
                ui.label(
                    RichText::new(if c.is_income { tr("Income") } else { tr("Expenses") }).small().color(pal().muted),
                );
            }
            let depth = c.path.matches(':').count();
            let label = format!("{}{}", "    ".repeat(depth), c.name);
            if ui.selectable_label(*value == Some(c.id), label).clicked() {
                *value = Some(c.id);
                changed = true;
            }
        }
    });
    if changed {
        // Choosing with the keyboard leaves the list open.
        egui::Popup::close_all(ui.ctx());
    }
    changed
}

pub fn account_combo(ui: &mut Ui, id: &str, accounts: &[Account], value: &mut Option<i64>) -> bool {
    let mut changed = false;
    let text = value
        .and_then(|v| accounts.iter().find(|a| a.id == v))
        .map(|a| a.name.clone())
        .unwrap_or_else(|| tr("Choose an account…").into());
    egui::ComboBox::from_id_salt(id).selected_text(text).width(220.0).show_ui(ui, |ui| {
        for a in accounts.iter().filter(|a| !a.archived) {
            if ui.selectable_label(*value == Some(a.id), format!("{} {}", kind_icon(a.kind), a.name)).clicked() {
                *value = Some(a.id);
                changed = true;
            }
        }
    });
    changed
}

pub fn column_combo(ui: &mut Ui, id: &str, headers: &[String], value: &mut Option<usize>) -> bool {
    let mut changed = false;
    let text = value.and_then(|i| importer::column_label(headers, i)).unwrap_or_else(|| "—".into());
    egui::ComboBox::from_id_salt(id).selected_text(text).width(190.0).show_ui(ui, |ui| {
        if ui.selectable_label(value.is_none(), "—").clicked() {
            *value = None;
            changed = true;
        }
        for i in 0..headers.len() {
            let Some(label) = importer::column_label(headers, i) else { continue };
            if ui.selectable_label(*value == Some(i), label).clicked() {
                *value = Some(i);
                changed = true;
            }
        }
    });
    changed
}

pub fn expense_toggle(ui: &mut Ui, expense: &mut bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let seg = |ui: &mut Ui, on: bool, text: &str, color: Color32| {
            ui.add(
                egui::Button::new(RichText::new(text).color(if on { Color32::WHITE } else { pal().text }))
                    .fill(if on { color } else { pal().track })
                    .min_size(Vec2::new(96.0, 28.0)),
            )
        };
        if seg(ui, *expense, tr("− Expense"), pal().red_fill).clicked() {
            *expense = true;
        }
        if seg(ui, !*expense, tr("+ Income"), pal().green_fill).clicked() {
            *expense = false;
        }
    });
}

pub fn form_row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized([120.0, 24.0], egui::Label::new(RichText::new(label).color(pal().muted)));
        add(ui);
    });
}

pub fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n.saturating_sub(1)).collect::<String>())
    }
}

// ------------------------------------------------------------------ grafici

pub fn short_eur(c: Cents) -> String {
    let e = c as f64 / 100.0;
    let (n, unit) =
        if e.abs() >= 1000.0 { (fmt_number(e.abs() / 1000.0, 1), "k") } else { (fmt_number(e.abs(), 0), "") };
    let sign = if e < 0.0 && n != "0" { "-" } else { "" };
    match lang() {
        Lang::It => format!("{sign}{n}{unit} €"),
        Lang::En => format!("{sign}€{n}{unit}"),
    }
}

fn month_label(m: &str) -> String {
    NaiveDate::parse_from_str(&format!("{m}-01"), "%Y-%m-%d").map(|d| month_short(d.month())).unwrap_or_default()
}

/// Barre affiancate entrate/uscite per mese.
pub fn bars_chart(ui: &mut Ui, data: &[db::MonthTotal], height: f32) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, height), Sense::hover());
    let p = ui.painter_at(rect);
    let max = data.iter().map(|m| m.income.max(m.expense)).max().unwrap_or(1).max(1) as f32;
    let plot = Rect::from_min_max(rect.min + Vec2::new(54.0, 6.0), rect.max - Vec2::new(4.0, 22.0));
    for i in 0..=4 {
        let y = plot.bottom() - plot.height() * i as f32 / 4.0;
        p.hline(plot.x_range(), y, Stroke::new(1.0, if i == 0 { pal().border } else { pal().grid }));
        let v = (max * i as f32 / 4.0) as i64;
        p.text(
            Pos2::new(plot.left() - 6.0, y),
            Align2::RIGHT_CENTER,
            short_eur(v),
            FontId::proportional(10.5),
            pal().muted,
        );
    }
    let n = data.len().max(1) as f32;
    let slot = plot.width() / n;
    let bw = (slot * 0.3).min(22.0);
    let top = CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 };
    for (i, m) in data.iter().enumerate() {
        let cx = plot.left() + slot * (i as f32 + 0.5);
        let hi = plot.height() * m.income as f32 / max;
        let he = plot.height() * m.expense as f32 / max;
        p.rect_filled(
            Rect::from_min_max(Pos2::new(cx - bw - 1.0, plot.bottom() - hi), Pos2::new(cx - 1.0, plot.bottom())),
            top,
            pal().green,
        );
        p.rect_filled(
            Rect::from_min_max(Pos2::new(cx + 1.0, plot.bottom() - he), Pos2::new(cx + bw + 1.0, plot.bottom())),
            top,
            pal().red,
        );
        p.text(
            Pos2::new(cx, plot.bottom() + 11.0),
            Align2::CENTER_CENTER,
            month_label(&m.month),
            FontId::proportional(11.0),
            pal().muted,
        );
    }
}

pub fn legend(ui: &mut Ui, items: &[(&str, Color32)]) {
    ui.horizontal(|ui| {
        for (t, c) in items {
            let (r, _) = ui.allocate_exact_size(Vec2::new(10.0, 10.0), Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(2), *c);
            ui.label(RichText::new(*t).size(12.0).color(pal().muted));
            ui.add_space(6.0);
        }
    });
}

/// Barre orizzontali (nome, valore).
pub fn hbars(ui: &mut Ui, items: &[(String, Cents)]) {
    let max = items.iter().map(|x| x.1).max().unwrap_or(1).max(1) as f32;
    for (i, (name, v)) in items.iter().enumerate() {
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 32.0), Sense::hover());
        let p = ui.painter_at(rect);
        let color = palette(i);
        p.text(rect.left_top() + Vec2::new(0.0, 1.0), Align2::LEFT_TOP, name, FontId::proportional(12.5), pal().text);
        p.text(
            rect.right_top() + Vec2::new(0.0, 1.0),
            Align2::RIGHT_TOP,
            fmt_eur(*v),
            FontId::proportional(12.5),
            pal().text,
        );
        let track = Rect::from_min_size(rect.left_top() + Vec2::new(0.0, 20.0), Vec2::new(w, 7.0));
        p.rect_filled(track, CornerRadius::same(4), pal().track);
        p.rect_filled(
            Rect::from_min_size(track.min, Vec2::new(w * (*v as f32 / max), 7.0)),
            CornerRadius::same(4),
            color,
        );
    }
}

/// Ciambella delle spese per categoria.
pub fn donut(ui: &mut Ui, items: &[(String, Cents)], size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let p = ui.painter_at(rect);
    let total = total(items.iter().map(|x| x.1));
    let c = rect.center();
    let r_out = size / 2.0 - 4.0;
    let r_in = r_out * 0.62;
    if total <= 0 {
        p.circle_stroke(c, (r_out + r_in) / 2.0, Stroke::new(r_out - r_in, pal().track));
    } else {
        let mut a0 = -std::f32::consts::FRAC_PI_2;
        for (i, (_, v)) in items.iter().enumerate() {
            let sweep = std::f32::consts::TAU * (*v as f32 / total as f32);
            let steps = ((sweep / 0.04).ceil() as usize).max(1);
            let color = palette(i);
            // Ogni spicchio è una serie di quadrilateri convessi.
            for s in 0..steps {
                let a = a0 + sweep * s as f32 / steps as f32;
                let b = a0 + sweep * (s + 1) as f32 / steps as f32 + 0.002;
                let quad = vec![
                    c + Vec2::angled(a) * r_out,
                    c + Vec2::angled(b) * r_out,
                    c + Vec2::angled(b) * r_in,
                    c + Vec2::angled(a) * r_in,
                ];
                p.add(egui::Shape::convex_polygon(quad, color, Stroke::NONE));
            }
            a0 += sweep;
        }
    }
    p.text(c - Vec2::new(0.0, 10.0), Align2::CENTER_CENTER, tr("Total"), FontId::proportional(12.0), pal().muted);
    p.text(c + Vec2::new(0.0, 9.0), Align2::CENTER_CENTER, fmt_eur(total), FontId::proportional(15.0), pal().text);
}

/// Linea dell'andamento del patrimonio.
pub fn line_chart(ui: &mut Ui, data: &[(String, Cents)], height: f32) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, height), Sense::hover());
    let p = ui.painter_at(rect);
    let (Some(min), Some(max)) = (data.iter().map(|x| x.1).min(), data.iter().map(|x| x.1).max()) else { return };
    let (min, max) = (min as f32, max as f32);
    let (min, max) = if (max - min).abs() < 1.0 { (min - 100.0, max + 100.0) } else { (min, max) };
    let pad = (max - min) * 0.12;
    let (min, max) = (min - pad, max + pad);
    let plot = Rect::from_min_max(rect.min + Vec2::new(60.0, 8.0), rect.max - Vec2::new(10.0, 22.0));
    for i in 0..=3 {
        let y = plot.bottom() - plot.height() * i as f32 / 3.0;
        p.hline(plot.x_range(), y, Stroke::new(1.0, pal().grid));
        let v = min + (max - min) * i as f32 / 3.0;
        p.text(
            Pos2::new(plot.left() - 6.0, y),
            Align2::RIGHT_CENTER,
            short_eur(v as i64),
            FontId::proportional(10.5),
            pal().muted,
        );
    }
    let n = data.len();
    let pts: Vec<Pos2> = data
        .iter()
        .enumerate()
        .map(|(i, (_, v))| {
            let x = if n <= 1 { plot.center().x } else { plot.left() + plot.width() * i as f32 / (n - 1) as f32 };
            Pos2::new(x, plot.bottom() - plot.height() * ((*v as f32 - min) / (max - min)))
        })
        .collect();
    for (a, b) in pts.iter().zip(pts.iter().skip(1)) {
        let quad = vec![*a, *b, Pos2::new(b.x, plot.bottom()), Pos2::new(a.x, plot.bottom())];
        p.add(egui::Shape::convex_polygon(quad, pal().accent.gamma_multiply(0.11), Stroke::NONE));
    }
    p.line(pts.clone(), Stroke::new(2.5, pal().accent));
    for (pt, (m, _)) in pts.iter().zip(data) {
        p.circle_filled(*pt, 3.0, pal().accent);
        p.text(
            Pos2::new(pt.x, plot.bottom() + 11.0),
            Align2::CENTER_CENTER,
            month_label(m),
            FontId::proportional(11.0),
            pal().muted,
        );
    }
    if let Some(h) = resp.hover_pos()
        && let Some((pt, (_, v))) =
            pts.iter().zip(data).min_by(|(a, _), (b, _)| (a.x - h.x).abs().total_cmp(&(b.x - h.x).abs()))
    {
        p.circle_stroke(*pt, 6.0, Stroke::new(2.0, pal().accent));
        p.text(*pt - Vec2::new(0.0, 12.0), Align2::CENTER_BOTTOM, fmt_eur(*v), FontId::proportional(12.0), pal().text);
    }
}

pub fn progress(ui: &mut Ui, frac: f32, color: Color32) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 8.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(4), pal().track);
    p.rect_filled(
        Rect::from_min_size(rect.min, Vec2::new(w * frac.clamp(0.0, 1.0), 8.0)),
        CornerRadius::same(4),
        color,
    );
}

pub fn budget_color(spent: Cents, budget: Cents) -> Color32 {
    let f = if budget > 0 { spent as f32 / budget as f32 } else { 0.0 };
    if f > 1.0 {
        pal().red
    } else if f > 0.85 {
        pal().amber
    } else {
        pal().green
    }
}

impl App {
    pub fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some((msg, until)) = self.toast.clone() else {
            return;
        };
        let now = ctx.input(|i| i.time);
        if now > until || msg.is_empty() {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        egui::Area::new(egui::Id::unique("toast")).anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -40.0)).show(
            ctx,
            |ui| {
                Frame::new()
                    .fill(LIGHT.text)
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(16, 10))
                    .show(ui, |ui| {
                        ui.label(RichText::new(msg).color(Color32::WHITE));
                    });
            },
        );
    }
}
