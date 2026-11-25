# Rules and tags

A rule has conditions and actions. It matches a transaction when **all** the conditions it has are met:

| Condition | Meaning |
|---|---|
| text in the payee or the notes, in the payee, in the notes | keywords separated by `\|`, or a regular expression |
| range of amounts | whatever the sign, both ends included |
| money in, money out | the sign |
| account | only its transactions; the rule goes when the account is deleted |

The case never counts (`(?-i)` in a regular expression makes it count). The spaces inside a keyword are part of it:
`bar |pub` finds the word "bar" and not "barber"; those at the two ends of the whole text are dropped when it is saved. The
old syntax `a|b` is the text in the payee or the notes. A keyword there may go from the end of the payee to the start of the
notes, as always; a regular expression looks at one text at a time, at its first 2,000 characters.

Actions: a category, tags, and a new payee. In a rule of regular expressions `$1`, `${1}` and `${name}` in the new payee
are what a group matched (of the expression of the payee, else of the one for both, else of the notes) and `$$` is a
dollar; a group that is not there is refused. A new payee that comes out empty changes nothing.

Deleting a category deletes the rules that give it, with their tags and new payee; the window and the operations say how many.

## Order

Rules run from the first to the last, in the order the user gives. The first that matches and has a category decides it. The
new payee is that of the first rule with one, up to the rule that decides the category. Tags are those of every rule that
matches. A rule made from a transaction goes before the others.

What is looked at is the payee the bank wrote, or the one that was typed for a transaction recorded by hand, with the
notes of the bank and those of the user; never what a rule wrote. So running twice is like running once. When a new payee
takes the place of one and there are no notes, the old one goes into the notes, as before format 8; those notes are marked
(`rules.memo`) and are not the user's.

## Whose text it is

`original_payee` and `original_memo` of a posting have the bank's text, kept by the import. Where they are not there, as in
files older than format 8, the keys of the import prove it: they are made of the text, and it is the bank's when the key made
from it is the one of the row (CSV and Excel, also with the payee moved into the notes; the payee alone for OFX). Editing
a transaction keeps what is proved first. All the rest may be the user's: it is never shown as the bank's nor compared as such
with another statement. When a rule takes its place, because the user asked or on recording, it stays in the same columns with the mark
`typed`, and is shown as typed.

## When they run

- On each transaction that is imported, or recorded without a category. Lines that take the place of the side of a transfer are
  left alone.
- On demand, after a preview: on the transactions without a category, or on all. On all, tags are added, payees cleaned up and
  the categories **set by rules** follow the rules; a category set by hand is replaced only when that is asked, and so are
  any payee that may be the user's (written by hand, put back to the bank's, or of unknown origin) and a tag that was removed. Notes are only
  ever filled, when they are empty and were not emptied by hand. Transfers, splits and entries on accounts of the chart
  are never touched. The preview has a token: a run that is given it is refused when the changes would be others by now.

The transaction keeps the ids of the rules that set its category, payee and tags (`book_postings.rules`). The mark of the category
counts only while that is its category, and goes when a category is chosen by hand, even the same one. The categories of
files older than format 8 count as set by hand.

## Undo

The last run on demand that changed something can be undone, once, also by another program and later (`rule_runs`,
`rule_changes`). All in one transaction. What was changed since in category, texts or tags stays as it is and is listed, as is what was
deleted or whose old category was; when that is all of them, nothing is done. The amount, the date and the cleared mark
may change meanwhile. The undo is given the number of the run, which is the last one, so that it is never another than the one that
was shown. What the rules did to an import is not a run.

## Imports

The keys of an import, the likely duplicates and the sides of transfers depend on the file and on the text of the bank, never on the
rules ([import.md](import.md)).

## Limits

Regular expressions are those of the `regex` crate: the time they take grows with the length of the text, whatever they are.
An expression has at most 500 characters and 256 KB when compiled; there are at most 500 rules, 20 tags for each rule or
transaction, 40 characters in a tag and 200 in a new payee. Tags given by hand come before those of the rules when there are too many. Joining two lines of which one goes keeps the tags
of both. What is not valid is refused when the rule is saved. A rule that is
damaged in the file is skipped and reported by lists, previews, imports and runs.
