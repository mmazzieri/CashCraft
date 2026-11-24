//! The chart of accounts, the trial balance and the form of an entry.

use crate::gui::{
    App, Dialog, MUTED, RED, TEXT, TRACK, card, error_text, form_row, or_report, primary_button, section_title,
    small_caps,
};
use anyhow::Result;
use cashcraft_engine::db::{self, Cents};
use cashcraft_engine::i18n::{tr, trf};
use cashcraft_engine::ledger::{Balance, Class, LedgerAccount, NewEntry, NewLine, TrialBalance};
use cashcraft_engine::money::{fmt_date, fmt_eur, fmt_plain, parse_amount_exact, parse_date};
use egui::{self, Align, Layout, RichText, Ui, Vec2};
use std::collections::{BTreeMap, HashSet};

/// What the view shows, read once.
pub struct LedgerData {
    pub accounts: Vec<LedgerAccount>,
    pub trial: TrialBalance,
}

#[derive(Clone, Default)]
pub struct EntryLine {
    pub account: Option<i64>,
    pub debit: String,
    pub credit: String,
}

#[derive(Clone)]
pub struct EntryForm {
    pub date: String,
    pub payee: String,
    pub memo: String,
    pub lines: Vec<EntryLine>,
    pub error: String,
    /// Whether the keyboard has been given to the form.
    pub focused: bool,
}

impl Default for EntryForm {
    fn default() -> Self {
        EntryForm {
            date: fmt_date(db::today()),
            payee: String::new(),
            memo: String::new(),
            lines: vec![EntryLine::default(); 2],
            error: String::new(),
            focused: false,
        }
    }
}

impl EntryLine {
    /// Debit minus credit.
    pub fn amount(&self) -> Result<Cents> {
        let side = |s: &str| if s.trim().is_empty() { Ok(0) } else { parse_amount_exact(s) };
        side(&self.debit)?
            .checked_sub(side(&self.credit)?)
            .ok_or_else(|| anyhow::anyhow!(tr("the amount is too large")))
    }

    fn is_blank(&self) -> bool {
        self.account.is_none() && self.debit.trim().is_empty() && self.credit.trim().is_empty()
    }
}

impl EntryForm {
    /// Nothing would be lost by closing it.
    pub fn is_untouched(&self) -> bool {
        self.payee.is_empty() && self.memo.is_empty() && self.lines.iter().all(EntryLine::is_blank)
    }

    /// What is missing for the entry to balance: debits minus credits.
    pub fn difference(&self) -> Result<Cents> {
        self.lines.iter().try_fold(0, |sum: Cents, l| {
            sum.checked_add(l.amount()?).ok_or_else(|| anyhow::anyhow!(tr("the amount is too large")))
        })
    }

    pub fn entry(&self) -> Result<NewEntry> {
        let date = parse_date(&self.date).ok_or_else(|| anyhow::anyhow!(tr("the date is not valid")))?;
        let mut lines = Vec::new();
        for l in self.lines.iter().filter(|l| !l.is_blank()) {
            let ledger_account_id = l.account.ok_or_else(|| anyhow::anyhow!(tr("choose an account")))?;
            lines.push(NewLine { ledger_account_id, amount: l.amount()?, currency: None });
        }
        if lines.iter().all(|l| l.amount == 0) {
            anyhow::bail!(tr("enter an amount"));
        }
        if self.payee.trim().is_empty() {
            anyhow::bail!(tr("write a description"));
        }
        Ok(NewEntry { date, payee: self.payee.clone(), memo: self.memo.clone(), lines })
    }
}

#[derive(Clone, Default)]
pub struct LedgerAccountForm {
    pub name: String,
    pub code: String,
    pub class: Option<Class>,
    pub parent: Option<i64>,
    pub currency: String,
    pub error: String,
}

/// A row of the tree: the account, how deep it is, and its balances with those of the accounts below.
pub struct TreeRow<'a> {
    pub account: &'a LedgerAccount,
    pub depth: usize,
    pub balances: Vec<Balance>,
}

/// The accounts of a class, each under the one above it.
pub fn tree(accounts: &[LedgerAccount], class: Class) -> Vec<TreeRow<'_>> {
    let mine: Vec<&LedgerAccount> = accounts.iter().filter(|a| a.class == class).collect();
    let ids: HashSet<i64> = mine.iter().map(|a| a.id).collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    // Roots first; then whatever a damaged file does not hang from any root.
    let roots = mine.iter().filter(|a| a.parent_id.is_none_or(|p| !ids.contains(&p)));
    for a in roots.chain(mine.iter()) {
        branch(a, &mine, 0, &mut seen, &mut out);
    }
    out
}

fn branch<'a>(
    a: &'a LedgerAccount,
    all: &[&'a LedgerAccount],
    depth: usize,
    seen: &mut HashSet<i64>,
    out: &mut Vec<TreeRow<'a>>,
) -> BTreeMap<String, Cents> {
    let mut sums = BTreeMap::new();
    if !seen.insert(a.id) {
        return sums;
    }
    let at = out.len();
    out.push(TreeRow { account: a, depth, balances: Vec::new() });
    for b in &a.balances {
        sums.insert(b.currency.clone(), b.amount);
    }
    for c in all.iter().filter(|c| c.parent_id == Some(a.id)) {
        for (currency, amount) in branch(c, all, depth + 1, seen, out) {
            let sum = sums.entry(currency).or_default();
            *sum = amount.saturating_add(*sum);
        }
    }
    if let Some(row) = out.get_mut(at) {
        row.balances = sums.iter().map(|(c, v)| Balance { currency: c.clone(), amount: *v }).collect();
    }
    sums
}

/// An amount with its currency.
pub fn fmt_currency(c: Cents, currency: &str) -> String {
    if currency == "EUR" { fmt_eur(c) } else { format!("{} {currency}", fmt_plain(c)) }
}

/// The balances of an account as its class reads them: what grows it is positive.
fn balance_text(balances: &[Balance], class: Class) -> String {
    let sign = |c: Cents| if class.grows_by_credit() { c.saturating_neg() } else { c };
    let parts: Vec<String> =
        balances.iter().filter(|b| b.amount != 0).map(|b| fmt_currency(sign(b.amount), &b.currency)).collect();
    if parts.is_empty() { "—".into() } else { parts.join(" · ") }
}

fn ledger_combo(ui: &mut Ui, id: (&str, usize), accounts: &[LedgerAccount], value: &mut Option<i64>, none: &str) {
    let name = |a: &LedgerAccount| {
        if a.code.is_empty() { a.path.clone() } else { format!("{} {}", a.code, a.path) }
    };
    let text = value.and_then(|v| accounts.iter().find(|a| a.id == v)).map_or_else(|| none.to_string(), name);
    let before = *value;
    let combo = egui::ComboBox::from_id_salt(id).selected_text(text).width(260.0).height(420.0).show_ui(ui, |ui| {
        ui.selectable_value(value, None, none);
        for class in Class::ALL {
            ui.label(RichText::new(class.label()).small().color(MUTED));
            for a in accounts.iter().filter(|a| a.class == class) {
                ui.selectable_value(value, Some(a.id), name(a));
            }
        }
    });
    if *value != before {
        // Choosing with the keyboard leaves the list open; what follows goes on from here.
        egui::Popup::close_all(ui.ctx());
        combo.response.request_focus();
    }
}

impl App {
    pub fn ledger_view(&mut self, ui: &mut Ui) {
        let date = self.ledger_date.trim().to_string();
        let until = parse_date(&date);
        let data = match self.ledger.take() {
            Some(d) => d,
            None => LedgerData {
                accounts: or_report(self.db.ledger_accounts(), &mut self.toast),
                trial: or_report(self.db.trial_balance(until), &mut self.toast),
            },
        };
        let mut changed = false;
        let mut add: Option<LedgerAccountForm> = None;
        ui.horizontal(|ui| {
            if primary_button(ui, tr("+ Entry")).on_hover_text(tr("New entry in the ledger (⌘J)")).clicked() {
                self.open_entry();
            }
            if ui.button(tr("+ Account in the chart")).clicked() {
                add = Some(LedgerAccountForm { class: Some(Class::Asset), ..Default::default() });
            }
            if ui.button(tr("✔ Check the books")).clicked() {
                let message = match self.db.verify_ledger() {
                    Ok(v) if v.problems.is_empty() => trf(
                        "The books are sound: {entries} entries, {postings} postings",
                        &[("entries", &v.entries), ("postings", &v.postings)],
                    ),
                    Ok(v) => v.problems.into_iter().take(3).collect::<Vec<_>>().join("; "),
                    Err(e) => error_text(&e),
                };
                self.notify(ui, message);
            }
        });
        ui.add_space(10.0);
        let gap = 14.0;
        let half = (ui.available_width() - gap) / 2.0;
        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.allocate_ui(Vec2::new(half, height), |ui| {
                card(ui, |ui| {
                    ui.set_width(half - 30.0);
                    section_title(ui, tr("Accounts"));
                    egui::ScrollArea::vertical().id_salt("chart").auto_shrink([false, false]).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for class in Class::ALL {
                            ui.add_space(6.0);
                            small_caps(ui, class.label());
                            for row in tree(&data.accounts, class) {
                                chart_row(ui, &row, &mut add);
                            }
                        }
                    });
                });
            });
            ui.allocate_ui(Vec2::new(half, height), |ui| {
                card(ui, |ui| {
                    ui.set_width(half - 30.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        section_title(ui, tr("Trial balance"));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let edit = egui::TextEdit::singleline(&mut self.ledger_date)
                                .background_color(TRACK)
                                .hint_text(tr("yyyy-mm-dd"))
                                .desired_width(100.0);
                            changed = ui.add(edit).changed();
                            ui.label(RichText::new(tr("up to")).color(MUTED));
                        });
                    });
                    if !date.is_empty() && until.is_none() {
                        ui.label(RichText::new(tr("the date is not valid")).color(RED));
                    }
                    ui.spacing_mut().item_spacing.x = 8.0;
                    trial_table(ui, &data.trial);
                });
            });
        });
        if !changed {
            self.ledger = Some(data);
        }
        if let Some(f) = add {
            self.dialog = Some(Dialog::LedgerAccount(f));
        }
    }

    pub fn open_entry(&mut self) {
        self.dialog = Some(Dialog::Entry(EntryForm::default()));
    }

    /// The form of an entry: whether it stays open, and what to tell.
    pub fn entry_dialog(&mut self, ui: &mut Ui, f: &mut EntryForm) -> (bool, Option<Result<String>>) {
        ui.set_min_width(640.0);
        ui.heading(tr("New entry"));
        ui.label(RichText::new(tr("Debits and credits must be equal.")).color(MUTED));
        ui.add_space(8.0);
        form_row(ui, tr("Date"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut f.date)
                    .background_color(TRACK)
                    .hint_text(tr("yyyy-mm-dd"))
                    .desired_width(120.0),
            );
        });
        form_row(ui, tr("Description"), |ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut f.payee).background_color(TRACK).desired_width(360.0));
            if !f.focused {
                f.focused = true;
                r.request_focus();
            }
        });
        form_row(ui, tr("Notes"), |ui| {
            ui.add(egui::TextEdit::singleline(&mut f.memo).background_color(TRACK).desired_width(360.0));
        });
        ui.add_space(6.0);
        let accounts = or_report(self.db.ledger_accounts(), &mut self.toast);
        let mut remove = None;
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            egui::Grid::new("entry_lines").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
                for h in [tr("Account"), tr("Debit"), tr("Credit"), ""] {
                    ui.label(RichText::new(h.to_uppercase()).size(11.0).color(MUTED));
                }
                ui.end_row();
                let removable = f.lines.len() > 2;
                for (i, l) in f.lines.iter_mut().enumerate() {
                    ledger_combo(ui, ("entry_account", i), &accounts, &mut l.account, tr("Choose an account…"));
                    for text in [&mut l.debit, &mut l.credit] {
                        let edit = egui::TextEdit::singleline(text)
                            .hint_text(fmt_plain(0))
                            .background_color(TRACK)
                            .align(egui::Align2::RIGHT_CENTER);
                        ui.add_sized([120.0, 26.0], edit);
                    }
                    if ui.add_enabled(removable, egui::Button::new("✖")).on_hover_text(tr("Remove the line")).clicked()
                    {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
        });
        if let Some(i) = remove {
            f.lines.remove(i);
        }
        ui.horizontal(|ui| {
            if ui.button(tr("+ Line")).clicked() {
                f.lines.push(EntryLine::default());
            }
            let (text, color) = match f.difference() {
                Ok(0) => (trf("Difference: {amount}", &[("amount", &fmt_plain(0))]), MUTED),
                Ok(d) => (trf("Difference: {amount}", &[("amount", &fmt_plain(d))]), RED),
                Err(e) => (error_text(&e), RED),
            };
            ui.label(RichText::new(text).strong().color(color));
        });
        if !f.error.is_empty() {
            ui.label(RichText::new(&f.error).color(RED));
        }
        ui.add_space(8.0);
        let mut keep = true;
        let mut message = None;
        ui.horizontal(|ui| {
            if primary_button(ui, tr("Post")).clicked() {
                match f.entry().and_then(|e| self.db.post_entry(&e)) {
                    Ok(_) => {
                        message = Some(Ok(tr("Entry posted").to_string()));
                        keep = false;
                    }
                    Err(e) => f.error = format!("{e:#}"),
                }
            }
            if ui.button(tr("Cancel")).clicked() {
                keep = false;
            }
        });
        (keep, message)
    }

    pub fn ledger_account_dialog(&mut self, ui: &mut Ui, f: &mut LedgerAccountForm) -> (bool, Option<Result<String>>) {
        ui.heading(tr("New account in the chart"));
        ui.add_space(8.0);
        let accounts = or_report(self.db.ledger_accounts(), &mut self.toast);
        form_row(ui, tr("Name"), |ui| {
            ui.add(egui::TextEdit::singleline(&mut f.name).background_color(TRACK).desired_width(260.0));
        });
        form_row(ui, tr("Code"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut f.code)
                    .background_color(TRACK)
                    .hint_text(tr("optional"))
                    .desired_width(120.0),
            );
        });
        form_row(ui, tr("Under"), |ui| {
            let above: Vec<LedgerAccount> = accounts.iter().filter(|a| a.system.is_none()).cloned().collect();
            ledger_combo(ui, ("above", 0), &above, &mut f.parent, tr("(none: top level)"));
        });
        let class = match f.parent.and_then(|p| accounts.iter().find(|a| a.id == p)) {
            Some(p) => Some(p.class),
            None => f.class,
        };
        form_row(ui, tr("Class"), |ui| {
            if f.parent.is_some() {
                ui.label(class.map_or("", Class::label));
            } else {
                let text = class.map_or("", Class::label);
                egui::ComboBox::from_id_salt("class").selected_text(text).width(200.0).show_ui(ui, |ui| {
                    for c in Class::ALL {
                        ui.selectable_value(&mut f.class, Some(c), c.label());
                    }
                });
            }
        });
        let is_category = matches!(class, Some(Class::Income | Class::Expense));
        if is_category {
            ui.label(RichText::new(tr("An income or expense account is a category.")).color(MUTED));
        } else {
            form_row(ui, tr("Currency"), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut f.currency)
                        .background_color(TRACK)
                        .hint_text("EUR")
                        .desired_width(80.0),
                );
            });
        }
        if !f.error.is_empty() {
            ui.label(RichText::new(&f.error).color(RED));
        }
        ui.add_space(8.0);
        let mut keep = true;
        let mut message = None;
        ui.horizontal(|ui| {
            if primary_button(ui, tr("Add")).clicked() {
                let currency = (!is_category).then_some(f.currency.as_str());
                match self.db.create_ledger_account(&f.name, class, f.parent, &f.code, currency) {
                    Ok(_) => {
                        message = Some(Ok(tr("Account created").to_string()));
                        keep = false;
                    }
                    Err(e) => f.error = format!("{e:#}"),
                }
            }
            if ui.button(tr("Cancel")).clicked() {
                keep = false;
            }
        });
        (keep, message)
    }
}

fn chart_row(ui: &mut Ui, row: &TreeRow, add: &mut Option<LedgerAccountForm>) {
    let a = row.account;
    ui.horizontal(|ui| {
        ui.add_space(row.depth as f32 * 18.0);
        if !a.code.is_empty() {
            ui.label(RichText::new(&a.code).size(12.0).color(MUTED));
        }
        let name = RichText::new(&a.name).color(if a.system.is_some() { MUTED } else { TEXT });
        ui.label(if row.depth == 0 { name.strong() } else { name });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let button = ui.add_enabled(a.system.is_none(), egui::Button::new("+").small());
            let hint = trf("Add an account under \"{name}\"", &[("name", &a.path)]);
            // The name of the button for screen readers.
            button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, hint.clone()));
            if button.on_hover_text(hint.clone()).clicked() {
                *add = Some(LedgerAccountForm { parent: Some(a.id), ..Default::default() });
            }
            ui.label(balance_text(&row.balances, a.class));
        });
    });
}

fn trial_table(ui: &mut Ui, t: &TrialBalance) {
    if t.lines.is_empty() {
        ui.label(RichText::new(tr("Nothing has been recorded yet.")).color(MUTED));
        return;
    }
    let cell = |ui: &mut Ui, w: f32, layout: Layout, text: RichText| {
        ui.allocate_ui_with_layout(Vec2::new(w, 22.0), layout, |ui| {
            ui.set_min_width(w);
            ui.add(egui::Label::new(text).truncate());
        });
    };
    let left = Layout::left_to_right(Align::Center);
    let right = Layout::right_to_left(Align::Center);
    egui::ScrollArea::vertical().id_salt("trial").auto_shrink([false, false]).show(ui, |ui| {
        let w = ui.available_width() - 30.0;
        let (name_w, amount_w) = (w * 0.48, w * 0.26);
        egui::Grid::new("trial_balance").striped(true).num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
            let head = |s: &str| RichText::new(s.to_uppercase()).size(11.0).color(MUTED);
            cell(ui, name_w, left, head(tr("Account")));
            cell(ui, amount_w, right, head(tr("Debit")));
            cell(ui, amount_w, right, head(tr("Credit")));
            ui.end_row();
            for l in &t.lines {
                let name = if l.code.is_empty() { l.account.clone() } else { format!("{} {}", l.code, l.account) };
                cell(ui, name_w, left, RichText::new(name));
                let text = fmt_currency(l.amount.saturating_abs(), &l.currency);
                let (debit, credit) = if l.amount > 0 { (text, String::new()) } else { (String::new(), text) };
                cell(ui, amount_w, right, RichText::new(debit));
                cell(ui, amount_w, right, RichText::new(credit));
                ui.end_row();
            }
            for x in &t.totals {
                cell(ui, name_w, left, RichText::new(tr("Total")).strong());
                for c in [x.debit, x.credit] {
                    let color = if x.debit == x.credit { TEXT } else { RED };
                    cell(ui, amount_w, right, RichText::new(fmt_currency(c, &x.currency)).strong().color(color));
                }
                ui.end_row();
            }
        });
    });
}
