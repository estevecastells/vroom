// A small JPEG poster for the library and the share page.

import AVFoundation
import ImageIO
import UniformTypeIdentifiers

public enum Thumbnail {
    public static func write(video: URL, to dest: URL, maxSide: CGFloat = 640) async throws {
        let asset = AVURLAsset(url: video)
        let generator = AVAssetImageGenerator(asset: asset)
        generator.appliesPreferredTrackTransform = true
        generator.maximumSize = CGSize(width: maxSide, height: maxSide)
        generator.requestedTimeToleranceBefore = .positiveInfinity
        generator.requestedTimeToleranceAfter = .positiveInfinity
        let duration = try await asset.load(.duration)
        // A second in skips the countdown's last frame and black starts.
        let at = CMTime(seconds: min(1, CMTimeGetSeconds(duration) / 2), preferredTimescale: 600)
        let (image, _) = try await generator.image(at: at)
        guard let out = CGImageDestinationCreateWithURL(dest as CFURL, UTType.jpeg.identifier as CFString, 1, nil) else {
            throw CocoaError(.fileWriteUnknown)
        }
        CGImageDestinationAddImage(out, image, [kCGImageDestinationLossyCompressionQuality: 0.75] as CFDictionary)
        guard CGImageDestinationFinalize(out) else { throw CocoaError(.fileWriteUnknown) }
    }
}
