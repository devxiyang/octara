// Native validation helper: operate only on the explicitly supplied test process.
// Build: swiftc examples/live_window.swift -o /tmp/octara-live-window
import AppKit
import ApplicationServices

let arguments = CommandLine.arguments
guard arguments.count >= 3,
      let pid = Int32(arguments[2]), pid > 1,
      let app = NSRunningApplication(processIdentifier: pid) else { exit(2) }
let operation = arguments[1]
guard arguments.count == (operation == "size" ? 5 : 3) else { exit(2) }
switch operation {
case "activate":
    guard app.activate(options: [.activateAllWindows]) else { exit(3) }
case "close", "size":
    guard AXIsProcessTrusted() else {
        fputs("Accessibility access unavailable\n", stderr)
        exit(4)
    }
    let application = AXUIElementCreateApplication(pid)
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(application, kAXWindowsAttribute as CFString, &value) == .success,
          let windows = value as? [AXUIElement] else { exit(5) }
    if operation == "size" {
        guard let width = Double(arguments[3]), let height = Double(arguments[4]),
              width.isFinite, height.isFinite, width > 0, height > 0 else { exit(2) }
        let resizable = windows.filter { window in
            var settable = DarwinBoolean(false)
            return AXUIElementIsAttributeSettable(window, kAXSizeAttribute as CFString, &settable) == .success && settable.boolValue
        }
        guard resizable.count == 1 else {
            fputs("Expected one resizable native window for the test process\n", stderr)
            exit(7)
        }
        var requested = CGSize(width: width, height: height)
        guard let size = AXValueCreate(.cgSize, &requested),
              AXUIElementSetAttributeValue(resizable[0], kAXSizeAttribute as CFString, size) == .success else {
            fputs("Could not resize the native test window\n", stderr)
            exit(6)
        }
        exit(0)
    }
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
