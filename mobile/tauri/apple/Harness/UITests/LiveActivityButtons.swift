import XCTest

/// The Live Activity's buttons tapped where the person taps them, on the Lock Screen card and in the expanded island, the
/// real way: the system runs the button's intent in the harness app, whose call's stand-in (App/CallStandIn.swift) is the
/// class the intent hands the action to, and answers as the app's call would. Each test asserts that the stand-in was asked
/// for the action (`harness.actions`, read from the app afterwards) and that the activity shows the answer: the new mic
/// state within 1 s of the tap (seen by a query begun within 1 s; each query of SpringBoard takes a while) and still 2 s
/// later, the card gone after a hang-up, the next state after a skip or a cancel. Mute's rule (desktop#430): a tap does
/// what the card showed, so Silenciar mutes and Activar micrófono unmutes. Light appearance only.
///
/// Not tapped: the audio switch, which the activity draws only while headphones are connected, and the Simulator has none.
final class LiveActivityButtons: XCTestCase {
    let es = Texts.of(.es)
    /// Whether the first Lock Screen after boot has been seen (it shows no card, and asks to allow Live Activities).
    static var warm = false

    enum Place: String {
        case lock, island
    }

    override func setUpWithError() throws {
        continueAfterFailure = true
        let appearance = ProcessInfo.processInfo.environment["LA_APPEARANCE"] ?? "light"
        try XCTSkipIf(appearance != "light", "the buttons are tapped in light appearance only")
        if !Self.warm {
            Self.warm = true
            XCTContext.runActivity(named: "warm-up") { _ in
                guard call(Fixture.details("idle")) != nil else { return }
                showLockScreen()
                XCUIDevice.shared.press(.home)
                settle(1)
            }
        }
    }

    // MARK: the Lock Screen card

    func testLockScreenMute() {
        guard let app = call(Fixture.details("listening")) else { return }
        showLockScreen()
        tap(es.mute, in: .lock, sees: control(es.unmute), stays: true)
        tap(es.unmute, in: .lock, sees: control(es.mute), stays: true)
        XCUIDevice.shared.press(.home)
        settle(1)
        assertMutes(heard(app), [true, false])
    }

    func testLockScreenHangUp() {
        guard let app = call(Fixture.details("working")) else { return }
        showLockScreen()
        tap(es.hangUp, in: .lock, gone: true)
        XCUIDevice.shared.press(.home)
        settle(1)
        XCTAssertEqual(heard(app), ["hang-up"])
    }

    func testLockScreenSkip() {
        guard let app = call(Fixture.details("speaking"), answers: ["skip": Fixture.details("listening")]) else { return }
        showLockScreen()
        tap(es.skipName, in: .lock, sees: text(es.states["listening"]!))
        XCUIDevice.shared.press(.home)
        settle(1)
        XCTAssertEqual(heard(app), ["skip"])
    }

    // MARK: the expanded island (the mute switch and hang up while closed, skip while speaking, × while listening)

    func testIslandMute() {
        guard let app = call(Fixture.details("working")) else { return }
        openIsland()
        tap(es.mute, in: .island, sees: control(es.unmute), stays: true)
        tap(es.unmute, in: .island, sees: control(es.mute), stays: true)
        assertMutes(heard(app), [true, false])
    }

    func testIslandHangUp() {
        guard let app = call(Fixture.details("working")) else { return }
        openIsland()
        tap(es.hangUp, in: .island, gone: true)
        XCTAssertEqual(heard(app), ["hang-up"])
    }

    func testIslandSkip() {
        guard let app = call(Fixture.details("speaking"), answers: ["skip": Fixture.details("listening")]) else { return }
        openIsland()
        tap(es.skipName, in: .island, sees: control(es.cancelSend))
        XCTAssertEqual(heard(app), ["skip"])
    }

    func testIslandCancelSend() {
        guard let app = call(Fixture.details("listening"), answers: ["cancel-send": Fixture.details("idle")]) else {
            return
        }
        openIsland()
        tap(es.cancelSend, in: .island, sees: control(es.mute))
        XCTAssertEqual(heard(app), ["cancel-send"])
    }

    // MARK: helpers

    /// The harness app started on `details`, its stand-in answering `answers` (details by action); nil, failed, if the
    /// activity did not start.
    func call(_ details: [String: Any], answers: [String: [String: Any]] = [:]) -> XCUIApplication? {
        let app = XCUIApplication()
        app.launchEnvironment["LA_FIXTURE"] = Self.json(details)
        app.launchEnvironment["LA_ANSWERS"] = Self.json(answers)
        let said = start(app)
        guard said == "started" else {
            XCTFail("the activity did not start (\(said))")
            return nil
        }
        return app
    }

    /// The app in the background, the island expanded.
    func openIsland() {
        XCUIDevice.shared.press(.home)
        settle(1.5)
        expandIsland()
    }

    /// A button or switch of the activity, by its accessibility label.
    func control(_ label: String) -> XCUIElement {
        let types = [XCUIElement.ElementType.button.rawValue, XCUIElement.ElementType.switch.rawValue]
        return springboard.descendants(matching: .any)
            .matching(NSPredicate(format: "label == %@ AND elementType IN %@", label, types as NSArray))
            .firstMatch
    }

    /// A text of the activity that starts with `prefix` (a state's name, before its timer).
    func text(_ prefix: String) -> XCUIElement {
        springboard.staticTexts.matching(NSPredicate(format: "label BEGINSWITH %@", prefix)).firstMatch
    }

    /// Taps the control labelled `label` in `place`, then asserts what the activity shows: `sees` within 1 s (and, with
    /// `stays`, still 2 s later: the activity's state, not only a toggle's own face), or, with `gone`, no card within 3 s.
    func tap(_ label: String, in place: Place, sees: XCUIElement? = nil, stays: Bool = false, gone: Bool = false) {
        let step = "\(name) \(place.rawValue) \(label)"
        var button = control(label)
        if !button.exists, place == .island {
            // The island may have closed after an earlier tap.
            expandIsland()
            button = control(label)
        }
        guard button.exists else {
            fail(step, "no control labelled \(label)")
            return
        }
        button.tap()
        if gone {
            let card = control(es.hangUp)
            let over = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: card)
            if XCTWaiter.wait(for: [over], timeout: 3) != .completed { fail(step, "the card is still there 3 s after") }
            return
        }
        guard let sees else { return }
        guard let after = appears(sees, within: 1) else {
            if place == .island {
                expandIsland()
                fail(step, sees.exists ? "shown only once the island was opened again" : "not shown")
            } else {
                fail(step, "not shown within 1 s")
            }
            return
        }
        XCTContext.runActivity(named: "\(step): shown \(String(format: "%.2f", after)) s after the tap") { _ in }
        if stays {
            settle(2)
            if !sees.exists { fail(step, "shown, then gone 2 s later: the activity's state did not follow") }
        }
    }

    /// The seconds after which `element` was found, by a query begun within `limit` s; nil if none found it.
    func appears(_ element: XCUIElement, within limit: TimeInterval) -> TimeInterval? {
        let tapped = Date()
        while Date().timeIntervalSince(tapped) <= limit {
            if element.exists { return Date().timeIntervalSince(tapped) }
            settle(0.1)
        }
        return nil
    }

    /// Fails `step`, with a shot of the screen and SpringBoard's tree beside the PNGs.
    func fail(_ step: String, _ why: String, file: StaticString = #filePath, line: UInt = #line) {
        let base = "buttons-failed-" + step.filter { $0.isLetter || $0.isNumber || $0 == " " }
            .replacingOccurrences(of: " ", with: "-")
        save(base)
        try? springboard.debugDescription.write(
            to: out.appendingPathComponent("\(base).txt"), atomically: true, encoding: .utf8)
        XCTFail("\(step): \(why)", file: file, line: line)
    }

    /// What the call's stand-in was asked, in order, as the harness app says (`harness.actions`), brought to the front.
    func heard(_ app: XCUIApplication) -> [String] {
        app.activate()
        let actions = app.staticTexts["harness.actions"]
        guard actions.waitForExistence(timeout: 10) else { return ["no harness.actions"] }
        return actions.label == "none" ? [] : actions.label.components(separatedBy: ", ")
    }

    /// Each of `heard` a mute (`mute`, `unmute` or `toggle-mute`), whose outcomes are `muted`.
    func assertMutes(_ heard: [String], _ muted: [Bool], file: StaticString = #filePath, line: UInt = #line) {
        let outcomes = heard.map { $0.hasSuffix(" muted=true") ? "true" : $0.hasSuffix(" muted=false") ? "false" : $0 }
        XCTAssertEqual(outcomes, muted.map(String.init), "the stand-in heard \(heard)", file: file, line: line)
    }

    static func json(_ object: Any) -> String {
        let data = try! JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        return String(decoding: data, as: UTF8.self)
    }
}
