//! CashCraft without its window: the data file, the import of statements and the operations
//! shared by the desktop app, the command line and the MCP server.
#![forbid(unsafe_code)]

mod card_pdf;
pub mod cards;
pub mod db;
pub mod demo;
pub mod i18n;
pub mod importer;
pub mod ledger;
mod migrate;
pub mod money;
pub mod ops;
mod paths;
pub mod pdf;
pub mod transfers;
mod xls;
