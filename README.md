<p align="center">
  <img src="assets/app-icon/hicolor/256x256/apps/it.mauromazzieri.cashcraft.png" width="128" height="128" alt="CashCraft icon: a white coin with a green euro sign on a teal rounded square">
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
  <a href="https://github.com/mmazzieri/CashCraft/releases/latest"><b>Download CashCraft</b></a> ·
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

The interface and the command line are in English and Italian: CashCraft follows the system,
and **System / English / Italiano** at the bottom of the sidebar chooses another language for the
data file. `CASHCRAFT_LANG=en` or `it` wins over both. Categories, rules and sample data of a new
file are created in the language in use and are not translated afterwards. Amounts and dates are
shown as `1.234,56 €` and dd/mm/yyyy in Italian, `€1,234.56` and yyyy-mm-dd in English; both
styles can be typed in either language. Only one separator before exactly three digits depends on
it: `1.234` is 1234 in Italian, `1,234` is 1234 in English. Bank statements are read the same in
every language.

Data stays on your computer in a single SQLite file.

## Install

Each [release](https://github.com/mmazzieri/CashCraft/releases) has these files, listed with their checksums in `SHA256SUMS.txt`:

| System | File |
|---|---|
| Windows 10 and 11, x64 | `cashcraft-<version>-windows-x64.msi` (installer) or `-windows-x64-portable.zip` |
| macOS 11 or newer, Intel and Apple silicon | `cashcraft-<version>-macos-universal.dmg` |
| Debian, Ubuntu and derivatives | `cashcraft-<version>-linux-<arch>.deb` |
| Fedora, openSUSE, RHEL | `cashcraft-<version>-linux-<arch>.rpm` |
| Any Linux with glibc 2.35 or newer | `cashcraft-<version>-linux-<arch>.tar.gz` |

`<arch>` is `x86_64` or `aarch64`. Every package also contains `cashcraft-cli`. The MSI installs for the current user or for every user of the computer. The Windows files are not signed yet, so SmartScreen may ask for a confirmation the first time.

On macOS, drag CashCraft to Applications. Until a release is signed with a Developer ID and notarized, macOS blocks it the first time. On macOS 15 or newer, open it once, close the warning, then go to **System Settings › Privacy & Security** and choose **Open Anyway** next to CashCraft; on older versions, right-click it, choose **Open** and confirm. From Terminal, `xattr -dr com.apple.quarantine /Applications/CashCraft.app` does the same.

## Command line

    cashcraft-cli --help
    cashcraft-cli accounts list
    cashcraft-cli accounts add "Main account" --kind bank --opening 1500
    cashcraft-cli import statement.csv --account "Main account" --dry-run
    cashcraft-cli import statement.csv --account "Main account"
    cashcraft-cli tx list --account "Main account" --from 2026-01-01
    cashcraft-cli tx add -a "Main account" --amount -12,50 -p "Bar" -c "Restaurants and bars"
    cashcraft-cli report categories --month 2026-10
    cashcraft-cli budget set Groceries 400
    cashcraft-cli rules add "panificio|forno" Groceries
    cashcraft-cli tools                     # every operation, as in MCP
    cashcraft-cli call list_accounts '{}'   # any operation with JSON arguments

Commands have Italian names too (`conti`, `movimenti`, `importa`, `regole`…); the help is in
English. Category names above are those of a data file created in English. Add `--json` for
machine-readable output. The data file is `CashCraft/cashcraft.db` in your user data folder;
use `--db <file>` or `CASHCRAFT_DB` to pick another.

### CSV export

The layout follows the language:

| | Italian | English |
|---|---|---|
| Separator | `;` | `,` |
| Columns | Data, Conto, Beneficiario, Categoria, Note, Importo, Verificato | Date, Account, Payee, Category, Notes, Amount, Cleared |
| Amounts | `-1.234,56` | `-1234.56` |
| Dates | dd/mm/yyyy | yyyy-mm-dd |
| Cleared | `sì`, `no` | `yes`, `no` |

Files are UTF-8. Choose a layout with, for example, `CASHCRAFT_LANG=en cashcraft-cli export out.csv`.

## MCP server

    cashcraft-cli mcp --automation-read-root <folder> --automation-write-root <folder>

Both folders are required: the agent may only import CSV files from the read folder and only
write exports into the write folder. The server always speaks English. See [docs/mcp.md](docs/mcp.md).

## Building

Needs Rust 1.92 or newer.

    cargo run --release -p cashcraft
    cargo xtask ci      # fmt, clippy -D warnings, tests

The packages are built by `packaging/linux/package.sh`, `packaging/macos/package.sh` and `packaging/windows/package.ps1`, and by the Release workflow for every pull request that touches them; a tag `vX.Y.Z` opens a draft release with all of them.

## Clean room

CashCraft is independent and was written from a description of the features people expect
from a personal finance manager. No other product's code, files, artwork or text was used.
See [ATTRIBUTION.md](ATTRIBUTION.md).

## License

MIT OR Apache-2.0, at your option: [LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE).

## Italiano

CashCraft, in italiano e in inglese, è un programma libero per gestire le finanze personali: conti, movimenti, categorie,
budget, report, movimenti ricorrenti e importazione dei CSV delle banche italiane. Funziona su
macOS, Windows e Linux, anche da terminale (`cashcraft-cli`) e con gli assistenti AI tramite MCP.
Scarica il file per il tuo computer dalla [pagina delle release](https://github.com/mmazzieri/CashCraft/releases/latest).
