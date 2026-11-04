import SwiftUI
import XCTest

@testable import CallActivity

/// Every state of the Live Activity drawn to PNG at 3×, as the gallery draws it (research/live-activity-design-2026-11-02,
/// its png/ and spec/GEOMETRY.md): the Lock Screen card in light and dark, and the island's expanded, compact and minimal
/// views, each in the frame the system gives it on an iPhone 17 Pro (380 pt; the island always black). One sheet per
/// state puts them side by side, as the gallery's `pieces-<state>.png`; the mic off, Always-On, a stale activity, the
/// captions half heard and a 17 Pro Max are drawn too. The PNGs go to `$CALL_ACTIVITY_RENDERS` (`cargo xtask mobile renders
/// ios` sets it), or the temporary directory.
@available(iOS 17.0, *)
@MainActor
final class RenderTests: XCTestCase {
    static let out: URL = {
        let path = ProcessInfo.processInfo.environment["CALL_ACTIVITY_RENDERS"] ?? NSTemporaryDirectory() + "call-activity"
        let url = URL(fileURLWithPath: path, isDirectory: true)
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }()

    /// The gallery's call: "Globo del escenario", two other conversations wearing big accessories, AirPods connected.
    static func sample(_ phase: CallState.Phase, micOff: Bool = false) -> CallState {
        CallStills.directory = Bundle.module.url(forResource: "Fixtures", withExtension: nil)
        var state = CallState()
        state.phase = phase
        state.title = "Globo del escenario"
        state.model = "Opus 5.5"
        state.machine = "mac-studio"
        state.micOff = micOff
        state.canSkip = phase == .speaking
        state.canCancelSend = phase == .listening || phase == .transcribing
        // Your words are not a state (D-LA-SAID-STATE): the header says the call is working, with its timer.
        state.workingSince = phase == .working ? Date().addingTimeInterval(-102) : phase == .said ? Date().addingTimeInterval(-2) : nil
        state.creature = .init(file: "straw-hat.png", k: 1.3)
        state.others = [.init(file: "bunny-headphones.png", k: 1.35), .init(file: "wizard-hat.png", k: 1.4)]
        state.conversationCount = 3
        state.language = "es"
        state.route = .headphones
        state.routeName = "AirPods Pro"
        switch phase {
        case .speaking:
            // The first chunk of the reply (three measured lines at most), as the chunker cuts it.
            state.text = Self.reply[0]
        case .said:
            state.text = "Sí, súbelo a TestFlight, y de paso mira si el globo sigue tapando la cabecera en el iPad en "
                + "horizontal, que ayer me pareció que se montaba un poco encima."
        default:
            break
        }
        if phase == .said { state.text = Self.said[0] }
        var words = CallState.Words()
        words.state = [
            .idle: "En reposo", .listening: "Escuchándote", .transcribing: "Transcribiendo…", .said: "Trabajando",
            .working: "Trabajando", .speaking: "Hablando", .holding: "No puede recibir", .offline: "Desconectada",
        ][phase]!
        words.offline = "Desconectada"
        words.mute = "Silenciar"
        words.unmute = "Activar micrófono"
        words.skip = "Saltar"
        words.skipName = "Saltar lo que está sonando"
        words.cancelSend = "Cancelar envío"
        words.hangUp = "Colgar"
        words.route = "Salida de audio: {route}"
        words.speaker = "Altavoz"
        words.conversations = "Conversaciones (3)"
        state.words = words
        return state
    }

    static let replyText = "He movido el globo encima del bicho: ya no le tapa la cara. También he ajustado la cola para que "
        + "apunte a quien habla, y en el móvil el globo baja un poco para no pisar la cabecera. He revisado el iPad "
        + "en horizontal y ahí también encaja. ¿Lo subo a TestFlight y lo pruebas en el iPhone, o prefieres que antes "
        + "repase el modo oscuro?"
    static let saidText = "Sí, súbelo a TestFlight, y de paso mira si el globo sigue tapando la cabecera en el iPad en "
        + "horizontal, que ayer me pareció que se montaba un poco encima."
    /// The chunks the app would send for them, measured with DM Sans as on the device.
    static let reply = TextChunker.dmSans().chunks(replyText).map(\.text)
    static let said = TextChunker.dmSans().saidChunks(saidText).map(\.text)

    static let phases: [CallState.Phase] = [.idle, .listening, .transcribing, .said, .working, .speaking, .holding, .offline]

    func testEveryState() throws {
        for phase in Self.phases {
            try sheet(Self.sample(phase), name: phase.rawValue)
        }
    }

    func testMicOff() throws {
        for phase in Self.phases {
            try sheet(Self.sample(phase, micOff: true), name: "\(phase.rawValue)-mic-off")
        }
    }

    func testAlwaysOnStaleCaptionsAndProMax() throws {
        for phase in [CallState.Phase.working, .speaking, .listening] {
            try save(
                lock(Self.sample(phase), dark: true).environment(\.isLuminanceReduced, true),
                "always-on-lock-dark-\(phase.rawValue)")
        }
        try sheet(Self.sample(.working).stale, name: "stale")
        // The reply's other chunks, and your words' (D-LA-CHUNKS).
        for (index, chunk) in Self.reply.enumerated().dropFirst() {
            var next = Self.sample(.speaking)
            next.text = chunk
            try save(expanded(next), "expanded-speaking-chunk-\(index + 1)")
        }
        for (index, chunk) in Self.said.enumerated().dropFirst() {
            var next = Self.sample(.said)
            next.text = chunk
            try save(expanded(next), "expanded-said-chunk-\(index + 1)")
        }
        // Your words before the room knows when the work began: the real header, with no timer (never a made-up time).
        var noTimer = Self.sample(.said)
        noTimer.workingSince = nil
        try sheet(noTimer, name: "said-no-timer")
        // Before the web pin that says working: your words with the page's own idle line and no timer, never green.
        var prePin = Self.sample(.said)
        prePin.workingSince = nil
        prePin.words.state = "En reposo"
        try sheet(prePin, name: "said-pre-pin")
        // A still that has not arrived draws the Sidevoice mark, never an empty slot (D-LA-STILL-FALLBACK).
        for phase in [CallState.Phase.working, .listening] {
            var missing = Self.sample(phase)
            missing.creature = nil
            missing.others = [nil, nil]
            try sheet(missing, name: "\(phase.rawValue)-no-still")
        }
        var speaker = Self.sample(.idle)
        speaker.route = .speaker
        try sheet(speaker, name: "idle-on-speaker")
        var noHeadphones = Self.sample(.idle)
        noHeadphones.route = nil
        try sheet(noHeadphones, name: "idle-no-headphones")
        var long = Self.sample(.working)
        long.title = "Refactorizar las notificaciones del escritorio y del móvil"
        long.model = "claude-opus-5-5 · xhigh"
        long.machine = "MacBook-Pro-de-Ruben.local"
        try sheet(long, name: "working-long-texts")
        // A label that does not fit whole leaves its button as the glyph alone, never an ellipsis (German).
        var german = Self.sample(.working, micOff: true)
        german.words.state = "Arbeitet"
        german.words.mute = "Stummschalten"
        german.words.unmute = "Stummschaltung aufheben"
        try sheet(german, name: "working-mic-off-de")
        var germanIdle = Self.sample(.idle)
        germanIdle.words.state = "Bereit"
        germanIdle.words.mute = "Stummschalten"
        try sheet(germanIdle, name: "idle-de")
        try save(lock(Self.sample(.working), dark: false, width: 418), "promax-lock-light-working")
        try save(expanded(Self.sample(.speaking), width: 418), "promax-expanded-speaking")
    }

    // MARK: The presentations, in the frames the system gives them

    func lock(_ state: CallState, dark: Bool, width: CGFloat = 380) -> some View {
        LockCard(state: state)
            .frame(width: width)
            // The gallery's panel (the system's material on a device).
            .background(dark ? Color(hex: 0x1c1c20, alpha: 0.7) : Color(hex: 0xfaf9fc, alpha: 0.78))
            .background(dark ? Color(hex: 0x14121a) : Color(hex: 0xe9e2f4))
            .clipShape(RoundedRectangle(cornerRadius: 24, style: .continuous))
            .environment(\.colorScheme, dark ? .dark : .light)
    }

    /// The expanded island: black, radius 44, the camera at the top; B's content is `ExpandedBody`, which carries its own
    /// paddings (18 at the sides, 14 or 10 below, 4 below the camera's row), as on the device; speaking's creature is
    /// `ExpandedLeading`, beside the camera.
    func expanded(_ state: CallState, width: CGFloat = 380) -> some View {
        ZStack(alignment: .topLeading) {
            camera(x: (width - 125) / 2)
            // The `.leading` region, beside the camera: its content centred on the camera's row.
            ExpandedLeading(state: state).frame(height: ExpandedBody.cameraRow)
            // The system's `.bottom` region starts below the camera's row and clips its content at its bounds, as the
            // device does; its side and bottom margins are zeroed.
            ExpandedBody(state: state).clipped().padding(.top, ExpandedBody.cameraRow)
        }
        .frame(width: width, alignment: .topLeading)
        .background(Color.black)
        .clipShape(RoundedRectangle(cornerRadius: 44, style: .continuous))
        .environment(\.colorScheme, .dark)
    }

    /// The compact island: the face 6 pt from the pill's edge, the camera (125 pt), the trailing glyph 4 pt after it.
    func compact(_ state: CallState) -> some View {
        HStack(spacing: 0) {
            CompactLeading(state: state).padding(.leading, 6).frame(width: 36, alignment: .leading)
            Color.clear.frame(width: 125)
            CompactTrailing(state: state).padding(.leading, 4).padding(.trailing, 9)
        }
        .frame(height: 36.66)
        .overlay(alignment: .leading) { lens.offset(x: 137) }
        .background(Capsule().fill(Color.black))
        .environment(\.colorScheme, .dark)
    }

    func minimal(_ state: CallState) -> some View {
        MinimalFace(state: state)
            .frame(width: 36.66, height: 36.66)
            .background(Color.black)
            // The island's own shape: anything outside the circle is cut, as on the device.
            .clipShape(Circle())
            .environment(\.colorScheme, .dark)
    }

    func camera(x: CGFloat) -> some View {
        lens.offset(x: x + 101, y: 12.83)
    }

    var lens: some View {
        Circle().fill(Color(hex: 0x0e1016)).frame(width: 11, height: 11)
    }

    // MARK: Output

    /// One state's sheet, as the gallery's pieces: Lock Screen light and dark, expanded, compact, minimal. Each piece is saved
    /// on its own too.
    func sheet(_ state: CallState, name: String) throws {
        try save(lock(state, dark: false), "lock-light-\(name)")
        try save(lock(state, dark: true), "lock-dark-\(name)")
        try save(expanded(state), "expanded-\(name)")
        try save(compact(state), "compact-\(name)")
        try save(minimal(state), "minimal-\(name)")
        let sheet = HStack(alignment: .top, spacing: 24) {
            labelled("LOCK SCREEN · LIGHT", lock(state, dark: false))
            labelled("LOCK SCREEN · DARK", lock(state, dark: true))
            labelled("EXPANDED", expanded(state))
            VStack(alignment: .leading, spacing: 16) {
                labelled("COMPACT", compact(state))
                labelled("MINIMAL", minimal(state))
            }
        }
        .padding(20)
        .background(Color(hex: 0xf4f1f8))
        try save(sheet, "pieces-\(name)")
    }

    func labelled(_ label: String, _ view: some View) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(label).font(.system(size: 11, weight: .semibold)).foregroundStyle(Color(hex: 0x686b7a))
            view
        }
    }

    func save(_ view: some View, _ name: String) throws {
        let renderer = ImageRenderer(content: view.fixedSize())
        renderer.scale = 3
        let image = try XCTUnwrap(renderer.uiImage, name)
        let png = try XCTUnwrap(image.pngData(), name)
        try png.write(to: Self.out.appendingPathComponent("\(name).png"))
        // The height the HIG allows: every card ≤ 160 pt (HANDOVER §0).
        if name.hasPrefix("lock-") || name.hasPrefix("promax-lock-") {
            XCTAssertLessThanOrEqual(image.size.height, 160, "\(name): \(image.size.height) pt tall")
        }
        // The expanded island's budget is 144 pt (R2-8, D-LA-144): everything of every state fits, nothing clipped.
        if name.hasPrefix("expanded-") || name.hasPrefix("promax-expanded-") {
            XCTAssertLessThanOrEqual(image.size.height, 144.5, "\(name): \(image.size.height) pt tall")
        }
        print("render \(name): \(image.size.width) × \(image.size.height) pt")
    }
}

/// The captions' lines: broken as CoreText breaks them, at most `lines`, and a cut page ends in "…" after a whole word.
@available(iOS 17.0, *)
@MainActor
final class LinesTests: XCTestCase {
    func testALongPageEndsAfterAWholeWord() throws {
        // The whole reply, not a chunk: longer than four lines whatever the chunks' size.
        let text = RenderTests.replyText + " Y luego seguimos con el modo oscuro, que también tiene lo suyo."
        let lines = Lines.lines(text, width: 344, font: DMSans.ctFont(15, 550), tracking: -0.075, max: 4)
        XCTAssertEqual(lines.count, 4)
        XCTAssertEqual(lines.filter(\.ellipsis).count, 1)
        let last = try XCTUnwrap(lines.last)
        XCTAssertTrue(last.ellipsis)
        let whole = text as NSString
        // The character after the cut is not a letter of the same word.
        let next = whole.substring(with: NSRange(location: last.range.upperBound, length: 1))
        XCTAssertTrue(next.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || ",.;:".contains(next), "cut before \(next)")
        for (a, b) in zip(lines, lines.dropFirst()) {
            XCTAssertEqual(a.range.upperBound, b.range.location, "the lines follow each other")
        }
    }

    func testABlankLineBeforeMoreTextNeverBreaksTheCut() {
        let text = "Primera línea.\n\n\n\n\nY después, mucho más texto que ya no cabe en la página."
        let lines = Lines.lines(text, width: 344, font: DMSans.ctFont(15, 550), tracking: -0.075, max: 4)
        XCTAssertLessThanOrEqual(lines.count, 4)
        for line in lines {
            XCTAssertGreaterThanOrEqual(line.range.length, 0)
            XCTAssertLessThanOrEqual(line.range.upperBound, (text as NSString).length)
        }
        XCTAssertTrue(lines.last?.ellipsis ?? false)
    }

    func testTextWithoutSpacesIsCutBetweenCharacters() {
        let text = String(repeating: "会話の続きをここに表示します", count: 12)
        let lines = Lines.lines(text, width: 344, font: DMSans.ctFont(15, 550), tracking: 0, max: 4)
        XCTAssertEqual(lines.count, 4)
        let last = lines[3]
        XCTAssertTrue(last.ellipsis)
        XCTAssertGreaterThan(last.range.length, 0, "not collapsed to the ellipsis alone")
    }

    func testAShortPageIsWhole() {
        let lines = Lines.lines("Hecho.", width: 344, font: DMSans.ctFont(15, 550), tracking: 0, max: 4)
        XCTAssertEqual(lines.count, 1)
        XCTAssertFalse(lines[0].ellipsis)
        XCTAssertEqual(lines[0].range.length, 6)
    }
}
