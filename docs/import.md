# Importing statements

CSV, Excel 97 (`.xls`), OFX, the PDF statements of credit cards ([cards.md](cards.md)) and the lists of an
account's movements that the web sites of banks print to PDF. Files have at most 32 MB.

## CSV and Excel

The separator, encoding, headings, columns and decimal separator are found in the file and can be changed, with
the order of the dates (automatic, or day, month and year as given) and the sign. After an import these settings
are remembered in `csv_layouts` for that account, and used for the next file with the same separator and headings
(whatever their case and spaces) into it. Another account starts from what is found: the sign of a card is not
that of a bank account. What is given for a file wins over what is remembered, and a remembered sign is said
before anything is written. Nothing is remembered of a file without headings (two banks may have as many columns),
of an import that fails, of a trial, or when no row was read or more lines look wrong than were read; `--no-saved`
and `use_saved: false` neither use nor remember. "Forget them" in the window, `columns forget` and
`forget_saved_columns` go back to what is found. The 200 layouts used last are kept. Remembered columns that cannot
be read stop the import until they are forgotten.

With other columns than those found, the decimal separator stays that of the columns found, as in 0.3.0; when the
chosen ones seem to have the other one it is said.

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
- `TRNAMT` is exact: a sign, digits, one point or comma. More than two decimals are refused unless they are
  zeros; three digits after the separator always are: `1.500` may be a thousand and a half.
- The sign is that of the file, and can be inverted. When most types (`DEBIT`, `CREDIT`, `CHECK`, `DEP`…; not those
  that go either way) say the opposite, the import is refused until the user says which sign is right.
- The date is the day written in `DTPOSTED`; its time zone is checked and does not move it.
- The payee is `NAME`, else `PAYEE/NAME`, `EXTDNAME`, or what is found in `MEMO`; `MEMO` goes into the notes.
- `CURDEF` must be the currency of the account; a transaction in another one (`CURRENCY`) is refused.
- `LEDGERBAL` is compared with the balance of the books on its day, and shown. It never stops an import.
- With several statements the user chooses one; they show the last digits of their account.
- At most 100 statements and 100,000 transactions.

Investment statements, and the matching of an OFX account with one of CashCraft, are not there.

## What is there already

## Lists of movements in PDF

The web sites of many banks print the movements of a current account as a PDF list: a heading of columns
(*Data contabile*, *Data valuta*, *Importo*, *Descrizione*), a row for each movement, newest first, whose
description goes on over several lines and may start a little above its date, and the balances at the start and
at the end (*Saldo iniziale al…*, *Saldo finale al…*). The PDF is read in the process that reads card statements
([cards.md](cards.md)); its rows then become those of a table and are imported as a CSV file is, with columns that
can be changed and that are remembered for the account. The balances are not imported: when the list has both,
the movements must add up from one to the other, and a list with a row that could not be read or that does not
add up is refused whole. Lists of different periods may overlap: rows already imported are skipped.

The heading above the table names the account: the bank on its first line, and the IBAN, which is taken only when
its check digits are right. The list goes to the account with that IBAN; when none has it, the import proposes a
new current account named after the bank, with that IBAN and the list's opening balance as its own, and the name
can be changed or emptied to choose an account that is there. An account chosen that has no IBAN is given the
list's, so that the next list finds it; its opening balance is left as it is, since it may have older movements.
The closing balance is then checked against the books on its day, as the balance of an OFX statement is. The
preview of an agent (`preview_csv`) gives the same as `list`.

Each row has a key, unique in its account:

| Row | Key |
|---|---|
| OFX with a FITID that is believed | FITID, date, amount, order |
| Other OFX | date, amount, payee, order |
| CSV, Excel, list of movements in PDF | date, amount, payee, notes, order |
| Card statement in PDF | see [cards.md](cards.md) |

Rules change no key, and payees are compared with what the bank wrote ([rules.md](rules.md)).
A row whose key is known is skipped. Date and amount are in it because some banks use a FITID more than once. One
is not believed when the file or the books have it on several dates or amounts; none is when the bank numbers
each file again: FITIDs in order of which one is on something else in the books, or at least two and half of those
that are there.

These rows may be there already. The import is refused until the user says of each what it is:

- a row with the amount of a transaction that came from a file of another format, at most 5 days apart: the day
  of the purchase and that of the bank can be a weekend with a holiday apart. Only a question is asked. Between two
  OFX files it is one day, for they tell transactions apart by FITID and a daily coffee would be asked every time;
  two files of any other one format are compared by key alone;
- a FITID that is believed and that the account has with another date or amount;
- a row that the file has twice, FITID and all;
- one that replaces another (`CORRECTFITID`): it can be added, and the user deletes the old one, or skipped.

For each row the transactions it may be are shown, the likeliest first: what is exactly the same, the same date, the
payee, the distance. One is proposed, and the user can choose another. Rows and transactions are paired in this
order: same date and payee; same date and a payee alike (one contains the other, or a word of four letters in
common); same date; then other dates, the same payee first, then alike, then any, the nearest first. What comes
later never takes what comes before. A transaction is one row only. "Surely the same" is the
same date, amount and payee with no other row or transaction like it: only those can be skipped together
(`same`); the others one by one. What is skipped gives its key to the transaction (`book_postings.other_hashes`),
so that either file can be imported again without questions. Rows are numbered by date, as in the preview.
Transactions typed by hand are not compared.

What the bank deletes (`CORRECTACTION` `DELETE`) is never added. If the transaction is there it is said, until an
import is made, and the user deletes it.

After that, sides of transfers are taken as for every import ([ledger.md](ledger.md)).

## Tests against damaged files

Each reader has a test that damages good files in thousands of ways from a fixed seed, for a limited time: CSV,
OFX and Excel in `importer.rs`, PDF in `apps/cashcraft-cli/tests/pdf.rs` through the process that reads it; the
words of a list of movements, in any place and order, in `bank_pdf/tests.rs`.
`cargo-fuzz` would need a nightly compiler, a linked fuzzer and a package outside the workspace.
