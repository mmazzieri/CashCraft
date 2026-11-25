//! The data file in the window: its status, other files, synced folders, conflicts. A shared file is read and written
//! by a thread, so that a folder that does not answer never stops the window.

use crate::gui::{AMBER, App, GREEN, MUTED, RED, error_text, primary_button};
use anyhow::Result;
use cashcraft_engine::i18n::{self, Lang, lang, tr, trf};
use cashcraft_engine::money::fmt_date;
use cashcraft_engine::store::{
    self, Differences, Hold, Home, Opened, Presence, RENEW, Report, Service, Shared, Status, Use,
};
use cashcraft_engine::{db, demo};
use egui::{Color32, Id, RichText, Ui};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// After the last change, before it is written.
const PAUSE: f64 = 1.0;
/// How often the shared file is looked at.
const LOOK: Duration = Duration::from_secs(3);
const RETRY: Duration = Duration::from_secs(10);
const FULL: Duration = Duration::from_secs(60);
/// How long closing waits for a folder that does not answer. The changes are in the copy anyway.
const LAST: Duration = Duration::from_secs(8);
/// How many differences are drawn.
const SHOWN: usize = 200;

enum Order {
    Now,
    Stop,
    PutBack,
    Dismiss,
    Compare(PathBuf),
}

/// What the thread last saw.
#[derive(Default)]
struct Seen {
    report: Option<std::result::Result<Report, String>>,
    busy: bool,
    others: Vec<String>,
    /// To be told to the user once.
    news: Vec<String>,
    /// Conflict files that were not dismissed, and all other versions.
    conflicts: bool,
    versions: Vec<PathBuf>,
    missing: bool,
    compared: Option<std::result::Result<Differences, String>>,
}

pub struct Syncer {
    orders: mpsc::Sender<Order>,
    stopped: mpsc::Receiver<()>,
    seen: Arc<Mutex<Seen>>,
}

fn locked(m: &Mutex<Seen>) -> MutexGuard<'_, Seen> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Syncer {
    pub fn start(shared: Shared, first: Report, redraw: impl Fn() + Send + 'static) -> Syncer {
        let (orders, receive) = mpsc::channel();
        let (tell, stopped) = mpsc::channel();
        let news = first.conflict_notice().into_iter().collect();
        let seen = Arc::new(Mutex::new(Seen { report: Some(Ok(first)), news, ..Default::default() }));
        let shown = seen.clone();
        std::thread::spawn(move || {
            let mut presence = Presence::new(shared.home(), shared.path());
            let (mut renewed, mut looked): (Option<Instant>, Instant) = (None, Instant::now());
            loop {
                if renewed.is_none_or(|t| t.elapsed() >= RENEW) {
                    presence.renew(chrono::Utc::now().timestamp());
                    renewed = Some(Instant::now());
                }
                let others = presence.others(chrono::Utc::now().timestamp(), Instant::now());
                let (conflicts, versions) = (!shared.conflicts().is_empty(), shared.versions());
                let missing = shared.path().symlink_metadata().is_err();
                {
                    let mut s = locked(&shown);
                    (s.others, s.conflicts, s.versions, s.missing) = (others, conflicts, versions, missing);
                }
                redraw();
                let (stop, asked) = match receive.recv_timeout(LOOK) {
                    Ok(Order::Now) => (false, true),
                    Ok(Order::Stop) | Err(RecvTimeoutError::Disconnected) => (true, true),
                    Err(RecvTimeoutError::Timeout) => (false, false),
                    Ok(order) => {
                        let done = match order {
                            Order::PutBack => shared.put_back().map(|_| ()),
                            Order::Dismiss => shared.dismiss_conflicts(),
                            Order::Compare(other) => {
                                let found = shared.differences(&other).map_err(|e| format!("{e:#}"));
                                locked(&shown).compared = Some(found);
                                Ok(())
                            }
                            Order::Now | Order::Stop => Ok(()),
                        };
                        if let Err(e) = done {
                            locked(&shown).news.push(error_text(&e));
                        }
                        (false, true)
                    }
                };
                let saved = matches!(&locked(&shown).report, Some(Ok(r)) if r.status == Status::Saved);
                let every = if saved { FULL } else { RETRY };
                if asked || looked.elapsed() >= every || (saved && shared.looks_changed()) {
                    locked(&shown).busy = true;
                    redraw();
                    let report = shared.sync().map_err(|e| format!("{e:#}"));
                    looked = Instant::now();
                    let mut s = locked(&shown);
                    s.busy = false;
                    if let Ok(r) = &report {
                        s.news.extend(r.conflict_notice());
                    }
                    s.report = Some(report);
                }
                if stop {
                    presence.leave();
                    let _ = tell.send(());
                    return;
                }
            }
        });
        Syncer { orders, stopped, seen }
    }

    pub fn now(&self) {
        let _ = self.orders.send(Order::Now);
    }

    /// Writes what there is to write and ends. False: the folder does not answer.
    pub fn stop(&self) -> bool {
        self.orders.send(Order::Stop).is_ok() && self.stopped.recv_timeout(LAST).is_ok()
    }

    /// The footer: what and in which colour.
    fn status(&self) -> (String, Color32) {
        let s = locked(&self.seen);
        match &s.report {
            _ if s.busy => (tr("saving…").into(), MUTED),
            Some(Ok(r)) if r.status == Status::Saved && r.pending => (tr("to be saved").into(), MUTED),
            Some(Ok(r)) if r.status == Status::Saved => (tr("saved").into(), GREEN),
            Some(Ok(r)) => (r.status.text(), AMBER),
            Some(Err(e)) => (trf("Error: {error}", &[("error", e)]), RED),
            None => (tr("to be saved").into(), MUTED),
        }
    }
}

/// What the window of the data file is doing.
#[derive(Default)]
pub struct FileWindow {
    moving: bool,
    service: Option<Service>,
    /// What the user said of the file, and whether it will be shared at the next start.
    choice: Use,
    shared_next: bool,
    error: String,
}

/// The part of the window that knows the data file. Without a home, as in the tests, it only shows the path.
#[derive(Default)]
pub struct DataFile {
    pub home: Option<Home>,
    /// The file the user chose, not its copy.
    pub path: PathBuf,
    asked: Use,
    shared: Option<Shared>,
    syncer: Option<Syncer>,
    hold: Option<Hold>,
    epoch: u64,
    writes: u64,
    changed_at: Option<f64>,
    unfocused: bool,
    pub window: Option<FileWindow>,
    /// Another file that is being opened.
    opening: Option<mpsc::Receiver<Result<(PathBuf, Opened)>>>,
    /// The file that the one being opened takes the place of.
    old: Option<PathBuf>,
}

/// The end of a long path.
fn tail(path: &str, n: usize) -> String {
    let extra = path.chars().count().saturating_sub(n);
    if extra == 0 { path.into() } else { format!("…{}", path.chars().skip(extra + 1).collect::<String>()) }
}

/// Opens a data file; a new one gets the sample data.
pub fn open(home: &Home, path: &Path, asked: Use, must_exist: bool) -> Result<Opened> {
    let opened = store::open(home, path, asked, lang(), must_exist)?;
    if opened.is_new {
        demo::seed(&opened.db)?;
    }
    Ok(opened)
}

impl App {
    /// Shows a data file that was opened.
    pub fn of(opened: Opened, home: Home, path: PathBuf, asked: Use, ctx: &egui::Context) -> App {
        i18n::set_lang(Lang::at_start(opened.db.meta(i18n::META_KEY).as_deref()));
        let mut app = App::new(opened.db, None);
        app.file.home = Some(home);
        app.file.path = path;
        app.file.asked = asked;
        if let Some((shared, report)) = opened.shared {
            app.file.epoch = shared.epoch();
            let ctx = ctx.clone();
            let syncer = Syncer::start(shared.clone(), report, move || ctx.request_repaint());
            if opened.is_new {
                syncer.now();
            }
            app.file.syncer = Some(syncer);
            app.file.shared = Some(shared);
        }
        app
    }

    /// The ids on the screen stay good while a form is open, and for the time of a frame.
    pub(crate) fn file_before(&mut self, ui: &Ui) {
        if let Some(shared) = &self.file.shared {
            if self.file.hold.is_none() {
                match shared.hold() {
                    Ok(h) => self.file.hold = Some(h),
                    Err(e) => self.toast = Some((error_text(&e), f64::INFINITY)),
                }
            }
            let version = self.db.data_version();
            if version != self.data_version {
                self.data_version = version;
                let epoch = shared.epoch();
                if epoch != self.file.epoch {
                    self.file.epoch = epoch;
                    self.selected = None;
                    self.budget_edit = None;
                    self.rename_cat = None;
                    self.journal = None;
                }
                self.reload();
            }
        }
        let received = self.file.opening.as_ref().filter(|_| self.dialog.is_none()).map(|r| r.try_recv());
        match received {
            None | Some(Err(mpsc::TryRecvError::Empty)) => {}
            Some(Err(mpsc::TryRecvError::Disconnected)) => self.file.opening = None,
            Some(Ok(Ok((path, opened)))) => self.switch(ui, path, opened),
            Some(Ok(Err(e))) => {
                self.file.opening = None;
                self.file.old = None;
                match &mut self.file.window {
                    Some(w) => w.error = error_text(&e),
                    None => self.notify(ui, error_text(&e)),
                }
            }
        }
    }

    pub(crate) fn file_after(&mut self, ui: &Ui) {
        let Some(syncer) = &self.file.syncer else { return };
        let now = ui.input(|i| i.time);
        let writes = self.db.conn.total_changes();
        if writes != self.file.writes {
            self.file.writes = writes;
            self.file.changed_at = Some(now);
        }
        let focused = ui.input(|i| i.viewport().focused) != Some(false);
        let left = !focused && !self.file.unfocused;
        self.file.unfocused = !focused;
        if self.file.changed_at.is_some_and(|t| left || now - t >= PAUSE) {
            self.file.changed_at = None;
            syncer.now();
        } else if self.file.changed_at.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        let news: Vec<String> = std::mem::take(&mut locked(&syncer.seen).news);
        if !news.is_empty() {
            self.toast = Some((news.join("\n"), now + 30.0));
        }
        if self.dialog.is_none() && self.budget_edit.is_none() && self.rename_cat.is_none() {
            // What the programs of other computers wrote may come in.
            self.file.hold = None;
        }
    }

    pub(crate) fn file_close(&mut self) {
        self.file.hold = None;
        if let Some(s) = self.file.syncer.take() {
            s.stop();
        }
    }

    /// Opens another file beside the window, which goes on drawing.
    fn open_file(&mut self, path: PathBuf) {
        let Some(home) = self.file.home.clone() else { return };
        self.file.old = None;
        self.open_after(path, |_, _| Ok(()), home);
    }

    fn open_after(
        &mut self,
        path: PathBuf,
        first: impl FnOnce(&Home, &Path) -> Result<()> + Send + 'static,
        home: Home,
    ) {
        let (send, receive) = mpsc::channel();
        let language = lang();
        std::thread::spawn(move || {
            let opened = i18n::with_lang(language, || {
                first(&home, &path)?;
                open(&home, &path, Use::Auto, true)
            });
            let _ = send.send(opened.map(|o| (path, o)));
        });
        self.file.opening = Some(receive);
    }

    fn switch(&mut self, ui: &Ui, path: PathBuf, opened: Opened) {
        let Some(home) = self.file.home.clone() else { return };
        self.file_close();
        let old = self.file.old.take();
        let remembered = home.remember(Some(&store::resolved(&path)));
        let direct = self.file.shared.is_none();
        *self = App::of(opened, home, path.clone(), Use::Auto, ui.ctx());
        let mut text = trf("The data file is now {path}", &[("path", &path.display())]);
        if let Err(e) = remembered {
            text = error_text(&e);
        }
        if let Some(old) = old
            && direct
            && let Err(e) = store::retire(&old, &self.db)
        {
            text = error_text(&e);
        }
        self.notify(ui, text);
    }

    pub(crate) fn is_shared(&self) -> bool {
        self.file.shared.is_some()
    }

    /// The footer.
    pub fn file_status(&mut self, ui: &mut Ui) {
        let path = match (&self.file.shared, &self.db.path) {
            (Some(s), _) => s.path().display().to_string(),
            (None, Some(p)) => p.display().to_string(),
            (None, None) => tr("trial data in memory").into(),
        };
        let text = RichText::new(trf("Data file: {path}", &[("path", &tail(&path, 90))])).size(11.5).color(MUTED);
        if self.file.home.is_none() {
            ui.label(text);
            return;
        }
        let button = ui.add(egui::Button::new(text).frame(false));
        if button.on_hover_text(tr("Open another file, synced folders…")).clicked() && self.dialog.is_none() {
            self.open_file_window();
        }
        let small = |ui: &mut Ui, text: String, color| {
            ui.add(egui::Label::new(RichText::new(&text).size(11.5).color(color)).truncate()).on_hover_text(text);
        };
        if let Some(s) = &self.file.syncer {
            if locked(&s.seen).conflicts {
                small(ui, tr("conflict").into(), RED);
            }
            let (text, color) = match self.file.changed_at {
                Some(_) => (tr("to be saved").to_string(), MUTED),
                None => s.status(),
            };
            small(ui, text, color);
            let others = locked(&s.seen).others.join(", ");
            if !others.is_empty() {
                small(ui, trf("Also open on {computer}", &[("computer", &others)]), AMBER);
            }
        }
    }

    /// What is asked of the folder is asked once, not at every frame.
    pub(crate) fn open_file_window(&mut self) {
        let Some(home) = &self.file.home else { return };
        let (shared_next, service) = store::way(home, &self.file.path, self.file.asked);
        let choice = home.choice(&self.file.path);
        self.file.window = Some(FileWindow { service, choice, shared_next, ..Default::default() });
    }

    pub fn file_window(&mut self, ctx: &egui::Context) {
        let (Some(mut w), Some(home)) = (self.file.window.take(), self.file.home.clone()) else { return };
        let mut keep = true;
        let mut open_this: Option<PathBuf> = None;
        let modal = egui::Modal::new(Id::unique("data file")).show(ctx, |ui| {
            ui.set_min_width(560.0);
            ui.set_max_width(760.0);
            ui.heading(tr("Data file"));
            let path = self.file.path.clone();
            ui.label(path.display().to_string());
            let how = match (self.file.shared.is_some(), w.service) {
                (false, None) => tr("Used directly").to_string(),
                (false, Some(k)) => trf("Used directly, in {service}", &[("service", &k.label())]),
                (true, None) => tr("Used through a private copy").to_string(),
                (true, Some(k)) => trf("Used through a private copy, in {service}", &[("service", &k.label())]),
            };
            ui.label(RichText::new(how).color(MUTED));
            if let Some(s) = &self.file.syncer {
                let (text, color) = s.status();
                ui.label(RichText::new(text).color(color));
            }
            if w.shared_next != self.file.shared.is_some() {
                ui.label(
                    RichText::new(tr("It will be used in the other way when CashCraft is started again.")).color(AMBER),
                );
            }
            ui.horizontal(|ui| {
                ui.label(tr("How to use it"));
                let label = |c| match c {
                    Use::Auto => tr("Automatic"),
                    Use::Direct => tr("Directly, where it lies"),
                    Use::Shared => tr("Through a private copy (synced folders, network drives)"),
                };
                let before = w.choice;
                let mut chosen = before;
                egui::ComboBox::from_id_salt("file mode").selected_text(label(before)).width(360.0).show_ui(ui, |ui| {
                    for c in Use::ALL {
                        ui.selectable_value(&mut chosen, c, label(c));
                    }
                });
                if chosen != before {
                    match home.set_choice(&path, chosen) {
                        Ok(()) => {
                            w.choice = chosen;
                            w.shared_next = store::way(&home, &path, self.file.asked).0;
                        }
                        Err(e) => w.error = error_text(&e),
                    }
                }
            });
            ui.add_space(8.0);
            if let Some(syncer) = &self.file.syncer {
                syncer.draw_versions(ui, &mut open_this);
                if locked(&syncer.seen).missing && ui.button(tr("Write it again where it was")).clicked() {
                    let _ = syncer.orders.send(Order::PutBack);
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(tr("📂 Open another data file…")).clicked()
                    && let Some(p) =
                        rfd::FileDialog::new().add_filter("CashCraft", &["db", "sqlite", "cashcraft"]).pick_file()
                {
                    open_this = Some(p);
                }
                let default = db::default_db_path();
                if default != path && default.exists() && ui.button(tr("Open the default one")).clicked() {
                    open_this = Some(default);
                }
            });
            ui.horizontal(|ui| {
                let clicked = ui
                    .button(tr("Keep it in a synced folder…"))
                    .on_hover_text(tr(
                        "A complete copy is put in the folder you choose, checked, and used from then on.",
                    ))
                    .clicked();
                if self.file.shared.is_none() {
                    ui.checkbox(&mut w.moving, tr("then remove the old file"));
                }
                if clicked && let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    w.error.clear();
                    self.keep_in(&home, folder, w.moving);
                }
            });
            if self.file.opening.is_some() {
                ui.label(tr("Opening…"));
            }
            if !w.error.is_empty() {
                ui.label(RichText::new(&w.error).color(RED));
            }
            ui.add_space(8.0);
            if primary_button(ui, tr("Close")).clicked() {
                keep = false;
            }
        });
        if modal.should_close() {
            keep = false;
        }
        if let Some(p) = open_this {
            w.error.clear();
            self.open_file(p);
        }
        if keep {
            self.file.window = Some(w);
        }
    }

    /// Puts a copy of the data file in a folder and opens it, beside the window.
    pub(crate) fn keep_in(&mut self, home: &Home, folder: PathBuf, moving: bool) {
        let name = self.file.path.file_name().unwrap_or("cashcraft.db".as_ref());
        let target = folder.join(name);
        let Some(from) = self.db.path.clone() else { return };
        self.file.old = moving.then(|| self.file.path.clone());
        self.open_after(target, move |home, target| store::copy_to(home, &from, target), home.clone());
    }
}

impl Syncer {
    fn draw_versions(&self, ui: &mut Ui, open_this: &mut Option<PathBuf>) {
        let mut seen = locked(&self.seen);
        if seen.versions.is_empty() {
            return;
        }
        ui.label(tr("Other versions beside it, such as those of conflicts:"));
        let mut compare = None;
        egui::ScrollArea::vertical().id_salt("versions").max_height(120.0).show(ui, |ui| {
            for v in &seen.versions {
                ui.horizontal(|ui| {
                    ui.label(v.file_name().unwrap_or_default().to_string_lossy().into_owned());
                    if ui.small_button(tr("Show what differs")).clicked() {
                        compare = Some(v.clone());
                    }
                    if ui.small_button(tr("Open it")).clicked() {
                        *open_this = Some(v.clone());
                    }
                });
            }
        });
        if let Some(v) = compare {
            seen.compared = None;
            let _ = self.orders.send(Order::Compare(v));
        }
        if seen.conflicts && ui.button(tr("Do not report these conflicts any more")).clicked() {
            let _ = self.orders.send(Order::Dismiss);
        }
        let d = match &seen.compared {
            None => return,
            Some(Ok(d)) => d,
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(RED));
                return;
            }
        };
        egui::ScrollArea::vertical().id_salt("differences").max_height(220.0).show(ui, |ui| {
            for (title, lines) in [(tr("Only in the data file"), &d.first), (tr("Only in the other file"), &d.second)] {
                if lines.is_empty() {
                    continue;
                }
                ui.label(RichText::new(title).strong());
                for l in lines.iter().take(SHOWN) {
                    ui.label(format!("{}  {}  {}  {}", fmt_date(l.date), l.payee, l.memo, l.amounts()));
                }
                if lines.len() > SHOWN {
                    ui.label("…");
                }
            }
            if d.first.is_empty() && d.second.is_empty() {
                ui.label(if d.others {
                    tr("The entries are the same; something else is different, such as names, budgets or rules.")
                } else {
                    tr("The two files hold the same.")
                });
            }
        });
    }
}

/// The window when the data file could not be opened: try again, another one, or nothing.
pub struct Start {
    pub home: Home,
    pub path: PathBuf,
    pub must_exist: bool,
    pub asked: Use,
    pub error: String,
    pub app: Option<Box<App>>,
}

impl Start {
    fn open(&mut self, ctx: &egui::Context, path: PathBuf) {
        match open(&self.home, &path, self.asked, self.must_exist) {
            Ok(o) => self.app = Some(Box::new(App::of(o, self.home.clone(), path, self.asked, ctx))),
            Err(e) => self.error = error_text(&e),
        }
    }

    pub fn draw(&mut self, ui: &mut Ui) {
        ui.heading(tr("The data file cannot be opened"));
        ui.label(self.path.display().to_string());
        ui.label(RichText::new(&self.error).color(RED));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let ctx = ui.ctx().clone();
            if primary_button(ui, tr("Try again")).clicked() {
                self.open(&ctx, self.path.clone());
            }
            if ui.button(tr("📂 Open another data file…")).clicked()
                && let Some(p) =
                    rfd::FileDialog::new().add_filter("CashCraft", &["db", "sqlite", "cashcraft"]).pick_file()
            {
                self.asked = Use::Auto;
                self.must_exist = true;
                self.path = p.clone();
                self.open(&ctx, p);
                if self.app.is_some()
                    && let Err(e) = self.home.remember(Some(&store::resolved(&self.path)))
                {
                    eprintln!("{}", error_text(&e));
                }
            }
            if ui.button(tr("Quit")).clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }
}

impl eframe::App for Start {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        match &mut self.app {
            Some(app) => app.ui(ui, frame),
            None => {
                egui::CentralPanel::default().show(ui, |ui| self.draw(ui));
            }
        }
    }

    fn on_exit(&mut self) {
        if let Some(app) = &mut self.app {
            app.on_exit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::{Dialog, TxForm};
    use cashcraft_engine::db::{AccountKind, NewTx};

    struct Place(PathBuf);
    impl Drop for Place {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    impl Place {
        fn new(name: &str) -> Place {
            let dir = std::env::temp_dir().join(format!("cashcraft-window-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("folder")).unwrap();
            Place(dir)
        }
        fn file(&self) -> PathBuf {
            self.0.join("folder").join("books.db")
        }
        fn home(&self, computer: &str) -> Home {
            Home { dir: self.0.join(computer), computer: computer.into() }
        }
        fn names(&self) -> Vec<String> {
            let all = std::fs::read_dir(self.0.join("folder")).unwrap();
            let mut out: Vec<String> = all.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            out.sort();
            out
        }
    }

    fn add(db: &db::Db, payee: &str) {
        let account_id = db.accounts(true).unwrap()[0].id;
        db.add_transaction(&NewTx {
            account_id,
            date: db::today(),
            amount: -1,
            payee: payee.into(),
            ..Default::default()
        })
        .unwrap();
    }

    fn has(db: &db::Db, payee: &str) -> bool {
        db.transactions(&Default::default()).unwrap().iter().any(|t| t.payee == payee)
    }

    /// What eframe does at each frame, for the data file.
    fn frame(ctx: &egui::Context, app: &mut App, time: f64, focused: bool) {
        let mut input = egui::RawInput { time: Some(time), focused, ..Default::default() };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().focused = Some(focused);
        ctx.run_ui(input, |ui| {
            app.file_before(ui);
            app.status_bar(ui);
            app.file_window(&ui.ctx().clone());
            app.file_after(ui);
        })
        .textures_delta
        .clear();
    }

    fn until(what: &str, mut f: impl FnMut() -> bool) {
        let start = Instant::now();
        while !f() {
            assert!(start.elapsed() < Duration::from_secs(20), "{what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn the_window_writes_and_sees_what_others_write() {
        let p = Place::new("sync");
        let ctx = egui::Context::default();
        let seed = open(&p.home("two"), &p.file(), Use::Shared, false).unwrap();
        seed.db.clear_accounts().unwrap();
        seed.db.add_account("Bank", AccountKind::Bank, "EUR", 0, "").unwrap();
        let (two, other) = (seed.db, seed.shared.unwrap().0);
        other.sync().unwrap();
        let o = open(&p.home("one"), &p.file(), Use::Shared, true).unwrap();
        let mut app = App::of(o, p.home("one"), p.file(), Use::Shared, &ctx);
        frame(&ctx, &mut app, 0.0, true);
        // Written after a pause.
        add(&app.db, "paused");
        frame(&ctx, &mut app, 1.0, true);
        assert!(app.file.changed_at.is_some());
        frame(&ctx, &mut app, 2.5, true);
        assert!(app.file.changed_at.is_none());
        let seen = |payee: &str| {
            until(payee, || {
                other.sync().unwrap();
                has(&two, payee)
            });
        };
        seen("paused");
        // At once when the window is left.
        add(&app.db, "left");
        frame(&ctx, &mut app, 3.0, false);
        assert!(app.file.changed_at.is_none());
        seen("left");
        frame(&ctx, &mut app, 4.0, true);
        // A form that is open comes first; what another computer wrote is seen once it is closed.
        app.dialog = Some(Dialog::Tx(TxForm::default()));
        frame(&ctx, &mut app, 5.0, true);
        assert!(app.file.hold.is_some());
        add(&two, "from two");
        other.sync().unwrap();
        app.selected = Some(1);
        app.file.syncer.as_ref().unwrap().now();
        until("held", || {
            frame(&ctx, &mut app, 6.0, true);
            app.file.syncer.as_ref().unwrap().status().0.contains("close the form")
        });
        assert!(!has(&app.db, "from two") && app.selected == Some(1));
        app.dialog = None;
        until("adopted", || {
            frame(&ctx, &mut app, 7.0, true);
            app.file.syncer.as_ref().unwrap().now();
            app.txs.iter().any(|t| t.payee == "from two")
        });
        assert_eq!(app.selected, None);
        // Both change: the user is told, and nothing is lost.
        app.dialog = Some(Dialog::Tx(TxForm::default()));
        frame(&ctx, &mut app, 7.5, true);
        until("all saved", || {
            other.sync().unwrap();
            app.file.syncer.as_ref().unwrap().status().0 == "saved"
        });
        add(&two, "theirs");
        other.sync().unwrap();
        add(&app.db, "mine");
        app.dialog = None;
        app.file.syncer.as_ref().unwrap().now();
        until("conflict", || {
            frame(&ctx, &mut app, 8.0, true);
            app.toast.as_ref().is_some_and(|t| t.0.contains("Nothing was lost"))
        });
        assert!(p.names().iter().any(|n| n.contains("(conflict one ")));
        app.open_file_window();
        let versions = || locked(&app.file.syncer.as_ref().unwrap().seen).versions.clone();
        until("versions", || !versions().is_empty());
        let _ = app.file.syncer.as_ref().unwrap().orders.send(Order::Compare(versions().remove(0)));
        until("compared", || {
            let s = locked(&app.file.syncer.as_ref().unwrap().seen);
            matches!(&s.compared, Some(Ok(d)) if d.second.iter().any(|l| l.payee == "mine"))
        });
        frame(&ctx, &mut app, 9.0, true);
        frame(&ctx, &mut app, 10.0, true);
        assert!(app.file.window.is_some());
        // Closing writes the last change and leaves nothing behind.
        app.file.window = None;
        add(&app.db, "last");
        app.file_close();
        other.sync().unwrap();
        assert!(has(&two, "last"));
        assert!(p.names().iter().all(|n| n.ends_with(".db")), "{:?}", p.names());
    }

    #[test]
    fn a_file_is_kept_in_a_folder_and_the_old_one_removed() {
        let p = Place::new("keep");
        let ctx = egui::Context::default();
        let old = p.0.join("books.db");
        let home = p.home("one");
        let o = open(&home, &old, Use::Direct, false).unwrap();
        let mut app = App::of(o, home.clone(), old.clone(), Use::Direct, &ctx);
        add(&app.db, "x");
        app.keep_in(&home, p.0.join("folder"), true);
        until("opened", || {
            frame(&ctx, &mut app, 1.0, true);
            app.file.shared.is_some()
        });
        assert!(has(&app.db, "x") && !old.exists());
        assert_eq!(home.recent(), Some(store::resolved(&p.file())));
        app.keep_in(&home, p.0.join("folder"), false);
        app.open_file_window();
        until("taken", || {
            frame(&ctx, &mut app, 1.5, true);
            app.file.window.as_ref().is_some_and(|w| w.error.contains("already"))
        });
        // One that cannot be opened leaves things as they are.
        std::fs::write(p.0.join("bad.db"), "not a data file").unwrap();
        app.file.window = Some(FileWindow::default());
        app.open_file(p.0.join("bad.db"));
        until("refused", || {
            frame(&ctx, &mut app, 2.0, true);
            app.file.window.as_ref().is_some_and(|w| !w.error.is_empty())
        });
        assert!(has(&app.db, "x"));
        app.file_close();
    }

    #[test]
    fn what_cannot_be_opened_is_said_and_tried_again() {
        let p = Place::new("start");
        let ctx = egui::Context::default();
        let mut start = Start {
            home: p.home("one"),
            path: p.file(),
            must_exist: true,
            asked: Use::Shared,
            error: String::new(),
            app: None,
        };
        start.open(&ctx, p.file());
        assert!(start.app.is_none() && start.error.contains("not there") && !p.file().exists());
        ctx.run_ui(Default::default(), |ui| start.draw(ui)).textures_delta.clear();
        start.must_exist = false;
        start.open(&ctx, p.file());
        assert!(start.app.is_some());
        eframe::App::on_exit(&mut start);
        assert!(p.file().exists());
    }
}
