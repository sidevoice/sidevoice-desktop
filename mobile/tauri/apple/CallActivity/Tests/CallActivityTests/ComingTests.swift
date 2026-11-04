import XCTest

@testable import CallActivity

final class ComingTests: XCTestCase {
    func testNothingIsOnItsWayBeforeACall() {
        XCTAssertFalse(Coming().get())
    }

    func testACallWithLiveActivitiesOnHasItsActivityOnItsWayUntilItIsRequested() {
        let coming = Coming()
        coming.start(enabled: true)
        XCTAssertTrue(coming.get())
        coming.set(false)
        XCTAssertFalse(coming.get())
    }

    func testACallWithLiveActivitiesOffHasNoneOnItsWay() {
        let coming = Coming()
        coming.start(enabled: false)
        XCTAssertFalse(coming.get())
    }

    func testTheNextCallStartsAfreshEitherWay() {
        let coming = Coming()
        coming.start(enabled: true)
        coming.start(enabled: false)
        XCTAssertFalse(coming.get())
        coming.start(enabled: true)
        XCTAssertTrue(coming.get())
    }
}
