import XCTest

/// What the harness's UI tests share: the harness app started on a fixture, and the Lock Screen and the island it is seen on.
extension XCTestCase {
    var springboard: XCUIApplication { XCUIApplication(bundleIdentifier: "com.apple.springboard") }

    /// The island's centre, from the top of the screen (pt): iPhone 17 Pro, top 11 and 36.67 tall (spec/GEOMETRY.md).
    var islandCenterY: CGFloat { 29 }

    /// Launches the harness app with its environment (`LA_FIXTURE`, …), once more if the activity did not start: a launch
    /// now and then reaches the app without its environment. What its status said: `started`, or why not.
    func start(_ app: XCUIApplication) -> String {
        var said = ""
        for _ in 0..<2 {
            app.launch()
            said = started(app)
            if said == "started" { break }
            app.terminate()
        }
        return said
    }

    /// What the harness app's status says once the activity started or failed to (up to 30 s).
    func started(_ app: XCUIApplication) -> String {
        let status = app.staticTexts["harness.status"]
        let done = NSPredicate(format: "label == 'started' OR label BEGINSWITH 'failed'")
        _ = XCTWaiter.wait(for: [XCTNSPredicateExpectation(predicate: done, object: status)], timeout: 30)
        return status.exists ? status.label : "no status"
    }

    /// Locked, then woken: the Lock Screen, with the app's activity on it.
    func showLockScreen() {
        XCUIDevice.shared.perform(NSSelectorFromString("pressLockButton"))
        settle(1)
        XCUIDevice.shared.press(.home)
        settle(1.5)
        allowLiveActivities()
    }

    /// A long press on the island, which expands it (the app in the background).
    func expandIsland() {
        let screen = springboard.frame
        springboard.coordinate(withNormalizedOffset: .zero)
            .withOffset(CGVector(dx: screen.width / 2, dy: islandCenterY))
            .press(forDuration: 1)
        settle(1.2)
    }

    /// Answers the system's questions over the app's card on the Lock Screen: whether it may show Live Activities
    /// (first "Allow", later "Always Allow"), until none is left.
    func allowLiveActivities() {
        // One query for both: each query of SpringBoard costs seconds.
        let button = springboard.buttons.matching(NSPredicate(format: "label IN {'Allow', 'Always Allow'}")).firstMatch
        for _ in 0..<3 where button.exists {
            button.tap()
            settle(2)
        }
    }

    func settle(_ seconds: TimeInterval) {
        Thread.sleep(forTimeInterval: seconds)
    }

    /// Where the PNGs go: `$LA_OUT` (`cargo xtask mobile live-activity ios` sets it).
    var out: URL {
        let path = ProcessInfo.processInfo.environment["LA_OUT"] ?? NSTemporaryDirectory() + "live-activity-sim"
        let url = URL(fileURLWithPath: path, isDirectory: true)
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }

    /// The whole screen as it is now, to `<name>.png`.
    func save(_ name: String) {
        let png = XCUIScreen.main.screenshot().pngRepresentation
        do {
            try png.write(to: out.appendingPathComponent("\(name).png"))
        } catch {
            XCTFail("\(name).png: \(error)")
        }
    }
}
