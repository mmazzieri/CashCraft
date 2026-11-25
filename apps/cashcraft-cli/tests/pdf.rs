//! The program reads PDF files in another process of its own: a file that ends that process is an error.

use cashcraft_engine::pdf::{Failure, Limits, statement_with};
use cashcraft_engine::testing::{hostile, statement_file};
use std::path::Path;
use std::time::{Duration, Instant};

const PROGRAM: &str = env!("CARGO_BIN_EXE_cashcraft-cli");

#[test]
fn a_statement_is_read_by_the_other_process() {
    let s = statement_with(Path::new(PROGRAM), &statement_file(), Limits::default()).unwrap();
    assert!(s.is_sound());
    assert_eq!(s.rows.len(), 9);
}

#[test]
fn hostile_files_end_the_other_process_only() {
    for (name, bytes) in hostile() {
        let limits = Limits { time: Duration::from_secs(4), ..Default::default() };
        let start = Instant::now();
        let got = statement_with(Path::new(PROGRAM), &bytes, limits);
        let expected: &[Failure] = match name {
            // The stack of the process that reads.
            "nested arrays"
            | "nested dictionaries"
            | "nested content"
            | "nested content, compressed"
            | "no pages"
            | "only a heading"
            | "garbage" => &[Failure::Damaged],
            "chain of page trees" => &[Failure::Damaged, Failure::TimeOut],
            "a form that draws itself twice" => &[Failure::TimeOut],
            "thousands of pages" => &[Failure::TooLarge],
            "page tree in a circle" | "a huge count of pages" | "a scan" => &[Failure::NoText],
            _ => &[Failure::Unknown, Failure::TimeOut],
        };
        assert!(got.as_ref().is_err_and(|e| expected.contains(e)), "{name}: {got:?}");
        assert!(start.elapsed() < limits.time + Duration::from_secs(4), "{name}");
    }
}

/// The memory is watched where the system tells how much there is.
#[cfg(target_os = "linux")]
#[test]
fn the_other_process_stops_at_its_memory() {
    use cashcraft_engine::testing::stream;
    let objects = [
        b"<</Type/Catalog/Pages 2 0 R>>".to_vec(),
        b"<</Type/Pages/Kids[3 0 R]/Count 1>>".to_vec(),
        b"<</Type/Page/Parent 2 0 R/MediaBox[0 0 595 842]/Contents 4 0 R>>".to_vec(),
        stream(b"q Q", ""),
    ];
    let bytes = cashcraft_engine::testing::file(&objects);
    let read = |memory| statement_with(Path::new(PROGRAM), &bytes, Limits { memory, ..Default::default() });
    assert_eq!(read(1 << 30), Err(Failure::NoText));
    // Less than the program itself takes: it is stopped before it answers.
    let big: Vec<u8> = [bytes.clone(), vec![b' '; 24 << 20]].concat();
    let stopped = statement_with(Path::new(PROGRAM), &big, Limits { memory: 1 << 20, ..Default::default() });
    assert_eq!(stopped, Err(Failure::Damaged));
}

#[test]
fn a_program_that_is_not_there_is_an_error() {
    let got = statement_with(Path::new("/nowhere/cashcraft"), &statement_file(), Limits::default());
    assert_eq!(got, Err(Failure::NoProcess));
}

/// The whole way, from the command line.
#[test]
fn the_command_line_reports_a_file_that_ends_the_other_process() {
    let dir = std::env::temp_dir().join(format!("cashcraft-cli-pdf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("file.pdf");
    let preview = |bytes: &[u8]| {
        std::fs::write(&file, bytes).unwrap();
        let out = std::process::Command::new(PROGRAM)
            .env("CASHCRAFT_LANG", "en")
            .env("CASHCRAFT_DB", dir.join("books.db"))
            .args(["--json", "preview"])
            .arg(&file)
            .output()
            .unwrap();
        (out.status.code(), [out.stdout, out.stderr].concat())
    };
    let (code, text) = preview(&statement_file());
    assert!(code == Some(0) && String::from_utf8_lossy(&text).contains("\"sound\": true"));
    for (_, bytes) in hostile().into_iter().filter(|h| h.0.starts_with("nested")) {
        let (code, text) = preview(&bytes);
        assert_eq!(code, Some(1));
        assert!(String::from_utf8_lossy(&text).contains("damaged"));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
