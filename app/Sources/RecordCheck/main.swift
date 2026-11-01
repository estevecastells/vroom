// Records the main display for a few seconds and prints what came out:
// `swift run RecordCheck /tmp/check.mp4 [seconds] [--mic]`. Exercises the
// capture pipeline without the UI (needs Screen Recording permission).

import AVFoundation
import Foundation
import VroomCapture

let args = CommandLine.arguments.dropFirst()
guard let path = args.first else {
    print("usage: RecordCheck <out.mp4> [seconds] [--mic] [--system]")
    exit(2)
}
let seconds = args.dropFirst().first.flatMap(Double.init) ?? 3
let mic = args.contains("--mic") ? Devices.defaultMicrophone : nil
let system = args.contains("--system")
let url = URL(fileURLWithPath: path)

guard Devices.canRecordScreen else {
    print("Screen Recording permission missing for this terminal")
    Devices.requestScreen()
    exit(1)
}

Task {
    do {
        let recorder = Recorder(settings: RecorderSettings(displayID: CGMainDisplayID(), microphoneID: mic, systemAudio: system), url: url)
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
