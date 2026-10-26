# The global mute shortcut

The app registers one global shortcut, mute / unmute (sidevoice/sidevoice-desktop#4): `⌃⌥M` on macOS, `Ctrl+Alt+M`
on Windows and Linux (`DEFAULT_MUTE_SHORTCUT`, core `settings.rs`), editable only in the web's Settings
(`host.muteShortcut`, docs/BRIDGE.md). When the system refuses it, the app answers so with those of
`SHORTCUT_ALTERNATIVES` that the system does take, `Ctrl+Alt+Shift+M` then `Ctrl+Alt+Shift+K`, for the page to offer.
On macOS it also says when VoiceOver is on and the shortcut holds Control and Option together.

## Checked against the systems' published default shortcuts

Checked on 2026-10-01 against the lists and sources below. Nothing here was tried on a real machine.

| | `Ctrl+Alt+M` / `⌃⌥M` | `Ctrl+Alt+Shift+M` / `⌃⌥⇧M` | `Ctrl+Alt+Shift+K` / `⌃⌥⇧K` |
|---|---|---|---|
| macOS default shortcuts | not used | not used | not used |
| macOS VoiceOver (off by default) | VO-M: move to the menu bar | VO-Shift-M: open a shortcut menu | VO-Shift-K: Option keys control VoiceOver, on or off |
| Windows 11 default shortcuts | not used | not used | not used |
| Windows Magnifier (off by default) | cycle through views | not used | not used (`Ctrl+Alt+K` without Shift reads the next sentence) |
| GNOME (upstream and Ubuntu) | not used | not used | not used |
| KDE Plasma (global) | not used (Konsole's own "Previous Profile" while Konsole has focus) | not used | not used |

None of the three is a default shortcut on any of the four systems. Two assistive tools that are off by default use
them. That is why the app warns while VoiceOver is on. No warning exists for Magnifier yet.

### Windows: Ctrl+Alt is also AltGr

Windows treats Ctrl+Alt as AltGr on many keyboard layouts, so a global `Ctrl+Alt+<letter>` can stop the person typing
that layout's AltGr character. Microsoft's guidance is not to use Ctrl+Alt combinations.

| Shortcut | Character it can block | Layouts |
|---|---|---|
| `Ctrl+Alt+M` | µ | German, Swedish, Norwegian, Danish, Finnish, Dutch, Icelandic, Greek Latin, US-International |
| `Ctrl+Alt+M` | § | Polish (214), Romanian (Legacy), Serbian (Latin), Slovenian |
| `Ctrl+Alt+Shift+M` | W | Latvian |
| `Ctrl+Alt+Shift+K` | Ķ | Latvian |

This is not a system shortcut collision, so #4's check passes. It is still a known cost of the default on Windows,
and it is an open question for the person who decides the defaults.

## Sources

- macOS:
  - [Mac keyboard shortcuts](https://support.apple.com/en-us/102650) (published 2026-09-14).
  - The VoiceOver command pages: [general](https://support.apple.com/guide/voiceover/general-commands-cpvokys01/mac),
    [orientation](https://support.apple.com/guide/voiceover/orientation-commands-cpvokys03/mac),
    [navigation](https://support.apple.com/guide/voiceover/navigation-commands-cpvokys04/mac),
    [text](https://support.apple.com/guide/voiceover/text-commands-cpvokys06/mac),
    [interaction](https://support.apple.com/guide/voiceover/interaction-commands-cpvokys07/mac),
    [search](https://support.apple.com/guide/voiceover/search-commands-cpvokys08/mac).
    "VO" is Control+Option (or Caps Lock).
- Windows:
  - [Keyboard shortcuts in Windows](https://support.microsoft.com/en-us/windows/keyboard-shortcuts-in-windows-dcc61a57-8ff0-cffe-9796-cb9706c75eec).
  - [Windows keyboard shortcuts for accessibility](https://support.microsoft.com/en-us/windows/windows-keyboard-shortcuts-for-accessibility-021bcb62-45c8-e4ef-1e4f-41b8c1fc87fd).
  - [Magnifier reading](https://support.microsoft.com/en-us/accessibility/windows/magnifier/how-to-use-magnifier-reading).
  - [Narrator commands](https://support.microsoft.com/en-us/windows/appendix-b-narrator-keyboard-commands-and-touch-gestures-8bdab3f4-b3e9-4554-7f28-8b15bd37410a).
  - [Win32 keyboard guidelines](https://learn.microsoft.com/en-us/windows/win32/uxguide/inter-keyboard) (on Ctrl+Alt).
  - AltGr characters per layout come from [kbdlayout.info](https://kbdlayout.info), a third-party site generated from
    the Windows layout files.
- GNOME:
  - [Useful keyboard shortcuts](https://help.gnome.org/users/gnome-help/stable/shell-keyboard-shortcuts.html).
  - The defaults in the settings schemas on each project's main branch, 2026-10-01:
    - gsettings-desktop-schemas `org.gnome.desktop.wm.keybindings` (`2b991c09`);
    - gnome-settings-daemon `org.gnome.settings-daemon.plugins.media-keys` (`bf7df434`);
    - gnome-shell `org.gnome.shell.keybindings` (`fae3b090`);
    - mutter `org.gnome.mutter` and `org.gnome.mutter.wayland` (`e0666ab5`).
  - Ubuntu's additions (`Ctrl+Alt+T`, `Ctrl+Alt+D`):
    [gnome-settings-daemon packaging](https://git.launchpad.net/ubuntu/+source/gnome-settings-daemon) and the
    ubuntu-settings gsettings override.
- KDE:
  - [Common keyboard shortcuts](https://docs.kde.org/stable_kf6/en/khelpcenter/fundamentals/kbd.html). It is older
    than the code, so the defaults were also read in the source on each project's master branch, 2026-10-01:
    - kwin `src/useractions.cpp`;
    - plasma-workspace `startkde/session-shortcuts/main.cpp`;
    - plasma-pa `src/kded/audioshortcutsservice.cpp`;
    - konsole `desktop/org.kde.konsole.desktop` and `src/session/SessionController.cpp`;
    - spectacle, plasma-desktop and kscreenlocker.

Not checked: Orca (GNOME's screen reader), the X11 `Ctrl+Alt+Backspace` option, Ubuntu Dock's own schema, and
distributions' own KDE defaults.

## Desktop platform boundary

`platform::application` (`configure`, `prepare`, `activate`, `handle_event`, `report`) owns single-instance setup, native launch policy, activation, reopen and diagnostic
observations. The common shell still shows/restores/focuses the room and keeps it alive when closed. macOS retains
its Regular activation policy, visible Dock tile, macOS 14+ activation and Dock reopen handling. Windows/Linux use
Tauri window focus and the existing single-instance plugin. No native accessibility equivalent is inferred there.

`platform::shortcut_access` owns the maintained global-shortcut plugin, live VoiceOver reservation and label style.
The saved default and alternatives above are unchanged; [#10](https://github.com/sidevoice/sidevoice-desktop/issues/10)
remains the operator's decision. A backend failure leaves the app running and reports `unavailable` through
`host.muteShortcut`; an individual registration conflict remains `refused`. An empty shortcut remains `off`.

The plugin's [global-hotkey backend](https://docs.rs/global-hotkey/0.8.0/global_hotkey/) supports Windows, macOS and
Linux X11. Native Wayland reports `unavailable`, including sessions that also expose an XWayland `DISPLAY`; GTK's
actual display backend is checked. X11 registration remains subject to the window manager's restrictions.
Windows Magnifier/AltGr and Linux accessibility conflicts listed above remain unobserved, rather than reported safe.

Runtime verification for this extraction is pending on macOS, Windows and Linux X11/Wayland. Cross-compilation
checks the interface and portable tests check unavailable/disabled states and labels; neither establishes native
focus, key delivery or assistive-technology behavior.
