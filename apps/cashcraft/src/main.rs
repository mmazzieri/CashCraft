//! CashCraft desktop app. Terminal and MCP use live in `cashcraft-cli`.
#![forbid(unsafe_code)]
// No console window behind the app on Windows (release builds).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use cashcraft_engine::i18n::trf;
use cashcraft_engine::store::{Home, Use};
use cashcraft_ui_egui::gui;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "cashcraft",
    version,
    about = "CashCraft — personal finance",
    after_help = "For the terminal and AI agents (MCP) use `cashcraft-cli`. Default data file: CashCraft/cashcraft.db in your user data folder (or the variable CASHCRAFT_DB, or the file chosen last). Language: CASHCRAFT_LANG=en or it, else the one chosen in the window, else the system's."
)]
struct Args {
    /// Data file to use
    #[arg(long)]
    db: Option<PathBuf>,
    /// Use the data file where it lies, even in a synced folder
    #[arg(long, conflicts_with = "shared")]
    direct: bool,
    /// Use the data file through a private copy, as in a synced folder
    #[arg(long)]
    shared: bool,
    /// Open the window, draw it, save a PNG picture of it and quit
    #[arg(long)]
    screenshot: Option<PathBuf>,
}

fn main() {
    if cashcraft_engine::pdf::answer_if_child() {
        return;
    }
    // The copy of the program that installs a new version once this one has closed.
    if let Some(code) = cashcraft_ui_egui::updates::finish_update_if_asked() {
        std::process::exit(code);
    }
    let args = Args::parse();
    let asked = match (args.direct, args.shared) {
        (true, _) => Use::Direct,
        (_, true) => Use::Shared,
        _ => Use::Auto,
    };
    let home = Home::user();
    let file = home.pick(args.db);
    if let Err(e) = gui::run(home, file, asked, args.screenshot) {
        eprintln!("{}", trf("Error: {error}", &[("error", &format_args!("{e:#}"))]));
        std::process::exit(1);
    }
}
