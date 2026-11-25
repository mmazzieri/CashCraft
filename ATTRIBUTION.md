# Attribution

## Provenance of the design

CashCraft was written by a coding agent from its author's description of the features expected from a personal finance manager. Nothing inside another product (its code, files, pictures or text) was read, run or decompiled. Bank CSV support is based on the column names commonly found in exported statements, not on any bank's software.

| File | Source | License |
|---|---|---|
| `assets/app-icon/` | Our own work: drawn by `crates/icon` (`cargo xtask icons`) | MIT OR Apache-2.0 |
| `crates/engine/src/demo.rs` sample accounts and transactions | Our own invented data (no real people, banks or brands) | MIT OR Apache-2.0 |
| `crates/engine/src/db.rs` default categories and rules | Our own wording | MIT OR Apache-2.0 |
| Statement fixtures in the tests | Our own invented examples | MIT OR Apache-2.0 |
| `packaging/` | Adapted from `storytold/craft-launcher`, as the headers say | MIT OR Apache-2.0 |

## Third-party material

Rust crates are listed with their licenses by `cargo deny list`; `cargo deny check licenses` keeps them on the allowlist in `deny.toml`. Direct dependencies:

| Crate | Version | License |
|---|---|---|
| egui, eframe (and egui-wgpu, egui-winit) | 0.37 | MIT OR Apache-2.0 |
| rfd | 0.17.2 | MIT |
| rusqlite, with SQLite bundled | 0.37 | MIT; SQLite is public domain |
| craft-xls (storytold/craft-libs) | 0.1.0, pinned commit | MIT OR Apache-2.0 |
| csv | 1.3 | Unlicense/MIT |
| encoding_rs | 0.8 | (Apache-2.0 OR MIT) AND BSD-3-Clause |
| image (PNG and ICO only) | 0.25 | MIT OR Apache-2.0 |
| sys-locale | 0.3 | MIT OR Apache-2.0 |
| clap | 4 | MIT OR Apache-2.0 |
| anyhow, chrono, serde, serde_json | 1, 0.4, 1, 1 | MIT OR Apache-2.0 |

Fonts bundled in the app through `epaint_default_fonts` 0.37 (egui's `default_fonts` feature):

| Font | License |
|---|---|
| Ubuntu Light | Ubuntu Font Licence 1.0 (`Ubuntu-font-1.0`) |
| Hack Regular | MIT, with Bitstream Vera parts |
| Noto Emoji Regular | SIL Open Font License 1.1 (`OFL-1.1`) |
| emoji-icon-font | MIT |
| egui-icons (subset of emoji-icon-font) | MIT |

The license texts of the fonts and of the encoding_rs data are collected, unchanged, in [`THIRD-PARTY-LICENSES.txt`](THIRD-PARTY-LICENSES.txt), which every release package carries together with this file, `NOTICE` and the two license texts.
