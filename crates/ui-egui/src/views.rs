//! Schermate della finestra: barra laterale, barra superiore, panoramica,
//! registro movimenti, report, budget, categorie, regole e pianificati.

use crate::gui::{
    ACCENT, ACCENT_SOFT, AMBER, AccountForm, App, BLUE, BORDER, CARD, Dialog, GREEN, MUTED, Period, RED, SIDEBAR,
    SchedForm, TEXT, TRACK, TransferForm, View, ZEBRA, amount_color, bars_chart, budget_color, card, category_combo,
    cut, donut, hbars, kind_icon, legend, line_chart, money, nav_item, or_report, palette, primary_button, progress,
    section_title, small_caps,
};
use cashcraft_engine::db::{self, Account, AccountKind, Category, Cents, Transaction};
use cashcraft_engine::money::{fmt_date, fmt_eur, month_name, parse_amount};
use chrono::{Datelike, Months};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Frame, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, Ui,
    Vec2,
};

impl App {
    pub fn sidebar(&mut self, ui: &mut Ui) {
        egui::Panel::left("sidebar")
            .exact_size(264.0)
            .resizable(false)
            .frame(Frame::new().fill(SIDEBAR).inner_margin(Margin::symmetric(12, 14)).stroke(Stroke::new(1.0, BORDER)))
            .show(ui, |ui| {
                // Marchio: una moneta stilizzata.
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                    let p = ui.painter();
                    p.circle_filled(r.center(), 15.0, ACCENT);
                    p.circle_stroke(r.center(), 11.0, Stroke::new(1.5, Color32::from_rgb(0xBF, 0xE3, 0xDA)));
                    p.text(r.center(), Align2::CENTER_CENTER, "€", FontId::proportional(16.0), Color32::WHITE);
                    ui.label(RichText::new("CashCraft").size(19.0).strong().color(TEXT));
                });
                ui.add_space(12.0);
                let list_h = (ui.available_height() - 74.0).max(100.0);
                egui::ScrollArea::vertical()
                    .id_salt("sidebar_scroll")
                    .auto_shrink([false, false])
                    .max_height(list_h)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        if nav_item(ui, self.view == View::Dashboard, "🏠", "Panoramica", None).clicked() {
                            self.go(View::Dashboard);
                        }
                        if nav_item(ui, self.view == View::Register(None), "📋", "Tutti i movimenti", None).clicked()
                        {
                            self.go(View::Register(None));
                        }
                        let accounts = self.accounts.clone();
                        for kind in AccountKind::ALL {
                            let group: Vec<&Account> =
                                accounts.iter().filter(|a| a.kind == kind && !a.archived).collect();
                            if group.is_empty() {
                                continue;
                            }
                            ui.add_space(10.0);
                            let total: Cents = group.iter().map(|a| a.balance).sum();
                            ui.horizontal(|ui| {
                                small_caps(ui, kind.group_label());
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(RichText::new(fmt_eur(total)).size(11.0).color(MUTED));
                                });
                            });
                            for a in group {
                                let r = nav_item(
                                    ui,
                                    self.view == View::Register(Some(a.id)),
                                    kind_icon(a.kind),
                                    &a.name,
                                    Some((fmt_eur(a.balance), if a.balance < 0 { RED } else { TEXT })),
                                );
                                if r.clicked() {
                                    self.go(View::Register(Some(a.id)));
                                }
                                let a2 = a.clone();
                                r.context_menu(|ui| {
                                    if ui.button("✏ Modifica conto…").clicked() {
                                        self.dialog = Some(Dialog::Account(AccountForm {
                                            id: Some(a2.id),
                                            name: a2.name.clone(),
                                            kind: a2.kind,
                                            opening: cashcraft_engine::money::fmt_plain(a2.opening_balance),
                                            iban: a2.iban.clone(),
                                            archived: a2.archived,
                                            error: String::new(),
                                        }));
                                    }
                                    if ui.button("⬇ Importa CSV in questo conto…").clicked() {
                                        self.go(View::Register(Some(a2.id)));
                                        self.open_import(None);
                                    }
                                    if ui.button(RichText::new("🗑 Elimina conto…").color(RED)).clicked() {
                                        self.dialog = Some(Dialog::ConfirmDeleteAccount(a2.id, a2.name.clone()));
                                    }
                                });
                            }
                        }
                        ui.add_space(6.0);
                        if ui
                            .add(egui::Button::new(RichText::new("+ Nuovo conto").color(ACCENT)).frame(false))
                            .clicked()
                        {
                            self.dialog = Some(Dialog::Account(AccountForm {
                                id: None,
                                name: String::new(),
                                kind: AccountKind::Bank,
                                opening: "0,00".into(),
                                iban: String::new(),
                                archived: false,
                                error: String::new(),
                            }));
                        }
                        ui.add_space(12.0);
                        small_caps(ui, "Analisi");
                        if nav_item(ui, self.view == View::Reports, "📊", "Report", None).clicked() {
                            self.go(View::Reports);
                        }
                        if nav_item(ui, self.view == View::Budget, "🎯", "Budget", None).clicked() {
                            self.go(View::Budget);
                        }
                        if nav_item(ui, self.view == View::Scheduled, "📅", "Pianificati", None).clicked() {
                            self.go(View::Scheduled);
                        }
                        ui.add_space(12.0);
                        small_caps(ui, "Organizza");
                        if nav_item(ui, self.view == View::Categories, "🏷", "Categorie", None).clicked() {
                            self.go(View::Categories);
                        }
                        if nav_item(ui, self.view == View::Rules, "⚙", "Regole automatiche", None).clicked() {
                            self.go(View::Rules);
                        }
                    });
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    let total: Cents = self.accounts.iter().filter(|a| !a.archived).map(|a| a.balance).sum();
                    ui.label(RichText::new(fmt_eur(total)).size(21.0).strong().color(if total < 0 {
                        RED
                    } else {
                        TEXT
                    }));
                    small_caps(ui, "Patrimonio netto");
                    ui.separator();
                });
            });
    }

    fn view_title(&self) -> (String, String) {
        match self.view {
            View::Dashboard => {
                let t = db::today();
                ("Panoramica".into(), format!("{} {}", month_name(t.month()), t.year()))
            }
            View::Register(None) => ("Tutti i movimenti".into(), "Tutti i conti".into()),
            View::Register(Some(a)) => {
                let acc = self.accounts.iter().find(|x| x.id == a);
                (
                    acc.map(|a| a.name.clone()).unwrap_or_default(),
                    acc.map(|a| {
                        let mut s = a.kind.label().to_string();
                        if !a.iban.is_empty() {
                            s.push_str(&format!(" · {}", a.iban));
                        }
                        s
                    })
                    .unwrap_or_default(),
                )
            }
            View::Reports => ("Report".into(), "Dove vanno i tuoi soldi".into()),
            View::Budget => ("Budget".into(), "Limiti di spesa mensili per categoria".into()),
            View::Categories => ("Categorie".into(), "Come raggruppare entrate e uscite".into()),
            View::Rules => ("Regole automatiche".into(), "Assegnano la categoria ai movimenti importati".into()),
            View::Scheduled => ("Pianificati".into(), "Movimenti ricorrenti e scadenze".into()),
        }
    }

    pub fn toolbar(&mut self, ui: &mut Ui) {
        egui::Panel::top("toolbar")
            .frame(Frame::new().fill(CARD).inner_margin(Margin::symmetric(22, 12)).stroke(Stroke::new(1.0, BORDER)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (title, sub) = self.view_title();
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(RichText::new(title).size(22.0).strong().color(TEXT));
                        ui.label(RichText::new(sub).size(12.5).color(MUTED));
                    });
                    if let View::Register(Some(a)) = self.view
                        && let Some(acc) = self.accounts.iter().find(|x| x.id == a) {
                            ui.add_space(24.0);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                ui.label(RichText::new(fmt_eur(acc.balance)).size(22.0).strong().color(amount_color(acc.balance)));
                                ui.label(RichText::new("Saldo attuale").size(12.5).color(MUTED));
                            });
                        }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .button("⬇ Importa CSV")
                            .on_hover_text("Importa l'estratto conto della banca (⌘I). Puoi anche trascinare il file sulla finestra.")
                            .clicked()
                        {
                            self.open_import(None);
                        }
                        if ui.button("⇄ Giroconto").clicked() {
                            self.dialog = Some(Dialog::Transfer(TransferForm {
                                from: self.default_account(),
                                date: fmt_date(db::today()),
                                ..Default::default()
                            }));
                        }
                        if primary_button(ui, "+ Movimento").on_hover_text("Nuovo movimento (⌘N)").clicked() {
                            self.open_new_tx();
                        }
                    });
                });
                if self.demo {
                    ui.add_space(8.0);
                    Frame::new()
                        .fill(Color32::from_rgb(0xFF, 0xF4, 0xDC))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("ℹ Stai guardando dati di esempio, per provare il programma.")
                                        .color(Color32::from_rgb(0x7A, 0x55, 0x10)),
                                );
                                if ui.link("Eliminali e inizia con i tuoi conti").clicked() {
                                    self.dialog = Some(Dialog::ConfirmClearDemo);
                                }
                            });
                        });
                }
            });
    }

    pub fn status_bar(&mut self, ui: &mut Ui) {
        egui::Panel::bottom("status")
            .frame(Frame::new().fill(SIDEBAR).inner_margin(Margin::symmetric(14, 4)).stroke(Stroke::new(1.0, BORDER)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let path = self
                        .db
                        .path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "archivio di prova in memoria".into());
                    ui.label(RichText::new(format!("Archivio: {path}")).size(11.5).color(MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new("Pilotabile da terminale (cashcraft --help) e da MCP (cashcraft mcp)")
                                .size(11.5)
                                .color(MUTED),
                        );
                    });
                });
            });
    }

    // -------------------------------------------------------------- panoramica

    pub fn dashboard(&mut self, ui: &mut Ui) {
        self.ensure_dash();
        let Some(d) = self.dash.take() else { return };
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let net: Cents = self.accounts.iter().filter(|a| !a.archived).map(|a| a.balance).sum();
            let saving = d.month_income - d.month_expense;
            let mname = month_name(db::today().month()).to_string();
            let cards = [
                ("Patrimonio netto", fmt_eur(net), if net < 0 { RED } else { TEXT }, "tutti i conti".to_string()),
                ("Entrate del mese", fmt_eur(d.month_income), GREEN, mname.clone()),
                ("Uscite del mese", fmt_eur(d.month_expense), RED, mname),
                (
                    "Risparmio del mese",
                    fmt_eur(saving),
                    amount_color(saving),
                    if saving < 0 {
                        "uscite oltre le entrate".into()
                    } else if d.month_income > 0 {
                        format!("{:.0}% delle entrate", saving as f64 * 100.0 / d.month_income as f64)
                    } else {
                        "—".into()
                    },
                ),
            ];
            let gap = 14.0;
            let cw = (ui.available_width() - gap * 3.0) / 4.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (title, value, color, sub) in cards {
                    ui.allocate_ui(Vec2::new(cw, 96.0), |ui| {
                        card(ui, |ui| {
                            ui.set_width(cw - 30.0);
                            small_caps(ui, title);
                            ui.label(RichText::new(value).size(23.0).strong().color(color));
                            ui.label(RichText::new(sub).size(12.0).color(MUTED));
                        });
                    });
                }
            });
            ui.add_space(gap);
            let half = (ui.available_width() - gap) / 2.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.allocate_ui(Vec2::new(half, 300.0), |ui| {
                    card(ui, |ui| {
                        ui.set_width(half - 30.0);
                        ui.horizontal(|ui| {
                            section_title(ui, "Entrate e uscite");
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                legend(ui, &[("Uscite", RED), ("Entrate", GREEN)]);
                            });
                        });
                        ui.label(RichText::new("Ultimi 6 mesi, giroconti esclusi").size(12.0).color(MUTED));
                        ui.add_space(6.0);
                        bars_chart(ui, &d.monthly, 216.0);
                    });
                });
                ui.allocate_ui(Vec2::new(half, 300.0), |ui| {
                    card(ui, |ui| {
                        ui.set_width(half - 30.0);
                        ui.set_min_height(266.0);
                        section_title(ui, "Spese per categoria");
                        ui.label(RichText::new("Questo mese").size(12.0).color(MUTED));
                        ui.add_space(6.0);
                        if d.top_expenses.is_empty() {
                            ui.label(RichText::new("Nessuna spesa questo mese.").color(MUTED));
                        } else {
                            hbars(ui, &d.top_expenses);
                        }
                    });
                });
            });
            ui.add_space(gap);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.allocate_ui(Vec2::new(half, 340.0), |ui| {
                    card(ui, |ui| {
                        ui.set_width(half - 30.0);
                        ui.horizontal(|ui| {
                            section_title(ui, "Ultimi movimenti");
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.link("Vedi tutti").clicked() {
                                    self.go(View::Register(None));
                                }
                            });
                        });
                        if d.uncategorized > 0 {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("● {} movimenti senza categoria", d.uncategorized))
                                        .size(12.5)
                                        .color(AMBER),
                                );
                                if ui.link("Sistemali").clicked() {
                                    self.only_uncategorized = true;
                                    self.period = Period::All;
                                    self.go(View::Register(None));
                                    self.reload_txs();
                                }
                            });
                        }
                        ui.add_space(4.0);
                        for t in &d.recent {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(t.date.format("%d/%m").to_string()).size(12.5).color(MUTED));
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    ui.label(RichText::new(&t.payee).size(13.5).color(TEXT));
                                    let sub = if t.category.is_empty() {
                                        "da categorizzare".to_string()
                                    } else {
                                        t.category.clone()
                                    };
                                    ui.label(RichText::new(format!("{sub} · {}", t.account)).size(11.5).color(MUTED));
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(money(t.amount).size(13.5));
                                });
                            });
                        }
                    });
                });
                ui.allocate_ui(Vec2::new(half, 340.0), |ui| {
                    ui.vertical(|ui| {
                        card(ui, |ui| {
                            ui.set_width(half - 30.0);
                            ui.horizontal(|ui| {
                                section_title(ui, "Budget del mese");
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.link("Gestisci").clicked() {
                                        self.go(View::Budget);
                                    }
                                });
                            });
                            if d.budgets.is_empty() {
                                ui.label(RichText::new("Nessun budget impostato.").color(MUTED));
                            }
                            for b in d.budgets.iter().take(5) {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(&b.category).size(13.0));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(
                                            RichText::new(format!("{} / {}", fmt_eur(b.spent), fmt_eur(b.budget)))
                                                .size(12.0)
                                                .color(MUTED),
                                        );
                                    });
                                });
                                progress(ui, b.spent as f32 / b.budget.max(1) as f32, budget_color(b.spent, b.budget));
                                ui.add_space(2.0);
                            }
                        });
                        ui.add_space(gap);
                        card(ui, |ui| {
                            ui.set_width(half - 30.0);
                            section_title(ui, "Prossime scadenze");
                            if d.upcoming.is_empty() {
                                ui.label(RichText::new("Nessun movimento pianificato.").color(MUTED));
                            }
                            for s in &d.upcoming {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(fmt_date(s.next_date)).size(12.5).color(MUTED));
                                    ui.label(RichText::new(&s.payee).size(13.0));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        let amt = if s.to_account_id.is_some() { -s.amount.abs() } else { s.amount };
                                        ui.label(money(amt).size(13.0));
                                    });
                                });
                            }
                        });
                    });
                });
            });
        });
        if self.dash.is_none() {
            self.dash = Some(d);
        }
    }

    // -------------------------------------------------------------- registro

    pub fn register(&mut self, ui: &mut Ui, account: Option<i64>) {
        let mut changed = false;
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("period").selected_text(self.period.label()).width(140.0).show_ui(ui, |ui| {
                for p in Period::ALL {
                    if ui.selectable_value(&mut self.period, p, p.label()).changed() {
                        changed = true;
                    }
                }
            });
            let cats = self.categories.clone();
            if category_combo(ui, "catfilter", &cats, &mut self.cat_filter, "Tutte le categorie") {
                changed = true;
            }
            if ui.checkbox(&mut self.only_uncategorized, "Solo senza categoria").changed() {
                changed = true;
            }
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("🔍 Cerca beneficiario, note, importo…")
                    .desired_width(240.0),
            );
            if r.changed() {
                changed = true;
            }
            if (!self.search.is_empty() || self.cat_filter.is_some() || self.only_uncategorized)
                && ui.button("✕ Azzera filtri").clicked()
            {
                self.search.clear();
                self.cat_filter = None;
                self.only_uncategorized = false;
                changed = true;
            }
        });
        if changed {
            self.selected = None;
            self.reload_txs();
        }
        ui.add_space(4.0);
        let income: Cents =
            self.txs.iter().filter(|t| t.amount > 0 && t.transfer_peer.is_none()).map(|t| t.amount).sum();
        let expense: Cents =
            self.txs.iter().filter(|t| t.amount < 0 && t.transfer_peer.is_none()).map(|t| t.amount).sum();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} movimenti", self.txs.len())).color(MUTED));
            ui.label(RichText::new("· Entrate").color(MUTED));
            ui.label(money(income));
            ui.label(RichText::new("· Uscite").color(MUTED));
            ui.label(money(expense));
            ui.label(RichText::new("· Differenza").color(MUTED));
            ui.label(money(income + expense));
        });
        ui.add_space(6.0);

        let show_account = account.is_none();
        let show_balance =
            account.is_some() && self.search.is_empty() && self.cat_filter.is_none() && !self.only_uncategorized;
        let bottom_bar = if self.selected.is_some() { 44.0 } else { 0.0 };
        let avail = ui.available_size();
        let table_h = (avail.y - bottom_bar).max(120.0);
        let mut open_edit: Option<Transaction> = None;
        let mut toggle_cleared: Option<(i64, bool)> = None;
        let mut delete: Option<i64> = None;
        let mut set_cat: Option<(i64, Option<i64>)> = None;
        let mut want_import = false;
        let cats = self.categories.clone();
        Frame::new().fill(CARD).stroke(Stroke::new(1.0, BORDER)).corner_radius(CornerRadius::same(10)).show(ui, |ui| {
            ui.set_width(avail.x - 2.0);
            ui.set_height(table_h - 4.0);
            let w = ui.available_width();
            let fixed_date = 96.0;
            let fixed_amt = 116.0;
            let fixed_bal = if show_balance { 116.0 } else { 0.0 };
            let fixed_chk = 36.0;
            let fixed_acc = if show_account { 140.0 } else { 0.0 };
            let flex = (w - fixed_date - fixed_amt - fixed_bal - fixed_chk - fixed_acc - 16.0).max(240.0);
            let cols = [
                ("Data", fixed_date, false),
                ("Beneficiario", flex * 0.36, false),
                ("Categoria", flex * 0.30, false),
                ("Note", flex * 0.34, false),
                ("Conto", fixed_acc, false),
                ("Importo", fixed_amt, true),
                ("Saldo", fixed_bal, true),
                ("✓", fixed_chk, true),
            ];
            let (hrect, _) = ui.allocate_exact_size(Vec2::new(w, 32.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(hrect, CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 }, Color32::from_rgb(0xF4, 0xF6, 0xF5));
            p.hline(hrect.x_range(), hrect.bottom(), Stroke::new(1.0, BORDER));
            let mut x = hrect.left() + 12.0;
            for (name, cw, right) in cols {
                if cw <= 0.0 {
                    continue;
                }
                let (pos, al) = if right {
                    (Pos2::new(x + cw - 10.0, hrect.center().y), Align2::RIGHT_CENTER)
                } else {
                    (Pos2::new(x, hrect.center().y), Align2::LEFT_CENTER)
                };
                p.text(pos, al, name.to_uppercase(), FontId::proportional(11.0), MUTED);
                x += cw;
            }
            if self.txs.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("Nessun movimento in questo periodo.").color(MUTED).size(15.0));
                    ui.add_space(8.0);
                    if ui.button("⬇ Importa un estratto conto CSV").clicked() {
                        want_import = true;
                    }
                });
                return;
            }
            let row_h = 30.0;
            egui::ScrollArea::vertical().id_salt("register_scroll").auto_shrink([false, false]).show_rows(
                ui,
                row_h,
                self.txs.len(),
                |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for i in range {
                        let Some(t) = self.txs.get(i) else { continue };
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, row_h), Sense::click());
                        let p = ui.painter();
                        let selected = self.selected == Some(t.id);
                        let bg = if selected {
                            ACCENT_SOFT
                        } else if resp.hovered() {
                            Color32::from_rgb(0xEF, 0xF4, 0xF3)
                        } else if i % 2 == 1 {
                            ZEBRA
                        } else {
                            CARD
                        };
                        p.rect_filled(rect, CornerRadius::ZERO, bg);
                        let mut x = rect.left() + 12.0;
                        let mut cell = |cw: f32, text: &str, color: Color32, right: bool, size: f32| {
                            if cw <= 0.0 {
                                return;
                            }
                            let r =
                                Rect::from_min_max(Pos2::new(x, rect.top()), Pos2::new(x + cw - 10.0, rect.bottom()));
                            let (pos, al) = if right {
                                (r.right_center(), Align2::RIGHT_CENTER)
                            } else {
                                (r.left_center(), Align2::LEFT_CENTER)
                            };
                            p.with_clip_rect(r).text(pos, al, text, FontId::proportional(size), color);
                            x += cw;
                        };
                        cell(cols[0].1, &fmt_date(t.date), MUTED, false, 13.0);
                        cell(cols[1].1, &t.payee, TEXT, false, 13.5);
                        if t.category.is_empty() {
                            cell(cols[2].1, "— da categorizzare —", AMBER, false, 12.5);
                        } else {
                            cell(
                                cols[2].1,
                                &t.category,
                                if t.transfer_peer.is_some() { BLUE } else { TEXT },
                                false,
                                13.0,
                            );
                        }
                        cell(cols[3].1, &t.memo, MUTED, false, 12.5);
                        cell(cols[4].1, &t.account, MUTED, false, 12.5);
                        cell(cols[5].1, &fmt_eur(t.amount), amount_color(t.amount), true, 13.5);
                        let bal = t.balance.map(fmt_eur).unwrap_or_default();
                        cell(cols[6].1, &bal, if t.balance.unwrap_or(0) < 0 { RED } else { TEXT }, true, 13.0);
                        let chk_c = Pos2::new(x + fixed_chk / 2.0 - 6.0, rect.center().y);
                        if t.cleared {
                            p.circle_filled(chk_c, 7.0, GREEN);
                            p.text(chk_c, Align2::CENTER_CENTER, "✓", FontId::proportional(10.0), Color32::WHITE);
                        } else {
                            p.circle_stroke(chk_c, 7.0, Stroke::new(1.2, Color32::from_rgb(0xB8, 0xC0, 0xBE)));
                        }
                        if resp.clicked() {
                            if let Some(pos) = resp.interact_pointer_pos()
                                && pos.distance(chk_c) < 11.0
                            {
                                toggle_cleared = Some((t.id, !t.cleared));
                            }
                            self.selected = Some(t.id);
                        }
                        if resp.double_clicked() {
                            open_edit = Some(t.clone());
                        }
                        let tc = t.clone();
                        resp.context_menu(|ui| {
                            if ui.button("✏ Modifica…").clicked() {
                                open_edit = Some(tc.clone());
                            }
                            let lbl =
                                if tc.cleared { "Segna come non verificato" } else { "✓ Segna come verificato" };
                            if ui.button(lbl).clicked() {
                                toggle_cleared = Some((tc.id, !tc.cleared));
                            }
                            if tc.transfer_peer.is_none() {
                                ui.menu_button("🏷 Categoria", |ui| {
                                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                                        if ui.button("Nessuna").clicked() {
                                            set_cat = Some((tc.id, None));
                                        }
                                        for c in &cats {
                                            if ui.button(&c.path).clicked() {
                                                set_cat = Some((tc.id, Some(c.id)));
                                            }
                                        }
                                    });
                                });
                            }
                            ui.separator();
                            if ui.button(RichText::new("🗑 Elimina").color(RED)).clicked() {
                                delete = Some(tc.id);
                            }
                        });
                    }
                },
            );
        });

        // Barra del movimento selezionato.
        if let Some(sel) = self.selected
            && let Some(t) = self.txs.iter().find(|t| t.id == sel).cloned()
        {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} · {}", fmt_date(t.date), cut(&t.payee, 40))).strong());
                ui.label(money(t.amount));
                if t.transfer_peer.is_none() {
                    let mut c = t.category_id;
                    if category_combo(ui, "selcat", &cats, &mut c, "Senza categoria") {
                        set_cat = Some((t.id, c));
                    }
                }
                if ui.button("✏ Modifica").clicked() {
                    open_edit = Some(t.clone());
                }
                if ui.button(RichText::new("🗑 Elimina").color(RED)).clicked() {
                    delete = Some(t.id);
                }
            });
        }

        if want_import {
            self.open_import(None);
        }
        if let Some(t) = open_edit {
            self.open_edit_tx(&t);
        }
        if let Some((id, c)) = toggle_cleared {
            let r = self.db.set_cleared(id, c).map(|_| String::new());
            self.act(ui, r);
        }
        if let Some((id, c)) = set_cat {
            let r = self.db.set_category(id, c).map(|_| "Categoria aggiornata".to_string());
            self.act(ui, r);
        }
        if let Some(id) = delete {
            let r = self.db.delete_transaction(id).map(|_| "Movimento eliminato".to_string());
            self.selected = None;
            self.act(ui, r);
        }
    }

    // -------------------------------------------------------------- report

    pub fn reports(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for p in Period::ALL {
                let on = self.report_period == p;
                let b = egui::Button::new(RichText::new(p.label()).color(if on { Color32::WHITE } else { TEXT }))
                    .fill(if on { ACCENT } else { TRACK });
                if ui.add(b).clicked() && !on {
                    self.report_period = p;
                    self.report = None;
                }
            }
        });
        ui.add_space(10.0);
        self.ensure_report();
        let Some(r) = self.report.take() else { return };
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let income: Cents = r.totals.iter().filter(|c| c.total > 0).map(|c| c.total).sum();
            let expense: Cents = -r.totals.iter().filter(|c| c.total < 0).map(|c| c.total).sum::<Cents>();
            let net = income - expense;
            let gap = 14.0;
            let cw = (ui.available_width() - gap * 3.0) / 4.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                let rate = if income > 0 {
                    format!("{:.1}%", net as f64 * 100.0 / income as f64).replace('.', ",")
                } else {
                    "—".into()
                };
                for (t, v, c) in [
                    ("Entrate", fmt_eur(income), GREEN),
                    ("Uscite", fmt_eur(expense), RED),
                    ("Risparmio", fmt_eur(net), amount_color(net)),
                    ("Tasso di risparmio", rate, ACCENT),
                ] {
                    ui.allocate_ui(Vec2::new(cw, 80.0), |ui| {
                        card(ui, |ui| {
                            ui.set_width(cw - 30.0);
                            small_caps(ui, t);
                            ui.label(RichText::new(v).size(22.0).strong().color(c));
                        });
                    });
                }
            });
            ui.add_space(gap);
            let half = (ui.available_width() - gap) / 2.0;
            let mut exp: Vec<(String, Cents)> =
                r.totals.iter().filter(|c| c.total < 0).map(|c| (c.category.clone(), -c.total)).collect();
            exp.sort_by_key(|x| -x.1);
            let mut top: Vec<(String, Cents)> = exp.iter().take(8).cloned().collect();
            let rest: Cents = exp.iter().skip(8).map(|x| x.1).sum();
            if rest > 0 {
                top.push(("Altro".into(), rest));
            }
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.allocate_ui(Vec2::new(half, 320.0), |ui| {
                    card(ui, |ui| {
                        ui.set_width(half - 30.0);
                        ui.set_min_height(290.0);
                        section_title(ui, "Spese per categoria");
                        ui.add_space(6.0);
                        ui.horizontal_top(|ui| {
                            donut(ui, &top, 220.0);
                            ui.add_space(10.0);
                            ui.vertical(|ui| {
                                for (i, (name, v)) in top.iter().enumerate() {
                                    ui.horizontal(|ui| {
                                        let (rr, _) = ui.allocate_exact_size(Vec2::new(10.0, 10.0), Sense::hover());
                                        ui.painter().rect_filled(rr, CornerRadius::same(2), palette(i));
                                        ui.label(RichText::new(cut(name, 26)).size(12.5));
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            let pct =
                                                if expense > 0 { *v as f64 * 100.0 / expense as f64 } else { 0.0 };
                                            ui.label(RichText::new(format!("{:.0}%", pct)).size(12.0).color(MUTED));
                                        });
                                    });
                                }
                            });
                        });
                    });
                });
                ui.allocate_ui(Vec2::new(half, 320.0), |ui| {
                    card(ui, |ui| {
                        ui.set_width(half - 30.0);
                        ui.set_min_height(290.0);
                        ui.horizontal(|ui| {
                            section_title(ui, "Entrate e uscite per mese");
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                legend(ui, &[("Uscite", RED), ("Entrate", GREEN)]);
                            });
                        });
                        ui.add_space(6.0);
                        bars_chart(ui, &r.monthly, 240.0);
                    });
                });
            });
            ui.add_space(gap);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                section_title(ui, "Andamento del patrimonio netto (12 mesi)");
                ui.add_space(6.0);
                line_chart(ui, &r.net_worth, 200.0);
            });
            ui.add_space(gap);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                section_title(ui, "Dettaglio per categoria");
                ui.add_space(6.0);
                egui::Grid::new("cat_totals").striped(true).num_columns(4).min_col_width(140.0).show(ui, |ui| {
                    for h in ["CATEGORIA", "TIPO", "TOTALE", "% SUL TOTALE"] {
                        ui.label(RichText::new(h).size(11.0).color(MUTED));
                    }
                    ui.end_row();
                    for c in &r.totals {
                        ui.label(&c.category);
                        ui.label(RichText::new(if c.total > 0 { "Entrata" } else { "Uscita" }).color(MUTED));
                        ui.label(money(c.total));
                        let base = if c.total > 0 { income } else { expense };
                        ui.label(
                            format!("{:.1}%", c.total.abs() as f64 * 100.0 / base.max(1) as f64).replace('.', ","),
                        );
                        ui.end_row();
                    }
                });
            });
        });
        if self.report.is_none() {
            self.report = Some(r);
        }
    }

    // -------------------------------------------------------------- budget

    pub fn budget(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if ui.button("◀").clicked() {
                self.budget_month = self.budget_month.checked_sub_months(Months::new(1)).unwrap_or(self.budget_month);
            }
            ui.label(
                RichText::new(format!("{} {}", month_name(self.budget_month.month()), self.budget_month.year()))
                    .size(17.0)
                    .strong(),
            );
            if ui.button("▶").clicked() {
                self.budget_month = self.budget_month.checked_add_months(Months::new(1)).unwrap_or(self.budget_month);
            }
            if self.budget_month != db::first_of_month(db::today()) && ui.link("Mese corrente").clicked() {
                self.budget_month = db::first_of_month(db::today());
            }
        });
        ui.add_space(10.0);
        let lines = or_report(self.db.budget_status(self.budget_month), &mut self.toast);
        let total_b: Cents = lines.iter().map(|l| l.budget).sum();
        let total_s: Cents = lines.iter().map(|l| l.spent).sum();
        let mut save: Option<(i64, Cents)> = None;
        let mut remove: Option<i64> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    section_title(ui, "Totale");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let rem = total_b - total_s;
                        ui.label(
                            RichText::new(if rem >= 0 {
                                format!("restano {}", fmt_eur(rem))
                            } else {
                                format!("sforato di {}", fmt_eur(-rem))
                            })
                            .color(amount_color(rem)),
                        );
                        ui.label(
                            RichText::new(format!("{} spesi su {}", fmt_eur(total_s), fmt_eur(total_b))).color(MUTED),
                        );
                    });
                });
                progress(ui, total_s as f32 / total_b.max(1) as f32, budget_color(total_s, total_b));
            });
            ui.add_space(10.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                if lines.is_empty() {
                    ui.label(RichText::new("Nessun budget: aggiungine uno qui sotto.").color(MUTED));
                }
                for l in &lines {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&l.category).size(14.5).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("🗑").on_hover_text("Rimuovi budget").clicked() {
                                remove = Some(l.category_id);
                            }
                            match &mut self.budget_edit {
                                Some((id, text)) if *id == l.category_id => {
                                    if ui.small_button("✔").clicked()
                                        && let Ok(v) = parse_amount(text)
                                    {
                                        save = Some((*id, v));
                                    }
                                    ui.add(egui::TextEdit::singleline(text).desired_width(90.0));
                                }
                                _ => {
                                    if ui.small_button("✏").on_hover_text("Modifica importo").clicked() {
                                        self.budget_edit =
                                            Some((l.category_id, cashcraft_engine::money::fmt_plain(l.budget)));
                                    }
                                }
                            }
                            let rem = l.budget - l.spent;
                            ui.label(
                                RichText::new(if rem >= 0 {
                                    format!("restano {}", fmt_eur(rem))
                                } else {
                                    format!("{} oltre", fmt_eur(-rem))
                                })
                                .color(if rem >= 0 { MUTED } else { RED }),
                            );
                            ui.label(RichText::new(format!("{} / {}", fmt_eur(l.spent), fmt_eur(l.budget))));
                        });
                    });
                    progress(ui, l.spent as f32 / l.budget.max(1) as f32, budget_color(l.spent, l.budget));
                    ui.add_space(8.0);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Nuovo budget:");
                    let cats: Vec<Category> = self.categories.iter().filter(|c| !c.is_income).cloned().collect();
                    category_combo(ui, "newbudget", &cats, &mut self.new_budget.0, "Scegli categoria…");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_budget.1)
                            .hint_text("importo al mese")
                            .desired_width(110.0),
                    );
                    if primary_button(ui, "Aggiungi").clicked()
                        && let (Some(c), Ok(v)) = (self.new_budget.0, parse_amount(&self.new_budget.1))
                    {
                        save = Some((c, v));
                        self.new_budget = (None, String::new());
                    }
                });
            });
        });
        if let Some((c, v)) = save {
            self.budget_edit = None;
            let r = self.db.set_budget(c, v).map(|_| "Budget salvato".to_string());
            self.act(ui, r);
        }
        if let Some(c) = remove {
            let r = self.db.set_budget(c, 0).map(|_| "Budget rimosso".to_string());
            self.act(ui, r);
        }
    }

    // -------------------------------------------------------------- categorie

    pub fn categories_view(&mut self, ui: &mut Ui) {
        let cats = self.categories.clone();
        let mut del: Option<i64> = None;
        let mut rename: Option<(i64, String)> = None;
        let mut show: Option<i64> = None;
        let mut create: Option<(String, bool)> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label("Nuova categoria:");
                    ui.add(egui::TextEdit::singleline(&mut self.new_cat.0).hint_text("nome").desired_width(180.0));
                    ui.label("dentro");
                    let parents: Vec<Category> = cats.iter().filter(|c| c.parent_id.is_none()).cloned().collect();
                    category_combo(ui, "newcatparent", &parents, &mut self.new_cat.2, "(nessuna: principale)");
                    ui.checkbox(&mut self.new_cat.1, "è un'entrata");
                    if primary_button(ui, "Aggiungi").clicked() && !self.new_cat.0.trim().is_empty() {
                        let parent = self.new_cat.2.and_then(|p| cats.iter().find(|c| c.id == p));
                        let path = match parent {
                            Some(p) => format!("{}:{}", p.path, self.new_cat.0.trim()),
                            None => self.new_cat.0.trim().to_string(),
                        };
                        let income = parent.map(|p| p.is_income).unwrap_or(self.new_cat.1);
                        create = Some((path, income));
                    }
                });
            });
            ui.add_space(12.0);
            let gap = 14.0;
            let half = (ui.available_width() - gap) / 2.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for income in [false, true] {
                    ui.allocate_ui(Vec2::new(half, 10.0), |ui| {
                        card(ui, |ui| {
                            ui.set_width(half - 30.0);
                            section_title(ui, if income { "Entrate" } else { "Uscite" });
                            ui.add_space(4.0);
                            for c in cats.iter().filter(|c| c.is_income == income) {
                                let depth = c.path.matches(':').count();
                                ui.horizontal(|ui| {
                                    ui.add_space(depth as f32 * 22.0);
                                    match &mut self.rename_cat {
                                        Some((id, text)) if *id == c.id => {
                                            ui.add(egui::TextEdit::singleline(text).desired_width(160.0));
                                            if ui.small_button("✔").clicked() {
                                                rename = Some((c.id, text.clone()));
                                            }
                                        }
                                        _ => {
                                            let t = if depth == 0 {
                                                RichText::new(&c.name).strong()
                                            } else {
                                                RichText::new(&c.name)
                                            };
                                            ui.label(t);
                                        }
                                    }
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui
                                            .small_button("🗑")
                                            .on_hover_text("Elimina (i movimenti restano senza categoria)")
                                            .clicked()
                                        {
                                            del = Some(c.id);
                                        }
                                        if ui.small_button("✏").on_hover_text("Rinomina").clicked() {
                                            self.rename_cat = Some((c.id, c.name.clone()));
                                        }
                                        if ui.small_button("📋").on_hover_text("Vedi i movimenti").clicked() {
                                            show = Some(c.id);
                                        }
                                    });
                                });
                            }
                        });
                    });
                }
            });
        });
        if let Some((path, income)) = create {
            let r = self.db.category_by_path(&path, income, true).map(|_| "Categoria creata".to_string());
            self.new_cat = (String::new(), false, None);
            self.act(ui, r);
        }
        if let Some((id, name)) = rename {
            self.rename_cat = None;
            let r = self.db.rename_category(id, &name).map(|_| "Categoria rinominata".to_string());
            self.act(ui, r);
        }
        if let Some(id) = del {
            let r = self.db.delete_category(id).map(|_| "Categoria eliminata".to_string());
            self.act(ui, r);
        }
        if let Some(id) = show {
            self.cat_filter = Some(id);
            self.only_uncategorized = false;
            self.period = Period::All;
            self.go(View::Register(None));
            self.reload_txs();
        }
    }

    // -------------------------------------------------------------- regole

    pub fn rules_view(&mut self, ui: &mut Ui) {
        let rules = or_report(self.db.rules(), &mut self.toast);
        let mut del: Option<i64> = None;
        let mut add = false;
        let mut apply = false;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(
                        "Quando il beneficiario o la descrizione contengono una delle parole chiave (separate da | ), \
                         il movimento riceve la categoria indicata. Le regole valgono per ogni movimento nuovo o importato senza categoria.",
                    )
                    .color(MUTED),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.new_rule.0).hint_text("parole chiave, es. panificio|forno").desired_width(240.0));
                    ui.label("→");
                    let cats = self.categories.clone();
                    category_combo(ui, "rulecat", &cats, &mut self.new_rule.1, "Scegli categoria…");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_rule.2)
                            .hint_text("rinomina beneficiario (facoltativo)")
                            .desired_width(220.0),
                    );
                    if primary_button(ui, "Aggiungi regola").clicked() {
                        add = true;
                    }
                });
                ui.add_space(6.0);
                if ui.button("▶ Applica ora ai movimenti senza categoria").clicked() {
                    apply = true;
                }
            });
            ui.add_space(12.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::Grid::new("rules").striped(true).num_columns(4).min_col_width(80.0).spacing([16.0, 8.0]).show(ui, |ui| {
                    for h in ["PAROLE CHIAVE", "CATEGORIA", "RINOMINA IN", ""] {
                        ui.label(RichText::new(h).size(11.0).color(MUTED));
                    }
                    ui.end_row();
                    for r in &rules {
                        ui.label(r.pattern.replace('|', " · "));
                        ui.label(RichText::new(&r.category).color(ACCENT));
                        ui.label(RichText::new(&r.rename_payee).color(MUTED));
                        if ui.small_button("🗑").clicked() {
                            del = Some(r.id);
                        }
                        ui.end_row();
                    }
                });
            });
        });
        if add {
            if let Some(c) = self.new_rule.1 {
                let r = self.db.add_rule(&self.new_rule.0, c, &self.new_rule.2).map(|_| "Regola aggiunta".to_string());
                if r.is_ok() {
                    self.new_rule = (String::new(), None, String::new());
                }
                self.act(ui, r);
            } else {
                self.notify(ui, "Scegli la categoria della regola");
            }
        }
        if apply {
            let r = self.db.apply_rules().map(|n| format!("{n} movimenti categorizzati"));
            self.act(ui, r);
        }
        if let Some(id) = del {
            let r = self.db.delete_rule(id).map(|_| "Regola eliminata".to_string());
            self.act(ui, r);
        }
    }

    // -------------------------------------------------------------- pianificati

    pub fn scheduled_view(&mut self, ui: &mut Ui) {
        let items = or_report(self.db.scheduled(), &mut self.toast);
        let mut del: Option<i64> = None;
        let mut post = false;
        ui.horizontal(|ui| {
            if primary_button(ui, "+ Nuovo pianificato").clicked() {
                self.dialog = Some(Dialog::Scheduled(SchedForm {
                    account: self.default_account(),
                    expense: true,
                    date: fmt_date(db::today()),
                    every: "1".into(),
                    unit: "month".into(),
                    ..Default::default()
                }));
            }
            let due = items.iter().filter(|s| s.next_date <= db::today()).count();
            if ui
                .add_enabled(due > 0, egui::Button::new(format!("✔ Registra le scadenze fino a oggi ({due})")))
                .clicked()
            {
                post = true;
            }
        });
        ui.add_space(10.0);
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            if items.is_empty() {
                ui.label(RichText::new("Nessun movimento pianificato.").color(MUTED));
                return;
            }
            egui::Grid::new("sched").striped(true).num_columns(7).spacing([18.0, 8.0]).show(ui, |ui| {
                for h in ["PROSSIMA", "CONTO", "DESCRIZIONE", "CATEGORIA", "IMPORTO", "RIPETIZIONE", ""] {
                    ui.label(RichText::new(h).size(11.0).color(MUTED));
                }
                ui.end_row();
                for s in &items {
                    let due = s.next_date <= db::today();
                    ui.label(RichText::new(fmt_date(s.next_date)).color(if due { AMBER } else { TEXT }));
                    ui.label(&s.account);
                    ui.label(&s.payee);
                    let cat = match s.to_account_id {
                        Some(to) => format!("[Giroconto] {}", self.account_name(to)),
                        None => s.category.clone(),
                    };
                    ui.label(RichText::new(cat).color(MUTED));
                    let amt = if s.to_account_id.is_some() { -s.amount.abs() } else { s.amount };
                    ui.label(money(amt));
                    ui.label(db::unit_label(&s.unit, s.every));
                    if ui.small_button("🗑").clicked() {
                        del = Some(s.id);
                    }
                    ui.end_row();
                }
            });
        });
        if post {
            let r = self.db.post_scheduled(db::today()).map(|n| format!("{n} movimenti registrati"));
            self.act(ui, r);
        }
        if let Some(id) = del {
            let r = self.db.delete_scheduled(id).map(|_| "Pianificato eliminato".to_string());
            self.act(ui, r);
        }
    }
}
