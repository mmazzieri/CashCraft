//! The rules from the command line: what is printed before and after a run.

use std::path::PathBuf;
use std::process::Command;

struct Dir(PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_preview_shows_the_old_and_the_new() {
    let dir = Dir(std::env::temp_dir().join(format!("cashcraft-cli-rules-{}", std::process::id())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let csv = dir.0.join("a.csv");
    std::fs::write(&csv, "Data;Descrizione;Importo\n05/01/2026;POS 7 Zoo;-1,20\n").unwrap();
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_cashcraft-cli"))
            .env("CASHCRAFT_LANG", "en")
            .env("CASHCRAFT_DB", dir.0.join("books.db"))
            .args(args)
            .output()
            .unwrap();
        (out.status.success(), String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap())
    };
    let ok = |args: &[&str]| {
        let (ok, out, err) = run(args);
        assert!(ok, "{args:?} {err}");
        out
    };
    ok(&["accounts", "add", "Bank"]);
    ok(&["import", csv.to_str().unwrap(), "--account", "Bank"]);
    ok(&["tx", "edit", "1", "--category", "Travel"]);
    let add =
        ["rules", "add", "--payee", r"^pos \d (.+)$", "--regex", "--rename", "$1", "--category", "Leisure", "--first"];
    ok(&add);
    let seen = ok(&["rules", "preview", "--all", "--overwrite"]);
    assert!(seen.contains("Travel ➡ Leisure") && seen.contains("POS 7 Zoo ➡ Zoo"), "{seen}");
    assert!(ok(&["rules", "apply", "--all", "--overwrite"]).contains("rules undo 1"));
    // Without its number the run is named, and not undone.
    let (done, _, error) = run(&["rules", "undo"]);
    assert!(!done && error.contains("the last run is 1") && error.contains("1 transaction"), "{error}");
    let (done, _, error) = run(&["rules", "undo", "9"]);
    assert!(!done && error.contains("run 9 cannot be undone"), "{error}");
    ok(&["rules", "undo", "1"]);
    assert!(ok(&["tx", "show", "1"]).contains("Travel"));
}
