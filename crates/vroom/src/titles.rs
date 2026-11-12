//! Titles written by Claude from a recording's captions, for recordings that
//! still have their default "Recording <date>" title. Opt-in: it needs an
//! Anthropic API key (`vroom config anthropic`), and sends the transcript, never
//! the video, to Anthropic.

use crate::out::Out;
use crate::secret;
use crate::{take_flag, Ctx};
use serde_json::{json, Value};
use std::time::Duration;
use vroom_core::i18n::{self, t};
use vroom_core::layout::CAPTIONS;
use vroom_core::library::Entry;
use vroom_core::meta::{default_title_in, Meta};
use vroom_core::tr;

const API: &str = "https://api.anthropic.com/v1";
const VERSION: &str = "2026-10-21";
const MODEL: &str = "claude-opus-5-5";
const SYSTEM: &str = "You name screen recordings from their transcript. Write a short, specific title \
    (at most 10 words) that says what the recording is about, in the language the transcript is \
    in. No quotes, no emoji, no trailing period.";
/// Fewer words than this is silence or noise, not something to name.
const MIN_WORDS: usize = 5;

/// `vroom title <id>... | --missing [--force]`
pub fn run(out: &Out, ctx: &Ctx, mut args: Vec<String>) -> Result<(), String> {
    let missing = take_flag(&mut args, "--missing");
    let force = take_flag(&mut args, "--force");
    let key = secret::anthropic_key()?.ok_or_else(|| t("falta la clave de Anthropic: vroom config anthropic"))?;
    let ids: Vec<String> = if missing {
        ctx.lib
            .list()?
            .into_iter()
            .filter(|e| e.captions.is_some() && is_default_title(&e.meta))
            .map(|e| e.meta.id)
            .collect()
    } else {
        args.into_iter().filter(|a| a != "--").collect()
    };
    if ids.is_empty() && !missing {
        return Err(tr!("falta el argumento <{what}>", what = "id"));
    }
    let mut failed = 0;
    for id in &ids {
        match retitle(ctx, &key, id, force) {
            Ok(Some(entry)) => out.entry(&entry),
            Ok(None) => {}
            Err(e) => {
                failed += 1;
                out.warning(Some(id), &e.message);
                // No credits or a bad key fail every other recording the same way.
                if e.account {
                    return Err(e.message);
                }
            }
        }
    }
    if failed > 0 {
        return Err(tr!("{failed} de {total} títulos fallaron", failed = failed, total = ids.len()));
    }
    Ok(())
}

/// After a transcription: a title when there is a key and the recording has its default one.
pub fn after_transcribe(ctx: &Ctx, id: &str) -> Result<Option<Entry>, String> {
    match secret::anthropic_key()? {
        Some(key) => retitle(ctx, &key, id, false).map_err(|e| e.message),
        None => Ok(None),
    }
}

/// Why a title wasn't written.
struct TitleError {
    message: String,
    /// The account or key is the problem (no credits, revoked key), not this recording.
    account: bool,
}

impl From<String> for TitleError {
    fn from(message: String) -> Self {
        TitleError { message, account: false }
    }
}

fn retitle(ctx: &Ctx, key: &str, id: &str, force: bool) -> Result<Option<Entry>, TitleError> {
    let entry = ctx.lib.entry(id)?;
    if !force && !is_default_title(&entry.meta) {
        return Ok(None);
    }
    let captions = std::fs::read_to_string(entry.dir.join(CAPTIONS))
        .map_err(|_| t("no tiene subtítulos: transcríbela primero"))?;
    let text = transcript(&captions);
    if text.split_whitespace().count() < MIN_WORDS {
        return Ok(None);
    }
    let title = ask(key, &text)?;
    Ok(Some(ctx.lib.rename(id, &title)?))
}

/// Whether a recording still has the title it was created with, in any language.
pub fn is_default_title(meta: &Meta) -> bool {
    i18n::available().iter().any(|lang| meta.title == default_title_in(lang, meta.created_at))
}

/// The spoken text of a WebVTT file: no header, cue numbers or timings.
fn transcript(vtt: &str) -> String {
    let mut words = Vec::new();
    let mut in_note = false;
    for line in vtt.lines().map(str::trim) {
        if line.is_empty() {
            in_note = false;
            continue;
        }
        if line.starts_with("WEBVTT") || line.starts_with("NOTE") {
            in_note = line.starts_with("NOTE");
            continue;
        }
        if in_note || line.contains("-->") || line.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        words.push(line);
    }
    words.join(" ")
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(180)))
        .build()
        .into()
}

/// Checks a key without spending anything: the models list needs only a valid key.
pub fn check_key(key: &str) -> Result<(), String> {
    let mut resp = agent()
        .get(&format!("{API}/models?limit=1"))
        .header("x-api-key", key)
        .header("anthropic-version", VERSION)
        .call()
        .map_err(|e| tr!("no se pudo conectar con Anthropic: {e}", e))?;
    let status = resp.status().as_u16();
    if status == 200 {
        return Ok(());
    }
    let body: Value = resp.body_mut().read_json().unwrap_or_default();
    Err(api_error(status, &body).message)
}

fn ask(key: &str, text: &str) -> Result<String, TitleError> {
    let body = json!({
        "model": MODEL,
        "max_tokens": 4000,
        "system": SYSTEM,
        "output_config": {
            "effort": "low",
            "format": {
                "type": "json_schema",
                "schema": {
                    "type": "object",
                    "properties": { "title": { "type": "string" } },
                    "required": ["title"],
                    "additionalProperties": false
                }
            }
        },
        "messages": [{ "role": "user", "content": format!("<transcript>\n{text}\n</transcript>") }]
    });
    let mut resp = agent()
        .post(&format!("{API}/messages"))
        .header("x-api-key", key)
        .header("anthropic-version", VERSION)
        .send_json(&body)
        .map_err(|e| tr!("no se pudo conectar con Anthropic: {e}", e))?;
    let status = resp.status().as_u16();
    let reply: Value = resp.body_mut().read_json().map_err(|e| tr!("respuesta de Anthropic ilegible: {e}", e))?;
    if status != 200 {
        return Err(api_error(status, &reply));
    }
    if reply["stop_reason"] != "end_turn" {
        return Err(tr!("Anthropic no devolvió un título ({reason})", reason = reply["stop_reason"]).into());
    }
    let text = reply["content"]
        .as_array()
        .and_then(|blocks| blocks.iter().find(|b| b["type"] == "text"))
        .and_then(|b| b["text"].as_str())
        .unwrap_or_default();
    let title: Value = serde_json::from_str(text).map_err(|e| tr!("respuesta de Anthropic ilegible: {e}", e))?;
    Ok(clean(title["title"].as_str().unwrap_or_default()).ok_or_else(|| t("Anthropic devolvió un título vacío"))?)
}

fn api_error(status: u16, body: &Value) -> TitleError {
    let kind = body["error"]["type"].as_str().unwrap_or_default();
    let message = body["error"]["message"].as_str().unwrap_or_default();
    let account = match (status, kind) {
        (401, _) | (_, "authentication_error") => {
            Some(t("la clave de Anthropic no es válida: cámbiala en Ajustes › Transcripción"))
        }
        (403, _) | (_, "permission_error") => Some(t("la clave de Anthropic no tiene permiso para usar la API")),
        (_, "billing_error") => Some(t("la cuenta de Anthropic se ha quedado sin créditos")),
        _ if message.to_ascii_lowercase().contains("credit balance") => {
            Some(t("la cuenta de Anthropic se ha quedado sin créditos"))
        }
        _ => None,
    };
    match account {
        Some(message) => TitleError { message, account: true },
        None => tr!("Anthropic respondió {status}: {message}", status, message).into(),
    }
}

/// One line, no wrapping quotes or trailing period, at most 120 characters.
fn clean(title: &str) -> Option<String> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = title.trim_matches(|c: char| matches!(c, '"' | '“' | '”' | '«' | '»' | '\'') || c.is_whitespace());
    let title = title.trim_end_matches(|c: char| matches!(c, '.' | '"' | '”' | '»' | '\'') || c.is_whitespace());
    let title: String = title.chars().take(120).collect();
    (!title.is_empty()).then_some(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_spoken_text() {
        let vtt = "WEBVTT\n\nNOTE made by whisper\nstill a note\n\n1\n00:00:00.000 --> 00:00:02.000\nHola a todos,\n\n2\n00:00:02.000 --> 00:00:04.500\nhoy os enseño R2.\n";
        assert_eq!(transcript(vtt), "Hola a todos, hoy os enseño R2.");
    }

    #[test]
    fn cleans_titles() {
        assert_eq!(clean("  «Configurar R2\n para Vroom».  ").as_deref(), Some("Configurar R2 para Vroom"));
        assert_eq!(clean(" \"\" "), None);
        assert_eq!(clean(&"a".repeat(300)).map(|t| t.chars().count()), Some(120));
    }

    #[test]
    fn tells_account_problems_apart() {
        let credits = json!({"type": "error", "error": {"type": "invalid_request_error",
            "message": "Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing to upgrade or purchase credits."}});
        assert!(api_error(400, &credits).account);
        assert!(api_error(400, &json!({"error": {"type": "billing_error", "message": "x"}})).account);
        assert!(
            api_error(401, &json!({"error": {"type": "authentication_error", "message": "invalid x-api-key"}})).account
        );
        let overloaded = api_error(529, &json!({"error": {"type": "overloaded_error", "message": "Overloaded"}}));
        assert!(!overloaded.account && overloaded.message.contains("529"));
        assert!(!api_error(400, &json!({"error": {"type": "invalid_request_error", "message": "bad"}})).account);
    }

    #[test]
    fn knows_default_titles() {
        let offset = time::UtcOffset::from_hms(1, 0, 0).unwrap();
        let at = time::OffsetDateTime::from_unix_timestamp(1_793_626_320).unwrap().to_offset(offset);
        let mut meta = Meta::new("x".into(), default_title_in("en", at), at);
        assert!(is_default_title(&meta));
        meta.title = default_title_in("es", at);
        assert!(is_default_title(&meta));
        meta.title = "Demo de LLM Pulse".into();
        assert!(!is_default_title(&meta));
    }
}
