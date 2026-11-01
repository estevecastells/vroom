//! Output: JSON lines for the apps, short text for people.

use std::io::Write;
use std::time::{Duration, Instant};
use vroom_core::i18n::t;
use vroom_core::library::{Entry, Status};
use vroom_core::protocol::Event;
use vroom_core::share::format_duration;

pub struct Out {
    json: bool,
    /// Progress lines are throttled so a fast upload does not flood the app.
    last_progress: std::sync::Mutex<Option<Instant>>,
    /// A warning went out, so a final error only sums them up.
    warned: std::sync::atomic::AtomicBool,
}

impl Out {
    pub fn new(json: bool) -> Out {
        Out { json, last_progress: std::sync::Mutex::new(None), warned: std::sync::atomic::AtomicBool::new(false) }
    }

    pub fn event(&self, event: &Event, text: impl FnOnce() -> String) {
        let mut stdout = std::io::stdout().lock();
        if self.json {
            let _ = stdout.write_all(event.to_line().as_bytes());
        } else {
            let t = text();
            if matches!(event, Event::Error { .. } | Event::Warning { .. }) {
                eprintln!("{}", t.trim_end());
            } else if !t.is_empty() {
                let _ = writeln!(stdout, "{}", t.trim_end());
            }
        }
        let _ = stdout.flush();
    }

    pub fn entry(&self, entry: &Entry) {
        self.event(&Event::Entry { entry: entry.clone() }, || format!("{}  {}", entry.meta.id, entry.dir.display()));
    }

    pub fn ok(&self, message: String) {
        self.event(&Event::Ok { message: message.clone() }, || message);
    }

    pub fn done(&self, id: &str, what: &str) {
        self.event(&Event::Done { id: id.into(), what: what.into() }, || {
            let verb = match what {
                "uploaded" => t("subida"),
                "restored" => t("recuperada"),
                "freed" => t("copia local liberada"),
                "imported" => t("importada"),
                "transcribed" => t("transcrita"),
                other => other.to_string(),
            };
            format!("{id}: {verb}")
        });
    }

    pub fn warned(&self) -> bool {
        self.warned.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn warning(&self, id: Option<&str>, message: &str) {
        self.warned.store(true, std::sync::atomic::Ordering::Relaxed);
        self.event(&Event::Warning { id: id.map(str::to_string), message: message.into() }, || match id {
            Some(id) => format!("{id}: {message}"),
            None => message.to_string(),
        });
    }

    pub fn progress(&self, id: &str, done: u64, total: u64) {
        let now = Instant::now();
        let finished = total > 0 && done >= total;
        {
            let mut last = self.last_progress.lock().unwrap_or_else(|e| e.into_inner());
            if !finished && last.is_some_and(|t| now - t < Duration::from_millis(250)) {
                return;
            }
            *last = Some(now);
        }
        if self.json {
            self.event(&Event::Progress { id: id.into(), done, total }, String::new);
        } else {
            let pct = (done * 100).checked_div(total).unwrap_or(0);
            eprint!("\r{id}: {pct:>3}% ({} / {})", mb(done), mb(total));
            if finished {
                eprintln!();
            }
        }
    }
}

impl Out {
    /// Progress of work measured in percent (transcription), as `done` of 100.
    pub fn percent(&self, id: &str, pct: u64) {
        if self.json {
            self.event(&Event::Progress { id: id.into(), done: pct.min(100), total: 100 }, String::new);
        } else {
            eprint!("\r{id}: {pct:>3}%");
            if pct >= 100 {
                eprintln!();
            }
        }
    }
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

pub fn table(entries: &[Entry]) -> String {
    if entries.is_empty() {
        return t("No hay grabaciones.");
    }
    let status = |s: Status| match s {
        Status::Recording => t("grabando"),
        Status::Local => t("local"),
        Status::Uploading => t("subiendo"),
        Status::Synced => t("en R2"),
        Status::Cloud => t("solo R2"),
    };
    // As wide as the longest status in this language, so the titles line up.
    let width = [Status::Recording, Status::Local, Status::Uploading, Status::Synced, Status::Cloud]
        .map(|s| status(s).chars().count())
        .into_iter()
        .max()
        .unwrap_or(0);
    entries
        .iter()
        .map(|e| {
            format!(
                "{}  {}  {:>8}  {:>9}  {:<width$}  {}",
                e.meta.id,
                e.meta.created_at.date(),
                format_duration(e.meta.duration_secs),
                mb(e.meta.size_bytes),
                status(e.status),
                e.meta.title
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
