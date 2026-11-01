//! The share page uploaded next to each video (`index.html`). With a public
//! bucket URL it is what a share link opens: the video, its title and date.
//! Relative links only, so it works under any domain or prefix.

use crate::layout::{CAPTIONS, THUMB, VIDEO};
use crate::meta::Meta;

pub fn page(meta: &Meta, has_thumb: bool, has_captions: bool) -> String {
    let title = escape(&meta.title);
    let date = meta.created_at.date();
    let duration = format_duration(meta.duration_secs);
    let poster = if has_thumb { format!(r#" poster="{THUMB}""#) } else { String::new() };
    let track = if has_captions {
        format!(r#"<track kind="captions" src="{CAPTIONS}" srclang="es" label="Subtítulos" default>"#)
    } else {
        String::new()
    };
    let og_image = if has_thumb { format!(r#"<meta property="og:image" content="{THUMB}">"#) } else { String::new() };
    let description =
        meta.description.as_deref().map(|d| format!("<p class=\"desc\">{}</p>", escape(d))).unwrap_or_default();
    format!(
        r#"<!doctype html>
<html lang="es">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>{title}</title>
<meta property="og:title" content="{title}">
<meta property="og:type" content="video.other">
{og_image}
<style>
:root {{ color-scheme: light dark; --bg: #f6f6f4; --fg: #1b1b1a; --muted: #6b6b66; }}
@media (prefers-color-scheme: dark) {{ :root {{ --bg: #111110; --fg: #eeeeea; --muted: #9a9a93; }} }}
* {{ box-sizing: border-box; }}
body {{ margin: 0; background: var(--bg); color: var(--fg); font: 16px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
main {{ max-width: 1200px; margin: 0 auto; padding: 24px 16px 48px; }}
video {{ width: 100%; max-height: 80vh; background: #000; border-radius: 10px; display: block; }}
h1 {{ font-size: 1.4rem; margin: 18px 0 4px; }}
.meta {{ color: var(--muted); font-size: .9rem; }}
.meta a {{ color: inherit; }}
.desc {{ white-space: pre-wrap; }}
</style>
</head>
<body>
<main>
<video src="{VIDEO}" controls playsinline preload="metadata"{poster}>{track}</video>
<h1>{title}</h1>
<div class="meta">{date} · {duration} · <a href="{VIDEO}" download>Descargar</a></div>
{description}
</main>
</body>
</html>
"#
    )
}

pub fn format_duration(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn escapes_the_title() {
        let mut m =
            Meta::new("abcdefghij".into(), "<script>alert(1)</script> & co".into(), datetime!(2026-11-01 13:00 UTC));
        m.duration_secs = 75.4;
        let html = page(&m, true, false);
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; co"));
        assert!(!html.contains("<script>"));
        assert!(html.contains(r#"poster="thumb.jpg""#));
        assert!(html.contains("2026-11-01 · 1:15"));
        assert!(!html.contains("<track"));
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(0.0), "0:00");
        assert_eq!(format_duration(59.6), "1:00");
        assert_eq!(format_duration(3725.0), "1:02:05");
    }
}
