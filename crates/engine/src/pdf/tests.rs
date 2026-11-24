use super::*;
use std::fmt::Write as _;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Font {
    /// Embedded, with the widths of its characters.
    TrueType,
    /// Drawn by the PDF; nothing tells the widths to the reader.
    Type3,
    /// Only a name.
    NotEmbedded,
}

/// Widths of Helvetica from 32 to 126, its real ones: not those the reader supposes.
const HELVETICA: [u32; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556,
    556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833,
    722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556,
    556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334,
    260, 334, 584,
];

#[derive(Clone, Debug)]
pub struct Item {
    pub page: usize,
    pub x: f64,
    /// From the top of the page.
    pub y: f64,
    pub text: String,
}

pub fn file(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut xref = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for (i, o) in objects.iter().enumerate() {
        writeln!(xref, "{:010} 00000 n ", out.len()).unwrap();
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

/// A PDF of A4 pages with text. The spaces are gaps, as in the real statement.
pub fn document(font: Font, items: &[Item]) -> Vec<u8> {
    let pages = items.iter().map(|i| i.page).max().unwrap_or(0) + 1;
    let kids: String = (0..pages).map(|p| format!("{} 0 R ", 10 + 2 * p)).collect();
    let mut objects: Vec<Vec<u8>> = vec![
        b"<</Type/Catalog/Pages 2 0 R>>".to_vec(),
        format!("<</Type/Pages/Kids[{kids}]/Count {pages}>>").into_bytes(),
    ];
    let widths = |w: &[u32]| w.iter().map(|w| format!("{w} ")).collect::<String>();
    match font {
        Font::TrueType => {
            let hack = epaint_default_fonts::HACK_REGULAR;
            objects.push(
                format!(
                    "<</Type/Font/Subtype/TrueType/BaseFont/AAAAAA+Hack/Encoding/WinAnsiEncoding/FirstChar 32
                     /LastChar 126/Widths[{}]/FontDescriptor 4 0 R>>",
                    widths(&[602; 95])
                )
                .into_bytes(),
            );
            objects.push(b"<</Type/FontDescriptor/FontName/AAAAAA+Hack/Flags 32/FontFile2 5 0 R>>".to_vec());
            objects.push(stream(hack, ""));
        }
        Font::Type3 => {
            let names: String = (0..95).map(|_| "/a").collect();
            objects.push(
                format!(
                    "<</Type/Font/Subtype/Type3/FontBBox[0 0 1000 1000]/FontMatrix[.001 0 0 .001 0 0]/CharProcs<<>>
                     /Encoding<</Type/Encoding/Differences[32{names}]>>/FirstChar 32/LastChar 126/Widths[{}]
                     /ToUnicode 4 0 R>>",
                    widths(&HELVETICA)
                )
                .into_bytes(),
            );
            objects.push(stream(
                b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /X def
                  1 begincodespacerange <00><FF> endcodespacerange
                  1 beginbfrange <20><7E><0020> endbfrange endcmap end end",
                "",
            ));
        }
        Font::NotEmbedded => {
            objects.push(b"<</Type/Font/Subtype/Type1/BaseFont/Helvetica/Encoding/WinAnsiEncoding>>".to_vec());
        }
    }
    objects.resize(9, b"null".to_vec());
    for p in 0..pages {
        let mut content = String::new();
        for i in items.iter().filter(|i| i.page == p) {
            // The gap of a space of Helvetica.
            let text: Vec<String> = i
                .text
                .split(' ')
                .map(|w| format!("({})", w.replace('\\', "").replace('(', "\\(").replace(')', "\\)")))
                .collect();
            // The embedded font is wide.
            let (gap, size) = if font == Font::TrueType { (" -300 ", 6) } else { (" -278 ", 8) };
            writeln!(content, "BT /F {size} Tf 1 0 0 1 {} {} Tm [{}] TJ ET", i.x, 842.0 - i.y, text.join(gap)).unwrap();
        }
        objects.push(
            format!(
                "<</Type/Page/Parent 2 0 R/MediaBox[0 0 595 842]/Contents {} 0 R/Resources<</Font<</F 3 0 R>>>>>>",
                11 + 2 * p
            )
            .into_bytes(),
        );
        objects.push(stream(content.as_bytes(), ""));
    }
    file(&objects)
}

fn item(page: usize, x: f64, y: f64, text: &str) -> Item {
    Item { page, x, y, text: text.into() }
}

fn texts(line: &Line) -> Vec<&str> {
    line.words.iter().map(|w| w.text.as_str()).collect()
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
        z.write_all(data).unwrap();
        stream(&z.finish().unwrap(), "/Filter/FlateDecode")
    };
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
        ("no pages", file(std::slice::from_ref(&cat))),
        ("a scan", with(stream(b"0 0 10 10 re f", ""), "")),
        ("only a heading", b"%PDF-1.7".to_vec()),
        ("garbage", [b"%PDF-1.4\n".to_vec(), (0..20_000_u32).map(|i| ((i * 7919) >> 3) as u8).collect()].concat()),
    ]
}

#[test]
fn words_are_separated_by_gaps() {
    let items = [
        item(0, 34.0, 100.0, "01/11/26"),
        item(0, 75.0, 100.0, "BAR DEL MOLO ANCONA ITA"),
        item(0, 370.0, 100.5, "1.234,56"),
        item(0, 75.0, 96.0, "Illimitata Wwmm ill fit. A B C"),
        item(1, 34.0, 50.0, "Second page"),
    ];
    for font in [Font::TrueType, Font::Type3] {
        let pages = pages(&document(font, &items)).unwrap();
        assert_eq!(pages.len(), 2);
        let got: Vec<Vec<&str>> = pages[0].iter().map(texts).collect();
        let expected = [
            vec!["Illimitata", "Wwmm", "ill", "fit.", "A", "B", "C"],
            vec!["01/11/26", "BAR", "DEL", "MOLO", "ANCONA", "ITA", "1.234,56"],
        ];
        assert_eq!(got, expected, "{font:?}");
        assert!((pages[0][1].y - 100.0).abs() < 0.1 && (pages[0][1].words[1].x0 - 75.0).abs() < 0.1);
        assert_eq!(texts(&pages[1][0]), ["Second", "page"]);
    }
}

#[test]
fn spaced_letters_are_one_word_and_text_drawn_twice_is_read_once() {
    let drawn = |x: f64, c: &str| Drawn { x0: x, x1: x + 5.0, y: 0.0, size: 10.0, text: c.into(), guessed: false };
    // 0.8 points between the letters, 3 between the words.
    let line: Vec<Drawn> = [(0.0, "C"), (5.8, "A"), (11.6, "R"), (19.6, "D"), (25.4, "S"), (31.2, "X")]
        .into_iter()
        .map(|(x, c)| drawn(x, c))
        .collect();
    let got: Vec<String> = words(&line).into_iter().map(|w| w.text).collect();
    assert_eq!(got, ["CAR", "DSX"]);
    let page = lines(vec![drawn(0.0, "A"), drawn(0.3, "A"), drawn(5.0, "B"), drawn(5.0, "B")]);
    assert_eq!(texts(&page[0]), ["AB"]);
}

#[test]
fn files_that_cannot_be_read_say_why() {
    let text = [item(0, 34.0, 100.0, "Text")];
    let why = |bytes: &[u8]| in_thread(bytes.to_vec()).unwrap_err();
    assert_eq!(why(&document(Font::NotEmbedded, &text)), Failure::NoText);
    let good = document(Font::Type3, &text);
    let many: Vec<Item> = (0..=MAX_PAGES).map(|p| item(p, 10.0, 10.0, "x")).collect();
    assert_eq!(why(&document(Font::Type3, &many)), Failure::TooLarge);
    let long = [item(0, 0.0, 100.0, &"x".repeat(MAX_GLYPHS + 1))];
    assert_eq!(why(&document(Font::Type3, &long)), Failure::TooLarge);
    // A password, which this file does not really have.
    let locked = String::from_utf8(good).unwrap().replace(
        "trailer\n<<",
        &format!("trailer\n<</ID[<00><00>]/Encrypt<</Filter/Standard/V 1/R 2/P -4/O<{0}>/U<{0}>>>", "00".repeat(32)),
    );
    assert_eq!(why(locked.as_bytes()), Failure::Encrypted);
    crate::i18n::with_lang(crate::i18n::Lang::It, || {
        for f in [
            Failure::Damaged,
            Failure::Encrypted,
            Failure::NoText,
            Failure::TooLarge,
            Failure::TimeOut,
            Failure::NoProcess,
        ] {
            assert!(!f.message().starts_with("the "), "{f:?}");
        }
    });
}

/// The process that reads; run by the next test, with the PDF on its standard input.
#[test]
#[ignore = "run by hostile_files_end_the_other_process"]
fn child() {
    let mut bytes = Vec::new();
    std::io::stdin().lock().read_to_end(&mut bytes).unwrap();
    let outcome = in_thread(bytes);
    println!("outcome {}", serde_json::to_string(&outcome).unwrap());
}

/// What overflows the stack or never ends stops the other process, not this one.
#[test]
fn hostile_files_end_the_other_process() {
    let limit = Duration::from_secs(2);
    for (name, bytes) in hostile() {
        let start = Instant::now();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "pdf::tests::child", "--ignored", "--nocapture"]);
        // Its answer comes among the lines of the test: never an answer, always a failure.
        let got = in_process(&mut command, &bytes, limit);
        assert!(matches!(got, Err(Failure::Damaged | Failure::TimeOut)), "{name}: {got:?}");
        assert!(start.elapsed() < limit * 4, "{name}");
    }
}

#[test]
fn damaged_files_never_panic() {
    let good = document(Font::Type3, &[item(0, 34.0, 100.0, "Some text 1,00"), item(1, 34.0, 100.0, "More")]);
    for n in 0..good.len() {
        if n % 13 == 0 {
            let _ = in_thread(good[..n].to_vec());
        }
        let mut bad = good.clone();
        bad[n] ^= 1 << (n % 8);
        if n % 5 == 0 {
            let _ = in_thread(bad);
        }
    }
}
