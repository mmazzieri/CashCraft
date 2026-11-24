# The double-entry ledger

Every amount in the data file is a *posting*. An *entry* is a date, a payee, a note and at least two
postings whose debits equal their credits in each currency. Debit is positive, credit negative, so a
posting on an account of the user has the sign people expect (negative = money out). Amounts are
integers, in cents. Format 2 has no other record of money: balances, registers, budgets and reports
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
debits that differ from credits in any currency. There are no exchange rates: an entry across
currencies passes only if each currency balances on its own, and a transfer between accounts of
different currencies is refused.

## Transactions

A transaction is a posting on an account of the user that is not an opening balance, and its id is the
id of the posting. Its category is the other posting of the entry; `transfer_peer` is the other posting
when that is on an account of the user too. Each side of a transfer keeps its own payee. Import hashes
are unique in each account.

An entry with more than two postings shows as **Split**. Its date, payee, notes and cleared mark can be
edited from the register; its amount and category cannot. Reports and the category filter see each of
its postings. Deleting any transaction deletes its whole entry.

## Opening balances

An entry dated `0001-01-01` against *Opening balances*, marked with `entries.opening_account_id`; none
when the balance is zero. The date comes before any transaction, so the balance counts in every period
as it always did: running balances and the history of the net worth do not depend on when an account
was opened. It is read and changed as `opening_balance` of the account.

## Deleting

- A category, with those under it: its postings move to *Uncategorized*.
- An account: its entries go, except those that also touch another account of the user, or an asset,
  liability or equity account added by hand. There the deleted side moves to *Uncategorized*, so that no
  other balance changes. As before, what was a transfer then counts in the reports.

## Checking

`trial_balance` (up to a date) lists every balance with equal totals of debits and credits.
`verify_ledger` checks each entry, posting and account and reports what is wrong; it changes nothing.

## Migration from formats 0 and 1

On the first opening, in one transaction that holds the write lock (the version is read again inside
it, so that programs starting together migrate once):

1. A copy of the old file is made beside it: `cashcraft.v1-backup.db`, or `-2`, `-3`… when that name is
   taken; nothing is overwritten. Without a copy that passes a check, there is no migration.
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

0.2.0 refuses a file of format 2. If it already has the file open, its writes to money fail, because the
table is gone.
A number or an empty table that an older program leaves behind is put right at the next opening.

A transfer between currencies scheduled with 0.2.0 stays in the list, but posting the scheduled
transactions stops at it, names it and posts nothing until it is deleted.
