//! Telling the owner when a new version of CashCraft is out and, for a copy installed from the
//! Windows `.msi`, downloading it, checking it by the release's own signature and installing it
//! when the window closes, after a copy of the data file has been made (`cashcraft-update`, which
//! follows MailCraft's own updater).
//!
//! - The daily check is off until the owner turns it on: at the first starts a line above the
//!   status bar asks once a run, at most [`MOST_ASKS`] runs. The sidebar changes it later, and its
//!   button checks once, on or off. A development build never checks by itself.
//! - A newer version is said in the status bar, with a link to its page. A copy that updates
//!   itself downloads it in the background (unless the owner chose to be told only), and the
//!   status bar then says "Restart to update". Pressed, the window closes, the new version is
//!   installed and started; not pressed, it is installed when the owner quits.
//! - Before it is installed, the data file is copied, whole and checked, into the private folder
//!   of this computer (`updates/backups`); without that copy nothing is installed.
//! - The next start says what came of it, once.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use cashcraft_engine::i18n::{tr, trf};
use cashcraft_update::apply::{self, Folders, StartNews};
use cashcraft_update::download::{self, GitHub, Verified};
use cashcraft_update::github::{self, Release};
use cashcraft_update::install::{self, Install};
use cashcraft_update::version::Version;
use cashcraft_update::{MOST_ASKS, Problem, Project, Settings};
use egui::{Frame, Margin, RichText, Stroke, Ui};

use crate::gui::{App, pal, primary_button};

/// CashCraft, as its updater sees it.
pub const PROJECT: Project = Project {
    name: "CashCraft",
    program: "cashcraft",
    repository: env!("CARGO_PKG_REPOSITORY"),
    keys: cashcraft_engine::RELEASE_KEYS,
};

/// What the owner chose in the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    Notify,
    Install,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Off => tr("Off"),
            Mode::Notify => tr("Tell me"),
            Mode::Install => tr("Install by itself"),
        }
    }
}

/// What a run knows of new versions.
pub struct Updates {
    pub settings: Settings,
    folders: Folders,
    pub install: Install,
    /// The question is on screen.
    pub asking: bool,
    /// A check on its way, and whether the owner pressed for it.
    pending: Option<(Receiver<Result<Release, Problem>>, bool)>,
    /// A download on its way.
    downloading: Option<(Receiver<Result<Verified, Problem>>, Version)>,
    /// The newest release this run found, when it is newer than the running build.
    pub newer: Option<Release>,
    /// Downloaded and checked: installed when the window closes.
    pub ready: Option<Verified>,
    /// Restart to update was pressed.
    restart: bool,
}

/// The version that is running.
fn running() -> Option<Version> {
    Version::parse(env!("CARGO_PKG_VERSION"))
}

fn problem_text(p: Problem) -> String {
    match p {
        Problem::Unreachable | Problem::TooSlow | Problem::AskLater | Problem::Status(_) => {
            tr("GitHub could not be reached; try again later.").to_string()
        }
        Problem::NoRelease | Problem::PreRelease => tr("CashCraft is up to date.").to_string(),
        Problem::NotSigned | Problem::Mismatch => {
            tr("The new version was refused: it is not the one the project signed. Nothing was installed.").to_string()
        }
        Problem::Disk(kind) => trf("The new version could not be saved: {error}", &[("error", &kind)]),
        Problem::BadAnswer | Problem::Unsigned | Problem::NoKeys | Problem::NoFile | Problem::TooBig => {
            tr("The new version cannot be installed by CashCraft: download it from its page.").to_string()
        }
    }
}

impl Updates {
    /// For this computer's private folder `dir`.
    pub fn new(dir: PathBuf) -> Updates {
        let folders = Folders::new(dir);
        Updates {
            settings: Settings::load(&folders.settings()),
            folders,
            install: install::this_copy(PROJECT.program),
            asking: false,
            pending: None,
            downloading: None,
            newer: None,
            ready: None,
            restart: false,
        }
    }

    /// Whether this copy can install by itself what it finds.
    pub fn can_install(&self) -> bool {
        self.install.updates_itself() && PROJECT.has_keys()
    }

    pub fn mode(&self) -> Mode {
        match self.settings.check {
            Some(true) if self.settings.install && self.can_install() => Mode::Install,
            Some(true) => Mode::Notify,
            _ => Mode::Off,
        }
    }

    fn save(&self) -> Option<String> {
        self.settings
            .save(&self.folders.settings())
            .err()
            .map(|e| trf("The update settings cannot be saved: {error}", &[("error", &e)]))
    }

    /// The owner's choice in the sidebar, or the answer to the question. What to tell, if anything.
    pub fn choose(&mut self, mode: Mode) -> Option<String> {
        self.asking = false;
        self.settings.check = Some(mode != Mode::Off);
        if mode != Mode::Off && self.can_install() {
            self.settings.install = mode == Mode::Install;
        }
        self.save()
    }

    /// Ask GitHub once, on a thread of its own.
    fn check(&mut self, ctx: &egui::Context, owner: bool) {
        if self.pending.is_some() {
            return;
        }
        let pre = running().is_some_and(|v| v.is_pre_release());
        let (send, receive) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = send.send(github::latest(&PROJECT, pre));
            ctx.request_repaint();
        });
        self.pending = Some((receive, owner));
    }

    fn download(&mut self, ctx: &egui::Context, release: Release) {
        let Some(name) = self.install.file_name(PROJECT.program, &release.version, std::env::consts::ARCH) else {
            return;
        };
        if self.downloading.is_some() || !download::is_signed(&release) {
            return;
        }
        let dir = self.folders.downloads();
        let version = release.version.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let fetch = GitHub { user_agent: PROJECT.name };
            let _ = send.send(download::download(&PROJECT, &release, &name, &dir, &fetch));
            ctx.request_repaint();
        });
        self.downloading = Some((receive, version));
    }

    /// The owner pressed Check now.
    pub fn check_now(&mut self, ctx: &egui::Context) {
        self.check(ctx, true);
    }

    pub fn checking(&self) -> bool {
        self.pending.is_some() || self.downloading.is_some()
    }

    /// What is done at the start: what came of the last update, the question, the daily check.
    /// What to tell, if anything.
    fn start(&mut self, ctx: &egui::Context) -> Option<String> {
        let this = env!("CARGO_PKG_VERSION");
        let news = apply::at_start(&self.folders, this).map(|n| match n {
            StartNews::Updated { from, backup: Some(b) } => trf(
                "CashCraft was updated from {from} to {to}. The data file from before is kept in {path}.",
                &[("from", &from), ("to", &this), ("path", &b.display())],
            ),
            StartNews::Updated { from, backup: None } => {
                trf("CashCraft was updated from {from} to {to}.", &[("from", &from), ("to", &this)])
            }
            StartNews::NotInstalled { to } => {
                trf("CashCraft {version} was not installed: this is still the version before.", &[("version", &to)])
            }
        });
        if self.settings.check.is_none() && self.settings.asked < MOST_ASKS {
            self.asking = true;
            self.settings.asked = self.settings.asked.saturating_add(1);
            let _ = self.save();
        }
        if !cfg!(debug_assertions) && running().is_some() && self.settings.due(cashcraft_update::now()) {
            self.check(ctx, false);
        }
        news
    }

    /// What came back from GitHub this frame. What to tell, if anything.
    fn poll(&mut self, ctx: &egui::Context) -> Option<String> {
        let mut said = None;
        if let Some((receive, owner)) = &self.pending {
            let owner = *owner;
            match receive.try_recv() {
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.pending = None,
                Ok(answer) => {
                    self.pending = None;
                    if !matches!(answer, Err(Problem::Unreachable | Problem::TooSlow)) {
                        self.settings.last_check = cashcraft_update::now();
                        let _ = self.save();
                    }
                    match answer {
                        Ok(release) if running().is_some_and(|r| release.version.is_newer_than(&r)) => {
                            if owner {
                                said = Some(trf("CashCraft {version} is available.", &[("version", &release.version)]));
                            }
                            let wanted = self.mode() == Mode::Install && self.ready.is_none();
                            self.newer = Some(release.clone());
                            if wanted {
                                self.download(ctx, release);
                            }
                        }
                        Ok(_) => said = owner.then(|| tr("CashCraft is up to date.").to_string()),
                        Err(p) => said = owner.then(|| problem_text(p)),
                    }
                }
            }
        }
        if let Some((receive, _)) = &self.downloading {
            match receive.try_recv() {
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.downloading = None,
                Ok(Ok(verified)) => {
                    self.downloading = None;
                    self.ready = Some(verified);
                }
                Ok(Err(p)) => {
                    self.downloading = None;
                    if p.refused() {
                        said = Some(problem_text(p));
                    } else {
                        eprintln!("CashCraft update: {p}");
                    }
                }
            }
        }
        said
    }
}

impl App {
    /// Each frame: start, collect what came back, and the question above the status bar.
    pub(crate) fn updates_tick(&mut self, ui: &mut Ui) {
        if self.updates.is_none() {
            if self.screenshot_mode() {
                return;
            }
            let Some(home) = self.file.home.as_ref() else { return };
            let mut u = Updates::new(home.dir.join("updates"));
            let news = u.start(ui.ctx());
            self.updates = Some(u);
            if let Some(n) = news {
                self.toast = Some((n, ui.input(|i| i.time) + 20.0));
            }
        }
        let ctx = ui.ctx().clone();
        let Some(u) = self.updates.as_mut() else { return };
        if let Some(said) = u.poll(&ctx) {
            self.toast = Some((said, ui.input(|i| i.time) + 8.0));
        }
        if u.checking() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if !u.asking {
            return;
        }
        let mut answer = None;
        egui::Panel::bottom("update-question")
            .frame(Frame::new().fill(pal().card).inner_margin(Margin::symmetric(14, 6)).stroke(Stroke::new(1.0, pal().border)))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(tr(
                        "Should CashCraft check once a day whether a new version is out? It asks GitHub, and sends nothing about you or your data.",
                    ));
                    if primary_button(ui, tr("Yes, check daily")).clicked() {
                        answer = Some(Mode::Install);
                    }
                    if ui.button(tr("No")).clicked() {
                        answer = Some(Mode::Off);
                    }
                });
            });
        if let Some(mode) = answer {
            let said = u.choose(mode);
            if mode != Mode::Off {
                u.check(&ctx, false);
            }
            if let Some(s) = said {
                self.notify(ui, s);
            }
        }
    }

    /// The updates' part of the status bar, drawn right to left: whether there is one.
    pub(crate) fn updates_status(&mut self, ui: &mut Ui) -> bool {
        let Some(u) = self.updates.as_mut() else { return false };
        if let Some(ready) = u.ready.as_ref().filter(|_| u.mode() == Mode::Install) {
            if primary_button(ui, tr("Restart to update")).clicked() {
                u.restart = true;
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ui.label(
                RichText::new(trf(
                    "CashCraft {version} is ready: it is installed when CashCraft closes",
                    &[("version", &ready.version)],
                ))
                .size(11.5)
                .color(pal().text),
            );
            return true;
        }
        if let Some(newer) = &u.newer {
            let text = if newer.security {
                trf("CashCraft {version} is available and fixes a security problem", &[("version", &newer.version)])
            } else {
                trf("CashCraft {version} is available", &[("version", &newer.version)])
            };
            if u.downloading.is_some() {
                ui.spinner();
            }
            ui.hyperlink_to(RichText::new(text).size(11.5), &newer.page);
            return true;
        }
        false
    }

    /// The row of the sidebar: what to do about new versions, and Check now.
    pub(crate) fn updates_setting(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let Some(u) = self.updates.as_mut() else { return };
        let mut mode = u.mode();
        let modes: &[Mode] =
            if u.can_install() { &[Mode::Off, Mode::Notify, Mode::Install] } else { &[Mode::Off, Mode::Notify] };
        let mut said = None;
        ui.horizontal(|ui| {
            crate::views::setting_label(ui, tr("Updates"));
            egui::ComboBox::from_id_salt("updates").selected_text(mode.name()).show_ui(ui, |ui| {
                for &m in modes {
                    ui.selectable_value(&mut mode, m, m.name());
                }
            });
        });
        // On a line of its own, under the choice: the sidebar is too narrow for both side by side.
        ui.horizontal(|ui| {
            crate::views::setting_label(ui, "");
            let busy = u.checking();
            let text = format!("⟳ {}", tr("Check now"));
            if ui.add_enabled(!busy, egui::Button::new(text).small()).clicked() {
                u.check_now(&ctx);
            }
        });
        if mode != u.mode() {
            said = u.choose(mode);
            if mode != Mode::Off && u.settings.due(cashcraft_update::now()) {
                u.check(&ctx, false);
            } else if mode == Mode::Install
                && u.ready.is_none()
                && let Some(release) = u.newer.clone()
            {
                // Found by today's check while the owner wanted only to be told.
                u.download(&ctx, release);
            }
        }
        if let Some(s) = said {
            self.notify(ui, s);
        }
    }

    /// When the window closes: a version downloaded and checked is installed, after a copy of the
    /// data file has been made, and started again when Restart to update was pressed.
    pub(crate) fn updates_on_exit(&mut self) {
        let Some(u) = self.updates.as_mut() else { return };
        if u.mode() != Mode::Install {
            return;
        }
        let Some(ready) = u.ready.take() else { return };
        let backups = u.folders.root().join("backups");
        let backup = match self.db.path {
            Some(_) => match self.db.copy_into(&backups, &format!("before-{}", ready.version)) {
                Ok(path) => Some(path),
                Err(e) => {
                    eprintln!("CashCraft update: the data file could not be copied, nothing is installed: {e:#}");
                    return;
                }
            },
            None => None,
        };
        if let Err(e) = apply::on_exit(
            &u.folders,
            u.install,
            &ready,
            env!("CARGO_PKG_VERSION"),
            backup,
            u.restart,
            std::env::args_os().skip(1).collect(),
        ) {
            eprintln!("CashCraft update: {e}");
        }
    }
}

/// Started as the copy that installs an update (`--finish-update …`): install it and say so by
/// the exit code. `None` when the program was started for anything else.
pub fn finish_update_if_asked() -> Option<i32> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if args.first().and_then(|a| a.to_str()) != Some(apply::FINISH_UPDATE) {
        return None;
    }
    let Some(job) = apply::Job::parse(&args) else { return Some(2) };
    // The helper lives in `<private folder>/updates/helper/program/`.
    let root = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.parent()?.parent()?.to_path_buf()));
    Some(root.map_or(2, |root| apply::finish(&job, &Folders::new(root))))
}
