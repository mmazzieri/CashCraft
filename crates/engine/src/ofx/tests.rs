use super::*;
use crate::i18n::{Lang, with_lang};

/// An invented statement in SGML: values without an end tag, comma, time zone.
pub fn sgml() -> String {
    "OFXHEADER:100\r\nDATA:OFXSGML\r\nVERSION:102\r\nSECURITY:NONE\r\nENCODING:USASCII\r\nCHARSET:1252\r\n\
     COMPRESSION:NONE\r\nOLDFILEUID:NONE\r\nNEWFILEUID:NONE\r\n\r\n\
     <OFX>\r\n<SIGNONMSGSRSV1><SONRS><STATUS><CODE>0<SEVERITY>INFO</STATUS><DTSERVER>20260131<LANGUAGE>ITA</SONRS></SIGNONMSGSRSV1>\r\n\
     <BANKMSGSRSV1><STMTTRNRS><TRNUID>1<STATUS><CODE>0<SEVERITY>INFO</STATUS>\r\n<STMTRS>\r\n<CURDEF>EUR\r\n\
     <BANKACCTFROM><BANKID>00000<ACCTID>000012345678<ACCTTYPE>CHECKING</BANKACCTFROM>\r\n\
     <BANKTRANLIST><DTSTART>20260101<DTEND>20260131\r\n\
     <STMTTRN>\r\n<TRNTYPE>DEBIT\r\n<DTPOSTED>20260105235900.000[+1:CET]\r\n<TRNAMT>-12,50\r\n<FITID>A1\r\n<NAME>BAR &amp; CAFFE\r\n<MEMO>Operazione < 20\r\n</STMTTRN>\r\n\
     <STMTTRN><TRNTYPE>CREDIT<DTPOSTED>20260127<TRNAMT>+1500.00<FITID>A2<CHECKNUM><NAME><MEMO>Bonifico a favore di Anna Prova</STMTTRN>\r\n\
     </BANKTRANLIST>\r\n<LEDGERBAL><BALAMT>1487.50<DTASOF>20260131120000[0:GMT]</LEDGERBAL>\r\n\
     <AVAILBAL><BALAMT>99.00<DTASOF>20260131</AVAILBAL>\r\n</STMTRS></STMTTRNRS></BANKMSGSRSV1></OFX>\r\n"
        .into()
}

/// An invented file in XML: a bank account and a credit card.
pub fn xml() -> String {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<?OFX OFXHEADER="200" VERSION="211" SECURITY="NONE" OLDFILEUID="NONE" NEWFILEUID="NONE"?>
<!-- invented -->
<OFX>
 <BANKMSGSRSV1><STMTTRNRS><STMTRS><CURDEF>EUR</CURDEF>
  <BANKACCTFROM><BANKID>1</BANKID><ACCTID>IT00X1111</ACCTID><ACCTTYPE>SAVINGS</ACCTTYPE></BANKACCTFROM>
  <BANKTRANLIST><DTSTART>20260101</DTSTART><DTEND>20260131</DTEND>
   <STMTTRN><TRNTYPE>INT</TRNTYPE><DTPOSTED>20260110</DTPOSTED><TRNAMT>0.10</TRNAMT><FITID>1</FITID><NAME>Perch&#233; &lt;no&gt;</NAME></STMTTRN>
  </BANKTRANLIST>
 </STMTRS></STMTTRNRS></BANKMSGSRSV1>
 <CREDITCARDMSGSRSV1><CCSTMTTRNRS><CCSTMTRS><CURDEF>usd</CURDEF>
  <CCACCTFROM><ACCTID>0000111122223333</ACCTID></CCACCTFROM>
  <BANKTRANLIST>
   <STMTTRN><TRNTYPE>POS</TRNTYPE><DTPOSTED>20260111</DTPOSTED><TRNAMT>-7.00</TRNAMT><FITID>9</FITID>
    <PAYEE><NAME>Shop</NAME><ADDR1>Via Prova</ADDR1></PAYEE><MEMO><![CDATA[a & <b>]]></MEMO>
    <CCACCTTO><ACCTID>x</ACCTID></CCACCTTO><ORIGCURRENCY><CURRATE>1.1</CURRATE><CURSYM>EUR</CURSYM></ORIGCURRENCY></STMTTRN>
  </BANKTRANLIST>
  <LEDGERBAL><BALAMT>-7</BALAMT><DTASOF>20260131</DTASOF></LEDGERBAL>
 </CCSTMTRS></CCSTMTTRNRS></CREDITCARDMSGSRSV1>
</OFX>
"#
    .into()
}

fn error(text: &str) -> String {
    with_lang(Lang::En, || read(text.as_bytes()).unwrap_err().to_string())
}

#[test]
fn sgml_statements_are_read() {
    let text = sgml();
    assert!(is_ofx(text.as_bytes()));
    let (o, encoding) = read(text.as_bytes()).unwrap();
    assert_eq!((encoding, o.chosen, o.statements.len()), ("windows-1252", Some(0), 1));
    let s = o.statement().unwrap();
    assert_eq!((s.kind, s.account.as_str(), s.currency.as_str()), (Kind::Bank, "000012345678", "EUR"));
    assert_eq!(s.account_label(), "…5678");
    assert_eq!(s.balance, Some((148_750, date("20260131"))));
    let rows: Vec<_> =
        s.rows.iter().map(|r| (r.line, r.date.to_string(), r.amount, r.name.as_str(), r.memo.as_str())).collect();
    assert_eq!(
        rows,
        [
            (18, "2026-01-05".to_string(), -1250, "BAR & CAFFE", "Operazione < 20"),
            (26, "2026-01-27".to_string(), 150_000, "", "Bonifico a favore di Anna Prova"),
        ]
    );
    assert_eq!(s.rows[0].fitid.as_deref(), Some("A1"));
    // All on one line, as some banks do.
    let one = text.replace("\r\n", "");
    assert_eq!(read(one.as_bytes()).unwrap().0.statements[0].rows.len(), 2);
    // The end tags that SGML lets out may be there.
    let closed = text.replace("<FITID>A2", "<FITID>A2</FITID>").replace("<CHECKNUM>", "<CHECKNUM></CHECKNUM>");
    assert_eq!(read(closed.as_bytes()).unwrap().0.statements[0].rows, o.statements[0].rows);
    assert_eq!(read(text.to_lowercase().as_bytes()).unwrap().0.statements[0].rows.len(), 2);
}

#[test]
fn xml_files_with_several_statements_are_read() {
    let (mut o, _) = read(xml().as_bytes()).unwrap();
    assert_eq!((o.chosen, o.statements.len(), o.statement()), (None, 2, None));
    let (a, b) = (&o.statements[0], &o.statements[1]);
    assert_eq!(
        (a.kind, a.rows[0].amount, a.rows[0].name.as_str(), a.balance),
        (Kind::Savings, 10, "Perché <no>", None)
    );
    assert_eq!((b.kind, b.currency.as_str()), (Kind::CreditCard, "USD"));
    assert_eq!((b.rows[0].amount, b.rows[0].name.as_str(), b.rows[0].memo.as_str()), (-700, "Shop", "a & <b>"));
    for (key, place) in [("1", 0), ("2", 1), ("IT00X1111", 0), ("0000111122223333", 1)] {
        o.choose(key).unwrap();
        assert_eq!(o.chosen, Some(place));
    }
    for key in ["0", "3", "", "-1", "1111", "99999999999999999999999"] {
        assert!(o.choose(key).is_err(), "{key}");
    }
    // The same account twice is told by its place alone.
    o.statements[1].account = "IT00X1111".into();
    assert!(o.choose("IT00X1111").is_err());
    let label = o.list();
    assert!(label.contains("…1111") && !label.contains("IT00X1111"));
}

#[test]
fn character_sets() {
    let name = |bytes: &[u8]| read(bytes).unwrap().0.statements[0].rows[0].name.clone();
    let with = |encoding: &str, charset: &str| {
        sgml()
            .replace("ENCODING:USASCII", &format!("ENCODING:{encoding}"))
            .replace("CHARSET:1252", &format!("CHARSET:{charset}"))
            .replace("&amp; CAFFE", "à€")
    };
    let latin = |text: &str| encoding_rs::WINDOWS_1252.encode(text).0.into_owned();
    for (encoding, charset) in
        [("USASCII", "1252"), ("USASCII", "ISO-8859-1"), ("USASCII", "8859-1"), ("USASCII", "NONE")]
    {
        assert_eq!(name(&latin(&with(encoding, charset))), "BAR à€", "{encoding} {charset}");
        assert_eq!(name(&latin(&with(encoding, charset).replace("\r\n", ""))), "BAR à€", "{charset} on one line");
    }
    for (encoding, charset) in
        [("UTF-8", "NONE"), ("UNICODE", "NONE"), ("UTF-8", "CSUNICODE"), ("USASCII", "NONE"), ("USASCII", "1252")]
    {
        assert_eq!(name(with(encoding, charset).as_bytes()), "BAR à€", "{encoding} {charset}");
    }
    assert_eq!(name(&[b"\xEF\xBB\xBF", with("USASCII", "1252").as_bytes()].concat()), "BAR à€");
    // Declared UTF-8, and not.
    assert_eq!(name(&latin(&with("UTF-8", "NONE"))), "BAR à€");
    let utf16: Vec<u8> =
        [0xFEFF].into_iter().chain(with("UNICODE", "NONE").encode_utf16()).flat_map(u16::to_le_bytes).collect();
    assert!(is_ofx(&utf16));
    assert_eq!(name(&utf16), "BAR à€");
    let xml = xml().replace("UTF-8", "ISO-8859-15").replace("Perch&#233;", "è€");
    let bytes = encoding_rs::ISO_8859_15.encode(&xml).0.into_owned();
    assert_eq!(name(&bytes), "è€ <no>");
    for other in ["", "Data;Importo\nOFXHEADER;1", "<html>", "<?xml version=\"1.0\"?><a/>"] {
        assert!(!is_ofx(other.as_bytes()), "{other}");
    }
}

#[test]
fn amounts_are_exact() {
    for (text, cents) in [
        ("-12.50", -1250),
        ("12,5", 1250),
        ("+7", 700),
        (".05", 5),
        ("0012.3400", 1234),
        ("-0", 0),
        ("900719925474.09", 90_071_992_547_409),
        ("1000000000000", MAX_AMOUNT),
    ] {
        assert_eq!(amount(text).unwrap_or(-1), cents, "{text}");
    }
    for bad in [
        "",
        "-",
        ".",
        "1.234,56",
        "1,234",
        "12.345",
        "1 000",
        "1e3",
        "--1",
        "€1",
        "１",
        "1000000000000.01",
        "-1.500",
        "2,000",
        "0.500",
        "1234.000",
        "1.2.3",
        "1,2.3",
        "99999999999999999999999999",
        "NaN",
    ] {
        assert!(amount(bad).is_err(), "{bad}");
    }
}

#[test]
fn dates_with_time_zones() {
    for text in [
        "20260105",
        "2026010523",
        "202601052359",
        "20260105235959",
        "20260105235959.123",
        "20260105235959[-5:EST]",
        "20260105000000.000[+05.30:IST]",
        "20260105120000[0:GMT]",
        "20260105[+1]",
        "20260105 [CET]",
        "20260105120000[-0500:EST]",
        "20260105[+0530]",
        "20260105120000[GMT+1]",
        "20260105120000.000[GMT-03.30:X]",
    ] {
        assert_eq!(date(text).map(|d| d.to_string()).as_deref(), Some("2026-01-05"), "{text}");
    }
    for bad in [
        "",
        "2026",
        "20261301",
        "20260230",
        "2026-01-05",
        "20260105250000",
        "20260105120000[",
        "20260105[99:X]",
        "20260105[]",
        "20260105[+9999]",
        "20260105[GMT+]",
        "20260105[1x]",
        "00010101",
        "99991231",
        "2026010５",
        "20260105120000[+1:CET",
        "202601051",
    ] {
        assert_eq!(date(bad), None, "{bad}");
    }
}

#[test]
fn what_is_wrong_is_told_with_its_line() {
    let ok = sgml();
    for (from, to, expected) in [
        ("-12,50", "-12,505", "line 21: \"-12,505\" has more than two decimals"),
        ("-12,50", "", "line 18: TRNAMT is missing"),
        ("20260105235900.000[+1:CET]", "20261305", "line 20: \"20261305\" is not a valid OFX date"),
        ("<DTPOSTED>20260127", "", "line 26: DTPOSTED is missing"),
        ("<FITID>A1", "<TRNAMT>-5", "line 22: TRNAMT is there twice"),
        ("</STMTTRN>\r\n<STMTTRN><TRNTYPE>CREDIT", "<STMTTRN><TRNTYPE>CREDIT", "line 25: a transaction is outside"),
        ("</STMTTRN>\r\n</BANKTRANLIST>", "</BANKTRANLIST>", "line 26: BANKTRANLIST ends here, and STMTTRN of line 26"),
        ("</STMTTRN>\r\n</BANKTRANLIST>", "", "line 29: STMTRS ends here, and STMTTRN of line 26"),
        ("<CURDEF>EUR", "", "line 14: CURDEF is missing"),
        ("<CURDEF>EUR", "<CURDEF>euro", "line 15: \"EURO\" is not a currency code"),
        ("<ACCTID>000012345678", "", "line 14: BANKACCTFROM.ACCTID is missing"),
        ("<FITID>A1", "<CURRENCY><CURRATE>1.1<CURSYM>USD</CURRENCY>", "line 22: the transaction is in USD"),
        ("</OFX>", "", "line 11: OFX never ends"),
        ("</OFX>", "</OFX><OFX></OFX>", "line 30: there is something after"),
        ("<OFX>", "<!DOCTYPE x [<!ENTITY a \"b\">]><OFX>", "line 11: declarations are not accepted"),
        ("<FITID>A1", "<FITID", "line 22: \">\" is missing"),
        ("<FITID>A1", "<![CDATA[", "line 22: \"]]>\" is missing"),
    ] {
        let bad = ok.replacen(from, to, 1);
        assert_ne!(bad, ok, "{from}");
        let e = error(&bad);
        assert!(e.starts_with(expected), "{from}: {e}");
    }
    // Cut anywhere, it is an error and never half a statement.
    for n in (0..ok.trim_end().len()).filter(|n| ok.is_char_boundary(*n)) {
        assert!(read(&ok.as_bytes()[..n]).is_err(), "{n}");
    }
    assert!(error("<OFX></OFX>").contains("no statement"));
    // A balance that cannot be read is only a check that is not there.
    let s = read(ok.replace("1487.50", "x").as_bytes()).unwrap().0;
    assert_eq!(s.statements[0].balance, None);
}

#[test]
fn a_root_with_attributes_and_tags_that_are_text() {
    let xml = xml().replace("<OFX>", "<OFX xmlns=\"urn:x\" a='b'>");
    assert!(is_ofx(xml.as_bytes()));
    assert_eq!(read(xml.as_bytes()).unwrap().0.statements.len(), 2);
    let bare = xml.split_once("-->").unwrap().1.trim_start();
    assert!(is_ofx(bare.as_bytes()) && is_ofx(format!("<?xml version=\"1.0\"?>\n{bare}").as_bytes()));
    assert!(!is_ofx(b"<OFXX>"));
    for (text, kept) in [
        ("A <B C> D", "A <B C> D"),
        ("A<B", "A<B"),
        ("A<B & C>D", "A<B & C>D"),
        ("1 < 2 > 1", "1 < 2 > 1"),
        ("x </b c> y", "x </b c> y"),
        ("x <a href=\"y\">z", "x"),
    ] {
        for end in ["", "</MEMO>"] {
            let file = sgml().replace("Operazione < 20", &format!("{text}{end}"));
            let s = read(file.as_bytes()).unwrap().0.statements.remove(0);
            assert_eq!((s.rows[0].memo.as_str(), s.rows.len()), (kept, 2), "{text}{end}");
        }
    }
}

#[test]
fn corrections_and_types() {
    let with = |x: &str| read(sgml().replace("<FITID>A1", x).as_bytes()).unwrap().0.statements.remove(0).rows.remove(0);
    let c = |delete| Some(Correction { fitid: "Z".into(), delete });
    assert_eq!(with("<FITID>B<CORRECTFITID>Z<CORRECTACTION>DELETE").corrects, c(true));
    assert_eq!(with("<CORRECTFITID>Z<CORRECTACTION>REPLACE").corrects, c(false));
    assert_eq!(with("<CORRECTFITID>Z").corrects, c(false));
    assert_eq!((with("<FITID>B").corrects, with("<FITID>B").goes_out), (None, Some(true)));
}

#[test]
fn nothing_is_expanded_but_the_standard_entities() {
    assert_eq!(unescape("a&amp;lt;b&#8364;&#x41;&nbsp;&apos;&quot;"), "a&lt;b€A '\"");
    for same in ["&", "&;", "&x;", "&#;", "&#xD800;", "&#99999999999;", "AT&T", "&amp", "&lol9;", "&è;"] {
        assert_eq!(unescape(same), same);
    }
}

#[test]
fn large_files_are_refused_or_read_quickly() {
    let start = std::time::Instant::now();
    let (head, tail) = (
        "<OFX><STMTRS><CURDEF>EUR<BANKACCTFROM><ACCTID>1</BANKACCTFROM><BANKTRANLIST>",
        "</BANKTRANLIST></STMTRS></OFX>",
    );
    let one = "<STMTTRN><DTPOSTED>20260101<TRNAMT>1</STMTTRN>";
    let file = |n: usize| format!("{head}{}{tail}", one.repeat(n));
    assert_eq!(read(file(MAX_TRANSACTIONS).as_bytes()).unwrap().0.statements[0].rows.len(), MAX_TRANSACTIONS);
    assert!(error(&file(MAX_TRANSACTIONS + 1)).contains("too many transactions"));
    let statement = "<STMTRS><CURDEF>EUR<BANKACCTFROM><ACCTID>1</BANKACCTFROM></STMTRS>";
    assert!(error(&format!("<OFX>{}</OFX>", statement.repeat(101))).contains("too many statements"));
    assert!(error(&format!("<OFX>{}", "<PAYEE>".repeat(1_000_000))).contains("nested too deep"));
    for s in [
        "<".repeat(2_000_000),
        "<a".repeat(1_000_000),
        "<A>".repeat(1_000_000),
        "&".repeat(2_000_000),
        "<!--".repeat(500_000),
    ] {
        assert!(read(format!("{head}{s}").as_bytes()).is_err());
    }
    // A long name, and a value in many pieces.
    let pieces = format!(
        "<STMTTRN><{}>{}<DTPOSTED>20260101<TRNAMT>1</STMTTRN>",
        "A".repeat(1_000_000),
        "x<!---->".repeat(200_000)
    );
    assert_eq!(read(format!("{head}{pieces}{tail}").as_bytes()).unwrap().0.statements[0].rows.len(), 1);
    let memo = format!("{head}<STMTTRN><DTPOSTED>20260101<TRNAMT>1<MEMO>{}</STMTTRN>{tail}", "è".repeat(100_000));
    assert_eq!(read(memo.as_bytes()).unwrap().0.statements[0].rows[0].memo.chars().count(), MAX_TEXT);
    let long = error(&format!("{head}<STMTTRN><DTPOSTED>20260101<TRNAMT>{}</STMTTRN>{tail}", "9".repeat(100_000)));
    assert!(long.len() < 200);
    assert!(start.elapsed().as_secs() < 30);
}
