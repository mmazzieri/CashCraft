//! CashCraft command line and MCP server.
#![forbid(unsafe_code)]

mod cli;
mod data;
mod mcp;

use cashcraft_engine::i18n::{Lang, META_KEY, set_lang, trf};
use cashcraft_engine::store::{self, Home, Use};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "cashcraft-cli",
    version,
    about = "CashCraft for the terminal, and MCP server for AI agents",
    after_help = "Default data file: CashCraft/cashcraft.db in your user data folder (or the variable CASHCRAFT_DB, or the file chosen last in the app).\nLanguage of what is printed: CASHCRAFT_LANG=en or it, else the one chosen in the app, else the system's.\nMCP server: cashcraft-cli mcp --automation-read-root FOLDER --automation-write-root FOLDER"
)]
struct Cli {
    /// Data file to use
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Use the data file where it lies, even in a synced folder
    #[arg(long, global = true, conflicts_with = "shared")]
    direct: bool,
    /// Use the data file through a private copy, as in a synced folder
    #[arg(long, global = true)]
    shared: bool,
    /// Print the results as JSON
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: cli::Cmd,
}

fn main() {
    if cashcraft_engine::pdf::answer_if_child() {
        return;
    }
    let args = Cli::parse();
    let home = Home::user();
    let (path, must_exist) = home.pick(args.db);
    let asked = match (args.direct, args.shared) {
        (true, _) => Use::Direct,
        (_, true) => Use::Shared,
        _ => Use::Auto,
    };
    let user = Lang::at_start(None);
    // An agent gets English, but a data file it creates is the user's.
    let mcp = matches!(args.cmd, cli::Cmd::Mcp { .. });
    set_lang(if mcp { Lang::En } else { user });
    let result = match args.cmd {
        cli::Cmd::Data(cmd) => data::run(&home, &path, asked, cmd, args.json),
        cli::Cmd::Updates => updates(args.json),
        cmd => store::open(&home, &path, asked, user, must_exist).and_then(|o| {
            if !mcp {
                set_lang(Lang::at_start(o.db.meta(META_KEY).as_deref()));
            }
            for n in &o.notices {
                eprintln!("! {n}");
            }
            let guard = match o.shared {
                None => cli::Guard::Open(home.clone(), path.clone()),
                Some((shared, report)) => {
                    for n in report.news() {
                        eprintln!("! {n}");
                    }
                    cli::Guard::Shared(shared)
                }
            };
            cli::run(&o.db, &guard, cmd, args.json)
        }),
    };
    if let Err(e) = result {
        eprintln!("{}", trf("Error: {error}", &[("error", &format_args!("{e:#}"))]));
        std::process::exit(1);
    }
}

/// `cashcraft-cli updates`: the newest release, and whether it is newer than this program.
fn updates(json: bool) -> anyhow::Result<()> {
    use cashcraft_update::version::Version;
    let project = cashcraft_update::Project {
        name: "CashCraft",
        program: "cashcraft",
        repository: env!("CARGO_PKG_REPOSITORY"),
        keys: cashcraft_engine::RELEASE_KEYS,
    };
    let running = Version::parse(env!("CARGO_PKG_VERSION"));
    let pre = running.as_ref().is_some_and(Version::is_pre_release);
    let release = match cashcraft_update::github::latest(&project, pre) {
        Ok(r) => Some(r),
        Err(cashcraft_update::Problem::NoRelease | cashcraft_update::Problem::PreRelease) => None,
        Err(p) => anyhow::bail!("{p}"),
    };
    let newer = release.as_ref().filter(|r| running.as_ref().is_some_and(|v| r.version.is_newer_than(v)));
    if json {
        let value = serde_json::json!({
            "running": env!("CARGO_PKG_VERSION"),
            "latest": release.as_ref().map(|r| r.version.to_string()),
            "newer": newer.is_some(),
            "page": release.as_ref().map(|r| r.page.clone()),
            "security": newer.is_some_and(|r| r.security),
            "signed": release.as_ref().is_some_and(cashcraft_update::download::is_signed),
        });
        println!("{value}");
    } else if let Some(r) = newer {
        println!("{}", trf("CashCraft {version} is available: {page}", &[("version", &r.version), ("page", &r.page)]));
    } else {
        println!("{}", cashcraft_engine::i18n::tr("CashCraft is up to date."));
    }
    Ok(())
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
            ("cards", "carte"),
            ("data", "archivio"),
            ("updates", "aggiornamenti"),
        ] {
            let cli = Cli::command();
            let name = |n: &str| cli.find_subcommand(n).map(|c| c.get_name().to_string());
            assert!(name(english).is_some(), "{english}");
            assert_eq!(name(english), name(italian));
        }
    }
}
