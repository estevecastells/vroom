//! Upload, resume, restore, download and delete against a tiny in-process S3
//! that speaks just enough of the protocol (signatures are not checked).

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use time::macros::datetime;
use vroom_cloud::{sync, R2};
use vroom_core::layout::{META, PAGE, THUMB, VIDEO};
use vroom_core::{Library, R2Config, Status};

#[derive(Default)]
struct Store {
    objects: BTreeMap<String, Vec<u8>>,
    uploads: HashMap<String, BTreeMap<u32, Vec<u8>>>,
    next_upload: u32,
    part_puts: u32,
    /// Part numbers above this answer 500, to cut an upload halfway.
    fail_parts_above: Option<u32>,
}

type Shared = Arc<Mutex<Store>>;

fn serve() -> (String, Shared) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    let store: Shared = Arc::default();
    let s = store.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let s = s.clone();
            std::thread::spawn(move || handle(stream, s));
        }
    });
    (addr, store)
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

fn handle(stream: TcpStream, store: Shared) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut out = stream;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let target = parts.next().unwrap_or_default().to_string();
        let mut len = 0usize;
        loop {
            let mut h = String::new();
            reader.read_line(&mut h).unwrap();
            if h.trim().is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                if k.eq_ignore_ascii_case("content-length") {
                    len = v.trim().parse().unwrap();
                }
            }
        }
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).unwrap();

        let (path, query) = target.split_once('?').unwrap_or((&target, ""));
        let q: HashMap<String, String> = query
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|p| {
                let (k, v) = p.split_once('=').unwrap_or((p, ""));
                (decode(k), decode(v))
            })
            .collect();
        let path = decode(path);
        let key = path.trim_start_matches('/').split_once('/').map(|(_, k)| k.to_string()).unwrap_or_default();
        let (status, headers, resp) = route(&method, &key, &q, body, &store);
        let mut head = format!("HTTP/1.1 {status} X\r\ncontent-length: {}\r\n", resp.len());
        for (k, v) in headers {
            head += &format!("{k}: {v}\r\n");
        }
        head += "\r\n";
        if out.write_all(head.as_bytes()).and_then(|_| out.write_all(&resp)).is_err() {
            return;
        }
    }
}

const NS: &str = r#"xmlns="http://s3.amazonaws.com/doc/2006-03-01/""#;

fn route(
    method: &str,
    key: &str,
    q: &HashMap<String, String>,
    body: Vec<u8>,
    store: &Shared,
) -> (u16, Vec<(&'static str, String)>, Vec<u8>) {
    let mut s = store.lock().unwrap();
    match method {
        "GET" if key.is_empty() => {
            let prefix = q.get("prefix").cloned().unwrap_or_default();
            let items: String = s
                .objects
                .iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(k, v)| {
                    format!("<Contents><Key>{k}</Key><LastModified>2026-11-01T00:00:00.000Z</LastModified><Size>{}</Size></Contents>", v.len())
                })
                .collect();
            let xml = format!("<?xml version=\"1.0\"?><ListBucketResult {NS}><Name>b</Name><IsTruncated>false</IsTruncated>{items}</ListBucketResult>");
            (200, vec![], xml.into_bytes())
        }
        "GET" => match s.objects.get(key) {
            Some(v) => (200, vec![], v.clone()),
            None => (404, vec![], b"<Error><Code>NoSuchKey</Code><Message>no</Message></Error>".to_vec()),
        },
        "POST" if q.contains_key("uploads") => {
            s.next_upload += 1;
            let id = format!("up{}", s.next_upload);
            s.uploads.insert(id.clone(), BTreeMap::new());
            let xml = format!("<InitiateMultipartUploadResult {NS}><Bucket>b</Bucket><Key>{key}</Key><UploadId>{id}</UploadId></InitiateMultipartUploadResult>");
            (200, vec![], xml.into_bytes())
        }
        "POST" if q.contains_key("uploadId") => {
            let parts = s.uploads.remove(&q["uploadId"]).expect("upload exists");
            let n = String::from_utf8_lossy(&body).matches("<Part>").count();
            assert_eq!(n, parts.len(), "complete lists every uploaded part");
            let joined: Vec<u8> = parts.into_values().flatten().collect();
            s.objects.insert(key.to_string(), joined);
            let xml = format!("<CompleteMultipartUploadResult {NS}><Key>{key}</Key><ETag>\"x\"</ETag></CompleteMultipartUploadResult>");
            (200, vec![], xml.into_bytes())
        }
        "PUT" if q.contains_key("partNumber") => {
            let n: u32 = q["partNumber"].parse().unwrap();
            if s.fail_parts_above.is_some_and(|max| n > max) {
                return (500, vec![], b"<Error><Code>InternalError</Code><Message>boom</Message></Error>".to_vec());
            }
            s.part_puts += 1;
            match s.uploads.get_mut(&q["uploadId"]) {
                Some(u) => {
                    u.insert(n, body);
                    (200, vec![("etag", format!("\"p{n}\""))], vec![])
                }
                None => (404, vec![], b"<Error><Code>NoSuchUpload</Code><Message>no</Message></Error>".to_vec()),
            }
        }
        "PUT" => {
            s.objects.insert(key.to_string(), body);
            (200, vec![("etag", "\"o\"".into())], vec![])
        }
        "DELETE" if q.contains_key("uploadId") => {
            s.uploads.remove(&q["uploadId"]);
            (204, vec![], vec![])
        }
        "DELETE" => {
            s.objects.remove(key);
            (204, vec![], vec![])
        }
        _ => (400, vec![], vec![]),
    }
}

fn r2(endpoint: &str) -> R2 {
    let cfg = R2Config {
        account_id: String::new(),
        bucket: "b".into(),
        access_key_id: "k".into(),
        public_url: None,
        prefix: "team/".into(),
        jurisdiction: None,
        endpoint: Some(endpoint.into()),
    };
    let mut r2 = R2::new(&cfg, "s").unwrap();
    r2.retry_base = Duration::from_millis(1);
    r2
}

fn recording(lib: &Library, size: usize) -> (String, Vec<u8>) {
    let e = lib.create(Some("Demo".into()), datetime!(2026-11-01 13:00 UTC), 0).unwrap();
    let video: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    std::fs::write(e.dir.join(VIDEO), &video).unwrap();
    std::fs::write(e.dir.join(THUMB), [0xFF, 0xD8, 1, 2]).unwrap();
    lib.finalize(&e.meta.id, 3.0, 1920, 1080).unwrap();
    (e.meta.id, video)
}

#[test]
fn small_upload_restore_and_delete() {
    let (endpoint, store) = serve();
    let r2 = r2(&endpoint);
    r2.check().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let lib = Library::open(tmp.path().join("a")).unwrap();
    let (id, video) = recording(&lib, 1000);

    sync::upload(&r2, &lib, &id, |_, _| {}).unwrap();
    assert_eq!(lib.entry(&id).unwrap().status, Status::Synced);
    {
        let s = store.lock().unwrap();
        assert_eq!(s.objects[&format!("team/recordings/{id}/{VIDEO}")], video);
        assert!(!s.objects.contains_key(&format!("team/recordings/{id}/{META}")), "metadata is not public");
        let private =
            s.objects.keys().find(|k| k.starts_with("team/library/") && k.ends_with(&format!("/meta/{id}.json")));
        assert!(private.is_some_and(|k| k.len() > "team/library//meta/.json".len() + id.len() + 16));
        assert!(String::from_utf8_lossy(&s.objects[&format!("team/recordings/{id}/{PAGE}")]).contains("Demo"));
    }

    // A new computer: the library comes back from R2, video on demand.
    let other = Library::open(tmp.path().join("b")).unwrap();
    let restored = sync::restore(&r2, &other, |_| {}).unwrap();
    assert_eq!(restored.added, vec![id.clone()]);
    let e = other.entry(&id).unwrap();
    assert_eq!((e.status, e.meta.title.as_str(), e.meta.width), (Status::Cloud, "Demo", 1920));
    assert!(e.thumb.is_some());
    assert_eq!(sync::restore(&r2, &other, |_| {}).unwrap().already_here, 1);
    sync::download(&r2, &other, &id, |_, _| {}).unwrap();
    assert_eq!(std::fs::read(other.dir(&id).unwrap().join(VIDEO)).unwrap(), video);
    assert_eq!(other.entry(&id).unwrap().status, Status::Synced);

    // Renaming re-uploads only the metadata and the page.
    lib.rename(&id, "Demo final").unwrap();
    assert_eq!(lib.pending_upload().unwrap(), vec![id.clone()]);
    sync::upload(&r2, &lib, &id, |_, _| {}).unwrap();
    assert!(lib.pending_upload().unwrap().is_empty());
    let renamed =
        store.lock().unwrap().objects.iter().any(|(k, v)| {
            k.ends_with(&format!("/meta/{id}.json")) && String::from_utf8_lossy(v).contains("Demo final")
        });
    assert!(renamed);

    // Hand-made folders travel under a random key, never a guessable one.
    lib.create_folder("Clientes/Acme").unwrap();
    assert!(sync::push_folders(&r2, &lib).unwrap());
    assert!(!sync::push_folders(&r2, &lib).unwrap(), "unchanged: not uploaded again");
    let key = store.lock().unwrap().objects.keys().find(|k| k.ends_with("/folders.json")).cloned().unwrap();
    assert!(key.starts_with("team/library/") && key.len() > "team/library//folders.json".len() + 16);
    let third = Library::open(tmp.path().join("c")).unwrap();
    sync::restore(&r2, &third, |_| {}).unwrap();
    assert_eq!(third.saved_folders(), vec!["Clientes/Acme"]);
    store.lock().unwrap().objects.remove(&key);

    sync::delete_remote(&r2, &lib, &id).unwrap();
    assert!(store.lock().unwrap().objects.is_empty());
}

/// Recordings uploaded before the private area (public meta.json) restore
/// fine and get migrated by the next upload.
#[test]
fn legacy_public_meta_is_migrated() {
    let (endpoint, store) = serve();
    let r2 = r2(&endpoint);
    let tmp = tempfile::tempdir().unwrap();
    let old = Library::open(tmp.path().join("old")).unwrap();
    let (id, _) = recording(&old, 1000);
    let meta = serde_json::to_vec(&old.meta(&id).unwrap()).unwrap();
    {
        let mut s = store.lock().unwrap();
        s.objects.insert(format!("team/recordings/{id}/{META}"), meta);
        s.objects.insert(format!("team/recordings/{id}/{VIDEO}"), b"v".to_vec());
    }
    let lib = Library::open(tmp.path().join("new")).unwrap();
    assert_eq!(sync::restore(&r2, &lib, |_| {}).unwrap().added, vec![id.clone()]);
    assert_eq!(lib.pending_upload().unwrap(), vec![id.clone()], "needs migrating");
    sync::upload(&r2, &lib, &id, |_, _| {}).unwrap();
    let s = store.lock().unwrap();
    assert!(!s.objects.contains_key(&format!("team/recordings/{id}/{META}")));
    assert!(s.objects.keys().any(|k| k.ends_with(&format!("/meta/{id}.json"))));
    drop(s);
    assert!(lib.pending_upload().unwrap().is_empty());
}

#[test]
fn multipart_resumes_where_it_stopped() {
    let (endpoint, store) = serve();
    let r2 = r2(&endpoint);
    let tmp = tempfile::tempdir().unwrap();
    let lib = Library::open(tmp.path()).unwrap();
    let (id, video) = recording(&lib, 20 * 1024 * 1024 + 123); // 3 parts of 8 MiB

    store.lock().unwrap().fail_parts_above = Some(2);
    let err = sync::upload(&r2, &lib, &id, |_, _| {}).unwrap_err();
    assert!(err.contains("500"), "{err}");
    let e = lib.entry(&id).unwrap();
    assert_eq!(e.status, Status::Uploading);
    assert!((e.progress.unwrap() - 16.0 * 1024.0 * 1024.0 / video.len() as f64).abs() < 1e-9);
    assert!(e.error.is_some());
    assert_eq!(store.lock().unwrap().part_puts, 2);

    store.lock().unwrap().fail_parts_above = None;
    let mut seen = Vec::new();
    sync::upload(&r2, &lib, &id, |done, _| seen.push(done)).unwrap();
    assert_eq!(store.lock().unwrap().part_puts, 3, "parts 1 and 2 were not sent again");
    assert_eq!(seen.first(), Some(&(16 * 1024 * 1024)));
    let e = lib.entry(&id).unwrap();
    assert_eq!((e.status, e.error), (Status::Synced, None));
    assert_eq!(store.lock().unwrap().objects[&format!("team/recordings/{id}/{VIDEO}")], video);
    assert!(lib.state(&id).multipart.is_none());
}

#[test]
fn share_links() {
    let (endpoint, _store) = serve();
    let mut r2 = r2(&endpoint);
    let (url, expires) = sync::share_link(&r2, "abcdefghij");
    assert!(url.contains("/b/team/recordings/abcdefghij/video.mp4?") && url.contains("X-Amz-Signature"));
    assert!(expires.is_some());
    r2.public_url = Some("https://v.example.com".into());
    assert_eq!(
        sync::share_link(&r2, "abcdefghij"),
        ("https://v.example.com/team/recordings/abcdefghij/index.html".into(), None)
    );
}
