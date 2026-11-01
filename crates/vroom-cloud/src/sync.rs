//! Moving recordings between the library and R2: upload, restore on a new
//! computer, download a cloud-only recording, delete, and share links.

use crate::r2::{MAX_PRESIGN, R2};
use std::collections::BTreeSet;
use time::OffsetDateTime;
use vroom_core::i18n::t;
use vroom_core::layout::{self, AFTER_VIDEO, CAPTIONS, META, PAGE, THUMB, VIDEO};
use vroom_core::library::{captions_hash, Library, State};
use vroom_core::{share, tr};

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
        latest.meta_private = true;
        latest.meta_dirty = false;
        latest.multipart = None;
        latest.captions_uploaded = state.captions_uploaded.or(latest.captions_uploaded);
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
    let first_private = !state.meta_private;
    if video_needed {
        if !video.is_file() {
            return Err(t("no hay vídeo local que subir"));
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
    let has_captions = dir.join(CAPTIONS).is_file();
    let captions_needed = video_needed || lib.captions_pending(id);
    for file in AFTER_VIDEO {
        let bytes = match file {
            PAGE => share::page(&meta, has_thumb, has_captions).into_bytes(),
            // The thumbnail only changes with the video; captions also with a new transcription.
            THUMB if !video_needed => continue,
            CAPTIONS if !captions_needed => continue,
            _ => match std::fs::read(dir.join(file)) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.to_string()),
            },
        };
        r2.put(&layout::key(&r2.prefix, id, file), &bytes, layout::content_type(file))?;
        if file == CAPTIONS {
            state.captions_uploaded = Some(captions_hash(&bytes));
        }
    }
    // Last: the private metadata marks the upload complete.
    let json = serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?;
    r2.put(&layout::private_meta_key(&private_dir(r2)?, id), &json, "application/json")?;
    if first_private {
        // Older versions published meta.json next to the video.
        r2.delete(&layout::key(&r2.prefix, id, META))?;
    }
    state.meta_private = true;
    if video_needed {
        state.uploaded_at = Some(OffsetDateTime::now_utc());
    }
    Ok(())
}

/// `<prefix>library/<secret>/`: the existing private area, or a new random one.
pub fn private_dir(r2: &R2) -> Result<String, String> {
    if let Some(p) = r2.private.get() {
        return Ok(p.clone());
    }
    let root = layout::private_root(&r2.prefix);
    let mut found = None;
    let mut legacy_folders = None;
    for o in r2.list(&root)? {
        let rest = &o.key[root.len()..];
        match rest.split_once('/') {
            Some((secret, _)) if secret.len() >= 20 && vroom_core::id::is_valid(secret) => {
                found = Some(secret.to_string());
                break;
            }
            // Before the private area, the folder list sat at library/folders-<random>.json.
            _ if rest.starts_with("folders-") => legacy_folders = Some(o.key.clone()),
            _ => {}
        }
    }
    let dir = format!("{root}{}/", found.unwrap_or_else(vroom_core::id::generate));
    if let Some(old) = legacy_folders {
        if let Some(bytes) = r2.get_bytes(&old)? {
            r2.put(&format!("{dir}{}", layout::FOLDERS), &bytes, "application/json")?;
        }
        r2.delete(&old)?;
    }
    Ok(r2.private.get_or_init(|| dir).clone())
}

fn folders_key(r2: &R2) -> Result<String, String> {
    Ok(format!("{}{}", private_dir(r2)?, layout::FOLDERS))
}

/// Upload the hand-made folder list when it changed since the last upload.
pub fn push_folders(r2: &R2, lib: &Library) -> Result<bool, String> {
    let local = serde_json::to_vec_pretty(&vroom_core::library::FolderList { folders: lib.saved_folders() })
        .map_err(|e| e.to_string())?;
    let marker = lib.root().join(".folders.uploaded");
    if std::fs::read(&marker).ok().as_deref() == Some(local.as_slice()) {
        return Ok(false);
    }
    r2.put(&folders_key(r2)?, &local, "application/json")?;
    std::fs::write(&marker, &local).map_err(|e| e.to_string())?;
    Ok(true)
}

fn pull_folders(r2: &R2, lib: &Library) -> Result<(), String> {
    let key = folders_key(r2)?;
    if let Some(bytes) = r2.get_bytes(&key)? {
        if let Ok(list) = serde_json::from_slice::<vroom_core::library::FolderList>(&bytes) {
            lib.merge_folders(&list.folders)?;
        }
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
    pull_folders(r2, lib)?;
    let private = private_dir(r2)?;
    let objects = r2.list(&layout::recordings_prefix(&r2.prefix))?;
    let private_ids: BTreeSet<String> = r2
        .list(&format!("{private}meta/"))?
        .iter()
        .filter_map(|o| layout::id_from_private_meta(&private, &o.key).map(str::to_string))
        .collect();
    // Uploads from before the private area kept meta.json next to the video.
    let legacy_ids: BTreeSet<String> = objects
        .iter()
        .filter(|o| o.key.ends_with(&format!("/{META}")))
        .filter_map(|o| layout::id_from_key(&r2.prefix, &o.key).map(str::to_string))
        .collect();
    let ids: BTreeSet<&String> = private_ids.iter().chain(legacy_ids.iter()).collect();
    let mut out = Restored::default();
    for id in ids {
        if lib.contains(id) {
            out.already_here += 1;
            continue;
        }
        let is_private = private_ids.contains(id);
        let key = if is_private { layout::private_meta_key(&private, id) } else { layout::key(&r2.prefix, id, META) };
        let Some(bytes) = r2.get_bytes(&key)? else { continue };
        let meta: vroom_core::Meta =
            serde_json::from_slice(&bytes).map_err(|e| tr!("meta.json de {id} no es válido: {e}", id, e))?;
        let id = id.as_str();
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
        lib.write_state(
            id,
            &State { uploaded_at: Some(OffsetDateTime::now_utc()), meta_private: is_private, ..State::default() },
        )?;
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
        return Err(tr!("id no válido: {id}", id));
    }
    let state = lib.state(id);
    if let Some(mp) = &state.multipart {
        r2.abort_multipart(&layout::key(&r2.prefix, id, VIDEO), &mp.upload_id);
    }
    for o in r2.list(&format!("{}{id}/", layout::recordings_prefix(&r2.prefix)))? {
        r2.delete(&o.key)?;
    }
    r2.delete(&layout::private_meta_key(&private_dir(r2)?, id))
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
