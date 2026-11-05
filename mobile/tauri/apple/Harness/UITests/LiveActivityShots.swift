import XCTest

/// Every fixture's Live Activity as the system draws it on the Simulator: the Lock Screen card, the compact island (the
/// app in the background) and the expanded island (a long press on it). The minimal island is not shot: a second
/// activity of the same app does not make the island show two minimal views (it shows one compact).
/// The PNGs go to `$LA_OUT` as `<presentation>-<fixture>-<appearance>.png`, `$LA_APPEARANCE` being the Simulator's
/// (`cargo xtask mobile live-activity ios` sets both, and the appearance); beside each expanded shot, the frames (pt) of
/// what SpringBoard shows at the top of the screen, as JSON. Dark appearance shoots only the fixtures marked `dark`.
/// `$LA_ONLY` (fixture names, comma-separated) shoots only those, in either appearance.
final class LiveActivityShots: XCTestCase {
    let environment = ProcessInfo.processInfo.environment

    lazy var appearance = environment["LA_APPEARANCE"] ?? "light"

    override func setUp() {
        continueAfterFailure = true
    }

    func testEveryFixture() throws {
        let only = Set((environment["LA_ONLY"] ?? "").split(separator: ",").map(String.init))
        // Named fixtures are shot in either appearance; otherwise dark shoots those marked for it.
        let fixtures = Fixture.all.filter {
            only.isEmpty ? appearance == "light" || $0.dark : only.contains($0.name)
        }
        XCTAssertFalse(fixtures.isEmpty, "no fixture named in LA_ONLY")
        // A first fixture whose shots are not kept: the first Lock Screen after the simulator boots shows no card, and
        // it is where the system first asks to allow the app's Live Activities.
        if let first = Fixture.all.first {
            XCTContext.runActivity(named: "warm-up") { _ in shoot(first, keep: false) }
        }
        for fixture in fixtures {
            XCTContext.runActivity(named: fixture.name) { _ in shoot(fixture) }
        }
    }

    /// Starts `fixture`'s activity in the harness app, then takes its shots.
    func shoot(_ fixture: Fixture, keep: Bool = true) {
        let app = XCUIApplication()
        app.launchEnvironment["LA_FIXTURE"] = fixture.json
        let said = start(app)
        guard said == "started" else {
            XCTFail("\(fixture.name): the activity did not start (\(said))")
            save("failed-\(fixture.name)")
            return
        }

        if fixture.presentations.contains(.lock) {
            // The Lock Screen: locked, then woken, with the app's activity on it; then unlocked, back to the app.
            showLockScreen()
            if keep { save(name(.lock, fixture)) }
            XCUIDevice.shared.press(.home)
            settle(1)
        }

        // The app in the background: the island shows the activity, compact.
        XCUIDevice.shared.press(.home)
        settle(1.5)
        if keep, fixture.presentations.contains(.compact) { save(name(.compact, fixture)) }

        if keep, fixture.presentations.contains(.expanded) {
            expandIsland()
            let shot = name(.expanded, fixture)
            save(shot)
            frames(shot)
            XCUIDevice.shared.press(.home)
            settle(0.8)
        }
    }

    func name(_ presentation: Fixture.Presentation, _ fixture: Fixture) -> String {
        "\(presentation.rawValue)-\(fixture.name)-\(appearance)"
    }

    /// What SpringBoard's accessibility shows in the top 300 pt of the screen, its frames in pt, to `<name>.json`: the
    /// island and the activity's elements in it, for the clipping checks to read.
    func frames(_ name: String) {
        var elements: [[String: Any]] = []
        func walk(_ element: XCUIElementSnapshot, _ depth: Int) {
            let frame = element.frame
            if !frame.isEmpty, frame.minY < 300 {
                elements.append([
                    "depth": depth,
                    "type": element.elementType.rawValue,
                    "identifier": element.identifier,
                    "label": element.label,
                    "value": (element.value as? CustomStringConvertible)?.description ?? "",
                    "frame": ["x": frame.minX, "y": frame.minY, "width": frame.width, "height": frame.height],
                ])
            }
            for child in element.children { walk(child, depth + 1) }
        }
        do {
            walk(try springboard.snapshot(), 0)
        } catch {
            XCTFail("\(name): no snapshot of SpringBoard: \(error)")
        }
        let screen = springboard.frame
        let report: [String: Any] = [
            "shot": "\(name).png",
            "screen": ["width": screen.width, "height": screen.height],
            "elements": elements,
        ]
        do {
            let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
            try data.write(to: out.appendingPathComponent("\(name).json"))
        } catch {
            XCTFail("\(name).json: \(error)")
        }
    }
}
