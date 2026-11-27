//! CashCraft without its window: the data file, the import of statements and the operations
//! shared by the desktop app, the command line and the MCP server.
#![forbid(unsafe_code)]

pub mod bank_pdf;
mod card_pdf;
pub mod cards;
pub mod db;
pub mod demo;
pub mod duplicates;
pub mod i18n;
pub mod importer;
pub mod layouts;
pub mod ledger;
mod migrate;
pub mod money;
pub mod ofx;
pub mod ops;
pub mod paths;
pub mod pdf;
pub mod reconcile;
pub mod rules;
pub mod store;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod transfers;
mod xls;

/// The public keys of CashCraft's release signature, in base64 (32 bytes each): the one the release workflow signs
/// `SHA256SUMS.txt` with, and a spare kept offline. A copy that updates itself takes nothing that neither signed;
/// while both are empty, new versions are told of and never downloaded (`docs/updates.md`).
pub const RELEASE_KEYS: [&str; 2] = ["", ""];
