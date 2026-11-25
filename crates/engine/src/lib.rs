//! CashCraft without its window: the data file, the import of statements and the operations
//! shared by the desktop app, the command line and the MCP server.
#![forbid(unsafe_code)]

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
mod paths;
pub mod pdf;
pub mod rules;
pub mod store;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod transfers;
mod xls;
