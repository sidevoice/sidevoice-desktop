import XCTest

@testable import CallActivity

/// The words in chunks (D-LA-CHUNKS): where a chunk ends, and when it changes. The text is measured with a fake font, 8 pt a
/// character, so a 330-pt line holds 41 characters and the cases are exact; the real font is the render tests'.
final class TextChunksTests: XCTestCase {
    let chunker = TextChunker { CGFloat($0.count) * 8 }

    /// `count` words of eight `letter`s, the last one ending the sentence.
    func sentence(_ letter: String, words count: Int) -> String {
        (0..<count).map { _ in String(repeating: letter, count: 8) }.joined(separator: " ") + "."
    }

    func testAShortTextIsOneChunk() {
        let chunks = chunker.chunks("Hola mundo. Adiós.")
        XCTAssertEqual(chunks, [.init(text: "Hola mundo. Adiós.", start: 0)])
    }

    func testWholeSentencesFillAChunkAndTheNextSentenceStartsTheNext() {
        // One sentence is 3 lines: a second one would make 5.
        let text = sentence("a", words: 10) + " " + sentence("b", words: 10) + " " + sentence("c", words: 10)
        let chunks = chunker.chunks(text)
        XCTAssertEqual(chunks.map(\.text), [sentence("a", words: 10), sentence("b", words: 10), sentence("c", words: 10)])
        XCTAssertEqual(chunks.map(\.start), [0, 91, 182])
    }

    func testTwoSentencesShareAChunkWhenTheyFit() {
        let text = sentence("a", words: 4) + " " + sentence("b", words: 4)
        XCTAssertEqual(chunker.chunks(text).count, 1)
    }

    func testASentenceTooLongIsCutAtItsLastClauseWhenThatKeepsHalf() {
        let text = "aaaaaaaa bbbbbbbb cccccccc, dddddddd eeeeeeee ffffffff"
        let chunks = chunker.chunks(text, lines: 1)
        XCTAssertEqual(chunks.map(\.text), ["aaaaaaaa bbbbbbbb cccccccc,", "dddddddd eeeeeeee ffffffff"])
        XCTAssertEqual(chunks.map(\.start), [0, 28])
    }

    func testAClauseTooEarlyIsIgnoredAndTheCutIsAfterTheLastWordThatFits() {
        let text = "a, bbbbbbbb cccccccc dddddddd eeeeeeee ffffffff"
        let chunks = chunker.chunks(text, lines: 1)
        XCTAssertEqual(chunks.map(\.text), ["a, bbbbbbbb cccccccc dddddddd eeeeeeee", "ffffffff"])
    }

    func testEveryChunkFitsAndNoneEndsInAnEllipsisOfItsOwn() {
        let text = (0..<12).map { sentence(String(UnicodeScalar(97 + $0)!), words: 7 + $0 % 5) }.joined(separator: " ")
        for lines in [2, 4] {
            let chunks = chunker.chunks(text, lines: lines)
            XCTAssertGreaterThan(chunks.count, 1)
            for chunk in chunks {
                XCTAssertTrue(chunker.fitsText(chunk.text, lines), "\(chunk.text) is more than \(lines) lines")
                XCTAssertFalse(chunk.text.hasSuffix("…"))
            }
            // Nothing is lost or reordered.
            XCTAssertEqual(chunks.map(\.text).joined(separator: " "), text)
        }
    }

    func testAWordLongerThanALineIsBrokenByCharactersAndStillFitsItsLines() {
        let text = String(repeating: "x", count: 100) + " fin."
        let chunks = chunker.chunks(text, lines: 4)
        XCTAssertEqual(chunks.count, 1)
        XCTAssertTrue(chunker.fitsText(chunks[0].text, 4))
    }

    func testYourWordsAreTwoLineChunksAtMostThreeTheLastEndingInAnEllipsis() {
        let text = (0..<5).map { sentence(String(UnicodeScalar(97 + $0)!), words: 8) }.joined(separator: " ")
        let chunks = chunker.saidChunks(text)
        XCTAssertEqual(chunks.count, 3)
        XCTAssertEqual(chunks[0].text, sentence("a", words: 8))
        XCTAssertTrue(chunks[2].text.hasSuffix("…"))
        XCTAssertTrue(chunker.fitsText(chunks[2].text, 2))
    }

    func testYourWordsWithinThreeChunksAreWhole() {
        let text = sentence("a", words: 8) + " " + sentence("b", words: 8)
        let chunks = chunker.saidChunks(text)
        XCTAssertEqual(chunks.count, 2)
        XCTAssertFalse(chunks.contains { $0.text.hasSuffix("…") })
    }

    func testBlankTextHasNoChunks() {
        XCTAssertTrue(chunker.chunks("  \n ").isEmpty)
    }

    func testTheRealFontCutsTheDesignsExampleInTwoChunksOfFourLines() {
        let real = TextChunker.dmSans()
        let text = "He movido el globo encima del bicho: ya no le tapa la cara. También he ajustado la cola para que apunte a "
            + "quien habla, y en el móvil el globo baja un poco para no pisar la cabecera. He revisado el iPad en "
            + "horizontal y ahí también encaja. ¿Lo subo a TestFlight y lo pruebas en el iPhone, o prefieres que antes "
            + "repase el modo oscuro?"
        let chunks = real.chunks(text)
        XCTAssertEqual(chunks.count, 2)
        XCTAssertTrue(chunks[0].text.hasPrefix("He movido el globo"))
        XCTAssertTrue(chunks[1].text.hasPrefix("He revisado el iPad"))
        for chunk in chunks { XCTAssertTrue(real.fitsText(chunk.text, 4)) }
    }
}

extension TextChunker {
    /// Whether `text` takes at most `lines` lines.
    func fitsText(_ text: String, _ lines: Int) -> Bool {
        lineCount(Self.words(text)[...], limit: lines) <= lines
    }
}

/// When a chunk changes (D-LA-CHUNKS): at least 2.5 s each, never ahead of the voice, one update per chunk.
final class TextPagerTests: XCTestCase {
    let chunker = TextChunker { CGFloat($0.count) * 8 }
    let t0 = Date(timeIntervalSince1970: 1_000_000)

    func sentence(_ letter: String, words count: Int) -> String {
        (0..<count).map { _ in String(repeating: letter, count: 8) }.joined(separator: " ") + "."
    }

    var reply: String {
        [sentence("a", words: 10), sentence("b", words: 10), sentence("c", words: 10)].joined(separator: " ")
    }

    func testTheChunkTheVoiceIsInIsShownAndItStaysAtLeastTwoAndAHalfSeconds() {
        var pager = TextPager(chunker: chunker)
        let first = pager.page(kind: .reply, text: reply, heard: 0, now: t0)
        XCTAssertEqual(first.text, sentence("a", words: 10))
        XCTAssertNil(first.wake)
        // The voice reaches the second chunk's first word after a second: the first stays, and says when to look again.
        let early = pager.page(kind: .reply, text: reply, heard: 91, now: t0 + 1)
        XCTAssertEqual(early.text, sentence("a", words: 10))
        XCTAssertEqual(early.wake, t0 + 2.5)
        let later = pager.page(kind: .reply, text: reply, heard: 91, now: t0 + 2.5)
        XCTAssertEqual(later.text, sentence("b", words: 10))
        XCTAssertNil(later.wake)
    }

    func testAChunkIsNeverShownAheadOfTheVoice() {
        var pager = TextPager(chunker: chunker)
        _ = pager.page(kind: .reply, text: reply, heard: 0, now: t0)
        let slow = pager.page(kind: .reply, text: reply, heard: 89, now: t0 + 60)
        XCTAssertEqual(slow.text, sentence("a", words: 10), "the voice has not reached the second chunk's first word")
        XCTAssertNil(slow.wake)
    }

    func testAVoiceThatIsPastTheNextChunkJumpsToTheOneItIsIn() {
        var pager = TextPager(chunker: chunker)
        _ = pager.page(kind: .reply, text: reply, heard: 0, now: t0)
        let jumped = pager.page(kind: .reply, text: reply, heard: 185, now: t0 + 10)
        XCTAssertEqual(jumped.text, sentence("c", words: 10))
    }

    func testAnUpdateOnlyWhenTheChunkChanges() {
        var pager = TextPager(chunker: chunker)
        let a = pager.page(kind: .reply, text: reply, heard: 0, now: t0)
        let b = pager.page(kind: .reply, text: reply, heard: 40, now: t0 + 1)
        XCTAssertEqual(a.text, b.text, "the voice moving inside a chunk changes nothing the activity is sent")
    }

    func testANewTextStartsAtTheChunkTheVoiceIsInWithoutWaiting() {
        var pager = TextPager(chunker: chunker)
        _ = pager.page(kind: .reply, text: reply, heard: 0, now: t0)
        let other = [sentence("x", words: 10), sentence("y", words: 10)].joined(separator: " ")
        let page = pager.page(kind: .reply, text: other, heard: 95, now: t0 + 0.2)
        XCTAssertEqual(page.text, sentence("y", words: 10))
    }

    func testTheLastChunkStaysUntilTheStateChanges() {
        var pager = TextPager(chunker: chunker)
        _ = pager.page(kind: .reply, text: reply, heard: 0, now: t0)
        let end = pager.page(kind: .reply, text: reply, heard: 9999, now: t0 + 30)
        XCTAssertEqual(end.text, sentence("c", words: 10))
        XCTAssertNil(end.wake)
        let gone = pager.page(kind: nil, text: nil, heard: 0, now: t0 + 31)
        XCTAssertNil(gone.text)
    }

    func testYourWordsAreTwoAndAHalfSecondsAChunkAtMostThree() {
        var pager = TextPager(chunker: chunker)
        let said = (0..<4).map { sentence(String(UnicodeScalar(97 + $0)!), words: 8) }.joined(separator: " ")
        let first = pager.page(kind: .said, text: said, heard: 0, now: t0)
        XCTAssertEqual(first.text, sentence("a", words: 8))
        XCTAssertEqual(first.wake, t0 + 2.5)
        let second = pager.page(kind: .said, text: said, heard: 0, now: t0 + 2.6)
        XCTAssertEqual(second.text, sentence("b", words: 8))
        XCTAssertEqual(second.wake, t0 + 5)
        let third = pager.page(kind: .said, text: said, heard: 0, now: t0 + 5.1)
        XCTAssertTrue(third.text?.hasPrefix("cccccccc") == true)
        XCTAssertTrue(third.text?.hasSuffix("…") == true, "more was said than the three chunks hold")
        XCTAssertNil(third.wake)
        let still = pager.page(kind: .said, text: said, heard: 0, now: t0 + 100)
        XCTAssertEqual(still.text, third.text)
    }
}

/// Your words outlast the page's own 3 s (the page clears them then; three chunks take 7.5 s).
final class SaidHoldTests: XCTestCase {
    let chunker = TextChunker { CGFloat($0.count) * 8 }
    let t0 = Date(timeIntervalSince1970: 1_000_000)

    func sentence(_ letter: String) -> String {
        (0..<8).map { _ in String(repeating: letter, count: 8) }.joined(separator: " ") + "."
    }

    var said: String { ["a", "b", "c"].map(sentence).joined(separator: " ") }

    func testYourWordsStayThroughWorkingUntilTheirLastChunkHasHadItsTime() {
        var pager = TextPager(chunker: chunker)
        XCTAssertEqual(pager.page(kind: .said, text: said, heard: 0, now: t0).text, sentence("a"))
        // The page cleared them after 3 s and the call is working: the second chunk is still due, then the third.
        let second = pager.page(kind: nil, text: nil, heard: 0, holdable: true, now: t0 + 3)
        XCTAssertEqual(second.text, sentence("b"))
        XCTAssertTrue(second.held)
        XCTAssertEqual(second.wake, t0 + 5)
        let third = pager.page(kind: nil, text: nil, heard: 0, holdable: true, now: t0 + 5.1)
        XCTAssertEqual(third.text, sentence("c"))
        XCTAssertEqual(third.wake, t0 + 7.5, "it looks again to let go")
        let gone = pager.page(kind: nil, text: nil, heard: 0, holdable: true, now: t0 + 7.5)
        XCTAssertNil(gone.text)
        XCTAssertFalse(gone.held)
    }

    func testAnythingElseTakesOverAtOnce() {
        var pager = TextPager(chunker: chunker)
        _ = pager.page(kind: .said, text: said, heard: 0, now: t0)
        let other = pager.page(kind: nil, text: nil, heard: 0, holdable: false, now: t0 + 3)
        XCTAssertNil(other.text)
        XCTAssertFalse(other.held)
        // And what was held does not come back.
        XCTAssertNil(pager.page(kind: nil, text: nil, heard: 0, holdable: true, now: t0 + 3.1).text)
    }

    func testAReplyThatStartsWhileYourWordsAreHeldWins() {
        var pager = TextPager(chunker: chunker)
        _ = pager.page(kind: .said, text: said, heard: 0, now: t0)
        let reply = pager.page(kind: .reply, text: sentence("x"), heard: 0, holdable: false, now: t0 + 3)
        XCTAssertEqual(reply.text, sentence("x"))
        XCTAssertFalse(reply.held)
    }
}

/// Your voice's bars: 44 columns of 6 pt at least, dropped from the left in a narrow capsule.
@available(iOS 16.1, *)
final class VoiceBarsTests: XCTestCase {
    func testTheColumnsAreTheDesignsAndNeverADot() {
        XCTAssertEqual(VoiceBars.heights.count, 44)
        XCTAssertEqual(VoiceBars.heights.min(), 6)
        XCTAssertEqual(VoiceBars.heights.last, 6)
    }

    func testANarrowCapsuleDropsColumnsFromTheLeft() {
        XCTAssertEqual(VoiceBars.columns(in: 290), 44)
        XCTAssertEqual(VoiceBars.columns(in: 264), 44)
        XCTAssertEqual(VoiceBars.columns(in: 100), 17)
        XCTAssertEqual(VoiceBars.columns(in: 2), 0)
    }
}
