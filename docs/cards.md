# Credit card statements

## Reading the PDF

The text and its positions come from `hayro-interpret`, in another process: the program started again with
`--read-pdf-from-stdin`. A crafted file can overflow the stack of the library, which ends a process whatever
the thread, or keep it busy for ever, and a thread cannot be stopped. The process gets 10 seconds; on Linux it
also stops at 1 GB of memory. Files have at most 32 MB, 100 pages and 200,000 characters. Where it fails, or
is stopped, the error is "the PDF is damaged" or "takes too long". PDF files with a password or without text
(scans, or fonts that are not in the file) are refused too.

Words are separated by the gaps between characters as well as by spaces. Columns come from the headings of
each page and figures from their labels, not from fixed positions.

## What is imported

| On the statement | Row |
|---|---|
| *Dettaglio dei suoi movimenti* | one each, purchases negative and refunds (minus before or after) positive |
| *Box finanziamenti…*, lines that start with *Rata* | one each, negative |
| *Imposta di bollo* | one, on the last day of the statement |

The line of the conversion fee goes into the notes, with the amount and rate in the other currency: the
amount of the purchase contains it. The plan of each financing (*Situazione contabile…*) and the debt left
from the month before are not imported.

Checks: the purchases add up to *Totale spese*; the instalments to *Totale complessivo finanziamenti*;
purchases, instalments, stamp duty and the previous debt, less the new one, to *Totale addebito*; that is also
the amount on the first page. If one fails, or a row cannot be read, nothing is imported unless the user
confirms.

A row is known by its statement (last day and cards), date, amount, text and order: the same statement adds
nothing twice, and equal purchases of two statements are both kept.

## The payment

Importing a statement records its last day, debit date and amount in `card_statements`. Its payment may be a
transaction of a current account in the same currency, for that amount, from 3 days before the debit date to
10 after, that can become a transfer ([ledger.md](ledger.md)). The candidates are listed and the user chooses.
Linking turns it into a transfer to the card, joined with the card's own line of the payment when there is
one; with several of those it is refused. The purchases of the statement remain the only expenses.
Unlinking separates the transfer: the bank's line becomes uncategorized. A transfer of the same amount near the
debit date, made by hand, also counts as the payment.

## Not verified yet

The layout was seen on one statement with its contents hidden. To be confirmed on others: how a refund is
written (minus before or after), that the amount of a foreign purchase contains the fee, several cards on one
statement, where the debit date is on the first page, and lists that go on over more pages.
