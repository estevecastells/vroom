//! `config.toml` in the platform config folder. The R2 secret is not here: it
//! lives in the OS credential store (Keychain, Windows Credential Manager).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where recordings are kept. Defaults to `~/Movies/Vroom` (`~/Videos/Vroom` on Windows).
    pub library: Option<PathBuf>,
    /// Upload each recording as soon as it is saved.
    pub auto_upload: bool,
    /// Remove the local video this many days after it is safely in R2 (0 keeps it forever).
    pub keep_local_days: u32,
    pub r2: Option<R2Config>,
}

impl Default for Config {
    fn default() -> Self {
        Config { library: None, auto_upload: true, keep_local_days: 0, r2: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct R2Config {
    pub account_id: String,
    pub bucket: String,
    pub access_key_id: String,
    /// Public URL of the bucket (a custom domain or `https://pub-….r2.dev`).
    /// With it, share links never expire; without it they are signed for 7 days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    /// Key prefix inside the bucket, to share one bucket between uses.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prefix: String,
    /// `eu` or `fedramp` for jurisdiction-restricted buckets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jurisdiction: Option<String>,
    /// Any other S3-compatible endpoint (MinIO, tests) instead of R2's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

impl R2Config {
    pub fn endpoint(&self) -> String {
        if let Some(e) = self.endpoint.as_deref().filter(|e| !e.is_empty()) {
            return e.trim_end_matches('/').to_string();
        }
        match self.jurisdiction.as_deref() {
            Some(j) if !j.is_empty() => format!("https://{}.{j}.r2.cloudflarestorage.com", self.account_id),
            _ => format!("https://{}.r2.cloudflarestorage.com", self.account_id),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.endpoint.is_none()
            && (self.account_id.len() != 32 || !self.account_id.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err("el Account ID son 32 caracteres hexadecimales".into());
        }
        if self.bucket.is_empty() || self.access_key_id.is_empty() {
            return Err("faltan el bucket o el Access Key ID".into());
        }
        if let Some(u) = &self.public_url {
            if !u.starts_with("https://") && !u.starts_with("http://") {
                return Err("la URL pública debe empezar por https://".into());
            }
        }
        Ok(())
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, String> {
        match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let s = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::library::write_atomic(path, s.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn library_dir(&self) -> PathBuf {
        self.library.clone().unwrap_or_else(default_library_dir)
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default()
}

pub fn default_library_dir() -> PathBuf {
    if cfg!(windows) {
        home().join("Videos").join("Vroom")
    } else {
        home().join("Movies").join("Vroom")
    }
}

/// `VROOM_CONFIG` overrides it, which the tests and a portable install use.
pub fn default_config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("VROOM_CONFIG") {
        return PathBuf::from(p);
    }
    if cfg!(target_os = "macos") {
        home().join("Library/Application Support/Vroom/config.toml")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(home).join("Vroom").join("config.toml")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config"))
            .join("vroom/config.toml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r2() -> R2Config {
        R2Config {
            account_id: "0123456789abcdef0123456789abcdef".into(),
            bucket: "vroom".into(),
            access_key_id: "key".into(),
            public_url: None,
            prefix: String::new(),
            jurisdiction: None,
            endpoint: None,
        }
    }

    #[test]
    fn endpoints() {
        let mut c = r2();
        assert_eq!(c.endpoint(), "https://0123456789abcdef0123456789abcdef.r2.cloudflarestorage.com");
        c.jurisdiction = Some("eu".into());
        assert_eq!(c.endpoint(), "https://0123456789abcdef0123456789abcdef.eu.r2.cloudflarestorage.com");
    }

    #[test]
    fn validates_account_id() {
        let mut c = r2();
        assert!(c.validate().is_ok());
        c.account_id = "nope".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn round_trips_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        let c = Config { r2: Some(r2()), keep_local_days: 30, ..Config::default() };
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), c);
    }
}
