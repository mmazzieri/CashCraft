# MCP server

`cashcraft-cli mcp` speaks the Model Context Protocol (JSON-RPC 2.0, one message per line) on
standard input and output. It offers the same operations as the command line
(`cashcraft-cli tools` lists them): accounts, transactions, transfers, CSV preview and import,
categories, rules, budgets, reports, scheduled transactions, CSV export and the double-entry ledger.

## Ledger

`list_ledger_accounts`, `create_ledger_account`, `rename_ledger_account`, `delete_ledger_account`,
`post_entry`, `get_entry`, `list_entries`, `delete_entry`, `set_posting_category`, `set_account_currency`,
`trial_balance` and `verify_ledger` work on the ledger ([ledger.md](ledger.md)). Their
amounts come out as decimal text (`"-12.50"`), debit positive, and more than two decimals are refused.
`post_entry` takes at least two postings and refuses, with nothing written, what does not balance:

    {"date": "2026-01-31", "payee": "Pay", "postings": [
      {"account": "Main account", "amount": "1500.00"},
      {"account": "Taxes", "amount": "500.00"},
      {"account": "Salary", "amount": "-2000.00"}]}

Transactions have an `entry_id` and a `kind`: `category`, `uncategorized`, `transfer`, `other` (an
account of the chart) or `split`. The amount and category of a split one cannot be edited.
Transfers between accounts of different currencies are refused.

`transfer_candidates`, `turn_into_transfer` and `separate_transfer` make a transfer of an imported line and
back. When transactions may be the other side, `turn_into_transfer` is refused and lists them: ask the user,
then pass `match` with the id of one, or `"none"`. An import reports the lines that took the place of an
added side of a transfer (`adopted`) and those that may (`maybe_transfers`). Booleans must be booleans.

An account of the chart is named by its path or name, or as `ledger:5` (its `ledger_account_id`), `code:3010`
or `system:uncategorized`; a bare number is refused. `post_scheduled` posts what it can: `failed` lists the
rest, with the reason.

## Statements

`preview_csv` and `import_csv` read CSV, Excel 97, OFX and PDF ([import.md](import.md)). `problems` lists the
lines that could not be read, `notices` what may not be expected, such as a remembered sign. Columns,
`decimal_separator`, `date_format` and `invert` are remembered by an import for that account and for files with the
same headings (`saved_columns` in `format`); what is given wins; `use_saved: false` neither uses nor
remembers them. `list_saved_columns` and `forget_saved_columns` manage them. With several OFX statements pass
`statement`. `preview_csv` with an `account` gives `likely_duplicates`, each with `existing` and the one `proposed`;
`import_csv` is refused while one is not in `skip_rows` (`"2,5=31"`: row 5 is transaction 31; `"same"`: all
that are `certain`) or `add_rows` (`"3"`, or `"all"` that are left): ask the user. `dry_run` makes the same checks:
`refused` says why, `would_import` what. An OFX statement whose signs contradict its types needs `invert`, true or
false. `deletions` are transactions the bank takes back: nothing is done. `ledger_balance` is the balance the bank
states against the books, only a check.

## Card statements

`preview_csv` and `import_csv` read the PDF statement of a credit card too: `statement` has its dates and
`checks`, the totals it states against what was read, in exact decimal text. When they differ
(`sound: false`) the import is refused, unless `allow_mismatch` is true: ask the user first. It is also refused
when it is not `identified`, and into an account that is not a credit card without `any_account`.
`propose_card_payments` (also `card_payments` in the answer of an import) lists the bank transactions that
may pay each statement, with the `card_lines` that may be the same; `link_card_payment` makes one the transfer
to the card (`join`: the card line, only if the user says it is the same), `unlink_card_payment` undoes
it, `list_card_statements` shows them all. See [cards.md](cards.md).

## Rules and tags

`list_rules`, `add_rule`, `update_rule`, `move_rule`, `delete_rule` and `test_rule` (how many transactions a rule would match)
manage the rules ([rules.md](rules.md)). `add_rule` with `from_transaction` categorizes others like it. `preview_rules` lists what
`apply_rules` would change: show it, then pass its `token`. `scope: "all"` takes the transactions with a category too, and
`overwrite`, only if the user asked, what was set by hand. `undo_rules` with the `run` that
`apply_rules` gave (or `can_undo` of `list_rules`) undoes the last run: ask the user first; `skipped` lists what was changed since
and stays. `preview_csv` gives each row `ruled`: the category, payee and tags it would get, and the rules; nothing for rows that would not be added. `broken_rules`
are rules that cannot be read and were skipped: tell the user. `get_transaction` has the rules that set something of it and what the bank
wrote, or what was typed before a rule; `tags` of `add_transaction` and `update_transaction` is text with commas, or a list; `list_transactions` takes `tag`.

## Allowed folders

The server refuses to start without both folders:

    cashcraft-cli mcp --automation-read-root <folder> --automation-write-root <folder>

- `preview_csv` and `import_csv` read only files inside the read folder.
- `export_csv` writes only inside the write folder.
- Relative paths start from that folder. Paths that lead outside it, including through `..`
  or symbolic links, are refused with an error the agent can read.

The data file itself is chosen by whoever starts the server (`--db` or `CASHCRAFT_DB`, else the one chosen last). For one in a synced folder ([data-file.md](data-file.md)) each call first takes what
other computers wrote and then writes what it changed; when something must be known, such as a conflict or changes that wait,
the result has it in `data_file` and as text. After a conflict ids may mean other things: list again. `cashcraft-cli data status` says more.

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

A parameter of another type than the declared one is refused, and nothing is done; null is a parameter that is not given.

Amounts are in euros; negative means money going out. Accounts and categories can be named
by id or by name (`"Home:Utilities"` for a subcategory, or whatever its name is in the data
file). Amounts are best given as numbers; in text, `"1,234"` is 1234 and `"1.234"` is 1.23.

The server is in English whatever the language of the app: descriptions, messages, errors,
the CSV layout of `export_csv` and the payee of new transfers ("Transfer to …"). A data file it
creates gets categories in the user's language. Errors come back as tool results with
`isError: true` and a message, never as a crash.
