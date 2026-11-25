# Importing statements

CSV, Excel 97 (`.xls`), OFX, and the PDF statements of credit cards ([cards.md](cards.md)). Files have at most
32 MB.

## CSV and Excel

The separator, encoding, headings, columns and decimal separator are found in the file and can be changed, with
the order of the dates (automatic, or day, month and year as given) and the sign. After an import these settings
are remembered in `csv_layouts` and used for the next file with the same separator and headings, whatever their
case, accents and spaces. A file without headings is not remembered: two banks may have as many columns. An
import that fails, or a trial, remembers nothing. "Forget them" in the window, `columns forget` and
`forget_saved_columns` go back to what is found. The 200 layouts used last are kept. Remembered columns that cannot be
read are an error, never replaced in silence.

A quotation mark that opens a value and is never closed refuses the file, with its line: it would swallow the
rest. So does a table with more than 1,000 columns or 2,000,000 cells. Rows that have something of a
transaction and cannot be read, such as a date with no valid amount, are skipped and listed with line and reason
(the first 100).

## OFX

Read by `ofx.rs`, written from the public specification: 1.x (SGML, values without end tags) and 2.x (XML),
`STMTRS` and `CCSTMTRS`. No entity is expanded but the standard and the numeric ones; declarations are refused.
The encoding is that of the mark, or what the header declares (`ENCODING`, `CHARSET`, `encoding`); one declared
UTF-8 that is not is read as Windows-1252, and one declared otherwise that is UTF-8 is read as UTF-8.

- What is wrong with a transaction or with the nesting refuses the whole file, with the line: a missing or
  invalid `DTPOSTED` or `TRNAMT`, a block that does not end, a file that is cut.
- `TRNAMT` is exact: a sign, digits, one point or comma. More than two decimals, unless zero, are refused. The
  sign is that of the file.
- The date is the day written in `DTPOSTED`; its time zone is checked and does not move it.
- The payee is `NAME`, else `PAYEE/NAME`, `EXTDNAME`, or what is found in `MEMO`; `MEMO` goes into the notes.
- `CURDEF` must be the currency of the account; a transaction in another one (`CURRENCY`) is refused.
- `LEDGERBAL` is compared with the balance of the books on its day, and shown. It never stops an import.
- With several statements the user chooses one; they show the last digits of their account.
- At most 100 statements and 100,000 transactions.

Investment statements, and the matching of an OFX account with one of CashCraft, are not there.

## What is there already

Each row has a key, unique in its account:

| Row | Key |
|---|---|
| OFX with `FITID` | FITID, date, amount, order |
| OFX without | date, amount, payee, order |
| CSV, Excel | date, amount, payee, notes, order |
| Card statement in PDF | see [cards.md](cards.md) |

A row whose key is known is skipped. Date and amount are part of it because some banks use a FITID more than
once. These rows may be there already, and the import is refused until the user says of each whether it is
the same:

- an OFX row with the amount of an imported transaction at most one day apart, or a row of another file with
  that of a transaction that came from OFX alone;
- a FITID that the account has with another date or amount (unless it is used for several in the file or in
  the account: then it tells nothing);
- a row that the file has twice, FITID and all;
- a correction (`CORRECTFITID`). It is never applied: whoever adds it changes the old transaction by hand.

A transaction can be the same as one row only; saying more is refused. What is skipped gives its key to the
transaction (`postings.other_hashes`), so that either file can be imported again without questions. Rows are
numbered by date, as in the preview. Two CSV files are compared by key alone. Transactions typed by hand are
not compared.

After that, sides of transfers are taken as for every import ([ledger.md](ledger.md)).

## Tests against damaged files

Each reader has a test that damages good files in thousands of ways from a fixed seed, for a limited time: CSV,
OFX and Excel in `importer.rs`, PDF in `apps/cashcraft-cli/tests/pdf.rs` through the process that reads it.
`cargo-fuzz` would need a nightly compiler, a linked fuzzer and a package outside the workspace.
