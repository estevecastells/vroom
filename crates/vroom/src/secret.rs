//! The R2 secret in the OS credential store (macOS Keychain, Windows Credential
//! Manager). `VROOM_R2_SECRET` takes precedence, for scripts and tests.

use vroom_core::tr;

const SERVICE: &str = "vroom-r2";

pub fn get(access_key_id: &str) -> Result<Option<String>, String> {
    if let Ok(s) = std::env::var("VROOM_R2_SECRET") {
        return Ok(Some(s).filter(|s| !s.is_empty()));
    }
    match entry(access_key_id)?.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(tr!("no se pudo leer el secret del llavero: {e}", e)),
    }
}

pub fn set(access_key_id: &str, secret: &str) -> Result<(), String> {
    if std::env::var_os("VROOM_R2_SECRET").is_some() {
        return Ok(());
    }
    entry(access_key_id)?.set_password(secret).map_err(|e| tr!("no se pudo guardar el secret en el llavero: {e}", e))
}

pub fn delete(access_key_id: &str) {
    if std::env::var_os("VROOM_R2_SECRET").is_none() {
        if let Ok(e) = entry(access_key_id) {
            let _ = e.delete_credential();
        }
    }
}

fn entry(access_key_id: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, access_key_id).map_err(|e| e.to_string())
}

const ANTHROPIC_SERVICE: &str = "vroom-anthropic";
const ANTHROPIC_ACCOUNT: &str = "api-key";

/// The Anthropic API key for automatic titles. `VROOM_ANTHROPIC_KEY` takes precedence.
pub fn anthropic_key() -> Result<Option<String>, String> {
    if let Ok(k) = std::env::var("VROOM_ANTHROPIC_KEY") {
        return Ok(Some(k).filter(|k| !k.is_empty()));
    }
    match anthropic_entry()?.get_password() {
        Ok(k) => Ok(Some(k)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(tr!("no se pudo leer la clave de Anthropic del llavero: {e}", e)),
    }
}

pub fn set_anthropic_key(key: &str) -> Result<(), String> {
    anthropic_entry()?
        .set_password(key)
        .map_err(|e| tr!("no se pudo guardar la clave de Anthropic en el llavero: {e}", e))
}

pub fn delete_anthropic_key() {
    if let Ok(e) = anthropic_entry() {
        let _ = e.delete_credential();
    }
}

fn anthropic_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(ANTHROPIC_SERVICE, ANTHROPIC_ACCOUNT).map_err(|e| e.to_string())
}
