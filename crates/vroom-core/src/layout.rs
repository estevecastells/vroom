//! File names inside a recording folder and object keys in the bucket. The two
//! mirror each other: `<library>/<id>/video.mp4` ↔ `<prefix>recordings/<id>/video.mp4`.

pub const VIDEO: &str = "video.mp4";
pub const META: &str = "meta.json";
pub const THUMB: &str = "thumb.jpg";
pub const PAGE: &str = "index.html";
pub const CAPTIONS: &str = "captions.vtt";
/// Folders created by hand (they can be empty), at the library root and in the
/// bucket's private area: `{"folders": ["Clientes", "Clientes/Acme"]}`.
pub const FOLDERS: &str = "folders.json";
/// Local only: upload progress and cloud state.
pub const STATE: &str = "state.json";

/// Public files uploaded after the video. `meta.json` is not public: it goes,
/// last of all, to the private area (`private_meta_key`), and a restore only
/// trusts recordings whose metadata is there, so everything before it arrived.
pub const AFTER_VIDEO: [&str; 3] = [THUMB, CAPTIONS, PAGE];

/// The bucket may be public, so what only the owner should see (titles of
/// folders, Loom ids, descriptions) lives under `<prefix>library/<secret>/`,
/// a random name that only an authenticated listing finds.
pub fn private_root(prefix: &str) -> String {
    format!("{prefix}library/")
}

pub fn private_meta_key(private: &str, id: &str) -> String {
    format!("{private}meta/{id}.json")
}

/// The id in `<private>meta/<id>.json`.
pub fn id_from_private_meta<'a>(private: &str, key: &'a str) -> Option<&'a str> {
    let id = key.strip_prefix(private)?.strip_prefix("meta/")?.strip_suffix(".json")?;
    crate::id::is_valid(id).then_some(id)
}

pub fn recordings_prefix(prefix: &str) -> String {
    format!("{prefix}recordings/")
}

pub fn key(prefix: &str, id: &str, file: &str) -> String {
    format!("{prefix}recordings/{id}/{file}")
}

/// The id in `<prefix>recordings/<id>/<file>`.
pub fn id_from_key<'a>(prefix: &str, key: &'a str) -> Option<&'a str> {
    let rest = key.strip_prefix(prefix)?.strip_prefix("recordings/")?;
    let (id, _) = rest.split_once('/')?;
    crate::id::is_valid(id).then_some(id)
}

pub fn content_type(file: &str) -> &'static str {
    match file.rsplit('.').next() {
        Some("mp4") => "video/mp4",
        Some("json") => "application/json",
        Some("jpg") => "image/jpeg",
        Some("html") => "text/html; charset=utf-8",
        Some("vtt") => "text/vtt",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip() {
        let k = key("team/", "abcdefghij", VIDEO);
        assert_eq!(k, "team/recordings/abcdefghij/video.mp4");
        assert_eq!(id_from_key("team/", &k), Some("abcdefghij"));
        assert_eq!(id_from_key("", "recordings/../x"), None);
    }
}
