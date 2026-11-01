// Choosing what to record, Loom style: the screens dim, the window under the
// mouse lights up and a click picks it; or a drag draws the area to record.
// Esc cancels. One overlay per screen; they share the result.

import AppKit
import VroomCapture

@MainActor
enum SelectionOverlay {
    enum Mode { case window, region }

    /// Coordinates: CoreGraphics global space (origin at the top-left of the main display).
    struct WindowInfo {
        let id: CGWindowID
        let frame: CGRect
        let app: String
    }

    static func pick(_ mode: Mode) async -> CaptureTarget? {
        let windows = mode == .window ? onScreenWindows() : []
        return await withCheckedContinuation { cont in
            var panels: [OverlayPanel] = []
            var finished = false
            let finish: (CaptureTarget?) -> Void = { target in
                guard !finished else { return }
                finished = true
                panels.forEach { $0.orderOut(nil) }
                panels.removeAll()
                NSCursor.arrow.set()
                cont.resume(returning: target)
            }
            NSApp.activate(ignoringOtherApps: true)
            for screen in NSScreen.screens {
                let panel = OverlayPanel(screen: screen)
                panel.contentView = SelectionView(mode: mode, screen: screen, windows: windows, finish: finish)
                panel.orderFrontRegardless()
                panels.append(panel)
            }
            // The panel under the mouse takes the keyboard (Esc) and mouse-moved events.
            let mouse = NSEvent.mouseLocation
            (panels.first { NSMouseInRect(mouse, $0.frame, false) } ?? panels.first)?.makeKey()
            NSCursor.crosshair.set()
        }
    }

    /// Normal windows of other apps, front to back.
    static func onScreenWindows() -> [WindowInfo] {
        let own = ProcessInfo.processInfo.processIdentifier
        guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
            as? [[String: Any]] else { return [] }
        return list.compactMap { w in
            guard (w[kCGWindowLayer as String] as? Int) == 0,
                  (w[kCGWindowOwnerPID as String] as? Int32) != own,
                  let id = w[kCGWindowNumber as String] as? CGWindowID,
                  let b = w[kCGWindowBounds as String] as? NSDictionary,
                  let rect = CGRect(dictionaryRepresentation: b),
                  rect.width > 60, rect.height > 60,
                  (w[kCGWindowAlpha as String] as? Double ?? 1) > 0
            else { return nil }
            return WindowInfo(id: id, frame: rect, app: w[kCGWindowOwnerName as String] as? String ?? "")
        }
    }

    /// Height of the main display: flips between AppKit (bottom-left) and CoreGraphics (top-left).
    static var flipHeight: CGFloat { NSScreen.screens.first?.frame.maxY ?? 0 }

    static func cocoaRect(fromCG r: CGRect) -> NSRect {
        NSRect(x: r.minX, y: flipHeight - r.maxY, width: r.width, height: r.height)
    }

    static func displayID(_ screen: NSScreen) -> CGDirectDisplayID {
        (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value ?? CGMainDisplayID()
    }
}

final class OverlayPanel: NSPanel {
    init(screen: NSScreen) {
        super.init(contentRect: screen.frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        level = .screenSaver
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        acceptsMouseMovedEvents = true
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        isReleasedWhenClosed = false
    }

    override var canBecomeKey: Bool { true }
}

private final class SelectionView: NSView {
    let mode: SelectionOverlay.Mode
    let screen: NSScreen
    let windows: [SelectionOverlay.WindowInfo]
    let finish: (CaptureTarget?) -> Void
    private var hovered: SelectionOverlay.WindowInfo?
    private var dragStart: NSPoint?
    private var dragRect: NSRect?

    init(mode: SelectionOverlay.Mode, screen: NSScreen, windows: [SelectionOverlay.WindowInfo],
         finish: @escaping (CaptureTarget?) -> Void) {
        self.mode = mode
        self.screen = screen
        self.windows = windows
        self.finish = finish
        super.init(frame: NSRect(origin: .zero, size: screen.frame.size))
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .activeAlways, .inVisibleRect, .cursorUpdate],
                                       owner: self))
    }

    required init?(coder: NSCoder) { fatalError() }

    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func cursorUpdate(with event: NSEvent) { NSCursor.crosshair.set() }

    /// The window's frame in this view's coordinates.
    private func local(_ w: SelectionOverlay.WindowInfo) -> NSRect {
        let r = SelectionOverlay.cocoaRect(fromCG: w.frame)
        return r.offsetBy(dx: -screen.frame.minX, dy: -screen.frame.minY)
    }

    override func mouseMoved(with event: NSEvent) {
        window?.makeKey()
        guard mode == .window else { return }
        let global = NSEvent.mouseLocation
        let cg = CGPoint(x: global.x, y: SelectionOverlay.flipHeight - global.y)
        let hit = windows.first { $0.frame.contains(cg) }
        if hit?.id != hovered?.id {
            hovered = hit
            needsDisplay = true
        }
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        switch mode {
        case .window:
            mouseMoved(with: event)
            if let hovered { finish(.window(hovered.id)) }
        case .region:
            dragStart = p
            dragRect = nil
        }
    }

    override func mouseDragged(with event: NSEvent) {
        guard mode == .region, let start = dragStart else { return }
        let p = convert(event.locationInWindow, from: nil)
        dragRect = NSRect(x: min(start.x, p.x), y: min(start.y, p.y), width: abs(p.x - start.x), height: abs(p.y - start.y))
            .intersection(bounds)
        needsDisplay = true
    }

    override func mouseUp(with event: NSEvent) {
        guard mode == .region, let r = dragRect?.integral, r.width >= 80, r.height >= 80 else {
            dragRect = nil
            needsDisplay = true
            return
        }
        // Display-local points with a top-left origin, as ScreenCaptureKit wants.
        let rect = CGRect(x: r.minX, y: bounds.height - r.maxY, width: r.width, height: r.height)
        finish(.region(SelectionOverlay.displayID(screen), rect))
    }

    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { finish(nil) } // Esc
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.black.withAlphaComponent(0.35).setFill()
        bounds.fill()
        let accent = NSColor.systemRed
        var target: NSRect?
        if mode == .window, let hovered { target = local(hovered) }
        if mode == .region { target = dragRect }
        if let target {
            NSColor.clear.setFill()
            target.fill(using: .copy)
            accent.withAlphaComponent(0.12).setFill()
            target.fill()
            let border = NSBezierPath(rect: target.insetBy(dx: 1.5, dy: 1.5))
            border.lineWidth = 3
            accent.setStroke()
            border.stroke()
            let label = mode == .window ? (hovered?.app ?? "") : "\(Int(target.width)) × \(Int(target.height))"
            drawPill(label, at: NSPoint(x: target.midX, y: target.maxY - 28))
        }
        let hint = mode == .window
            ? String(localized: "Haz clic en la ventana que quieres grabar · Esc para cancelar")
            : String(localized: "Arrastra para elegir el área que quieres grabar · Esc para cancelar")
        drawPill(hint, at: NSPoint(x: bounds.midX, y: bounds.maxY - 90))
    }

    private func drawPill(_ text: String, at center: NSPoint) {
        guard !text.isEmpty else { return }
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 14, weight: .medium), .foregroundColor: NSColor.white,
        ]
        let size = (text as NSString).size(withAttributes: attrs)
        let box = NSRect(x: center.x - size.width / 2 - 12, y: center.y - size.height / 2 - 6,
                         width: size.width + 24, height: size.height + 12)
        NSColor.black.withAlphaComponent(0.75).setFill()
        NSBezierPath(roundedRect: box, xRadius: box.height / 2, yRadius: box.height / 2).fill()
        (text as NSString).draw(at: NSPoint(x: box.minX + 12, y: box.minY + 6), withAttributes: attrs)
    }
}

/// A dashed frame around the recorded area. It belongs to this app, so it is not recorded.
@MainActor
final class RegionFrame {
    private let panel: FloatingPanel

    init(around rect: NSRect) {
        let frame = rect.insetBy(dx: -4, dy: -4)
        panel = FloatingPanel(rect: frame, movable: false)
        panel.ignoresMouseEvents = true
        panel.hasShadow = false
        let view = DashedBorder(frame: NSRect(origin: .zero, size: frame.size))
        panel.contentView = view
    }

    func show() { panel.orderFrontRegardless() }
    func close() { panel.orderOut(nil) }
}

private final class DashedBorder: NSView {
    override func draw(_ dirtyRect: NSRect) {
        let path = NSBezierPath(rect: bounds.insetBy(dx: 1.5, dy: 1.5))
        path.lineWidth = 3
        path.setLineDash([10, 6], count: 2, phase: 0)
        NSColor.systemRed.setStroke()
        path.stroke()
    }
}
