import CoreText
import SwiftUI

// The visual tokens (HANDOVER §2; the numbers are spec/GEOMETRY.md's, from the gallery). The island is always dark; the
// Lock Screen card follows the system's appearance.

extension Color {
    init(hex: UInt32, alpha: Double = 1) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xff) / 255,
            green: Double((hex >> 8) & 0xff) / 255,
            blue: Double(hex & 0xff) / 255,
            opacity: alpha)
    }
}

/// A state's colour family: its ink (text, glyphs), its fill (the dot, the ring) and its soft fill.
struct Tone {
    var ink: Color
    var fill: Color
    var soft: Color

    /// The tone of `phase`, scheme E (the operator's, 2026-11-05): listening, transcribing and your words green, speaking
    /// blue, working violet, can't receive amber, idle and offline greys. Light: the colour itself, its soft 14 % of it on
    /// white; dark: 55 % (fill) and 50 % (ink) of it on white, its soft 30 % on #111 (the gallery's mix).
    static func of(_ phase: CallState.Phase, dark: Bool) -> Tone {
        switch phase {
        case .listening, .transcribing, .said:
            return dark ? Tone(ink: Color(hex: 0x97dbbe), fill: Color(hex: 0x8dd7b7), soft: Color(hex: 0x1a4331))
                : Tone(ink: Color(hex: 0x2fb67c), fill: Color(hex: 0x2fb67c), soft: Color(hex: 0xe2f5ed))
        case .speaking:
            return dark ? Tone(ink: Color(hex: 0x9ec5fe), fill: Color(hex: 0x94bffe), soft: Color(hex: 0x1e3658))
                : Tone(ink: Color(hex: 0x3d8bfd), fill: Color(hex: 0x3d8bfd), soft: Color(hex: 0xe4efff))
        case .working:
            return dark ? Tone(ink: Color(hex: 0xb5a7ec), fill: Color(hex: 0xae9eea), soft: Color(hex: 0x2c244d))
                : Tone(ink: Color(hex: 0x6b4fd8), fill: Color(hex: 0x6b4fd8), soft: Color(hex: 0xeae6fa))
        case .holding:
            return dark ? Tone(ink: Color(hex: 0xf0d497), fill: Color(hex: 0xeecf8c), soft: Color(hex: 0x4f3e1a))
                : Tone(ink: Color(hex: 0xe0a82e), fill: Color(hex: 0xe0a82e), soft: Color(hex: 0xfbf3e2))
        case .idle:
            return dark ? Tone(ink: Color(hex: 0xdcd9e2), fill: Color(hex: 0xd9d5df), soft: Color(hex: 0x434247))
                : Tone(ink: Color(hex: 0xb9b3c4), fill: Color(hex: 0xb9b3c4), soft: Color(hex: 0xf5f4f7))
        case .offline:
            return dark ? Tone(ink: Color(hex: 0xc5c2ca), fill: Color(hex: 0xbfbcc4), soft: Color(hex: 0x353438))
                : Tone(ink: Color(hex: 0x8a8594), fill: Color(hex: 0x8a8594), soft: Color(hex: 0xefeef0))
        }
    }
}

/// The light or dark palette of a presentation.
struct Palette {
    var dark: Bool
    var text: Color { dark ? Color(hex: 0xfafafa) : Color(hex: 0x1b1d24) }
    var text2: Color { dark ? Color(hex: 0xb8b8c0) : Color(hex: 0x545766) }
    var text3: Color { dark ? Color(hex: 0x93939c) : Color(hex: 0x686b7a) }
    /// The neutral button's fill.
    var button: Color { dark ? Color(hex: 0xffffff, alpha: 0.13) : Color(hex: 0x1b1d24, alpha: 0.08) }
    /// The creature tile's background.
    var tile: Color { dark ? Color(hex: 0x2f2741) : Color(hex: 0xece4f7) }

    static let island = Palette(dark: true)
}

enum Ink {
    static let hangUp = Color(hex: 0xdc362e)
    /// The mic muted (scheme E): the badge on the face.
    static let muted = Color(hex: 0xe5484d)
    static let mutedFill = Color(hex: 0xf9dedc)
    static let mutedInk = Color(hex: 0x601410)
    /// The compact island's mic-off glyph: the mic muted's dark ink (scheme E).
    static let micOffGlyph = Color(hex: 0xf2a4a6)
    /// Your voice's bars: the brand purple, the web's accent (`--sv-accent`: #4a2f6b light, #c9b6ec dark), whatever the phase's tone is.
    static func voice(dark: Bool) -> Color { dark ? Color(hex: 0xc9b6ec) : Color(hex: 0x4a2f6b) }
    static let disabled = 0.32
}

/// DM Sans (OFL), bundled: the variable font, at the gallery's weights (400, 550, 600, 650, 700) and its optical size.
public enum DMSans {
    static let registered: Bool = {
        guard let url = Bundle.module.url(forResource: "DMSans", withExtension: "ttf") else { return false }
        return CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
    }()

    /// The font's PostScript name (its default instance).
    static let name = "DMSans-9ptRegular"

    /// DM Sans at `size` pt and the variable weight `weight` (100–1000). The optical size follows the size, as the gallery's
    /// browser does (`font-optical-sizing: auto`).
    static func font(_ size: CGFloat, _ weight: CGFloat) -> Font {
        Font(ctFont(size, weight))
    }

    /// The same, as CoreText's font (to break lines as SwiftUI will draw them).
    static func ctFont(_ size: CGFloat, _ weight: CGFloat) -> CTFont {
        _ = registered
        let axes: [NSNumber: NSNumber] = [
            NSNumber(value: 0x7767_6874): NSNumber(value: Double(weight)),  // 'wght'
            NSNumber(value: 0x6f70_737a): NSNumber(value: Double(min(max(size, 9), 40))),  // 'opsz'
        ]
        let attributes: [CFString: Any] = [kCTFontNameAttribute: name, kCTFontVariationAttribute: axes]
        let descriptor = CTFontDescriptorCreateWithAttributes(attributes as CFDictionary)
        return CTFontCreateWithFontDescriptor(descriptor, size, nil)
    }
}

extension View {
    /// One line of text in the gallery's line box (`size`/`lineHeight`, CSS centres the glyphs in it), ending in an ellipsis.
    func line(_ lineHeight: CGFloat) -> some View {
        lineLimit(1).truncationMode(.tail).frame(height: lineHeight)
    }
}
