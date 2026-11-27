# Reconciling with the bank

A reconciliation checks an account against a statement of its bank. It has a *set* of transactions: those matched
with the lines of the statement, or ticked. The *balance of the books* of a reconciliation is the opening balance
of the account and the transactions of its set and of the finished reconciliations before it; it must be the
balance the statement states on its last day. The difference is shown until it is zero; then the reconciliation
can be finished, and the transactions of its set are *reconciled*.

A transaction belongs to the reconciliation that takes it, whatever its date: a cheque dated in October and
cashed in November is matched in November, and October stays as it was. A transaction dated in a month already
reconciled, typed, imported or cleared later, changes nothing of that month.

## Starting

One reconciliation at a time for each account, on the day of the last finished one or later. What it starts from:

| | Balance | Day |
|---|---|---|
| typed | as the user writes it | as the user writes it |
| OFX | `LEDGERBAL`, with the sign of the rows | its `DTASOF` |
| card statement in PDF | what is owed at its end: the debit and the debt left after it (*Debito residuo al* its last day), negative | its last day |
| Excel or CSV | a cell outside the movements beginning with *Saldo contabile*, *Saldo finale*, *Saldo al* or *Saldo*, best first, and the figure after it; *Saldo disponibile* only when there is no other, with a notice; never *iniziale*, *precedente* or *punti* | the day written after *al*, *fino a*, *to* or *until* in that cell, else in another cell (*Dati aggiornati al …*), else the last movement |

What a file states is a proposal: the window and `stated` of MCP show it with what it was read from and, for an
available balance, a notice: it may count what is not booked yet, or a credit line. The user can give another.
The balance is that of the books: the debt of a card or a loan is negative; the window asks a card for its debt
and changes the sign. A row that is a movement is never read as a balance, whatever its text.

A statement that is a file brings its lines, in the order of the file, with the import keys an import would give
them (`importer`), so that matching them is the same as importing them. Lines after the day of the statement and
lines the bank takes back are left out, and the user is told how many. At most 20,000 lines. A reconciliation
without lines starts with the cleared transactions up to its day ticked.

## Matching and ticking

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
of the account, dated up to the statement, with the same amount. Matching puts it in the set, clears it, takes away
the mark of an added side of a transfer and gives it the keys of the line, so that importing the statement later
adds nothing and asks nothing. A transaction ticked before becomes the match.

The transactions up to the statement date that no line is or may be are listed apart, ticked or not. Ticking puts
one in the set and clears it; unticking takes it out. Ticking never takes the mark of an added side: the bank's
line imported later still takes its place, and keeps the date the reconciliation checked.

Undoing a match, unticking, or discarding the reconciliation puts back what was changed (the cleared mark, the mark
of an added side, every key given, those that name the file of an older import too) only where the transaction is
still the one taken: same account, amount and date. A line whose transaction is deleted is a line to match again.

## Finishing and the lock

Finishing is refused while the difference is not zero, and while a line of the statement is neither matched nor
added (one ticked on the side of the books instead would be imported again later). Each transaction of the set is
checked again: of the account, cleared, up to the statement date, not reconciled elsewhere, and with the amount of
its line; it is recorded as it is then, with its date, amount and account (`reconciled_postings`). The opening
balance and the currency of the account are recorded too.

A reconciled transaction is one whose record still agrees with it: same date, amount and account of the chart,
cleared, there. Changing one of those, unclearing it or deleting it is refused with `reconcile::Reconciled`, which
names the transactions and the reconciliations; so is changing the opening balance or the currency of the account.
After the user's yes (`Db::unreconciling`, `unreconcile: true`, the question in the window) the change is made: the
transaction leaves the set, and the reconciliation shows as broken. The category, the payee, the notes and the tags
are not checked against a bank and change freely; so does the other side of a transfer, on its own account, unless
its date or amount is shared with the reconciled side. A transfer is reconciled on each account by itself.

The check is made where every change of the books ends: `save_entry` and `remove_entry` note the reconciled
postings they touch, and the outermost `atomically` compares them with their records before it commits. A change
that leaves a posting as it was, such as a transfer separated with the same date and amount, passes.

Ids of postings are never given again (`posting_ids` keeps the highest), so that a record finds its posting or
none.

## Status

| | |
|---|---|
| open | being done |
| finished | the books agree with it |
| broken | a transaction of its set is not as it was or left it, a transaction of a reconciliation before it left that (its balance of the books is another), or the opening balance or currency of the account changed |
| done again later | broken, and a reconciliation of the account at the same day or later was finished after it |
| undone | |

A broken one is done again by a new reconciliation on its day or later, which takes again what was released.

## Undoing

An open reconciliation is discarded, after a question in the window: see above for what is put back; the
transactions it added stay. A finished one can be undone only if it is the last of its account and no other is
open: its transactions stay cleared and are no longer reconciled, and it stays in the list as undone. Finishing asks
too.

## Programs that do not know it

Format 9 renames the table of the postings again (`journal_postings`): a window of format 8 that has the file open
can write no posting from then on, and could not change a reconciled transaction without asking. What it can still
do, deleting an entry, and any change made outside the program, shows the reconciliation as broken: nothing is
taken for reconciled that is not as it was found.

## MCP, the command line and the phone

`start_reconciliation`, `preview_reconciliation`, `match_reconciliation`, `unmatch_reconciliation`,
`add_reconciliation_lines`, `clear_reconciliation_transactions`, `finish_reconciliation`, `undo_reconciliation` and
`list_reconciliations`; `cashcraft-cli reconcile start|show|match|unmatch|add|clear|finish|undo|list`, and
`--unreconcile` on the commands that may change reconciled transactions or accounts. Amounts of a reconciliation are
exact decimal text. Each change takes the ids from the preview: lines by their number in the file, transactions by
id. An agent proposes, the user confirms ([mcp.md](mcp.md)). The phone apps get the refusal in words for the user,
with the ids, and pass `unreconcile` after asking ([mobile.md](mobile.md)).

## Not verified yet

The balance of a card statement on two statements in a row with a financing; the cells of balance of other banks'
Excel files; whether *Saldo disponibile* of BPER is the booked balance.
