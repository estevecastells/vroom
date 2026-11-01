//! What the app remembers between launches, in %APPDATA%\Vroom\app.json. The
//! library, R2 and upload settings live in the CLI's config.toml, as on the Mac.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    /// A whole monitor, chosen in the app.
    #[default]
    Screen,
    /// Windows' own picker: a window or a monitor, chosen at each recording.
    Pick,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub capture_mode: CaptureMode,
    /// Index in `Monitor::enumerate()`; `None` is the primary monitor.
    pub monitor: Option<usize>,
    /// Device name; `None` is the default microphone, `Some("")` no microphone.
    pub microphone: Option<String>,
    pub system_audio: bool,
    pub fps: u32,
    pub transcribe: bool,
    /// Language code chosen in Settings; `None` follows Windows' display language.
    pub language: Option<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            capture_mode: CaptureMode::Screen,
            monitor: None,
            microphone: None,
            system_audio: false,
            fps: 30,
            transcribe: true,
            language: None,
        }
    }
}

fn path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Vroom").join("app.json"))
}

impl Prefs {
    pub fn load() -> Prefs {
        path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(p, json);
        }
    }
}
