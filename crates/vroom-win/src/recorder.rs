//! One recording: Windows.Graphics.Capture frames (GPU textures, sent only when
//! the screen changes) go straight to Media Foundation's hardware H.264
//! encoder, with the mixed microphone/system audio as an AAC track.
//!
//! Capture runs on its own thread with a Windows message loop; stopping posts
//! WM_QUIT to it. The encoder sits in a mutex shared with the audio thread.

use crate::audio::{self, Audio};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows_capture::capture::{Context, GraphicsCaptureApiHandler};
use windows_capture::encoder::{
    AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder, VideoSettingsSubType,
};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::graphics_capture_picker::GraphicsCapturePicker;
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings, MinimumUpdateIntervalSettings,
    SecondaryWindowSettings, Settings,
};
use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};

pub enum Target {
    /// Index in `Monitor::enumerate()`, or the primary monitor.
    Monitor(Option<usize>),
    /// Windows' capture picker: a window or a monitor.
    Pick,
}

pub struct Options {
    pub target: Target,
    pub microphone: Option<String>,
    pub system_audio: bool,
    pub fps: u32,
    pub video: PathBuf,
    pub thumb: PathBuf,
}

pub enum StartError {
    /// The picker was closed without choosing anything.
    Cancelled,
    Failed(String),
}

pub struct Recorded {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
}

struct Shared {
    encoder: Mutex<Option<VideoEncoder>>,
}

pub struct Recorder {
    shared: Arc<Shared>,
    thread_id: u32,
    capture: Option<JoinHandle<Result<(), String>>>,
    audio: Option<Audio>,
    started: Instant,
    size: (u32, u32),
}

/// Monitor names for the settings, in `Target::Monitor` index order.
pub fn monitors() -> Vec<String> {
    Monitor::enumerate()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let size = match (m.width(), m.height()) {
                (Ok(w), Ok(h)) => format!(" ({w}×{h})"),
                _ => String::new(),
            };
            format!("Pantalla {}{size}", i + 1)
        })
        .collect()
}

impl Recorder {
    pub fn start(options: Options) -> Result<Recorder, StartError> {
        let shared = Arc::new(Shared { encoder: Mutex::new(None) });
        let has_audio = options.microphone.as_deref() != Some("") || options.system_audio;
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, u32, u32), StartError>>();
        let flags_shared = shared.clone();
        let Options { target, microphone, system_audio, fps, video, thumb } = options;
        let capture = std::thread::Builder::new()
            .name("vroom-capture".into())
            .spawn(move || -> Result<(), String> {
                // Our own WinRT reference, so the apartment outlives `start` and the
                // encoder can be finished here once capture stops.
                let _apartment = Apartment::new();
                let thread_id = unsafe { GetCurrentThreadId() };
                let fail = |tx: &mpsc::Sender<_>, e: String| {
                    let _ = tx.send(Err(StartError::Failed(e.clone())));
                    Err(e)
                };
                let result = match target {
                    Target::Monitor(index) => {
                        let monitor = match index.map_or_else(Monitor::primary, |i| Monitor::from_index(i + 1)) {
                            Ok(m) => m,
                            Err(e) => return fail(&ready_tx, format!("pantalla: {e}")),
                        };
                        let (w, h) = match (monitor.width(), monitor.height()) {
                            (Ok(w), Ok(h)) => (w, h),
                            _ => return fail(&ready_tx, "no se pudo leer el tamaño de la pantalla".into()),
                        };
                        let flags = Flags::new(&flags_shared, w, h, fps, has_audio, &video, &thumb);
                        let _ = ready_tx.send(Ok((thread_id, flags.width, flags.height)));
                        Handler::start(settings(monitor, flags))
                    }
                    Target::Pick => {
                        let item = match GraphicsCapturePicker::pick_item() {
                            Ok(Some(item)) => item,
                            Ok(None) => {
                                let _ = ready_tx.send(Err(StartError::Cancelled));
                                return Ok(());
                            }
                            Err(e) if e.to_string().to_lowercase().contains("cancel") => {
                                let _ = ready_tx.send(Err(StartError::Cancelled));
                                return Ok(());
                            }
                            Err(e) => return fail(&ready_tx, format!("selector: {e}")),
                        };
                        let (w, h) = match item.size() {
                            Ok((w, h)) => (w.max(2) as u32, h.max(2) as u32),
                            Err(e) => return fail(&ready_tx, e.to_string()),
                        };
                        let flags = Flags::new(&flags_shared, w, h, fps, has_audio, &video, &thumb);
                        let _ = ready_tx.send(Ok((thread_id, flags.width, flags.height)));
                        Handler::start(settings(item, flags))
                    }
                };
                // Capture is over (WM_QUIT): close the file properly.
                let encoder = flags_shared.encoder.lock().unwrap().take();
                let finished = encoder.map_or(Ok(()), |e| e.finish().map_err(|e| format!("codificador: {e}")));
                result.map_err(|e| format!("captura: {e}"))?;
                finished
            })
            .map_err(|e| StartError::Failed(e.to_string()))?;

        let (thread_id, width, height) = match ready_rx.recv() {
            Ok(Ok(ready)) => ready,
            Ok(Err(e)) => {
                let _ = capture.join();
                return Err(e);
            }
            Err(_) => {
                let reason = match capture.join() {
                    Ok(Err(e)) => e,
                    _ => "la captura no arrancó".into(),
                };
                return Err(StartError::Failed(reason));
            }
        };

        let audio = if has_audio {
            let sink_shared = shared.clone();
            let started = Audio::start(microphone, system_audio, move |pcm| {
                // Audio before the encoder exists (the first milliseconds) is dropped.
                if let Some(encoder) = sink_shared.encoder.lock().unwrap().as_mut() {
                    let _ = encoder.send_audio_buffer(pcm, 0);
                }
            });
            match started {
                Ok(a) => Some(a),
                Err(e) => {
                    // Undo the capture that already started.
                    let recorder = Recorder {
                        shared,
                        thread_id,
                        capture: Some(capture),
                        audio: None,
                        started: Instant::now(),
                        size: (width, height),
                    };
                    let _ = recorder.stop();
                    return Err(StartError::Failed(e));
                }
            }
        } else {
            None
        };
        Ok(Recorder {
            shared,
            thread_id,
            capture: Some(capture),
            audio,
            started: Instant::now(),
            size: (width, height),
        })
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Stops capture and audio and closes the MP4. Blocks until the file is complete.
    pub fn stop(mut self) -> Result<Recorded, String> {
        let duration = self.started.elapsed().as_secs_f64();
        if let Some(audio) = self.audio.take() {
            audio.stop();
        }
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
        let result = match self.capture.take().map(JoinHandle::join) {
            Some(Ok(r)) => r,
            Some(Err(_)) => Err("la captura se cerró de forma inesperada".into()),
            None => Ok(()),
        };
        // In case the thread ended before taking it (it should not).
        drop(self.shared.encoder.lock().unwrap().take());
        result?;
        Ok(Recorded { duration, width: self.size.0, height: self.size.1 })
    }
}

/// The capture thread's multithreaded COM/WinRT apartment.
struct Apartment;

impl Apartment {
    fn new() -> Self {
        unsafe { CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32) };
        Apartment
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct Flags {
    shared: Arc<Shared>,
    width: u32,
    height: u32,
    fps: u32,
    audio: bool,
    video: PathBuf,
    thumb: PathBuf,
}

impl Flags {
    fn new(shared: &Arc<Shared>, w: u32, h: u32, fps: u32, audio: bool, video: &PathBuf, thumb: &PathBuf) -> Flags {
        // H.264 wants even sizes; the encoder pads or crops frames to this.
        Flags {
            shared: shared.clone(),
            width: w & !1,
            height: h & !1,
            fps: fps.clamp(1, 60),
            audio,
            video: video.clone(),
            thumb: thumb.clone(),
        }
    }
}

fn settings<T: TryInto<windows_capture::settings::GraphicsCaptureItemType>>(
    item: T,
    flags: Flags,
) -> Settings<Flags, T> {
    let interval = Duration::from_millis(1000 / flags.fps as u64);
    Settings::new(
        item,
        CursorCaptureSettings::WithCursor,
        // No yellow border: it would end up in the video.
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Custom(interval),
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        flags,
    )
}

struct Handler {
    shared: Arc<Shared>,
    thumb: Option<PathBuf>,
    first_frame: Option<Instant>,
}

impl GraphicsCaptureApiHandler for Handler {
    type Flags = Flags;
    type Error = String;

    fn new(ctx: Context<Flags>) -> Result<Self, String> {
        let f = ctx.flags;
        // About 0.1 bit per pixel per frame: plenty for screen content.
        let bitrate = ((f.width as u64 * f.height as u64 * f.fps as u64) / 10).clamp(2_000_000, 25_000_000) as u32;
        let audio = if f.audio {
            AudioSettingsBuilder::new().sample_rate(audio::RATE).channel_count(audio::CHANNELS).bit_per_sample(16)
        } else {
            AudioSettingsBuilder::new().disabled(true)
        };
        let encoder = VideoEncoder::new(
            VideoSettingsBuilder::new(f.width, f.height)
                .sub_type(VideoSettingsSubType::H264)
                .frame_rate(f.fps)
                .bitrate(bitrate),
            audio,
            ContainerSettingsBuilder::new(),
            &f.video,
        )
        .map_err(|e| format!("no se pudo crear el vídeo: {e}"))?;
        *f.shared.encoder.lock().unwrap() = Some(encoder);
        Ok(Handler { shared: f.shared, thumb: Some(f.thumb), first_frame: None })
    }

    fn on_frame_arrived(&mut self, frame: &mut Frame, _control: InternalCaptureControl) -> Result<(), String> {
        if let Some(encoder) = self.shared.encoder.lock().unwrap().as_mut() {
            encoder.send_frame(frame).map_err(|e| e.to_string())?;
        }
        // The thumbnail: a frame about a second in, once the countdown and the
        // first redraws are past.
        let first = *self.first_frame.get_or_insert_with(Instant::now);
        if first.elapsed() >= Duration::from_secs(1) {
            if let Some(path) = self.thumb.take() {
                save_thumbnail(frame, path);
            }
        }
        Ok(())
    }
}

/// Copies the frame to the CPU and writes a 640-px-wide JPEG off the capture thread.
fn save_thumbnail(frame: &mut Frame, path: PathBuf) {
    let Ok(buffer) = frame.buffer() else { return };
    let (w, h) = (buffer.width(), buffer.height());
    let mut bytes = Vec::new();
    let bgra = buffer.as_nopadding_buffer(&mut bytes).to_vec();
    std::thread::spawn(move || {
        let mut rgba = bgra;
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let Some(img) = image::RgbaImage::from_raw(w, h, rgba) else { return };
        let tw = 640.min(w);
        let th = (h as u64 * tw as u64 / w.max(1) as u64).max(1) as u32;
        let small = image::imageops::thumbnail(&img, tw, th);
        let rgb = image::DynamicImage::ImageRgba8(small).to_rgb8();
        let _ = rgb.save_with_format(&path, image::ImageFormat::Jpeg);
    });
}
