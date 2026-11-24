//! Finestra di CashCraft (eframe/egui): avvio, stato, stile e widget comuni.
//! Le singole schermate sono in `views.rs`, le finestre di dialogo in `dialogs.rs`.

use crate::db::{
    self, Account, AccountKind, Category, Cents, Db, Scheduled, Transaction, TxFilter,
};
use crate::importer::{self, ImportResult, Mapping, ParsedRow, Table};
use crate::money::{fmt_date, fmt_eur, month_short};
use crate::{demo, ops};
use anyhow::Result;
use chrono::{Datelike, NaiveDate};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Frame, Margin, Pos2, Rect, RichText, Sense,
    Stroke, Ui, Vec2,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// ------------------------------------------------------------------ colori

pub const ACCENT: Color32 = Color32::from_rgb(0x1F, 0x7A, 0x6A);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(0xDD, 0xEF, 0xEA);
pub const SIDEBAR: Color32 = Color32::from_rgb(0xF2, 0xF4, 0xF3);
pub const BG: Color32 = Color32::from_rgb(0xFA, 0xFB, 0xFB);
pub const CARD: Color32 = Color32::WHITE;
pub const BORDER: Color32 = Color32::from_rgb(0xE1, 0xE5, 0xE4);
pub const TEXT: Color32 = Color32::from_rgb(0x22, 0x2B, 0x2A);
pub const MUTED: Color32 = Color32::from_rgb(0x6B, 0x76, 0x74);
pub const GREEN: Color32 = Color32::from_rgb(0x1E, 0x8E, 0x4E);
pub const RED: Color32 = Color32::from_rgb(0xC6, 0x3B, 0x34);
pub const AMBER: Color32 = Color32::from_rgb(0xD9, 0x8E, 0x1C);
pub const BLUE: Color32 = Color32::from_rgb(0x3B, 0x6E, 0xC4);
pub const ZEBRA: Color32 = Color32::from_rgb(0xF6, 0xF8, 0xF8);
pub const TRACK: Color32 = Color32::from_rgb(0xEC, 0xEF, 0xEE);

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

pub fn amount_color(c: Cents) -> Color32 {
    if c < 0 {
        RED
    } else if c > 0 {
        GREEN
    } else {
        MUTED
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
    pub const ALL: [Period; 6] = [
        Period::ThisMonth,
        Period::LastMonth,
        Period::Last3,
        Period::ThisYear,
        Period::Last12,
        Period::All,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Period::ThisMonth => "Questo mese",
            Period::LastMonth => "Mese scorso",
            Period::Last3 => "Ultimi 3 mesi",
            Period::ThisYear => "Quest'anno",
            Period::Last12 => "Ultimi 12 mesi",
            Period::All => "Tutto",
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

#[derive(Clone)]
pub struct AccountForm {
    pub id: Option<i64>,
    pub name: String,
    pub kind: AccountKind,
    pub opening: String,
    pub iban: String,
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
    pub category: Option<i64>,
    pub date: String,
    pub every: String,
    pub unit: String,
    pub error: String,
}

#[derive(Default)]
pub struct ImportState {
    pub path_text: String,
    pub table: Option<Table>,
    pub mapping: Mapping,
    pub invert: bool,
    pub account: Option<i64>,
    pub new_account: String,
    pub rows: Vec<ParsedRow>,
    pub skipped: usize,
    pub error: String,
    pub result: Option<ImportResult>,
}

impl ImportState {
    pub fn load(&mut self) {
        self.error.clear();
        self.result = None;
        self.table = None;
        self.rows.clear();
        let p = PathBuf::from(self.path_text.trim());
        match importer::read_text(&p).and_then(|(t, enc)| importer::analyze(&t, enc)) {
            Ok(t) => {
                self.mapping = t.mapping.clone();
                self.table = Some(t);
                self.reparse();
            }
            Err(e) => self.error = format!("{e:#}"),
        }
    }
    pub fn reparse(&mut self) {
        if let Some(t) = &self.table {
            let (rows, skipped) = importer::parse_rows(t, &self.mapping, self.invert);
            self.rows = rows;
            self.skipped = skipped;
        }
    }
}

pub enum Dialog {
    Tx(TxForm),
    Transfer(TransferForm),
    Account(AccountForm),
    Import(Box<ImportState>),
    Scheduled(SchedForm),
    ConfirmDeleteAccount(i64, String),
    ConfirmClearDemo,
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
    pub new_rule: (String, Option<i64>, String),
    pub new_cat: (String, bool, Option<i64>),
    pub rename_cat: Option<(i64, String)>,
    pub dash: Option<Dash>,
    pub report: Option<ReportData>,
    data_version: i64,
    last_check: f64,
    pub demo: bool,
    screenshot: Option<PathBuf>,
    frame_count: u32,
    shot_requested: bool,
    shot_done: Arc<AtomicBool>,
}

// ------------------------------------------------------------------ avvio

pub fn run(path: PathBuf, screenshot: Option<PathBuf>) -> Result<()> {
    let db = if screenshot.is_some() && !path.exists() {
        // Per la foto di controllo: dati di esempio in memoria, senza creare file.
        let d = Db::open_in_memory()?;
        demo::seed(&d)?;
        d
    } else {
        let (d, is_new) = Db::open(&path)?;
        if is_new {
            demo::seed(&d)?;
        }
        d
    };
    let shooting = screenshot.is_some();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("CashCraft")
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([900.0, 560.0])
        .with_icon({
            let img = crate::icon::render(256);
            egui::IconData {
                rgba: img.into_raw(),
                width: 256,
                height: 256,
            }
        });
    if shooting {
        viewport = viewport.with_active(false);
        #[cfg(target_os = "windows")]
        {
            viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }
    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    #[cfg(target_os = "macos")]
    if shooting {
        options.event_loop_builder = Some(Box::new(|b| {
            use winit::platform::macos::EventLoopBuilderExtMacOS;
            b.with_activate_ignoring_other_apps(false);
        }));
    }
    eframe::run_native(
        "CashCraft",
        options,
        Box::new(move |cc| {
            setup_style(&cc.egui_ctx);
            Ok(Box::new(App::new(db, screenshot)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("impossibile aprire la finestra: {e}"))
}

fn setup_style(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    let mut v = egui::Visuals::light();
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.extreme_bg_color = Color32::WHITE;
    v.faint_bg_color = ZEBRA;
    v.selection.bg_fill = ACCENT_SOFT;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(10);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(6);
    }
    v.widgets.inactive.bg_fill = TRACK;
    v.widgets.inactive.weak_bg_fill = TRACK;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0xE0, 0xE8, 0xE6);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    style.visuals = v;
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(10.0, 4.0);
    style.spacing.interact_size.y = 26.0;
    use egui::TextStyle::*;
    style.text_styles.insert(Body, FontId::proportional(14.0));
    style.text_styles.insert(Button, FontId::proportional(14.0));
    style
        .text_styles
        .insert(Heading, FontId::proportional(22.0));
    style.text_styles.insert(Small, FontId::proportional(11.5));
    ctx.set_global_style(style);
}

fn save_png(path: &std::path::Path, img: &egui::ColorImage) -> Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let [w, h] = img.size;
    let mut bytes = Vec::with_capacity(w * h * 4);
    for p in &img.pixels {
        bytes.extend_from_slice(&[p.r(), p.g(), p.b(), 255]);
    }
    let buf = image::RgbaImage::from_raw(w as u32, h as u32, bytes)
        .ok_or_else(|| anyhow::anyhow!("immagine non valida"))?;
    buf.save(path)?;
    Ok(())
}

// ------------------------------------------------------------------ App

impl App {
    fn new(db: Db, screenshot: Option<PathBuf>) -> Self {
        let demo = db.meta("demo").as_deref() == Some("1");
        let mut app = App {
            db,
            view: View::Dashboard,
            accounts: vec![],
            categories: vec![],
            txs: vec![],
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
            new_rule: (String::new(), None, String::new()),
            new_cat: (String::new(), false, None),
            rename_cat: None,
            dash: None,
            report: None,
            data_version: 0,
            last_check: 0.0,
            demo,
            screenshot,
            frame_count: 0,
            shot_requested: false,
            shot_done: Arc::new(AtomicBool::new(false)),
        };
        app.data_version = app.db.data_version();
        app.reload();
        // Schermata iniziale facoltativa (utile per le foto di controllo).
        if let Ok(v) = std::env::var("CASHCRAFT_START_VIEW") {
            let first = app.accounts.first().map(|a| a.id);
            app.view = match v.as_str() {
                "movimenti" => View::Register(first),
                "tutti" => View::Register(None),
                "report" => View::Reports,
                "budget" => View::Budget,
                "categorie" => View::Categories,
                "regole" => View::Rules,
                "pianificati" => View::Scheduled,
                _ => View::Dashboard,
            };
            app.reload_txs();
            if v == "importa" {
                app.open_import(None);
            }
        }
        app
    }

    pub fn reload(&mut self) {
        self.accounts = self.db.accounts(true).unwrap_or_default();
        self.categories = self.db.categories().unwrap_or_default();
        self.demo = self.db.meta("demo").as_deref() == Some("1");
        self.reload_txs();
        self.dash = None;
        self.report = None;
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
            limit: None,
        };
        self.txs = self.db.transactions(&f).unwrap_or_default();
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
            Err(e) => self.notify(ui, format!("Errore: {e:#}")),
        }
        self.reload();
    }

    pub fn go(&mut self, v: View) {
        if self.view != v {
            self.view = v;
            self.selected = None;
            if matches!(v, View::Register(_)) {
                self.reload_txs();
            }
        }
    }

    pub fn account_name(&self, id: i64) -> String {
        self.accounts
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.name.clone())
            .unwrap_or_default()
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
            ..Default::default()
        }));
    }

    pub fn open_edit_tx(&mut self, t: &Transaction) {
        self.dialog = Some(Dialog::Tx(TxForm {
            id: Some(t.id),
            account: Some(t.account_id),
            date: fmt_date(t.date),
            amount: crate::money::fmt_plain(t.amount.abs()),
            expense: t.amount < 0,
            payee: t.payee.clone(),
            memo: t.memo.clone(),
            category: t.category_id,
            cleared: t.cleared,
            is_transfer: t.transfer_peer.is_some(),
            error: String::new(),
        }));
    }

    pub fn open_import(&mut self, path: Option<PathBuf>) {
        let mut st = ImportState {
            account: self.default_account(),
            ..Default::default()
        };
        if let Some(p) = path {
            st.path_text = p.display().to_string();
            st.load();
        }
        self.dialog = Some(Dialog::Import(Box::new(st)));
    }

    pub fn ensure_dash(&mut self) {
        if self.dash.is_some() {
            return;
        }
        let t = db::today();
        let (from, to) = db::month_range(t);
        let totals = self.db.category_totals(from, to).unwrap_or_default();
        let month_income = totals.iter().filter(|c| c.total > 0).map(|c| c.total).sum();
        let month_expense = -totals
            .iter()
            .filter(|c| c.total < 0)
            .map(|c| c.total)
            .sum::<Cents>();
        let mut top: Vec<(String, Cents)> = totals
            .iter()
            .filter(|c| c.total < 0)
            .map(|c| (c.category.clone(), -c.total))
            .collect();
        top.sort_by_key(|x| -x.1);
        top.truncate(6);
        let uncategorized = self
            .db
            .transactions(&TxFilter {
                uncategorized: true,
                ..Default::default()
            })
            .map(|v| v.len())
            .unwrap_or(0);
        self.dash = Some(Dash {
            month_income,
            month_expense,
            monthly: self.db.monthly_totals(6, t).unwrap_or_default(),
            top_expenses: top,
            recent: self
                .db
                .transactions(&TxFilter {
                    limit: Some(8),
                    ..Default::default()
                })
                .unwrap_or_default(),
            upcoming: self
                .db
                .scheduled()
                .unwrap_or_default()
                .into_iter()
                .take(5)
                .collect(),
            budgets: self.db.budget_status(t).unwrap_or_default(),
            uncategorized,
        });
    }

    pub fn ensure_report(&mut self) {
        if self.report.is_some() {
            return;
        }
        let t = db::today();
        let (from, to) = self.report_period.range();
        let from = from.unwrap_or(NaiveDate::from_ymd_opt(1970, 1, 1).unwrap());
        let to = to.unwrap_or(t);
        let months = match self.report_period {
            Period::ThisMonth | Period::LastMonth | Period::Last3 => 6,
            Period::ThisYear => t.month().max(3),
            _ => 12,
        };
        self.report = Some(ReportData {
            totals: self.db.category_totals(from, to).unwrap_or_default(),
            monthly: self.db.monthly_totals(months, to).unwrap_or_default(),
            net_worth: self.db.net_worth_history(12, t).unwrap_or_default(),
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Foto di controllo (--screenshot): ~120 fotogrammi, poi salva ed esce.
        if let Some(path) = self.screenshot.clone() {
            self.frame_count += 1;
            ctx.request_repaint();
            if self.frame_count >= 120 && !self.shot_requested {
                self.shot_requested = true;
                let done = self.shot_done.clone();
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                    egui::ScreenshotCallback::new(move |img| {
                        if let Err(e) = save_png(&path, &img) {
                            eprintln!("Errore nel salvare l'immagine: {e:#}");
                        }
                        done.store(true, Ordering::SeqCst);
                    }),
                ));
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
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if let Some(p) = dropped.into_iter().next() {
            self.open_import(Some(p));
        }

        // Scorciatoie.
        if self.dialog.is_none() {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::N)) {
                self.open_new_tx();
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::I)) {
                self.open_import(None);
            }
        }

        self.sidebar(ui);
        self.toolbar(ui);
        self.status_bar(ui);
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(22, 16)),
            )
            .show(ui, |ui| match self.view {
                View::Dashboard => self.dashboard(ui),
                View::Register(a) => self.register(ui, a),
                View::Reports => self.reports(ui),
                View::Budget => self.budget(ui),
                View::Categories => self.categories_view(ui),
                View::Rules => self.rules_view(ui),
                View::Scheduled => self.scheduled_view(ui),
            });
        self.dialogs(&ctx);
        self.draw_toast(&ctx);
    }
}

// ------------------------------------------------------------------ widget

pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| ui.vertical(add).inner)
        .inner
}

pub fn section_title(ui: &mut Ui, s: &str) {
    ui.label(RichText::new(s).size(15.0).strong().color(TEXT));
}

pub fn small_caps(ui: &mut Ui, s: &str) {
    ui.label(
        RichText::new(s.to_uppercase())
            .size(11.0)
            .color(MUTED)
            .strong(),
    );
}

pub fn money(c: Cents) -> RichText {
    RichText::new(fmt_eur(c)).color(amount_color(c))
}

pub fn primary_button(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
            .fill(ACCENT)
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
        p.rect_filled(rect, CornerRadius::same(6), ACCENT_SOFT);
        p.rect_filled(
            Rect::from_min_size(
                rect.min + Vec2::new(0.0, 5.0),
                Vec2::new(3.0, rect.height() - 10.0),
            ),
            CornerRadius::same(2),
            ACCENT,
        );
    } else if resp.hovered() {
        p.rect_filled(
            rect,
            CornerRadius::same(6),
            Color32::from_rgb(0xE6, 0xEA, 0xE9),
        );
    }
    let color = if selected { ACCENT } else { TEXT };
    p.text(
        rect.left_center() + Vec2::new(10.0, 0.0),
        Align2::LEFT_CENTER,
        icon,
        FontId::proportional(14.0),
        color,
    );
    let right_w = if let Some((t, c)) = &right {
        let g = p.layout_no_wrap(t.clone(), FontId::proportional(12.5), *c);
        let w = g.size().x;
        p.galley(
            Pos2::new(rect.right() - 8.0 - w, rect.center().y - g.size().y / 2.0),
            g,
            *c,
        );
        w + 12.0
    } else {
        0.0
    };
    let label_rect = Rect::from_min_max(
        rect.min + Vec2::new(34.0, 0.0),
        Pos2::new(rect.right() - right_w - 4.0, rect.bottom()),
    );
    p.with_clip_rect(label_rect).text(
        label_rect.left_center(),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.0),
        color,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn category_combo(
    ui: &mut Ui,
    id: &str,
    cats: &[Category],
    value: &mut Option<i64>,
    none_label: &str,
) -> bool {
    let mut changed = false;
    let text = value
        .and_then(|v| cats.iter().find(|c| c.id == v))
        .map(|c| c.path.clone())
        .unwrap_or_else(|| none_label.to_string());
    egui::ComboBox::from_id_salt(id)
        .selected_text(text)
        .width(220.0)
        .height(420.0)
        .show_ui(ui, |ui| {
            if ui.selectable_label(value.is_none(), none_label).clicked() {
                *value = None;
                changed = true;
            }
            let mut last_income = None;
            for c in cats {
                if last_income != Some(c.is_income) {
                    last_income = Some(c.is_income);
                    ui.label(
                        RichText::new(if c.is_income { "Entrate" } else { "Uscite" })
                            .small()
                            .color(MUTED),
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
    changed
}

pub fn account_combo(ui: &mut Ui, id: &str, accounts: &[Account], value: &mut Option<i64>) -> bool {
    let mut changed = false;
    let text = value
        .and_then(|v| accounts.iter().find(|a| a.id == v))
        .map(|a| a.name.clone())
        .unwrap_or_else(|| "Scegli un conto…".into());
    egui::ComboBox::from_id_salt(id)
        .selected_text(text)
        .width(220.0)
        .show_ui(ui, |ui| {
            for a in accounts.iter().filter(|a| !a.archived) {
                if ui
                    .selectable_label(
                        *value == Some(a.id),
                        format!("{} {}", kind_icon(a.kind), a.name),
                    )
                    .clicked()
                {
                    *value = Some(a.id);
                    changed = true;
                }
            }
        });
    changed
}

pub fn column_combo(ui: &mut Ui, id: &str, headers: &[String], value: &mut Option<usize>) -> bool {
    let mut changed = false;
    let text = value
        .and_then(|i| headers.get(i).cloned())
        .unwrap_or_else(|| "—".into());
    egui::ComboBox::from_id_salt(id)
        .selected_text(text)
        .width(190.0)
        .show_ui(ui, |ui| {
            if ui.selectable_label(value.is_none(), "—").clicked() {
                *value = None;
                changed = true;
            }
            for (i, h) in headers.iter().enumerate() {
                let label = if h.is_empty() {
                    format!("Colonna {}", i + 1)
                } else {
                    h.clone()
                };
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
                egui::Button::new(RichText::new(text).color(if on {
                    Color32::WHITE
                } else {
                    TEXT
                }))
                .fill(if on { color } else { TRACK })
                .min_size(Vec2::new(96.0, 28.0)),
            )
        };
        if seg(ui, *expense, "− Uscita", RED).clicked() {
            *expense = true;
        }
        if seg(ui, !*expense, "+ Entrata", GREEN).clicked() {
            *expense = false;
        }
    });
}

pub fn form_row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [120.0, 24.0],
            egui::Label::new(RichText::new(label).color(MUTED)),
        );
        add(ui);
    });
}

pub fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!(
            "{}…",
            s.chars().take(n.saturating_sub(1)).collect::<String>()
        )
    }
}

// ------------------------------------------------------------------ grafici

pub fn short_eur(c: Cents) -> String {
    let e = c as f64 / 100.0;
    if e.abs() >= 1000.0 {
        format!("{:.1}k €", e / 1000.0).replace('.', ",")
    } else {
        format!("{:.0} €", e)
    }
}

fn month_label(m: &str) -> String {
    NaiveDate::parse_from_str(&format!("{m}-01"), "%Y-%m-%d")
        .map(|d| month_short(d.month()))
        .unwrap_or_default()
}

/// Barre affiancate entrate/uscite per mese.
pub fn bars_chart(ui: &mut Ui, data: &[db::MonthTotal], height: f32) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, height), Sense::hover());
    let p = ui.painter_at(rect);
    let max = data
        .iter()
        .map(|m| m.income.max(m.expense))
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let plot = Rect::from_min_max(
        rect.min + Vec2::new(54.0, 6.0),
        rect.max - Vec2::new(4.0, 22.0),
    );
    for i in 0..=4 {
        let y = plot.bottom() - plot.height() * i as f32 / 4.0;
        p.hline(
            plot.x_range(),
            y,
            Stroke::new(
                1.0,
                if i == 0 {
                    BORDER
                } else {
                    Color32::from_rgb(0xEE, 0xF1, 0xF0)
                },
            ),
        );
        let v = (max * i as f32 / 4.0) as i64;
        p.text(
            Pos2::new(plot.left() - 6.0, y),
            Align2::RIGHT_CENTER,
            short_eur(v),
            FontId::proportional(10.5),
            MUTED,
        );
    }
    let n = data.len().max(1) as f32;
    let slot = plot.width() / n;
    let bw = (slot * 0.3).min(22.0);
    let top = CornerRadius {
        nw: 3,
        ne: 3,
        sw: 0,
        se: 0,
    };
    for (i, m) in data.iter().enumerate() {
        let cx = plot.left() + slot * (i as f32 + 0.5);
        let hi = plot.height() * m.income as f32 / max;
        let he = plot.height() * m.expense as f32 / max;
        p.rect_filled(
            Rect::from_min_max(
                Pos2::new(cx - bw - 1.0, plot.bottom() - hi),
                Pos2::new(cx - 1.0, plot.bottom()),
            ),
            top,
            GREEN,
        );
        p.rect_filled(
            Rect::from_min_max(
                Pos2::new(cx + 1.0, plot.bottom() - he),
                Pos2::new(cx + bw + 1.0, plot.bottom()),
            ),
            top,
            RED,
        );
        p.text(
            Pos2::new(cx, plot.bottom() + 11.0),
            Align2::CENTER_CENTER,
            month_label(&m.month),
            FontId::proportional(11.0),
            MUTED,
        );
    }
}

pub fn legend(ui: &mut Ui, items: &[(&str, Color32)]) {
    ui.horizontal(|ui| {
        for (t, c) in items {
            let (r, _) = ui.allocate_exact_size(Vec2::new(10.0, 10.0), Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(2), *c);
            ui.label(RichText::new(*t).size(12.0).color(MUTED));
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
        let color = PALETTE[i % PALETTE.len()];
        p.text(
            rect.left_top() + Vec2::new(0.0, 1.0),
            Align2::LEFT_TOP,
            name,
            FontId::proportional(12.5),
            TEXT,
        );
        p.text(
            rect.right_top() + Vec2::new(0.0, 1.0),
            Align2::RIGHT_TOP,
            fmt_eur(*v),
            FontId::proportional(12.5),
            TEXT,
        );
        let track = Rect::from_min_size(rect.left_top() + Vec2::new(0.0, 20.0), Vec2::new(w, 7.0));
        p.rect_filled(track, CornerRadius::same(4), TRACK);
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
    let total: Cents = items.iter().map(|x| x.1).sum();
    let c = rect.center();
    let r_out = size / 2.0 - 4.0;
    let r_in = r_out * 0.62;
    if total <= 0 {
        p.circle_stroke(c, (r_out + r_in) / 2.0, Stroke::new(r_out - r_in, TRACK));
    } else {
        let mut a0 = -std::f32::consts::FRAC_PI_2;
        for (i, (_, v)) in items.iter().enumerate() {
            let sweep = std::f32::consts::TAU * (*v as f32 / total as f32);
            let steps = ((sweep / 0.04).ceil() as usize).max(1);
            let color = PALETTE[i % PALETTE.len()];
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
    p.text(
        c - Vec2::new(0.0, 10.0),
        Align2::CENTER_CENTER,
        "Totale",
        FontId::proportional(12.0),
        MUTED,
    );
    p.text(
        c + Vec2::new(0.0, 9.0),
        Align2::CENTER_CENTER,
        fmt_eur(total),
        FontId::proportional(15.0),
        TEXT,
    );
}

/// Linea dell'andamento del patrimonio.
pub fn line_chart(ui: &mut Ui, data: &[(String, Cents)], height: f32) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, height), Sense::hover());
    let p = ui.painter_at(rect);
    if data.is_empty() {
        return;
    }
    let min = data.iter().map(|x| x.1).min().unwrap() as f32;
    let max = data.iter().map(|x| x.1).max().unwrap() as f32;
    let (min, max) = if (max - min).abs() < 1.0 {
        (min - 100.0, max + 100.0)
    } else {
        (min, max)
    };
    let pad = (max - min) * 0.12;
    let (min, max) = (min - pad, max + pad);
    let plot = Rect::from_min_max(
        rect.min + Vec2::new(60.0, 8.0),
        rect.max - Vec2::new(10.0, 22.0),
    );
    for i in 0..=3 {
        let y = plot.bottom() - plot.height() * i as f32 / 3.0;
        p.hline(
            plot.x_range(),
            y,
            Stroke::new(1.0, Color32::from_rgb(0xEE, 0xF1, 0xF0)),
        );
        let v = min + (max - min) * i as f32 / 3.0;
        p.text(
            Pos2::new(plot.left() - 6.0, y),
            Align2::RIGHT_CENTER,
            short_eur(v as i64),
            FontId::proportional(10.5),
            MUTED,
        );
    }
    let n = data.len();
    let pts: Vec<Pos2> = data
        .iter()
        .enumerate()
        .map(|(i, (_, v))| {
            let x = if n <= 1 {
                plot.center().x
            } else {
                plot.left() + plot.width() * i as f32 / (n - 1) as f32
            };
            Pos2::new(
                x,
                plot.bottom() - plot.height() * ((*v as f32 - min) / (max - min)),
            )
        })
        .collect();
    for i in 0..pts.len().saturating_sub(1) {
        let quad = vec![
            pts[i],
            pts[i + 1],
            Pos2::new(pts[i + 1].x, plot.bottom()),
            Pos2::new(pts[i].x, plot.bottom()),
        ];
        p.add(egui::Shape::convex_polygon(
            quad,
            Color32::from_rgba_unmultiplied(0x1F, 0x7A, 0x6A, 28),
            Stroke::NONE,
        ));
    }
    p.line(pts.clone(), Stroke::new(2.5, ACCENT));
    for (i, (m, _)) in data.iter().enumerate() {
        p.circle_filled(pts[i], 3.0, ACCENT);
        p.text(
            Pos2::new(pts[i].x, plot.bottom() + 11.0),
            Align2::CENTER_CENTER,
            month_label(m),
            FontId::proportional(11.0),
            MUTED,
        );
    }
    if let Some(h) = resp.hover_pos() {
        let i = (0..pts.len())
            .min_by(|a, b| {
                (pts[*a].x - h.x)
                    .abs()
                    .partial_cmp(&(pts[*b].x - h.x).abs())
                    .unwrap()
            })
            .unwrap();
        p.circle_stroke(pts[i], 6.0, Stroke::new(2.0, ACCENT));
        p.text(
            pts[i] - Vec2::new(0.0, 12.0),
            Align2::CENTER_BOTTOM,
            fmt_eur(data[i].1),
            FontId::proportional(12.0),
            TEXT,
        );
    }
}

pub fn progress(ui: &mut Ui, frac: f32, color: Color32) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 8.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(4), TRACK);
    p.rect_filled(
        Rect::from_min_size(rect.min, Vec2::new(w * frac.clamp(0.0, 1.0), 8.0)),
        CornerRadius::same(4),
        color,
    );
}

pub fn budget_color(spent: Cents, budget: Cents) -> Color32 {
    let f = if budget > 0 {
        spent as f32 / budget as f32
    } else {
        0.0
    };
    if f > 1.0 {
        RED
    } else if f > 0.85 {
        AMBER
    } else {
        GREEN
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
        egui::Area::new(egui::Id::unique("toast"))
            .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -40.0))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(TEXT)
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(16, 10))
                    .show(ui, |ui| {
                        ui.label(RichText::new(msg).color(Color32::WHITE));
                    });
            });
    }
}
