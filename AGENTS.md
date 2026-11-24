# AGENTS.md

Notes for people and coding agents working on CashCraft. The shared rules for Craft apps apply: https://github.com/storytold/craft-contrib/blob/main/standards/app-agents.md.

- Layout: `crates/engine` (data file, bank statement import, operations; no window), `crates/ui-egui` (interface), `crates/icon` (the icon, drawn by code), `apps/cashcraft` (desktop shell), `apps/cashcraft-cli` (command line and MCP server), `xtask/`, `packaging/`.
- **Amounts are exact.** `Cents` is an integer; no amount goes through floating point. Negative is money going out.
- **All or nothing.** A change made of several writes runs inside `Db::atomically`.
- The data file format is numbered in `PRAGMA user_version` (`SCHEMA_VERSION`). A change to the tables raises it, migrates older files and comes with a test that opens one. A file from a newer version is refused, never rewritten.
- Every operation is in `ops.rs` as JSON in, JSON out, shared by the command line and MCP. `ops::Roots` confines MCP file access; the MCP server refuses to start without both roots.
- Statements and MCP parameters are hostile input: capped sizes and errors, never a panic. A crash fix adds its input to the sweeps in the tests.
- A failed read is shown to the user (`or_report`), never drawn as "no data".
- Checks: `cargo xtask ci` (fmt, clippy with `-D warnings`, tests). `unsafe` is forbidden; `unwrap`, `expect`, `panic`, `todo`, `unreachable` and `unimplemented` are denied outside tests. No `allow` attributes.
- Licenses: dependencies and assets only under the license allowlist of the Craft apps, checked by `cargo deny check` (no MPL; no GPL, LGPL or AGPL code, not even read). Record every addition in `ATTRIBUTION.md`.
- Clean room: no code, text, icons or assets from proprietary products. Do not name other products in the interface.
- Screenshot: `cargo run -p cashcraft -- --screenshot shot.png` (`CASHCRAFT_START_VIEW=movimenti|report|budget|…` for another screen); with no data file it uses sample data in memory.
- Icons: `cargo xtask icons` after a change to `crates/icon`.
