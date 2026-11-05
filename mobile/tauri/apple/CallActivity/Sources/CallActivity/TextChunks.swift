import CoreGraphics
import CoreText
import Foundation

/// The words in the expanded island, without karaoke (design round 2, D-LA-CHUNKS; reference implementation:
/// research/live-activity-design-2026-11-02/src/text-chunks.js, ported here). A text is cut into chunks that always fit
/// their lines, measured with the island's own font; the activity draws one chunk at a time, replaced as the voice moves
/// on. Nothing is cut by the view: a chunk is built to fit.
///
/// The chunking runs where the activity's updates are produced (the app's `CallActivityApp` controller), because the
/// page's text arrives there with the voice's position and the app holds DM Sans; the page sends text, never chunks.
public struct TextChunker {
    /// One chunk: its text, and where its first word starts in the source text (UTF-16 units, as the voice's position).
    public struct Chunk: Equatable {
        public var text: String
        public var start: Int

        public init(text: String, start: Int) {
            self.text = text
            self.start = start
        }
    }

    /// The width a text's width is measured in a line of; the island's narrowest text column is 322 pt (17 Pro: 374 - 2 x 26,
    /// the bottom band's insets: D-LA-CORNERS), 2.5 % spare for the difference between this greedy wrap and the view's.
    public static let width: CGFloat = 314
    /// The reply: three lines, with Saltar below (the island's budget is 144 pt: R2-8, D-LA-144).
    public static let replyLines = 3
    /// Your words: all there is below the camera, five lines (the 90 pt under it).
    public static let saidLines = 5

    /// The width of a one-line string, in points.
    public typealias Measure = (String) -> CGFloat

    private let measure: Measure
    private let width: CGFloat

    public init(width: CGFloat = TextChunker.width, measure: @escaping Measure) {
        self.width = width
        self.measure = measure
    }

    /// The island's text: DM Sans 550 at 15 pt, measured by CoreText.
    public static func dmSans() -> TextChunker {
        let font = DMSans.ctFont(15, 550)
        return TextChunker { text in
            let attributes: [NSAttributedString.Key: Any] = [NSAttributedString.Key(kCTFontAttributeName as String): font]
            let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: attributes))
            return CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil))
        }
    }

    // MARK: Words

    struct Word {
        var text: String
        var start: Int
        /// Ends a sentence: `. ! ? …`, then any closing quote or bracket.
        var endsSentence: Bool
        /// Ends a clause: `, ; : — –`.
        var endsClause: Bool
    }

    static func words(_ text: String) -> [Word] {
        let whole = text as NSString
        var words: [Word] = []
        var at = 0
        let spaces = CharacterSet.whitespacesAndNewlines
        while at < whole.length {
            while at < whole.length, isSpace(whole.character(at: at), spaces) { at += 1 }
            let start = at
            while at < whole.length, !isSpace(whole.character(at: at), spaces) { at += 1 }
            guard at > start else { break }
            let word = whole.substring(with: NSRange(location: start, length: at - start))
            words.append(Word(text: word, start: start, endsSentence: endsSentence(word), endsClause: endsClause(word)))
        }
        return words
    }

    private static func isSpace(_ unit: unichar, _ spaces: CharacterSet) -> Bool {
        UnicodeScalar(unit).map(spaces.contains) ?? false
    }

    private static func endsSentence(_ word: String) -> Bool {
        var trimmed = Substring(word)
        while let last = trimmed.last, "\"»”)’'".contains(last) { trimmed = trimmed.dropLast() }
        guard let last = trimmed.last else { return false }
        return ".!?…".contains(last)
    }

    private static func endsClause(_ word: String) -> Bool {
        guard let last = word.last else { return false }
        return ",;:—–".contains(last)
    }

    // MARK: Fitting

    /// How many lines `text` takes: greedy word wrap at `width`; a word wider than a line is broken by characters.
    func lineCount(_ words: ArraySlice<Word>, limit: Int) -> Int {
        var lines = 0
        var line = ""
        for word in words {
            let next = line.isEmpty ? word.text : "\(line) \(word.text)"
            if measure(next) <= width {
                line = next
                continue
            }
            if !line.isEmpty { lines += 1 }
            line = word.text
            while measure(line) > width {
                var count = line.count - 1
                while count > 1, measure(String(line.prefix(count))) > width { count -= 1 }
                guard count >= 1, count < line.count else { break }
                lines += 1
                line = String(line.dropFirst(count))
            }
            if lines > limit { return lines }
        }
        if !line.isEmpty { lines += 1 }
        return lines
    }

    private func fits(_ words: ArraySlice<Word>, _ lines: Int) -> Bool {
        lineCount(words, limit: lines) <= lines
    }

    private func chunk(_ words: ArraySlice<Word>) -> Chunk {
        Chunk(text: words.map(\.text).joined(separator: " "), start: words.first?.start ?? 0)
    }

    /// `text` as chunks of at most `lines` lines, each but the last filling its lines (a chunk is never one line while more
    /// follows):
    ///  1. as many words as fit;
    ///  2. ended after the last sentence among them, else the last clause break, when the chunk then still takes as many
    ///     lines and keeps at least half of its words; otherwise after the last word that fits;
    ///  3. the rest, a sentence's tail included, starts the next chunk, which goes on filling with what follows. No ellipsis
    ///     inside a reply: the next chunk follows.
    public func chunks(_ text: String, lines: Int = TextChunker.replyLines) -> [Chunk] {
        let words = Self.words(text)
        var out: [Chunk] = []
        var from = 0
        while from < words.count {
            // One word at least: a word wider than the lines is broken by characters.
            var cut = from + 1
            while cut < words.count, fits(words[from...cut], lines) { cut += 1 }
            if cut < words.count { cut = end(words, from: from, cut: cut, lines: lines) }
            out.append(chunk(words[from..<cut]))
            from = cut
        }
        return out
    }

    /// Where the chunk `words[from..<cut]` (as many words as fit, more following) ends: after its last sentence, else its last
    /// clause break, if the chunk then takes as many lines and keeps at least half its words; otherwise at `cut`.
    private func end(_ words: [Word], from: Int, cut: Int, lines: Int) -> Int {
        let full = lineCount(words[from..<cut], limit: lines)
        for ends in [\Word.endsSentence, \Word.endsClause] {
            for index in stride(from: cut - 1, through: from, by: -1) where words[index][keyPath: ends] {
                // The last one decides: an earlier one keeps fewer words and no more lines.
                guard (index + 1 - from) * 2 >= cut - from, lineCount(words[from...index], limit: lines) == full else { break }
                return index + 1
            }
        }
        return cut
    }
}

/// Which chunk of a text the activity shows, and when it looks again: one chunk per update, at least 2.5 s each, never ahead
/// of the voice (D-LA-CHUNKS). Pure bookkeeping, apart from the clock, which is passed in.
public struct TextPager {
    public enum Kind: Equatable {
        /// The agent speaking: the chunk the voice is in.
        case reply
        /// Your words: 2.5 s a chunk, in order.
        case said
    }

    /// What to draw now, and when to ask again (nil: when something else changes).
    public struct Page: Equatable {
        public var text: String?
        public var wake: Date?
        /// Your words are still being shown although the call's phase has moved on (the page clears them after 3 s, the
        /// chunks take 2.5 s each): the caller draws them as your words.
        public var held = false
    }

    /// Whether the call's phase lets your words finish after the page has cleared them: the stage waits 3 s before it says
    /// working, so the page's phase is still idle (or can't-receive) when its said text ends. The voice speaking, you
    /// listening or transcribing, and offline take over at once.
    public static func holds(_ phase: CallState.Phase) -> Bool {
        switch phase {
        case .working, .idle, .holding: true
        case .listening, .transcribing, .said, .speaking, .offline: false
        }
    }

    /// A chunk stays up at least this long, even if the voice is faster.
    public static let minimum: TimeInterval = 2.5

    private let chunker: TextChunker
    private var source: (kind: Kind, text: String, scope: String)?
    private var chunks: [TextChunker.Chunk] = []
    private var shown = 0
    private var since = Date.distantPast
    private var begun = Date.distantPast

    public init(chunker: TextChunker) {
        self.chunker = chunker
    }

    /// The page at `now`: `heard` is how much of `text` the voice has said (UTF-16 units), for a reply. `holdable`: the call
    /// is in a phase that may let your words finish (`holds`): they stay, by the pager's clock, until their last chunk has had
    /// its 2.5 s, whatever the page said of them meanwhile (it clears them after 3 s). Anything else that comes (the voice
    /// speaking, you listening, offline), or another conversation (`scope`), takes over at once.
    public mutating func page(kind: Kind?, text: String?, heard: Int, holdable: Bool = false, scope: String = "", now: Date) -> Page {
        guard let kind, let text, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            if holdable, source?.kind == .said, source?.scope == scope, !chunks.isEmpty {
                let elapsed = now.timeIntervalSince(begun)
                let end = Self.minimum * Double(chunks.count)
                if elapsed < end {
                    let index = min(Int(elapsed / Self.minimum), chunks.count - 1)
                    let wake = begun.addingTimeInterval(Self.minimum * Double(index + 1))
                    return Page(text: chunks[index].text, wake: wake, held: true)
                }
            }
            source = nil
            chunks = []
            return Page(text: nil, wake: nil)
        }
        if source?.kind != kind || source?.text != text || source?.scope != scope {
            // A new text: the chunk the voice is in (a reply that starts mid-way), or the first of your words, at once.
            source = (kind, text, scope)
            chunks = chunker.chunks(text, lines: kind == .reply ? TextChunker.replyLines : TextChunker.saidLines)
            begun = now
            shown = kind == .reply ? target(heard) : 0
            since = now
        }
        guard !chunks.isEmpty else { return Page(text: nil, wake: nil) }
        switch kind {
        case .reply:
            let wanted = target(heard)
            if wanted > shown, now.timeIntervalSince(since) >= Self.minimum {
                shown = wanted
                since = now
            }
            let wake = wanted > shown ? since.addingTimeInterval(Self.minimum) : nil
            return Page(text: chunks[shown].text, wake: wake)
        case .said:
            shown = min(Int(now.timeIntervalSince(begun) / Self.minimum), chunks.count - 1)
            let wake = shown < chunks.count - 1 ? begun.addingTimeInterval(Self.minimum * Double(shown + 1)) : nil
            return Page(text: chunks[shown].text, wake: wake)
        }
    }

    /// The chunk the voice is in: the last whose first word it has reached.
    private func target(_ heard: Int) -> Int {
        chunks.lastIndex { $0.start <= heard } ?? 0
    }
}
