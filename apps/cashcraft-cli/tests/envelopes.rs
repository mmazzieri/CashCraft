//! The envelopes from the command line: a month assigned to the last euro, an overspent envelope covered.

use std::path::PathBuf;
use std::process::Command;

struct Dir(PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_month_is_assigned_and_an_overspent_envelope_covered() {
    let dir = Dir(std::env::temp_dir().join(format!("cashcraft-cli-envelopes-{}", std::process::id())));
    std::fs::create_dir_all(&dir.0).unwrap();
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
    ok(&["budget", "set", "Groceries", "300"]);
    let set = ok(&["envelopes", "settings", "--mode", "envelopes", "--start", "2026-03"]);
    assert!(set.contains("1 envelope made"), "{set}");
    ok(&["envelopes", "add", "Fun", "--categories", "Leisure"]);
    ok(&[
        "tx",
        "add",
        "--account",
        "Bank",
        "--date",
        "2026-03-01",
        "--amount",
        "1000",
        "--payee",
        "Work",
        "--category",
        "Salary",
    ]);
    ok(&[
        "tx",
        "add",
        "--account",
        "Bank",
        "--date",
        "2026-03-05",
        "--amount",
        "-350",
        "--payee",
        "Shop",
        "--category",
        "Groceries",
    ]);
    ok(&["envelopes", "assign", "Groceries", "300", "--month", "2026-03"]);
    ok(&["envelopes", "assign", "Fun", "700", "--month", "2026-03"]);
    let march = ok(&["envelopes", "list", "2026-03"]);
    assert!(march.contains("every euro has a job") && march.contains("overspent"), "{march}");
    let moved = ok(&["envelopes", "move", "Fun", "Groceries", "50", "--month", "2026-03", "--reason", "dinner"]);
    assert!(moved.contains("recomputed"), "{moved}");
    let april = ok(&["envelopes", "list", "2026-04"]);
    assert!(april.contains("€650.00") && !april.contains("overspent"), "{april}");
    ok(&["envelopes", "undo"]);
    assert!(ok(&["envelopes", "list", "2026-04"]).contains("-€50.00"));
    assert!(!run(&["envelopes", "move", "Fun", "Fun", "5"]).0);
    // A later first month would drop March's money: only with --confirm.
    assert!(!run(&["envelopes", "settings", "--start", "2026-04"]).0);
    let later = ok(&["envelopes", "settings", "--start", "2026-04", "--confirm"]);
    assert!(later.contains("recomputed") && later.contains("2026-03"), "{later}");
    // A part of a name finds nothing.
    assert!(!run(&["envelopes", "assign", "Gro", "5"]).0);
    let deleted = ok(&["envelopes", "delete", "Fun"]);
    assert!(deleted.contains("brings it back"), "{deleted}");
    ok(&["envelopes", "undo"]);
    assert!(ok(&["envelopes", "list", "2026-04"]).contains("Fun"));
    assert!(!run(&["envelopes", "assign", "Nothing", "5"]).0);
}
