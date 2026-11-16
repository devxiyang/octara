// Native validation helper: operate only on the explicitly supplied test process.
// Build: swiftc examples/live_window.swift -o /tmp/octara-live-window
import AppKit
import ApplicationServices

guard CommandLine.arguments.count == 3,
      let pid = Int32(CommandLine.arguments[2]),
      let app = NSRunningApplication(processIdentifier: pid) else { exit(2) }
switch CommandLine.arguments[1] {
case "activate":
    guard app.activate(options: [.activateAllWindows]) else { exit(3) }
case "close":
    guard AXIsProcessTrusted() else {
        fputs("Accessibility access unavailable\n", stderr)
        exit(4)
    }
    let application = AXUIElementCreateApplication(pid)
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(application, kAXWindowsAttribute as CFString, &value) == .success,
          let windows = value as? [AXUIElement] else { exit(5) }
    // AccessKit may also expose an application-root entry; use only a native close button.
    for window in windows {
        var button: CFTypeRef?
        if AXUIElementCopyAttributeValue(window, kAXCloseButtonAttribute as CFString, &button) == .success,
           let button = button {
            let result = AXUIElementPerformAction(unsafeBitCast(button, to: AXUIElement.self), kAXPressAction as CFString)
            exit(result == .success ? 0 : 6)
        }
    }
    fputs("No accessible native close button; check that the desktop is unlocked\n", stderr)
    exit(7)
default:
    exit(2)
}
