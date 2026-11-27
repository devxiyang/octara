// PID-scoped driver for the Shader Lab input-pressure fixture.
import AppKit
import ApplicationServices
import Carbon
import Foundation
import QuartzCore

struct ProbeError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

struct VisibleWindow {
    let frame: CGRect
    let number: Int64
    let pid: pid_t
    let layer: Int
}

func visibleWindows() -> [VisibleWindow] {
    let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    return windows.compactMap { entry in
        guard let bounds = entry[kCGWindowBounds as String] as? [String: Any], let frame = CGRect(dictionaryRepresentation: bounds as CFDictionary),
              let number = entry[kCGWindowNumber as String] as? Int64, let pid = entry[kCGWindowOwnerPID as String] as? Int32,
              let layer = entry[kCGWindowLayer as String] as? Int, (entry[kCGWindowAlpha as String] as? Double ?? 1) > 0 else { return nil }
        return VisibleWindow(frame: frame, number: number, pid: pid, layer: layer)
    }
}

func window(_ pid: pid_t) throws -> VisibleWindow {
    guard let window = visibleWindows().first(where: { $0.pid == pid && $0.layer == 0 && $0.frame.width >= 800 && $0.frame.height >= 620 }) else {
        throw ProbeError("The test process has no onscreen Shader Lab window")
    }
    return window
}

func resize(_ pid: pid_t, _ width: Double, _ height: Double) throws {
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(app, 0.5)
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value) == .success,
          let windows = value as? [AXUIElement], let window = windows.first(where: { window in
              var title: CFTypeRef?
              return AXUIElementCopyAttributeValue(window, kAXTitleAttribute as CFString, &title) == .success
                  && title as? String == "Octara Shader Lab Input Pressure"
          }) else {
        throw ProbeError("Could not identify the single native test window for resizing")
    }
    var requested = CGSize(width: width, height: height)
    guard let size = AXValueCreate(.cgSize, &requested), AXUIElementSetAttributeValue(window, kAXSizeAttribute as CFString, size) == .success else {
        throw ProbeError("Native resize failed")
    }
}

func source(_ id: String) throws {
    let filters = [kTISPropertyInputSourceID as String: id] as CFDictionary
    let matching = TISCreateInputSourceList(filters, false).takeRetainedValue() as NSArray
    guard matching.count == 1 else { throw ProbeError("The requested input source is not enabled: \(id)") }
    let source = unsafeBitCast(matching[0] as AnyObject, to: TISInputSource.self)
    guard TISSelectInputSource(source) == noErr else { throw ProbeError("Could not select input source: \(id)") }
}

func accessibilityHit(_ point: CGPoint, pid: pid_t) -> Bool {
    let system = AXUIElementCreateSystemWide()
    AXUIElementSetMessagingTimeout(system, 0.15)
    var hit: AXUIElement?
    var owner: pid_t = 0
    return AXUIElementCopyElementAtPosition(system, Float(point.x), Float(point.y), &hit) == .success
        && hit.map { AXUIElementGetPid($0, &owner) == .success && owner == pid } == true
}

struct Request: Decodable {
    let op: String
    let point: [Double]?
    let code: UInt16?
    let shift: Bool?
    let delta: Int32?
}

@MainActor
func post(_ request: Request, pid: pid_t, number: Int64, source: CGEventSource, press: inout Int64, cursor: inout CGPoint?) throws -> Double {
    let event: CGEvent?
    var keyUp: CGEvent?
    if request.op == "key", let code = request.code {
        guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else {
            throw ProbeError("The test window lost keyboard focus")
        }
        event = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: true)
        keyUp = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: false)
        event?.flags = request.shift == true ? .maskShift : []
        keyUp?.flags = []
    } else if let position = request.point, position.count == 2, position.allSatisfy(\.isFinite) {
        let point = CGPoint(x: position[0], y: position[1])
        // Send mouse events through the session only when the test window will receive them.
        let unflipped = CGPoint(x: point.x, y: CGDisplayBounds(CGMainDisplayID()).height - point.y)
        let windowAtPoint = NSWindow.windowNumber(at: unflipped, belowWindowWithWindowNumber: 0)
        let active = NSWorkspace.shared.frontmostApplication?.processIdentifier == pid
        let foreground = visibleWindows().first { $0.frame.contains(point) }
        guard active, try window(pid).frame.contains(point),
              windowAtPoint == Int(number) || foreground?.pid == pid || accessibilityHit(point, pid: pid) else {
            throw ProbeError("Mouse location did not hit the test window (active=\(active), foremost layer=\(foreground?.layer ?? -1), owned=\(foreground?.pid == pid))")
        }
        if request.op == "wheel", let delta = request.delta {
            event = CGEvent(scrollWheelEvent2Source: source, units: .pixel, wheelCount: 1, wheel1: delta, wheel2: 0, wheel3: 0)
            event?.location = point
        } else {
            let types: [String: CGEventType] = ["move": .mouseMoved, "down": .leftMouseDown, "up": .leftMouseUp, "drag": .leftMouseDragged]
            guard let type = types[request.op] else { throw ProbeError("Unknown native input operation") }
            event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: .left)
            event?.setIntegerValueField(.mouseEventClickState, value: 1)
            if type == .leftMouseDown { press += 1 }
            event?.setIntegerValueField(.mouseEventNumber, value: press)
            if let cursor {
                event?.setIntegerValueField(.mouseEventDeltaX, value: Int64((point.x - cursor.x).rounded()))
                event?.setIntegerValueField(.mouseEventDeltaY, value: Int64((point.y - cursor.y).rounded()))
            }
            cursor = point
        }
    } else {
        throw ProbeError("Missing input location or key code")
    }
    guard let event else { throw ProbeError("Could not create a system input event") }
    let sent = CACurrentMediaTime()
    event.post(tap: .cghidEventTap)
    keyUp?.post(tap: .cghidEventTap)
    return sent
}

func write(_ value: [String: Any]) throws {
    var data = try JSONSerialization.data(withJSONObject: value)
    data.append(10)
    try FileHandle.standardOutput.write(contentsOf: data)
}

@MainActor
func run() throws {
    guard CommandLine.arguments.count == 2, let pid = pid_t(CommandLine.arguments[1]), pid > 1,
          CGPreflightPostEventAccess(), let events = CGEventSource(stateID: .hidSystemState) else {
        throw ProbeError("Provide the Shader Lab process ID and grant native event posting access")
    }
    let number = try window(pid).number
    NSApplication.shared.setActivationPolicy(.prohibited)
    let original = TISCopyCurrentKeyboardInputSource()?.takeRetainedValue()
    let pointer = CGEvent(source: nil)?.location
    var cursor = pointer
    var press: Int64 = 1
    defer {
        if let original {
            TISSelectInputSource(original)
        }
        if let pointer, NSWorkspace.shared.frontmostApplication?.processIdentifier == pid {
            CGWarpMouseCursorPosition(pointer)
        }
    }
    while let line = readLine() {
        do {
            let request = try JSONDecoder().decode(Request.self, from: Data(line.utf8))
            switch request.op {
            case "bounds":
                let current = try window(pid)
                guard number == current.number else { throw ProbeError("The native test window was replaced") }
                let frame = current.frame
                try write(["bounds": [frame.minX, frame.minY, frame.width, frame.height],
                           "active": NSWorkspace.shared.frontmostApplication?.processIdentifier == pid])
            case "resize":
                guard let size = request.point, size.count == 2 else { throw ProbeError("Missing frame dimensions") }
                let sent = CACurrentMediaTime()
                try resize(pid, size[0], size[1])
                try write(["sent_s": sent])
            case "latin", "pinyin":
                try source(request.op == "latin" ? "com.apple.keylayout.ABC" : "com.apple.inputmethod.SCIM.ITABC")
                try write(["ok": true])
            default:
                try write(["sent_s": try post(request, pid: pid, number: number, source: events, press: &press, cursor: &cursor)])
            }
        } catch {
            try write(["error": String(describing: error)])
        }
    }
}

do {
    try run()
} catch {
    try write(["error": String(describing: error)])
    exit(1)
}
