use crate::gui::{App, Dialog, LANGUAGES, short_eur};
use crate::ledger::{EntryForm, tree};
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
    app.db.conn.execute_batch("ALTER TABLE book_postings RENAME TO gone").unwrap();
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
