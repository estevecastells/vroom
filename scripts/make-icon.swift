// Draws the app icon: `swift scripts/make-icon.swift app/Resources/AppIcon.icns`.
// A macOS-grid squircle with a warm gradient, a record button and speed lines.

import AppKit

let out = CommandLine.arguments.dropFirst().first ?? "AppIcon.icns"
let size: CGFloat = 1024

func render(_ px: Int) -> Data {
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8,
                               samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                               bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let ctx = NSGraphicsContext.current!.cgContext
    ctx.scaleBy(x: CGFloat(px) / size, y: CGFloat(px) / size)

    // Big Sur grid: 824 pt body, 100 pt margin, ~185 pt corner radius, soft shadow.
    let body = CGRect(x: 100, y: 100, width: 824, height: 824)
    let shape = CGPath(roundedRect: body, cornerWidth: 185, cornerHeight: 185, transform: nil)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: NSColor.black.withAlphaComponent(0.3).cgColor)
    ctx.addPath(shape)
    ctx.setFillColor(NSColor.black.cgColor)
    ctx.fillPath()
    ctx.restoreGState()

    ctx.saveGState()
    ctx.addPath(shape)
    ctx.clip()
    let colors = [NSColor(srgbRed: 1.00, green: 0.47, blue: 0.29, alpha: 1).cgColor,
                  NSColor(srgbRed: 0.86, green: 0.15, blue: 0.38, alpha: 1).cgColor] as CFArray
    let gradient = CGGradient(colorsSpace: CGColorSpace(name: CGColorSpace.sRGB), colors: colors, locations: [0, 1])!
    ctx.drawLinearGradient(gradient, start: CGPoint(x: 150, y: 924), end: CGPoint(x: 874, y: 100), options: [])
    // A faint highlight on the top half.
    ctx.setFillColor(NSColor.white.withAlphaComponent(0.07).cgColor)
    ctx.fillEllipse(in: CGRect(x: -200, y: 560, width: 1424, height: 760))
    ctx.restoreGState()

    // Speed lines on the left.
    ctx.setStrokeColor(NSColor.white.withAlphaComponent(0.85).cgColor)
    ctx.setLineCap(.round)
    for (y, x0, x1, w) in [(612.0, 210.0, 330.0, 34.0), (512.0, 170.0, 320.0, 38.0), (412.0, 210.0, 330.0, 34.0)] {
        ctx.setLineWidth(w)
        ctx.move(to: CGPoint(x: x0, y: y))
        ctx.addLine(to: CGPoint(x: x1, y: y))
        ctx.strokePath()
    }

    // Record button: white ring, red dot.
    let center = CGPoint(x: 590, y: 512)
    ctx.setFillColor(NSColor.white.cgColor)
    ctx.fillEllipse(in: CGRect(x: center.x - 230, y: center.y - 230, width: 460, height: 460))
    ctx.setFillColor(NSColor(srgbRed: 0.93, green: 0.18, blue: 0.27, alpha: 1).cgColor)
    ctx.fillEllipse(in: CGRect(x: center.x - 150, y: center.y - 150, width: 300, height: 300))

    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])!
}

let iconset = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("Vroom-\(getpid()).iconset")
try? FileManager.default.removeItem(at: iconset)
try FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
for base in [16, 32, 128, 256, 512] {
    try render(base).write(to: iconset.appendingPathComponent("icon_\(base)x\(base).png"))
    try render(base * 2).write(to: iconset.appendingPathComponent("icon_\(base)x\(base)@2x.png"))
}
let p = Process()
p.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
p.arguments = ["-c", "icns", iconset.path, "-o", out]
try p.run()
p.waitUntilExit()
try? render(1024).write(to: URL(fileURLWithPath: out).deletingPathExtension().appendingPathExtension("png"))
try? FileManager.default.removeItem(at: iconset)
exit(p.terminationStatus)
