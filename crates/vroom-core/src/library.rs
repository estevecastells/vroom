//! The library is a plain folder: one subfolder per recording, named by its id,
//! with `video.mp4`, `meta.json`, `thumb.jpg` and the local `state.json`.
//! Anything can be copied, backed up or opened in Finder/Explorer as is.

use crate::layout::{CAPTIONS, META, STATE, THUMB, VIDEO};
use crate::meta::Meta;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

/// Present while a process is still writing the video; holds that process's id.
pub const RECORDING_MARKER: &str = ".recording";

/// Mark a folder as being written by process `pid`.
pub fn mark_in_progress(dir: &Path, pid: u32) -> std::io::Result<()> {
    std::fs::write(dir.join(RECORDING_MARKER), pid.to_string())
}

/// Whether the marker's owner may still be writing. An empty or unreadable
/// marker (older versions) counts as abandoned.
fn owner_alive(dir: &Path) -> bool {
    let Some(pid) = std::fs::read_to_string(dir.join(RECORDING_MARKER)).ok().and_then(|s| s.trim().parse::<u32>().ok())
    else {
        return false;
    };
    process_alive(pid)
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else { return false };
    if pid <= 0 {
        return false;
    }
    // Signal 0 checks existence; EPERM means it exists but belongs to someone else.
    let exists = unsafe { libc::kill(pid, 0) } == 0;
    exists || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    if pid == 0 {
        return false;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            // Gone, or not ours to open: access denied still means it exists.
            return std::io::Error::last_os_error().raw_os_error() == Some(ERROR_ACCESS_DENIED as i32);
        }
        let mut code = 0u32;
        let alive = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32;
        CloseHandle(handle);
        alive
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The app is writing the video right now.
    Recording,
    /// Only on this computer.
    Local,
    /// Part of the video is in R2; the upload resumes where it stopped.
    Uploading,
    /// On this computer and in R2.
    Synced,
    /// Only in R2 (restored on a new computer, or the local copy was freed).
    Cloud,
}

/// Upload bookkeeping, local to this computer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    #[serde(with = "time::serde::rfc3339::option", skip_serializing_if = "Option::is_none")]
    pub uploaded_at: Option<OffsetDateTime>,
    /// Title or description changed since the upload: `meta.json` and the page go up again.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub meta_dirty: bool,
    /// `meta.json` is in the bucket's private area (older uploads put it next to the video).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub meta_private: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multipart: Option<Multipart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// `captions_hash` of the captions.vtt in R2; a new transcription differs and goes up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captions_uploaded: Option<u64>,
}

/// FNV-1a: stable across builds, enough to notice that a file changed.
pub fn captions_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

/// A multipart upload in progress, so it can resume after a crash or a reboot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Multipart {
    pub upload_id: String,
    pub part_size: u64,
    /// The video's size when the upload started; a different size starts over.
    pub file_size: u64,
    /// ETags of the parts already uploaded, in order.
    pub etags: Vec<String>,
}

impl Multipart {
    pub fn uploaded_bytes(&self) -> u64 {
        (self.etags.len() as u64 * self.part_size).min(self.file_size)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    #[serde(flatten)]
    pub meta: Meta,
    pub status: Status,
    pub dir: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captions: Option<PathBuf>,
    #[serde(default, with = "time::serde::rfc3339::option", skip_serializing_if = "Option::is_none")]
    pub uploaded_at: Option<OffsetDateTime>,
    /// 0…1 while uploading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct Library {
    root: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderList {
    #[serde(default)]
    pub folders: Vec<String>,
}

/// `a/b/c` → `a`, `a/b`, `a/b/c`.
fn with_parents(path: &str) -> impl Iterator<Item = String> + '_ {
    path.match_indices('/').map(|(i, _)| path[..i].to_string()).chain(std::iter::once(path.to_string()))
}

fn is_within(folder: &str, root: &str) -> bool {
    folder == root || folder.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
}

impl Library {
    pub fn open(root: impl Into<PathBuf>) -> std::io::Result<Library> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Library { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn dir(&self, id: &str) -> Result<PathBuf, String> {
        if !crate::id::is_valid(id) {
            return Err(format!("id no válido: {id}"));
        }
        Ok(self.root.join(id))
    }

    fn existing_dir(&self, id: &str) -> Result<PathBuf, String> {
        let dir = self.dir(id)?;
        if !dir.join(META).is_file() {
            return Err(format!("no existe la grabación {id}"));
        }
        Ok(dir)
    }

    /// Start a recording: a folder with a provisional `meta.json` and the marker.
    /// `owner` is the process that will write the video (the app passes its own pid).
    pub fn create(&self, title: Option<String>, now: OffsetDateTime, owner: u32) -> Result<Entry, String> {
        let id = crate::id::generate();
        let dir = self.dir(&id)?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        mark_in_progress(&dir, owner).map_err(|e| e.to_string())?;
        let title = title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| crate::meta::default_title(now));
        self.write_meta(&Meta::new(id.clone(), title, now))?;
        self.entry(&id)
    }

    /// The app finished writing the video.
    pub fn finalize(&self, id: &str, duration_secs: f64, width: u32, height: u32) -> Result<Entry, String> {
        let dir = self.existing_dir(id)?;
        let mut meta = self.meta(id)?;
        meta.duration_secs = duration_secs;
        meta.width = width;
        meta.height = height;
        meta.size_bytes = file_size(&dir.join(VIDEO)).ok_or("no se encontró el vídeo")?;
        self.write_meta(&meta)?;
        let _ = std::fs::remove_file(dir.join(RECORDING_MARKER));
        self.entry(id)
    }

    /// After a crash: keep whatever video was written (fragmented MP4 plays up
    /// to the last fragment) and drop folders that never got one.
    pub fn recover(&self) -> Result<Vec<String>, String> {
        let mut recovered = Vec::new();
        for id in self.ids()? {
            let dir = self.root.join(&id);
            if !dir.join(RECORDING_MARKER).exists() || owner_alive(&dir) {
                continue;
            }
            if file_size(&dir.join(VIDEO)).unwrap_or(0) > 0 {
                let meta = self.meta(&id)?;
                self.finalize(&id, meta.duration_secs, meta.width, meta.height)?;
                recovered.push(id);
            } else {
                std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
            }
        }
        Ok(recovered)
    }

    pub fn meta(&self, id: &str) -> Result<Meta, String> {
        let path = self.dir(id)?.join(META);
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn write_meta(&self, meta: &Meta) -> Result<(), String> {
        let dir = self.dir(&meta.id)?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let json = serde_json::to_vec_pretty(meta).map_err(|e| e.to_string())?;
        write_atomic(&dir.join(META), &json).map_err(|e| e.to_string())
    }

    pub fn state(&self, id: &str) -> State {
        self.dir(id)
            .ok()
            .and_then(|d| std::fs::read(d.join(STATE)).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn write_state(&self, id: &str, state: &State) -> Result<(), String> {
        let path = self.dir(id)?.join(STATE);
        let json = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
        write_atomic(&path, &json).map_err(|e| e.to_string())
    }

    pub fn rename(&self, id: &str, title: &str) -> Result<Entry, String> {
        let title = title.trim();
        if title.is_empty() {
            return Err("el título no puede estar vacío".into());
        }
        let mut meta = self.meta(id)?;
        meta.title = title.to_string();
        self.write_meta(&meta)?;
        let mut state = self.state(id);
        if state.uploaded_at.is_some() {
            state.meta_dirty = true;
            self.write_state(id, &state)?;
        }
        self.entry(id)
    }

    /// Folders created by hand, which may be empty.
    pub fn saved_folders(&self) -> Vec<String> {
        std::fs::read(self.root.join(crate::layout::FOLDERS))
            .ok()
            .and_then(|b| serde_json::from_slice::<FolderList>(&b).ok())
            .map(|l| l.folders)
            .unwrap_or_default()
    }

    fn save_folders(&self, mut folders: Vec<String>) -> Result<(), String> {
        folders.sort();
        folders.dedup();
        let json = serde_json::to_vec_pretty(&FolderList { folders }).map_err(|e| e.to_string())?;
        write_atomic(&self.root.join(crate::layout::FOLDERS), &json).map_err(|e| e.to_string())
    }

    /// Every folder: the ones created by hand plus those holding recordings,
    /// with their parents, sorted.
    pub fn folders(&self) -> Result<Vec<String>, String> {
        let mut all: Vec<String> = self.saved_folders();
        for e in self.list()? {
            all.extend(e.meta.folder);
        }
        let mut out: Vec<String> = all.iter().flat_map(|f| with_parents(f).collect::<Vec<_>>()).collect();
        out.sort();
        out.dedup();
        Ok(out)
    }

    pub fn create_folder(&self, path: &str) -> Result<String, String> {
        let path = crate::meta::normalize_folder(path).ok_or("el nombre de la carpeta está vacío")?;
        let mut folders = self.saved_folders();
        folders.push(path.clone());
        self.save_folders(folders)?;
        Ok(path)
    }

    /// Merge folders from another copy of the library (a restore from R2).
    pub fn merge_folders(&self, more: &[String]) -> Result<(), String> {
        let mut folders = self.saved_folders();
        folders.extend(more.iter().filter_map(|f| crate::meta::normalize_folder(f)));
        self.save_folders(folders)
    }

    /// Rename or move a folder with its subfolders and recordings.
    pub fn rename_folder(&self, from: &str, to: &str) -> Result<String, String> {
        let from = crate::meta::normalize_folder(from).ok_or("falta la carpeta")?;
        let to = crate::meta::normalize_folder(to).ok_or("el nombre nuevo está vacío")?;
        if is_within(&to, &from) && to != from {
            return Err("no se puede mover una carpeta dentro de sí misma".into());
        }
        let moved = |f: &str| if is_within(f, &from) { format!("{to}{}", &f[from.len()..]) } else { f.to_string() };
        let folders: Vec<String> = self.saved_folders().iter().map(|f| moved(f)).chain([to.clone()]).collect();
        self.save_folders(folders)?;
        for e in self.list()? {
            if let Some(f) = e.meta.folder.as_deref().filter(|f| is_within(f, &from)) {
                self.set_folder(&e.meta.id, Some(&moved(f)))?;
            }
        }
        Ok(to)
    }

    /// Delete a folder and its subfolders; their recordings go to the deleted folder's parent.
    pub fn delete_folder(&self, path: &str) -> Result<(), String> {
        let path = crate::meta::normalize_folder(path).ok_or("falta la carpeta")?;
        let parent = path.rsplit_once('/').map(|(p, _)| p.to_string());
        self.save_folders(self.saved_folders().into_iter().filter(|f| !is_within(f, &path)).collect())?;
        for e in self.list()? {
            if e.meta.folder.as_deref().is_some_and(|f| is_within(f, &path)) {
                self.set_folder(&e.meta.id, parent.as_deref())?;
            }
        }
        Ok(())
    }

    /// Put a recording in a folder (`None` or empty: the top level).
    pub fn set_folder(&self, id: &str, folder: Option<&str>) -> Result<Entry, String> {
        let mut meta = self.meta(id)?;
        let folder = folder.and_then(crate::meta::normalize_folder);
        if meta.folder != folder {
            meta.folder = folder;
            self.write_meta(&meta)?;
            let mut state = self.state(id);
            if state.uploaded_at.is_some() {
                state.meta_dirty = true;
                self.write_state(id, &state)?;
            }
        }
        self.entry(id)
    }

    /// Free disk space: drop the local video of a recording that is safely in R2.
    pub fn remove_local_video(&self, id: &str) -> Result<Entry, String> {
        let dir = self.existing_dir(id)?;
        if self.state(id).uploaded_at.is_none() {
            return Err("esta grabación aún no está en la nube; no se borra la única copia".into());
        }
        match std::fs::remove_file(dir.join(VIDEO)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        self.entry(id)
    }

    /// Forget the recording on this computer (the cloud copy is the caller's business).
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let dir = self.dir(id)?;
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn ids(&self) -> Result<Vec<String>, String> {
        let rd = std::fs::read_dir(&self.root).map_err(|e| format!("{}: {e}", self.root.display()))?;
        Ok(rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| crate::id::is_valid(n))
            .collect())
    }

    pub fn contains(&self, id: &str) -> bool {
        self.dir(id).map(|d| d.join(META).is_file()).unwrap_or(false)
    }

    pub fn entry(&self, id: &str) -> Result<Entry, String> {
        let dir = self.existing_dir(id)?;
        let meta = self.meta(id)?;
        let state = self.state(id);
        let video = dir.join(VIDEO);
        let has_video = video.is_file();
        let status = if dir.join(RECORDING_MARKER).exists() {
            Status::Recording
        } else if state.uploaded_at.is_some() {
            if has_video {
                Status::Synced
            } else {
                Status::Cloud
            }
        } else if state.multipart.is_some() {
            Status::Uploading
        } else {
            Status::Local
        };
        let progress = match (&state.multipart, status) {
            (Some(m), Status::Uploading) if m.file_size > 0 => Some(m.uploaded_bytes() as f64 / m.file_size as f64),
            _ => None,
        };
        let thumb = dir.join(THUMB);
        let captions = dir.join(CAPTIONS);
        Ok(Entry {
            status,
            video: has_video.then_some(video),
            thumb: thumb.is_file().then_some(thumb),
            captions: captions.is_file().then_some(captions),
            uploaded_at: state.uploaded_at,
            progress,
            error: state.last_error,
            dir,
            meta,
        })
    }

    /// Newest first. Broken folders are skipped rather than failing the list.
    pub fn list(&self) -> Result<Vec<Entry>, String> {
        let mut entries: Vec<Entry> = self.ids()?.iter().filter_map(|id| self.entry(id).ok()).collect();
        entries.sort_by(|a, b| b.meta.created_at.cmp(&a.meta.created_at).then_with(|| a.meta.id.cmp(&b.meta.id)));
        Ok(entries)
    }

    /// Recordings whose video or metadata still has to go up.
    /// The recording is in R2 but its local captions.vtt is newer than the one there.
    pub fn captions_pending(&self, id: &str) -> bool {
        let state = self.state(id);
        let Some(uploaded_at) = state.uploaded_at else { return false };
        let Ok(path) = self.dir(id).map(|d| d.join(CAPTIONS)) else { return false };
        match state.captions_uploaded {
            Some(hash) => std::fs::read(&path).is_ok_and(|b| captions_hash(&b) != hash),
            // Uploaded before this was tracked: captions then went up with the video.
            None => {
                std::fs::metadata(&path).and_then(|m| m.modified()).is_ok_and(|t| OffsetDateTime::from(t) > uploaded_at)
            }
        }
    }

    pub fn pending_upload(&self) -> Result<Vec<String>, String> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|e| match e.status {
                Status::Local | Status::Uploading => e.video.is_some(),
                Status::Synced | Status::Cloud => {
                    let s = self.state(&e.meta.id);
                    s.meta_dirty || !s.meta_private || self.captions_pending(&e.meta.id)
                }
                Status::Recording => false,
            })
            .map(|e| e.meta.id)
            .collect())
    }

    /// Synced recordings uploaded more than `days` ago, whose local video can go.
    pub fn expired_local(&self, days: u32, now: OffsetDateTime) -> Result<Vec<String>, String> {
        if days == 0 {
            return Ok(Vec::new());
        }
        let cutoff = now - time::Duration::days(days.into());
        Ok(self
            .list()?
            .into_iter()
            .filter(|e| e.status == Status::Synced && e.uploaded_at.is_some_and(|t| t < cutoff))
            .map(|e| e.meta.id)
            .collect())
    }

    pub fn captions_path(&self, id: &str) -> Result<PathBuf, String> {
        Ok(self.dir(id)?.join(CAPTIONS))
    }
}

pub fn file_size(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().filter(|m| m.is_file()).map(|m| m.len())
}

/// Write to a sibling temp file, then rename, so a crash never leaves half a file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", path.extension().and_then(|e| e.to_str()).unwrap_or_default()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn lib() -> (tempfile::TempDir, Library) {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open(dir.path().join("lib")).unwrap();
        (dir, lib)
    }

    const NOW: OffsetDateTime = datetime!(2026-11-01 13:00 UTC);

    #[test]
    fn record_finalize_upload_free() {
        let (_d, lib) = lib();
        let e = lib.create(None, NOW, 0).unwrap();
        assert_eq!(e.status, Status::Recording);
        assert_eq!(e.meta.title, "Grabación 1 nov 2026, 13:00");
        let id = e.meta.id.clone();
        assert!(lib.pending_upload().unwrap().is_empty(), "recording in progress is not uploaded");

        std::fs::write(e.dir.join(VIDEO), vec![0u8; 1000]).unwrap();
        let e = lib.finalize(&id, 12.5, 1920, 1080).unwrap();
        assert_eq!((e.status, e.meta.size_bytes, e.meta.width), (Status::Local, 1000, 1920));
        assert_eq!(lib.pending_upload().unwrap(), vec![id.clone()]);
        assert!(lib.remove_local_video(&id).is_err(), "never delete the only copy");

        let mut s = lib.state(&id);
        s.multipart =
            Some(Multipart { upload_id: "u".into(), part_size: 400, file_size: 1000, etags: vec!["a".into()] });
        lib.write_state(&id, &s).unwrap();
        let e = lib.entry(&id).unwrap();
        assert_eq!(e.status, Status::Uploading);
        assert_eq!(e.progress, Some(0.4));

        lib.write_state(&id, &State { uploaded_at: Some(NOW), meta_private: true, ..State::default() }).unwrap();
        assert_eq!(lib.entry(&id).unwrap().status, Status::Synced);
        assert!(lib.pending_upload().unwrap().is_empty());
        assert_eq!(lib.expired_local(30, NOW + time::Duration::days(31)).unwrap(), vec![id.clone()]);
        assert!(lib.expired_local(30, NOW + time::Duration::days(29)).unwrap().is_empty());

        lib.set_folder(&id, Some(" Clientes / Acme ")).unwrap();
        assert_eq!(lib.meta(&id).unwrap().folder.as_deref(), Some("Clientes/Acme"));
        lib.rename(&id, "Demo").unwrap();
        assert_eq!(lib.pending_upload().unwrap(), vec![id.clone()], "renamed: meta goes up again");

        let e = lib.remove_local_video(&id).unwrap();
        assert_eq!(e.status, Status::Cloud);
        assert!(e.video.is_none());
    }

    #[test]
    fn recover_leaves_live_writers_alone() {
        let (_d, lib) = lib();
        let live = lib.create(None, NOW, std::process::id()).unwrap();
        std::fs::write(live.dir.join(VIDEO), b"half").unwrap();
        assert!(lib.recover().unwrap().is_empty());
        assert_eq!(lib.entry(&live.meta.id).unwrap().status, Status::Recording);
    }

    #[test]
    fn recover_after_crash() {
        let (_d, lib) = lib();
        let with_video = lib.create(None, NOW, 0).unwrap();
        std::fs::write(with_video.dir.join(VIDEO), b"partial").unwrap();
        let empty = lib.create(None, NOW, 0).unwrap();
        assert_eq!(lib.recover().unwrap(), vec![with_video.meta.id.clone()]);
        assert_eq!(lib.entry(&with_video.meta.id).unwrap().status, Status::Local);
        assert!(!empty.dir.exists());
    }

    #[test]
    fn list_is_newest_first_and_skips_junk() {
        let (_d, lib) = lib();
        let old = lib.create(Some("old".into()), NOW, 0).unwrap();
        let new = lib.create(Some("new".into()), NOW + time::Duration::hours(1), 0).unwrap();
        std::fs::create_dir_all(lib.root().join("not-a-recording")).unwrap();
        std::fs::create_dir_all(lib.root().join(".DS_Store")).unwrap();
        let ids: Vec<_> = lib.list().unwrap().into_iter().map(|e| e.meta.title).collect();
        assert_eq!(ids, vec!["new", "old"]);
        let _ = (old, new);
    }

    #[test]
    fn folders_create_rename_delete() {
        let (_d, lib) = lib();
        lib.create_folder("Vacía").unwrap();
        let a = lib.create(Some("a".into()), NOW, 0).unwrap().meta.id;
        let b = lib.create(Some("b".into()), NOW, 0).unwrap().meta.id;
        lib.set_folder(&a, Some("Clientes/Acme")).unwrap();
        lib.set_folder(&b, Some("Clientesviejos")).unwrap();
        assert_eq!(lib.folders().unwrap(), vec!["Clientes", "Clientes/Acme", "Clientesviejos", "Vacía"]);

        lib.rename_folder("Clientes", "Cuentas").unwrap();
        assert_eq!(lib.meta(&a).unwrap().folder.as_deref(), Some("Cuentas/Acme"));
        assert_eq!(lib.meta(&b).unwrap().folder.as_deref(), Some("Clientesviejos"), "only whole path components");
        assert!(lib.rename_folder("Cuentas", "Cuentas/Sub").is_err());

        lib.delete_folder("Cuentas/Acme").unwrap();
        assert_eq!(lib.meta(&a).unwrap().folder.as_deref(), Some("Cuentas"));
        lib.delete_folder("Vacía").unwrap();
        assert_eq!(lib.folders().unwrap(), vec!["Clientesviejos", "Cuentas"]);
    }

    #[test]
    fn rejects_traversal() {
        let (_d, lib) = lib();
        assert!(lib.dir("../../etc").is_err());
        assert!(lib.remove("..").is_err());
    }
}
