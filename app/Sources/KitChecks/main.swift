// Protocol conformance checks for VroomKit: `swift run KitChecks`.
// Wire fixtures live in /fixtures and are shared with the Rust tests.

import CryptoKit
import Foundation
import VroomKit

var failures = 0
var passed = 0

func check(_ condition: @autoclosure () throws -> Bool, _ name: String, line: UInt = #line) {
    do {
        if try condition() { passed += 1 } else { failures += 1; print("FAIL \(name) (line \(line))") }
    } catch {
        failures += 1
        print("FAIL \(name): threw \(error)")
    }
}

let repoRoot = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
    .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
func fixture(_ name: String) -> Data {
    try! Data(contentsOf: repoRoot.appendingPathComponent("fixtures/\(name)"))
}
func decode(_ name: String) throws -> Event { try JSONDecoder.vroom.decode(Event.self, from: fixture(name)) }

check(try {
    guard case let .entries(e) = try decode("entries.json"), e.count == 3 else { return false }
    let a = e[0], b = e[1], c = e[2]
    return a.id == "k3m9x2p7q4w8z1v5b6n0" && a.status == .synced && a.width == 2940 && a.sizeBytes == 48_211_337
        && a.createdAt == Date(timeIntervalSince1970: 1_793_539_925) && a.uploadedAt != nil && a.thumbURL != nil
        && a.isInCloud
        && b.status == .uploading && b.progress == 0.5 && b.error == "conexión perdida" && b.uploadedAt == nil
        && a.folder == nil && c.folder == "Equipo/Weekly"
        && c.source == .loom && c.sourceId == "0f2b8c1e9a7d4b3c" && c.status == .cloud && c.videoURL == nil
}(), "decodes the library list")

check(try {
    guard case let .progress(id, done, total) = try decode("progress.json") else { return false }
    return id == "a1b2c3d4e5f6g7h8j9k0" && done == 4_500_000 && total == 9_000_000
}(), "decodes progress")

check(try {
    guard case let .link(_, url, expires) = try decode("link.json") else { return false }
    return url.hasSuffix("/index.html") && expires == nil
}(), "decodes a public link")

check(try {
    guard case let .config(c) = try decode("config.json") else { return false }
    return c.cloudReady && c.config.keepLocalDays == 30 && c.config.r2?.publicUrl == "https://videos.example.com"
        && c.config.r2?.prefix == nil && c.library == "/Users/me/Movies/Vroom"
}(), "decodes config")

check(try {
    guard case let .error(m, code) = try decode("error.json") else { return false }
    return m.contains("R2") && code == nil
}(), "decodes errors")

check(try {
    guard case let .error(_, code) = try decode("error-busy.json") else { return false }
    return code == ErrorCode.busy
}(), "decodes error codes")

check(try {
    let line = #"{"type":"entry","entry":{"id":"at0f29ktsk8cwk4zbnap","title":"Prueba","created_at":"2026-11-01T14:37:28.57461+01:00","duration_secs":0.0,"width":0,"height":0,"size_bytes":0,"source":"vroom","status":"recording","dir":"/x"}}"#
    guard case let .entry(e) = try JSONDecoder.vroom.decode(Event.self, from: Data(line.utf8)) else { return false }
    return e.status == .recording && abs(e.createdAt.timeIntervalSince1970 - 1_793_540_248.57461) < 0.001
}(), "decodes fractional local-offset dates from the CLI")

check(try {
    let line = #"{"type":"folders","folders":["Clientes","Clientes/Acme"]}"#
    guard case let .folders(f) = try JSONDecoder.vroom.decode(Event.self, from: Data(line.utf8)) else { return false }
    return f == ["Clientes", "Clientes/Acme"]
}(), "decodes folders")

check(try {
    guard case let .transcriber(model, whisper) = try decode("transcriber.json") else { return false }
    return model?.hasSuffix("ggml-large-v3-turbo-q5_0.bin") == true && whisper == "/opt/homebrew/bin/whisper-cli"
}(), "decodes the transcriber status")

// Updates: versions, release JSON, signatures and checksums.
check(Version("v0.10.0")! > Version("0.9.9")! && Version("1.0")! == Version("1.0.0")! && Version("0.1.0")! < Version("0.1.1")!,
      "version ordering")
check(Version("v1.x") == nil && Version("") == nil && Version("1..2") == nil && Version("1.") == nil, "invalid versions rejected")
check(try {
    let json = #"""
    {"tag_name": "v0.2.0", "html_url": "https://github.com/estevecastells/vroom/releases/tag/v0.2.0",
     "draft": false, "prerelease": false, "body": "Notes",
     "assets": [
      {"name": "Vroom-macos-arm64.zip", "browser_download_url": "https://github.com/estevecastells/vroom/releases/download/v0.2.0/Vroom-macos-arm64.zip", "size": 1},
      {"name": "SHA256SUMS", "browser_download_url": "https://github.com/estevecastells/vroom/releases/download/v0.2.0/SHA256SUMS", "size": 1}
     ]}
    """#
    let r = try Release.decode(Data(json.utf8))
    return r.version == Version("0.2.0") && !r.draft && r.asset(UpdateConfig.appAsset) != nil && r.asset("nope") == nil
}(), "decodes a GitHub release")
check(!UpdateConfig.publicKeys.isEmpty && (try? ReleaseVerifier(publicKeysBase64: UpdateConfig.publicKeys)) != nil,
      "embedded public keys are valid")

let testKey = Curve25519.Signing.PrivateKey()
let payload = Data("abc123  Vroom-macos-arm64.zip\ndef456  vroom-macos-arm64.tar.gz\n".utf8)
let verifier = try! ReleaseVerifier(publicKeyBase64: testKey.publicKey.rawRepresentation.base64EncodedString())
let goodSig = Data(try! testKey.signature(for: payload).base64EncodedString().utf8)
check(try verifier.verifiedSums(sums: payload, signature: goodSig)["Vroom-macos-arm64.zip"] == "abc123",
      "valid signature accepted")
check((try? verifier.verifiedSums(sums: payload + Data("x".utf8), signature: goodSig)) == nil, "tampered sums rejected")
let otherSig = Data(try! Curve25519.Signing.PrivateKey().signature(for: payload).base64EncodedString().utf8)
check((try? verifier.verifiedSums(sums: payload, signature: otherSig)) == nil, "signature from another key rejected")
let rotated = try! ReleaseVerifier(publicKeysBase64: [
    Curve25519.Signing.PrivateKey().publicKey.rawRepresentation.base64EncodedString(),
    testKey.publicKey.rawRepresentation.base64EncodedString(),
])
check((try? rotated.verifiedSums(sums: payload, signature: goodSig)) != nil, "signature from any listed key accepted")
check((try? rotated.verifiedSums(sums: payload, signature: otherSig)) == nil, "signature from an unlisted key rejected")
check((try? ReleaseVerifier(publicKeysBase64: [])) == nil, "an empty key list is refused")
check(ReleaseVerifier.parseSums("ABC  *a.zip\nbad line\n")["a.zip"] == "abc", "parses binary-mode shasum lines")

let tmpFile = FileManager.default.temporaryDirectory.appendingPathComponent("kitchecks-\(getpid()).bin")
try! Data("hello".utf8).write(to: tmpFile)
let helloHex = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
check((try? ReleaseVerifier.check(tmpFile, named: "f", in: ["f": helloHex])) != nil, "checksum match")
check((try? ReleaseVerifier.check(tmpFile, named: "f", in: ["f": String(repeating: "0", count: 64)])) == nil,
      "checksum mismatch rejected")
check((try? ReleaseVerifier.check(tmpFile, named: "g", in: ["f": helloHex])) == nil, "missing checksum entry rejected")
try? FileManager.default.removeItem(at: tmpFile)

print("\(passed) passed, \(failures) failed")
exit(failures == 0 ? 0 : 1)
