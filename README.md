<div align="center">

<img src="docs/images/icon.png" alt="" width="112">

<h1>Vroom</h1>

<p><strong>Record your screen, a window or an area, with camera, voice and system audio, and keep every video in your own Cloudflare R2 bucket.</strong><br/>
A small, free replacement for the basic Loom workflow: press ⌥⇧R, talk, stop, and the share link is already on your clipboard while the video uploads.</p>

<a href="https://github.com/estevecastells/vroom/releases/latest/download/Vroom-macos-arm64.zip"><img src="https://img.shields.io/badge/Download_for_Mac-Apple_Silicon-0A84FF?style=for-the-badge&logo=apple&logoColor=white" alt="Download Vroom for Mac (Apple Silicon)" height="44"/></a>
&nbsp;
<a href="https://github.com/estevecastells/vroom/releases/latest/download/vroom-windows-x64.zip"><img src="https://img.shields.io/badge/Windows-CLI_(no_recorder_yet)-0078D4?style=for-the-badge&logo=windows&logoColor=white" alt="Download the vroom CLI for Windows" height="44"/></a>

<p>
<a href="https://github.com/estevecastells/vroom/releases/latest"><img src="https://img.shields.io/github/v/release/estevecastells/vroom?label=latest&style=flat-square" alt="Latest release"/></a>
<img src="https://img.shields.io/badge/macOS-14%2B-555?style=flat-square&logo=apple" alt="macOS 14 or later"/>
<a href="https://github.com/estevecastells/vroom/actions/workflows/ci.yml"><img src="https://github.com/estevecastells/vroom/actions/workflows/ci.yml/badge.svg" alt="CI"/></a>
<a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="MIT license"/></a>
</p>

**Install on a Mac in one line** (no security prompt; verifies the checksum):

</div>

```sh
curl -fsSL https://raw.githubusercontent.com/estevecastells/vroom/main/scripts/get.sh | bash
```

<div align="center"><sub>
The app is not notarized. If you download the zip instead, macOS may refuse to open it the first time: go to System Settings › Privacy &amp; Security and press <b>Open Anyway</b>.
The Windows download is the <code>vroom</code> command-line tool (library, R2 uploads, links, restore, Loom import). There is no Windows recorder yet.
</sub></div>

## Vroom vs. Loom

Measured on the same Mac (M5 Pro, 64 GB, macOS 26.6), Loom desktop 0.382.0 against Vroom 0.1.0, both open and not recording, measured one to two minutes after launch. Memory is the sum over every process of the app (physical footprint, as Activity Monitor shows it); CPU is the average over 60 seconds, from the processes' cumulative CPU time.

| | Loom | Vroom |
|---|---:|---:|
| App size on disk | 850 MB | 5.4 MB |
| Processes | 15 | 1 |
| Memory, idle | 1,250 MB | 19 MB |
| CPU, idle | 11.9% | 0.01% |
| CPU / memory while recording 1080p30 + mic | not measured | 2.4% / 15 MB |
| Extra system components | an audio driver (`LoomAudioDevice`) that stays loaded when Loom is closed | none |
| Where videos live | Loom's cloud | a folder on your Mac and your own R2 bucket |
| Cost | per-seat subscription | free; R2 storage is $0.015/GB-month, no egress fees, first 10 GB free |

Loom's recording figures need someone to drive its UI, so they are not in the table yet.

## Features

- **Pick what to record.** The whole screen, one window (click it), or an area (drag it). The camera shows as a round bubble you can drag, and the control bar is never in the video.
- **Folders.** Organise recordings in folders and subfolders. A Loom import keeps Loom's folder tree and titles.
- **Subtitles on your Mac.** When a recording ends, Whisper (whisper.cpp) transcribes it on this Mac and the share page shows the subtitles. The audio never leaves the computer. It uses MacWhisper's models if you have them, or downloads the open large-v3-turbo model (574 MB, SHA-256 checked); a 2.5-minute video takes about 11 seconds on an M5 Pro.
- **Local first.** Recordings are plain MP4 files in `~/Movies/Vroom/<id>/`, each with its `meta.json` and thumbnail. You can open, copy or back them up without the app.
- **Your bucket.** Every recording goes to R2 when it is saved. If the Mac breaks or is stolen, install Vroom on another one and run **Restaurar desde R2**: the whole library comes back, and videos download when you open them.
- **Light.** 0% CPU and about 18 MB when idle. While recording, ScreenCaptureKit sends frames only when the screen changes, already in the encoder's format, and the hardware H.264/HEVC encoder writes them. The file is fragmented every 4 seconds, so a crash or power cut keeps what was recorded.
- **Leaving Loom.** `vroom loom import` downloads every video you created in Loom, including those in folders, with their captions, and uploads them to R2.

The UI is in Spanish.

## Layout

| Part | What it does |
|---|---|
| `crates/vroom-core` | The library on disk, the config file, the bucket layout, the share page and the CLI's JSON protocol. Cross-platform. |
| `crates/vroom-cloud` | R2 through its S3 API (`rusty-s3` + `ureq`, no async runtime): resumable multipart uploads, restore, download, delete and links. |
| `crates/vroom` | The `vroom` CLI. The apps use it for everything except capture, and it works on its own on macOS, Windows and Linux. |
| `app/` | The macOS menu bar app (SwiftUI): recording with ScreenCaptureKit + AVAssetWriter, camera bubble, control bar, library and settings. |

The Mac app runs `vroom --json …` and reads one event per line (`fixtures/` holds samples that both the Rust tests and `KitChecks` decode). A Windows app would reuse the same CLI and only needs its own capture (Windows.Graphics.Capture + Media Foundation) and UI. Today the CLI is tested on Windows in CI; there is no Windows recorder yet.

## Build

Needs Rust and the Xcode Command Line Tools on macOS 14+.

```sh
make app       # build/Vroom.app (the vroom CLI is inside)
make install   # copy it to /Applications
make test      # Rust tests (including a fake S3 server) + Swift protocol checks
```

The first recording asks for Screen Recording permission (then reopen the app), and for the camera and microphone if you use them. An ad-hoc signature changes with every build, and macOS then forgets the permission. Run `scripts/make-signing-cert.sh` once: it creates a local self-signed identity that `make app` picks up from then on, so permissions survive rebuilds.

## Connect Cloudflare R2

**From the Cloudflare CLI** (`npm i -g cf && cf auth login`):

```sh
scripts/setup-r2.sh vroom --public
```

This creates the `vroom` bucket and an API token that can only read and write that bucket. It saves the secret in the macOS Keychain and connects Vroom. `--public` turns on the bucket's r2.dev URL, so share links open a page with the video and never expire. r2.dev is rate-limited, so for heavy use attach a custom domain to the bucket and set it as the public URL. Without a public URL, links are signed and last 7 days.

**By hand:** create a bucket and an R2 API token with *Object Read & Write* in the dashboard (R2 › Manage API tokens), then fill in **Ajustes › Cloudflare R2**, or run:

```sh
vroom config r2 --account-id <id> --bucket vroom --access-key-id <key> [--public-url https://videos.example.com]
# then paste the Secret Access Key
```

Public objects are stored as `recordings/<id>/{video.mp4, thumb.jpg, captions.vtt, index.html}`. Titles, folders and Loom ids are not public: they go to `library/<random>/meta/<id>.json`, uploaded last, so a restore only picks up recordings that finished uploading. Ids carry 100 random bits, so public links can't be guessed.

## Import from Loom

Loom has no public API for listing or downloading videos and no bulk export ([LOOM-168](https://jira.atlassian.com/browse/LOOM-168)). Vroom uses the private GraphQL API of loom.com with your browser session, as the maintained open-source archivers do.

1. Sign in to loom.com, open the developer tools › Application › Cookies, and copy `connect.sid`.
2. In **Ajustes › Importar de Loom**, paste it and press **Ver cuántos hay**, then **Importar todo**. Or from the terminal:

```sh
VROOM_LOOM_COOKIE='<connect.sid>' vroom loom list
VROOM_LOOM_COOKIE='<connect.sid>' vroom loom import --free-local   # --free-local keeps only the R2 copy
```

Re-running skips what is already imported, so it can stop and resume. Videos that Loom serves only as HLS (some long ones) need `ffmpeg` (`brew install ffmpeg`). Only videos you created are listed, so a workspace admin has to run it for each member.

## Subtitles

Vroom runs [whisper.cpp](https://github.com/ggml-org/whisper.cpp) locally:

```sh
brew install whisper-cpp ffmpeg
vroom transcribe --download-model     # large-v3-turbo (574 MB); or base, small, large-v3-turbo
vroom transcribe <id>                 # or --missing for every recording without subtitles
```

With MacWhisper installed, its models are used without downloading anything. In the app: **Ajustes › Transcripción**, and right-click › **Transcribir** in the library. A new transcription of a recording that is already in R2 goes up on the next sync.

## CLI

```text
vroom list | sync | upload <id>|--pending | share <id> | download <id> | restore
vroom rename <id> <title> | move <id> [folder/sub] | free <id> | delete <id> [--keep-cloud]
vroom config | config r2 … | config set library|auto_upload|keep_local_days <value> | check
vroom transcribe <id>|--missing [--language es] [--model file.bin] | transcribe --download-model [name] | --status
vroom loom list | loom import [--no-upload] [--free-local] [--limit N]
```

`--json` prints one JSON event per line. `VROOM_CONFIG` points to another config file, and `VROOM_R2_SECRET` overrides the Keychain.

## License

MIT
