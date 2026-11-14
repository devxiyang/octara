#!/usr/bin/env swift
// Read-only, PID-scoped observation of an already running candidate-window host.
// Prepare/review this helper before use; run it only when native observation is authorized.
// It never launches/activates apps, sends input, performs AX actions, or requests permissions.
// Usage: swift examples/ime_candidates.swift --pid CANDIDATE_HOST_PID --output /new/evidence/dir
// IMKit may host the visible remote candidate panel in the client app, not the IME service.
// Optional: --include-offscreen-metadata (metadata only; offscreen windows are never captured).

import AppKit
import ApplicationServices
import CoreGraphics
import Darwin
import Foundation
import ImageIO

struct ObservationError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

struct Options {
    let pid: pid_t
    let output: URL
    let includeOffscreenMetadata: Bool

    static func parse() throws -> Options {
        var args = Array(CommandLine.arguments.dropFirst())
        if args == ["--help"] {
            print("Usage: ime_candidates.swift --pid CANDIDATE_HOST_PID --output /new/evidence/dir [--include-offscreen-metadata]")
            exit(0)
        }
        var pid: pid_t?
        var output: URL?
        var includeOffscreenMetadata = false
        while !args.isEmpty {
            let flag = args.removeFirst()
            if flag == "--include-offscreen-metadata" {
                guard !includeOffscreenMetadata else { throw ObservationError("Duplicate --include-offscreen-metadata") }
                includeOffscreenMetadata = true
                continue
            }
            guard !args.isEmpty else { throw ObservationError("Missing value for \(flag)") }
            let value = args.removeFirst()
            switch flag {
            case "--pid":
                guard pid == nil, let number = Int32(value), number > 1 else {
                    throw ObservationError("--pid must occur once and name an existing candidate-window host PID > 1")
                }
                pid = number
            case "--output":
                guard output == nil, value.hasPrefix("/"), value.utf8.count < 4096 else {
                    throw ObservationError("--output must occur once and be an absolute local path")
                }
                output = URL(fileURLWithPath: value, isDirectory: true).standardizedFileURL
            default:
                throw ObservationError("Unknown option: \(flag)")
            }
        }
        guard let pid, let output, output.path != "/" else {
            throw ObservationError("Both --pid and --output are required")
        }
        guard kill(pid, 0) == 0 || errno == EPERM else {
            throw ObservationError("The requested PID is not running")
        }
        return Options(pid: pid, output: output, includeOffscreenMetadata: includeOffscreenMetadata)
    }
}

let maxWindows = 8
let maxNodes = 256
let maxDepth = 10
let maxChildren = 64
let maxText = 1024
let maxPNGBytes = 8 * 1024 * 1024
let maxJSONBytes = 4 * 1024 * 1024

func rectJSON(_ rect: CGRect) -> [String: Double] {
    ["x": rect.minX, "y": rect.minY, "width": rect.width, "height": rect.height]
}

func validRect(_ rect: CGRect) -> Bool {
    [rect.minX, rect.minY, rect.width, rect.height].allSatisfy(\.isFinite)
        && rect.width > 0 && rect.height > 0
        && rect.width <= 4096 && rect.height <= 4096
        && rect.width * rect.height <= 4_000_000
}

struct WindowInfo {
    let id: CGWindowID
    let pid: pid_t
    let bounds: CGRect
    let owner: String
    let layer: Int
    let alpha: Double
    let isOnscreen: Bool

    var eligibleForCapture: Bool { isOnscreen && validRect(bounds) && alpha.isFinite && alpha > 0 }

    var json: [String: Any] {
        let finiteBounds = [bounds.minX, bounds.minY, bounds.width, bounds.height].allSatisfy(\.isFinite)
        return ["id": id, "owner_pid": pid, "owner": owner,
                "bounds": finiteBounds ? rectJSON(bounds) as Any : NSNull(), "layer": layer,
                "alpha": alpha.isFinite ? alpha as Any : NSNull(), "is_onscreen": isOnscreen,
                "eligible_for_capture": eligibleForCapture]
    }
}

// WindowServer has no PID-filtered enumeration API. Inspect only ownership before retaining
// these metadata fields; no other process's titles, AX trees or pixels are retained or emitted.
func targetWindows(pid: pid_t, only id: CGWindowID? = nil, includeOffscreen: Bool = false) throws -> [WindowInfo] {
    let flags: CGWindowListOption
    if id != nil { flags = [.optionIncludingWindow] }
    else if includeOffscreen { flags = [.optionAll, .excludeDesktopElements] }
    else { flags = [.optionOnScreenOnly, .excludeDesktopElements] }
    guard let records = CGWindowListCopyWindowInfo(flags, id ?? kCGNullWindowID) as? [[String: Any]] else {
        throw ObservationError("WindowServer returned no window metadata")
    }
    return records.compactMap { record in
        guard (record[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid,
              let number = record[kCGWindowNumber as String] as? NSNumber else { return nil }
        let rawBounds = record[kCGWindowBounds as String] as? [String: Any]
        let bounds = rawBounds.flatMap { CGRect(dictionaryRepresentation: $0 as CFDictionary) } ?? .null
        let alpha = (record[kCGWindowAlpha as String] as? NSNumber)?.doubleValue ?? 1
        return WindowInfo(id: number.uint32Value, pid: pid, bounds: bounds,
                          owner: String((record[kCGWindowOwnerName as String] as? String ?? "").prefix(maxText)),
                          layer: (record[kCGWindowLayer as String] as? NSNumber)?.intValue ?? 0,
                          alpha: alpha, isOnscreen: record[kCGWindowIsOnscreen as String] as? Bool ?? false)
    }
}

final class AXReader {
    let pid: pid_t
    let deadline = Date().addingTimeInterval(5)
    var visited: [AXUIElement] = []
    var truncated = false
    var errors: [String] = []

    init(pid: pid_t) { self.pid = pid }

    func value(_ node: AXUIElement, _ attribute: String) -> CFTypeRef? {
        guard Date() < deadline else { truncated = true; return nil }
        var value: CFTypeRef?
        let status = AXUIElementCopyAttributeValue(node, attribute as CFString, &value)
        if status != .success && status != .attributeUnsupported && status != .noValue && errors.count < 16 {
            errors.append("\(attribute): AXError \(status.rawValue)")
        }
        return status == .success ? value : nil
    }

    func elements(_ node: AXUIElement, _ attribute: String, limit: Int) -> [AXUIElement] {
        guard Date() < deadline else { truncated = true; return [] }
        var count = 0
        let status = AXUIElementGetAttributeValueCount(node, attribute as CFString, &count)
        guard status == .success else {
            if status != .attributeUnsupported && status != .noValue && errors.count < 16 {
                errors.append("\(attribute): AXError \(status.rawValue)")
            }
            return []
        }
        if count > limit { truncated = true }
        guard count > 0, Date() < deadline else { return [] }
        var values: CFArray?
        let copied = AXUIElementCopyAttributeValues(node, attribute as CFString, 0, min(count, limit), &values)
        guard copied == .success else {
            if errors.count < 16 { errors.append("\(attribute): AXError \(copied.rawValue)") }
            return []
        }
        return values as? [AXUIElement] ?? []
    }

    func walk(_ node: AXUIElement, depth: Int = 0) -> [String: Any]? {
        guard Date() < deadline, visited.count < maxNodes, depth <= maxDepth else {
            truncated = true
            return nil
        }
        var owner: pid_t = 0
        guard AXUIElementGetPid(node, &owner) == .success, owner == pid else { return nil }
        guard !visited.contains(where: { CFEqual($0, node) }) else { return ["repeated": true] }
        visited.append(node)
        // This changes only this client's RPC timeout; it sends no application UI mutation.
        AXUIElementSetMessagingTimeout(node, 0.15)
        var result: [String: Any] = [:]
        for attribute in ["AXRole", "AXSubrole", "AXTitle", "AXDescription"] {
            if let text = value(node, attribute) as? String {
                result[attribute] = String(text.prefix(maxText))
            }
        }
        if result["AXRole"] as? String == "AXMenuBar" {
            result["children_omitted"] = "AXMenuBar is outside candidate-window observation"
            return result
        }
        let secure = result["AXSubrole"] as? String == "AXSecureTextField"
        if secure {
            result["redacted"] = true
        } else if let raw = value(node, "AXValue") {
            if let text = raw as? String {
                result["AXValue"] = String(text.prefix(maxText))
            } else if let number = raw as? NSNumber, number.doubleValue.isFinite {
                result["AXValue"] = number
            }
        }
        for attribute in ["AXSelected", "AXEnabled"] {
            if let number = value(node, attribute) as? NSNumber { result[attribute] = number.boolValue }
        }
        if let raw = value(node, "AXPosition"), CFGetTypeID(raw) == AXValueGetTypeID() {
            let ax = unsafeBitCast(raw, to: AXValue.self)
            var point = CGPoint.zero
            if AXValueGetValue(ax, .cgPoint, &point), point.x.isFinite, point.y.isFinite {
                result["position"] = [point.x, point.y]
            }
        }
        if let raw = value(node, "AXSize"), CFGetTypeID(raw) == AXValueGetTypeID() {
            let ax = unsafeBitCast(raw, to: AXValue.self)
            var size = CGSize.zero
            if AXValueGetValue(ax, .cgSize, &size), size.width.isFinite, size.height.isFinite {
                result["size"] = [size.width, size.height]
            }
        }
        let children = elements(node, "AXChildren", limit: maxChildren)
        if !children.isEmpty { result["children"] = children.compactMap { walk($0, depth: depth + 1) } }
        return result
    }

    func snapshot() -> [String: Any] {
        let app = AXUIElementCreateApplication(pid)
        AXUIElementSetMessagingTimeout(app, 0.15)
        let windows = elements(app, "AXWindows", limit: maxWindows)
        if windows.isEmpty {
            // Never recurse through an application's children as a fallback: menu agents can
            // expose unrelated system menus and recent documents there. Pixels remain PID/window scoped.
            var summary: [String: String] = [:]
            for attribute in ["AXRole", "AXTitle"] {
                if let text = value(app, attribute) as? String { summary[attribute] = String(text.prefix(maxText)) }
            }
            return ["roots": [], "root_kind": "none", "application": summary,
                    "no_eligible_ax_window": true, "nodes": 0, "truncated": truncated, "errors": errors]
        }
        let trees = windows.compactMap { walk($0) }
        return ["roots": trees, "root_kind": "windows", "no_eligible_ax_window": false,
                "nodes": visited.count, "truncated": truncated, "errors": errors]
    }
}

// The current SDK obsoletes the imported declaration. Resolve the same public symbol at
// runtime, as octara-shell's native_mac.rs does; never substitute an entire-display capture.
typealias WindowCapture = @convention(c) (CGRect, UInt32, CGWindowID, UInt32) -> Unmanaged<CGImage>?

func capture(_ window: WindowInfo, pid: pid_t, output: URL) throws -> [String: Any] {
    guard let current = try targetWindows(pid: pid, only: window.id).first,
          current.eligibleForCapture,
          current.bounds == window.bounds else {
        throw ObservationError("Window disappeared, changed bounds, or changed owner before capture")
    }
    guard let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGWindowListCreateImage") else {
        throw ObservationError("CGWindowListCreateImage is unavailable on this OS")
    }
    let create = unsafeBitCast(symbol, to: WindowCapture.self)
    // Exactly this verified window ID; ignore its shadow/frame and request best resolution.
    guard let image = create(current.bounds, 1 << 3, current.id, (1 << 0) | (1 << 3))?.takeRetainedValue() else {
        throw ObservationError("WindowServer returned no image for the requested window")
    }
    guard image.width > 0, image.height > 0, image.width <= 8192, image.height <= 8192,
          image.width * image.height <= 16_000_000 else {
        throw ObservationError("Window image exceeds the 16-megapixel capture limit")
    }
    let buffer = NSMutableData()
    guard let destination = CGImageDestinationCreateWithData(buffer, "public.png" as CFString, 1, nil) else {
        throw ObservationError("Cannot prepare the PNG encoder")
    }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination), buffer.length <= maxPNGBytes else {
        throw ObservationError("PNG encoding failed or exceeds the 8-MiB per-window limit")
    }
    let name = "window-\(window.id).png"
    try (buffer as Data).write(to: output.appendingPathComponent(name), options: .withoutOverwriting)
    return ["file": name, "width": image.width, "height": image.height,
            "bytes": buffer.length, "source": "CGWindowListCreateImage", "window_id": window.id]
}

func main() throws {
    let options = try Options.parse()
    // mkdir fails for any existing path, including symlinks. No output is ever overwritten.
    guard mkdir(options.output.path, 0o700) == 0 else {
        throw ObservationError("Output must be a new directory with an existing parent: \(String(cString: strerror(errno)))")
    }
    let axAllowed = AXIsProcessTrusted()
    let captureAllowed = CGPreflightScreenCaptureAccess()
    var errors: [String] = []
    var report: [String: Any] = [
        "schema": "octara.ime-candidates/1", "pid": options.pid,
        "observed_at": ISO8601DateFormatter().string(from: Date()),
        "permissions": ["accessibility": axAllowed, "screen_capture": captureAllowed],
        "scope": "Only the explicitly requested PID; captures are eligible on-screen windows, AX traversal starts at AXWindows only",
        "include_offscreen_metadata": options.includeOffscreenMetadata,
        "limits": ["windows": maxWindows, "ax_nodes": maxNodes, "ax_depth": maxDepth,
                   "ax_seconds": 5, "png_bytes_per_window": maxPNGBytes]
    ]
    if let app = NSRunningApplication(processIdentifier: options.pid) {
        report["application"] = ["bundle_identifier": app.bundleIdentifier ?? "",
                                 "executable": app.executableURL?.path ?? ""]
    }
    let metadata: [WindowInfo]
    do { metadata = try targetWindows(pid: options.pid, includeOffscreen: options.includeOffscreenMetadata) }
    catch { metadata = []; errors.append(String(describing: error)) }
    report["cg_windows"] = metadata.map(\.json)
    report["target_window_count"] = metadata.count
    let windows = metadata.filter(\.eligibleForCapture)
    report["eligible_window_count"] = windows.count
    report["window_limit_reached"] = windows.count > maxWindows
    var observed: [[String: Any]] = []
    for window in windows.prefix(maxWindows) {
        var entry = window.json
        if captureAllowed {
            do { entry["capture"] = try capture(window, pid: options.pid, output: options.output) }
            catch { entry["capture_error"] = String(describing: error) }
        } else {
            entry["capture_error"] = "Existing Screen Recording permission is unavailable; no request was made"
        }
        observed.append(entry)
    }
    report["windows"] = observed
    if axAllowed {
        let reader = AXReader(pid: options.pid)
        report["accessibility"] = reader.snapshot()
        if !reader.errors.isEmpty || reader.truncated { errors.append("AX observation is incomplete; see accessibility errors/truncated") }
    } else { errors.append("Existing Accessibility permission is unavailable; no request was made") }
    if !captureAllowed { errors.append("Existing Screen Recording permission is unavailable; no request was made") }
    if windows.isEmpty { errors.append("This PID has no eligible on-screen windows; no image was captured") }
    report["errors"] = errors
    let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
    guard data.count <= maxJSONBytes else { throw ObservationError("Report exceeds the 4-MiB JSON limit") }
    let reportURL = options.output.appendingPathComponent("report.json")
    try data.write(to: reportURL, options: .withoutOverwriting)
    print(reportURL.path)
    // A saved report can still contain per-window capture errors; inspect it before claiming success.
    if !errors.isEmpty || observed.contains(where: { $0["capture_error"] != nil }) { exit(1) }
}

do { try main() }
catch {
    FileHandle.standardError.write(Data("FAIL: \(error)\n".utf8))
    exit(1)
}
