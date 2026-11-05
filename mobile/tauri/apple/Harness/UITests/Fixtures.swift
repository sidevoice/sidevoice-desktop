import Foundation

/// The call's details the harness starts the Live Activity from: the JSON the Rust side hands `SidevoiceCallActivity`
/// (`details_json` in mobile/tauri/plugin/src/call_surface.rs: `title`, `muted`, and the `CallSurface` in camelCase),
/// with the gallery's sample texts (research/live-activity-design-2026-11-02, src/data.js `STRINGS`, es and en).
struct Fixture {
    enum Presentation: String, CaseIterable {
        case lock, compact, expanded
    }

    /// `<state>[-<variant>]`, as the shots are named.
    var name: String
    var details: [String: Any]
    var presentations: [Presentation] = [.lock, .compact, .expanded]
    /// Shot in dark appearance too: every state, and the variants the gallery draws dark. The others are shot in light
    /// only, which keeps the run within the CI's time.
    var dark = false

    var json: String {
        let data = try! JSONSerialization.data(withJSONObject: details, options: [.sortedKeys])
        return String(decoding: data, as: UTF8.self)
    }
}

enum Language: String {
    case es, en
}

/// The gallery's texts (src/data.js `STRINGS`).
struct Texts {
    var states: [String: String]
    var mute, unmute, skip, skipName, cancelSend, hangUp, route, speaker: String
    var conversations: (Int) -> String
    var title, model, machine: String
    var long: (title: String, model: String, machine: String)
    var others: [String]
    var agentSays, agentTooLong, youSaid, youSaidLong: String

    static func of(_ language: Language) -> Texts {
        switch language {
        case .es:
            return Texts(
                states: [
                    "idle": "En reposo", "listening": "Escuchándote", "transcribing": "Transcribiendo…",
                    // Your words are not a state: the header says the call is working (D-LA-SAID-STATE).
                    "said": "Trabajando", "working": "Trabajando", "speaking": "Hablando", "holding": "No puede recibir",
                    "offline": "Desconectada",
                ],
                mute: "Silenciar", unmute: "Activar micrófono", skip: "Saltar", skipName: "Saltar lo que está sonando",
                cancelSend: "Cancelar envío", hangUp: "Colgar", route: "Salida de audio: {route}", speaker: "Altavoz",
                conversations: { "Conversaciones (\($0))" },
                title: "Globo del escenario", model: "Opus 5.5", machine: "mac-studio",
                long: (
                    "Refactorizar las notificaciones del escritorio y del móvil", "claude-opus-5-5 · xhigh",
                    "MacBook-Pro-de-Ruben.local"
                ),
                others: ["Ajustes de voz", "Notas de la versión", "Arreglar la CI"],
                agentSays: "He movido el globo encima del bicho: ya no le tapa la cara. ¿Lo subo a TestFlight?",
                agentTooLong: "He movido el globo encima del bicho: ya no le tapa la cara. También he ajustado la cola para "
                    + "que apunte a quien habla, y en el móvil el globo baja un poco para no pisar la cabecera. He revisado "
                    + "el iPad en horizontal y ahí también encaja. ¿Lo subo a TestFlight y lo pruebas en el iPhone, o "
                    + "prefieres que antes repase el modo oscuro?",
                youSaid: "Sí, súbelo a TestFlight y avísame cuando esté.",
                youSaidLong: "Sí, súbelo a TestFlight, y de paso mira si el globo sigue tapando la cabecera en el iPad en "
                    + "horizontal, que ayer me pareció que se montaba un poco encima.")
        case .en:
            return Texts(
                states: [
                    "idle": "Resting", "listening": "Listening to you", "transcribing": "Transcribing…",
                    "said": "Working", "working": "Working", "speaking": "Speaking", "holding": "Can't receive",
                    "offline": "Offline",
                ],
                mute: "Mute", unmute: "Unmute", skip: "Skip", skipName: "Skip what is playing",
                cancelSend: "Cancel sending", hangUp: "Hang up", route: "Audio output: {route}", speaker: "Speaker",
                conversations: { "Conversations (\($0))" },
                title: "Stage balloon", model: "Opus 5.5", machine: "mac-studio",
                long: (
                    "Refactor the desktop and mobile notifications end to end", "claude-opus-5-5 · xhigh",
                    "MacBook-Pro-de-Ruben.local"
                ),
                others: ["Voice settings", "Release notes", "Fix the CI"],
                agentSays: "I moved the balloon above the creature, so it no longer covers its face. Shall I push it to "
                    + "TestFlight?",
                agentTooLong: "I moved the balloon above the creature, so it no longer covers its face. I also turned its "
                    + "tail toward whoever is speaking, and on the phone the balloon sits a little lower so it clears the "
                    + "header. I checked the iPad in landscape too and it fits there. Shall I push it to TestFlight so you "
                    + "can try it on the iPhone, or should I go over dark mode first?",
                youSaid: "Yes, push it to TestFlight and tell me when it's up.",
                youSaidLong: "Yes, push it to TestFlight, and while you are at it check whether the balloon still covers "
                    + "the header on the iPad in landscape, it looked a bit off yesterday.")
        }
    }
}

extension Fixture {
    static let phases = ["idle", "listening", "transcribing", "said", "working", "speaking", "holding", "offline"]

    /// The stills the harness app writes (its bundled PNGs, each under its id's file name), and their `k`.
    static let current = ("straw-hat", 1.3)
    static let others = [("bunny-headphones", 1.35), ("wizard-hat", 1.4), ("straw-hat", 1.3)]

    /// The Rust side's file name of still `id` (`Still::file_name`): the id's bytes in hex.
    static func file(_ id: String) -> String {
        id.utf8.map { String(format: "%02x", $0) }.joined() + ".png"
    }

    static func still(_ id: String, _ k: Double) -> [String: Any] {
        ["id": id, "k": k, "file": file(id)]
    }

    /// The details for `phase`, as the Rust side would send them for the gallery's call.
    static func details(
        _ phase: String, language: Language = .es, micOff: Bool = false, long: Bool = false, stills: Bool = true,
        conversations: Int = 3
    ) -> [String: Any] {
        let texts = Texts.of(language)
        let now = Date().timeIntervalSince1970 * 1000
        // A still the page named but whose PNG has not arrived: the activity draws the Sidevoice mark (D-LA-STILL-FALLBACK).
        let named = { (id: String, k: Double) in stills ? still(id, k) : still("missing-\(id)", k) }
        let others = Self.others.prefix(min(conversations - 1, 3)).enumerated().map { index, other in
            ["title": texts.others[index], "still": named(other.0, other.1)] as [String: Any]
        }
        var surface: [String: Any] = [
            "phase": phase,
            "canSkip": phase == "speaking",
            "canCancelSend": phase == "listening" || phase == "transcribing",
            "model": long ? texts.long.model : texts.model,
            "machine": long ? texts.long.machine : texts.machine,
            "still": named(current.0, current.1),
            "others": others,
            "conversationCount": conversations,
            "language": language.rawValue,
            "words": [
                "state": texts.states[phase]!,
                "offline": texts.states["offline"]!,
                "mute": texts.mute,
                "unmute": texts.unmute,
                "skip": texts.skip,
                "skipName": texts.skipName,
                "cancelSend": texts.cancelSend,
                "hangUp": texts.hangUp,
                "route": texts.route,
                "speaker": texts.speaker,
                "conversations": texts.conversations(conversations),
            ],
        ]
        // Working for 1:42 (the gallery's), your words sent 2 s ago.
        if phase == "working" { surface["workingSince"] = now - 102_000 }
        if phase == "said" {
            surface["workingSince"] = now - 2_000
            surface["said"] = long ? texts.youSaidLong : texts.youSaid
        }
        if phase == "speaking" { surface["page"] = ["text": long ? texts.agentTooLong : texts.agentSays, "lit": 0] }
        return ["title": long ? texts.long.title : texts.title, "muted": micOff, "surface": surface]
    }

    /// Every fixture the harness shoots (`dark`: in dark appearance too): each state; mic off; English for the states with words; long texts in both
    /// languages; stills missing; 6 and 9 conversations.
    static var all: [Fixture] {
        var all: [Fixture] = []
        for phase in phases {
            all.append(Fixture(name: phase, details: details(phase), dark: true))
        }
        for phase in ["idle", "listening", "working", "speaking"] {
            all.append(Fixture(name: "\(phase)-mic-off", details: details(phase, micOff: true), dark: phase == "working"))
        }
        // English where the layout carries the call's words, not only the state's.
        for phase in ["listening", "said", "working", "speaking"] {
            all.append(Fixture(name: "\(phase)-en", details: details(phase, language: .en)))
        }
        for phase in ["working", "said", "speaking"] {
            all.append(Fixture(name: "\(phase)-long", details: details(phase, long: true)))
            all.append(Fixture(name: "\(phase)-long-en", details: details(phase, language: .en, long: true)))
        }
        for phase in ["working", "listening"] {
            all.append(Fixture(name: "\(phase)-no-still", details: details(phase, stills: false), dark: true))
        }
        for count in [6, 9] {
            all.append(Fixture(name: "working-conversations-\(count)", details: details("working", conversations: count)))
        }
        return all
    }
}
