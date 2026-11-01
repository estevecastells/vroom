// Mirrors of vroom-core's protocol (crates/vroom-core/src/protocol.rs).
// Samples live in /fixtures and are decoded by KitChecks and the Rust tests.

import Foundation

public enum RecordingStatus: String, Codable, Sendable {
    case recording, local, uploading, synced, cloud
}

public enum RecordingSource: String, Codable, Sendable {
    case vroom, loom
}

public struct Entry: Codable, Equatable, Identifiable, Sendable {
    public var id: String
    public var title: String
    public var createdAt: Date
    public var durationSecs: Double
    public var width: Int
    public var height: Int
    public var sizeBytes: Int64
    public var source: RecordingSource
    public var sourceId: String?
    public var description: String?
    /// `Parent/Child`, or nil at the top level.
    public var folder: String?
    public var status: RecordingStatus
    public var dir: String
    public var video: String?
    public var thumb: String?
    public var uploadedAt: Date?
    public var progress: Double?
    public var error: String?

    public var videoURL: URL? { video.map { URL(fileURLWithPath: $0) } }
    public var thumbURL: URL? { thumb.map { URL(fileURLWithPath: $0) } }
    public var dirURL: URL { URL(fileURLWithPath: dir) }
    public var isInCloud: Bool { status == .synced || status == .cloud }
}

public struct R2Config: Codable, Equatable, Sendable {
    public var accountId: String
    public var bucket: String
    public var accessKeyId: String
    public var publicUrl: String?
    public var prefix: String?
    public var jurisdiction: String?
    public var endpoint: String?

    public init(accountId: String, bucket: String, accessKeyId: String, publicUrl: String? = nil, prefix: String? = nil) {
        self.accountId = accountId
        self.bucket = bucket
        self.accessKeyId = accessKeyId
        self.publicUrl = publicUrl
        self.prefix = prefix
    }
}

public struct Config: Codable, Equatable, Sendable {
    public var library: String?
    public var autoUpload: Bool
    public var keepLocalDays: Int
    public var r2: R2Config?
}

public struct ConfigInfo: Equatable, Sendable {
    public var config: Config
    public var path: String
    public var library: String
    public var hasSecret: Bool

    public var cloudReady: Bool { config.r2 != nil && hasSecret }
}

public enum Event: Decodable, Equatable, Sendable {
    case version(String, protocol: Int)
    case entries([Entry])
    case entry(Entry)
    case folders([String])
    case config(ConfigInfo)
    case progress(id: String, done: Int64, total: Int64)
    case done(id: String, what: String)
    case link(id: String, url: String, expiresAt: Date?)
    case warning(id: String?, message: String)
    case ok(String)
    case error(String)

    private enum Keys: String, CodingKey {
        case type, version, `protocol`, entries, entry, folders, config, path, library, hasSecret, id, done, total, what, url,
            expiresAt, message
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .type) {
        case "version":
            self = .version(try c.decode(String.self, forKey: .version), protocol: try c.decode(Int.self, forKey: .protocol))
        case "entries": self = .entries(try c.decode([Entry].self, forKey: .entries))
        case "entry": self = .entry(try c.decode(Entry.self, forKey: .entry))
        case "folders": self = .folders(try c.decode([String].self, forKey: .folders))
        case "config":
            self = .config(ConfigInfo(
                config: try c.decode(Config.self, forKey: .config),
                path: try c.decode(String.self, forKey: .path),
                library: try c.decode(String.self, forKey: .library),
                hasSecret: try c.decode(Bool.self, forKey: .hasSecret)))
        case "progress":
            self = .progress(
                id: try c.decode(String.self, forKey: .id),
                done: try c.decode(Int64.self, forKey: .done),
                total: try c.decode(Int64.self, forKey: .total))
        case "done": self = .done(id: try c.decode(String.self, forKey: .id), what: try c.decode(String.self, forKey: .what))
        case "link":
            let expires = try c.decodeIfPresent(Double.self, forKey: .expiresAt).map { Date(timeIntervalSince1970: $0) }
            self = .link(id: try c.decode(String.self, forKey: .id), url: try c.decode(String.self, forKey: .url), expiresAt: expires)
        case "warning":
            self = .warning(id: try c.decodeIfPresent(String.self, forKey: .id), message: try c.decode(String.self, forKey: .message))
        case "ok": self = .ok(try c.decode(String.self, forKey: .message))
        case "error": self = .error(try c.decode(String.self, forKey: .message))
        case let other:
            throw DecodingError.dataCorruptedError(forKey: .type, in: c, debugDescription: "unknown event \(other)")
        }
    }
}

extension JSONDecoder {
    /// snake_case keys and RFC 3339 dates with or without fractional seconds.
    public static let vroom: JSONDecoder = {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        d.dateDecodingStrategy = .custom { decoder in
            let s = try decoder.singleValueContainer().decode(String.self)
            if let date = RFC3339.parse(s) { return date }
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "bad date \(s)"))
        }
        return d
    }()
}

enum RFC3339 {
    // ISO8601DateFormatter is thread-safe; one per variant.
    nonisolated(unsafe) static let fractional: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()
    nonisolated(unsafe) static let plain = ISO8601DateFormatter()

    static func parse(_ s: String) -> Date? {
        fractional.date(from: s) ?? plain.date(from: s)
    }
}
