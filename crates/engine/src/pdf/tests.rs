use super::*;
use crate::testing::{self, Item, type3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Font {
    /// Embedded, with the widths of its characters.
    TrueType,
    /// Drawn by the PDF; nothing tells the widths to the reader.
    Type3,
    /// Only a name.
    NotEmbedded,
}

/// A PDF of A4 pages with text.
pub fn document(font: Font, items: &[Item]) -> Vec<u8> {
    match font {
        Font::Type3 => testing::document(items, type3(), 8, 278),
        Font::NotEmbedded => {
            let f = b"<</Type/Font/Subtype/Type1/BaseFont/Helvetica/Encoding/WinAnsiEncoding>>".to_vec();
            testing::document(items, vec![f], 8, 278)
        }
        // The font is wide.
        Font::TrueType => {
            let font = format!(
                "<</Type/Font/Subtype/TrueType/BaseFont/AAAAAA+Hack/Encoding/WinAnsiEncoding/FirstChar 32
                 /LastChar 126/Widths[{}]/FontDescriptor 4 0 R>>",
                "602 ".repeat(95)
            );
            let descriptor = b"<</Type/FontDescriptor/FontName/AAAAAA+Hack/Flags 32/FontFile2 5 0 R>>".to_vec();
            let data = testing::stream(epaint_default_fonts::HACK_REGULAR, "");
            testing::document(items, vec![font.into_bytes(), descriptor, data], 6, 300)
        }
    }
}

/// The lines of a file, read in a thread with enough stack.
pub fn read_pages(bytes: &[u8]) -> std::result::Result<Vec<Page>, Failure> {
    let bytes = bytes.to_vec();
    let thread = std::thread::Builder::new().stack_size(STACK).spawn(move || pages(bytes)).unwrap();
    thread.join().unwrap()
}

fn item(page: usize, x: f64, y: f64, text: &str) -> Item {
    Item { page, x, y, text: text.into() }
}

fn texts(line: &Line) -> Vec<&str> {
    line.words.iter().map(|w| w.text.as_str()).collect()
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
        let pages = read_pages(&document(font, &items)).unwrap();
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
    let why = |bytes: &[u8]| read_pages(bytes).unwrap_err();
    // A font that is named and not carried is read all the same.
    let named = read_pages(&document(Font::NotEmbedded, &text)).unwrap();
    assert_eq!(texts(&named[0][0]), ["Text"]);
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
            Failure::Unknown,
            Failure::Mismatch,
        ] {
            assert!(!f.message().starts_with("the "), "{f:?}");
        }
    });
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

/// A long line of marks is as quick to search as any other.
#[test]
fn a_long_line_of_marks_takes_no_longer() {
    let words =
        |n: usize| -> Vec<Word> { (0..n).map(|i| Word { x0: i as f64, x1: i as f64, text: "-".into() }).collect() };
    let page: Page = vec![Line { y: 1.0, words: words(150_000) }];
    let start = Instant::now();
    assert_eq!(card_pdf::parse(&[page]), Err(Failure::Unknown));
    assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
}
