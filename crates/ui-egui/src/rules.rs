//! Automatic rules: the screen, the form of a rule with the live count, and the preview of a run.

use crate::gui::{
    ACCENT, AMBER, App, Dialog, MUTED, RED, account_combo, card, category_combo, cut, form_row, money, or_report,
    primary_button,
};
use anyhow::Result;
use cashcraft_engine::db::{Account, Category, Db, Transaction, TxFilter};
use cashcraft_engine::i18n::{lang, tr, trf, trn, with_lang};
use cashcraft_engine::money::{fmt_date, fmt_plain, parse_amount};
use cashcraft_engine::rules::{Place, Plan, Rule, Scope, Sign, broken_message, count_matches, parse_tags};
use egui::{self, RichText, Ui};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// A rule as it is written.
pub struct RuleForm {
    pub id: Option<i64>,
    pub rule: Rule,
    pub min: String,
    pub max: String,
    pub tags: String,
    /// Before the other rules.
    pub first: bool,
    /// What the live count is of.
    txs: Arc<Vec<Transaction>>,
    count: LiveCount,
    pub error: String,
}

impl RuleForm {
    pub fn new(db: &Db, rule: Rule, id: Option<i64>, first: bool) -> Result<Self> {
        let text = |a: Option<i64>| a.map(fmt_plain).unwrap_or_default();
        Ok(RuleForm {
            id,
            min: text(rule.amount_min),
            max: text(rule.amount_max),
            tags: rule.tags.join(", "),
            rule,
            first,
            txs: Arc::new(db.transactions(&TxFilter::default())?),
            count: LiveCount::default(),
            error: String::new(),
        })
    }

    pub fn read(&self) -> Result<Rule> {
        let amount = |t: &str| if t.trim().is_empty() { Ok(None) } else { parse_amount(t).map(Some) };
        Ok(Rule {
            id: self.id.unwrap_or_default(),
            amount_min: amount(&self.min)?,
            amount_max: amount(&self.max)?,
            tags: parse_tags(&self.tags)?,
            ..self.rule.clone()
        })
    }

    /// How many transactions the rule matches as it is written now; `None` while it is not known. Counted beside
    /// the window, once the user stops writing.
    pub fn count(&mut self) -> Option<Counted> {
        let rule = match self.read() {
            Ok(r) => r,
            Err(e) => return Some(Err(format!("{e:#}"))),
        };
        self.count.of(rule, &self.txs)
    }
}

/// How many match, and whether those are all.
pub type Counted = std::result::Result<(usize, bool), String>;

/// How long a rule stays as it is before it is counted.
const PAUSE: Duration = Duration::from_millis(250);
/// How long a count may take.
const LONGEST: Duration = Duration::from_secs(2);

#[derive(Default)]
struct LiveCount {
    /// The rule as it was last seen, and since when.
    rule: Option<(Rule, Option<Instant>)>,
    running: Option<(mpsc::Receiver<Counted>, Arc<AtomicBool>)>,
    done: Option<Counted>,
}

impl LiveCount {
    fn of(&mut self, rule: Rule, txs: &Arc<Vec<Transaction>>) -> Option<Counted> {
        if self.rule.as_ref().is_none_or(|(of, _)| *of != rule) {
            self.stop();
            self.rule = Some((rule, Some(Instant::now())));
            self.done = None;
        }
        if let Some((rule, since)) = &mut self.rule
            && since.is_some_and(|s| s.elapsed() >= PAUSE)
        {
            *since = None;
            let (send, receive) = mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            let (rule, txs, told, language) = (rule.clone(), txs.clone(), stop.clone(), lang());
            std::thread::spawn(move || {
                let n = with_lang(language, || count_matches(&rule, &txs, LONGEST, &told));
                // Nobody may be waiting any more.
                let _ = send.send(n.map_err(|e| format!("{e:#}")));
            });
            self.running = Some((receive, stop));
        }
        if let Some((receive, _)) = &self.running
            && let Ok(n) = receive.try_recv()
        {
            self.done = Some(n);
            self.running = None;
        }
        self.done.clone()
    }

    fn stop(&mut self) {
        if let Some((_, stop)) = self.running.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

impl Drop for LiveCount {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What a run would change.
#[derive(Default)]
pub struct RunForm {
    pub scope: Scope,
    pub plan: Option<(Scope, Plan)>,
    pub error: String,
}

impl App {
    pub fn rules_view(&mut self, ui: &mut Ui) {
        if self.rules.is_none() {
            let rules = or_report(self.db.rules(), &mut self.toast);
            let last = or_report(self.db.last_rule_run(), &mut self.toast);
            self.rules = Some((rules, last));
        }
        let Some((rules, last)) = self.rules.clone() else { return };
        let mut open: Option<Rule> = None;
        let mut run = false;
        let mut undo = false;
        let mut moved: Option<(i64, Place)> = None;
        let mut delete: Option<i64> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(tr(
                        "Rules run from the first to the last on every new or imported transaction without a category. \
                         The first that matches decides the category; tags are those of every rule that matches.",
                    ))
                    .color(MUTED),
                );
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    if primary_button(ui, tr("+ New rule")).clicked() {
                        open = Some(Rule::default());
                    }
                    run = ui.button(tr("▶ Apply to transactions…")).clicked();
                    if let Some(l) = &last {
                        let text = trn(
                            l.changed,
                            "Undo the last run ({n} transaction)…",
                            "Undo the last run ({n} transactions)…",
                        );
                        undo = ui.button(text).on_hover_text(&l.made_at).clicked();
                    }
                });
            });
            ui.add_space(12.0);
            card(ui, |ui| {
                ui.set_width(ui.available_width());
                if rules.is_empty() {
                    ui.label(RichText::new(tr("There are no rules.")).color(MUTED));
                    return;
                }
                egui::Grid::new("rules").striped(true).num_columns(4).min_col_width(40.0).spacing([16.0, 8.0]).show(
                    ui,
                    |ui| {
                        for h in ["", tr("When"), tr("Then"), ""] {
                            ui.label(RichText::new(h.to_uppercase()).size(11.0).color(MUTED));
                        }
                        ui.end_row();
                        for (i, r) in rules.iter().enumerate() {
                            ui.label(RichText::new((i + 1).to_string()).color(MUTED))
                                .on_hover_text(trf("Rule {id}", &[("id", &r.id)]));
                            ui.vertical(|ui| {
                                ui.label(cut(&r.conditions(), 70)).on_hover_text(r.conditions());
                                if let Some(p) = &r.problem {
                                    ui.label(RichText::new(trf("skipped: {problem}", &[("problem", p)])).color(RED));
                                }
                            });
                            ui.label(RichText::new(cut(&r.actions(), 50)).color(ACCENT)).on_hover_text(r.actions());
                            ui.horizontal(|ui| {
                                if ui.add_enabled(i > 0, egui::Button::new("⬆").small()).clicked() {
                                    moved = Some((r.id, Place::Up));
                                }
                                if ui.add_enabled(i + 1 < rules.len(), egui::Button::new("⬇").small()).clicked() {
                                    moved = Some((r.id, Place::Down));
                                }
                                if ui.small_button("✏").clicked() {
                                    open = Some(r.clone());
                                }
                                if ui.small_button("🗑").clicked() {
                                    delete = Some(r.id);
                                }
                            });
                            ui.end_row();
                        }
                    },
                );
            });
        });
        if let Some(r) = open {
            let id = (r.id != 0).then_some(r.id);
            self.open_rule(r, id, false);
        }
        if run {
            self.dialog = Some(Dialog::RunRules(RunForm::default()));
        }
        if undo {
            self.dialog = last.map(Dialog::ConfirmUndoRules);
        }
        if let Some((id, to)) = moved {
            let r = self.db.move_rule(id, to).map(|_| String::new());
            self.act(ui, r);
        }
        if let Some(id) = delete {
            let r = self.db.delete_rule(id).map(|_| tr("Rule deleted").to_string());
            self.act(ui, r);
        }
    }

    pub fn open_rule(&mut self, rule: Rule, id: Option<i64>, first: bool) {
        match RuleForm::new(&self.db, rule, id, first) {
            Ok(f) => self.dialog = Some(Dialog::Rule(Box::new(f))),
            Err(e) => self.toast = Some((format!("{e:#}"), f64::INFINITY)),
        }
    }

    /// A rule that categorizes others like this transaction.
    pub fn open_rule_from(&mut self, t: &Transaction) {
        match self.db.rule_from(t.id) {
            Ok(r) => self.open_rule(r, None, true),
            Err(e) => self.toast = Some((format!("{e:#}"), f64::INFINITY)),
        }
    }

    /// The rules that set something of a transaction, one a line.
    pub fn rules_of(&self, t: &Transaction) -> Result<String> {
        let all = self.db.rules()?;
        let one = |id: i64| match all.iter().find(|r| r.id == id) {
            Some(r) => trf("rule {id}: {what}", &[("id", &id), ("what", &r.summary())]),
            None => trf("rule {id}, deleted", &[("id", &id)]),
        };
        Ok(t.rules.ids().into_iter().map(one).collect::<Vec<_>>().join("\n"))
    }

    pub fn rule_dialog(
        &mut self,
        ui: &mut Ui,
        f: &mut RuleForm,
        accounts: &[Account],
        cats: &[Category],
    ) -> (bool, Option<Result<String>>) {
        let mut keep = true;
        let mut message = None;
        ui.set_min_width(620.0);
        ui.heading(if f.id.is_some() { tr("Edit rule") } else { tr("New rule") });
        ui.add_space(6.0);
        ui.label(RichText::new(tr("When all of these are met (leave out what does not matter):")).strong());
        let hint = if f.rule.regex { tr("regular expression") } else { tr("keywords, e.g. bakery|pastry shop") };
        for (label, text) in [
            (tr("Payee or notes"), &mut f.rule.pattern),
            (tr("Payee"), &mut f.rule.payee),
            (tr("Notes"), &mut f.rule.memo),
        ] {
            form_row(ui, label, |ui| {
                ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(380.0));
            });
        }
        form_row(ui, "", |ui| {
            ui.checkbox(&mut f.rule.regex, tr("Regular expressions, instead of keywords separated by |"));
        });
        form_row(ui, tr("Amount €"), |ui| {
            ui.label(tr("from"));
            ui.add(egui::TextEdit::singleline(&mut f.min).desired_width(90.0));
            ui.label(tr("to"));
            ui.add(egui::TextEdit::singleline(&mut f.max).desired_width(90.0));
            egui::ComboBox::from_id_salt("rulesign").selected_text(f.rule.sign.label()).show_ui(ui, |ui| {
                for s in Sign::ALL {
                    ui.selectable_value(&mut f.rule.sign, s, s.label());
                }
            });
        });
        form_row(ui, tr("Account"), |ui| {
            let mut limited = f.rule.account_id.is_some();
            if ui.checkbox(&mut limited, tr("only this one")).changed() {
                f.rule.account_id = if limited { accounts.first().map(|a| a.id) } else { None };
            }
            if limited {
                account_combo(ui, "ruleacc", accounts, &mut f.rule.account_id);
            }
        });
        ui.add_space(6.0);
        ui.label(RichText::new(tr("Then:")).strong());
        form_row(ui, tr("Category"), |ui| {
            category_combo(ui, "rulecat", cats, &mut f.rule.category_id, tr("None"));
        });
        form_row(ui, tr("Tags"), |ui| {
            ui.add(egui::TextEdit::singleline(&mut f.tags).hint_text(tr("separated by commas")).desired_width(280.0));
        });
        form_row(ui, tr("New payee"), |ui| {
            let hint =
                if f.rule.regex { tr("$1 is what the first group matched") } else { tr("rename payee (optional)") };
            ui.add(egui::TextEdit::singleline(&mut f.rule.rename_payee).hint_text(hint).desired_width(380.0));
        });
        if f.id.is_none() {
            form_row(ui, "", |ui| {
                ui.checkbox(&mut f.first, tr("Before the other rules"));
            });
        }
        ui.add_space(6.0);
        match f.count() {
            Some(Ok((n, whole))) => {
                let text = if whole {
                    trn(n, "It matches {n} transaction", "It matches {n} transactions")
                } else {
                    trn(n, "It matches {n} transaction at least", "It matches {n} transactions at least")
                };
                ui.label(RichText::new(text).strong().color(ACCENT));
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(AMBER));
            }
            None => {
                ui.label(RichText::new(tr("Counting…")).color(MUTED));
                ui.ctx().request_repaint_after(Duration::from_millis(50));
            }
        }
        if !f.error.is_empty() {
            ui.label(RichText::new(&f.error).color(RED));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if primary_button(ui, tr("Save")).clicked() {
                let saved = f.read().and_then(|r| match f.id {
                    Some(_) => self.db.update_rule(&r).map(|_| tr("Rule changed").to_string()),
                    None => self.db.insert_rule(&r, f.first).map(|_| tr("Rule added").to_string()),
                });
                match saved {
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
        (keep, message)
    }

    pub fn run_dialog(&mut self, ui: &mut Ui, f: &mut RunForm) -> (bool, Option<Result<String>>) {
        let mut keep = true;
        let mut message = None;
        ui.set_min_width(780.0);
        ui.heading(tr("Apply the rules"));
        ui.label(RichText::new(tr("Transfers and splits are left alone.")).color(MUTED));
        ui.add_space(6.0);
        ui.radio_value(&mut f.scope, Scope::default(), tr("Transactions without a category"));
        let all = Scope { all: true, overwrite: false };
        ui.radio_value(&mut f.scope, all, tr("All: also tags, payees and the categories that rules set"));
        let overwrite = Scope { all: true, overwrite: true };
        ui.radio_value(&mut f.scope, overwrite, tr("All, also the categories and payees set by hand"));
        if f.plan.as_ref().is_none_or(|(of, _)| *of != f.scope) {
            match self.db.plan_rules(f.scope) {
                Ok(p) => {
                    f.plan = Some((f.scope, p));
                    f.error.clear();
                }
                Err(e) => {
                    f.plan = Some((f.scope, Plan::default()));
                    f.error = format!("{e:#}");
                }
            }
        }
        let Some((_, plan)) = &f.plan else { return (keep, message) };
        ui.add_space(8.0);
        let n = plan.changes.len();
        ui.label(RichText::new(trn(n, "{n} transaction would change", "{n} transactions would change")).strong());
        if !plan.broken.is_empty() {
            ui.label(RichText::new(broken_message(&plan.broken)).color(RED));
        }
        egui::ScrollArea::vertical().id_salt("run_preview").max_height(320.0).auto_shrink([false, true]).show_rows(
            ui,
            22.0,
            n,
            |ui, range| {
                egui::Grid::new("run").striped(true).num_columns(5).min_row_height(22.0).spacing([14.0, 0.0]).show(
                    ui,
                    |ui| {
                        for c in plan.changes.get(range).unwrap_or_default() {
                            ui.label(RichText::new(fmt_date(c.date)).color(MUTED));
                            ui.label(cut(&c.payee, 30));
                            ui.label(money(c.amount));
                            let mut what = Vec::new();
                            if let Some(x) = &c.new_category {
                                let old = if c.category.is_empty() { "—" } else { &c.category };
                                what.push(format!("{old} ➡ {x}"));
                            }
                            what.extend(c.new_payee.as_ref().map(|p| format!("\"{p}\"")));
                            what.extend(c.new_tags.iter().map(|t| format!("#{t}")));
                            let what = what.join(", ");
                            ui.label(RichText::new(cut(&what, 56)).color(ACCENT)).on_hover_text(what);
                            let ids: Vec<String> = c.rules.iter().map(i64::to_string).collect();
                            ui.label(
                                RichText::new(trf("rules {ids}", &[("ids", &ids.join(", "))])).color(MUTED).size(12.0),
                            );
                            ui.end_row();
                        }
                    },
                );
            },
        );
        if !f.error.is_empty() {
            ui.label(RichText::new(&f.error).color(RED));
        }
        ui.add_space(8.0);
        let token = plan.token.clone();
        ui.horizontal(|ui| {
            if ui.add_enabled(n > 0, egui::Button::new(tr("Apply these changes"))).clicked() {
                match self.db.run_rules(f.scope, Some(&token)) {
                    Ok(r) => {
                        message = Some(Ok(trn(
                            r.changed,
                            "{n} transaction changed by the rules",
                            "{n} transactions changed by the rules",
                        )));
                        keep = false;
                    }
                    Err(e) => {
                        // What is shown next is how things are now.
                        f.plan = self.db.plan_rules(f.scope).ok().map(|p| (f.scope, p));
                        f.error = format!("{e:#}");
                    }
                }
            }
            if ui.button(tr("Cancel")).clicked() {
                keep = false;
            }
        });
        (keep, message)
    }
}
