//! Dati di esempio mostrati al primo avvio della finestra (eliminabili con un clic).

use crate::db::{self, AccountKind, Db, NewScheduled, NewTx};
use anyhow::Result;
use chrono::{Datelike, Duration, Months, NaiveDate};

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % ((hi - lo).max(1) as u64)) as i64
    }
}

pub fn seed(db: &Db) -> Result<()> {
    let today = db::today();
    let cat = |p: &str| db.category_by_path(p, false, false).ok();
    let conto = db.add_account("Conto corrente", AccountKind::Bank, "EUR", 248_000, "")?;
    let carta = db.add_account("Carta di credito", AccountKind::CreditCard, "EUR", 0, "")?;
    let cash = db.add_account("Portafoglio", AccountKind::Cash, "EUR", 12_000, "")?;
    let risp = db.add_account("Conto deposito", AccountKind::Savings, "EUR", 850_000, "")?;
    let mut rng = Lcg(42);
    let start = crate::ops::months_back(today, 6);
    let add = |acc: i64, d: NaiveDate, amt: i64, payee: &str, memo: &str, c: Option<i64>| -> Result<()> {
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
        let day = |d: u32| month.with_day(d).unwrap_or(month);
        add(conto, day(1), -75_000, "Affitto appartamento", "Canone mensile", cat("Casa:Affitto e mutuo"))?;
        add(conto, day(27), 215_000, "Stipendio", "Bonifico emolumenti", cat("Stipendio"))?;
        add(
            conto,
            day(5),
            -rng.range(5_500, 9_500),
            "Energia Elettrica",
            "Addebito diretto bolletta",
            cat("Casa:Bollette"),
        )?;
        add(conto, day(12), -2_990, "Fibra Casa", "Abbonamento internet", cat("Abbonamenti"))?;
        add(carta, day(8), -1_299, "Streaming Video", "Abbonamento mensile", cat("Abbonamenti"))?;
        add(conto, day(28), -350, "Canone conto", "Spese tenuta conto", cat("Banca e tasse:Commissioni"))?;
        if month.month().is_multiple_of(2) {
            add(conto, day(18), -rng.range(4_000, 7_000), "Gas e acqua", "Bolletta bimestrale", cat("Casa:Bollette"))?;
        }
        for w in 0..4 {
            let d = day(3 + w * 7);
            add(carta, d, -rng.range(4_500, 11_000), "Supermercato Centro", "Spesa settimanale", cat("Alimentari"))?;
            add(cash, d + Duration::days(2), -rng.range(250, 900), "Bar Piazza", "Colazione", cat("Ristoranti e bar"))?;
        }
        add(carta, day(14), -rng.range(2_500, 8_000), "Pizzeria Da Gino", "Cena", cat("Ristoranti e bar"))?;
        add(
            carta,
            day(10),
            -rng.range(5_000, 7_000),
            "Distributore Carburante",
            "Rifornimento",
            cat("Trasporti:Carburante"),
        )?;
        add(
            carta,
            day(24),
            -rng.range(5_000, 7_000),
            "Distributore Carburante",
            "Rifornimento",
            cat("Trasporti:Carburante"),
        )?;
        add(conto, day(16), -rng.range(800, 4_000), "Farmacia Comunale", "", cat("Salute"))?;
        add(carta, day(20), -rng.range(2_000, 9_000), "Cinema e libri", "", cat("Svago"))?;
        if month.month().is_multiple_of(3) {
            add(carta, day(21), -rng.range(6_000, 16_000), "Negozio Abbigliamento", "", cat("Abbigliamento"))?;
        }
        add(conto, day(19), -rng.range(1_500, 6_000), "Pagamento POS 0412", "", None)?;
        // Saldo carta e risparmio.
        let pay = day(2);
        if pay <= today && month > start {
            db.add_transfer(conto, carta, pay, 42_000, "Saldo estratto carta")?;
        }
        let save = day(28);
        if save <= today {
            db.add_transfer(conto, risp, save, 30_000, "Risparmio mensile")?;
        }
        if day(9) <= today {
            db.add_transfer(conto, cash, day(9), 10_000, "Prelievo bancomat")?;
        }
        let Some(next) = month.checked_add_months(Months::new(1)) else { break };
        month = next;
    }
    add(risp, db::first_of_month(today), 1_240, "Interessi creditori", "", cat("Interessi"))?;

    for (c, amt) in [
        ("Alimentari", 40_000),
        ("Ristoranti e bar", 15_000),
        ("Trasporti:Carburante", 14_000),
        ("Svago", 8_000),
        ("Casa:Bollette", 12_000),
        ("Abbonamenti", 4_500),
    ] {
        if let Some(id) = cat(c) {
            db.set_budget(id, amt)?;
        }
    }
    let next = |d: u32| {
        let m = today.with_day(d).unwrap_or(today);
        if m > today { m } else { m.checked_add_months(Months::new(1)).unwrap_or(m) }
    };
    let repeats = [
        ("Affitto appartamento", None, -75_000, "Canone mensile", cat("Casa:Affitto e mutuo"), 1),
        ("Stipendio", None, 215_000, "", cat("Stipendio"), 27),
        ("Risparmio mensile", Some(risp), 30_000, "Risparmio mensile", None, 28),
    ];
    for (payee, to_account_id, amount, memo, category_id, day) in repeats {
        db.add_scheduled(&NewScheduled {
            account_id: conto,
            to_account_id,
            amount,
            payee: payee.into(),
            memo: memo.into(),
            category_id,
            next_date: next(day),
            every: 1,
            unit: "month".into(),
        })?;
    }
    db.set_meta("demo", "1")?;
    Ok(())
}
