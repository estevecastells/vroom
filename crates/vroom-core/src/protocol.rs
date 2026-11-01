//! What `vroom --json` prints: one JSON object per line. Long commands print
//! `progress` lines while they work and finish with a result or an `error`.
//! Samples live in `/fixtures` and are checked from both Rust and Swift.

use crate::config::Config;
use crate::library::Entry;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Version {
        version: String,
        protocol: u32,
    },
    Entries {
        entries: Vec<Entry>,
    },
    Entry {
        entry: Entry,
    },
    Config {
        config: Config,
        path: String,
        library: String,
        has_secret: bool,
    },
    /// Bytes of one recording transferred so far.
    Progress {
        id: String,
        done: u64,
        total: u64,
    },
    /// A step of a long command finished for one recording.
    Done {
        id: String,
        what: String,
    },
    Link {
        id: String,
        url: String,
        /// Unix seconds; absent for links that do not expire.
        #[serde(skip_serializing_if = "Option::is_none")]
        expires_at: Option<i64>,
    },
    /// A problem with one recording that did not stop the command.
    Warning {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        message: String,
    },
    Ok {
        message: String,
    },
    Error {
        message: String,
    },
}

impl Event {
    pub fn to_line(&self) -> String {
        let mut s = serde_json::to_string(self).unwrap_or_else(|e| format!(r#"{{"type":"error","message":"{e}"}}"#));
        s.push('\n');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
    }

    /// Every fixture parses, and writes back to the same JSON.
    #[test]
    fn fixtures_round_trip() {
        let mut n = 0;
        for f in std::fs::read_dir(fixtures()).unwrap() {
            let path = f.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let event: Event = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let original: serde_json::Value = serde_json::from_str(&text).unwrap();
            let written = serde_json::to_value(&event).unwrap();
            assert_eq!(original, written, "{}", path.display());
            n += 1;
        }
        assert!(n >= 4);
    }

    #[test]
    fn one_line_each() {
        let line = Event::Progress { id: "abcdefghij".into(), done: 1, total: 2 }.to_line();
        assert_eq!(line, "{\"type\":\"progress\",\"id\":\"abcdefghij\",\"done\":1,\"total\":2}\n");
    }
}
