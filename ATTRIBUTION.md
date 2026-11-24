# Attribution

Every asset and fixture in this repository, with its provenance and licence.

| File | Source | Licence |
|---|---|---|
| `assets/icons/*.png`, `assets/icons/icon.ico` | Our own work: drawn by code in `src/icon.rs` (`cashcraft-cli icon <file> --size <n>`) | MIT OR Apache-2.0 |
| `src/demo.rs` sample accounts and transactions | Our own invented sample data (no real people, banks or brands) | MIT OR Apache-2.0 |
| `src/db.rs` default categories and rules | Our own wording | MIT OR Apache-2.0 |
| CSV fixtures in `src/importer.rs` tests | Our own invented examples of common Italian bank export layouts | MIT OR Apache-2.0 |
| `LICENSE-APACHE` | Standard text of the Apache License 2.0 | Apache-2.0 |

## Clean room

CashCraft was written from a description of the features people expect from a personal
finance manager. Nothing inside another product (its code, files, pictures or text) was read,
run or decompiled while writing it. Bank CSV support is based on the column names commonly
found in exported statements, not on any bank's software.

## Dependencies

Rust crates are listed in `Cargo.lock`; their licences are all permissive (MIT, Apache-2.0,
BSD, Unlicense, Zlib, Unicode-3.0). Run `cargo tree` or `cargo about` to list them.
