# Attribution

## Provenance of the design

CashCraft was written by a coding agent from its author's description of the features expected from a personal finance manager. Nothing inside another product (its code, files, pictures or text) was read, run or decompiled. Bank CSV support is based on the column names commonly found in exported statements, not on any bank's software. The OFX reader was written from the public OFX specification alone. The reader of card statements is based on the layout of one statement (positions and headings, with its contents hidden).

| File | Source | License |
|---|---|---|
| `assets/app-icon/cashcraft-source.png` (an engraved squirrel holding an acorn, on an ochre tile) | Made by Mauro Mazzieri with Google Gemini (image generation, November 2026) for CashCraft | MIT OR Apache-2.0 |
| The other files in `assets/app-icon/` and the phone apps' icons in `mobile/` | Scaled from it by `crates/icon` (`cargo xtask icons`) | MIT OR Apache-2.0 |
| `crates/engine/src/demo.rs` sample accounts and transactions | Our own invented data (no real people, banks or brands) | MIT OR Apache-2.0 |
| `crates/engine/src/db.rs` default categories and rules | Our own wording | MIT OR Apache-2.0 |
| Statement fixtures in the tests | Our own invented examples | MIT OR Apache-2.0 |
| `packaging/` | Adapted from `storytold/craft-launcher`, as the headers say | MIT OR Apache-2.0 |
| `crates/update` (checking, downloading, verifying and installing new versions) | Adapted from MailCraft's own updater ([UCS-Care-Group/mailcraft](https://github.com/UCS-Care-Group/mailcraft): `crates/model/src/update.rs`, `crates/net/src/update.rs`, `crates/net/src/update_download.rs`, `apps/mailcraft/src/install.rs`, `apps/mailcraft/src/self_update.rs`), Copyright MailCraft contributors, as the headers say | MIT OR Apache-2.0 |

## Third-party material

Rust crates are listed with their licenses by `cargo deny list`; `cargo deny check licenses` keeps them on the allowlist in `deny.toml`. Direct dependencies:

| Crate | Version | License |
|---|---|---|
| egui, eframe (and egui-wgpu, egui-winit) | 0.37 | MIT OR Apache-2.0 |
| rfd | 0.17.2 | MIT |
| winresource (build only, Windows: the icon inside the executable) | 0.1 | MIT |
| jni (Android only) | 0.21 | MIT OR Apache-2.0 |
| AndroidX: Core, Activity, Lifecycle, Compose with Material 3 and its core icons (Gradle, not in this repository) | | Apache-2.0 |
| Android Gradle plugin, Kotlin and Compose Gradle plugins (build only, not in this repository) | 8.10, 2.1 | Apache-2.0 |
| `mobile/android/gradlew`, `gradlew.bat`, `gradle/wrapper/` (Gradle wrapper) | 8.14 | Apache-2.0 |
| rusqlite, with SQLite bundled | 0.37 | MIT; SQLite is public domain |
| ureq (with rustls and ring), rustls-native-certs (the system's own root certificates; none bundled) | 3.5, 0.8 | MIT OR Apache-2.0; rustls-native-certs: Apache-2.0 OR ISC OR MIT; ring: Apache-2.0 AND ISC |
| base64 | 0.22 | MIT OR Apache-2.0 |
| winreg (Windows only) | 0.55 | MIT |
| craft-xls (storytold/craft-libs) | 0.1.0, pinned commit | MIT OR Apache-2.0 |
| hayro-interpret, hayro-syntax (text of PDF files; no fonts or CMaps bundled; they use `unsafe` code of their own) | 0.7.0, 0.7.2, pinned | MIT OR Apache-2.0 |
| kurbo | 0.13 | MIT OR Apache-2.0 |
| epaint_default_fonts, flate2 (tests only) | 0.37, 1 | MIT OR Apache-2.0; fonts as below |
| regex (with regex-automata, regex-syntax, aho-corasick; the last one Unlicense OR MIT) | 1.13 | MIT OR Apache-2.0 |
| sha2 | 0.10 | MIT OR Apache-2.0 |
| csv | 1.3 | Unlicense/MIT |
| encoding_rs | 0.8 | (Apache-2.0 OR MIT) AND BSD-3-Clause |
| image (PNG and ICO only) | 0.25 | MIT OR Apache-2.0 |
| sys-locale | 0.3 | MIT OR Apache-2.0 |
| clap | 4 | MIT OR Apache-2.0 |
| anyhow, chrono, serde, serde_json | 1, 0.4, 1, 1 | MIT OR Apache-2.0 |

The PDF crates bring in zune-jpeg, moxcms, skrifa, read-fonts, font-types, phf, fearless_simd (MIT, Apache-2.0, Zlib or BSD-3-Clause, at choice) and yoke, zerofrom (Unicode-3.0).

Fonts bundled in the app through `epaint_default_fonts` 0.37 (egui's `default_fonts` feature):

| Font | License |
|---|---|
| Ubuntu Light | Ubuntu Font Licence 1.0 (`Ubuntu-font-1.0`) |
| Hack Regular | MIT, with Bitstream Vera parts |
| Noto Emoji Regular | SIL Open Font License 1.1 (`OFL-1.1`) |
| emoji-icon-font | MIT |
| egui-icons (subset of emoji-icon-font) | MIT |

The license texts of the fonts, of the encoding_rs data and of MailCraft (whose updater `crates/update` is adapted from) are collected, unchanged, in [`THIRD-PARTY-LICENSES.txt`](THIRD-PARTY-LICENSES.txt), which every release package carries together with this file, `NOTICE` and the two license texts.
