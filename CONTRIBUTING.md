# Contributing

Thanks for helping. Issues and pull requests are welcome, from small fixes to
whole features (the Windows app in `crates/vroom-win` especially needs people
who run Windows).

## Before you open a pull request

The full test suite must pass on your machine before you push. CI runs it
again on macOS and Windows for every pull request, but that is a check, not a
substitute.

```bash
make ci        # fmt, clippy -D warnings, Rust tests (with a fake S3), Swift protocol checks, app build
```

On Windows, or without Xcode tools:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

Never skip or delete a failing test to get green, and never push with
`--no-verify`. If a test is wrong, fix it in the same pull request and say
why.

## What a good pull request looks like

- **One change.** A bug fix, a feature, or a refactor; not all three.
- **Tests for behaviour changes.** The library, the R2 layout, sync and the
  CLI protocol all have tests; add to them. Anything that talks to R2 can be
  tested against the fake S3 in `crates/vroom-cloud/tests/fake_s3.rs`.
- **Protocol changes on both sides.** The apps read `vroom --json` events
  (`crates/vroom-core/src/protocol.rs`). A new or changed event updates
  `fixtures/`, the Rust types, `app/Sources/VroomKit/Models.swift` and
  `app/Sources/KitChecks`, and the Windows app if it uses it.
- **Recordings stay safe.** Never delete or overwrite a user's video unless
  the user asked for it or a complete copy is confirmed in R2. Crash recovery
  (`vroom recover`) must keep whatever reached the disk.
- **Low resource use.** Vroom sits idle most of the day. No polling loops,
  timers that wake the CPU, or work on the main thread that can be avoided.
  If a change affects idle CPU, memory or recording CPU, say how you measured.
- **No new dependencies without a reason** in the pull request description.
- **Spanish UI, English code.** User-facing text is in Spanish; code,
  comments, commits and docs are in English.
- **No secrets.** Never commit account ids, bucket URLs, access keys, Loom
  cookies or recordings, not even in tests or screenshots. Use placeholders.

## Layout

| Path | What |
|---|---|
| `crates/vroom-core` | Library on disk, config, bucket layout, share page, JSON protocol |
| `crates/vroom-cloud` | R2: presigned requests, resumable multipart, sync and restore |
| `crates/vroom` | The `vroom` CLI, Loom import, transcription |
| `crates/vroom-win` | The Windows app (see its README for status) |
| `app/` | The macOS app (SwiftUI, ScreenCaptureKit, AVAssetWriter) |
| `fixtures/` | Protocol samples checked by both Rust and Swift |

## Reporting bugs and security issues

Use the bug report form for bugs. Report security issues privately, as
described in [SECURITY.md](SECURITY.md).
