# MCP server

`cashcraft-cli mcp` speaks the Model Context Protocol (JSON-RPC 2.0, one message per line) on
standard input and output. It offers the same operations as the command line
(`cashcraft-cli tools` lists them): accounts, transactions, transfers, CSV preview and import,
categories, rules, budgets and envelopes ([envelopes.md](envelopes.md)), reports, scheduled transactions, CSV export and the double-entry ledger.

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
rest, with the reason. `add_scheduled` with `from_transaction` starts from a transaction in the register: the
other parameters change what they name, and the next date is one period after the transaction.

`forecast` projects one account (`account`) or every account in use for `days` days (1 to 730, 90 by default):
today's balance, then the transactions already in the books with a later date and each occurrence of the
scheduled transactions; the past due dates of one not recorded yet are counted today as one item whose
`overdue` says how many they are. `shortfall` is the first day the balance
goes below `minimum`, with `needed`, what must come in before then to stay at the minimum to the end. The
minimum is the one set with `set_minimum_balance` (`minimum: null` takes it away), else zero, and none for a
credit card or a loan. Nothing else is foreseen: what is neither booked nor scheduled is not in it.

## Investments

`record_valuation` writes down what an investment account is worth on a day (it replaces the value of that day),
`delete_valuation` takes one away, `list_valuations` gives each investment account's `balance`, `invested`, `gain` and
values. A gain is neither income nor spending, and the net worth counts the value ([investments.md](investments.md)).

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

## Reconciliation

`start_reconciliation` (an `account`, and a `path` of a statement, or `balance` and `date`), `preview_reconciliation`,
`match_reconciliation` (`pairs`: `"3=41,5=42"`, line number and transaction id), `unmatch_reconciliation`,
`add_reconciliation_lines`, `clear_reconciliation_transactions`, `finish_reconciliation`, `undo_reconciliation` and
`list_reconciliations` ([reconcile.md](reconcile.md)). Their amounts are exact decimal text. The agent proposes and the
user confirms: show the balance a file states (`stated`) before relying on it, and the proposed matches before
passing them; exact ones may go together if the user says so, proposed ones one by one, with the ids of the preview.
Ask before finishing or undoing. `update_transaction`, `delete_transaction`, `turn_into_transfer`, `separate_transfer`,
`delete_entry`, `link_card_payment`, `unlink_card_payment` and `set_account_currency` are refused when they would
change the amount, date or account of a reconciled transaction, unclear or delete it, or the currency of a
reconciled account, and the error names it: ask the user, then pass
`unreconcile: true`; its reconciliation shows as broken until it is done again.

## Rules and tags

`list_rules`, `add_rule`, `update_rule`, `move_rule`, `delete_rule` and `test_rule` (how many transactions a rule would match)
manage the rules ([rules.md](rules.md)). `add_rule` with `from_transaction` categorizes others like it. `preview_rules` lists what
`apply_rules` would change: show it, then pass its `token`. `scope: "all"` takes the transactions with a category too, and
`overwrite`, only if the user asked, what was set by hand. `undo_rules` with the `run` that
`apply_rules` gave (or `can_undo` of `list_rules`) undoes the last run: ask the user first; `skipped` lists what was changed since
and stays. `preview_csv` gives each row `ruled`: the category, payee and tags it would get, and the rules; nothing for rows that would not be added. `broken_rules`
are rules that cannot be read and were skipped: tell the user. `get_transaction` has the rules that set something of it and what the bank
wrote, or what was typed before a rule; `tags` of `add_transaction` and `update_transaction` is text with commas, or a list; `list_transactions` takes `tag`.
`add_transaction` and `add_transfer` with `every` or `unit` also repeat: the movement of `date` is recorded and the next ones are
scheduled from one period later, in one change (a `date` still to come is only scheduled); the result has `scheduled` and `next_date`.

## Allowed folders

The server refuses to start without both folders:

    cashcraft-cli mcp --automation-read-root <folder> --automation-write-root <folder>

- `preview_csv` and `import_csv` read only files inside the read folder.
- `export_csv` writes only inside the write folder.
- Relative paths start from that folder. Paths that lead outside it, including through `..`
  or symbolic links, are refused with an error the agent can read.

The data file itself is chosen by whoever starts the server (`--db` or `CASHCRAFT_DB`, else the one chosen last). For one in a synced folder ([data-file.md](data-file.md)) each call first takes what
other computers wrote and then writes what it changed; when something must be known, such as a conflict or changes that wait,
the result has it in `data_file` and as text. Ids stay good when another computer changes the data. `cashcraft-cli data status` says more.

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
