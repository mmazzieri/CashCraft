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

## Order

Rules run from the first to the last, in the order the user gives. The first that matches and has a category decides it. The
new payee is that of the first rule with one, up to the rule that decides the category. Tags are those of every rule that
matches. A rule made from a transaction goes before the others.

What is looked at is the text the bank wrote (`original_payee`, `original_memo` of the posting), so that cleaning up twice is
like once; when the user wrote another payee or other notes, those are looked at too. When a new payee takes the
place of one and there are no notes, the old one goes into the notes, as before format 8.

## When they run

- On each transaction that is imported, or recorded without a category. Lines that take the place of the side of a transfer are
  left alone.
- On demand, after a preview: on the transactions without a category, or on all. On all, tags are added, payees cleaned up and
  the categories **set by rules** follow the rules; a category set by hand is replaced only when that is asked, and so are
  a payee that was written instead of the bank's and a tag that was removed. Transfers, splits and entries on accounts of the chart
  are never touched. The preview has a token: a run that is given it is refused when the changes would be others by now.

The transaction keeps the ids of the rules that set its category, payee and tags (`book_postings.rules`). The mark of the category
counts only while that is its category, and goes when a category is chosen by hand, even the same one. The categories of
files older than format 8 count as set by hand.

## Undo

The last run on demand that changed something can be undone, once, also by another program and later (`rule_runs`,
`rule_changes`). All in one transaction. What was changed since in category, texts or tags stays as it is and is listed, as is what was
deleted or whose old category was; when that is all of them, nothing is done. The amount, the date and the cleared mark
may change meanwhile. What the rules did to an import is not a run.

## Imports

The keys of an import, the likely duplicates and the sides of transfers depend on the file and on the text of the bank, never on the
rules ([import.md](import.md)).

## Limits

Regular expressions are those of the `regex` crate: the time they take grows with the length of the text, whatever they are.
An expression has at most 500 characters and 256 KB when compiled; there are at most 500 rules, 20 tags for each rule or
transaction, 40 characters in a tag and 200 in a new payee. What is not valid is refused when the rule is saved. A rule that is
damaged in the file is skipped and reported by lists, previews, imports and runs.
