//! CashCraft desktop app. Terminal and MCP use live in `cashcraft-cli`.

// No console window behind the app on Windows (release builds).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use cashcraft::{db, gui};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "cashcraft",
    version,
    about = "CashCraft — gestione delle finanze personali",
    after_help = "Per terminale e agenti AI (MCP) usa `cashcraft-cli`. Archivio predefinito: cartella dati dell'utente/CashCraft/cashcraft.db (oppure variabile CASHCRAFT_DB)."
)]
struct Args {
    /// File dell'archivio da usare
    #[arg(long)]
    db: Option<PathBuf>,
    /// Apre la finestra, la disegna, ne salva un'immagine PNG ed esce
    #[arg(long)]
    screenshot: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();
    let path = args.db.unwrap_or_else(db::default_db_path);
    if let Err(e) = gui::run(path, args.screenshot) {
        eprintln!("Errore: {e:#}");
        std::process::exit(1);
    }
}
