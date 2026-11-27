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
    b.db.set_envelopes_start(month("2026-01")).unwrap();
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
    let food = b.db.add_envelope("Food", &[b.food], Overspending::Carry).unwrap();
    let home = b.db.add_envelope("Home", &[b.home], Overspending::Carry).unwrap();
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
    assert_eq!(b.db.changes(None).unwrap().len(), 3);
    assert_whole(&b.db, "2026-01");
}

#[test]
fn overspending_in_march_is_carried_into_april() {
    for policy in [Overspending::Carry, Overspending::NextMonth] {
        let b = books();
        b.db.set_envelopes_start(month("2026-03")).unwrap();
        let food = b.db.add_envelope("Food", &[b.food], policy).unwrap();
        let fun = b.db.add_envelope("Fun", &[b.fun], Overspending::Carry).unwrap();
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
        let before = b.db.fingerprints(month("2026-03")).unwrap();
        b.db.undo_envelope_change(Some(g)).unwrap();
        assert_eq!(line(&view(&b.db, "2026-03"), "Food").available, -5_000);
        let changed = b.db.months_changed_after(month("2026-03"), &before).unwrap();
        assert!(changed.contains(&"2026-04".to_string()) && changed.contains(&"2026-11".to_string()), "{changed:?}");
        let all = b.db.changes(None).unwrap();
        assert_eq!(all.len(), 4);
        assert!(all.iter().any(|c| c.group_id == g && c.undone));
        assert!(b.db.undo_envelope_change(Some(g)).is_err());
        assert!(b.db.undo_envelope_change(Some(all[0].group_id)).is_err());
        assert_whole(&b.db, "2026-11");
    }
}

#[test]
fn transfers_and_card_payments_are_not_spending_and_card_purchases_are() {
    let b = books();
    let food = b.db.add_envelope("Food", &[b.food], Overspending::Carry).unwrap();
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
    b.db.add_envelope("Food", &[b.food], Overspending::Carry).unwrap();
    b.db.add_envelope("House", &[b.home], Overspending::Carry).unwrap();
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
    b.db.add_envelope("House", &[b.home], Overspending::Carry).unwrap();
    b.db.add_envelope("Rent", &[b.rent], Overspending::Carry).unwrap();
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
    let other = b.db.add_envelope("Other", &[], Overspending::Carry).unwrap();
    assert!(b.db.edit_envelope(other, None, Some(&[b.rent]), None).is_err());
    assert!(b.db.edit_envelope(other, None, Some(&[b.salary]), None).is_err());
    assert!(b.db.add_envelope("rent", &[], Overspending::Carry).is_err());
    assert!(b.db.add_envelope("  ", &[], Overspending::Carry).is_err());
    // A category that goes takes its link with it; its spending is then uncategorized.
    b.db.delete_category(b.rent).unwrap();
    let v = view(&b.db, "2026-01");
    assert_eq!((line(&v, "Rent").spent, v.uncategorized), (0, 70_200));
    assert_whole(&b.db, "2026-01");
}

#[test]
fn other_currencies_are_left_out_and_counted() {
    let b = books();
    b.db.add_envelope("Food", &[b.food], Overspending::Carry).unwrap();
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
    let food = b.db.add_envelope("Food", &[b.food], Overspending::Carry).unwrap();
    let fun = b.db.add_envelope("Fun", &[b.fun], Overspending::Carry).unwrap();
    tx(&b, b.bank, "2026-01-02", 10_000, Some(b.salary));
    b.db.assign(food, month("2026-01"), 6_000, "").unwrap();
    b.db.assign(fun, month("2026-01"), 4_000, "").unwrap();
    b.db.move_money(fun, food, month("2026-01"), 1_000, "").unwrap();
    tx(&b, b.bank, "2026-01-03", -2_000, Some(b.food));
    b.db.delete_envelope(food).unwrap();
    let v = view(&b.db, "2026-01");
    assert_eq!((v.outside, line(&v, "Fun").available, v.to_be_assigned), (2_000, 3_000, 5_000));
    assert!(b.db.changes(None).unwrap().iter().any(|c| c.to.as_deref() == Some(tr("deleted envelope"))));
    assert_whole(&b.db, "2026-01");
    assert!(b.db.delete_envelope(food).is_err());
}

#[test]
fn months_out_of_range_and_large_amounts_are_refused() {
    let b = books();
    let food = b.db.add_envelope("Food", &[b.food], Overspending::Carry).unwrap();
    let fun = b.db.add_envelope("Fun", &[b.fun], Overspending::Carry).unwrap();
    let v = view(&b.db, "2025-12");
    assert!(v.before_start && v.envelopes.iter().all(|e| e.available == 0));
    assert!(b.db.assign(food, month("2025-12"), 1, "").is_err());
    assert!(b.db.envelope_month(month("9999-12")).is_err());
    assert!(b.db.assign(food, month("9999-12"), 1, "").is_err());
    assert!(b.db.set_envelopes_start(month("9999-12")).is_err());
    assert!(b.db.assign(food, month("2026-01"), MAX_BALANCE + 1, "").is_err());
    assert!(b.db.assign(food, month("2026-01"), i64::MIN, "").is_err());
    assert!(b.db.move_money(food, fun, month("2026-01"), 0, "").is_err());
    assert!(b.db.move_money(food, fun, month("2026-01"), -5, "").is_err());
    assert!(b.db.move_money(food, food, month("2026-01"), 5, "").is_err());
    assert!(b.db.move_money(food, 999, month("2026-01"), 5, "").is_err());
    assert!(b.db.assign(food, month("2026-01"), 1, &"x".repeat(MAX_TEXT + 1)).is_err());
    assert!(b.db.undo_envelope_change(Some(424242)).is_err());
    assert!(b.db.undo_envelope_change(None).is_err());
    // As large as allowed, again and again: refused before any sum leaves 64 bits.
    let mut n = 0;
    while b.db.assign(food, month("2026-01"), if n % 2 == 0 { MAX_BALANCE } else { 0 }, "").is_ok() {
        n += 1;
        assert!(n < 5000);
    }
    assert!(n > 100);
    view(&b.db, "2026-12");
    assert_eq!(b.db.changes(None).unwrap().len(), n);
}

#[test]
fn a_copy_gives_ids_that_no_other_computer_gives() {
    let dir = std::env::temp_dir().join(format!("cashcraft-envelope-ids-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (db, _) = Db::open_copy(&dir.join("books.db"), crate::i18n::Lang::En).unwrap();
    let e = db.add_envelope("Food", &[], Overspending::Carry).unwrap();
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

/// What the test knows of a transaction: its month (None before the start), whether in dollars, and the debits
/// on categories.
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

#[derive(Clone, Debug, Default)]
struct Oracle {
    txs: Vec<Known>,
    /// Groups: rows of (month, from, to, amount), undoes.
    groups: Vec<(i64, Vec<(usize, Option<usize>, Option<usize>, Cents)>, Option<i64>)>,
    next_month: [bool; 4],
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
    lines: Vec<[Cents; 4]>,
}

/// The envelope of a category, written out by hand: Food has the restaurant under it, Rent has its own.
fn envelope_of(c: Cat) -> Option<usize> {
    match c {
        Cat::Food | Cat::Restaurant => Some(0),
        Cat::House => Some(1),
        Cat::Rent => Some(2),
        Cat::Fun => Some(3),
        _ => None,
    }
}

impl Oracle {
    fn months(&self) -> Vec<Expected> {
        let mut out: Vec<Expected> = Vec::new();
        for m in 0..MONTHS {
            let mut e = Expected {
                left_over: 0,
                income: 0,
                outside: 0,
                uncategorized: 0,
                overspent_before: 0,
                assigned: 0,
                to_be_assigned: 0,
                other: 0,
                lines: vec![[0; 4]; 4],
            };
            if let Some(p) = out.last() {
                e.left_over = p.to_be_assigned;
                for (i, l) in p.lines.iter().enumerate() {
                    if l[3] < 0 && self.next_month[i] {
                        e.overspent_before += -l[3];
                    } else {
                        e.lines[i][0] = l[3];
                    }
                }
            }
            for t in self.txs.iter().filter(|t| t.month == Some(m)) {
                for (c, debit) in &t.lines {
                    if t.dollars {
                        e.other += 1;
                        continue;
                    }
                    match (c, envelope_of(*c)) {
                        (Cat::Pay, _) => e.income -= debit,
                        (_, Some(i)) => e.lines[i][2] += debit,
                        (Cat::None, None) => e.uncategorized += debit,
                        (_, None) => e.outside += debit,
                    }
                }
            }
            for (_, rows, _) in &self.groups {
                for (month, from, to, amount) in rows {
                    if *month != m {
                        continue;
                    }
                    if let Some(f) = from {
                        e.lines[*f][1] -= amount;
                    }
                    if let Some(t) = to {
                        e.lines[*t][1] += amount;
                    }
                }
            }
            for l in &mut e.lines {
                l[3] = l[0] + l[1] - l[2];
                e.assigned += l[1];
            }
            e.to_be_assigned = e.left_over + e.income - e.outside - e.uncategorized - e.overspent_before - e.assigned;
            out.push(e);
        }
        out
    }

    fn assigned(&self, envelope: usize, m: usize) -> Cents {
        self.months()[m].lines[envelope][1]
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
/// edits and deletions of transactions and changes of the overspending: every month agrees with the test's own count.
#[test]
fn every_month_agrees_with_an_independent_count() {
    for seed in 1..=12u64 {
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
        let envs: Vec<i64> = [("Food", b.food), ("House", b.home), ("Rent", b.rent), ("Fun", b.fun)]
            .iter()
            .map(|(n, c)| b.db.add_envelope(n, &[*c], Overspending::Carry).unwrap())
            .collect();
        let mut r = Seeded(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut o = Oracle::default();
        let pick_month = |r: &mut Seeded| if r.next(10) == 0 { None } else { Some(r.next(MONTHS)) };
        for step in 0..120 {
            match r.next(12) {
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
                    let g = b.db.assign(envs[e], month_n(m), target, "a").unwrap();
                    assert_eq!(g.is_none(), before == target);
                    if let Some(g) = g {
                        o.groups.push((g, vec![(m, None, Some(e), target - before)], None));
                    }
                }
                8 => {
                    let (f, t, m) = (r.next(4), r.next(4), r.next(MONTHS));
                    let a = amount(&mut r);
                    match b.db.move_money(envs[f], envs[t], month_n(m), a, "cover") {
                        Ok(g) => o.groups.push((g, vec![(m, Some(f), Some(t), a)], None)),
                        Err(_) => assert_eq!(f, t),
                    }
                }
                9 => {
                    let undone: Vec<i64> = o.groups.iter().filter_map(|g| g.2).collect();
                    let last = o.groups.iter().rev().find(|g| g.2.is_none() && !undone.contains(&g.0)).cloned();
                    match (b.db.undo_envelope_change(None), last) {
                        (Ok(g), Some((target, rows, _))) => {
                            let back = rows.iter().map(|(m, f, t, a)| (*m, *f, *t, -a)).collect();
                            o.groups.push((g, back, Some(target)));
                        }
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
                _ => {
                    let e = r.next(4);
                    o.next_month[e] = !o.next_month[e];
                    let p = if o.next_month[e] { Overspending::NextMonth } else { Overspending::Carry };
                    b.db.edit_envelope(envs[e], None, None, Some(p)).unwrap();
                }
            }
        }
        let expected = o.months();
        for (m, want) in expected.iter().enumerate() {
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
                lines: envs
                    .iter()
                    .map(|id| {
                        let l = v.envelopes.iter().find(|l| l.id == *id).unwrap();
                        [l.carried, l.assigned, l.spent, l.available]
                    })
                    .collect(),
            };
            assert_eq!(&got, want, "seed {seed}, month {m}");
            assert_whole(&b.db, &month_key(month_n(m)));
        }
        assert_sound(&b.db, "oracle");
    }
}
