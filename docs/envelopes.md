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

The envelopes start in a month (`meta.envelopes_from`): the month in which they were first chosen, or the one set
with `--start`. Nothing before it counts, so the income of past years does not land in *to be assigned*. Changing
it computes every month again from there. A month earlier than the start shows everything at zero; a month more than
1200 after it is refused. The start is at most 1200 months away from this month, so this month can always be shown; one
further away in a file changed by hand is taken as the furthest allowed.

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

Nothing of a month is stored but the changes: an amount assigned to an envelope (from *to be assigned*, or back to
it when negative), money moved from one envelope to another, each with its month, the time it was made and a
reason. `assign_envelope` takes the new amount assigned in the month and writes the difference. The table
`envelope_changes` is only ever added to: an undo (`undo_envelope_change`) writes the opposite of a change, as a
change of its own that cannot itself be undone, and the history keeps both.

Every figure is computed again from the ledger each time. So editing a past month — an assignment, a move, a
transaction — changes every month after it. That is what happens to money, and the screen says so: a past month
carries a note, and an assignment to it lists the months whose figures changed (`recomputed`).

Deleting an envelope makes every month as if it had never been: what was assigned to it is to be assigned again, the
spending of its categories is outside the envelopes, and its changes stay in the history as those of a deleted
envelope. A category that is deleted leaves its envelope; its transactions become uncategorized.

## Currencies

Envelopes are in euros. Postings in other currencies are left out and counted (`other_currencies`), with a warning:
there are no exchange rates in CashCraft, and an envelope per currency would split each budget in two.

## Limits

At most 500 envelopes; a category in one envelope at most, and only expense categories; names and reasons of at
most 200 characters; an amount of at most 10^13 euros a change; all the changes together, whatever their sign, at
most `MAX_TURNOVER`, so that every sum fits in 64 bits.

## The data file

Format 10 adds `envelopes`, `envelope_categories` and `envelope_changes`, after a copy as `cashcraft.v9-backup.db`
([ledger.md](ledger.md)). A window of a program of format 9 that has the file open goes on working: what it writes to the books is in
the envelopes, which are computed from them, and a category it deletes takes its link with it, as the foreign keys
are in the file. Nothing it can do leaves an envelope wrong, so no table is renamed to keep it out. New rows get
their ids from `Db::new_id` ([data-file.md](data-file.md)).

## Where

- Window: *Budget*, *Envelopes*. Arrows up and down choose an envelope, typing an amount or Enter starts assigning
  it, Enter saves, Esc cancels; arrows while typing save and move on; ⌘Z (Ctrl+Z) undoes the last change of the month shown.
- Command line: `cashcraft-cli envelopes list|add|edit|delete|assign|move|from-budgets|undo|settings`.
- MCP: `list_envelopes`, `add_envelope`, `edit_envelope`, `delete_envelope`, `assign_envelope`,
  `move_envelope_money`, `assign_budgets`, `undo_envelope_change`, `envelope_settings`.
- Phone: the budget tab shows the envelopes of the month when the file uses them, and an envelope is assigned by
  tapping it ([mobile.md](mobile.md)).
