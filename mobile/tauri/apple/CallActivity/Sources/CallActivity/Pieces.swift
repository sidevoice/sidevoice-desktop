import SwiftUI
import UIKit
import os

// The pieces the presentations share: the creature in its slots, the state's glyphs, your voice's bars, the stack.

/// A creature's still from the stills folder. One that has not arrived (not named, not kept, no App Group, does not decode)
/// is never an empty slot (D-LA-STILL-FALLBACK): the Sidevoice mark, white, on the slot's own ground. `overflow` lets a
/// big accessory stand out of the slot, by the still's `k`; the mark never does.
@available(iOS 16.1, *)
struct Still: View {
    /// The ground a missing still's mark sits on: the tile's own (the tile draws it, the mark is 56 % of the slot), or a
    /// `#2f2741` disc (the compact, the minimal and the stack's faces, the mark at 60 %).
    enum Ground { case tile, disc }

    var creature: CallState.Creature?
    var grey = false
    var ground = Ground.tile
    var overflow = false
    /// The slot's side, in points: the still is drawn from a copy scaled down to it (`CallStills.image(_:side:)`).
    var side: CGFloat

    @ViewBuilder var body: some View {
        if let file = creature?.file, let image = CallStills.image(file, side: side * (overflow ? creature?.k ?? 1 : 1)) {
            let still = Image(uiImage: image).resizable().interpolation(.high).scaledToFit()
            let drawn = grey
                // Offline: the gallery's `grayscale(1) brightness(.85)` at 70 %.
                ? AnyView(still.grayscale(1).colorMultiply(Color(white: 0.85)).opacity(0.7))
                : AnyView(still)
            if overflow {
                drawn.scaleEffect(creature?.k ?? 1, anchor: UnitPoint(x: 0.5, y: 0.96))
            } else {
                drawn
            }
        } else {
            let _ = CallStills.missing(creature)
            GeometryReader { box in
                ZStack {
                    if ground == .disc {
                        Circle().fill(Color(hex: 0x2f2741))
                    }
                    SidevoiceMark()
                        .frame(width: box.size.width * (ground == .disc ? 0.6 : 0.56))
                        .opacity(grey ? 0.7 : 1)
                }
                .frame(width: box.size.width, height: box.size.height)
            }
        }
    }
}

/// The Sidevoice mark (assets/brand/sidevoice-mark-white.svg): five rounded bars on a 24 grid, white, the fourth at 45 %.
struct SidevoiceMark: View {
    private static let bars: [(x: CGFloat, y: CGFloat, height: CGFloat, opacity: Double)] = [
        (1.5, 9, 6, 1), (6, 6, 12, 1), (10.5, 1.5, 21, 1), (15, 6, 12, 0.45), (19.5, 9, 6, 1),
    ]

    var body: some View {
        GeometryReader { box in
            let unit = box.size.width / 24
            ZStack(alignment: .topLeading) {
                ForEach(0..<Self.bars.count, id: \.self) { index in
                    let bar = Self.bars[index]
                    Capsule()
                        .fill(Color.white.opacity(bar.opacity))
                        .frame(width: 3 * unit, height: bar.height * unit)
                        .offset(x: bar.x * unit, y: bar.y * unit)
                }
            }
        }
        .aspectRatio(1, contentMode: .fit)
    }
}

/// The creature's tile (Lock Screen 64, expanded 56 / 44): radius 30 %, a radial gradient from the tone into the tile's
/// background, and the still inset by 7 %, enlarged by `k` from near its bottom so a big accessory stands out (D-LA-FIT,
/// overflow; the presentation clips it). The red mic badge, bottom right, while the mic is off.
@available(iOS 16.1, *)
struct CreatureTile: View {
    var state: CallState
    var size: CGFloat
    var palette: Palette
    @Environment(\.isLuminanceReduced) private var reduced

    var body: some View {
        let tone = Tone.of(state.phase, dark: palette.dark)
        let inner = size * 0.86
        ZStack {
            if !reduced {
                RoundedRectangle(cornerRadius: size * 0.3, style: .continuous)
                    .fill(
                        RadialGradient(
                            stops: [
                                .init(color: tone.fill.opacity(0.34), location: 0),
                                .init(color: palette.tile, location: 0.72),
                            ],
                            center: UnitPoint(x: 0.5, y: 0.7),
                            startRadius: 0,
                            // CSS `circle` to the farthest corner from (50 %, 70 %).
                            endRadius: size * 0.8602))
            }
            Still(creature: state.creature, grey: state.phase == .offline, overflow: true, side: inner)
                .frame(width: inner, height: inner)
        }
        .frame(width: size, height: size)
        // The badge's 2-pt ring is a gap cut out of the tile, so whatever is behind the card shows through it (the Lock
        // Screen's material, the island's black), as the gallery's ring in the panel's colour reads.
        .mask {
            // The tile's box, and far around it for an accessory that stands out of it, less the badge's ring.
            ZStack(alignment: .bottomTrailing) {
                Rectangle().padding(-size)
                if state.mutedBadge {
                    Circle().frame(width: 26, height: 26).offset(x: 6, y: 6).blendMode(.destinationOut)
                }
            }
            .compositingGroup()
        }
        .overlay(alignment: .bottomTrailing) {
            if state.mutedBadge {
                MicBadge(size: 22, glyph: 12).offset(x: 4, y: 4)
            }
        }
    }
}

/// The red mic-off badge.
@available(iOS 16.1, *)
struct MicBadge: View {
    var size: CGFloat
    var glyph: CGFloat
    /// Around the badge, where nothing cuts a gap for it (the minimal island: its black).
    var ring: Color = .clear
    var ringWidth: CGFloat = 0

    var body: some View {
        Circle().fill(Ink.muted)
            .overlay(Image(systemName: "mic.slash.fill").font(.system(size: glyph * 0.85, weight: .semibold))
                .foregroundStyle(.white))
            .frame(width: size, height: size)
            .background(Circle().fill(ring).padding(-ringWidth))
    }
}

/// A round face: the whole still, shrunk to fit its circle (the island; D-LA-FIT, whole).
@available(iOS 16.1, *)
struct RoundFace: View {
    var state: CallState
    var size: CGFloat

    var body: some View {
        Still(creature: state.creature, grey: state.phase == .offline, ground: .disc, side: size)
            .frame(width: size, height: size)
            .clipShape(Circle())
    }
}

/// The other conversations: up to three faces of 26 overlapping by 8, each ringed apart from the next, enlarged by their
/// `k` (overflow), then the count of conversations (the call's own included, as the list it opens shows them);
/// nothing at all when the call has no other conversation.
/// One link to the app's conversation list (D-LA-SIMPLE).
@available(iOS 16.1, *)
struct OthersStack: View {
    var state: CallState
    var palette: Palette

    var body: some View {
        if !state.others.isEmpty {
            stack
        }
    }

    private var stack: some View {
        Link(destination: CallLinks.conversations) {
            HStack(spacing: 0) {
                faces
                if state.conversationCount > 0 {
                    Text("\(state.conversationCount)")
                        .font(DMSans.font(13, 650))
                        .foregroundStyle(palette.text2)
                        .line(19.11)
                        .padding(.leading, 3)
                        .padding(.trailing, 2)
                }
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(state.words.conversations)
    }

    static let face: CGFloat = 26
    static let step: CGFloat = 18
    /// The gap between a face and the one over it.
    static let ring: CGFloat = 2

    /// The faces, each over the one before it, which has a ring-wide gap cut out of it around the newer face: whatever is
    /// behind the card (the Lock Screen's material, the island's black) shows through, as the mic badge's ring does, in
    /// light and dark alike. The first of `others` underneath, on the left. The gap follows the newer creature's outline
    /// (`cut`), never its slot's circle: a creature is not a disc, and a circle cut a disc out of the face beneath that the
    /// newer one did not cover (on the Lock Screen's dark card, a black disc behind each face).
    private var faces: some View {
        let shown = Array(state.others.prefix(3).enumerated())
        return ZStack(alignment: .leading) {
            ForEach(shown, id: \.offset) { index, other in
                Still(creature: other, ground: .disc, overflow: true, side: Self.face)
                    .frame(width: Self.face, height: Self.face)
                    .mask {
                        ZStack {
                            Rectangle().padding(-Self.face)
                            ForEach(Array((index + 1)..<shown.count), id: \.self) { over in
                                Self.cut(shown[over].element)
                                    .offset(x: CGFloat(over - index) * Self.step)
                                    .blendMode(.destinationOut)
                            }
                        }
                        .compositingGroup()
                    }
                    .offset(x: CGFloat(index) * Self.step)
            }
        }
        .frame(width: Self.face + CGFloat(max(shown.count - 1, 0)) * Self.step, height: Self.face, alignment: .leading)
    }

    /// What a face cuts out of the one beneath it: its own outline grown by the ring, laid out as `Still` lays the face out
    /// (fitted to the slot, enlarged by its `k` from near its bottom, so the ring is `ring` wide once enlarged). A face whose
    /// still has not arrived is the placeholder's disc, so it cuts the disc grown by the ring.
    @ViewBuilder static func cut(_ creature: CallState.Creature?) -> some View {
        let k = creature?.k ?? 1
        if let file = creature?.file, let outline = CallStills.outline(file, slot: face, k: k, grow: ring / k) {
            Image(uiImage: outline.image).resizable().interpolation(.high)
                .frame(width: outline.size.width, height: outline.size.height)
                .scaleEffect(k, anchor: outline.anchor)
                .frame(width: face, height: face)
        } else {
            Circle().frame(width: face + 2 * ring, height: face + 2 * ring)
        }
    }
}

enum CallLinks {
    static let call = URL(string: "sidevoice://call")!
    static let conversations = URL(string: "sidevoice://conversations")!
}

/// The state's dot (8 pt, the tone's fill) and its line, in the tone's ink; while working, " · 1:42", a system timer
/// since the working phase began (no other state shows a time).
@available(iOS 16.1, *)
struct StateLine: View {
    var state: CallState
    var dark: Bool
    var size: CGFloat
    var lineHeight: CGFloat

    var body: some View {
        let tone = Tone.of(state.phase, dark: dark)
        HStack(spacing: 6) {
            Circle().fill(tone.fill).frame(width: 8, height: 8)
            label(tone)
                .font(DMSans.font(size, 650))
                .foregroundStyle(tone.ink)
                .line(lineHeight)
        }
    }

    private func label(_ tone: Tone) -> Text {
        let words = Text(state.words.state)
        guard state.phase == .working, let since = state.workingSince else { return words }
        // Tabular digits, so "0:42" keeps its width as it counts.
        let time = (Text("· ") + Text(since, style: .timer)).monospacedDigit()
        // The gallery's 6-pt gap: an en space (7.5 pt at 15, 7 at 14), the closest a run of text has.
        return words + Text("\u{2002}") + time.font(DMSans.font(size, 600)).foregroundColor(tone.ink.opacity(0.85))
    }
}

/// The compact island's state glyph, 20 pt in the tone (none while idle), with its entrance (≤ 2 s, then still; never on
/// Always-On; HANDOVER §5).
@available(iOS 16.1, *)
struct StateGlyph: View {
    var phase: CallState.Phase
    var size: CGFloat = 20

    var body: some View {
        let tone = Tone.of(phase, dark: true)
        Group {
            switch phase {
            case .idle:
                EmptyView()
            case .listening:
                // The mic (scheme E): you are talking.
                symbol("mic.fill", effect: .pulse)
            case .transcribing:
                // The bars, at full tone, breathing while the words are transcribed (operator, 2026-11-05).
                GlyphBars(breathe: true).frame(width: size, height: size).foregroundStyle(Ink.voice(dark: true))
            case .said:
                GlyphBars().frame(width: size, height: size).foregroundStyle(Ink.voice(dark: true))
            case .working:
                symbol("ellipsis", effect: .bounce)
            case .speaking:
                symbol("speaker.wave.2.fill", effect: .pulse)
            case .holding:
                // A slashed tray (operator, 2026-11-05): SF Symbols has none, so the tray with a slash drawn over it.
                TraySlash(size: size)
                    .transition(.opacity.combined(with: .scale(scale: 0.6, anchor: .bottom)).animation(.easeOut(duration: 0.5)))
            case .offline:
                symbol("circle.slash", effect: .fade)
            }
        }
        .foregroundStyle(tone.ink)
    }

    enum Entrance { case bounce, pulse, fade }

    private func symbol(_ name: String, effect: Entrance) -> some View {
        SymbolEntrance(name: name, effect: effect, size: size)
    }
}

/// A symbol with its entrance: it plays once when the symbol appears (the dots bounce three times, the speaker's waves
/// pulse, the pause and the slash fade and grow in). A `symbolEffect(value:)` on a symbol that exists only in its phase
/// never sees the value change, so the entrance starts from the symbol's own appearance.
@available(iOS 16.1, *)
struct SymbolEntrance: View {
    var name: String
    var effect: StateGlyph.Entrance
    var size: CGFloat
    @State private var played = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.isLuminanceReduced) private var reduced

    var body: some View {
        let image = Image(systemName: name).font(.system(size: size * 0.8, weight: .bold))
            .frame(width: size, height: size)
        if reduceMotion || reduced {
            image
        } else if #available(iOS 17.0, *) {
            switch effect {
            case .bounce:
                image.symbolEffect(.bounce.byLayer, options: .repeat(3), value: played).onAppear { played = true }
            case .pulse:
                image.symbolEffect(.pulse.byLayer, options: .repeat(1), value: played).onAppear { played = true }
            case .fade:
                // A transition, not a state: a snapshot that never runs `onAppear` still draws the glyph.
                image.transition(.opacity.combined(with: .scale(scale: 0.6, anchor: .bottom)).animation(.easeOut(duration: 0.5)))
            }
        } else {
            image
        }
    }
}

/// Your voice's bars in the compact island (five; a stand-in for the app's): 2.4 wide at x 2.5 / 6.6 / 10.7 / 14.8 / 18.9 of
/// a 24 box, 7 / 12 / 16 / 11 / 7 tall, centred; still, since a Live Activity cannot follow the voice. They grow from
/// their bottom when they appear; always at full tone.
@available(iOS 16.1, *)
struct GlyphBars: View {
    /// Transcribing: the bars fade to 35 % and back, twice, as they appear. A Live Activity's animations end within about
    /// 2 s, so this is the slow blink the operator asked for, as long as the system lets it last.
    var breathe = false
    @State private var appeared = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.isLuminanceReduced) private var reduced
    static let heights: [CGFloat] = [7, 12, 16, 11, 7]

    @ViewBuilder var body: some View {
        if #available(iOS 17.0, *), breathe, !reduceMotion, !reduced {
            bars
                .keyframeAnimator(initialValue: 1.0, trigger: appeared) { content, opacity in
                    content.opacity(opacity)
                } keyframes: { _ in
                    CubicKeyframe(0.35, duration: 0.5)
                    CubicKeyframe(1, duration: 0.5)
                    CubicKeyframe(0.35, duration: 0.5)
                    CubicKeyframe(1, duration: 0.5)
                }
                .onAppear { appeared = true }
        } else {
            bars
        }
    }

    private var bars: some View {
        GeometryReader { box in
            let scale = box.size.width / 24
            ZStack(alignment: .topLeading) {
                ForEach(0..<Self.heights.count, id: \.self) { index in
                    let height = Self.heights[index]
                    Capsule()
                        .frame(width: 2.4 * scale, height: height * scale)
                        .offset(x: (2.5 + 4.1 * CGFloat(index)) * scale, y: (12 - height / 2) * scale)
                        // The bar's own bottom, in the frame it has before the offset moves it.
                        .transition(reduceMotion || reduced ? .identity : .modifier(
                            active: Stretch(y: 0.25, anchor: UnitPoint(x: 0.5, y: (12 + height / 2) / height)),
                            identity: Stretch(y: 1, anchor: .center))
                            .animation(.spring(response: 0.5, dampingFraction: 0.6).delay(0.08 * Double(index))))
                }
            }
        }
    }
}

/// A vertical-only stretch: the bars rise, their width stays.
@available(iOS 16.1, *)
struct Stretch: ViewModifier {
    var y: CGFloat
    var anchor: UnitPoint

    func body(content: Content) -> some View {
        content.scaleEffect(x: 1, y: y, anchor: anchor)
    }
}

/// Can't receive: SF Symbols' `tray` with a slash from top left to bottom right, cut out of the tray with a gap on each
/// side (the gallery's `traySlash`).
@available(iOS 16.1, *)
struct TraySlash: View {
    var size: CGFloat

    var body: some View {
        let slash = Path { path in
            path.move(to: CGPoint(x: size * 0.15, y: size * 0.1))
            path.addLine(to: CGPoint(x: size * 0.85, y: size * 0.82))
        }
        ZStack {
            Image(systemName: "tray")
                .font(.system(size: size * 0.8, weight: .bold))
                .frame(width: size, height: size)
                .mask {
                    ZStack {
                        Rectangle()
                        slash.stroke(style: StrokeStyle(lineWidth: size * 0.22, lineCap: .round)).blendMode(.destinationOut)
                    }
                    .compositingGroup()
                }
            slash.stroke(style: StrokeStyle(lineWidth: size * 0.09, lineCap: .round))
        }
        .frame(width: size, height: size)
    }
}

/// A round button's face: `glyph` on a circle of `size`.
@available(iOS 16.1, *)
struct RoundButtonFace: View {
    var symbol: String
    var size: CGFloat
    var glyph: CGFloat
    var fill: Color
    var ink: Color
    @Environment(\.isLuminanceReduced) private var reduced

    /// The skip button draws the app's own glyph (`SkipGlyph.symbol`); the others an SF Symbol.
    @ViewBuilder private var face: some View {
        if symbol == SkipGlyph.symbol {
            SkipGlyph(size: glyph)
        } else {
            Image(systemName: symbol)
                .font(.system(size: glyph * 0.85, weight: .semibold))
        }
    }

    var body: some View {
        face
            .foregroundStyle(reduced && fill == Ink.hangUp ? Ink.hangUp : ink)
            .frame(width: size, height: size)
            .background(Circle().fill(reduced ? Color.clear : fill))
    }
}

// MARK: - Reading a still

@available(iOS 16.1, *)
extension CallStills {
    private final class Once: @unchecked Sendable {
        private let lock = NSLock()
        private var seen = Set<String>()
        func first(_ key: String) -> Bool {
            lock.lock()
            defer { lock.unlock() }
            return seen.insert(key).inserted
        }
    }

    private static let once = Once()
    private static let log = CallLog.stills

    /// Why the still `file` cannot be drawn, or nil when it can: no App Group container (the widget extension is signed
    /// without the group), no such file (the app wrote it elsewhere, or not at all), or a file that does not decode.
    static func problem(_ file: String) -> String? {
        guard directory != nil else { return "no App Group container (\(appGroup))" }
        guard let url = url(file) else { return "not a file name: \(file)" }
        guard FileManager.default.fileExists(atPath: url.path) else { return "no file \(file) in the App Group container" }
        return UIImage(contentsOfFile: url.path) == nil ? "\(file) does not decode as an image" : nil
    }

    /// The stills an activity's state names (the creature, then the stack's faces), each by its file and whether this
    /// process can draw it (`problem`), or `none`: the app says it in the unified log as the state starts naming others,
    /// beside what the extension says as it draws them.
    public static func naming(_ creatures: [CallState.Creature?]) -> String {
        creatures.map { (creature: CallState.Creature?) -> String in
            guard let file = creature?.file, !file.isEmpty else { return "none" }
            return "\(file) (\(problem(file) ?? "there"))"
        }.joined(separator: ", ")
    }

    /// A slot that has no still to draw (the placeholder is drawn instead): said once, by cause, in the unified log, so that a
    /// call whose creature never shows says whether the app named none (nothing kept, nothing in the report) or could not read
    /// the one it named (`image` says why).
    static func missing(_ creature: CallState.Creature?) {
        guard creature?.file.isEmpty != false else { return }
        if once.first("not named") { log.error("no still named in the call's state: the app has not kept one for it") }
    }

    private static let scaled = NSCache<NSString, UIImage>()

    /// `image` redrawn at `pixels` (1x scale), alpha kept. `UIImage.preparingThumbnail(of:)` returned an opaque bitmap for a
    /// PNG with transparency (the corners came out black: in the stack each face covered the next), so the copy is drawn
    /// into a renderer whose format is explicitly not opaque.
    static func scaledKeepingAlpha(_ image: UIImage, to pixels: CGSize) -> UIImage {
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        format.opaque = false
        return UIGraphicsImageRenderer(size: pixels, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: pixels))
        }
    }

    private static let outlines = NSCache<NSString, UIImage>()

    /// The still `file` as `Still` draws it in a `slot` enlarged by `k` (`image(_:side:)` at `slot * k`), its opaque part
    /// grown by `grow` points of the slot on every side, in black: what a face cuts out of the face beneath it (`OthersStack`).
    /// `size` is the outline's size in the slot's points, centred where the fitted still is, and `anchor` the point of it that
    /// `Still` enlarges the still from (0.5, 0.96 of the fitted still); nil when the still is not there.
    /// Drawn here, in CoreGraphics, rather than by a blur or a filter the system might draw otherwise: the creature's solid
    /// part (`solid`) stamped around circles of up to `grow`. A faint shadow under a creature is not part of it, so it
    /// cuts nothing.
    static func outline(_ file: String, slot: CGFloat, k: CGFloat, grow: CGFloat) -> (image: UIImage, size: CGSize, anchor: UnitPoint)? {
        guard let still = image(file, side: slot * k), let solid = solid(still) else { return nil }
        let width = still.size.width * still.scale, height = still.size.height * still.scale
        // The slot's points per pixel of the still, as `scaledToFit` lays it out.
        let point = slot / max(width, height, 1)
        // The ring in the still's pixels, and the whole pixels the canvas grows by to hold it.
        let reach = grow / point
        let pad = reach.rounded(.up)
        let pixels = CGSize(width: width + 2 * pad, height: height + 2 * pad)
        let size = CGSize(width: pixels.width * point, height: pixels.height * point)
        let anchor = UnitPoint(x: 0.5, y: (pad + 0.96 * height) / pixels.height)
        let key = "\(directory?.path ?? "-")/\(file)@\(Int(width))x\(Int(height))+\(Int(pad))" as NSString
        if let cached = outlines.object(forKey: key) { return (cached, size, anchor) }
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        format.opaque = false
        let drawn = UIGraphicsImageRenderer(size: pixels, format: format).image { _ in
            let box = CGRect(x: pad, y: pad, width: width, height: height)
            solid.draw(in: box)
            // Rings at a third, two thirds and the whole of the reach, 32 stamps each: every point within `reach` of the
            // creature is covered, the edge within a fraction of a pixel of round.
            for step in 1...3 where reach > 0 {
                let radius = reach * CGFloat(step) / 3
                for index in 0..<32 {
                    let angle = CGFloat(index) / 32 * 2 * .pi
                    solid.draw(in: box.offsetBy(dx: radius * cos(angle), dy: radius * sin(angle)))
                }
            }
        }
        outlines.setObject(drawn, forKey: key)
        return (drawn, size, anchor)
    }

    /// `image`'s pixels at least half opaque, in opaque black, the rest transparent: the creature without its soft shadow and
    /// edges, at the still's own pixels.
    static func solid(_ image: UIImage) -> UIImage? {
        guard let cg = image.cgImage else { return nil }
        let width = cg.width, height = cg.height
        var bytes = [UInt8](repeating: 0, count: width * height * 4)
        let made: CGImage? = bytes.withUnsafeMutableBytes { buffer in
            guard let context = CGContext(
                data: buffer.baseAddress, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
                space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
            else { return nil }
            context.draw(cg, in: CGRect(x: 0, y: 0, width: width, height: height))
            for index in stride(from: 0, to: buffer.count, by: 4) {
                let solid: UInt8 = buffer[index + 3] >= 128 ? 255 : 0
                buffer[index] = 0
                buffer[index + 1] = 0
                buffer[index + 2] = 0
                buffer[index + 3] = solid
            }
            return context.makeImage()
        }
        return made.map { UIImage(cgImage: $0, scale: 1, orientation: .up) }
    }

    /// The still `file` as an image, scaled down to `side` points at the screen's 3×; nil draws the placeholder. The
    /// compact island drew no creature on the phone while the expanded island and the Lock Screen drew the same 240–480 px
    /// file: the system leaves out an image much larger than its slot there. A copy at the slot's size also keeps the
    /// extension's memory down. What stops it is logged once for each cause, so that a card with no creature says why in
    /// the device's log (the widget extension has no other voice).
    static func image(_ file: String, side: CGFloat = 64) -> UIImage? {
        let pixels = max(1, (side * 3).rounded(.up))
        let key = "\(directory?.path ?? "-")/\(file)@\(Int(pixels))" as NSString
        if let image = scaled.object(forKey: key) { return image }
        if let url = url(file), let image = UIImage(contentsOfFile: url.path) {
            if once.first("drawn \(file)") { log.notice("still drawn: \(file, privacy: .public)") }
            let width = image.size.width * image.scale, height = image.size.height * image.scale
            let fit = min(1, pixels / max(width, height, 1))
            let small = fit < 1
                ? scaledKeepingAlpha(image, to: CGSize(width: (width * fit).rounded(), height: (height * fit).rounded()))
                : image
            scaled.setObject(small, forKey: key)
            return small
        }
        if let reason = problem(file) {
            if once.first(reason) { log.error("still not drawn: \(reason, privacy: .public)") }
        }
        return nil
    }
}

// MARK: - Skip's glyph

/// The skip icon of the app's call controls (sidevoice-web, `SkipIcon`: Lucide `skip-forward`, drawn on a 24 grid with a
/// 1.8 stroke, round caps and joins): an outlined triangle and a bar. SF Symbols has no such glyph (`forward.fill` is two
/// filled triangles), so the Live Activity draws the same lines.
struct SkipGlyph: View {
    /// What `RoundButtonFace` is given as its `symbol` to draw this glyph.
    static let symbol = "sidevoice.skip"
    /// The box, in points (the app's button draws it at 17 in the card, on a 40-pt button).
    var size: CGFloat

    var body: some View {
        SkipShape()
            .stroke(style: StrokeStyle(lineWidth: 1.8 / 24 * size, lineCap: .round, lineJoin: .round))
            .frame(width: size, height: size)
    }
}

/// Lucide `skip-forward`: `polygon 5 4 15 12 5 20` and `line 19 5 -> 19 19`, in a 24 × 24 box.
struct SkipShape: Shape {
    func path(in rect: CGRect) -> Path {
        let unit = rect.width / 24
        func point(_ x: CGFloat, _ y: CGFloat) -> CGPoint {
            CGPoint(x: rect.minX + x * unit, y: rect.minY + y * unit)
        }
        var path = Path()
        path.move(to: point(5, 4))
        path.addLine(to: point(15, 12))
        path.addLine(to: point(5, 20))
        path.closeSubpath()
        path.move(to: point(19, 5))
        path.addLine(to: point(19, 19))
        return path
    }
}
