//! OFX and saved columns from the command line.

use std::process::Command;

#[test]
fn a_statement_is_previewed_decided_and_imported() {
    let dir = std::env::temp_dir().join(format!("cashcraft-cli-ofx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let statement = |account: &str, date: &str| {
        format!(
            "<STMTRS><CURDEF>EUR<BANKACCTFROM><ACCTID>{account}</BANKACCTFROM><BANKTRANLIST>\
             <STMTTRN><DTPOSTED>{date}<TRNAMT>-1,20<FITID>A<NAME>BAR</STMTTRN></BANKTRANLIST>\
             <LEDGERBAL><BALAMT>-1.20<DTASOF>20260131</LEDGERBAL></STMTRS>"
        )
    };
    let file = format!("<OFX>{}{}</OFX>", statement("11112222", "20260106"), statement("33334444", "20260107"));
    std::fs::write(dir.join("a.ofx"), file).unwrap();
    std::fs::write(dir.join("a.csv"), "Data;Nome;Importo;Netto\n05/01/2026;Bar;1;1,20\n").unwrap();
    std::fs::write(dir.join("bad.csv"), "Data;Nome;Importo\n05/01/2026;\"Bar;1,20\n").unwrap();
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_cashcraft-cli"))
            .current_dir(&dir)
            .env("CASHCRAFT_LANG", "en")
            .env("CASHCRAFT_DB", dir.join("books.db"))
            .args(args)
            .output()
            .unwrap();
        (out.status.success(), String::from_utf8_lossy(&[out.stdout, out.stderr].concat()).into_owned())
    };
    let ok = |args: &[&str]| {
        let (done, text) = run(args);
        assert!(done, "{args:?}: {text}");
        text
    };
    let refused = |args: &[&str], why: &str| {
        let (done, text) = run(args);
        assert!(!done && text.contains(why), "{args:?}: {text}");
    };
    ok(&["accounts", "add", "Bank"]);
    refused(&["preview", "bad.csv"], "line 2: ");
    ok(&["import", "a.csv", "-a", "Bank", "--amount-col", "Netto", "--invert", "--decimals", ",", "--dates", "dmy"]);
    let again = ok(&["preview", "a.csv", "-a", "Bank"]);
    assert!(
        again.contains("Columns remembered") && again.contains("-€1.20") && again.contains("inverted, as in"),
        "{again}"
    );
    assert!(!ok(&["preview", "a.csv"]).contains("Columns remembered"));
    assert!(!ok(&["preview", "a.csv", "-a", "Bank", "--no-saved"]).contains("Columns remembered"));
    let plain = ok(&["preview", "a.csv", "-a", "Bank", "--no-invert"]);
    assert!(plain.contains(" €1.20") && !plain.contains("inverted"), "{plain}");
    // Said before it is written.
    let done = ok(&["import", "a.csv", "-a", "Bank"]);
    assert!(done.find("inverted, as in").unwrap() < done.find("already present").unwrap(), "{done}");
    // Another account does not have the sign of the first.
    ok(&["accounts", "add", "Two"]);
    ok(&["import", "a.csv", "-a", "Two", "--amount-col", "Netto"]);
    assert!(ok(&["accounts", "list"]).contains(" €1.20"));
    let list = ok(&["columns", "list"]);
    assert!(list.contains("amount=Netto; ") || list.contains("amount=Netto, "), "{list}");
    assert!(list.contains("sign inverted") && list.contains("sign as in the file"), "{list}");
    refused(&["columns", "forget", "7"], "no saved columns 7");
    ok(&["columns", "forget", "--all"]);
    let preview = ok(&["preview", "a.ofx"]);
    assert!(preview.contains("--statement") && preview.contains("…2222"), "{preview}");
    refused(&["import", "a.ofx", "-a", "Bank"], "several statements");
    let preview = ok(&["preview", "a.ofx", "--statement", "11112222", "-a", "Bank"]);
    assert!(
        preview.contains("-1.20 in the books") && preview.contains("--skip-rows") && preview.contains("→"),
        "{preview}"
    );
    refused(&["import", "a.ofx", "-a", "Bank", "--statement", "1"], "may be there already");
    let trial = ok(&["import", "a.ofx", "-a", "Bank", "--statement", "1", "--dry-run"]);
    assert!(trial.contains("would be refused"), "{trial}");
    refused(&["import", "a.ofx", "-a", "Bank", "--statement", "1", "--skip-rows", "all"], "one by one");
    ok(&["import", "a.ofx", "-a", "Bank", "--statement", "1", "--skip-rows", "1"]);
    let again = ok(&["import", "a.ofx", "-a", "Bank", "--statement", "1"]);
    assert!(again.contains("0 imported, 1 already present"), "{again}");
    assert!(ok(&["ledger", "verify"]).contains("sound"));
    std::fs::remove_dir_all(&dir).unwrap();
}
