//! The reconciliation of an account with a bank statement: its balance against the cleared balance of the books,
//! with the difference in sight, the lines of the statement against the transactions, and the history.

use crate::gui::{App, Dialog, View, card, cut, or_report, pal, primary_button, section_title, small_caps};
use anyhow::{Result, anyhow};
use cashcraft_engine::db::{AccountKind, Cents, Transaction};
use cashcraft_engine::i18n::{tr, trf, trn};
use cashcraft_engine::importer::ParsedRow;
use cashcraft_engine::money::{fmt_date, fmt_plain, parse_amount, parse_date};
use cashcraft_engine::reconcile::{LineView, Preview, Reconciled, Reconciliation, Source, Start, Stated, Status};
use cashcraft_engine::transfers::OtherSide;
use egui::{self, Color32, Frame, Key, Margin, Modifiers, RichText, Ui};

/// What the screen keeps between frames.
#[derive(Default)]
pub struct ReconcileState {
    pub account: i64,
    /// The reconciliation being done, if there is one.
    pub preview: Option<Preview>,
    pub history: Vec<Reconciliation>,
    /// Read again at the next frame.
    pub stale: bool,
    /// The form of a new one.
    pub date: String,
    pub balance: String,
    /// The lines of a statement just imported, to reconcile with.
    pub rows: Vec<ParsedRow>,
    pub source: Option<Source>,
    pub stated: Option<Stated>,
    pub error: String,
    /// The row chosen with the keyboard: the lines first, then the transactions of the books.
    pub cursor: usize,
    /// The candidate chosen for each line, when it is not the one proposed.
    pub chosen: std::collections::HashMap<i64, i64>,
    /// The first field still has to take the keyboard.
    pub focus: bool,
}

/// A change that would touch reconciled transactions, asked again once the user says yes.
pub enum Unlock {
    Save(Box<crate::gui::TxForm>),
    Delete(i64),
    Cleared(i64, bool),
    Separate(i64, bool),
    DeleteEntry(i64),
    Turn(i64, i64, OtherSide),
}

/// What a row of the screen does with Enter or Space.
enum Act {
    Match(i64, i64),
    Unmatch(i64),
    Add(i64),
    Clear(i64, bool),
}

fn amount(c: Cents, currency: &str) -> String {
    if currency == "EUR" { cashcraft_engine::money::fmt_eur(c) } else { format!("{} {currency}", fmt_plain(c)) }
}

fn describe(t: &Transaction) -> String {
    format!("{} · {}", fmt_date(t.date), cut(&t.payee, 34))
}

impl ReconcileState {
    fn new(account: i64) -> Self {
        ReconcileState { account, stale: true, focus: true, ..Default::default() }
    }

    /// How many rows the keyboard goes through.
    fn rows(&self) -> usize {
        self.preview.as_ref().map_or(0, |p| p.lines.len() + p.books_only.len())
    }
}

impl App {
    /// The screen of an account, read again when the books changed.
    fn reconcile_state(&mut self, account: i64) -> &mut ReconcileState {
        let fresh = self.reconcile.as_ref().is_none_or(|r| r.account != account);
        if fresh {
            self.reconcile = Some(ReconcileState::new(account));
        }
        let mut toast = None;
        if let Some(r) = self.reconcile.as_mut().filter(|r| r.stale) {
            r.stale = false;
            let open = or_report(self.db.open_reconciliation(account), &mut toast);
            r.preview = open.and_then(|id| or_report(self.db.preview_reconciliation(id).map(Some), &mut toast));
            r.history = or_report(self.db.reconciliations(Some(account)), &mut toast);
            r.cursor = r.cursor.min(r.rows().saturating_sub(1));
            if r.date.is_empty() {
                r.date = fmt_date(cashcraft_engine::db::today());
            }
        }
        if toast.is_some() {
            self.toast = toast;
        }
        self.reconcile.get_or_insert_with(|| ReconcileState::new(account))
    }

    /// Opens the screen of an account with the lines of a statement just imported.
    pub fn reconcile_with(&mut self, account: i64, rows: Vec<ParsedRow>, source: Source, stated: Option<Stated>) {
        let liability = self.is_liability(account);
        let mut r = ReconcileState::new(account);
        let date = stated.as_ref().and_then(|s| s.date).or_else(|| rows.iter().map(|r| r.date).max());
        r.date = date.map(fmt_date).unwrap_or_default();
        r.balance =
            stated.as_ref().map(|s| fmt_plain(if liability { -s.balance } else { s.balance })).unwrap_or_default();
        (r.rows, r.source, r.stated) = (rows, Some(source), stated);
        self.reconcile = Some(r);
        self.view = View::Reconcile(account);
        self.selected = None;
    }

    fn is_liability(&self, account: i64) -> bool {
        self.accounts.iter().any(|a| a.id == account && matches!(a.kind, AccountKind::CreditCard | AccountKind::Loan))
    }

    fn currency_of(&self, account: i64) -> String {
        self.accounts.iter().find(|a| a.id == account).map(|a| a.currency.clone()).unwrap_or_default()
    }

    pub fn reconcile_view(&mut self, ui: &mut Ui, account: i64) {
        self.reconcile_state(account);
        let currency = self.currency_of(account);
        let liability = self.is_liability(account);
        let Some(mut st) = self.reconcile.take() else { return };
        let mut outcome: Option<Result<String>> = None;
        egui::ScrollArea::vertical().id_salt("reconcile").show(ui, |ui| {
            if st.preview.is_some() {
                outcome = self.reconcile_open(ui, &mut st, &currency);
            } else {
                outcome = self.reconcile_start(ui, &mut st, &currency, liability);
            }
            ui.add_space(14.0);
            if let Some(r) = self.reconcile_history(ui, &st, &currency) {
                outcome = Some(r);
            }
        });
        st.stale |= outcome.is_some();
        self.reconcile = Some(st);
        if let Some(r) = outcome {
            self.act(ui, r);
            if let Some(s) = self.reconcile.as_mut() {
                s.stale = true;
            }
        }
    }

    fn reconcile_start(
        &mut self,
        ui: &mut Ui,
        st: &mut ReconcileState,
        currency: &str,
        liability: bool,
    ) -> Option<Result<String>> {
        let mut start = false;
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, tr("New reconciliation"));
            ui.label(
                RichText::new(tr(
                    "Type the day and the balance of the bank statement, then tick the transactions it shows until the difference is zero.",
                ))
                .color(pal().muted),
            );
            ui.add_space(8.0);
            egui::Grid::new("reconcile_start").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
                ui.label(tr("Statement date"));
                let date =
                    ui.add(egui::TextEdit::singleline(&mut st.date).hint_text(tr("yyyy-mm-dd")).desired_width(120.0));
                if std::mem::take(&mut st.focus) {
                    date.request_focus();
                }
                ui.end_row();
                ui.label(if liability { tr("Debt on the statement") } else { tr("Balance on the statement") });
                ui.horizontal(|ui| {
                    let r = ui
                        .add(egui::TextEdit::singleline(&mut st.balance).hint_text(fmt_plain(0)).desired_width(140.0));
                    ui.label(RichText::new(currency).color(pal().muted));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        start = true;
                    }
                });
                ui.end_row();
            });
            if let Some(s) = &st.stated {
                let shown = if liability { -s.balance } else { s.balance };
                ui.label(
                    RichText::new(trf(
                        "The statement states {amount} ({label}): check it.",
                        &[("amount", &amount(shown, currency)), ("label", &s.label)],
                    ))
                    .color(pal().muted),
                );
            }
            if st.rows.is_empty() {
                ui.label(
                    RichText::new(tr(
                        "To match the lines of a statement, import it (⌘I) and choose to reconcile at the end.",
                    ))
                    .color(pal().muted)
                    .size(12.5),
                );
            } else {
                ui.label(trn(
                    st.rows.len(),
                    "{n} line of the statement will be matched with the transactions.",
                    "{n} lines of the statement will be matched with the transactions.",
                ));
            }
            if !st.error.is_empty() {
                ui.label(RichText::new(&st.error).color(pal().red));
            }
            ui.add_space(6.0);
            start |= primary_button(ui, tr("Start reconciling")).clicked();
        });
        if !start {
            return None;
        }
        let parsed = (|| -> Result<Start<'_>> {
            let date = parse_date(&st.date).ok_or_else(|| anyhow!(tr("the date is not valid (yyyy-mm-dd)")))?;
            let typed = parse_amount(&st.balance)?;
            let balance = if liability {
                typed.checked_neg().ok_or_else(|| anyhow!(tr("the amount is too large")))?
            } else {
                typed
            };
            Ok(Start { account: st.account, date, balance, source: st.source.unwrap_or(Source::Typed), rows: &st.rows })
        })();
        let done = parsed.and_then(|s| self.db.start_reconciliation(&s));
        match done {
            Ok(_) => {
                st.error.clear();
                st.rows.clear();
                st.stated = None;
                st.source = None;
                Some(Ok(tr("Reconciliation started").into()))
            }
            Err(e) => {
                st.error = format!("{e:#}");
                None
            }
        }
    }

    fn reconcile_open(&mut self, ui: &mut Ui, st: &mut ReconcileState, currency: &str) -> Option<Result<String>> {
        let p = st.preview.as_ref()?;
        let r = &p.reconciliation;
        let id = r.id;
        let mut act: Option<Act> = None;
        let mut finish = false;
        let mut exact = false;
        let mut discard = false;
        // The difference, large, above all.
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                for (label, value) in
                    [(tr("Statement balance"), r.statement_balance), (tr("Cleared in the books"), r.cleared_balance)]
                {
                    ui.vertical(|ui| {
                        small_caps(ui, label);
                        ui.label(RichText::new(amount(value, currency)).size(19.0).strong().color(pal().text));
                    });
                    ui.add_space(28.0);
                }
                ui.vertical(|ui| {
                    small_caps(ui, tr("Difference"));
                    let zero = r.difference == 0;
                    let color = if zero { pal().green } else { pal().red };
                    ui.label(RichText::new(amount(r.difference, currency)).size(30.0).strong().color(color));
                });
            });
            ui.label(
                RichText::new(trf(
                    "Statement of {date}. Started {when}.",
                    &[("date", &fmt_date(r.statement_date)), ("when", &r.started_at)],
                ))
                .color(pal().muted)
                .size(12.5),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let can = r.difference == 0;
                let b = egui::Button::new(RichText::new(tr("✔ Finish")).color(Color32::WHITE).strong()).fill(if can {
                    pal().green_fill
                } else {
                    pal().track
                });
                if ui.add_enabled(can, b).clicked() {
                    finish = true;
                }
                if p.exact_pending > 0
                    && ui
                        .button(trf("Match all exact ones ({n}) (⌘E)", &[("n", &p.exact_pending)]))
                        .on_hover_text(tr("The lines whose own imported transaction is in the books."))
                        .clicked()
                {
                    exact = true;
                }
                if ui.button(tr("Discard")).clicked() {
                    discard = true;
                }
            });
            ui.label(
                RichText::new(tr(
                    "Arrows up and down choose a row · Space or Enter: match, confirm, add or tick it · Delete: undo its match · ⌘ Enter: finish",
                ))
                .color(pal().muted)
                .size(12.0),
            );
        });
        // The keyboard.
        let rows = st.rows();
        if self.dialog.is_none() && !ui.ctx().egui_wants_keyboard_input() {
            ui.input_mut(|i| {
                if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    st.cursor = (st.cursor + 1).min(rows.saturating_sub(1));
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    st.cursor = st.cursor.saturating_sub(1);
                }
                finish |= i.consume_key(Modifiers::COMMAND, Key::Enter);
                exact |= i.consume_key(Modifiers::COMMAND, Key::E);
            });
        }
        let keys = if self.dialog.is_none() && !ui.ctx().egui_wants_keyboard_input() {
            ui.input_mut(|i| {
                let go = i.consume_key(Modifiers::NONE, Key::Space) || i.consume_key(Modifiers::NONE, Key::Enter);
                let back =
                    i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace);
                (go, back)
            })
        } else {
            (false, false)
        };
        ui.add_space(12.0);
        if !p.lines.is_empty() {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                section_title(ui, &trf("Lines of the statement ({n} to match)", &[("n", &p.unmatched_lines)]));
                ui.add_space(4.0);
                egui::Grid::new("reconcile_lines").striped(true).num_columns(6).spacing([14.0, 6.0]).show(ui, |ui| {
                    for h in [tr("Line"), tr("Date"), tr("Description"), tr("Amount"), tr("In the books"), ""] {
                        ui.label(RichText::new(h.to_uppercase()).size(11.0).color(pal().muted));
                    }
                    ui.end_row();
                    for (i, l) in p.lines.iter().enumerate() {
                        let here = st.cursor == i;
                        let mark = if here { "▶ " } else { "" };
                        let strong = |t: String| if here { RichText::new(t).strong() } else { RichText::new(t) };
                        ui.label(strong(format!("{mark}{}", l.line)));
                        ui.label(strong(fmt_date(l.date)));
                        ui.label(strong(cut(&l.payee, 40)));
                        ui.label(strong(amount(l.amount, currency)).color(crate::gui::amount_color(l.amount)));
                        let primary = line_cell(ui, l, &mut st.chosen);
                        if let Some(a) = primary.filter(|_| here && keys.0) {
                            act = Some(a);
                        }
                        if let Some(a) = line_buttons(ui, l, &st.chosen) {
                            act = Some(a);
                        }
                        if here && keys.1 && l.matched.is_some() {
                            act = Some(Act::Unmatch(l.line));
                        }
                        ui.end_row();
                    }
                });
            });
            ui.add_space(12.0);
        }
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            let title = if p.lines.is_empty() {
                tr("Transactions up to the statement date")
            } else {
                tr("In the books, on no line of the statement")
            };
            section_title(ui, title);
            ui.label(
                RichText::new(tr(
                    "Tick those the statement shows: the cleared ones count in the balance of the books.",
                ))
                .color(pal().muted)
                .size(12.5),
            );
            if p.books_only.is_empty() {
                ui.label(RichText::new(tr("None.")).color(pal().muted));
                return;
            }
            egui::Grid::new("reconcile_books").striped(true).num_columns(4).spacing([14.0, 6.0]).show(ui, |ui| {
                for (i, t) in p.books_only.iter().enumerate() {
                    let here = st.cursor == p.lines.len() + i;
                    let mut on = t.cleared;
                    let label = format!("{}{}", if here { "▶ " } else { "" }, fmt_date(t.date));
                    if ui.checkbox(&mut on, RichText::new(label)).changed() {
                        act = Some(Act::Clear(t.id, on));
                    }
                    if here && keys.0 {
                        act = Some(Act::Clear(t.id, !t.cleared));
                    }
                    ui.label(cut(&t.payee, 40));
                    ui.label(RichText::new(&t.category).color(pal().muted));
                    ui.label(RichText::new(amount(t.amount, currency)).color(crate::gui::amount_color(t.amount)));
                    ui.end_row();
                }
            });
        });
        if discard {
            Some(self.db.undo_reconciliation(id))
        } else if finish && r.difference == 0 {
            Some(self.db.finish_reconciliation(id).map(|f| {
                trn(f.reconciled, "Reconciled: {n} transaction is locked", "Reconciled: {n} transactions are locked")
            }))
        } else if exact {
            let pairs: Vec<(i64, i64)> =
                p.lines.iter().filter_map(|l| l.exact.as_ref().map(|t| (l.line, t.id))).collect();
            Some(self.db.match_lines(id, &pairs).map(|n| trn(n, "{n} line matched", "{n} lines matched")))
        } else {
            act.map(|a| match a {
                Act::Match(line, t) => self.db.match_lines(id, &[(line, t)]).map(|_| String::new()),
                Act::Unmatch(line) => self.db.unmatch_lines(id, &[line]).map(|_| String::new()),
                Act::Add(line) => self.db.add_lines(id, &[line]).map(|_| tr("Added to the books").to_string()),
                Act::Clear(t, on) => self.db.clear_for_reconciliation(id, &[t], on).map(|_| String::new()),
            })
        }
    }

    fn reconcile_history(&mut self, ui: &mut Ui, st: &ReconcileState, currency: &str) -> Option<Result<String>> {
        if st.history.iter().all(|r| r.status == Status::Open) {
            return None;
        }
        let last = st.history.iter().find(|r| matches!(r.status, Status::Finished | Status::Broken | Status::Redone));
        let mut undo = None;
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, tr("Reconciliations"));
            egui::Grid::new("reconcile_history").striped(true).num_columns(5).spacing([14.0, 6.0]).show(ui, |ui| {
                for h in [tr("Statement date"), tr("Balance"), tr("Transactions"), tr("Status"), ""] {
                    ui.label(RichText::new(h.to_uppercase()).size(11.0).color(pal().muted));
                }
                ui.end_row();
                for r in st.history.iter().filter(|r| r.status != Status::Open) {
                    ui.label(fmt_date(r.statement_date));
                    ui.label(amount(r.statement_balance, currency));
                    ui.label(r.reconciled.to_string());
                    let color = match r.status {
                        Status::Finished => pal().green,
                        Status::Broken => pal().red,
                        _ => pal().muted,
                    };
                    let mut text = r.status.label().to_string();
                    if r.status == Status::Broken && r.difference != 0 {
                        text = format!(
                            "{text} · {}",
                            trf("difference {amount}", &[("amount", &amount(r.difference, currency))])
                        );
                    }
                    ui.label(RichText::new(text).color(color));
                    if last.is_some_and(|l| l.id == r.id) && ui.small_button(tr("Undo it")).clicked() {
                        undo = Some(r.id);
                    }
                    ui.end_row();
                }
            });
        });
        undo.map(|id| self.db.undo_reconciliation(id))
    }

    /// Runs again, after the user's yes, a change that touches reconciled transactions.
    pub(crate) fn run_unlock(&self, u: &Unlock) -> Result<String> {
        let db = &self.db;
        match u {
            Unlock::Save(f) => db.unreconciling(|| self.save_tx(f)),
            Unlock::Delete(id) => {
                db.unreconciling(|| db.delete_transaction(*id)).map(|_| tr("Transaction deleted").into())
            }
            Unlock::Cleared(id, on) => db.unreconciling(|| db.set_cleared(*id, *on)).map(|_| String::new()),
            Unlock::Separate(id, keep) => {
                db.unreconciling(|| db.separate_transfer(*id, *keep)).map(|_| tr("Transfer separated").into())
            }
            Unlock::DeleteEntry(id) => db.unreconciling(|| db.delete_entry(*id)).map(|_| tr("Entry deleted").into()),
            Unlock::Turn(id, account, other) => db
                .unreconciling(|| db.turn_into_transfer(*id, *account, *other))
                .map(|_| tr("Transfer recorded").into()),
        }
    }

    /// The outcome of a change: one refused because it would change reconciled transactions asks the user instead,
    /// and gives `None`.
    pub(crate) fn unless_reconciled(
        &mut self,
        r: Result<String>,
        retry: impl FnOnce() -> Unlock,
    ) -> Option<Result<String>> {
        match r {
            Err(e) => match cashcraft_engine::reconcile::reconciled_in(&e) {
                Some(locked) => {
                    self.dialog = Some(Dialog::ConfirmUnreconcile(Box::new((locked.clone(), retry()))));
                    None
                }
                None => Some(Err(e)),
            },
            ok => Some(ok),
        }
    }

    /// The question before a change to reconciled transactions; whether the dialog stays.
    pub(crate) fn unlock_dialog(
        &mut self,
        ui: &mut Ui,
        asked: &(Reconciled, Unlock),
        message: &mut Option<Result<String>>,
    ) -> bool {
        let (locked, retry) = asked;
        ui.heading(tr("Change reconciled transactions?"));
        let ids = locked.transactions.iter().map(i64::to_string).collect::<Vec<_>>().join(", ");
        ui.label(trn(
            locked.transactions.len(),
            "This changes a transaction that was checked against a bank statement:",
            "This changes transactions that were checked against a bank statement:",
        ));
        ui.label(RichText::new(ids).strong());
        for id in &locked.reconciliations {
            if let Ok(r) = self.db.reconciliation(*id) {
                ui.label(format!("· {} · {}", r.account, fmt_date(r.statement_date)));
            }
        }
        Frame::new().fill(pal().warn_bg).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.label(
                RichText::new(tr(
                    "Its reconciliation will no longer count it and shows as broken until it is done again. Notes, payee, category and tags can be changed without this.",
                ))
                .color(pal().warn_text),
            );
        });
        ui.add_space(8.0);
        let mut keep = true;
        ui.horizontal(|ui| {
            let b = egui::Button::new(RichText::new(tr("Change it and undo its reconciliation")).color(Color32::WHITE))
                .fill(pal().red_fill);
            if ui.add(b).clicked() {
                *message = Some(self.run_unlock(retry));
                keep = false;
            }
            if ui.button(tr("Cancel")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                keep = false;
            }
        });
        keep
    }
}

/// What the books have for a line; what Enter does on it.
fn line_cell(ui: &mut Ui, l: &LineView, chosen: &mut std::collections::HashMap<i64, i64>) -> Option<Act> {
    ui.horizontal(|ui| {
        if let Some(t) = &l.matched {
            ui.label(RichText::new(format!("✔ {}", describe(t))).color(pal().green));
            return None;
        }
        if let Some(id) = l.reconciled_before {
            ui.label(RichText::new(trf("reconciled before ({id})", &[("id", &id)])).color(pal().muted));
            return None;
        }
        if let Some(t) = &l.exact {
            ui.label(RichText::new(trf("its own: {line}", &[("line", &describe(t))])).color(pal().text));
            return Some(Act::Match(l.line, t.id));
        }
        if let Some(t) = &l.proposed {
            let pick = chosen.get(&l.line).copied().unwrap_or(t.id);
            if l.candidates.len() > 1 {
                let shown = l.candidates.iter().find(|c| c.id == pick).map(describe).unwrap_or_default();
                egui::ComboBox::from_id_salt(("candidate", l.line)).selected_text(shown).width(280.0).show_ui(
                    ui,
                    |ui| {
                        for c in &l.candidates {
                            if ui.selectable_label(c.id == pick, describe(c)).clicked() {
                                chosen.insert(l.line, c.id);
                            }
                        }
                    },
                );
            } else {
                ui.label(RichText::new(trf("perhaps {line}", &[("line", &describe(t))])).color(pal().amber));
            }
            return Some(Act::Match(l.line, pick));
        }
        ui.label(RichText::new(tr("missing in the books")).color(pal().red));
        Some(Act::Add(l.line))
    })
    .inner
}

/// The buttons of a line.
fn line_buttons(ui: &mut Ui, l: &LineView, chosen: &std::collections::HashMap<i64, i64>) -> Option<Act> {
    let mut out = None;
    ui.horizontal(|ui| {
        if l.matched.is_some() {
            if ui.small_button(tr("Undo the match")).clicked() {
                out = Some(Act::Unmatch(l.line));
            }
        } else if let Some(t) = &l.exact {
            if ui.small_button(tr("Match")).clicked() {
                out = Some(Act::Match(l.line, t.id));
            }
        } else if let Some(t) = &l.proposed {
            if ui.small_button(tr("Confirm")).clicked() {
                out = Some(Act::Match(l.line, chosen.get(&l.line).copied().unwrap_or(t.id)));
            }
        } else if l.reconciled_before.is_none() && ui.small_button(tr("Add to the books")).clicked() {
            out = Some(Act::Add(l.line));
        }
    });
    out
}
