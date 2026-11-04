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

    /// The tone of `phase` (D-LA-STATES, D-LA-VOICE-LILA): the voice states lila, working green, can't receive amber, idle
    /// and offline grey.
    static func of(_ phase: CallState.Phase, dark: Bool) -> Tone {
        switch phase {
        case .listening, .transcribing, .speaking:
            return dark ? Tone(ink: Color(hex: 0xcdbbf0), fill: Color(hex: 0xc9b6ec), soft: Color(hex: 0x2f2741))
                : Tone(ink: Color(hex: 0x4a2f6b), fill: Color(hex: 0x4a2f6b), soft: Color(hex: 0xece4f7))
        case .working:
            return dark ? Tone(ink: Color(hex: 0x74d3a6), fill: Color(hex: 0x74d3a6), soft: Color(hex: 0x14271e))
                : Tone(ink: Color(hex: 0x1d6b45), fill: Color(hex: 0x1d6b45), soft: Color(hex: 0xe2f3e9))
        case .holding:
            return dark ? Tone(ink: Color(hex: 0xf5a35c), fill: Color(hex: 0xf5a35c), soft: Color(hex: 0x2e1f10))
                : Tone(ink: Color(hex: 0x9a4a00), fill: Color(hex: 0x9a4a00), soft: Color(hex: 0xfdeedd))
        case .idle, .offline, .said:
            // `.said` never gets here: `CallState.tonePhase` says what the call is.
            return dark ? Tone(ink: Color(hex: 0xb8b8c0), fill: Color(hex: 0x93939c), soft: Color(hex: 0x26262b))
                : Tone(ink: Color(hex: 0x545766), fill: Color(hex: 0x686b7a), soft: Color(hex: 0xefebf4))
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
    static let mutedFill = Color(hex: 0xf9dedc)
    static let mutedInk = Color(hex: 0x601410)
    /// The compact island's mic-off glyph (GEOMETRY: rgb(255, 180, 171)).
    static let micOffGlyph = Color(hex: 0xffb4ab)
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
