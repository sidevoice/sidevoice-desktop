import SwiftUI
import XCTest

@testable import CallActivity

/// The stack of other conversations: each face has a ring-wide gap cut out of it around the face over it, following that
/// creature's outline. A circle around the slot cut a disc out of the face beneath that the face over it did not cover,
/// which read as a black disc on the Lock Screen's dark card and passed unseen in the dark renders. Here the stack is drawn
/// alone, on a light Lock Screen panel, a mid-tone wallpaper and the dark card, saved for a person to look at, and its
/// pixels checked against each face drawn on its own: within the ring, the background; beyond it, the face beneath, whole.
@available(iOS 17.0, *)
@MainActor
final class StackRingTests: XCTestCase {
    /// Room around the stack for the accessories that stand out of their slots.
    static let margin: CGFloat = 16
    static let scale: CGFloat = 3
    /// The light Lock Screen panel of the renders (`RenderTests.lock`, opaque).
    static let light = Color(hex: 0xf4f1f8)

    /// Three others, every fixture, each with its big accessory (as the gallery's call, and the simulator's `others-3`).
    static func sample() -> CallState {
        var state = RenderTests.sample(.working)
        state.others = [
            .init(file: "bunny-headphones.png", k: 1.35), .init(file: "wizard-hat.png", k: 1.4), .init(file: "straw-hat.png", k: 1.3),
        ]
        state.conversationCount = 4
        return state
    }

    func testTheStackOnLightMidToneAndDarkBackgrounds() throws {
        let state = Self.sample()
        let backgrounds: [(String, AnyView, Bool)] = [
            ("light", AnyView(Self.light), false),
            // A wallpaper's mid-tones under the card's material, as on a phone.
            ("wallpaper", AnyView(LinearGradient(
                colors: [Color(hex: 0x6fb3a8), Color(hex: 0x8a94a6), Color(hex: 0x3b5fc4)],
                startPoint: .topLeading, endPoint: .bottomTrailing)), false),
            // The card as the operator's phone drew it (near black).
            ("dark", AnyView(Color(hex: 0x080a0c)), true),
        ]
        for (name, background, dark) in backgrounds {
            let view = OthersStack(state: state, palette: Palette(dark: dark))
                .padding(Self.margin)
                .background(background)
            try save(view, "stack-on-\(name)", scale: Self.scale)
            // Big enough to see the ring without zooming.
            try save(view, "stack-on-\(name)-8x", scale: 8)
        }
    }

    func testTheRingIsAGapAroundTheFaceOverItAndNothingMore() throws {
        let state = Self.sample()
        let others = Array(state.others.prefix(3))
        let stack = try Pixels(OthersStack(state: state, palette: Palette(dark: false)).padding(Self.margin).background(Self.light))
        let layers = try others.indices.map { try Pixels(layer(others, $0).padding(Self.margin)) }
        let background = try XCTUnwrap(stack.rgba(0, 0))
        let ring = Int((OthersStack.ring * Self.scale).rounded())

        for below in 0..<(layers.count - 1) {
            let over = layers[(below + 1)...]
            // Where some face over this one is opaque: its outline, which the ring is measured from.
            let outline = Set(over.flatMap { $0.points(alpha: 128) })
            var gap = 0, gapWrong = 0, kept = 0, keptWrong = 0
            for y in 0..<stack.height {
                for x in 0..<stack.width {
                    guard layers[below].alpha(x, y) >= 250, over.allSatisfy({ $0.alpha(x, y) <= 5 }) else { continue }
                    let distance = Self.distance(x, y, to: outline, within: ring + 3)
                    let drawn = stack.rgba(x, y)
                    if distance <= ring - 2 {
                        // Within the ring: the background, not black, not the face beneath.
                        gap += 1
                        if !Self.close(drawn, background) { gapWrong += 1 }
                    } else if distance > ring + 2 {
                        // Beyond it: the face beneath, untouched.
                        kept += 1
                        if !Self.close(drawn, layers[below].rgba(x, y)) { keptWrong += 1 }
                    }
                }
            }
            XCTAssertGreaterThan(gap, 20, "face \(below): no ring to check")
            XCTAssertGreaterThan(kept, 200, "face \(below): nothing of it beyond the ring")
            XCTAssertLessThanOrEqual(gapWrong, gap / 50, "face \(below): \(gapWrong) of \(gap) pixels in the ring are not the background")
            XCTAssertLessThanOrEqual(keptWrong, kept / 100, "face \(below): \(keptWrong) of \(kept) pixels beyond the ring are cut")
        }
    }

    /// The `index`th face alone, where `OthersStack` lays it out, with nothing cut out of it.
    func layer(_ others: [CallState.Creature?], _ index: Int) -> some View {
        ZStack(alignment: .leading) {
            Still(creature: others[index], ground: .disc, overflow: true, side: OthersStack.face)
                .frame(width: OthersStack.face, height: OthersStack.face)
                .offset(x: CGFloat(index) * OthersStack.step)
        }
        .frame(width: OthersStack.face + CGFloat(others.count - 1) * OthersStack.step, height: OthersStack.face, alignment: .leading)
    }

    static func distance(_ x: Int, _ y: Int, to points: Set<Pixels.Point>, within limit: Int) -> Double {
        var best = Double.infinity
        for dy in -limit...limit {
            for dx in -limit...limit where points.contains(Pixels.Point(x: x + dx, y: y + dy)) {
                best = min(best, (Double(dx * dx + dy * dy)).squareRoot())
            }
        }
        return best
    }

    static func close(_ a: [UInt8]?, _ b: [UInt8]?, tolerance: Int = 10) -> Bool {
        guard let a, let b else { return false }
        return zip(a, b).allSatisfy { abs(Int($0) - Int($1)) <= tolerance }
    }

    func save(_ view: some View, _ name: String, scale: CGFloat) throws {
        let renderer = ImageRenderer(content: view.fixedSize())
        renderer.scale = scale
        let png = try XCTUnwrap(renderer.uiImage?.pngData(), name)
        try png.write(to: RenderTests.out.appendingPathComponent("\(name).png"))
    }
}

/// A view drawn at `StackRingTests.scale`, its pixels as premultiplied RGBA, the top-left first.
@available(iOS 17.0, *)
struct Pixels {
    struct Point: Hashable {
        var x: Int
        var y: Int
    }

    let width: Int
    let height: Int
    private let bytes: [UInt8]

    @MainActor
    init(_ view: some View) throws {
        let renderer = ImageRenderer(content: view.fixedSize())
        renderer.scale = StackRingTests.scale
        let image = try XCTUnwrap(renderer.cgImage)
        width = image.width
        height = image.height
        var bytes = [UInt8](repeating: 0, count: width * height * 4)
        let context = try XCTUnwrap(CGContext(
            data: &bytes, width: width, height: height, bitsPerComponent: 8, bytesPerRow: width * 4,
            space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
        context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        self.bytes = bytes
    }

    func rgba(_ x: Int, _ y: Int) -> [UInt8]? {
        guard x >= 0, y >= 0, x < width, y < height else { return nil }
        let index = (y * width + x) * 4
        return Array(bytes[index..<index + 4])
    }

    func alpha(_ x: Int, _ y: Int) -> UInt8 {
        rgba(x, y)?[3] ?? 0
    }

    func points(alpha threshold: UInt8) -> [Point] {
        var points: [Point] = []
        for y in 0..<height {
            for x in 0..<width where alpha(x, y) >= threshold {
                points.append(Point(x: x, y: y))
            }
        }
        return points
    }
}
