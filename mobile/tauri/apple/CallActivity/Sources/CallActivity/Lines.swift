import CoreText
import SwiftUI

/// Text on the gallery's own line height, all of it at full strength (no karaoke, D-LA-CHUNKS). SwiftUI's lines are the
/// font's (DM Sans: 1.302 × its size), taller than the gallery's (15 on 18), and it has no line height below that, so the
/// lines are broken here, as CoreText breaks them for the same font and width, and each is drawn in its own box. A chunk
/// (`TextChunker`) is built to fit, so the ellipsis, after a whole word, is only a safety net: when it shows, the
/// measurement has drifted from the device's, and its length is logged (subsystem ai.sidevoice, category text; never its words).
@available(iOS 16.1, *)
struct Lines: View {
    var text: String
    var size: CGFloat
    var weight: CGFloat
    var tracking: CGFloat = 0
    var lineHeight: CGFloat
    var lines: Int
    var ink: Color

    var body: some View {
        GeometryReader { box in
            let broken = Self.lines(text, width: box.size.width, font: DMSans.ctFont(size, weight), tracking: tracking, max: lines)
            let _ = Self.report(broken, text, width: box.size.width)
            VStack(alignment: .leading, spacing: 0) {
                ForEach(Array(broken.enumerated()), id: \.offset) { _, line in
                    draw(line)
                        .font(DMSans.font(size, weight)).tracking(tracking)
                        .contentTransition(.identity)
                        .lineLimit(1)
                        .fixedSize()
                        .frame(height: lineHeight, alignment: .leading)
                }
            }
            // A new chunk replaces the last at once: no cross-fade, no line of one under the other (the system animates a
            // Live Activity's update with transitions, which these turn off).
            .id(text)
            .transition(.identity)
            .transaction { $0.animation = nil }
        }
        .frame(height: lineHeight * CGFloat(lines), alignment: .topLeading)
    }

    private func draw(_ line: Line) -> Text {
        let piece = (text as NSString).substring(with: line.range).replacingOccurrences(of: "\n", with: " ")
        return Text(line.ellipsis ? piece + "…" : piece).foregroundColor(ink)
    }

    /// Logs a chunk the view had to cut: the chunker's measurement drifted from the device's.
    private static func report(_ lines: [Line], _ text: String, width: CGFloat) {
        guard lines.last?.ellipsis == true else { return }
        CallLog.text.error("chunk of \(text.count) characters cut by the view at \(Int(width)) pt")
    }

    struct Line {
        var range: NSRange
        var ellipsis: Bool
    }

    /// `text` broken into at most `max` lines of `width`; the last one cut after a whole word, with room for "…", when
    /// more follows.
    static func lines(_ text: String, width: CGFloat, font: CTFont, tracking: CGFloat, max: Int) -> [Line] {
        guard width > 0, !text.isEmpty else { return [] }
        let attributes: [NSAttributedString.Key: Any] = [
            NSAttributedString.Key(kCTFontAttributeName as String): font,
            NSAttributedString.Key(kCTKernAttributeName as String): tracking,
        ]
        let string = NSAttributedString(string: text, attributes: attributes)
        let typesetter = CTTypesetterCreateWithAttributedString(string)
        let whole = text as NSString
        var lines: [Line] = []
        var at = 0
        while at < whole.length && lines.count < max {
            let count = CTTypesetterSuggestLineBreak(typesetter, at, Double(width))
            guard count > 0 else { break }
            lines.append(Line(range: NSRange(location: at, length: count), ellipsis: false))
            at += count
        }
        guard at < whole.length, var last = lines.popLast() else { return lines }
        // More follows: the last line keeps the whole words that fit before "…"; a line with no space to cut at (Chinese,
        // Japanese, a long address) is cut between characters, never inside one.
        let ellipsisWidth = measure("…", attributes)
        let start = last.range.location
        let spaces = CharacterSet.whitespacesAndNewlines
        var end = NSMaxRange(last.range)
        while end > start {
            // Without what ends it blank (spaces, a blank line).
            while end > start, let unit = UnicodeScalar(whole.character(at: end - 1)), spaces.contains(unit) {
                end -= 1
            }
            guard end > start else { break }
            let kept = whole.substring(with: NSRange(location: start, length: end - start))
            if measure(kept, attributes) + ellipsisWidth <= width {
                break
            }
            let space = whole.rangeOfCharacter(
                from: spaces, options: .backwards, range: NSRange(location: start, length: end - start))
            end = space.location != NSNotFound && space.location > start
                ? space.location
                : whole.rangeOfComposedCharacterSequence(at: end - 1).location
        }
        last.range.length = end - start
        last.ellipsis = true
        lines.append(last)
        return lines
    }

    private static func measure(_ text: String, _ attributes: [NSAttributedString.Key: Any]) -> CGFloat {
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: attributes))
        return CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
    }
}

