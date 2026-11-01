//! Microphone and system audio (WASAPI loopback) mixed into 48 kHz stereo
//! 16-bit PCM for the encoder. The mix is paced by the wall clock, not by a
//! device: loopback delivers nothing while no app plays sound, and a source
//! that falls behind is padded with silence instead of stalling the other.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;
/// A source more than this far ahead of the mix loses its oldest samples.
const MAX_QUEUE: usize = (RATE as usize / 5) * CHANNELS as usize;

/// Names of the microphones, for the settings.
pub fn microphones() -> Vec<String> {
    cpal::default_host().input_devices().map(|d| d.map(|d| d.to_string()).collect()).unwrap_or_default()
}

/// A device's samples, already 48 kHz stereo, and the stream that fills them.
type Source = (Arc<Mutex<VecDeque<f32>>>, Stream);

pub struct Audio {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Audio {
    /// `microphone`: `None` the default one, `Some("")` none, else a device name.
    /// `sink` gets interleaved little-endian i16 stereo at 48 kHz, every ~10 ms.
    pub fn start(
        microphone: Option<String>,
        system: bool,
        mut sink: impl FnMut(&[u8]) + Send + 'static,
    ) -> Result<Audio, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let stop2 = stop.clone();
        // cpal streams stay on the thread that built them.
        let thread = std::thread::Builder::new()
            .name("vroom-audio".into())
            .spawn(move || {
                let host = cpal::default_host();
                let mut sources: Vec<Source> = vec![];
                let mic = match microphone.as_deref() {
                    Some("") => None,
                    None => host.default_input_device(),
                    Some(name) => host
                        .input_devices()
                        .ok()
                        .and_then(|mut ds| ds.find(|d| d.to_string() == name))
                        .or_else(|| host.default_input_device()),
                };
                if let Some(device) = mic {
                    match open(&device, false) {
                        Ok(s) => sources.push(s),
                        Err(e) => {
                            let _ = ready_tx.send(Err(format!("micrófono: {e}")));
                            return;
                        }
                    }
                }
                if system {
                    // An output device opened for input is WASAPI loopback.
                    match host
                        .default_output_device()
                        .ok_or("no hay salida de audio".to_string())
                        .and_then(|d| open(&d, true))
                    {
                        Ok(s) => sources.push(s),
                        Err(e) => {
                            let _ = ready_tx.send(Err(format!("audio del sistema: {e}")));
                            return;
                        }
                    }
                }
                let _ = ready_tx.send(Ok(()));
                mix(&sources, &stop2, &mut sink);
                drop(sources);
            })
            .map_err(|e| e.to_string())?;
        ready_rx.recv().map_err(|_| "el audio no arrancó".to_string())??;
        Ok(Audio { stop, thread: Some(thread) })
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn mix(sources: &[Source], stop: &AtomicBool, sink: &mut impl FnMut(&[u8])) {
    let clock_start = Instant::now();
    let mut emitted: u64 = 0;
    let mut out = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(10));
        let due = (clock_start.elapsed().as_secs_f64() * RATE as f64) as u64;
        let frames = due.saturating_sub(emitted) as usize;
        if frames == 0 {
            continue;
        }
        let mut mixed = vec![0f32; frames * CHANNELS as usize];
        for (queue, _) in sources {
            let mut q = queue.lock().unwrap();
            for sample in mixed.iter_mut() {
                match q.pop_front() {
                    Some(s) => *sample += s,
                    None => break,
                }
            }
        }
        out.clear();
        for s in mixed {
            out.extend_from_slice(&((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
        }
        sink(&out);
        emitted += frames as u64;
    }
}

/// Opens one device and returns the queue its callback fills with 48 kHz stereo.
fn open(device: &cpal::Device, loopback: bool) -> Result<Source, String> {
    let supported = if loopback { device.default_output_config() } else { device.default_input_config() }
        .map_err(|e| e.to_string())?;
    let format = supported.sample_format();
    let config = supported.config();
    let channels = config.channels as usize;
    let queue = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_QUEUE)));
    let mut resampler = Resampler::new(config.sample_rate, channels);
    let q = queue.clone();
    let mut push = move |samples: &mut dyn Iterator<Item = f32>| {
        let stereo = resampler.process(samples);
        let mut q = q.lock().unwrap();
        q.extend(stereo);
        let over = q.len().saturating_sub(MAX_QUEUE);
        q.drain(..over);
    };
    let on_error = |e| eprintln!("audio: {e}");
    let stream = match format {
        SampleFormat::F32 => device.build_input_stream::<f32, _, _>(
            config,
            move |data, _| push(&mut data.iter().copied()),
            on_error,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream::<i16, _, _>(
            config,
            move |data, _| push(&mut data.iter().map(|s| *s as f32 / 32768.0)),
            on_error,
            None,
        ),
        SampleFormat::I32 => device.build_input_stream::<i32, _, _>(
            config,
            move |data, _| push(&mut data.iter().map(|s| *s as f32 / 2_147_483_648.0)),
            on_error,
            None,
        ),
        other => return Err(format!("formato de audio no soportado: {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((queue, stream))
}

/// Any rate and channel count to 48 kHz stereo, by linear interpolation:
/// enough for speech and system sounds, and cheap.
struct Resampler {
    step: f64,
    channels: usize,
    pos: f64,
    /// The last input frame of the previous call, to interpolate across calls.
    prev: [f32; 2],
}

impl Resampler {
    fn new(rate: u32, channels: usize) -> Self {
        Resampler { step: rate as f64 / RATE as f64, channels: channels.max(1), pos: 0.0, prev: [0.0; 2] }
    }

    fn process(&mut self, samples: &mut dyn Iterator<Item = f32>) -> Vec<f32> {
        // Input as stereo frames, with the previous call's last frame first.
        let mut frames = vec![self.prev];
        let mut frame = Vec::with_capacity(self.channels);
        for s in samples {
            frame.push(s);
            if frame.len() == self.channels {
                let left = frame[0];
                let right = if self.channels > 1 { frame[1] } else { left };
                frames.push([left, right]);
                frame.clear();
            }
        }
        let mut out = Vec::new();
        while self.pos + 1.0 < frames.len() as f64 {
            let i = self.pos as usize;
            let t = (self.pos - i as f64) as f32;
            let (a, b) = (frames[i], frames[i + 1]);
            out.push(a[0] + (b[0] - a[0]) * t);
            out.push(a[1] + (b[1] - a[1]) * t);
            self.pos += self.step;
        }
        self.pos -= (frames.len() - 1) as f64;
        self.prev = *frames.last().expect("at least prev");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples_mono_44k_to_stereo_48k() {
        let mut r = Resampler::new(44_100, 1);
        let mut total = 0;
        for _ in 0..100 {
            total += r.process(&mut std::iter::repeat_n(0.5, 441)).len();
        }
        // One second in, one second out, give or take a frame.
        assert!((total as i64 / 2 - 48_000).abs() <= 2, "{total}");
    }
}
