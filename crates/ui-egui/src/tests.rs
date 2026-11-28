use crate::dialogs::follow_repetition;
use crate::gui::{App, Dialog, LANGUAGES, SchedForm, View, short_eur};
use crate::ledger::{EntryForm, tree};
use crate::reconcile::Unlock;
use cashcraft_engine::db::{AccountKind, Db, NewTx};
use cashcraft_engine::i18n::{Lang, META_KEY, with_lang};
use cashcraft_engine::ledger::{Class, NewEntry, NewLine};
use cashcraft_engine::money::{parse_amount, parse_date};

#[test]
fn chart_amounts_follow_the_language() {
    let both = |c| (with_lang(Lang::It, || short_eur(c)), with_lang(Lang::En, || short_eur(c)));
    assert_eq!(both(123_456), ("1,2k €".into(), "€1.2k".into()));
    assert_eq!(both(-123_456), ("-1,2k €".into(), "-€1.2k".into()));
    assert_eq!(both(-5_000), ("-50 €".into(), "-€50".into()));
    assert_eq!(both(-10), ("0 €".into(), "€0".into()));
}

/// The form of a transaction opens with text that reads back as the transaction.
#[test]
fn the_form_reads_back_what_it_shows() {
    for l in [Lang::It, Lang::En] {
        with_lang(l, || {
            let db = Db::open_in_memory().unwrap();
            let account_id = db.add_account("A", AccountKind::Bank, "EUR", 0, "").unwrap();
            let mut app = App::new(db, None);
            for amount in [-123_400, 100_000_000, -5, 99_999, 1_234] {
                let tx = NewTx { account_id, amount, date: parse_date("2026-03-04").unwrap(), ..Default::default() };
                let id = app.db.add_transaction(&tx).unwrap().unwrap();
                let t = app.db.transaction(id).unwrap();
                app.open_edit_tx(&t);
                let Some(Dialog::Tx(f)) = app.dialog.take() else { panic!("no form") };
                let a = parse_amount(&f.amount).unwrap();
                assert_eq!((if f.expense { -a } else { a }, parse_date(&f.date)), (amount, Some(tx.date)));
            }
        });
    }
}

#[test]
fn the_window_starts_from_the_stored_language() {
    for l in LANGUAGES {
        let db = Db::open_in_memory().unwrap();
        db.set_meta(META_KEY, l.map_or("system", Lang::code)).unwrap();
        assert_eq!(App::new(db, None).language, l);
    }
}

#[test]
fn the_form_of_an_entry_shows_the_difference_until_zero() {
    for (l, a, b) in [(Lang::It, "1.234,50", "1.234,5"), (Lang::En, "1,234.50", "1234.5")] {
        with_lang(l, || {
            let db = Db::open_in_memory().unwrap();
            db.add_account("A", AccountKind::Bank, "EUR", 0, "").unwrap();
            let id = |k: &str| db.find_ledger_account(k).unwrap().id;
            let mut f = EntryForm { payee: "x".into(), ..Default::default() };
            assert_eq!(f.difference().unwrap(), 0);
            assert!(f.entry().is_err());
            f.lines[0].account = Some(id("A"));
            f.lines[0].debit = a.into();
            f.lines[1].account = Some(id("uncategorized"));
            f.lines[1].credit = "34,50".into();
            assert_eq!(f.difference().unwrap(), 120_000);
            let before = db.entries(&Default::default()).unwrap().len();
            assert!(db.post_entry(&f.entry().unwrap()).is_err());
            assert_eq!(db.entries(&Default::default()).unwrap().len(), before);
            f.lines[1].credit = b.into();
            // A blank line does not count.
            f.lines.push(Default::default());
            assert_eq!(f.difference().unwrap(), 0);
            db.post_entry(&f.entry().unwrap()).unwrap();
            f.lines[0].credit = "1".into();
            assert!(f.difference().is_err() && f.entry().is_err());
            f.lines[0].credit.clear();
            f.lines[1].credit = "1,2345".into();
            assert!(f.difference().is_err());
        });
    }
}

#[test]
fn the_tree_has_every_account_once_with_the_sums_of_those_below() {
    let db = Db::open_in_memory().unwrap();
    let account_id = db.add_account("A", AccountKind::Bank, "EUR", 0, "").unwrap();
    for c in ["Home:Utilities", "Home:Maintenance"] {
        let category_id = Some(db.find_category(c).unwrap().id);
        let tx = NewTx { account_id, amount: -100, payee: "x".into(), category_id, ..Default::default() };
        db.add_transaction(&tx).unwrap();
    }
    let all = db.ledger_accounts().unwrap();
    let rows: Vec<_> = Class::ALL.into_iter().flat_map(|c| tree(&all, c)).collect();
    assert_eq!(rows.len(), all.len());
    let home = rows.iter().find(|r| r.account.name == "Home").unwrap();
    assert_eq!((home.depth, home.balances[0].amount), (0, 200));
    assert!(rows.iter().any(|r| r.account.path == "Home:Utilities" && r.depth == 1));
}

/// Symbols drawn by the interface must exist in the bundled fonts, or they show as empty boxes.
#[test]
fn every_symbol_has_a_glyph() {
    let ctx = egui::Context::default();
    let sources = [
        include_str!("gui.rs"),
        include_str!("views.rs"),
        include_str!("dialogs.rs"),
        include_str!("ledger.rs"),
        include_str!("rules.rs"),
        include_str!("reconcile.rs"),
        // What it writes of a rule is drawn too.
        include_str!("../../engine/src/rules.rs"),
        cashcraft_engine::i18n::IT_TSV,
    ]
    .concat();
    let mut missing: Vec<char> = Vec::new();
    ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
    ctx.fonts_mut(|f| {
        for c in sources.chars().filter(|c| !c.is_ascii() && *c != '\u{fe0f}') {
            if !f.has_glyph(&egui::FontId::proportional(14.0), c) && !missing.contains(&c) {
                missing.push(c);
            }
        }
    });
    assert!(missing.is_empty(), "no glyph for {missing:?}");
}

/// An entry is written and posted without the mouse.
#[test]
fn an_entry_is_posted_with_the_keyboard() {
    use egui::{Event, Key, Modifiers};
    with_lang(Lang::En, || {
        let db = Db::open_in_memory().unwrap();
        db.add_account("A", AccountKind::Bank, "EUR", 0, "").unwrap();
        let mut app = App::new(db, None);
        let ctx = egui::Context::default();
        let mut time = 0.0;
        let mut send = |app: &mut App, events: Vec<Event>| {
            for events in [events, vec![]] {
                time += 0.5;
                let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0)));
                let input = egui::RawInput { events, screen_rect, time: Some(time), ..Default::default() };
                ctx.run_ui(input, |ui| app.dialogs(&ui.ctx().clone())).textures_delta.clear();
            }
        };
        let press = |key| {
            let event =
                |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
            vec![event(true), event(false)]
        };
        let write = |s: &str| vec![Event::Text(s.into())];
        app.open_entry();
        send(&mut app, vec![]);
        send(&mut app, press(Key::Escape));
        assert!(app.dialog.is_none());
        app.open_entry();
        send(&mut app, vec![]);
        send(&mut app, write("Pay"));
        // What is written is not lost.
        send(&mut app, press(Key::Escape));
        assert!(app.dialog.is_some());
        app.open_entry();
        send(&mut app, vec![]);
        send(&mut app, write("Pay"));
        // Description, notes, and for each line account, debit, credit.
        send(&mut app, press(Key::Tab));
        send(&mut app, write("January"));
        for (n, amount) in [(1, "12.50"), (2, "12.50")] {
            send(&mut app, press(Key::Tab));
            send(&mut app, press(Key::Enter));
            for _ in 0..=n {
                send(&mut app, press(Key::ArrowDown));
            }
            send(&mut app, press(Key::Enter));
            for _ in 0..n {
                send(&mut app, press(Key::Tab));
            }
            send(&mut app, write(amount));
            if n == 1 {
                send(&mut app, press(Key::Tab));
            }
        }
        let Some(Dialog::Entry(f)) = &app.dialog else { panic!("no form") };
        let seen: Vec<_> = f.lines.iter().map(|l| (l.account.is_some(), l.debit.as_str(), l.credit.as_str())).collect();
        assert_eq!(seen, [(true, "12.50", ""), (true, "", "12.50")]);
        assert_eq!((f.payee.as_str(), f.memo.as_str()), ("Pay", "January"));
        // Add a line, post.
        send(&mut app, press(Key::Tab));
        send(&mut app, press(Key::Tab));
        send(&mut app, press(Key::Enter));
        assert!(app.dialog.is_none());
        assert_eq!(app.db.entries(&Default::default()).unwrap().len(), 1);
    });
}

/// Deleting a split asks first and says how many lines go; the detail of the entry draws every posting.
#[test]
fn deleting_a_split_asks_first() {
    let db = Db::open_in_memory().unwrap();
    let account_id = db.add_account("A", AccountKind::Bank, "EUR", 0, "").unwrap();
    let id = |k: &str| db.find_ledger_account(k).unwrap().id;
    let all = [("A", -30), ("system:uncategorized", 10), ("Health", 20)];
    let lines = all.map(|(k, amount)| NewLine { ledger_account_id: id(k), amount, currency: None }).to_vec();
    let entry = NewEntry { date: parse_date("2026-03-04").unwrap(), payee: "x".into(), memo: "".into(), lines };
    let e = db.post_entry(&entry).unwrap();
    let tx = NewTx { account_id, amount: -1, payee: "x".into(), ..Default::default() };
    let simple = db.add_transaction(&tx).unwrap().unwrap();
    let mut app = App::new(db, None);
    let simple = app.db.transaction(simple).unwrap();
    assert!(!app.confirm_split(&simple) && app.dialog.is_none());
    let split = app.db.transactions(&Default::default()).unwrap().into_iter().find(|t| t.entry_id == e).unwrap();
    assert!(app.confirm_split(&split));
    assert!(matches!(app.dialog, Some(Dialog::ConfirmDeleteEntry(x, 3)) if x == e));
    assert!(app.db.entry(e).is_ok());
    let ctx = egui::Context::default();
    for dialog in [Dialog::EntryDetail(e), Dialog::ConfirmDeleteEntry(e, 3)] {
        app.dialog = Some(dialog);
        for _ in 0..3 {
            ctx.run_ui(Default::default(), |ui| app.dialogs(&ui.ctx().clone())).textures_delta.clear();
        }
        assert!(app.dialog.is_some());
    }
    // The journal and the trial balance draw too, and a failed read is not shown as empty.
    app.journal = Some((app.db.find_ledger_account("A").unwrap().id, 0));
    for _ in 0..2 {
        ctx.run_ui(Default::default(), |ui| app.ledger_view(ui)).textures_delta.clear();
    }
    assert_eq!(app.ledger.as_ref().unwrap().journal.as_ref().unwrap().0.len(), 2);
    app.db.conn.execute_batch("ALTER TABLE journal_postings RENAME TO gone").unwrap();
    app.ledger = None;
    ctx.run_ui(Default::default(), |ui| app.ledger_view(ui)).textures_delta.clear();
    let l = app.ledger.as_ref().unwrap();
    assert!(l.trial.is_err() && l.journal.is_err());
}

/// From the dialog: one candidate is proposed, several are not chosen, and a transfer is separated.
#[test]
fn a_transaction_is_turned_into_a_transfer() {
    use cashcraft_engine::transfers::OtherSide;
    use egui::{Event, Key, Modifiers};
    with_lang(Lang::En, || {
        let db = Db::open_in_memory().unwrap();
        let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 0, "").unwrap();
        let savings = db.add_account("Savings", AccountKind::Savings, "EUR", 0, "").unwrap();
        let date = parse_date("2026-03-04").unwrap();
        let add = |db: &Db, account_id, amount| {
            let tx = NewTx { account_id, amount, date, payee: "x".into(), ..Default::default() };
            db.add_transaction(&tx).unwrap().unwrap()
        };
        let (out, into) = (add(&db, bank, -50), add(&db, savings, 50));
        let mut app = App::new(db, None);
        let t = app.db.transaction(out).unwrap();
        let choice = |app: &mut App, account| {
            app.open_turn(&t);
            let Some(Dialog::TurnIntoTransfer(f)) = &mut app.dialog else { panic!("no dialog") };
            f.account = Some(account);
            f.reload(&app.db);
            (f.other, f.error.is_empty())
        };
        assert_eq!(choice(&mut app, bank), (OtherSide::New, false));
        // Never chosen for the user, however few.
        assert_eq!(choice(&mut app, savings), (OtherSide::Auto, true));
        let second = add(&app.db, savings, 50);
        assert_eq!(choice(&mut app, savings).0, OtherSide::Auto);
        app.db.delete_transaction(second).unwrap();
        choice(&mut app, savings);
        if let Some(Dialog::TurnIntoTransfer(f)) = &mut app.dialog {
            f.other = OtherSide::Join(into);
        }
        // Account, the candidate, none of these, then the button.
        let ctx = egui::Context::default();
        let send = |app: &mut App, events: Vec<Event>| {
            for events in [events, vec![]] {
                let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0)));
                let input = egui::RawInput { events, screen_rect, ..Default::default() };
                ctx.run_ui(input, |ui| app.dialogs(&ui.ctx().clone())).textures_delta.clear();
            }
        };
        let press = |key| {
            let event =
                |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
            vec![event(true), event(false)]
        };
        send(&mut app, vec![]);
        for _ in 0..4 {
            send(&mut app, press(Key::Tab));
        }
        send(&mut app, press(Key::Enter));
        assert!(app.dialog.is_none());
        let t = app.db.transaction(out).unwrap();
        assert_eq!(t.transfer_peer, Some(into));
        app.open_turn(&t);
        assert!(matches!(&app.dialog, Some(Dialog::SeparateTransfer(_, true))));
        send(&mut app, vec![]);
        for _ in 0..3 {
            send(&mut app, press(Key::Tab));
        }
        send(&mut app, press(Key::Enter));
        assert!(app.dialog.is_none() && app.db.transaction(out).unwrap().transfer_peer.is_none());
        assert_eq!(app.db.transaction(into).unwrap().amount, 50);
    });
}

/// A card statement that does not add up is imported only when the user says so; then its payment is
/// proposed, linked and unlinked.
#[test]
fn a_card_statement_is_checked_imported_and_paid() {
    use crate::gui::ImportState;
    use cashcraft_engine::importer::{CardRow, Check, CheckKind, Statement, Table};
    for l in [Lang::It, Lang::En] {
        with_lang(l, || {
            let db = Db::open_in_memory().unwrap();
            let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 0, "").unwrap();
            let date = parse_date("2026-12-15").unwrap();
            let tx = NewTx { account_id: bank, amount: -900, date, payee: "x".into(), ..Default::default() };
            let paid = db.add_transaction(&tx).unwrap().unwrap();
            let row = CardRow { date, amount: -900, payee: "shop".into(), memo: String::new(), key: "k".into() };
            let check = |found| Check { kind: CheckKind::Debit, expected: Some(900), found };
            let statement = |found| Statement {
                closing: parse_date("2026-11-30"),
                debit_date: Some(date),
                debit: Some(900),
                residual: None,
                cards: vec!["1234".into()],
                rows: vec![row.clone()],
                unread: 0,
                unread_card: false,
                checks: vec![check(found)],
            };
            let mut app = App::new(db, None);
            let table = |found| Table {
                delimiter: b'\t',
                encoding: "PDF",
                records: vec![],
                header_row: None,
                headers: vec![],
                mapping: Default::default(),
                decimal_comma: true,
                statement: Some(statement(found)),
                lines: vec![],
                ofx: None,
                list: None,
            };
            let mut st = ImportState { account: Some(bank), table: Some(table(901)), ..Default::default() };
            st.reparse();
            assert_eq!(st.rows.len(), 1);
            app.card_account(&mut st);
            assert!(st.account.is_none() && st.new_account.ends_with("1234"));
            app.dialog = Some(Dialog::Import(Box::new(st)));
            let ctx = egui::Context::default();
            let draw = |app: &mut App| {
                for _ in 0..2 {
                    ctx.run_ui(Default::default(), |ui| app.dialogs(&ui.ctx().clone())).textures_delta.clear();
                }
            };
            draw(&mut app);
            let Some(Dialog::Import(st)) = &mut app.dialog else { panic!("no dialog") };
            assert!(!st.can_import());
            st.allow_mismatch = true;
            assert!(st.can_import());
            st.allow_mismatch = false;
            st.table = Some(table(900));
            assert!(st.can_import());
            let card = app.db.add_account("Card", AccountKind::CreditCard, "EUR", 0, "").unwrap();
            app.accounts = app.db.accounts(true).unwrap();
            let Some(Dialog::Import(mut st)) = app.dialog.take() else { panic!("no dialog") };
            app.card_account(&mut st);
            assert_eq!(st.account, Some(card));
            let t = table(900);
            cashcraft_engine::importer::import(&app.db, card, &t, &st.rows, 0, false).unwrap();
            st.cards.reload(&app.db);
            assert_eq!(st.cards.proposals[0].candidates[0].transaction.id, paid);
            app.db.link_card_payment(st.cards.proposals[0].statement.id, paid, None).unwrap();
            st.cards.reload(&app.db);
            assert!(st.cards.proposals.is_empty() && st.cards.statements[0].payment_id == Some(paid));
            st.table = None;
            app.dialog = Some(Dialog::Import(st));
            draw(&mut app);
        });
    }
}

/// A list of movements goes to the account with its IBAN, or to a new one that starts at its opening balance and
/// ends at its closing one.
#[test]
fn a_list_of_movements_finds_or_makes_its_account() {
    use crate::gui::ImportState;
    use cashcraft_engine::bank_pdf::Movements;
    use cashcraft_engine::importer::{self, movements_table};
    const IBAN: &str = "IT60X0542811101000000123456";
    let row = |d: &str, a: &str, t: &str| [d.to_string(), d.to_string(), a.to_string(), t.to_string()];
    let m = Movements {
        rows: vec![row("02/11/2026", "-20,00", "Bar"), row("03/11/2026", "50,00", "Bonifico")],
        opening: Some(10_000),
        closing: Some(13_000),
        closing_on: parse_date("2026-11-03"),
        bank: "Banca di Esempio".into(),
        iban: IBAN.into(),
    };
    let db = Db::open_in_memory().unwrap();
    let old = db.add_account("Old", AccountKind::Bank, "EUR", 0, "").unwrap();
    let mut app = App::new(db, None);
    let state = || {
        let t = movements_table(&m);
        let mut st = ImportState { account: Some(old), ..Default::default() };
        st.use_settings(importer::Settings::detected(&t), None);
        st.table = Some(t);
        st.reparse();
        st
    };
    // No account has the IBAN: a new one is proposed, named after the bank.
    let mut st = state();
    app.list_account(&mut st);
    assert_eq!(st.new_account, "Banca di Esempio");
    let acc = app.db.atomically(|| st.destination(&app.db, &app.accounts, false)).unwrap();
    let res = importer::import(&app.db, acc, st.table.as_ref().unwrap(), &st.rows, 0, false).unwrap();
    assert_eq!((st.rows.len(), res.added), (2, 2), "{:?}", st.rows);
    let made = app.db.accounts(true).unwrap().into_iter().find(|a| a.id == acc).unwrap();
    assert_eq!((made.opening_balance, made.iban.as_str(), made.balance), (10_000, IBAN, 13_000));
    st.new_account.clear();
    st.account = Some(acc);
    st.read_balance(&app.db).unwrap();
    assert_eq!(st.books, Some(13_000));
    // The next list goes to it.
    app.accounts = app.db.accounts(true).unwrap();
    let mut st = state();
    app.list_account(&mut st);
    assert_eq!((st.account, st.new_account.as_str()), (Some(acc), ""));
    // An account chosen by hand, that has no IBAN, takes the list's; its opening balance stays.
    app.db.delete_account(acc).unwrap();
    app.accounts = app.db.accounts(true).unwrap();
    let mut st = state();
    app.list_account(&mut st);
    st.new_account.clear();
    assert_eq!(app.db.atomically(|| st.destination(&app.db, &app.accounts, false)).unwrap(), old);
    let old = app.db.accounts(true).unwrap().into_iter().find(|a| a.id == old).unwrap();
    assert_eq!((old.iban.as_str(), old.opening_balance), (IBAN, 0));
    // Drawn, in both languages.
    for l in [Lang::It, Lang::En] {
        with_lang(l, || {
            let mut st = state();
            app.list_account(&mut st);
            app.dialog = Some(Dialog::Import(Box::new(st)));
            let ctx = egui::Context::default();
            ctx.run_ui(Default::default(), |ui| app.dialogs(&ui.ctx().clone())).textures_delta.clear();
        });
    }
}

/// The window goes on while a file is read, and what was not waited for is not shown.
#[test]
fn a_file_is_read_beside_the_window() {
    use crate::gui::ImportState;
    let file = std::env::temp_dir().join(format!("cashcraft-ui-{}.csv", std::process::id()));
    std::fs::write(&file, "Data;Descrizione;Importo\n01/02/2026;Bar;-1,20\n").unwrap();
    let mut st = ImportState { path_text: file.display().to_string(), ..Default::default() };
    st.load();
    assert!(st.loading.is_some() && st.table.is_none());
    let start = std::time::Instant::now();
    while !st.poll() {
        assert!(start.elapsed().as_secs() < 20);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(st.loading.is_none() && st.rows.len() == 1 && st.can_import() == st.account.is_some());
    st.load();
    st.loading = None;
    assert!(!st.poll() && st.table.is_none());
    st.path_text.push('x');
    st.load();
    while !st.poll() {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!st.error.is_empty());
    std::fs::remove_file(&file).unwrap();
}

/// Rows that may be there already wait for the user, the balance is a check, the columns are remembered.
#[test]
fn likely_duplicates_wait_for_the_user() {
    use crate::gui::ImportState;
    use cashcraft_engine::importer;
    let dir = std::env::temp_dir().join(format!("cashcraft-ui-ofx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("a.csv");
    std::fs::write(&csv, "Data;Descrizione;Importo\n05/01/2026;Bar;-1,20\n06/01/2026;x;y\n").unwrap();
    let ofx = dir.join("a.ofx");
    let body = |currency: &str, account: &str| {
        format!(
            "<STMTRS><CURDEF>{currency}<BANKACCTFROM><ACCTID>{account}</BANKACCTFROM><BANKTRANLIST>\
             <STMTTRN><DTPOSTED>20260106<TRNAMT>-1.20<FITID>A<NAME>BAR</STMTTRN></BANKTRANLIST>\
             <LEDGERBAL><BALAMT>-1.20<DTASOF>20260131</LEDGERBAL></STMTRS>"
        )
    };
    std::fs::write(&ofx, format!("<OFX>{}{}</OFX>", body("EUR", "1"), body("USD", "2"))).unwrap();
    for l in [Lang::It, Lang::En] {
        with_lang(l, || {
            let db = Db::open_in_memory().unwrap();
            let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 0, "").unwrap();
            let mut app = App::new(db, None);
            app.accounts = app.db.accounts(true).unwrap();
            let ctx = egui::Context::default();
            let draw = |app: &mut App| {
                for _ in 0..2 {
                    ctx.run_ui(Default::default(), |ui| app.dialogs(&ui.ctx().clone())).textures_delta.clear();
                }
            };
            let load = |app: &mut App, path: &std::path::Path| {
                let mut st =
                    ImportState { account: Some(bank), path_text: path.display().to_string(), ..Default::default() };
                st.load();
                app.dialog = Some(Dialog::Import(Box::new(st)));
                let start = std::time::Instant::now();
                loop {
                    draw(app);
                    let Some(Dialog::Import(st)) = &app.dialog else { panic!("no dialog") };
                    if st.loading.is_none() {
                        break;
                    }
                    assert!(start.elapsed().as_secs() < 20);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                let Some(Dialog::Import(st)) = app.dialog.take() else { panic!("no dialog") };
                st
            };
            let st = load(&mut app, &csv);
            assert!(st.error.is_empty() && st.rows.len() == 1 && st.problems.len() == 1 && st.saved.is_none());
            let mut settings = st.settings();
            settings.dates = cashcraft_engine::money::DateOrder::Dmy;
            let choices = importer::Choices { settings: Some(&settings), ..Default::default() };
            importer::import_with(&app.db, bank, st.table.as_ref().unwrap(), &st.rows, 0, &choices).unwrap();
            let st = load(&mut app, &csv);
            assert!(st.saved.is_some() && st.settings() == settings);
            app.dialog = Some(Dialog::Import(st));
            draw(&mut app);

            let mut st = load(&mut app, &ofx);
            assert!(st.rows.is_empty() && !st.can_import());
            st.table.as_mut().unwrap().ofx.as_mut().unwrap().chosen = Some(0);
            st.reparse();
            app.dialog = Some(Dialog::Import(st));
            draw(&mut app);
            let Some(Dialog::Import(st)) = &mut app.dialog else { panic!("no dialog") };
            assert_eq!((st.likely.len(), st.books, st.can_import()), (1, Some(-120), false));
            let proposed = st.likely[0].proposed;
            st.decided.insert(0, cashcraft_engine::duplicates::Choice::Skip(proposed));
            assert!(st.can_import());
            // Another account is compared again.
            st.new_account = "New".into();
            draw(&mut app);
            let Some(Dialog::Import(st)) = &mut app.dialog else { panic!("no dialog") };
            assert!(st.likely.is_empty() && st.decided.is_empty() && st.books.is_none() && st.can_import());
            // What was said is there again when the typing is undone, and for every letter of it.
            st.new_account = "Ne".into();
            draw(&mut app);
            let Some(Dialog::Import(st)) = &mut app.dialog else { panic!("no dialog") };
            st.new_account.clear();
            draw(&mut app);
            let Some(Dialog::Import(st)) = &mut app.dialog else { panic!("no dialog") };
            assert!(st.decided.len() == 1 && st.likely.len() == 1 && st.can_import());
            st.table.as_mut().unwrap().ofx.as_mut().unwrap().chosen = Some(1);
            st.new_account.clear();
            st.reparse();
            draw(&mut app);
        });
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The sign and the columns are those of the account; saved columns that cannot be read stop the import.
#[test]
fn saved_columns_are_of_the_account() {
    use crate::gui::ImportState;
    use cashcraft_engine::importer::{self, Settings, analyze};
    let text = "Data;Descrizione;Importo\n05/01/2026;Bar;-1,20\n";
    let db = Db::open_in_memory().unwrap();
    let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 0, "").unwrap();
    let card = db.add_account("Card", AccountKind::CreditCard, "EUR", 0, "").unwrap();
    let t = analyze(text, "UTF-8").unwrap();
    let inverted = Settings { invert: true, ..Settings::detected(&t) };
    let rows = importer::read_rows(&t, &inverted).rows;
    let choices = importer::Choices { settings: Some(&inverted), ..Default::default() };
    importer::import_with(&db, card, &t, &rows, 0, &choices).unwrap();
    let mut app = App::new(db, None);
    app.accounts = app.db.accounts(true).unwrap();
    let ctx = egui::Context::default();
    let draw = |app: &mut App| {
        for _ in 0..2 {
            ctx.run_ui(Default::default(), |ui| app.dialogs(&ui.ctx().clone())).textures_delta.clear();
        }
    };
    let mut st = ImportState { account: Some(card), table: Some(t.clone()), ..Default::default() };
    st.use_settings(Settings::detected(&t), None);
    st.loaded = st.settings();
    st.reparse();
    app.dialog = Some(Dialog::Import(Box::new(st)));
    let state = |app: &mut App, f: &mut dyn FnMut(&mut ImportState)| match &mut app.dialog {
        Some(Dialog::Import(st)) => f(st),
        _ => panic!("no dialog"),
    };
    draw(&mut app);
    state(&mut app, &mut |st| assert!(st.invert && st.saved.is_some() && st.rows[0].amount == 120));
    state(&mut app, &mut |st| st.account = Some(bank));
    draw(&mut app);
    state(&mut app, &mut |st| {
        assert!(!st.invert && st.saved.is_none() && st.rows[0].amount == -120);
        // Changed by hand, and only that stays.
        st.account = Some(card);
    });
    draw(&mut app);
    state(&mut app, &mut |st| {
        st.mapping.extra = Some(1);
        st.reparse();
        st.account = Some(bank);
    });
    draw(&mut app);
    state(&mut app, &mut |st| {
        assert!(!st.invert && st.mapping.extra == Some(1), "the sign of the card is not that of the bank");
        st.sign_by_hand = true;
        st.invert = true;
        st.account = Some(card);
    });
    draw(&mut app);
    state(&mut app, &mut |st| {
        assert!(st.invert);
        st.account = Some(bank);
    });
    draw(&mut app);
    state(&mut app, &mut |st| assert!(st.invert, "chosen by hand"));
    app.db.conn.execute("UPDATE csv_layouts SET settings='{'", []).unwrap();
    state(&mut app, &mut |st| {
        st.loaded_for = None;
        st.account = Some(card);
    });
    draw(&mut app);
    state(&mut app, &mut |st| assert!(st.broken.is_some() && !st.can_import()));
    app.db.forget_layout(app.db.layouts().unwrap()[0].id).unwrap();
    state(&mut app, &mut |st| st.loaded_for = None);
    draw(&mut app);
    state(&mut app, &mut |st| assert!(st.broken.is_none() && st.can_import()));
}

/// "Always categorize like this" with the live count, the order, the preview of a run, the undo and the preview of
/// an import, drawn and used.
#[test]
fn rules_are_made_run_and_undone() {
    use crate::gui::{ImportState, View};
    use crate::rules::{RuleForm, RunForm};
    use cashcraft_engine::rules::{Place, Rule, Scope};
    for l in [Lang::It, Lang::En] {
        with_lang(l, || {
            let db = Db::open_in_memory().unwrap();
            let bank = db.add_account("Bank", AccountKind::Bank, "EUR", 0, "").unwrap();
            let add = |payee: &str, amount| {
                let hash = Some(payee.to_string());
                let tx =
                    NewTx { account_id: bank, amount, payee: payee.into(), import_hash: hash, ..Default::default() };
                db.add_transaction(&tx).unwrap().unwrap()
            };
            let ids = [add("Zoo 1", -500), add("Zoo 2", -700), add("Other", -1)];
            let category = db.category_by_path("Animals", false, true).unwrap();
            db.set_category(ids[0], Some(category)).unwrap();
            db.set_tags(ids[0], &["fun".into()]).unwrap();
            let mut app = App::new(db, None);
            let ctx = egui::Context::default();
            let draw = |app: &mut App| {
                for _ in 0..3 {
                    ctx.run_ui(Default::default(), |ui| {
                        app.rules_view(ui);
                        app.dialogs(&ui.ctx().clone());
                    })
                    .textures_delta
                    .clear();
                }
            };
            let wait = |f: &mut RuleForm| {
                for _ in 0..1000 {
                    if let Some(n) = f.count() {
                        return n;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                panic!("never counted");
            };
            let t = app.db.transaction(ids[0]).unwrap();
            app.open_rule_from(&t);
            draw(&mut app);
            let Some(Dialog::Rule(f)) = &mut app.dialog else { panic!("no dialog") };
            assert!(f.first && f.rule.category_id == Some(category) && f.tags == "fun");
            assert_eq!(wait(f), Ok((1, true)));
            f.rule.payee = "zoo".into();
            assert_eq!(f.count(), None, "not while it is written");
            assert_eq!(wait(f), Ok((2, true)));
            f.max = "6".into();
            assert_eq!(wait(f), Ok((1, true)));
            f.max = "x".into();
            assert!(wait(f).is_err() && f.read().is_err());
            f.max.clear();
            f.rule.regex = true;
            f.rule.payee = "(".into();
            assert!(wait(f).is_err());
            draw(&mut app);
            let Some(Dialog::Rule(f)) = &mut app.dialog else { panic!("no dialog") };
            f.rule.payee = r"^zoo (\d)".into();
            f.rule.rename_payee = "Zoo n. $1".into();
            let rule = f.read().unwrap();
            let id = app.db.insert_rule(&rule, true).unwrap();
            app.db.move_rule(id, Place::Down).unwrap();
            app.db
                .conn
                .execute("INSERT INTO transaction_rules(position,pattern,is_regex,category_id) VALUES(0,'(',1,1)", [])
                .unwrap();
            app.dialog = Some(Dialog::RunRules(RunForm::default()));
            app.reload();
            draw(&mut app);
            let Some(Dialog::RunRules(f)) = &mut app.dialog else { panic!("no dialog") };
            let plan = &f.plan.as_ref().unwrap().1;
            assert_eq!((plan.changes.len(), plan.broken.len(), plan.changes[0].id), (1, 1, ids[1]));
            let token = plan.token.clone();
            f.scope = Scope { all: true, overwrite: false };
            draw(&mut app);
            let Some(Dialog::RunRules(f)) = &app.dialog else { panic!("no dialog") };
            assert_eq!(f.plan.as_ref().unwrap().1.changes.len(), 2);
            assert_eq!(app.db.run_rules(Scope::default(), Some(&token)).unwrap().changed, 1);
            app.dialog = None;
            app.reload();
            draw(&mut app);
            let (rules, last) = app.rules.clone().unwrap();
            assert!(rules[0].problem.is_some() && last.as_ref().unwrap().changed == 1);
            // The transaction says which rule it was, and what the bank wrote.
            let t = app.db.transaction(ids[1]).unwrap();
            app.open_edit_tx(&t);
            let Some(Dialog::Tx(f)) = &app.dialog else { panic!("no dialog") };
            assert!(f.detail.contains("Zoo 2") && f.detail.contains(&id.to_string()), "{}", f.detail);
            draw(&mut app);
            app.dialog = None;
            app.view = View::Register(Some(bank));
            app.period = crate::gui::Period::All;
            app.selected = Some(ids[1]);
            app.reload();
            ctx.run_ui(Default::default(), |ui| app.register(ui, Some(bank))).textures_delta.clear();
            // Before an import.
            let mut st = ImportState { account: Some(bank), ..Default::default() };
            st.table = Some(
                cashcraft_engine::importer::analyze("Data;Descrizione;Importo\n05/01/2026;Zoo 7;-1,20\n", "UTF-8")
                    .unwrap(),
            );
            st.use_settings(cashcraft_engine::importer::Settings::detected(st.table.as_ref().unwrap()), None);
            st.reparse();
            app.dialog = Some(Dialog::Import(Box::new(st)));
            draw(&mut app);
            let Some(Dialog::Import(st)) = &app.dialog else { panic!("no dialog") };
            let (what, why) = &st.ruled[&0];
            assert!(what.contains("Animals") && what.contains("Zoo n. 7") && why.contains(&id.to_string()));
            assert!(!st.broken_rules.is_empty());
            app.dialog = None;
            // Asked first, with the run.
            let run = last.clone().unwrap();
            app.dialog = Some(Dialog::ConfirmUndoRules(run.clone()));
            draw(&mut app);
            assert!(matches!(&app.dialog, Some(Dialog::ConfirmUndoRules(l)) if l.text().contains(&l.made_at)));
            app.dialog = None;
            assert_eq!(app.db.undo_run(run.id).unwrap().undone, 1);
            assert_eq!(app.db.transaction(ids[1]).unwrap().payee, "Zoo 2");
            // A category that takes rules with it is deleted after a question.
            ctx.run_ui(Default::default(), |ui| app.delete_category(ui, category)).textures_delta.clear();
            assert!(matches!(&app.dialog, Some(Dialog::ConfirmDeleteCategory(c, _, 1)) if *c == category));
            draw(&mut app);
            app.dialog = Some(Dialog::ConfirmDeleteAccount(bank, "Bank".into(), 2));
            draw(&mut app);
            app.dialog = None;
            // The category that a rule gave, chosen again, is the user's.
            app.db.run_rules(Scope::default(), None).unwrap();
            let mut form = form_of(&mut app, ids[1]);
            form.category_chosen = false;
            app.save_tx(&form).unwrap();
            assert!(app.db.transaction(ids[1]).unwrap().rules.category.is_some());
            form.category_chosen = true;
            app.save_tx(&form).unwrap();
            assert!(app.db.transaction(ids[1]).unwrap().rules.category.is_none());
            // A rule as it is, and a new one.
            let f = RuleForm::new(&app.db, app.db.rule(id).unwrap(), Some(id), false).unwrap();
            assert_eq!(f.read().unwrap(), app.db.rule(id).unwrap());
            app.open_rule(Rule::default(), None, false);
            draw(&mut app);
            assert!(matches!(&app.dialog, Some(Dialog::Rule(_))));
        });
    }
}

fn form_of(app: &mut App, id: i64) -> crate::gui::TxForm {
    let t = app.db.transaction(id).unwrap();
    app.open_edit_tx(&t);
    match app.dialog.take() {
        Some(Dialog::Tx(f)) => f,
        _ => panic!("no form"),
    }
}

/// A long condition stays on two lines, in a column that takes most of the width.
#[test]
fn long_rules_stay_on_two_lines() {
    let long = "payee: instalment of the financing for social security contributions ".repeat(4);
    let mut heights = Vec::new();
    egui::__run_test_ui(|ui| {
        let [_, when, then, _] = crate::rules::rule_columns(900.0);
        assert!(when > then && when > 900.0 / 2.0 - 100.0);
        for text in ["x", long.as_str()] {
            let top = ui.cursor().top();
            crate::rules::rule_cell(ui, when, |ui| crate::rules::two_lines(ui, text, egui::Color32::BLACK));
            heights.push(ui.cursor().top() - top);
        }
    });
    for pair in heights.chunks(2) {
        assert!(pair[1] > pair[0] && pair[1] < pair[0] * 2.5, "{pair:?}");
    }
}

#[test]
fn both_themes_keep_text_readable() {
    use crate::gui::{DARK, LIGHT};
    use egui::Color32;
    fn lum(c: Color32) -> f32 {
        let ch = |v: u8| {
            let s = f32::from(v) / 255.0;
            if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    }
    fn contrast(a: Color32, b: Color32) -> f32 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }
    for p in [&LIGHT, &DARK] {
        for bg in [p.bg, p.card, p.sidebar, p.zebra, p.accent_soft] {
            assert!(contrast(p.text, bg) >= 4.5);
            assert!(contrast(p.muted, bg) >= 4.5);
        }
        // Amounts and links on cards: normal-size text.
        for fg in [p.accent, p.red, p.green] {
            assert!(contrast(fg, p.card) >= 4.5);
        }
        // The selected item of the sidebar: accent text on its soft fill.
        for bg in [p.accent_soft, p.sidebar] {
            assert!(contrast(p.accent, bg) >= 4.5);
        }
        // White text on filled buttons.
        for fill in [p.accent_fill, p.red_fill, p.green_fill] {
            assert!(contrast(Color32::WHITE, fill) >= 4.5);
        }
    }
}

#[test]
fn the_theme_follows_the_system_until_chosen() {
    use crate::gui::Theme;
    assert_eq!(Theme::from_setting(None), Theme::System);
    assert_eq!(Theme::from_setting(Some("something else")), Theme::System);
    for t in Theme::ALL {
        assert_eq!(Theme::from_setting(Some(t.code())), t);
    }
}

/// A statement is reconciled without the mouse; a reconciled transaction asks before it goes.
#[test]
fn a_statement_is_reconciled_with_the_keyboard() {
    use cashcraft_engine::reconcile::{Source, Start, Status};
    use egui::{Event, Key, Modifiers};
    with_lang(Lang::En, || {
        let db = Db::open_in_memory().unwrap();
        let a = db.add_account("A", AccountKind::Bank, "EUR", 0, "").unwrap();
        let date = parse_date("2026-01-02").unwrap();
        let bar = NewTx { account_id: a, date, amount: -120, payee: "BAR".into(), ..Default::default() };
        let bar = db.add_transaction(&bar).unwrap().unwrap();
        let t = cashcraft_engine::importer::analyze(
            "Data;Descrizione;Importo\n02/01/2026;BAR;-1,20\n10/01/2026;STIPENDIO;1500,00\n",
            "UTF-8",
        )
        .unwrap();
        let rows = cashcraft_engine::importer::parse_rows(&t, &t.mapping, false).0;
        let end = parse_date("2026-01-31").unwrap();
        let id = db
            .start_reconciliation(&Start { account: a, date: end, balance: 149_880, source: Source::Csv, rows: &rows })
            .unwrap()
            .id;
        let mut app = App::new(db, None);
        app.view = View::Reconcile(a);
        let ctx = egui::Context::default();
        let mut time = 0.0;
        let mut send = |app: &mut App, events: Vec<Event>| {
            for events in [events, vec![]] {
                time += 0.5;
                let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0)));
                let input = egui::RawInput { events, screen_rect, time: Some(time), ..Default::default() };
                ctx.run_ui(input, |ui| {
                    app.reconcile_view(ui, a);
                    app.dialogs(&ui.ctx().clone());
                })
                .textures_delta
                .clear();
            }
        };
        let press = |key, modifiers| {
            let event = |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
            vec![event(true), event(false)]
        };
        send(&mut app, vec![]);
        // The bar is proposed: Space confirms it. The salary is missing: Space adds it.
        send(&mut app, press(Key::Space, Modifiers::NONE));
        assert!(app.db.transaction(bar).unwrap().cleared);
        send(&mut app, press(Key::ArrowDown, Modifiers::NONE));
        send(&mut app, press(Key::Space, Modifiers::NONE));
        assert_eq!(app.db.reconciliation(id).unwrap().difference, 0);
        // Finishing asks first: Tab reaches its button, Enter presses it.
        send(&mut app, press(Key::Enter, Modifiers::COMMAND));
        assert!(matches!(app.dialog, Some(Dialog::ConfirmReconcile(_))));
        assert_eq!(app.db.reconciliation(id).unwrap().status, Status::Open);
        send(&mut app, press(Key::Tab, Modifiers::NONE));
        send(&mut app, press(Key::Enter, Modifiers::NONE));
        assert!(app.dialog.is_none());
        assert_eq!(app.db.reconciliation(id).unwrap().status, Status::Finished);
        assert!(app.db.transaction(bar).unwrap().reconciled);
        send(&mut app, vec![]);
        assert!(app.reconcile.as_ref().is_some_and(|r| r.preview.is_none() && r.history.len() == 1));
        // Deleting it asks first; after the yes it goes and the reconciliation is broken.
        let r = app.db.delete_transaction(bar).map(|_| String::new());
        assert!(app.unless_reconciled(r, || Unlock::Delete(bar)).is_none());
        assert!(matches!(app.dialog, Some(Dialog::ConfirmUnreconcile(_))));
        send(&mut app, vec![]);
        assert!(app.db.transaction(bar).is_ok());
        // Its first button is the yes.
        send(&mut app, press(Key::Tab, Modifiers::NONE));
        send(&mut app, press(Key::Enter, Modifiers::NONE));
        assert!(app.dialog.is_none());
        assert!(app.db.transaction(bar).is_err());
        assert_eq!(app.db.reconciliation(id).unwrap().status, Status::Broken);
        // The form of a new one, and the history, draw.
        app.reconcile = None;
        app.view = View::Reconcile(a);
        for _ in 0..2 {
            ctx.run_ui(Default::default(), |ui| app.reconcile_view(ui, a)).textures_delta.clear();
        }
    });
}

/// An amount is typed on the envelope chosen with the arrows and saved with Enter; an overspent one is covered.
#[test]
fn envelopes_are_assigned_with_the_keyboard() {
    use cashcraft_engine::envelopes::{Mode, Overspending};
    use egui::{Event, Key, Modifiers};
    with_lang(Lang::En, || {
        let db = Db::open_in_memory().unwrap();
        let a = db.add_account("A", AccountKind::Bank, "EUR", 0, "").unwrap();
        let food = db.category_by_path("Food", false, true).unwrap();
        let fun = db.category_by_path("Fun", false, true).unwrap();
        let pay = db.category_by_path("Pay", true, true).unwrap();
        db.set_budget_mode(Mode::Envelopes).unwrap();
        let month = cashcraft_engine::db::first_of_month(cashcraft_engine::db::today());
        for (amount, category) in [(100_000, pay), (-3_000, food)] {
            let t = NewTx {
                account_id: a,
                date: month,
                amount,
                payee: "x".into(),
                category_id: Some(category),
                ..Default::default()
            };
            db.add_transaction(&t).unwrap();
        }
        let e_food = db.add_envelope("Food", &[food], Overspending::Carry, db.envelopes_start()).unwrap();
        let e_fun = db.add_envelope("Fun", &[fun], Overspending::Carry, db.envelopes_start()).unwrap();
        let mut app = App::new(db, None);
        app.view = View::Budget;
        let ctx = egui::Context::default();
        let mut time = 0.0;
        let mut send = |app: &mut App, events: Vec<Event>| {
            for events in [events, vec![]] {
                time += 0.5;
                let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0)));
                let input = egui::RawInput { events, screen_rect, time: Some(time), ..Default::default() };
                ctx.run_ui(input, |ui| {
                    app.budget(ui);
                    app.dialogs(&ui.ctx().clone());
                })
                .textures_delta
                .clear();
            }
        };
        let press = |key| {
            let event =
                |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
            vec![event(true), event(false)]
        };
        let line = |app: &App, id| {
            let v = app.db.envelope_month(month).unwrap();
            v.envelopes.iter().find(|e| e.id == id).map(|e| (e.assigned, e.available)).unwrap()
        };
        send(&mut app, vec![]);
        // Fun is the second: an amount typed there is saved with Enter.
        send(&mut app, press(Key::ArrowDown));
        send(&mut app, vec![Event::Text("9".into())]);
        send(&mut app, vec![Event::Text("70".into())]);
        send(&mut app, press(Key::Enter));
        assert_eq!(line(&app, e_fun), (97_000, 97_000));
        // Up, Enter edits Food; Escape leaves it as it was.
        send(&mut app, press(Key::ArrowUp));
        send(&mut app, press(Key::Enter));
        assert!(app.envelopes.editing.is_some());
        send(&mut app, press(Key::Escape));
        assert!(app.envelopes.editing.is_none());
        assert_eq!(line(&app, e_food), (0, -3_000));
        assert_eq!(app.db.envelope_month(month).unwrap().to_be_assigned, 3_000);
        // Food is overspent: covered from Fun.
        app.envelopes.cover = Some((e_food, Some(e_fun), "30".into(), "groceries".into()));
        send(&mut app, vec![]);
        app.db.move_money(e_fun, e_food, month, 3_000, "groceries").unwrap();
        app.envelopes.cover = None;
        app.reload();
        send(&mut app, vec![]);
        assert_eq!(line(&app, e_food), (3_000, 0));
        // ⌘Z undoes the last change of the month shown, not one made later in another month.
        let next = month.checked_add_months(chrono::Months::new(1)).unwrap();
        app.db.assign(e_food, next, 1, "").unwrap();
        app.reload();
        let z = vec![
            Event::Key { key: Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND },
            Event::Key {
                key: Key::Z,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: Modifiers::COMMAND,
            },
        ];
        send(&mut app, z);
        assert_eq!(line(&app, e_food), (0, -3_000));
        let later = app.db.envelope_month(next).unwrap();
        assert_eq!(later.envelopes.iter().find(|e| e.id == e_food).unwrap().assigned, 1);
        // The settings of an envelope, the changes and a month before the start draw.
        app.envelopes.settings = Some((e_food, "Food".into(), None));
        send(&mut app, vec![]);
        app.budget_month = month.checked_sub_months(chrono::Months::new(1)).unwrap();
        send(&mut app, vec![]);
        // Back to the monthly limits.
        app.db.set_budget_mode(Mode::Monthly).unwrap();
        send(&mut app, vec![]);
    });
}

/// The middle of the widget whose accessible label is `label`, as the screen last drew it.
fn spot(out: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
    let update = out.platform_output.accesskit_update.as_ref()?;
    update.nodes.iter().find_map(|(_, n)| {
        let b = n.bounds().filter(|_| n.label().is_some_and(|l| l == label))?;
        Some(egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32))
    })
}

/// The budget screen driven as a person would: keys, and clicks where the buttons are drawn.
struct Screen {
    app: App,
    ctx: egui::Context,
    time: f64,
    last: Option<egui::FullOutput>,
}

impl Screen {
    fn new(app: App) -> Screen {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut s = Screen { app, ctx, time: 0.0, last: None };
        s.send(vec![]);
        s.send(vec![]);
        s
    }

    fn send(&mut self, events: Vec<egui::Event>) {
        self.time += 0.5;
        let screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0)));
        let input = egui::RawInput { events, screen_rect, time: Some(self.time), ..Default::default() };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(input, |ui| {
            app.budget(ui);
            app.dialogs(&ui.ctx().clone());
        });
        out.textures_delta.clear();
        self.last = Some(out);
    }

    fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        let event = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
        self.send(vec![event(true), event(false)]);
        self.send(vec![]);
    }

    fn click(&mut self, label: &str) {
        use egui::{Event, Modifiers, PointerButton};
        let pos = self.last.as_ref().and_then(|o| spot(o, label)).unwrap_or_else(|| panic!("no \"{label}\""));
        self.send(vec![Event::PointerMoved(pos)]);
        self.send(vec![Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }]);
        self.send(vec![Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        self.send(vec![]);
        self.send(vec![]);
    }

    fn shows(&self, label: &str) -> bool {
        self.last.as_ref().and_then(|o| spot(o, label)).is_some()
    }
}

/// What is being typed belongs to its month: another month, or another screen, drops it; nothing reaches the other
/// month. Enter is the field's only while it has the keyboard.
#[test]
fn an_amount_being_typed_never_reaches_another_month() {
    use cashcraft_engine::envelopes::{Mode, Overspending};
    use egui::{Event, Key, Modifiers};
    with_lang(Lang::En, || {
        let db = Db::open_in_memory().unwrap();
        let fun = db.category_by_path("Fun", false, true).unwrap();
        db.set_budget_mode(Mode::Envelopes).unwrap();
        let this = cashcraft_engine::db::first_of_month(cashcraft_engine::db::today());
        let next = this.checked_add_months(chrono::Months::new(1)).unwrap();
        let e = db.add_envelope("Fun", &[fun], Overspending::Carry, this).unwrap();
        let mut app = App::new(db, None);
        app.view = View::Budget;
        let mut s = Screen::new(app);
        let assigned =
            |s: &Screen, m| s.app.db.envelope_month(m).unwrap().envelopes.iter().find(|l| l.id == e).unwrap().assigned;
        s.send(vec![Event::Text("75".into())]);
        s.send(vec![]);
        assert!(s.app.envelopes.editing.is_some());
        s.click("▶");
        assert_eq!(s.app.budget_month, next);
        assert!(s.app.envelopes.editing.is_none());
        assert_eq!((assigned(&s, this), assigned(&s, next)), (0, 0));
        // Typed and saved with Enter, after the click: in the month shown.
        s.send(vec![Event::Text("12".into())]);
        s.send(vec![]);
        s.key(Key::Enter, Modifiers::NONE);
        assert_eq!((assigned(&s, this), assigned(&s, next)), (0, 1_200));
        // Typed, then another screen and back: dropped.
        s.send(vec![Event::Text("3".into())]);
        s.send(vec![]);
        assert!(s.app.envelopes.editing.is_some());
        s.app.go(View::Reports);
        s.app.go(View::Budget);
        s.send(vec![]);
        assert!(s.app.envelopes.editing.is_none());
        // Typed, then Escape: dropped; Enter alone opens the field with what the month has.
        s.send(vec![Event::Text("4".into())]);
        s.send(vec![]);
        s.key(Key::Escape, Modifiers::NONE);
        assert!(s.app.envelopes.editing.is_none());
        s.key(Key::Enter, Modifiers::NONE);
        assert_eq!(s.app.envelopes.editing.as_ref().map(|e| e.1.as_str()), Some("12.00"));
        s.key(Key::Escape, Modifiers::NONE);
        assert_eq!((assigned(&s, this), assigned(&s, next)), (0, 1_200));
    });
}

/// Cover…, Move, and the deletion that asks first and comes back with ⌘Z, all by clicking.
#[test]
fn an_overspent_envelope_is_covered_and_one_deleted_by_clicking() {
    use cashcraft_engine::envelopes::{Mode, Overspending};
    use egui::{Key, Modifiers};
    with_lang(Lang::En, || {
        let db = Db::open_in_memory().unwrap();
        let a = db.add_account("A", AccountKind::Bank, "EUR", 0, "").unwrap();
        let food = db.category_by_path("Food", false, true).unwrap();
        let fun = db.category_by_path("Fun", false, true).unwrap();
        db.set_budget_mode(Mode::Envelopes).unwrap();
        let month = cashcraft_engine::db::first_of_month(cashcraft_engine::db::today());
        let t = NewTx {
            account_id: a,
            date: month,
            amount: -3_000,
            payee: "x".into(),
            category_id: Some(food),
            ..Default::default()
        };
        db.add_transaction(&t).unwrap();
        let e_food = db.add_envelope("Food", &[food], Overspending::Carry, month).unwrap();
        let e_fun = db.add_envelope("Fun", &[fun], Overspending::Carry, month).unwrap();
        db.assign(e_fun, month, 10_000, "").unwrap();
        let mut app = App::new(db, None);
        app.view = View::Budget;
        let mut s = Screen::new(app);
        let line = |s: &Screen, id| {
            let v = s.app.db.envelope_month(month).unwrap();
            v.envelopes.iter().find(|l| l.id == id).map(|l| (l.assigned, l.available))
        };
        s.click("Cover…");
        assert!(s.app.envelopes.cover.is_some());
        s.click("Move");
        assert_eq!((line(&s, e_food), line(&s, e_fun)), (Some((3_000, 0)), Some((7_000, 7_000))));
        assert!(!s.shows("Cover…"));
        // Deleting asks first, and says what goes back.
        s.app.envelopes.cursor = 1;
        s.app.envelopes.settings = Some((e_fun, "Fun".into(), None));
        s.send(vec![]);
        s.click("Delete the envelope…");
        assert!(line(&s, e_fun).is_some());
        let asked = s.last.as_ref().and_then(|o| o.platform_output.accesskit_update.as_ref()).is_some_and(|u| {
            u.nodes.iter().any(|(_, n)| {
                [n.label(), n.value()]
                    .into_iter()
                    .flatten()
                    .any(|l| l.contains("€70.00, which goes back to be assigned"))
            })
        });
        assert!(asked);
        s.click("Delete");
        assert_eq!(line(&s, e_fun), None);
        s.key(Key::Z, Modifiers::COMMAND);
        assert_eq!(line(&s, e_fun), Some((7_000, 7_000)));
    });
}

/// The start view of the screenshots turns only the sample data in memory into envelopes, never a file of the user's.
#[test]
fn the_start_view_never_changes_a_file() {
    use cashcraft_engine::envelopes::Mode;
    let dir = std::env::temp_dir().join(format!("cashcraft-start-view-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (db, _) = Db::open(&dir.join("books.db")).unwrap();
    let mut app = App::new(db, None);
    app.sample_envelopes("envelopes");
    assert_eq!(app.db.budget_mode(), Mode::Monthly);
    assert!(app.db.meta("envelopes_from").is_none());
    let mut sample = App::new(Db::open_in_memory().unwrap(), None);
    sample.sample_envelopes("envelopes");
    assert_eq!(sample.db.budget_mode(), Mode::Envelopes);
    drop(app);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Make recurring (#42) opens the scheduled form filled in from the transaction, a month after it; the
/// due date follows the repetition until the user writes another.
#[test]
fn the_recurring_form_reads_back_the_transaction() {
    for l in [Lang::It, Lang::En] {
        with_lang(l, || {
            let db = Db::open_in_memory().unwrap();
            let account_id = db.add_account("A", AccountKind::Bank, "EUR", 0, "").unwrap();
            let mut app = App::new(db, None);
            let date = parse_date("2026-01-31").unwrap();
            let tx = NewTx { account_id, amount: -65_050, date, payee: "Mortgage".into(), ..Default::default() };
            let id = app.db.add_transaction(&tx).unwrap().unwrap();
            let t = app.db.transaction(id).unwrap();
            app.open_recurring(&t).unwrap();
            let Some(Dialog::Scheduled(f)) = &mut app.dialog else { panic!("no scheduled form") };
            assert_eq!(
                (f.account, f.transfer, f.expense, f.payee.as_str()),
                (Some(account_id), false, true, "Mortgage")
            );
            assert_eq!(parse_amount(&f.amount).unwrap(), 65_050);
            assert_eq!(parse_date(&f.date), parse_date("2026-02-28"));
            let repeat = |f: &mut SchedForm, unit: &str| {
                let was = (f.every.clone(), f.unit.clone());
                f.unit = unit.into();
                follow_repetition(f, (&was.0, &was.1));
            };
            repeat(f, "week");
            assert_eq!(parse_date(&f.date), parse_date("2026-02-07"));
            repeat(f, "month");
            assert_eq!(parse_date(&f.date), parse_date("2026-02-28"));
            // A date the user wrote stays, even one that a repetition would give.
            f.date = "07/02/2026".into();
            for unit in ["week", "month", "day"] {
                repeat(f, unit);
                assert_eq!(f.date, "07/02/2026", "{unit}");
            }
        });
    }
}
