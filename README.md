<p align="center">
  <img src="assets/icons/256x256.png" width="128" height="128" alt="CashCraft icon: a white coin with a green euro sign on a teal rounded square">
</p>

<h1 align="center">CashCraft</h1>

<p align="center">
  <b>Personal finance for people who bank in Italy: accounts, budgets, reports and one-click import of Italian bank CSV statements.</b><br>
  An open-source, clean-room desktop app in pure Rust, with a command line and an MCP server for AI agents.
</p>

<p align="center">
  <img alt="100% Rust" src="https://img.shields.io/badge/100%25-Rust-b7410e?style=flat-square&logo=rust">
  <img alt="macOS · Windows · Linux" src="https://img.shields.io/badge/macOS%20%C2%B7%20Windows%20%C2%B7%20Linux-native-26282e?style=flat-square">
  <img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-3a3a3a?style=flat-square">
  <img alt="Status: early alpha" src="https://img.shields.io/badge/status-early%20alpha-d69e2e?style=flat-square">
</p>

<p align="center">
  <a href="https://github.com/mmazzieri/cashcraft/releases/latest"><b>Download CashCraft</b></a> ·
  <a href="docs/mcp.md">MCP server</a> ·
  <a href="docs/artcraft-launcher.md">ArtCraft Launcher</a> ·
  <a href="#italiano">Italiano</a>
</p>

## What it does

- **Accounts** grouped by type (current accounts, cash, credit cards, savings, investments,
  loans) with live balances and net worth.
- **Register** for each account with running balance, a "cleared" mark, search and filters by
  period and category; transfers between accounts.
- **Overview, reports and budgets**: income vs. spending by month, spending by category,
  net worth over time, monthly budgets per category with progress bars.
- **Categories and rules**: nested categories and keyword rules that categorize new and imported
  transactions automatically.
- **Scheduled transactions**: recurring payments and transfers, posted when due.
- **Italian bank CSV import**: detects the encoding (UTF-8 or Windows-1252), the separator, the
  heading lines to skip and the columns by name (*Data contabile*, *Data valuta*, *Descrizione*,
  *Causale*, *Importo*, *Accrediti/Addebiti*, *Entrate/Uscite*, *Dare/Avere*…); reads Italian
  amounts (`1.234,56`) and dates (`dd/mm/yyyy`); pulls the merchant out of card payment
  descriptions; never imports the same transaction twice. Columns can be remapped by hand.
- **Command line and MCP**: every operation also works without the window, from
  `cashcraft-cli` or an AI agent. The window refreshes when they change the data.

The interface is in Italian. Data stays on your computer in a single SQLite file.

## Downloads

Get the file for your computer from the [latest release](https://github.com/mmazzieri/cashcraft/releases/latest).
Every release has a `SHA256SUMS.txt` to check the files against.

| System | File |
|---|---|
| macOS (Apple silicon and Intel) | `cashcraft-<ver>-macos-universal.dmg` |
| Windows x64 | `cashcraft-<ver>-windows-x64.msi` or `cashcraft-<ver>-windows-x64-portable.zip` |
| Linux x86_64 / aarch64 | `cashcraft-<ver>-linux-<arch>.AppImage`, `.deb` or `.tar.gz` |
| Command line only (macOS) | `cashcraft-cli-<ver>-macos-universal.zip` |

## Command line

    cashcraft-cli --help
    cashcraft-cli accounts list
    cashcraft-cli accounts add "Main account" --kind bank --opening 1500
    cashcraft-cli import statement.csv --account "Main account" --dry-run
    cashcraft-cli import statement.csv --account "Main account"
    cashcraft-cli tx list --account "Main account" --from 2026-01-01
    cashcraft-cli tx add -a "Main account" --amount -12,50 -p "Bar" -c "Ristoranti e bar"
    cashcraft-cli report categories --month 2026-10
    cashcraft-cli budget set Alimentari 400
    cashcraft-cli rules add "conad|coop" Alimentari
    cashcraft-cli tools                     # every operation, as in MCP
    cashcraft-cli call list_accounts '{}'   # any operation with JSON arguments

Italian aliases work too (`conti`, `movimenti`, `importa`, `regole`…). Add `--json` for
machine-readable output. The data file is `CashCraft/cashcraft.db` in your user data folder;
use `--db <file>` or `CASHCRAFT_DB` to pick another.

## MCP server

    cashcraft-cli mcp --automation-read-root <folder> --automation-write-root <folder>

Both folders are required: the agent may only import CSV files from the read folder and only
write exports into the write folder. See [docs/mcp.md](docs/mcp.md).

## Build from source

Needs Rust 1.85 or newer.

    cargo build --release          # target/release/cashcraft and cashcraft-cli
    cargo test

Packages: `packaging/macos/package.sh`, `packaging/windows/package.ps1`,
`packaging/linux/package.sh`; see [docs/releasing.md](docs/releasing.md). On a Mac,
double-click **Installa CashCraft.command** to build and install the app into Applications.

## Clean room

CashCraft is independent and was written from a description of the features people expect
from a personal finance manager. No other product's code, files, artwork or text was used.
See [ATTRIBUTION.md](ATTRIBUTION.md).

## License

MIT OR Apache-2.0, at your option: [LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE).

## Italiano

CashCraft è un programma libero per gestire le finanze personali: conti, movimenti, categorie,
budget, report, movimenti ricorrenti e importazione dei CSV delle banche italiane. Funziona su
macOS, Windows e Linux, anche da terminale (`cashcraft-cli`) e con gli assistenti AI tramite MCP.
Scarica il file per il tuo computer dalla [pagina delle release](https://github.com/mmazzieri/cashcraft/releases/latest).
