// swift-tools-version: 6.0
import Foundation
import PackageDescription

// build.sh builds the Rust static library into build/rust and generates the UniFFI bindings
// (Sources/recto_ffiFFI/include and Sources/RectoPDF/Generated). This manifest only links them.
let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().path

let package = Package(
    name: "RectoPDF",
    platforms: [.macOS(.v15)],
    targets: [
        .target(name: "recto_ffiFFI", path: "Sources/recto_ffiFFI", publicHeadersPath: "include"),
        .executableTarget(
            name: "RectoPDF",
            dependencies: ["recto_ffiFFI"],
            path: "Sources/RectoPDF",
            swiftSettings: [.swiftLanguageMode(.v5)],
            linkerSettings: [
                .unsafeFlags(["-L\(root)/build/rust", "-lrecto_ffi"]),
                .linkedFramework("IOSurface"),
                .linkedFramework("QuartzCore"),
            ]
        ),
    ]
)
