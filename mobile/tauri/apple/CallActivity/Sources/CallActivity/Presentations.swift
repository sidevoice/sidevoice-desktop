import SwiftUI

// The presentations (HANDOVER §4–§5, sizes and offsets from spec/GEOMETRY.md): the Lock Screen card and the Dynamic
// Island's expanded (variant B: everything below the camera), compact and minimal views. Never text or transcripts on the
// Lock Screen (D-LA-NO-TEXT-LOCKED); texts end in an ellipsis everywhere.

/// The Lock Screen card: the tile, the conversation, its model and machine, the state with the action it offers (skip while
/// the agent speaks, cancel send during the person's turn: the operator's option B); the audio switch while headphones are
/// connected; mute, the other conversations and hang up, the same row in every state. 14 pt in from the card's edge.
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
                    HStack(alignment: .top, spacing: 12) {
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
                    // The state, and at its end the action it offers; 28 pt in every state, so the card keeps its height.
                    // The state gives way (an ellipsis), never the action. Your words have no state line: nothing of what
                    // the call does under them shows (D-LA-SAID-PRECEDENCE), and they are never on the Lock Screen
                    // (D-LA-NO-TEXT-LOCKED).
                    HStack(spacing: 8) {
                        if state.phase != .said {
                            StateLine(state: state, dark: palette.dark, size: 15, lineHeight: 22.05)
                        }
                        Spacer(minLength: 0)
                        StateAction(state: state, fill: palette.button, ink: palette.text, reduced: reduced)
                    }
                    .frame(height: 28)
                }
            }
            .frame(height: 73.09, alignment: .top)
            HStack(spacing: 8) {
                // A button never ends in an ellipsis: its label whole, or its glyph alone (same colours, same name).
                ViewThatFits(in: .horizontal) {
                    ActionButton(action: .mute) {
                        MutePill(state: state, micOff: state.micOff, palette: palette, reduced: reduced)
                    }
                    .accessibilityLabel(state.micOff ? state.words.unmute : state.words.mute)
                    ActionButton(action: .mute) {
                        RoundButtonFace(
                            symbol: state.micOff ? "mic.slash.fill" : "mic.fill", size: 40, glyph: 17,
                            fill: state.micOff ? Ink.mutedFill : palette.button,
                            ink: state.micOff ? Ink.mutedInk : palette.text)
                    }
                    .accessibilityLabel(state.micOff ? state.words.unmute : state.words.mute)
                }
                Spacer(minLength: 0)
                OthersStack(state: state, palette: palette)
                // Hang up never gives way.
                ActionButton(action: .hangUp) {
                    RoundButtonFace(symbol: "phone.down.fill", size: 40, glyph: 20, fill: Ink.hangUp, ink: .white)
                }
                .accessibilityLabel(state.words.hangUp)
                .fixedSize()
                .layoutPriority(1)
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

/// The action the state offers, as a 28-pt pill (the Lock Screen's state line; Saltar under the island's reply): "Saltar"
/// while the agent speaks, "Cancelar envío" during the person's turn, nothing otherwise. Its label is the page's, whole.
@available(iOS 16.1, *)
struct StateAction: View {
    var state: CallState
    var fill: Color
    var ink: Color
    var reduced = false

    var body: some View {
        if state.phase == .speaking, state.canSkip {
            ActionPill(action: .skip, label: state.words.skip, fill: fill, ink: ink, reduced: reduced) { SkipGlyph(size: 13) }
                .accessibilityLabel(state.words.skipName)
        } else if state.phase == .listening || state.phase == .transcribing, state.canCancelSend {
            ActionPill(action: .cancelSend, label: state.words.cancelSend, fill: fill, ink: ink, reduced: reduced) {
                Image(systemName: "xmark").font(.system(size: 11, weight: .semibold))
            }
            .accessibilityLabel(state.words.cancelSend)
        }
    }
}

/// A 28-pt pill: a 13-pt glyph and its label (13 pt, 650), 9 and 11 in from its ends, never squeezed.
@available(iOS 16.1, *)
struct ActionPill<Glyph: View>: View {
    var action: CallControl
    var label: String
    var fill: Color
    var ink: Color
    var reduced = false
    @ViewBuilder var glyph: () -> Glyph

    var body: some View {
        ActionButton(action: action) {
            HStack(spacing: 5) {
                glyph().frame(width: 13, height: 13)
                Text(label)
                    .font(DMSans.font(13, 650))
                    .lineLimit(1)
                    .fixedSize()
            }
            .foregroundStyle(ink)
            .padding(.leading, 9)
            .padding(.trailing, 11)
            .frame(height: 28)
            .background(Capsule().fill(reduced ? Color.clear : fill))
        }
        .fixedSize()
        .layoutPriority(1)
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

/// The expanded island's content below the camera (variant B, D-LA-EXPANDED-B): the header and the controls; in the voice
/// states (the speaking layout: `ExpandedLeading`, `ExpandedTrailing`) your pill, your words or the reply alone. The island
/// adds its edges.
@available(iOS 16.1, *)
public struct ExpandedBody: View {
    public var state: CallState

    public init(state: CallState) {
        self.state = state
    }

    /// The island's geometry (R2-7, R2-8), made deterministic: the widget zeroes the system's margins at the sides and the
    /// bottom (`contentMargins`, iOS 17), so these paddings are all there is there: 18 at the sides, 14 below. The device
    /// clips the content about 13.3 pt above the island's bottom edge, so nothing ends lower than 14. The bottom band
    /// (anything whose bottom lies within 36 pt of the island's bottom: the controls, the pill, Saltar, and the text lines
    /// there) keeps 26 from the sides, Saltar 30, clear of the device's rounder corners (D-LA-CORNERS). The top is the
    /// system's: its `.bottom` region starts below the camera row (36.7 pt measured on a 17 Pro Max; `cameraRow` is the
    /// gallery's 36), and `top` takes it to 40 from the island's edge. The budget is 144 pt (D-LA-144), so each state's
    /// content fits in the 90 under that (`room`): the reply is three lines, your words five, and the gaps are what gives
    /// first, never a line of text or a button.
    public var body: some View {
        Group { content }
            .environment(\.locale, state.locale)
            .padding(.horizontal, Self.side)
            .padding(.top, Self.top)
            .padding(.bottom, Self.bottom)
    }

    public static let side: CGFloat = 18
    /// The bottom band's distance from the island's sides (D-LA-CORNERS): `side` and `bandInset`.
    public static let corner: CGFloat = 26
    static let bandInset: CGFloat = corner - side
    /// Saltar's: at 26 the Simulator still cut its bottom-right end (D-LA-CORNERS allows 28 or 30; 28 measured marginal).
    static let skipInset: CGFloat = 30 - side
    /// What the system leaves above the `.bottom` region (the camera's row), which the render tests reproduce.
    public static let cameraRow: CGFloat = 36
    /// The gap from the camera row to the first element: 40 pt from the island's edge in all.
    public static let top: CGFloat = 4

    /// Below every state, the closed states' row too (D-LA-CORNERS).
    public static let bottom: CGFloat = 14
    /// What is under the camera row, within the budget: 144 - 36 - 4 - 14.
    static let room: CGFloat = 90

    @ViewBuilder
    private var content: some View {
        switch state.phase {
        case .speaking:
            Reply(state: state)
        case .listening, .transcribing:
            // The speaking layout (the face and the glyph are the regions beside the camera): only your pill, in the
            // middle of what is under the camera row. No header.
            VoicePill(state: state)
                .frame(maxWidth: .infinity)
                .frame(height: Self.room)
        case .said:
            // Only your words, whole: five lines fill the room under the camera row, 26 from the sides (D-LA-CORNERS).
            Lines(
                text: state.text ?? "", size: 15, weight: 550, lineHeight: 18, lines: TextChunker.saidLines,
                ink: Palette.island.text)
                .padding(.horizontal, Self.bandInset)
        case .idle, .working, .holding, .offline:
            // 6 between the header and the row, so the row's 14 below still fits 144 (D-LA-CORNERS).
            VStack(spacing: 6) {
                ClosedHeader(state: state)
                HStack(spacing: 8) {
                    ActionButton(action: .mute) {
                        RoundButtonFace(
                            symbol: state.micOff ? "mic.slash.fill" : "mic.fill", size: 36, glyph: 18,
                            fill: state.micOff ? Ink.mutedFill : Palette.island.button,
                            ink: state.micOff ? Ink.mutedInk : Palette.island.text)
                    }
                    .accessibilityLabel(state.micOff ? state.words.unmute : state.words.mute)
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
                .padding(.horizontal, Self.bandInset)
            }
        }
    }
}

/// Idle, working, can't receive, offline: tile 48, the conversation and the state (the model and the machine stay on the
/// Lock Screen card: the island has no room for them), the stack.
@available(iOS 16.1, *)
struct ClosedHeader: View {
    var state: CallState

    var body: some View {
        let palette = Palette.island
        HStack(alignment: .center, spacing: 12) {
            CreatureTile(state: state, size: 48, palette: palette)
            VStack(alignment: .leading, spacing: 0) {
                Text(state.title)
                    .font(DMSans.font(16, 700)).tracking(-0.16)
                    .foregroundStyle(palette.text)
                    .line(23.52)
                StateLine(state: state, dark: true, size: 14, lineHeight: 20.58)
                    .padding(.top, 1.99)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            OthersStack(state: state, palette: palette)
        }
        .frame(height: 48)
    }
}

/// Your pill, under the camera row while you talk (scheme E's island): a small white rounded pill, 34 tall, 13 in at its
/// start and 7 at its end, 9 between its parts, ink #1b1d24: the mic (13 pt), then a few bars while listening or the
/// state ("Transcribiendo…", 14 pt, 600) while transcribing; Cancelar envío as a 20-pt × on a 10 % disc of the ink.
@available(iOS 16.1, *)
struct VoicePill: View {
    var state: CallState
    static let ink = Color(hex: 0x1b1d24)

    var body: some View {
        HStack(spacing: 9) {
            Image(systemName: "mic.fill")
                .font(.system(size: 11, weight: .semibold))
                .frame(width: 13, height: 13)
            if state.phase == .transcribing {
                Text(state.words.state)
                    .font(DMSans.font(14, 600))
                    .lineLimit(1)
                    .fixedSize()
            } else {
                PillBars()
            }
            ActionButton(action: .cancelSend, enabled: state.canCancelSend) {
                Image(systemName: "xmark")
                    .font(.system(size: 9, weight: .bold))
                    .frame(width: 20, height: 20)
                    .background(Circle().fill(Self.ink.opacity(0.1)))
            }
            .accessibilityLabel(state.words.cancelSend)
        }
        .foregroundStyle(Self.ink)
        .padding(.leading, 13)
        .padding(.trailing, 7)
        .frame(height: 34)
        .background(Capsule().fill(Color.white))
        .fixedSize()
    }
}

/// The pill's bars: five, 2.5 wide every 4.5, 5 / 9 / 13 / 8 / 4 tall, centred in a 22 × 14 box; still, since a Live
/// Activity cannot follow the voice.
@available(iOS 16.1, *)
struct PillBars: View {
    static let heights: [CGFloat] = [5, 9, 13, 8, 4]

    var body: some View {
        HStack(spacing: 2) {
            ForEach(Array(Self.heights.enumerated()), id: \.offset) { _, height in
                RoundedRectangle(cornerRadius: 1.25).frame(width: 2.5, height: height)
            }
        }
        .frame(width: 22, height: 14)
    }
}

/// Speaking: the reply's chunk, 15 on 18, three lines, all at full strength (no karaoke), 26 from the island's sides; Saltar
/// 4 pt below it, right, 30 from the island's side (D-LA-CORNERS), the Lock Screen's pill.
/// The creature beside the camera is `ExpandedLeading`'s.
@available(iOS 16.1, *)
struct Reply: View {
    var state: CallState

    var body: some View {
        VStack(alignment: .trailing, spacing: 4) {
            Lines(
                text: state.text ?? "", size: 15, weight: 550, tracking: -0.075, lineHeight: 18, lines: TextChunker.replyLines,
                ink: Palette.island.text)
                .padding(.horizontal, ExpandedBody.bandInset)
            if state.canSkip {
                StateAction(state: state, fill: Palette.island.button, ink: Palette.island.text)
                    .padding(.trailing, ExpandedBody.skipInset)
            } else {
                Color.clear.frame(height: 28)
            }
        }
    }
}

/// Expanded, leading: the creature of the voice states (listening, transcribing, your words, speaking: they share the
/// speaking layout), a 30-pt circle beside the camera, 28 pt from the island's edge (B's 18 and 10). It has its own region
/// because the device clips `.bottom`'s content at that region's top, below the camera row; 30 pt stays within the row
/// (36.7 measured), so it never pushes `.bottom` down. Empty in every other state.
@available(iOS 16.1, *)
public struct ExpandedLeading: View {
    public var state: CallState

    public init(state: CallState) {
        self.state = state
    }

    public static let face: CGFloat = 30
    public static let inset: CGFloat = ExpandedBody.side + 10

    public var body: some View {
        if state.voiceLayout {
            RoundFace(state: state, size: Self.face)
                .padding(.leading, Self.inset)
        }
    }
}

/// Expanded, trailing: in the voice states, the state's glyph in its tone, 22 pt from the island's edge, centred on the
/// creature's row: the mic while listening, the bars while transcribing and for your words, the speaker while speaking.
@available(iOS 16.1, *)
public struct ExpandedTrailing: View {
    public var state: CallState

    public init(state: CallState) {
        self.state = state
    }

    public static let inset: CGFloat = 22

    public var body: some View {
        if state.voiceLayout {
            StateGlyph(phase: state.phase, size: 22)
                .frame(height: ExpandedLeading.face)
                .padding(.trailing, Self.inset)
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
        if (state.phase == .idle || state.phase == .said) && !state.mutedBadge {
            EmptyView()
        } else {
            HStack(spacing: 6) {
                if state.mutedBadge {
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
        .overlay {
            if state.phase == .offline {
                // Offline: the face struck through, a grey bar edged in the island's black.
                ZStack {
                    Capsule().fill(Color.black).frame(width: 5.5, height: 37)
                    Capsule().fill(Color(hex: 0xb8b8c0)).frame(width: 2.5, height: 34)
                }
                .rotationEffect(.degrees(-45))
            }
        }
        .overlay(alignment: .bottomTrailing) {
            if state.mutedBadge {
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
