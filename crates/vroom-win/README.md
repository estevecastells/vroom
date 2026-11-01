# Vroom for Windows

A tray app with a library window, written in Rust. Like the Mac app it
records by itself and runs `vroom.exe --json` for everything else: library,
folders, R2 sync, share links, Loom import and transcription. Both apps share
the same CLI, protocol and on-disk library, so a library written on one opens
on the other.

| Piece | Built on |
|---|---|
| Window (library, settings) | [egui](https://github.com/emilk/egui) via eframe, glow renderer |
| Tray icon, menu | [tray-icon](https://github.com/tauri-apps/tray-icon) |
| Shortcut Alt+Shift+R | [global-hotkey](https://github.com/tauri-apps/global-hotkey) |
| Screen and window capture | [windows-capture](https://github.com/NiiightmareXD/windows-capture): Windows.Graphics.Capture, frames stay on the GPU |
| Encoding | Media Foundation hardware H.264 + AAC, MP4, through windows-capture's `VideoEncoder` |
| Microphone, system audio | [cpal](https://github.com/RustAudio/cpal) on WASAPI; system audio is WASAPI loopback |

## Status

This was written on a Mac and only compiled by CI (`cargo clippy` and `cargo
test` on `windows-latest`). **Nobody has run it on Windows yet.** Expect the
first run to need fixes.

Done:

- Tray icon and menu (record or stop, discard, open the library, quit), and
  Alt+Shift+R. Closing the window keeps Vroom in the tray.
- Recording of a whole monitor, or a window or monitor picked with Windows'
  own capture picker. The library window hides while recording.
- Microphone and system audio, mixed into one AAC track (`src/audio.rs`).
- Thumbnail one second in, `finalize`, share link on the clipboard, upload,
  and transcription when it ends, as on the Mac.
- Library: folders (create, rename, delete), cards with thumbnails and
  status, right-click menu (play, copy link, rename, move, reveal,
  transcribe, upload, free space, download, delete).
- Settings: what to record, monitor, microphone, system audio, fps;
  connect, check, restore and disconnect R2; library folder, auto upload,
  local copy retention; Whisper model download; Loom import.

Not done yet, in rough order of value:

1. **Pause.** windows-capture stamps video with the capture time, so pausing
   would desync audio and video. Options: one file per segment joined with
   `ffmpeg -f concat -c copy`, or `send_frame_buffer` with our own timestamps.
2. **Crash safety.** Media Foundation writes the MP4 index at the end, so a
   crash loses the recording. The Mac app writes fragmented MP4 every 4 s.
   Try `MF_TRANSCODE_CONTAINERTYPE_FMPEG4` or segment files.
3. **Floating control bar and countdown**, excluded from capture with
   `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`. Today recording
   is controlled from the tray menu and the shortcut.
4. **Camera bubble.**
5. **Area capture.** `Frame::buffer_crop` plus `send_frame_buffer` (CPU path),
   or a cropped texture.
6. **Installer and exe icon.** Today the zip has `Vroom.exe` and `vroom.exe`
   side by side; an icon resource needs a `build.rs` (for example `winres`).
   Start at login: a shortcut in `shell:startup` running `Vroom.exe --hidden`.
7. **Scale down 4K** to 1080p like the Mac setting (the encoder pads or crops
   to the first frame's size today).

## Build and test

On Windows 10 2004 or later (Windows 11 recommended), with Rust:

```powershell
cargo build --release -p vroom -p vroom-win
copy target\release\vroom-app.exe target\release\Vroom.exe
target\release\Vroom.exe
```

`Vroom.exe` looks for `vroom.exe` in its own folder. Settings are in
`%APPDATA%\Vroom\` (`config.toml` for the library and R2, `app.json` for the
app) and the R2 secret is in the Windows Credential Manager.

For transcription, put `whisper-cli.exe` and its DLLs from
[whisper.cpp's releases](https://github.com/ggml-org/whisper.cpp/releases)
next to `vroom.exe`, and install ffmpeg (`winget install Gyan.FFmpeg`). Then
download the model from **Ajustes › Transcripción**.

Things worth checking first: a 10-second recording with the microphone, one
with system audio playing, the window picker, and that the MP4 plays in the
Films & TV app and in a browser.
