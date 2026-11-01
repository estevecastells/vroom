//! Moving recordings between the library and R2: upload, restore on a new
//! computer, download a cloud-only recording, delete, and share links.

use crate::r2::{MAX_PRESIGN, R2};
use std::collections::BTreeSet;
use time::OffsetDateTime;
use vroom_core::layout::{self, AFTER_VIDEO, META, PAGE, THUMB, VIDEO};
use vroom_core::library::{Library, State};
use vroom_core::share;

/// Upload everything of one recording that is not in R2 yet: the video (resuming
/// a previous attempt), then the thumbnail, captions, share page and `meta.json`.
pub fn upload(r2: &R2, lib: &Library, id: &str, progress: impl FnMut(u64, u64)) -> Result<(), String> {
    let dir = lib.dir(id)?;
    let mut state = lib.state(id);
    let result = upload_inner(r2, lib, id, &dir, &mut state, progress);
    // Re-read: `upload_inner` saved multipart progress as it went.
    let mut latest = lib.state(id);
    latest.last_error = result.as_ref().err().cloned();
    if result.is_ok() {
        latest.uploaded_at = state.uploaded_at;
        latest.meta_dirty = false;
        latest.multipart = None;
    }
    lib.write_state(id, &latest)?;
    result
}

fn upload_inner(
    r2: &R2,
    lib: &Library,
    id: &str,
    dir: &std::path::Path,
    state: &mut State,
    progress: impl FnMut(u64, u64),
) -> Result<(), String> {
    let meta = lib.meta(id)?;
    let video = dir.join(VIDEO);
    let video_needed = state.uploaded_at.is_none();
    if video_needed {
        if !video.is_file() {
            return Err("no hay vídeo local que subir".into());
        }
        let key = layout::key(&r2.prefix, id, VIDEO);
        r2.upload_file(
            &key,
            &video,
            layout::content_type(VIDEO),
            state.multipart.clone(),
            |mp| {
                let mut s = lib.state(id);
                s.multipart = mp.cloned();
                lib.write_state(id, &s)
            },
            progress,
        )?;
    }
    let has_thumb = dir.join(THUMB).is_file();
    let has_captions = dir.join(layout::CAPTIONS).is_file();
    for file in AFTER_VIDEO {
        let bytes = match file {
            PAGE => share::page(&meta, has_thumb, has_captions).into_bytes(),
            META => serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?,
            // Thumbnail and captions only change with the video.
            _ if !video_needed => continue,
            _ => match std::fs::read(dir.join(file)) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.to_string()),
            },
        };
        r2.put(&layout::key(&r2.prefix, id, file), &bytes, layout::content_type(file))?;
    }
    if video_needed {
        state.uploaded_at = Some(OffsetDateTime::now_utc());
    }
    Ok(())
}

/// What a restore found.
#[derive(Debug, Default)]
pub struct Restored {
    pub added: Vec<String>,
    pub already_here: usize,
}

/// Rebuild the library from R2: every recording whose `meta.json` is in the
/// bucket and not on this computer comes back as a cloud-only entry, with its
/// thumbnail. Videos stay in R2 until downloaded.
pub fn restore(r2: &R2, lib: &Library, mut added: impl FnMut(&str)) -> Result<Restored, String> {
    let objects = r2.list(&layout::recordings_prefix(&r2.prefix))?;
    let ids: BTreeSet<&str> = objects
        .iter()
        .filter(|o| o.key.ends_with(&format!("/{META}")))
        .filter_map(|o| layout::id_from_key(&r2.prefix, &o.key))
        .collect();
    let mut out = Restored::default();
    for id in ids {
        if lib.contains(id) {
            out.already_here += 1;
            continue;
        }
        let Some(bytes) = r2.get_bytes(&layout::key(&r2.prefix, id, META))? else { continue };
        let meta: vroom_core::Meta =
            serde_json::from_slice(&bytes).map_err(|e| format!("meta.json de {id} no es válido: {e}"))?;
        if meta.id != id {
            continue;
        }
        lib.write_meta(&meta)?;
        let dir = lib.dir(id)?;
        if objects.iter().any(|o| o.key == layout::key(&r2.prefix, id, THUMB)) {
            if let Some(thumb) = r2.get_bytes(&layout::key(&r2.prefix, id, THUMB))? {
                std::fs::write(dir.join(THUMB), thumb).map_err(|e| e.to_string())?;
            }
        }
        lib.write_state(id, &State { uploaded_at: Some(OffsetDateTime::now_utc()), ..State::default() })?;
        added(id);
        out.added.push(id.to_string());
    }
    Ok(out)
}

/// Bring a cloud-only recording's video back to this computer.
pub fn download(r2: &R2, lib: &Library, id: &str, progress: impl FnMut(u64, u64)) -> Result<(), String> {
    let dir = lib.dir(id)?;
    if dir.join(VIDEO).is_file() {
        return Ok(());
    }
    r2.download(&layout::key(&r2.prefix, id, VIDEO), &dir.join(VIDEO), progress)
}

/// Remove every object of a recording from R2 (and any half-finished upload).
pub fn delete_remote(r2: &R2, lib: &Library, id: &str) -> Result<(), String> {
    if !vroom_core::id::is_valid(id) {
        return Err(format!("id no válido: {id}"));
    }
    let state = lib.state(id);
    if let Some(mp) = &state.multipart {
        r2.abort_multipart(&layout::key(&r2.prefix, id, VIDEO), &mp.upload_id);
    }
    for o in r2.list(&format!("{}{id}/", layout::recordings_prefix(&r2.prefix)))? {
        r2.delete(&o.key)?;
    }
    Ok(())
}

/// A link to watch the recording: the share page on the public URL when the
/// bucket has one (never expires), else the video signed for 7 days.
pub fn share_link(r2: &R2, id: &str) -> (String, Option<i64>) {
    if let Some(url) = r2.public(&layout::key(&r2.prefix, id, PAGE)) {
        return (url, None);
    }
    let url = r2.presign_get(&layout::key(&r2.prefix, id, VIDEO), MAX_PRESIGN);
    let expires = OffsetDateTime::now_utc().unix_timestamp() + MAX_PRESIGN.as_secs() as i64;
    (url, Some(expires))
}

/// A short-lived URL the app can stream a cloud-only recording from.
pub fn stream_url(r2: &R2, id: &str) -> String {
    r2.public(&layout::key(&r2.prefix, id, VIDEO)).unwrap_or_else(|| {
        r2.presign_get(&layout::key(&r2.prefix, id, VIDEO), std::time::Duration::from_secs(6 * 3600))
    })
}
