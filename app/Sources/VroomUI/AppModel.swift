import AVFoundation
import AppKit
import Foundation
import Network
import Observation
import OSLog
import ServiceManagement
import VroomCapture
import VroomKit

public enum Phase: Equatable {
    case idle
    case preparing
    /// `pausedAt` is set while paused; `pausedTotal` sums the finished pauses.
    case recording(started: Date, pausedAt: Date?, pausedTotal: TimeInterval)
    case saving
}

/// Recording choices, remembered between launches.
public enum CaptureMode: String, CaseIterable {
    case screen, window, region
}

public struct Prefs: Equatable {
    var captureMode: CaptureMode
    /// `nil` records the display under the mouse.
    var displayID: CGDirectDisplayID?
    var cameraID: String?
    var microphoneID: String?
    var systemAudio: Bool
    var maxHeight: Int
    var fps: Int
    var codec: RecorderSettings.Codec
    var countdown: Bool
    /// Subtitles with Whisper on this Mac when a recording ends.
    var transcribe: Bool

    static func load(_ d: UserDefaults = .standard) -> Prefs {
        Prefs(
            captureMode: d.string(forKey: "captureMode").flatMap(CaptureMode.init(rawValue:)) ?? .screen,
            displayID: (d.object(forKey: "displayID") as? NSNumber).map { CGDirectDisplayID($0.uint32Value) },
            cameraID: d.string(forKey: "cameraID"),
            microphoneID: d.object(forKey: "microphoneID") == nil ? Devices.defaultMicrophone : d.string(forKey: "microphoneID"),
            systemAudio: d.bool(forKey: "systemAudio"),
            maxHeight: d.object(forKey: "maxHeight") as? Int ?? 1080,
            fps: d.object(forKey: "fps") as? Int ?? 30,
            codec: d.string(forKey: "codec").flatMap(RecorderSettings.Codec.init(rawValue:)) ?? .h264,
            countdown: d.object(forKey: "countdown") as? Bool ?? true,
            transcribe: d.object(forKey: "transcribe") as? Bool ?? true)
    }

    func save(_ d: UserDefaults = .standard) {
        d.set(captureMode.rawValue, forKey: "captureMode")
        d.set(displayID.map { NSNumber(value: $0) }, forKey: "displayID")
        d.set(cameraID, forKey: "cameraID")
        // An empty string means "no microphone", unlike a missing key (the default one).
        d.set(microphoneID ?? "", forKey: "microphoneID")
        d.set(systemAudio, forKey: "systemAudio")
        d.set(maxHeight, forKey: "maxHeight")
        d.set(fps, forKey: "fps")
        d.set(codec.rawValue, forKey: "codec")
        d.set(countdown, forKey: "countdown")
        d.set(transcribe, forKey: "transcribe")
    }
}

let log = Logger(subsystem: "io.github.estevecastells.vroom", category: "app")

@MainActor
@Observable
public final class AppModel {
    public private(set) var phase: Phase = .idle
    public private(set) var entries: [Entry] = []
    /// Every folder, including empty ones, sorted.
    private(set) var folders: [String] = []
    public private(set) var config: ConfigInfo?
    /// Upload/download progress by recording id, 0…1.
    private(set) var transfers: [String: Double] = [:]
    private(set) var syncing = false
    /// The Whisper model and whisper-cli `vroom transcribe` would use; nil when missing.
    private(set) var transcriberModel: String?
    private(set) var whisperPath: String?
    /// Transcription progress by recording id, 0…1; queued ones are at 0.
    private(set) var transcribing: [String: Double] = [:]
    /// 0…1 while the Whisper model downloads.
    private(set) var modelDownload: Double?
    private(set) var toast: String?
    private(set) var lastError: String? {
        didSet { if let lastError { log.error("\(lastError, privacy: .public)") } }
    }
    private(set) var openAtLogin = SMAppService.mainApp.status == .enabled
    /// macOS only reports a newly granted Screen Recording permission after a relaunch.
    private(set) var canRecordScreen = Devices.canRecordScreen

    var prefs = Prefs.load() {
        didSet { if prefs != oldValue { prefs.save() } }
    }

    let cli: CLI?
    private var recorder: Recorder?
    private var recordingID: String?
    private var bubble: CameraBubble?
    private var controls: ControlBar?
    private var countdown: Countdown?
    private var regionFrame: RegionFrame?
    private var startTask: Task<Void, Never>?
    private var toastTask: Task<Void, Never>?
    private var syncAgain = false
    private var transcribeQueue: [String] = []
    private let pathMonitor = NWPathMonitor()
    private var hotKey: HotKey?

    public init() {
        cli = CLI.locate()
        if cli == nil { lastError = CLIError.missingBinary.localizedDescription }
        hotKey = HotKey { [weak self] in Task { @MainActor in self?.toggleRecording() } }
        NotificationCenter.default.addObserver(forName: NSWindow.willCloseNotification, object: nil, queue: .main) { note in
            let closing = note.object as? NSWindow
            DispatchQueue.main.async {
                let open = NSApp.windows.contains { $0 !== closing && $0.isVisible && $0.styleMask.contains(.titled) }
                if !open { NSApp.setActivationPolicy(.accessory) }
            }
        }
        Task { await boot() }
    }

    private func boot() async {
        guard let cli else { return }
        _ = try? await cli.run(["recover"])
        await reloadConfig()
        await refresh()
        await reloadTranscriber()
        // Uploads left over from last time (no network, app quit) resume on their own.
        sync()
        // Retry when the network comes back rather than polling.
        pathMonitor.pathUpdateHandler = { [weak self] path in
            guard path.status == .satisfied else { return }
            Task { @MainActor [weak self] in
                guard let self, self.hasPendingUploads else { return }
                self.sync()
            }
        }
        pathMonitor.start(queue: .global(qos: .utility))
    }

    var hasPendingUploads: Bool {
        entries.contains { ($0.status == .local || $0.status == .uploading) && $0.video != nil }
    }

    var cloudReady: Bool { config?.cloudReady ?? false }

    func refresh() async {
        guard let cli else { return }
        do {
            let list = try await cli.list()
            if list != entries { entries = list }
            let f = try await cli.folders()
            if f != folders { folders = f }
        } catch {
            lastError = error.localizedDescription
        }
    }

    func reloadConfig() async {
        guard let cli else { return }
        do { config = try await cli.config() } catch { lastError = error.localizedDescription }
    }

    func show(_ message: String) {
        toast = message
        toastTask?.cancel()
        toastTask = Task {
            try? await Task.sleep(for: .seconds(3))
            if !Task.isCancelled { toast = nil }
        }
    }

    func clearError() { lastError = nil }

    func checkPermissions() { canRecordScreen = Devices.canRecordScreen }

    func openScreenRecordingSettings() {
        Devices.requestScreen()
        Devices.openPrivacySettings("Privacy_ScreenCapture")
    }

    /// Start a fresh copy (so macOS applies new permissions) and quit this one.
    func relaunch() {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/sh")
        p.arguments = ["-c", "sleep 1; /usr/bin/open \"$0\"", Bundle.main.bundlePath]
        try? p.run()
        NSApp.terminate(nil)
    }

    // MARK: Language

    /// The language chosen in Settings, stored as an `AppleLanguages` override in the app's
    /// own defaults; nil follows the macOS language order. It applies on the next launch.
    var appLanguage: String? = AppModel.languageOverride() {
        didSet {
            guard appLanguage != oldValue else { return }
            if let appLanguage {
                UserDefaults.standard.set([appLanguage], forKey: "AppleLanguages")
            } else {
                UserDefaults.standard.removeObject(forKey: "AppleLanguages")
            }
        }
    }

    /// The override this process started with: a different choice needs a relaunch.
    let launchLanguage = AppModel.languageOverride()

    /// Read from the app's domain only; `UserDefaults.standard` would also return the system-wide list.
    private static func languageOverride() -> String? {
        guard let id = Bundle.main.bundleIdentifier,
              let list = UserDefaults.standard.persistentDomain(forName: id)?["AppleLanguages"] as? [String]
        else { return nil }
        return list.first
    }

    /// The languages this build ships (one `.lproj` each), named in their own language.
    static let availableLanguages: [(code: String, name: String)] = {
        var seen = Set<String>()
        return Bundle.main.localizations
            .filter { $0 != "Base" && seen.insert($0).inserted }
            .map { code in
                let locale = Locale(identifier: code)
                let name = locale.localizedString(forLanguageCode: code)?.capitalized(with: locale) ?? code
                return (code, name)
            }
            .sorted { $0.name.localizedCompare($1.name) == .orderedAscending }
    }()

    // MARK: Recording

    var isRecording: Bool { phase != .idle }

    public func toggleRecording() {
        log.info("toggle in phase \(String(describing: self.phase), privacy: .public), screen permission \(Devices.canRecordScreen)")
        switch phase {
        case .idle: startRecording()
        case .recording: stopRecording()
        case .preparing: cancelRecording()
        case .saving: break
        }
    }

    func startRecording() {
        guard phase == .idle, let cli else { return }
        phase = .preparing
        lastError = nil
        startTask = Task {
            do {
                try await start(cli: cli)
            } catch is CancellationError {
                await cleanUpAfterCancel()
            } catch {
                lastError = error.localizedDescription
                await cleanUpAfterCancel()
            }
        }
    }

    private func start(cli: CLI) async throws {
        canRecordScreen = Devices.canRecordScreen
        guard canRecordScreen else {
            Devices.requestScreen()
            throw CLIError.failed(String(localized:
                "Vroom necesita permiso de Grabación de pantalla: actívalo en Ajustes del Sistema › Privacidad y seguridad y vuelve a abrir la app."))
        }
        var prefs = prefs
        if prefs.microphoneID != nil, !(await Devices.request(.audio)) {
            prefs.microphoneID = nil
            show(String(localized: "Sin permiso para el micrófono: se graba sin audio"))
        }
        if prefs.cameraID != nil, !(await Devices.request(.video)) {
            prefs.cameraID = nil
            show(String(localized: "Sin permiso para la cámara"))
        }
        // What to record, and where it is on screen (AppKit coordinates).
        let target: CaptureTarget
        let area: NSRect
        let screen: NSScreen
        switch prefs.captureMode {
        case .screen:
            let id = prefs.displayID.flatMap { id in Devices.displays().contains { $0.id == id } ? id : nil }
                ?? Devices.displayUnderMouse()
            screen = Devices.screen(for: id) ?? NSScreen.main!
            target = .display(id)
            area = screen.visibleFrame
        case .window:
            guard case let .window(id)? = await SelectionOverlay.pick(.window),
                  let w = SelectionOverlay.onScreenWindows().first(where: { $0.id == id })
            else { throw CancellationError() }
            target = .window(id)
            area = SelectionOverlay.cocoaRect(fromCG: w.frame)
            screen = NSScreen.screens.first { $0.frame.intersects(area) } ?? NSScreen.main!
        case .region:
            guard let picked = await SelectionOverlay.pick(.region), case let .region(id, rect) = picked
            else { throw CancellationError() }
            screen = Devices.screen(for: id) ?? NSScreen.main!
            target = picked
            area = NSRect(x: screen.frame.minX + rect.minX, y: screen.frame.maxY - rect.maxY,
                          width: rect.width, height: rect.height)
            let frame = RegionFrame(around: area)
            frame.show()
            regionFrame = frame
        }
        try Task.checkCancellation()

        let entry = try await cli.entry(["new", "--owner-pid", String(ProcessInfo.processInfo.processIdentifier)])
        recordingID = entry.id
        if let cam = prefs.cameraID, let bubble = CameraBubble(cameraID: cam, in: area) {
            bubble.show()
            self.bubble = bubble
        }
        if prefs.countdown {
            let c = Countdown(on: screen)
            countdown = c
            guard await c.run() else { throw CancellationError() }
            countdown = nil
        }
        try Task.checkCancellation()

        let settings = RecorderSettings(target: target, microphoneID: prefs.microphoneID,
                                        systemAudio: prefs.systemAudio, maxHeight: prefs.maxHeight, fps: prefs.fps, codec: prefs.codec)
        let recorder = Recorder(settings: settings, url: entry.dirURL.appendingPathComponent("video.mp4"))
        recorder.onStop = { [weak self] error in
            Task { @MainActor [weak self] in
                self?.lastError = error.map { String(localized: "La grabación se detuvo: \($0.localizedDescription)") }
                self?.stopRecording()
            }
        }
        let kept = bubble.map { [$0.windowID] } ?? []
        do {
            try await Timeout.run(seconds: 15) { try await recorder.start(exceptWindows: kept) }
        } catch is Timeout.Expired {
            Task { await recorder.cancel() }
            canRecordScreen = Devices.canRecordScreen
            throw CLIError.failed(String(localized:
                "macOS no ha dejado empezar la grabación. Revisa en Ajustes del Sistema › Privacidad y seguridad › Grabación de pantalla y audio del sistema que Vroom esté activado, y reabre Vroom."))
        }
        guard !Task.isCancelled else {
            await recorder.cancel()
            throw CancellationError()
        }
        self.recorder = recorder
        phase = .recording(started: Date(), pausedAt: nil, pausedTotal: 0)
        log.info("recording \(entry.id, privacy: .public): \(String(describing: target), privacy: .public)")
        let bar = ControlBar(model: self, on: screen)
        bar.show()
        controls = bar
    }

    func pauseRecording() {
        guard case let .recording(started, nil, total) = phase else { return }
        recorder?.pause()
        phase = .recording(started: started, pausedAt: Date(), pausedTotal: total)
    }

    func resumeRecording() {
        guard case let .recording(started, pausedAt?, total) = phase else { return }
        recorder?.resume()
        phase = .recording(started: started, pausedAt: nil, pausedTotal: total + Date().timeIntervalSince(pausedAt))
    }

    func stopRecording() {
        guard case .recording = phase, let recorder, let id = recordingID, let cli else { return }
        phase = .saving
        closeOverlays()
        Task {
            do {
                let result = try await recorder.stop()
                let thumb = result.url.deletingLastPathComponent().appendingPathComponent("thumb.jpg")
                try? await Thumbnail.write(video: result.url, to: thumb)
                let entry = try await cli.entry([
                    "finalize", id, "--duration", String(result.duration),
                    "--width", String(result.width), "--height", String(result.height),
                ])
                finishRecording()
                await refresh()
                await afterRecording(entry)
            } catch {
                lastError = error.localizedDescription
                finishRecording()
                // Whatever reached the disk is kept as a recording.
                _ = try? await cli.run(["recover"])
                await refresh()
            }
        }
    }

    func cancelRecording() {
        switch phase {
        case .preparing:
            // The start may be stuck waiting on macOS (a permission prompt); don't wait for it.
            startTask?.cancel()
            startTask = nil
            Task { await cleanUpAfterCancel() }
        case .recording:
            phase = .saving
            Task { await cleanUpAfterCancel() }
        case .idle, .saving:
            break
        }
    }

    private func cleanUpAfterCancel() async {
        closeOverlays()
        await recorder?.cancel()
        if let id = recordingID, let cli { _ = try? await cli.run(["delete", id, "--keep-cloud"]) }
        finishRecording()
        await refresh()
    }

    private func closeOverlays() {
        bubble?.close()
        bubble = nil
        controls?.close()
        controls = nil
        regionFrame?.close()
        regionFrame = nil
    }

    private func finishRecording() {
        recorder = nil
        recordingID = nil
        countdown = nil
        phase = .idle
    }

    /// Like Loom: the link is on the clipboard as soon as the recording stops;
    /// it works once the upload (already running) finishes.
    private func afterRecording(_ entry: Entry) async {
        if prefs.transcribe && canTranscribe { transcribe(entry) }
        guard cloudReady, config?.config.autoUpload == true else {
            show(String(localized: "Grabación guardada"))
            return
        }
        if let cli, let link = try? await cli.link(["share", entry.id, "--before-upload"]) {
            copy(link.url.absoluteString)
            show(String(localized: "Enlace copiado; se está subiendo"))
        }
        sync()
    }

    // MARK: Cloud

    /// Upload everything pending. Runs once at a time; asks made meanwhile run after it.
    func sync() {
        guard cloudReady, let cli else { return }
        if syncing {
            syncAgain = true
            return
        }
        syncing = true
        Task {
            defer { syncing = false }
            repeat {
                syncAgain = false
                do {
                    try await cli.stream(["sync"]) { event in
                        Task { @MainActor [weak self] in self?.handle(event) }
                    }
                } catch {
                    if !Self.isQuiet(error) { lastError = error.localizedDescription }
                }
                transfers = [:]
                await refresh()
            } while syncAgain
        }
    }

    /// vroom failures that need no banner: per-recording failures already showed up as
    /// warnings, a Loom import from the terminal holds the lock (it uploads as it goes),
    /// or R2 is not set up yet.
    static func isQuiet(_ error: Error) -> Bool {
        guard case let .failed(_, code)? = error as? CLIError else { return false }
        return [ErrorCode.busy, ErrorCode.partial, ErrorCode.noCloud].contains(code)
    }

    private func handle(_ event: Event) {
        switch event {
        case let .progress(id, done, total) where total > 0:
            transfers[id] = Double(done) / Double(total)
        case let .done(id, _):
            transfers[id] = nil
            Task { await refresh() }
        case let .warning(id, message):
            let title = id.flatMap { id in entries.first { $0.id == id }?.title }
            lastError = title.map { "\($0): \(message)" } ?? message
        default:
            break
        }
    }

    func copyLink(_ entry: Entry) {
        guard let cli else { return }
        Task {
            do {
                let link = try await cli.link(["share", entry.id] + (entry.isInCloud ? [] : ["--before-upload"]))
                copy(link.url.absoluteString)
                if let expires = link.expiresAt {
                    show(String(localized: "Enlace copiado (caduca el \(expires.formatted(date: .abbreviated, time: .omitted)))"))
                } else {
                    show(entry.isInCloud ? String(localized: "Enlace copiado")
                         : String(localized: "Enlace copiado; funcionará al terminar la subida"))
                }
            } catch {
                lastError = error.localizedDescription
            }
        }
    }

    func play(_ entry: Entry) {
        if let url = entry.videoURL {
            NSWorkspace.shared.open(url)
            return
        }
        guard let cli else { return }
        Task {
            do {
                // The share page when the bucket is public; otherwise the signed video.
                let link = try await cli.link(config?.config.r2?.publicUrl != nil ? ["share", entry.id] : ["stream-url", entry.id])
                NSWorkspace.shared.open(link.url)
            } catch {
                lastError = error.localizedDescription
            }
        }
    }

    func reveal(_ entry: Entry) {
        NSWorkspace.shared.activateFileViewerSelecting([entry.videoURL ?? entry.dirURL])
    }

    func upload(_ entry: Entry) { sync() }

    func download(_ entry: Entry) {
        guard let cli else { return }
        transfers[entry.id] = 0
        Task {
            do {
                try await cli.stream(["download", entry.id]) { event in
                    Task { @MainActor [weak self] in self?.handle(event) }
                }
            } catch {
                lastError = error.localizedDescription
            }
            transfers[entry.id] = nil
            await refresh()
        }
    }

    func rename(_ entry: Entry, to title: String) {
        run(["rename", entry.id, "--", title]) { $0.sync() }
    }

    func move(_ entry: Entry, to folder: String) {
        run(["move", entry.id, "--", folder]) { $0.sync() }
    }

    func createFolder(_ path: String) { folderCommand(["folder", "create", "--", path]) }
    func renameFolder(_ path: String, to new: String) { folderCommand(["folder", "rename", "--", path, new]) }
    /// Without `recordings`, what was inside moves to the folder above.
    func deleteFolder(_ path: String, recordings: Bool = false, keepCloud: Bool = false) {
        let flags = recordings ? ["--recordings"] + (keepCloud ? ["--keep-cloud"] : []) : []
        folderCommand(["folder", "delete"] + flags + ["--", path])
    }

    private func folderCommand(_ args: [String]) {
        guard let cli else { return }
        Task {
            do {
                folders = try await cli.folders(args)
                await refresh()
                sync()
            } catch {
                lastError = error.localizedDescription
            }
        }
    }

    func freeLocal(_ entry: Entry) {
        run(["free", entry.id]) { $0.show(String(localized: "Copia local borrada; sigue en R2")) }
    }

    func delete(_ entry: Entry, keepCloud: Bool) {
        run(["delete", entry.id] + (keepCloud ? ["--keep-cloud"] : [])) { $0.show(String(localized: "Grabación eliminada")) }
    }

    private func run(_ args: [String], then: @escaping (AppModel) -> Void = { _ in }) {
        guard let cli else { return }
        Task {
            do {
                try await cli.run(args)
                then(self)
            } catch {
                lastError = error.localizedDescription
            }
            await refresh()
        }
    }

    // MARK: Transcription

    var canTranscribe: Bool { transcriberModel != nil && whisperPath != nil }

    func reloadTranscriber() async {
        guard let cli, let events = try? await cli.run(["transcribe", "--status"]) else { return }
        for case let .transcriber(model, whisper) in events {
            transcriberModel = model
            whisperPath = whisper
        }
    }

    /// Queue a recording; one transcription runs at a time, since each takes the whole GPU.
    func transcribe(_ entry: Entry) {
        guard canTranscribe, entry.videoURL != nil, transcribing[entry.id] == nil else { return }
        transcribing[entry.id] = 0
        transcribeQueue.append(entry.id)
        if transcribeQueue.count == 1 { Task { await drainTranscriptions() } }
    }

    private func drainTranscriptions() async {
        guard let cli else { return }
        while let id = transcribeQueue.first {
            do {
                try await cli.stream(["transcribe", id]) { event in
                    Task { @MainActor [weak self] in
                        switch event {
                        case let .progress(id, done, total) where total > 0:
                            self?.transcribing[id] = Double(done) / Double(total)
                        case let .warning(_, message):
                            self?.lastError = String(localized: "Transcripción: \(message)")
                        default:
                            break
                        }
                    }
                }
            } catch {
                // The warning above already said why.
                log.error("transcribe \(id, privacy: .public): \(error.localizedDescription, privacy: .public)")
            }
            transcribeQueue.removeFirst()
            transcribing[id] = nil
            await refresh()
            // New captions go up with the next sync.
            sync()
        }
    }

    func downloadModel() {
        guard let cli, modelDownload == nil else { return }
        modelDownload = 0
        Task {
            do {
                try await cli.stream(["transcribe", "--download-model"]) { event in
                    Task { @MainActor [weak self] in
                        if case let .progress(_, done, total) = event, total > 0 {
                            self?.modelDownload = Double(done) / Double(total)
                        }
                    }
                }
                show(String(localized: "Modelo de Whisper descargado"))
            } catch {
                lastError = error.localizedDescription
            }
            modelDownload = nil
            await reloadTranscriber()
        }
    }

    private func copy(_ s: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
    }

    // MARK: Settings

    /// Connect R2: the CLI checks the credentials before saving anything.
    func connect(accountID: String, bucket: String, accessKeyID: String, secret: String, publicURL: String,
                 prefix: String) async -> String? {
        guard let cli else { return CLIError.missingBinary.localizedDescription }
        var args = ["r2", "--account-id", accountID, "--bucket", bucket, "--access-key-id", accessKeyID]
        if !publicURL.isEmpty { args += ["--public-url", publicURL] }
        if !prefix.isEmpty { args += ["--prefix", prefix] }
        do {
            config = try await cli.config(args, stdin: secret)
            sync()
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    /// Save (the CLI checks it first) or, with an empty key, remove the Anthropic key.
    func setAnthropicKey(_ key: String) async -> String? {
        guard let cli else { return CLIError.missingBinary.localizedDescription }
        do {
            config = try await key.isEmpty ? cli.config(["anthropic", "--clear"]) : cli.config(["anthropic"], stdin: key)
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    func disconnect() async {
        guard let cli else { return }
        config = try? await cli.config(["disconnect"])
    }

    func set(_ key: String, _ value: String) {
        guard let cli else { return }
        Task {
            do {
                config = try await cli.config(["set", key, value])
                if key == "library" { await refresh() }
            } catch {
                lastError = error.localizedDescription
            }
        }
    }

    func restore() async -> String {
        guard let cli else { return CLIError.missingBinary.localizedDescription }
        do {
            let events = try await cli.run(["restore"])
            await refresh()
            for case let .ok(message) in events { return message }
            return String(localized: "Hecho")
        } catch {
            return error.localizedDescription
        }
    }

    func setOpenAtLogin(_ on: Bool) {
        do {
            if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
        } catch {
            lastError = String(localized: "No se pudo cambiar el inicio de sesión: \(error.localizedDescription)")
        }
        openAtLogin = SMAppService.mainApp.status == .enabled
    }
}
