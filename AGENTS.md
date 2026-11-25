# AGENTS.md

Notes for people and coding agents working on CashCraft. The shared rules for Craft apps apply: https://github.com/storytold/craft-contrib/blob/main/standards/app-agents.md.

- Layout: `crates/engine` (data file, bank statement import, operations; no window), `crates/ui-egui` (interface), `crates/icon` (the icon, drawn by code), `apps/cashcraft` (desktop shell), `apps/cashcraft-cli` (command line and MCP server), `crates/ffi` and `mobile/` (the phone apps: [docs/mobile.md](docs/mobile.md)), `xtask/`, `packaging/`.
- **Amounts are exact.** `Cents` is an integer; no amount goes through floating point. Negative is money going out.
- **Debits equal credits.** Every amount is a posting of the ledger ([docs/ledger.md](docs/ledger.md)); only `Db::save_entry` writes postings. A test that changes the books ends with `assert_sound`.
- **All or nothing.** A change made of several writes runs inside `Db::atomically`.
- What the bank wrote stays in `original_payee` and `original_memo`; import keys and comparisons never use what a rule wrote ([docs/rules.md](docs/rules.md)).
- The data file format is numbered in `PRAGMA user_version` (`SCHEMA_VERSION`). A change to the tables raises it, migrates older files and comes with a test that opens one. A file from a newer version is refused, never rewritten.
- A data file in a synced folder is used through `store::Shared` ([docs/data-file.md](docs/data-file.md)): no code opens it by SQLite, and a shared file that is not the one that was read is never replaced. What reads and then changes the copy runs under its `hold`. A row of a table with ids is inserted with `Db::new_id`. A change to the protocol adds its steps to the fault tests of `store`, and keeps to what the phone apps expect.
- Every operation is in `ops.rs` as JSON in, JSON out, shared by the command line and MCP; `cashcraft-cli data` alone is about the file and not the books. `ops::Roots` confines MCP file access; the MCP server refuses to start without both roots.
- Statements and MCP parameters are hostile input: capped sizes and errors, never a panic. A crash fix adds its input to the sweeps in the tests.
- PDF files are read by the program started again (`pdf::answer_if_child`, the first line of each `main`): the PDF library can overflow the stack or never end. New binaries that import statements call it too.
- A failed read is shown to the user (`or_report`), never drawn as "no data".
- Text is written in English inside `tr`, `trf` or `trn` with a literal key, and its Italian goes in `crates/engine/src/i18n/it.tsv`; a test scans the sources for both. Command help and MCP text are English only. Tests pick a language with `with_lang`, never `set_lang`.
- Checks: `cargo xtask ci` (fmt, clippy with `-D warnings`, tests). `unsafe` is forbidden, except in `crates/ffi/src/android.rs` and `crates/ffi/src/c.rs`; `unwrap`, `expect`, `panic`, `todo`, `unreachable` and `unimplemented` are denied outside tests. No `allow` attributes.
- Licenses: dependencies and assets only under the license allowlist of the Craft apps, checked by `cargo deny check` (no MPL; no GPL, LGPL or AGPL code, not even read). Record every addition in `ATTRIBUTION.md`.
- Clean room: no code, text, icons or assets from proprietary products. Do not name other products in the interface.
- Screenshot: `cargo run -p cashcraft -- --screenshot shot.png` (`CASHCRAFT_START_VIEW=register|reports|budget|…` for another screen, `CASHCRAFT_THEME=light|dark` for a theme); with no data file it uses sample data in memory.
- Icons: `cargo xtask icons` after a change to `crates/icon`.
