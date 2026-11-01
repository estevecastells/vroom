//! Every string the Rust crates translate has an English entry, and the English
//! catalog has nothing the code no longer asks for.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// Crates whose strings live in `vroom-core/locales` (the Windows app has its own).
const CRATES: [&str; 3] = ["vroom-core", "vroom-cloud", "vroom"];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let path = e.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

/// The value of the Rust string literal starting at `s` (just after its opening quote).
fn literal(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '0' => out.push('\0'),
                'u' => {
                    let hex: String = chars.by_ref().skip(1).take_while(|c| *c != '}').collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                // A line continuation drops the newline and the next line's indentation.
                '\n' => {
                    while chars.peek().is_some_and(|c| c.is_whitespace()) {
                        chars.next();
                    }
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// Source strings passed to `tr!("…")`, `t("…")` and `t_in(lang, "…")`.
fn sources(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    for call in ["tr!(", "t(", "t_in("] {
        for (i, _) in code.match_indices(call) {
            let before = code[..i].chars().next_back();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let rest = &code[i + call.len()..];
            let args = rest.trim_start();
            let start = if call == "t_in(" {
                // The language comes first: skip to the literal, if the call has one.
                match args.find(['"', ')']) {
                    Some(q) if args.as_bytes()[q] == b'"' => q + 1,
                    _ => continue,
                }
            } else if args.starts_with('"') {
                1
            } else {
                continue;
            };
            found.push(literal(&args[start..]).unwrap_or_else(|| panic!("unterminated literal after {call}")));
        }
    }
    found
}

#[test]
fn english_catalog_matches_the_code() {
    let en: HashMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(root().join("vroom-core/locales/en.json")).unwrap()).unwrap();
    let mut used = BTreeSet::new();
    for krate in CRATES {
        let mut files = Vec::new();
        rust_files(&root().join(krate).join("src"), &mut files);
        for file in files {
            // i18n.rs shows the macros in its docs with made-up examples.
            if file.ends_with("i18n.rs") {
                continue;
            }
            used.extend(sources(&std::fs::read_to_string(&file).unwrap()));
        }
    }
    let missing: Vec<&String> = used.iter().filter(|s| !en.contains_key(*s)).collect();
    assert!(missing.is_empty(), "missing from locales/en.json:\n{}", serde_json::to_string_pretty(&missing).unwrap());
    let stale: Vec<&String> = en.keys().filter(|k| !used.contains(*k)).collect();
    assert!(stale.is_empty(), "in locales/en.json but not in the code:\n{stale:#?}");
}

#[test]
fn finds_literals() {
    let code = r#"tr!("a {x}", x); t("b \"q\""); i18n::t("c"); split("no"); t_in(lang, "d"); t_in(lang, MONTHS)"#;
    assert_eq!(sources(code), vec!["a {x}", "b \"q\"", "c", "d"]);
    assert_eq!(literal("x\\\n    y\""), Some("xy".into()));
}
