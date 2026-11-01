//! `meta.json`: what a recording is. The same file lives next to the video on
//! disk and in the bucket, so a new computer can rebuild the library from R2.

use crate::i18n::{fill, t_in};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    #[default]
    Vroom,
    Loom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub id: String,
    pub title: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default)]
    pub duration_secs: f64,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub source: Source,
    /// The video's id at the original service, for imports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Folder path inside the library, `/`-separated (`Clientes/Acme`); none at the top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

impl Meta {
    pub fn new(id: String, title: String, created_at: OffsetDateTime) -> Self {
        Meta {
            id,
            title,
            created_at,
            duration_secs: 0.0,
            width: 0,
            height: 0,
            size_bytes: 0,
            source: Source::Vroom,
            source_id: None,
            description: None,
            folder: None,
        }
    }
}

/// "Grabación 1 nov 2026, 14:32" in local time when the offset is known, in the current language.
pub fn default_title(at: OffsetDateTime) -> String {
    default_title_in(&crate::i18n::lang(), at)
}

/// The default title in a given language: "Recording Nov 1, 2026, 14:32" in English.
pub fn default_title_in(lang: &str, at: OffsetDateTime) -> String {
    let months = |lang| t_in(lang, "ene feb mar abr may jun jul ago sep oct nov dic");
    let mut names = months(lang);
    if names.split_whitespace().count() != 12 {
        names = months(crate::i18n::SOURCE);
    }
    let month = names.split_whitespace().nth(at.month() as usize - 1).unwrap_or_default().to_string();
    fill(
        &t_in(lang, "Grabación {day} {month} {year}, {time}"),
        &[
            ("day", at.day().to_string()),
            ("month", month),
            ("year", at.year().to_string()),
            ("time", format!("{:02}:{:02}", at.hour(), at.minute())),
        ],
    )
}

/// `" a / b//c "` → `Some("a/b/c")`; empty → `None`.
pub fn normalize_folder(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').map(str::trim).filter(|p| !p.is_empty()).collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn folders_are_normalized() {
        assert_eq!(normalize_folder(" Clientes / Acme//"), Some("Clientes/Acme".into()));
        assert_eq!(normalize_folder(" / "), None);
    }

    #[test]
    fn titles_in_spanish_and_english() {
        let at = datetime!(2026-11-01 14:32 +01:00);
        assert_eq!(default_title_in("es", at), "Grabación 1 nov 2026, 14:32");
        assert_eq!(default_title_in("en", at), "Recording Nov 1, 2026, 14:32");
        assert_eq!(default_title_in("en", datetime!(2026-05-09 08:05 UTC)), "Recording May 9, 2026, 08:05");
    }

    #[test]
    fn old_files_without_optional_fields_still_load() {
        let m: Meta =
            serde_json::from_str(r#"{"id":"abcdefghij","title":"x","created_at":"2026-11-01T13:32:00Z"}"#).unwrap();
        assert_eq!(m.source, Source::Vroom);
        assert_eq!(m.size_bytes, 0);
    }
}
