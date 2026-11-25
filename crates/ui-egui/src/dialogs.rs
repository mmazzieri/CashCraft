//! Finestre di dialogo: movimento, giroconto, conto, pianificato, importazione CSV, conferme.

use crate::gui::{
    ACCENT, AMBER, App, CardPayments, Dialog, GREEN, ImportState, MUTED, PREVIEWED, Period, RED, TurnForm, TxForm,
    View, ZEBRA, account_combo, card, category_combo, column_combo, cut, error_text, expense_toggle, form_row,
    kind_icon, money, primary_button, section_title,
};
use anyhow::Result;
use cashcraft_engine::cards::{CardStatement, PaymentProposal};
use cashcraft_engine::db::{Account, AccountKind, NewScheduled, NewTx, Transaction, TxEdit};
use cashcraft_engine::duplicates::{Choice, Decision, LikelyDuplicate};
use cashcraft_engine::i18n::{tr, trf, trn};
use cashcraft_engine::importer::{self, ImportResult, Statement};
use cashcraft_engine::layouts;
use cashcraft_engine::money::{DateOrder, fmt_date, fmt_eur, fmt_plain, parse_amount, parse_date, total};
use cashcraft_engine::ofx::Kind;
use cashcraft_engine::rules::parse_tags;
use cashcraft_engine::transfers::OtherSide;
use egui::{self, Color32, CornerRadius, Frame, Id, Margin, RichText, Ui};
use std::collections::BTreeMap;

impl App {
    pub fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else {
            return;
        };
        let mut keep = true;
        let mut message: Option<Result<String>> = None;
        let accounts = self.accounts.clone();
        let cats = self.categories.clone();
        let modal = egui::Modal::new(Id::unique("dialog")).show(ctx, |ui| {
            ui.set_min_width(480.0);
            match &mut dialog {
                Dialog::Tx(f) => {
                    ui.heading(if f.id.is_some() { tr("Edit transaction") } else { tr("New transaction") });
                    ui.add_space(8.0);
                    if f.is_transfer {
                        ui.label(RichText::new(tr("Transfer: the amount changes in both accounts.")).color(MUTED));
                    } else {
                        expense_toggle(ui, &mut f.expense);
                        ui.add_space(6.0);
                    }
                    form_row(ui, tr("Account"), |ui| {
                        if f.id.is_some() {
                            ui.label(
                                accounts
                                    .iter()
                                    .find(|a| Some(a.id) == f.account)
                                    .map(|a| a.name.clone())
                                    .unwrap_or_default(),
                            );
                        } else {
                            account_combo(ui, "txacc", &accounts, &mut f.account);
                        }
                    });
                    form_row(ui, tr("Date"), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut f.date).hint_text(tr("yyyy-mm-dd")).desired_width(120.0),
                        );
                    });
                    form_row(ui, tr("Amount €"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.amount).hint_text(fmt_plain(0)).desired_width(120.0));
                    });
                    form_row(ui, tr("Payee"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.payee).desired_width(280.0));
                    });
                    if f.is_split {
                        ui.label(
                            RichText::new(tr(
                                "Split over several accounts: amount and category cannot be changed here.",
                            ))
                            .color(MUTED),
                        );
                    }
                    if f.has_category {
                        form_row(ui, tr("Category"), |ui| {
                            f.category_chosen |=
                                category_combo(ui, "txcat", &cats, &mut f.category, tr("Automatic (rules)"));
                        });
                    }
                    form_row(ui, tr("Notes"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.memo).desired_width(280.0));
                    });
                    form_row(ui, tr("Tags"), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut f.tags)
                                .hint_text(tr("separated by commas"))
                                .desired_width(280.0),
                        );
                    });
                    form_row(ui, "", |ui| {
                        ui.checkbox(&mut f.cleared, tr("Checked against the bank statement"));
                    });
                    if !f.detail.is_empty() {
                        ui.label(RichText::new(&f.detail).color(MUTED).size(12.5));
                    }
                    if !f.error.is_empty() {
                        ui.label(RichText::new(&f.error).color(RED));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, tr("Save")).clicked() {
                            match self.save_tx(f) {
                                Ok(m) => {
                                    message = Some(Ok(m));
                                    keep = false;
                                }
                                Err(e) => f.error = format!("{e:#}"),
                            }
                        }
                        if let Some(id) = f.id
                            && f.has_category
                            && ui
                                .button(tr("🔁 Turn into transfer…"))
                                .on_hover_text(tr("What was changed here is not saved."))
                                .clicked()
                        {
                            match self.db.transaction(id) {
                                Ok(t) => {
                                    self.open_turn(&t);
                                    keep = false;
                                }
                                Err(e) => f.error = format!("{e:#}"),
                            }
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Transfer(f) => {
                    ui.heading(tr("Transfer"));
                    ui.label(
                        RichText::new(tr("Move money between two of your accounts (it does not count as an expense)."))
                            .color(MUTED),
                    );
                    ui.add_space(8.0);
                    form_row(ui, tr("From"), |ui| {
                        account_combo(ui, "trfrom", &accounts, &mut f.from);
                    });
                    form_row(ui, tr("To"), |ui| {
                        account_combo(ui, "trto", &accounts, &mut f.to);
                    });
                    form_row(ui, tr("Date"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.date).desired_width(120.0));
                    });
                    form_row(ui, tr("Amount €"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.amount).hint_text(fmt_plain(0)).desired_width(120.0));
                    });
                    form_row(ui, tr("Notes"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.memo).desired_width(280.0));
                    });
                    if !f.error.is_empty() {
                        ui.label(RichText::new(&f.error).color(RED));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, tr("Record")).clicked() {
                            let r = (|| -> Result<String> {
                                let from =
                                    f.from.ok_or_else(|| anyhow::anyhow!(tr("choose the account to transfer from")))?;
                                let to =
                                    f.to.ok_or_else(|| anyhow::anyhow!(tr("choose the account to transfer to")))?;
                                let d =
                                    parse_date(&f.date).ok_or_else(|| anyhow::anyhow!(tr("the date is not valid")))?;
                                let a = parse_amount(&f.amount)?;
                                if a == 0 {
                                    anyhow::bail!(tr("enter an amount"));
                                }
                                self.db.add_transfer(from, to, d, a, &f.memo)?;
                                Ok(tr("Transfer recorded").into())
                            })();
                            match r {
                                Ok(m) => {
                                    message = Some(Ok(m));
                                    keep = false;
                                }
                                Err(e) => f.error = format!("{e:#}"),
                            }
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::TurnIntoTransfer(f) => {
                    (keep, message) = self.turn_dialog(ui, f, &accounts);
                }
                Dialog::SeparateTransfer(t, keep_other) => {
                    ui.heading(tr("Separate the transfer?"));
                    ui.label(format!("{} · {} · {}", fmt_date(t.date), cut(&t.payee, 40), fmt_eur(t.amount)));
                    ui.label(RichText::new(tr("It becomes an ordinary transaction, without a category.")).color(MUTED));
                    ui.add_space(6.0);
                    ui.radio_value(keep_other, true, tr("Keep the other side as a transaction of its account"));
                    ui.radio_value(
                        keep_other,
                        false,
                        tr("Delete the other side (the balance of its account changes)"),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, tr("Separate")).clicked() {
                            let done = self.db.separate_transfer(t.id, *keep_other);
                            message = Some(done.map(|_| tr("Transfer separated").into()));
                            keep = false;
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Account(f) => {
                    ui.heading(if f.id.is_some() { tr("Edit account") } else { tr("New account") });
                    ui.add_space(8.0);
                    form_row(ui, tr("Name"), |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut f.name)
                                .hint_text(tr("e.g. Main account"))
                                .desired_width(260.0),
                        );
                    });
                    form_row(ui, tr("Type"), |ui| {
                        egui::ComboBox::from_id_salt("acckind")
                            .selected_text(format!("{} {}", kind_icon(f.kind), f.kind.label()))
                            .width(200.0)
                            .show_ui(ui, |ui| {
                                for k in AccountKind::ALL {
                                    ui.selectable_value(&mut f.kind, k, format!("{} {}", kind_icon(k), k.label()));
                                }
                            });
                    });
                    form_row(ui, tr("Opening balance €"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.opening).desired_width(120.0));
                    });
                    form_row(ui, tr("Currency"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.currency).hint_text("EUR").desired_width(80.0));
                    });
                    form_row(ui, "IBAN", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.iban).hint_text(tr("optional")).desired_width(260.0));
                    });
                    if f.id.is_some() {
                        form_row(ui, "", |ui| {
                            ui.checkbox(&mut f.archived, tr("Archived (hidden from the sidebar)"));
                        });
                    }
                    if !f.error.is_empty() {
                        ui.label(RichText::new(&f.error).color(RED));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, tr("Save")).clicked() {
                            let r = (|| -> Result<String> {
                                let opening = if f.opening.trim().is_empty() { 0 } else { parse_amount(&f.opening)? };
                                match f.id {
                                    Some(id) => {
                                        self.db.atomically(|| {
                                            self.db.update_account(id, &f.name, f.kind, opening, &f.iban, f.archived)?;
                                            if f.currency.trim() != f.old_currency {
                                                self.db.set_account_currency(id, &f.currency)?;
                                            }
                                            Ok(())
                                        })?;
                                        Ok(tr("Account updated").into())
                                    }
                                    None => {
                                        let id = self.db.add_account(&f.name, f.kind, &f.currency, opening, &f.iban)?;
                                        self.view = View::Register(Some(id));
                                        Ok(tr("Account created").into())
                                    }
                                }
                            })();
                            match r {
                                Ok(m) => {
                                    message = Some(Ok(m));
                                    keep = false;
                                }
                                Err(e) => f.error = format!("{e:#}"),
                            }
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Scheduled(f) => {
                    ui.heading(tr("New scheduled transaction"));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut f.transfer, false, tr("Transaction"));
                        ui.radio_value(&mut f.transfer, true, tr("Transfer"));
                    });
                    if !f.transfer {
                        expense_toggle(ui, &mut f.expense);
                    }
                    form_row(ui, if f.transfer { tr("From") } else { tr("Account") }, |ui| {
                        account_combo(ui, "schacc", &accounts, &mut f.account);
                    });
                    if f.transfer {
                        form_row(ui, tr("To"), |ui| {
                            account_combo(ui, "schto", &accounts, &mut f.to_account);
                        });
                    }
                    form_row(ui, tr("Amount €"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.amount).hint_text(fmt_plain(0)).desired_width(120.0));
                    });
                    form_row(ui, tr("Description"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.payee).desired_width(260.0));
                    });
                    if !f.transfer {
                        form_row(ui, tr("Category"), |ui| {
                            category_combo(ui, "schcat", &cats, &mut f.category, tr("None"));
                        });
                    }
                    form_row(ui, tr("First due date"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.date).desired_width(120.0));
                    });
                    form_row(ui, tr("Repeat every"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.every).desired_width(40.0));
                        let unit_label = match f.unit.as_str() {
                            "day" => tr("days"),
                            "week" => tr("weeks"),
                            "year" => tr("years"),
                            _ => tr("months"),
                        };
                        egui::ComboBox::from_id_salt("schunit").selected_text(unit_label).show_ui(ui, |ui| {
                            ui.selectable_value(&mut f.unit, "day".to_string(), tr("days"));
                            ui.selectable_value(&mut f.unit, "week".to_string(), tr("weeks"));
                            ui.selectable_value(&mut f.unit, "month".to_string(), tr("months"));
                            ui.selectable_value(&mut f.unit, "year".to_string(), tr("years"));
                        });
                    });
                    if !f.error.is_empty() {
                        ui.label(RichText::new(&f.error).color(RED));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, tr("Save")).clicked() {
                            let r = (|| -> Result<String> {
                                let acc = f.account.ok_or_else(|| anyhow::anyhow!(tr("choose the account")))?;
                                let d =
                                    parse_date(&f.date).ok_or_else(|| anyhow::anyhow!(tr("the date is not valid")))?;
                                let a = parse_amount(&f.amount)?.abs();
                                let to = if f.transfer {
                                    Some(
                                        f.to_account
                                            .ok_or_else(|| anyhow::anyhow!(tr("choose the account to transfer to")))?,
                                    )
                                } else {
                                    None
                                };
                                let amt = if f.transfer || !f.expense { a } else { -a };
                                let every: i64 = f.every.trim().parse().unwrap_or(1);
                                if f.payee.trim().is_empty() {
                                    anyhow::bail!(tr("write a description"));
                                }
                                let memo = if f.transfer { f.payee.clone() } else { String::new() };
                                let cat = if f.transfer { None } else { f.category };
                                self.db.add_scheduled(&NewScheduled {
                                    account_id: acc,
                                    to_account_id: to,
                                    amount: amt,
                                    payee: f.payee.clone(),
                                    memo,
                                    category_id: cat,
                                    next_date: d,
                                    every,
                                    unit: f.unit.clone(),
                                })?;
                                Ok(tr("Scheduled transaction added").into())
                            })();
                            match r {
                                Ok(m) => {
                                    message = Some(Ok(m));
                                    keep = false;
                                }
                                Err(e) => f.error = format!("{e:#}"),
                            }
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Rule(f) => {
                    (keep, message) = self.rule_dialog(ui, f, &accounts, &cats);
                }
                Dialog::RunRules(f) => {
                    (keep, message) = self.run_dialog(ui, f);
                }
                Dialog::ConfirmUndoRules(last) => {
                    ui.heading(tr("Undo the rules?"));
                    ui.label(last.text());
                    ui.label(tr("Transactions changed since then stay as they are."));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, tr("Undo")).clicked() {
                            message = Some(self.db.undo_run(last.id).map(|u| u.message()));
                            keep = false;
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::ConfirmDeleteCategory(id, name, rules) => {
                    ui.heading(tr("Delete the category?"));
                    ui.label(trf("The transactions of \"{name}\" stay, uncategorized.", &[("name", name)]));
                    let text = trn(*rules, "{n} rule that gives it goes too.", "{n} rules that give it go too.");
                    ui.label(RichText::new(text).color(AMBER));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let delete = egui::Button::new(RichText::new(tr("Delete")).color(Color32::WHITE)).fill(RED);
                        if ui.add(delete).clicked() {
                            message = Some(self.db.delete_category(*id).map(|_| tr("Category deleted").to_string()));
                            keep = false;
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Import(st) => {
                    let (k, m) = self.import_dialog(ui, st, &accounts);
                    keep = k;
                    if m.is_some() {
                        message = m;
                    }
                }
                Dialog::Entry(f) => {
                    (keep, message) = self.entry_dialog(ui, f);
                }
                Dialog::LedgerAccount(f) => {
                    (keep, message) = self.ledger_account_dialog(ui, f);
                }
                Dialog::RenameLedgerAccount(id, name, error) => {
                    ui.heading(tr("Rename the account"));
                    form_row(ui, tr("Name"), |ui| {
                        ui.add(egui::TextEdit::singleline(name).desired_width(260.0));
                    });
                    if !error.is_empty() {
                        ui.label(RichText::new(&*error).color(RED));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, tr("Save")).clicked() {
                            match self.db.rename_ledger_account(*id, name) {
                                Ok(()) => {
                                    message = Some(Ok(tr("Account updated").into()));
                                    keep = false;
                                }
                                Err(e) => *error = format!("{e:#}"),
                            }
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::ConfirmDeleteLedgerAccount(id, name) => {
                    ui.heading(tr("Delete the account?"));
                    ui.label(trf("\"{name}\" has no entries.", &[("name", name)]));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new(RichText::new(tr("Delete")).color(Color32::WHITE)).fill(RED))
                            .clicked()
                        {
                            message = Some(self.db.delete_ledger_account(*id).map(|_| tr("Account deleted").into()));
                            keep = false;
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::EntryDetail(id) => {
                    (keep, message) = self.entry_detail(ui, *id);
                }
                Dialog::ConfirmDeleteEntry(id, lines) => {
                    ui.heading(tr("Delete the entry?"));
                    ui.label(trn(
                        *lines,
                        "Its {n} line will be deleted. This cannot be undone.",
                        "All of its {n} lines will be deleted, in every account they are on. This cannot be undone.",
                    ));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new(RichText::new(tr("Delete")).color(Color32::WHITE)).fill(RED))
                            .clicked()
                        {
                            message = Some(self.db.delete_entry(*id).map(|_| tr("Entry deleted").into()));
                            keep = false;
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::ConfirmDeleteAccount(id, name, rules) => {
                    ui.heading(tr("Delete the account?"));
                    ui.label(trf(
                        "The account \"{name}\" and all its transactions will be deleted. This cannot be undone.",
                        &[("name", name)],
                    ));
                    ui.label(tr(
                        "The other side of its transfers stays in the other account, against the opening balances: no other balance changes.",
                    ));
                    if *rules > 0 {
                        let text = trn(*rules, "{n} rule for this account goes too.", "{n} rules for this account go too.");
                        ui.label(RichText::new(text).color(AMBER));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new(RichText::new(tr("Delete")).color(Color32::WHITE)).fill(RED))
                            .clicked()
                        {
                            message = Some(self.db.delete_account(*id).map(|_| tr("Account deleted").to_string()));
                            self.view = View::Dashboard;
                            keep = false;
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::ConfirmClearDemo => {
                    ui.heading(tr("Delete the sample data?"));
                    ui.label(tr("Sample accounts, transactions, budgets and scheduled transactions will be deleted."));
                    ui.label(tr("Categories and rules stay, so you can start right away."));
                    let of_account = |a: &Account| self.db.rules_of_account(a.id).unwrap_or(0);
                    let gone: usize = accounts.iter().map(of_account).sum();
                    if gone > 0 {
                        let text = trn(gone, "{n} rule for one account goes too.", "{n} rules for one account go too.");
                        ui.label(RichText::new(text).color(AMBER));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, tr("Delete and start")).clicked() {
                            message = Some(
                                self.db
                                    .clear_accounts()
                                    .map(|_| tr("Ready: create your first account or import a CSV").to_string()),
                            );
                            self.view = View::Dashboard;
                            keep = false;
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            keep = false;
                        }
                    });
                }
            }
        });
        // Esc o clic fuori chiudono (tranne l'importazione, per non perdere la mappatura).
        // Nor what is written in the form of an entry.
        let precious = match &dialog {
            Dialog::Import(_) => true,
            Dialog::Entry(f) => !f.is_untouched(),
            _ => false,
        };
        if modal.should_close() && !precious {
            keep = false;
        }
        if keep {
            self.dialog = Some(dialog);
        }
        if let Some(m) = message {
            let time = ctx.input(|i| i.time);
            match m {
                Ok(s) => self.toast = Some((s, time + 4.0)),
                Err(e) => self.toast = Some((error_text(&e), time + 6.0)),
            }
            self.reload();
        }
    }

    fn turn_dialog(&mut self, ui: &mut Ui, f: &mut TurnForm, accounts: &[Account]) -> (bool, Option<Result<String>>) {
        let (mut keep, mut message) = (true, None);
        ui.heading(tr("Turn into a transfer"));
        ui.label(format!(
            "{} · {} · {} · {}",
            fmt_date(f.tx.date),
            cut(&f.tx.payee, 40),
            f.tx.account,
            fmt_eur(f.tx.amount)
        ));
        ui.label(
            RichText::new(tr("Money moved between two of your accounts: it no longer counts as income or an expense."))
                .color(MUTED),
        );
        ui.add_space(8.0);
        let others: Vec<Account> = accounts.iter().filter(|a| a.id != f.tx.account_id).cloned().collect();
        form_row(ui, if f.tx.amount < 0 { tr("To account") } else { tr("From account") }, |ui| {
            if account_combo(ui, "turnacc", &others, &mut f.account) {
                f.reload(&self.db);
            }
        });
        if f.account.is_some() && f.error.is_empty() {
            ui.add_space(6.0);
            if f.candidates.is_empty() {
                ui.label(
                    RichText::new(tr("No matching transaction in that account: the other side will be added."))
                        .color(MUTED),
                );
            } else {
                ui.label(tr("The other side is already in that account?"));
                egui::ScrollArea::vertical().id_salt("turn_candidates").max_height(220.0).show(ui, |ui| {
                    for c in &f.candidates {
                        let category = if c.category.is_empty() { tr("Uncategorized") } else { &c.category };
                        let text = format!(
                            "{} · {} · {} · {}",
                            fmt_date(c.date),
                            cut(&c.payee, 44),
                            fmt_eur(c.amount),
                            cut(category, 30)
                        );
                        ui.radio_value(&mut f.other, OtherSide::Join(c.id), text);
                    }
                });
                ui.radio_value(&mut f.other, OtherSide::New, tr("None of these: add the other side"));
            }
        }
        if !f.error.is_empty() {
            ui.label(RichText::new(&f.error).color(RED));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let account = f.account.filter(|_| f.error.is_empty() && f.other != OtherSide::Auto);
            let button = egui::Button::new(RichText::new(tr("Turn into transfer")).color(Color32::WHITE).strong());
            if ui.add_enabled(account.is_some(), button.fill(ACCENT)).clicked()
                && let Some(account) = account
            {
                match self.db.turn_into_transfer(f.tx.id, account, f.other) {
                    Ok(done) => {
                        let text = if done.joined {
                            tr("The two transactions are one transfer now")
                        } else {
                            tr("Transfer recorded")
                        };
                        message = Some(Ok(text.into()));
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

    pub(crate) fn save_tx(&mut self, f: &TxForm) -> Result<String> {
        let account = f.account.ok_or_else(|| anyhow::anyhow!(tr("choose an account")))?;
        let date = parse_date(&f.date).ok_or_else(|| anyhow::anyhow!(tr("the date is not valid (yyyy-mm-dd)")))?;
        let a = parse_amount(&f.amount)?.abs();
        if a == 0 {
            anyhow::bail!(tr("enter an amount"));
        }
        let amount = if f.expense { -a } else { a };
        if f.payee.trim().is_empty() {
            anyhow::bail!(tr("write the payee or a description"));
        }
        let tags = parse_tags(&f.tags)?;
        self.db.atomically(|| match f.id {
            Some(id) => {
                if f.category_chosen {
                    // The one it has too, when it was a rule's.
                    self.db.set_category(id, f.category)?;
                }
                self.db.update_transaction(
                    id,
                    &TxEdit {
                        date,
                        amount,
                        payee: f.payee.clone(),
                        memo: f.memo.clone(),
                        category_id: f.category,
                        cleared: f.cleared,
                    },
                )?;
                if f.tags != f.old_tags {
                    self.db.set_tags(id, &tags)?;
                }
                Ok(tr("Transaction updated").into())
            }
            None => {
                let id = self.db.add_transaction(&NewTx {
                    account_id: account,
                    date,
                    amount,
                    payee: f.payee.clone(),
                    memo: f.memo.clone(),
                    category_id: f.category,
                    cleared: f.cleared,
                    import_hash: None,
                    import_marks: None,
                    card_row: false,
                })?;
                if let Some(id) = id.filter(|_| !tags.is_empty()) {
                    self.db.add_tags(id, &tags)?;
                }
                Ok(tr("Transaction recorded").into())
            }
        })
    }

    fn import_dialog(
        &mut self,
        ui: &mut Ui,
        st: &mut ImportState,
        accounts: &[Account],
    ) -> (bool, Option<Result<String>>) {
        let mut keep = true;
        let mut msg = None;
        ui.set_min_width(780.0);
        ui.heading(tr("Import a statement"));
        ui.label(
            RichText::new(tr(
                "It works with CSV, Excel 97 and OFX files exported from online banking and with the PDF statement of a \
                 credit card. Transactions already imported are skipped.",
            ))
            .color(MUTED),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(tr("📂 Choose file…")).clicked()
                && let Some(p) = rfd::FileDialog::new()
                    .add_filter(
                        "CSV, Excel, OFX, PDF",
                        &["csv", "CSV", "txt", "xls", "XLS", "ofx", "OFX", "pdf", "PDF"],
                    )
                    .pick_file()
            {
                st.path_text = p.display().to_string();
                st.load();
            }
            let r = ui.add(
                egui::TextEdit::singleline(&mut st.path_text)
                    .hint_text(tr("…or paste the path of the file"))
                    .desired_width(440.0),
            );
            if ui.button(tr("Analyze")).clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                st.load();
            }
        });
        if st.poll() {
            self.card_account(st);
        }
        if st.loading.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tr("Reading the file…"));
                if ui.button(tr("Stop")).clicked() {
                    st.loading = None;
                }
            });
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        if !st.error.is_empty() {
            ui.label(RichText::new(&st.error).color(RED));
        }
        if let Some(res) = &st.result {
            ui.add_space(10.0);
            card(ui, |ui| {
                ui.label(RichText::new(tr("✔ Import completed")).size(16.0).strong().color(GREEN));
                ui.label(trn(res.added, "{n} transaction imported", "{n} transactions imported"));
                ui.label(trf("{n} already present (skipped)", &[("n", &(res.duplicates + res.same))]));
                ui.label(trf("{n} categorized automatically by the rules", &[("n", &res.categorized)]));
                if res.skipped > 0 {
                    ui.label(
                        RichText::new(trn(
                            res.skipped,
                            "{n} row was not a transaction (balances, totals…)",
                            "{n} rows were not transactions (balances, totals…)",
                        ))
                        .color(MUTED),
                    );
                }
                for a in &res.adopted {
                    let line = format!("{} · {} · {}", fmt_date(a.date), cut(&a.payee, 44), fmt_eur(a.amount));
                    ui.label(trf("Side of a transfer that was there: {line}", &[("line", &line)]));
                }
                balance_check(ui, st);
            });
            self.card_payments(ui, &mut st.cards);
            self.maybe_transfers(ui, st);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if primary_button(ui, tr("Go to the account")).clicked() {
                    if let Some(a) = st.account {
                        self.view = View::Register(Some(a));
                        self.period = Period::All;
                        self.selected = None;
                    }
                    keep = false;
                    msg = Some(Ok(String::new()));
                }
                if ui.button(tr("Close")).clicked() {
                    keep = false;
                    msg = Some(Ok(String::new()));
                }
            });
            return (keep, msg);
        }
        if st.table.is_none() && st.loading.is_none() {
            self.card_payments(ui, &mut st.cards);
        }
        if let Err(e) = st.load_saved(&self.db) {
            st.error = format!("{e:#}");
        }
        let statement = st.table.as_ref().and_then(|t| t.statement.clone());
        if let Some(s) = &statement {
            statement_summary(ui, s, &mut st.allow_mismatch);
        } else if st.table.as_ref().is_some_and(|t| t.ofx.is_some()) {
            ofx_statements(ui, st);
        } else if let Some(t) = &st.table {
            ui.add_space(8.0);
            let comma = st.settings().comma(t);
            ui.label(
                RichText::new(trf(
                    "Encoding {encoding} · separator «{separator}» · decimals with «{decimal}» · header {header}",
                    &[
                        ("encoding", &t.encoding),
                        (
                            "separator",
                            &if t.delimiter == b'\t' { "tab".to_string() } else { (t.delimiter as char).to_string() },
                        ),
                        ("decimal", &if comma { "," } else { "." }),
                        (
                            "header",
                            &t.header_row
                                .map_or_else(|| tr("missing").to_string(), |h| trf("on row {n}", &[("n", &(h + 1))])),
                        ),
                    ],
                ))
                .color(MUTED)
                .size(12.5),
            );
            ui.add_space(4.0);
            let headers = t.headers.clone();
            let mut changed = false;
            egui::Grid::new("mapping").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label(tr("Date"));
                changed |= column_combo(ui, "m_date", &headers, &mut st.mapping.date);
                ui.label(tr("Amount (signed)"));
                changed |= column_combo(ui, "m_amount", &headers, &mut st.mapping.amount);
                ui.end_row();
                ui.label(tr("Description"));
                changed |= column_combo(ui, "m_desc", &headers, &mut st.mapping.description);
                ui.label(tr("Income / credits"));
                changed |= column_combo(ui, "m_credit", &headers, &mut st.mapping.credit);
                ui.end_row();
                ui.label(tr("Payee"));
                changed |= column_combo(ui, "m_payee", &headers, &mut st.mapping.payee);
                ui.label(tr("Expenses / debits"));
                changed |= column_combo(ui, "m_debit", &headers, &mut st.mapping.debit);
                ui.end_row();
                ui.label(tr("Details"));
                changed |= column_combo(ui, "m_extra", &headers, &mut st.mapping.extra);
                ui.label("");
                let sign = ui.checkbox(&mut st.invert, tr("Invert the sign (credit cards)")).changed();
                st.sign_by_hand |= sign;
                changed |= sign;
                ui.end_row();
                ui.label(tr("Dates"));
                egui::ComboBox::from_id_salt("m_dates").selected_text(st.dates.label()).width(190.0).show_ui(
                    ui,
                    |ui| {
                        for o in DateOrder::ALL {
                            changed |= ui.selectable_value(&mut st.dates, o, o.label()).changed();
                        }
                    },
                );
                ui.label(tr("Decimals"));
                let label = |d| match d {
                    None => tr("automatic"),
                    Some(true) => ",",
                    Some(false) => ".",
                };
                let now = label(st.decimal_comma);
                egui::ComboBox::from_id_salt("m_decimals").selected_text(now).width(190.0).show_ui(ui, |ui| {
                    for d in [None, Some(true), Some(false)] {
                        changed |= ui.selectable_value(&mut st.decimal_comma, d, label(d)).changed();
                    }
                });
                ui.end_row();
            });
            let detected = importer::Settings::detected(t);
            if st.broken.is_some() {
                ui.label(RichText::new(tr("The columns remembered for this layout cannot be read")).color(RED));
            }
            if let Some(id) = st.saved.or(st.broken) {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(tr("Columns remembered from the last import of this layout"))
                            .color(MUTED)
                            .size(12.5),
                    );
                    if ui.button(tr("Forget them")).clicked() {
                        match self.db.forget_layout(id) {
                            Ok(()) => {
                                st.use_settings(detected, None);
                                st.loaded_for = None;
                                changed = true;
                            }
                            Err(e) => st.error = format!("{e:#}"),
                        }
                    }
                });
            }
            if changed {
                st.reparse();
            }
        }
        if st.table.is_some() {
            ui.add_space(8.0);
            let total = total(st.rows.iter().map(|r| r.amount));
            let mut summary = trn(st.rows.len(), "{n} transaction found", "{n} transactions found");
            if st.skipped > 0 {
                summary.push(' ');
                summary.push_str(&trn(st.skipped, "({n} row skipped)", "({n} rows skipped)"));
            }
            summary.push_str(" · ");
            summary.push_str(&trf("sum {amount}", &[("amount", &fmt_eur(total))]));
            if let (Some(a), Some(b)) = (st.rows.first(), st.rows.last()) {
                summary.push_str(" · ");
                summary
                    .push_str(&trf("from {from} to {to}", &[("from", &fmt_date(a.date)), ("to", &fmt_date(b.date))]));
            }
            ui.label(RichText::new(summary).strong());
            problems(ui, st);
            notices(ui, st, accounts);
            Frame::new().fill(ZEBRA).corner_radius(CornerRadius::same(6)).inner_margin(Margin::same(8)).show(
                ui,
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("import_preview")
                        .max_height(200.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            egui::Grid::new("preview").striped(true).num_columns(6).spacing([14.0, 4.0]).show(
                                ui,
                                |ui| {
                                    for (i, r) in st.rows.iter().enumerate().rev().take(PREVIEWED) {
                                        ui.label(RichText::new((i + 1).to_string()).color(MUTED).size(12.0));
                                        ui.label(RichText::new(fmt_date(r.date)).color(MUTED));
                                        ui.label(cut(&r.payee, 38));
                                        ui.label(RichText::new(cut(&r.memo, 40)).color(MUTED).size(12.0));
                                        ui.label(money(r.amount));
                                        if let Some((what, why)) = st.ruled.get(&i) {
                                            ui.label(RichText::new(cut(what, 34)).color(ACCENT).size(12.0))
                                                .on_hover_text(why);
                                        }
                                        ui.end_row();
                                    }
                                },
                            );
                        });
                },
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(tr("Import into the account:"));
                account_combo(ui, "impacc", accounts, &mut st.account);
                ui.label(tr("or into a new account:"));
                ui.add(egui::TextEdit::singleline(&mut st.new_account).hint_text(tr("name")).desired_width(150.0));
            });
            let target = accounts.iter().find(|a| Some(a.id) == st.account);
            if statement.is_some()
                && st.new_account.trim().is_empty()
                && target.is_some_and(|a| a.kind != AccountKind::CreditCard)
            {
                ui.label(RichText::new(tr("⚠ This account is not a credit card.")).color(AMBER));
            }
            if let Err(e) = st.compare(&self.db).and_then(|_| st.look_at_rules(&self.db)) {
                st.error = format!("{e:#}");
            }
            if let Some(a) = target.filter(|_| st.new_account.trim().is_empty()) {
                ofx_account(ui, st, a);
            }
            if !st.broken_rules.is_empty() {
                ui.label(RichText::new(&st.broken_rules).color(RED));
            }
            balance_check(ui, st);
            likely_duplicates(ui, st);
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let can = st.can_import();
            let btn = egui::Button::new(
                RichText::new(trn(st.rows.len(), "Import {n} transaction", "Import {n} transactions"))
                    .color(Color32::WHITE)
                    .strong(),
            )
            .fill(ACCENT);
            if ui.add_enabled(can, btn).clicked() {
                let r = self.db.atomically(|| -> Result<(i64, ImportResult)> {
                    let acc = if !st.new_account.trim().is_empty() {
                        let ofx = st.table.as_ref().and_then(|t| t.ofx.as_ref()).and_then(|o| o.statement());
                        let kind = match ofx.map(|s| s.kind) {
                            Some(Kind::CreditCard) => AccountKind::CreditCard,
                            Some(Kind::Savings) => AccountKind::Savings,
                            Some(Kind::Bank) => AccountKind::Bank,
                            None if statement.is_some() => AccountKind::CreditCard,
                            None => AccountKind::Bank,
                        };
                        let currency = ofx.map_or("EUR", |s| s.currency.as_str());
                        self.db.add_account(st.new_account.trim(), kind, currency, 0, "")?
                    } else {
                        st.account.ok_or_else(|| anyhow::anyhow!(tr("choose an account")))?
                    };
                    let table = st.table.as_ref().ok_or_else(|| anyhow::anyhow!(tr("choose a file")))?;
                    let settings = st.settings();
                    let choices = importer::Choices {
                        allow_mismatch: st.allow_mismatch,
                        likely: Decision { each: st.decided.clone(), ..Default::default() },
                        settings: st.convincing.then_some(&settings),
                        sign_confirmed: st.sign_by_hand,
                    };
                    let res = importer::import_with(&self.db, acc, table, &st.rows, st.skipped, &choices)?;
                    Ok((acc, res))
                });
                match r {
                    Ok((acc, res)) => {
                        st.account = Some(acc);
                        st.cards.reload(&self.db);
                        st.result = Some(res);
                        st.new_account.clear();
                        if let Err(e) = st.read_balance(&self.db) {
                            st.error = format!("{e:#}");
                        }
                        self.reload();
                    }
                    Err(e) => {
                        st.error = format!("{e:#}");
                        // The books may have changed meanwhile.
                        st.compared = None;
                    }
                }
            }
            if ui.button(tr("Cancel")).clicked() {
                keep = false;
            }
        });
        (keep, msg)
    }

    /// Imported lines that may be the side of a transfer that is there.
    fn maybe_transfers(&mut self, ui: &mut Ui, st: &mut ImportState) {
        let Some(res) = st.result.as_mut().filter(|r| !r.maybe_transfers.is_empty()) else { return };
        let mut joined: Option<(usize, Result<()>)> = None;
        ui.add_space(8.0);
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(tr("Sides of transfers that are there already?")).strong());
            egui::ScrollArea::vertical().id_salt("maybe_transfers").max_height(160.0).show(ui, |ui| {
                for (i, m) in res.maybe_transfers.iter().enumerate() {
                    let t = &m.transaction;
                    ui.label(format!("{} · {} · {}", fmt_date(t.date), cut(&t.payee, 44), fmt_eur(t.amount)));
                    for c in &m.candidates {
                        ui.horizontal(|ui| {
                            if ui.button(tr("Join")).clicked() {
                                let done = self.db.turn_into_transfer(t.id, c.account_id, OtherSide::Join(c.id));
                                joined = Some((i, done.map(|_| ())));
                            }
                            ui.label(format!("{} · {} · {}", fmt_date(c.date), c.account, cut(&c.payee, 40)));
                        });
                    }
                }
            });
        });
        match joined {
            Some((i, Ok(()))) => {
                res.maybe_transfers.remove(i);
                self.reload();
            }
            Some((_, Err(e))) => st.error = format!("{e:#}"),
            None => {}
        }
    }

    /// The card statements without a payment, with what may pay them, and the latest ones that have it.
    fn card_payments(&mut self, ui: &mut Ui, cards: &mut CardPayments) {
        let pending: Vec<&PaymentProposal> = cards.proposals.iter().filter(|p| !p.candidates.is_empty()).collect();
        let linked: Vec<&CardStatement> = cards.statements.iter().filter(|s| s.payment_id.is_some()).collect();
        if pending.is_empty() && linked.is_empty() && cards.error.is_empty() {
            return;
        }
        let mut done: Option<Result<()>> = None;
        ui.add_space(8.0);
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            section_title(ui, tr("Card payments"));
            egui::ScrollArea::vertical().id_salt("card_payments").max_height(180.0).show(ui, |ui| {
                for p in pending {
                    ui.label(statement_text(&p.statement));
                    ui.label(
                        RichText::new(tr(
                            "Is it one of these bank transactions? It becomes a transfer, not an expense.",
                        ))
                        .color(MUTED)
                        .size(12.5),
                    );
                    for c in &p.candidates {
                        let t = &c.transaction;
                        ui.horizontal(|ui| {
                            if ui.button(tr("Link")).clicked() {
                                let join = cards.join.get(&t.id).copied();
                                done = Some(self.db.link_card_payment(p.statement.id, t.id, join));
                            }
                            ui.label(format!("{} · {} · {}", fmt_date(t.date), t.account, cut(&t.payee, 44)));
                            ui.label(money(t.amount));
                        });
                        if c.card_lines.is_empty() {
                            continue;
                        }
                        let mut chosen = cards.join.get(&t.id).copied();
                        ui.indent(("card_lines", t.id), |ui| {
                            ui.radio_value(&mut chosen, None, tr("Add the side of the card"));
                            for l in &c.card_lines {
                                ui.radio_value(&mut chosen, Some(l.id), join_text(l));
                            }
                        });
                        match chosen {
                            Some(l) => cards.join.insert(t.id, l),
                            None => cards.join.remove(&t.id),
                        };
                    }
                }
                for s in linked {
                    ui.horizontal(|ui| {
                        if s.matches {
                            ui.label(RichText::new("✔").color(GREEN));
                        } else {
                            ui.label(RichText::new("⚠").color(AMBER))
                                .on_hover_text(tr("The payment has another amount"));
                        }
                        ui.label(statement_text(s));
                        if !s.matches {
                            ui.label(RichText::new(tr("The payment has another amount")).color(AMBER));
                        }
                        if cards.unlinking != Some(s.id) {
                            if ui.button(tr("Unlink")).clicked() {
                                cards.unlinking = Some(s.id);
                            }
                            return;
                        }
                        ui.label(tr("The bank's line becomes an uncategorized expense."));
                        if ui.button(RichText::new(tr("Unlink")).color(RED)).clicked() {
                            done = Some(self.db.unlink_card_payment(s.id));
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            cards.unlinking = None;
                        }
                    });
                }
            });
            if !cards.error.is_empty() {
                ui.label(RichText::new(&cards.error).color(RED));
            }
        });
        if let Some(r) = done {
            cards.reload(&self.db);
            if let Err(e) = r {
                cards.error = format!("{e:#}");
            }
            self.reload();
        }
    }
}

/// A line of the card that may be the payment.
fn join_text(t: &Transaction) -> String {
    let category = if t.category.is_empty() { tr("Uncategorized") } else { &t.category };
    let line = format!("{} · {} · {} · {category}", fmt_date(t.date), cut(&t.payee, 40), fmt_eur(t.amount));
    trf("It is this one: {line}", &[("line", &line)])
}

fn statement_text(s: &CardStatement) -> String {
    trf(
        "{account}, statement of {closing}: {amount} debited on {debit}",
        &[
            ("account", &s.account),
            ("closing", &fmt_date(s.closing_date)),
            ("amount", &fmt_eur(s.amount)),
            ("debit", &fmt_date(s.debit_date)),
        ],
    )
}

/// The statements of an OFX file, for the user to choose when there are several.
fn ofx_statements(ui: &mut Ui, st: &mut ImportState) {
    let Some(o) = st.table.as_mut().and_then(|t| t.ofx.as_mut()) else { return };
    ui.add_space(8.0);
    if o.statements.len() > 1 {
        ui.label(RichText::new(tr("The file has several statements: choose one")).strong());
    }
    let before = o.chosen;
    egui::ScrollArea::vertical().id_salt("ofx_statements").max_height(110.0).show(ui, |ui| {
        for (i, s) in o.statements.iter().enumerate() {
            let kind = match s.kind {
                Kind::Bank => tr("Bank account"),
                Kind::Savings => tr("Savings account"),
                Kind::CreditCard => tr("Credit card"),
            };
            let range = match (s.first_date(), s.last_date()) {
                (Some(a), Some(b)) => trf("from {from} to {to}", &[("from", &fmt_date(a)), ("to", &fmt_date(b))]),
                _ => String::new(),
            };
            let count = trn(s.rows.len(), "{n} transaction", "{n} transactions");
            let text = format!("{kind} {} · {} · {count} {range}", s.account_label(), s.currency);
            ui.radio_value(&mut o.chosen, Some(i), text);
        }
    });
    if o.chosen != before {
        st.sign_by_hand = false;
        st.invert = false;
        st.reparse();
    }
    if !st.signs_contradict() {
        if ui.checkbox(&mut st.invert, tr("Invert the sign (credit cards)")).changed() {
            st.sign_by_hand = true;
            st.reparse();
        }
        return;
    }
    let text = tr("The signs of the amounts contradict the types of the transactions: which are right?");
    ui.label(RichText::new(text).color(RED));
    let mut sign = st.sign_by_hand.then_some(st.invert);
    ui.horizontal(|ui| {
        ui.radio_value(&mut sign, Some(false), tr("as in the file"));
        ui.radio_value(&mut sign, Some(true), tr("inverted"));
    });
    if sign != st.sign_by_hand.then_some(st.invert) {
        st.sign_by_hand = true;
        st.invert = sign == Some(true);
        st.reparse();
    }
}

/// Whether an OFX statement can go into an account.
fn ofx_account(ui: &mut Ui, st: &ImportState, account: &Account) {
    let Some(s) = st.table.as_ref().and_then(|t| t.ofx.as_ref()).and_then(|o| o.statement()) else { return };
    if s.currency != account.currency {
        let text = trf(
            "the statement is in {statement} and the account is in {account}: nothing was imported",
            &[("statement", &s.currency), ("account", &account.currency)],
        );
        ui.label(RichText::new(text).color(RED));
    }
    if s.kind == Kind::CreditCard && account.kind != AccountKind::CreditCard {
        ui.label(RichText::new(tr("⚠ This account is not a credit card.")).color(AMBER));
    }
}

/// The balance that the bank states, against the books. Only a check.
fn balance_check(ui: &mut Ui, st: &ImportState) {
    let statement = st.table.as_ref().and_then(|t| t.ofx.as_ref()).and_then(|o| o.statement());
    let Some((stated, on)) = statement.and_then(|s| s.balance) else { return };
    let books = st.books.map_or_else(|| "—".to_string(), fmt_plain);
    let date = on.map_or_else(|| "—".to_string(), fmt_date);
    let text = trf(
        "Balance on {date}: {stated} on the statement, {books} in the books",
        &[("date", &date), ("stated", &fmt_plain(stated)), ("books", &books)],
    );
    let mark =
        if st.books == Some(stated) { RichText::new("✔").color(GREEN) } else { RichText::new("·").color(MUTED) };
    ui.horizontal(|ui| {
        ui.label(mark);
        ui.label(RichText::new(text).color(MUTED));
    });
}

/// The lines that look like transactions and could not be read.
fn problems(ui: &mut Ui, st: &ImportState) {
    if st.problems.is_empty() {
        return;
    }
    let title = trn(st.problems.len(), "{n} line could not be read", "{n} lines could not be read");
    egui::CollapsingHeader::new(RichText::new(title).color(AMBER)).id_salt("problems").show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("problem_list").max_height(90.0).show(ui, |ui| {
            for p in &st.problems {
                ui.label(trf("line {line}: {error}", &[("line", &p.line), ("error", &p.error)]));
            }
        });
    });
}

/// What the file is read with and may not be expected, and what the bank takes back.
fn notices(ui: &mut Ui, st: &ImportState, accounts: &[Account]) {
    let Some(t) = &st.table else { return };
    let account = accounts.iter().find(|a| Some(a.id) == st.target());
    let remembered = account.filter(|_| st.saved.is_some() && !st.sign_by_hand).map(|a| a.name.as_str());
    for n in layouts::notices(t, &st.settings(), remembered) {
        ui.label(RichText::new(format!("⚠ {n}")).color(AMBER));
    }
    for d in &st.deletions {
        let e = &d.existing;
        let line = format!("{} · {} · {}", fmt_date(e.date), cut(&e.payee, 40), fmt_plain(e.amount));
        let text = trf("the bank deletes a transaction, do it by hand if it is right: {line}", &[("line", &line)]);
        ui.label(RichText::new(format!("⚠ {text}")).color(AMBER));
    }
}

/// The rows that may be in the books already: the user says what each is. Only those that are surely the same
/// can be skipped together.
fn likely_duplicates(ui: &mut Ui, st: &mut ImportState) {
    if st.likely.is_empty() {
        return;
    }
    ui.add_space(8.0);
    card(ui, |ui| {
        ui.set_width(ui.available_width());
        let left = st.likely.iter().filter(|l| !st.decided.contains_key(&l.index)).count();
        ui.label(RichText::new(tr("Are these there already? Say it for each one")).strong());
        ui.label(RichText::new(trn(left, "{n} is left", "{n} are left")).color(MUTED).size(12.0));
        egui::ScrollArea::vertical().id_salt("likely").max_height(220.0).show(ui, |ui| {
            for certain in [true, false] {
                let group: Vec<&LikelyDuplicate> = st.likely.iter().filter(|l| l.certain == certain).collect();
                if group.is_empty() {
                    continue;
                }
                ui.horizontal(|ui| {
                    let (title, button, choice): (_, _, fn(&LikelyDuplicate) -> Choice) = if certain {
                        (tr("The same date, amount and payee"), tr("All the same: skip"), |l| Choice::Skip(l.proposed))
                    } else {
                        (tr("To look at one by one"), tr("All others: import"), |_| Choice::Add)
                    };
                    ui.label(RichText::new(title).strong());
                    if ui.button(button).clicked() {
                        st.decided.extend(group.iter().map(|l| (l.index, choice(l))));
                    }
                });
                for l in group {
                    likely_row(ui, l, &mut st.decided);
                }
            }
        });
    });
}

fn likely_row(ui: &mut Ui, l: &LikelyDuplicate, decided: &mut BTreeMap<usize, Choice>) {
    let mut said = decided.get(&l.index).copied();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{}.", l.index + 1)).color(MUTED));
        ui.label(format!("{} · {}", fmt_date(l.date), cut(&l.payee, 34)));
        ui.label(money(l.amount));
        ui.radio_value(&mut said, Some(Choice::Add), tr("another: import"));
        if l.existing.is_empty() {
            ui.radio_value(&mut said, Some(Choice::Skip(None)), tr("the same: skip"));
        }
    });
    ui.indent(("likely", l.index), |ui| {
        if !l.certain {
            ui.label(RichText::new(l.reason.label()).color(MUTED).size(12.0));
        }
        for t in &l.existing {
            let line = format!("{} · {} · {}", fmt_date(t.date), cut(&t.payee, 44), fmt_plain(t.amount));
            let text = trf("skip, it is this: {line}", &[("line", &line)]);
            let text = if l.proposed == Some(t.id) { RichText::new(text).strong() } else { RichText::new(text) };
            ui.radio_value(&mut said, Some(Choice::Skip(Some(t.id))), text);
        }
    });
    if let Some(s) = said {
        decided.insert(l.index, s);
    }
}

/// What a card statement says of itself, against what was read.
fn statement_summary(ui: &mut Ui, s: &Statement, allow_mismatch: &mut bool) {
    let day = |d: Option<chrono::NaiveDate>| d.map_or_else(|| "—".to_string(), fmt_date);
    ui.add_space(8.0);
    ui.label(
        RichText::new(trf(
            "Card statement {cards}: up to {closing}, debited on {debit}",
            &[("cards", &s.cards.join(", ")), ("closing", &day(s.closing)), ("debit", &day(s.debit_date))],
        ))
        .strong(),
    );
    for c in &s.checks {
        let said = c.expected.map_or_else(|| "—".to_string(), fmt_eur);
        let text = trf(
            "{what}: {said} on the statement, {read} read",
            &[("what", &c.label()), ("said", &said), ("read", &fmt_eur(c.found))],
        );
        ui.horizontal(|ui| {
            ui.label(if c.ok() { RichText::new("✔").color(GREEN) } else { RichText::new("✖").color(RED) });
            ui.label(text);
        });
    }
    if s.unread > 0 {
        ui.label(RichText::new(trn(s.unread, "{n} row could not be read", "{n} rows could not be read")).color(RED));
    }
    if s.debit_date.is_none() || s.debit.is_none() {
        ui.label(RichText::new(tr("The debit was not found: its payment will not be proposed.")).color(AMBER));
    }
    if !s.is_identified() {
        let text = tr("The date of the statement or the number of the card was not read: it cannot be imported.");
        ui.label(RichText::new(text).color(RED));
    } else if !s.is_sound() {
        ui.checkbox(allow_mismatch, tr("The totals do not match: import the rows all the same"));
    }
}
