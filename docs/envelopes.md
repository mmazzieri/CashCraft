# Envelopes

A zero-based budget: every euro that arrives waits in *to be assigned* until it is given to an envelope, and an
envelope holds what it was given and not yet spent. It sits beside the monthly budgets of the categories, which
stay as they are.

## Monthly limits or envelopes

Each data file chooses what its budget screen shows (`envelope_settings`, `envelopes settings --mode`, the two
buttons at the top of the screen; `meta.budget_mode`). Both live in the file whatever is chosen: switching back
and forth loses nothing.

The first time envelopes are chosen, when the file has none and no change was ever made, each expense category with
a monthly budget becomes an envelope linked to it, named after it, with that budget assigned in the first month.
After that the monthly budgets go on as a guide: each envelope shows the sum of the budgets of its categories
(`budget`), and *Assign the monthly budgets* (`assign_budgets`) gives each envelope that sum in a month, as one
change undone at once. One difference: a monthly budget counts the categories under its own even when they have a
budget too, while an envelope does not count those that are in another envelope.

## The first month

The envelopes start in a month (`meta.envelopes_from`). Nothing before it counts, so the income of past years does not
land in *to be assigned*. It is fixed by the first change of the envelopes, whoever makes it — choosing the mode, adding
an envelope, assigning, over the window, the command line, MCP or the phone — at the month of that change or this one,
whichever comes first; until then it is this month, and nothing depends on it. It moves only when the user moves it
(`envelope_settings start`, `--start`): earlier at once; later only with `confirm`, when money assigned or moved before
the new first month would stop counting. The answer lists the months whose figures changed.

A month earlier than the start shows everything at zero; a month more than 1200 after it is refused. The start is at
most 1200 months away from this month, so this month can always be shown; one further away in a file changed by hand
is taken as the furthest allowed.

The money already in the accounts at the start is not income: opening balances are equity. To budget it, record
it as income in the first month, or start from an empty month and let the next salary fill the envelopes.

## A month

For each month, from the first one on:

- **Income**: the postings in euros on income categories, with refunds of income taken off.
- **Spent** by an envelope: the postings in euros on its categories and on those under them, unless one of those has
  an envelope of its own (the nearest envelope above a category takes it). Refunds take spending down, so an
  envelope can spend less than zero. Each posting of a split counts in its own category. Transfers between accounts
  of the user, and so the payment of a credit card, are neither income nor spending; a purchase with the card is
  spending on the day and in the category of the purchase ([cards.md](cards.md)).
- **Outside the envelopes**: spending in expense categories with no envelope, and the uncategorized money, out minus
  in. Both are shown as warnings and taken from *to be assigned*, so that no euro is lost.
- **Carried**: what the envelope had at the end of the month before.
- **Available** = carried + assigned − spent.
- **To be assigned** = what was left the month before (below zero when more was assigned than there was) + income −
  outside − uncategorized − what overspent envelopes took from it (below) − assigned this month. It must reach zero;
  below zero is shown as over-assigned.

So at the end of every month the envelopes and *to be assigned* hold, together, exactly the income less the spending
since the first month: a test checks it on random months against a count of its own.

Envelopes are taken by the category, not by the sign: a refund on a category of spending makes spending smaller,
while the reports count it as income ([ledger.md](ledger.md#reports)).

## Overspending

An envelope that ends a month below zero is shown in red, and money can be moved into it from another
(`move_envelope_money`, *Cover…*). What it still lacks at the end of the month depends on the envelope:

- `carry` (the default): it starts the next month below zero, until money is moved in. The overspending stays where
  it happened, in sight, and *to be assigned* is not touched behind the user's back.
- `next_month`: it starts the next month at zero, and what it lacked is taken from the next month's *to be
  assigned* (`overspent_before`).

Changing it changes every month, as it is a setting of the envelope.

## Changes

Nothing of a month is stored but the changes (`envelope_changes`), each with its month, the time it was made and a
reason:

- an amount assigned to an envelope (from *to be assigned*, or back to it when negative): `assign_envelope` takes the
  new amount assigned in the month and writes the difference;
- money moved from one envelope to another;
- a category put in an envelope, or taken out, **from a month on**: the months before keep the envelope they had
  (`month` of `add_envelope` and `edit_envelope`, the month shown in the window, this month by default). A category is
  in one envelope at most in each month; to move it, take it out of one and put it in the other;
- an envelope deleted.

The table is only ever added to. An undo (`undo_envelope_change`, ⌘Z in the window) is a change of its own that names
the group it undoes, which then counts no more; it cannot itself be undone, and the history keeps both.

Every figure is computed again from the ledger each time. So editing a past month — an assignment, a move, a category
put in an envelope, a transaction — changes every month after it. That is what happens to money, and it is shown: a
past month carries a note, and every change answers with the months whose figures changed (`recomputed`).

Deleting an envelope makes every month as if it had never been: what was assigned to it is to be assigned again and
the spending of its categories is outside the envelopes. The window asks first, saying what goes back. The envelope
stays in the file, so its id is never given to another and its changes keep its name; undoing the deletion brings it
back as it was, unless another envelope has taken its name. A category that is deleted leaves its envelope; its
transactions become uncategorized.

Envelopes are named by their id or their whole name, whatever the case; a text that is the id of one and the name of
another is refused. Nothing is found by a part of its name.

When two computers changed the envelopes of a shared file at once, the conflict copy is kept as for any change
([data-file.md](data-file.md)), and *Show what differs* lists, besides the entries, the changes of the envelopes in
force in one file alone.

## Currencies

Envelopes are in euros. Postings in other currencies are left out and counted (`other_currencies`), with a warning:
there are no exchange rates in CashCraft, and an envelope per currency would split each budget in two.

## Limits

At most 500 envelopes; a category in one envelope at most, and only expense categories; names and reasons of at
most 200 characters; what an envelope is assigned in a month, and so each change, of at most 10^13 euros. There is no
limit on how many changes there are: sums are made in 128 bits, a figure that would not fit in 64 is an error of that
month and not of the file, and an undo is always allowed.

## The data file

Format 10 adds `envelopes` and `envelope_changes`, after a copy as `cashcraft.v9-backup.db`
([ledger.md](ledger.md)). A window of a program of format 9 that has the file open goes on working: what it writes to the books is in
the envelopes, which are computed from them, and a category it deletes leaves its envelope, as the foreign keys
are in the file. Nothing it can do leaves an envelope wrong, so no table is renamed to keep it out. New rows get
their ids from `Db::new_id` ([data-file.md](data-file.md)).

## Where

- Window: *Budget*, *Envelopes*. Arrows up and down choose an envelope, typing an amount or Enter starts assigning
  it, Enter saves, Esc cancels; arrows while typing save and move on; ⌘Z (Ctrl+Z) undoes the last change of the month shown.
  What is typed belongs to its month: going to another month or screen, or clicking elsewhere, drops it. *Cover…*
  moves money into an overspent envelope from the one with the most left, or another chosen.
- Command line: `cashcraft-cli envelopes list|add|edit|delete|assign|move|from-budgets|undo|settings`.
- MCP: `list_envelopes`, `add_envelope`, `edit_envelope`, `delete_envelope`, `assign_envelope`,
  `move_envelope_money`, `assign_budgets`, `undo_envelope_change`, `envelope_settings`.
- Phone: the budget tab shows the envelopes of the month when the file uses them, and an envelope is assigned by
  tapping it ([mobile.md](mobile.md)).
