//! `data`: where the data file is and how it is used. Not operations of `ops`: they are about the file, and work also
//! when it cannot be opened.

use anyhow::{Result, anyhow, bail};
use cashcraft_engine::i18n::{lang, tr, trf};
use cashcraft_engine::money::fmt_date;
use cashcraft_engine::store::{self, Home, Presence, Shared, Use};
use clap::Subcommand;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Subcommand, Debug)]
pub enum DataCmd {
    /// Where the data file is, how it is used and whether everything is saved in it
    Status,
    /// Remember how to use this data file: auto, direct (where it lies) or shared (through a private copy)
    Mode { mode: String },
    /// Entries that only the data file, or only another one, has
    Compare { other: PathBuf },
    /// Copy the data file into a folder, for instance a synced one, and use that from now on. The old file stays
    Keep {
        folder: PathBuf,
        /// Name of the new file
        #[arg(long)]
        name: Option<String>,
        /// Remove the old file, once the new one is verified
        #[arg(long = "move")]
        moving: bool,
    },
    /// Open this data file when none is named, from now on; without a file, the default one
    Use { file: Option<PathBuf> },
    /// Write a shared data file again where it was removed
    PutBack,
    /// Take away a log that a program left beside a shared data file, and put what it holds in a file beside
    Recover {
        /// No program has the file open, on any computer
        #[arg(long)]
        nobody_has_it_open: bool,
    },
    /// Speak no more of the conflict files; they stay where they are
    Dismiss,
}

pub fn run(home: &Home, path: &Path, asked: Use, cmd: DataCmd, as_json: bool) -> Result<()> {
    let (is_shared, service) = store::way(home, path, asked);
    let shared = || Shared::new(home, path);
    let only_shared = || {
        if is_shared { Ok(shared()) } else { Err(anyhow!(tr("the data file is used directly"))) }
    };
    match cmd {
        DataCmd::Status => {
            let s = is_shared.then(shared);
            // A file that is not there is not made by asking.
            let report = match &s {
                Some(s) => Some(s.sync()?),
                None => None,
            };
            let now = chrono::Utc::now().timestamp();
            let others = s.as_ref().map(|s| Presence::new(home, s.path()).others(now, Instant::now()));
            let names = |v: Vec<PathBuf>| v.iter().map(|p| p.display().to_string()).collect::<Vec<_>>();
            let unsaved: Vec<_> = home.unsaved();
            if as_json {
                let unsaved: Vec<_> =
                    unsaved.iter().map(|(f, c)| json!({"file": f, "copy": c.display().to_string()})).collect();
                let v = json!({
                    "file": store::resolved(path).display().to_string(),
                    "exists": path.exists(),
                    "mode": if is_shared { "shared" } else { "direct" },
                    "service": service.map(|s| s.label()),
                    "copy": s.as_ref().map(|s| s.copy().display().to_string()),
                    "saved": report.as_ref().map(|r| !r.pending),
                    "status": report.as_ref().map(|r| r.status.text()),
                    "conflict": report.as_ref().and_then(|r| r.conflict.clone()),
                    "conflicts": s.as_ref().map(|s| names(s.conflicts())),
                    "versions": s.as_ref().map(|s| names(s.versions())),
                    "open_on": others,
                    "unsaved": unsaved,
                });
                println!("{}", serde_json::to_string_pretty(&v)?);
                return Ok(());
            }
            println!("{}", trf("Data file: {path}", &[("path", &store::resolved(path).display())]));
            match (&s, service) {
                (None, None) => println!("{}", tr("Used directly")),
                (None, Some(k)) => println!("{}", trf("Used directly, in {service}", &[("service", &k.label())])),
                (Some(_), None) => println!("{}", tr("Used through a private copy")),
                (Some(_), Some(k)) => {
                    println!("{}", trf("Used through a private copy, in {service}", &[("service", &k.label())]));
                }
            }
            if let (Some(s), Some(r)) = (&s, &report) {
                println!("{}", trf("Private copy: {path}", &[("path", &s.copy().display())]));
                println!("{}", r.status.text());
                for n in r.notices() {
                    println!("! {n}");
                }
                for p in s.conflicts() {
                    println!("! {}", trf("Conflict file: {path}", &[("path", &p.display())]));
                }
            }
            for name in others.into_iter().flatten() {
                println!("! {}", trf("Also open on {computer}", &[("computer", &name)]));
            }
            for (file, copy) in unsaved {
                println!(
                    "! {}",
                    trf(
                        "Changes not written to {file} yet are in {copy}",
                        &[("file", &file), ("copy", &copy.display())]
                    )
                );
            }
        }
        DataCmd::Mode { mode } => {
            let choice = Use::from_code(&mode)
                .ok_or_else(|| anyhow!(trf("unknown mode \"{mode}\": auto, direct or shared", &[("mode", &mode)])))?;
            home.set_choice(path, choice)?;
            println!("{}", tr("Saved"));
        }
        DataCmd::Compare { other } => {
            let d = if is_shared {
                let s = shared();
                s.sync()?;
                s.differences(&other)?
            } else {
                store::differences_of(home, path, &other)?
            };
            if as_json {
                println!("{}", serde_json::to_string_pretty(&d)?);
                return Ok(());
            }
            let lists = [(tr("Only in the data file"), &d.first), (tr("Only in the other file"), &d.second)];
            for (title, lines) in lists.into_iter().filter(|(_, l)| !l.is_empty()) {
                println!("{title}");
                for l in lines {
                    println!("  {}  {}  {}  {}", fmt_date(l.date), l.payee, l.memo, l.amounts());
                }
            }
            if d.first.is_empty() && d.second.is_empty() {
                println!(
                    "{}",
                    if d.others {
                        tr("The entries are the same; something else is different, such as names, budgets or rules.")
                    } else {
                        tr("The two files hold the same.")
                    }
                );
            }
        }
        DataCmd::Keep { folder, name, moving } => {
            let opened = store::open(home, path, asked, lang(), true)?;
            let name = name.map(PathBuf::from).or_else(|| path.file_name().map(PathBuf::from)).unwrap_or_default();
            if name.file_name() != Some(name.as_os_str()) {
                bail!(trf("the path \"{path}\" has no file name", &[("path", &name.display())]));
            }
            let target = folder.join(name);
            let from = opened.db.path.clone().unwrap_or_else(|| path.to_path_buf());
            let direct = opened.shared.is_none();
            let kept = store::keep(home, path, &from, &target)?;
            println!("{}", trf("The data file is now {path}", &[("path", &kept.path.display())]));
            let new = kept.release();
            drop(opened);
            if moving
                && direct
                && let Some(copy) = &new.db.path
            {
                store::retire(path, copy)?;
            } else {
                println!("{}", trf("The old one stays: {path}", &[("path", &path.display())]));
            }
        }
        DataCmd::Recover { nobody_has_it_open } if !nobody_has_it_open => {
            bail!(tr("close the data file in every program, on every computer, and repeat with --nobody-has-it-open"))
        }
        DataCmd::Recover { .. } => println!("{}", only_shared()?.recover()?.text()),
        DataCmd::Use { file } => {
            if let Some(f) = &file
                && !f.exists()
            {
                bail!(trf("{path} is not there", &[("path", &f.display())]));
            }
            home.remember(file.map(|f| store::resolved(&f)).as_deref())?;
            println!("{}", tr("Saved"));
        }
        DataCmd::PutBack => {
            let r = only_shared()?.put_back()?;
            println!("{}", r.status.text());
        }
        DataCmd::Dismiss => {
            only_shared()?.dismiss_conflicts()?;
            println!("{}", tr("Saved"));
        }
    }
    Ok(())
}
