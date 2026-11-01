//! The S3 calls Vroom needs, with retries. Every request is a presigned URL, so
//! nothing here depends on how the HTTP client builds headers.

use rusty_s3::actions::{
    AbortMultipartUpload, CompleteMultipartUpload, CreateMultipartUpload, DeleteObject, GetObject, ListObjectsV2,
    PutObject, UploadPart,
};
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;
use vroom_core::i18n::t;
use vroom_core::library::Multipart;
use vroom_core::tr;
use vroom_core::R2Config;

const SIGN_FOR: Duration = Duration::from_secs(15 * 60);
/// Share links without a public bucket URL. S3 allows at most 7 days.
pub const MAX_PRESIGN: Duration = Duration::from_secs(7 * 24 * 3600);
const MIN_PART: u64 = 8 * 1024 * 1024;
const MAX_PARTS: u64 = 10_000;
const ATTEMPTS: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub key: String,
    pub size: u64,
    pub last_modified: Option<String>,
}

pub struct R2 {
    bucket: Bucket,
    credentials: Credentials,
    agent: ureq::Agent,
    pub prefix: String,
    pub public_url: Option<String>,
    /// First retry delay; it doubles on each attempt.
    pub retry_base: Duration,
    /// `<prefix>library/<secret>/`, found or made once per process.
    pub private: std::sync::OnceLock<String>,
}

impl R2 {
    pub fn new(cfg: &R2Config, secret: &str) -> Result<R2, String> {
        cfg.validate()?;
        let endpoint = cfg.endpoint().parse().map_err(|e| format!("endpoint: {e}"))?;
        let bucket = Bucket::new(endpoint, UrlStyle::Path, cfg.bucket.clone(), "auto").map_err(|e| e.to_string())?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(120)))
            .timeout_per_read(Some(Duration::from_secs(60)))
            .timeout_per_write(Some(Duration::from_secs(60)))
            .user_agent(concat!("vroom/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Ok(R2 {
            bucket,
            credentials: Credentials::new(cfg.access_key_id.clone(), secret.to_string()),
            agent,
            prefix: cfg.prefix.clone(),
            public_url: cfg.public_url.as_ref().map(|u| u.trim_end_matches('/').to_string()),
            retry_base: Duration::from_millis(500),
            private: std::sync::OnceLock::new(),
        })
    }

    /// Credentials and bucket work: list one key.
    pub fn check(&self) -> Result<(), String> {
        let mut list = ListObjectsV2::new(&self.bucket, Some(&self.credentials));
        list.with_max_keys(1);
        let url = list.sign(SIGN_FOR);
        let (status, body) = self.call(|| self.agent.get(url.as_str()).call())?;
        expect_ok(status, &body)
    }

    pub fn put(&self, key: &str, bytes: &[u8], content_type: &str) -> Result<(), String> {
        let mut action = PutObject::new(&self.bucket, Some(&self.credentials), key);
        action.headers_mut().insert("content-type", content_type.to_string());
        let url = action.sign(SIGN_FOR);
        let (status, body) =
            self.call(|| self.agent.put(url.as_str()).header("content-type", content_type).send(bytes))?;
        expect_ok(status, &body)
    }

    /// `None` when the object does not exist.
    pub fn get_bytes(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let url = GetObject::new(&self.bucket, Some(&self.credentials), key).sign(SIGN_FOR);
        let (status, body) = self.call(|| self.agent.get(url.as_str()).call())?;
        if status == 404 {
            return Ok(None);
        }
        expect_ok(status, &body)?;
        Ok(Some(body))
    }

    /// Stream an object into `path` (through a temp file, renamed when complete).
    pub fn download(&self, key: &str, path: &Path, mut progress: impl FnMut(u64, u64)) -> Result<(), String> {
        let tmp = path.with_extension("part");
        let mut last_err = String::new();
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(self.retry_base * 2u32.pow(attempt - 1));
            }
            let url = GetObject::new(&self.bucket, Some(&self.credentials), key).sign(SIGN_FOR);
            let resp = match self.agent.get(url.as_str()).call() {
                Ok(r) => r,
                Err(e) => {
                    last_err = e.to_string();
                    continue;
                }
            };
            let status = resp.status().as_u16();
            if status != 200 {
                let body = resp.into_body().read_to_vec().unwrap_or_default();
                let err = s3_error(status, &body);
                if status >= 500 {
                    last_err = err;
                    continue;
                }
                return Err(err);
            }
            let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()?.parse().ok()).unwrap_or(0);
            match copy_to_file(resp.into_body().into_reader(), &tmp, total, &mut progress) {
                Ok(()) => return std::fs::rename(&tmp, path).map_err(|e| e.to_string()),
                Err(e) => last_err = e,
            }
        }
        let _ = std::fs::remove_file(&tmp);
        Err(last_err)
    }

    pub fn list(&self, prefix: &str) -> Result<Vec<Object>, String> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        // A million keys is far beyond any library; stop rather than loop forever.
        for _ in 0..1000 {
            let mut list = ListObjectsV2::new(&self.bucket, Some(&self.credentials));
            list.with_prefix(prefix.to_string());
            if let Some(t) = &token {
                list.with_continuation_token(t.clone());
            }
            let url = list.sign(SIGN_FOR);
            let (status, body) = self.call(|| self.agent.get(url.as_str()).call())?;
            expect_ok(status, &body)?;
            let text = String::from_utf8_lossy(&body);
            let parsed = ListObjectsV2::parse_response(&text).map_err(|e| tr!("respuesta de R2: {e}", e))?;
            out.extend(parsed.contents.into_iter().map(|c| Object {
                key: c.key,
                size: c.size.unwrap_or(0),
                last_modified: c.last_modified,
            }));
            match parsed.next_continuation_token {
                Some(t) if !t.is_empty() => token = Some(t),
                _ => return Ok(out),
            }
        }
        Err(t("R2 devolvió demasiadas páginas al listar"))
    }

    pub fn delete(&self, key: &str) -> Result<(), String> {
        let url = DeleteObject::new(&self.bucket, Some(&self.credentials), key).sign(SIGN_FOR);
        let (status, body) = self.call(|| self.agent.delete(url.as_str()).call())?;
        if status == 404 {
            return Ok(());
        }
        expect_ok(status, &body)
    }

    pub fn abort_multipart(&self, key: &str, upload_id: &str) {
        let url = AbortMultipartUpload::new(&self.bucket, Some(&self.credentials), key, upload_id).sign(SIGN_FOR);
        let _ = self.call(|| self.agent.delete(url.as_str()).call());
    }

    /// Upload a file in parts, resuming `resume` when it belongs to the same file.
    /// `save` is called after every part so a crash loses at most one part.
    pub fn upload_file(
        &self,
        key: &str,
        path: &Path,
        content_type: &str,
        resume: Option<Multipart>,
        mut save: impl FnMut(Option<&Multipart>) -> Result<(), String>,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<(), String> {
        let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let size = file.metadata().map_err(|e| e.to_string())?.len();
        let part_size = part_size(size);

        if size <= part_size {
            if let Some(old) = resume {
                self.abort_multipart(key, &old.upload_id);
            }
            let mut bytes = Vec::with_capacity(size as usize);
            file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
            self.put(key, &bytes, content_type)?;
            progress(size, size);
            return save(None);
        }

        let mut mp = match resume {
            Some(m) if m.file_size == size && m.part_size == part_size => m,
            other => {
                if let Some(old) = other {
                    self.abort_multipart(key, &old.upload_id);
                }
                let mut create = CreateMultipartUpload::new(&self.bucket, Some(&self.credentials), key);
                create.headers_mut().insert("content-type", content_type.to_string());
                let url = create.sign(SIGN_FOR);
                let (status, body) =
                    self.call(|| self.agent.post(url.as_str()).header("content-type", content_type).send_empty())?;
                expect_ok(status, &body)?;
                let parsed = CreateMultipartUpload::parse_response(&String::from_utf8_lossy(&body))
                    .map_err(|e| tr!("respuesta de R2: {e}", e))?;
                let mp = Multipart {
                    upload_id: parsed.upload_id().to_string(),
                    part_size,
                    file_size: size,
                    etags: Vec::new(),
                };
                save(Some(&mp))?;
                mp
            }
        };

        let parts = size.div_ceil(part_size);
        let mut buf = vec![0u8; part_size as usize];
        progress(mp.uploaded_bytes(), size);
        for n in mp.etags.len() as u64..parts {
            let offset = n * part_size;
            let len = part_size.min(size - offset) as usize;
            file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
            file.read_exact(&mut buf[..len]).map_err(|e| e.to_string())?;
            let part_number = u16::try_from(n + 1).map_err(|_| t("demasiadas partes"))?;
            let url =
                UploadPart::new(&self.bucket, Some(&self.credentials), key, part_number, &mp.upload_id).sign(SIGN_FOR);
            let mut etag = None;
            let (status, body) = self.call(|| {
                let resp = self.agent.put(url.as_str()).send(&buf[..len])?;
                etag = resp.headers().get("etag").and_then(|v| v.to_str().ok()).map(str::to_string);
                Ok(resp)
            })?;
            if status == 404 {
                // The upload expired or was aborted elsewhere: start over next time.
                save(None)?;
                return Err(t("la subida a medias caducó en R2; se reiniciará"));
            }
            expect_ok(status, &body)?;
            mp.etags.push(etag.ok_or_else(|| t("R2 no devolvió ETag"))?);
            save(Some(&mp))?;
            progress(mp.uploaded_bytes(), size);
        }

        let complete = CompleteMultipartUpload::new(
            &self.bucket,
            Some(&self.credentials),
            key,
            &mp.upload_id,
            mp.etags.iter().map(String::as_str),
        );
        let url = complete.sign(SIGN_FOR);
        let body = CompleteMultipartUpload::new(
            &self.bucket,
            Some(&self.credentials),
            key,
            &mp.upload_id,
            mp.etags.iter().map(String::as_str),
        )
        .body();
        let (status, resp) = self.call(|| self.agent.post(url.as_str()).send(body.as_bytes()))?;
        expect_ok(status, &resp)?;
        // R2 can answer 200 with an error document, as S3 does.
        if String::from_utf8_lossy(&resp).contains("<Error>") {
            return Err(s3_error(500, &resp));
        }
        save(None)
    }

    pub fn presign_get(&self, key: &str, valid_for: Duration) -> String {
        GetObject::new(&self.bucket, Some(&self.credentials), key).sign(valid_for).to_string()
    }

    /// Public URL of a key when the bucket has one.
    pub fn public(&self, key: &str) -> Option<String> {
        self.public_url.as_ref().map(|base| format!("{base}/{key}"))
    }

    /// Run a request with retries on network errors and 5xx; returns status and body.
    fn call(
        &self,
        mut send: impl FnMut() -> Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<(u16, Vec<u8>), String> {
        let mut last_err = String::new();
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(self.retry_base * 2u32.pow(attempt - 1));
            }
            match send() {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let body = resp.into_body().with_config().limit(64 * 1024 * 1024).read_to_vec();
                    match body {
                        Ok(body) if status >= 500 || status == 429 => last_err = s3_error(status, &body),
                        Ok(body) => return Ok((status, body)),
                        Err(e) => last_err = e.to_string(),
                    }
                }
                Err(e) => last_err = describe(&e),
            }
        }
        Err(last_err)
    }
}

fn describe(e: &ureq::Error) -> String {
    match e {
        ureq::Error::Io(io) => tr!("sin conexión con R2: {io}", io),
        ureq::Error::Timeout(_) => t("R2 tardó demasiado en responder"),
        other => other.to_string(),
    }
}

/// 8 MiB, or larger for files that would need more than 10,000 parts.
pub fn part_size(size: u64) -> u64 {
    let needed = size.div_ceil(MAX_PARTS);
    needed.max(MIN_PART).next_multiple_of(1024 * 1024)
}

fn expect_ok(status: u16, body: &[u8]) -> Result<(), String> {
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(s3_error(status, body))
    }
}

/// `<Code>` and `<Message>` from an S3 error document, in words a person can act on.
pub fn s3_error(status: u16, body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let tag = |name: &str| {
        let open = format!("<{name}>");
        let start = text.find(&open)? + open.len();
        let end = text[start..].find(&format!("</{name}>"))? + start;
        Some(text[start..end].to_string())
    };
    let code = tag("Code").unwrap_or_default();
    let hint = match (status, code.as_str()) {
        (_, "NoSuchBucket") => t("el bucket no existe"),
        (_, "InvalidAccessKeyId") => t("el Access Key ID no es válido"),
        (_, "SignatureDoesNotMatch") => t("el Secret Access Key no coincide"),
        (401 | 403, _) => t("las credenciales no tienen acceso a este bucket"),
        _ => String::new(),
    };
    let message = tag("Message").unwrap_or_default();
    match (hint.is_empty(), code.is_empty()) {
        (false, _) => format!("R2 {status}: {hint}"),
        (true, false) => format!("R2 {status} {code}: {message}"),
        (true, true) => tr!("R2 respondió {status}", status),
    }
}

fn copy_to_file(
    mut reader: impl Read,
    path: &Path,
    total: u64,
    progress: &mut impl FnMut(u64, u64),
) -> Result<(), String> {
    let mut out = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    let mut reported = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if done - reported >= 4 * 1024 * 1024 {
            progress(done, total);
            reported = done;
        }
    }
    out.sync_all().map_err(|e| e.to_string())?;
    if total > 0 && done != total {
        return Err(tr!("descarga incompleta ({done} de {total} bytes)", done, total));
    }
    progress(done, total.max(done));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_sizes() {
        assert_eq!(part_size(1), MIN_PART);
        assert_eq!(part_size(70 * 1024 * 1024 * 1024), 8 * 1024 * 1024);
        let huge = 200 * 1024 * 1024 * 1024u64;
        assert!(huge.div_ceil(part_size(huge)) <= MAX_PARTS);
    }

    #[test]
    fn errors_are_readable() {
        vroom_core::i18n::set_lang("es");
        let body = b"<?xml version=\"1.0\"?><Error><Code>NoSuchBucket</Code><Message>x</Message></Error>";
        assert_eq!(s3_error(404, body), "R2 404: el bucket no existe");
        assert_eq!(s3_error(400, b"<Error><Code>Weird</Code><Message>m</Message></Error>"), "R2 400 Weird: m");
        assert_eq!(s3_error(502, b""), "R2 respondió 502");
    }

    #[test]
    fn signs_against_the_account_endpoint() {
        let cfg = R2Config {
            account_id: "0123456789abcdef0123456789abcdef".into(),
            bucket: "vroom".into(),
            access_key_id: "AKID".into(),
            public_url: Some("https://v.example.com/".into()),
            prefix: String::new(),
            jurisdiction: None,
            endpoint: None,
        };
        let r2 = R2::new(&cfg, "secret").unwrap();
        let url = r2.presign_get("recordings/abcdefghij/video.mp4", MAX_PRESIGN);
        assert!(url.starts_with(
            "https://0123456789abcdef0123456789abcdef.r2.cloudflarestorage.com/vroom/recordings/abcdefghij/video.mp4?"
        ));
        assert!(url.contains("X-Amz-Expires=604800"));
        assert_eq!(r2.public("k").as_deref(), Some("https://v.example.com/k"));
    }
}
