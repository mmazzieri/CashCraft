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
    /// The month the panels below are of: another month closes them, and what was being typed is dropped.
    pub month: Option<NaiveDate>,
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
    /// The deletion of this envelope waits for a yes.
    pub deleting: Option<i64>,
}

impl EnvelopeState {
    /// Closes what was open on one month: nothing typed there ever reaches another.
    pub fn leave(&mut self) {
        self.editing = None;
        self.cover = None;
        self.settings = None;
        self.deleting = None;
        self.month = None;
    }
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
        if st.month != Some(month) {
            st.leave();
            st.month = Some(month);
        }
        if st.stale || st.view.as_ref().is_none_or(|v| v.month != month_key(month)) {
            st.stale = false;
            st.view = or_report(self.db.envelope_month(month).map(Some), &mut self.toast);
        }
        let Some(v) = st.view.clone() else { return };
        let mut todo: Option<Do> = None;
        let cats: Vec<Category> = self.categories.iter().filter(|c| !c.is_income).cloned().collect();
        let rows = v.envelopes.len();
        st.cursor = st.cursor.min(rows.saturating_sub(1));
        // The last change of this month that can be undone: the changes come last first.
        let last = v.changes.iter().find(|c| c.kind != ChangeKind::Undo && !c.undone).map(|c| c.group_id);
        // Keys are the screen's while no text field has them; Enter is left to a button that has the keyboard.
        let free = self.dialog.is_none() && st.editing.is_none() && !ui.ctx().text_edit_focused();
        let nothing_focused = !ui.ctx().egui_wants_keyboard_input();
        if free
            && let Some(g) = last
            && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z))
        {
            todo = Some(Do::Undo(Some(g)));
        }

        // The keyboard, when no field has it.
        if free && rows > 0 {
            ui.input_mut(|i| {
                if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    st.cursor = (st.cursor + 1).min(rows - 1);
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    st.cursor = st.cursor.saturating_sub(1);
                }
                let line = v.envelopes.get(st.cursor);
                if let Some(l) = line {
                    if nothing_focused && i.consume_key(Modifiers::NONE, Key::Enter) {
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
                                let opening = st.focus;
                                if opening {
                                    r.request_focus();
                                    st.focus = false;
                                }
                                // Keys are the field's only while it has the keyboard; leaving it otherwise drops
                                // what was typed.
                                let focused = r.has_focus() || r.lost_focus();
                                let (enter, escape, down, up) = if focused && self.dialog.is_none() {
                                    ui.input_mut(|i| {
                                        (
                                            i.consume_key(Modifiers::NONE, Key::Enter),
                                            i.consume_key(Modifiers::NONE, Key::Escape),
                                            i.consume_key(Modifiers::NONE, Key::ArrowDown),
                                            i.consume_key(Modifiers::NONE, Key::ArrowUp),
                                        )
                                    })
                                } else {
                                    (false, false, false, false)
                                };
                                let save = enter || down || up;
                                if escape || (!save && !opening && !r.has_focus()) {
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
                                // From the envelope with the most left, which the user can change.
                                let richest = v
                                    .envelopes
                                    .iter()
                                    .filter(|e| e.id != l.id && e.available > 0)
                                    .max_by_key(|e| e.available)
                                    .map(|e| e.id);
                                st.cover = Some((l.id, richest, fmt_plain(-l.available), String::new()));
                            }
                            if ui.small_button("⚙").on_hover_text(tr("Settings of the envelope")).clicked() {
                                st.settings = Some((l.id, l.name.clone(), None));
                            }
                        });
                        ui.end_row();
                    }
                });
                ui.label(
                    RichText::new(tr("Arrows up and down choose an envelope · type an amount or press Enter to assign it · Enter saves · Esc cancels · ⌘Z undoes the last change of the month"))
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
                    let own = e.category_ids.clone();
                    ui.horizontal_wrapped(|ui| {
                        ui.label(trf(
                            "Categories from {month} on (with those under them):",
                            &[("month", &month_and_year(month))],
                        ));
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
                    if st.deleting == Some(e.id) {
                        ui.label(
                            RichText::new(trf(
                                "Delete \"{name}\"? In {month} it holds {amount}, which goes back to be assigned, and the spending of its categories counts outside the envelopes in every month. The deletion can be undone.",
                                &[("name", &e.name), ("month", &month_and_year(month)), ("amount", &fmt_eur(e.available))],
                            ))
                            .color(pal().red),
                        );
                        ui.horizontal(|ui| {
                            if ui.button(tr("Delete")).clicked() {
                                todo = Some(Do::Delete(e.id));
                            }
                            if ui.button(tr("Cancel")).clicked() {
                                st.deleting = None;
                            }
                        });
                    } else {
                        ui.horizontal(|ui| {
                            if ui.button(tr("Delete the envelope…")).clicked() {
                                st.deleting = Some(e.id);
                            }
                            if ui.button(tr("Close")).clicked() {
                                st.settings = None;
                            }
                        });
                    }
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
                    if let Some(g) = last
                        && ui.button(tr("Undo the last change of the month")).on_hover_text("⌘Z").clicked()
                    {
                        todo = Some(Do::Undo(Some(g)));
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
                                let text = format!("{}  {}  {}", c.made_at, c.text, c.reason);
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
        let before = or_report(self.db.fingerprints(), &mut self.toast);
        let r = match todo {
            Do::Assign(id, c) => self.db.assign(id, month, c, "").map(|g| match g {
                None => String::new(),
                Some(_) => tr("Assigned").to_string(),
            }),
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
                self.db.add_envelope(&name, &cats, Overspending::Carry, month).map(|_| tr("Envelope added").to_string())
            }
            Do::Rename(id, name) => {
                self.db.edit_envelope(id, Some(&name), None, None).map(|_| tr("Envelope renamed").to_string())
            }
            Do::Categories(id, all) => {
                if let Some(s) = self.envelopes.settings.as_mut() {
                    s.2 = None;
                }
                self.db
                    .edit_envelope(id, None, Some((&all, month)), None)
                    .map(|_| trf("Categories saved from {month} on", &[("month", &month_text)]))
            }
            Do::Overspending(id, o) => self.db.edit_envelope(id, None, None, Some(o)).map(|_| tr("Saved").to_string()),
            Do::Delete(id) => {
                self.envelopes.settings = None;
                self.envelopes.deleting = None;
                self.db.delete_envelope(id, month).map(|_| tr("Envelope deleted: ⌘Z brings it back").to_string())
            }
            Do::Start => self
                .db
                .set_envelopes_start(month, false)
                .map(|_| trf("The envelopes start in {month}", &[("month", &month_text)])),
        };
        // Which other months the change moved, whatever it was.
        let r = r.and_then(|said| {
            let mut later = self.db.months_changed(&before)?;
            later.retain(|m| *m != month_key(month));
            Ok(match (said.is_empty(), later.is_empty()) {
                (_, true) => said,
                (true, false) => trf("recomputed: {months}", &[("months", &later.join(", "))]),
                (false, false) => format!("{said}; {}", trf("recomputed: {months}", &[("months", &later.join(", "))])),
            })
        });
        self.act(ui, r);
        self.envelopes.stale = true;
    }
}
