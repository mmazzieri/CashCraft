//! Finestre di dialogo: movimento, giroconto, conto, pianificato, importazione CSV, conferme.

use crate::gui::{
    ACCENT, App, Dialog, GREEN, ImportState, MUTED, Period, RED, TurnForm, TxForm, View, ZEBRA, account_combo, card,
    category_combo, column_combo, cut, error_text, expense_toggle, form_row, kind_icon, money, primary_button,
};
use anyhow::Result;
use cashcraft_engine::db::{Account, AccountKind, NewScheduled, NewTx, TxEdit};
use cashcraft_engine::i18n::{tr, trf, trn};
use cashcraft_engine::importer::{self, ImportResult};
use cashcraft_engine::money::{fmt_date, fmt_eur, fmt_plain, parse_amount, parse_date, total};
use cashcraft_engine::transfers::OtherSide;
use egui::{self, Color32, CornerRadius, Frame, Id, Margin, RichText, Ui};

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
                            category_combo(ui, "txcat", &cats, &mut f.category, tr("Automatic (rules)"));
                        });
                    }
                    form_row(ui, tr("Notes"), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.memo).desired_width(280.0));
                    });
                    form_row(ui, "", |ui| {
                        ui.checkbox(&mut f.cleared, tr("Checked against the bank statement"));
                    });
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
                Dialog::ConfirmDeleteAccount(id, name) => {
                    ui.heading(tr("Delete the account?"));
                    ui.label(trf(
                        "The account \"{name}\" and all its transactions will be deleted. This cannot be undone.",
                        &[("name", name)],
                    ));
                    ui.label(tr(
                        "The other side of its transfers stays in the other account, against the opening balances: no other balance changes.",
                    ));
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

    fn save_tx(&mut self, f: &TxForm) -> Result<String> {
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
        match f.id {
            Some(id) => {
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
                Ok(tr("Transaction updated").into())
            }
            None => {
                self.db.add_transaction(&NewTx {
                    account_id: account,
                    date,
                    amount,
                    payee: f.payee.clone(),
                    memo: f.memo.clone(),
                    category_id: f.category,
                    cleared: f.cleared,
                    import_hash: None,
                })?;
                Ok(tr("Transaction recorded").into())
            }
        }
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
        ui.heading(tr("Import a CSV statement"));
        ui.label(
            RichText::new(tr(
                "It works with CSV files exported from online banking: columns, separator, dates and Italian amounts \
                 are recognized. Transactions already imported are skipped.",
            ))
            .color(MUTED),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(tr("📂 Choose file…")).clicked()
                && let Some(p) =
                    rfd::FileDialog::new().add_filter("CSV, Excel", &["csv", "CSV", "txt", "xls", "XLS"]).pick_file()
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
        if !st.error.is_empty() {
            ui.label(RichText::new(&st.error).color(RED));
        }
        if let Some(res) = &st.result {
            ui.add_space(10.0);
            card(ui, |ui| {
                ui.label(RichText::new(tr("✔ Import completed")).size(16.0).strong().color(GREEN));
                ui.label(trn(res.added, "{n} transaction imported", "{n} transactions imported"));
                ui.label(trf("{n} already present (skipped)", &[("n", &res.duplicates)]));
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
            });
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
        if let Some(t) = &st.table {
            ui.add_space(8.0);
            ui.label(
                RichText::new(trf(
                    "Encoding {encoding} · separator «{separator}» · decimals with «{decimal}» · header {header}",
                    &[
                        ("encoding", &t.encoding),
                        (
                            "separator",
                            &if t.delimiter == b'\t' { "tab".to_string() } else { (t.delimiter as char).to_string() },
                        ),
                        ("decimal", &if t.decimal_comma { "," } else { "." }),
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
                changed |= ui.checkbox(&mut st.invert, tr("Invert the sign (credit cards)")).changed();
                ui.end_row();
            });
            if changed {
                st.reparse();
            }
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
            Frame::new().fill(ZEBRA).corner_radius(CornerRadius::same(6)).inner_margin(Margin::same(8)).show(
                ui,
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("import_preview")
                        .max_height(200.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            egui::Grid::new("preview").striped(true).num_columns(4).spacing([14.0, 4.0]).show(
                                ui,
                                |ui| {
                                    for r in st.rows.iter().rev().take(50) {
                                        ui.label(RichText::new(fmt_date(r.date)).color(MUTED));
                                        ui.label(cut(&r.payee, 38));
                                        ui.label(RichText::new(cut(&r.memo, 40)).color(MUTED).size(12.0));
                                        ui.label(money(r.amount));
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
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let can = !st.rows.is_empty() && (st.account.is_some() || !st.new_account.trim().is_empty());
            let btn = egui::Button::new(
                RichText::new(trn(st.rows.len(), "Import {n} transaction", "Import {n} transactions"))
                    .color(Color32::WHITE)
                    .strong(),
            )
            .fill(ACCENT);
            if ui.add_enabled(can, btn).clicked() {
                let r = (|| -> Result<ImportResult> {
                    let acc = if !st.new_account.trim().is_empty() {
                        self.db.add_account(st.new_account.trim(), AccountKind::Bank, "EUR", 0, "")?
                    } else {
                        st.account.ok_or_else(|| anyhow::anyhow!(tr("choose an account")))?
                    };
                    st.account = Some(acc);
                    importer::import_rows(&self.db, acc, &st.rows, st.skipped)
                })();
                match r {
                    Ok(res) => {
                        st.result = Some(res);
                        st.new_account.clear();
                        self.reload();
                    }
                    Err(e) => st.error = format!("{e:#}"),
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
}
