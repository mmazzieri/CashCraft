//! Where the data file is, and how it is used: *directly*, by SQLite where it lies, or *shared*, through a private
//! working copy when a sync app or a network carries it. `docs/data-file.md` says what happens and why.

mod compare;
mod content;
mod detect;
mod presence;
#[cfg(test)]
mod tests;

use compare::differences;
pub use compare::{Differences, Line};
pub use detect::{Service, System, classify, service_of};
pub use presence::{Presence, RENEW};

use crate::db::{Db, SCHEMA_VERSION};
use crate::i18n::{Lang, tr, trf};
use crate::migrate::publish;
use anyhow::{Context, Result, anyhow, bail};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A data file is never larger.
const MAX_FILE: u64 = 1 << 30;

/// The folder of a path; the current one for a name alone.
pub(crate) fn parent(path: &Path) -> &Path {
    path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

/// A name that can be part of the name of a file everywhere.
pub(crate) fn clean_name(name: &str) -> String {
    let good = |c: &char| !c.is_control() && !r#"<>:"/\|?*"#.contains(*c);
    let out: String = name.chars().filter(good).take(40).collect();
    let out = out.trim().trim_end_matches('.').to_string();
    if out.is_empty() { "computer".into() } else { out }
}

fn computer_name() -> String {
    let var = std::env::var("COMPUTERNAME").ok();
    let name = var.or_else(|| std::fs::read_to_string("/proc/sys/kernel/hostname").ok());
    let name = name.or_else(|| {
        let out = std::process::Command::new("hostname").output().ok()?;
        String::from_utf8(out.stdout).ok()
    });
    clean_name(&name.unwrap_or_default())
}

/// A folder that goes away with the value.
pub(crate) struct Scratch(pub PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The private folder of this user on this computer.
#[derive(Clone, Debug)]
pub struct Home {
    pub dir: PathBuf,
    pub computer: String,
}

impl Home {
    pub fn user() -> Home {
        let chosen = std::env::var_os("CASHCRAFT_PRIVATE").filter(|v| !v.is_empty()).map(PathBuf::from);
        let dir = chosen.filter(|p| p.is_absolute());
        Home { dir: dir.unwrap_or_else(|| crate::paths::local_dir().join("CashCraft")), computer: computer_name() }
    }

    fn make(dir: &Path) -> Result<()> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(dir).with_context(|| trf("cannot create {path}", &[("path", &dir.display())]))
    }

    /// A number of this computer, so that what it leaves in a folder is not taken for another's.
    fn id(&self) -> String {
        let file = self.dir.join("computer-id");
        let read = || {
            let text = std::fs::read_to_string(&file).ok()?;
            Some(text.trim().to_string()).filter(|t| t.len() == 8 && t.bytes().all(|b| b.is_ascii_hexdigit()))
        };
        read().unwrap_or_else(|| {
            let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos();
            let seed = format!("{} {now} {} {}", self.computer, std::process::id(), self.dir.display());
            let id: String = content::sha256(seed.as_bytes()).chars().take(8).collect();
            let made = Home::make(&self.dir).is_ok()
                && OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&file)
                    .and_then(|mut f| f.write_all(id.as_bytes()))
                    .is_ok();
            if made { id } else { read().unwrap_or(id) }
        })
    }

    pub(crate) fn scratch(&self) -> Result<Scratch> {
        static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = self.dir.join(format!("scratch-{}-{n}", std::process::id()));
        Home::make(&dir)?;
        Ok(Scratch(dir))
    }

    /// The private folder of a shared file.
    fn copy_of(&self, path: &Path) -> PathBuf {
        let id: String = content::sha256(path.as_os_str().as_encoded_bytes()).chars().take(24).collect();
        self.dir.join("copies").join(id)
    }

    /// What the user said of this file.
    pub fn choice(&self, path: &Path) -> Use {
        let text = std::fs::read_to_string(self.copy_of(&resolved(path)).join("mode")).unwrap_or_default();
        Use::from_code(text.trim()).unwrap_or(Use::Auto)
    }

    pub fn set_choice(&self, path: &Path, choice: Use) -> Result<()> {
        let dir = self.copy_of(&resolved(path));
        Home::make(&dir)?;
        replace(&dir.join("mode"), choice.code().as_bytes())
    }

    fn last_file(&self) -> PathBuf {
        self.dir.join("last-file")
    }

    /// The file the user opened last, if it was another than the default one.
    pub fn recent(&self) -> Option<PathBuf> {
        let bytes = std::fs::read(self.last_file()).ok().filter(|b| !b.is_empty())?;
        #[cfg(unix)]
        let path = PathBuf::from(<std::ffi::OsString as std::os::unix::ffi::OsStringExt>::from_vec(bytes));
        #[cfg(not(unix))]
        let path = PathBuf::from(String::from_utf8(bytes).ok()?);
        Some(path)
    }

    /// Opens this file when none is named, from now on; `None` for the default one.
    pub fn remember(&self, path: Option<&Path>) -> Result<()> {
        let Some(path) = path else {
            return match std::fs::remove_file(self.last_file()) {
                Err(e) if e.kind() != ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            };
        };
        // On Windows the encoded bytes of a name that is not Unicode could not be read back.
        if cfg!(not(unix)) && path.to_str().is_none() {
            bail!(tr("the path has characters that cannot be remembered"));
        }
        Home::make(&self.dir)?;
        // It is in use again.
        let _ = std::fs::remove_file(self.copy_of(&resolved(path)).join("moved"));
        replace(&self.last_file(), path.as_os_str().as_encoded_bytes())
    }

    /// The data file to use, and whether it must be there: the one that was named, the one opened last, the default.
    pub fn pick(&self, named: Option<PathBuf>) -> (PathBuf, bool) {
        let by_variable = std::env::var("CASHCRAFT_DB").is_ok_and(|p| !p.trim().is_empty());
        match named {
            Some(p) => (p, false),
            None if by_variable => (crate::db::default_db_path(), false),
            None => self.recent().map_or_else(|| (crate::db::default_db_path(), false), |p| (p, true)),
        }
    }

    /// Where a file went with `keep`.
    pub fn moved(&self, path: &Path) -> Option<PathBuf> {
        let text = std::fs::read_to_string(self.copy_of(&resolved(path)).join("moved")).ok()?;
        Some(PathBuf::from(text)).filter(|p| !p.as_os_str().is_empty())
    }

    /// Working copies with changes that are not in their file, whatever file this is.
    pub fn unsaved(&self) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        for dir in std::fs::read_dir(self.dir.join("copies")).into_iter().flatten().flatten() {
            let state = State::load(&dir.path());
            let copy = dir.path().join(COPY);
            if !copy.exists() {
                continue;
            }
            // One that cannot be read is worth saying too.
            let Ok(print) = copy_print(&copy) else {
                out.push((state.path, copy));
                continue;
            };
            if print.is_some() && print != state.base.map(|b| b.print) {
                out.push((state.path, copy));
            }
        }
        out.sort();
        out
    }
}

/// How a file is used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Use {
    /// Shared when it is seen to be in a synced folder or on a network drive.
    #[default]
    Auto,
    Direct,
    Shared,
}

impl Use {
    pub const ALL: [Use; 3] = [Use::Auto, Use::Direct, Use::Shared];

    pub fn code(self) -> &'static str {
        match self {
            Use::Auto => "auto",
            Use::Direct => "direct",
            Use::Shared => "shared",
        }
    }

    pub fn from_code(code: &str) -> Option<Use> {
        Use::ALL.into_iter().find(|c| c.code() == code)
    }
}

/// The name that a folder has for what was found as `name`, whatever its case.
fn real_name(name: &OsStr, names: impl Iterator<Item = std::ffi::OsString>) -> std::ffi::OsString {
    let mut like = None;
    for n in names {
        if n == name {
            return n;
        }
        if n.to_string_lossy().to_lowercase() == name.to_string_lossy().to_lowercase() {
            like = Some(n);
        }
    }
    like.unwrap_or_else(|| name.to_os_string())
}

/// On Linux a disk that does not tell cases apart gives each name as it was asked.
fn real_names(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        let std::path::Component::Normal(name) = part else {
            out.push(part);
            continue;
        };
        // Where cases differ, another case is not there.
        let text = name.to_string_lossy();
        let other: String = text
            .chars()
            .map(|c| if c.is_ascii_lowercase() { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() })
            .collect();
        let insensitive = other != text && out.join(&*other).symlink_metadata().is_ok() && out.join(name).exists();
        let listed = insensitive.then(|| std::fs::read_dir(&out).ok()).flatten();
        let real = listed.map(|d| real_name(name, d.flatten().map(|e| e.file_name())));
        out.push(real.as_deref().unwrap_or(name));
    }
    out
}

/// The path of the file itself: links are followed, for a shared file is replaced whole.
pub fn resolved(path: &Path) -> PathBuf {
    let found = std::fs::canonicalize(path).ok().or_else(|| {
        let dir = std::fs::canonicalize(parent(path)).ok()?;
        Some(dir.join(path.file_name()?))
    });
    let found = found.or_else(|| std::path::absolute(path).ok()).unwrap_or_else(|| path.to_path_buf());
    let found = if cfg!(target_os = "linux") { real_names(&found) } else { found };
    match found.to_str() {
        Some(text) if cfg!(windows) => {
            PathBuf::from(String::from_utf8_lossy(&detect::plain(text.as_bytes())).into_owned())
        }
        _ => found,
    }
}

/// How this file is to be used, and the service that keeps it.
pub fn way(home: &Home, path: &Path, asked: Use) -> (bool, Option<Service>) {
    let system = System::here();
    let of = |p: &Path| service_of(&resolved(p), &system).or_else(|| service_of(p, &system));
    let service = of(path);
    let choice = if asked == Use::Auto { home.choice(path) } else { asked };
    let shared = match choice {
        // Once shared, whatever the environment says next time.
        Use::Auto if Shared::new(home, path).copy().exists() => true,
        // A home folder on a network has no local place for a copy: as it always was.
        Use::Auto if service == Some(Service::Network) => of(&home.dir.join(COPY)) != Some(Service::Network),
        Use::Auto => service.is_some(),
        Use::Direct => false,
        Use::Shared => true,
    };
    (shared, service)
}

/// An open data file.
pub struct Opened {
    pub db: Db,
    pub is_new: bool,
    /// Of a shared file, with what happened when it was read.
    pub shared: Option<(Shared, Report)>,
    /// What to say when the file is opened.
    pub notices: Vec<String>,
}

/// Opens a data file, or makes it unless it must be there.
pub fn open(home: &Home, path: &Path, asked: Use, seed: Lang, must_exist: bool) -> Result<Opened> {
    let mut notices = Vec::new();
    let missing = || anyhow!(trf("{path} is not there", &[("path", &path.display())]));
    let shared = Shared::new(home, path);
    let has_copy = copy_print(&shared.copy())?.is_some();
    // A file that is known, or a link to one, is never replaced by an empty one.
    let known = has_copy
        || std::fs::symlink_metadata(path).is_ok()
        || home.recent().is_some_and(|p| resolved(&p) == shared.path);
    let must_exist = must_exist || known;
    if let Some(now) = home.moved(path) {
        notices.push(trf("the data file was moved to {path}", &[("path", &now.display())]));
    }
    for (file, copy) in home.unsaved() {
        if copy != shared.copy() {
            notices.push(trf(
                "Changes not written to {file} yet are in {copy}",
                &[("file", &file), ("copy", &copy.display())],
            ));
        }
    }
    let (is_shared, service) = way(home, path, asked);
    if !is_shared {
        if must_exist && !path.exists() {
            return Err(missing());
        }
        shared.allow_direct(path)?;
        let (db, is_new) = Db::open_seeding(path, seed)?;
        return Ok(Opened { db, is_new, shared: None, notices });
    }
    if let Some(s) = service_of(&resolved(&home.dir.join(COPY)), &System::here()) {
        bail!(trf(
            "the private folder {path} is in {service}: name one on this computer in CASHCRAFT_PRIVATE",
            &[("path", &home.dir.display()), ("service", &s.label())]
        ));
    }
    let there = path.exists();
    #[cfg(unix)]
    if std::fs::metadata(path).is_ok_and(|m| std::os::unix::fs::MetadataExt::nlink(&m) > 1) {
        notices.push(tr("The data file has other names (hard links): they will keep what it has now.").into());
    }
    let mut report = shared.sync()?;
    let hold = shared.hold()?;
    if !has_copy && copy_print(&shared.copy())?.is_none() {
        match &report.status {
            Status::Absent if !must_exist => {}
            Status::Absent => return Err(missing()),
            other => bail!("{}", other.text()),
        }
    }
    let (db, is_new) = Db::open_seeding(&shared.copy(), seed)?;
    drop(hold);
    if is_new {
        report = shared.sync()?;
    }
    if there && !has_copy {
        let why = service.map_or_else(|| tr("a synced folder").to_string(), |s| s.label().to_string());
        notices.push(trf(
            "The data file is from now on used through a private copy, as it is in {service}. See `cashcraft-cli data status`.",
            &[("service", &why)],
        ));
    }
    Ok(Opened { db, is_new, shared: Some((shared, report)), notices })
}

/// How things are between the working copy and the shared file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// The shared file has everything.
    Saved,
    /// There is no shared file yet, nor anything to make it of.
    Absent,
    /// The shared file cannot be read or written now, and why.
    Waiting(String),
    /// Another program has it open in place.
    InUse,
    /// It changed, and a form that is open on this computer comes first.
    Held,
}

impl Status {
    pub fn text(&self) -> String {
        match self {
            Status::Saved => tr("saved").into(),
            Status::Absent => tr("the data file is not there").into(),
            Status::Waiting(why) => trf("waiting for the folder: {reason}", &[("reason", why)]),
            Status::InUse => tr("waiting: a program has the data file open, or left its log beside it (see `cashcraft-cli data recover`)").into(),
            Status::Held => tr("changed on another computer: close the form that is open to see it").into(),
        }
    }
}

/// What a synchronization did.
#[derive(Clone, Debug)]
pub struct Report {
    pub status: Status,
    /// The working copy has changes that the shared file has not.
    pub pending: bool,
    /// The working copy took what another computer wrote.
    pub adopted: bool,
    /// The name of the file that now has the changes of this computer, because another had changed the shared one.
    pub conflict: Option<String>,
    /// Of the content the copy had at the end.
    pub revision: String,
    /// What to say once, when the file is opened.
    pub first: Vec<String>,
}

impl Report {
    pub fn conflict_notice(&self) -> Option<String> {
        let name = self.conflict.as_ref()?;
        Some(trf(
            "The data file was changed on another computer too. Nothing was lost: the changes of this computer are in \"{name}\", beside it, and the data file now shows those of the other.",
            &[("name", name)],
        ))
    }

    /// What people should know; nothing when all is saved.
    pub fn notices(&self) -> Vec<String> {
        let mut out: Vec<String> = self.conflict_notice().into_iter().collect();
        if self.status != Status::Saved {
            out.push(match self.pending {
                true => trf(
                    "The changes are safe on this computer and will be written later. Data file: {status}",
                    &[("status", &self.status.text())],
                ),
                false => trf("Data file: {status}", &[("status", &self.status.text())]),
            });
        }
        out
    }
}

fn new_revision(of: &str) -> String {
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos();
    content::sha256(format!("{now} {} {of}", std::process::id()).as_bytes()).chars().take(10).collect()
}

const COPY: &str = "copy.db";

/// What is known of the shared file, kept beside the working copy.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct State {
    /// For people only.
    path: String,
    /// The shared file as it was last read or written.
    base: Option<Base>,
    /// What is about to be written.
    writing: Option<Base>,
    /// What is about to be taken, and the fingerprint the copy has then: another one, if the format is older.
    taking: Option<(Base, String)>,
    /// A file of ours in the shared folder that has not its final name yet.
    temporary: Option<String>,
    /// What was last put in a conflict file.
    kept: Option<String>,
    /// Conflict files that the user has not dismissed.
    conflicts: Vec<String>,
    /// Changes when the copy takes another content: ids of before may be of other things.
    revision: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
struct Base {
    len: u64,
    modified: Option<SystemTime>,
    sha: String,
    /// The fingerprint of what it holds.
    print: String,
}

impl State {
    fn load(dir: &Path) -> State {
        // Without it, what is in the copy counts as changed: nothing is lost.
        let text = std::fs::read(dir.join("state.json")).unwrap_or_default();
        serde_json::from_slice(&text).unwrap_or_default()
    }

    fn save(&self, dir: &Path) -> Result<()> {
        replace(&dir.join("state.json"), &serde_json::to_vec(self)?)
    }
}

/// Replaces a private file whole.
fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.part", std::process::id()));
    let part = path.with_file_name(name);
    let mut f = File::create(&part)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    rename(&part, path)
}

/// Renames over what is there. On Windows a program that looks at the file for a moment makes it fail.
fn rename(from: &Path, to: &Path) -> Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if tries < 6 && cfg!(windows) && e.kind() != ErrorKind::NotFound => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(100 * tries));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn sync_folder(dir: &Path) {
    // Where a folder can be opened; the rename is atomic anyway.
    if cfg!(unix)
        && let Ok(d) = File::open(dir)
    {
        let _ = d.sync_all();
    }
}

/// The fingerprint of a working copy; `None` when there is no data file in it.
fn copy_print(copy: &Path) -> Result<Option<String>> {
    if !copy.exists() {
        return Ok(None);
    }
    let read = || {
        let conn = content::open(copy)?;
        if !content::has_accounts(&conn)? {
            return Ok(None);
        }
        content::fingerprint(&conn).map(Some)
    };
    read().with_context(|| trf("the working copy {path} cannot be read", &[("path", &copy.display())]))
}

/// The shared file, read whole.
struct Fetched {
    base: Base,
    bytes: Vec<u8>,
}

/// Why it was not.
enum Unread {
    Missing,
    Later(String),
}

impl Unread {
    fn text(&self) -> String {
        match self {
            Unread::Missing => tr("the data file is not there").into(),
            Unread::Later(why) => why.clone(),
        }
    }
}

/// What became of a write.
enum Written {
    Done,
    /// The shared file is not the one that was read.
    Changed,
    InUse,
    Failed(String),
}

/// Keeps the working copy as it is while it lives: nothing of other computers comes in.
pub struct Hold(File);
impl Drop for Hold {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// The error of a program that dies, in the tests.
const KILLED: &str = "killed";

#[cfg(test)]
type Faults = std::sync::Arc<dyn Fn(&'static str) -> Result<()> + Send + Sync>;

/// A data file that is used through a working copy.
#[derive(Clone)]
pub struct Shared {
    home: Home,
    path: PathBuf,
    dir: PathBuf,
    /// Where the program dies, in the tests.
    #[cfg(test)]
    faults: Option<Faults>,
}

impl Shared {
    pub fn new(home: &Home, path: &Path) -> Shared {
        let path = resolved(path);
        Shared {
            home: home.clone(),
            dir: home.copy_of(&path),
            path,
            #[cfg(test)]
            faults: None,
        }
    }

    /// The shared file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The working copy.
    pub fn copy(&self) -> PathBuf {
        self.dir.join(COPY)
    }

    pub fn home(&self) -> &Home {
        &self.home
    }

    fn folder(&self) -> &Path {
        parent(&self.path)
    }

    fn step(&self, _name: &'static str) -> Result<()> {
        #[cfg(test)]
        if let Some(f) = &self.faults {
            f(_name)?;
        }
        Ok(())
    }

    fn lock_file(&self, name: &str) -> Result<File> {
        Home::make(&self.dir)?;
        Ok(OpenOptions::new().read(true).write(true).create(true).truncate(false).open(self.dir.join(name))?)
    }

    /// To be had while the working copy is read and changed.
    pub fn hold(&self) -> Result<Hold> {
        let f = self.lock_file("hold.lock")?;
        f.lock_shared()?;
        Ok(Hold(f))
    }

    /// [`Shared::hold`] without waiting: `None` while something comes in.
    pub fn try_hold(&self) -> Result<Option<Hold>> {
        let f = self.lock_file("hold.lock")?;
        match f.try_lock_shared() {
            Ok(()) => Ok(Some(Hold(f))),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }

    /// `None` while somebody on this computer is using the copy.
    fn alone(&self) -> Result<Option<Hold>> {
        let f = self.lock_file("hold.lock")?;
        for _ in 0..40 {
            match f.try_lock() {
                Ok(()) => return Ok(Some(Hold(f))),
                Err(std::fs::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(25)),
                Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
            }
        }
        Ok(None)
    }

    /// Changes when the copy takes what another computer wrote: ids may mean other things since.
    pub fn revision(&self) -> String {
        State::load(&self.dir).revision
    }

    /// The revision that the command line of this computer last read; empty when it read none.
    pub fn last_read(&self) -> String {
        std::fs::read_to_string(self.dir.join("read")).unwrap_or_default()
    }

    pub fn set_last_read(&self, revision: &str) -> Result<()> {
        replace(&self.dir.join("read"), revision.as_bytes())
    }

    /// What waits in a private copy would be forgotten, if the file were changed where it is.
    fn allow_direct(&self, path: &Path) -> Result<()> {
        let Some(print) = copy_print(&self.copy())? else { return Ok(()) };
        let base = State::load(&self.dir).base.map(|b| b.print);
        let same = if base.is_some() {
            base == Some(print)
        } else {
            let scratch = self.home.scratch()?;
            path.exists() && snapshot(path, &scratch.0.join("there.db")).is_ok_and(|(_, p)| p == print)
        };
        if !same {
            bail!(trf(
                "{path} cannot be used directly: there are changes in its private copy that are not in it yet. Use it through the copy first (--shared).",
                &[("path", &path.display())]
            ));
        }
        Ok(())
    }

    /// Whether the shared file does not look like the one that was last read or written.
    pub fn looks_changed(&self) -> bool {
        let base = State::load(&self.dir).base;
        let now = std::fs::metadata(&self.path).ok().map(|m| (m.len(), m.modified().ok()));
        base.map(|b| (b.len, b.modified)) != now
    }

    /// The conflict files that are still there and were not dismissed.
    pub fn conflicts(&self) -> Vec<PathBuf> {
        let all = State::load(&self.dir).conflicts;
        all.iter().map(|n| self.folder().join(n)).filter(|p| p.exists()).collect()
    }

    /// Speaks no more of the conflict files; they stay where they are.
    pub fn dismiss_conflicts(&self) -> Result<()> {
        let lock = self.lock_file("sync.lock")?;
        lock.lock()?;
        let mut st = State::load(&self.dir);
        st.conflicts.clear();
        st.save(&self.dir)
    }

    /// Other versions of the file beside it, whoever made them: `name (…).db`.
    pub fn versions(&self) -> Vec<PathBuf> {
        let start = format!("{} (", self.path.file_stem().unwrap_or_default().to_string_lossy());
        let mut out: Vec<PathBuf> = std::fs::read_dir(self.folder())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension() == self.path.extension())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&start)))
            .collect();
        out.sort();
        out
    }

    /// The entries that only the data file, or only another one, has.
    pub fn differences(&self, other: &Path) -> Result<Differences> {
        let scratch = self.home.scratch()?;
        let here = scratch.0.join("here.db");
        snapshot(&self.copy(), &here)?;
        differences(&self.home, &here, other)
    }

    /// Whether the folder is there and the file is not.
    pub fn is_missing(&self) -> bool {
        matches!(self.read_whole(), Err(Unread::Missing))
    }

    /// Writes the file again where it was and is not any more. What is known of it stays until that is done.
    pub fn put_back(&self) -> Result<Report> {
        let lock = self.lock_file("sync.lock")?;
        lock.lock()?;
        let mut st = State::load(&self.dir);
        match self.read_whole() {
            Err(Unread::Missing) => {}
            Err(Unread::Later(why)) => bail!(why),
            Ok(_) => bail!(trf("{path} is there already", &[("path", &self.path.display())])),
        }
        if self.has_log() {
            bail!("{}", Status::InUse.text());
        }
        match self.write(&mut st, None)? {
            Written::Done => {}
            Written::Changed => bail!(trf("{path} is there already", &[("path", &self.path.display())])),
            Written::InUse => bail!("{}", Status::InUse.text()),
            Written::Failed(why) => bail!(why),
        }
        drop(lock);
        self.sync()
    }

    /// Carries what changed, one way or the other. Never overwrites a shared file that is not the one that was read,
    /// nor changes of the copy that are nowhere else.
    pub fn sync(&self) -> Result<Report> {
        let lock = self.lock_file("sync.lock")?;
        lock.lock()?;
        let mut st = State::load(&self.dir);
        st.path = self.path.display().to_string();
        let mut report = Report {
            status: Status::Saved,
            pending: false,
            adopted: false,
            conflict: None,
            revision: String::new(),
            first: Vec::new(),
        };
        if st.revision.is_empty() {
            st.revision = new_revision("");
            st.save(&self.dir)?;
        }
        // A shared file that changes all the time is for the next turn.
        for _ in 0..4 {
            report.status = match self.turn(&mut st, &mut report)? {
                Some(s) => s,
                None => continue,
            };
            let print = copy_print(&self.copy())?;
            report.revision = st.revision;
            report.pending = print.is_some() && print != st.base.map(|b| b.print);
            return Ok(report);
        }
        report.status = Status::Waiting(tr("the data file is changing").into());
        report.pending = true;
        report.revision = st.revision;
        Ok(report)
    }

    /// One look at the two files. `None` when it must be done again.
    fn turn(&self, st: &mut State, report: &mut Report) -> Result<Option<Status>> {
        // Whose it is cannot be told: the user says when nobody has the file open.
        if self.has_log() {
            return Ok(Some(Status::InUse));
        }
        let fetched = match self.fetch() {
            Ok(f) => f,
            Err(Unread::Later(why)) => return Ok(Some(Status::Waiting(why))),
            Err(Unread::Missing) => {
                if st.base.is_some() {
                    return Ok(Some(Status::Waiting(tr("the data file is not there").into())));
                }
                return match copy_print(&self.copy())? {
                    Some(_) => Ok(written(self.write(st, None)?)),
                    None => Ok(Some(Status::Absent)),
                };
            }
        };
        self.sweep(st)?;
        // A write that ended and was not noted.
        if let Some(w) = st.writing.take() {
            if w.sha == fetched.base.sha {
                st.base = Some(w);
            }
            st.save(&self.dir)?;
        }
        // And a content that was taken.
        if let Some((taken, print)) = st.taking.take() {
            if copy_print(&self.copy())? == Some(print) {
                st.base = Some(taken);
            }
            st.save(&self.dir)?;
        }
        if st.base.as_ref().is_some_and(|b| b.sha == fetched.base.sha) {
            let print = copy_print(&self.copy())?;
            if print.is_some() {
                // Touched, not changed.
                if let Some(b) = &mut st.base
                    && b.modified != fetched.base.modified
                {
                    b.modified = fetched.base.modified;
                    st.save(&self.dir)?;
                }
                if print.as_ref() == st.base.as_ref().map(|b| &b.print) {
                    return Ok(Some(Status::Saved));
                }
                return Ok(written(self.write(st, Some(&fetched.base.sha))?));
            }
        }
        self.take(st, fetched, report)
    }

    /// The shared file is another: the copy takes it, and what it had of its own goes into a file beside.
    fn take(&self, st: &mut State, fetched: Fetched, report: &mut Report) -> Result<Option<Status>> {
        let Fetched { mut base, bytes } = fetched;
        let scratch = self.home.scratch()?;
        let file = scratch.0.join("shared.db");
        std::fs::write(&file, &bytes)?;
        match self.check(&file) {
            Ok(print) => base.print = print,
            Err(e) => return Ok(Some(Status::Waiting(format!("{e:#}")))),
        }
        self.step("checked")?;
        // Written again by somebody, with the same content: what is to be written still is.
        if copy_print(&self.copy())?.is_some() && st.base.as_ref().is_some_and(|b| b.print == base.print) {
            st.base = Some(base);
            st.save(&self.dir)?;
            return Ok(None);
        }
        let Some(_alone) = self.alone()? else {
            return Ok(Some(Status::Held));
        };
        let local = copy_print(&self.copy())?;
        if local.as_ref() != Some(&base.print) {
            // Of an older format: updated here, with its backup in the private folder. It is written back as a change.
            if let Err(e) = self.update(&file) {
                if e.to_string() == KILLED {
                    return Err(e);
                }
                return Ok(Some(Status::Waiting(format!("{e:#}"))));
            }
            let dirty = match (&local, &st.base) {
                (None, _) => false,
                (Some(l), Some(b)) => *l != b.print,
                (Some(_), None) => true,
            };
            if dirty && st.kept != local {
                match self.keep(st)? {
                    Ok(name) => {
                        report.conflict = Some(name.clone());
                        st.conflicts.push(name);
                        st.kept = local.clone();
                        self.step("kept")?;
                        st.save(&self.dir)?;
                    }
                    Err(why) => return Ok(Some(Status::Waiting(why))),
                }
            }
            // Said before it happens: a revision that changes for nothing only asks to read again.
            st.revision = new_revision(&base.sha);
            st.taking = Some((base.clone(), content::fingerprint(&content::open(&file)?)?));
            st.save(&self.dir)?;
            self.step("adopting")?;
            self.adopt(&file)?;
            report.adopted |= local.is_some();
            self.step("adopted")?;
        }
        st.base = Some(base);
        st.taking = None;
        st.kept = None;
        st.save(&self.dir)?;
        // There may be something to write: a format that was updated.
        Ok(None)
    }

    /// Whether the bytes that were read are a data file of ours, and the fingerprint of what they hold.
    fn check(&self, file: &Path) -> Result<String> {
        let conn = content::open(file)?;
        let damaged = || anyhow!(tr("the data file is damaged, or has not been downloaded whole"));
        if !content::is_sound(&conn).unwrap_or(false) {
            return Err(damaged());
        }
        let version = content::format(&conn)?;
        if version > SCHEMA_VERSION {
            bail!(trf(
                "the data file was written by a newer version of CashCraft (format {version})",
                &[("version", &version)]
            ));
        }
        if !content::has_accounts(&conn)? {
            bail!(tr("it is not a data file of CashCraft"));
        }
        content::fingerprint(&conn)
    }

    fn update(&self, file: &Path) -> Result<()> {
        if content::format(&content::open(file)?)? == SCHEMA_VERSION {
            return Ok(());
        }
        // The backup is made beside, and named after the file.
        let dir = self.dir.join("updating");
        let _ = std::fs::remove_dir_all(&dir);
        Home::make(&dir)?;
        let stem = self.path.file_stem().unwrap_or_default();
        let mut name = stem.to_os_string();
        name.push(".db");
        let there = dir.join(name);
        std::fs::copy(file, &there)?;
        let (db, _) = Db::open(&there)?;
        self.step("updated")?;
        db.conn.close().map_err(|(_, e)| e)?;
        content::settle(content::open(&there)?, &there)?;
        std::fs::copy(&there, file)?;
        content::remove_with_logs(&there);
        for backup in std::fs::read_dir(&dir)?.flatten() {
            let from = backup.path();
            let stem = from.file_stem().unwrap_or_default().to_os_string();
            for n in 1..1000 {
                let mut name = stem.clone();
                if n > 1 {
                    name.push(format!("-{n}"));
                }
                name.push(".db");
                if publish(&from, &self.dir.join(name))? {
                    break;
                }
            }
        }
        self.step("backup kept")?;
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    /// The copy takes the content of a file, in one transaction: programs that have it open go on.
    fn adopt(&self, file: &Path) -> Result<()> {
        let mut copy = Connection::open(self.copy())?;
        copy.busy_timeout(Duration::from_secs(5))?;
        let from = content::open(file)?;
        let page = |c: &Connection| c.query_row("PRAGMA page_size", [], |r| r.get::<_, i64>(0));
        if content::has_accounts(&copy)? && page(&copy)? != page(&from)? {
            let size = page(&copy)?;
            let _: String = from.query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))?;
            from.pragma_update(None, "page_size", size)?;
            from.execute_batch("VACUUM")?;
        }
        content::copy_into(&from, &mut copy)?;
        from.close().map_err(|(_, e)| e)?;
        Ok(())
    }

    /// Puts what the copy has into a new file beside the shared one. The inner error is of the folder.
    fn keep(&self, st: &mut State) -> Result<std::result::Result<String, String>> {
        let bytes = snapshot(&self.copy(), &self.dir.join("kept.db"))?.0;
        self.beside(st, "conflict", &bytes)
    }

    fn beside(&self, st: &mut State, what: &str, bytes: &[u8]) -> Result<std::result::Result<String, String>> {
        let stem = self.path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let extension = self.path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let when = chrono::Local::now().format("%Y-%m-%d %H-%M-%S");
        for n in 1..1000 {
            let more = if n > 1 { format!(" {n}") } else { String::new() };
            let name = format!("{stem} ({what} {} {when}{more}){extension}", clean_name(&self.home.computer));
            match self.place_new(st, name.as_ref(), bytes)? {
                Ok(true) => return Ok(Ok(name)),
                Ok(false) => {}
                Err(why) => return Ok(Err(why)),
            }
        }
        Ok(Err(tr("the changes of this computer cannot be put beside the data file").into()))
    }

    /// Makes a complete new file in the shared folder. False: the name is taken.
    fn place_new(&self, st: &mut State, name: &OsStr, bytes: &[u8]) -> Result<std::result::Result<bool, String>> {
        let part = match self.part(st, bytes)? {
            Ok(p) => p,
            Err(why) => return Ok(Err(why)),
        };
        self.step("new file written")?;
        let placed = publish(&part, &self.folder().join(name));
        self.step("new file named")?;
        self.sweep(st)?;
        sync_folder(self.folder());
        Ok(placed.map_err(|e| e.to_string()))
    }

    /// Writes bytes into the shared folder under a name of passage.
    fn part(&self, st: &mut State, bytes: &[u8]) -> Result<std::result::Result<PathBuf, String>> {
        self.sweep(st)?;
        let name = format!(".{}.{}-{}.cashcraft-tmp", self.file_name(), self.home.id(), std::process::id());
        let part = self.folder().join(&name);
        st.temporary = Some(name);
        st.save(&self.dir)?;
        let half = bytes.len() / 2;
        let write = || -> Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
                let of = std::fs::metadata(&self.path).map_or(0o600, |m| m.permissions().mode() & 0o777);
                options.mode(of);
            }
            let mut f = options.open(&part)?;
            f.write_all(bytes.get(..half).unwrap_or_default())?;
            self.step("half written")?;
            f.write_all(bytes.get(half..).unwrap_or_default())?;
            f.sync_all()?;
            Ok(())
        };
        match write() {
            Ok(()) => Ok(Ok(part)),
            Err(e) if e.is::<std::io::Error>() => {
                self.sweep(st)?;
                Ok(Err(e.to_string()))
            }
            Err(e) => Err(e),
        }
    }

    fn file_name(&self) -> String {
        self.path.file_name().unwrap_or_default().to_string_lossy().into_owned()
    }

    /// Removes what a write that stopped left in the shared folder.
    fn sweep(&self, st: &mut State) -> Result<()> {
        let Some(name) = &st.temporary else { return Ok(()) };
        let gone = match std::fs::remove_file(self.folder().join(name)) {
            Ok(()) => true,
            Err(e) => e.kind() == ErrorKind::NotFound && self.folder().is_dir(),
        };
        if gone {
            st.temporary = None;
            st.save(&self.dir)?;
        }
        Ok(())
    }

    /// Writes the copy as the new shared file, if that still has the content `expected`, or is still not there.
    fn write(&self, st: &mut State, expected: Option<&str>) -> Result<Written> {
        self.step("start")?;
        let out = self.dir.join("written.db");
        let (bytes, print) = snapshot(&self.copy(), &out)?;
        let permissions = match std::fs::metadata(&self.path) {
            Ok(m) if m.permissions().readonly() => {
                return Ok(Written::Failed(tr("the data file can only be read").into()));
            }
            Ok(m) => Some(m.permissions()),
            Err(_) => None,
        };
        if self.has_log() {
            return Ok(Written::InUse);
        }
        let mut new = Base { len: bytes.len() as u64, modified: None, sha: content::sha256(&bytes), print };
        st.writing = Some(new.clone());
        st.save(&self.dir)?;
        self.step("snapshot")?;
        let Some(expected) = expected else {
            return match self.place_new(st, self.path.file_name().unwrap_or_default(), &bytes)? {
                Ok(true) => {
                    self.step("renamed")?;
                    self.wrote(st, new)
                }
                Ok(false) => {
                    st.writing = None;
                    Ok(Written::Changed)
                }
                Err(why) => {
                    st.writing = None;
                    Ok(Written::Failed(why))
                }
            };
        };
        let part = match self.part(st, &bytes)? {
            Ok(p) => p,
            Err(why) => return Ok(Written::Failed(why)),
        };
        if cfg!(unix)
            && let Some(p) = permissions
        {
            // A file system without permissions.
            let _ = std::fs::set_permissions(&part, p);
        }
        new.modified = std::fs::metadata(&part).ok().and_then(|m| m.modified().ok());
        st.writing = Some(new.clone());
        st.save(&self.dir)?;
        self.step("written")?;
        // Looked at again at the last moment: from here to the new name nobody else should write.
        let answer = match self.read_whole() {
            Ok(now) if now.base.sha == expected && !self.has_log() => None,
            Ok(now) if now.base.sha == expected => Some(Written::InUse),
            Ok(_) => Some(Written::Changed),
            Err(Unread::Missing) => Some(Written::Failed(tr("the data file is not there").into())),
            Err(Unread::Later(why)) => Some(Written::Failed(why)),
        };
        let answer = match answer {
            Some(a) => Some(a),
            None => rename(&part, &self.path).err().map(|e| Written::Failed(e.to_string())),
        };
        if let Some(a) = answer {
            st.writing = None;
            self.sweep(st)?;
            st.save(&self.dir)?;
            return Ok(a);
        }
        self.step("renamed")?;
        sync_folder(self.folder());
        self.wrote(st, new)
    }

    fn wrote(&self, st: &mut State, mut new: Base) -> Result<Written> {
        new.modified = std::fs::metadata(&self.path).ok().and_then(|m| m.modified().ok());
        st.base = Some(new);
        st.writing = None;
        st.temporary = None;
        st.save(&self.dir)?;
        self.step("noted")?;
        Ok(Written::Done)
    }

    /// The shared file when it is complete.
    fn fetch(&self) -> std::result::Result<Fetched, Unread> {
        let mut last = Unread::Missing;
        for n in 0..3 {
            if n > 0 {
                std::thread::sleep(Duration::from_millis(120));
            }
            match self.read_whole() {
                Ok(f) if content::is_whole(&f.bytes) => return Ok(f),
                Ok(_) => last = Unread::Later(tr("the data file is damaged, or has not been downloaded whole").into()),
                Err(Unread::Missing) => return Err(Unread::Missing),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    fn read_whole(&self) -> std::result::Result<Fetched, Unread> {
        let later = |e: std::io::Error| Unread::Later(e.to_string());
        let before = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                // A folder that is away, or a file that iCloud has not downloaded.
                let stub = self.folder().join(format!(".{}.icloud", self.file_name()));
                return Err(if !self.folder().is_dir() {
                    Unread::Later(trf("the folder {path} is not there", &[("path", &self.folder().display())]))
                } else if stub.exists() {
                    Unread::Later(tr("the data file has not been downloaded yet").into())
                } else if std::fs::symlink_metadata(&self.path).is_ok() {
                    Unread::Later(e.to_string())
                } else {
                    Unread::Missing
                });
            }
            Err(e) => return Err(later(e)),
        };
        if !before.is_file() || before.len() > MAX_FILE {
            return Err(Unread::Later(tr("it is not a data file of CashCraft").into()));
        }
        let mut bytes = Vec::new();
        File::open(&self.path).and_then(|f| f.take(MAX_FILE + 1).read_to_end(&mut bytes)).map_err(later)?;
        let after = std::fs::metadata(&self.path).map_err(later)?;
        let same = before.len() == after.len() && before.modified().ok() == after.modified().ok();
        if !same || bytes.len() as u64 != after.len() {
            return Err(Unread::Later(tr("the data file is changing").into()));
        }
        let base = Base {
            len: after.len(),
            modified: after.modified().ok(),
            sha: content::sha256(&bytes),
            print: String::new(),
        };
        Ok(Fetched { base, bytes })
    }

    /// Whether a log lies beside the shared file: a program has it open in place, or had.
    pub fn has_log(&self) -> bool {
        ["-wal", "-journal"].iter().any(|s| std::fs::symlink_metadata(content::log(&self.path, s)).is_ok())
    }

    /// Deals with a log that a program left beside the shared file, when the user says that nobody has it open. The log
    /// is brought into the file only if nothing that is in the file changes or goes; else what it would give is put
    /// beside as another version, and the file stays as it is.
    pub fn recover(&self) -> Result<Recovered> {
        let lock = self.lock_file("sync.lock")?;
        lock.lock()?;
        let mut st = State::load(&self.dir);
        self.sweep(&mut st)?;
        if !self.has_log() {
            return Ok(Recovered::Nothing);
        }
        content::prove_alone(&self.path)
            .with_context(|| tr("the data file is open in a program of this computer, or cannot be locked"))?;
        // Everything is kept before anything is tried.
        let dir = self.new_folder("recovered")?;
        let (before, plain, tried) = (dir.join("before.db"), dir.join("file.db"), dir.join("with-log.db"));
        let Fetched { bytes, .. } = self.fetch().map_err(|e| anyhow!("{}", e.text()))?;
        let logs: Vec<(&str, Vec<u8>)> = ["-wal", "-journal"]
            .into_iter()
            .filter_map(|suffix| match std::fs::read(content::log(&self.path, suffix)) {
                Err(e) if e.kind() == ErrorKind::NotFound => None,
                read => Some(read.map(|b| (suffix, b))),
            })
            .collect::<std::io::Result<_>>()?;
        std::fs::write(&plain, &bytes)?;
        for target in [&before, &tried] {
            std::fs::write(target, &bytes)?;
            for (suffix, log) in &logs {
                std::fs::write(content::log(target, suffix), log)?;
            }
        }
        self.step("log kept")?;
        let file = self.tested(&plain);
        let with = self.tested(&tried);
        if let Err(e) = &file {
            // The file needs its log, and whether this is it cannot be told.
            bail!(trf(
                "the data file is not complete without its log: {error}. Nothing was changed; copies are in {path}",
                &[("error", &format_args!("{e:#}")), ("path", &dir.display())]
            ));
        }
        let owns = content::uses_log(&bytes) && logs.iter().all(|(s, _)| *s == "-wal");
        let sure = owns && with.is_ok() && content::contains(&content::open(&tried)?, &content::open(&plain)?)?;
        let version = match &with {
            Ok(print) if !sure && file.as_ref().ok() != Some(print) => {
                let (bytes, _) = snapshot(&tried, &dir.join("version.db"))?;
                match self.beside(&mut st, "recovered", &bytes)? {
                    Ok(name) => {
                        st.conflicts.push(name.clone());
                        st.save(&self.dir)?;
                        Some(name)
                    }
                    Err(why) => bail!(why),
                }
            }
            _ => None,
        };
        self.step("log tried")?;
        // Nothing may have moved since it was read.
        let now = self.read_whole().map_err(|e| anyhow!("{}", e.text()))?;
        let same = logs.iter().all(|(s, l)| std::fs::read(content::log(&self.path, s)).is_ok_and(|b| b == *l));
        if now.bytes != bytes || !same {
            bail!(tr("the data file is changing"));
        }
        if sure {
            let conn = content::open(&self.path)?;
            conn.busy_timeout(Duration::from_millis(500))?;
            content::settle(conn, &self.path)?;
        } else {
            for (suffix, _) in &logs {
                std::fs::remove_file(content::log(&self.path, suffix))?;
                self.step("log removed")?;
            }
            // What SQLite rebuilds.
            let _ = std::fs::remove_file(content::log(&self.path, "-shm"));
        }
        self.step("log done")?;
        Ok(match version {
            Some(name) => Recovered::Beside(name),
            None => Recovered::Joined,
        })
    }

    /// A folder of its own in the private one.
    fn new_folder(&self, name: &str) -> Result<PathBuf> {
        Home::make(&self.dir)?;
        for n in 1..10_000 {
            let dir = self.dir.join(format!("{name}-{n}"));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(dir),
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
        bail!(tr("there are too many backup copies"))
    }

    /// Whether a file, with the log beside it if there is one, is sound to the last index and to the last entry.
    fn tested(&self, file: &Path) -> Result<String> {
        content::settle(content::open(file)?, file)?;
        let print = self.check(file)?;
        let damaged = || anyhow!(tr("the data file is damaged"));
        let conn = content::open(file)?;
        if conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))? != "ok" {
            return Err(damaged());
        }
        drop(conn);
        // Also the books, whatever the format.
        let scratch = self.home.scratch()?;
        let copy = scratch.0.join("books.db");
        std::fs::copy(file, &copy)?;
        let (db, _) = Db::open(&copy)?;
        if !db.verify_ledger()?.problems.is_empty() {
            return Err(damaged());
        }
        Ok(print)
    }
}

/// What became of a log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recovered {
    /// There is none.
    Nothing,
    /// The file has it, or it held nothing.
    Joined,
    /// What it would give is in this file, beside.
    Beside(String),
}

impl Recovered {
    pub fn text(&self) -> String {
        match self {
            Recovered::Nothing => tr("there is no log beside the data file").into(),
            Recovered::Joined => tr("the log beside the data file was dealt with").into(),
            Recovered::Beside(name) => trf(
                "the log may be of another version of the data file, which was left as it is; what the log would give is in \"{name}\", beside it",
                &[("name", name)],
            ),
        }
    }
}

fn written(w: Written) -> Option<Status> {
    match w {
        Written::Done => Some(Status::Saved),
        Written::Changed => None,
        Written::InUse => Some(Status::InUse),
        Written::Failed(why) => Some(Status::Waiting(why)),
    }
}

/// A database as one complete file: its bytes, and the fingerprint of what they hold.
fn snapshot(from: &Path, out: &Path) -> Result<(Vec<u8>, String)> {
    let source = content::open(from)?;
    content::remove_with_logs(out);
    let mut copy = Connection::open(out)?;
    content::copy_into(&source, &mut copy)?;
    let print = content::fingerprint(&copy)?;
    content::settle(copy, out)?;
    let bytes = std::fs::read(out)?;
    if !content::is_whole(&bytes) {
        bail!(tr("the data file is damaged"));
    }
    Ok((bytes, print))
}

/// How the data stands when an operation is about to run.
#[derive(Clone, Debug)]
pub struct Turn {
    /// It took what another computer wrote just now.
    pub adopted: bool,
    pub revision: String,
}

impl Turn {
    /// Refuses what was decided on another content: `read` is the revision that was last read, if any. What changes
    /// something does not run just after the content changed; what names things by their ids runs only on the
    /// revision they were read from.
    pub fn check(&self, reads: bool, ids: bool, read: Option<&str>) -> Result<()> {
        if (ids && read != Some(self.revision.as_str())) || (!reads && self.adopted) {
            bail!(trf(
                "nothing was done: the data was changed on another computer, and ids may be of other things. Read it again (revision {revision})",
                &[("revision", &self.revision)]
            ));
        }
        Ok(())
    }
}

/// Runs an operation on a shared file: what others wrote comes in before, what it changed goes out after. Also gives
/// what people should know. The operation asks `Turn::check` whether it may run.
pub fn around<T>(shared: &Shared, f: impl FnOnce(&Turn) -> T) -> Result<(T, Vec<String>)> {
    let before = shared.sync()?;
    if before.status == Status::Held {
        bail!("{}", before.status.text());
    }
    let hold = shared.hold()?;
    // Under the hold: what another program of this computer took meanwhile counts too.
    let turn = Turn { adopted: before.adopted || before.revision != shared.revision(), revision: shared.revision() };
    let out = f(&turn);
    drop(hold);
    let mut notices: Vec<String> = before.conflict_notice().into_iter().collect();
    match shared.sync() {
        Ok(after) => notices.extend(after.notices()),
        Err(e) => notices.push(trf("Error: {error}", &[("error", &format_args!("{e:#}"))])),
    }
    Ok((out, notices))
}

/// A data file put in a folder and opened there. While it lives nobody can write in the old one.
pub struct Kept {
    pub opened: Opened,
    pub path: PathBuf,
    _source: Connection,
}

impl Kept {
    /// Lets others write in the old file again.
    pub fn release(self) -> Opened {
        self.opened
    }
}

/// Puts a complete copy of a data file, or of a working copy (`from`), into a folder, under a name that must be free,
/// opens it there and remembers it. Nobody writes in the old one (`old`) meanwhile, and programs that still have it
/// are told. Whatever the folder is, the new file is used through a private copy. On failure nothing is remembered.
pub fn keep(home: &Home, old: &Path, from: &Path, target: &Path) -> Result<Kept> {
    keep_through(Shared::new(home, target), old, from)
}

fn keep_through(shared: Shared, old: &Path, from: &Path) -> Result<Kept> {
    let (home, target) = (shared.home.clone(), shared.path.clone());
    let there = || anyhow!(trf("{path} is there already", &[("path", &target.display())]));
    if std::fs::symlink_metadata(&target).is_ok() || copy_print(&shared.copy())?.is_some() {
        return Err(there());
    }
    let source = content::open(from)?;
    source.execute_batch("BEGIN IMMEDIATE")?;
    let scratch = home.scratch()?;
    let (bytes, print) = snapshot(from, &scratch.0.join("out.db"))?;
    let lock = shared.lock_file("sync.lock")?;
    lock.lock()?;
    let mut st = State::load(&shared.dir);
    match shared.place_new(&mut st, target.file_name().unwrap_or_default(), &bytes)? {
        Ok(true) => {}
        Ok(false) => return Err(there()),
        Err(why) => bail!(trf("{path} cannot be written: {error}", &[("path", &target.display()), ("error", &why)])),
    }
    drop(lock);
    let finish = || -> Result<Opened> {
        if !shared.read_whole().is_ok_and(|f| f.bytes == bytes) {
            bail!(trf("{path} was not written whole", &[("path", &target.display())]));
        }
        shared.step("placed")?;
        home.set_choice(&target, Use::Shared)?;
        let opened = open(&home, &target, Use::Shared, crate::i18n::lang(), true)?;
        if content::fingerprint(&opened.db.conn)? != print {
            bail!(trf("{path} was not written whole", &[("path", &target.display())]));
        }
        shared.step("opened")?;
        home.remember(Some(&target))?;
        let mark = home.copy_of(&resolved(old));
        Home::make(&mark)?;
        // For people: a path with other characters is shown as it can be.
        replace(&mark.join("moved"), target.display().to_string().as_bytes())?;
        Ok(opened)
    };
    match finish() {
        Ok(opened) => Ok(Kept { opened, path: target, _source: source }),
        Err(e) => {
            // Nothing of it stays, if the file is still the one that was put there. Not after a kill, in the tests.
            if e.to_string() != KILLED && shared.read_whole().is_ok_and(|f| f.bytes == bytes) {
                let _ = std::fs::remove_file(&target);
                content::remove_with_logs(&shared.copy());
                for name in ["state.json", "mode"] {
                    let _ = std::fs::remove_file(shared.dir.join(name));
                }
            }
            Err(e)
        }
    }
}

/// The entries that only a file used directly, or only another one, has.
pub fn differences_of(home: &Home, path: &Path, other: &Path) -> Result<Differences> {
    let scratch = home.scratch()?;
    let here = scratch.0.join("here.db");
    snapshot(path, &here)?;
    differences(home, &here, other)
}

/// Removes a data file that was used directly, once another has all it holds. Fails, and leaves it, if a program has
/// it open or it changed.
pub fn retire(old: &Path, new: &Path) -> Result<()> {
    let conn = content::open(old)?;
    conn.busy_timeout(Duration::from_millis(500))?;
    let alone: String = conn.query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))?;
    conn.execute_batch("BEGIN EXCLUSIVE")?;
    let same = copy_print(new)? == Some(content::fingerprint_in(&conn)?);
    conn.execute_batch("COMMIT")?;
    if alone != "delete" || !same {
        bail!(tr("the old data file was changed or is open in another program: it was left where it is"));
    }
    conn.close().map_err(|(_, e)| e)?;
    std::fs::remove_file(old)?;
    Ok(())
}
