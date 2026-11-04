import XCTest

@testable import CallActivity

final class ActivityLifecycleTests: XCTestCase {
    func testNoCallNeedsNothing() {
        let life = ActivityLifecycle()
        XCTAssertEqual(life.need(inFront: true), .nothing)
        XCTAssertEqual(life.need(inFront: false), .nothing)
    }

    func testACallWithoutActivityRequestsOneInFrontAndWaitsInTheBackground() {
        var life = ActivityLifecycle()
        life.start()
        XCTAssertEqual(life.need(inFront: true), .request)
        XCTAssertEqual(life.need(inFront: false), .wait)
    }

    func testALiveActivityNeedsNothing() {
        var life = ActivityLifecycle()
        life.start()
        life.adopt("a")
        XCTAssertEqual(life.current, "a")
        XCTAssertEqual(life.need(inFront: true), .nothing)
        XCTAssertFalse(life.observed("a", .alive))
        XCTAssertEqual(life.current, "a")
    }

    func testAnEndedOrDismissedActivityIsDroppedAndAnotherRequested() {
        var life = ActivityLifecycle()
        life.start()
        life.adopt("a")
        XCTAssertTrue(life.observed("a", .over))
        XCTAssertNil(life.current)
        XCTAssertEqual(life.need(inFront: true), .request)
        life.adopt("b")
        XCTAssertEqual(life.current, "b")
        XCTAssertEqual(life.need(inFront: true), .nothing)
    }

    func testAReportOnAnOlderActivityDoesNotDropTheCurrentOne() {
        var life = ActivityLifecycle()
        life.start()
        life.adopt("a")
        XCTAssertTrue(life.observed("a", .over))
        life.adopt("b")
        XCTAssertFalse(life.observed("a", .over))
        XCTAssertEqual(life.current, "b")
    }

    func testTheCallEndingDropsEverythingAndWantsNothing() {
        var life = ActivityLifecycle()
        life.start()
        life.adopt("a")
        life.end()
        XCTAssertNil(life.current)
        XCTAssertEqual(life.need(inFront: true), .nothing)
        XCTAssertFalse(life.observed("a", .over))
    }

    func testANewCallForgetsTheOldActivity() {
        var life = ActivityLifecycle()
        life.start()
        life.adopt("a")
        life.start()
        XCTAssertNil(life.current)
        XCTAssertEqual(life.need(inFront: true), .request)
    }
}
