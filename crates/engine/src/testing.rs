//! PDF files for the tests of every crate: an invented statement in the known layout, and hostile files.

use std::fmt::Write as _;
use std::io::Write as _;

#[derive(Clone, Debug)]
pub struct Item {
    pub page: usize,
    pub x: f64,
    /// From the top of the page.
    pub y: f64,
    pub text: String,
}

/// Widths of Helvetica from 32 to 126, its real ones: not those the reader supposes.
const HELVETICA: [u32; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556,
    556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833,
    722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556,
    556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334,
    260, 334, 584,
];

/// Pseudo-random numbers that are the same at every run.
pub struct Seeded(pub u64);

impl Seeded {
    pub fn next(&mut self, below: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        if below == 0 { 0 } else { usize::try_from(self.0 % below as u64).unwrap_or(0) }
    }
}

/// Damages a file in a few places: bytes changed, removed, repeated, or replaced by some of `pieces`.
pub fn mutate(r: &mut Seeded, bytes: &[u8], pieces: &[&str]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for _ in 0..=r.next(6) {
        let from = r.next(out.len() + 1);
        let to = (from + r.next(24)).min(out.len());
        let new: Vec<u8> = match r.next(5) {
            0 => vec![u8::try_from(r.next(256)).unwrap_or(0)],
            1 => Vec::new(),
            2 => out.get(from..to).unwrap_or_default().repeat(1 + r.next(20)),
            3 => {
                out.truncate(from);
                continue;
            }
            _ => {
                let n = r.next(pieces.len());
                pieces.get(n).map_or(Vec::new(), |p| p.as_bytes().to_vec())
            }
        };
        out.splice(from..to, new);
    }
    out
}

pub fn file(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut xref = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for (i, o) in objects.iter().enumerate() {
        let _ = writeln!(xref, "{:010} 00000 n ", out.len());
        out.extend(format!("{} 0 obj\n", i + 1).into_bytes());
        out.extend(o);
        out.extend(b"\nendobj\n");
    }
    let at = out.len();
    out.extend(xref.into_bytes());
    out.extend(format!("trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{at}\n%%EOF", objects.len() + 1).into_bytes());
    out
}

pub fn stream(data: &[u8], extra: &str) -> Vec<u8> {
    [format!("<<{extra}/Length {}>>stream\n", data.len()).as_bytes(), data, b"\nendstream"].concat()
}

/// The objects of a font drawn by the PDF, from number 3 on; nothing tells its widths to the reader.
pub fn type3() -> Vec<Vec<u8>> {
    let names = "/a".repeat(95);
    let widths: String = HELVETICA.iter().map(|w| format!("{w} ")).collect();
    vec![
        format!(
            "<</Type/Font/Subtype/Type3/FontBBox[0 0 1000 1000]/FontMatrix[.001 0 0 .001 0 0]/CharProcs<<>>
             /Encoding<</Type/Encoding/Differences[32{names}]>>/FirstChar 32/LastChar 126/Widths[{widths}]
             /ToUnicode 4 0 R>>"
        )
        .into_bytes(),
        stream(
            b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /X def
              1 begincodespacerange <00><FF> endcodespacerange
              1 beginbfrange <20><7E><0020> endbfrange endcmap end end",
            "",
        ),
    ]
}

/// A PDF of A4 pages with these contents (page, operators) and the font `F`, which starts at object 3.
pub fn document_with(contents: &[(usize, String)], font: Vec<Vec<u8>>) -> Vec<u8> {
    let pages = contents.iter().map(|c| c.0).max().unwrap_or(0) + 1;
    let kids: String = (0..pages).map(|p| format!("{} 0 R ", 10 + 2 * p)).collect();
    let mut objects: Vec<Vec<u8>> = vec![
        b"<</Type/Catalog/Pages 2 0 R>>".to_vec(),
        format!("<</Type/Pages/Kids[{kids}]/Count {pages}>>").into_bytes(),
    ];
    objects.extend(font);
    objects.resize(9, b"null".to_vec());
    for p in 0..pages {
        objects.push(
            format!(
                "<</Type/Page/Parent 2 0 R/MediaBox[0 0 595 842]/Contents {} 0 R/Resources<</Font<</F 3 0 R>>>>>>",
                11 + 2 * p
            )
            .into_bytes(),
        );
        let content: String = contents.iter().filter(|c| c.0 == p).map(|c| c.1.as_str()).collect();
        objects.push(stream(content.as_bytes(), ""));
    }
    file(&objects)
}

/// The items as text of this size. The spaces are gaps, as in the real statement.
pub fn document(items: &[Item], font: Vec<Vec<u8>>, size: u32, gap: u32) -> Vec<u8> {
    let contents: Vec<(usize, String)> = items
        .iter()
        .map(|i| {
            let words: Vec<String> = i
                .text
                .split(' ')
                .map(|w| format!("({})", w.replace('\\', "").replace('(', "\\(").replace(')', "\\)")))
                .collect();
            let text = words.join(&format!(" -{gap} "));
            (i.page, format!("BT /F {size} Tf 1 0 0 1 {} {} Tm [{text}] TJ ET\n", i.x, 842.0 - i.y))
        })
        .collect();
    document_with(&contents, font)
}

/// [`statement`] as a file.
pub fn statement_file() -> Vec<u8> {
    document(&statement(), type3(), 8, 278)
}

/// A statement laid out as the real one, with invented contents.
pub fn statement() -> Vec<Item> {
    let mut v: Vec<Item> = Vec::new();
    let mut add = |page, y, words: &[(f64, &str)]| {
        for (x, text) in words {
            v.push(Item { page, x: *x, y, text: text.to_string() });
        }
    };
    add(0, 120.0, &[(40.0, "QUESTO MESE HA SPESO"), (300.0, "QUESTO MESE LE SARANNO ADDEBITATI")]);
    add(0, 140.0, &[(40.0, "Euro 60,10"), (300.0, "Euro 1.163,10")]);
    add(0, 160.0, &[(40.0, "Milano, 1 Dicembre 2026"), (300.0, "In data"), (340.0, "15 Dicembre 2026")]);

    add(1, 60.0, &[(34.0, "RIEPILOGO DEI SUOI MOVIMENTI"), (320.0, "I SUOI PUNTI")]);
    add(1, 80.0, &[(34.0, "Data"), (75.0, "Descrizione"), (230.0, "Importo in Euro"), (320.0, "Saldo punti 1.200")]);
    add(1, 95.0, &[(34.0, "31/10/26"), (75.0, "Debito residuo al mese precedente"), (265.0, "5,00")]);
    add(1, 110.0, &[(75.0, "Totale spese con carte a saldo"), (260.0, "60,10"), (320.0, "Valore 12,00")]);
    add(1, 125.0, &[(75.0, "Totale complessivo finanziamenti per contributi previdenziali")]);
    add(1, 126.0, &[(330.0, "1.100,50")]);
    add(1, 140.0, &[(75.0, "Imposta di bollo"), (265.0, "2,00")]);
    add(1, 155.0, &[(75.0, "TOTALE ADDEBITO SUL SUO C/C"), (253.0, "1.163,10")]);
    add(1, 170.0, &[(75.0, "Debito residuo al 30/11/2026"), (265.0, "4,50")]);
    add(1, 200.0, &[(34.0, "Le ricordiamo che 10,00 punti valgono")]);
    add(1, 300.0, &[(34.0, "CARTA NUMERO ************1234 A SALDO SCADENZA 01/29")]);
    add(1, 320.0, &[(34.0, "DETTAGLIO DEI SUOI MOVIMENTI")]);
    add(
        1,
        338.0,
        &[
            (34.0, "Data"),
            (75.0, "Descrizione"),
            (339.0, "Importo in Euro"),
            (417.0, "Importo in altre valute"),
            (529.0, "Cambio"),
        ],
    );
    add(1, 356.0, &[(34.0, "02/11/26"), (75.0, "BAR DEL MOLO ANCONA ITA"), (370.0, "1,20")]);
    add(1, 370.0, &[(34.0, "02/11/26"), (75.0, "BAR DEL MOLO ANCONA ITA"), (370.0, "1,20")]);
    add(1, 386.0, &[(75.0, "ONLINE SHOP NEW YORK USA")]);
    add(1, 389.5, &[(34.0, "05/11/26"), (366.0, "50,70"), (455.0, "55,00 USD"), (535.0, "1,0848")]);
    add(1, 393.0, &[(75.0, "Commissione di conversione applicata da Nexi: 0,70 Euro*")]);
    add(1, 407.0, &[(75.0, "NEGOZIO CON UN NOME MOLTO LUNGO")]);
    add(1, 410.5, &[(34.0, "09/11/26"), (366.0, "12,00")]);
    add(1, 414.0, &[(75.0, "VIA DELLE ROSE 5 MACERATA")]);
    add(1, 430.0, &[(34.0, "11/11/26"), (75.0, "RIMBORSO NEGOZIO"), (366.0, "-10,00")]);
    add(1, 446.0, &[(34.0, "30/11/26"), (75.0, "EDICOLA CENTRALE"), (370.0, "5,00")]);
    add(1, 462.0, &[(75.0, "TOTALE SPESE"), (366.0, "60,10")]);

    add(2, 60.0, &[(34.0, "CARTA NUMERO ************1234 A SALDO SCADENZA 01/29")]);
    add(2, 80.0, &[(34.0, "DETTAGLIO DEI SUOI MOVIMENTI")]);
    add(
        2,
        98.0,
        &[(34.0, "Data"), (75.0, "Descrizione"), (339.0, "Importo in Euro"), (417.0, "Importo in altre valute")],
    );
    add(2, 115.0, &[(34.0, "*Per rendere confrontabili le commissioni vale 1,00")]);
    add(2, 200.0, &[(34.0, "BOX FINANZIAMENTI PER CONTRIBUTI PREVIDENZIALI")]);
    add(2, 220.0, &[(34.0, "Data"), (75.0, "Descrizione"), (339.0, "Importo in Euro")]);
    for (y, id, amount) in [(238.0, "11", "600,50"), (254.0, "12", "500,00")] {
        let text = format!("Rata finanziamento per contributi previdenziali (Id. finanziamento: {id})");
        add(2, y, &[(34.0, "15/11/26"), (75.0, &text), (362.0, amount)]);
    }
    add(2, 300.0, &[(34.0, "SITUAZIONE CONTABILE SECONDA LINEA DI CREDITO - 9 MESI (11)")]);
    add(2, 320.0, &[(34.0, "Data"), (75.0, "Descrizione"), (339.0, "Importo in Euro")]);
    add(2, 340.0, &[(34.0, "15/11/26"), (75.0, "Importo rata (3/9)(A+B)"), (362.0, "600,50")]);
    add(2, 355.0, &[(34.0, "15/11/26"), (75.0, "Quota interessi (B)"), (370.0, "9,50")]);
    add(2, 400.0, &[(34.0, "CONDIZIONI APPLICATE")]);
    add(2, 420.0, &[(34.0, "15/11/26"), (75.0, "TAN 9,90")]);
    v
}

/// Helvetica, named and not carried, as the web sites of banks print it.
pub fn helvetica() -> Vec<Vec<u8>> {
    vec![b"<</Type/Font/Subtype/Type1/BaseFont/Helvetica/Encoding/WinAnsiEncoding>>".to_vec()]
}

/// [`movements`] as a file.
pub fn movements_file() -> Vec<u8> {
    document(&movements(), helvetica(), 7, 278)
}

/// How wide a text of Helvetica is at the size of [`movements`].
fn helvetica_width(text: &str) -> f64 {
    let units: u32 =
        text.bytes().map(|b| HELVETICA.get(usize::from(b.saturating_sub(32))).copied().unwrap_or(556)).sum();
    f64::from(units) * 7.0 / 1000.0
}

/// A list of an account's movements laid out as the real one of a bank, with invented contents: the
/// newest first, descriptions over several lines that may start above their date, the balances.
pub fn movements() -> Vec<Item> {
    let mut v: Vec<Item> = Vec::new();
    let mut add = |page: usize, y: f64, words: &[(f64, &str)]| {
        for (x, text) in words {
            v.push(Item { page, x: *x, y, text: text.to_string() });
        }
    };
    // Amounts are aligned on the right.
    let right = |text: &str| 295.0 - helvetica_width(text);
    for page in 0..2 {
        add(page, 33.7, &[(333.7, "Banca di Esempio")]);
        add(page, 80.0, &[(346.8, "Movimenti")]);
        add(page, 121.0, &[(31.0, "Conto"), (134.0, "00000 00000 000000000000 conti correnti ROSSI MARIA")]);
        add(page, 133.0, &[(31.0, "IBAN"), (134.0, "IT00X0000000000000000000000")]);
        add(page, 145.0, &[(31.0, "Saldo contabile"), (134.0, "996,00")]);
        add(page, 181.0, &[(31.0, "Periodo"), (134.0, "Ultimi tre mesi")]);
        add(page, 246.3, &[(29.0, "Movimenti")]);
        add(
            page,
            263.5,
            &[(46.7, "Data contabile"), (142.7, "Data valuta"), (265.2, "Importo"), (303.0, "Descrizione")],
        );
        let foot = format!(
            "Stampa da Home Banking per il contratto: ROSSI MARIA in data 26/11/2026 07.21.06 Pagina {} di 2",
            page + 1
        );
        add(page, 818.8, &[(196.2, &foot)]);
    }
    add(0, 278.5, &[(54.0, "26/11/2026"), (right("996,00"), "996,00"), (303.0, "Saldo finale al 26/11/2026")]);
    add(0, 290.3, &[(303.0, "Operazione POS Eurozona Del 21.11.26 18:19 Carta *0000 NEGOZIO")]);
    add(0, 293.5, &[(54.0, "25/11/2026"), (144.0, "21/11/2026"), (right("-9,35"), "-9,35")]);
    add(0, 299.6, &[(303.0, "ESEMPIO ROMA IT")]);
    for y in [311.4, 323.4] {
        let row = [(54.0, "23/11/2026"), (144.0, "23/11/2026"), (right("-2,00"), "-2,00")];
        add(0, y, &[row[0], row[1], row[2], (303.0, "Comm. richiesta incasso SEPA B2C")]);
    }
    add(0, 335.2, &[(303.0, "Bonifico a vs favore *INST 14:14 DITTA ESEMPIO Giroconto Info")]);
    add(0, 338.4, &[(54.0, "20/11/2026"), (144.0, "20/11/2026"), (right("2.000,00"), "2.000,00")]);
    add(0, 344.4, &[(303.0, "aggiuntive:")]);
    add(0, 353.7, &[(303.0, "Ragione sociale ordinante: DITTA ESEMPIO SRL")]);
    add(0, 363.0, &[(303.0, "ID_BONIFICO: 0000")]);
    add(
        0,
        374.8,
        &[
            (54.0, "18/11/2026"),
            (144.0, "18/11/2026"),
            (right("-1.011,65"), "-1.011,65"),
            (303.0, "Utilizzo carta di credito ****000 EMITTENTE"),
        ],
    );
    add(1, 275.3, &[(303.0, "Operazione POS Eurozona Del 25.08.26 10:44 Carta *0000")]);
    add(1, 278.5, &[(54.0, "28/08/2026"), (144.0, "25/08/2026"), (right("-79,00"), "-79,00")]);
    add(1, 284.6, &[(303.0, "OFFICINA ESEMPIO ROMA IT")]);
    add(1, 296.5, &[(54.0, "28/08/2026"), (right("100,00"), "100,00"), (303.0, "Saldo iniziale al 28/08/2026")]);
    v
}

/// Files that a damaged or hostile PDF may be. The first ones would end a process whatever the thread.
pub fn hostile() -> Vec<(&'static str, Vec<u8>)> {
    let cat = b"<</Type/Catalog/Pages 2 0 R>>".to_vec();
    let pages = b"<</Type/Pages/Kids[3 0 R]/Count 1>>".to_vec();
    let page =
        |more: &str| format!("<</Type/Page/Parent 2 0 R/MediaBox[0 0 595 842]/Contents 4 0 R {more}>>").into_bytes();
    let with = |content: Vec<u8>, more: &str| file(&[cat.clone(), pages.clone(), page(more), content]);
    let n = 1_000_000;
    let zipped = |data: &[u8]| {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        let _ = z.write_all(data);
        stream(&z.finish().unwrap_or_default(), "/Filter/FlateDecode")
    };
    let with_text = |content: String| document_with(&[(0, content)], type3());
    let mut chain = vec![cat.clone()];
    chain.extend((0..60_000).map(|i| format!("<</Type/Pages/Kids[{} 0 R]/Count 1>>", i + 3).into_bytes()));
    chain.push(page(""));
    let many: String = (0..5000).map(|_| "3 0 R ").collect();
    vec![
        ("nested arrays", with(stream(b"", ""), &format!("/Foo {}", "[".repeat(n)))),
        ("nested dictionaries", with(stream(b"", ""), &format!("/Foo {}", "<</A".repeat(n)))),
        ("nested content", with(stream("[".repeat(n).as_bytes(), ""), "")),
        ("nested content, compressed", with(zipped("<</A".repeat(n).as_bytes()), "")),
        ("chain of page trees", file(&chain)),
        (
            "a form that draws itself twice",
            file(&[
                cat.clone(),
                pages.clone(),
                page("/Resources<</XObject<</X 5 0 R>>>>"),
                stream(b"/X Do", ""),
                stream(b"/X Do /X Do", "/Type/XObject/Subtype/Form/BBox[0 0 1 1]/Resources<</XObject<</X 5 0 R>>>>"),
            ]),
        ),
        ("page tree in a circle", file(&[cat.clone(), b"<</Type/Pages/Kids[2 0 R]/Count 1>>".to_vec()])),
        (
            "thousands of pages",
            file(&[
                cat.clone(),
                format!("<</Type/Pages/Kids[{many}]/Count 5000>>").into_bytes(),
                page(""),
                stream(b"", ""),
            ]),
        ),
        (
            "a huge count of pages",
            file(&[cat.clone(), b"<</Type/Pages/Kids[3 0 R]/Count 2147483647>>".to_vec(), page(""), stream(b"", "")]),
        ),
        ("dashes", with_text(format!("BT /F 1 Tf 1 Tc 50 700 Td ({}) Tj ET", "-".repeat(150_000)))),
        ("no pages", file(std::slice::from_ref(&cat))),
        ("a scan", with(stream(b"0 0 10 10 re f", ""), "")),
        ("only a heading", b"%PDF-1.7".to_vec()),
        ("garbage", [b"%PDF-1.4\n".to_vec(), (0..20_000_u32).map(|i| ((i * 7919) >> 3) as u8).collect()].concat()),
    ]
}
