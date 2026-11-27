//! The budget made of envelopes: what is to be assigned, and for each envelope what it carried, what it was given,
//! what it spent and what is left. Arrows choose an envelope; typing an amount, or Enter, assigns it; Enter saves.

use crate::gui::{App, amount_color, card, category_combo, cut, or_report, pal, primary_button, section_title};
use cashcraft_engine::db::{Category, Cents};
use cashcraft_engine::envelopes::{ChangeKind, MonthView, Overspending, month_key};
use cashcraft_engine::i18n::{tr, trf, trn};
use cashcraft_engine::money::{fmt_eur, fmt_plain, month_and_year, parse_amount};
use chrono::NaiveDate;
use egui::{self, Align, Event, Key, Layout, Modifiers, RichText, Ui};

/// What the screen keeps between frames.
#[derive(Default)]
pub struct EnvelopeState {
    /// The month shown, as last read.
    pub view: Option<MonthView>,
    /// Read again at the next frame.
    pub stale: bool,
    /// The envelope chosen with the keyboard.
    pub cursor: usize,
    /// The envelope whose assigned amount is being typed, and the text.
    pub editing: Option<(i64, String)>,
    /// The field just opened still has to take the keyboard.
    pub focus: bool,
    /// Money being moved into an envelope: to, from, amount, reason.
    pub cover: Option<(i64, Option<i64>, String, String)>,
    /// The envelope whose settings are open, with the name typed and a category to add.
    pub settings: Option<(i64, String, Option<i64>)>,
    /// A new envelope: name and first category.
    pub new: (String, Option<i64>),
}

/// What the user asked for in a frame, done once the screen is drawn.
enum Do {
    Assign(i64, Cents),
    Move(i64, i64, Cents, String),
    Undo(Option<i64>),
    Budgets,
    Add(String, Option<i64>),
    Rename(i64, String),
    Categories(i64, Vec<i64>),
    Overspending(i64, Overspending),
    Delete(i64),
    Start,
}

fn label_amount(c: Cents) -> RichText {
    RichText::new(fmt_eur(c))
}

impl App {
    pub fn envelopes_view(&mut self, ui: &mut Ui, month: NaiveDate) {
        let st = &mut self.envelopes;
        if st.stale || st.view.as_ref().is_none_or(|v| v.month != month_key(month)) {
            st.stale = false;
            st.view = or_report(self.db.envelope_month(month).map(Some), &mut self.toast);
        }
        let Some(v) = st.view.clone() else { return };
        let mut todo: Option<Do> = None;
        let cats: Vec<Category> = self.categories.iter().filter(|c| !c.is_income).cloned().collect();
        let rows = v.envelopes.len();
        st.cursor = st.cursor.min(rows.saturating_sub(1));

        // The keyboard, when no field has it.
        if self.dialog.is_none() && st.editing.is_none() && !ui.ctx().egui_wants_keyboard_input() && rows > 0 {
            ui.input_mut(|i| {
                if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    st.cursor = (st.cursor + 1).min(rows - 1);
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    st.cursor = st.cursor.saturating_sub(1);
                }
                let line = v.envelopes.get(st.cursor);
                if let Some(l) = line {
                    if i.consume_key(Modifiers::NONE, Key::Enter) {
                        st.editing = Some((l.id, fmt_plain(l.assigned)));
                        st.focus = true;
                    }
                    // Typing an amount starts it.
                    let typed: String = i
                        .events
                        .iter()
                        .filter_map(|e| match e {
                            Event::Text(t) if t.chars().all(|c| c.is_ascii_digit() || ",.-".contains(c)) => {
                                Some(t.clone())
                            }
                            _ => None,
                        })
                        .collect();
                    if !typed.is_empty() {
                        i.events.retain(|e| !matches!(e, Event::Text(_)));
                        st.editing = Some((l.id, typed));
                        st.focus = true;
                    }
                }
            });
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if v.before_start {
                card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(trf("The envelopes start in {month}.", &[("month", &v.start)]));
                    if ui.button(tr("Start them in this month")).clicked() {
                        todo = Some(Do::Start);
                    }
                });
                return;
            }
            // What is to be assigned.
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                let left = v.to_be_assigned;
                ui.horizontal(|ui| {
                    section_title(ui, tr("To be assigned"));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (text, color) = match left {
                            0 => (tr("Every euro has a job").to_string(), pal().green),
                            l if l < 0 => (trf("Over-assigned by {amount}", &[("amount", &fmt_eur(-l))]), pal().red),
                            _ => (tr("Still to assign").to_string(), pal().amber),
                        };
                        ui.label(RichText::new(text).color(color));
                        ui.label(RichText::new(fmt_eur(left)).size(22.0).strong().color(color));
                    });
                });
                let mut parts = vec![
                    trf("left from before {amount}", &[("amount", &fmt_eur(v.left_over))]),
                    trf("income {amount}", &[("amount", &fmt_eur(v.income))]),
                    trf("assigned {amount}", &[("amount", &fmt_eur(v.assigned))]),
                ];
                if v.overspent_before != 0 {
                    parts.push(trf("overspent last month {amount}", &[("amount", &fmt_eur(v.overspent_before))]));
                }
                ui.label(RichText::new(parts.join(" · ")).color(pal().muted).size(12.5));
                let warn = |ui: &mut Ui, t: String| {
                    ui.label(RichText::new(format!("⚠ {t}")).color(pal().amber).size(12.5));
                };
                if v.outside != 0 {
                    warn(
                        ui,
                        trf(
                            "{amount} spent in categories without an envelope, taken from here",
                            &[("amount", &fmt_eur(v.outside))],
                        ),
                    );
                }
                if v.uncategorized != 0 {
                    warn(
                        ui,
                        trf("{amount} of uncategorized money, taken from here", &[("amount", &fmt_eur(v.uncategorized))]),
                    );
                }
                if v.other_currencies > 0 {
                    let n = usize::try_from(v.other_currencies).unwrap_or(usize::MAX);
                    warn(
                        ui,
                        trn(
                            n,
                            "{n} amount in another currency is not in the envelopes",
                            "{n} amounts in other currencies are not in the envelopes",
                        ),
                    );
                }
                if v.past {
                    ui.label(
                        RichText::new(tr("A past month: what changes here is carried into every month after it."))
                            .color(pal().muted)
                            .size(12.5),
                    );
                }
            });
            ui.add_space(10.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                if v.envelopes.is_empty() {
                    ui.label(RichText::new(tr("No envelopes: add one below.")).color(pal().muted));
                }
                egui::Grid::new("envelopes").striped(true).num_columns(7).spacing([14.0, 6.0]).show(ui, |ui| {
                    if !v.envelopes.is_empty() {
                        for h in [tr("Envelope"), tr("Categories"), tr("Carried"), tr("Assigned"), tr("Spent"), tr("Available"), ""] {
                            ui.label(RichText::new(h.to_uppercase()).size(11.0).color(pal().muted));
                        }
                        ui.end_row();
                    }
                    for (i, l) in v.envelopes.iter().enumerate() {
                        let here = st.cursor == i;
                        let name = format!("{}{}", if here { "▶ " } else { "" }, cut(&l.name, 28));
                        let name = if here { RichText::new(name).strong() } else { RichText::new(name) };
                        if ui.add(egui::Label::new(name).sense(egui::Sense::click())).clicked() {
                            st.cursor = i;
                        }
                        let categories = if l.categories.is_empty() { tr("none").to_string() } else { l.categories.join(", ") };
                        ui.label(RichText::new(cut(&categories, 34)).color(pal().muted).size(12.5));
                        ui.label(label_amount(l.carried).color(amount_color(l.carried.min(0))));
                        match &mut st.editing {
                            Some((id, text)) if *id == l.id => {
                                let r = ui.add(egui::TextEdit::singleline(text).desired_width(90.0));
                                if st.focus {
                                    r.request_focus();
                                    st.focus = false;
                                }
                                let (enter, escape, down, up) = ui.input_mut(|i| {
                                    (
                                        i.consume_key(Modifiers::NONE, Key::Enter),
                                        i.consume_key(Modifiers::NONE, Key::Escape),
                                        i.consume_key(Modifiers::NONE, Key::ArrowDown),
                                        i.consume_key(Modifiers::NONE, Key::ArrowUp),
                                    )
                                });
                                let save = enter || down || up || (r.lost_focus() && !escape);
                                if escape {
                                    st.editing = None;
                                } else if save {
                                    match parse_amount(text) {
                                        Ok(c) => todo = Some(Do::Assign(l.id, c)),
                                        Err(e) => self.toast = Some((format!("{e:#}"), ui.input(|i| i.time) + 4.0)),
                                    }
                                    st.editing = None;
                                    if down {
                                        st.cursor = (i + 1).min(rows - 1);
                                    }
                                    if up {
                                        st.cursor = i.saturating_sub(1);
                                    }
                                }
                            }
                            _ => {
                                let shown = label_amount(l.assigned);
                                if ui.add(egui::Label::new(shown).sense(egui::Sense::click())).clicked() {
                                    st.cursor = i;
                                    st.editing = Some((l.id, fmt_plain(l.assigned)));
                                    st.focus = true;
                                }
                            }
                        }
                        ui.label(label_amount(l.spent));
                        let mut available = label_amount(l.available).color(amount_color(l.available)).strong();
                        if l.available < 0 {
                            available = available.background_color(pal().warn_bg);
                        }
                        ui.label(available);
                        ui.horizontal(|ui| {
                            if l.available < 0 && ui.small_button(tr("Cover…")).clicked() {
                                st.cover = Some((l.id, None, fmt_plain(-l.available), String::new()));
                            }
                            if ui.small_button("⚙").on_hover_text(tr("Settings of the envelope")).clicked() {
                                st.settings = Some((l.id, l.name.clone(), None));
                            }
                        });
                        ui.end_row();
                    }
                });
                ui.label(
                    RichText::new(tr("Arrows up and down choose an envelope · type an amount or press Enter to assign it · Enter saves · Esc cancels"))
                        .color(pal().muted)
                        .size(12.0),
                );
                // Covering an overspent envelope.
                let mut cancel = false;
                if let Some((to, from, amount, reason)) = &mut st.cover {
                    ui.separator();
                    let to_name = v.envelopes.iter().find(|e| e.id == *to).map(|e| e.name.clone()).unwrap_or_default();
                    ui.horizontal_wrapped(|ui| {
                        ui.label(trf("Move money into \"{name}\" from", &[("name", &to_name)]));
                        let shown = from
                            .and_then(|f| v.envelopes.iter().find(|e| e.id == f))
                            .map_or_else(|| tr("Choose an envelope…").to_string(), |e| e.name.clone());
                        egui::ComboBox::from_id_salt("cover_from").selected_text(shown).show_ui(ui, |ui| {
                            for e in v.envelopes.iter().filter(|e| e.id != *to) {
                                let text = format!("{} ({})", e.name, fmt_eur(e.available));
                                ui.selectable_value(from, Some(e.id), text);
                            }
                        });
                        ui.add(egui::TextEdit::singleline(amount).desired_width(90.0));
                        ui.add(egui::TextEdit::singleline(reason).hint_text(tr("reason")).desired_width(160.0));
                        if let (Some(f), Ok(a)) = (*from, parse_amount(amount))
                            && primary_button(ui, tr("Move")).clicked()
                        {
                            todo = Some(Do::Move(f, *to, a, reason.clone()));
                        }
                        if ui.button(tr("Cancel")).clicked() {
                            cancel = true;
                        }
                    });
                }
                if cancel {
                    st.cover = None;
                }
                // The settings of an envelope.
                if let Some((id, name, add)) = &mut st.settings
                    && let Some(e) = v.envelopes.iter().find(|e| e.id == *id)
                {
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tr("Name:"));
                        ui.add(egui::TextEdit::singleline(name).desired_width(160.0));
                        if name.trim() != e.name && ui.button(tr("Rename")).clicked() {
                            todo = Some(Do::Rename(e.id, name.clone()));
                        }
                    });
                    let own = or_report(self.db.envelopes(), &mut self.toast)
                        .into_iter()
                        .find(|x| x.id == e.id)
                        .map(|x| x.category_ids)
                        .unwrap_or_default();
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tr("Categories (with those under them):"));
                        for c in &own {
                            let path = cats.iter().find(|x| x.id == *c).map(|x| x.path.clone()).unwrap_or_default();
                            if ui.small_button(format!("{path} ✕")).on_hover_text(tr("Take it out")).clicked() {
                                todo = Some(Do::Categories(e.id, own.iter().copied().filter(|x| x != c).collect()));
                            }
                        }
                        category_combo(ui, "envelope_add_category", &cats, add, tr("Add a category…"));
                        if let Some(c) = *add
                            && ui.button(tr("Add")).clicked()
                        {
                            let mut all = own.clone();
                            all.push(c);
                            todo = Some(Do::Categories(e.id, all));
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tr("When it is overspent:"));
                        for o in [Overspending::Carry, Overspending::NextMonth] {
                            if ui.selectable_label(e.overspending == o, o.label()).clicked() && e.overspending != o {
                                todo = Some(Do::Overspending(e.id, o));
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button(tr("Delete the envelope")).clicked() {
                            todo = Some(Do::Delete(e.id));
                        }
                        if ui.button(tr("Close")).clicked() {
                            st.settings = None;
                        }
                    });
                }
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label(tr("New envelope:"));
                    ui.add(egui::TextEdit::singleline(&mut st.new.0).hint_text(tr("name")).desired_width(160.0));
                    category_combo(ui, "envelope_new_category", &cats, &mut st.new.1, tr("Choose a category…"));
                    if !st.new.0.trim().is_empty() && primary_button(ui, tr("Add")).clicked() {
                        todo = Some(Do::Add(st.new.0.clone(), st.new.1));
                    }
                });
                ui.horizontal(|ui| {
                    if v.envelopes.iter().any(|e| e.budget != 0)
                        && ui
                            .button(tr("Assign the monthly budgets"))
                            .on_hover_text(tr("Each envelope gets the monthly budgets of its categories"))
                            .clicked()
                    {
                        todo = Some(Do::Budgets);
                    }
                    let undoable = v.changes.iter().any(|c| c.kind != ChangeKind::Undo && !c.undone);
                    if undoable && ui.button(tr("Undo the last change")).clicked() {
                        todo = Some(Do::Undo(None));
                    }
                });
            });
            if !v.changes.is_empty() {
                ui.add_space(10.0);
                card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.collapsing(tr("Changes of this month"), |ui| {
                        for c in &v.changes {
                            ui.horizontal(|ui| {
                                let from = c.from.clone().unwrap_or_else(|| tr("To be assigned").to_string());
                                let to = c.to.clone().unwrap_or_default();
                                let text = format!("{}  {from} → {to}  {}  {}", c.made_at, fmt_eur(c.amount), c.reason);
                                let color = if c.undone { pal().muted } else { pal().text };
                                ui.label(RichText::new(text).color(color).size(12.5));
                                if c.undone {
                                    ui.label(RichText::new(tr("undone")).color(pal().muted).size(12.0));
                                } else if c.kind != ChangeKind::Undo && ui.small_button(tr("Undo")).clicked() {
                                    todo = Some(Do::Undo(Some(c.group_id)));
                                }
                            });
                        }
                    });
                });
            }
        });

        let Some(todo) = todo else { return };
        let month_text = month_and_year(month);
        let r = match todo {
            Do::Assign(id, c) => {
                let before = or_report(self.db.fingerprints(month), &mut self.toast);
                self.db.assign(id, month, c, "").and_then(|g| {
                    let later = self.db.months_changed_after(month, &before)?;
                    Ok(match (g, later.is_empty()) {
                        (None, _) => String::new(),
                        (Some(_), true) => tr("Assigned").to_string(),
                        (Some(_), false) => trf("Assigned; recomputed: {months}", &[("months", &later.join(", "))]),
                    })
                })
            }
            Do::Move(from, to, a, reason) => {
                self.envelopes.cover = None;
                self.db.move_money(from, to, month, a, &reason).map(|_| tr("Money moved").to_string())
            }
            Do::Undo(g) => self.db.undo_envelope_change(g).map(|_| tr("Change undone").to_string()),
            Do::Budgets => self
                .db
                .assign_budgets(month)
                .map(|_| trf("Monthly budgets assigned in {month}", &[("month", &month_text)])),
            Do::Add(name, c) => {
                self.envelopes.new = (String::new(), None);
                let cats: Vec<i64> = c.into_iter().collect();
                self.db.add_envelope(&name, &cats, Overspending::Carry).map(|_| tr("Envelope added").to_string())
            }
            Do::Rename(id, name) => {
                self.db.edit_envelope(id, Some(&name), None, None).map(|_| tr("Envelope renamed").to_string())
            }
            Do::Categories(id, all) => {
                if let Some(s) = self.envelopes.settings.as_mut() {
                    s.2 = None;
                }
                self.db.edit_envelope(id, None, Some(&all), None).map(|_| tr("Categories saved").to_string())
            }
            Do::Overspending(id, o) => self.db.edit_envelope(id, None, None, Some(o)).map(|_| tr("Saved").to_string()),
            Do::Delete(id) => {
                self.envelopes.settings = None;
                self.db.delete_envelope(id).map(|_| tr("Envelope deleted").to_string())
            }
            Do::Start => self
                .db
                .set_envelopes_start(month)
                .map(|_| trf("The envelopes start in {month}", &[("month", &month_text)])),
        };
        self.act(ui, r);
        self.envelopes.stale = true;
    }
}
