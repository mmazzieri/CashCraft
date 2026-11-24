//! CashCraft command line and MCP server.

use cashcraft::{cli, db, icon};
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
    let result = match args.cmd {
        // L'icona non ha bisogno dell'archivio.
        cli::Cmd::Icon { file, size } => icon::render(size.clamp(16, 2048))
            .save(&file)
            .map_err(anyhow::Error::from),
        cmd => {
            let path = args.db.unwrap_or_else(db::default_db_path);
            db::Db::open(&path).and_then(|(d, _)| cli::run(&d, cmd, args.json))
        }
    };
    if let Err(e) = result {
        eprintln!("Errore: {e:#}");
        std::process::exit(1);
    }
}
