//! `meta.json`: what a recording is. The same file lives next to the video on
//! disk and in the bucket, so a new computer can rebuild the library from R2.

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
        }
    }
}

/// "Grabación 1 nov 2026, 14:32" in local time when the offset is known.
pub fn default_title(at: OffsetDateTime) -> String {
    const MONTHS: [&str; 12] = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"];
    format!(
        "Grabación {} {} {}, {:02}:{:02}",
        at.day(),
        MONTHS[at.month() as usize - 1],
        at.year(),
        at.hour(),
        at.minute()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn title_in_spanish() {
        assert_eq!(default_title(datetime!(2026-11-01 14:32 +01:00)), "Grabación 1 nov 2026, 14:32");
    }

    #[test]
    fn old_files_without_optional_fields_still_load() {
        let m: Meta =
            serde_json::from_str(r#"{"id":"abcdefghij","title":"x","created_at":"2026-11-01T13:32:00Z"}"#).unwrap();
        assert_eq!(m.source, Source::Vroom);
        assert_eq!(m.size_bytes, 0);
    }
}
