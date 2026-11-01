// Turns a recording's microphone and system-audio tracks into one AAC track.
// The video is copied as is (no re-encoding); only the audio is decoded, mixed
// and encoded again, which takes a moment even for long recordings.

import AVFoundation

public enum AudioMixdown {
    /// System sound a bit lower, so the voice stays on top.
    static let systemVolume: Float = 0.7

    public static func run(_ url: URL) async throws {
        let asset = AVURLAsset(url: url)
        let audio = try await asset.loadTracks(withMediaType: .audio)
        var tracks: [AVAssetTrack] = []
        for t in audio where CMTimeGetSeconds(try await t.load(.timeRange).duration) > 0 { tracks.append(t) }
        guard tracks.count > 1, let video = try await asset.loadTracks(withMediaType: .video).first else { return }

        let tmp = url.deletingLastPathComponent().appendingPathComponent("video.mixing.mp4")
        try? FileManager.default.removeItem(at: tmp)
        let reader = try AVAssetReader(asset: asset)
        let videoOut = AVAssetReaderTrackOutput(track: video, outputSettings: nil)
        let audioOut = AVAssetReaderAudioMixOutput(audioTracks: tracks, audioSettings: [
            AVFormatIDKey: kAudioFormatLinearPCM,
            AVLinearPCMBitDepthKey: 16,
            AVLinearPCMIsFloatKey: false,
            AVLinearPCMIsBigEndianKey: false,
            AVLinearPCMIsNonInterleaved: false,
            AVSampleRateKey: 48_000,
            AVNumberOfChannelsKey: 2,
        ])
        let mix = AVMutableAudioMix()
        mix.inputParameters = tracks.enumerated().map { i, track in
            let p = AVMutableAudioMixInputParameters(track: track)
            p.setVolume(i == 0 ? 1 : systemVolume, at: .zero)
            return p
        }
        audioOut.audioMix = mix
        reader.add(videoOut)
        reader.add(audioOut)

        let writer = try AVAssetWriter(outputURL: tmp, fileType: .mp4)
        let hint = try await video.load(.formatDescriptions).first
        let videoIn = AVAssetWriterInput(mediaType: .video, outputSettings: nil, sourceFormatHint: hint)
        videoIn.transform = try await video.load(.preferredTransform)
        let audioIn = AVAssetWriterInput(mediaType: .audio, outputSettings: [
            AVFormatIDKey: kAudioFormatMPEG4AAC,
            AVSampleRateKey: 48_000,
            AVNumberOfChannelsKey: 2,
            AVEncoderBitRateKey: 128_000,
        ])
        writer.add(videoIn)
        writer.add(audioIn)

        guard reader.startReading() else { throw RecorderError.writer(reader.error?.localizedDescription ?? "lectura") }
        guard writer.startWriting() else { throw RecorderError.writer(writer.error?.localizedDescription ?? "escritura") }
        writer.startSession(atSourceTime: .zero)

        async let v: Void = pump(videoOut, into: videoIn, label: "video")
        async let a: Void = pump(audioOut, into: audioIn, label: "audio")
        _ = await (v, a)

        if reader.status == .failed {
            writer.cancelWriting()
            try? FileManager.default.removeItem(at: tmp)
            throw RecorderError.writer(reader.error?.localizedDescription ?? "lectura")
        }
        await writer.finishWriting()
        guard writer.status == .completed else {
            try? FileManager.default.removeItem(at: tmp)
            throw RecorderError.writer(writer.error?.localizedDescription ?? "escritura")
        }
        _ = try FileManager.default.replaceItemAt(url, withItemAt: tmp)
    }

    private static func pump(_ output: AVAssetReaderOutput, into input: AVAssetWriterInput, label: String) async {
        let queue = DispatchQueue(label: "vroom.mixdown.\(label)")
        nonisolated(unsafe) let output = output
        nonisolated(unsafe) let input = input
        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
            input.requestMediaDataWhenReady(on: queue) {
                while input.isReadyForMoreMediaData {
                    guard let sample = output.copyNextSampleBuffer() else {
                        input.markAsFinished()
                        cont.resume()
                        return
                    }
                    if !input.append(sample) {
                        input.markAsFinished()
                        cont.resume()
                        return
                    }
                }
            }
        }
    }
}
