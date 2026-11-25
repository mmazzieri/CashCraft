//! A data file in a synced folder, from the command line and MCP, on two computers.

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output, Stdio};
    use std::time::Duration;

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    impl Dir {
        fn new(name: &str) -> Dir {
            let dir = std::env::temp_dir().join(format!("cashcraft-cli-shared-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("OneDrive")).unwrap();
            Dir(dir)
        }
        fn folder(&self) -> PathBuf {
            self.0.join("OneDrive")
        }
        fn file(&self) -> PathBuf {
            self.folder().join("books.db")
        }
        fn command(&self, computer: &str, args: &[&str]) -> Command {
            let home = self.0.join(computer);
            let mut c = Command::new(env!("CARGO_BIN_EXE_cashcraft-cli"));
            c.env("CASHCRAFT_LANG", "en").env_remove("CASHCRAFT_DB").env("COMPUTERNAME", computer);
            for name in ["XDG_DATA_HOME", "APPDATA", "LOCALAPPDATA", "HOME"] {
                c.env(name, &home);
            }
            c.args(args).stdin(Stdio::null());
            c
        }
        fn run(&self, computer: &str, args: &[&str]) -> Output {
            self.command(computer, args).output().unwrap()
        }
        /// On the shared file, without errors: what was printed and the notices.
        fn ok(&self, computer: &str, args: &[&str]) -> (String, String) {
            let file = self.file();
            let all: Vec<&str> = ["--db", file.to_str().unwrap()].into_iter().chain(args.iter().copied()).collect();
            let out = self.run(computer, &all);
            let (text, notices) = (String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap());
            assert!(out.status.success(), "{args:?} {text} {notices}");
            (text, notices)
        }
        fn names(&self) -> BTreeSet<String> {
            let all = std::fs::read_dir(self.folder()).unwrap();
            all.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()
        }
    }

    fn complete(file: &Path) {
        let bytes = std::fs::read(file).unwrap();
        assert!(bytes.starts_with(b"SQLite format 3\0") && bytes.len().is_multiple_of(4096) && bytes[18] == 1);
    }

    #[test]
    fn two_computers_and_one_file() {
        let d = Dir::new("two");
        d.ok("one", &["accounts", "add", "Bank"]);
        d.ok("one", &["tx", "add", "--account", "Bank", "--amount=-1", "--payee", "first"]);
        complete(&d.file());
        assert_eq!(d.names(), ["books.db".to_string()].into());
        assert!(d.ok("two", &["tx", "list"]).0.contains("first"));
        d.ok("two", &["tx", "add", "--account", "Bank", "--amount=-2", "--payee", "second"]);
        assert!(d.ok("one", &["tx", "list"]).0.contains("second"));
        let status = d.ok("one", &["data", "status"]).0;
        assert!(status.contains("private copy, in OneDrive") && status.contains("saved"), "{status}");
        // The folder goes away, and both change.
        let aside = d.0.join("aside");
        let bytes = std::fs::read(d.file()).unwrap();
        std::fs::rename(d.folder(), &aside).unwrap();
        let (_, notice) = d.ok("one", &["tx", "add", "--account", "Bank", "--amount=-3", "--payee", "mine"]);
        assert!(notice.contains("safe on this computer"), "{notice}");
        assert!(d.ok("one", &["tx", "list"]).0.contains("mine"));
        std::fs::rename(&aside, d.folder()).unwrap();
        assert_eq!(std::fs::read(d.file()).unwrap(), bytes);
        d.ok("two", &["tx", "add", "--account", "Bank", "--amount=-4", "--payee", "theirs"]);
        let (list, notice) = d.ok("one", &["tx", "list"]);
        assert!(notice.contains("Nothing was lost") && list.contains("theirs") && !list.contains("mine"), "{notice}");
        let conflict = d.names().into_iter().find(|n| n.contains("(conflict one ")).unwrap();
        let conflict = d.folder().join(conflict);
        let compared = d.ok("one", &["data", "compare", conflict.to_str().unwrap()]).0;
        assert!(compared.contains("mine") && compared.contains("theirs") && !compared.contains("second"), "{compared}");
        let json: serde_json::Value = serde_json::from_str(&d.ok("one", &["--json", "data", "status"]).0).unwrap();
        assert_eq!((&json["saved"], json["conflicts"].as_array().unwrap().len()), (&true.into(), 1));
        d.ok("one", &["data", "dismiss"]);
        assert!(!d.ok("one", &["data", "status"]).0.contains("Conflict file"));
        complete(&d.file());
        assert_eq!(d.names().len(), 2);
    }

    #[test]
    fn the_user_decides_how_a_file_is_used() {
        let d = Dir::new("mode");
        d.ok("one", &["--direct", "accounts", "add", "Bank"]);
        assert!(!d.0.join("one").join("CashCraft").join("copies").exists());
        // Asking makes nothing.
        let other = d.folder().join("absent.db");
        let out = d.run("one", &["--db", other.to_str().unwrap(), "data", "status"]);
        assert!(out.status.success() && !other.exists());
        assert!(!d.run("one", &["--direct", "--shared", "tools"]).status.success());
        d.ok("one", &["data", "mode", "direct"]);
        assert!(d.ok("one", &["data", "status"]).0.contains("Used directly, in OneDrive"));
        let file = d.file();
        let bad = d.run("one", &["--db", file.to_str().unwrap(), "data", "mode", "x"]);
        assert!(!bad.status.success());
        // From a local file to the folder, and it is the one used from then on.
        let local = d.0.join("local.db");
        let on = |args: &[&str]| d.run("one", &[&["--db", local.to_str().unwrap()], args].concat());
        assert!(on(&["accounts", "add", "Savings"]).status.success());
        let keep = ["data", "keep", d.folder().to_str().unwrap(), "--name", "kept.db", "--move"].map(String::from);
        let keep: Vec<&str> = keep.iter().map(String::as_str).collect();
        let out = on(&keep);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(!local.exists() && d.names().contains("kept.db") && !d.names().contains("kept.db-wal"));
        let list = d.run("one", &["accounts", "list"]);
        assert!(String::from_utf8_lossy(&list.stdout).contains("Savings"));
        complete(&d.folder().join("kept.db"));
        // Removed: nothing is made in its place without saying so.
        std::fs::remove_file(d.folder().join("kept.db")).unwrap();
        let list = d.run("one", &["accounts", "list"]);
        assert!(list.status.success() && !d.names().contains("kept.db"));
        assert!(d.run("one", &["data", "put-back"]).status.success() && d.names().contains("kept.db"));
        assert!(d.run("one", &["data", "use"]).status.success());
        assert!(!d.run("one", &["data", "use", "nowhere.db"]).status.success());
    }

    #[test]
    fn mcp_carries_each_call() {
        let d = Dir::new("mcp");
        d.ok("one", &["accounts", "add", "Bank"]);
        let root = d.0.to_str().unwrap().to_string();
        let file = d.file();
        let args =
            ["--db", file.to_str().unwrap(), "mcp", "--automation-read-root", &root, "--automation-write-root", &root];
        let mut child = d.command("two", &args).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut lines = std::io::BufRead::lines(std::io::BufReader::new(child.stdout.take().unwrap()));
        let mut call = |tool: &str, arguments: serde_json::Value| -> serde_json::Value {
            let params = serde_json::json!({"name": tool, "arguments": arguments});
            let msg = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params});
            writeln!(stdin, "{msg}").unwrap();
            serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap()
        };
        let tx = |payee: &str| serde_json::json!({"account": "Bank", "amount": -1, "payee": payee});
        assert_eq!(call("add_transaction", tx("from mcp"))["result"]["isError"], false);
        assert!(d.ok("one", &["tx", "list"]).0.contains("from mcp"));
        d.ok("one", &["tx", "add", "--account", "Bank", "--amount=-1", "--payee", "from the terminal"]);
        // The server has been running since before.
        let seen = call("list_transactions", serde_json::json!({}));
        assert!(
            seen.to_string().contains("from the terminal")
                && seen["result"]["structuredContent"]["data_file"].is_null()
        );
        let aside = d.0.join("aside");
        std::fs::rename(d.folder(), &aside).unwrap();
        let waiting = call("add_transaction", tx("later"));
        assert!(
            waiting["result"]["structuredContent"]["data_file"][0].as_str().unwrap().contains("safe on this computer")
        );
        std::fs::rename(&aside, d.folder()).unwrap();
        call("list_accounts", serde_json::json!({}));
        assert!(d.ok("one", &["tx", "list"]).0.contains("later"));
        drop(call);
        drop(stdin);
        child.wait().unwrap();
        assert_eq!(d.names(), ["books.db".to_string()].into());
    }

    /// Programs that are really killed, at any moment.
    #[test]
    fn killed_programs_leave_a_complete_file() {
        let d = Dir::new("kill");
        d.ok("one", &["accounts", "add", "Bank"]);
        let file = d.file();
        for i in 0..25u64 {
            let payee = format!("p{i}");
            let args =
                ["--db", file.to_str().unwrap(), "tx", "add", "--account", "Bank", "--amount=-1", "--payee", &payee];
            let mut child = d.command("one", &args).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
            std::thread::sleep(Duration::from_millis(i * 9 % 160));
            let _ = child.kill();
            child.wait().unwrap();
            complete(&file);
        }
        let (list, notice) = d.ok("one", &["--json", "tx", "list", "--limit", "100"]);
        assert_eq!(notice, "");
        // What is in the copy is in the file: another computer sees the same.
        assert_eq!(d.ok("two", &["--json", "tx", "list", "--limit", "100"]).0, list);
        assert_eq!(d.names(), ["books.db".to_string()].into());
        complete(&file);
    }
}
