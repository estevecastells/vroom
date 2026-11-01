// Records the main display for a few seconds and prints what came out:
// `swift run RecordCheck /tmp/check.mp4 [seconds] [--mic]`. Exercises the
// capture pipeline without the UI (needs Screen Recording permission).

import AVFoundation
import Foundation
import VroomCapture

let args = CommandLine.arguments.dropFirst()
guard let path = args.first else {
    print("usage: RecordCheck <out.mp4> [seconds] [--mic] [--system] [--region | --window]")
    exit(2)
}
let seconds = args.dropFirst().first.flatMap(Double.init) ?? 3
let mic = args.contains("--mic") ? Devices.defaultMicrophone : nil
let system = args.contains("--system")
// --region records 800×500 points of the main display; --window the frontmost window of another app.
let target: CaptureTarget = {
    if args.contains("--region") { return .region(CGMainDisplayID(), CGRect(x: 100, y: 100, width: 800, height: 500)) }
    if args.contains("--window") {
        let own = ProcessInfo.processInfo.processIdentifier
        let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
        if let w = list.first(where: { ($0[kCGWindowLayer as String] as? Int) == 0 && ($0[kCGWindowOwnerPID as String] as? Int32) != own }),
           let id = w[kCGWindowNumber as String] as? CGWindowID {
            print("window \(id) of \(w[kCGWindowOwnerName as String] ?? "?")")
            return .window(id)
        }
    }
    return .display(CGMainDisplayID())
}()
let url = URL(fileURLWithPath: path)

guard Devices.canRecordScreen else {
    print("Screen Recording permission missing for this terminal")
    Devices.requestScreen()
    exit(1)
}

Task {
    do {
        let recorder = Recorder(settings: RecorderSettings(target: target, microphoneID: mic, systemAudio: system), url: url)
        try await recorder.start()
        try await Task.sleep(for: .seconds(seconds / 2))
        recorder.pause()
        try await Task.sleep(for: .seconds(1))
        recorder.resume()
        try await Task.sleep(for: .seconds(seconds / 2))
        let clock = ContinuousClock.now
        let result = try await recorder.stop()
        let finish = ContinuousClock.now - clock
        let size = (try FileManager.default.attributesOfItem(atPath: path)[.size] as? Int) ?? 0
        print("ok \(result.width)x\(result.height) \(String(format: "%.2f", result.duration))s \(size) bytes, finished in \(finish)")
        try await Thumbnail.write(video: url, to: url.deletingPathExtension().appendingPathExtension("jpg"))
        exit(0)
    } catch {
        print("FAIL \(error.localizedDescription)")
        exit(1)
    }
}
RunLoop.main.run()
