//! The value of an investment account, above its register: what it is worth, what was put in and what it gained, the
//! values written so far and a row to write today's, all from `cashcraft_engine::valuations`.

use crate::gui::{App, amount_color, card, error_text, money, pal, primary_button, small_caps};
use cashcraft_engine::db::{self, AccountKind};
use cashcraft_engine::i18n::{tr, trf};
use cashcraft_engine::money::{fmt_date, fmt_eur, parse_amount, parse_date};
use cashcraft_engine::valuations::Performance;
use egui::{self, RichText, Ui};

/// What is typed in the row of a new value, and the values of the account shown, read again when the data changes.
#[derive(Default)]
pub struct ValuesState {
    pub account: Option<i64>,
    pub read: Option<Result<Performance, String>>,
    pub date: String,
    pub value: String,
    pub error: String,
    /// The list of values is open.
    pub open: bool,
}

impl App {
    /// The card above the register of an investment account.
    pub(crate) fn values_card(&mut self, ui: &mut Ui, account_id: i64) {
        let investment = self.accounts.iter().any(|a| a.id == account_id && a.kind == AccountKind::Investment);
        if !investment {
            return;
        }
        if self.values.account != Some(account_id) {
            self.values = ValuesState { account: Some(account_id), ..Default::default() };
        }
        if self.values.date.is_empty() {
            self.values.date = fmt_date(db::today());
        }
        if self.values.read.is_none() {
            self.values.read = Some(self.db.performance(account_id).map_err(|e| format!("{e:#}")));
        }
        let read = self.values.read.clone();
        let mut delete = None;
        let mut save = false;
        card(ui, |ui| {
            ui.set_width(ui.available_width());
            let p = match read {
                Some(Ok(p)) => p,
                Some(Err(e)) => {
                    ui.label(
                        RichText::new(trf("The value of the account cannot be read: {error}", &[("error", &e)]))
                            .color(pal().red),
                    );
                    if ui.link(tr("Try again")).clicked() {
                        self.values.read = None;
                    }
                    return;
                }
                None => return,
            };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 28.0;
                figure(ui, tr("Worth"), RichText::new(fmt_eur(p.balance)).color(pal().text));
                figure(ui, tr("Put in"), RichText::new(fmt_eur(p.invested)).color(pal().text));
                let gain = match percent(p.gain, p.invested) {
                    Some(pct) => format!("{} ({pct})", fmt_eur(p.gain)),
                    None => fmt_eur(p.gain),
                };
                figure(ui, tr("Gain"), RichText::new(gain).color(amount_color(p.gain)));
            });
            ui.add_space(2.0);
            let note = match &p.last {
                Some(v) => trf("Last value written: {date}.", &[("date", &fmt_date(v.date))]).to_string(),
                None => tr("No value written yet: the balance is the money put in.").to_string(),
            };
            ui.label(RichText::new(note).size(12.0).color(pal().muted));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(tr("Worth on"));
                ui.add(egui::TextEdit::singleline(&mut self.values.date).desired_width(100.0))
                    .on_hover_text(tr("The day of the value, as the bank or the platform shows it"));
                ui.label("€");
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.values.value).hint_text(cashcraft_engine::money::fmt_plain(0)).desired_width(120.0),
                );
                let entered = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if primary_button(ui, tr("Write the value"))
                    .on_hover_text(tr(
                        "The balance becomes this value; the difference is a change in value, neither income nor spending",
                    ))
                    .clicked()
                    || entered
                {
                    save = true;
                }
            });
            if !self.values.error.is_empty() {
                ui.label(RichText::new(&self.values.error).color(pal().red));
            }
            if !p.valuations.is_empty() {
                ui.add_space(4.0);
                let n = p.valuations.len();
                let title = trf("Values written ({n})", &[("n", &n)]);
                egui::CollapsingHeader::new(title).id_salt(("values", account_id)).default_open(self.values.open).show(
                    ui,
                    |ui| {
                        egui::ScrollArea::vertical().id_salt(("valuescroll", account_id)).max_height(180.0).show(
                            ui,
                            |ui| {
                                egui::Grid::new(("valuegrid", account_id))
                                    .striped(true)
                                    .num_columns(4)
                                    .spacing([18.0, 4.0])
                                    .show(ui, |ui| {
                                        for h in [tr("Date"), tr("Value"), tr("Change in value"), ""] {
                                            small_caps(ui, h);
                                        }
                                        ui.end_row();
                                        for v in p.valuations.iter().rev() {
                                            ui.label(fmt_date(v.date));
                                            ui.label(fmt_eur(v.balance));
                                            ui.label(money(v.amount));
                                            if ui.small_button("🗑").on_hover_text(tr("Delete this value")).clicked()
                                            {
                                                delete = Some(v.date);
                                            }
                                            ui.end_row();
                                        }
                                    });
                            },
                        );
                    },
                );
            }
        });
        ui.add_space(8.0);
        if save {
            let written = (|| -> anyhow::Result<String> {
                let date = parse_date(&self.values.date).ok_or_else(|| anyhow::anyhow!(tr("the date is not valid")))?;
                let value = parse_amount(&self.values.value)?;
                let v = self.db.record_valuation(account_id, date, value)?;
                Ok(trf("Worth {value} on {date}", &[("value", &fmt_eur(v.balance)), ("date", &fmt_date(v.date))])
                    .to_string())
            })();
            match written {
                Ok(m) => {
                    self.values.value.clear();
                    self.values.error.clear();
                    self.values.open = true;
                    self.act(ui, Ok(m));
                }
                Err(e) => self.values.error = error_text(&e),
            }
        }
        if let Some(date) = delete {
            let r = self.db.delete_valuation(account_id, date).map(|_| tr("Value deleted").to_string());
            self.act(ui, r);
        }
    }
}

fn figure(ui: &mut Ui, label: &str, value: RichText) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        small_caps(ui, label);
        ui.label(value.size(18.0).strong());
    });
}

/// The gain as a share of what was put in, with one decimal; none when nothing was put in.
fn percent(gain: i64, invested: i64) -> Option<String> {
    if invested <= 0 {
        return None;
    }
    let tenths = i128::from(gain).checked_mul(1000)?.checked_div(i128::from(invested))?;
    let sign = if tenths < 0 { "-" } else { "+" };
    let tenths = tenths.unsigned_abs();
    let sep = if cashcraft_engine::i18n::lang() == cashcraft_engine::i18n::Lang::It { "," } else { "." };
    Some(format!("{sign}{}{sep}{}%", tenths / 10, tenths % 10))
}
