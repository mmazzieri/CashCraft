# The double-entry ledger

Every amount in the data file is a *posting*. An *entry* is a date, a payee, a note and at least two
postings whose debits equal their credits in each currency. Debit is positive, credit negative, so a
posting on an account of the user has the sign people expect (negative = money out). Amounts are
integers, in cents. Since format 2 there is no other record of money: balances, registers, budgets and reports
are sums of postings.

## Chart of accounts (`ledger_accounts`)

| Account | Class | Currency |
|---|---|---|
| each account of the user (`money_account_id`) | liability for credit cards and loans, else asset | its own |
| each category (`category_id`) | income or expense | any |
| *Opening balances* (`system = opening_balances`) | equity | any |
| *Uncategorized* (`system = uncategorized`) | expense | any |
| *Changes in value* (`system = market_value`, since format 11) | equity | any |
| accounts added by hand | that of the account above, or the one chosen | one; EUR by default |

Triggers give each account and category its one row, whoever adds it. Income and expense accounts
added to the chart are categories, so that budgets and reports see them.

*Uncategorized* holds income and spending alike. It needs a class that counts in the reports, and
"expense" is where most of it ends up; reports tell income from spending by the sign of each posting,
so the class changes no figure. Equity would have hidden uncategorized money from the reports.

## Writing

`Db::save_entry` is the only code that inserts or changes postings (a test scans the sources). It
refuses, with nothing written: fewer than two or more than 1000 postings; an account that is not there;
a posting in a currency other than its account's; an amount beyond `MAX_AMOUNT`; a sum that overflows;
debits that differ from credits in any currency; a balance beyond `MAX_BALANCE` (10^15 cents) on an account
it touches; postings that together, whatever their sign, go beyond `MAX_TURNOVER` (4·10^18 cents, kept up to
date by triggers in `ledger_turnover`). No sum of some of the postings can be larger than that one, at any
date, so every total fits in 64 bits. There are no exchange rates: an entry across
currencies passes only if each currency balances on its own, and a transfer between accounts of
different currencies is refused.

## Transactions

A transaction is a posting on an account of the user that is not an opening balance, and its id is the
id of the posting. Its category is the other posting of the entry; `transfer_peer` is the other posting
when that is on an account of the user too. Each side of a transfer keeps its own payee. Import hashes
are unique in each account; `journal_postings.other_hashes` has those of other files ([import.md](import.md)).
`original_payee` and `original_memo` keep what the bank wrote, `tags` the tags and `rules` what the rules did
([rules.md](rules.md)); they follow the posting, also through a transfer.

## Turning a transaction into a transfer

`turn_into_transfer` gives the other posting of a transaction to another account of the user. When the
other account has its own transaction for the same money (opposite amount, at most 10 days apart) the two
entries become one, with the postings of both: ids, texts, cleared marks and import hashes stay, so either
statement can be imported again. Which one it is, or none, is always the user's choice. Transactions on a
category, on *Uncategorized* or on *Opening balances* (half a transfer of an older file) can be turned;
splits, transfers and those on accounts added by hand cannot.

The two banks may date the same transfer differently. The entry has the date of the transaction that was
turned; a side that was joined keeps its own in `journal_postings.date`. Registers, their running balances, filters and
exports use the date of the side, so they go on agreeing with each bank's statement. The trial balance, the
net worth over time and the journal use the date of the entry, so that debits equal credits on every date:
for them both sides happen on that day. Editing the date of a side that has its own changes that
one alone, and so does editing its texts: only a note that both sides take from the entry changes for both.
When a side is left alone, because the other account was deleted or the transfer separated, its date becomes
that of the entry.

A side that was added, and not joined, is marked (`journal_postings.origin = 'added'`), as are both sides of a
transfer made by hand. A line imported later on its account, of the same amount and at most 10 days apart,
takes its place: the date, text and import hash of the line move onto it and no row is added. That is done
only when the line fits one added side and the side fits one new line, and the result of the import says
so; otherwise the line is added, and listed for the user to join. The migration marks the sides of transfers
that were never imported.

`separate_transfer` gives each side an entry of its own, uncategorized, with its date and texts; the
categories of before are not brought back. With `keep_other` false the other side goes instead.

## Reports

An entry with one posting on a category or *Uncategorized* is income or spending by the sign of that posting,
as before the ledger. With several, each stays on the side of its class: income accounts add up into income,
expense accounts and *Uncategorized* into expenses, so that moving money between two categories is neither.
The register counts a split once, for its categories only. The net worth is the sum of every asset and
liability account, those added by hand and archived ones included; amounts of different currencies are added
as they are.

## Currencies

Codes are kept in capitals; €, Euro and Euros are EUR. New accounts take a code of three letters. The
migration leaves as it is what it does not know, like `$`. `set_account_currency` changes the currency of an
account, its entries and the opposite postings on accounts of any currency, amounts unchanged; it is refused
when an entry would no longer balance, that is when it also moves an account of another currency.

## Names

`ledger:5` is the account of the chart with that `ledger_account_id`, `code:3010` one with that code,
`system:uncategorized` and `system:opening_balances` the two that every file has. Otherwise a code, path or
name must be that of one account only. A bare number is refused: ids of accounts and of categories are
different numbers.

## Opening balances

An entry dated `0001-01-01` against *Opening balances*, marked with `entries.opening_account_id`; none
when the balance is zero. The date comes before any transaction, so the balance counts in every period
as it always did: running balances and the history of the net worth do not depend on when an account
was opened. It is read and changed as `opening_balance` of the account.

## Deleting

- A category, with those under it: its postings move to *Uncategorized*.
- An account: its entries go, except those that also touch another account of the user, or an asset,
  liability or equity account added by hand. There the deleted side moves to *Opening balances*: no other
  balance changes and nothing becomes income or an expense. 0.2.0 counted the other side of a transfer in
  the reports.
- An account added to the chart by hand, only while nothing is on it or under it.

## Checking

`trial_balance` (up to a date) lists every balance with equal totals of debits and credits.
`verify_ledger` checks each entry, posting and account and reports what is wrong; it changes nothing.

## Migration from formats 2 to 9

The table `postings` (`ledger_postings` in format 7, `book_postings` in 8) becomes `journal_postings`, and `category_rules` becomes `transaction_rules`, with the same ids and
order ([rules.md](rules.md)): a window of an older program that has the file open can neither
read nor write a posting any more. The columns `date`, `origin`, `other_hashes`, `original_payee`, `original_memo`, `tags` and `rules` and the tables `card_statements`
([cards.md](cards.md)) and `csv_layouts` ([import.md](import.md)) are
added where they are missing, and the dates of sides left alone put right, after a copy as
`cashcraft.v2-backup.db` (or `v3` to `v9`) with the rules below. Format 9 adds the tables of the reconciliations
([reconcile.md](reconcile.md)), format 10 those of the envelopes ([envelopes.md](envelopes.md)), format 11 the values of
investment accounts and *Changes in value* ([investments.md](investments.md)). Lines with an import hash on a card with
statements of format 4 are taken as rows of those statements.

## Migration from formats 0 and 1

On the first opening, in one transaction that holds the write lock (the version is read again inside
it, so that programs starting together migrate once):

1. A copy of the old file is made beside it (beside its working copy for one in a synced folder, see [data-file.md](data-file.md)): `cashcraft.v1-backup.db`, or `-2`, `-3`… when that name is
   taken; nothing is overwritten. The name appears only once the copy is complete, with as many rows in every
   table and the permissions of the file. Without it, there is no migration.
2. Each transaction becomes an entry, and its id the id of the posting on its account.
3. The two sides of a transfer become one entry when they point at each other, on two accounts of the
   same currency, with the same date and opposite amounts. Anything else with a `transfer_peer` goes
   against *Opening balances*: like before it is in the balances and not in the reports. The same happens
   to transfers between different currencies; each side keeps its amount.
4. A missing category becomes *Uncategorized*. A row of a missing account, which no register showed, is
   kept against *Opening balances*; the reports still count it, and the history of the net worth, which
   counted it by mistake, now agrees with the balances.
5. Every balance is compared with the old one and the books are verified. Any difference undoes it all
   and leaves the old file as it was.

The tables `budgets`, `rules` and `scheduled` are renamed (`category_budgets`, `category_rules`,
`scheduled_items`) and `transactions` goes. 0.2.0 refuses a file of format 2. If it already has the file open,
whatever it writes to them fails at the first statement; it can still add or rename categories and change its
settings (`meta`), which harms nothing. If it starts during the update it can still mark the file as format 1
and add its own empty tables: they are dropped at the next opening, or, if it wrote in them, a new copy is made
and what is there is added to the ledger.

A scheduled transfer between currencies, which 0.2.0 allowed, cannot be posted: it is reported and stays due,
and the others are posted.
