import SwiftUI
import UIKit

// The pieces the presentations share: the creature in its slots, the state's glyphs, your voice's bars, the stack.

/// A creature's still from the stills folder; none (not kept, or no App Group) draws nothing.
@available(iOS 16.1, *)
struct Still: View {
    var creature: CallState.Creature?
    var grey = false

    var body: some View {
        if let file = creature?.file, let url = CallStills.url(file), let image = UIImage(contentsOfFile: url.path) {
            let still = Image(uiImage: image).resizable().interpolation(.high).scaledToFit()
            if grey {
                // Offline: the gallery's `grayscale(1) brightness(.85)` at 70 %.
                still.grayscale(1).colorMultiply(Color(white: 0.85)).opacity(0.7)
            } else {
                still
            }
        }
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
    /// The colour around the badge: the card's, or the island's black.
    var badgeRing: Color
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
            Still(creature: state.creature, grey: state.phase == .offline)
                .frame(width: inner, height: inner)
                .scaleEffect(state.creature?.k ?? 1, anchor: UnitPoint(x: 0.5, y: 0.96))
        }
        .frame(width: size, height: size)
        .overlay(alignment: .bottomTrailing) {
            if state.micOff {
                MicBadge(size: 22, glyph: 12, ring: badgeRing, ringWidth: 2).offset(x: 4, y: 4)
            }
        }
    }
}

/// The red mic-off badge.
@available(iOS 16.1, *)
struct MicBadge: View {
    var size: CGFloat
    var glyph: CGFloat
    var ring: Color
    var ringWidth: CGFloat

    var body: some View {
        Circle().fill(Ink.hangUp)
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
        Still(creature: state.creature, grey: state.phase == .offline)
            .frame(width: size, height: size)
            .clipShape(Circle())
    }
}

/// The other conversations: up to three faces of 26 overlapping by 8, enlarged by their `k` (overflow), then the count.
/// One link to the app's conversation list (D-LA-SIMPLE).
@available(iOS 16.1, *)
struct OthersStack: View {
    var state: CallState
    var palette: Palette

    var body: some View {
        Link(destination: CallLinks.conversations) {
            HStack(spacing: 0) {
                HStack(spacing: -8) {
                    ForEach(Array(state.others.prefix(3).enumerated()), id: \.offset) { _, other in
                        Still(creature: other)
                            .frame(width: 26, height: 26)
                            .scaleEffect(other?.k ?? 1, anchor: UnitPoint(x: 0.5, y: 0.96))
                    }
                }
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
        let time = Text("· ") + Text(since, style: .timer)
        return words + Text("  ") + time.font(DMSans.font(size, 600)).foregroundColor(tone.ink.opacity(0.85))
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
            case .listening, .transcribing, .said:
                GlyphBars(phase: phase).frame(width: size, height: size)
            case .working:
                symbol("ellipsis", effect: .bounce)
            case .speaking:
                symbol("speaker.wave.2.fill", effect: .pulse)
            case .holding:
                symbol("pause.circle.fill", effect: .fade)
            case .offline:
                symbol("circle.slash", effect: .fade)
            }
        }
        .foregroundStyle(tone.ink)
    }

    enum Entrance { case bounce, pulse, fade }

    @ViewBuilder
    private func symbol(_ name: String, effect: Entrance) -> some View {
        let image = Image(systemName: name).font(.system(size: size * 0.8, weight: .bold))
            .frame(width: size, height: size)
        if #available(iOS 17.0, *) {
            switch effect {
            case .bounce: image.symbolEffect(.bounce.byLayer, value: phase)
            case .pulse: image.symbolEffect(.pulse.byLayer, options: .nonRepeating, value: phase)
            case .fade: image.transition(.opacity.animation(.easeIn(duration: 0.6)))
            }
        } else {
            image
        }
    }
}

/// Your voice's bars in the compact island (five; a stand-in for the app's): still, since a Live Activity cannot follow
/// the voice; 45 % while transcribing. They rise when listening starts.
@available(iOS 16.1, *)
struct GlyphBars: View {
    var phase: CallState.Phase
    static let bars: [(y: CGFloat, h: CGFloat)] = [(9.5, 5), (6.5, 11), (4, 16), (7.5, 9), (10, 4)]

    var body: some View {
        GeometryReader { box in
            let scale = box.size.width / 24
            ZStack(alignment: .topLeading) {
                ForEach(0..<Self.bars.count, id: \.self) { index in
                    let bar = Self.bars[index]
                    Capsule()
                        .frame(width: 2.2 * scale, height: bar.h * scale)
                        .offset(x: (2.5 + 4.1 * CGFloat(index)) * scale, y: bar.y * scale)
                        .transition(.scale(scale: 0.15, anchor: .center)
                            .animation(.spring(response: 0.5, dampingFraction: 0.55).delay(0.08 * Double(index))))
                }
            }
        }
        .opacity(phase == .listening ? 1 : 0.45)
        .animation(.easeInOut(duration: 0.6), value: phase)
    }
}

/// Your voice's bars in the expanded island's capsule: 28 bars, 3 wide every 6, centred (the gallery's `BAR_HEIGHTS`),
/// lila; 40 % while transcribing. Listening: they rise, left to right, 0.035 s apart. Transcribing: a wave of light runs
/// through them, then 40 %.
@available(iOS 16.1, *)
struct VoiceBars: View {
    var dim: Bool
    static let heights: [CGFloat] = [5, 9, 14, 8, 18, 12, 6, 15, 20, 11, 7, 16, 10, 5, 13, 19, 9, 6, 14, 8, 17, 11, 6, 12, 7, 15, 9, 5]

    var body: some View {
        HStack(spacing: 3) {
            ForEach(0..<Self.heights.count, id: \.self) { index in
                Capsule()
                    .fill(Color(hex: 0xc9b6ec))
                    .frame(width: 3, height: Self.heights[index])
                    .opacity(dim ? 0.4 : 1)
                    .animation(.easeInOut(duration: 0.6).delay(0.035 * Double(index)), value: dim)
                    .transition(.scale(scale: 0.15, anchor: .center)
                        .animation(.spring(response: 0.5, dampingFraction: 0.55).delay(0.035 * Double(index))))
            }
        }
        .frame(height: 24)
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

    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: glyph * 0.85, weight: .semibold))
            .foregroundStyle(reduced && fill == Ink.hangUp ? Ink.hangUp : ink)
            .frame(width: size, height: size)
            .background(Circle().fill(reduced ? Color.clear : fill))
    }
}
