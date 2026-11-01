//! Output: JSON lines for the apps, short text for people.

use std::io::Write;
use std::time::{Duration, Instant};
use vroom_core::library::{Entry, Status};
use vroom_core::protocol::Event;
use vroom_core::share::format_duration;

pub struct Out {
    json: bool,
    /// Progress lines are throttled so a fast upload does not flood the app.
    last_progress: std::cell::Cell<Option<Instant>>,
}

impl Out {
    pub fn new(json: bool) -> Out {
        Out { json, last_progress: std::cell::Cell::new(None) }
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
                "uploaded" => "subida",
                "restored" => "recuperada",
                "freed" => "copia local liberada",
                "imported" => "importada",
                other => other,
            };
            format!("{id}: {verb}")
        });
    }

    pub fn warning(&self, id: Option<&str>, message: &str) {
        self.event(&Event::Warning { id: id.map(str::to_string), message: message.into() }, || match id {
            Some(id) => format!("{id}: {message}"),
            None => message.to_string(),
        });
    }

    pub fn progress(&self, id: &str, done: u64, total: u64) {
        let now = Instant::now();
        let finished = total > 0 && done >= total;
        if !finished && self.last_progress.get().is_some_and(|t| now - t < Duration::from_millis(250)) {
            return;
        }
        self.last_progress.set(Some(now));
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

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

pub fn table(entries: &[Entry]) -> String {
    if entries.is_empty() {
        return "No hay grabaciones.".into();
    }
    entries
        .iter()
        .map(|e| {
            let status = match e.status {
                Status::Recording => "grabando",
                Status::Local => "local",
                Status::Uploading => "subiendo",
                Status::Synced => "en R2",
                Status::Cloud => "solo R2",
            };
            format!(
                "{}  {}  {:>8}  {:>9}  {:<8}  {}",
                e.meta.id,
                e.meta.created_at.date(),
                format_duration(e.meta.duration_secs),
                mb(e.meta.size_bytes),
                status,
                e.meta.title
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
