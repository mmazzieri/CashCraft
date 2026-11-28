//! The projected balance in the window: the dashboard's card, the warning above an account's
//! register, and the scheduled view's day-by-day table, all from `cashcraft_engine::forecast`.

use crate::gui::{App, View, card, money, pal, section_title, short_eur};
use cashcraft_engine::db;
use cashcraft_engine::forecast::{DEFAULT_DAYS, Forecast};
use cashcraft_engine::i18n::{tr, trf};
use cashcraft_engine::money::{fmt_date, fmt_day, fmt_eur};
use egui::{self, Align, Align2, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};

impl App {
    /// The projections of the accounts in use, made once a day until the data changes; why they
    /// could not be made, which is never shown as "nothing ahead".
    pub(crate) fn ensure_forecasts(&mut self) -> Result<Vec<Forecast>, String> {
        let today = db::today();
        match &self.forecasts {
            Some((day, made)) if *day == today => made.clone(),
            _ => {
                let made = self.db.forecasts(today, DEFAULT_DAYS).map_err(|e| format!("{e:#}"));
                self.forecasts = Some((today, made.clone()));
                made
            }
        }
    }

    /// Why the projection is missing, with a way to try again.
    fn forecast_failed(&mut self, ui: &mut Ui, error: &str) {
        ui.label(
            RichText::new(trf("The projection could not be made: {error}", &[("error", &error)])).color(pal().red),
        );
        if ui.link(tr("Try again")).clicked() {
            self.forecasts = None;
        }
    }

    /// The account whose projection is drawn: the one chosen, else the first that goes below its
    /// minimum, else the one with most ahead.
    fn chosen_forecast<'a>(&self, list: &'a [Forecast]) -> Option<&'a Forecast> {
        self.forecast_account
            .and_then(|id| list.iter().find(|f| f.account_id == id))
            .or_else(|| list.iter().find(|f| f.shortfall.is_some()))
            .or_else(|| list.iter().max_by_key(|f| f.items.len()))
    }

    fn forecast_picker(&mut self, ui: &mut Ui, id: &str, list: &[Forecast], shown: &Forecast) {
        if list.len() < 2 {
            return;
        }
        egui::ComboBox::from_id_salt(id).selected_text(&shown.account).width(200.0).show_ui(ui, |ui| {
            for f in list {
                if ui.selectable_label(f.account_id == shown.account_id, &f.account).clicked() {
                    self.forecast_account = Some(f.account_id);
                }
            }
        });
    }

    /// The dashboard's card: which accounts go below their minimum and when, and a chart.
    pub(crate) fn forecast_card(&mut self, ui: &mut Ui) {
        let made = self.ensure_forecasts();
        let list = made.clone().unwrap_or_default();
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                section_title(ui, tr("Projected balance"));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.link(tr("Day by day")).clicked() {
                        self.go(View::Scheduled);
                    }
                    if let Some(f) = self.chosen_forecast(&list) {
                        let f = f.clone();
                        self.forecast_picker(ui, "dashforecast", &list, &f);
                    }
                });
            });
            ui.label(RichText::new(projection_note()).size(12.0).color(pal().muted));
            ui.add_space(4.0);
            if let Err(e) = &made {
                self.forecast_failed(ui, e);
                return;
            }
            let short: Vec<&Forecast> = list.iter().filter(|f| f.shortfall.is_some()).collect();
            if short.is_empty() {
                ui.label(RichText::new(tr("No account goes below its minimum.")).color(pal().green));
            }
            for f in short {
                ui.label(RichText::new(format!("⚠ {}", f.message())).color(pal().red).strong());
            }
            if let Some(f) = self.chosen_forecast(&list) {
                ui.add_space(6.0);
                forecast_chart(ui, f, 170.0);
            }
        });
    }

    /// The warning above an account's register when its projection goes below the minimum.
    pub(crate) fn forecast_banner(&mut self, ui: &mut Ui, account_id: i64) {
        // A failure is said on the overview and in the scheduled view.
        let list = self.ensure_forecasts().unwrap_or_default();
        let Some(f) = list.iter().find(|f| f.account_id == account_id && f.shortfall.is_some()) else { return };
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("⚠ {}", f.message())).color(pal().red).strong());
            if ui.link(tr("Day by day")).clicked() {
                self.forecast_account = Some(account_id);
                self.go(View::Scheduled);
            }
        });
        ui.add_space(4.0);
    }

    /// The scheduled view's projection of one account: chart and table, the days below the
    /// minimum in red.
    pub(crate) fn projection(&mut self, ui: &mut Ui) {
        let list = match self.ensure_forecasts() {
            Ok(list) => list,
            Err(e) => {
                card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    section_title(ui, tr("Projected balance"));
                    self.forecast_failed(ui, &e);
                });
                return;
            }
        };
        let Some(f) = self.chosen_forecast(&list).cloned() else { return };
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                section_title(ui, tr("Projected balance"));
                self.forecast_picker(ui, "schedforecast", &list, &f);
            });
            ui.label(RichText::new(projection_note()).size(12.0).color(pal().muted));
            let color = if f.shortfall.is_some() { pal().red } else { pal().text };
            ui.label(RichText::new(f.message()).color(color).strong());
            ui.add_space(6.0);
            forecast_chart(ui, &f, 190.0);
            ui.add_space(8.0);
            egui::Grid::new("projection").striped(true).num_columns(4).spacing([18.0, 6.0]).show(ui, |ui| {
                for h in [tr("Date"), tr("Description"), tr("Amount"), tr("Balance after")] {
                    ui.label(RichText::new(h.to_uppercase()).size(11.0).color(pal().muted));
                }
                ui.end_row();
                ui.label(fmt_date(f.from));
                ui.label(RichText::new(tr("Balance today")).color(pal().muted));
                ui.label("");
                ui.label(money(f.balance));
                ui.end_row();
                for i in &f.items {
                    let below = f.minimum.is_some_and(|m| i.balance < m);
                    ui.label(fmt_date(i.date));
                    let payee = match i.overdue {
                        0 => i.payee.clone(),
                        1 => trf("{payee} (overdue)", &[("payee", &i.payee)]),
                        n => trf("{payee} ({n} due dates overdue)", &[("payee", &i.payee), ("n", &n)]),
                    };
                    ui.label(RichText::new(payee).color(if i.overdue > 0 { pal().amber } else { pal().text }));
                    ui.label(money(i.amount));
                    let b = RichText::new(fmt_eur(i.balance));
                    ui.label(if below { b.color(pal().red).strong() } else { b.color(pal().text) });
                    ui.end_row();
                }
            });
            if f.items.is_empty() {
                ui.label(RichText::new(tr("Nothing is due on this account in the coming days.")).color(pal().muted));
            }
        });
    }
}

fn projection_note() -> String {
    trf(
        "Next {n} days: today's balance, the transactions already in the books with a later date and the scheduled ones. Nothing else is foreseen.",
        &[("n", &DEFAULT_DAYS)],
    )
}

/// The balance at the end of each day, with the minimum as a dashed line and the first day below it
/// marked.
pub fn forecast_chart(ui: &mut Ui, f: &Forecast, height: f32) {
    let data = f.daily();
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, height), Sense::hover());
    let p = ui.painter_at(rect);
    let values = data.iter().map(|x| x.1).chain(f.minimum);
    let (Some(min), Some(max)) = (values.clone().min(), values.max()) else { return };
    let (min, max) = (min as f32, max as f32);
    let (min, max) = if (max - min).abs() < 1.0 { (min - 100.0, max + 100.0) } else { (min, max) };
    let pad = (max - min) * 0.12;
    let (min, max) = (min - pad, max + pad);
    let plot = Rect::from_min_max(rect.min + Vec2::new(60.0, 8.0), rect.max - Vec2::new(10.0, 22.0));
    let y_of = |v: f32| plot.bottom() - plot.height() * ((v - min) / (max - min));
    for i in 0..=3 {
        let y = plot.bottom() - plot.height() * i as f32 / 3.0;
        p.hline(plot.x_range(), y, Stroke::new(1.0, pal().grid));
        let v = min + (max - min) * i as f32 / 3.0;
        p.text(
            Pos2::new(plot.left() - 6.0, y),
            Align2::RIGHT_CENTER,
            short_eur(v as i64),
            FontId::proportional(10.5),
            pal().muted,
        );
    }
    let n = data.len();
    let x_of = |i: usize| if n <= 1 { plot.center().x } else { plot.left() + plot.width() * i as f32 / (n - 1) as f32 };
    let pts: Vec<Pos2> = data.iter().enumerate().map(|(i, (_, v))| Pos2::new(x_of(i), y_of(*v as f32))).collect();
    if let Some(m) = f.minimum {
        let y = y_of(m as f32);
        p.extend(egui::Shape::dashed_line(
            &[Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            Stroke::new(1.5, pal().red),
            6.0,
            4.0,
        ));
        p.text(
            Pos2::new(plot.right(), y - 3.0),
            Align2::RIGHT_BOTTOM,
            trf("minimum {amount}", &[("amount", &short_eur(m))]),
            FontId::proportional(10.5),
            pal().red,
        );
    }
    p.line(pts.clone(), Stroke::new(2.0, pal().accent));
    // A few dates along the bottom, so that a daily line stays readable.
    let ticks = 5usize.min(n);
    for k in 0..ticks {
        let i = if ticks <= 1 { 0 } else { k * (n - 1) / (ticks - 1) };
        if let Some((d, _)) = data.get(i) {
            p.text(
                Pos2::new(x_of(i), plot.bottom() + 11.0),
                // The first and the last inside the chart.
                match k {
                    0 => Align2::LEFT_CENTER,
                    _ if k + 1 == ticks => Align2::RIGHT_CENTER,
                    _ => Align2::CENTER_CENTER,
                },
                fmt_day(*d),
                FontId::proportional(11.0),
                pal().muted,
            );
        }
    }
    if let Some(s) = &f.shortfall
        && let Some(i) = data.iter().position(|(d, _)| *d == s.date)
        && let Some(pt) = pts.get(i)
    {
        p.circle_filled(*pt, 4.5, pal().red);
    }
    if let Some(h) = resp.hover_pos()
        && let Some((pt, (d, v))) =
            pts.iter().zip(&data).min_by(|(a, _), (b, _)| (a.x - h.x).abs().total_cmp(&(b.x - h.x).abs()))
    {
        p.circle_stroke(*pt, 6.0, Stroke::new(2.0, pal().accent));
        p.text(
            *pt - Vec2::new(0.0, 12.0),
            Align2::CENTER_BOTTOM,
            format!("{} · {}", fmt_date(*d), fmt_eur(*v)),
            FontId::proportional(12.0),
            pal().text,
        );
    }
    let message = f.message();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, &message));
}
