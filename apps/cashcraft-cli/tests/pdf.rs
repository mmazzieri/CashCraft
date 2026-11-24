//! The program reads PDF files in another process of its own: a file that ends that process is an error.

use std::path::{Path, PathBuf};
use std::process::Command;

fn pdf(objects: &[String]) -> Vec<u8> {
    let mut out = String::from("%PDF-1.4\n");
    let mut xref = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for (i, o) in objects.iter().enumerate() {
        xref.push_str(&format!("{:010} 00000 n \n", out.len()));
        out.push_str(&format!("{} 0 obj\n{o}\nendobj\n", i + 1));
    }
    let at = out.len();
    out.push_str(&format!("{xref}trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{at}\n%%EOF", objects.len() + 1));
    out.into_bytes()
}

fn page(more: &str, content: &str) -> Vec<u8> {
    let to_unicode = "begincmap 1 begincodespacerange <00><FF> endcodespacerange \
                      1 beginbfrange <20><7E><0020> endbfrange endcmap";
    let stream = |s: &str| format!("<</Length {}>>stream\n{s}\nendstream", s.len());
    pdf(&[
        "<</Type/Catalog/Pages 2 0 R>>".into(),
        "<</Type/Pages/Kids[3 0 R]/Count 1>>".into(),
        format!(
            "<</Type/Page/Parent 2 0 R/MediaBox[0 0 595 842]/Contents 4 0 R/Resources<</Font<</F 5 0 R>>>>{more}>>"
        ),
        stream(content),
        format!(
            "<</Type/Font/Subtype/Type3/FontBBox[0 0 9 9]/FontMatrix[.001 0 0 .001 0 0]/CharProcs<<>>\
             /Encoding<</Differences[32{}]>>/FirstChar 32/LastChar 126/Widths[{}]/ToUnicode 6 0 R>>",
            "/a".repeat(95),
            "500 ".repeat(95)
        ),
        stream(to_unicode),
    ])
}

fn preview(dir: &Path, bytes: &[u8]) -> std::io::Result<(Option<i32>, String)> {
    let file: PathBuf = dir.join("file.pdf");
    std::fs::write(&file, bytes)?;
    let out = Command::new(env!("CARGO_BIN_EXE_cashcraft-cli"))
        .env("CASHCRAFT_LANG", "en")
        .env("CASHCRAFT_DB", dir.join("books.db"))
        .arg("preview")
        .arg(&file)
        .output()?;
    Ok((out.status.code(), String::from_utf8_lossy(&out.stderr).into_owned()))
}

#[test]
fn pdf_files_are_read_by_another_process() {
    let dir = std::env::temp_dir().join(format!("cashcraft-cli-pdf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Read, but not a statement.
    let (code, error) = preview(&dir, &page("", "BT /F 10 Tf 50 700 Td (Hello) Tj ET")).unwrap();
    assert_eq!(code, Some(1), "{error}");
    assert!(error.contains("not a card statement"), "{error}");
    // Each of these overflows the stack of the process that reads.
    let deep = "[".repeat(1_000_000);
    for bytes in [page(&format!("/Foo {deep}"), ""), page("", &deep)] {
        let (code, error) = preview(&dir, &bytes).unwrap();
        assert_eq!(code, Some(1), "{error}");
        assert!(error.contains("damaged"), "{error}");
    }
    let (code, error) = preview(&dir, b"%PDF-1.4 nothing").unwrap();
    assert!(code == Some(1) && error.contains("damaged"), "{error}");
    std::fs::remove_dir_all(&dir).unwrap();
}
