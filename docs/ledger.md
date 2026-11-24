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
are unique in each account.

## Turning a transaction into a transfer

`turn_into_transfer` gives the other posting of a transaction to another account of the user. When the
other account has its own transaction for the same money (opposite amount, at most 10 days apart) the two
entries become one, with the postings of both: ids, texts, cleared marks and import hashes stay, so either
statement can be imported again. Several that match are listed and none is chosen. Transactions on a
category, on *Uncategorized* or on *Opening balances* (half a transfer of an older file) can be turned;
splits, transfers and those on accounts added by hand cannot.

The two banks may date the same transfer differently. The entry has the date of the transaction that was
turned; a side that was joined keeps its own in `postings.date`. Registers, their running balances, filters and
exports use the date of the side, so they go on agreeing with each bank's statement. The trial balance, the
net worth over time and the journal use the date of the entry, so that debits equal credits on every date:
for them both sides happen on that day. Editing the date of a side that has its own changes that
one alone.

`separate_transfer` gives each side an entry of its own, uncategorized, with its date and texts; the
categories of before are not brought back. With `keep_other` false the other side goes instead, for one that
was only added by mistake. An other side that was added, and not joined, has no import hash: the statement
of its account, imported later, will add its own line.

An entry with more than two postings shows as **Split**. Its date, payee, notes and cleared mark can be
edited from the register; its amount and category cannot. Reports and the category filter see each of
its postings. A posting on a category can be moved to another one from the entry (`set_posting_category`);
the uncategorized filter lists the splits that need it. Deleting any transaction deletes its whole entry:
the window asks first when it is a split, and says how many lines it has.

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

## Migration from format 2

The column `postings.date` is added, after a copy as `cashcraft.v2-backup.db` with the rules below.

## Migration from formats 0 and 1

On the first opening, in one transaction that holds the write lock (the version is read again inside
it, so that programs starting together migrate once):

1. A copy of the old file is made beside it: `cashcraft.v1-backup.db`, or `-2`, `-3`… when that name is
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
