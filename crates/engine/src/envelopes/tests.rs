use super::*;
use crate::db::{AccountKind, NewTx, TxEdit};
use crate::ledger::tests::assert_sound;
use crate::ledger::{NewEntry, NewLine};
use crate::money::parse_date;
use crate::testing::Seeded;

fn day(s: &str) -> NaiveDate {
    parse_date(s).unwrap()
}

fn month(s: &str) -> NaiveDate {
    parse_month(s).unwrap()
}

struct Books {
    db: Db,
    bank: i64,
    card: i64,
    savings: i64,
    dollars: i64,
    salary: i64,
    food: i64,
    restaurant: i64,
    home: i64,
    rent: i64,
    fun: i64,
    misc: i64,
}

fn books() -> Books {
    let db = Db::open_in_memory().unwrap();
    let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 50_000, "").unwrap();
    let card = db.add_account("Card", AccountKind::CreditCard, "EUR", 0, "").unwrap();
    let savings = db.add_account("Savings", AccountKind::Savings, "EUR", 0, "").unwrap();
    let dollars = db.add_account("Dollars", AccountKind::Bank, "USD", 0, "").unwrap();
    let cat = |p: &str, income: bool| db.category_by_path(p, income, true).unwrap();
    let b = Books {
        bank,
        card,
        savings,
        dollars,
        salary: cat("Pay", true),
        food: cat("Food", false),
        restaurant: cat("Food:Restaurant", false),
        home: cat("House", false),
        rent: cat("House:Rent", false),
        fun: cat("Fun", false),
        misc: cat("Misc", false),
        db,
    };
    b.db.set_envelopes_start(month("2026-01"), false).unwrap();
    b.db.set_budget_mode(Mode::Envelopes).unwrap();
    b
}

fn tx(b: &Books, account: i64, date: &str, amount: Cents, category: Option<i64>) -> i64 {
    let t = NewTx {
        account_id: account,
        date: day(date),
        amount,
        payee: "x".into(),
        category_id: category,
        ..Default::default()
    };
    b.db.add_transaction(&t).unwrap().unwrap()
}

fn view(db: &Db, m: &str) -> MonthView {
    db.envelope_month(month(m)).unwrap()
}

fn line<'a>(v: &'a MonthView, name: &str) -> &'a EnvelopeLine {
    v.envelopes.iter().find(|e| e.name == name).unwrap()
}

/// Every euro in envelopes or in *to be assigned* is a euro that came in and was not spent.
fn assert_whole(db: &Db, m: &str) {
    let v = view(db, m);
    let (from, _) = crate::db::month_range(month(&v.start));
    let (_, to) = crate::db::month_range(month(m));
    let net: i64 = db
        .conn
        .query_row(
            "SELECT COALESCE(-SUM(p.amount),0) FROM journal_postings p JOIN entries e ON e.id=p.entry_id
             JOIN ledger_accounts l ON l.id=p.ledger_account_id
             WHERE l.class IN ('income','expense') AND p.currency='EUR' AND e.date>=?1 AND e.date<=?2",
            params![crate::db::d2s(from), crate::db::d2s(to)],
            |r| r.get(0),
        )
        .unwrap();
    let held: i64 = v.envelopes.iter().map(|e| e.available).sum::<i64>() + v.to_be_assigned;
    assert_eq!(held, net, "{m}");
}

#[test]
fn assigning_all_income_leaves_nothing_to_assign() {
    let b = books();
    let food = b.db.add_envelope("Food", &[b.food], Overspending::Carry, b.db.envelopes_start()).unwrap();
    let home = b.db.add_envelope("Home", &[b.home], Overspending::Carry, b.db.envelopes_start()).unwrap();
    tx(&b, b.bank, "2026-01-27", 200_000, Some(b.salary));
    tx(&b, b.bank, "2026-01-10", 30_000, Some(b.salary));
    let v = view(&b.db, "2026-01");
    assert_eq!((v.income, v.to_be_assigned), (230_000, 230_000));
    b.db.assign(food, month("2026-01"), 50_000, "").unwrap();
    b.db.assign(home, month("2026-01"), 180_000, "rent and bills").unwrap();
    let v = view(&b.db, "2026-01");
    assert_eq!((v.assigned, v.to_be_assigned), (230_000, 0));
    // One more is over-assigned, and shown below zero.
    b.db.assign(home, month("2026-01"), 180_001, "").unwrap();
    assert_eq!(view(&b.db, "2026-01").to_be_assigned, -1);
    // The same amount again changes nothing.
    assert_eq!(b.db.assign(home, month("2026-01"), 180_001, "").unwrap(), None);
    let money = b.db.changes(None).unwrap().into_iter().filter(|c| c.kind == ChangeKind::Assign).count();
    assert_eq!(money, 3);
    assert_whole(&b.db, "2026-01");
}

#[test]
fn overspending_in_march_is_carried_into_april() {
    for policy in [Overspending::Carry, Overspending::NextMonth] {
        let b = books();
        b.db.set_envelopes_start(month("2026-03"), false).unwrap();
        let food = b.db.add_envelope("Food", &[b.food], policy, b.db.envelopes_start()).unwrap();
        let fun = b.db.add_envelope("Fun", &[b.fun], Overspending::Carry, b.db.envelopes_start()).unwrap();
        tx(&b, b.bank, "2026-03-01", 100_000, Some(b.salary));
        b.db.assign(food, month("2026-03"), 40_000, "").unwrap();
        b.db.assign(fun, month("2026-03"), 60_000, "").unwrap();
        tx(&b, b.bank, "2026-03-20", -45_000, Some(b.food));
        tx(&b, b.card, "2026-03-21", -10_000, Some(b.fun));
        let march = view(&b.db, "2026-03");
        assert_eq!(line(&march, "Food").available, -5_000);
        assert_eq!(march.overspent().map(|e| e.id).collect::<Vec<_>>(), [food]);
        assert_eq!(march.to_be_assigned, 0);
        let april = view(&b.db, "2026-04");
        assert_eq!(line(&april, "Fun").carried, 50_000);
        match policy {
            Overspending::Carry => {
                assert_eq!((line(&april, "Food").carried, line(&april, "Food").available), (-5_000, -5_000));
                assert_eq!((april.overspent_before, april.to_be_assigned), (0, 0));
            }
            Overspending::NextMonth => {
                assert_eq!((line(&april, "Food").carried, line(&april, "Food").available), (0, 0));
                assert_eq!((april.overspent_before, april.to_be_assigned), (5_000, -5_000));
            }
        }
        assert_whole(&b.db, "2026-04");
        // Covered in March by moving money from Fun: April starts at zero, and nothing is taken.
        let g = b.db.move_money(fun, food, month("2026-03"), 5_000, "dinner").unwrap();
        let april = view(&b.db, "2026-04");
        assert_eq!((line(&april, "Food").carried, april.to_be_assigned, line(&april, "Fun").carried), (0, 0, 45_000));
        let m = &view(&b.db, "2026-03").changes[0];
        assert_eq!(
            (m.kind, m.from.as_deref(), m.to.as_deref(), m.reason.as_str()),
            (ChangeKind::Move, Some("Fun"), Some("Food"), "dinner")
        );
        // Undone, it is as before; the history keeps both.
        let before = b.db.fingerprints().unwrap();
        b.db.undo_envelope_change(Some(g)).unwrap();
        assert_eq!(line(&view(&b.db, "2026-03"), "Food").available, -5_000);
        let changed = b.db.months_changed(&before).unwrap();
        assert!(changed.contains(&"2026-04".to_string()) && changed.contains(&"2026-11".to_string()), "{changed:?}");
        let all = b.db.changes(None).unwrap();
        // Two categories put in, two assignments, the move and its undo.
        assert_eq!(all.len(), 6);
        assert!(all.iter().any(|c| c.group_id == g && c.undone));
        assert!(b.db.undo_envelope_change(Some(g)).is_err());
        assert!(b.db.undo_envelope_change(Some(all[0].group_id)).is_err());
        assert_whole(&b.db, "2026-11");
    }
}

#[test]
fn transfers_and_card_payments_are_not_spending_and_card_purchases_are() {
    let b = books();
    let food = b.db.add_envelope("Food", &[b.food], Overspending::Carry, b.db.envelopes_start()).unwrap();
    tx(&b, b.card, "2026-02-03", -2_500, Some(b.food));
    b.db.add_transfer(b.bank, b.card, day("2026-02-25"), 2_500, "card").unwrap();
    b.db.add_transfer(b.bank, b.savings, day("2026-02-26"), 10_000, "save").unwrap();
    let v = view(&b.db, "2026-02");
    let l = line(&v, "Food");
    assert_eq!((l.id, l.spent, v.outside, v.uncategorized, v.income), (food, 2_500, 0, 0, 0));
    assert_eq!((l.available, v.to_be_assigned), (-2_500, 0));
    assert_whole(&b.db, "2026-02");
    assert_sound(&b.db, "transfers");
}

#[test]
fn splits_count_in_each_category_and_refunds_take_spending_down() {
    let b = books();
    b.db.add_envelope("Food", &[b.food], Overspending::Carry, b.db.envelopes_start()).unwrap();
    b.db.add_envelope("House", &[b.home], Overspending::Carry, b.db.envelopes_start()).unwrap();
    let l = |id| NewLine { ledger_account_id: id, amount: 0, currency: None };
    let (bank, food, home) = (
        b.db.money_ledger(b.bank).unwrap().0,
        b.db.category_ledger(Some(b.food)).unwrap(),
        b.db.category_ledger(Some(b.home)).unwrap(),
    );
    let lines = vec![
        NewLine { amount: -9_000, ..l(bank) },
        NewLine { amount: 6_000, ..l(food) },
        NewLine { amount: 3_000, ..l(home) },
    ];
    b.db.post_entry(&NewEntry { date: day("2026-05-02"), payee: "Market".into(), memo: String::new(), lines }).unwrap();
    // A refund on the restaurant, under Food.
    tx(&b, b.bank, "2026-05-09", 1_500, Some(b.restaurant));
    let v = view(&b.db, "2026-05");
    assert_eq!((line(&v, "Food").spent, line(&v, "House").spent), (4_500, 3_000));
    assert_eq!(v.income, 0);
    assert_whole(&b.db, "2026-05");
}

#[test]
fn the_nearest_envelope_above_a_category_takes_its_spending() {
    let b = books();
    b.db.add_envelope("House", &[b.home], Overspending::Carry, b.db.envelopes_start()).unwrap();
    b.db.add_envelope("Rent", &[b.rent], Overspending::Carry, b.db.envelopes_start()).unwrap();
    tx(&b, b.bank, "2026-01-02", -70_000, Some(b.rent));
    tx(&b, b.bank, "2026-01-03", -5_000, Some(b.home));
    tx(&b, b.bank, "2026-01-04", -800, Some(b.misc));
    tx(&b, b.bank, "2026-01-05", -300, None);
    tx(&b, b.bank, "2026-01-06", 100, None);
    let v = view(&b.db, "2026-01");
    assert_eq!(
        (line(&v, "Rent").spent, line(&v, "House").spent, v.outside, v.uncategorized),
        (70_000, 5_000, 800, 200)
    );
    assert_eq!(v.to_be_assigned, -1_000);
    // A category is in one envelope only, and income is not in any.
    let other = b.db.add_envelope("Other", &[], Overspending::Carry, b.db.envelopes_start()).unwrap();
    assert!(b.db.edit_envelope(other, None, Some((&[b.rent], month("2026-01"))), None).is_err());
    assert!(b.db.edit_envelope(other, None, Some((&[b.salary], month("2026-01"))), None).is_err());
    assert!(b.db.add_envelope("rent", &[], Overspending::Carry, b.db.envelopes_start()).is_err());
    assert!(b.db.add_envelope("  ", &[], Overspending::Carry, b.db.envelopes_start()).is_err());
    // A category that goes takes its link with it; its spending is then uncategorized.
    b.db.delete_category(b.rent).unwrap();
    let v = view(&b.db, "2026-01");
    assert_eq!((line(&v, "Rent").spent, v.uncategorized), (0, 70_200));
    assert_whole(&b.db, "2026-01");
}

#[test]
fn other_currencies_are_left_out_and_counted() {
    let b = books();
    b.db.add_envelope("Food", &[b.food], Overspending::Carry, b.db.envelopes_start()).unwrap();
    tx(&b, b.dollars, "2026-01-02", -1_000, Some(b.food));
    tx(&b, b.dollars, "2026-01-03", 5_000, Some(b.salary));
    let v = view(&b.db, "2026-01");
    assert_eq!((line(&v, "Food").spent, v.income, v.other_currencies), (0, 0, 2));
    assert_whole(&b.db, "2026-01");
}

#[test]
fn monthly_budgets_become_envelopes_the_first_time() {
    let db = Db::open_in_memory().unwrap();
    let food = db.category_by_path("Food", false, true).unwrap();
    let pay = db.category_by_path("Pay", true, true).unwrap();
    db.set_budget(food, 30_000).unwrap();
    db.set_budget(pay, 100_000).unwrap();
    assert_eq!(db.budget_mode(), Mode::Monthly);
    assert_eq!(db.set_budget_mode(Mode::Envelopes).unwrap(), 1);
    let this = first_of_month(today());
    let v = db.envelope_month(this).unwrap();
    assert_eq!((v.mode, v.start.clone()), (Mode::Envelopes, month_key(this)));
    let e = line(&v, "Food");
    assert_eq!((e.assigned, e.budget, e.categories.clone()), (30_000, 30_000, vec!["Food".to_string()]));
    // The budgets stay, and the screen can go back to them.
    assert_eq!(db.set_budget_mode(Mode::Monthly).unwrap(), 0);
    assert_eq!(db.budget_status(this).unwrap().len(), 2);
    assert_eq!(db.set_budget_mode(Mode::Envelopes).unwrap(), 0);
    assert_eq!(db.envelopes().unwrap().len(), 1);
    // Next month, the budgets are assigned again in one change, undone at once.
    let next = this.checked_add_months(Months::new(1)).unwrap();
    db.set_budget(food, 32_000).unwrap();
    let g = db.assign_budgets(next).unwrap().unwrap();
    assert_eq!(line(&db.envelope_month(next).unwrap(), "Food").assigned, 32_000);
    assert_eq!(db.assign_budgets(next).unwrap(), None);
    assert_eq!(db.undo_envelope_change(None).unwrap(), db.changes(None).unwrap()[0].group_id);
    assert!(db.changes(None).unwrap().iter().any(|c| c.group_id == g && c.undone));
    assert_eq!(line(&db.envelope_month(next).unwrap(), "Food").assigned, 0);
}

#[test]
fn deleting_an_envelope_gives_back_what_it_held() {
    let b = books();
    let food = b.db.add_envelope("Food", &[b.food], Overspending::Carry, b.db.envelopes_start()).unwrap();
    let fun = b.db.add_envelope("Fun", &[b.fun], Overspending::Carry, b.db.envelopes_start()).unwrap();
    tx(&b, b.bank, "2026-01-02", 10_000, Some(b.salary));
    b.db.assign(food, month("2026-01"), 6_000, "").unwrap();
    b.db.assign(fun, month("2026-01"), 4_000, "").unwrap();
    b.db.move_money(fun, food, month("2026-01"), 1_000, "").unwrap();
    tx(&b, b.bank, "2026-01-03", -2_000, Some(b.food));
    let before = view(&b.db, "2026-01");
    let g = b.db.delete_envelope(food, month("2026-01")).unwrap();
    let v = view(&b.db, "2026-01");
    assert_eq!((v.outside, line(&v, "Fun").available, v.to_be_assigned), (2_000, 3_000, 5_000));
    assert!(v.envelopes.iter().all(|e| e.id != food));
    let c = &v.changes[0];
    assert_eq!((c.kind, c.to.as_deref()), (ChangeKind::Delete, Some("Food")));
    assert_whole(&b.db, "2026-01");
    assert!(b.db.delete_envelope(food, month("2026-01")).is_err());
    assert!(b.db.find_envelope("Food").is_err());
    // Its id is never given again, and while another envelope has its name it cannot come back.
    let other = b.db.add_envelope("food", &[], Overspending::Carry, month("2026-01")).unwrap();
    assert!(other > food);
    assert!(b.db.undo_envelope_change(Some(g)).is_err());
    b.db.edit_envelope(other, Some("Groceries"), None, None).unwrap();
    b.db.undo_envelope_change(Some(g)).unwrap();
    let back = view(&b.db, "2026-01");
    assert_eq!(line(&back, "Food"), line(&before, "Food"));
    assert_eq!(back.to_be_assigned, before.to_be_assigned);
}

#[test]
fn months_out_of_range_and_large_amounts_are_refused() {
    let b = books();
    let food = b.db.add_envelope("Food", &[b.food], Overspending::Carry, b.db.envelopes_start()).unwrap();
    let fun = b.db.add_envelope("Fun", &[b.fun], Overspending::Carry, b.db.envelopes_start()).unwrap();
    let v = view(&b.db, "2025-12");
    assert!(v.before_start && v.envelopes.iter().all(|e| e.available == 0));
    assert!(b.db.assign(food, month("2025-12"), 1, "").is_err());
    assert!(b.db.envelope_month(month("9999-12")).is_err());
    assert!(b.db.assign(food, month("9999-12"), 1, "").is_err());
    assert!(b.db.set_envelopes_start(month("9999-12"), false).is_err());
    // The oldest first month that may be chosen still shows this month; one older is refused.
    let this = first_of_month(today());
    let oldest = this.checked_sub_months(Months::new(MAX_MONTHS)).unwrap();
    assert!(b.db.set_envelopes_start(oldest.checked_sub_months(Months::new(1)).unwrap(), false).is_err());
    b.db.set_envelopes_start(oldest, true).unwrap();
    assert!(!b.db.envelope_month(this).unwrap().before_start);
    // One written by hand is taken as the oldest.
    b.db.set_meta("envelopes_from", "1066-10").unwrap();
    assert_eq!(b.db.envelopes_start(), oldest);
    b.db.envelope_month(this).unwrap();
    b.db.set_envelopes_start(month("2026-01"), true).unwrap();
    assert!(b.db.assign(food, month("2026-01"), MAX_BALANCE + 1, "").is_err());
    assert!(b.db.assign(food, month("2026-01"), i64::MIN, "").is_err());
    assert!(b.db.move_money(food, fun, month("2026-01"), 0, "").is_err());
    assert!(b.db.move_money(food, fun, month("2026-01"), -5, "").is_err());
    assert!(b.db.move_money(food, food, month("2026-01"), 5, "").is_err());
    assert!(b.db.move_money(food, 999, month("2026-01"), 5, "").is_err());
    assert!(b.db.assign(food, month("2026-01"), 1, &"x".repeat(MAX_TEXT + 1)).is_err());
    assert!(b.db.undo_envelope_change(Some(424242)).is_err());
    // As large as allowed, again and again: never blocked, never beyond what a month shows.
    for n in 0..300 {
        b.db.assign(food, month("2026-01"), if n % 2 == 0 { MAX_BALANCE } else { -MAX_BALANCE }, "").unwrap();
        b.db.move_money(fun, food, month("2026-02"), MAX_BALANCE, "").ok();
        b.db.move_money(food, fun, month("2026-02"), MAX_BALANCE, "").ok();
    }
    // A move that would take what one is assigned beyond the limit is refused.
    b.db.assign(fun, month("2026-03"), MAX_BALANCE, "").unwrap();
    assert!(b.db.move_money(food, fun, month("2026-03"), 1, "").is_err());
    assert!(b.db.undo_envelope_change(None).is_ok());
    view(&b.db, "2026-12");
    assert_whole(&b.db, "2026-12");
}

#[test]
fn a_copy_gives_ids_that_no_other_computer_gives() {
    let dir = std::env::temp_dir().join(format!("cashcraft-envelope-ids-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (db, _) = Db::open_copy(&dir.join("books.db"), crate::i18n::Lang::En).unwrap();
    let e = db.add_envelope("Food", &[], Overspending::Carry, db.envelopes_start()).unwrap();
    let g = db.assign(e, first_of_month(today()), 100, "").unwrap().unwrap();
    assert!(e > 100_000_000 && g > 100_000_000, "{e} {g}");
    drop(db);
    std::fs::remove_dir_all(&dir).unwrap();
}

// ------------------------------------------------------------------------------------------------- the oracle

const MONTHS: usize = 12;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cat {
    Pay,
    Food,
    Restaurant,
    House,
    Rent,
    Fun,
    Misc,
    None,
}

const CATS: [Cat; 8] = [Cat::Pay, Cat::Food, Cat::Restaurant, Cat::House, Cat::Rent, Cat::Fun, Cat::Misc, Cat::None];
/// Those that may be in an envelope.
const LINKABLE: [Cat; 6] = [Cat::Food, Cat::Restaurant, Cat::House, Cat::Rent, Cat::Fun, Cat::Misc];

fn parent(c: Cat) -> Option<Cat> {
    match c {
        Cat::Restaurant => Some(Cat::Food),
        Cat::Rent => Some(Cat::House),
        _ => None,
    }
}

/// What the test knows of a transaction: its month (None before January), whether in dollars, and the debits on
/// categories.
#[derive(Clone, Debug)]
struct Known {
    kind: Kept,
    month: Option<usize>,
    dollars: bool,
    lines: Vec<(Cat, Cents)>,
}

#[derive(Clone, Copy, Debug)]
enum Kept {
    Tx(i64),
    Split(i64),
}

/// One row of a change, as the test keeps it.
#[derive(Clone, Copy, Debug)]
enum Did {
    /// Month, from, to, amount.
    Money(usize, Option<usize>, Option<usize>, Cents),
    /// From this month on, the category is in this envelope, or in none.
    Link(usize, Cat, Option<usize>),
    Delete(usize),
}

#[derive(Clone, Debug, Default)]
struct Oracle {
    txs: Vec<Known>,
    /// Groups in the order they were made: their id, rows, and whether undone.
    groups: Vec<(i64, Vec<Did>, bool)>,
    next_month: [bool; 4],
    start: usize,
}

#[derive(Debug, PartialEq, Eq)]
struct Expected {
    left_over: Cents,
    income: Cents,
    outside: Cents,
    uncategorized: Cents,
    overspent_before: Cents,
    assigned: Cents,
    to_be_assigned: Cents,
    other: i64,
    /// Of the envelopes that are not deleted.
    lines: Vec<(usize, [Cents; 4])>,
}

impl Oracle {
    fn rows(&self) -> impl Iterator<Item = &Did> {
        self.groups.iter().filter(|g| !g.2).flat_map(|g| g.1.iter())
    }

    fn deleted(&self, e: usize) -> bool {
        self.rows().any(|d| matches!(d, Did::Delete(x) if *x == e))
    }

    /// The envelope of each category in a month, deleted envelopes being none.
    fn links(&self, m: usize) -> Vec<(Cat, usize)> {
        let mut all: Vec<(usize, usize, Cat, Option<usize>)> = Vec::new();
        for (n, d) in self.rows().enumerate() {
            if let Did::Link(month, c, e) = d
                && *month <= m
            {
                all.push((*month, n, *c, *e));
            }
        }
        all.sort_by_key(|l| (l.0, l.1));
        let mut out: Vec<(Cat, Option<usize>)> = Vec::new();
        for (_, _, c, e) in all {
            out.retain(|x| x.0 != c);
            out.push((c, e));
        }
        out.into_iter().filter_map(|(c, e)| e.filter(|e| !self.deleted(*e)).map(|e| (c, e))).collect()
    }

    fn envelope_of(&self, c: Cat, m: usize) -> Option<usize> {
        let links = self.links(m);
        let mut at = Some(c);
        while let Some(x) = at {
            if let Some((_, e)) = links.iter().find(|l| l.0 == x) {
                return Some(*e);
            }
            at = parent(x);
        }
        None
    }

    fn months(&self) -> Vec<Expected> {
        let mut out: Vec<Expected> = Vec::new();
        let live: Vec<usize> = (0..4).filter(|e| !self.deleted(*e)).collect();
        for m in self.start..MONTHS {
            let mut e = Expected {
                left_over: 0,
                income: 0,
                outside: 0,
                uncategorized: 0,
                overspent_before: 0,
                assigned: 0,
                to_be_assigned: 0,
                other: 0,
                lines: live.iter().map(|i| (*i, [0; 4])).collect(),
            };
            if let Some(p) = out.last() {
                e.left_over = p.to_be_assigned;
                for (n, (i, l)) in p.lines.iter().enumerate() {
                    if l[3] < 0 && self.next_month[*i] {
                        e.overspent_before += -l[3];
                    } else {
                        e.lines[n].1[0] = l[3];
                    }
                }
            }
            for t in self.txs.iter().filter(|t| t.month == Some(m)) {
                for (c, debit) in &t.lines {
                    if t.dollars {
                        e.other += 1;
                        continue;
                    }
                    match (c, self.envelope_of(*c, m)) {
                        (Cat::Pay, _) => e.income -= debit,
                        (_, Some(i)) => {
                            if let Some(l) = e.lines.iter_mut().find(|l| l.0 == i) {
                                l.1[2] += debit;
                            }
                        }
                        (Cat::None, None) => e.uncategorized += debit,
                        (_, None) => e.outside += debit,
                    }
                }
            }
            for d in self.rows() {
                if let Did::Money(month, from, to, amount) = d
                    && *month == m
                {
                    for (x, sign) in [(from, -1), (to, 1)] {
                        if let Some(l) = x.and_then(|x| e.lines.iter_mut().find(|l| l.0 == x)) {
                            l.1[1] += sign * amount;
                        }
                    }
                }
            }
            for (_, l) in &mut e.lines {
                l[3] = l[0] + l[1] - l[2];
                e.assigned += l[1];
            }
            e.to_be_assigned = e.left_over + e.income - e.outside - e.uncategorized - e.overspent_before - e.assigned;
            out.push(e);
        }
        out
    }

    fn assigned(&self, envelope: usize, m: usize) -> Cents {
        self.rows()
            .map(|d| match d {
                Did::Money(month, from, to, a) if *month == m => {
                    (if *to == Some(envelope) { *a } else { 0 }) - if *from == Some(envelope) { *a } else { 0 }
                }
                _ => 0,
            })
            .sum()
    }
}

fn month_n(m: usize) -> NaiveDate {
    month("2026-01").checked_add_months(Months::new(m as u32)).unwrap()
}

fn date_in(r: &mut Seeded, m: Option<usize>) -> NaiveDate {
    let first = match m {
        Some(m) => month_n(m),
        None => month("2025-11").checked_add_months(Months::new(r.next(2) as u32)).unwrap(),
    };
    first.with_day(1 + r.next(28) as u32).unwrap()
}

fn amount(r: &mut Seeded) -> Cents {
    1 + r.next(50_000) as Cents
}

/// Random months of income, spending, splits, refunds, transfers, card payments, dollars, assignments, moves, undos,
/// categories put in and taken out from a month on, envelopes deleted, first months moved, edits and deletions of
/// transactions and changes of the overspending: every month agrees with the test's own count.
#[test]
fn every_month_agrees_with_an_independent_count() {
    for seed in 1..=16u64 {
        let b = books();
        let cat_id = |c: Cat| match c {
            Cat::Pay => Some(b.salary),
            Cat::Food => Some(b.food),
            Cat::Restaurant => Some(b.restaurant),
            Cat::House => Some(b.home),
            Cat::Rent => Some(b.rent),
            Cat::Fun => Some(b.fun),
            Cat::Misc => Some(b.misc),
            Cat::None => None,
        };
        let mut o = Oracle::default();
        let mut envs: Vec<i64> = Vec::new();
        for (i, (n, c)) in
            [("Food", Cat::Food), ("House", Cat::House), ("Rent", Cat::Rent), ("Fun", Cat::Fun)].into_iter().enumerate()
        {
            let id = b.db.add_envelope(n, &[cat_id(c).unwrap()], Overspending::Carry, month_n(0)).unwrap();
            envs.push(id);
            let g = b.db.last_undoable(None).unwrap().unwrap();
            o.groups.push((g, vec![Did::Link(0, c, Some(i))], false));
        }
        let mut r = Seeded(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let pick_month = |r: &mut Seeded| if r.next(10) == 0 { None } else { Some(r.next(MONTHS)) };
        for step in 0..150 {
            match r.next(16) {
                0 | 1 => {
                    let m = pick_month(&mut r);
                    let a = amount(&mut r) * 4;
                    let id = tx(&b, b.bank, &crate::db::d2s(date_in(&mut r, m)), a, Some(b.salary));
                    o.txs.push(Known { kind: Kept::Tx(id), month: m, dollars: false, lines: vec![(Cat::Pay, -a)] });
                }
                2..=4 => {
                    let m = pick_month(&mut r);
                    let c = CATS[r.next(CATS.len())];
                    let refund = r.next(6) == 0;
                    let a = if refund { amount(&mut r) } else { -amount(&mut r) };
                    let account = [b.bank, b.card, b.dollars][r.next(3)];
                    let id = tx(&b, account, &crate::db::d2s(date_in(&mut r, m)), a, cat_id(c));
                    o.txs.push(Known {
                        kind: Kept::Tx(id),
                        month: m,
                        dollars: account == b.dollars,
                        lines: vec![(c, -a)],
                    });
                }
                5 => {
                    let m = pick_month(&mut r);
                    let parts: Vec<(Cat, Cents)> = (0..2 + r.next(3))
                        .map(|_| {
                            let c = CATS[r.next(CATS.len())];
                            (c, amount(&mut r))
                        })
                        .collect();
                    let total: Cents = parts.iter().map(|p| p.1).sum();
                    let mut lines = vec![NewLine {
                        ledger_account_id: b.db.money_ledger(b.card).unwrap().0,
                        amount: -total,
                        currency: None,
                    }];
                    for (c, a) in &parts {
                        lines.push(NewLine {
                            ledger_account_id: b.db.category_ledger(cat_id(*c)).unwrap(),
                            amount: *a,
                            currency: None,
                        });
                    }
                    let e = NewEntry { date: date_in(&mut r, m), payee: "split".into(), memo: String::new(), lines };
                    let id = b.db.post_entry(&e).unwrap();
                    o.txs.push(Known { kind: Kept::Split(id), month: m, dollars: false, lines: parts });
                }
                6 => {
                    let (from, to) = [(b.bank, b.card), (b.bank, b.savings), (b.savings, b.bank)][r.next(3)];
                    let m = pick_month(&mut r);
                    let date = date_in(&mut r, m);
                    b.db.add_transfer(from, to, date, amount(&mut r), "t").unwrap();
                }
                7 => {
                    let (e, m) = (r.next(4), r.next(MONTHS));
                    let target = amount(&mut r) - 5_000;
                    let before = o.assigned(e, m);
                    let fine = m >= o.start && !o.deleted(e);
                    match b.db.assign(envs[e], month_n(m), target, "a") {
                        Ok(g) => {
                            assert!(fine, "{seed}/{step}");
                            assert_eq!(g.is_none(), before == target);
                            if let Some(g) = g {
                                o.groups.push((g, vec![Did::Money(m, None, Some(e), target - before)], false));
                            }
                        }
                        Err(_) => assert!(!fine, "{seed}/{step}"),
                    }
                }
                8 => {
                    let (f, t, m) = (r.next(4), r.next(4), r.next(MONTHS));
                    let a = amount(&mut r);
                    let fine = f != t && m >= o.start && !o.deleted(f) && !o.deleted(t);
                    match b.db.move_money(envs[f], envs[t], month_n(m), a, "cover") {
                        Ok(g) => {
                            assert!(fine, "{seed}/{step}");
                            o.groups.push((g, vec![Did::Money(m, Some(f), Some(t), a)], false));
                        }
                        Err(_) => assert!(!fine, "{seed}/{step}"),
                    }
                }
                9 => {
                    let last = o.groups.iter().rposition(|g| !g.2);
                    // An envelope that comes back keeps its name: no other envelope took it here.
                    match (b.db.undo_envelope_change(None), last) {
                        (Ok(_), Some(i)) => o.groups[i].2 = true,
                        (Err(_), None) => {}
                        (got, want) => panic!("{seed}/{step}: {got:?} {want:?}"),
                    }
                }
                10 => {
                    // A transaction edited or deleted.
                    if o.txs.is_empty() {
                        continue;
                    }
                    let i = r.next(o.txs.len());
                    let t = o.txs[i].clone();
                    match (t.kind, r.next(2)) {
                        (Kept::Tx(id), 0) => {
                            let m = pick_month(&mut r);
                            let c = CATS[r.next(CATS.len())];
                            let a = -amount(&mut r);
                            let edit = TxEdit {
                                date: date_in(&mut r, m),
                                amount: a,
                                payee: "e".into(),
                                memo: String::new(),
                                category_id: cat_id(c),
                                cleared: false,
                            };
                            b.db.update_transaction(id, &edit).unwrap();
                            o.txs[i] = Known { month: m, lines: vec![(c, -a)], ..t };
                        }
                        (Kept::Tx(id), _) => {
                            b.db.delete_transaction(id).unwrap();
                            o.txs.remove(i);
                        }
                        (Kept::Split(id), _) => {
                            b.db.delete_entry(id).unwrap();
                            o.txs.remove(i);
                        }
                    }
                }
                11 | 12 => {
                    // A category put in an envelope, or taken out, from a month on.
                    let (e, m, c) = (r.next(4), r.next(MONTHS), LINKABLE[r.next(LINKABLE.len())]);
                    let links = o.links(m);
                    let mine: Vec<Cat> = links.iter().filter(|l| l.1 == e).map(|l| l.0).collect();
                    let taking = mine.contains(&c);
                    let elsewhere = links.iter().any(|l| l.0 == c && l.1 != e);
                    let wanted: Vec<i64> = mine
                        .iter()
                        .copied()
                        .filter(|x| *x != c)
                        .chain((!taking).then_some(c))
                        .map(|x| cat_id(x).unwrap())
                        .collect();
                    let fine = m >= o.start && !o.deleted(e) && !elsewhere;
                    let before = b.db.last_undoable(None).unwrap();
                    match b.db.edit_envelope(envs[e], None, Some((&wanted, month_n(m))), None) {
                        Ok(()) => {
                            assert!(fine, "{seed}/{step}");
                            let g = b.db.last_undoable(None).unwrap().unwrap();
                            assert_ne!(Some(g), before);
                            o.groups.push((g, vec![Did::Link(m, c, (!taking).then_some(e))], false));
                        }
                        Err(_) => assert!(!fine, "{seed}/{step}"),
                    }
                }
                13 => {
                    let (e, m) = (r.next(4), r.next(MONTHS));
                    let fine = m >= o.start && !o.deleted(e);
                    match b.db.delete_envelope(envs[e], month_n(m)) {
                        Ok(g) => {
                            assert!(fine, "{seed}/{step}");
                            o.groups.push((g, vec![Did::Delete(e)], false));
                        }
                        Err(_) => assert!(!fine, "{seed}/{step}"),
                    }
                }
                14 => {
                    // The first month moves, earlier or later; money before it stops counting only when confirmed.
                    let m = r.next(4);
                    let dropped = o.rows().any(|d| matches!(d, Did::Money(x, ..) if *x < m));
                    let confirm = r.next(2) == 0;
                    match b.db.set_envelopes_start(month_n(m), confirm) {
                        Ok(()) => {
                            assert!(confirm || !dropped, "{seed}/{step}");
                            o.start = m;
                        }
                        Err(_) => assert!(dropped && !confirm, "{seed}/{step}"),
                    }
                }
                _ => {
                    let e = r.next(4);
                    o.next_month[e] = !o.next_month[e];
                    let p = if o.next_month[e] { Overspending::NextMonth } else { Overspending::Carry };
                    // The setting of a deleted envelope cannot change; it counts again if it comes back.
                    if b.db.edit_envelope(envs[e], None, None, Some(p)).is_err() {
                        assert!(o.deleted(e));
                        o.next_month[e] = !o.next_month[e];
                    }
                }
            }
        }
        let expected = o.months();
        for (n, want) in expected.iter().enumerate() {
            let m = o.start + n;
            let v = b.db.envelope_month(month_n(m)).unwrap();
            let got = Expected {
                left_over: v.left_over,
                income: v.income,
                outside: v.outside,
                uncategorized: v.uncategorized,
                overspent_before: v.overspent_before,
                assigned: v.assigned,
                to_be_assigned: v.to_be_assigned,
                other: v.other_currencies,
                lines: v
                    .envelopes
                    .iter()
                    .map(|l| {
                        let i = envs.iter().position(|e| *e == l.id).unwrap();
                        (i, [l.carried, l.assigned, l.spent, l.available])
                    })
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            };
            assert_eq!(&got, want, "seed {seed}, month {m}");
            assert_whole(&b.db, &month_key(month_n(m)));
        }
        if o.start > 0 {
            assert!(b.db.envelope_month(month_n(o.start - 1)).unwrap().before_start);
        }
        assert_sound(&b.db, "oracle");
    }
}

#[test]
fn the_operations_speak_euros_and_say_what_a_past_change_moved() {
    use crate::ops::call;
    use serde_json::json;
    crate::i18n::with_lang(crate::i18n::Lang::En, || {
        let b = books();
        let s = call(&b.db, "envelope_settings", &json!({}), None).unwrap();
        assert_eq!((s["mode"].as_str(), s["start"].as_str()), (Some("envelopes"), Some("2026-01")));
        call(&b.db, "add_envelope", &json!({"name": "Food", "categories": "Food, Food:Restaurant"}), None).unwrap();
        call(&b.db, "add_envelope", &json!({"name": "Fun", "categories": ["Fun"], "overspending": "next_month"}), None)
            .unwrap();
        assert!(call(&b.db, "add_envelope", &json!({"name": "X", "overspending": "sometimes"}), None).is_err());
        assert!(call(&b.db, "add_envelope", &json!({"name": "Y", "categories": [3]}), None).is_err());
        tx(&b, b.bank, "2026-02-01", 100_000, Some(b.salary));
        let r =
            call(&b.db, "assign_envelope", &json!({"envelope": "Food", "amount": "400.50", "month": "2026-02"}), None)
                .unwrap();
        assert!(r["recomputed"].as_array().unwrap().iter().any(|m| m == "2026-03"), "{r}");
        assert!(r["message"].as_str().unwrap().contains("recomputed"), "{r}");
        let again =
            call(&b.db, "assign_envelope", &json!({"envelope": "Food", "amount": 400.5, "month": "2026-02"}), None)
                .unwrap();
        assert!(again["change"].is_null());
        call(
            &b.db,
            "move_envelope_money",
            &json!({"from": "Food", "to": "Fun", "amount": 0.5, "month": "2026-02", "reason": "r"}),
            None,
        )
        .unwrap();
        let v = call(&b.db, "list_envelopes", &json!({"month": "2026-02"}), None).unwrap();
        assert_eq!(v["to_be_assigned"], json!(599.5));
        assert_eq!(v["envelopes"][0]["assigned"], json!(400.0));
        assert_eq!(v["changes"][0]["amount"], json!(0.5));
        let cents = crate::ops::call_in_cents(&b.db, "list_envelopes", &json!({"month": "2026-02"})).unwrap();
        assert_eq!(cents["to_be_assigned"], json!(59_950));
        call(&b.db, "undo_envelope_change", &json!({}), None).unwrap();
        let v = call(&b.db, "list_envelopes", &json!({"month": "2026-02"}), None).unwrap();
        assert_eq!(v["envelopes"][0]["available"], json!(400.5));
        call(&b.db, "edit_envelope", &json!({"envelope": "Fun", "name": "Leisure", "categories": ""}), None).unwrap();
        call(&b.db, "delete_envelope", &json!({"envelope": "Leisure"}), None).unwrap();
        let s = call(&b.db, "envelope_settings", &json!({"mode": "monthly", "start": "2026-02"}), None).unwrap();
        assert_eq!((s["mode"].as_str(), s["start"].as_str()), (Some("monthly"), Some("2026-02")));
        assert_eq!(call(&b.db, "budget_status", &json!({}), None).unwrap()["mode"], json!("monthly"));
        for bad in [json!({"mode": "both"}), json!({"start": "2026-13"}), json!({"start": "9999-01"})] {
            assert!(call(&b.db, "envelope_settings", &bad, None).is_err(), "{bad}");
        }
    });
}

/// The first change of the envelopes, by any way, fixes the first month: it never slides with the clock.
#[test]
fn the_first_change_fixes_the_first_month() {
    use crate::ops::call;
    use serde_json::json;
    let db = Db::open_in_memory().unwrap();
    let food = db.category_by_path("Food", false, true).unwrap();
    assert!(db.meta("envelopes_from").is_none());
    // Over MCP, without choosing the mode: an envelope, then money in a past month.
    call(&db, "add_envelope", &json!({"name": "Food", "categories": food.to_string(), "month": "2026-02"}), None)
        .unwrap();
    assert_eq!(db.meta("envelopes_from").as_deref(), Some("2026-02"));
    assert_eq!(db.budget_mode(), Mode::Monthly);
    assert!(call(&db, "assign_envelope", &json!({"envelope": "Food", "amount": 5, "month": "2026-01"}), None).is_err());
    call(&db, "assign_envelope", &json!({"envelope": "Food", "amount": 5, "month": "2026-03"}), None).unwrap();
    // Choosing the mode later keeps it.
    db.set_budget_mode(Mode::Envelopes).unwrap();
    assert_eq!(db.envelopes_start(), month("2026-02"));
    // Later, only when confirmed: the money of March would stop counting.
    let later = json!({"start": "2026-04"});
    assert!(call(&db, "envelope_settings", &later, None).is_err());
    assert_eq!(db.envelopes_start(), month("2026-02"));
    let r = call(&db, "envelope_settings", &json!({"start": "2026-04", "confirm": true}), None).unwrap();
    assert_eq!(db.envelopes_start(), month("2026-04"));
    assert!(r["recomputed"].as_array().unwrap().iter().any(|m| m == "2026-03"), "{r}");
    // Earlier needs no yes.
    call(&db, "envelope_settings", &json!({"start": "2026-01"}), None).unwrap();
    assert_eq!(db.envelope_month(month("2026-03")).unwrap().envelopes[0].assigned, 500);
    // A file whose first change is the deletion of a category's link, or an undo, has it fixed too.
    let other = Db::open_in_memory().unwrap();
    let e = other.add_envelope("X", &[], Overspending::Carry, month("2025-06")).unwrap();
    assert_eq!(other.envelopes_start(), month("2025-06"));
    other.delete_envelope(e, month("2025-06")).unwrap();
    assert!(other.delete_envelope(e, month("2025-06")).is_err());
}

/// An envelope is found by its id or its whole name; nothing else, and never one for another.
#[test]
fn envelopes_are_found_by_id_or_whole_name_only() {
    let b = books();
    let food = b.db.add_envelope("Food", &[], Overspending::Carry, month("2026-01")).unwrap();
    let fun = b.db.add_envelope("Fun", &[], Overspending::Carry, month("2026-01")).unwrap();
    assert_eq!(b.db.find_envelope("food").unwrap().id, food);
    assert_eq!(b.db.find_envelope(&fun.to_string()).unwrap().id, fun);
    for no in ["Foo", "F", "", "ood", "999999"] {
        assert!(b.db.find_envelope(no).is_err(), "{no}");
    }
    // A name that is the id of another is refused.
    let named = b.db.add_envelope(&food.to_string(), &[], Overspending::Carry, month("2026-01")).unwrap();
    assert_ne!(named, food);
    assert!(b.db.find_envelope(&food.to_string()).is_err());
    assert_eq!(b.db.find_envelope(&named.to_string()).unwrap().id, named);
    // Ids are never given again, also after the last one is deleted.
    b.db.delete_envelope(named, month("2026-01")).unwrap();
    let next = b.db.add_envelope("New", &[], Overspending::Carry, month("2026-01")).unwrap();
    assert!(next > named);
}

/// A category goes into an envelope from a month on: the months before keep what they had, and undoing it puts it back.
#[test]
fn categories_change_envelope_from_a_month_on() {
    let b = books();
    let food = b.db.add_envelope("Food", &[b.food], Overspending::Carry, month("2026-01")).unwrap();
    let eat_out = b.db.add_envelope("Eating out", &[], Overspending::Carry, month("2026-01")).unwrap();
    for d in ["2026-01-10", "2026-02-10", "2026-03-10"] {
        tx(&b, b.bank, d, -1_000, Some(b.restaurant));
    }
    tx(&b, b.bank, "2026-02-11", -300, Some(b.misc));
    let spent = |m: &str, e: i64| view(&b.db, m).envelopes.iter().find(|l| l.id == e).unwrap().spent;
    let before = b.db.fingerprints().unwrap();
    b.db.edit_envelope(eat_out, None, Some((&[b.restaurant, b.misc], month("2026-02"))), None).unwrap();
    assert_eq!((spent("2026-01", food), spent("2026-01", eat_out)), (1_000, 0));
    assert_eq!((spent("2026-02", food), spent("2026-02", eat_out)), (0, 1_300));
    assert_eq!(spent("2026-03", eat_out), 1_000);
    let changed = b.db.months_changed(&before).unwrap();
    assert!(!changed.contains(&"2026-01".to_string()) && changed.contains(&"2026-02".to_string()), "{changed:?}");
    assert_eq!(view(&b.db, "2026-01").envelopes.iter().find(|l| l.id == eat_out).unwrap().categories.len(), 0);
    // Taken out from March: back under Food, whose envelope it is below.
    b.db.edit_envelope(eat_out, None, Some((&[b.misc], month("2026-03"))), None).unwrap();
    assert_eq!((spent("2026-03", food), spent("2026-03", eat_out)), (1_000, 0));
    // Food cannot take Misc while it is in Eating out.
    assert!(b.db.edit_envelope(food, None, Some((&[b.food, b.misc], month("2026-03"))), None).is_err());
    b.db.undo_envelope_change(None).unwrap();
    b.db.undo_envelope_change(None).unwrap();
    assert_eq!(b.db.months_changed(&before).unwrap(), Vec::<String>::new());
    assert_whole(&b.db, "2026-03");
}

/// Reading, undoing and listing stay in proportion to the changes there are.
#[test]
fn many_changes_take_time_in_proportion() {
    let time = |n: usize| {
        let b = books();
        let e = b.db.add_envelope("Food", &[b.food], Overspending::Carry, month("2026-01")).unwrap();
        b.db.atomically(|| {
            for i in 0..n {
                b.db.conn.execute(
                    "INSERT INTO envelope_changes(group_id, month, kind, to_envelope_id, amount, made_at)
                     VALUES (0, ?1, 'assign', ?2, 1, 'x')",
                    params![month_key(month_n(i % MONTHS)), e],
                )?;
            }
            b.db.conn.execute("UPDATE envelope_changes SET group_id=id WHERE group_id=0", [])?;
            Ok(())
        })
        .unwrap();
        let start = std::time::Instant::now();
        for _ in 0..3 {
            b.db.changes(None).unwrap();
            b.db.envelope_month(month_n(11)).unwrap();
            let before = b.db.fingerprints().unwrap();
            b.db.undo_envelope_change(None).unwrap();
            b.db.assign(e, month_n(5), 7, "").unwrap();
            b.db.months_changed(&before).unwrap();
        }
        start.elapsed().as_secs_f64()
    };
    time(300);
    let (small, large) = (time(3_000), time(30_000));
    // Ten times the changes: well under the hundred times of something that grows with their square.
    assert!(large < small * 40.0, "{small} s, then {large} s");
}
