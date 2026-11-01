# Security policy

Vroom holds an R2 access key, uploads recordings that may be private, and can
use a Loom session cookie, so we take security reports seriously.

## Reporting

Please report vulnerabilities privately via [GitHub security advisories](https://github.com/estevecastells/vroom/security/advisories/new), not public issues.

## Threat model (summary)

- The R2 secret lives in the macOS Keychain or the Windows Credential
  Manager, never in the config file. `VROOM_R2_SECRET` overrides it for
  scripts.
- Recordings get ids with 100 random bits, so public links can't be guessed.
  Titles, folders and Loom ids are not public: they go under a random private
  path in the bucket.
- Share pages escape every user-provided string.
- The Loom importer only downloads from public `https` hosts: the resolver
  rejects private, loopback and other non-global addresses at connect time,
  each redirect is checked, and proxy variables are ignored. ffmpeg only reads
  local files of a known format.
- Whisper models downloaded by Vroom are checked against pinned SHA-256
  hashes before use. Transcription runs locally; audio never leaves the
  computer.
- Release binaries carry build provenance attestations:
  `gh attestation verify <file> --repo estevecastells/vroom`.
