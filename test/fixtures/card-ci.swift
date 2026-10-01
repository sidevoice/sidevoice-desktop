// CI only: acts as the person at the Mac, so CI can check the call controls card (sidevoice/sidevoice-desktop#4) as it
// is used: pointer, clicks, drags and typing posted as the system's own input events, and where the window server has
// each window. Points are global, top-left origin.
//   card-ci trusted                        whether this process may post input events (Accessibility)
//   card-ci move <x> <y>                   moves the pointer there
//   card-ci click <x> <y>                  a left click there
//   card-ci drag <x> <y> <dx> <dy>         press there, move by dx,dy in steps, release
//   card-ci type <text>                    types it into whatever has the keyboard
//   card-ci windows <card pid> <editor pid>
//       the card's window (that app's window at the status level, 25) and the editor's, as the window server lists
//       what is on screen, front to back: "card onscreen=… above_editor=… x=… y=… w=… h=…"
import AppKit
import ApplicationServices
import CoreGraphics
import Foundation

/// Every line says when, and which app is frontmost after the action.
func say(_ line: String) {
    let front = NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "?"
    print(String(format: "%.2f ", Date().timeIntervalSince1970) + line + " (front: \(front))")
}

func post(_ type: CGEventType, _ at: CGPoint) {
    CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: at, mouseButton: .left)!.post(tap: .cghidEventTap)
    usleep(30_000)
}

let args = Array(CommandLine.arguments.dropFirst())
let number = { (i: Int) -> Double in Double(args[i])! }
switch args.first {
case "trusted":
    print("trusted=\(AXIsProcessTrusted())")
case "move" where args.count == 3:
    CGWarpMouseCursorPosition(CGPoint(x: number(1), y: number(2)))
    say("pointer at \(args[1]),\(args[2])")
case "click" where args.count == 3:
    let at = CGPoint(x: number(1), y: number(2))
    post(.mouseMoved, at)
    post(.leftMouseDown, at)
    post(.leftMouseUp, at)
    usleep(200_000)
    say("clicked at \(args[1]),\(args[2])")
case "drag" where args.count == 5:
    let from = CGPoint(x: number(1), y: number(2))
    let (dx, dy) = (number(3), number(4))
    post(.mouseMoved, from)
    post(.leftMouseDown, from)
    for step in 1...20 {
        post(.leftMouseDragged, CGPoint(x: from.x + dx * Double(step) / 20, y: from.y + dy * Double(step) / 20))
    }
    post(.leftMouseUp, CGPoint(x: from.x + dx, y: from.y + dy))
    usleep(200_000)
    say("dragged from \(args[1]),\(args[2]) by \(args[3]),\(args[4])")
case "type" where args.count == 2:
    for unit in args[1].utf16 {
        for down in [true, false] {
            let key = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: down)!
            var char = unit
            key.keyboardSetUnicodeString(stringLength: 1, unicodeString: &char)
            key.post(tap: .cghidEventTap)
            usleep(30_000)
        }
    }
    say("typed \(args[1])")
case "windows" where args.count == 3:
    let (card, editor) = (Int(args[1])!, Int(args[2])!)
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
        as? [[String: Any]] ?? []
    let pid = { (w: [String: Any]) in w[kCGWindowOwnerPID as String] as? Int ?? -1 }
    let layer = { (w: [String: Any]) in w[kCGWindowLayer as String] as? Int ?? -1 }
    let cardIndex = list.firstIndex { pid($0) == card && layer($0) == 25 }
    let editorIndex = list.firstIndex { pid($0) == editor && layer($0) == 0 }
    guard let c = cardIndex else {
        print("card onscreen=false editor_onscreen=\(editorIndex != nil)")
        for w in list { print("  pid=\(pid(w)) layer=\(layer(w)) bounds=\(w[kCGWindowBounds as String] ?? "")") }
        exit(1)
    }
    let b = list[c][kCGWindowBounds as String] as? [String: Double] ?? [:]
    let above = editorIndex.map { c < $0 } ?? false
    print(String(format: "card onscreen=true above_editor=%@ editor_onscreen=%@ x=%.0f y=%.0f w=%.0f h=%.0f",
                 "\(above)", "\(editorIndex != nil)", b["X"] ?? 0, b["Y"] ?? 0, b["Width"] ?? 0, b["Height"] ?? 0))
default:
    print("usage: see the top of test/fixtures/card-ci.swift")
    exit(2)
}
