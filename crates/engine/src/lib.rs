//! CashCraft without its window: the data file, the import of bank statements and the operations
//! shared by the desktop app, the command line and the MCP server.
#![forbid(unsafe_code)]

pub mod db;
pub mod demo;
pub mod i18n;
pub mod importer;
pub mod money;
pub mod ops;
mod paths;
