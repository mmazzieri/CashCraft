# Reconciling with the bank

A reconciliation checks an account against a statement of its bank: the balance the statement states on its
last day against the *cleared balance* of the books on that day, that is the opening balance and the cleared
transactions dated up to it. The difference is shown until it is zero; then the reconciliation can be finished,
and the transactions it checked are *reconciled*.

## Starting

One reconciliation at a time for each account, on the day of the last finished one or later. What it starts from:

| | Balance | Day |
|---|---|---|
| typed | as the user writes it | as the user writes it |
| OFX | `LEDGERBAL`, with the sign of the rows | its `DTASOF` |
| card statement in PDF | what is owed at its end: the debit and the debt left after it (*Debito residuo al* its last day), negative | its last day |
| Excel or CSV | a cell beginning with *Saldo contabile*, *Saldo finale*, *Saldo al*, *Saldo disponibile* or *Saldo*, best first, and the figure after it; never *iniziale*, *precedente* or *punti* | a date written in the file, such as *Dati aggiornati al …*, else its last line |

What a file states is a proposal: the window and `stated` of MCP show it with what it was read from, and the
user can give another. *Saldo disponibile*, the only one in the export of BPER that was seen, may count what is not
booked yet. The balance is that of the books: the debt of a card or a loan is negative; the window asks a card
for its debt and changes the sign.

A statement that is a file brings its lines, in the order of the file, with the import keys an import would give
them (`importer`), so that matching them is the same as importing them. Lines after the day of the statement and
lines the bank takes back are left out. At most 20,000 lines.

## Matching

For each line, in this order:

1. **Exact**: the transaction that has the key of the line: it came from that line, or a line of another file was
   said to be it. A transaction reconciled by another reconciliation is shown as such and not offered.
2. **Proposed**: a transaction of the same amount at most five days apart, with the order of the import
   ([import.md](import.md)): the same date and payee, then a payee alike, then any, then the other days, nearest
   first. What comes later never takes what comes before; transactions typed by hand are compared too. The others it
   may be are listed. *Surely the same* (same date, amount and payee, nothing else like it) is said, never acted on.
3. **Missing in the books**: it can be added, cleared, with the keys of an import; the rules run on it.

Nothing is matched by itself. The user confirms the exact ones together (*Match all exact ones*, `--exact`) or one
by one, and each proposal one by one, choosing another candidate if needed. A transaction is the match of one line,
of the account, dated up to the statement, with the same amount. Matching clears it, takes away the mark of an
added side of a transfer and gives it the keys of the line, so that importing the statement later adds nothing and
asks nothing. Undoing a match puts back the mark and the keys it changed.

The transactions up to the statement date that no line is are listed apart: those that are cleared count in the
balance of the books. For a statement without lines this is all there is: the user ticks what the statement shows.

## Finishing and the lock

Finishing is refused while the difference is not zero, and while a line of the statement is neither matched nor
added (one ticked on the side of the books instead would be imported again later). Every cleared transaction of the account dated up to the
statement and not reconciled already is recorded with its date, amount and account (`reconciled_postings`); an
added side of a transfer among them is no longer one.

A reconciled transaction is one whose record still agrees with it: same date, amount and account of the chart,
cleared, there. Changing one of those, unclearing it or deleting it is refused with `reconcile::Reconciled`, which
names the transactions and the reconciliations; after the user's yes (`Db::unreconciling`, `unreconcile: true`, the
question in the window) the change is made and the reconciliation no longer counts it. The category, the payee, the
notes and the tags are not checked against a bank and change freely; so does the other side of a transfer, on its
own account, unless its date or amount is shared with the reconciled side. A transfer is reconciled on each account
by itself.

The check is made where every change of the books ends: `save_entry` and `remove_entry` note the reconciled
postings they touch, and the outermost `atomically` compares them with their records before it commits. A change
that leaves a posting as it was, such as a transfer separated with the same date and amount, passes.

## Status

| | |
|---|---|
| open | being done |
| finished | the books agree with it |
| broken | a transaction it checked is not as it was, or the cleared balance on its day is another (a cleared transaction added or moved there later) |
| done again later | broken, and a reconciliation of the account at the same day or later was finished after it |
| undone | |

A broken one is done again by a new reconciliation on its day or later: what it no longer counts is not
reconciled, and can be checked again.

## Undoing

An open reconciliation is discarded: what its matches changed is put back where it is as they left it; the
transactions it added stay. A finished one can be undone only if it is the last of its account and no other is
open: its transactions stay cleared and are no longer reconciled, and it stays in the list as undone.

## Programs that do not know it

Format 9 renames the table of the postings again (`journal_postings`): a window of format 8 that has the file open
can write no posting from then on, and could not change a reconciled transaction without asking. What it can still
do, deleting an entry, and any change made outside the program, shows the reconciliation as broken: nothing is
taken for reconciled that is not as it was found.

## MCP and the command line

`start_reconciliation`, `preview_reconciliation`, `match_reconciliation`, `unmatch_reconciliation`,
`add_reconciliation_lines`, `clear_reconciliation_transactions`, `finish_reconciliation`, `undo_reconciliation` and
`list_reconciliations`; `cashcraft-cli reconcile start|show|match|unmatch|add|clear|finish|undo|list`. Amounts of a
reconciliation are exact decimal text. Each change takes the ids from the preview: lines by their number in the
file, transactions by id. An agent proposes, the user confirms ([mcp.md](mcp.md)).

## Not verified yet

The balance of a card statement on two statements in a row with a financing; the cells of balance of other banks'
Excel files; whether *Saldo disponibile* of BPER is the booked balance.
