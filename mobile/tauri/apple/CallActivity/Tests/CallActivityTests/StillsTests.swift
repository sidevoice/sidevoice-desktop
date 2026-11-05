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

    func testAStillIsScaledDownToItsSlot() throws {
        CallStills.directory = Bundle.module.url(forResource: "Fixtures", withExtension: nil)
        let image = try XCTUnwrap(CallStills.image("straw-hat.png", side: 26))
        XCTAssertLessThanOrEqual(max(image.size.width, image.size.height) * image.scale, 78)
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

    func testTheNamedStillsSayWhetherEachIsThere() {
        CallStills.directory = Bundle.module.url(forResource: "Fixtures", withExtension: nil)
        let named = CallStills.naming([CallState.Creature(file: "straw-hat.png"), nil, CallState.Creature(file: "nope.png")])
        XCTAssertTrue(named.hasPrefix("straw-hat.png (there), none, nope.png (no file nope.png in "), named)
    }

    func testAPathIsNotAFileName() {
        CallStills.directory = Bundle.module.url(forResource: "Fixtures", withExtension: nil)
        XCTAssertNotNil(CallStills.problem("../x.png"))
    }
}
