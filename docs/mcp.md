# MCP server

`cashcraft-cli mcp` speaks the Model Context Protocol (JSON-RPC 2.0, one message per line) on
standard input and output. It offers the same operations as the command line
(`cashcraft-cli tools` lists them): accounts, transactions, transfers, CSV preview and import,
categories, rules, budgets, reports, scheduled transactions and CSV export.

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
by id or by name (`"Casa:Bollette"` for a subcategory). Errors come back as tool results with
`isError: true` and a message, never as a crash.
