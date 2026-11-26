//! Schermate della finestra: barra laterale, barra superiore, panoramica,
//! registro movimenti, report, budget, categorie, regole e pianificati.

use crate::gui::{
    AccountForm, App, Dialog, LANGUAGES, Period, SchedForm, Theme, TransferForm, View, amount_color, bars_chart,
    budget_color, card, category_combo, cut, donut, error_text, has_category, hbars, kind_icon, language_name, legend,
    line_chart, money, nav_item, or_report, pal, palette, primary_button, progress, section_title, small_caps,
};
use cashcraft_engine::db::{self, Account, AccountKind, Category, Cents, Transaction, TxKind};
use cashcraft_engine::i18n::{tr, trf, trn};
use cashcraft_engine::money::{
    fmt_date, fmt_day, fmt_eur, fmt_number, fmt_plain, month_and_year, month_name, parse_amount, total,
};
use chrono::{Datelike, Months};
use egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Frame, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, Ui,
    Vec2,
};

impl App {
    pub fn sidebar(&mut self, ui: &mut Ui) {
        egui::Panel::left("sidebar")
            .exact_size(264.0)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(pal().sidebar)
                    .inner_margin(Margin::symmetric(12, 14))
                    .stroke(Stroke::new(1.0, pal().border)),
            )
            .show(ui, |ui| {
                // Marchio: una moneta stilizzata.
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                    let p = ui.painter();
                    p.circle_filled(r.center(), 15.0, pal().accent_fill);
                    p.circle_stroke(r.center(), 11.0, Stroke::new(1.5, pal().logo_ring));
                    p.text(r.center(), Align2::CENTER_CENTER, "€", FontId::proportional(16.0), Color32::WHITE);
                    ui.label(RichText::new("CashCraft").size(19.0).strong().color(pal().text));
                });
                ui.add_space(12.0);
                let list_h = (ui.available_height() - 112.0).max(100.0);
                egui::ScrollArea::vertical()
                    .id_salt("sidebar_scroll")
                    .auto_shrink([false, false])
                    .max_height(list_h)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        if nav_item(ui, self.view == View::Dashboard, "🏠", tr("Overview"), None).clicked() {
                            self.go(View::Dashboard);
                        }
                        if nav_item(ui, self.view == View::Register(None), "📋", tr("All transactions"), None).clicked()
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
                            let total = total(group.iter().map(|a| a.balance));
                            ui.horizontal(|ui| {
                                small_caps(ui, kind.group_label());
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(RichText::new(fmt_eur(total)).size(11.0).color(pal().muted));
                                });
                            });
                            for a in group {
                                let r = nav_item(
                                    ui,
                                    self.view == View::Register(Some(a.id)),
                                    kind_icon(a.kind),
                                    &a.name,
                                    Some((fmt_eur(a.balance), if a.balance < 0 { pal().red } else { pal().text })),
                                );
                                if r.clicked() {
                                    self.go(View::Register(Some(a.id)));
                                }
                                let a2 = a.clone();
                                r.context_menu(|ui| {
                                    if ui.button(tr("✏ Edit account…")).clicked() {
                                        self.dialog = Some(Dialog::Account(AccountForm {
                                            id: Some(a2.id),
                                            name: a2.name.clone(),
                                            kind: a2.kind,
                                            opening: fmt_plain(a2.opening_balance),
                                            currency: a2.currency.clone(),
                                            old_currency: a2.currency.clone(),
                                            iban: a2.iban.clone(),
                                            archived: a2.archived,
                                            error: String::new(),
                                        }));
                                    }
                                    if ui.button(tr("⬇ Import a statement into this account…")).clicked() {
                                        self.go(View::Register(Some(a2.id)));
                                        self.open_import(None);
                                    }
                                    if ui.button(RichText::new(tr("🗑 Delete account…")).color(pal().red)).clicked()
                                    {
                                        let rules = or_report(self.db.rules_of_account(a2.id), &mut self.toast);
                                        self.dialog = Some(Dialog::ConfirmDeleteAccount(a2.id, a2.name.clone(), rules));
                                    }
                                });
                            }
                        }
                        ui.add_space(6.0);
                        if ui
                            .add(egui::Button::new(RichText::new(tr("+ New account")).color(pal().accent)).frame(false))
                            .clicked()
                        {
                            self.dialog = Some(Dialog::Account(AccountForm {
                                id: None,
                                name: String::new(),
                                kind: AccountKind::Bank,
                                opening: fmt_plain(0),
                                currency: "EUR".into(),
                                old_currency: String::new(),
                                iban: String::new(),
                                archived: false,
                                error: String::new(),
                            }));
                        }
                        ui.add_space(12.0);
                        small_caps(ui, tr("Analysis"));
                        if nav_item(ui, self.view == View::Reports, "📊", tr("Reports"), None).clicked() {
                            self.go(View::Reports);
                        }
                        if nav_item(ui, self.view == View::Budget, "🎯", tr("Budget"), None).clicked() {
                            self.go(View::Budget);
                        }
                        if nav_item(ui, self.view == View::Scheduled, "📅", tr("Scheduled"), None).clicked() {
                            self.go(View::Scheduled);
                        }
                        ui.add_space(12.0);
                        small_caps(ui, tr("Organize"));
                        if nav_item(ui, self.view == View::Categories, "🏷", tr("Categories"), None).clicked() {
                            self.go(View::Categories);
                        }
                        if nav_item(ui, self.view == View::Rules, "⚙", tr("Automatic rules"), None).clicked() {
                            self.go(View::Rules);
                        }
                        if nav_item(ui, self.view == View::Ledger, "📖", tr("Chart of accounts"), None).clicked() {
                            self.go(View::Ledger);
                        }
                    });
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    let total = self.net_worth;
                    ui.label(RichText::new(fmt_eur(total)).size(21.0).strong().color(if total < 0 {
                        pal().red
                    } else {
                        pal().text
                    }));
                    small_caps(ui, tr("Net worth"));
                    ui.separator();
                    let mut chosen = self.language;
                    let mut theme = self.theme;
                    // Bottom-up: the language row is drawn first, below the theme row.
                    ui.horizontal(|ui| {
                        setting_label(ui, tr("Language"));
                        egui::ComboBox::from_id_salt("language").selected_text(language_name(chosen)).show_ui(
                            ui,
                            |ui| {
                                for l in LANGUAGES {
                                    ui.selectable_value(&mut chosen, l, language_name(l));
                                }
                            },
                        );
                    });
                    ui.horizontal(|ui| {
                        setting_label(ui, tr("Theme"));
                        egui::ComboBox::from_id_salt("theme").selected_text(theme.name()).show_ui(ui, |ui| {
                            for t in Theme::ALL {
                                ui.selectable_value(&mut theme, t, t.name());
                            }
                        });
                    });
                    if chosen != self.language {
                        self.choose_language(ui, chosen);
                    }
                    if theme != self.theme {
                        self.choose_theme(ui, theme);
                    }
                });
            });
    }

    fn view_title(&self) -> (String, String) {
        match self.view {
            View::Dashboard => {
                let t = db::today();
                (tr("Overview").into(), month_and_year(t))
            }
            View::Register(None) => (tr("All transactions").into(), tr("All accounts").into()),
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
            View::Reports => (tr("Reports").into(), tr("Where your money goes").into()),
            View::Budget => (tr("Budget").into(), tr("Monthly spending limits by category").into()),
            View::Categories => (tr("Categories").into(), tr("How income and expenses are grouped").into()),
            View::Rules => (tr("Automatic rules").into(), tr("They categorize imported transactions").into()),
            View::Scheduled => (tr("Scheduled").into(), tr("Recurring transactions and due dates").into()),
            View::Ledger => (
                tr("Chart of accounts").into(),
                tr("Double-entry ledger: every amount has a debit and a credit").into(),
            ),
        }
    }

    pub fn toolbar(&mut self, ui: &mut Ui) {
        egui::Panel::top("toolbar")
            .frame(
                Frame::new()
                    .fill(pal().card)
                    .inner_margin(Margin::symmetric(22, 12))
                    .stroke(Stroke::new(1.0, pal().border)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (title, sub) = self.view_title();
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(RichText::new(title).size(22.0).strong().color(pal().text));
                        ui.label(RichText::new(sub).size(12.5).color(pal().muted));
                    });
                    if let View::Register(Some(a)) = self.view
                        && let Some(acc) = self.accounts.iter().find(|x| x.id == a)
                    {
                        ui.add_space(24.0);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.label(
                                RichText::new(fmt_eur(acc.balance))
                                    .size(22.0)
                                    .strong()
                                    .color(amount_color(acc.balance)),
                            );
                            ui.label(RichText::new(tr("Current balance")).size(12.5).color(pal().muted));
                        });
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .button(tr("⬇ Import"))
                            .on_hover_text(tr(
                                "Import a bank statement (⌘I). You can also drop the file on the window.",
                            ))
                            .clicked()
                        {
                            self.open_import(None);
                        }
                        if ui.button(tr("🔁 Transfer")).clicked() {
                            self.dialog = Some(Dialog::Transfer(TransferForm {
                                from: self.default_account(),
                                date: fmt_date(db::today()),
                                ..Default::default()
                            }));
                        }
                        if primary_button(ui, tr("+ Transaction")).on_hover_text(tr("New transaction (⌘N)")).clicked()
                        {
                            self.open_new_tx();
                        }
                    });
                });
                if self.demo {
                    ui.add_space(8.0);
                    Frame::new()
                        .fill(pal().warn_bg)
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(tr("ℹ You are looking at sample data, to try the app out."))
                                        .color(pal().warn_text),
                                );
                                if ui.link(tr("Delete it and start with your own accounts")).clicked() {
                                    self.dialog = Some(Dialog::ConfirmClearDemo);
                                }
                            });
                        });
                }
            });
    }

    pub fn status_bar(&mut self, ui: &mut Ui) {
        egui::Panel::bottom("status")
            .frame(
                Frame::new()
                    .fill(pal().sidebar)
                    .inner_margin(Margin::symmetric(14, 4))
                    .stroke(Stroke::new(1.0, pal().border)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    self.file_status(ui);
                    if self.is_shared() {
                        // The room is for the status.
                        return;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(tr(
                                "Works from the terminal (cashcraft-cli --help) and over MCP (cashcraft-cli mcp)",
                            ))
                            .size(11.5)
                            .color(pal().muted),
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
            let net = self.net_worth;
            let saving = d.month_income.saturating_sub(d.month_expense);
            let mname = month_name(db::today().month()).to_string();
            let cards = [
                (
                    tr("Net worth"),
                    fmt_eur(net),
                    if net < 0 { pal().red } else { pal().text },
                    tr("all accounts").to_string(),
                ),
                (tr("Income this month"), fmt_eur(d.month_income), pal().green, mname.clone()),
                (tr("Expenses this month"), fmt_eur(d.month_expense), pal().red, mname),
                (
                    tr("Saved this month"),
                    fmt_eur(saving),
                    amount_color(saving),
                    if saving < 0 {
                        tr("expenses above income").into()
                    } else if d.month_income > 0 {
                        {
                            let percent = fmt_number(saving as f64 * 100.0 / d.month_income as f64, 0);
                            trf("{percent}% of income", &[("percent", &percent)])
                        }
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
                            ui.label(RichText::new(sub).size(12.0).color(pal().muted));
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
                            section_title(ui, tr("Income and expenses"));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                legend(ui, &[(tr("Expenses"), pal().red), (tr("Income"), pal().green)]);
                            });
                        });
                        ui.label(RichText::new(tr("Last 6 months, transfers excluded")).size(12.0).color(pal().muted));
                        ui.add_space(6.0);
                        bars_chart(ui, &d.monthly, 216.0);
                    });
                });
                ui.allocate_ui(Vec2::new(half, 300.0), |ui| {
                    card(ui, |ui| {
                        ui.set_width(half - 30.0);
                        ui.set_min_height(266.0);
                        section_title(ui, tr("Expenses by category"));
                        ui.label(RichText::new(tr("This month")).size(12.0).color(pal().muted));
                        ui.add_space(6.0);
                        if d.top_expenses.is_empty() {
                            ui.label(RichText::new(tr("No expenses this month.")).color(pal().muted));
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
                            section_title(ui, tr("Latest transactions"));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.link(tr("See all")).clicked() {
                                    self.go(View::Register(None));
                                }
                            });
                        });
                        if d.uncategorized > 0 {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(trn(
                                        d.uncategorized,
                                        "⚫ {n} uncategorized transaction",
                                        "⚫ {n} uncategorized transactions",
                                    ))
                                    .size(12.5)
                                    .color(pal().amber),
                                );
                                if ui.link(tr("Sort them out")).clicked() {
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
                                ui.label(RichText::new(fmt_day(t.date)).size(12.5).color(pal().muted));
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    ui.label(RichText::new(&t.payee).size(13.5).color(pal().text));
                                    let sub = if t.category.is_empty() {
                                        tr("to categorize").to_string()
                                    } else {
                                        t.category.clone()
                                    };
                                    ui.label(
                                        RichText::new(format!("{sub} · {}", t.account)).size(11.5).color(pal().muted),
                                    );
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
                                section_title(ui, tr("Budget this month"));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.link(tr("Manage")).clicked() {
                                        self.go(View::Budget);
                                    }
                                });
                            });
                            if d.budgets.is_empty() {
                                ui.label(RichText::new(tr("No budget set.")).color(pal().muted));
                            }
                            for b in d.budgets.iter().take(5) {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(&b.category).size(13.0));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(
                                            RichText::new(format!("{} / {}", fmt_eur(b.spent), fmt_eur(b.budget)))
                                                .size(12.0)
                                                .color(pal().muted),
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
                            section_title(ui, tr("Upcoming"));
                            if d.upcoming.is_empty() {
                                ui.label(RichText::new(tr("No scheduled transactions.")).color(pal().muted));
                            }
                            for s in &d.upcoming {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(fmt_date(s.next_date)).size(12.5).color(pal().muted));
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
            if category_combo(ui, "catfilter", &cats, &mut self.cat_filter, tr("All categories")) {
                changed = true;
            }
            if ui.checkbox(&mut self.only_uncategorized, tr("Only uncategorized")).changed() {
                changed = true;
            }
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text(tr("🔍 Search payee, notes, amount…"))
                    .desired_width(270.0),
            );
            if r.changed() {
                changed = true;
            }
            if (!self.search.is_empty() || self.cat_filter.is_some() || self.only_uncategorized)
                && ui.button(tr("✖ Clear filters")).clicked()
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
        let (income, expense) = self.tx_totals;
        ui.horizontal(|ui| {
            ui.label(RichText::new(trn(self.txs.len(), "{n} transaction", "{n} transactions")).color(pal().muted));
            ui.label(RichText::new(tr("· Income")).color(pal().muted));
            ui.label(money(income));
            ui.label(RichText::new(tr("· Expenses")).color(pal().muted));
            ui.label(money(expense.saturating_neg()));
            ui.label(RichText::new(tr("· Difference")).color(pal().muted));
            ui.label(money(income.saturating_sub(expense)));
        });
        ui.add_space(6.0);

        let show_account = account.is_none();
        let show_balance =
            account.is_some() && self.search.is_empty() && self.cat_filter.is_none() && !self.only_uncategorized;
        let avail = ui.available_size();
        // In a narrow window the commands of the selected row take two lines.
        let bottom_bar = match self.selected {
            None => 0.0,
            Some(_) if avail.x < 1240.0 => 78.0,
            Some(_) => 44.0,
        };
        let table_h = (avail.y - bottom_bar).max(120.0);
        let mut open_edit: Option<Transaction> = None;
        let mut toggle_cleared: Option<(i64, bool)> = None;
        let mut delete: Option<Transaction> = None;
        let mut open_entry: Option<i64> = None;
        let mut turn: Option<Transaction> = None;
        let mut rule_from: Option<Transaction> = None;
        let mut set_cat: Option<(i64, Option<i64>)> = None;
        let mut want_import = false;
        let cats = self.categories.clone();
        Frame::new()
            .fill(pal().card)
            .stroke(Stroke::new(1.0, pal().border))
            .corner_radius(CornerRadius::same(10))
            .show(ui, |ui| {
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
                    (tr("Date"), fixed_date, false),
                    (tr("Payee"), flex * 0.36, false),
                    (tr("Category"), flex * 0.30, false),
                    (tr("Notes"), flex * 0.34, false),
                    (tr("Account"), fixed_acc, false),
                    (tr("Amount"), fixed_amt, true),
                    (tr("Balance"), fixed_bal, true),
                    ("✔", fixed_chk, true),
                ];
                let (hrect, _) = ui.allocate_exact_size(Vec2::new(w, 32.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(hrect, CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 }, pal().header);
                p.hline(hrect.x_range(), hrect.bottom(), Stroke::new(1.0, pal().border));
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
                    p.text(pos, al, name.to_uppercase(), FontId::proportional(11.0), pal().muted);
                    x += cw;
                }
                if self.txs.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(tr("No transactions in this period.")).color(pal().muted).size(15.0));
                        ui.add_space(8.0);
                        if ui.button(tr("⬇ Import a statement")).clicked() {
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
                                pal().accent_soft
                            } else if resp.hovered() {
                                pal().row_selected
                            } else if i % 2 == 1 {
                                pal().zebra
                            } else {
                                pal().card
                            };
                            p.rect_filled(rect, CornerRadius::ZERO, bg);
                            if resp.has_focus() {
                                p.rect_stroke(
                                    rect,
                                    CornerRadius::ZERO,
                                    Stroke::new(1.0, pal().accent),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            let mut x = rect.left() + 12.0;
                            let mut cell = |cw: f32, text: &str, color: Color32, right: bool, size: f32| {
                                if cw <= 0.0 {
                                    return;
                                }
                                let r = Rect::from_min_max(
                                    Pos2::new(x, rect.top()),
                                    Pos2::new(x + cw - 10.0, rect.bottom()),
                                );
                                let (pos, al) = if right {
                                    (r.right_center(), Align2::RIGHT_CENTER)
                                } else {
                                    (r.left_center(), Align2::LEFT_CENTER)
                                };
                                p.with_clip_rect(r).text(pos, al, text, FontId::proportional(size), color);
                                x += cw;
                            };
                            cell(cols[0].1, &fmt_date(t.date), pal().muted, false, 13.0);
                            cell(cols[1].1, &t.payee, pal().text, false, 13.5);
                            if t.category.is_empty() {
                                cell(cols[2].1, tr("— to categorize —"), pal().amber, false, 12.5);
                            } else {
                                cell(
                                    cols[2].1,
                                    &t.category,
                                    if t.kind == TxKind::Category { pal().text } else { pal().blue },
                                    false,
                                    13.0,
                                );
                            }
                            let tags: Vec<String> = t.tags.iter().map(|x| format!("#{x}")).collect();
                            let notes = format!("{} {}", tags.join(" "), t.memo);
                            cell(cols[3].1, notes.trim(), pal().muted, false, 12.5);
                            cell(cols[4].1, &t.account, pal().muted, false, 12.5);
                            cell(cols[5].1, &fmt_eur(t.amount), amount_color(t.amount), true, 13.5);
                            let bal = t.balance.map(fmt_eur).unwrap_or_default();
                            cell(
                                cols[6].1,
                                &bal,
                                if t.balance.unwrap_or(0) < 0 { pal().red } else { pal().text },
                                true,
                                13.0,
                            );
                            let chk_c = Pos2::new(x + fixed_chk / 2.0 - 6.0, rect.center().y);
                            if t.cleared {
                                p.circle_filled(chk_c, 7.0, pal().green_fill);
                                p.text(chk_c, Align2::CENTER_CENTER, "✔", FontId::proportional(10.0), Color32::WHITE);
                            } else {
                                p.circle_stroke(chk_c, 7.0, Stroke::new(1.2, pal().check_border));
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
                                if ui.button(tr("✏ Edit…")).clicked() {
                                    open_edit = Some(tc.clone());
                                }
                                let lbl =
                                    if tc.cleared { tr("Mark as not cleared") } else { tr("✔ Mark as cleared") };
                                if ui.button(lbl).clicked() {
                                    toggle_cleared = Some((tc.id, !tc.cleared));
                                }
                                if has_category(&tc) {
                                    ui.menu_button(tr("🏷 Category"), |ui| {
                                        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                                            if ui.button(tr("None")).clicked() {
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
                                if has_category(&tc) && rule_button(ui) {
                                    rule_from = Some(tc.clone());
                                }
                                if turn_button(ui, &tc) {
                                    turn = Some(tc.clone());
                                }
                                if !has_category(&tc) && ui.button(tr("📖 Entry…")).clicked() {
                                    open_entry = Some(tc.entry_id);
                                }
                                ui.separator();
                                if ui.button(RichText::new(tr("🗑 Delete")).color(pal().red)).clicked() {
                                    delete = Some(tc.clone());
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
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{} · {}", fmt_date(t.date), cut(&t.payee, 40))).strong());
                ui.label(money(t.amount));
                if has_category(&t) {
                    let mut c = t.category_id;
                    if category_combo(ui, "selcat", &cats, &mut c, tr("Uncategorized")) {
                        set_cat = Some((t.id, c));
                    }
                }
                if !t.rules.is_empty() {
                    let ids: Vec<String> = t.rules.ids().iter().map(i64::to_string).collect();
                    ui.label(
                        RichText::new(trf("rules {ids}", &[("ids", &ids.join(", "))])).color(pal().muted).size(12.0),
                    )
                    .on_hover_ui(|ui| {
                        ui.label(self.rules_of(&t).unwrap_or_else(|e| error_text(&e)));
                    });
                }
                if has_category(&t) && rule_button(ui) {
                    rule_from = Some(t.clone());
                }
                if turn_button(ui, &t) {
                    turn = Some(t.clone());
                }
                if !has_category(&t) && ui.button(tr("📖 Entry…")).clicked() {
                    open_entry = Some(t.entry_id);
                }
                if ui.button(tr("✏ Edit")).clicked() {
                    open_edit = Some(t.clone());
                }
                if ui.button(RichText::new(tr("🗑 Delete")).color(pal().red)).clicked() {
                    delete = Some(t.clone());
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
            let r = self.db.set_category(id, c).map(|_| tr("Category updated").to_string());
            self.act(ui, r);
        }
        if let Some(t) = rule_from {
            self.open_rule_from(&t);
        }
        if let Some(t) = turn {
            self.open_turn(&t);
        }
        if let Some(id) = open_entry {
            self.dialog = Some(Dialog::EntryDetail(id));
        }
        if let Some(t) = delete
            && !self.confirm_split(&t)
        {
            let r = self.db.delete_transaction(t.id).map(|_| tr("Transaction deleted").to_string());
            self.selected = None;
            self.act(ui, r);
        }
    }

    // -------------------------------------------------------------- report

    pub fn reports(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for p in Period::ALL {
                let on = self.report_period == p;
                let b = egui::Button::new(RichText::new(p.label()).color(if on { Color32::WHITE } else { pal().text }))
                    .fill(if on { pal().accent_fill } else { pal().track });
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
            let (income, expense) = or_report(db::CategoryTotal::income_and_expense(&r.totals), &mut self.toast);
            let net = income.saturating_sub(expense);
            let gap = 14.0;
            let cw = (ui.available_width() - gap * 3.0) / 4.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                let rate = if income > 0 {
                    format!("{}%", fmt_number(net as f64 * 100.0 / income as f64, 1))
                } else {
                    "—".into()
                };
                for (t, v, c) in [
                    (tr("Income"), fmt_eur(income), pal().green),
                    (tr("Expenses"), fmt_eur(expense), pal().red),
                    (tr("Saved"), fmt_eur(net), amount_color(net)),
                    (tr("Savings rate"), rate, pal().accent),
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
            let rest = total(exp.iter().skip(8).map(|x| x.1));
            if rest > 0 {
                top.push((tr("Other").into(), rest));
            }
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.allocate_ui(Vec2::new(half, 320.0), |ui| {
                    card(ui, |ui| {
                        ui.set_width(half - 30.0);
                        ui.set_min_height(290.0);
                        section_title(ui, tr("Expenses by category"));
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
                                            ui.label(
                                                RichText::new(format!("{:.0}%", pct)).size(12.0).color(pal().muted),
                                            );
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
                            section_title(ui, tr("Income and expenses by month"));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                legend(ui, &[(tr("Expenses"), pal().red), (tr("Income"), pal().green)]);
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
                section_title(ui, tr("Net worth over time (12 months)"));
                ui.add_space(6.0);
                line_chart(ui, &r.net_worth, 200.0);
            });
            ui.add_space(gap);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                section_title(ui, tr("Details by category"));
                ui.add_space(6.0);
                egui::Grid::new("cat_totals").striped(true).num_columns(4).min_col_width(140.0).show(ui, |ui| {
                    for h in [tr("Category"), tr("Type"), tr("Total"), tr("% of total")] {
                        ui.label(RichText::new(h.to_uppercase()).size(11.0).color(pal().muted));
                    }
                    ui.end_row();
                    for c in &r.totals {
                        ui.label(&c.category);
                        ui.label(
                            RichText::new(if c.total > 0 { tr("Money in") } else { tr("Money out") })
                                .color(pal().muted),
                        );
                        ui.label(money(c.total));
                        let base = if c.total > 0 { income } else { expense };
                        ui.label(format!("{}%", fmt_number(c.total.abs() as f64 * 100.0 / base.max(1) as f64, 1)));
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
            ui.label(RichText::new(month_and_year(self.budget_month)).size(17.0).strong());
            if ui.button("▶").clicked() {
                self.budget_month = self.budget_month.checked_add_months(Months::new(1)).unwrap_or(self.budget_month);
            }
            if self.budget_month != db::first_of_month(db::today()) && ui.link(tr("Current month")).clicked() {
                self.budget_month = db::first_of_month(db::today());
            }
        });
        ui.add_space(10.0);
        let lines = or_report(self.db.budget_status(self.budget_month), &mut self.toast);
        let total_b = total(lines.iter().map(|l| l.budget));
        let total_s = total(lines.iter().map(|l| l.spent));
        let mut save: Option<(i64, Cents)> = None;
        let mut remove: Option<i64> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    section_title(ui, tr("Total"));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let rem = total_b - total_s;
                        ui.label(
                            RichText::new(if rem >= 0 {
                                trf("{amount} left", &[("amount", &fmt_eur(rem))])
                            } else {
                                trf("{amount} over budget", &[("amount", &fmt_eur(-rem))])
                            })
                            .color(amount_color(rem)),
                        );
                        ui.label(
                            RichText::new(trf(
                                "{spent} spent of {budget}",
                                &[("spent", &fmt_eur(total_s)), ("budget", &fmt_eur(total_b))],
                            ))
                            .color(pal().muted),
                        );
                    });
                });
                progress(ui, total_s as f32 / total_b.max(1) as f32, budget_color(total_s, total_b));
            });
            ui.add_space(10.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                if lines.is_empty() {
                    ui.label(RichText::new(tr("No budgets: add one below.")).color(pal().muted));
                }
                for l in &lines {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&l.category).size(14.5).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("🗑").on_hover_text(tr("Remove budget")).clicked() {
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
                                    if ui.small_button("✏").on_hover_text(tr("Edit amount")).clicked() {
                                        self.budget_edit = Some((l.category_id, fmt_plain(l.budget)));
                                    }
                                }
                            }
                            let rem = l.budget - l.spent;
                            ui.label(
                                RichText::new(if rem >= 0 {
                                    trf("{amount} left", &[("amount", &fmt_eur(rem))])
                                } else {
                                    trf("{amount} over", &[("amount", &fmt_eur(-rem))])
                                })
                                .color(if rem >= 0 { pal().muted } else { pal().red }),
                            );
                            ui.label(RichText::new(format!("{} / {}", fmt_eur(l.spent), fmt_eur(l.budget))));
                        });
                    });
                    progress(ui, l.spent as f32 / l.budget.max(1) as f32, budget_color(l.spent, l.budget));
                    ui.add_space(8.0);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(tr("New budget:"));
                    let cats: Vec<Category> = self.categories.iter().filter(|c| !c.is_income).cloned().collect();
                    category_combo(ui, "newbudget", &cats, &mut self.new_budget.0, tr("Choose a category…"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_budget.1)
                            .hint_text(tr("amount a month"))
                            .desired_width(150.0),
                    );
                    if primary_button(ui, tr("Add")).clicked()
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
            let r = self.db.set_budget(c, v).map(|_| tr("Budget saved").to_string());
            self.act(ui, r);
        }
        if let Some(c) = remove {
            let r = self.db.set_budget(c, 0).map(|_| tr("Budget removed").to_string());
            self.act(ui, r);
        }
    }

    // -------------------------------------------------------------- categorie

    /// Asks first when rules go with it.
    pub fn delete_category(&mut self, ui: &Ui, id: i64) {
        match self.db.rules_of_category(id) {
            Ok(0) => {
                let r = self.db.delete_category(id).map(|_| tr("Category deleted").to_string());
                self.act(ui, r);
            }
            Ok(n) => {
                let name = self.categories.iter().find(|c| c.id == id).map(|c| c.path.clone()).unwrap_or_default();
                self.dialog = Some(Dialog::ConfirmDeleteCategory(id, name, n));
            }
            Err(e) => self.act(ui, Err(e)),
        }
    }

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
                    ui.label(tr("New category:"));
                    ui.add(egui::TextEdit::singleline(&mut self.new_cat.0).hint_text(tr("name")).desired_width(180.0));
                    ui.label(tr("inside"));
                    let parents: Vec<Category> = cats.iter().filter(|c| c.parent_id.is_none()).cloned().collect();
                    category_combo(ui, "newcatparent", &parents, &mut self.new_cat.2, tr("(none: top level)"));
                    ui.checkbox(&mut self.new_cat.1, tr("is income"));
                    if primary_button(ui, tr("Add")).clicked() && !self.new_cat.0.trim().is_empty() {
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
                            section_title(ui, if income { tr("Income") } else { tr("Expenses") });
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
                                            .on_hover_text(tr("Delete (its transactions stay, uncategorized)"))
                                            .clicked()
                                        {
                                            del = Some(c.id);
                                        }
                                        if ui.small_button("✏").on_hover_text(tr("Rename")).clicked() {
                                            self.rename_cat = Some((c.id, c.name.clone()));
                                        }
                                        if ui.small_button("📋").on_hover_text(tr("See the transactions")).clicked() {
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
            let r = self.db.category_by_path(&path, income, true).map(|_| tr("Category created").to_string());
            self.new_cat = (String::new(), false, None);
            self.act(ui, r);
        }
        if let Some((id, name)) = rename {
            self.rename_cat = None;
            let r = self.db.rename_category(id, &name).map(|_| tr("Category renamed").to_string());
            self.act(ui, r);
        }
        if let Some(id) = del {
            self.delete_category(ui, id);
        }
        if let Some(id) = show {
            self.cat_filter = Some(id);
            self.only_uncategorized = false;
            self.period = Period::All;
            self.go(View::Register(None));
            self.reload_txs();
        }
    }

    // -------------------------------------------------------------- pianificati

    pub fn scheduled_view(&mut self, ui: &mut Ui) {
        let items = or_report(self.db.scheduled(), &mut self.toast);
        let mut del: Option<i64> = None;
        let mut post = false;
        ui.horizontal(|ui| {
            if primary_button(ui, tr("+ New scheduled transaction")).clicked() {
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
                .add_enabled(due > 0, egui::Button::new(trf("✔ Record what is due up to today ({n})", &[("n", &due)])))
                .clicked()
            {
                post = true;
            }
        });
        ui.add_space(10.0);
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            if items.is_empty() {
                ui.label(RichText::new(tr("No scheduled transactions.")).color(pal().muted));
                return;
            }
            egui::Grid::new("sched").striped(true).num_columns(7).spacing([18.0, 8.0]).show(ui, |ui| {
                for h in
                    [tr("Next"), tr("Account"), tr("Description"), tr("Category"), tr("Amount"), tr("Repetition"), ""]
                {
                    ui.label(RichText::new(h.to_uppercase()).size(11.0).color(pal().muted));
                }
                ui.end_row();
                for s in &items {
                    let due = s.next_date <= db::today();
                    ui.label(RichText::new(fmt_date(s.next_date)).color(if due { pal().amber } else { pal().text }));
                    ui.label(&s.account);
                    ui.label(&s.payee);
                    let cat = match s.to_account_id {
                        Some(to) => trf("[Transfer] {account}", &[("account", &self.account_name(to))]),
                        None => s.category.clone(),
                    };
                    ui.label(RichText::new(cat).color(pal().muted));
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
            let r = self.db.post_scheduled(db::today()).map(|p| p.message());
            self.act(ui, r);
        }
        if let Some(id) = del {
            let r = self.db.delete_scheduled(id).map(|_| tr("Scheduled transaction deleted").to_string());
            self.act(ui, r);
        }
    }
}

/// The button that makes a rule of a transaction; whether it was clicked.
fn rule_button(ui: &mut Ui) -> bool {
    ui.button(tr("⚙ Always categorize like this…"))
        .on_hover_text(tr("Makes a rule for the transactions like this one, those that will be imported too."))
        .clicked()
}

/// The button that turns a transaction into a transfer, or separates one; whether it was clicked.
pub fn turn_button(ui: &mut Ui, t: &Transaction) -> bool {
    let text = match t.kind {
        TxKind::Split => return false,
        TxKind::Transfer => tr("🔀 Separate the transfer…"),
        _ => tr("🔁 Turn into transfer…"),
    };
    ui.button(text)
        .on_hover_text(tr("Money moved between two of your accounts is neither income nor an expense (⌘T)."))
        .clicked()
}

/// A setting's name in the sidebar, padded so the menus beside it line up.
fn setting_label(ui: &mut Ui, text: &str) {
    let width = ui.label(RichText::new(text).color(pal().muted)).rect.width();
    ui.add_space((72.0 - width).max(4.0));
}
