//! CashCraft: personal finance with Italian bank CSV import.
//!
//! Two programs share this library:
//! - `cashcraft`: the desktop window;
//! - `cashcraft-cli`: the command line and the MCP server for AI agents.

pub mod cli;
pub mod db;
pub mod demo;
mod dialogs;
pub mod gui;
pub mod icon;
pub mod importer;
pub mod mcp;
pub mod money;
pub mod ops;
mod views;
