//! Runs `vroom.exe --json …` and reads its events, one JSON object per line
//! (crates/vroom-core/src/protocol.rs). Short commands return every event;
//! long ones hand each event over as it arrives.

use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use vroom_core::i18n::t;
use vroom_core::library::Entry;
use vroom_core::protocol::Event;
use vroom_core::tr;

/// No console window flashing up for each command.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone)]
pub struct Cli {
    exe: PathBuf,
}

impl Cli {
    /// `vroom.exe` next to this program (the release zip), else on the PATH.
    pub fn locate() -> Option<Cli> {
        let here = std::env::current_exe().ok()?.with_file_name("vroom.exe");
        if here.is_file() {
            return Some(Cli { exe: here });
        }
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path).map(|d| d.join("vroom.exe")).find(|p| p.is_file()).map(|exe| Cli { exe })
    }

    fn command(&self, args: &[&str], envs: &[(&str, &str)]) -> Command {
        let mut cmd = Command::new(&self.exe);
        cmd.arg("--json").args(args).creation_flags(CREATE_NO_WINDOW);
        // Messages and errors come back in the app's language.
        cmd.env("VROOM_LANG", vroom_core::i18n::lang());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd
    }

    pub fn run(&self, args: &[&str]) -> Result<Vec<Event>, String> {
        self.run_with(args, None, &[])
    }

    /// With text on stdin (secrets) or extra environment (the Loom cookie).
    pub fn run_with(&self, args: &[&str], stdin: Option<&str>, envs: &[(&str, &str)]) -> Result<Vec<Event>, String> {
        let mut events = vec![];
        self.stream_with(args, stdin, envs, |e| events.push(e))?;
        Ok(events)
    }

    pub fn stream(&self, args: &[&str], on_event: impl FnMut(Event)) -> Result<(), String> {
        self.stream_with(args, None, &[], on_event)
    }

    /// Each event as it comes; an `error` event or a failed exit is the `Err`.
    pub fn stream_with(
        &self,
        args: &[&str],
        stdin: Option<&str>,
        envs: &[(&str, &str)],
        on_event: impl FnMut(Event),
    ) -> Result<(), String> {
        self.stream_coded(args, stdin, envs, on_event).map_err(|(message, _)| message)
    }

    /// Like `stream_with`, with the error's `code` (`vroom_core::protocol::error_code`),
    /// for the failures the app handles instead of showing.
    pub fn stream_coded(
        &self,
        args: &[&str],
        stdin: Option<&str>,
        envs: &[(&str, &str)],
        mut on_event: impl FnMut(Event),
    ) -> Result<(), (String, Option<String>)> {
        let mut child = self
            .command(args, envs)
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| (tr!("no se pudo abrir vroom.exe: {e}", e), None))?;
        if let Some(text) = stdin {
            let mut pipe = child.stdin.take().expect("piped");
            let _ = writeln!(pipe, "{text}");
        }
        let mut error = None;
        for line in BufReader::new(child.stdout.take().expect("piped")).lines().map_while(Result::ok) {
            match serde_json::from_str::<Event>(&line) {
                Ok(Event::Error { message, code }) => error = Some((message, code)),
                Ok(event) => on_event(event),
                Err(_) => {}
            }
        }
        let output = child.wait_with_output().map_err(|e| (e.to_string(), None))?;
        if let Some(error) = error {
            return Err(error);
        }
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let message =
                if stderr.is_empty() { tr!("vroom.exe terminó con {status}", status = output.status) } else { stderr };
            return Err((message, None));
        }
        Ok(())
    }

    pub fn entries(&self) -> Result<Vec<Entry>, String> {
        self.run(&["list"])?
            .into_iter()
            .find_map(|e| match e {
                Event::Entries { entries } => Some(entries),
                _ => None,
            })
            .ok_or_else(|| t("respuesta inesperada de vroom list"))
    }

    pub fn entry(&self, args: &[&str]) -> Result<Entry, String> {
        self.run(args)?
            .into_iter()
            .find_map(|e| match e {
                Event::Entry { entry } => Some(entry),
                _ => None,
            })
            .ok_or_else(|| t("respuesta inesperada de vroom"))
    }

    pub fn folders(&self, args: &[&str]) -> Result<Vec<String>, String> {
        self.run(args)?
            .into_iter()
            .find_map(|e| match e {
                Event::Folders { folders } => Some(folders),
                _ => None,
            })
            .ok_or_else(|| t("respuesta inesperada de vroom folders"))
    }

    pub fn link(&self, args: &[&str]) -> Result<String, String> {
        self.run(args)?
            .into_iter()
            .find_map(|e| match e {
                Event::Link { url, .. } => Some(url),
                _ => None,
            })
            .ok_or_else(|| t("respuesta inesperada de vroom share"))
    }
}
