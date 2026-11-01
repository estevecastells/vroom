// Protocol conformance checks for VroomKit: `swift run KitChecks`.
// Wire fixtures live in /fixtures and are shared with the Rust tests.

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
    guard case let .error(m) = try decode("error.json") else { return false }
    return m.contains("R2")
}(), "decodes errors")

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

print("\(passed) passed, \(failures) failed")
exit(failures == 0 ? 0 : 1)
