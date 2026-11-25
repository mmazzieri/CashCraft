use super::{ALLOWED, beside, call};
use cashcraft_engine::ops;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("cashcraft-ffi-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir(&d).unwrap();
        Dir(d)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn answer(request: Value) -> Value {
    serde_json::from_str(&call(&request.to_string())).unwrap()
}

fn ask(request: Value) -> Value {
    let a = answer(request.clone());
    a.get("ok").cloned().unwrap_or_else(|| panic!("{request}: {a}"))
}

fn tool(path: &Path, name: &str, args: Value) -> Value {
    ask(json!({"op": "call", "path": path, "tool": name, "args": args}))
}

/// Debits equal credits, whatever the calls did.
fn assert_sound(path: &Path) {
    let (db, _) = cashcraft_engine::db::Db::open(path).unwrap();
    let problems = ops::call(&db, "verify_ledger", &json!({}), None).unwrap();
    assert_eq!(problems["ok"], true, "{problems}");
}

/// The file of one phone, carried whole to another, has everything, and no log lies beside it.
#[test]
fn a_file_is_whole_after_every_call() {
    let d = Dir::new("whole");
    let (a, b) = (d.file("a.db"), d.file("b.db"));
    ask(json!({"op": "new", "path": a}));
    tool(&a, "create_account", json!({"name": "Wallet", "kind": "cash"}));
    tool(&a, "add_transaction", json!({"account": "Wallet", "amount": -12.5, "payee": "Bakery"}));
    assert!(!beside(&a, "-wal").exists());
    std::fs::copy(&a, d.file("fetched")).unwrap();
    ask(json!({"op": "adopt", "from": d.file("fetched"), "path": b}));
    assert_eq!(tool(&b, "list_accounts", json!({}))["accounts"][0]["balance"], json!(-1250));
    assert_sound(&a);
    assert_sound(&b);
    assert!(!d.file("fetched").exists());
}

/// A log left by a copy that is replaced never reaches the file that takes its place.
#[test]
fn an_old_log_does_not_follow_a_new_file() {
    let d = Dir::new("log");
    let (a, b) = (d.file("a.db"), d.file("b.db"));
    for f in [&a, &b] {
        ask(json!({"op": "new", "path": f}));
    }
    tool(&b, "create_account", json!({"name": "New"}));
    std::fs::write(beside(&a, "-wal"), vec![7u8; 8192]).unwrap();
    ask(json!({"op": "adopt", "from": b, "path": a}));
    assert!(!beside(&a, "-wal").exists());
    assert_eq!(tool(&a, "list_accounts", json!({}))["accounts"][0]["name"], "New");
    assert_sound(&a);
}

/// Two phones that add a row each to the same file do not give it the same id.
#[test]
fn two_phones_number_rows_apart() {
    let d = Dir::new("ids");
    let (a, b) = (d.file("a.db"), d.file("b.db"));
    ask(json!({"op": "new", "path": a}));
    tool(&a, "create_account", json!({"name": "Wallet"}));
    std::fs::copy(&a, &b).unwrap();
    let ids: Vec<Value> = [&a, &b]
        .iter()
        .map(|f| tool(f, "add_transaction", json!({"account": "Wallet", "amount": -1, "payee": "x"}))["id"].clone())
        .collect();
    assert_ne!(ids[0], ids[1]);
    assert!(ids[0].as_i64().unwrap() > 1_000_000, "a spread id, not the next number");
    assert_sound(&a);
}

#[test]
fn what_is_not_a_whole_data_file_is_not_adopted() {
    let d = Dir::new("bad");
    let good = d.file("good.db");
    ask(json!({"op": "new", "path": good}));
    tool(&good, "create_account", json!({"name": "Kept"}));
    let bytes = std::fs::read(&good).unwrap();
    let other = d.file("other.db");
    rusqlite::Connection::open(&other).unwrap().execute_batch("CREATE TABLE x(y); PRAGMA user_version = 3;").unwrap();
    let empty = d.file("empty.db");
    rusqlite::Connection::open(&empty)
        .unwrap()
        .execute_batch(&format!(
            "CREATE TABLE categories(x); CREATE TABLE meta(x); PRAGMA user_version = {};",
            cashcraft_engine::db::SCHEMA_VERSION
        ))
        .unwrap();
    let newer = d.file("newer.db");
    std::fs::copy(&good, &newer).unwrap();
    rusqlite::Connection::open(&newer).unwrap().pragma_update(None, "user_version", 1_000_000).unwrap();
    let cases = [
        Vec::new(),
        b"hello".to_vec(),
        bytes.get(..bytes.len() / 2).unwrap().to_vec(),
        std::fs::read(&other).unwrap(),
        std::fs::read(&empty).unwrap(),
        std::fs::read(&newer).unwrap(),
    ];
    for (i, case) in cases.iter().enumerate() {
        let from = d.file("from");
        std::fs::write(&from, case).unwrap();
        assert!(answer(json!({"op": "adopt", "from": from, "path": good})).get("error").is_some(), "case {i}");
        assert_eq!(std::fs::read(&good).unwrap(), bytes, "case {i}: the copy is as it was");
    }
    assert!(
        answer(json!({"op": "call", "path": d.file("none"), "tool": "list_accounts", "args": {}}))
            .get("error")
            .is_some()
    );
    assert!(!d.file("none").exists(), "a file that is not there is not made");
    assert!(answer(json!({"op": "new", "path": good})).get("error").is_some());
}

#[test]
fn hostile_params_never_panic() {
    let d = Dir::new("hostile");
    let f = d.file("a.db");
    ask(json!({"op": "new", "path": f}));
    tool(&f, "create_account", json!({"name": "Wallet"}));
    let values = [
        json!(null),
        json!(true),
        json!(-1),
        json!(1e308),
        json!(u64::MAX),
        json!(""),
        json!("\0"),
        json!("€"),
        json!("x".repeat(10_000)),
        json!([]),
        json!({"a": 1}),
    ];
    let defs = ops::tools();
    for name in ALLOWED {
        let def = defs.iter().find(|t| t.name == *name).unwrap();
        let keys: Vec<String> = def.schema["properties"].as_object().unwrap().keys().cloned().collect();
        for v in &values {
            let args: Map<String, Value> = keys.iter().map(|k| (k.clone(), v.clone())).collect();
            answer(json!({"op": "call", "path": f, "tool": name, "args": args}));
            answer(json!({"op": "call", "path": f, "tool": name, "args": v}));
        }
    }
    for name in ["export_csv", "import_csv", "preview_csv", "nothing"] {
        assert!(
            answer(json!({"op": "call", "path": f, "tool": name, "args": {"path": "/etc/passwd"}}))
                .get("error")
                .is_some()
        );
    }
    for junk in ["", "null", "{}", "{\"op\":\"nothing\"}", "{\"op\":\"settle\",\"path\":\"\"}"] {
        let a: Value = serde_json::from_str(&call(junk)).unwrap();
        assert!(a.get("error").is_some(), "{junk:?}");
    }
    assert_sound(&f);
    let request = std::ffi::CString::new("{}").unwrap();
    // SAFETY: a valid string in, and the answer freed once.
    #[allow(unsafe_code)]
    unsafe {
        let a = crate::c::cashcraft_call(request.as_ptr());
        assert!(std::ffi::CStr::from_ptr(a).to_str().unwrap().contains("\"error\""));
        crate::c::cashcraft_free(a);
        assert!(!crate::c::cashcraft_call(std::ptr::null()).is_null());
    }
}
