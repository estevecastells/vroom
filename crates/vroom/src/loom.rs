//! Import every video of a Loom account into the library (and R2).
//!
//! Loom has no public API to list or download videos, so this uses the private
//! GraphQL API of loom.com with the browser's `connect.sid` session cookie, as
//! the maintained open-source archivers do (loom-archiver, loom-downloader,
//! yt-dlp). Only videos the signed-in person created are listed. Re-running an
//! import skips what is already in the library, so it can stop and resume.

use crate::out::Out;
use crate::{find_tool, take_flag, take_opt, Ctx};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use time::OffsetDateTime;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use vroom_cloud::sync;
use vroom_core::i18n::t;
use vroom_core::layout::{CAPTIONS, THUMB, VIDEO};
use vroom_core::library::mark_in_progress;
use vroom_core::{tr, Meta, Source};

const GRAPHQL: &str = "https://www.loom.com/graphql";
/// Loom documents no rate limit; the archivers pause like this between calls.
const PAUSE: Duration = Duration::from_millis(400);
/// Videos imported side by side.
const WORKERS: usize = 3;
/// HLS segments fetched side by side: they are small, so one at a time is
/// mostly waiting on round trips.
const SEGMENTS_AT_ONCE: usize = 8;
const PAGE: u32 = 50;
/// Bounds on what a (possibly compromised) Loom response can make us fetch.
const MAX_SEGMENTS: usize = 50_000;
const MAX_VIDEO_BYTES: u64 = 50 * 1024 * 1024 * 1024;

const LIST_VIDEOS: &str = "query GetLoomsForLibrary($limit: Int!, $cursor: String, $folderId: String, $sourceValue: String, $source: LoomsSource!, $sortType: LoomsSortType!, $sortOrder: LoomsSortOrder!, $sortGrouping: LoomsSortGrouping, $filters: [[LoomsCollectionFilter!]!], $timeRange: TimeRange) {
  getLooms {
    __typename
    ... on GetLoomsPayload {
      videos(first: $limit, after: $cursor, folderId: $folderId, sourceValue: $sourceValue, source: $source, sortType: $sortType, sortOrder: $sortOrder, sortGrouping: $sortGrouping, filters: $filters, timeRange: $timeRange) {
        edges {
          node {
            id name createdAt playable_duration
            ... on RegularUserVideo { signedDefaultThumbnails { default static } }
            __typename
          }
          __typename
        }
        pageInfo { endCursor hasNextPage __typename }
        __typename
      }
    }
  }
}";

const LIST_FOLDERS: &str = "query GetPublishedFolders($first: Int!, $after: String, $source: FolderSource!, $sourceValue: String, $sortType: LoomsSortType!, $sortOrder: LoomsSortOrder!, $parentFolderId: String, $workspaceId: ID, $filters: [LoomsCollectionFilter!]) {
  getPublishedFolders {
    __typename
    ... on GetPublishedFoldersPayload {
      folders(first: $first, after: $after, source: $source, sourceValue: $sourceValue, sortType: $sortType, sortOrder: $sortOrder, parentFolderId: $parentFolderId, workspaceId: $workspaceId, filters: $filters) {
        edges { cursor node { id name __typename } __typename }
        pageInfo { endCursor hasNextPage __typename }
        __typename
      }
      __typename
    }
  }
}";

const TRANSCODED_URL: &str = "query GetVideoTranscodedUrl($videoId: ID!, $forceOriginal: Boolean) {
  getVideoTranscodedUrl(videoId: $videoId, forceOriginal: $forceOriginal) {
    ... on VideoSource { url __typename }
    __typename
  }
}";

const VIDEO_SOURCE: &str =
    "query GetVideoSource($videoId: ID!, $password: String, $acceptableMimes: [CloudfrontVideoAcceptableMime]) {
  getVideo(id: $videoId, password: $password) {
    ... on RegularUserVideo {
      id
      nullableRawCdnUrl(acceptableMimes: $acceptableMimes, password: $password) { url __typename }
      __typename
    }
    __typename
  }
}";

const TRANSCRIPT: &str = "query FetchVideoTranscript($videoId: ID!, $password: String) {
  fetchVideoTranscript(videoId: $videoId, password: $password) {
    ... on VideoTranscriptDetails { id video_id source_url captions_source_url __typename }
    __typename
  }
}";

#[derive(Debug, Clone)]
pub struct Video {
    pub id: String,
    pub name: String,
    pub created_at: Option<OffsetDateTime>,
    pub duration_secs: f64,
    pub thumbnail: Option<String>,
    /// Loom folder path, `Parent/Child`.
    pub folder: Option<String>,
}

pub fn run(out: &Out, ctx: &Ctx, mut args: Vec<String>) -> Result<(), String> {
    let sub = if args.is_empty() { String::new() } else { args.remove(0) };
    let no_upload = take_flag(&mut args, "--no-upload");
    let free_local = take_flag(&mut args, "--free-local");
    let limit: usize = take_opt(&mut args, "--limit").and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    if sub != "list" && sub != "import" {
        return Err("vroom loom list | vroom loom import [--no-upload] [--free-local] [--limit N]".into());
    }
    let cookie = match std::env::var("VROOM_LOOM_COOKIE") {
        Ok(c) if !c.trim().is_empty() => c,
        _ => crate::read_stdin_line()?,
    };
    let loom = Loom::new(&cookie)?;
    let videos = loom.all_videos(out)?;

    if sub == "list" {
        let total: f64 = videos.iter().map(|v| v.duration_secs).sum();
        for v in &videos {
            let date = v.created_at.map(|d| d.date().to_string()).unwrap_or_default();
            out.ok(format!("{}  {date}  {:>8}  {}", v.id, vroom_core::share::format_duration(v.duration_secs), v.name));
        }
        out.ok(tr!(
            "{n} vídeos en Loom, {duration} en total",
            n = videos.len(),
            duration = vroom_core::share::format_duration(total)
        ));
        return Ok(());
    }

    let _lock = ctx.lock()?;
    let r2 = if no_upload {
        None
    } else {
        Some(ctx.r2().map_err(|e| tr!("{e} (o usa --no-upload para importar solo en local)", e))?)
    };
    if free_local && r2.is_none() {
        return Err(t("--free-local necesita subir a R2"));
    }
    // An import stopped halfway left folders without a video: drop them, so
    // those videos are imported again rather than counted as done.
    ctx.lib.recover()?;
    // Already imported, by Loom id. Imports get random ids like any recording:
    // Loom ids are in old share links, and the bucket may be public.
    let mut known: std::collections::HashMap<String, String> = ctx
        .lib
        .list()?
        .into_iter()
        .filter(|e| e.meta.source == Source::Loom)
        .filter_map(|e| Some((e.meta.source_id?, e.meta.id)))
        .collect();
    let jobs: Vec<(&Video, String, bool)> = videos
        .iter()
        .take(limit)
        .map(|v| {
            let fresh = !known.contains_key(&v.id);
            let id = known.entry(v.id.clone()).or_insert_with(vroom_core::id::generate).clone();
            (v, id, fresh)
        })
        .collect();
    // A few videos at a time: each one waits on Loom's API, its CDN and R2 in
    // turn, so running them side by side keeps the line busy.
    let next = AtomicUsize::new(0);
    let (imported, skipped) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let failed = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..WORKERS.min(jobs.len()) {
            s.spawn(|| {
                while let Some((v, id, fresh)) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    if *fresh {
                        if let Err(e) = import_one(out, ctx, &loom, v, id) {
                            out.warning(Some(id), &format!("{}: {e}", v.name));
                            failed.lock().unwrap_or_else(|e| e.into_inner()).push(v.name.clone());
                            let _ = ctx.lib.remove(id);
                            continue;
                        }
                        imported.fetch_add(1, Ordering::Relaxed);
                        out.done(id, "imported");
                    } else {
                        skipped.fetch_add(1, Ordering::Relaxed);
                        // Imported before folders existed, or moved in Loom since.
                        if v.folder.is_some() && ctx.lib.meta(id).ok().and_then(|m| m.folder).is_none() {
                            if let Err(e) = ctx.lib.set_folder(id, v.folder.as_deref()) {
                                out.warning(Some(id), &e);
                            }
                        }
                    }
                    if let Some(r2) = &r2 {
                        let state = ctx.lib.state(id);
                        if state.uploaded_at.is_none() || state.meta_dirty {
                            match sync::upload(r2, &ctx.lib, id, |d, t| out.progress(id, d, t)) {
                                Ok(()) => out.done(id, "uploaded"),
                                Err(e) => {
                                    out.warning(Some(id), &tr!("subida: {e}", e));
                                    continue;
                                }
                            }
                        }
                        if free_local {
                            if let Err(e) = ctx.lib.remove_local_video(id) {
                                out.warning(Some(id), &e);
                            }
                        }
                    }
                }
            });
        }
    });
    let (imported, skipped) = (imported.into_inner(), skipped.into_inner());
    let failed = failed.into_inner().unwrap_or_else(|e| e.into_inner());
    out.ok(tr!(
        "Loom: {imported} importados, {skipped} ya estaban, {failed} fallaron",
        imported = imported,
        skipped = skipped,
        failed = failed.len()
    ));
    if !failed.is_empty() {
        return Err(tr!(
            "no se pudieron importar: {names}. Vuelve a lanzar la importación para reintentarlos",
            names = failed.join(", ")
        ));
    }
    Ok(())
}

fn import_one(out: &Out, ctx: &Ctx, loom: &Loom, v: &Video, id: &str) -> Result<(), String> {
    let mut meta = Meta::new(
        id.to_string(),
        if v.name.trim().is_empty() { t("Loom sin título") } else { v.name.clone() },
        v.created_at.unwrap_or_else(OffsetDateTime::now_utc),
    );
    meta.source = Source::Loom;
    meta.source_id = Some(v.id.clone());
    meta.folder = v.folder.clone();
    meta.duration_secs = v.duration_secs;
    ctx.lib.write_meta(&meta)?;
    let dir = ctx.lib.dir(id)?;
    // A crash mid-download leaves no video, so `recover` drops the folder.
    mark_in_progress(&dir, std::process::id()).map_err(|e| e.to_string())?;

    let video = dir.join(VIDEO);
    match loom.source(&v.id)? {
        Media::File(url) => loom.download(&url, &video, |d, t| out.progress(id, d, t))?,
        Media::Hls(url) => loom.download_hls(&url, &dir, &video)?,
    }

    if let Some(url) = &v.thumbnail {
        if let Ok(bytes) = loom.get_bytes(url) {
            if bytes.starts_with(&[0xFF, 0xD8]) {
                std::fs::write(dir.join(THUMB), bytes).map_err(|e| e.to_string())?;
            }
        }
    }
    if let Some(url) = loom.captions_url(&v.id) {
        if let Ok(bytes) = loom.get_bytes(&url) {
            if bytes.starts_with(b"WEBVTT") || bytes.starts_with("\u{feff}WEBVTT".as_bytes()) {
                std::fs::write(dir.join(CAPTIONS), bytes).map_err(|e| e.to_string())?;
            }
        }
    }
    ctx.lib.finalize(id, meta.duration_secs, 0, 0)?;
    Ok(())
}

enum Media {
    File(String),
    Hls(String),
}

struct Loom {
    agent: ureq::Agent,
    cookie: String,
}

impl Loom {
    fn new(cookie: &str) -> Result<Loom, String> {
        let cookie = cookie.trim().trim_start_matches("Cookie:").trim();
        if cookie.is_empty() {
            return Err(t("falta la cookie de loom.com"));
        }
        // A whole Cookie header (name=value; …) as copied from the browser, or just
        // the value of connect.sid.
        let cookie = if cookie.contains('=') { cookie_pairs(cookie) } else { format!("connect.sid={cookie}") };
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_recv_response(Some(Duration::from_secs(120)))
            .timeout_per_read(Some(Duration::from_secs(120)))
            // Loom and its CDN are all https. Redirects are followed by hand
            // (`get_media`) so every hop goes through `check_media_url`.
            .https_only(true)
            .max_redirects(0)
            // A proxy would be what `PublicOnly` resolves, not the media host.
            .proxy(None)
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)")
            .build();
        let agent = ureq::Agent::with_parts(config, DefaultConnector::default(), PublicOnly::default());
        Ok(Loom { agent, cookie })
    }

    fn gql(&self, op: &str, query: &str, variables: Value) -> Result<Value, String> {
        std::thread::sleep(PAUSE);
        let body = json!({ "operationName": op, "query": query, "variables": variables });
        let mut last = String::new();
        for attempt in 0..4u32 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_secs(2u64.pow(attempt)));
            }
            let resp = self
                .agent
                .post(GRAPHQL)
                .header("Cookie", &self.cookie)
                .header("Origin", "https://www.loom.com")
                .header("apollographql-client-name", "web")
                .header("x-loom-request-source", "loom_web_1")
                .header("graphql-operation-name", op)
                .send_json(&body);
            let mut resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    last = tr!("sin conexión con Loom: {e}", e);
                    continue;
                }
            };
            let status = resp.status().as_u16();
            if status == 401 || status == 403 {
                return Err(expired());
            }
            if status == 429 || status >= 500 {
                last = tr!("Loom respondió {status}", status);
                continue;
            }
            let v: Value =
                resp.body_mut().read_json().map_err(|e| tr!("respuesta de Loom ({status}): {e}", status, e))?;
            if let Some(errors) = v.get("errors").and_then(Value::as_array) {
                if errors
                    .iter()
                    .any(|e| e.pointer("/extensions/code").and_then(Value::as_str) == Some("UNAUTHENTICATED"))
                {
                    return Err(expired());
                }
                if v.get("data").is_none_or(Value::is_null) {
                    let msgs: Vec<&str> = errors.iter().filter_map(|e| e.get("message")?.as_str()).collect();
                    return Err(format!("Loom ({op}): {}", msgs.join("; ")));
                }
            }
            return v.get("data").cloned().ok_or_else(|| tr!("Loom ({op}) no devolvió datos", op));
        }
        Err(last)
    }

    /// Folder ids and their paths (`Parent/Child`), walking subfolders: videos
    /// inside folders are not in the main list.
    fn folders(&self) -> Result<Vec<(String, String)>, String> {
        let mut found: Vec<(String, String)> = Vec::new();
        let mut queue: Vec<(Option<String>, String)> = vec![(None, String::new())];
        while let Some((parent, parent_path)) = queue.pop() {
            let mut after: Option<String> = None;
            loop {
                let data = self.gql(
                    "GetPublishedFolders",
                    LIST_FOLDERS,
                    json!({
                        "first": PAGE, "after": after, "source": "ACTIVE", "sortType": "RECENT", "sortOrder": "DESC",
                        "parentFolderId": parent, "filters": [{ "type": "CREATED_BY_ME" }]
                    }),
                )?;
                let conn = &data["getPublishedFolders"]["folders"];
                for edge in conn["edges"].as_array().into_iter().flatten() {
                    if let Some(id) = edge["node"]["id"].as_str() {
                        if !found.iter().any(|(f, _)| f == id) {
                            // A slash inside a name would read as a subfolder.
                            let name = edge["node"]["name"]
                                .as_str()
                                .map_or_else(|| t("Sin nombre"), str::to_string)
                                .replace('/', "-");
                            let path = if parent_path.is_empty() { name } else { format!("{parent_path}/{name}") };
                            found.push((id.to_string(), path.clone()));
                            queue.push((Some(id.to_string()), path));
                        }
                    }
                }
                // The last page still has an endCursor: stop on hasNextPage.
                if conn["pageInfo"]["hasNextPage"].as_bool() != Some(true) {
                    break;
                }
                after = conn["pageInfo"]["endCursor"].as_str().map(str::to_string);
            }
        }
        Ok(found)
    }

    fn videos_in(&self, folder: Option<&str>, path: Option<&str>, into: &mut Vec<Video>) -> Result<(), String> {
        let filters = match folder {
            None => json!([[{ "type": "CREATED_BY_ME" }], [{ "type": "NOT_IN_FOLDER" }]]),
            Some(_) => json!([[{ "type": "CREATED_BY_ME" }]]),
        };
        let mut cursor: Option<String> = None;
        loop {
            let data = self.gql(
                "GetLoomsForLibrary",
                LIST_VIDEOS,
                json!({
                    "limit": PAGE, "cursor": cursor, "folderId": folder, "source": "MINE",
                    "sortType": "RECENT", "sortOrder": "DESC", "filters": filters
                }),
            )?;
            let conn = &data["getLooms"]["videos"];
            for edge in conn["edges"].as_array().into_iter().flatten() {
                if let Some(mut v) = parse_video(&edge["node"]) {
                    v.folder = path.and_then(vroom_core::meta::normalize_folder);
                    if !into.iter().any(|x| x.id == v.id) {
                        into.push(v);
                    }
                }
            }
            if conn["pageInfo"]["hasNextPage"].as_bool() != Some(true) {
                return Ok(());
            }
            cursor = conn["pageInfo"]["endCursor"].as_str().map(str::to_string);
        }
    }

    fn all_videos(&self, out: &Out) -> Result<Vec<Video>, String> {
        let mut videos = Vec::new();
        self.videos_in(None, None, &mut videos)?;
        let folders = self.folders()?;
        for (id, path) in &folders {
            self.videos_in(Some(id), Some(path), &mut videos)?;
        }
        out.warning(
            None,
            &tr!(
                "Loom: {videos} vídeos en la biblioteca y {folders} carpetas",
                videos = videos.len(),
                folders = folders.len()
            ),
        );
        videos.sort_by_key(|v| v.created_at);
        Ok(videos)
    }

    /// A single MP4 when Loom has one; otherwise the HLS playlist.
    fn source(&self, id: &str) -> Result<Media, String> {
        let data =
            self.gql("GetVideoTranscodedUrl", TRANSCODED_URL, json!({ "videoId": id, "forceOriginal": false }))?;
        if let Some(url) = data["getVideoTranscodedUrl"]["url"].as_str().filter(|u| !u.is_empty()) {
            return Ok(Media::File(url.to_string()));
        }
        let data = self.gql(
            "GetVideoSource",
            VIDEO_SOURCE,
            json!({ "videoId": id, "password": null, "acceptableMimes": ["MP4"] }),
        )?;
        if let Some(url) = data["getVideo"]["nullableRawCdnUrl"]["url"].as_str().filter(|u| !u.is_empty()) {
            return Ok(Media::File(url.to_string()));
        }
        // Without acceptableMimes Loom returns the streaming manifest.
        let data = self.gql(
            "GetVideoSource",
            VIDEO_SOURCE,
            json!({ "videoId": id, "password": null, "acceptableMimes": null }),
        )?;
        match data["getVideo"]["nullableRawCdnUrl"]["url"].as_str() {
            Some(url) if url.contains(".m3u8") => Ok(Media::Hls(url.to_string())),
            Some(url) if !url.is_empty() => {
                Err(tr!("formato no soportado ({url})", url = url.split('?').next().unwrap_or(url)))
            }
            _ => Err(t("Loom no da ninguna URL de descarga para este vídeo")),
        }
    }

    fn captions_url(&self, id: &str) -> Option<String> {
        let data = self.gql("FetchVideoTranscript", TRANSCRIPT, json!({ "videoId": id, "password": null })).ok()?;
        data["fetchVideoTranscript"]["captions_source_url"].as_str().filter(|u| !u.is_empty()).map(str::to_string)
    }

    /// GET a CDN URL, following up to 5 redirects, each checked first.
    fn get_media(&self, url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
        let mut url = url::Url::parse(url).map_err(|_| t("URL de Loom no válida"))?;
        for _ in 0..=5 {
            check_media_url(url.as_str())?;
            let resp = self.agent.get(url.as_str()).call().map_err(|e| e.to_string())?;
            if !resp.status().is_redirection() {
                return Ok(resp);
            }
            let location = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| t("redirección sin destino"))?;
            url = url.join(location).map_err(|_| t("redirección no válida"))?;
        }
        Err(t("demasiadas redirecciones"))
    }

    /// One HLS segment, retried: with hundreds per video, a dropped connection
    /// should not cost the whole download.
    fn get_segment(&self, url: &str) -> Result<Vec<u8>, String> {
        let mut attempt = 0;
        loop {
            match self.get_bytes(url) {
                Ok(b) => return Ok(b),
                Err(e) if attempt >= 3 => return Err(e),
                Err(_) => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(500 * 2u64.pow(attempt)));
                }
            }
        }
    }

    /// CDN URLs are signed; no cookie goes to them.
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        let resp = self.get_media(url)?;
        if resp.status() != 200 {
            return Err(format!("HTTP {}", resp.status()));
        }
        resp.into_body().with_config().limit(256 * 1024 * 1024).read_to_vec().map_err(|e| e.to_string())
    }

    fn download(&self, url: &str, dest: &Path, mut progress: impl FnMut(u64, u64)) -> Result<(), String> {
        let resp = self.get_media(url)?;
        if resp.status() != 200 {
            return Err(tr!("descarga: HTTP {status}", status = resp.status()));
        }
        let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()?.parse().ok()).unwrap_or(0);
        let tmp = dest.with_extension("part");
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut reader = resp.into_body().into_reader();
        let mut buf = vec![0u8; 256 * 1024];
        let mut done = 0u64;
        loop {
            let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            if done > MAX_VIDEO_BYTES {
                return Err(t("el vídeo supera el tamaño máximo esperado"));
            }
            progress(done, total);
        }
        file.sync_all().map_err(|e| e.to_string())?;
        if total > 0 && done != total {
            return Err(tr!("descarga incompleta ({done} de {total} bytes)", done, total));
        }
        std::fs::rename(&tmp, dest).map_err(|e| e.to_string())
    }

    /// Long Loom videos may only exist as CloudFront-signed HLS. The signature
    /// in the playlist's query must go on every segment, so fetch the segments
    /// here, join them, and let ffmpeg remux to MP4 offline.
    fn download_hls(&self, master_url: &str, dir: &Path, dest: &Path) -> Result<(), String> {
        let ffmpeg = find_tool("ffmpeg", "VROOM_FFMPEG")
            .ok_or_else(|| t("este vídeo solo está en streaming (HLS); instala ffmpeg para importarlo"))?;
        let master = String::from_utf8_lossy(&self.get_bytes(master_url)?).into_owned();
        let (video_uri, audio_uri) = if master.contains("#EXT-X-STREAM-INF") {
            let (v, a) = parse_master(&master);
            (v.ok_or_else(|| t("lista HLS sin vídeo"))?, a)
        } else {
            (master_url.to_string(), None)
        };
        let video_tmp = dir.join("hls-video.part");
        let audio_tmp = dir.join("hls-audio.part");
        let cleanup = || {
            let _ = std::fs::remove_file(&video_tmp);
            let _ = std::fs::remove_file(&audio_tmp);
        };
        let result = (|| {
            // The joined segments are local files of a known container: ffmpeg
            // must not guess (a "segment" could be a playlist pointing elsewhere).
            let input = |cmd: &mut std::process::Command, fmp4: bool, path: &Path| {
                cmd.args(["-protocol_whitelist", "file", "-f", if fmp4 { "mp4" } else { "mpegts" }, "-i"]).arg(path);
            };
            let fmp4 = self.concat_segments(&sign(master_url, &video_uri), &video_tmp)?;
            let mut cmd = std::process::Command::new(&ffmpeg);
            cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
            input(&mut cmd, fmp4, &video_tmp);
            if let Some(a) = &audio_uri {
                let fmp4 = self.concat_segments(&sign(master_url, a), &audio_tmp)?;
                input(&mut cmd, fmp4, &audio_tmp);
                cmd.args(["-map", "0:v:0", "-map", "1:a:0"]);
            }
            let out_tmp = dest.with_extension("hls.mp4");
            cmd.args(["-c", "copy", "-movflags", "+faststart"]).arg(&out_tmp);
            let output = cmd.output().map_err(|e| format!("ffmpeg: {e}"))?;
            if !output.status.success() {
                let _ = std::fs::remove_file(&out_tmp);
                return Err(format!("ffmpeg: {}", String::from_utf8_lossy(&output.stderr).trim()));
            }
            std::fs::rename(&out_tmp, dest).map_err(|e| e.to_string())
        })();
        cleanup();
        result
    }

    /// Download a media playlist's segments into one file; true when they are
    /// fragmented MP4 (an `#EXT-X-MAP` init segment) rather than MPEG-TS.
    fn concat_segments(&self, playlist_url: &str, dest: &Path) -> Result<bool, String> {
        let text = String::from_utf8_lossy(&self.get_bytes(playlist_url)?).into_owned();
        if text.contains("#EXT-X-KEY") && !text.contains("METHOD=NONE") {
            return Err(t("vídeo HLS cifrado; no soportado"));
        }
        let uris = segment_uris(&text);
        if uris.len() > MAX_SEGMENTS {
            return Err(tr!("lista HLS con demasiados segmentos ({n})", n = uris.len()));
        }
        let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
        let mut total = 0u64;
        for batch in uris.chunks(SEGMENTS_AT_ONCE) {
            let fetched: Vec<Result<Vec<u8>, String>> = std::thread::scope(|s| {
                let handles: Vec<_> =
                    batch.iter().map(|uri| s.spawn(|| self.get_segment(&sign(playlist_url, uri)))).collect();
                handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(t("descarga interrumpida")))).collect()
            });
            for bytes in fetched {
                let bytes = bytes?;
                total += bytes.len() as u64;
                if total > MAX_VIDEO_BYTES {
                    return Err(t("el vídeo supera el tamaño máximo esperado"));
                }
                file.write_all(&bytes).map_err(|e| e.to_string())?;
            }
        }
        file.sync_all().map_err(|e| e.to_string())?;
        Ok(text.contains("#EXT-X-MAP"))
    }
}

/// Keep the `name=value` pairs of a Cookie header, or of a pasted Set-Cookie
/// line, dropping attributes such as `Path=/`, `Expires=…` or `HttpOnly`.
fn cookie_pairs(raw: &str) -> String {
    const ATTRIBUTES: [&str; 8] =
        ["domain", "path", "expires", "max-age", "httponly", "secure", "samesite", "priority"];
    raw.split(';')
        .map(str::trim)
        .filter(|p| p.contains('='))
        .filter(|p| !ATTRIBUTES.contains(&p.split('=').next().unwrap_or("").trim().to_ascii_lowercase().as_str()))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Media URLs come from Loom's responses: only https hosts with a public-looking
/// name. Where the name points is checked by `PublicOnly` when connecting.
fn check_media_url(url: &str) -> Result<(), String> {
    let u = url::Url::parse(url).map_err(|_| t("URL de Loom no válida"))?;
    let rejected = || tr!("URL de descarga rechazada: {host}", host = u.host_str().unwrap_or("?"));
    let Some(url::Host::Domain(host)) = u.host() else { return Err(rejected()) };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let local = [".local", ".internal", ".localhost", ".lan", ".home.arpa"];
    if u.scheme() != "https" || !host.contains('.') || local.iter().any(|s| host.ends_with(s)) {
        return Err(rejected());
    }
    Ok(())
}

/// Resolves like ureq does, then refuses hosts that point at this machine or a
/// private network. Checking here, rather than before the request, means the
/// addresses checked are the ones connected to.
#[derive(Debug, Default)]
struct PublicOnly(DefaultResolver);

impl Resolver for PublicOnly {
    fn resolve(&self, uri: &Uri, config: &Config, timeout: NextTimeout) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let addrs = self.0.resolve(uri, config, timeout)?;
        if addrs.iter().all(|a| is_public(a.ip())) {
            Ok(addrs)
        } else {
            Err(ureq::Error::BadUri(tr!("{host} apunta a una red local", host = uri.host().unwrap_or("?"))))
        }
    }
}

/// Global unicast only.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..128).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..32).contains(&b))
                || (a == 192 && b == 168)
                || (a == 192 && b == 0 && (c == 0 || c == 2))
                || (a == 198 && (b == 18 || b == 19))
                || (a == 198 && b == 51 && c == 100)
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(v4.into());
            }
            let s = v6.segments();
            match s[0] {
                // 6to4 carries an IPv4 address.
                0x2002 => is_public(Ipv4Addr::new((s[1] >> 8) as u8, s[1] as u8, (s[2] >> 8) as u8, s[2] as u8).into()),
                // Teredo and documentation.
                0x2001 if s[1] == 0 || s[1] == 0x0db8 => false,
                first => first & 0xe000 == 0x2000,
            }
        }
    }
}

fn expired() -> String {
    t("la sesión de Loom no es válida o ha caducado: vuelve a copiar la cookie de loom.com")
}

fn parse_video(node: &Value) -> Option<Video> {
    let id = node["id"].as_str()?.to_string();
    Some(Video {
        name: node["name"].as_str().unwrap_or_default().to_string(),
        created_at: node["createdAt"]
            .as_str()
            .and_then(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()),
        duration_secs: node["playable_duration"].as_f64().unwrap_or(0.0),
        thumbnail: node["signedDefaultThumbnails"]["static"]
            .as_str()
            .or(node["signedDefaultThumbnails"]["default"].as_str())
            .map(str::to_string),
        folder: None,
        id,
    })
}

/// Highest-bandwidth video variant and the audio rendition, if separate.
fn parse_master(text: &str) -> (Option<String>, Option<String>) {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let (mut video, mut best, mut audio) = (None, -1i64, None);
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("#EXT-X-MEDIA:") && line.contains("TYPE=AUDIO") {
            audio = attr(line, "URI").or(audio);
        } else if line.starts_with("#EXT-X-STREAM-INF:") {
            let bw = attr_raw(line, "BANDWIDTH").and_then(|b| b.parse().ok()).unwrap_or(0);
            let uri = lines[i + 1..].iter().find(|l| !l.is_empty() && !l.starts_with('#'));
            if let Some(uri) = uri {
                if bw > best {
                    best = bw;
                    video = Some(uri.to_string());
                }
            }
        }
    }
    (video, audio)
}

/// Segment URIs in order, including an fMP4 init segment (`#EXT-X-MAP`).
fn segment_uris(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with("#EXT-X-MAP:") {
            out.extend(attr(line, "URI"));
        } else if !line.is_empty() && !line.starts_with('#') {
            out.push(line.to_string());
        }
    }
    out
}

fn attr(line: &str, name: &str) -> Option<String> {
    let start = line.find(&format!("{name}=\""))? + name.len() + 2;
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

fn attr_raw<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let start = line.find(&format!("{name}="))? + name.len() + 1;
    Some(line[start..].split(',').next()?.trim_matches('"'))
}

/// Resolve `uri` against `base` and carry `base`'s signature query along.
fn sign(base: &str, uri: &str) -> String {
    if uri.starts_with("http://") || uri.starts_with("https://") {
        return uri.to_string();
    }
    let (path, query) = base.split_once('?').unwrap_or((base, ""));
    let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or(path);
    let (uri, own_query) = uri.split_once('?').unwrap_or((uri, ""));
    let query = if own_query.is_empty() { query } else { own_query };
    if query.is_empty() {
        format!("{dir}/{uri}")
    } else {
        format!("{dir}/{uri}?{query}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_best_variant_and_audio() {
        let master = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",URI=\"audio/index.m3u8\"\n\
            #EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\n360/index.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=4000000,RESOLUTION=1920x1080\n1080/index.m3u8\n";
        assert_eq!(parse_master(master), (Some("1080/index.m3u8".into()), Some("audio/index.m3u8".into())));
    }

    #[test]
    fn segments_keep_the_signature() {
        let playlist = "https://cdn.loom.com/sessions/x/1080/index.m3u8?Policy=p&Signature=s&Key-Pair-Id=k";
        assert_eq!(
            sign(playlist, "seg0.ts"),
            "https://cdn.loom.com/sessions/x/1080/seg0.ts?Policy=p&Signature=s&Key-Pair-Id=k"
        );
        assert_eq!(sign(playlist, "https://other/x.ts"), "https://other/x.ts");
        let text = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4,\nseg0.m4s\n#EXTINF:4,\nseg1.m4s\n#EXT-X-ENDLIST\n";
        assert_eq!(segment_uris(text), vec!["init.mp4", "seg0.m4s", "seg1.m4s"]);
    }

    #[test]
    fn only_public_https_media_urls() {
        assert!(check_media_url("http://cdn.loom.com/x.mp4").is_err());
        assert!(check_media_url("https://127.0.0.1/x").is_err());
        assert!(check_media_url("https://[::1]/x").is_err());
        assert!(check_media_url("https://localhost/x").is_err());
        assert!(check_media_url("https://localhost./x").is_err());
        assert!(check_media_url("https://printer.LOCAL./x").is_err());
        assert!(!is_public("10.0.0.1".parse().unwrap()));
        assert!(!is_public("::127.0.0.1".parse().unwrap()));
        assert!(!is_public("64:ff9b::7f00:1".parse().unwrap()));
        assert!(!is_public("2002:7f00:1::".parse().unwrap()));
        assert!(!is_public("fec0::1".parse().unwrap()));
        assert!(!is_public("ff02::1".parse().unwrap()));
        assert!(!is_public("198.18.0.1".parse().unwrap()));
        assert!(is_public("2606:4700::1111".parse().unwrap()));
        assert!(!is_public("169.254.169.254".parse().unwrap()));
        assert!(!is_public("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!is_public("fd00::1".parse().unwrap()));
        assert!(is_public("1.1.1.1".parse().unwrap()));
        assert!(check_media_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn cookies_from_set_cookie_lines() {
        assert_eq!(
            cookie_pairs("connect.sid=s%3Aabc.def; Domain=www.loom.com; Path=/; Expires=Tue, 01 Dec 2026 14:20:33 GMT; HttpOnly; Secure; SameSite=Lax"),
            "connect.sid=s%3Aabc.def"
        );
        assert_eq!(cookie_pairs("a=1; connect.sid=x"), "a=1; connect.sid=x");
    }

    #[test]
    fn parses_list_nodes() {
        let node = json!({
            "id": "0F2B8C1E9A7D4B3C", "name": "Weekly", "createdAt": "2025-03-14T09:00:00.000Z",
            "playable_duration": 902.5, "signedDefaultThumbnails": { "default": "https://x/a.gif", "static": "https://x/a.jpg" }
        });
        let v = parse_video(&node).unwrap();
        assert_eq!(v.duration_secs, 902.5);
        assert_eq!(v.thumbnail.as_deref(), Some("https://x/a.jpg"));
        assert_eq!(v.created_at.unwrap().year(), 2025);
    }
}
