import XCTest

@testable import CallActivity

/// Why a creature's still is not drawn, which the widget extension can only say in the device's log.
@available(iOS 16.1, *)
final class StillsTests: XCTestCase {
    private var saved: URL?

    override func setUp() {
        saved = CallStills.directory
    }

    override func tearDown() {
        CallStills.directory = saved
    }

    func testAStillThatIsThereHasNoProblem() {
        CallStills.directory = Bundle.module.url(forResource: "Fixtures", withExtension: nil)
        XCTAssertNil(CallStills.problem("straw-hat.png"))
        XCTAssertNotNil(CallStills.image("straw-hat.png"))
    }

    func testNoAppGroupContainerIsNamed() {
        CallStills.directory = nil
        XCTAssertTrue(CallStills.problem("straw-hat.png")?.contains("no App Group container") ?? false)
        XCTAssertNil(CallStills.image("straw-hat.png"))
    }

    func testAMissingFileIsNamed() {
        CallStills.directory = Bundle.module.url(forResource: "Fixtures", withExtension: nil)
        XCTAssertTrue(CallStills.problem("nope.png")?.contains("no file nope.png") ?? false)
        XCTAssertNil(CallStills.image("nope.png"))
    }

    func testAPathIsNotAFileName() {
        CallStills.directory = Bundle.module.url(forResource: "Fixtures", withExtension: nil)
        XCTAssertNotNil(CallStills.problem("../x.png"))
    }
}
