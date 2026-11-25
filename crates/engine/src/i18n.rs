//! The language of the interface. Text is written in English where it is used, inside [`tr`],
//! [`trf`] or [`trn`]; `i18n/it.tsv` has its Italian, one `English<TAB>Italian` line each.

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::{Display, Write};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    It,
}

/// The Italian catalogue.
pub const IT_TSV: &str = include_str!("i18n/it.tsv");

/// Name of the setting in the data file: `en`, `it`, or anything else for the system's language.
pub const META_KEY: &str = "language";

static LANG: AtomicU8 = AtomicU8::new(0);

thread_local! {
    static OVERRIDE: Cell<Option<Lang>> = const { Cell::new(None) };
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::It => "it",
        }
    }

    /// The language for a locale name or language tag: `it`, `it-IT`, `it_CH.UTF-8`…
    pub fn from_tag(tag: &str) -> Self {
        let primary = tag.trim().split(['-', '_', '.', '@']).next().unwrap_or_default();
        if primary.eq_ignore_ascii_case("it") { Lang::It } else { Lang::En }
    }

    /// The language stored in the data file, if one was chosen.
    pub fn from_setting(value: Option<&str>) -> Option<Self> {
        match value? {
            "en" => Some(Lang::En),
            "it" => Some(Lang::It),
            _ => None,
        }
    }

    /// The language of the system: the usual environment variables when set (so that
    /// `LANG=en_US.UTF-8` overrides the desktop), otherwise the language of the user interface as
    /// the system reports it (Windows and macOS do not set those variables). English otherwise.
    pub fn from_system() -> Self {
        let env = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().filter_map(|k| env_var(k)).next();
        Self::from_tag(&env.or_else(sys_locale::get_locale).unwrap_or_default())
    }

    /// The language to start in: `CASHCRAFT_LANG`, else the one stored in the data file, else the
    /// system's.
    pub fn at_start(stored: Option<&str>) -> Self {
        Self::resolve(env_var("CASHCRAFT_LANG"), stored, Self::from_system)
    }

    fn resolve(env: Option<String>, stored: Option<&str>, system: impl FnOnce() -> Self) -> Self {
        match env {
            Some(tag) => Self::from_tag(&tag),
            None => Self::from_setting(stored).unwrap_or_else(system),
        }
    }
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// The language in use.
pub fn lang() -> Lang {
    OVERRIDE.get().unwrap_or(match LANG.load(Ordering::Relaxed) {
        1 => Lang::It,
        _ => Lang::En,
    })
}

/// Sets the language of the whole process.
pub fn set_lang(l: Lang) {
    LANG.store(if l == Lang::It { 1 } else { 0 }, Ordering::Relaxed);
}

/// Runs `f` in a language, on this thread only and whatever the language of the process is.
pub fn with_lang<T>(l: Lang, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Lang>);
    impl Drop for Restore {
        fn drop(&mut self) {
            OVERRIDE.set(self.0);
        }
    }
    let _restore = Restore(OVERRIDE.replace(Some(l)));
    f()
}

fn rows(tsv: &'static str) -> impl Iterator<Item = (&'static str, &'static str)> {
    tsv.lines().filter_map(|l| l.split_once('\t'))
}

/// The text in the language in use; the English itself when it has no Italian.
pub fn tr(english: &'static str) -> &'static str {
    static ITALIAN: OnceLock<HashMap<&str, &str>> = OnceLock::new();
    match lang() {
        Lang::En => english,
        Lang::It => ITALIAN.get_or_init(|| rows(IT_TSV).collect()).get(english).copied().unwrap_or(english),
    }
}

/// [`tr`] with each `{name}` replaced by its value, in whatever order the language needs.
pub fn trf(english: &'static str, args: &[(&str, &dyn Display)]) -> String {
    fill(tr(english), args)
}

/// [`trf`] for a count, the only placeholder being `{n}`.
pub fn trn(n: usize, one: &'static str, many: &'static str) -> String {
    trf(if n == 1 { one } else { many }, &[("n", &n)])
}

/// Replaces the placeholders in one pass, so that braces inside a value are left alone.
fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some((before, after)) = rest.split_once('{') {
        out.push_str(before);
        let value = after.split_once('}').and_then(|(name, tail)| {
            let (_, v) = args.iter().find(|(n, _)| *n == name)?;
            Some((v, tail))
        });
        match value {
            Some((v, tail)) => {
                let _ = write!(out, "{v}");
                rest = tail;
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    #[test]
    fn tags_and_settings() {
        for t in ["it", "IT", "it-IT", "it_CH.UTF-8", "it@euro"] {
            assert_eq!(Lang::from_tag(t), Lang::It, "{t}");
        }
        for t in ["", "en_US.UTF-8", "C", "POSIX", "ita", "de-DE"] {
            assert_eq!(Lang::from_tag(t), Lang::En, "{t}");
        }
        assert_eq!(Lang::from_setting(Some("it")), Some(Lang::It));
        assert_eq!(Lang::from_setting(Some("en")), Some(Lang::En));
        assert_eq!(Lang::from_setting(Some("system")), None);
        assert_eq!(Lang::from_setting(None), None);
    }

    #[test]
    fn the_variable_wins_over_the_data_file_and_that_over_the_system() {
        let it = || Lang::It;
        assert_eq!(Lang::resolve(Some("en".into()), Some("it"), it), Lang::En);
        assert_eq!(Lang::resolve(None, Some("en"), it), Lang::En);
        assert_eq!(Lang::resolve(None, Some("system"), it), Lang::It);
        assert_eq!(Lang::resolve(None, None, it), Lang::It);
    }

    #[test]
    fn placeholders_are_filled_in_one_pass() {
        assert_eq!(fill("{b} then {a}{b}", &[("a", &"{b}"), ("b", &1)]), "1 then {b}1");
        assert_eq!(fill("{x} {", &[("a", &1)]), "{x} {");
    }

    #[test]
    fn a_language_can_be_scoped_and_unknown_text_stays_english() {
        with_lang(Lang::It, || {
            assert_eq!(tr("Save"), "Salva");
            with_lang(Lang::En, || assert_eq!(tr("Save"), "Save"));
            assert_eq!(lang(), Lang::It);
        });
        assert_eq!(with_lang(Lang::It, || tr("no such text")), "no such text");
        assert_eq!(with_lang(Lang::It, || trn(1, "{n} transaction", "{n} transactions")), "1 movimento");
        assert_eq!(with_lang(Lang::It, || trn(2, "{n} transaction", "{n} transactions")), "2 movimenti");
    }

    fn placeholders(s: &str) -> BTreeSet<&str> {
        s.split('{').skip(1).filter_map(|p| p.split_once('}')).map(|(name, _)| name).collect()
    }

    #[test]
    fn the_catalogue_is_well_formed() {
        let mut seen = BTreeSet::new();
        for (i, line) in IT_TSV.lines().enumerate() {
            let cols: Vec<&str> = line.split('\t').collect();
            assert_eq!(cols.len(), 2, "line {}: {line}", i + 1);
            let (en, it) = (cols[0], cols[1]);
            assert!(!en.trim().is_empty() && !it.trim().is_empty(), "line {}", i + 1);
            assert!(seen.insert(en), "twice: {en}");
            assert_eq!(placeholders(en), placeholders(it), "{en}");
        }
    }

    /// A call to `tr`, `trf` or `trn` found in the sources.
    struct Call {
        function: &'static str,
        keys: Vec<String>,
        arguments: String,
        place: String,
    }

    fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                sources(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("engine/src/i18n.rs") {
                out.push(p);
            }
        }
    }

    /// Reads a string literal at the start of `s`: its value and what follows.
    fn literal(s: &str) -> Option<(String, &str)> {
        let body = s.strip_prefix('"')?;
        let mut out = String::new();
        let mut chars = body.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => return Some((out, &body[i + 1..])),
                '\\' => match chars.next()?.1 {
                    '\n' => {
                        while chars.as_str().starts_with(char::is_whitespace) {
                            chars.next();
                        }
                    }
                    'n' => out.push('\n'),
                    c => out.push(c),
                },
                c => out.push(c),
            }
        }
        None
    }

    /// The text up to the parenthesis that closes the call.
    fn arguments(s: &str) -> &str {
        let (mut depth, mut quoted, mut escaped) = (1, false, false);
        for (i, c) in s.char_indices() {
            match c {
                _ if escaped => escaped = false,
                '\\' if quoted => escaped = true,
                '"' => quoted = !quoted,
                '(' if !quoted => depth += 1,
                ')' if !quoted => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                return &s[..i];
            }
        }
        s
    }

    fn calls() -> Vec<Call> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        for dir in ["crates", "apps"] {
            sources(&root.join(dir), &mut files);
        }
        let mut out = Vec::new();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            for function in ["tr", "trf", "trn"] {
                for (at, name) in text.match_indices(&format!("{function}(")) {
                    if text[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    let line = text[..at].matches('\n').count() + 1;
                    let place = format!("{}:{line}", f.display());
                    let args = arguments(&text[at + name.len()..]);
                    let mut rest = args;
                    if function == "trn" {
                        // The count comes first: the keys start at the first literal.
                        rest = &rest[rest.find('"').unwrap_or_else(|| panic!("no literal at {place}"))..];
                    }
                    let mut keys = Vec::new();
                    for _ in 0..if function == "trn" { 2 } else { 1 } {
                        let trimmed = rest.trim_start_matches(|c: char| c == ',' || c.is_whitespace());
                        let (key, tail) =
                            literal(trimmed).unwrap_or_else(|| panic!("{function} needs a string literal at {place}"));
                        keys.push(key);
                        rest = tail;
                    }
                    out.push(Call { function, keys, arguments: rest.to_string(), place });
                }
            }
        }
        out
    }

    #[test]
    fn the_catalogue_has_exactly_the_text_the_sources_use() {
        let calls = calls();
        assert!(calls.len() > 200, "the scan found only {} calls", calls.len());
        let used: BTreeMap<&str, &str> =
            calls.iter().flat_map(|c| c.keys.iter().map(|k| (k.as_str(), c.place.as_str()))).collect();
        let known: BTreeSet<&str> = rows(IT_TSV).map(|(en, _)| en).collect();
        let missing: Vec<_> = used.iter().filter(|(k, _)| !known.contains(*k)).collect();
        let unused: Vec<_> = known.iter().filter(|k| !used.contains_key(*k)).collect();
        assert!(missing.is_empty() && unused.is_empty(), "no Italian for: {missing:#?}\nnever used: {unused:#?}");
    }

    #[test]
    fn every_placeholder_gets_a_value() {
        for c in calls() {
            for k in &c.keys {
                let names = placeholders(k);
                match c.function {
                    "tr" => assert!(names.is_empty(), "{k} at {}", c.place),
                    "trn" => assert!(names.iter().all(|n| *n == "n"), "{k} at {}", c.place),
                    _ => {
                        assert!(!names.is_empty(), "use tr at {}", c.place);
                        for n in names {
                            assert!(c.arguments.contains(&format!("\"{n}\",")), "{{{n}}} at {}", c.place);
                        }
                    }
                }
            }
        }
    }
}
