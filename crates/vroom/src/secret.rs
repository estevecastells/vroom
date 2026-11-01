//! The R2 secret in the OS credential store (macOS Keychain, Windows Credential
//! Manager). `VROOM_R2_SECRET` takes precedence, for scripts and tests.

const SERVICE: &str = "vroom-r2";

pub fn get(access_key_id: &str) -> Result<Option<String>, String> {
    if let Ok(s) = std::env::var("VROOM_R2_SECRET") {
        return Ok(Some(s).filter(|s| !s.is_empty()));
    }
    match entry(access_key_id)?.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("no se pudo leer el secret del llavero: {e}")),
    }
}

pub fn set(access_key_id: &str, secret: &str) -> Result<(), String> {
    if std::env::var_os("VROOM_R2_SECRET").is_some() {
        return Ok(());
    }
    entry(access_key_id)?.set_password(secret).map_err(|e| format!("no se pudo guardar el secret en el llavero: {e}"))
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
