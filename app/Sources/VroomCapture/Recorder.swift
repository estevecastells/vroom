// Screen + microphone into one MP4, cheaply:
//  - ScreenCaptureKit hands over IOSurface-backed YUV frames only when the
//    screen changes, at most `fps` per second;
//  - AVAssetWriter feeds them straight to the hardware H.264/HEVC encoder;
//  - the file is fragmented every few seconds, so a crash keeps what was recorded.
// Every sample is handled on one serial queue, so the writer needs no locks.

import AVFoundation
import CoreMedia
import ScreenCaptureKit

/// What to record.
public enum CaptureTarget: Sendable, Equatable {
    case display(CGDirectDisplayID)
    case window(CGWindowID)
    /// A rectangle of a display, in points from the display's top-left corner.
    case region(CGDirectDisplayID, CGRect)
}

public struct RecorderSettings: Sendable, Equatable {
    public enum Codec: String, Sendable, CaseIterable { case h264, hevc }

    public var target: CaptureTarget
    /// `nil` records no audio.
    public var microphoneID: String?
    /// Longest side is scaled down to this many pixels (0 keeps the native size).
    public var maxHeight: Int
    public var fps: Int
    public var codec: Codec
    /// Average bits per pixel per frame; screen content compresses well.
    public var bitsPerPixel: Double
    public var showsCursor: Bool
    /// What the Mac plays (other apps, calls, videos), without Vroom's own sounds.
    public var systemAudio: Bool

    public init(target: CaptureTarget, microphoneID: String?, systemAudio: Bool = false, maxHeight: Int = 1080,
                fps: Int = 30, codec: Codec = .h264, bitsPerPixel: Double = 0.07, showsCursor: Bool = true) {
        self.target = target
        self.microphoneID = microphoneID
        self.systemAudio = systemAudio
        self.maxHeight = maxHeight
        self.fps = fps
        self.codec = codec
        self.bitsPerPixel = bitsPerPixel
        self.showsCursor = showsCursor
    }
}

public struct RecordingResult: Sendable {
    public let url: URL
    public let duration: Double
    public let width: Int
    public let height: Int
}

public enum RecorderError: Error, LocalizedError {
    case noDisplay
    case noWindow
    case writer(String)
    case noFrames

    public var errorDescription: String? {
        switch self {
        case .noDisplay: String(localized: "No se encuentra la pantalla seleccionada.")
        case .noWindow: String(localized: "La ventana elegida ya no está abierta.")
        case let .writer(m): String(localized: "No se pudo escribir el vídeo: \(m)")
        case .noFrames: String(localized: "No llegó ninguna imagen de la pantalla. Revisa el permiso de Grabación de pantalla.")
        }
    }
}

public final class Recorder: NSObject, @unchecked Sendable {
    public let settings: RecorderSettings
    public let url: URL

    private let queue = DispatchQueue(label: "vroom.recorder", qos: .userInitiated)
    private var stream: SCStream?
    private var micSession: AVCaptureSession?
    private var writer: AVAssetWriter?
    private var videoInput: AVAssetWriterInput?
    private var audioInput: AVAssetWriterInput?
    private var systemInput: AVAssetWriterInput?
    private var size = CGSize.zero

    // Queue-confined timing state (host clock, shared by SCStream and AVCapture).
    private var sessionStart: CMTime?
    private var lastVideoTime = CMTime.zero
    private var pausedAt: CMTime?
    private var pausedTotal = CMTime.zero
    private var failure: Error?
    private var abandoned = false

    /// Called on the recorder queue if capture stops by itself (display gone, permission revoked).
    public var onStop: (@Sendable (Error?) -> Void)?

    public init(settings: RecorderSettings, url: URL) {
        self.settings = settings
        self.url = url
    }

    /// `exceptWindows` are this app's windows that should appear in the video
    /// (the camera bubble); every other window of the app is left out.
    public func start(exceptWindows: [CGWindowID] = []) async throws {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        let ownBundle = Bundle.main.bundleIdentifier
        let ownApps = content.applications.filter {
            $0.processID == ProcessInfo.processInfo.processIdentifier || (ownBundle != nil && $0.bundleIdentifier == ownBundle)
        }
        let kept = content.windows.filter { exceptWindows.contains($0.windowID) }
        func display(_ id: CGDirectDisplayID) throws -> SCDisplay {
            guard let d = content.displays.first(where: { $0.displayID == id }) ?? content.displays.first
            else { throw RecorderError.noDisplay }
            return d
        }

        let config = SCStreamConfiguration()
        let filter: SCContentFilter
        let points: CGSize
        let scale: Double
        switch settings.target {
        case let .display(id):
            let d = try display(id)
            filter = SCContentFilter(display: d, excludingApplications: ownApps, exceptingWindows: kept)
            points = CGSize(width: d.width, height: d.height)
            scale = Self.backingScale(d.displayID)
        case let .region(id, rect):
            let d = try display(id)
            filter = SCContentFilter(display: d, excludingApplications: ownApps, exceptingWindows: kept)
            config.sourceRect = rect
            points = rect.size
            scale = Self.backingScale(d.displayID)
        case let .window(id):
            guard let w = content.windows.first(where: { $0.windowID == id }) else { throw RecorderError.noWindow }
            let center = CGPoint(x: w.frame.midX, y: w.frame.midY)
            let d = content.displays.first { $0.frame.contains(center) } ?? content.displays.first
            guard let d else { throw RecorderError.noDisplay }
            if kept.isEmpty {
                // Just the window, even when covered or moved around.
                filter = SCContentFilter(desktopIndependentWindow: w)
            } else {
                // The window plus the camera bubble, as they sit on the display.
                filter = SCContentFilter(display: d, including: [w] + kept)
                config.sourceRect = w.frame.offsetBy(dx: -d.frame.minX, dy: -d.frame.minY)
            }
            points = w.frame.size
            scale = Self.backingScale(d.displayID)
        }

        size = Self.outputSize(width: points.width, height: points.height, scale: scale,
                               maxHeight: settings.maxHeight)
        try makeWriter()

        config.width = Int(size.width)
        config.height = Int(size.height)
        config.minimumFrameInterval = CMTime(value: 1, timescale: CMTimeScale(settings.fps))
        // Bi-planar 4:2:0 is what the encoder wants: no colour conversion on our side.
        config.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange
        config.colorSpaceName = CGColorSpace.sRGB
        config.showsCursor = settings.showsCursor
        config.queueDepth = 5
        config.capturesAudio = settings.systemAudio
        config.excludesCurrentProcessAudio = true
        config.sampleRate = 48_000
        config.channelCount = 2

        let stream = SCStream(filter: filter, configuration: config, delegate: self)
        try stream.addStreamOutput(self, type: .screen, sampleHandlerQueue: queue)
        if settings.systemAudio { try stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: queue) }
        self.stream = stream

        if let micID = settings.microphoneID { try startMicrophone(id: micID) }
        try await stream.startCapture()
        // Cancelled (or timed out) while macOS was making up its mind.
        if abandoned {
            try? await stream.stopCapture()
            micSession?.stopRunning()
        }
    }

    public func pause() {
        queue.async { if self.pausedAt == nil { self.pausedAt = Self.now() } }
    }

    public func resume() {
        queue.async {
            guard let at = self.pausedAt else { return }
            self.pausedTotal = CMTimeAdd(self.pausedTotal, CMTimeSubtract(Self.now(), at))
            self.pausedAt = nil
        }
    }

    /// Stop capturing and close the file.
    public func stop() async throws -> RecordingResult {
        try? await stream?.stopCapture()
        stream = nil
        micSession?.stopRunning()
        micSession = nil
        let finished: RecordingResult = try await withCheckedThrowingContinuation { cont in
            queue.async {
                guard let writer = self.writer, let start = self.sessionStart, writer.status == .writing else {
                    self.writer?.cancelWriting()
                    cont.resume(throwing: self.failure ?? RecorderError.noFrames)
                    return
                }
                let end = self.lastVideoTime
                writer.endSession(atSourceTime: end)
                self.videoInput?.markAsFinished()
                self.audioInput?.markAsFinished()
                self.systemInput?.markAsFinished()
                let result = RecordingResult(
                    url: self.url, duration: CMTimeGetSeconds(CMTimeSubtract(end, start)),
                    width: Int(self.size.width), height: Int(self.size.height))
                writer.finishWriting {
                    if writer.status == .completed {
                        cont.resume(returning: result)
                    } else {
                        cont.resume(throwing: RecorderError.writer(writer.error?.localizedDescription ?? String(localized: "desconocido")))
                    }
                }
            }
        }
        // Voice and system sound were kept apart while recording; browsers
        // play only the first audio track, so the shared file gets one.
        if settings.microphoneID != nil && settings.systemAudio {
            try await AudioMixdown.run(url)
        }
        return finished
    }

    /// Stop and throw the file away.
    public func cancel() async {
        abandoned = true
        try? await stream?.stopCapture()
        stream = nil
        micSession?.stopRunning()
        micSession = nil
        await withCheckedContinuation { cont in
            queue.async {
                self.writer?.cancelWriting()
                try? FileManager.default.removeItem(at: self.url)
                cont.resume()
            }
        }
    }

    // MARK: Setup

    private func makeWriter() throws {
        try? FileManager.default.removeItem(at: url)
        let writer = try AVAssetWriter(outputURL: url, fileType: .mp4)
        // Fragments keep a crashed recording playable up to the last one; the
        // initial movie header goes first, so browsers can start playing at once.
        writer.movieFragmentInterval = CMTime(seconds: 4, preferredTimescale: 600)
        writer.shouldOptimizeForNetworkUse = false

        let pixels = size.width * size.height
        let bitrate = Int(pixels * Double(settings.fps) * settings.bitsPerPixel)
        var compression: [String: Any] = [
            AVVideoAverageBitRateKey: max(bitrate, 1_000_000),
            AVVideoExpectedSourceFrameRateKey: settings.fps,
            AVVideoMaxKeyFrameIntervalDurationKey: 2,
            AVVideoAllowFrameReorderingKey: false,
        ]
        if settings.codec == .h264 { compression[AVVideoProfileLevelKey] = AVVideoProfileLevelH264HighAutoLevel }
        let video = AVAssetWriterInput(mediaType: .video, outputSettings: [
            AVVideoCodecKey: settings.codec == .hevc ? AVVideoCodecType.hevc : AVVideoCodecType.h264,
            AVVideoWidthKey: Int(size.width),
            AVVideoHeightKey: Int(size.height),
            AVVideoCompressionPropertiesKey: compression,
            AVVideoColorPropertiesKey: [
                AVVideoColorPrimariesKey: AVVideoColorPrimaries_ITU_R_709_2,
                AVVideoTransferFunctionKey: AVVideoTransferFunction_ITU_R_709_2,
                AVVideoYCbCrMatrixKey: AVVideoYCbCrMatrix_ITU_R_709_2,
            ],
        ])
        video.expectsMediaDataInRealTime = true
        guard writer.canAdd(video) else { throw RecorderError.writer(String(localized: "vídeo no admitido")) }
        writer.add(video)
        videoInput = video

        if settings.microphoneID != nil {
            let audio = AVAssetWriterInput(mediaType: .audio, outputSettings: [
                AVFormatIDKey: kAudioFormatMPEG4AAC,
                AVSampleRateKey: 48_000,
                AVNumberOfChannelsKey: 1,
                AVEncoderBitRateKey: 96_000,
            ])
            audio.expectsMediaDataInRealTime = true
            if writer.canAdd(audio) {
                writer.add(audio)
                audioInput = audio
            }
        }
        // Added after the microphone: the mixdown relies on this track order.
        if settings.systemAudio {
            let system = AVAssetWriterInput(mediaType: .audio, outputSettings: [
                AVFormatIDKey: kAudioFormatMPEG4AAC,
                AVSampleRateKey: 48_000,
                AVNumberOfChannelsKey: 2,
                AVEncoderBitRateKey: 128_000,
            ])
            system.expectsMediaDataInRealTime = true
            if writer.canAdd(system) {
                writer.add(system)
                systemInput = system
            }
        }
        guard writer.startWriting() else {
            throw RecorderError.writer(writer.error?.localizedDescription ?? String(localized: "no se pudo empezar"))
        }
        self.writer = writer
    }

    private func startMicrophone(id: String) throws {
        guard let device = AVCaptureDevice(uniqueID: id) ?? AVCaptureDevice.default(for: .audio) else { return }
        let session = AVCaptureSession()
        let input = try AVCaptureDeviceInput(device: device)
        guard session.canAddInput(input) else { return }
        session.addInput(input)
        let output = AVCaptureAudioDataOutput()
        output.setSampleBufferDelegate(self, queue: queue)
        guard session.canAddOutput(output) else { return }
        session.addOutput(output)
        session.startRunning()
        micSession = session
    }

    // MARK: Samples (recorder queue)

    private func append(_ buffer: CMSampleBuffer, to input: AVAssetWriterInput?, isVideo: Bool) {
        guard let input, let writer, writer.status == .writing else {
            if let writer, writer.status == .failed, failure == nil {
                failure = RecorderError.writer(writer.error?.localizedDescription ?? String(localized: "desconocido"))
            }
            return
        }
        let pts = CMSampleBufferGetPresentationTimeStamp(buffer)
        if pausedAt.map({ CMTimeCompare(pts, $0) >= 0 }) == true { return }
        // Audio waits for the first frame, so the file starts with a picture.
        guard sessionStart != nil || isVideo else { return }
        let shifted = CMTimeSubtract(pts, pausedTotal)
        if sessionStart == nil {
            writer.startSession(atSourceTime: shifted)
            sessionStart = shifted
        }
        if isVideo && CMTimeCompare(shifted, lastVideoTime) <= 0 && lastVideoTime != .zero { return }
        guard input.isReadyForMoreMediaData else { return }
        let sample = pausedTotal == .zero ? buffer : Self.retimed(buffer, by: pausedTotal) ?? buffer
        if input.append(sample), isVideo { lastVideoTime = shifted }
    }

    private static func retimed(_ buffer: CMSampleBuffer, by offset: CMTime) -> CMSampleBuffer? {
        var count: CMItemCount = 0
        CMSampleBufferGetSampleTimingInfoArray(buffer, entryCount: 0, arrayToFill: nil, entriesNeededOut: &count)
        var timing = [CMSampleTimingInfo](repeating: CMSampleTimingInfo(), count: count)
        CMSampleBufferGetSampleTimingInfoArray(buffer, entryCount: count, arrayToFill: &timing, entriesNeededOut: &count)
        for i in timing.indices {
            timing[i].presentationTimeStamp = CMTimeSubtract(timing[i].presentationTimeStamp, offset)
            if timing[i].decodeTimeStamp.isValid {
                timing[i].decodeTimeStamp = CMTimeSubtract(timing[i].decodeTimeStamp, offset)
            }
        }
        var out: CMSampleBuffer?
        CMSampleBufferCreateCopyWithNewTiming(
            allocator: nil, sampleBuffer: buffer, sampleTimingEntryCount: count, sampleTimingArray: &timing,
            sampleBufferOut: &out)
        return out
    }

    private static func now() -> CMTime { CMClockGetTime(CMClockGetHostTimeClock()) }

    // MARK: Geometry

    static func backingScale(_ displayID: CGDirectDisplayID) -> Double {
        guard let mode = CGDisplayCopyDisplayMode(displayID), mode.width > 0 else { return 2 }
        return Double(mode.pixelWidth) / Double(mode.width)
    }

    /// Native pixels, scaled so the shorter side is at most `maxHeight`, even sizes for the encoder.
    public static func outputSize(width: Double, height: Double, scale: Double, maxHeight: Int) -> CGSize {
        var w = width * scale
        var h = height * scale
        let short = min(w, h)
        if maxHeight > 0, short > Double(maxHeight) {
            let f = Double(maxHeight) / short
            w *= f
            h *= f
        }
        func even(_ v: Double) -> Double { Double(Int(v.rounded()) & ~1) }
        return CGSize(width: even(w), height: even(h))
    }
}

extension Recorder: SCStreamOutput, SCStreamDelegate, AVCaptureAudioDataOutputSampleBufferDelegate {
    public func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard sampleBuffer.isValid else { return }
        if type == .audio {
            append(sampleBuffer, to: systemInput, isVideo: false)
            return
        }
        guard type == .screen else { return }
        // Only complete frames carry an image; idle ones just say nothing changed.
        guard let attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, createIfNecessary: false)
            as? [[SCStreamFrameInfo: Any]],
            let raw = attachments.first?[.status] as? Int,
            SCFrameStatus(rawValue: raw) == .complete
        else { return }
        append(sampleBuffer, to: videoInput, isVideo: true)
    }

    public func stream(_ stream: SCStream, didStopWithError error: Error) {
        queue.async {
            self.failure = error
            self.onStop?(error)
        }
    }

    public func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        append(sampleBuffer, to: audioInput, isVideo: false)
    }
}
