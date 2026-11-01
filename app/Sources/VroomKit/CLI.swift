// Runs the bundled `vroom` binary with --json and decodes its event lines.
// Short commands return their result; long ones stream progress as they go.

import Foundation

public enum CLIError: Error, LocalizedError, Equatable {
    case missingBinary
    case failed(String)
    case badOutput(String)

    public var errorDescription: String? {
        switch self {
        case .missingBinary: "No se encuentra el programa vroom dentro de la app."
        case let .failed(m): m
        case let .badOutput(m): "Respuesta inesperada de vroom: \(m)"
        }
    }
}

public struct CLI: Sendable {
    public let binary: URL

    /// `VROOM_BIN`, then the copy inside the app bundle, then a dev build next to the package.
    public static func locate() -> CLI? {
        var candidates: [URL] = []
        if let env = ProcessInfo.processInfo.environment["VROOM_BIN"] { candidates.append(URL(fileURLWithPath: env)) }
        if let res = Bundle.main.resourceURL { candidates.append(res.appendingPathComponent("vroom")) }
        let exe = URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath()
        var dir = exe.deletingLastPathComponent()
        for _ in 0..<6 {
            candidates.append(dir.appendingPathComponent("target/release/vroom"))
            candidates.append(dir.appendingPathComponent("target/debug/vroom"))
            dir = dir.deletingLastPathComponent()
        }
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0.path) }.map(CLI.init(binary:))
    }

    public init(binary: URL) { self.binary = binary }

    /// Run to completion and return every event; an `error` event throws.
    @discardableResult
    public func run(_ args: [String], stdin: String? = nil) async throws -> [Event] {
        let events = LockedBox<[Event]>([])
        try await stream(args, stdin: stdin) { e in events.value.append(e) }
        return events.value
    }

    /// Run and hand each event to `onEvent` as it arrives (on a background task).
    /// Throws the last `error` event, or a generic failure on a non-zero exit.
    public func stream(_ args: [String], stdin: String? = nil, onEvent: @escaping @Sendable (Event) -> Void) async throws {
        let process = Process()
        process.executableURL = binary
        process.arguments = ["--json"] + args
        let out = Pipe()
        let err = Pipe()
        process.standardOutput = out
        process.standardError = err
        let input = Pipe()
        process.standardInput = input
        try process.run()
        if let stdin { input.fileHandleForWriting.write(Data((stdin + "\n").utf8)) }
        try? input.fileHandleForWriting.close()

        let cancel = { if process.isRunning { process.terminate() } }
        let lastError = LockedBox<String?>(nil)
        try await withTaskCancellationHandler {
            for try await line in out.fileHandleForReading.bytes.lines {
                guard let data = line.data(using: .utf8), !data.isEmpty else { continue }
                let event: Event
                do { event = try JSONDecoder.vroom.decode(Event.self, from: data) } catch {
                    throw CLIError.badOutput("\(line.prefix(200)) — \(error)")
                }
                if case let .error(message) = event { lastError.value = message }
                onEvent(event)
            }
        } onCancel: { cancel() }
        process.waitUntilExit()
        if let message = lastError.value { throw CLIError.failed(message) }
        if process.terminationStatus != 0 {
            let stderr = String(decoding: err.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            throw CLIError.failed(stderr.isEmpty ? "vroom terminó con código \(process.terminationStatus)" : stderr)
        }
    }

    // MARK: Typed helpers

    public func list() async throws -> [Entry] {
        for case let .entries(e) in try await run(["list"]) { return e }
        throw CLIError.badOutput("expected entries")
    }

    public func config(_ args: [String] = [], stdin: String? = nil) async throws -> ConfigInfo {
        for case let .config(c) in try await run(["config"] + args, stdin: stdin) { return c }
        throw CLIError.badOutput("expected config")
    }

    public func entry(_ args: [String]) async throws -> Entry {
        for case let .entry(e) in try await run(args) { return e }
        throw CLIError.badOutput("expected entry")
    }

    public func link(_ args: [String]) async throws -> (url: URL, expiresAt: Date?) {
        for case let .link(_, url, expires) in try await run(args) {
            guard let u = URL(string: url) else { break }
            return (u, expires)
        }
        throw CLIError.badOutput("expected link")
    }
}

/// A tiny lock-protected box for values set from the reading task.
final class LockedBox<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var _value: T
    init(_ value: T) { _value = value }
    var value: T {
        get { lock.withLock { _value } }
        set { lock.withLock { _value = newValue } }
    }
}
