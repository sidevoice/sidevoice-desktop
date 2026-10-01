// CI only: the person's editor, full screen in a Space of its own, as the call controls card must float over
// (sidevoice/sidevoice-desktop#4, AC 1–2). It logs to stdout what CI asserts: when it is full screen, every change of
// its text (typing must keep reaching it while the card is clicked or dragged), and whether it stops being the
// active app.
//   swiftc -o Editor.app/Contents/MacOS/Editor test/fixtures/fullscreen-editor.swift (+ an Info.plist); open Editor.app
import AppKit

setvbuf(stdout, nil, _IOLBF, 0)

/// One log line, with the time (seconds since 1970) so CI can order it against the app's.
func log(_ line: String) { print(String(format: "%.2f ", Date().timeIntervalSince1970) + line) }

final class Editor: NSObject, NSApplicationDelegate, NSWindowDelegate, NSTextViewDelegate {
    var window: NSWindow!
    var text: NSTextView!

    func applicationDidFinishLaunching(_ notification: Notification) {
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 800, height: 600),
                          styleMask: [.titled, .resizable, .closable], backing: .buffered, defer: false)
        window.title = "Editor (CI)"
        window.collectionBehavior = [.fullScreenPrimary]
        window.delegate = self
        text = NSTextView(frame: window.contentView!.bounds)
        text.autoresizingMask = [.width, .height]
        text.delegate = self
        window.contentView!.addSubview(text)
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(text)
        NSApp.activate(ignoringOtherApps: true)
        log("editor ready pid=\(ProcessInfo.processInfo.processIdentifier)")
        // Which app takes the activation when this one loses it.
        NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
        ) { note in
            let app = note.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication
            log("editor sees active app=\(app?.bundleIdentifier ?? "?") pid=\(app?.processIdentifier ?? 0)")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { self.window.toggleFullScreen(nil) }
    }
    func windowDidEnterFullScreen(_ notification: Notification) {
        window.makeFirstResponder(text)
        log("editor fullscreen=\(window.styleMask.contains(.fullScreen)) key=\(window.isKeyWindow)")
    }
    func textDidChange(_ notification: Notification) { log("editor text=\(text.string)") }
    func applicationDidBecomeActive(_ notification: Notification) { log("editor active=true") }
    func applicationDidResignActive(_ notification: Notification) { log("editor active=false") }
    func windowDidResignKey(_ notification: Notification) { log("editor key=false") }
    func windowDidBecomeKey(_ notification: Notification) { log("editor key=true") }
}

let app = NSApplication.shared
let editor = Editor()
app.delegate = editor
app.setActivationPolicy(.regular)
app.run()
