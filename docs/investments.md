# Investment accounts

An account of the kind *Investment* (a fund, a managed portfolio, a pension plan) has a value of its own, which is not
the money that went in. The user writes it down from time to time, as the bank or the platform shows it: *Write the
value* above the register, `cashcraft-cli accounts value <account> <value> [--date …]`, or `record_valuation` over MCP.

## In the books

Every value is an entry dated on its day, between the account and *Changes in value* (`system = market_value`, equity),
for the difference between the value and the balance the account would have on that day without it. So:

- the balance, the register, the net worth and its history count the investment at its value, and remain sums of
  postings ([ledger.md](ledger.md)): the net worth on a day is the sum of the balances on that day;
- a gain or a loss is neither income nor spending: reports and budgets do not see it;
- after the last value, the balance is that value with the money moved since.

The value itself is kept (`valuations`: the entry, the account and the value, since format 11). When a transaction of
the account is added, changed or deleted, every entry of its values is worked out again, in order of date, at the end
of the same change: a contribution of last month written down today leaves each value as it was written. A second
value of the same day replaces the first. The entry of a value cannot be edited in the register (it would no longer be
the value); deleting it there, or deleting the value, takes it away, and the values after it are worked out again.

Only accounts of the kind *Investment* take values; a value is zero or more, at most a thousand billion euros.

## What is shown

Above the register of an investment account: what it is worth, what was put in (the money moved in less the money
taken out, with the opening balance) and the gain (every change in value, with its share of what was put in), the
values written and their changes. `cashcraft-cli accounts values` and `list_valuations` give the same.

Quantities and prices of single securities, and fetching prices from the network, are not part of this.
