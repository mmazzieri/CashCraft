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

    /// A server that ends with the test.
    struct Server(std::process::Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
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
        assert!(seen.to_string().contains("from the terminal"));
        let told = seen["result"]["structuredContent"]["data_file"].to_string();
        assert!(told.contains("changed on another computer"), "{told}");
        let again = call("list_transactions", serde_json::json!({}));
        assert!(again["result"]["structuredContent"]["data_file"].is_null());
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

    /// A folder that is not known for a synced one, which the user chose.
    #[test]
    fn a_file_that_was_put_in_a_folder_is_never_opened_in_place() {
        let d = Dir::new("plain");
        let local = d.0.join("local.db");
        let folder = d.0.join("Sync");
        std::fs::create_dir_all(&folder).unwrap();
        let local = local.to_str().unwrap();
        assert!(d.run("one", &["--db", local, "accounts", "add", "Savings"]).status.success());
        assert!(d.run("one", &["--db", local, "data", "keep", folder.to_str().unwrap()]).status.success());
        let status = String::from_utf8(d.run("one", &["data", "status"]).stdout).unwrap();
        assert!(status.contains("Used through a private copy"), "{status}");
        assert!(d.run("one", &["accounts", "add", "Other"]).status.success());
        let names: Vec<_> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["local.db"]);
        complete(&folder.join("local.db"));
        assert!(std::path::Path::new(local).exists());
    }

    /// A program that stays open, as the window does, and others that start meanwhile on the same computer.
    #[test]
    fn programs_of_one_computer_work_at_once() {
        let d = Dir::new("same");
        d.ok("one", &["accounts", "add", "Bank"]);
        let file = d.file();
        let root = d.0.to_str().unwrap().to_string();
        let args =
            ["--db", file.to_str().unwrap(), "mcp", "--automation-read-root", &root, "--automation-write-root", &root];
        let mut server = d.command("one", &args).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut stdin = server.stdin.take().unwrap();
        let mut lines = std::io::BufRead::lines(std::io::BufReader::new(server.stdout.take().unwrap()));
        let mut expected = BTreeSet::new();
        for round in 0..3 {
            let mut children = Vec::new();
            for i in 0..5 {
                let payee = format!("cli-{round}-{i}");
                let file = file.to_str().unwrap();
                let args = ["--db", file, "tx", "add", "--account", "Bank", "--amount=-1", "--payee", &payee];
                children.push(d.command("one", &args).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap());
                expected.insert(payee);
            }
            for i in 0..3 {
                let payee = format!("mcp-{round}-{i}");
                let arguments = serde_json::json!({"account": "Bank", "amount": -1, "payee": payee});
                let params = serde_json::json!({"name": "add_transaction", "arguments": arguments});
                let msg = serde_json::json!({"jsonrpc": "2.0", "id": i, "method": "tools/call", "params": params});
                writeln!(stdin, "{msg}").unwrap();
                let reply: serde_json::Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
                assert_eq!(reply["result"]["isError"], false, "{reply}");
                expected.insert(payee);
            }
            for c in children {
                let out = c.wait_with_output().unwrap();
                assert!(out.status.success() && out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
            }
        }
        drop(stdin);
        server.wait().unwrap();
        complete(&file);
        assert_eq!(d.names(), ["books.db".to_string()].into());
        // From the file alone.
        let list = d.ok("two", &["--json", "tx", "list", "--limit", "100"]).0;
        assert!(expected.iter().all(|p| list.contains(&format!("\"{p}\""))), "{list}");
    }

    #[test]
    fn what_is_run_unattended_goes_on_after_others_changed_the_file() {
        let d = Dir::new("ids");
        d.ok("one", &["accounts", "add", "Bank"]);
        let csv = d.0.join("a.csv");
        std::fs::write(&csv, "Data;Descrizione;Importo\n05/01/2026;imported;-1,20\n").unwrap();
        let add = |c: &str, payee: &str| d.ok(c, &["tx", "add", "--account", "Bank", "--amount=-1", "--payee", payee]);
        for i in 0..3 {
            add("one", &format!("one-{i}"));
            let (_, notice) = add("two", &format!("two-{i}"));
            assert!(notice.contains("changed on another computer") || i == 0, "{notice}");
        }
        add("one", "more");
        d.ok("two", &["import", csv.to_str().unwrap(), "--account", "Bank"]);
        let list = |c: &str| -> Vec<(String, i64)> {
            let v: serde_json::Value = serde_json::from_str(&d.ok(c, &["--json", "tx", "list"]).0).unwrap();
            let all = v["transactions"].as_array().unwrap().iter();
            all.map(|t| (t["payee"].as_str().unwrap().to_string(), t["id"].as_i64().unwrap())).collect()
        };
        let before = list("one");
        assert_eq!(before.len(), 8);
        // Ids of a list of before: the same thing or nothing.
        let id = |payee: &str| before.iter().find(|t| t.0 == payee).unwrap().1.to_string();
        d.ok("two", &["tx", "delete", &id("one-0")]);
        add("two", "RENT");
        let file = d.file();
        let gone = d.run("one", &["--db", file.to_str().unwrap(), "tx", "delete", &id("one-0")]);
        assert!(!gone.status.success() && String::from_utf8_lossy(&gone.stderr).contains("not found"));
        d.ok("one", &["tx", "delete", &id("one-1")]);
        let after: BTreeSet<String> = list("two").into_iter().map(|t| t.0).collect();
        assert!(after.contains("RENT") && !after.contains("one-1") && !after.contains("one-0") && after.len() == 7);
        assert_eq!(d.names(), ["books.db".to_string()].into());
    }

    #[test]
    fn a_server_is_told_where_its_file_went() {
        let d = Dir::new("agent");
        let root = d.0.to_str().unwrap().to_string();
        let local = d.0.join("local.db");
        let at = local.to_str().unwrap();
        assert!(d.run("two", &["--db", at, "accounts", "add", "Bank"]).status.success());
        let args = ["--db", at, "mcp", "--automation-read-root", &root, "--automation-write-root", &root];
        let mut server = Server(d.command("two", &args).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap());
        let mut stdin = server.0.stdin.take().unwrap();
        let mut lines = std::io::BufRead::lines(std::io::BufReader::new(server.0.stdout.take().unwrap()));
        let mut call = |tool: &str, arguments: serde_json::Value| -> serde_json::Value {
            let params = serde_json::json!({"name": tool, "arguments": arguments});
            let msg = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params});
            writeln!(stdin, "{msg}").unwrap();
            serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap()
        };
        assert_eq!(call("list_accounts", serde_json::json!({}))["result"]["isError"], false);
        let folder = d.folder();
        let keep = ["--db", at, "data", "keep", folder.to_str().unwrap()];
        assert!(d.run("two", &keep).status.success());
        let late = call("create_account", serde_json::json!({"name": "Late"}));
        assert!(late.to_string().contains("was moved to"), "{late}");
    }

    #[test]
    fn a_log_is_not_taken_from_a_program_that_is_open() {
        let d = Dir::new("open");
        let root = d.0.to_str().unwrap().to_string();
        let file = d.file();
        let at = file.to_str().unwrap();
        let args = ["--db", at, "--direct", "mcp", "--automation-read-root", &root, "--automation-write-root", &root];
        let mut server = Server(d.command("one", &args).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap());
        let params = serde_json::json!({"name": "create_account", "arguments": {"name": "Bank"}});
        let msg = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params});
        let mut stdin = server.0.stdin.take().unwrap();
        writeln!(stdin, "{msg}").unwrap();
        let mut line = String::new();
        let mut out = std::io::BufReader::new(server.0.stdout.take().unwrap());
        std::io::BufRead::read_line(&mut out, &mut line).unwrap();
        assert!(d.names().contains("books.db-wal"));
        let before = d.names();
        for args in [&["tx", "list"][..], &["data", "recover", "--nobody-has-it-open"]] {
            let out = d.run("two", &[&["--db", at], args].concat());
            assert!(!out.status.success(), "{args:?}");
            assert_eq!(d.names(), before);
        }
        drop(stdin);
        server.0.wait().unwrap();
        // It brought its log in and left.
        assert!(d.ok("two", &["accounts", "list"]).0.contains("Bank"));
    }

    /// A reader that stops reading, as `less` does, keeps nobody waiting.
    #[test]
    fn nothing_is_held_while_it_is_printed() {
        let d = Dir::new("pipe");
        d.ok("one", &["accounts", "add", "Bank"]);
        let csv = d.0.join("big.csv");
        let rows: String = (0..3000).map(|i| format!("05/01/2026;payee number {i} of many;-{i},20\n")).collect();
        std::fs::write(&csv, format!("Data;Descrizione;Importo\n{rows}")).unwrap();
        d.ok("one", &["import", csv.to_str().unwrap(), "--account", "Bank"]);
        d.ok("two", &["tx", "list"]);
        let file = d.file();
        let args = ["--db", file.to_str().unwrap(), "--json", "tx", "list", "--limit", "3000"];
        let child = d.command("one", &args).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        let mut stuck = Server(child);
        // It has printed, and the pipe is full.
        let mut first = [0; 1];
        std::io::Read::read_exact(stuck.0.stdout.as_mut().unwrap(), &mut first).unwrap();
        d.ok("two", &["tx", "add", "--account", "Bank", "--amount=-1", "--payee", "theirs"]);
        assert!(d.ok("one", &["tx", "list", "--search", "theirs"]).0.contains("theirs"));
        assert!(stuck.0.try_wait().unwrap().is_none());
    }

    #[test]
    fn a_log_waits_for_the_user() {
        let d = Dir::new("log");
        d.ok("one", &["accounts", "add", "Bank"]);
        d.ok("one", &["tx", "add", "--account", "Bank", "--amount=-1", "--payee", "first"]);
        let before = std::fs::read(d.file()).unwrap();
        std::fs::write(d.folder().join("books.db-wal"), "").unwrap();
        let (_, notice) = d.ok("one", &["tx", "add", "--account", "Bank", "--amount=-1", "--payee", "second"]);
        assert!(notice.contains("data recover") && std::fs::read(d.file()).unwrap() == before, "{notice}");
        let refused = d.run("one", &["--db", d.file().to_str().unwrap(), "data", "recover"]);
        assert!(!refused.status.success() && d.folder().join("books.db-wal").exists());
        d.ok("one", &["data", "recover", "--nobody-has-it-open"]);
        assert_eq!(d.names(), ["books.db".to_string()].into());
        d.ok("one", &["tx", "list"]);
        assert!(d.ok("two", &["tx", "list"]).0.contains("second"));
        // A folder that is known for synced no more, as when a variable is gone.
        let said = d.run("three", &["--db", d.file().to_str().unwrap(), "tx", "list"]);
        assert!(String::from_utf8_lossy(&said.stderr).contains("private copy, as it is in OneDrive"));
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
