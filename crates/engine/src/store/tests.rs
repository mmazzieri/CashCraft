use super::*;
use crate::db::{AccountKind, NewTx};
use crate::ledger::tests::assert_sound;
use crate::testing::Seeded;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// A folder of a sync app and the private folders of some computers.
struct Place(PathBuf);
impl Drop for Place {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
impl Place {
    fn new() -> Place {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cashcraft-store-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        // The real path: the temporary folder is behind a link on macOS and has a short name on Windows.
        Place(resolved(&dir))
    }
    fn home(&self, computer: &str) -> Home {
        let name: String = computer.chars().filter(|c| c.is_alphanumeric()).collect();
        Home { dir: self.0.join(name), computer: computer.into() }
    }
    fn folder(&self) -> PathBuf {
        self.0.join("folder")
    }
    fn file(&self) -> PathBuf {
        self.folder().join("books.db")
    }
    fn names(&self) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(self.folder())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        out.sort();
        out
    }
    fn conflicts(&self) -> Vec<PathBuf> {
        self.names().iter().filter(|n| n.contains("(conflict ")).map(|n| self.folder().join(n)).collect()
    }
    fn start(&self, computer: &str) -> (Db, Shared, Report) {
        let o = open(&self.home(computer), &self.file(), Use::Shared, Lang::En, false).unwrap();
        let (shared, report) = o.shared.unwrap();
        (o.db, shared, report)
    }
}

fn add(db: &Db, payee: &str) {
    let account_id = match db.find_account("A") {
        Ok(a) => a.id,
        Err(_) => db.add_account("A", AccountKind::Bank, "EUR", 0, "").unwrap(),
    };
    let date = chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
    db.add_transaction(&NewTx { account_id, date, amount: -100, payee: payee.into(), ..Default::default() }).unwrap();
}

fn payees(db: &Db) -> BTreeSet<String> {
    db.transactions(&Default::default()).unwrap().into_iter().map(|t| t.payee).collect()
}

/// What a file holds, read as another program would: all its bytes, and nothing beside.
fn payees_of(path: &Path) -> BTreeSet<String> {
    let dir = Place::new();
    let copy = dir.0.join("x.db");
    let bytes = std::fs::read(path).unwrap();
    assert!(content::is_whole(&bytes), "{}", path.display());
    assert!(bytes[18] == 1, "{}", path.display());
    std::fs::write(&copy, bytes).unwrap();
    assert!(content::is_sound(&content::open(&copy).unwrap()).unwrap());
    let (db, _) = Db::open(&copy).unwrap();
    assert_sound(&db, "shared");
    payees(&db)
}

fn set(of: &[&str]) -> BTreeSet<String> {
    of.iter().map(|s| s.to_string()).collect()
}

#[test]
fn what_one_computer_writes_the_other_sees() {
    let p = Place::new();
    let (a, sa, report) = p.start("one");
    assert_eq!((report.status, report.pending), (Status::Saved, false));
    add(&a, "x");
    assert!(sa.sync().unwrap().notices().is_empty());
    assert_eq!(payees_of(&p.file()), set(&["x"]));
    let (b, sb, _) = p.start("two");
    assert_eq!(payees(&b), set(&["x"]));
    for i in 0..4 {
        add(&b, &format!("b{i}"));
        sb.sync().unwrap();
        // The other has it open: its connection sees the change.
        let before = a.data_version();
        assert!(sa.looks_changed());
        assert!(sa.sync().unwrap().adopted);
        assert!(!sa.looks_changed() && a.data_version() != before);
        add(&a, &format!("a{i}"));
        sa.sync().unwrap();
        assert!(sb.sync().unwrap().adopted);
        assert_eq!(payees(&a), payees(&b));
    }
    assert_eq!(payees_of(&p.file()).len(), 9);
    assert_eq!(p.names(), ["books.db"]);
    assert_sound(&a, "shared");
}

/// A change of the envelopes alone, which touches no posting, reaches the shared file and the other computer.
#[test]
fn an_assignment_alone_reaches_the_other_computer() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    let e = a.add_envelope("Fun", &[], crate::envelopes::Overspending::Carry, a.envelopes_start()).unwrap();
    sa.sync().unwrap();
    let (b, sb, _) = p.start("two");
    let month = crate::db::first_of_month(crate::db::today());
    a.assign(e, month, 1_234, "").unwrap();
    assert!(!sa.sync().unwrap().pending);
    assert!(sb.sync().unwrap().adopted);
    let assigned = |db: &Db| db.envelope_month(month).unwrap().envelopes[0].assigned;
    assert_eq!((assigned(&a), assigned(&b)), (1_234, 1_234));
}

#[test]
fn nothing_is_written_when_nothing_changed() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    add(&a, "x");
    sa.sync().unwrap();
    let bytes = std::fs::read(p.file()).unwrap();
    for _ in 0..3 {
        payees(&a);
        sa.sync().unwrap();
        let (b, sb, _) = p.start("two");
        payees(&b);
        sb.sync().unwrap();
        assert_eq!(std::fs::read(p.file()).unwrap(), bytes);
    }
    // A change that is taken back is none.
    a.set_meta("q", "1").unwrap();
    a.conn.execute("DELETE FROM meta WHERE key='q'", []).unwrap();
    assert!(!sa.sync().unwrap().pending);
    assert_eq!(std::fs::read(p.file()).unwrap(), bytes);
}

/// Two computers that change the envelopes at once: neither is lost, and the comparison lists what differs.
#[test]
fn envelope_changes_of_two_computers_are_both_kept_and_listed() {
    use crate::envelopes::Overspending;
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    let month = crate::db::first_of_month(crate::db::today());
    let e = a.add_envelope("Fun", &[], Overspending::Carry, month).unwrap();
    sa.sync().unwrap();
    let (b, sb, _) = p.start("two");
    a.assign(e, month, 1_000, "one").unwrap();
    b.assign(e, month, 2_500, "two").unwrap();
    b.add_envelope("Food", &[], Overspending::Carry, month).unwrap();
    sb.sync().unwrap();
    let report = sa.sync().unwrap();
    let name = report.conflict.clone().unwrap();
    let assigned = |db: &Db| db.envelope_month(month).unwrap().envelopes.iter().map(|l| l.assigned).sum::<i64>();
    assert_eq!(assigned(&a), 2_500);
    let d = sa.differences(&p.folder().join(&name)).unwrap();
    assert!(d.first.is_empty() && d.second.is_empty() && !d.others, "{d:?}");
    let key = crate::envelopes::month_key(month);
    assert_eq!(d.envelopes_first.len(), 1, "{d:?}");
    assert!(d.envelopes_first[0].starts_with(&key) && d.envelopes_first[0].contains("25"), "{d:?}");
    assert_eq!(d.envelopes_second.len(), 1, "{d:?}");
    assert!(d.envelopes_second[0].contains("10"), "{d:?}");
    // The same file twice: nothing listed.
    let same = sa.differences(&p.file()).unwrap();
    assert!(same.none_listed() && !same.others);
}

#[test]
fn when_both_change_both_are_kept() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    add(&a, "common");
    sa.sync().unwrap();
    let (b, sb, _) = p.start("two/..:");
    add(&a, "mine");
    add(&b, "theirs");
    sb.sync().unwrap();
    let report = sa.sync().unwrap();
    let name = report.conflict.clone().unwrap();
    assert!(name.starts_with("books (conflict one ") && name.ends_with(").db"), "{name}");
    assert!(report.notices()[0].contains(&name));
    assert_eq!(payees(&a), set(&["common", "theirs"]));
    assert_eq!(payees_of(&p.file()), set(&["common", "theirs"]));
    assert_eq!(payees_of(&p.folder().join(&name)), set(&["common", "mine"]));
    assert_eq!(p.names().len(), 2);
    assert_eq!((sa.conflicts(), sa.versions()), (p.conflicts(), p.conflicts()));
    let d = sa.differences(&p.conflicts()[0]).unwrap();
    let one = |l: &[Line]| l.iter().map(|l| l.payee.clone()).collect::<Vec<_>>();
    assert_eq!((one(&d.first), one(&d.second), d.others), (vec!["theirs".into()], vec!["mine".into()], false));
    assert!(d.second[0].amounts().contains("A -1"));
    assert_eq!(p.names().len(), 2);
    sa.dismiss_conflicts().unwrap();
    assert!(sa.conflicts().is_empty() && sa.versions().len() == 1);
    // The conflict file can be opened like any other.
    let o = open(&p.home("one"), &p.conflicts()[0], Use::Shared, Lang::En, true).unwrap();
    assert_eq!(payees(&o.db), set(&["common", "mine"]));
}

#[test]
fn names_of_conflict_files_never_replace_anything() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    let (b, sb, _) = p.start("two");
    let mut seen = BTreeSet::new();
    for i in 0..4 {
        add(&a, &format!("a{i}"));
        add(&b, &format!("b{i}"));
        sb.sync().unwrap();
        // Within a second most have the same time in their name.
        assert!(seen.insert(sa.sync().unwrap().conflict.unwrap()));
        sb.sync().unwrap();
    }
    assert_eq!(p.conflicts().len(), 4);
    let all: BTreeSet<String> = p.conflicts().iter().flat_map(|c| payees_of(c)).collect();
    assert!((0..4).all(|i| all.contains(&format!("a{i}"))));
}

#[test]
fn the_same_content_is_no_conflict() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    add(&a, "x");
    // Another computer has the same bytes already, as when the state was lost.
    sa.sync().unwrap();
    std::fs::remove_file(sa.dir.join("state.json")).unwrap();
    let report = sa.sync().unwrap();
    assert_eq!((report.conflict, report.adopted, report.status), (None, false, Status::Saved));
    // Lost with changes: they are kept.
    std::fs::write(sa.dir.join("state.json"), "{").unwrap();
    add(&a, "y");
    assert!(sa.sync().unwrap().conflict.is_some());
    assert_eq!(payees_of(&p.conflicts()[0]), set(&["x", "y"]));
}

/// The program dies at the `n`-th step.
fn die_at(shared: &Shared, n: usize) -> (Shared, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let faults: Faults = Arc::new(move |step| {
        // A read is no change.
        if step != "read" && c.fetch_add(1, Ordering::Relaxed) == n {
            bail!("killed");
        }
        Ok(())
    });
    (Shared { faults: Some(faults), ..shared.clone() }, count)
}

/// Whether the program died.
fn dies(shared: &Shared, n: usize) -> bool {
    match die_at(shared, n).0.sync() {
        Ok(_) => false,
        Err(e) => {
            assert_eq!(e.to_string(), "killed");
            true
        }
    }
}

#[test]
fn killed_at_any_step_the_file_is_the_old_or_the_new_one() {
    let mut steps = 0;
    for n in 0.. {
        let p = Place::new();
        let (a, sa, _) = p.start("one");
        add(&a, "old");
        sa.sync().unwrap();
        add(&a, "new");
        let died = dies(&sa, n);
        let there = payees_of(&p.file());
        assert!(there == set(&["old"]) || there == set(&["old", "new"]), "{n}");
        // The phone would not take what was left for a data file.
        assert!(p.names().iter().all(|f| f == "books.db" || f.ends_with(".cashcraft-tmp")));
        drop(a);
        let (a, _, report) = p.start("one");
        assert_eq!((payees(&a), report.conflict), (set(&["old", "new"]), None));
        assert_eq!(payees_of(&p.file()), set(&["old", "new"]));
        assert_eq!(p.names(), ["books.db"]);
        if !died {
            break;
        }
        steps += 1;
    }
    assert!(steps >= 6, "{steps}");
}

#[test]
fn killed_while_a_new_file_is_made_or_two_meet() {
    for n in 0.. {
        let p = Place::new();
        // New.
        let shared = Shared::new(&p.home("one"), &p.file());
        Home::make(&shared.dir).unwrap();
        let (a, _) = Db::open(&shared.copy()).unwrap();
        add(&a, "x");
        let died = dies(&shared, n);
        assert!(!p.file().exists() || payees_of(&p.file()) == set(&["x"]));
        let (_, _, report) = p.start("one");
        assert_eq!((report.conflict, payees_of(&p.file()), p.names()), (None, set(&["x"]), vec!["books.db".into()]));
        if !died {
            break;
        }
    }
    let mut steps = 0;
    for n in 0.. {
        // Both changed.
        let p = Place::new();
        let (a, sa, _) = p.start("one");
        let (b, sb, _) = p.start("two");
        add(&a, "mine");
        add(&b, "theirs");
        sb.sync().unwrap();
        let died = dies(&sa, n);
        let kept = |p: &Place| p.conflicts().iter().any(|c| payees_of(c) == set(&["mine"]));
        assert!(kept(&p) || payees(&a) == set(&["mine"]), "{n}");
        // Afterwards more is written, before the next look.
        if payees(&a) == set(&["mine"]) {
            add(&a, "more");
        }
        sa.sync().unwrap();
        let all: BTreeSet<String> = p.conflicts().iter().flat_map(|c| payees_of(c)).collect();
        assert!(all.contains("mine") && (payees(&a) == set(&["theirs"]) || all.contains("more")), "{n}");
        assert_eq!(payees_of(&p.file()), set(&["theirs"]));
        assert!(p.names().iter().all(|f| f.ends_with(".db")), "{:?}", p.names());
        if !died {
            break;
        }
        steps += 1;
    }
    assert!(steps >= 6, "{steps}");
}

#[test]
fn a_change_between_two_looks_is_not_overwritten() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    let (b, sb, _) = p.start("two");
    add(&a, "mine");
    add(&b, "theirs");
    // The other computer's file arrives while ours is being prepared.
    let done = Arc::new(AtomicUsize::new(0));
    let faults: Faults = Arc::new(move |step| {
        if step == "written" && done.fetch_add(1, Ordering::Relaxed) == 0 {
            sb.sync()?;
        }
        Ok(())
    });
    let report = Shared { faults: Some(faults), ..sa }.sync().unwrap();
    assert!(report.conflict.is_some());
    assert_eq!(payees_of(&p.file()), set(&["theirs"]));
    assert_eq!(payees_of(&p.conflicts()[0]), set(&["mine"]));
    assert_eq!(p.names().len(), 2);
}

#[test]
fn what_is_not_whole_is_neither_read_nor_replaced() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    add(&a, "x");
    sa.sync().unwrap();
    let good = std::fs::read(p.file()).unwrap();
    let mut damaged = good.clone();
    for b in damaged.iter_mut().skip(5000).take(9000) {
        *b = 0x55;
    }
    let mut newer = good.clone();
    newer[60..64].copy_from_slice(&99u32.to_be_bytes());
    let cases: [(&str, Vec<u8>); 8] = [
        ("empty", vec![]),
        ("short", good[..good.len() / 2].to_vec()),
        ("half a page", good[..good.len() - 100].to_vec()),
        ("one page", good[..4096].to_vec()),
        ("zeros", vec![0; good.len()]),
        ("text", b"placeholder of the cloud".to_vec()),
        ("damaged", damaged),
        ("newer", newer),
    ];
    add(&a, "y");
    for (what, bytes) in &cases {
        std::fs::write(p.file(), bytes).unwrap();
        let report = sa.sync().unwrap();
        assert!(matches!(report.status, Status::Waiting(_)) && report.pending, "{what} {report:?}");
        assert_eq!(&std::fs::read(p.file()).unwrap(), bytes, "{what}");
        assert_eq!(payees(&a), set(&["x", "y"]), "{what}");
        assert!(report.notices()[0].contains("safe on this computer"));
        // Nor is it opened by a computer that has nothing.
        let home = p.home(&format!("new {what}"));
        assert!(open(&home, &p.file(), Use::Shared, Lang::En, false).is_err(), "{what}");
        assert_eq!(&std::fs::read(p.file()).unwrap(), bytes, "{what}");
    }
    std::fs::write(p.file(), good).unwrap();
    assert_eq!(sa.sync().unwrap().status, Status::Saved);
    assert_eq!((payees_of(&p.file()), p.names()), (set(&["x", "y"]), vec!["books.db".into()]));
}

#[test]
fn a_file_or_a_folder_that_goes_away_is_waited_for() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    add(&a, "x");
    sa.sync().unwrap();
    add(&a, "y");
    let aside = p.0.join("aside");
    std::fs::rename(p.folder(), &aside).unwrap();
    let report = sa.sync().unwrap();
    assert!(matches!(&report.status, Status::Waiting(w) if w.contains("folder")) && report.pending);
    assert!(!p.folder().exists());
    assert_eq!(p.home("one").unsaved().len(), 1);
    // The program goes on without it.
    let (again, _, report) = p.start("one");
    assert!(report.pending && payees(&again) == set(&["x", "y"]));
    std::fs::rename(&aside, p.folder()).unwrap();
    assert_eq!(sa.sync().unwrap().status, Status::Saved);
    assert_eq!(payees_of(&p.file()), set(&["x", "y"]));
    assert!(p.home("one").unsaved().is_empty());
    // The file alone: it is made again only if the user says so.
    assert!(sa.put_back().is_err());
    std::fs::remove_file(p.file()).unwrap();
    assert!(matches!(sa.sync().unwrap().status, Status::Waiting(_)) && !p.file().exists());
    assert_eq!(sa.put_back().unwrap().status, Status::Saved);
    assert_eq!(payees_of(&p.file()), set(&["x", "y"]));
    // Not downloaded: nothing is made in its place.
    std::fs::rename(p.file(), p.folder().join(".books.db.icloud")).unwrap();
    assert!(open(&p.home("two"), &p.file(), Use::Shared, Lang::En, false).is_err());
    assert!(matches!(sa.sync().unwrap().status, Status::Waiting(_)) && !p.file().exists());
    // A file that must be there.
    let absent = p.folder().join("other.db");
    for how in [Use::Shared, Use::Direct] {
        assert!(open(&p.home("two"), &absent, how, Lang::En, true).is_err());
    }
    assert!(!absent.exists());
}

#[test]
fn a_file_that_can_only_be_read_is_not_replaced() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    let before = std::fs::read(p.file()).unwrap();
    let writable = std::fs::metadata(p.file()).unwrap().permissions();
    let mut only = writable.clone();
    only.set_readonly(true);
    std::fs::set_permissions(p.file(), only).unwrap();
    add(&a, "x");
    let report = sa.sync().unwrap();
    assert!(matches!(report.status, Status::Waiting(_)) && report.pending);
    assert_eq!(std::fs::read(p.file()).unwrap(), before);
    std::fs::set_permissions(p.file(), writable).unwrap();
    assert!(!sa.sync().unwrap().pending);
}

#[cfg(unix)]
#[test]
fn a_folder_that_can_only_be_read_is_waited_for_and_permissions_stay() {
    use std::os::unix::fs::PermissionsExt;
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    std::fs::set_permissions(p.file(), std::fs::Permissions::from_mode(0o640)).unwrap();
    std::fs::set_permissions(p.folder(), std::fs::Permissions::from_mode(0o500)).unwrap();
    let probe = std::fs::write(p.folder().join("probe"), "").is_ok();
    add(&a, "x");
    let report = sa.sync().unwrap();
    std::fs::set_permissions(p.folder(), std::fs::Permissions::from_mode(0o700)).unwrap();
    // Root writes anywhere.
    assert!(probe || (matches!(report.status, Status::Waiting(_)) && report.pending));
    sa.sync().unwrap();
    assert_eq!(std::fs::metadata(p.file()).unwrap().permissions().mode() & 0o777, 0o640);
    assert_eq!(std::fs::metadata(&p.home("one").dir).unwrap().permissions().mode() & 0o077, 0);
}

#[cfg(unix)]
#[test]
fn a_link_to_the_file_is_followed_and_names_that_are_not_utf8_work() {
    use std::os::unix::ffi::OsStrExt;
    let p = Place::new();
    // macOS has no names that are not UTF-8.
    let bytes: &[u8] = if cfg!(target_os = "macos") { b"conti.db" } else { b"conti \xe8.db" };
    let name = std::ffi::OsStr::from_bytes(bytes);
    let odd = p.folder().join(name);
    let home = p.home("one");
    let o = open(&home, &odd, Use::Shared, Lang::En, false).unwrap();
    add(&o.db, "x");
    let s = o.shared.unwrap().0;
    s.sync().unwrap();
    assert_eq!(payees_of(&odd), set(&["x"]));
    home.remember(Some(&odd)).unwrap();
    assert_eq!(home.recent(), Some(odd.clone()));
    home.remember(None).unwrap();
    home.remember(None).unwrap();
    assert_eq!(home.recent(), None);
    let link = p.0.join("link.db");
    std::os::unix::fs::symlink(&odd, &link).unwrap();
    let through = Shared::new(&home, &link);
    assert_eq!((through.path(), through.copy()), (s.path(), s.copy()));
    add(&o.db, "y");
    through.sync().unwrap();
    assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    assert_eq!(payees_of(&odd), set(&["x", "y"]));
}

/// What 0.3.0 leaves when it is killed: a file with its log.
fn left_open(file: &Path) -> Db {
    let (db, _) = Db::open(file).unwrap();
    add(&db, "in the file");
    db.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).unwrap();
    add(&db, "in the log");
    db
}

/// A shared file with the log of a program that was killed.
fn with_log(p: &Place) -> (PathBuf, Db) {
    let live = p.0.join("live.db");
    let db = left_open(&live);
    std::fs::copy(&live, p.file()).unwrap();
    let wal = content::log(&p.file(), "-wal");
    std::fs::copy(content::log(&live, "-wal"), &wal).unwrap();
    (wal, db)
}

fn both(p: &Place) -> Vec<(String, Vec<u8>)> {
    p.names()
        .into_iter()
        .filter(|n| n.starts_with("books.db"))
        .map(|n| (n.clone(), std::fs::read(p.folder().join(n)).unwrap()))
        .collect()
}

#[test]
fn a_log_is_in_use_until_the_user_says_otherwise() {
    let p = Place::new();
    let (wal, _live) = with_log(&p);
    // However old it is: a program elsewhere may be idle.
    let old = SystemTime::now() - Duration::from_secs(90 * 24 * 3600);
    for f in [&wal, &p.file()] {
        OpenOptions::new().write(true).open(f).unwrap().set_modified(old).unwrap();
    }
    let home = p.home("one");
    let start = || open(&home, &p.file(), Use::Shared, Lang::En, false);
    let before = both(&p);
    assert!(start().is_err() && both(&p) == before);
    let shared = Shared::new(&home, &p.file());
    let Recovered::Beside(name) = shared.recover().unwrap() else { panic!("the same") };
    assert!(name.starts_with("books (recovered one "), "{name}");
    // The file is byte for byte what it was.
    assert_eq!(std::fs::read(p.file()).unwrap(), before[0].1);
    assert_eq!(payees_of(&p.folder().join(&name)), set(&["in the file", "in the log"]));
    assert_eq!(p.names(), [name.as_str(), "books.db"]);
    assert_eq!(shared.recover().unwrap(), Recovered::Nothing);
    for (name, bytes) in &before {
        let kept = shared.dir.join("recovered-1").join(name.replace("books.db", "before.db"));
        assert_eq!(&std::fs::read(kept).unwrap(), bytes);
    }
    let o = start().unwrap();
    assert_eq!(payees(&o.db), set(&["in the file"]));
    // A log that appears later: changes wait.
    let old = std::fs::read(p.file()).unwrap();
    std::fs::write(content::log(&p.file(), "-journal"), "").unwrap();
    add(&o.db, "later");
    let shared = o.shared.unwrap().0;
    let report = shared.sync().unwrap();
    assert_eq!((report.status, report.pending), (Status::InUse, true));
    assert_eq!(std::fs::read(p.file()).unwrap(), old);
    assert_eq!(shared.recover().unwrap(), Recovered::Same);
    assert_eq!(shared.sync().unwrap().status, Status::Saved);
    assert_eq!(payees_of(&p.file()), set(&["in the file", "later"]));
}

/// The operating system is asked, and SQLite is not opened. Elsewhere a second program is needed: see the command line.
#[cfg(target_os = "linux")]
#[test]
fn a_program_of_this_computer_is_seen_by_its_locks() {
    let p = Place::new();
    let (_, _live) = with_log(&p);
    let shared = Shared::new(&p.home("one"), &p.file());
    let reader = content::open(&p.file()).unwrap();
    reader.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true).unwrap();
    reader.execute_batch("BEGIN; SELECT * FROM meta;").unwrap();
    let before = std::fs::read(p.file()).unwrap();
    // On Linux the locks of this very process are listed. Elsewhere they are seen by others too.
    assert!(content::in_use(&p.file()).unwrap());
    assert!(shared.recover().is_err());
    assert_eq!(std::fs::read(p.file()).unwrap(), before);
    drop(reader);
    assert!(!content::in_use(&p.file()).unwrap());
    assert!(!content::in_use(&p.folder().join("absent.db")).unwrap());
}

/// A journal as a program that dies in the middle of a change leaves it, and the file it had half written.
fn hot_journal(file: &Path, out: &Path) -> (Vec<u8>, Vec<u8>) {
    std::fs::copy(file, out).unwrap();
    let c = content::open(out).unwrap();
    c.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA cache_size=1; PRAGMA cache_spill=1; BEGIN IMMEDIATE;").unwrap();
    c.execute_batch("UPDATE entries SET payee=payee||'x'; CREATE TABLE zz(a);").unwrap();
    for _ in 0..400 {
        c.execute("INSERT INTO zz VALUES(zeroblob(4000))", []).unwrap();
    }
    let seen = (std::fs::read(out).unwrap(), std::fs::read(content::log(out, "-journal")).unwrap());
    c.execute_batch("ROLLBACK").unwrap();
    seen
}

/// Whatever lies beside the file, no command opens it by SQLite: it would roll back a journal, or bring a log in.
#[test]
fn nothing_opens_the_shared_file_in_place() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    add(&a, "first");
    sa.sync().unwrap();
    let (_, journal) = hot_journal(&p.file(), &p.0.join("work.db"));
    for payee in ["later 1", "later 2"] {
        add(&a, payee);
        sa.sync().unwrap();
    }
    // A journal of an older file.
    std::fs::write(content::log(&p.file(), "-journal"), &journal).unwrap();
    let before = both(&p);
    let home = p.home("two");
    assert!(open(&home, &p.file(), Use::Shared, Lang::En, false).is_err());
    add(&a, "waits");
    let there = sa.sync().unwrap();
    assert_eq!(there.status, Status::InUse);
    assert!(sa.put_back().is_err());
    let _ = sa.differences(&p.file()).unwrap();
    let _ = sa.allow_direct(&p.file());
    let _ = home.unsaved();
    assert_eq!(both(&p), before);
    let two = Shared::new(&home, &p.file());
    let done = two.recover().unwrap();
    assert_eq!(std::fs::read(p.file()).unwrap(), before[0].1, "{done:?}");
    assert_ne!(done, Recovered::Same);
    assert_eq!(std::fs::read(two.dir.join("recovered-1").join("before.db-journal")).unwrap(), journal);
    assert_eq!(payees_of(&p.file()), set(&["first", "later 1", "later 2"]));
    let sources = [
        include_str!("../store.rs"),
        include_str!("content.rs"),
        include_str!("compare.rs"),
        include_str!("presence.rs"),
    ];
    for s in sources {
        for opening in ["content::open(", "Connection::open(", "Db::open(", "snapshot("] {
            for path in ["&self.path", "self.path()", "shared.path()", "&shared.path", "&target", "other)", "second)"] {
                assert!(!s.contains(&format!("{opening}{path}")), "{opening}{path}");
            }
        }
    }
}

#[test]
fn a_file_that_is_torn_is_left_with_its_journal() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    for i in 0..300 {
        add(&a, &format!("p{i}"));
    }
    sa.sync().unwrap();
    drop(a);
    let (torn, journal) = hot_journal(&p.file(), &p.0.join("work.db"));
    std::fs::write(p.file(), &torn).unwrap();
    std::fs::write(content::log(&p.file(), "-journal"), &journal).unwrap();
    let before = both(&p);
    let two = Shared::new(&p.home("two"), &p.file());
    // Not complete without its journal, or not seen to be: nothing is lost either way.
    let done = two.recover();
    assert_eq!(std::fs::read(p.file()).unwrap(), torn);
    let beside = two.versions();
    assert_eq!(beside.len(), 1, "{done:?}");
    assert_eq!(payees_of(&beside[0]).len(), 300);
    if done.is_err() {
        assert_eq!(both(&p), before);
    }
}

#[test]
fn killed_while_a_log_is_dealt_with_nothing_is_lost() {
    let mut steps = 0;
    for n in 0.. {
        let p = Place::new();
        let (wal, _live) = with_log(&p);
        let file = std::fs::read(p.file()).unwrap();
        let shared = Shared::new(&p.home("one"), &p.file());
        let died = die_at(&shared, n).0.recover().is_err();
        assert_eq!(std::fs::read(p.file()).unwrap(), file);
        assert!(wal.exists() || shared.dir.join("recovered-1").join("before.db-wal").exists());
        shared.recover().unwrap();
        assert_eq!(std::fs::read(p.file()).unwrap(), file);
        assert!(shared.versions().iter().any(|v| payees_of(v).contains("in the log")), "{n}");
        assert!(p.names().iter().all(|f| f.ends_with(".db")), "{:?}", p.names());
        if !died {
            break;
        }
        steps += 1;
    }
    assert!(steps >= 6, "{steps}");
}

#[test]
fn the_same_content_in_other_bytes_keeps_what_is_to_be_written() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    add(&a, "x");
    sa.sync().unwrap();
    add(&a, "pending");
    // A phone, or a checkpoint, writes the same again.
    let c = content::open(&p.file()).unwrap();
    c.execute_batch("PRAGMA page_size=8192; VACUUM;").unwrap();
    drop(c);
    let report = sa.sync().unwrap();
    assert_eq!((report.conflict, report.adopted, report.pending, report.status), (None, false, false, Status::Saved));
    assert_eq!(payees_of(&p.file()), set(&["x", "pending"]));
    assert_eq!(p.names(), ["books.db"]);
}

#[test]
fn putting_back_forgets_nothing_when_it_fails() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    add(&a, "x");
    sa.sync().unwrap();
    assert!(!sa.is_missing() && sa.put_back().is_err());
    let aside = p.0.join("aside");
    std::fs::rename(p.folder(), &aside).unwrap();
    // The folder is away: the file may be fine, and others may be changing it.
    assert!(!sa.is_missing() && sa.put_back().is_err());
    let (b, sb, _) = {
        std::fs::rename(&aside, p.folder()).unwrap();
        p.start("two")
    };
    add(&b, "theirs");
    sb.sync().unwrap();
    let report = sa.sync().unwrap();
    assert_eq!((report.conflict, report.adopted), (None, true));
    // In the cloud, and not here.
    let stub = p.folder().join(".books.db.icloud");
    std::fs::rename(p.file(), &stub).unwrap();
    assert!(!sa.is_missing() && sa.put_back().is_err() && !p.file().exists());
    std::fs::remove_file(stub).unwrap();
    assert!(sa.is_missing());
    let mut steps = 0;
    for n in 0.. {
        let died = die_at(&sa, n).0.put_back().is_err();
        assert!(State::load(&sa.dir).base.is_some());
        if !died {
            break;
        }
        steps += 1;
        let _ = std::fs::remove_file(p.file());
        sa.sync().unwrap();
        let _ = std::fs::remove_file(p.file());
    }
    assert!(steps >= 4 && payees_of(&p.file()) == set(&["x", "theirs"]), "{steps}");
}

/// The tables whose ids are seen.
const WITH_IDS: [&str; 9] = [
    "accounts",
    "categories",
    "ledger_accounts",
    "entries",
    "journal_postings",
    "scheduled_items",
    "transaction_rules",
    "rule_runs",
    "csv_layouts",
];

fn ids(db: &Db, table: &str) -> Vec<i64> {
    let mut st = db.conn.prepare(&format!("SELECT id FROM {table} ORDER BY id")).unwrap();
    st.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
}

#[test]
fn every_new_row_of_a_shared_file_has_an_id_of_its_own() {
    let p = Place::new();
    let (db, _, _) = p.start("one");
    let seeded: Vec<(&str, Vec<i64>)> = WITH_IDS.iter().map(|t| (*t, ids(&db, t))).collect();
    let call = |tool: &str, a: serde_json::Value| crate::ops::call(&db, tool, &a, None).unwrap();
    let json = |v: &str| serde_json::from_str::<serde_json::Value>(v).unwrap();
    call("create_account", json(r#"{"name":"Bank","opening_balance":10}"#));
    call("create_account", json(r#"{"name":"Card","kind":"credit_card"}"#));
    call("create_category", json(r#"{"path":"Sub:New"}"#));
    call("create_ledger_account", json(r#"{"name":"Equipment","class":"asset"}"#));
    call("create_ledger_account", json(r#"{"name":"Rents","class":"income"}"#));
    call("add_transaction", json(r#"{"account":"Bank","amount":-5,"payee":"shop zoo"}"#));
    let moved = call("add_transfer", json(r#"{"from":"Bank","to":"Card","amount":7}"#));
    let side = db.transactions(&Default::default()).unwrap().iter().find(|t| t.transfer_peer.is_some()).unwrap().id;
    call("separate_transfer", serde_json::json!({"id": side}));
    call("add_scheduled", json(r#"{"account":"Bank","amount":-5,"payee":"rent","next_date":"2026-01-01"}"#));
    call("post_scheduled", json(r#"{"until":"2026-03-01"}"#));
    call("add_rule", json(r#"{"keywords":"zoo","category":"Leisure"}"#));
    let seen = call("preview_rules", json(r#"{"scope":"all","overwrite":true}"#));
    call("apply_rules", serde_json::json!({"scope": "all", "overwrite": true, "token": seen["token"]}));
    let csv = p.0.join("a.csv");
    std::fs::write(&csv, "Data;Descrizione;Importo\n05/01/2026;one;-1,20\n06/01/2026;two;-1,30\n").unwrap();
    call("import_csv", serde_json::json!({"path": csv, "account": "Bank"}));
    db.record_card_statement(db.find_account("Card").unwrap().id, crate::db::today(), crate::db::today(), 5, "k")
        .unwrap();
    // The opening balance and the two lines of the file.
    let balance = crate::money::fmt_machine(1_000 - 120 - 130);
    let r = call(
        "start_reconciliation",
        serde_json::json!({"account": "Bank", "path": csv, "date": "2026-01-31", "balance": balance}),
    );
    let id = r["reconciliation"]["id"].clone();
    let pairs: Vec<String> =
        r["lines"].as_array().unwrap().iter().map(|l| format!("{}={}", l["line"], l["exact"]["id"])).collect();
    call("match_reconciliation", serde_json::json!({"id": id, "pairs": pairs.join(",")}));
    call("finish_reconciliation", serde_json::json!({"id": id}));
    call("add_envelope", json(r#"{"name":"Fun","categories":"Leisure"}"#));
    call("assign_envelope", json(r#"{"envelope":"Fun","amount":5}"#));
    assert!(!moved.is_null());
    let reconciled =
        ["reconciliations", "reconciliation_lines", "reconciled_postings", "envelopes", "envelope_changes"]
            .map(|t| (t, vec![]));
    for (table, before) in
        seeded.iter().map(|(t, b)| (*t, b.clone())).chain([("card_statements", vec![])]).chain(reconciled)
    {
        let all = ids(&db, table);
        let new: Vec<i64> = all.iter().copied().filter(|i| !before.contains(i)).collect();
        assert!(!new.is_empty(), "{table}");
        assert!(new.iter().all(|i| *i > 1_000_000_000_000 && *i < 1 << 53), "{table} {new:?}");
    }
    // But for the three that every file has.
    assert!(
        seeded
            .iter()
            .all(|(t, b)| b.iter().filter(|i| **i < 1_000_000_000_000).count()
                <= 3 * usize::from(*t == "ledger_accounts"))
    );
    assert_sound(&db, "ids");
    // Every INSERT of these tables names its id; triggers and migrations are not of a shared copy.
    let sources = [
        include_str!("../db.rs"),
        include_str!("../ledger.rs"),
        include_str!("../rules.rs"),
        include_str!("../cards.rs"),
        include_str!("../layouts.rs"),
        include_str!("../transfers.rs"),
        include_str!("../importer.rs"),
        include_str!("../duplicates.rs"),
        include_str!("../reconcile.rs"),
        include_str!("../envelopes.rs"),
    ];
    let mut found = 0;
    for table in WITH_IDS.iter().chain(&[
        "card_statements",
        "reconciliations",
        "reconciliation_lines",
        "reconciled_postings",
        "envelopes",
        "envelope_changes",
    ]) {
        for s in sources {
            // Not the triggers.
            for (at, _) in s.match_indices(&format!("\"INSERT INTO {table}(")) {
                found += 1;
                let columns = s[at..].split_once('(').unwrap().1.split_once(')').unwrap().0;
                assert!(columns.split(',').any(|c| c.trim() == "id"), "{table}: {columns}");
            }
        }
    }
    assert!(found >= 12, "{found}");
}

#[test]
fn an_id_of_before_is_of_the_same_row_or_of_none() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    let (b, sb, _) = p.start("two");
    add(&a, "common");
    sa.sync().unwrap();
    sb.sync().unwrap();
    // Both add at once, from the same file.
    let last = |db: &Db| db.transactions(&Default::default()).unwrap().into_iter().max_by_key(|t| t.id).unwrap();
    let common = last(&b).id;
    add(&a, "a");
    add(&b, "b");
    let (of_a, of_b) = (last(&a), last(&b));
    assert!(of_a.id != of_b.id && of_a.entry_id != of_b.entry_id && of_a.id > common);
    sa.sync().unwrap();
    assert!(sb.sync().unwrap().conflict.is_some());
    let error = b.delete_transaction(of_b.id).unwrap_err();
    assert!(error.to_string().contains("not found"), "{error}");
    assert_eq!(b.transaction(common).unwrap().payee, "common");
    // One that is deleted elsewhere is not given again.
    a.delete_transaction(of_a.id).unwrap();
    add(&a, "rent");
    sa.sync().unwrap();
    sb.sync().unwrap();
    assert!(b.transaction(of_a.id).is_err());
    // Nor those of accounts, categories and their rows in the chart.
    for (db, name) in [(&a, "X"), (&b, "Y")] {
        db.add_account(name, AccountKind::Bank, "EUR", 0, "").unwrap();
        db.category_by_path(name, false, true).unwrap();
    }
    for table in ["accounts", "categories", "ledger_accounts"] {
        let (of_a, of_b) = (ids(&a, table), ids(&b, table));
        assert!(of_a.last() != of_b.last() && of_a.len() == of_b.len(), "{table}");
    }
    let ((), notices) = around(&sb, || add(&b, "goes on")).unwrap();
    assert!(notices.is_empty(), "{notices:?}");
}

#[test]
fn ids_grow_and_stay_exact_whatever_the_clock_and_the_file() {
    let p = Place::new();
    let (db, _, _) = p.start("one");
    let mut last = 0;
    for i in 0..200 {
        add(&db, &format!("{i}"));
        let id = *ids(&db, "journal_postings").last().unwrap();
        assert!(id > last);
        last = id;
    }
    let order: Vec<String> = db.transactions(&Default::default()).unwrap().into_iter().map(|t| t.payee).collect();
    assert!(
        order.is_sorted_by_key(|p| p.parse::<i32>().unwrap())
            || order.iter().rev().is_sorted_by_key(|p| p.parse::<i32>().unwrap())
    );
    for far in [9_000_000_000_000_000, i64::MAX - 1] {
        db.conn.execute("INSERT INTO entries(id,date) VALUES(?1,'2026-01-01')", [far]).unwrap();
        let id = db.new_id("entries").unwrap().unwrap();
        assert!(
            id > 0
                && id < 1 << 53
                && db.conn.query_row("SELECT COUNT(*) FROM entries WHERE id=?1", [id], |r| r.get::<_, i64>(0)).unwrap()
                    == 0
        );
        db.conn.execute("DELETE FROM entries WHERE id=?1", [far]).unwrap();
    }
    // A file used directly has the numbers it always had.
    let (direct, _) = Db::open(&p.0.join("direct.db")).unwrap();
    add(&direct, "x");
    assert!(ids(&direct, "journal_postings") == [1, 2] && direct.new_id("entries").unwrap().is_none());
}

#[test]
fn nothing_is_read_while_nothing_changes() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    add(&a, "x");
    let report = sa.sync().unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let r = reads.clone();
    let faults: Faults = Arc::new(move |s| {
        r.fetch_add(usize::from(s == "read"), Ordering::Relaxed);
        Ok(())
    });
    let watched = Shared { faults: Some(faults), ..sa.clone() };
    for _ in 0..5 {
        assert!(!watched.wants_sync(&report) && !watched.is_missing());
        let _ = (watched.conflicts(), watched.versions());
    }
    assert_eq!(reads.load(Ordering::Relaxed), 0);
    let (b, sb, _) = p.start("two");
    add(&b, "y");
    sb.sync().unwrap();
    assert!(watched.wants_sync(&report));
    watched.sync().unwrap();
    assert!(reads.load(Ordering::Relaxed) > 0);
    std::fs::write(content::log(&p.file(), "-wal"), "").unwrap();
    assert!(watched.wants_sync(&report));
}

#[test]
fn a_working_copy_that_cannot_be_read_makes_place() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    add(&a, "x");
    sa.sync().unwrap();
    drop(a);
    for (n, bytes) in [b"garbagegarbage".to_vec(), vec![7; 8192]].into_iter().enumerate() {
        content::remove_with_logs(&sa.copy());
        std::fs::write(sa.copy(), &bytes).unwrap();
        let o = open(&p.home("one"), &p.file(), Use::Shared, Lang::En, true).unwrap();
        assert_eq!(payees(&o.db), set(&["x"]));
        let aside = o.shared.unwrap().1.set_aside.unwrap();
        assert!(aside.parent().unwrap().ends_with(format!("unreadable-{}", n + 1)));
        assert_eq!(std::fs::read(aside).unwrap(), bytes);
        assert_eq!(p.names(), ["books.db"]);
    }
    // Neither does it stop a direct use.
    std::fs::write(sa.copy(), "garbage").unwrap();
    content::remove_with_logs(&sa.dir.join("none"));
    assert!(open(&p.home("one"), &p.file(), Use::Direct, Lang::En, true).is_ok());
}

#[test]
fn copies_that_are_saved_are_not_read_again() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    add(&a, "x");
    sa.sync().unwrap();
    let home = p.home("one");
    drop(a);
    assert!(home.unsaved().is_empty());
    // The same length and time, other bytes: nobody looks.
    let m = std::fs::metadata(sa.copy()).unwrap();
    std::fs::write(sa.copy(), vec![7; m.len() as usize]).unwrap();
    OpenOptions::new().write(true).open(sa.copy()).unwrap().set_modified(m.modified().unwrap()).unwrap();
    assert!(home.unsaved().is_empty());
    OpenOptions::new().write(true).open(sa.copy()).unwrap().set_modified(SystemTime::UNIX_EPOCH).unwrap();
    assert_eq!(home.unsaved().len(), 1);
}

#[cfg(unix)]
#[test]
fn a_folder_that_goes_away_keeps_its_path() {
    let p = Place::new();
    // As `/var` on macOS: a part of the path is a link.
    let link = p.0.join("link");
    std::os::unix::fs::symlink(p.0.join("."), &link).unwrap();
    let through = link.join("folder").join("books.db");
    let there = Shared::new(&p.home("one"), &through);
    assert_eq!(there.path(), Shared::new(&p.home("one"), &p.file()).path());
    std::fs::rename(p.folder(), p.0.join("away")).unwrap();
    assert_eq!(Shared::new(&p.home("one"), &through).copy(), there.copy());
}

#[test]
fn a_file_that_was_shared_stays_shared_and_is_never_made_empty() {
    let p = Place::new();
    let home = p.home("one");
    std::fs::write(p.folder().join(".stfolder"), "").unwrap();
    let o = open(&home, &p.file(), Use::Auto, Lang::En, false).unwrap();
    add(&o.db, "x");
    o.shared.unwrap().0.sync().unwrap();
    drop(o.db);
    // The mark of the service goes away, or a variable does.
    std::fs::remove_file(p.folder().join(".stfolder")).unwrap();
    assert!(open(&home, &p.file(), Use::Auto, Lang::En, false).unwrap().shared.is_some());
    // Nothing empty takes its place, in any way.
    let aside = p.0.join("aside.db");
    std::fs::rename(p.file(), &aside).unwrap();
    assert!(open(&home, &p.file(), Use::Direct, Lang::En, false).is_err() && !p.file().exists());
    let o = open(&home, &p.file(), Use::Auto, Lang::En, false).unwrap();
    assert!(payees(&o.db) == set(&["x"]) && !p.file().exists());
    drop(o);
    std::fs::rename(&aside, p.file()).unwrap();
    // Without its state, changes in the copy still stop a direct use.
    let shared = Shared::new(&home, &p.file());
    std::fs::remove_file(shared.dir.join("state.json")).unwrap();
    assert!(open(&home, &p.file(), Use::Direct, Lang::En, false).is_ok());
    let (db, _) = Db::open(&shared.copy()).unwrap();
    add(&db, "y");
    drop(db);
    assert!(open(&home, &p.file(), Use::Direct, Lang::En, false).is_err());
    // One that is remembered is not made again.
    let other = p.folder().join("other.db");
    drop(open(&home, &other, Use::Direct, Lang::En, false).unwrap());
    home.remember(Some(&other)).unwrap();
    std::fs::remove_file(&other).unwrap();
    assert!(open(&home, &other, Use::Direct, Lang::En, false).is_err() && !other.exists());
}

#[test]
fn the_first_time_and_other_names_are_said() {
    let p = Place::new();
    let (a, _, _) = p.start("one");
    drop(a);
    let o = open(&p.home("two"), &p.file(), Use::Shared, Lang::En, false).unwrap();
    assert!(o.notices.iter().any(|n| n.contains("private copy")), "{:?}", o.notices);
    let o = open(&p.home("two"), &p.file(), Use::Shared, Lang::En, false).unwrap();
    assert!(o.notices.is_empty(), "{:?}", o.notices);
    #[cfg(unix)]
    {
        std::fs::hard_link(p.file(), p.0.join("second.db")).unwrap();
        let o = open(&p.home("two"), &p.file(), Use::Shared, Lang::En, false).unwrap();
        assert!(o.notices.iter().any(|n| n.contains("hard links")));
        // A link to nothing is not a new file.
        let link = p.folder().join("link.db");
        std::os::unix::fs::symlink(p.0.join("gone.db"), &link).unwrap();
        for how in [Use::Direct, Use::Shared] {
            assert!(open(&p.home("two"), &link, how, Lang::En, false).is_err());
        }
        assert!(!p.0.join("gone.db").exists());
    }
    // A folder that was moved while changes waited.
    add(&o.db, "waiting");
    std::fs::rename(p.folder(), p.0.join("moved")).unwrap();
    let there = p.0.join("moved").join("books.db");
    let o = open(&p.home("two"), &there, Use::Shared, Lang::En, true).unwrap();
    assert!(o.notices.iter().any(|n| n.contains("not written")), "{:?}", o.notices);
}

#[test]
fn a_private_folder_that_is_synced_is_refused() {
    let p = Place::new();
    let home = Home { dir: p.0.join("Dropbox").join("CashCraft"), computer: "one".into() };
    std::fs::create_dir_all(&home.dir).unwrap();
    let Err(e) = open(&home, &p.file(), Use::Shared, Lang::En, false) else { panic!("opened") };
    assert!(e.to_string().contains("CASHCRAFT_PRIVATE"), "{e}");
    assert!(!p.file().exists());
    // By itself it is used as it always was, and that is said.
    let own = home.dir.join("cashcraft.db");
    let o = open(&home, &own, Use::Auto, Lang::En, false).unwrap();
    assert!(o.shared.is_none() && o.notices.iter().any(|n| n.contains("CASHCRAFT_PRIVATE")), "{:?}", o.notices);
    // Nor is the file of a private folder shared, whatever the path looks like.
    assert!(!way(&home, &own, Use::Auto).0);
    let beside = p.0.join("Dropbox").join("other.db");
    assert!(!way(&home, &beside, Use::Auto).0 && way(&p.home("two"), &beside, Use::Auto).0);
}

#[test]
fn names_are_those_of_the_disk() {
    let names = || ["Books.DB", "È.db", "x"].into_iter().map(std::ffi::OsString::from);
    for (asked, real) in [("books.db", "Books.DB"), ("è.db", "È.db"), ("x", "x"), ("new.db", "new.db")] {
        assert_eq!(real_name(asked.as_ref(), names()), real);
    }
    let both = || ["a.db", "A.db"].into_iter().map(std::ffi::OsString::from);
    assert_eq!(real_name("A.db".as_ref(), both()), "A.db");
    let p = Place::new();
    assert_eq!(real_names(&p.file()), p.file());
}

#[cfg(unix)]
#[test]
fn nobody_else_reads_what_is_being_written() {
    use std::os::unix::fs::PermissionsExt;
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    let mode = |f: &Path| std::fs::metadata(f).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&p.file()), 0o600);
    std::fs::set_permissions(p.file(), std::fs::Permissions::from_mode(0o400 | 0o200)).unwrap();
    add(&a, "x");
    let folder = p.folder();
    let seen = Arc::new(AtomicUsize::new(0));
    let s = seen.clone();
    let faults: Faults = Arc::new(move |step| {
        if step == "half written" {
            for f in std::fs::read_dir(&folder).unwrap().flatten().filter(|f| f.path().extension().unwrap() != "db") {
                assert_eq!(f.metadata().unwrap().permissions().mode() & 0o777, 0o600);
                s.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(())
    });
    Shared { faults: Some(faults), ..sa.clone() }.sync().unwrap();
    assert_eq!(seen.load(Ordering::Relaxed), 1);
    let (b, sb, _) = p.start("two");
    add(&b, "y");
    sb.sync().unwrap();
    add(&a, "z");
    sa.sync().unwrap();
    assert_eq!(mode(&p.conflicts()[0]), 0o600);
}

#[test]
fn keeping_a_file_elsewhere_fails_whole_or_not_at_all() {
    let mut steps = 0;
    for n in 0.. {
        let p = Place::new();
        let home = p.home("one");
        let old = p.0.join("old.db");
        let (db, _) = Db::open(&old).unwrap();
        add(&db, "x");
        let kept = keep_through(die_at(&Shared::new(&home, &p.file()), n).0, &old, &old);
        let died = kept.and_then(|k| k.finish()).is_err();
        // The old one goes on, and is the one in use until all is done.
        add(&db, "y");
        assert_eq!(home.recent().is_some(), !died);
        assert_eq!(home.moved(&old).is_some(), !died);
        if p.file().exists() {
            assert_eq!(payees_of(&p.file()), set(&["x"]));
        }
        if !died {
            break;
        }
        steps += 1;
    }
    assert!(steps >= 5, "{steps}");
    // However slow the folder is, the old file can be written; what is written comes too.
    let p = Place::new();
    let home = p.home("one");
    let old = p.0.join("old.db");
    let (db, _) = Db::open(&old).unwrap();
    add(&db, "x");
    db.conn.busy_timeout(Duration::from_millis(20)).unwrap();
    let count = AtomicUsize::new(0);
    let faults: Faults = {
        let old = old.clone();
        Arc::new(move |step| {
            if step == "finishing" {
                return Ok(());
            }
            let (other, _) = Db::open(&old)?;
            other.conn.busy_timeout(Duration::from_millis(20))?;
            add(&other, &format!("during {step} {}", count.fetch_add(1, Ordering::Relaxed)));
            Ok(())
        })
    };
    let slow = Shared { faults: Some(faults), ..Shared::new(&home, &p.file()) };
    let kept = keep_through(slow, &old, &old).unwrap();
    add(&db, "before the end");
    // Somebody writes just then: said, and not remembered.
    db.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(kept.finish().is_err() && home.recent().is_none() && home.moved(&old).is_none());
    db.conn.execute_batch("ROLLBACK").unwrap();
    kept.finish().unwrap();
    assert!(payees(&db).len() > 4);
    assert_eq!(payees(&kept.opened.db), payees(&db));
    assert!(!kept.opened.shared.as_ref().unwrap().0.sync().unwrap().pending);
    assert_eq!(payees_of(&p.file()), payees(&db));
    assert_eq!(home.recent(), Some(resolved(&p.file())));
    assert!(keep(&home, &old, &old, &p.file()).is_err());
    // Given up: nothing of it stays.
    home.remember(Some(&old)).unwrap();
    let other = p.folder().join("other.db");
    keep(&home, &old, &old, &other).unwrap().discard();
    assert!(!other.exists() && !Shared::new(&home, &other).copy().exists());
    assert_eq!((home.recent(), home.moved(&old)), (Some(old.clone()), None));
    // A folder that is not there.
    assert!(keep(&home, &old, &old, &p.0.join("nowhere").join("a.db")).is_err());
    assert_eq!(home.recent(), Some(old));
}

#[test]
fn going_back_to_the_default_file_ends_the_notice() {
    let p = Place::new();
    let home = p.home("one");
    // The mark alone: the default file of whoever runs the tests is not touched.
    let default = crate::db::default_db_path();
    let mark = home.copy_of(&resolved(&default));
    Home::make(&mark).unwrap();
    std::fs::write(mark.join("moved"), "elsewhere").unwrap();
    assert!(home.moved(&default).is_some());
    home.remember(None).unwrap();
    assert_eq!(home.moved(&default), None);
}

#[test]
fn what_was_taken_is_known_also_when_the_folder_is_away() {
    for log in [false, true] {
        let p = Place::new();
        let (a, sa, _) = p.start("one");
        let (b, sb, _) = p.start("two");
        add(&b, "theirs");
        sb.sync().unwrap();
        // Dies once the copy has it.
        let dying: Faults = Arc::new(|step| if step == "adopted" { bail!("killed") } else { Ok(()) });
        assert!(Shared { faults: Some(dying), ..sa.clone() }.sync().is_err());
        assert_eq!(payees(&a), set(&["theirs"]));
        let (aside, journal) = (p.0.join("aside"), content::log(&p.file(), "-journal"));
        if log {
            std::fs::write(&journal, "").unwrap();
        } else {
            std::fs::rename(p.folder(), &aside).unwrap();
        }
        let (again, sa, report) = p.start("one");
        assert!(report.status != Status::Saved && !report.pending);
        add(&again, "mine");
        assert!(sa.sync().unwrap().pending);
        if log {
            std::fs::remove_file(journal).unwrap();
        } else {
            std::fs::rename(aside, p.folder()).unwrap();
        }
        assert_eq!(sa.sync().unwrap().conflict, None);
        assert_eq!((payees_of(&p.file()), p.names()), (set(&["theirs", "mine"]), vec!["books.db".into()]));
    }
}

#[test]
fn killed_while_the_format_is_updated() {
    let mut steps = 0;
    for n in 0.. {
        let p = Place::new();
        let c = Connection::open(p.file()).unwrap();
        c.execute_batch(crate::migrate::tests::V1_SCHEMA).unwrap();
        c.execute_batch(
            "INSERT INTO accounts(name, kind) VALUES ('A', 'bank');
             INSERT INTO transactions(account_id, date, amount, payee) VALUES (1, '2026-01-02', -500, 'old');
             PRAGMA user_version = 1;",
        )
        .unwrap();
        drop(c);
        let old = std::fs::read(p.file()).unwrap();
        let shared = Shared::new(&p.home("one"), &p.file());
        let died = dies(&shared, n);
        assert!(std::fs::read(p.file()).unwrap() == old || payees_of(&p.file()) == set(&["old"]), "{n}");
        let (db, _, report) = p.start("one");
        assert_eq!((payees(&db), report.conflict, p.names()), (set(&["old"]), None, vec!["books.db".into()]));
        assert!(shared.dir.join("books.v1-backup.db").exists());
        if !died {
            break;
        }
        steps += 1;
    }
    assert!(steps >= 8, "{steps}");
}

#[test]
fn a_file_of_an_older_format_is_updated_with_a_private_backup() {
    let p = Place::new();
    let c = Connection::open(p.file()).unwrap();
    c.execute_batch(crate::migrate::tests::V1_SCHEMA).unwrap();
    c.execute_batch(
        "INSERT INTO accounts(name, kind) VALUES ('A', 'bank');
         INSERT INTO transactions(account_id, date, amount, payee) VALUES (1, '2026-01-02', -500, 'old');
         PRAGMA user_version = 1;",
    )
    .unwrap();
    drop(c);
    let old = std::fs::read(p.file()).unwrap();
    let home = p.home("one");
    let o = open(&home, &p.file(), Use::Shared, Lang::En, false).unwrap();
    let (shared, report) = o.shared.unwrap();
    assert_eq!((report.status, report.conflict), (Status::Saved, None));
    assert_eq!(payees(&o.db), set(&["old"]));
    assert_eq!(payees_of(&p.file()), set(&["old"]));
    assert_eq!(p.names(), ["books.db"]);
    // The backup holds the old tables.
    let backup = content::open(&shared.dir.join("books.v1-backup.db")).unwrap();
    assert_eq!(content::format(&backup).unwrap(), 1);
    // Another program that still has the copy open takes a second old file as well.
    std::fs::write(p.file(), old).unwrap();
    assert!(shared.sync().unwrap().adopted);
    assert_eq!(payees(&o.db), set(&["old"]));
    assert!(shared.dir.join("books.v1-backup-2.db").exists());
    assert_sound(&o.db, "shared");
}

#[test]
fn programs_of_one_computer_share_the_copy() {
    let p = Place::new();
    let (window, sw, _) = p.start("one");
    let (other, so, _) = p.start("one");
    assert_eq!(window.path, other.path);
    add(&window, "w");
    let ((), notices) = around(&so, || add(&other, "o")).unwrap();
    assert!(notices.is_empty());
    assert_eq!(payees_of(&p.file()), set(&["w", "o"]));
    // All at once.
    std::thread::scope(|s| {
        for t in 0..4 {
            let p = &p;
            s.spawn(move || {
                let (db, shared, _) = p.start("one");
                for i in 0..5 {
                    let ((), notices) = around(&shared, || add(&db, &format!("{t}-{i}"))).unwrap();
                    assert!(notices.is_empty(), "{notices:?}");
                }
            });
        }
    });
    sw.sync().unwrap();
    assert_eq!(payees_of(&p.file()).len(), 22);
    assert_eq!(p.names(), ["books.db"]);
    // A form that is open keeps what others write out, and lets changes go in.
    let (b, sb, _) = p.start("two");
    add(&b, "b");
    sb.sync().unwrap();
    let hold = sw.hold().unwrap();
    assert_eq!(sw.sync().unwrap().status, Status::Held);
    assert!(around(&so, || add(&other, "refused")).is_err());
    assert!(!payees(&window).contains("b") && !payees(&window).contains("refused"));
    drop(hold);
    assert!(sw.sync().unwrap().adopted);
    assert!(payees(&window).contains("b"));
    let _hold = sw.hold().unwrap();
    add(&window, "saved with a form open");
    assert_eq!(sw.sync().unwrap().status, Status::Saved);
    assert!(payees_of(&p.file()).contains("saved with a form open"));
}

#[test]
fn a_file_with_other_pages_is_taken_into_a_copy_that_is_open() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    add(&a, "x");
    sa.sync().unwrap();
    let c = content::open(&p.file()).unwrap();
    c.execute_batch("PRAGMA page_size=1024; VACUUM; INSERT INTO meta VALUES('elsewhere','1');").unwrap();
    drop(c);
    assert!(sa.sync().unwrap().adopted);
    assert_eq!(a.meta("elsewhere").as_deref(), Some("1"));
    add(&a, "y");
    sa.sync().unwrap();
    assert_eq!(payees_of(&p.file()), set(&["x", "y"]));
}

#[test]
fn the_old_file_goes_only_when_it_is_safe() {
    let p = Place::new();
    let old = p.0.join("old.db");
    let home = p.home("one");
    let o = open(&home, &old, Use::Auto, Lang::En, false).unwrap();
    assert!(o.shared.is_none());
    add(&o.db, "x");
    let new = {
        let k = keep(&home, &old, &old, &p.file()).unwrap();
        k.finish().unwrap();
        k.opened
    };
    assert_eq!((payees_of(&p.file()), p.names()), (set(&["x"]), vec!["books.db".into()]));
    assert!(new.shared.is_some() && way(&home, &p.file(), Use::Auto).0);
    // Open elsewhere, or changed since.
    o.db.conn.execute_batch("BEGIN; SELECT * FROM meta;").unwrap();
    let _ = retire(&old, new.db.path.as_ref().unwrap()).unwrap_err();
    o.db.conn.execute_batch("ROLLBACK").unwrap();
    add(&o.db, "late");
    drop(o.db);
    assert!(retire(&old, new.db.path.as_ref().unwrap()).is_err());
    assert!(old.exists());
    let second = {
        let k = keep(&home, &old, &old, &p.folder().join("second.db")).unwrap();
        k.finish().unwrap();
        k.opened
    };
    retire(&old, second.db.path.as_ref().unwrap()).unwrap();
    assert!(!old.exists() && !content::log(&old, "-wal").exists());
}

#[test]
fn what_the_user_says_of_a_file_is_remembered() {
    let p = Place::new();
    let home = p.home("one");
    assert!(!way(&home, &p.file(), Use::Auto).0);
    home.set_choice(&p.file(), Use::Shared).unwrap();
    assert!(way(&home, &p.file(), Use::Auto).0);
    assert!(!way(&home, &p.file(), Use::Direct).0);
    home.set_choice(&p.file(), Use::Auto).unwrap();
    std::fs::write(p.folder().join(".stfolder"), "").unwrap();
    assert_eq!(way(&home, &p.file(), Use::Auto), (true, Some(Service::Syncthing)));
    home.set_choice(&p.file(), Use::Direct).unwrap();
    assert!(!way(&home, &p.file(), Use::Auto).0);
    let o = open(&home, &p.file(), Use::Auto, Lang::En, false).unwrap();
    assert!(o.is_new && o.shared.is_none() && o.db.path == Some(p.file()));
    assert_eq!(home.pick(Some("a.db".into())), ("a.db".into(), false));
    assert_eq!(Use::from_code("other"), None);
}

#[test]
fn folders_of_sync_apps_are_known_by_their_path() {
    let linux = System {
        mounts: "/dev/sda / ext4 rw 0 0\nC:\\134 /mnt/c 9p rw 0 0\nnas:/x /mnt/my\\040nas nfs4 rw 0 0\n\
                 /dev/sdb /mnt/c/usb ext4 rw 0 0\nbad\n\\9 \\999 \\\n"
            .into(),
        ..Default::default()
    };
    let windows = System {
        windows: true,
        onedrive: vec![br"C:\Users\Ann\Cloud Files".to_vec(), br"\\?\D:\Work\".to_vec(), vec![]],
        ..Default::default()
    };
    use Service::{Cloud, Dropbox, GoogleDrive, ICloud, Network, Nextcloud, OneDrive};
    let cases: [(&[u8], &System, Option<Service>); 44] = [
        (b"/home/ann/.local/share/CashCraft/cashcraft.db", &linux, None),
        (b"/home/ann/OneDrive/cashcraft.db", &linux, Some(OneDrive)),
        (b"/home/ann/onedrive/a/b/cashcraft.db", &linux, Some(OneDrive)),
        (b"/home/ann/OneDrive - Company/cashcraft.db", &linux, Some(OneDrive)),
        (b"/home/ann/OneDriver/cashcraft.db", &linux, None),
        (b"/home/ann/OneDrive", &linux, None),
        (b"/home/ann/my OneDrive/cashcraft.db", &linux, None),
        (b"/home/ann/Dropbox/cashcraft.db", &linux, Some(Dropbox)),
        (b"/home/ann/Dropbox (Personal)/cashcraft.db", &linux, Some(Dropbox)),
        (b"/home/\xe8\xff/Dropbox/\xff.db", &linux, Some(Dropbox)),
        (b"/home/\xe8\xff/Dropb\xffx/\xff.db", &linux, None),
        (b"/home/ann/Nextcloud/cashcraft.db", &linux, Some(Nextcloud)),
        (b"/home/ann/Google Drive/cashcraft.db", &linux, Some(GoogleDrive)),
        (b"/home/ann\\Dropbox\\cashcraft.db", &linux, None),
        (b"/mnt/c/Users/cashcraft.db", &linux, Some(Network)),
        (b"/mnt/c", &linux, Some(Network)),
        (b"/mnt/cd/cashcraft.db", &linux, None),
        (b"/mnt/c/usb/cashcraft.db", &linux, None),
        (b"/mnt/my nas/cashcraft.db", &linux, Some(Network)),
        (b"/mnt/c/Users/ann/OneDrive/cashcraft.db", &linux, Some(OneDrive)),
        (b"", &linux, None),
        (b"cashcraft.db", &linux, None),
        (b"//server/share/cashcraft.db", &linux, None),
        (b"/Users/ann/Library/Mobile Documents/com~apple~CloudDocs/c.db", &linux, Some(ICloud)),
        (b"/Users/ann/Library/CloudStorage/OneDrive-Personal/c.db", &linux, Some(OneDrive)),
        (b"/Users/ann/Library/CloudStorage/GoogleDrive-a@b.it/My Drive/c.db", &linux, Some(GoogleDrive)),
        (b"/Users/ann/Library/CloudStorage/Box-Box/c.db", &linux, Some(Cloud)),
        (b"/Users/ann/Library/Application Support/CashCraft/c.db", &linux, None),
        (br"C:\Users\Ann\AppData\Roaming\CashCraft\cashcraft.db", &windows, None),
        (br"C:\Users\Ann\OneDrive\cashcraft.db", &windows, Some(OneDrive)),
        (br"c:\users\ann\ONEDRIVE - ACME SPA\x\cashcraft.db", &windows, Some(OneDrive)),
        (br"c:\users\ann\cloud files\cashcraft.db", &windows, Some(OneDrive)),
        (b"C:/Users/Ann/Cloud Files/cashcraft.db", &windows, Some(OneDrive)),
        (br"\\?\C:\Users\Ann\Cloud Files\cashcraft.db", &windows, Some(OneDrive)),
        (br"C:\Users\Ann\Cloud Files 2\cashcraft.db", &windows, None),
        (br"D:\work\cashcraft.db", &windows, Some(OneDrive)),
        (br"C:\Users\Ann\iCloudDrive\cashcraft.db", &windows, Some(ICloud)),
        (br"G:\My Drive\cashcraft.db", &windows, Some(GoogleDrive)),
        (br"G:\Il mio Drive\cashcraft.db", &windows, Some(GoogleDrive)),
        (br"\\server\share\cashcraft.db", &windows, Some(Network)),
        (br"\\?\UNC\server\share\cashcraft.db", &windows, Some(Network)),
        (br"\\?\C:\data\cashcraft.db", &windows, None),
        (b"C:\\\xed\xa0\x80\\Dropbox\\c.db", &windows, Some(Dropbox)),
        (b"D:\\\xed\xa0\x80\\c.db", &windows, None),
    ];
    for (path, system, expected) in cases {
        assert_eq!(classify(path, system), expected, "{}", String::from_utf8_lossy(path));
    }
    // Here, whatever the system is.
    let _ = service_of(Path::new("cashcraft.db"), &System::here());
}

#[test]
fn computers_see_each_other_whatever_their_clocks_say() {
    let p = Place::new();
    std::fs::write(p.file(), "").unwrap();
    let start = Instant::now();
    let after = |s: u64| start + Duration::from_secs(s);
    let mut one = Presence::new(&p.home("one"), &p.file());
    let mut two = Presence::new(&p.home("two\n/x"), &p.file());
    assert_ne!(one.others(0, start), ["two"]);
    two.renew(1000);
    one.renew(1001);
    assert_eq!(one.others(1001, start), ["twox"]);
    assert_eq!(two.others(1001, start), ["one"]);
    // Not renewed any more.
    assert_eq!(one.others(1200, after(200)), ["twox"]);
    assert!(one.others(1500, after(500)).is_empty());
    // A clock that is years wrong: at first it cannot be told, then it is seen to move.
    let mut late = Presence::new(&p.home("three"), &p.file());
    let year = 365 * 24 * 3600;
    two.renew(1000 + 3 * year);
    one.renew(1600);
    assert_eq!(late.others(1600, after(600)), ["one"]);
    two.renew(1120 + 3 * year);
    assert_eq!(late.others(1720, after(720)), ["one", "twox"]);
    // Garbage and the files of others do no harm.
    std::fs::write(p.folder().join(".books.db.x.cashcraft-open"), [0xff; 9000]).unwrap();
    std::fs::write(p.folder().join(".other.db.y.cashcraft-open"), "{}").unwrap();
    assert_eq!(late.others(1720, after(721)), ["one", "twox"]);
    // One that is abandoned is removed.
    assert!(Presence::new(&p.home("four"), &p.file()).others(1000 + 9 * year, start).is_empty());
    one.leave();
    two.leave();
    let left: Vec<String> = p.names().into_iter().filter(|n| n.starts_with(".books.db.")).collect();
    assert_eq!(left, [".books.db.x.cashcraft-open"]);
    // Nothing is written beside a file that is not there.
    std::fs::remove_file(p.file()).unwrap();
    one.renew(0);
    assert_eq!(p.names().iter().filter(|n| n.starts_with(".books.db.")).count(), 1);
}

#[test]
fn a_wrong_clock_of_the_files_changes_nothing() {
    let p = Place::new();
    let (a, sa, _) = p.start("one");
    let (b, sb, _) = p.start("two");
    let stamp = |t: SystemTime| OpenOptions::new().write(true).open(p.file()).unwrap().set_modified(t).unwrap();
    for (i, t) in [SystemTime::UNIX_EPOCH, SystemTime::now() + Duration::from_secs(1 << 30)].into_iter().enumerate() {
        add(&b, &format!("b{i}"));
        sb.sync().unwrap();
        stamp(t);
        assert!(sa.sync().unwrap().adopted);
        add(&a, &format!("a{i}"));
        sa.sync().unwrap();
        stamp(t);
        // The time of the file is the same as before: it is its content that counts.
        assert!(sb.sync().unwrap().adopted);
        assert_eq!(payees(&a), payees(&b));
    }
    assert!(p.conflicts().is_empty());
}

/// Whatever two computers do, and wherever they die, what was entered is in the shared file, in a conflict file or in
/// a working copy, and the shared file is whole.
#[test]
fn no_entry_is_lost_whatever_happens() {
    let mut deaths = 0;
    let mut clashes = 0;
    for seed in 1..=12u64 {
        let p = Place::new();
        let mut r = Seeded(seed * 7919);
        let mut entered = BTreeSet::new();
        let names = ["one", "two"];
        let mut open_now: Vec<Option<(Db, Shared)>> = vec![None, None];
        for turn in 0..30 {
            let i = r.next(2);
            if open_now[i].is_none() {
                // The start may die too.
                let shared = Shared::new(&p.home(names[i]), &p.file());
                if p.file().exists() && r.next(3) == 0 && dies(&shared, r.next(12)) {
                    deaths += 1;
                    continue;
                }
                let (db, s, _) = p.start(names[i]);
                open_now[i] = Some((db, s));
            }
            let (db, shared) = open_now[i].as_ref().unwrap();
            match r.next(6) {
                0..=2 => {
                    let payee = format!("{}-{turn}", names[i]);
                    let hold = shared.hold().unwrap();
                    add(db, &payee);
                    drop(hold);
                    entered.insert(payee);
                }
                3 => {
                    if dies(shared, r.next(14)) {
                        deaths += 1;
                        open_now[i] = None;
                    }
                }
                4 => {
                    if shared.sync().unwrap().conflict.is_some() {
                        clashes += 1;
                    }
                }
                _ => open_now[i] = None,
            }
            if p.file().exists() {
                payees_of(&p.file());
            }
            let mut there: BTreeSet<String> = p.conflicts().iter().flat_map(|c| payees_of(c)).collect();
            if p.file().exists() {
                there.extend(payees_of(&p.file()));
            }
            for name in names {
                let copy = Shared::new(&p.home(name), &p.file()).copy();
                if copy_print(&copy).unwrap().is_some() {
                    there.extend(payees(&Db::open(&copy).unwrap().0));
                }
            }
            let lost: Vec<_> = entered.difference(&there).collect();
            assert!(lost.is_empty(), "seed {seed}, turn {turn}: {lost:?}");
        }
        // In the end all of it is in the folder, and nothing else.
        for name in names.iter().chain(&names) {
            assert!(!p.start(name).2.pending);
        }
        let mut there = payees_of(&p.file());
        there.extend(p.conflicts().iter().flat_map(|c| payees_of(c)));
        assert!(entered.is_subset(&there), "seed {seed}");
        assert!(p.names().iter().all(|n| n.ends_with(".db")), "{:?}", p.names());
    }
    assert!(deaths > 10 && clashes > 0, "{deaths} {clashes}");
}
