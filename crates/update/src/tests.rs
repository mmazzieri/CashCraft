use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair as _};
use sha2::{Digest, Sha256};

use crate::apply::{self, Folders, Job, StartNews};
use crate::download::{self, Fetch, SIGNATURE, SUMS, Verified};
use crate::github::{self, Asset, Release};
use crate::install::{self, Install};
use crate::version::Version;
use crate::{Problem, Project, Settings, http};

const REPO: &str = "https://github.com/example-owner/example";

fn v(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cashcraft-update-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn key() -> Ed25519KeyPair {
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
    Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
}

fn public(k: &Ed25519KeyPair) -> &'static str {
    Box::leak(STANDARD.encode(k.public_key().as_ref()).into_boxed_str())
}

fn project(keys: [&'static str; 2]) -> Project {
    Project { name: "Example", program: "example", repository: REPO, keys }
}

/// A stand-in for GitHub: files by address, and the addresses asked for.
struct Fake {
    files: HashMap<String, Vec<u8>>,
    asked: RefCell<Vec<String>>,
}

impl Fetch for Fake {
    fn fetch(&self, url: &str, max: u64, sink: &mut dyn FnMut(&[u8]) -> Result<(), Problem>) -> Result<(), Problem> {
        self.asked.borrow_mut().push(url.to_string());
        let body = self.files.get(url).ok_or(Problem::NoRelease)?;
        if body.len() as u64 > max {
            return Err(Problem::TooBig);
        }
        sink(body)
    }
}

const MSI: &str = "example-0.5.0-windows-x64.msi";

fn url(name: &str) -> String {
    format!("{REPO}/releases/download/v0.5.0/{name}")
}

/// A release of `msi`, whose list of checksums names `listed` and is signed by `signer`.
fn release(signer: &Ed25519KeyPair, msi: &[u8], listed: &[u8]) -> (Release, Fake) {
    let sums = format!("{}  {MSI}\n{}  other.zip\n", download::hex(&Sha256::digest(listed)), "0".repeat(64));
    let sig = STANDARD.encode(signer.sign(sums.as_bytes()).as_ref());
    let files: HashMap<String, Vec<u8>> =
        [(url(MSI), msi.to_vec()), (url(SUMS), sums.into_bytes()), (url(SIGNATURE), sig.into_bytes())].into();
    let assets = [MSI, SUMS, SIGNATURE]
        .iter()
        .map(|n| Asset { name: n.to_string(), size: files[&url(n)].len() as u64, url: url(n) })
        .collect();
    let r = Release { version: v("0.5.0"), page: format!("{REPO}/releases/tag/v0.5.0"), security: false, assets };
    (r, Fake { files, asked: RefCell::new(Vec::new()) })
}

#[test]
fn versions_are_read_strictly_and_ordered() {
    assert_eq!(v("v1.2.3").to_string(), "1.2.3");
    for bad in ["", "1.2", "1.2.3.4", "01.2.3", "1.2.3+b", "1.2.3-", "latest", " 1.2.3"] {
        assert_eq!(Version::parse(bad), None, "{bad:?}");
    }
    for (newer, older) in [("0.4.0", "0.3.9"), ("0.10.0", "0.9.0"), ("0.4.0", "0.4.0-beta.1")] {
        assert!(v(newer).is_newer_than(&v(older)) && !v(older).is_newer_than(&v(newer)));
    }
    assert!(!v("0.4.0").is_newer_than(&v("v0.4.0")));
}

#[test]
fn only_the_release_s_own_files_are_taken_from_github_s_answer() {
    let answer = serde_json::json!({
        "tag_name": "v0.5.0",
        "html_url": "https://evil.example/page",
        "body": "Fixes GHSA-abcd-1234-wxyz.",
        "assets": [
            {"name": MSI, "size": 10, "browser_download_url": url(MSI)},
            {"name": "other.msi", "size": 10, "browser_download_url": "https://evil.example/other.msi"},
            {"name": "../x.msi", "size": 10, "browser_download_url": url("../x.msi")},
            {"name": "twice.txt", "size": 1, "browser_download_url": url("twice.txt")},
            {"name": "twice.txt", "size": 1, "browser_download_url": url("twice.txt")},
        ]
    });
    let r = github::release(answer.to_string().as_bytes(), "example-owner", "example", false).unwrap();
    assert_eq!(r.page, format!("{REPO}/releases/tag/v0.5.0"));
    assert!(r.security);
    assert_eq!(r.assets.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), [MSI]);
    let pre = serde_json::json!({"tag_name": "v0.6.0-beta.1", "assets": []}).to_string();
    assert_eq!(github::release(pre.as_bytes(), "example-owner", "example", false), Err(Problem::PreRelease));
    assert!(github::release(pre.as_bytes(), "example-owner", "example", true).is_ok());
    assert_eq!(github::release(b"<html>", "o", "r", false), Err(Problem::BadAnswer));
}

#[test]
fn nothing_is_asked_of_hosts_but_github_s() {
    for good in ["https://api.github.com/repos", "https://github.com/a/b", "https://objects.githubusercontent.com/x"] {
        assert!(http::allowed_host(good).is_some(), "{good}");
    }
    for bad in [
        "http://github.com/a",
        "https://github.com.evil.example/a",
        "https://user@github.com/a",
        "https://github.com:8443/a",
        "https://evil.example/github.com",
        "ftp://github.com/a",
    ] {
        assert!(http::allowed_host(bad).is_none(), "{bad}");
    }
}

#[test]
fn a_signed_release_is_downloaded_and_checked() {
    let k = key();
    let dir = scratch("signed");
    let (r, fake) = release(&k, b"the installer", b"the installer");
    let got = download::download(&project([public(&k), ""]), &r, MSI, &dir, &fake).unwrap();
    assert_eq!(got.path, dir.join(MSI));
    assert_eq!(std::fs::read(&got.path).unwrap(), b"the installer");
    assert_eq!(download::sha256(&got.path), Some(got.sha256));
    // Asked again, the file already there is taken: only the list and its signature are fetched.
    fake.asked.borrow_mut().clear();
    download::download(&project([public(&k), ""]), &r, MSI, &dir, &fake).unwrap();
    assert!(!fake.asked.borrow().contains(&url(MSI)));
    // The spare key does as well as the one in use.
    let spare = key();
    let (r, fake) = release(&spare, b"x", b"x");
    assert!(download::download(&project([public(&k), public(&spare)]), &r, MSI, &scratch("spare"), &fake).is_ok());
}

#[test]
fn a_release_signed_by_anyone_else_is_refused_and_nothing_is_kept() {
    let (ours, theirs) = (key(), key());
    let dir = scratch("theirs");
    let (r, fake) = release(&theirs, b"evil", b"evil");
    assert_eq!(download::download(&project([public(&ours), ""]), &r, MSI, &dir, &fake), Err(Problem::NotSigned));
    assert!(!fake.asked.borrow().contains(&url(MSI)), "the file itself is never fetched");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
}

#[test]
fn a_file_that_is_not_the_one_signed_for_is_deleted() {
    let k = key();
    let dir = scratch("changed");
    let (r, fake) = release(&k, b"changed on the way", b"the installer");
    let got = download::download(&project([public(&k), ""]), &r, MSI, &dir, &fake);
    assert_eq!(got, Err(Problem::Mismatch));
    assert!(got.unwrap_err().refused());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
}

#[test]
fn an_unsigned_release_or_a_build_without_keys_downloads_nothing() {
    let k = key();
    let (mut r, fake) = release(&k, b"x", b"x");
    assert_eq!(download::download(&project(["", ""]), &r, MSI, &scratch("nokeys"), &fake), Err(Problem::NoKeys));
    r.assets.retain(|a| a.name != SIGNATURE);
    assert!(!download::is_signed(&r));
    assert_eq!(
        download::download(&project([public(&k), ""]), &r, MSI, &scratch("unsigned"), &fake),
        Err(Problem::Unsigned)
    );
    assert!(fake.asked.borrow().is_empty());
    // A key that isn't one checks nothing.
    assert!(!download::signed_by(&["not base64!", ""], b"x", b"y"));
}

#[test]
fn the_list_of_checksums_is_read_strictly() {
    let a = "a".repeat(64);
    assert_eq!(download::sum_for(&format!("{a}  f.msi\n"), "f.msi"), Some([0xaa; 32]));
    assert_eq!(download::sum_for(&format!("{a} *f.msi\r\n"), "f.msi"), Some([0xaa; 32]));
    assert_eq!(download::sum_for(&format!("{a}  f.msi\n{a}  f.msi\n"), "f.msi"), None);
    assert_eq!(download::sum_for(&format!("{a}  g.msi\n"), "f.msi"), None);
    assert_eq!(download::sum_for("zz  f.msi\n", "f.msi"), None);
}

#[test]
fn only_an_msi_copy_updates_itself_in_the_scope_it_was_installed_in() {
    let exe = Path::new(r"C:\Users\x\AppData\Local\Programs\Example\example.exe");
    let other = Path::new(r"C:\Program Files\Example\example.exe");
    assert_eq!(install::windows(exe, Some(exe), None), Install::Msi { per_user: true });
    let quoted = PathBuf::from(r#""C:\USERS\X\AppData\Local\Programs\Example\example.exe""#);
    assert_eq!(install::windows(exe, Some(&quoted), None), Install::Msi { per_user: true });
    assert_eq!(install::windows(other, Some(exe), Some(other)), Install::Msi { per_user: false });
    // A portable copy, wherever it was unpacked.
    assert_eq!(install::windows(Path::new(r"D:\tools\example.exe"), Some(exe), Some(other)), Install::NoticeOnly);
    assert_eq!(Install::NoticeOnly.file_name("example", &v("0.5.0"), "x86_64"), None);
    assert_eq!(Install::Msi { per_user: true }.file_name("example", &v("0.5.0"), "x86_64").as_deref(), Some(MSI));
}

#[test]
fn the_installer_s_job_goes_through_its_arguments_and_nothing_else_is_one() {
    let msi = std::env::temp_dir().join(MSI);
    let exe = std::env::temp_dir().join("example.exe");
    let job = Job { msi: msi.clone(), sha256: [7; 32], per_user: true, exe: exe.clone(), restart: true };
    let mut args = job.args();
    assert_eq!(Job::parse(&args), Some(job.clone()));
    let stay = Job { restart: false, ..job.clone() };
    assert_eq!(Job::parse(&stay.args()), Some(stay));
    args.push("more".into());
    assert_eq!(Job::parse(&args), None);
    let bad = |i: usize, with: &str| {
        let mut a = job.args();
        a[i] = OsString::from(with);
        Job::parse(&a)
    };
    assert_eq!(bad(0, "--other"), None);
    assert_eq!(bad(1, "relative.msi"), None);
    assert_eq!(bad(1, &std::env::temp_dir().join("x.exe").to_string_lossy()), None);
    assert_eq!(bad(2, "00"), None);
    assert_eq!(bad(3, "everyone"), None);
    assert_eq!(bad(4, "example.exe"), None);
    assert_eq!(bad(5, "maybe"), None);
}

#[test]
fn windows_installer_is_handed_a_checked_copy_in_the_right_scope() {
    let dir = scratch("handed");
    let folders = Folders::new(dir.join("updates"));
    let msi = dir.join(MSI);
    std::fs::write(&msi, b"the installer").unwrap();
    let sha: [u8; 32] = Sha256::digest(b"the installer").into();
    let mut seen = Vec::new();
    let job = Job { msi: msi.clone(), sha256: sha, per_user: true, exe: dir.join("example.exe"), restart: false };
    let there = |_: &Path| true;
    let ok = apply::install_msi_with(
        &job,
        &folders,
        &mut |run| {
            let handed = PathBuf::from(&run.args[1]);
            assert_ne!(handed, msi);
            assert_eq!(std::fs::read(&handed).unwrap(), b"the installer");
            seen = run.args.clone();
            Some(0)
        },
        &there,
    );
    assert!(ok);
    assert!(seen.contains(&"MSIINSTALLPERUSER=1".into()) && seen.contains(&"/passive".into()));
    // The file installed from stays, for Windows Installer to repair from.
    assert_eq!(std::fs::read(PathBuf::from(&seen[1])).unwrap(), b"the installer");
    // A file changed since it was checked is never run.
    std::fs::write(&msi, b"changed").unwrap();
    let ran = std::cell::Cell::new(false);
    assert!(!apply::install_msi_with(
        &job,
        &folders,
        &mut |_| {
            ran.set(true);
            Some(0)
        },
        &there,
    ));
    assert!(!ran.get());
    // For the whole computer, and an installer that says no.
    std::fs::write(&msi, b"the installer").unwrap();
    let machine = Job { per_user: false, ..job.clone() };
    assert!(!apply::install_msi_with(
        &machine,
        &folders,
        &mut |run| {
            assert!(run.args.contains(&"ALLUSERS=1".into()) && !run.args.contains(&"MSIINSTALLPERUSER=1".into()));
            Some(1602)
        },
        &there,
    ));
    // An installer that says it is done and has left the program out: repaired once, and the
    // update counts only when the program is there afterwards.
    let runs = std::cell::RefCell::new(Vec::new());
    let repaired = std::cell::Cell::new(false);
    assert!(apply::install_msi_with(
        &job,
        &folders,
        &mut |run| {
            runs.borrow_mut().push(run.args[0].clone());
            if run.args[0] == "/fa" {
                repaired.set(true);
            }
            Some(0)
        },
        &|_| repaired.get(),
    ));
    assert_eq!(*runs.borrow(), vec![OsString::from("/i"), OsString::from("/fa")]);
    runs.borrow_mut().clear();
    assert!(!apply::install_msi_with(
        &job,
        &folders,
        &mut |run| {
            runs.borrow_mut().push(run.args[0].clone());
            Some(0)
        },
        &|_| false,
    ));
    assert_eq!(runs.borrow().len(), 2, "one repair, not more");
}

#[test]
fn the_next_start_says_once_what_came_of_the_update() {
    let dir = scratch("record");
    let folders = Folders::new(dir.clone());
    let msi = dir.join("downloads").join(MSI);
    std::fs::create_dir_all(msi.parent().unwrap()).unwrap();
    std::fs::write(&msi, b"x").unwrap();
    let verified = Verified { version: v("0.5.0"), path: msi.clone(), sha256: Sha256::digest(b"x").into() };
    // Not a copy that updates itself: nothing is recorded or started.
    assert!(apply::on_exit(&folders, Install::NoticeOnly, &verified, "0.4.0", None, false).is_err());
    assert_eq!(apply::at_start(&folders, "0.4.0"), None);
    assert!(msi.exists(), "a download newer than the running version is kept");
    // A record as on_exit writes it, read by the new version and by the old one.
    let record = |from: &str, to: &str| {
        let json = serde_json::json!({"from": from, "to": to, "backup": dir.join("b.db")});
        std::fs::write(dir.join("update-record.json"), json.to_string()).unwrap();
    };
    record("0.4.0", "0.5.0");
    assert_eq!(
        apply::at_start(&folders, "0.5.0"),
        Some(StartNews::Updated { from: "0.4.0".into(), backup: Some(dir.join("b.db")) })
    );
    assert_eq!(apply::at_start(&folders, "0.5.0"), None, "said once");
    assert!(!msi.exists(), "the installed version's download is gone");
    record("0.4.0", "0.5.0");
    assert_eq!(apply::at_start(&folders, "0.4.0"), Some(StartNews::NotInstalled { to: "0.5.0".into() }));
    std::fs::write(dir.join("update-record.json"), "{\"from\":\"<x>\",\"to\":\"0.5.0\"}").unwrap();
    assert_eq!(apply::at_start(&folders, "0.5.0"), None);
}

#[test]
fn settings_survive_a_broken_file_and_a_check_is_due_once_a_day() {
    let dir = scratch("settings");
    let path = dir.join("settings.json");
    assert_eq!(Settings::load(&path), Settings::default());
    std::fs::write(&path, "{\"check\": true, \"asked\": 250, \"junk\": [1]}").unwrap();
    let s = Settings::load(&path);
    assert_eq!((s.check, s.asked, s.install), (Some(true), crate::MOST_ASKS, true));
    std::fs::write(&path, "not json").unwrap();
    assert_eq!(Settings::load(&path), Settings::default());
    let mut s = Settings { check: Some(true), last_check: 1_000_000, ..Settings::default() };
    s.save(&path).unwrap();
    assert_eq!(Settings::load(&path), s);
    assert!(!s.due(1_000_000 + 3_600) && s.due(1_000_000 + 86_400));
    assert!(s.due(10), "a clock set back doesn't stop the checks");
    s.check = None;
    assert!(!s.due(2_000_000), "never before the owner said yes");
}
