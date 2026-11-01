//! Speech to text on this computer with whisper.cpp (`whisper-cli`). The model
//! is one Vroom downloaded from whisper.cpp's repository on Hugging Face, or
//! one MacWhisper already has. The result is the recording's captions.vtt,
//! which the share page plays as subtitles.

use crate::out::Out;
use crate::{find_tool, take_flag, take_opt, Ctx};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use vroom_core::config::default_config_path;
use vroom_core::i18n::t;
use vroom_core::layout::CAPTIONS;
use vroom_core::library::Status;
use vroom_core::protocol::Event;
use vroom_core::tr;

/// Models `--download-model` offers: name, size in bytes, SHA-256. The first is the default:
/// large-v3-turbo quantized, close to large-v3 in quality at a fraction of the size and time.
const MODELS: [(&str, u64, &str); 4] = [
    ("large-v3-turbo-q5_0", 574_041_195, "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2"),
    ("large-v3-turbo", 1_624_555_275, "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69"),
    ("small", 487_601_967, "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b"),
    ("base", 147_951_465, "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe"),
];

pub fn run(out: &Out, ctx: &Ctx, mut args: Vec<String>) -> Result<(), String> {
    if take_flag(&mut args, "--status") {
        return status(out);
    }
    if take_flag(&mut args, "--download-model") {
        let name = args.into_iter().find(|a| a != "--").unwrap_or_else(|| MODELS[0].0.to_string());
        let path = download_model(&name, |done, total| out.progress("model", done, total))?;
        out.ok(tr!("modelo descargado en {path}", path = path.display()));
        return Ok(());
    }
    let missing = take_flag(&mut args, "--missing");
    let language = take_opt(&mut args, "--language").unwrap_or_else(|| "auto".into());
    let model = find_model(take_opt(&mut args, "--model"))?;
    let whisper = find_tool("whisper-cli", "VROOM_WHISPER")
        .ok_or_else(|| t("falta whisper.cpp: instálalo con `brew install whisper-cpp`"))?;
    let ffmpeg =
        find_tool("ffmpeg", "VROOM_FFMPEG").ok_or_else(|| t("falta ffmpeg: instálalo con `brew install ffmpeg`"))?;
    let ids: Vec<String> = if missing {
        ctx.lib
            .list()?
            .into_iter()
            .filter(|e| e.video.is_some() && e.captions.is_none() && e.status != Status::Recording)
            .map(|e| e.meta.id)
            .collect()
    } else {
        args.into_iter().filter(|a| a != "--").collect()
    };
    if ids.is_empty() && !missing {
        return Err(tr!("falta el argumento <{what}>", what = "id"));
    }
    let mut failed = 0;
    for id in &ids {
        let result =
            transcribe(ctx, id, &Tools { whisper: &whisper, ffmpeg: &ffmpeg, model: &model }, &language, |pct| {
                out.percent(id, pct)
            });
        match result {
            Ok(()) => out.done(id, "transcribed"),
            Err(message) => {
                failed += 1;
                out.warning(Some(id), &message);
            }
        }
    }
    if failed > 0 {
        return Err(tr!("{failed} de {total} transcripciones fallaron", failed = failed, total = ids.len()));
    }
    Ok(())
}

struct Tools<'a> {
    whisper: &'a Path,
    ffmpeg: &'a Path,
    model: &'a Path,
}

fn transcribe(ctx: &Ctx, id: &str, tools: &Tools, language: &str, mut progress: impl FnMut(u64)) -> Result<(), String> {
    let entry = ctx.lib.entry(id)?;
    if entry.status == Status::Recording {
        return Err(t("todavía se está grabando"));
    }
    let video = entry.video.ok_or_else(|| t("el vídeo no está en este ordenador: descárgalo primero"))?;
    let dir = entry.dir;
    // whisper.cpp wants 16 kHz mono PCM.
    let wav = Temp(dir.join(".transcribe.wav"));
    let output = Command::new(tools.ffmpeg)
        .args(["-nostdin", "-y", "-loglevel", "error", "-i"])
        .arg(&video)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .arg(&wav.0)
        .output()
        .map_err(|e| format!("ffmpeg: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        if err.contains("does not contain any stream") || err.contains("matches no streams") {
            return Err(t("la grabación no tiene audio"));
        }
        return Err(format!("ffmpeg: {}", err.trim()));
    }

    // On silence Whisper invents text ("Thank you."): no voice, no captions.
    let samples = read_wav(&wav.0)?;
    if voiced_seconds(&samples) < 0.3 {
        return Err(t("no se oye voz en la grabación; no hay subtítulos"));
    }
    let duration = samples.len() as f64 / 16_000.0;

    let base = dir.join(".captions");
    let vtt = Temp(base.with_extension("vtt"));
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let mut child = Command::new(tools.whisper)
        .arg("-m")
        .arg(tools.model)
        .arg("-f")
        .arg(&wav.0)
        .args(["-l", language, "-t", &threads.to_string(), "-ovtt", "-pp", "-of"])
        .arg(&base)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("whisper-cli: {e}"))?;
    // Progress comes on stderr as "... progress =  42%"; keep the rest for errors.
    let mut log = Vec::new();
    for line in BufReader::new(child.stderr.take().expect("piped")).lines().map_while(Result::ok) {
        match parse_progress(&line) {
            Some(pct) => progress(pct),
            None => log.push(line),
        }
    }
    let status = child.wait().map_err(|e| format!("whisper-cli: {e}"))?;
    if !status.success() || !vtt.0.is_file() {
        let tail = log.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join("\n");
        return Err(tr!("whisper-cli falló: {tail}", tail));
    }
    let text = std::fs::read_to_string(&vtt.0).map_err(|e| e.to_string())?;
    std::fs::write(&vtt.0, shorten_cues(&text, duration)).map_err(|e| e.to_string())?;
    std::fs::rename(&vtt.0, dir.join(CAPTIONS)).map_err(|e| e.to_string())?;
    progress(100);
    Ok(())
}

/// Whisper's cues are whole sentences, often too long to read on screen. Split
/// them between words into about two lines each, timed by their share of the text.
fn shorten_cues(vtt: &str, duration: f64) -> String {
    const MAX: usize = 84;
    let mut out = String::from("WEBVTT\n");
    for block in vtt.split("\n\n") {
        let mut lines = block.lines();
        let Some((start, end)) = lines.next().and_then(|l| l.split_once(" --> ")) else { continue };
        let text = lines.collect::<Vec<_>>().join(" ");
        let (Some(t0), Some(t1)) = (parse_time(start), parse_time(end)) else { continue };
        // Whisper works in 30 s windows and can stamp past the end of the audio.
        let t1 = t1.min(duration.max(t0));
        // As many cues as needed, each about the same length, so none is a stray word or two.
        let n = text.len().div_ceil(MAX).max(1);
        let target = text.len() / n;
        let mut chunks: Vec<String> = vec![];
        for word in text.split_whitespace() {
            let last = chunks.len() == n;
            match chunks.last_mut() {
                Some(c) if c.len() + 1 + word.len() <= MAX && (c.len() < target || last) => {
                    c.push(' ');
                    c.push_str(word);
                }
                _ => chunks.push(word.to_string()),
            }
        }
        let total: usize = chunks.iter().map(String::len).sum::<usize>().max(1);
        let mut at = t0;
        for c in &chunks {
            let next = at + (t1 - t0) * c.len() as f64 / total as f64;
            out.push_str(&format!("\n{} --> {}\n{c}\n", format_time(at), format_time(next)));
            at = next;
        }
    }
    out
}

/// The samples of the 16-bit mono WAV that ffmpeg wrote.
fn read_wav(path: &Path) -> Result<Vec<i16>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let data =
        bytes.windows(4).position(|w| w == b"data").map(|i| i + 8).ok_or_else(|| t("audio extraído no válido"))?;
    Ok(bytes[data.min(bytes.len())..].as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect())
}

/// Seconds of 30 ms windows louder than -45 dBFS: speech, not room noise.
fn voiced_seconds(samples: &[i16]) -> f64 {
    const WINDOW: usize = 480;
    let threshold = 32768.0 * 10f64.powf(-45.0 / 20.0);
    let voiced = samples
        .chunks(WINDOW)
        .filter(|w| (w.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / w.len() as f64).sqrt() > threshold)
        .count();
    voiced as f64 * WINDOW as f64 / 16_000.0
}

fn parse_time(s: &str) -> Option<f64> {
    let mut secs = 0.0;
    for part in s.trim().split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(secs)
}

fn format_time(secs: f64) -> String {
    let ms = (secs * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

fn parse_progress(line: &str) -> Option<u64> {
    let rest = line.split("progress =").nth(1)?;
    rest.trim().trim_end_matches('%').trim().parse().ok()
}

fn status(out: &Out) -> Result<(), String> {
    let model = find_model(None).ok().map(|p| p.display().to_string());
    let whisper = find_tool("whisper-cli", "VROOM_WHISPER").map(|p| p.display().to_string());
    let text = format!(
        "{}: {}\nwhisper-cli: {}",
        t("modelo"),
        model.clone().unwrap_or_else(|| t("ninguno (vroom transcribe --download-model)")),
        whisper.clone().unwrap_or_else(|| t("no instalado (brew install whisper-cpp)"))
    );
    out.event(&Event::Transcriber { model, whisper }, || text);
    Ok(())
}

fn models_dir() -> PathBuf {
    default_config_path().parent().map_or_else(|| PathBuf::from("models"), |d| d.join("models"))
}

/// One of `MODELS` from whisper.cpp's repository on Hugging Face into `models_dir()`,
/// checked against the SHA-256 pinned above before it is used.
fn download_model(name: &str, mut progress: impl FnMut(u64, u64)) -> Result<PathBuf, String> {
    let (_, size, sha) = MODELS.iter().find(|m| m.0 == name).ok_or_else(|| {
        let names: Vec<&str> = MODELS.iter().map(|m| m.0).collect();
        tr!("modelo desconocido {name}; elige uno de: {names}", name = name, names = names.join(", "))
    })?;
    let dir = models_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join(format!("ggml-{name}.bin"));
    if std::fs::metadata(&dest).is_ok_and(|m| m.len() == *size) {
        return Ok(dest);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_connect(Some(std::time::Duration::from_secs(20)))
        .timeout_per_read(Some(std::time::Duration::from_secs(60)))
        .build()
        .into();
    let url = format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{name}.bin");
    let resp = agent.get(&url).call().map_err(|e| tr!("descarga del modelo: {e}", e))?;
    let mut reader = resp.into_body().into_with_config().limit(size + 1).reader();
    let part = Temp(dest.with_extension("part"));
    let mut file = std::fs::File::create(&part.0).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 1 << 20];
    let mut done = 0;
    loop {
        let n = reader.read(&mut buf).map_err(|e| tr!("descarga del modelo: {e}", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        progress(done, *size);
    }
    file.sync_all().map_err(|e| e.to_string())?;
    let hex: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if done != *size || hex != *sha {
        return Err(t("el modelo descargado no coincide con el publicado; no se usará"));
    }
    std::fs::rename(&part.0, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// `--model`, then `VROOM_WHISPER_MODEL`, then the biggest ggml model Vroom
/// downloaded, then MacWhisper's.
fn find_model(explicit: Option<String>) -> Result<PathBuf, String> {
    if let Some(p) = explicit.or_else(|| std::env::var("VROOM_WHISPER_MODEL").ok()) {
        let p = PathBuf::from(p);
        return if p.is_file() { Ok(p) } else { Err(tr!("no existe el modelo {path}", path = p.display())) };
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let biggest = |dir: PathBuf| {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|f| f.path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                name.starts_with("ggml-") && name.ends_with(".bin")
            })
            .max_by_key(|p| std::fs::metadata(p).map_or(0, |m| m.len()))
    };
    biggest(models_dir())
        .or_else(|| home.and_then(|h| biggest(h.join("Library/Application Support/MacWhisper/models"))))
        .ok_or_else(|| t("no hay ningún modelo de Whisper: descárgalo con `vroom transcribe --download-model`"))
}

/// A scratch file removed however the transcription ends.
struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_whisper_progress() {
        assert_eq!(parse_progress("whisper_print_progress_callback: progress =  42%"), Some(42));
        assert_eq!(parse_progress("progress = 100%"), Some(100));
        assert_eq!(parse_progress("whisper_init_from_file: loading model"), None);
    }

    #[test]
    fn long_cues_are_split_between_words() {
        let words = "uno dos tres cuatro cinco seis siete ocho nueve diez ".repeat(4);
        let vtt = format!("WEBVTT\n\n00:00:01.000 --> 00:00:07.000\n {}\n", words.trim());
        let short = shorten_cues(&vtt, 60.0);
        let cues: Vec<&str> = short.split("\n\n").skip(1).collect();
        assert_eq!(cues.len(), 3, "{short}");
        assert!(cues[0].starts_with("00:00:01.000 --> "));
        assert!(cues[2].contains(" --> 00:00:07.000\n"));
        let lens: Vec<usize> = cues.iter().map(|c| c.lines().nth(1).unwrap().len()).collect();
        assert!(lens.iter().all(|l| (50..=84).contains(l)), "{lens:?}");
        assert_eq!(format_time(parse_time("01:02:03.456").unwrap()), "01:02:03.456");
    }

    #[test]
    fn cues_end_with_the_audio() {
        let vtt = "WEBVTT\n\n00:00:00.000 --> 00:00:29.980\nHola.\n";
        assert!(shorten_cues(vtt, 5.0).contains("00:00:00.000 --> 00:00:05.000\nHola."));
    }

    #[test]
    fn silence_has_no_voice() {
        assert_eq!(voiced_seconds(&vec![0i16; 16_000 * 5]), 0.0);
        assert_eq!(voiced_seconds(&vec![3i16; 16_000 * 5]), 0.0);
        let speech: Vec<i16> = (0..16_000).map(|i| ((i as f64 * 0.05).sin() * 8000.0) as i16).collect();
        assert!(voiced_seconds(&speech) > 0.9);
    }
}
