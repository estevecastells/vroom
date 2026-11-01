//! The window (library and settings) and the app's state. Mirrors
//! app/Sources/VroomUI/AppModel.swift: anything slow runs on a worker thread
//! that reports back through `Msg`, and every library change goes through
//! `vroom.exe`.

use crate::cli::Cli;
use crate::prefs::{CaptureMode, Prefs};
use crate::recorder::{self, Options, Recorder, StartError, Target};
use crate::tray::{Tray, TrayMsg};
use eframe::egui::{self, Color32, RichText, Sense, Ui, Vec2};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};
use vroom_core::config::Config;
use vroom_core::layout::{THUMB, VIDEO};
use vroom_core::library::{Entry, Status};
use vroom_core::protocol::Event;
use vroom_core::share::format_duration;

enum Msg {
    Entries(Vec<Entry>),
    Folders(Vec<String>),
    Config { config: Config, library: String, has_secret: bool },
    Transcriber { model: Option<String>, whisper: Option<String> },
    Transfer(String, Option<f64>),
    Transcribing(String, Option<f64>),
    TranscribeFinished,
    SyncFinished,
    ModelDownload(Option<f64>),
    Started(Box<(Entry, Recorder)>),
    StartAborted,
    Recorded(Entry),
    Copy(String, String),
    Toast(String),
    Error(String),
    Loom(String),
    LoomFinished,
}

/// Sends from worker threads and wakes the UI, even while it is hidden.
#[derive(Clone)]
struct Mailbox {
    tx: Sender<Msg>,
    ctx: egui::Context,
}

impl Mailbox {
    fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
        self.ctx.request_repaint();
    }
}

enum Phase {
    Idle,
    Starting,
    Recording { entry: Entry, recorder: Recorder },
    Stopping,
}

#[derive(PartialEq, Clone)]
enum Filter {
    All,
    Unfiled,
    Folder(String),
}

#[derive(PartialEq)]
enum Tab {
    Library,
    Settings,
}

enum Dialog {
    Rename(Entry, String),
    Move(Entry, String),
    Delete(Entry),
    NewFolder(String),
    RenameFolder(String, String),
    DeleteFolder(String),
}

/// What a card's menu asked for; applied after the grid is drawn.
enum Action {
    Play(Entry),
    CopyLink(Entry),
    Reveal(Entry),
    Transcribe(Entry),
    Upload,
    Free(Entry),
    Download(Entry),
    Dialog(Dialog),
}

#[derive(Default)]
struct R2Form {
    account_id: String,
    bucket: String,
    access_key_id: String,
    secret: String,
    public_url: String,
    prefix: String,
    eu: bool,
}

pub struct VroomApp {
    cli: Option<Cli>,
    mail: Mailbox,
    rx: Receiver<Msg>,
    tray_rx: Receiver<TrayMsg>,
    tray: Option<Tray>,
    prefs: Prefs,
    phase: Phase,
    entries: Vec<Entry>,
    folders: Vec<String>,
    config: Option<(Config, String, bool)>,
    transcriber: (Option<String>, Option<String>),
    transfers: HashMap<String, f64>,
    transcribing: HashMap<String, f64>,
    transcribe_queue: VecDeque<String>,
    model_download: Option<f64>,
    syncing: bool,
    sync_again: bool,
    toast: Option<(String, Instant)>,
    error: Option<String>,
    filter: Filter,
    tab: Tab,
    dialog: Option<Dialog>,
    r2_form: R2Form,
    library_path: String,
    loom_cookie: String,
    loom_log: Vec<String>,
    loom_running: bool,
    monitors: Vec<String>,
    microphones: Vec<String>,
    quitting: bool,
}

impl VroomApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let (tx, rx) = channel();
        let (tray_tx, tray_rx) = channel();
        let mail = Mailbox { tx, ctx: cc.egui_ctx.clone() };
        let mut error = None;
        let tray = match Tray::new(cc.egui_ctx.clone(), tray_tx) {
            Ok(t) => Some(t),
            Err(e) => {
                error = Some(format!("icono de la bandeja: {e}"));
                None
            }
        };
        let cli = Cli::locate();
        if cli.is_none() {
            error = Some("No se encuentra vroom.exe: debe estar en la misma carpeta que Vroom.exe.".into());
        }
        let mut app = VroomApp {
            cli,
            mail,
            rx,
            tray_rx,
            tray,
            prefs: Prefs::load(),
            phase: Phase::Idle,
            entries: vec![],
            folders: vec![],
            config: None,
            transcriber: (None, None),
            transfers: HashMap::new(),
            transcribing: HashMap::new(),
            transcribe_queue: VecDeque::new(),
            model_download: None,
            syncing: false,
            sync_again: false,
            toast: None,
            error,
            filter: Filter::All,
            tab: Tab::Library,
            dialog: None,
            r2_form: R2Form::default(),
            library_path: String::new(),
            loom_cookie: String::new(),
            loom_log: vec![],
            loom_running: false,
            monitors: recorder::monitors(),
            microphones: crate::audio::microphones(),
            quitting: false,
        };
        app.boot();
        app
    }

    /// Run `job` on a worker thread with the CLI.
    fn spawn(&self, job: impl FnOnce(&Cli, &Mailbox) + Send + 'static) {
        let Some(cli) = self.cli.clone() else { return };
        let mail = self.mail.clone();
        std::thread::spawn(move || job(&cli, &mail));
    }

    fn boot(&mut self) {
        self.spawn(|cli, mail| {
            // Recordings left half-written by a crash become normal ones.
            let _ = cli.run(&["recover"]);
            load_config(cli, mail);
            load_library(cli, mail);
            load_transcriber(cli, mail);
        });
        self.sync();
    }

    fn refresh(&self) {
        self.spawn(load_library);
    }

    fn cloud_ready(&self) -> bool {
        self.config.as_ref().is_some_and(|(c, _, secret)| c.r2.is_some() && *secret)
    }

    fn can_transcribe(&self) -> bool {
        self.transcriber.0.is_some() && self.transcriber.1.is_some()
    }

    fn show_window(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    // MARK: Recording

    fn toggle_recording(&mut self, ctx: &egui::Context) {
        match self.phase {
            Phase::Idle => self.start_recording(ctx),
            Phase::Recording { .. } => self.stop_recording(false),
            Phase::Starting | Phase::Stopping => {}
        }
    }

    fn start_recording(&mut self, ctx: &egui::Context) {
        if self.cli.is_none() {
            return;
        }
        self.phase = Phase::Starting;
        // The library window must not be in the video.
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        let prefs = self.prefs.clone();
        let folder = match &self.filter {
            Filter::Folder(f) => Some(f.clone()),
            _ => None,
        };
        self.spawn(move |cli, mail| {
            let pid = std::process::id().to_string();
            let mut args = vec!["new", "--owner-pid", pid.as_str()];
            if let Some(f) = folder.as_deref() {
                args.extend(["--folder", f]);
            }
            let entry = match cli.entry(&args) {
                Ok(e) => e,
                Err(e) => {
                    mail.send(Msg::Error(e));
                    mail.send(Msg::StartAborted);
                    return;
                }
            };
            let options = Options {
                target: match prefs.capture_mode {
                    CaptureMode::Screen => Target::Monitor(prefs.monitor),
                    CaptureMode::Pick => Target::Pick,
                },
                microphone: prefs.microphone.clone(),
                system_audio: prefs.system_audio,
                fps: prefs.fps,
                video: entry.dir.join(VIDEO),
                thumb: entry.dir.join(THUMB),
            };
            match Recorder::start(options) {
                Ok(recorder) => mail.send(Msg::Started(Box::new((entry, recorder)))),
                Err(e) => {
                    let _ = cli.run(&["delete", &entry.meta.id, "--keep-cloud"]);
                    if let StartError::Failed(message) = e {
                        mail.send(Msg::Error(message));
                    }
                    mail.send(Msg::StartAborted);
                }
            }
        });
    }

    fn stop_recording(&mut self, discard: bool) {
        let Phase::Recording { entry, recorder } = std::mem::replace(&mut self.phase, Phase::Stopping) else { return };
        self.spawn(move |cli, mail| {
            let id = entry.meta.id.clone();
            let result = recorder.stop();
            if discard {
                let _ = cli.run(&["delete", &id, "--keep-cloud"]);
                mail.send(Msg::Toast("Grabación descartada".into()));
                mail.send(Msg::StartAborted);
                load_library(cli, mail);
                return;
            }
            let recorded = match result {
                Ok(r) => r,
                Err(e) => {
                    // What reached the disk stays; `recover` makes it a normal recording.
                    mail.send(Msg::Error(format!("La grabación no se cerró bien: {e}")));
                    let _ = cli.run(&["recover"]);
                    mail.send(Msg::StartAborted);
                    load_library(cli, mail);
                    return;
                }
            };
            let (d, w, h) = (recorded.duration.to_string(), recorded.width.to_string(), recorded.height.to_string());
            match cli.entry(&["finalize", &id, "--duration", &d, "--width", &w, "--height", &h]) {
                Ok(entry) => mail.send(Msg::Recorded(entry)),
                Err(e) => {
                    mail.send(Msg::Error(e));
                    mail.send(Msg::StartAborted);
                }
            }
            load_library(cli, mail);
        });
    }

    /// Like Loom: the link is on the clipboard as soon as the recording stops; it
    /// works once the upload (already running) finishes.
    fn after_recording(&mut self, entry: Entry) {
        if self.prefs.transcribe && self.can_transcribe() {
            self.transcribe(&entry);
        }
        let auto_upload = self.config.as_ref().is_some_and(|(c, _, _)| c.auto_upload);
        if !(self.cloud_ready() && auto_upload) {
            self.toast("Grabación guardada");
            return;
        }
        let id = entry.meta.id.clone();
        self.spawn(move |cli, mail| {
            if let Ok(url) = cli.link(&["share", &id, "--before-upload"]) {
                mail.send(Msg::Copy(url, "Enlace copiado; se está subiendo".into()));
            }
        });
        self.sync();
    }

    // MARK: Cloud

    /// Upload everything pending. Runs once at a time; asks made meanwhile run after it.
    fn sync(&mut self) {
        if !self.cloud_ready() && self.config.is_some() {
            return;
        }
        if self.syncing {
            self.sync_again = true;
            return;
        }
        self.syncing = true;
        self.spawn(|cli, mail| {
            let result = cli.stream(&["sync"], |e| match e {
                Event::Progress { id, done, total } if total > 0 => {
                    mail.send(Msg::Transfer(id, Some(done as f64 / total as f64)))
                }
                Event::Done { id, .. } => mail.send(Msg::Transfer(id, None)),
                Event::Warning { message, .. } => mail.send(Msg::Error(message)),
                _ => {}
            });
            if let Err(m) = result {
                // Per-recording failures were warnings; a Loom import may hold the lock.
                if !m.contains("se reintentarán") && !m.contains("sincronización en marcha") && !m.contains("R2 no") {
                    mail.send(Msg::Error(m));
                }
            }
            mail.send(Msg::SyncFinished);
            load_library(cli, mail);
        });
    }

    // MARK: Transcription

    fn transcribe(&mut self, entry: &Entry) {
        let id = entry.meta.id.clone();
        if !self.can_transcribe() || entry.video.is_none() || self.transcribing.contains_key(&id) {
            return;
        }
        self.transcribing.insert(id.clone(), 0.0);
        self.transcribe_queue.push_back(id);
        if self.transcribe_queue.len() == 1 {
            self.transcribe_next();
        }
    }

    /// One transcription at a time: each takes the whole GPU.
    fn transcribe_next(&mut self) {
        let Some(id) = self.transcribe_queue.front().cloned() else { return };
        self.spawn(move |cli, mail| {
            let result = cli.stream(&["transcribe", &id], |e| match e {
                Event::Progress { id, done, total } if total > 0 => {
                    mail.send(Msg::Transcribing(id, Some(done as f64 / total as f64)))
                }
                Event::Warning { message, .. } => mail.send(Msg::Error(format!("Transcripción: {message}"))),
                _ => {}
            });
            if let Err(e) = result {
                if !e.contains("transcripciones fallaron") {
                    mail.send(Msg::Error(format!("Transcripción: {e}")));
                }
            }
            mail.send(Msg::Transcribing(id, None));
            mail.send(Msg::TranscribeFinished);
        });
    }

    // MARK: Messages

    fn handle(&mut self, msg: Msg, ctx: &egui::Context) {
        match msg {
            Msg::Entries(e) => self.entries = e,
            Msg::Folders(f) => self.folders = f,
            Msg::Config { config, library, has_secret } => {
                if self.library_path.is_empty() {
                    self.library_path = library.clone();
                }
                self.config = Some((config, library, has_secret));
            }
            Msg::Transcriber { model, whisper } => self.transcriber = (model, whisper),
            Msg::Transfer(id, Some(p)) => {
                self.transfers.insert(id, p);
            }
            Msg::Transfer(id, None) => {
                self.transfers.remove(&id);
            }
            Msg::Transcribing(id, Some(p)) => {
                self.transcribing.insert(id, p);
            }
            Msg::Transcribing(id, None) => {
                self.transcribing.remove(&id);
            }
            Msg::TranscribeFinished => {
                self.transcribe_queue.pop_front();
                self.refresh();
                // New captions go up with the next sync.
                self.sync();
                self.transcribe_next();
            }
            Msg::SyncFinished => {
                self.syncing = false;
                self.transfers.clear();
                if std::mem::take(&mut self.sync_again) {
                    self.sync();
                }
            }
            Msg::ModelDownload(p) => {
                self.model_download = p;
                if p.is_none() {
                    self.spawn(load_transcriber);
                }
            }
            Msg::Started(started) => {
                let (entry, recorder) = *started;
                self.phase = Phase::Recording { entry, recorder };
                if let Some(t) = &self.tray {
                    t.set_recording(true);
                }
            }
            Msg::StartAborted => {
                self.phase = Phase::Idle;
                if let Some(t) = &self.tray {
                    t.set_recording(false);
                }
            }
            Msg::Recorded(entry) => {
                self.phase = Phase::Idle;
                if let Some(t) = &self.tray {
                    t.set_recording(false);
                }
                self.show_window(ctx);
                self.after_recording(entry);
            }
            Msg::Copy(text, message) => {
                ctx.copy_text(text);
                self.toast(message);
            }
            Msg::Toast(t) => self.toast(t),
            Msg::Error(e) => self.error = Some(e),
            Msg::Loom(line) => self.loom_log.push(line),
            Msg::LoomFinished => {
                self.loom_running = false;
                self.refresh();
            }
        }
    }

    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Play(entry) => {
                if let Some(video) = &entry.video {
                    open(video.as_os_str());
                    return;
                }
                let public =
                    self.config.as_ref().is_some_and(|(c, _, _)| c.r2.as_ref().is_some_and(|r| r.public_url.is_some()));
                let id = entry.meta.id.clone();
                self.spawn(move |cli, mail| {
                    // The share page when the bucket is public; otherwise the signed video.
                    let args: [&str; 2] = if public { ["share", &id] } else { ["stream-url", &id] };
                    match cli.link(&args) {
                        Ok(url) => open(std::ffi::OsStr::new(&url)),
                        Err(e) => mail.send(Msg::Error(e)),
                    }
                });
            }
            Action::CopyLink(entry) => {
                let id = entry.meta.id.clone();
                let in_cloud = matches!(entry.status, Status::Synced | Status::Cloud);
                self.spawn(move |cli, mail| {
                    let mut args = vec!["share", id.as_str()];
                    if !in_cloud {
                        args.push("--before-upload");
                    }
                    match cli.link(&args) {
                        Ok(url) => {
                            let m = if in_cloud {
                                "Enlace copiado"
                            } else {
                                "Enlace copiado; funcionará al terminar la subida"
                            };
                            mail.send(Msg::Copy(url, m.into()));
                        }
                        Err(e) => mail.send(Msg::Error(e)),
                    }
                });
            }
            Action::Reveal(entry) => {
                let target = entry.video.clone().unwrap_or(entry.dir.clone());
                let mut arg = std::ffi::OsString::from("/select,");
                arg.push(target.as_os_str());
                let _ = std::process::Command::new("explorer").arg(arg).spawn();
            }
            Action::Transcribe(entry) => self.transcribe(&entry),
            Action::Upload => self.sync(),
            Action::Free(entry) => {
                self.run_then_refresh(vec!["free".into(), entry.meta.id], "Copia local borrada; sigue en R2")
            }
            Action::Download(entry) => {
                let id = entry.meta.id.clone();
                self.transfers.insert(id.clone(), 0.0);
                self.spawn(move |cli, mail| {
                    let result = cli.stream(&["download", &id], |e| {
                        if let Event::Progress { id, done, total } = e {
                            if total > 0 {
                                mail.send(Msg::Transfer(id, Some(done as f64 / total as f64)));
                            }
                        }
                    });
                    if let Err(e) = result {
                        mail.send(Msg::Error(e));
                    }
                    mail.send(Msg::Transfer(id, None));
                    load_library(cli, mail);
                });
            }
            Action::Dialog(d) => self.dialog = Some(d),
        }
        ctx.request_repaint();
    }

    /// A short library command, then a refresh and a sync (titles and folders go to R2 too).
    fn run_then_refresh(&mut self, args: Vec<String>, done: &'static str) {
        self.spawn(move |cli, mail| {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            match cli.run(&args) {
                Ok(_) => {
                    if !done.is_empty() {
                        mail.send(Msg::Toast(done.into()));
                    }
                }
                Err(e) => mail.send(Msg::Error(e)),
            }
            load_library(cli, mail);
        });
        self.sync();
    }

    fn set_config(&mut self, key: &str, value: String) {
        let key = key.to_string();
        self.spawn(move |cli, mail| {
            if let Err(e) = cli.run(&["config", "set", &key, &value]) {
                mail.send(Msg::Error(e));
            }
            load_config(cli, mail);
            load_library(cli, mail);
        });
    }
}

fn load_library(cli: &Cli, mail: &Mailbox) {
    match cli.entries() {
        Ok(e) => mail.send(Msg::Entries(e)),
        Err(e) => mail.send(Msg::Error(e)),
    }
    if let Ok(f) = cli.folders(&["folders"]) {
        mail.send(Msg::Folders(f));
    }
}

fn load_config(cli: &Cli, mail: &Mailbox) {
    if let Ok(events) = cli.run(&["config"]) {
        for e in events {
            if let Event::Config { config, library, has_secret, .. } = e {
                mail.send(Msg::Config { config, library, has_secret });
            }
        }
    }
}

fn load_transcriber(cli: &Cli, mail: &Mailbox) {
    if let Ok(events) = cli.run(&["transcribe", "--status"]) {
        for e in events {
            if let Event::Transcriber { model, whisper } = e {
                mail.send(Msg::Transcriber { model, whisper });
            }
        }
    }
}

/// Opens a file or URL with its default app.
fn open(target: &std::ffi::OsStr) {
    let _ = std::process::Command::new("explorer").arg(target).spawn();
}

fn status_text(entry: &Entry) -> &'static str {
    match entry.status {
        Status::Recording => "grabando",
        Status::Local => "solo en este PC",
        Status::Uploading => "subiendo",
        Status::Synced => "en R2",
        Status::Cloud => "solo en R2",
    }
}

fn file_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

impl eframe::App for VroomApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.tray_rx.try_recv() {
            match msg {
                TrayMsg::Toggle => self.toggle_recording(ctx),
                TrayMsg::Cancel => self.stop_recording(true),
                TrayMsg::Show => self.show_window(ctx),
                TrayMsg::Quit => {
                    if matches!(self.phase, Phase::Recording { .. }) {
                        self.stop_recording(false);
                    }
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg, ctx);
        }
        // Closing the window keeps Vroom in the tray, like the Mac menu bar app.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        if self.toast.as_ref().is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(4)) {
            self.toast = None;
        }
        if matches!(self.phase, Phase::Recording { .. }) || self.toast.is_some() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("top").show(ui, |ui| self.top_bar(ui, &ctx));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        match self.tab {
            Tab::Library => {
                egui::Panel::left("folders").default_size(190.0).show(ui, |ui| self.sidebar(ui));
                egui::CentralPanel::default().show(ui, |ui| self.grid(ui, &ctx));
            }
            Tab::Settings => {
                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| self.settings(ui));
                });
            }
        }
        self.dialogs(&ctx);
    }
}

// MARK: Views

impl VroomApp {
    fn top_bar(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            match &self.phase {
                Phase::Idle => {
                    let record = egui::Button::new(RichText::new("● Grabar").color(Color32::WHITE).strong())
                        .fill(Color32::from_rgb(220, 38, 69));
                    if ui.add_enabled(self.cli.is_some(), record).on_hover_text("Alt+Mayús+R").clicked() {
                        self.start_recording(ctx);
                    }
                    egui::ComboBox::from_id_salt("mode")
                        .selected_text(match self.prefs.capture_mode {
                            CaptureMode::Screen => "Pantalla completa",
                            CaptureMode::Pick => "Elegir ventana…",
                        })
                        .show_ui(ui, |ui| {
                            let before = self.prefs.capture_mode;
                            ui.selectable_value(&mut self.prefs.capture_mode, CaptureMode::Screen, "Pantalla completa");
                            ui.selectable_value(
                                &mut self.prefs.capture_mode,
                                CaptureMode::Pick,
                                "Elegir ventana o pantalla…",
                            );
                            if self.prefs.capture_mode != before {
                                self.prefs.save();
                            }
                        });
                }
                Phase::Recording { recorder, .. } => {
                    let elapsed = format_duration(recorder.elapsed().as_secs_f64());
                    ui.label(RichText::new(format!("● {elapsed}")).color(Color32::from_rgb(220, 38, 69)).strong());
                    if ui.button("■ Detener").clicked() {
                        self.stop_recording(false);
                    }
                    if ui.button("Descartar").clicked() {
                        self.stop_recording(true);
                    }
                }
                Phase::Starting => {
                    ui.spinner();
                    ui.label("Preparando…");
                }
                Phase::Stopping => {
                    ui.spinner();
                    ui.label("Guardando…");
                }
            }
            ui.separator();
            ui.selectable_value(&mut self.tab, Tab::Library, "Biblioteca");
            ui.selectable_value(&mut self.tab, Tab::Settings, "Ajustes");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.syncing {
                    ui.spinner();
                } else if ui.add_enabled(self.cloud_ready(), egui::Button::new("Sincronizar")).clicked() {
                    self.sync();
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if let Some(e) = self.error.clone() {
                ui.label(RichText::new(e).color(Color32::from_rgb(220, 60, 60)));
                if ui.small_button("✕").clicked() {
                    self.error = None;
                }
            } else if let Some((t, _)) = &self.toast {
                ui.label(t);
            } else {
                ui.weak(format!("{} grabaciones", self.entries.len()));
            }
        });
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.selectable_value(&mut self.filter, Filter::All, "Todas");
        ui.selectable_value(&mut self.filter, Filter::Unfiled, "Sin carpeta");
        ui.separator();
        for folder in self.folders.clone() {
            let depth = folder.matches('/').count();
            let name = folder.rsplit('/').next().unwrap_or(&folder).to_string();
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 14.0);
                let selected = self.filter == Filter::Folder(folder.clone());
                let r = ui.selectable_label(selected, format!("📁 {name}"));
                if r.clicked() {
                    self.filter = Filter::Folder(folder.clone());
                }
                r.context_menu(|ui| {
                    if ui.button("Nueva subcarpeta…").clicked() {
                        self.dialog = Some(Dialog::NewFolder(format!("{folder}/")));
                    }
                    if ui.button("Renombrar…").clicked() {
                        self.dialog = Some(Dialog::RenameFolder(folder.clone(), folder.clone()));
                    }
                    if ui.button("Eliminar carpeta…").clicked() {
                        self.dialog = Some(Dialog::DeleteFolder(folder.clone()));
                    }
                });
            });
        }
        ui.add_space(6.0);
        if ui.button("＋ Nueva carpeta").clicked() {
            self.dialog = Some(Dialog::NewFolder(String::new()));
        }
    }

    fn grid(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let filtered: Vec<Entry> = self
            .entries
            .iter()
            .filter(|e| match &self.filter {
                Filter::All => true,
                Filter::Unfiled => e.meta.folder.is_none(),
                Filter::Folder(f) => e.meta.folder.as_ref() == Some(f),
            })
            .cloned()
            .collect();
        if filtered.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.weak("Sin grabaciones. Pulsa Alt+Mayús+R o el botón Grabar para empezar.");
            });
            return;
        }
        let mut action = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(16.0, 18.0);
                for entry in &filtered {
                    if let Some(a) = self.card(ui, entry) {
                        action = Some(a);
                    }
                }
            });
        });
        if let Some(a) = action {
            self.apply(a, ctx);
        }
    }

    fn card(&self, ui: &mut Ui, entry: &Entry) -> Option<Action> {
        let width = 240.0;
        let mut action = None;
        let inner = ui.allocate_ui(Vec2::new(width, 210.0), |ui| {
            ui.vertical(|ui| {
                ui.set_width(width);
                let size = Vec2::new(width, width * 10.0 / 16.0);
                match &entry.thumb {
                    Some(t) => {
                        ui.add(egui::Image::new(file_uri(t)).fit_to_exact_size(size).corner_radius(8));
                    }
                    None => {
                        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                        ui.painter().rect_filled(rect, 8.0, ui.visuals().faint_bg_color);
                    }
                }
                if let Some(p) = self.transfers.get(&entry.meta.id).copied().or(entry.progress) {
                    ui.add(egui::ProgressBar::new(p as f32).desired_height(4.0));
                }
                if let Some(t) = self.transcribing.get(&entry.meta.id) {
                    ui.small(if *t > 0.0 {
                        format!("Transcribiendo {} %", (t * 100.0) as u32)
                    } else {
                        "Transcripción en cola".into()
                    });
                }
                ui.label(RichText::new(&entry.meta.title).strong());
                let mut info = format!("{} · {}", format_duration(entry.meta.duration_secs), status_text(entry));
                if entry.captions.is_some() {
                    info.push_str(" · CC");
                }
                if entry.meta.source == vroom_core::Source::Loom {
                    info.push_str(" · Loom");
                }
                ui.weak(info);
            });
        });
        let response = inner.response.interact(Sense::click());
        if response.double_clicked() {
            action = Some(Action::Play(entry.clone()));
        }
        response.context_menu(|ui| {
            let mut pick = |a: Action| action = Some(a);
            if ui.button("Reproducir").clicked() {
                pick(Action::Play(entry.clone()));
            }
            if ui.add_enabled(self.cloud_ready(), egui::Button::new("Copiar enlace")).clicked() {
                pick(Action::CopyLink(entry.clone()));
            }
            if ui.button("Renombrar…").clicked() {
                pick(Action::Dialog(Dialog::Rename(entry.clone(), entry.meta.title.clone())));
            }
            if ui.button("Mover a carpeta…").clicked() {
                pick(Action::Dialog(Dialog::Move(entry.clone(), entry.meta.folder.clone().unwrap_or_default())));
            }
            if entry.video.is_some() {
                if ui.button("Mostrar en el Explorador").clicked() {
                    pick(Action::Reveal(entry.clone()));
                }
                let label = if entry.captions.is_none() { "Transcribir" } else { "Volver a transcribir" };
                let enabled = self.can_transcribe() && !self.transcribing.contains_key(&entry.meta.id);
                if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                    pick(Action::Transcribe(entry.clone()));
                }
            }
            ui.separator();
            match entry.status {
                Status::Local | Status::Uploading => {
                    if ui.add_enabled(self.cloud_ready(), egui::Button::new("Subir ahora")).clicked() {
                        pick(Action::Upload);
                    }
                }
                Status::Synced => {
                    if ui.button("Liberar espacio en este PC").clicked() {
                        pick(Action::Free(entry.clone()));
                    }
                }
                Status::Cloud => {
                    if ui.button("Descargar a este PC").clicked() {
                        pick(Action::Download(entry.clone()));
                    }
                }
                Status::Recording => {}
            }
            ui.separator();
            if ui.button(RichText::new("Eliminar…").color(Color32::from_rgb(220, 60, 60))).clicked() {
                pick(Action::Dialog(Dialog::Delete(entry.clone())));
            }
        });
        action
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let mut close = false;
        let mut run: Option<(Vec<String>, &'static str)> = None;
        let modal = egui::Modal::new(egui::Id::new("dialog")).show(ctx, |ui| {
            ui.set_width(360.0);
            match &mut dialog {
                Dialog::Rename(entry, title) => {
                    ui.heading("Renombrar");
                    ui.text_edit_singleline(title);
                    if ok_cancel(ui, "Guardar", &mut close) {
                        run = Some((vec!["rename".into(), entry.meta.id.clone(), "--".into(), title.clone()], ""));
                    }
                }
                Dialog::Move(entry, folder) => {
                    ui.heading("Mover a carpeta");
                    ui.label("Carpeta (vacía para el nivel superior; usa / para subcarpetas):");
                    ui.text_edit_singleline(folder);
                    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                        for f in &self.folders {
                            if ui.selectable_label(folder == f, f).clicked() {
                                *folder = f.clone();
                            }
                        }
                    });
                    if ok_cancel(ui, "Mover", &mut close) {
                        run = Some((vec!["move".into(), entry.meta.id.clone(), "--".into(), folder.clone()], ""));
                    }
                }
                Dialog::Delete(entry) => {
                    ui.heading("¿Eliminar la grabación?");
                    ui.label(&entry.meta.title);
                    let in_cloud = matches!(entry.status, Status::Synced | Status::Cloud | Status::Uploading);
                    ui.horizontal(|ui| {
                        if in_cloud && ui.button("Eliminar de este PC y de R2").clicked() {
                            run = Some((vec!["delete".into(), entry.meta.id.clone()], "Grabación eliminada"));
                        }
                        let label = if in_cloud { "Eliminar solo de este PC" } else { "Eliminar" };
                        if ui.button(label).clicked() {
                            run = Some((
                                vec!["delete".into(), entry.meta.id.clone(), "--keep-cloud".into()],
                                "Grabación eliminada",
                            ));
                        }
                        if ui.button("Cancelar").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::NewFolder(path) => {
                    ui.heading("Nueva carpeta");
                    ui.text_edit_singleline(path);
                    if ok_cancel(ui, "Crear", &mut close) {
                        run = Some((vec!["folder".into(), "create".into(), "--".into(), path.clone()], ""));
                    }
                }
                Dialog::RenameFolder(from, to) => {
                    ui.heading("Renombrar carpeta");
                    ui.text_edit_singleline(to);
                    if ok_cancel(ui, "Guardar", &mut close) {
                        run = Some((vec!["folder".into(), "rename".into(), "--".into(), from.clone(), to.clone()], ""));
                    }
                }
                Dialog::DeleteFolder(path) => {
                    ui.heading("¿Eliminar la carpeta?");
                    ui.label(format!("Las grabaciones de «{path}» pasan a la carpeta de arriba."));
                    if ok_cancel(ui, "Eliminar carpeta", &mut close) {
                        run = Some((vec!["folder".into(), "delete".into(), "--".into(), path.clone()], ""));
                    }
                }
            }
        });
        if modal.should_close() {
            close = true;
        }
        if let Some((args, done)) = run {
            if let Dialog::DeleteFolder(_) | Dialog::RenameFolder(..) = dialog {
                self.filter = Filter::All;
            }
            self.run_then_refresh(args, done);
            close = true;
        }
        if !close {
            self.dialog = Some(dialog);
        }
    }

    fn settings(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        ui.heading("Grabación");
        let mut prefs = self.prefs.clone();
        egui::Grid::new("recording").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            ui.label("Qué grabar");
            egui::ComboBox::from_id_salt("s-mode")
                .selected_text(match prefs.capture_mode {
                    CaptureMode::Screen => "Pantalla completa",
                    CaptureMode::Pick => "Elegir ventana o pantalla al grabar",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut prefs.capture_mode, CaptureMode::Screen, "Pantalla completa");
                    ui.selectable_value(
                        &mut prefs.capture_mode,
                        CaptureMode::Pick,
                        "Elegir ventana o pantalla al grabar",
                    );
                });
            ui.end_row();
            ui.label("Pantalla");
            let current = prefs.monitor.and_then(|i| self.monitors.get(i).cloned()).unwrap_or("Principal".into());
            egui::ComboBox::from_id_salt("s-monitor").selected_text(current).show_ui(ui, |ui| {
                ui.selectable_value(&mut prefs.monitor, None, "Principal");
                for (i, name) in self.monitors.iter().enumerate() {
                    ui.selectable_value(&mut prefs.monitor, Some(i), name);
                }
            });
            ui.end_row();
            ui.label("Micrófono");
            let current = match prefs.microphone.as_deref() {
                None => "Predeterminado".to_string(),
                Some("") => "Sin micrófono".to_string(),
                Some(name) => name.to_string(),
            };
            egui::ComboBox::from_id_salt("s-mic").selected_text(current).show_ui(ui, |ui| {
                ui.selectable_value(&mut prefs.microphone, None, "Predeterminado");
                ui.selectable_value(&mut prefs.microphone, Some(String::new()), "Sin micrófono");
                for name in &self.microphones {
                    ui.selectable_value(&mut prefs.microphone, Some(name.clone()), name);
                }
            });
            ui.end_row();
            ui.label("Audio del sistema");
            ui.checkbox(&mut prefs.system_audio, "Grabar también lo que suena en el PC");
            ui.end_row();
            ui.label("Fotogramas");
            egui::ComboBox::from_id_salt("s-fps").selected_text(format!("{} fps", prefs.fps)).show_ui(ui, |ui| {
                for fps in [15, 30, 60] {
                    ui.selectable_value(&mut prefs.fps, fps, format!("{fps} fps"));
                }
            });
            ui.end_row();
        });
        if prefs != self.prefs {
            prefs.save();
            self.prefs = prefs;
        }

        ui.add_space(16.0);
        ui.heading("Cloudflare R2");
        self.cloud_settings(ui);

        ui.add_space(16.0);
        ui.heading("Biblioteca");
        self.library_settings(ui);

        ui.add_space(16.0);
        ui.heading("Transcripción");
        self.transcription_settings(ui);

        ui.add_space(16.0);
        ui.heading("Importar de Loom");
        self.loom_settings(ui);
        ui.add_space(16.0);
    }

    fn cloud_settings(&mut self, ui: &mut Ui) {
        match self.config.as_ref().and_then(|(c, _, secret)| c.r2.clone().map(|r| (r, *secret))) {
            Some((r2, has_secret)) => {
                ui.label(format!(
                    "Bucket «{}»{}",
                    r2.bucket,
                    r2.public_url.map(|u| format!(" · público en {u}")).unwrap_or_default()
                ));
                if !has_secret {
                    ui.colored_label(
                        Color32::from_rgb(220, 60, 60),
                        "Falta el secret en el Administrador de credenciales: vuelve a conectar.",
                    );
                }
                ui.horizontal(|ui| {
                    if ui.button("Comprobar conexión").clicked() {
                        self.spawn(|cli, mail| match cli.run(&["check"]) {
                            Ok(_) => mail.send(Msg::Toast("R2 responde bien".into())),
                            Err(e) => mail.send(Msg::Error(e)),
                        });
                    }
                    if ui.button("Restaurar desde R2").on_hover_text("Recupera la biblioteca en un PC nuevo").clicked()
                    {
                        self.spawn(|cli, mail| {
                            match cli.run(&["restore"]) {
                                Ok(events) => {
                                    let text = events.iter().rev().find_map(|e| match e {
                                        Event::Ok { message } => Some(message.clone()),
                                        _ => None,
                                    });
                                    mail.send(Msg::Toast(text.unwrap_or_else(|| "Biblioteca restaurada".into())));
                                }
                                Err(e) => mail.send(Msg::Error(e)),
                            }
                            load_library(cli, mail);
                        });
                    }
                    if ui.button("Desconectar").clicked() {
                        self.spawn(|cli, mail| {
                            if let Err(e) = cli.run(&["config", "disconnect"]) {
                                mail.send(Msg::Error(e));
                            }
                            load_config(cli, mail);
                        });
                    }
                });
            }
            None => {
                ui.label("Conecta tu bucket para tener copia de todo y enlaces para compartir. Crea un token de R2 con permiso de lectura y escritura en el panel de Cloudflare.");
                let f = &mut self.r2_form;
                egui::Grid::new("r2").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                    ui.label("Account ID");
                    ui.text_edit_singleline(&mut f.account_id);
                    ui.end_row();
                    ui.label("Bucket");
                    ui.text_edit_singleline(&mut f.bucket);
                    ui.end_row();
                    ui.label("Access Key ID");
                    ui.text_edit_singleline(&mut f.access_key_id);
                    ui.end_row();
                    ui.label("Secret Access Key");
                    ui.add(egui::TextEdit::singleline(&mut f.secret).password(true));
                    ui.end_row();
                    ui.label("URL pública (opcional)");
                    ui.add(egui::TextEdit::singleline(&mut f.public_url).hint_text("https://pub-….r2.dev"));
                    ui.end_row();
                    ui.label("Prefijo (opcional)");
                    ui.text_edit_singleline(&mut f.prefix);
                    ui.end_row();
                    ui.label("");
                    ui.checkbox(&mut f.eu, "Bucket con jurisdicción UE");
                    ui.end_row();
                });
                let ready = !f.account_id.trim().is_empty()
                    && !f.bucket.trim().is_empty()
                    && !f.access_key_id.trim().is_empty()
                    && !f.secret.is_empty();
                if ui.add_enabled(ready, egui::Button::new("Conectar")).clicked() {
                    let mut args: Vec<String> = vec![
                        "config".into(),
                        "r2".into(),
                        "--account-id".into(),
                        f.account_id.trim().into(),
                        "--bucket".into(),
                        f.bucket.trim().into(),
                        "--access-key-id".into(),
                        f.access_key_id.trim().into(),
                    ];
                    if !f.public_url.trim().is_empty() {
                        args.extend(["--public-url".into(), f.public_url.trim().into()]);
                    }
                    if !f.prefix.trim().is_empty() {
                        args.extend(["--prefix".into(), f.prefix.trim().into()]);
                    }
                    if f.eu {
                        args.extend(["--jurisdiction".into(), "eu".into()]);
                    }
                    let secret = std::mem::take(&mut f.secret);
                    self.spawn(move |cli, mail| {
                        let a: Vec<&str> = args.iter().map(String::as_str).collect();
                        match cli.run_with(&a, Some(&secret), &[]).and_then(|_| cli.run(&["check"])) {
                            Ok(_) => mail.send(Msg::Toast("R2 conectado".into())),
                            Err(e) => mail.send(Msg::Error(e)),
                        }
                        load_config(cli, mail);
                    });
                }
            }
        }
    }

    fn library_settings(&mut self, ui: &mut Ui) {
        let Some((config, library, _)) = self.config.clone() else {
            ui.spinner();
            return;
        };
        ui.horizontal(|ui| {
            ui.label("Carpeta");
            ui.text_edit_singleline(&mut self.library_path);
            if self.library_path.trim() != library && ui.button("Usar esta carpeta").clicked() {
                self.set_config("library", self.library_path.trim().to_string());
            }
            if ui.button("Abrir").clicked() {
                open(std::ffi::OsStr::new(&library));
            }
        });
        let mut auto = config.auto_upload;
        if ui.checkbox(&mut auto, "Subir a R2 al terminar cada grabación").changed() {
            self.set_config("auto_upload", auto.to_string());
        }
        let mut days = config.keep_local_days;
        let label = |d: u32| {
            if d == 0 {
                "Conservar siempre".to_string()
            } else {
                format!("Borrar {d} días después de subirla")
            }
        };
        egui::ComboBox::from_label("Copia local").selected_text(label(days)).show_ui(ui, |ui| {
            for d in [0, 7, 30, 90] {
                ui.selectable_value(&mut days, d, label(d));
            }
        });
        if days != config.keep_local_days {
            self.set_config("keep_local_days", days.to_string());
        }
        ui.weak("Solo se borran vídeos que ya están en R2; siguen en la biblioteca y se descargan al abrirlos. Si pierdes o te roban el PC, todo lo subido sigue en tu bucket.");
    }

    fn transcription_settings(&mut self, ui: &mut Ui) {
        let mut transcribe = self.prefs.transcribe;
        if ui
            .add_enabled(
                self.can_transcribe(),
                egui::Checkbox::new(&mut transcribe, "Transcribir al terminar cada grabación"),
            )
            .changed()
        {
            self.prefs.transcribe = transcribe;
            self.prefs.save();
        }
        ui.weak("Los subtítulos se generan en este PC con Whisper: el audio no sale de aquí. Se ven en la página para compartir.");
        match &self.transcriber.0 {
            Some(m) => {
                ui.label(format!(
                    "Modelo: {}",
                    Path::new(m).file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
                ));
            }
            None => {
                ui.label("No hay ningún modelo todavía.");
            }
        }
        match self.model_download {
            Some(p) => {
                ui.add(egui::ProgressBar::new(p as f32).text("Descargando modelo…"));
            }
            None if self.transcriber.0.is_none() => {
                if ui.button("Descargar Whisper large-v3-turbo (574 MB)").clicked() {
                    self.model_download = Some(0.0);
                    self.spawn(|cli, mail| {
                        let result = cli.stream(&["transcribe", "--download-model"], |e| {
                            if let Event::Progress { done, total, .. } = e {
                                if total > 0 {
                                    mail.send(Msg::ModelDownload(Some(done as f64 / total as f64)));
                                }
                            }
                        });
                        match result {
                            Ok(()) => mail.send(Msg::Toast("Modelo de Whisper descargado".into())),
                            Err(e) => mail.send(Msg::Error(e)),
                        }
                        mail.send(Msg::ModelDownload(None));
                    });
                }
                ui.weak("Modelo abierto de OpenAI, del repositorio de whisper.cpp en Hugging Face; se comprueba su SHA-256 antes de usarlo.");
            }
            None => {}
        }
        if self.transcriber.1.is_none() {
            ui.label("Falta whisper.cpp: descarga whisper-bin-x64.zip de github.com/ggml-org/whisper.cpp/releases y deja whisper-cli.exe (con sus .dll) junto a Vroom.exe. También hace falta ffmpeg (winget install Gyan.FFmpeg).");
            if ui.button("Comprobar de nuevo").clicked() {
                self.spawn(load_transcriber);
            }
        }
    }

    fn loom_settings(&mut self, ui: &mut Ui) {
        ui.weak("En loom.com, abre las herramientas de desarrollo › Application › Cookies y copia connect.sid (o toda la cabecera Cookie).");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.loom_cookie).password(true).hint_text("connect.sid"));
            let ready = !self.loom_cookie.trim().is_empty() && !self.loom_running;
            if ui.add_enabled(ready, egui::Button::new("Ver cuántos hay")).clicked() {
                self.loom(vec!["loom", "list"]);
            }
            if ui.add_enabled(ready, egui::Button::new("Importar todo")).clicked() {
                self.loom(vec!["loom", "import"]);
            }
            if self.loom_running {
                ui.spinner();
            }
        });
        egui::ScrollArea::vertical().id_salt("loom-log").max_height(140.0).stick_to_bottom(true).show(ui, |ui| {
            for line in &self.loom_log {
                ui.monospace(line);
            }
        });
    }

    fn loom(&mut self, args: Vec<&'static str>) {
        self.loom_running = true;
        self.loom_log.clear();
        let cookie = self.loom_cookie.trim().to_string();
        self.spawn(move |cli, mail| {
            let result = cli.stream_with(&args, None, &[("VROOM_LOOM_COOKIE", &cookie)], |e| match e {
                Event::Ok { message } => mail.send(Msg::Loom(message)),
                Event::Done { id, what } => mail.send(Msg::Loom(format!("{id}: {what}"))),
                Event::Warning { id, message } => {
                    mail.send(Msg::Loom(format!("{}{message}", id.map(|i| format!("{i}: ")).unwrap_or_default())))
                }
                _ => {}
            });
            if let Err(e) = result {
                mail.send(Msg::Loom(format!("Error: {e}")));
            }
            mail.send(Msg::LoomFinished);
        });
    }
}

/// OK/Cancel row; true when OK was pressed (or Enter).
fn ok_cancel(ui: &mut Ui, ok: &str, close: &mut bool) -> bool {
    let mut pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
    ui.horizontal(|ui| {
        if ui.button(ok).clicked() {
            pressed = true;
        }
        if ui.button("Cancelar").clicked() {
            *close = true;
        }
    });
    pressed
}
