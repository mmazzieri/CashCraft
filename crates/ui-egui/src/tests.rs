use crate::gui::{App, Dialog, LANGUAGES, short_eur};
use cashcraft_engine::db::{AccountKind, Db, NewTx};
use cashcraft_engine::i18n::{Lang, META_KEY, with_lang};
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

/// Symbols drawn by the interface must exist in the bundled fonts, or they show as empty boxes.
#[test]
fn every_symbol_has_a_glyph() {
    let ctx = egui::Context::default();
    let sources =
        [include_str!("gui.rs"), include_str!("views.rs"), include_str!("dialogs.rs"), cashcraft_engine::i18n::IT_TSV]
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
