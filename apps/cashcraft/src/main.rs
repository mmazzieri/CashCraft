//! CashCraft desktop app. Terminal and MCP use live in `cashcraft-cli`.
#![forbid(unsafe_code)]
// No console window behind the app on Windows (release builds).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use cashcraft_engine::db;
use cashcraft_engine::i18n::trf;
use cashcraft_ui_egui::gui;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "cashcraft",
    version,
    about = "CashCraft — personal finance",
    after_help = "For the terminal and AI agents (MCP) use `cashcraft-cli`. Default data file: CashCraft/cashcraft.db in your user data folder (or the variable CASHCRAFT_DB). Language: CASHCRAFT_LANG=en or it, else the one chosen in the window, else the system's."
)]
struct Args {
    /// Data file to use
    #[arg(long)]
    db: Option<PathBuf>,
    /// Open the window, draw it, save a PNG picture of it and quit
    #[arg(long)]
    screenshot: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();
    let path = args.db.unwrap_or_else(db::default_db_path);
    if let Err(e) = gui::run(path, args.screenshot) {
        eprintln!("{}", trf("Error: {error}", &[("error", &format_args!("{e:#}"))]));
        std::process::exit(1);
    }
}
