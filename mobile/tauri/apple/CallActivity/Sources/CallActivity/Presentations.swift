import SwiftUI

// The presentations (HANDOVER §4–§5, sizes and offsets from spec/GEOMETRY.md): the Lock Screen card and the Dynamic
// Island's expanded (variant B: everything below the camera), compact and minimal views. Never text or transcripts on the
// Lock Screen (D-LA-NO-TEXT-LOCKED); texts end in an ellipsis everywhere.

/// The Lock Screen card: the tile, the conversation, its model and machine, the state; the audio switch while headphones
/// are connected; mute, skip, the other conversations and hang up. 14 pt in from the card's edge, about 152 pt tall.
@available(iOS 16.1, *)
public struct LockCard: View {
    public var state: CallState
    @Environment(\.colorScheme) private var scheme
    @Environment(\.isLuminanceReduced) private var reduced

    public init(state: CallState) {
        self.state = state
    }

    public var body: some View {
        let palette = Palette(dark: scheme == .dark)
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 12) {
                CreatureTile(state: state, size: 64, palette: palette)
                VStack(alignment: .leading, spacing: 0) {
                    Text(state.title)
                        .font(DMSans.font(17, 700)).tracking(-0.17)
                        .foregroundStyle(palette.text)
                        .line(24.98)
                        .padding(.top, 1)
                    Text(subtitle)
                        .font(DMSans.font(13, 400))
                        .foregroundStyle(palette.text2)
                        .line(19.11)
                    StateLine(state: state, dark: palette.dark, size: 15, lineHeight: 22.05)
                        .padding(.top, 5)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                if state.route != nil {
                    ActionButton(action: .route) {
                        RoundButtonFace(symbol: routeSymbol, size: 30, glyph: 16, fill: palette.button, ink: palette.text2)
                    }
                    .accessibilityLabel(state.routeLabel)
                    .padding(.top, -2)
                }
            }
            .frame(height: 72.14, alignment: .top)
            HStack(spacing: 8) {
                // A button never ends in an ellipsis: its label whole, or its glyph alone (same colours, same name).
                ViewThatFits(in: .horizontal) {
                    MuteToggle(micOff: state.micOff) { off in
                        MutePill(state: state, micOff: off, palette: palette, reduced: reduced)
                            .accessibilityLabel(off ? state.words.unmute : state.words.mute)
                    }
                    MuteToggle(micOff: state.micOff) { off in
                        RoundButtonFace(
                            symbol: off ? "mic.slash.fill" : "mic.fill", size: 40, glyph: 17,
                            fill: off ? Ink.mutedFill : palette.button,
                            ink: off ? Ink.mutedInk : palette.text)
                            .accessibilityLabel(off ? state.words.unmute : state.words.mute)
                    }
                }
                ActionButton(action: .skip, enabled: state.canSkip) {
                    RoundButtonFace(symbol: "forward.fill", size: 40, glyph: 17, fill: palette.button, ink: palette.text)
                }
                .opacity(state.canSkip ? 1 : Ink.disabled)
                .accessibilityLabel(state.words.skipName)
                Spacer(minLength: 0)
                OthersStack(state: state, palette: palette)
                ActionButton(action: .hangUp) {
                    RoundButtonFace(symbol: "phone.down.fill", size: 40, glyph: 20, fill: Ink.hangUp, ink: .white)
                }
                .accessibilityLabel(state.words.hangUp)
            }
            .frame(height: 40)
        }
        .padding(14)
        .environment(\.locale, state.locale)
    }

    private var subtitle: String {
        [state.model, state.machine].filter { !$0.isEmpty }.joined(separator: " · ")
    }

    private var routeSymbol: String {
        state.route == .speaker ? "speaker.wave.2.fill" : "airpods"
    }
}

/// Mute's pill: "Silenciar" on the neutral fill, or "Activar micrófono" in salmon while the mic is off.
@available(iOS 16.1, *)
struct MutePill: View {
    var state: CallState
    var micOff: Bool
    var palette: Palette
    var reduced: Bool

    var body: some View {
        let ink = micOff ? Ink.mutedInk : palette.text
        let fill = micOff ? Ink.mutedFill : palette.button
        HStack(spacing: 6) {
            Image(systemName: micOff ? "mic.slash.fill" : "mic.fill")
                .font(.system(size: 15, weight: .semibold))
                .frame(width: 17, height: 17)
            Text(micOff ? state.words.unmute : state.words.mute)
                .font(DMSans.font(14, 650))
                .lineLimit(1)
                .fixedSize()
                .frame(height: 20.58)
        }
        .foregroundStyle(reduced && micOff ? Ink.mutedFill : ink)
        .padding(.leading, 12)
        .padding(.trailing, 15)
        .frame(height: 40)
        .background(Capsule().fill(reduced ? Color.clear : fill))
    }
}

/// The expanded island's content below the camera (variant B, D-LA-EXPANDED-B): the header, then the controls, your
/// bars, your words or the reply. 344 pt wide on a 17 Pro; the island adds its edges.
@available(iOS 16.1, *)
public struct ExpandedBody: View {
    public var state: CallState

    public init(state: CallState) {
        self.state = state
    }

    public var body: some View {
        Group { content }.environment(\.locale, state.locale)
    }

    @ViewBuilder
    private var content: some View {
        switch state.phase {
        case .speaking:
            Reply(state: state)
                .overlay(alignment: .topLeading) {
                    RoundFace(state: state, size: 30).offset(x: 10, y: -32)
                }
        case .listening, .transcribing:
            VStack(spacing: 12) {
                VoiceHeader(state: state)
                VoiceCapsule(state: state)
            }
        case .said:
            VStack(alignment: .leading, spacing: 12) {
                VoiceHeader(state: state)
                Lines(
                    text: state.text ?? "", size: 15, weight: 550, lineHeight: 19, lines: 2,
                    ink: Palette.island.text, rest: Palette.island.text)
            }
        case .idle, .working, .holding, .offline:
            VStack(spacing: 8) {
                ClosedHeader(state: state)
                HStack(spacing: 8) {
                    MuteToggle(micOff: state.micOff) { off in
                        RoundButtonFace(
                            symbol: off ? "mic.slash.fill" : "mic.fill", size: 36, glyph: 18,
                            fill: off ? Ink.mutedFill : Palette.island.button,
                            ink: off ? Ink.mutedInk : Palette.island.text)
                            .accessibilityLabel(off ? state.words.unmute : state.words.mute)
                    }
                    if state.route != nil {
                        ActionButton(action: .route) {
                            RoundButtonFace(
                                symbol: state.route == .speaker ? "speaker.wave.2.fill" : "airpods", size: 30, glyph: 15,
                                fill: Palette.island.button, ink: Palette.island.text)
                        }
                        .accessibilityLabel(state.routeLabel)
                    }
                    Spacer(minLength: 0)
                    ActionButton(action: .hangUp) {
                        RoundButtonFace(symbol: "phone.down.fill", size: 36, glyph: 20, fill: Ink.hangUp, ink: .white)
                    }
                    .accessibilityLabel(state.words.hangUp)
                }
                .frame(height: 36)
            }
        }
    }
}

/// Idle, working, can't receive, offline: tile 56, the conversation, model · machine, the state, the stack.
@available(iOS 16.1, *)
struct ClosedHeader: View {
    var state: CallState

    var body: some View {
        let palette = Palette.island
        HStack(alignment: .center, spacing: 12) {
            CreatureTile(state: state, size: 56, palette: palette)
                .frame(maxHeight: .infinity, alignment: .top)
                .padding(.top, 1.2)
            VStack(alignment: .leading, spacing: 0) {
                Text(state.title)
                    .font(DMSans.font(16, 700)).tracking(-0.16)
                    .foregroundStyle(palette.text)
                    .line(23.52)
                Text([state.model, state.machine].filter { !$0.isEmpty }.joined(separator: " · "))
                    .font(DMSans.font(12.5, 400))
                    .foregroundStyle(palette.text2)
                    .line(18.38)
                StateLine(state: state, dark: true, size: 14, lineHeight: 20.58)
                    .padding(.top, 1.99)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            OthersStack(state: state, palette: palette)
        }
        .frame(height: 64.47)
    }
}

/// Listening, transcribing, your words: tile 44, the conversation and the state ("Escuchándote" / "Transcribiendo…"), the
/// stack.
@available(iOS 16.1, *)
struct VoiceHeader: View {
    var state: CallState

    var body: some View {
        let palette = Palette.island
        HStack(alignment: .center, spacing: 12) {
            CreatureTile(state: state, size: 44, palette: palette)
                .frame(maxHeight: .infinity, alignment: .top)
            VStack(alignment: .leading, spacing: 1.99) {
                Text(state.title)
                    .font(DMSans.font(16, 700)).tracking(-0.16)
                    .foregroundStyle(palette.text)
                    .line(23.52)
                StateLine(state: state, dark: true, size: 14, lineHeight: 20.58)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            OthersStack(state: state, palette: palette)
        }
        .frame(height: 50.05)
    }
}

/// Your bars in a lila capsule, 36 tall, with Cancelar envío as an × at its right end.
@available(iOS 16.1, *)
struct VoiceCapsule: View {
    var state: CallState

    var body: some View {
        HStack(spacing: 8) {
            VoiceBars(dim: state.phase == .transcribing)
                .frame(maxWidth: .infinity)
            ActionButton(action: .cancelSend, enabled: state.canCancelSend) {
                Image(systemName: "xmark")
                    .font(.system(size: 12, weight: .bold))
                    .foregroundStyle(Palette.island.text)
                    .frame(width: 28, height: 28)
                    .background(Circle().fill(Color.white.opacity(0.14)))
            }
            .opacity(state.canCancelSend ? 1 : Ink.disabled)
            .accessibilityLabel(state.words.cancelSend)
        }
        .padding(.leading, 14)
        .padding(.trailing, 4)
        .frame(height: 36)
        .background(Capsule().fill(Color(hex: 0xc9b6ec, alpha: 0.14)))
    }
}

/// Speaking: the creature (30 pt, beside the camera, 10 pt in from B's edge), the reply's page, 15 on 18, four lines, the
/// heard words lit and the rest at 38 %; Saltar 4 pt below it, right. All of it in `.bottom` (variant B): the creature is
/// drawn 32 pt above the region's top, where the camera row is.
@available(iOS 16.1, *)
struct Reply: View {
    var state: CallState

    var body: some View {
        VStack(alignment: .trailing, spacing: 4) {
            Lines(
                text: state.text ?? "", size: 15, weight: 550, tracking: -0.075, lineHeight: 18, lines: 4,
                lit: state.lit, ink: Palette.island.text, rest: Palette.island.text.opacity(0.38))
            if state.canSkip {
                // Never an ellipsis on a button: the label whole, or the glyph alone.
                ViewThatFits(in: .horizontal) {
                    ActionButton(action: .skip) {
                        HStack(spacing: 4) {
                            Image(systemName: "forward.fill")
                                .resizable().scaledToFit()
                                .frame(width: 14, height: 14)
                            Text(state.words.skip)
                                .font(DMSans.font(12, 650))
                                .lineLimit(1)
                                .fixedSize()
                                .frame(height: 17.64)
                        }
                        .foregroundStyle(Palette.island.text)
                        .padding(.leading, 8)
                        .padding(.trailing, 10)
                        .frame(height: 26)
                        .background(Capsule().fill(Palette.island.button))
                    }
                    ActionButton(action: .skip) {
                        RoundButtonFace(
                            symbol: "forward.fill", size: 26, glyph: 14, fill: Palette.island.button,
                            ink: Palette.island.text)
                    }
                }
                .accessibilityLabel(state.words.skipName)
            } else {
                Color.clear.frame(height: 26)
            }
        }
    }

}

/// Compact, leading: the face, a 26-pt circle with the whole creature.
@available(iOS 16.1, *)
public struct CompactLeading: View {
    public var state: CallState

    public init(state: CallState) {
        self.state = state
    }

    public var body: some View {
        RoundFace(state: state, size: 26)
    }
}

/// Compact, trailing: the state's glyph (none while idle), after a red mic-off glyph while the mic is off. No words: they
/// do not fit.
@available(iOS 16.1, *)
public struct CompactTrailing: View {
    public var state: CallState
    @Environment(\.isLuminanceReduced) private var reduced

    public init(state: CallState) {
        self.state = state
    }

    public var body: some View {
        if state.phase == .idle && !state.micOff {
            EmptyView()
        } else {
            HStack(spacing: 6) {
                if state.micOff {
                    Image(systemName: "mic.slash.fill")
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundStyle(Ink.micOffGlyph)
                        .frame(width: 18, height: 18)
                }
                StateGlyph(phase: state.phase)
            }
            .modifier(StillOnAlwaysOn(reduced: reduced))
            .environment(\.locale, state.locale)
        }
    }
}

/// Minimal: the face (24, whole) in a 30-pt ring of 2 pt in the tone; the mic off as a 12-pt badge inside the circle.
@available(iOS 16.1, *)
public struct MinimalFace: View {
    public var state: CallState

    public init(state: CallState) {
        self.state = state
    }

    public var body: some View {
        let tone = Tone.of(state.phase, dark: true)
        ZStack {
            Circle().strokeBorder(tone.fill, lineWidth: 2).frame(width: 34, height: 34)
            RoundFace(state: state, size: 24)
        }
        .frame(width: 34, height: 34)
        .overlay(alignment: .bottomTrailing) {
            if state.micOff {
                // Inside the island's circle (it clips anything outside): centre at (27, 27) of the 36.67 box.
                MicBadge(size: 12, glyph: 8, ring: .black, ringWidth: 1.5).offset(x: -2.33, y: -2.33)
            }
        }
    }
}

/// Nothing moves on Always-On (HANDOVER §5).
@available(iOS 16.1, *)
struct StillOnAlwaysOn: ViewModifier {
    var reduced: Bool

    func body(content: Content) -> some View {
        if #available(iOS 17.0, *) {
            content.symbolEffectsRemoved(reduced).transaction { if reduced { $0.disablesAnimations = true } }
        } else {
            content.transaction { if reduced { $0.disablesAnimations = true } }
        }
    }
}
