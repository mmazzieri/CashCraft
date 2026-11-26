//! A card statement or a list of an account's movements from a PDF: the text with its positions, then its meaning. All of it runs in another process, this program started
//! again with [`CHILD`]: on hostile files it can overflow the stack, which would end the program whatever
//! the thread, or go on for ever, and a thread cannot be stopped. Only the statement comes back.

use crate::bank_pdf::{self, Movements};
use crate::card_pdf::{self, Statement};
use crate::i18n::tr;
use anyhow::{Result, anyhow};
use hayro_interpret::font::{FontData, Glyph};
use hayro_interpret::hayro_cmap::BfString;
use hayro_interpret::{
    BlendMode, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache, InterpreterSettings, Paint,
    PathDrawMode, SoftMask, interpret_page,
};
use hayro_syntax::{LoadPdfError, Pdf};
use kurbo::{Affine, BezPath, Rect, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// The first argument of the process that reads; the second is how much memory it may use.
pub const CHILD: &str = "--read-pdf-from-stdin";
/// Set once the program is known to answer to [`CHILD`].
static CHILD_READY: AtomicBool = AtomicBool::new(false);

const TIME_LIMIT: Duration = Duration::from_secs(10);
const STACK: usize = 64 << 20;
const MAX_PAGES: usize = 100;
const MAX_GLYPHS: usize = 200_000;
/// Most bytes the process that reads may answer, and (where the system tells) use.
const MAX_ANSWER: u64 = 16 << 20;
const MAX_MEMORY: u64 = 1 << 30;
/// A font with no tables. A font the PDF names but does not carry (the standard ones above all, which
/// banks print with) is read through it: what its characters are comes from the encoding of the PDF and
/// their widths from the metrics of the standard fonts or from the PDF. Nothing is drawn, so no outlines are needed.
const NO_FONT: [u8; 12] = [0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// What the process that reads may take.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub time: Duration,
    /// Bytes, where the system tells them (Linux).
    pub memory: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { time: TIME_LIMIT, memory: MAX_MEMORY }
    }
}

/// Characters on baselines closer than this are on one line, in points.
const SAME_LINE: f64 = 2.0;
/// A gap wider than this part of the font size separates two words.
const SPACE: f64 = 0.16;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub x0: f64,
    pub x1: f64,
    pub text: String,
}

/// Words on one baseline, from left to right; `y` grows downwards.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub y: f64,
    pub words: Vec<Word>,
}

pub type Page = Vec<Line>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Failure {
    Damaged,
    Encrypted,
    NoText,
    TooLarge,
    TimeOut,
    NoProcess,
    /// Not a statement of a known kind.
    Unknown,
    /// A list of movements whose rows do not add up to its balances, or that has rows that could not be read.
    Mismatch,
}

impl Failure {
    fn message(self) -> &'static str {
        match self {
            Failure::Damaged => tr("the PDF is damaged and cannot be read"),
            Failure::Encrypted => tr("the PDF is protected by a password: save a copy without it"),
            Failure::NoText => tr("the PDF has no text that can be read: it may be a scan"),
            Failure::TooLarge => tr("the PDF is too large for a statement"),
            Failure::TimeOut => tr("the PDF takes too long to read"),
            Failure::NoProcess => {
                tr("the program that reads PDF files could not start: close CashCraft, open it again and retry")
            }
            Failure::Unknown => tr("the PDF is not a statement of a kind that can be read"),
            Failure::Mismatch => tr(
                "some movements of the PDF could not be read, or they do not add up to its balances: nothing was imported",
            ),
        }
    }
}

/// What a PDF turned out to be.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Document {
    Card(Statement),
    Movements(Movements),
}

pub type Outcome = std::result::Result<Document, Failure>;

pub fn is_pdf(bytes: &[u8]) -> bool {
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(0);
    bytes.get(start..).unwrap_or_default().starts_with(b"%PDF-")
}

/// The first thing a program does: whether it was started only to read a PDF, which is done.
pub fn answer_if_child() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_none_or(|a| a != CHILD) {
        CHILD_READY.store(true, Ordering::Relaxed);
        return false;
    }
    let most = args.next().and_then(|a| a.to_str()?.parse().ok()).unwrap_or(MAX_MEMORY);
    let start = Instant::now();
    std::thread::spawn(move || {
        // The parent may be gone.
        while start.elapsed() < TIME_LIMIT * 2 && memory().is_none_or(|m| m < most) {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::process::exit(3);
    });
    let mut bytes = Vec::new();
    if std::io::stdin().lock().take(crate::importer::MAX_FILE + 1).read_to_end(&mut bytes).is_ok()
        && let Ok(answer) = serde_json::to_vec(&in_thread(bytes))
    {
        let _ = std::io::stdout().lock().write_all(&answer);
    }
    true
}

/// Bytes of memory this process uses.
fn memory() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = text.lines().find_map(|l| l.strip_prefix("VmRSS:"))?;
    line.split_whitespace().next()?.parse::<u64>().ok()?.checked_mul(1024)
}

/// The card statement or the list of movements in a PDF.
pub fn document(bytes: &[u8]) -> Result<Document> {
    let outcome = if CHILD_READY.load(Ordering::Relaxed) {
        match std::env::current_exe() {
            Ok(exe) => read_with(&exe, bytes, Limits::default()),
            Err(_) => Err(Failure::NoProcess),
        }
    } else {
        // Tests, and programs that do not answer to CHILD.
        in_thread(bytes.to_vec())
    };
    outcome.map_err(|f| anyhow!(f.message()))
}

/// [`document`], read by a program that answers to [`CHILD`].
pub fn read_with(program: &Path, bytes: &[u8], limits: Limits) -> Outcome {
    let deadline = Instant::now() + limits.time;
    let mut child = Command::new(program)
        .args([CHILD, &limits.memory.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Failure::NoProcess)?;
    let (Some(mut input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
        return Err(Failure::NoProcess);
    };
    let (send, receive) = mpsc::channel();
    let bytes = bytes.to_vec();
    // Either pipe may fill up: both are served while the deadline runs. The threads end with the process.
    std::thread::spawn(move || {
        let _ = input.write_all(&bytes);
    });
    std::thread::spawn(move || {
        let mut answer = Vec::new();
        let _ = output.take(MAX_ANSWER).read_to_end(&mut answer);
        let _ = send.send(answer);
    });
    let answer = receive.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    let _ = child.kill();
    let _ = child.wait();
    match answer {
        Ok(a) => serde_json::from_slice::<Outcome>(&a).unwrap_or(Err(Failure::Damaged)),
        Err(_) => Err(Failure::TimeOut),
    }
}

/// With enough stack for any sound file; a panic is a damaged file.
fn in_thread(bytes: Vec<u8>) -> Outcome {
    let thread = std::thread::Builder::new().stack_size(STACK).spawn(move || parse(&pages(bytes)?));
    thread.map_err(|_| Failure::NoProcess)?.join().unwrap_or(Err(Failure::Damaged))
}

/// A card statement, else a list of movements.
pub(crate) fn parse(pages: &[Page]) -> Outcome {
    match card_pdf::parse(pages) {
        Err(Failure::Unknown) => bank_pdf::parse(pages).map(Document::Movements),
        other => other.map(Document::Card),
    }
}

/// The lines of each page.
pub(crate) fn pages(bytes: Vec<u8>) -> std::result::Result<Vec<Page>, Failure> {
    let pdf = Pdf::new(bytes).map_err(|e| match e {
        LoadPdfError::Decryption(_) => Failure::Encrypted,
        LoadPdfError::Invalid => Failure::Damaged,
    })?;
    if pdf.pages().len() > MAX_PAGES {
        return Err(Failure::TooLarge);
    }
    let mut left = MAX_GLYPHS;
    let mut pages = Vec::new();
    for page in pdf.pages().iter() {
        let (w, h) = page.render_dimensions();
        let cache = InterpreterCache::new();
        let settings = InterpreterSettings {
            render_annotations: false,
            font_resolver: Arc::new(|_| Some((Arc::new(NO_FONT) as FontData, 0))),
            ..Default::default()
        };
        let area = Rect::new(0.0, 0.0, f64::from(w), f64::from(h));
        let mut context =
            Context::new(convert(page.initial_transform(true).as_coeffs()), area, &cache, page.xref(), settings);
        let mut text = Text { glyphs: Vec::new(), left, full: false };
        interpret_page(page, &mut context, &mut text);
        if text.full {
            return Err(Failure::TooLarge);
        }
        left = text.left;
        pages.push(lines(text.glyphs));
    }
    if pages.iter().all(Vec::is_empty) {
        return Err(Failure::NoText);
    }
    Ok(pages)
}

fn convert(coeffs: [f64; 6]) -> Affine {
    Affine::new(coeffs)
}

struct Drawn {
    x0: f64,
    x1: f64,
    y: f64,
    size: f64,
    text: String,
    /// The font does not tell the width.
    guessed: bool,
}

/// Collects the characters of a page and draws nothing.
struct Text {
    glyphs: Vec<Drawn>,
    left: usize,
    full: bool,
}

impl<'a> Device<'a> for Text {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, glyph: &Glyph<'a>, t: Affine, own: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {
        let Some(left) = self.left.checked_sub(1) else {
            self.full = true;
            return;
        };
        self.left = left;
        let text = match glyph.as_unicode() {
            Some(BfString::Char(c)) => c.to_string(),
            Some(BfString::String(s)) => s,
            None => return,
        };
        // A thousandth of the font size is the unit.
        let m = t * own;
        let [a, b, c, d, x, y] = m.as_coeffs();
        let size = Vec2::new(c, d).hypot() * 1000.0;
        let upright = a > 0.0 && d < 0.0 && b.abs() < a * 0.02;
        if !upright || ![x, y, size].iter().all(|v| v.is_finite()) || text.is_empty() {
            return;
        }
        let advance = match glyph {
            Glyph::Outline(g) => g.advance_width().filter(|w| *w > 0.0).map(f64::from),
            Glyph::Type3(_) => None,
        };
        let guessed = advance.is_none();
        let advance = advance.unwrap_or_else(|| text.chars().map(usual_width).sum());
        self.glyphs.push(Drawn { x0: x, x1: x + advance * a, y, size, text, guessed });
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// The width of a character in most sans serif fonts, where the font does not say.
fn usual_width(c: char) -> f64 {
    match c {
        'i' | 'j' | 'l' | '\'' | '|' => 222.0,
        ' ' | '.' | ',' | ':' | ';' | '!' | '/' | 'f' | 't' | 'I' | '[' | ']' => 278.0,
        'r' | '-' | '(' | ')' | '*' => 350.0,
        'm' | 'M' | 'W' | 'w' | '%' | '@' => 830.0,
        c if c.is_uppercase() => 680.0,
        _ => 540.0,
    }
}

fn lines(mut glyphs: Vec<Drawn>) -> Page {
    glyphs.sort_by(|a, b| a.y.total_cmp(&b.y));
    let mut groups: Vec<Vec<Drawn>> = Vec::new();
    for g in glyphs {
        match groups.last_mut() {
            Some(l) if l.first().is_some_and(|f| g.y - f.y < SAME_LINE) => l.push(g),
            _ => groups.push(vec![g]),
        }
    }
    let mut widths: HashMap<String, f64> = HashMap::new();
    for group in &mut groups {
        group.sort_by(|a, b| a.x0.total_cmp(&b.x0));
        // Bold text may be drawn twice.
        group.dedup_by(|b, a| a.text == b.text && b.x0 - a.x0 < a.size * 0.08);
        // A character is as wide as the nearest one ever drawn after it.
        for (a, b) in group.iter().zip(group.iter().skip(1)).filter(|(a, _)| a.guessed) {
            let w = (b.x0 - a.x0) / a.size;
            widths.entry(a.text.clone()).and_modify(|x| *x = x.min(w)).or_insert(w);
        }
    }
    let mut out = Page::new();
    for mut group in groups {
        let y = group.first().map_or(0.0, |g| g.y);
        for g in group.iter_mut().filter(|g| g.guessed) {
            if let Some(w) = widths.get(&g.text).filter(|w| **w > 0.1) {
                g.x1 = g.x1.min(g.x0 + w * g.size);
            }
        }
        let words = words(&group);
        if !words.is_empty() {
            out.push(Line { y, words });
        }
    }
    out
}

/// Spaces are not always characters: a gap makes one too.
fn words(line: &[Drawn]) -> Vec<Word> {
    let gap = |a: &Drawn, b: &Drawn| (b.x0 - a.x1) / a.size.max(b.size).max(0.01);
    // Letters may be spaced out: a word break is wider than most gaps of the line.
    let mut gaps: Vec<f64> = line.iter().zip(line.iter().skip(1)).map(|(a, b)| gap(a, b)).collect();
    gaps.sort_by(f64::total_cmp);
    let usual = gaps.get(gaps.len() / 4).copied().unwrap_or(0.0).clamp(0.0, 0.1);
    let mut out: Vec<Word> = Vec::new();
    let mut last: Option<&Drawn> = None;
    for g in line {
        if g.text.trim().is_empty() {
            last = None;
            continue;
        }
        match (last, out.last_mut()) {
            (Some(l), Some(w)) if gap(l, g) < usual + SPACE => {
                w.text.push_str(g.text.trim());
                w.x1 = w.x1.max(g.x1);
            }
            _ => out.push(Word { x0: g.x0, x1: g.x1, text: g.text.trim().to_string() }),
        }
        last = Some(g);
    }
    out
}

#[cfg(test)]
pub(crate) mod tests;
