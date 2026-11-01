//! The app's language. Strings are written in Spanish and translated with
//! `vroom_core::tr!` / `vroom_core::i18n::t` (crates/vroom-core/src/i18n.rs).
//! The app's own strings live in `crates/vroom-win/locales/<code>.json`; to add
//! a language, add that file (and the CLI's, in crates/vroom-core/locales/)
//! and one line to `BUILT_IN`.

use vroom_core::i18n;

/// The app's catalogs, added to the CLI strings vroom-core already has.
const BUILT_IN: &[(&str, &str)] = &[("en", include_str!("../locales/en.json"))];

/// Registers the catalogs and picks the language: the one chosen in Settings,
/// else the first of Windows' display languages with a translation, else English.
pub fn init(choice: Option<&str>) {
    for (code, json) in BUILT_IN {
        i18n::register(code, json);
    }
    apply(choice);
}

/// Switches the language; `None` follows Windows.
pub fn apply(choice: Option<&str>) {
    let code = match choice {
        Some(c) if i18n::available().iter().any(|a| a == c) => c.to_string(),
        _ => i18n::resolve(&system_languages()),
    };
    i18n::set_lang(&code);
}

/// A language's name in that language, for the picker.
pub fn name(code: &str) -> String {
    match code {
        "en" => "English",
        "es" => "Español",
        other => other,
    }
    .to_string()
}

/// The user's preferred display languages (`es-ES`, `en-US`, …), most preferred first.
#[cfg(windows)]
fn system_languages() -> Vec<String> {
    use windows_sys::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    let (mut count, mut len) = (0u32, 0u32);
    // First the size, in UTF-16 units including the terminating nulls; then the list.
    if unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, std::ptr::null_mut(), &mut len) } == 0
        || len == 0
    {
        return vec![];
    }
    let mut buf = vec![0u16; len as usize];
    if unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buf.as_mut_ptr(), &mut len) } == 0 {
        return vec![];
    }
    buf.truncate(len as usize);
    parse_multi_string(&buf)
}

#[cfg(not(windows))]
fn system_languages() -> Vec<String> {
    vec![]
}

/// A Windows multi-string (`"es-ES\0en-US\0\0"`) as its strings.
fn parse_multi_string(buf: &[u16]) -> Vec<String> {
    buf.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap};

    #[test]
    fn reads_windows_multi_strings() {
        let buf: Vec<u16> = "es-ES\0en-US\0\0".encode_utf16().collect();
        assert_eq!(parse_multi_string(&buf), vec!["es-ES", "en-US"]);
        assert!(parse_multi_string(&[0, 0]).is_empty());
    }

    #[test]
    fn catalogs_cover_every_string() {
        let used = source_strings();
        assert!(used.len() > 100, "found only {} strings", used.len());
        for (code, json) in BUILT_IN {
            vroom_core::i18n::check_catalog(json).unwrap_or_else(|e| panic!("locales/{code}.json: {e}"));
            let catalog: HashMap<String, String> = serde_json::from_str(json).unwrap();
            let missing: Vec<_> = used.iter().filter(|s| !catalog.contains_key(*s)).collect();
            assert!(missing.is_empty(), "locales/{code}.json lacks {missing:#?}");
            let unused: Vec<_> = catalog.keys().filter(|k| !used.contains(*k)).collect();
            assert!(unused.is_empty(), "locales/{code}.json has strings no longer used: {unused:#?}");
        }
    }

    #[test]
    fn picker_names() {
        assert_eq!(name("es"), "Español");
        assert_eq!(name("pt"), "pt");
    }

    /// Every string literal given to the `tr!` macro or the `t` function in src/.
    fn source_strings() -> BTreeSet<String> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut found = BTreeSet::new();
        for file in std::fs::read_dir(dir).unwrap() {
            let path = file.unwrap().path();
            if path.extension().is_some_and(|e| e == "rs") {
                let code = std::fs::read_to_string(&path).unwrap();
                // Split so that this file's own text does not match.
                literals_after(&code, concat!("tr", "!("), &mut found);
                literals_after(&code, concat!("t", "("), &mut found);
            }
        }
        found
    }

    fn literals_after(code: &str, opener: &str, found: &mut BTreeSet<String>) {
        let mut rest = code;
        while let Some(at) = rest.find(opener) {
            // `t(` must be the whole name, not the end of `set_text(` or `print(`.
            let whole = rest[..at].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            rest = &rest[at + opener.len()..];
            // The literal may be on the next line.
            let Some(body) = rest.trim_start().strip_prefix('"') else { continue };
            if !whole {
                continue;
            }
            if let Some((literal, after)) = unescape(body) {
                found.insert(literal);
                rest = after;
            }
        }
    }

    /// The body of a string literal up to its closing quote, and what follows.
    fn unescape(s: &str) -> Option<(String, &str)> {
        let mut out = String::new();
        let mut chars = s.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => return Some((out, &s[i + 1..])),
                '\\' => match chars.next()?.1 {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    '0' => out.push('\0'),
                    '\n' => {
                        // A line continuation skips the next line's indentation.
                        while chars.clone().next().is_some_and(|(_, c)| c.is_whitespace()) {
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
}
