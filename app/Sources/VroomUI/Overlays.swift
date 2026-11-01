// Floating windows around a recording: the camera bubble (which is recorded),
// the 3-2-1 countdown and the control bar (which are not: the recorder leaves
// out every window of this app except the bubble).

import AVFoundation
import AppKit
import SwiftUI

/// A borderless panel that floats above everything and never takes focus from the app being recorded.
final class FloatingPanel: NSPanel {
    init(rect: NSRect, movable: Bool) {
        super.init(contentRect: rect, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        level = .statusBar
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        isMovableByWindowBackground = movable
        hidesOnDeactivate = false
        isReleasedWhenClosed = false
    }

    override var canBecomeKey: Bool { false }
}

// MARK: Camera bubble

/// A round, mirrored camera preview in a corner of the recorded screen.
@MainActor
final class CameraBubble {
    private let panel: FloatingPanel
    private let session = AVCaptureSession()

    var windowID: CGWindowID { CGWindowID(panel.windowNumber) }

    /// `area` is what is being recorded (AppKit coordinates); the bubble sits in its bottom-left corner.
    init?(cameraID: String, in area: NSRect, diameter requested: CGFloat = 200) {
        let diameter = min(requested, max(96, min(area.width, area.height) / 3))
        guard let device = AVCaptureDevice(uniqueID: cameraID) ?? AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device), session.canAddInput(input)
        else { return nil }
        // Small frames are plenty for a 200 pt circle and keep the camera cheap.
        session.sessionPreset = session.canSetSessionPreset(.vga640x480) ? .vga640x480 : .medium
        session.addInput(input)

        let margin = min(32, diameter / 6)
        let frame = NSRect(x: area.minX + margin, y: area.minY + margin,
                           width: diameter, height: diameter)
        panel = FloatingPanel(rect: frame, movable: true)
        let view = NSView(frame: NSRect(origin: .zero, size: frame.size))
        view.wantsLayer = true
        let preview = AVCaptureVideoPreviewLayer(session: session)
        preview.videoGravity = .resizeAspectFill
        preview.frame = view.bounds
        preview.cornerRadius = diameter / 2
        preview.masksToBounds = true
        preview.borderWidth = 3
        preview.borderColor = NSColor.white.withAlphaComponent(0.9).cgColor
        if let conn = preview.connection, conn.isVideoMirroringSupported {
            conn.automaticallyAdjustsVideoMirroring = false
            conn.isVideoMirrored = true
        }
        view.layer?.addSublayer(preview)
        panel.contentView = view
        panel.hasShadow = false
    }

    func show() {
        panel.orderFrontRegardless()
        let session = session
        DispatchQueue.global(qos: .userInitiated).async { session.startRunning() }
    }

    func close() {
        let session = session
        DispatchQueue.global(qos: .utility).async { session.stopRunning() }
        panel.orderOut(nil)
    }
}

// MARK: Countdown

@MainActor
final class Countdown {
    private let panel: FloatingPanel

    init(on screen: NSScreen) {
        let size: CGFloat = 160
        let frame = NSRect(x: screen.frame.midX - size / 2, y: screen.frame.midY - size / 2, width: size, height: size)
        panel = FloatingPanel(rect: frame, movable: false)
        panel.ignoresMouseEvents = true
    }

    /// Shows 3, 2, 1; returns false if cancelled meanwhile.
    func run(from seconds: Int = 3) async -> Bool {
        panel.orderFrontRegardless()
        defer { panel.orderOut(nil) }
        for n in stride(from: seconds, through: 1, by: -1) {
            panel.contentView = NSHostingView(rootView: CountdownView(number: n))
            do { try await Task.sleep(for: .milliseconds(850)) } catch { return false }
        }
        return true
    }
}

private struct CountdownView: View {
    let number: Int
    var body: some View {
        Text(verbatim: String(number))
            .font(.system(size: 84, weight: .semibold, design: .rounded))
            .foregroundStyle(.white)
            .frame(width: 160, height: 160)
            .background(Circle().fill(.black.opacity(0.6)))
    }
}

// MARK: Control bar

@MainActor
final class ControlBar {
    private let panel: FloatingPanel

    init(model: AppModel, on screen: NSScreen) {
        let size = NSSize(width: 250, height: 44)
        let frame = NSRect(x: screen.visibleFrame.midX - size.width / 2, y: screen.visibleFrame.minY + 24,
                           width: size.width, height: size.height)
        panel = FloatingPanel(rect: frame, movable: true)
        panel.contentView = NSHostingView(rootView: ControlBarView(model: model))
    }

    func show() { panel.orderFrontRegardless() }
    func close() { panel.orderOut(nil) }
}

private struct ControlBarView: View {
    let model: AppModel

    var body: some View {
        HStack(spacing: 10) {
            Button { model.stopRecording() } label: {
                Image(systemName: "stop.fill").foregroundStyle(.red)
            }
            .help("Terminar y guardar (⌥⇧R)")
            if case let .recording(started, pausedAt, pausedTotal) = model.phase {
                TimelineView(.periodic(from: .now, by: 1)) { ctx in
                    let end = pausedAt ?? ctx.date
                    Text(Elapsed.format(end.timeIntervalSince(started) - pausedTotal))
                        .font(.system(.body, design: .monospaced))
                        .foregroundStyle(pausedAt == nil ? .primary : .secondary)
                        .frame(width: 64)
                }
                Button { pausedAt == nil ? model.pauseRecording() : model.resumeRecording() } label: {
                    Image(systemName: pausedAt == nil ? "pause.fill" : "play.fill")
                }
                .help(pausedAt == nil ? LocalizedStringKey("Pausar") : LocalizedStringKey("Continuar"))
            } else {
                ProgressView().controlSize(.small).frame(width: 64)
            }
            Divider().frame(height: 20)
            Button { model.cancelRecording() } label: { Image(systemName: "trash") }
                .help("Descartar la grabación")
        }
        .buttonStyle(.borderless)
        .font(.system(size: 15, weight: .medium))
        .padding(.horizontal, 14)
        .frame(width: 250, height: 44)
        .background(.regularMaterial, in: Capsule())
    }
}

/// Waits for an async call that macOS may never answer (a pending permission).
enum Timeout {
    struct Expired: Error {}

    static func run(seconds: Double, _ op: @escaping @Sendable () async throws -> Void) async throws {
        let done = LockedFlag()
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            Task {
                do {
                    try await op()
                    if done.set() { cont.resume() }
                } catch {
                    if done.set() { cont.resume(throwing: error) }
                }
            }
            Task {
                try? await Task.sleep(for: .seconds(seconds))
                if done.set() { cont.resume(throwing: Expired()) }
            }
        }
    }
}

/// True only for the first caller.
final class LockedFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var isSet = false
    func set() -> Bool { lock.withLock { defer { isSet = true }; return !isSet } }
}

enum Elapsed {
    static func format(_ seconds: Double) -> String {
        let s = max(0, Int(seconds))
        return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60)
            : String(format: "%d:%02d", s / 60, s % 60)
    }
}
