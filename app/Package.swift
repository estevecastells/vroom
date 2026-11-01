// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "Vroom",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Vroom", targets: ["Vroom"]),
    ],
    targets: [
        // Models of the `vroom --json` protocol and the CLI runner. No UI; fully testable.
        .target(name: "VroomKit"),
        // Screen, camera and microphone capture into an MP4 (ScreenCaptureKit + AVAssetWriter).
        .target(name: "VroomCapture"),
        // SwiftUI views, overlays and the app model.
        .target(name: "VroomUI", dependencies: ["VroomKit", "VroomCapture"]),
        // The menu bar app.
        .executableTarget(name: "Vroom", dependencies: ["VroomUI"]),
        // Self-checking test runner (XCTest isn't available with Command Line Tools alone).
        .executableTarget(name: "KitChecks", dependencies: ["VroomKit"], path: "Sources/KitChecks"),
        // Records a few seconds of the main display: `swift run RecordCheck <out.mp4> [seconds]`.
        .executableTarget(name: "RecordCheck", dependencies: ["VroomCapture"], path: "Sources/RecordCheck"),
    ]
)
