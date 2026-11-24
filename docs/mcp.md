# MCP server

`cashcraft-cli mcp` speaks the Model Context Protocol (JSON-RPC 2.0, one message per line) on
standard input and output. It offers the same operations as the command line
(`cashcraft-cli tools` lists them): accounts, transactions, transfers, CSV preview and import,
categories, rules, budgets, reports, scheduled transactions, CSV export and the double-entry ledger.

## Ledger

`list_ledger_accounts`, `create_ledger_account`, `post_entry`, `get_entry`, `list_entries`,
`delete_entry`, `trial_balance` and `verify_ledger` work on the ledger ([ledger.md](ledger.md)). Their
amounts come out as decimal text (`"-12.50"`), debit positive, and more than two decimals are refused.
`post_entry` takes at least two postings and refuses, with nothing written, what does not balance:

    {"date": "2026-01-31", "payee": "Pay", "postings": [
      {"account": "Main account", "amount": "1500.00"},
      {"account": "Taxes", "amount": "500.00"},
      {"account": "Salary", "amount": "-2000.00"}]}

Transactions have an `entry_id` and a `kind`: `category`, `uncategorized`, `transfer`, `other` (an
account of the chart) or `split`. The amount and category of a split one cannot be edited.
Transfers between accounts of different currencies are refused.

## Allowed folders

The server refuses to start without both folders:

    cashcraft-cli mcp --automation-read-root <folder> --automation-write-root <folder>

- `preview_csv` and `import_csv` read only files inside the read folder.
- `export_csv` writes only inside the write folder.
- Relative paths start from that folder. Paths that lead outside it, including through `..`
  or symbolic links, are refused with an error the agent can read.

The data file itself is chosen by whoever starts the server (`--db` or `CASHCRAFT_DB`).

## Client configuration

    {
      "mcpServers": {
        "cashcraft": {
          "command": "/Applications/CashCraft.app/Contents/MacOS/cashcraft-cli",
          "args": ["mcp",
                   "--automation-read-root", "/Users/me/Downloads",
                   "--automation-write-root", "/Users/me/Documents/CashCraft exports"]
        }
      }
    }

On Windows the program is `C:\Program Files\CashCraft\cashcraft-cli.exe`; on Linux
`/usr/bin/cashcraft-cli`.

## Conventions

Amounts are in euros; negative means money going out. Accounts and categories can be named
by id or by name (`"Home:Utilities"` for a subcategory, or whatever its name is in the data
file). Amounts are best given as numbers; in text, `"1,234"` is 1234 and `"1.234"` is 1.23.

The server is in English whatever the language of the app: descriptions, messages, errors,
the CSV layout of `export_csv` and the payee of new transfers ("Transfer to …"). A data file it
creates gets categories in the user's language. Errors come back as tool results with
`isError: true` and a message, never as a crash.
