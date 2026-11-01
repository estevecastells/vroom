//! Translations for everything Rust shows to people: CLI messages and errors,
//! default titles, the share page, and the Windows app.
//!
//! The Spanish text in the code is the source. Each other language is a JSON
//! file mapping that Spanish text to its translation, `{name}` placeholders
//! included: `{"no existe la grabación {id}": "recording {id} does not exist"}`.
//! A missing entry shows the Spanish text, and a language with no file falls
//! back to English. To add a language, add `locales/<code>.json` here (and in
//! `crates/vroom-win/locales/`) and list it in `BUILT_IN`.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// The language the code is written in.
pub const SOURCE: &str = "es";
/// Used when none of the person's languages has a translation.
pub const FALLBACK: &str = "en";

/// Catalogs compiled into every Rust binary.
const BUILT_IN: &[(&str, &str)] = &[("en", include_str!("../locales/en.json"))];

struct State {
    lang: Option<String>,
    catalogs: HashMap<String, HashMap<String, String>>,
}

fn state() -> &'static RwLock<State> {
    static STATE: OnceLock<RwLock<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        let mut catalogs = HashMap::new();
        for (lang, json) in BUILT_IN {
            catalogs.insert(lang.to_string(), parse(lang, json));
        }
        RwLock::new(State { lang: None, catalogs })
    })
}

fn parse(lang: &str, json: &str) -> HashMap<String, String> {
    serde_json::from_str(json).unwrap_or_else(|e| panic!("locales/{lang}.json: {e}"))
}

/// Adds or extends a catalog: the Windows app registers its own strings.
pub fn register(lang: &str, json: &str) {
    let catalog = parse(lang, json);
    let mut s = state().write().unwrap_or_else(|e| e.into_inner());
    s.catalogs.entry(lang.to_string()).or_default().extend(catalog);
}

/// Languages with a translation, the source language first.
pub fn available() -> Vec<String> {
    let s = state().read().unwrap_or_else(|e| e.into_inner());
    let mut langs: Vec<String> = s.catalogs.keys().cloned().collect();
    langs.sort();
    langs.insert(0, SOURCE.to_string());
    langs
}

/// The first of `preferred` (codes like `ca-ES`, `en_US.UTF-8`, `pt-BR`) that
/// has a translation, else English.
pub fn resolve<S: AsRef<str>>(preferred: &[S]) -> String {
    let available = available();
    for p in preferred {
        let code = primary(p.as_ref());
        if available.contains(&code) {
            return code;
        }
    }
    FALLBACK.to_string()
}

/// `en_US.UTF-8` → `en`; `C` and `POSIX` say nothing about the language.
fn primary(code: &str) -> String {
    let code = code.split(['.', '@']).next().unwrap_or("");
    let code = code.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    if code == "c" || code == "posix" {
        String::new()
    } else {
        code
    }
}

/// From `VROOM_LANG` (the apps pass theirs), then the POSIX locale variables.
pub fn from_env() -> String {
    let preferred: Vec<String> = ["VROOM_LANG", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .filter(|v| !primary(v).is_empty())
        .collect();
    resolve(&preferred)
}

pub fn set_lang(code: &str) {
    state().write().unwrap_or_else(|e| e.into_inner()).lang = Some(code.to_string());
}

/// The current language; resolved from the environment on first use.
pub fn lang() -> String {
    if let Some(l) = state().read().unwrap_or_else(|e| e.into_inner()).lang.clone() {
        return l;
    }
    let l = from_env();
    // A `set_lang` made meanwhile wins over the environment.
    state().write().unwrap_or_else(|e| e.into_inner()).lang.get_or_insert(l).clone()
}

/// The translation of a Spanish source string, or the string itself.
pub fn t(source: &str) -> String {
    let lang = lang();
    if lang == SOURCE {
        return source.to_string();
    }
    let s = state().read().unwrap_or_else(|e| e.into_inner());
    s.catalogs.get(&lang).and_then(|c| c.get(source)).cloned().unwrap_or_else(|| source.to_string())
}

/// The translation into a given language, without touching the current one:
/// for tests, which share one process, and for anything rendered for someone else.
pub fn t_in(lang: &str, source: &str) -> String {
    if lang == SOURCE {
        return source.to_string();
    }
    let s = state().read().unwrap_or_else(|e| e.into_inner());
    s.catalogs.get(lang).and_then(|c| c.get(source)).cloned().unwrap_or_else(|| source.to_string())
}

/// Replaces each `{name}` with its value.
pub fn fill(template: &str, args: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// `tr!("no existe la grabación {id}", id)` or `tr!("… {n} …", n = list.len())`:
/// the translated text with its placeholders filled in.
#[macro_export]
macro_rules! tr {
    ($s:literal) => {
        $crate::i18n::t($s)
    };
    ($s:literal, $($name:ident = $val:expr),+ $(,)?) => {
        $crate::i18n::fill(&$crate::i18n::t($s), &[$((stringify!($name), ($val).to_string())),+])
    };
    ($s:literal, $($name:ident),+ $(,)?) => {
        $crate::i18n::fill(&$crate::i18n::t($s), &[$((stringify!($name), $name.to_string())),+])
    };
}

/// Placeholders like `{id}` in a template, sorted.
pub fn placeholders(template: &str) -> Vec<&str> {
    let mut found: Vec<&str> = template
        .split('{')
        .skip(1)
        .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
        .filter(|name| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .collect();
    found.sort_unstable();
    found
}

/// Checks a catalog: valid JSON, and every translation keeps its key's placeholders.
pub fn check_catalog(json: &str) -> Result<usize, String> {
    let catalog: HashMap<String, String> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    for (key, value) in &catalog {
        if placeholders(key) != placeholders(value) {
            return Err(format!("placeholders differ: {key:?} → {value:?}"));
        }
    }
    Ok(catalog.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_catalogs_keep_placeholders() {
        for (lang, json) in BUILT_IN {
            check_catalog(json).unwrap_or_else(|e| panic!("locales/{lang}.json: {e}"));
        }
    }

    #[test]
    fn resolves_like_an_os_would() {
        assert_eq!(resolve(&["ca-ES", "es-ES", "en"]), "es");
        assert_eq!(resolve(&["en_US.UTF-8"]), "en");
        assert_eq!(resolve(&["de-DE"]), "en");
        assert_eq!(resolve::<&str>(&[]), "en");
        assert_eq!(primary("C.UTF-8"), "");
    }

    #[test]
    fn fills_named_placeholders() {
        assert_eq!(fill("{n} de {total}", &[("n", "1".into()), ("total", "3".into())]), "1 de 3");
        assert_eq!(placeholders("a {id} b {n} {}"), vec!["id", "n"]);
    }
}
