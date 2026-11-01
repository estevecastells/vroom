// Checks the macOS app's translations: `swift scripts/check-strings.swift [--list]`.
//
// The keys are the Spanish text in the Swift sources (es.lproj needs no entries). Every other
// app/Resources/<lang>.lproj/Localizable.strings must translate every key the app uses, with
// the same placeholders, or the app would show Spanish there. It also lints every .strings
// file with plutil, warns about entries no code uses, and flags string literals with Spanish
// characters that are not localized (mark a line `// i18n-ignore` when that is intended).
// --list prints the keys found in the code; interpolations show as ⟨…⟩.

import Foundation

let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
let sourceDirs = ["Vroom", "VroomUI", "VroomCapture", "VroomKit"].map { root.appendingPathComponent("app/Sources/\($0)") }
let resources = root.appendingPathComponent("app/Resources")
/// The language the keys are written in.
let keyLanguage = "es"

// MARK: Scanning Swift sources

enum Token {
    case ident(String)
    case punct(Character)
    /// Decoded text; each interpolation is `\u{0}`.
    case string(String, interpolated: Bool)
}

struct Located {
    let token: Token
    let line: Int
}

/// A small Swift lexer: identifiers, punctuation and string literals, without comments.
func tokenize(_ src: String) -> [Located] {
    let c = Array(src)
    var i = 0, line = 1, out: [Located] = []

    func peek(_ o: Int = 0) -> Character? { i + o < c.count ? c[i + o] : nil }

    /// Reads a string literal starting at the opening quote; returns its text with `\u{0}` per interpolation.
    func readString() -> (String, Bool) {
        let multiline = peek(1) == "\"" && peek(2) == "\""
        i += multiline ? 3 : 1
        var text = "", interpolated = false
        while let ch = peek() {
            if multiline ? (ch == "\"" && peek(1) == "\"" && peek(2) == "\"") : ch == "\"" {
                i += multiline ? 3 : 1
                break
            }
            if ch == "\n" { line += 1 }
            if ch == "\\" {
                let next = peek(1)
                i += 2
                switch next {
                case "n": text.append("\n")
                case "t": text.append("\t")
                case "0": text.append("\0")
                case "\"": text.append("\"")
                case "'": text.append("'")
                case "\\": text.append("\\")
                case "u":
                    // \u{XXXX}
                    var hex = ""
                    if peek() == "{" { i += 1 }
                    while let h = peek(), h != "}" { hex.append(h); i += 1 }
                    i += 1
                    if let v = UInt32(hex, radix: 16), let s = Unicode.Scalar(v) { text.unicodeScalars.append(s) }
                case "(":
                    // Skip the interpolated expression, including nested parentheses and strings.
                    var depth = 1
                    while depth > 0, let e = peek() {
                        if e == "\"" { _ = readString(); continue }
                        if e == "(" { depth += 1 } else if e == ")" { depth -= 1 }
                        i += 1
                    }
                    text.append("\u{0}")
                    interpolated = true
                default: break
                }
                continue
            }
            text.append(ch)
            i += 1
        }
        return (text, interpolated)
    }

    while let ch = peek() {
        if ch == "\n" { line += 1; i += 1; continue }
        if ch.isWhitespace { i += 1; continue }
        if ch == "/" && peek(1) == "/" {
            while let e = peek(), e != "\n" { i += 1 }
            continue
        }
        if ch == "/" && peek(1) == "*" {
            i += 2
            while let e = peek(), !(e == "*" && peek(1) == "/") { if e == "\n" { line += 1 }; i += 1 }
            i += 2
            continue
        }
        if ch == "\"" {
            let start = line
            let (text, interpolated) = readString()
            out.append(Located(token: .string(text, interpolated: interpolated), line: start))
            continue
        }
        if ch.isLetter || ch == "_" {
            var name = ""
            while let e = peek(), e.isLetter || e.isNumber || e == "_" { name.append(e); i += 1 }
            out.append(Located(token: .ident(name), line: line))
            continue
        }
        out.append(Located(token: .punct(ch), line: line))
        i += 1
    }
    return out
}

/// Calls whose first unlabeled argument is a LocalizedStringKey (or String.LocalizationValue for
/// `String(localized:)`). Literals directly in that argument, also both sides of a ternary, are keys.
let keyCalls: Set<String> = [
    "Text", "Button", "Toggle", "Label", "Picker", "Section", "LabeledContent", "TextField", "SecureField",
    "Menu", "Link", "ProgressView", "Window", "WindowGroup", "ContentUnavailableView", "LocalizedStringKey",
    "confirmationDialog", "alert", "help", "navigationTitle",
]

struct Key {
    let text: String
    let interpolated: Bool
    let file: String
    let line: Int

    /// The key as SwiftUI and Foundation generate it, with `⟨…⟩` for each interpolation.
    var display: String { interpolated ? text.replacingOccurrences(of: "%", with: "%%").replacingOccurrences(of: "\0", with: "⟨…⟩") : text }

    /// Matches the generated key whatever the interpolated types are (%@, %lld, %lf…).
    var pattern: NSRegularExpression {
        let parts = text.split(separator: "\0", omittingEmptySubsequences: false).map {
            NSRegularExpression.escapedPattern(for: String($0).replacingOccurrences(of: "%", with: "%%"))
        }
        return try! NSRegularExpression(pattern: "^" + parts.joined(separator: "%(?:\\d+\\$)?(?:@|ll[dui]|l[dfui]|[dfiu])") + "$")
    }
}

var keys: [Key] = []
var errors: [String] = []
var warnings: [String] = []
let spanishChars = CharacterSet(charactersIn: "áéíóúüñÁÉÍÓÚÜÑ¿¡«»")

for dir in sourceDirs {
    let files = (try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)) ?? []
    for file in files.filter({ $0.pathExtension == "swift" }).sorted(by: { $0.path < $1.path }) {
        guard let src = try? String(contentsOf: file, encoding: .utf8) else { continue }
        let lines = src.components(separatedBy: "\n")
        let name = "\(dir.lastPathComponent)/\(file.lastPathComponent)"
        let t = tokenize(src)
        var used = Set<Int>() // token indexes taken as keys

        for (n, tok) in t.enumerated() {
            guard case let .ident(id) = tok.token, n + 1 < t.count, case .punct("(") = t[n + 1].token else { continue }
            var j = n + 2
            if id == "String" {
                // String(localized: "…")
                guard j + 1 < t.count, case .ident("localized") = t[j].token, case .punct(":") = t[j + 1].token else { continue }
                j += 2
            } else if keyCalls.contains(id) {
                // A labeled first argument (verbatim:, selection:, isOn:…) is not a key.
                if j + 1 < t.count, case .ident = t[j].token, case .punct(":") = t[j + 1].token { continue }
            } else {
                continue
            }
            var depth = 0
            while j < t.count {
                switch t[j].token {
                case .punct("("), .punct("["), .punct("{"): depth += 1
                case .punct(")"), .punct("]"), .punct("}"):
                    if depth == 0 { j = t.count; continue }
                    depth -= 1
                case .punct(",") where depth == 0: j = t.count; continue
                case let .string(text, interpolated) where depth == 0:
                    used.insert(j)
                    if !text.isEmpty { keys.append(Key(text: text, interpolated: interpolated, file: name, line: t[j].line)) }
                default: break
                }
                j += 1
            }
        }
        // `prompt: "…"` (searchable) takes a key too.
        for n in 2..<max(2, t.count) {
            if case let .string(text, interpolated) = t[n].token, case .punct(":") = t[n - 1].token,
               case .ident("prompt") = t[n - 2].token {
                used.insert(n)
                if !text.isEmpty { keys.append(Key(text: text, interpolated: interpolated, file: name, line: t[n].line)) }
            }
        }
        // Spanish text outside any localized position is probably a string someone forgot to localize.
        for (n, tok) in t.enumerated() where !used.contains(n) {
            guard case let .string(text, _) = tok.token, text.rangeOfCharacter(from: spanishChars) != nil else { continue }
            if lines.indices.contains(tok.line - 1), lines[tok.line - 1].contains("i18n-ignore") { continue }
            errors.append("\(name):\(tok.line): not localized: \"\(text.replacingOccurrences(of: "\0", with: "⟨…⟩"))\" "
                + "(use String(localized:), or add `// i18n-ignore` to the line)")
        }
    }
}

if CommandLine.arguments.contains("--list") {
    var seen = Set<String>()
    for k in keys where seen.insert(k.display).inserted { print(k.display) }
    exit(0)
}

// MARK: Checking the tables

/// Format specifiers in order of appearance, without positions (`%1$@` → `@`).
func specifiers(_ s: String) -> [String] {
    let re = try! NSRegularExpression(pattern: "%(?:\\d+\\$)?(ll[dui]|l[dfui]|[@dfiu])")
    let clean = s.replacingOccurrences(of: "%%", with: "")
    return re.matches(in: clean, range: NSRange(clean.startIndex..., in: clean)).map {
        String(clean[Range($0.range(at: 1), in: clean)!])
    }
}

func lint(_ file: URL) -> Bool {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/plutil")
    p.arguments = ["-lint", "-s", file.path]
    try? p.run()
    p.waitUntilExit()
    return p.terminationStatus == 0
}

let lprojs = ((try? FileManager.default.contentsOfDirectory(at: resources, includingPropertiesForKeys: nil)) ?? [])
    .filter { $0.pathExtension == "lproj" }
    .sorted { $0.path < $1.path }
if !lprojs.contains(where: { $0.deletingPathExtension().lastPathComponent == "en" }) {
    errors.append("app/Resources/en.lproj is missing")
}

var uniqueKeys: [Key] = []
var seenKeys = Set<String>()
for k in keys where seenKeys.insert(k.display).inserted { uniqueKeys.append(k) }

for lproj in lprojs {
    let lang = lproj.deletingPathExtension().lastPathComponent
    let stringsFiles = ((try? FileManager.default.contentsOfDirectory(at: lproj, includingPropertiesForKeys: nil)) ?? [])
        .filter { $0.pathExtension == "strings" }
    for f in stringsFiles where !lint(f) { errors.append("\(lang).lproj/\(f.lastPathComponent): invalid (plutil -lint)") }

    let tableURL = lproj.appendingPathComponent("Localizable.strings")
    guard FileManager.default.fileExists(atPath: tableURL.path) else {
        errors.append("\(lang).lproj/Localizable.strings is missing")
        continue
    }
    guard let table = NSDictionary(contentsOf: tableURL) as? [String: String] else {
        errors.append("\(lang).lproj/Localizable.strings could not be read")
        continue
    }
    for (key, value) in table where specifiers(key).sorted() != specifiers(value).sorted() {
        errors.append("\(lang).lproj: placeholders differ: \"\(key)\" = \"\(value)\"")
    }
    if lang == keyLanguage {
        continue // the keys are already in this language
    }
    var matched = Set<String>()
    for k in uniqueKeys {
        let found = k.interpolated
            ? table.keys.filter { k.pattern.firstMatch(in: $0, range: NSRange($0.startIndex..., in: $0)) != nil }
            : (table[k.text] != nil ? [k.text] : [])
        if found.isEmpty {
            errors.append("\(lang).lproj: missing \"\(k.display)\" (\(k.file):\(k.line))")
        }
        matched.formUnion(found)
    }
    for key in table.keys.sorted() where !matched.contains(key) {
        warnings.append("\(lang).lproj: unused entry \"\(key)\"")
    }
    print("\(lang): \(table.count) entries, \(uniqueKeys.count) keys in the code")
}

warnings.forEach { print("warning: \($0)") }
errors.forEach { print("error: \($0)") }
if !errors.isEmpty {
    print("\(errors.count) problem(s)")
    exit(1)
}
print("Translations OK")
