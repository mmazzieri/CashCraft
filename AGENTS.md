# Rules for agents

This project follows the shared rules for Craft repositories:
https://github.com/storytold/craft-contrib/blob/main/standards/app-agents.md
(clean room, licences, tests, pull requests). They override your tool's defaults.

## This repository

CashCraft is a Rust personal finance app (egui 0.37) for macOS, Windows and Linux.

- Layout: one crate. `src/lib.rs` holds the modules; `src/main.rs` is the desktop app
  (`cashcraft`), `src/bin/cashcraft-cli.rs` the command line and MCP server.
  - `db.rs`: SQLite model (amounts are `i64` cents, negative = money out).
  - `importer.rs`: Italian bank CSV detection and parsing.
  - `ops.rs`: every operation as JSON in/JSON out, shared by CLI and MCP; `Roots` confines MCP file access.
  - `cli.rs`, `mcp.rs`: command line and MCP.
  - `gui.rs` (state, style, widgets, charts), `views.rs` (screens), `dialogs.rs` (dialogs), `icon.rs` (app icon).
- Checks: `cargo fmt --all --check`, `cargo clippy --all-targets`, `cargo test`, `cargo build --bins`.
- Screenshot: `cargo run -- --screenshot shot.png` (with `CASHCRAFT_START_VIEW=movimenti|report|budget|…`
  for another screen); with no data file it uses in-memory sample data.
- `unsafe`: none.
- Assets: `ATTRIBUTION.md`, a row per asset. The icon is drawn by `src/icon.rs`.
- Releases: `docs/releasing.md`; ArtCraft Launcher fit and gaps: `docs/artcraft-launcher.md`.
- Known gaps: Italian-only UI without a translation system, colours not yet theme tokens,
  accessibility not reviewed.
