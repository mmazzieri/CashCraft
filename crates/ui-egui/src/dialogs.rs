//! Finestre di dialogo: movimento, giroconto, conto, pianificato, importazione CSV, conferme.

use crate::gui::{
    ACCENT, App, Dialog, GREEN, ImportState, MUTED, Period, RED, TxForm, View, ZEBRA, account_combo, card,
    category_combo, column_combo, cut, expense_toggle, form_row, kind_icon, money, primary_button,
};
use anyhow::Result;
use cashcraft_engine::db::{Account, AccountKind, Cents, NewScheduled, NewTx, TxEdit};
use cashcraft_engine::importer::{self, ImportResult};
use cashcraft_engine::money::{fmt_date, fmt_eur, parse_amount, parse_date};
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
                    ui.heading(if f.id.is_some() { "Modifica movimento" } else { "Nuovo movimento" });
                    ui.add_space(8.0);
                    if f.is_transfer {
                        ui.label(RichText::new("Giroconto: le modifiche valgono per entrambi i conti.").color(MUTED));
                    } else {
                        expense_toggle(ui, &mut f.expense);
                        ui.add_space(6.0);
                    }
                    form_row(ui, "Conto", |ui| {
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
                    form_row(ui, "Data", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.date).hint_text("gg/mm/aaaa").desired_width(120.0));
                    });
                    form_row(ui, "Importo €", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.amount).hint_text("0,00").desired_width(120.0));
                    });
                    form_row(ui, "Beneficiario", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.payee).desired_width(280.0));
                    });
                    if !f.is_transfer {
                        form_row(ui, "Categoria", |ui| {
                            category_combo(ui, "txcat", &cats, &mut f.category, "Automatica (regole)");
                        });
                    }
                    form_row(ui, "Note", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.memo).desired_width(280.0));
                    });
                    form_row(ui, "", |ui| {
                        ui.checkbox(&mut f.cleared, "Verificato sull'estratto conto");
                    });
                    if !f.error.is_empty() {
                        ui.label(RichText::new(&f.error).color(RED));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, "Salva").clicked() {
                            match self.save_tx(f) {
                                Ok(m) => {
                                    message = Some(Ok(m));
                                    keep = false;
                                }
                                Err(e) => f.error = format!("{e:#}"),
                            }
                        }
                        if ui.button("Annulla").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Transfer(f) => {
                    ui.heading("Giroconto");
                    ui.label(RichText::new("Sposta denaro tra due tuoi conti (non conta come spesa).").color(MUTED));
                    ui.add_space(8.0);
                    form_row(ui, "Da", |ui| {
                        account_combo(ui, "trfrom", &accounts, &mut f.from);
                    });
                    form_row(ui, "A", |ui| {
                        account_combo(ui, "trto", &accounts, &mut f.to);
                    });
                    form_row(ui, "Data", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.date).desired_width(120.0));
                    });
                    form_row(ui, "Importo €", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.amount).hint_text("0,00").desired_width(120.0));
                    });
                    form_row(ui, "Note", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.memo).desired_width(280.0));
                    });
                    if !f.error.is_empty() {
                        ui.label(RichText::new(&f.error).color(RED));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, "Registra").clicked() {
                            let r = (|| -> Result<String> {
                                let from = f.from.ok_or_else(|| anyhow::anyhow!("scegli il conto di origine"))?;
                                let to = f.to.ok_or_else(|| anyhow::anyhow!("scegli il conto di destinazione"))?;
                                let d = parse_date(&f.date).ok_or_else(|| anyhow::anyhow!("data non valida"))?;
                                let a = parse_amount(&f.amount)?;
                                if a == 0 {
                                    anyhow::bail!("inserisci un importo");
                                }
                                self.db.add_transfer(from, to, d, a, &f.memo)?;
                                Ok("Giroconto registrato".into())
                            })();
                            match r {
                                Ok(m) => {
                                    message = Some(Ok(m));
                                    keep = false;
                                }
                                Err(e) => f.error = format!("{e:#}"),
                            }
                        }
                        if ui.button("Annulla").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Account(f) => {
                    ui.heading(if f.id.is_some() { "Modifica conto" } else { "Nuovo conto" });
                    ui.add_space(8.0);
                    form_row(ui, "Nome", |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut f.name)
                                .hint_text("es. Conto principale")
                                .desired_width(260.0),
                        );
                    });
                    form_row(ui, "Tipo", |ui| {
                        egui::ComboBox::from_id_salt("acckind")
                            .selected_text(format!("{} {}", kind_icon(f.kind), f.kind.label()))
                            .width(200.0)
                            .show_ui(ui, |ui| {
                                for k in AccountKind::ALL {
                                    ui.selectable_value(&mut f.kind, k, format!("{} {}", kind_icon(k), k.label()));
                                }
                            });
                    });
                    form_row(ui, "Saldo iniziale €", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.opening).desired_width(120.0));
                    });
                    form_row(ui, "IBAN", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.iban).hint_text("facoltativo").desired_width(260.0));
                    });
                    if f.id.is_some() {
                        form_row(ui, "", |ui| {
                            ui.checkbox(&mut f.archived, "Archiviato (nascosto dalla barra laterale)");
                        });
                    }
                    if !f.error.is_empty() {
                        ui.label(RichText::new(&f.error).color(RED));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, "Salva").clicked() {
                            let r = (|| -> Result<String> {
                                let opening = if f.opening.trim().is_empty() { 0 } else { parse_amount(&f.opening)? };
                                match f.id {
                                    Some(id) => {
                                        self.db.update_account(id, &f.name, f.kind, opening, &f.iban, f.archived)?;
                                        Ok("Conto aggiornato".into())
                                    }
                                    None => {
                                        let id = self.db.add_account(&f.name, f.kind, "EUR", opening, &f.iban)?;
                                        self.view = View::Register(Some(id));
                                        Ok("Conto creato".into())
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
                        if ui.button("Annulla").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::Scheduled(f) => {
                    ui.heading("Nuovo movimento pianificato");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut f.transfer, false, "Movimento");
                        ui.radio_value(&mut f.transfer, true, "Giroconto");
                    });
                    if !f.transfer {
                        expense_toggle(ui, &mut f.expense);
                    }
                    form_row(ui, if f.transfer { "Da" } else { "Conto" }, |ui| {
                        account_combo(ui, "schacc", &accounts, &mut f.account);
                    });
                    if f.transfer {
                        form_row(ui, "A", |ui| {
                            account_combo(ui, "schto", &accounts, &mut f.to_account);
                        });
                    }
                    form_row(ui, "Importo €", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.amount).hint_text("0,00").desired_width(120.0));
                    });
                    form_row(ui, "Descrizione", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.payee).desired_width(260.0));
                    });
                    if !f.transfer {
                        form_row(ui, "Categoria", |ui| {
                            category_combo(ui, "schcat", &cats, &mut f.category, "Nessuna");
                        });
                    }
                    form_row(ui, "Prima scadenza", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.date).desired_width(120.0));
                    });
                    form_row(ui, "Ripeti ogni", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.every).desired_width(40.0));
                        let unit_label = match f.unit.as_str() {
                            "day" => "giorni",
                            "week" => "settimane",
                            "year" => "anni",
                            _ => "mesi",
                        };
                        egui::ComboBox::from_id_salt("schunit").selected_text(unit_label).show_ui(ui, |ui| {
                            ui.selectable_value(&mut f.unit, "day".to_string(), "giorni");
                            ui.selectable_value(&mut f.unit, "week".to_string(), "settimane");
                            ui.selectable_value(&mut f.unit, "month".to_string(), "mesi");
                            ui.selectable_value(&mut f.unit, "year".to_string(), "anni");
                        });
                    });
                    if !f.error.is_empty() {
                        ui.label(RichText::new(&f.error).color(RED));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, "Salva").clicked() {
                            let r = (|| -> Result<String> {
                                let acc = f.account.ok_or_else(|| anyhow::anyhow!("scegli il conto"))?;
                                let d = parse_date(&f.date).ok_or_else(|| anyhow::anyhow!("data non valida"))?;
                                let a = parse_amount(&f.amount)?.abs();
                                let to = if f.transfer {
                                    Some(
                                        f.to_account
                                            .ok_or_else(|| anyhow::anyhow!("scegli il conto di destinazione"))?,
                                    )
                                } else {
                                    None
                                };
                                let amt = if f.transfer || !f.expense { a } else { -a };
                                let every: i64 = f.every.trim().parse().unwrap_or(1);
                                if f.payee.trim().is_empty() {
                                    anyhow::bail!("scrivi una descrizione");
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
                                Ok("Pianificato aggiunto".into())
                            })();
                            match r {
                                Ok(m) => {
                                    message = Some(Ok(m));
                                    keep = false;
                                }
                                Err(e) => f.error = format!("{e:#}"),
                            }
                        }
                        if ui.button("Annulla").clicked() {
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
                Dialog::ConfirmDeleteAccount(id, name) => {
                    ui.heading("Eliminare il conto?");
                    ui.label(format!(
                        "Il conto \"{name}\" e tutti i suoi movimenti saranno eliminati. Non si può annullare."
                    ));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new("Elimina").color(Color32::WHITE)).fill(RED)).clicked()
                        {
                            message = Some(self.db.delete_account(*id).map(|_| "Conto eliminato".to_string()));
                            self.view = View::Dashboard;
                            keep = false;
                        }
                        if ui.button("Annulla").clicked() {
                            keep = false;
                        }
                    });
                }
                Dialog::ConfirmClearDemo => {
                    ui.heading("Eliminare i dati di esempio?");
                    ui.label(
                        "Conti, movimenti, budget e pianificati di esempio saranno eliminati.\n\
                         Categorie e regole restano, così puoi iniziare subito.",
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if primary_button(ui, "Elimina e inizia").clicked() {
                            message = Some(
                                self.db
                                    .clear_accounts()
                                    .map(|_| "Pronto: crea il tuo primo conto o importa un CSV".to_string()),
                            );
                            self.view = View::Dashboard;
                            keep = false;
                        }
                        if ui.button("Annulla").clicked() {
                            keep = false;
                        }
                    });
                }
            }
        });
        // Esc o clic fuori chiudono (tranne l'importazione, per non perdere la mappatura).
        if modal.should_close() && !matches!(dialog, Dialog::Import(_)) {
            keep = false;
        }
        if keep {
            self.dialog = Some(dialog);
        }
        if let Some(m) = message {
            let time = ctx.input(|i| i.time);
            match m {
                Ok(s) => self.toast = Some((s, time + 4.0)),
                Err(e) => self.toast = Some((format!("Errore: {e:#}"), time + 6.0)),
            }
            self.reload();
        }
    }

    fn save_tx(&mut self, f: &TxForm) -> Result<String> {
        let account = f.account.ok_or_else(|| anyhow::anyhow!("scegli un conto"))?;
        let date = parse_date(&f.date).ok_or_else(|| anyhow::anyhow!("data non valida (gg/mm/aaaa)"))?;
        let a = parse_amount(&f.amount)?.abs();
        if a == 0 {
            anyhow::bail!("inserisci un importo");
        }
        let amount = if f.expense { -a } else { a };
        if f.payee.trim().is_empty() {
            anyhow::bail!("scrivi il beneficiario o una descrizione");
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
                Ok("Movimento aggiornato".into())
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
                Ok("Movimento registrato".into())
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
        ui.heading("Importa estratto conto CSV");
        ui.label(
            RichText::new(
                "Funziona con i CSV esportati dall'home banking: colonne, separatore, date e importi all'italiana \
                 vengono riconosciuti da soli. I movimenti già importati vengono saltati.",
            )
            .color(MUTED),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("📂 Scegli file…").clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter("CSV", &["csv", "CSV", "txt"]).pick_file()
            {
                st.path_text = p.display().to_string();
                st.load();
            }
            let r = ui.add(
                egui::TextEdit::singleline(&mut st.path_text)
                    .hint_text("…oppure incolla il percorso del file")
                    .desired_width(440.0),
            );
            if ui.button("Analizza").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                st.load();
            }
        });
        if !st.error.is_empty() {
            ui.label(RichText::new(&st.error).color(RED));
        }
        if let Some(res) = &st.result {
            ui.add_space(10.0);
            card(ui, |ui| {
                ui.label(RichText::new("✔ Importazione completata").size(16.0).strong().color(GREEN));
                ui.label(format!("{} movimenti importati", res.added));
                ui.label(format!("{} già presenti (saltati)", res.duplicates));
                ui.label(format!("{} categorizzati automaticamente dalle regole", res.categorized));
                if res.skipped > 0 {
                    ui.label(
                        RichText::new(format!("{} righe non erano movimenti (saldi, totali…)", res.skipped))
                            .color(MUTED),
                    );
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if primary_button(ui, "Vai al conto").clicked() {
                    if let Some(a) = st.account {
                        self.view = View::Register(Some(a));
                        self.period = Period::All;
                        self.selected = None;
                    }
                    keep = false;
                    msg = Some(Ok(String::new()));
                }
                if ui.button("Chiudi").clicked() {
                    keep = false;
                    msg = Some(Ok(String::new()));
                }
            });
            return (keep, msg);
        }
        if let Some(t) = &st.table {
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "Codifica {} · separatore «{}» · decimali con «{}» · intestazione {}",
                    t.encoding,
                    if t.delimiter == b'\t' { "tab".to_string() } else { (t.delimiter as char).to_string() },
                    if t.decimal_comma { "," } else { "." },
                    t.header_row.map(|h| format!("alla riga {}", h + 1)).unwrap_or_else(|| "assente".into())
                ))
                .color(MUTED)
                .size(12.5),
            );
            ui.add_space(4.0);
            let headers = t.headers.clone();
            let mut changed = false;
            egui::Grid::new("mapping").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Data");
                changed |= column_combo(ui, "m_date", &headers, &mut st.mapping.date);
                ui.label("Importo (con segno)");
                changed |= column_combo(ui, "m_amount", &headers, &mut st.mapping.amount);
                ui.end_row();
                ui.label("Descrizione");
                changed |= column_combo(ui, "m_desc", &headers, &mut st.mapping.description);
                ui.label("Entrate / Accrediti");
                changed |= column_combo(ui, "m_credit", &headers, &mut st.mapping.credit);
                ui.end_row();
                ui.label("Beneficiario");
                changed |= column_combo(ui, "m_payee", &headers, &mut st.mapping.payee);
                ui.label("Uscite / Addebiti");
                changed |= column_combo(ui, "m_debit", &headers, &mut st.mapping.debit);
                ui.end_row();
                ui.label("Dettagli");
                changed |= column_combo(ui, "m_extra", &headers, &mut st.mapping.extra);
                ui.label("");
                changed |= ui.checkbox(&mut st.invert, "Inverti segno (carte di credito)").changed();
                ui.end_row();
            });
            if changed {
                st.reparse();
            }
            ui.add_space(8.0);
            let total: Cents = st.rows.iter().map(|r| r.amount).sum();
            ui.label(
                RichText::new(format!(
                    "{} movimenti trovati{} · somma {}{}",
                    st.rows.len(),
                    if st.skipped > 0 { format!(" ({} righe saltate)", st.skipped) } else { String::new() },
                    fmt_eur(total),
                    match (st.rows.first(), st.rows.last()) {
                        (Some(a), Some(b)) => format!(" · dal {} al {}", fmt_date(a.date), fmt_date(b.date)),
                        _ => String::new(),
                    }
                ))
                .strong(),
            );
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
                ui.label("Importa nel conto:");
                account_combo(ui, "impacc", accounts, &mut st.account);
                ui.label("oppure in un nuovo conto:");
                ui.add(egui::TextEdit::singleline(&mut st.new_account).hint_text("nome").desired_width(150.0));
            });
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let can = !st.rows.is_empty() && (st.account.is_some() || !st.new_account.trim().is_empty());
            let btn = egui::Button::new(
                RichText::new(format!("Importa {} movimenti", st.rows.len())).color(Color32::WHITE).strong(),
            )
            .fill(ACCENT);
            if ui.add_enabled(can, btn).clicked() {
                let r = (|| -> Result<ImportResult> {
                    let acc = if !st.new_account.trim().is_empty() {
                        self.db.add_account(st.new_account.trim(), AccountKind::Bank, "EUR", 0, "")?
                    } else {
                        st.account.ok_or_else(|| anyhow::anyhow!("scegli un conto"))?
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
            if ui.button("Annulla").clicked() {
                keep = false;
            }
        });
        (keep, msg)
    }
}
