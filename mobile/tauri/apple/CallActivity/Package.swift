// swift-tools-version:5.9
// The call's Live Activity (docs/MOBILE.md → "The call's Live Activity"): what the app and its widget extension share.
//
// - `CallActivity`: the activity's attributes and state, its views (Lock Screen card, Dynamic Island), its App Intents and
//   DM Sans. Linked by the widget extension, which draws it, and by the app, where the intents run.
// - `CallActivityApp`: the app's side only: starting, updating and ending the activity from the call's surface, which the
//   Rust side hands over through `SidevoiceCallActivity`, and the audio route the card switches.
// - The tests render every state to PNG (`cargo xtask mobile renders ios`), the evidence a pull request shows.
//
// The app runs on iOS 15 (mobile/tauri/tauri.conf.json); everything here is gated to iOS 16.1, and the buttons to iOS 17.
import PackageDescription

let package = Package(
    name: "CallActivity",
    platforms: [.iOS(.v15)],
    products: [
        .library(name: "CallActivity", targets: ["CallActivity"]),
        .library(name: "CallActivityApp", targets: ["CallActivityApp"]),
    ],
    targets: [
        .target(name: "CallActivity", resources: [.copy("Fonts/DMSans.ttf"), .copy("Fonts/OFL.txt")]),
        .target(name: "CallActivityApp", dependencies: ["CallActivity"]),
        .testTarget(name: "CallActivityTests", dependencies: ["CallActivity"], resources: [.copy("Fixtures")]),
    ]
)
