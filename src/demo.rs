//! Dati di esempio mostrati al primo avvio della finestra (eliminabili con un clic).

use crate::db::{self, AccountKind, Db, NewTx};
use anyhow::Result;
use chrono::{Datelike, Duration, Months, NaiveDate};

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % ((hi - lo).max(1) as u64)) as i64
    }
}

pub fn seed(db: &Db) -> Result<()> {
    let today = db::today();
    let cat = |p: &str| db.category_by_path(p, false, false).ok();
    let conto = db.add_account("Conto corrente", AccountKind::Bank, "EUR", 2_480_00, "")?;
    let carta = db.add_account("Carta di credito", AccountKind::CreditCard, "EUR", 0, "")?;
    let cash = db.add_account("Portafoglio", AccountKind::Cash, "EUR", 120_00, "")?;
    let risp = db.add_account("Conto deposito", AccountKind::Savings, "EUR", 8_500_00, "")?;
    let mut rng = Lcg(42);
    let start = db::first_of_month(today)
        .checked_sub_months(Months::new(6))
        .unwrap();
    let add =
        |acc: i64, d: NaiveDate, amt: i64, payee: &str, memo: &str, c: Option<i64>| -> Result<()> {
            if d <= today {
                db.add_transaction(&NewTx {
                    account_id: acc,
                    date: d,
                    amount: amt,
                    payee: payee.into(),
                    memo: memo.into(),
                    category_id: c,
                    cleared: d < today - Duration::days(4),
                    import_hash: None,
                })?;
            }
            Ok(())
        };
    let mut month = start;
    while month <= today {
        let day = |d: u32| NaiveDate::from_ymd_opt(month.year(), month.month(), d).unwrap();
        add(
            conto,
            day(1),
            -750_00,
            "Affitto appartamento",
            "Canone mensile",
            cat("Casa:Affitto e mutuo"),
        )?;
        add(
            conto,
            day(27),
            2_150_00,
            "Stipendio",
            "Bonifico emolumenti",
            cat("Stipendio"),
        )?;
        add(
            conto,
            day(5),
            -rng.range(55_00, 95_00),
            "Energia Elettrica",
            "Addebito diretto bolletta",
            cat("Casa:Bollette"),
        )?;
        add(
            conto,
            day(12),
            -29_90,
            "Fibra Casa",
            "Abbonamento internet",
            cat("Abbonamenti"),
        )?;
        add(
            carta,
            day(8),
            -12_99,
            "Streaming Video",
            "Abbonamento mensile",
            cat("Abbonamenti"),
        )?;
        add(
            conto,
            day(28),
            -3_50,
            "Canone conto",
            "Spese tenuta conto",
            cat("Banca e tasse:Commissioni"),
        )?;
        if month.month() % 2 == 0 {
            add(
                conto,
                day(18),
                -rng.range(40_00, 70_00),
                "Gas e acqua",
                "Bolletta bimestrale",
                cat("Casa:Bollette"),
            )?;
        }
        for w in 0..4 {
            let d = day(3 + w * 7);
            add(
                carta,
                d,
                -rng.range(45_00, 110_00),
                "Supermercato Centro",
                "Spesa settimanale",
                cat("Alimentari"),
            )?;
            add(
                cash,
                d + Duration::days(2),
                -rng.range(2_50, 9_00),
                "Bar Piazza",
                "Colazione",
                cat("Ristoranti e bar"),
            )?;
        }
        add(
            carta,
            day(14),
            -rng.range(25_00, 80_00),
            "Pizzeria Da Gino",
            "Cena",
            cat("Ristoranti e bar"),
        )?;
        add(
            carta,
            day(10),
            -rng.range(50_00, 70_00),
            "Distributore Carburante",
            "Rifornimento",
            cat("Trasporti:Carburante"),
        )?;
        add(
            carta,
            day(24),
            -rng.range(50_00, 70_00),
            "Distributore Carburante",
            "Rifornimento",
            cat("Trasporti:Carburante"),
        )?;
        add(
            conto,
            day(16),
            -rng.range(8_00, 40_00),
            "Farmacia Comunale",
            "",
            cat("Salute"),
        )?;
        add(
            carta,
            day(20),
            -rng.range(20_00, 90_00),
            "Cinema e libri",
            "",
            cat("Svago"),
        )?;
        if month.month() % 3 == 0 {
            add(
                carta,
                day(21),
                -rng.range(60_00, 160_00),
                "Negozio Abbigliamento",
                "",
                cat("Abbigliamento"),
            )?;
        }
        add(
            conto,
            day(19),
            -rng.range(15_00, 60_00),
            "Pagamento POS 0412",
            "",
            None,
        )?;
        // Saldo carta e risparmio.
        let pay = day(2);
        if pay <= today && month > start {
            db.add_transfer(conto, carta, pay, 420_00, "Saldo estratto carta")?;
        }
        let save = day(28);
        if save <= today {
            db.add_transfer(conto, risp, save, 300_00, "Risparmio mensile")?;
        }
        if day(9) <= today {
            db.add_transfer(conto, cash, day(9), 100_00, "Prelievo bancomat")?;
        }
        month = month.checked_add_months(Months::new(1)).unwrap();
    }
    add(
        risp,
        NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap(),
        12_40,
        "Interessi creditori",
        "",
        cat("Interessi"),
    )?;

    for (c, amt) in [
        ("Alimentari", 400_00),
        ("Ristoranti e bar", 150_00),
        ("Trasporti:Carburante", 140_00),
        ("Svago", 80_00),
        ("Casa:Bollette", 120_00),
        ("Abbonamenti", 45_00),
    ] {
        if let Some(id) = cat(c) {
            db.set_budget(id, amt)?;
        }
    }
    let next = |d: u32| {
        let m = NaiveDate::from_ymd_opt(today.year(), today.month(), d).unwrap();
        if m > today {
            m
        } else {
            m.checked_add_months(Months::new(1)).unwrap()
        }
    };
    db.add_scheduled(
        conto,
        None,
        -750_00,
        "Affitto appartamento",
        "Canone mensile",
        cat("Casa:Affitto e mutuo"),
        next(1),
        1,
        "month",
    )?;
    db.add_scheduled(
        conto,
        None,
        2_150_00,
        "Stipendio",
        "",
        cat("Stipendio"),
        next(27),
        1,
        "month",
    )?;
    db.add_scheduled(
        conto,
        Some(risp),
        300_00,
        "Risparmio mensile",
        "Risparmio mensile",
        None,
        next(28),
        1,
        "month",
    )?;
    db.set_meta("demo", "1")?;
    Ok(())
}
