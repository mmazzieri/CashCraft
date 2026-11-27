// Adapted from MailCraft (https://github.com/UCS-Care-Group/mailcraft, apps/mailcraft/src/self_update.rs),
// Copyright (c) 2026 the MailCraft contributors, MIT OR Apache-2.0. See ATTRIBUTION.md.
//! Installing a new version once the window has closed, for a copy that updates itself (the
//! Windows `.msi`), and saying at the next start what came of it.
//!
//! The `.msi` is installed by Windows Installer (`msiexec /i <file> /passive /norestart`), in the
//! scope the running copy was installed in (for the user alone, with no prompt; for the
//! computer, with Windows' administrator prompt). That can't happen while the program runs (its
//! own file is in use), so a copy of the program, in the updates folder, waits for this one to
//! have gone ([`FINISH_UPDATE`], [`finish`]): for its lock, and then for the program file itself
//! to be free, since the lock goes a moment before the process does and an installer started in
//! that moment leaves the program out. It checks the file again, runs the installer (with the
//! whole log), makes sure the program is in its place (repairing the installation once when it is
//! not), and starts the program again when it was asked to and is there. A no to the prompt leaves
//! the old version, which says so at its next start ([`at_start`]).
//!
//! Nothing is run before it is checked: the file installed is the one downloaded and checked by
//! the release's signature in this run, and its SHA-256, the one the signed list gives, is worked
//! out again right before it is used. Windows Installer is handed a copy of it in the source
//! folder, checked as it is opened and held open until the installer is done ([`Handed`]); the
//! copy stays there, since Windows Installer repairs an installation from the file it was made
//! from. Programs are started with fixed arguments, never through a shell.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::download::{Verified, hex, sha256, sha256_of, unhex};
use crate::github::is_file_name;
use crate::install::Install;
use crate::version::Version;

/// Started with this (a copy of the program in the updates folder), the program installs an
/// `.msi` once the one that started it has gone ([`finish`]), and does nothing else.
pub const FINISH_UPDATE: &str = "--finish-update";
/// How long the copy waits for the program to have gone.
const WAIT_FOR_OLD: Duration = Duration::from_secs(60);
/// Largest record read.
const MAX_JSON: u64 = 64 * 1024;
/// What Windows Installer answers when it installed (3010, 1641: a restart of the computer is
/// wanted, or was started).
const MSI_DONE: [i32; 3] = [0, 3010, 1641];

/// The places the updating keeps things in: one folder of the program's own, never synced.
#[derive(Clone, Debug)]
pub struct Folders {
    root: PathBuf,
}

impl Folders {
    pub fn new(root: PathBuf) -> Folders {
        Folders { root }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Downloads, until they are installed.
    pub fn downloads(&self) -> PathBuf {
        self.root.join("downloads")
    }
    /// The settings ([`crate::Settings`]).
    pub fn settings(&self) -> PathBuf {
        self.root.join("settings.json")
    }
    fn record(&self) -> PathBuf {
        self.root.join("update-record.json")
    }
    fn lock(&self) -> PathBuf {
        self.root.join("update.lock")
    }
    /// The copy of the program that installs the `.msi`, and the installer's log.
    fn helper(&self) -> PathBuf {
        self.root.join("helper")
    }
    /// The `.msi` the program was last installed from, kept there: Windows Installer goes back to
    /// it to repair the installation, and can't when it is gone.
    fn source(&self) -> PathBuf {
        self.root.join("source")
    }
}

/// The record of the last update: from which version to which, and the copy of the owner's data
/// made before it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    from: String,
    to: String,
    #[serde(default)]
    backup: Option<PathBuf>,
}

/// What the last update came to, said once at the next start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartNews {
    /// This version was installed over `from`; the owner's data was copied to `backup` before.
    Updated { from: String, backup: Option<PathBuf> },
    /// Version `to` was to be installed and wasn't (the prompt was answered no, or installing
    /// failed): the old version is still here.
    NotInstalled { to: String },
}

fn read_record(folders: &Folders) -> Option<Record> {
    let file = File::open(folders.record()).ok()?;
    let mut text = String::new();
    io::Read::read_to_string(&mut io::Read::take(file, MAX_JSON), &mut text).ok()?;
    let r: Record = serde_json::from_str(&text).ok()?;
    (Version::parse(&r.from).is_some() && Version::parse(&r.to).is_some()).then_some(r)
}

fn write_record(folders: &Folders, r: &Record) -> io::Result<()> {
    fs::create_dir_all(&folders.root)?;
    let text = serde_json::to_vec_pretty(r).map_err(io::Error::other)?;
    let part = folders.record().with_extension("json.part");
    fs::write(&part, text)?;
    fs::rename(&part, folders.record())
}

/// What came of the last update, for version `running`: said once, then forgotten. Downloads of
/// versions no newer than this are deleted.
pub fn at_start(folders: &Folders, running: &str) -> Option<StartNews> {
    let news = read_record(folders).and_then(|r| {
        if r.to == running {
            Some(StartNews::Updated { from: r.from, backup: r.backup })
        } else if r.from == running {
            Some(StartNews::NotInstalled { to: r.to })
        } else {
            None
        }
    });
    let _ = fs::remove_file(folders.record());
    let _ = fs::remove_dir_all(folders.helper().join("program"));
    if let (Some(now), Ok(list)) = (Version::parse(running), fs::read_dir(folders.downloads())) {
        for entry in list.flatten().take(1000) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stale = name.split('-').nth(1).and_then(Version::parse).is_none_or(|v| !v.is_newer_than(&now));
            if stale {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    news
}

static LOCK: OnceLock<Mutex<Option<File>>> = OnceLock::new();

fn open_lock(folders: &Folders) -> io::Result<File> {
    fs::create_dir_all(&folders.root)?;
    fs::OpenOptions::new().create(true).truncate(false).write(true).open(folders.lock())
}

/// Take the lock in the updates folder and hold it until this program has gone: the copy that
/// installs waits for it.
pub fn hold_lock(folders: &Folders) -> bool {
    let slot = LOCK.get_or_init(|| Mutex::new(None));
    let mut held = slot.lock().unwrap_or_else(|e| e.into_inner());
    if held.is_some() {
        return true;
    }
    match open_lock(folders).and_then(|f| f.try_lock().map_err(io::Error::other).map(|()| f)) {
        Ok(f) => {
            *held = Some(f);
            true
        }
        Err(_) => false,
    }
}

/// Wait at most `most` for nobody to hold the lock: whether that came.
pub fn wait_for_lock(folders: &Folders, most: Duration) -> bool {
    let Ok(file) = open_lock(folders) else { return false };
    let start = Instant::now();
    loop {
        if file.try_lock().is_ok() {
            let _ = file.unlock();
            return true;
        }
        if start.elapsed() > most {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// A program and its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl Run {
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        command
    }

    /// Run it and wait: its exit code (`None`: it couldn't be started).
    fn wait(&self) -> Option<i32> {
        self.command().status().ok().and_then(|s| s.code())
    }

    /// Start it and leave it running after this program has gone.
    fn spawn_detached(&self) -> io::Result<()> {
        let mut command = self.command();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
            command.creation_flags(0x0000_0008 | 0x0000_0200);
        }
        command.spawn().map(|_| ())
    }
}

/// Windows Installer, from the system's own folder (never one found on `PATH`).
fn msiexec_program() -> PathBuf {
    let root = std::env::var_os("SystemRoot")
        .filter(|r| Path::new(r).is_absolute())
        .unwrap_or_else(|| OsString::from(r"C:\Windows"));
    PathBuf::from(root).join("System32").join("msiexec.exe")
}

/// `msiexec /i <msi> /passive /norestart /l*vx <log>`, for the user alone when `per_user`:
/// progress shown, no questions but Windows' own administrator prompt for an installation for the
/// whole computer, never a restart of the computer. The log is the whole one, so that what
/// Windows Installer did with each file can be read afterwards.
pub fn msiexec(msi: &Path, log: &Path, per_user: bool) -> Run {
    let mut args: Vec<OsString> = vec!["/i".into(), msi.as_os_str().into(), "/passive".into(), "/norestart".into()];
    if per_user {
        args.extend(["ALLUSERS=2".into(), "MSIINSTALLPERUSER=1".into()]);
    } else {
        args.push("ALLUSERS=1".into());
    }
    args.extend(["/l*vx".into(), log.as_os_str().into()]);
    Run { program: msiexec_program(), args }
}

/// `msiexec /fa <msi> /passive /norestart /l*vx <log>`: every file of the product that `msi`
/// installed put back, whatever Windows Installer thinks is there. For an installation that
/// said it was done and left the program out.
pub fn msiexec_repair(msi: &Path, log: &Path) -> Run {
    let args: Vec<OsString> = vec![
        "/fa".into(),
        msi.as_os_str().into(),
        "/passive".into(),
        "/norestart".into(),
        "/l*vx".into(),
        log.as_os_str().into(),
    ];
    Run { program: msiexec_program(), args }
}

/// Wait at most `most` for the program file at `exe` to be free: no process is running it any
/// more (Windows refuses to open a running program for writing). The lock the program holds is
/// let go a moment before the process has gone, and an installer started in that moment finds the
/// program in use. Whether it came free (or is not there).
pub fn wait_until_free(exe: &Path, most: Duration) -> bool {
    let start = Instant::now();
    loop {
        match fs::OpenOptions::new().write(true).open(exe) {
            Ok(_) => return true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return true,
            Err(_) => {}
        }
        if start.elapsed() > most {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// What the copy of the program that installs an `.msi` is to do ([`finish`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub msi: PathBuf,
    pub sha256: [u8; 32],
    pub per_user: bool,
    /// The installed program: waited for until no process runs it, looked for once the installer
    /// is done, and started again when `restart`.
    pub exe: PathBuf,
    pub restart: bool,
}

impl Job {
    /// The arguments it is started with.
    pub fn args(&self) -> Vec<OsString> {
        vec![
            FINISH_UPDATE.into(),
            self.msi.clone().into_os_string(),
            hex(&self.sha256).into(),
            if self.per_user { "user" } else { "machine" }.into(),
            self.exe.clone().into_os_string(),
            if self.restart { "restart" } else { "stay" }.into(),
        ]
    }

    /// The job its arguments (after the program's name) say, when they say one.
    pub fn parse(args: &[OsString]) -> Option<Job> {
        let text = |i: usize| args.get(i).and_then(|a| a.to_str());
        if text(0) != Some(FINISH_UPDATE) || args.len() != 6 {
            return None;
        }
        let per_user = match text(3)? {
            "user" => true,
            "machine" => false,
            _ => return None,
        };
        let restart = match text(5)? {
            "restart" => true,
            "stay" => false,
            _ => return None,
        };
        let job = Job {
            msi: PathBuf::from(args.get(1)?),
            sha256: unhex(text(2)?)?,
            per_user,
            exe: PathBuf::from(args.get(4)?),
            restart,
        };
        let named = job.msi.file_name().and_then(OsStr::to_str).is_some_and(is_file_name);
        let msi = named && job.msi.is_absolute() && job.msi.extension() == Some(OsStr::new("msi"));
        let exe = job.exe.is_absolute() && job.exe.extension() == Some(OsStr::new("exe"));
        (msi && exe).then_some(job)
    }
}

/// The copy of the program in the updates folder: wait for the program that started it to have
/// gone, check the `.msi`, install it, and start the program again when it was asked to. Returns
/// the process's exit code.
pub fn finish(job: &Job, folders: &Folders) -> i32 {
    let started = Instant::now();
    let _ = wait_for_lock(folders, WAIT_FOR_OLD);
    let _ = wait_until_free(&job.exe, WAIT_FOR_OLD.saturating_sub(started.elapsed()));
    let installed = install_msi(job, folders, &mut |run| run.wait(), &|p| p.is_file());
    if installed {
        let _ = fs::remove_file(&job.msi);
    }
    // Never started when it isn't there: Windows would only say it can't be found.
    if job.restart && job.exe.is_file() {
        let _ = Run { program: job.exe.clone(), args: Vec::new() }.spawn_detached();
    }
    if installed { 0 } else { 1 }
}

/// The `.msi` installed by Windows Installer, run by `run`, from a checked copy of it
/// ([`Handed`]); whether it was installed, with the program in its place (`present`). An
/// installation that says it is done and has left the program out is repaired once.
fn install_msi(
    job: &Job,
    folders: &Folders,
    run: &mut dyn FnMut(&Run) -> Option<i32>,
    present: &dyn Fn(&Path) -> bool,
) -> bool {
    let Ok(handed) = Handed::new(&job.msi, job.sha256, &folders.source()) else { return false };
    let _ = fs::create_dir_all(folders.helper());
    let done = |code: Option<i32>| code.is_some_and(|c| MSI_DONE.contains(&c));
    let mut installed = done(run(&msiexec(&handed.path, &folders.helper().join("install.log"), job.per_user)));
    if installed && !present(&job.exe) {
        installed = done(run(&msiexec_repair(&handed.path, &folders.helper().join("repair.log")))) && present(&job.exe);
    }
    drop(handed);
    installed
}

/// A checked file handed to Windows Installer: copied into the source folder (whatever else was
/// there goes: it was the source of the version before), checked once more by its SHA-256 as it is
/// opened, and held open (shared for reading only, on Windows) until the installer is done with
/// it. The copy stays, as the source Windows Installer repairs the installation from.
struct Handed {
    path: PathBuf,
    held: Option<File>,
}

impl Handed {
    fn new(file: &Path, expected: [u8; 32], within: &Path) -> io::Result<Handed> {
        let name = file
            .file_name()
            .and_then(OsStr::to_str)
            .filter(|n| is_file_name(n))
            .ok_or_else(|| io::Error::other("no name"))?;
        fs::create_dir_all(within)?;
        if let Ok(list) = fs::read_dir(within) {
            for entry in list.flatten().take(100) {
                let _ = fs::remove_file(entry.path());
            }
        }
        let mut handed = Handed { path: within.join(name), held: None };
        fs::copy(file, &handed.path)?;
        let mut held = Handed::open(&handed.path)?;
        if sha256_of(&mut held) != Some(expected) {
            drop(held);
            let _ = fs::remove_file(&handed.path);
            return Err(io::Error::other("the copy isn't the file that was checked"));
        }
        handed.held = Some(held);
        Ok(handed)
    }

    fn open(path: &Path) -> io::Result<File> {
        let mut open = fs::OpenOptions::new();
        open.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            // FILE_SHARE_READ
            open.share_mode(0x0000_0001);
        }
        open.open(path)
    }
}

impl Drop for Handed {
    fn drop(&mut self) {
        drop(self.held.take());
    }
}

/// Once the window has closed (and the owner's data was copied to `backup`): install `verified`
/// by a copy of the program that waits for this one to have gone, and start the new version
/// when `restart`. Version `running` is the one replaced.
pub fn on_exit(
    folders: &Folders,
    install: Install,
    verified: &Verified,
    running: &str,
    backup: Option<PathBuf>,
    restart: bool,
) -> Result<(), String> {
    let Install::Msi { per_user } = install else { return Err("this copy doesn't update itself".into()) };
    if sha256(&verified.path) != Some(verified.sha256) {
        let _ = fs::remove_file(&verified.path);
        return Err("the download changed since it was checked; it is deleted".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    write_record(folders, &Record { from: running.into(), to: verified.version.to_string(), backup })
        .map_err(|e| e.kind().to_string())?;
    let job = Job { msi: verified.path.clone(), sha256: verified.sha256, per_user, exe: exe.clone(), restart };
    let dir = folders.helper().join("program");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = exe.file_name().map(|n| n.to_string_lossy().replace(".exe", "-update.exe")).unwrap_or_default();
    let copy = dir.join(if name.is_empty() { "update.exe".to_string() } else { name });
    fs::copy(&exe, &copy).map_err(|e| format!("the installer's helper couldn't be made: {}", e.kind()))?;
    if !hold_lock(folders) {
        return Err("another copy is installing an update".into());
    }
    Run { program: copy, args: job.args() }
        .spawn_detached()
        .map_err(|e| format!("the installer couldn't be started: {}", e.kind()))
}

#[cfg(test)]
pub(crate) fn install_msi_with(
    job: &Job,
    folders: &Folders,
    run: &mut dyn FnMut(&Run) -> Option<i32>,
    present: &dyn Fn(&Path) -> bool,
) -> bool {
    install_msi(job, folders, run, present)
}
