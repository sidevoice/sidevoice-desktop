// swift-tools-version:5.5
// S0 spike: the native audio holder of run N on iOS.

import PackageDescription

let package = Package(
    name: "tauri-plugin-s0-holder",
    platforms: [
        .macOS(.v10_13),
        .iOS(.v15),
    ],
    products: [
        .library(
            name: "tauri-plugin-s0-holder",
            type: .static,
            targets: ["tauri-plugin-s0-holder"]),
    ],
    dependencies: [
        .package(name: "Tauri", path: "../.tauri/tauri-api")
    ],
    targets: [
        .target(
            name: "tauri-plugin-s0-holder",
            dependencies: [
                .byName(name: "Tauri")
            ],
            path: "Sources")
    ]
)
