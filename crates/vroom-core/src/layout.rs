//! File names inside a recording folder and object keys in the bucket. The two
//! mirror each other: `<library>/<id>/video.mp4` ↔ `<prefix>recordings/<id>/video.mp4`.

pub const VIDEO: &str = "video.mp4";
pub const META: &str = "meta.json";
pub const THUMB: &str = "thumb.jpg";
pub const PAGE: &str = "index.html";
pub const CAPTIONS: &str = "captions.vtt";
/// Local only: upload progress and cloud state.
pub const STATE: &str = "state.json";

/// Files uploaded after the video, in order. `meta.json` goes last: a restore
/// only trusts recordings whose `meta.json` is in the bucket, so its presence
/// means everything before it arrived.
pub const AFTER_VIDEO: [&str; 4] = [THUMB, CAPTIONS, PAGE, META];

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
