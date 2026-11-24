//! CashCraft command line and MCP server.
#![forbid(unsafe_code)]

mod cli;
mod mcp;

use cashcraft_engine::db;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "cashcraft-cli",
    version,
    about = "CashCraft da terminale e server MCP per agenti AI",
    after_help = "Archivio predefinito: cartella dati dell'utente/CashCraft/cashcraft.db (oppure variabile CASHCRAFT_DB).\nServer MCP: cashcraft-cli mcp --automation-read-root CARTELLA --automation-write-root CARTELLA"
)]
struct Cli {
    /// File dell'archivio da usare
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Stampa i risultati in JSON
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: cli::Cmd,
}

fn main() {
    let args = Cli::parse();
    let path = args.db.unwrap_or_else(db::default_db_path);
    let result = db::Db::open(&path).and_then(|(d, _)| cli::run(&d, args.cmd, args.json));
    if let Err(e) = result {
        eprintln!("Errore: {e:#}");
        std::process::exit(1);
    }
}
