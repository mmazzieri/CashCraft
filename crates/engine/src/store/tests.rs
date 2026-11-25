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
        Place(dir)
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
    assert!(!content::uses_log(&bytes), "{}", path.display());
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
        let before = (a.data_version(), sa.epoch());
        assert!(sa.looks_changed());
        assert!(sa.sync().unwrap().adopted);
        assert!(!sa.looks_changed() && a.data_version() != before.0 && sa.epoch() == before.1 + 1);
        add(&a, &format!("a{i}"));
        sa.sync().unwrap();
        assert!(sb.sync().unwrap().adopted);
        assert_eq!(payees(&a), payees(&b));
    }
    assert_eq!(payees_of(&p.file()).len(), 9);
    assert_eq!(p.names(), ["books.db"]);
    assert_sound(&a, "shared");
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

fn die_at(shared: &Shared, n: usize) -> (Shared, Arc<AtomicUsize>) {
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let faults: Faults = Arc::new(move |_| {
        if c.fetch_add(1, Ordering::Relaxed) == n {
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
    let name = std::ffi::OsStr::from_bytes(b"conti \xe8.db");
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

fn quiet(path: &Path) {
    let f = OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(600)).unwrap();
}

#[test]
fn a_log_left_by_0_3_0_is_recovered_unless_it_is_in_use() {
    let p = Place::new();
    let live = p.0.join("live.db");
    let db = left_open(&live);
    let wal = content::log(&p.file(), "-wal");
    std::fs::copy(&live, p.file()).unwrap();
    std::fs::copy(content::log(&live, "-wal"), &wal).unwrap();
    let home = p.home("one");
    let start = || open(&home, &p.file(), Use::Shared, Lang::En, false);
    // Just written: a program may be running.
    assert!(start().is_err() && wal.exists());
    quiet(&wal);
    // With a program that has it open.
    let reader = content::open(&p.file()).unwrap();
    reader.execute_batch("BEGIN; SELECT * FROM meta;").unwrap();
    assert!(start().is_err());
    assert!(wal.exists());
    quiet(&wal);
    drop(reader);
    if !wal.exists() {
        // The last to close it brought the log in.
        std::fs::copy(&live, p.file()).unwrap();
        std::fs::copy(content::log(&live, "-wal"), &wal).unwrap();
        quiet(&wal);
    }
    let o = start().unwrap();
    assert_eq!(payees(&o.db), set(&["in the file", "in the log"]));
    let shared = o.shared.unwrap().0;
    assert_eq!(payees_of(&p.file()), set(&["in the file", "in the log"]));
    assert_eq!(p.names(), ["books.db"]);
    let before = shared.dir.join("recovered-1").join("before.db");
    assert!(before.exists() && content::log(&before, "-wal").exists());
    // A log that appears later is waited for, and changes are not written.
    let old = std::fs::read(p.file()).unwrap();
    std::fs::write(content::log(&p.file(), "-journal"), "").unwrap();
    add(&o.db, "later");
    let report = shared.sync().unwrap();
    assert_eq!((report.status, report.pending), (Status::InUse, true));
    assert_eq!(std::fs::read(p.file()).unwrap(), old);
    std::fs::remove_file(content::log(&p.file(), "-journal")).unwrap();
    // The log of another file is never played onto this one.
    std::fs::copy(content::log(&live, "-wal"), &wal).unwrap();
    quiet(&wal);
    let old = std::fs::read(p.file()).unwrap();
    assert!(matches!(shared.sync().unwrap().status, Status::Waiting(w) if w.contains("does not belong")));
    assert_eq!(std::fs::read(p.file()).unwrap(), old);
    std::fs::remove_file(&wal).unwrap();
    assert_eq!(shared.sync().unwrap().status, Status::Saved);
    drop(db);
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
fn a_copy_is_put_in_a_folder_and_the_old_file_goes_only_when_it_is_safe() {
    let p = Place::new();
    let old = p.0.join("old.db");
    let home = p.home("one");
    let o = open(&home, &old, Use::Auto, Lang::En, false).unwrap();
    assert!(o.shared.is_none());
    add(&o.db, "x");
    copy_to(&home, &o.db, &p.file()).unwrap();
    assert!(copy_to(&home, &o.db, &p.file()).is_err());
    assert_eq!((payees_of(&p.file()), p.names()), (set(&["x"]), vec!["books.db".into()]));
    let new = open(&home, &p.file(), Use::Shared, Lang::En, true).unwrap();
    // Open elsewhere, or changed since.
    o.db.conn.execute_batch("BEGIN; SELECT * FROM meta;").unwrap();
    let _ = retire(&old, &new.db).unwrap_err();
    o.db.conn.execute_batch("ROLLBACK").unwrap();
    add(&o.db, "late");
    drop(o.db);
    assert!(retire(&old, &new.db).is_err());
    assert!(old.exists());
    add(&new.db, "late");
    // Not the same entry for a computer: it has another id.
    let same = open(&home, &old, Use::Direct, Lang::En, true).unwrap().db;
    copy_to(&home, &same, &p.folder().join("second.db")).unwrap();
    drop(same);
    let second = open(&home, &p.folder().join("second.db"), Use::Shared, Lang::En, true).unwrap();
    retire(&old, &second.db).unwrap();
    assert!(!old.exists() && !content::log(&old, "-wal").exists());
    assert!(copy_to(&home, &new.db, &p.0.join("nowhere").join("a.db")).is_err());
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
    let limit = Instant::now() + Duration::from_secs(15);
    let mut deaths = 0;
    let mut clashes = 0;
    for seed in 1..=40u64 {
        if Instant::now() > limit && seed > 8 {
            break;
        }
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
