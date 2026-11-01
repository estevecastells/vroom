//! Just enough of a DASH manifest (MPD) to download a Loom video: each
//! Representation's SegmentTemplate, with a SegmentTimeline or a fixed segment
//! duration. Loom's manifests are static and generated, so this reads tags and
//! attributes rather than pulling in an XML parser.

use super::MAX_SEGMENTS;
use vroom_core::i18n::t;
use vroom_core::tr;

#[derive(Debug, Clone, Default)]
pub struct Rep {
    pub id: String,
    pub content: String,
    pub mime: String,
    pub codecs: String,
    pub bandwidth: u64,
    template: Template,
}

#[derive(Debug, Clone, Default)]
struct Template {
    init: String,
    media: String,
    start_number: u64,
    timescale: u64,
    /// Fixed segment length, when there is no timeline.
    duration: u64,
    /// Start time of each segment, from the SegmentTimeline.
    times: Vec<u64>,
}

impl Rep {
    /// The init segment, then every media segment, relative to the manifest.
    pub fn urls(&self) -> Vec<String> {
        let tpl = &self.template;
        let fill = |s: &str, number: u64, time: u64| {
            s.replace("$RepresentationID$", &self.id)
                .replace("$Bandwidth$", &self.bandwidth.to_string())
                .replace("$Number$", &number.to_string())
                .replace("$Time$", &time.to_string())
        };
        let mut out = vec![fill(&tpl.init, 0, 0)];
        out.extend(tpl.times.iter().enumerate().map(|(i, &time)| fill(&tpl.media, tpl.start_number + i as u64, time)));
        out
    }

    /// The ffmpeg demuxer for the joined segments.
    pub fn demuxer(&self) -> &'static str {
        if self.mime.ends_with("/webm") {
            "matroska"
        } else {
            "mp4"
        }
    }

    pub fn is_h264(&self) -> bool {
        self.mime.ends_with("/mp4") && self.codecs.starts_with("avc")
    }

    pub fn is_aac(&self) -> bool {
        self.mime.ends_with("/mp4") && self.codecs.starts_with("mp4a")
    }
}

/// Every Representation in the first Period.
pub fn parse(mpd: &str) -> Result<Vec<Rep>, String> {
    if mpd.contains("<ContentProtection") {
        return Err(t("vídeo DASH cifrado; no soportado"));
    }
    let total = attr(tag_after(mpd, "MPD").unwrap_or(""), "mediaPresentationDuration").map(iso_duration_secs);
    let mut reps = Vec::new();
    // Set-level defaults; a Representation's own values win.
    let (mut set_content, mut set_mime, mut set_tpl) = (String::new(), String::new(), None::<Template>);
    let mut current: Option<Rep> = None;
    for (name, attrs, closing, empty) in tags(mpd) {
        match (name, closing) {
            ("Period", true) => break,
            ("AdaptationSet", false) => {
                set_content = attr(attrs, "contentType").unwrap_or_default().to_string();
                set_mime = attr(attrs, "mimeType").unwrap_or_default().to_string();
                set_tpl = None;
            }
            ("Representation", false) => {
                let mime = attr(attrs, "mimeType").map(str::to_string).unwrap_or_else(|| set_mime.clone());
                let content = if set_content.is_empty() {
                    mime.split('/').next().unwrap_or_default().to_string()
                } else {
                    set_content.clone()
                };
                current = Some(Rep {
                    id: attr(attrs, "id").unwrap_or_default().to_string(),
                    content,
                    mime,
                    codecs: attr(attrs, "codecs").unwrap_or_default().to_string(),
                    bandwidth: attr(attrs, "bandwidth").and_then(|b| b.parse().ok()).unwrap_or(0),
                    template: set_tpl.clone().unwrap_or_default(),
                });
                if empty {
                    reps.extend(current.take());
                }
            }
            ("Representation", true) => reps.extend(current.take()),
            ("SegmentTemplate", false) => {
                let mut tpl = Template {
                    init: attr(attrs, "initialization").unwrap_or_default().to_string(),
                    media: attr(attrs, "media").unwrap_or_default().to_string(),
                    start_number: attr(attrs, "startNumber").and_then(|n| n.parse().ok()).unwrap_or(1),
                    timescale: attr(attrs, "timescale").and_then(|n| n.parse().ok()).unwrap_or(1),
                    duration: attr(attrs, "duration").and_then(|n| n.parse().ok()).unwrap_or(0),
                    times: Vec::new(),
                };
                if tpl.media.contains('%') || tpl.init.contains('%') {
                    return Err(t("plantilla DASH no soportada"));
                }
                if tpl.duration > 0 {
                    let secs =
                        total.filter(|s| s.is_finite() && *s > 0.0).ok_or_else(|| t("lista DASH sin duración"))?;
                    let len = tpl.duration as f64 / tpl.timescale.max(1) as f64;
                    let count = (secs / len).ceil();
                    // Checked before allocating: the manifest comes from the network.
                    if count > MAX_SEGMENTS as f64 {
                        return Err(too_many());
                    }
                    tpl.times = (0..count as u64).map(|i| i * tpl.duration).collect();
                }
                match current.as_mut() {
                    Some(rep) => rep.template = tpl,
                    None => set_tpl = Some(tpl),
                }
            }
            ("S", false) => {
                let tpl = match current.as_mut() {
                    Some(rep) => &mut rep.template,
                    None => set_tpl.get_or_insert_with(Template::default),
                };
                let d: u64 = attr(attrs, "d").and_then(|d| d.parse().ok()).ok_or_else(|| t("lista DASH no válida"))?;
                let repeat: u64 = attr(attrs, "r").and_then(|r| r.parse().ok()).unwrap_or(0);
                let mut time = match attr(attrs, "t").and_then(|t| t.parse().ok()) {
                    Some(t) => t,
                    None => tpl.times.last().map_or(0, |last| last + d),
                };
                if repeat >= (MAX_SEGMENTS - tpl.times.len().min(MAX_SEGMENTS)) as u64 {
                    return Err(too_many());
                }
                for _ in 0..=repeat {
                    tpl.times.push(time);
                    time = time.checked_add(d).ok_or_else(|| t("lista DASH no válida"))?;
                }
            }
            _ => {}
        }
    }
    if reps.iter().any(|r| r.template.media.is_empty()) {
        return Err(t("lista DASH sin plantilla de segmentos"));
    }
    if reps.is_empty() {
        return Err(t("lista DASH vacía"));
    }
    Ok(reps)
}

fn too_many() -> String {
    tr!("lista con demasiados segmentos ({n})", n = format!(">{MAX_SEGMENTS}"))
}

/// The best video and audio Representations, by bandwidth.
pub fn pick(reps: &[Rep]) -> (Option<Rep>, Option<Rep>) {
    let best = |kind: &str| reps.iter().filter(|r| r.content == kind).max_by_key(|r| r.bandwidth).cloned();
    (best("video"), best("audio"))
}

/// `PT1H2M31.32S` → seconds.
fn iso_duration_secs(s: &str) -> f64 {
    let mut total = 0.0;
    let mut number = String::new();
    for c in s.trim_start_matches('P').chars() {
        match c {
            '0'..='9' | '.' => number.push(c),
            'H' | 'M' | 'S' => {
                let n: f64 = number.parse().unwrap_or(0.0);
                total += n * match c {
                    'H' => 3600.0,
                    'M' => 60.0,
                    _ => 1.0,
                };
                number.clear();
            }
            _ => number.clear(),
        }
    }
    total
}

/// Each tag as (name, attributes, is a closing tag, is self-closing).
fn tags(xml: &str) -> impl Iterator<Item = (&str, &str, bool, bool)> {
    xml.split('<').skip(1).filter_map(|chunk| {
        let inner = chunk.split('>').next()?;
        if inner.starts_with('?') || inner.starts_with('!') {
            return None;
        }
        let closing = inner.starts_with('/');
        let empty = inner.ends_with('/');
        let inner = inner.trim_start_matches('/').trim_end_matches('/');
        let (name, attrs) = inner.split_once(char::is_whitespace).unwrap_or((inner, ""));
        Some((name, attrs, closing, empty))
    })
}

fn tag_after<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    tags(xml).find(|(n, _, closing, _)| *n == name && !closing).map(|(_, attrs, _, _)| attrs)
}

/// An attribute's raw value.
fn attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = attrs;
    while let Some(i) = rest.find(&format!("{name}=\"")) {
        let before = rest[..i].chars().next_back();
        let value = &rest[i + name.len() + 2..];
        if before.is_none_or(char::is_whitespace) {
            return value.split('"').next();
        }
        rest = value;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOM: &str = r#"<?xml version="1.0" encoding="UTF-8"?><!--Generated by Loom--><MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT31.320S"><Period id="0"><AdaptationSet id="0" contentType="audio" lang="en" segmentAlignment="true"><Representation id="0" bandwidth="129701" codecs="opus" mimeType="audio/webm" audioSamplingRate="48000"><AudioChannelConfiguration schemeIdUri="x" value="2"/><SegmentTemplate timescale="1000000" initialization="v-audio-init.webm" media="v-audio-$Number$.webm" startNumber="0"><SegmentTimeline><S t="0" d="3998000"/><S t="3998000" d="4080000" r="1"/></SegmentTimeline></SegmentTemplate></Representation></AdaptationSet><AdaptationSet id="1" contentType="video" segmentAlignment="true"><Representation id="original" bandwidth="1600000" codecs="vp9" mimeType="video/webm" width="1920" height="1080"><SegmentTemplate timescale="1000000" initialization="v-video-init.webm" media="v-video-$Number$.webm" startNumber="0"><SegmentTimeline><S t="0" d="4045000"/><S d="4053000"/></SegmentTimeline></SegmentTemplate></Representation><Representation id="1500000" bandwidth="1500000" codecs="vp9" mimeType="video/webm" width="1280" height="720"><SegmentTemplate timescale="1000000" initialization="v-video-bitrate1500-init.webm" media="v-video-bitrate1500-$Number$.webm" startNumber="0"><SegmentTimeline><S t="0" d="4045000"/></SegmentTimeline></SegmentTemplate></Representation></AdaptationSet></Period></MPD>"#;

    #[test]
    fn reads_loom_manifests() {
        let reps = parse(LOOM).unwrap();
        assert_eq!(reps.len(), 3);
        let (video, audio) = pick(&reps);
        let video = video.unwrap();
        let audio = audio.unwrap();
        assert_eq!(video.id, "original");
        assert_eq!(video.urls(), vec!["v-video-init.webm", "v-video-0.webm", "v-video-1.webm"]);
        assert_eq!(audio.urls(), vec!["v-audio-init.webm", "v-audio-0.webm", "v-audio-1.webm", "v-audio-2.webm"]);
        assert_eq!((video.demuxer(), video.is_h264(), audio.is_aac()), ("matroska", false, false));
    }

    #[test]
    fn templates_without_a_timeline() {
        let mpd = r#"<MPD mediaPresentationDuration="PT1M0.5S"><Period><AdaptationSet mimeType="video/mp4"><SegmentTemplate timescale="1000" duration="20000" initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/$Time$.m4s"/><Representation id="hd" bandwidth="1" codecs="avc1.64001f"/></AdaptationSet></Period></MPD>"#;
        let reps = parse(mpd).unwrap();
        assert_eq!(reps[0].content, "video");
        assert!(reps[0].is_h264());
        assert_eq!(reps[0].urls(), vec!["hd/init.mp4", "hd/0.m4s", "hd/20000.m4s", "hd/40000.m4s", "hd/60000.m4s"]);
        assert_eq!(iso_duration_secs("PT1H2M31.32S"), 3751.32);
    }

    #[test]
    fn refuses_what_it_cannot_download() {
        assert!(parse(r#"<MPD><Period><AdaptationSet><ContentProtection/></AdaptationSet></Period></MPD>"#).is_err());
        assert!(parse(r#"<MPD><Period></Period></MPD>"#).is_err());
        let huge = r#"<MPD><Period><AdaptationSet mimeType="video/webm"><Representation id="v"><SegmentTemplate media="$Number$" initialization="i"><SegmentTimeline><S t="0" d="1" r="18446744073709551614"/></SegmentTimeline></SegmentTemplate></Representation></AdaptationSet></Period></MPD>"#;
        assert!(parse(huge).is_err());
        let tiny = r#"<MPD mediaPresentationDuration="PT999999999S"><Period><AdaptationSet mimeType="video/webm"><Representation id="v"><SegmentTemplate media="$Number$" initialization="i" timescale="1000000" duration="1"/></Representation></AdaptationSet></Period></MPD>"#;
        assert!(parse(tiny).is_err());
    }
}
