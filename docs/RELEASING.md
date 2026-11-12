# Releasing

Releases are built and published by `.github/workflows/release.yml` when a version tag is pushed.

1. Bump `version` in the root `Cargo.toml` (for example `0.2.0`), then run `cargo build` to refresh `Cargo.lock`.
2. Check that `make ci` passes, then commit: `Release v0.2.0`.
3. Tag and push:
   ```sh
   git tag v0.2.0 && git push origin main v0.2.0
   ```
4. The workflow checks that the tag matches the Cargo version, runs the tests, builds the app and CLI on macOS and Windows, smoke-tests the CLI, and publishes a GitHub release with:
   - `Vroom-macos-arm64.zip`: the menu bar app, with the `vroom` CLI inside
   - `vroom-macos-arm64.tar.gz`: the CLI on its own
   - `vroom-windows-x64.zip`: the Windows CLI and app
   - `SHA256SUMS`
   - `SHA256SUMS.sig`: Ed25519 signature checked by the in-app updater, made with the `UPDATE_SIGNING_KEY` secret. The workflow fails rather than publish an unsigned release.

Asset names carry no version, so `releases/latest/download/<asset>` always resolves to the newest release. `scripts/get.sh` and the in-app updater depend on this.

To build the same artifacts locally, run `scripts/package-release.sh` (output goes to `dist/`; it doesn't sign the checksums).

## In-app updates

The app checks `releases/latest` 15 seconds after launch and then every 6 hours. With **Install updates automatically** on (the default, in Settings › Recording) it downloads the zip, verifies `SHA256SUMS.sig` against the public keys in `UpdateConfig.publicKeys` (`app/Sources/VroomKit/Updater.swift`), checks the zip's checksum, bundle identifier, version and code signature, swaps the app bundle and relaunches. Installing quits the app, so it waits while Vroom is recording, saving, uploading, transcribing or downloading the Whisper model.

To try a release JSON without publishing one, run the app with `VROOM_UPDATE_FEED=file:///path/release.json`.

## Repository secrets

| Secret | What it is | Backup (login keychain, generic password) |
| --- | --- | --- |
| `UPDATE_SIGNING_KEY` | Ed25519 private key for `SHA256SUMS.sig` (base64 raw) | `Vroom update signing key` |
| `VROOM_SIGNING_P12` | The "Vroom Release Signing" code signing identity, a base64 `.p12` | `Vroom release signing p12 (base64)` |
| `VROOM_SIGNING_P12_PASSWORD` | Its password | `Vroom release signing p12 password` |

A secret can't be read back from GitHub, so keep the backups. To restore one: `security find-generic-password -s "<name>" -w | gh secret set <SECRET> -R estevecastells/vroom`.

## Code signing

The app is signed with a self-signed certificate, "Vroom Release Signing". It is not a Developer ID, so a browser download still needs a one-time **Open Anyway** (`get.sh` avoids it because `curl` doesn't set the quarantine flag). What it does give is a stable designated requirement: macOS ties the Screen Recording, camera and microphone permissions to it, so they survive updates. An ad-hoc signature changes with every build and macOS would ask again after each one, which is why the workflow refuses to package without the identity.

`scripts/build-app.sh` uses the same identity when it is in your keychain, so a local `make install` and the releases share permissions. Contributors without it get "Vroom Local Signing" (`scripts/make-signing-cert.sh`) or ad hoc.

## Rotating the update signing key

Installed apps only accept a release signed by a key in `UpdateConfig.publicKeys`, so a new key needs one bridging release:

1. Generate a pair with `swift scripts/sign-update.swift --generate` and back up the private key in the keychain.
2. Put the new public key first in `UpdateConfig.publicKeys`, keeping the current one after it, and release as usual. That release is still signed with the current secret.
3. Once it's published, replace the `UPDATE_SIGNING_KEY` secret with the new private key. Later releases are signed with it, and the old public key can be dropped from the list.

Replacing the code signing certificate makes macOS ask for the permissions again once, after the update that brings it.
