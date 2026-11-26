# Credit card statements

## Reading the PDF

The text and its positions come from `hayro-interpret`. All of it, up to the statement, happens in another
process: the program started again with `--read-pdf-from-stdin`. A crafted file can overflow the stack of the
library, which ends a process whatever the thread, or keep it busy for ever, and a thread cannot be stopped.
The process gets 10 seconds; on Linux it also stops at 1 GB of memory. Files have at most 32 MB, 100 pages,
200,000 characters and 5,000 rows. PDF files with a password or without text (scans) are refused. A font that
the file names and does not carry, as the standard ones often are, is read all the same: what its characters are
comes from the encoding of the file and their widths from the metrics of the standard fonts or from the file; no
font is bundled, for nothing is drawn. A PDF that is not a card statement may be the list of an account's
movements ([import.md](import.md)). The window goes on while a file is read. When the program on disk was replaced by an
update while it was open, it may have to be started again.

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
purchases, instalments, stamp duty and the previous debt, less the new one, to *Totale addebito*; the amount
under its heading on the first page, when there is one, is that too. The last day, the debit date and the
number of the card must have been read. If something fails, or a row cannot be read, nothing is imported
unless the user confirms; without the last day or the number of the card, not even then.

A row is known by its statement (last day and cards), date, amount, text and order: the same statement adds
nothing twice, and equal purchases of two statements are both kept.

## The payment

Importing a statement records its last day, debit date and amount in `card_statements`. Its payment may be a
transaction of a current account in the same currency, for that amount, at most 10 days from the debit date,
that can become a transfer ([ledger.md](ledger.md)). The candidates are listed and the user chooses. Linking
turns it into a transfer to the card, whose side is added. Lines of the card that may be the same payment are
shown with date, text, amount and category, and are joined only if the user says so; rows that came from a
card statement are purchases and refunds, and never shown. The purchases of the statement remain the only
expenses.

Whether a statement is paid is read from the books: the linked transaction is a transfer to that card. When
its amount was changed afterwards the statement shows as not matching. A transfer of the same amount near the
debit date, made by hand, counts as the payment, and the bank's line imported later confirms it. Unlinking,
or separating the transfer in the register, makes the bank's line uncategorized; the side of the card goes
only if it was added and no statement has confirmed it.

## Not verified yet

The layout was seen on one statement with its contents hidden. To be confirmed on others: how a refund is
written (minus before or after), that the amount of a foreign purchase contains the fee, several cards on one
statement, where the amount and the debit date are on the first page (an amount that is not found there is not checked), and lists that go on over more pages.
