//! CashCraft command line and MCP server.
#![forbid(unsafe_code)]

mod cli;
mod mcp;

use cashcraft_engine::db;
use cashcraft_engine::i18n::{Lang, META_KEY, set_lang, trf};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "cashcraft-cli",
    version,
    about = "CashCraft for the terminal, and MCP server for AI agents",
    after_help = "Default data file: CashCraft/cashcraft.db in your user data folder (or the variable CASHCRAFT_DB).\nLanguage of what is printed: CASHCRAFT_LANG=en or it, else the one chosen in the app, else the system's.\nMCP server: cashcraft-cli mcp --automation-read-root FOLDER --automation-write-root FOLDER"
)]
struct Cli {
    /// Data file to use
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Print the results as JSON
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: cli::Cmd,
}

fn main() {
    let args = Cli::parse();
    let path = args.db.unwrap_or_else(db::default_db_path);
    let user = Lang::at_start(None);
    // An agent gets English, but a data file it creates is the user's.
    set_lang(if matches!(args.cmd, cli::Cmd::Mcp { .. }) { Lang::En } else { user });
    let result = db::Db::open_seeding(&path, user).and_then(|(d, _)| {
        if !matches!(args.cmd, cli::Cmd::Mcp { .. }) {
            set_lang(Lang::at_start(d.meta(META_KEY).as_deref()));
        }
        cli::run(&d, args.cmd, args.json)
    });
    if let Err(e) = result {
        eprintln!("{}", trf("Error: {error}", &[("error", &format_args!("{e:#}"))]));
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn commands_have_english_and_italian_names() {
        Cli::command().debug_assert();
        for (english, italian) in [
            ("accounts", "conti"),
            ("transactions", "movimenti"),
            ("transfer", "giroconto"),
            ("import", "importa"),
            ("preview", "anteprima"),
            ("categories", "categorie"),
            ("rules", "regole"),
            ("scheduled", "pianificati"),
            ("export", "esporta"),
            ("ledger", "contabilita"),
        ] {
            let cli = Cli::command();
            let name = |n: &str| cli.find_subcommand(n).map(|c| c.get_name().to_string());
            assert!(name(english).is_some(), "{english}");
            assert_eq!(name(english), name(italian));
        }
    }
}
