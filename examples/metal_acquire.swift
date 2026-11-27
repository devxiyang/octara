// swiftc -swift-version 6 -parse-as-library examples/metal_acquire.swift -o /tmp/octara-metal-acquire
// From an unlocked desktop, run the same binary with "main" and "worker".
import AppKit
import Metal
import QuartzCore

private let inputSubtype: Int16 = 9301

private func uptime() -> Double {
    Double(DispatchTime.now().uptimeNanoseconds) / 1e9
}

// The layer's configuration is fixed before handoff. The worker only calls nextDrawable().
// The main thread owns attachment and can retire a layer instead of changing its drawable size.
private struct DrawableSource: @unchecked Sendable {
    private let layer: CAMetalLayer

    init(_ layer: CAMetalLayer) { self.layer = layer }

    func take() -> (any CAMetalDrawable)? {
        autoreleasepool { layer.nextDrawable() }
    }
}

// The acquired drawable moves to the UI thread with exclusive use for encoding and presentation.
private struct Acquisition: @unchecked Sendable {
    let started: Double
    let enteredDrawable: Double
    let drawableReturned: Double
    let returned: Double
    let drawable: (any CAMetalDrawable)?
}

private enum Mode: String { case main, worker }
private enum Phase: Int, Sendable {
    case warmupFirst, warmupSecond, input, secondWindow, resizeRace, resized, closeRace, reopened
}
private enum Action: Int, Sendable { case edit, resize, close }

private struct Event: Sendable {
    let window: Int
    let action: Action
    let delayMs: Int
}

private struct InputObservation {
    let phase: Phase
    let action: Action
    let window: Int
    let sent: Double
    let handled: Double
    let finished: Double
    var latencyMs: Double { (handled - sent) * 1000 }
}

@MainActor
private final class InputWindow: NSWindow {
    var receive: ((NSEvent) -> Void)?

    override func sendEvent(_ event: NSEvent) {
        if event.type == .applicationDefined && event.subtype.rawValue == inputSubtype {
            receive?(event)
        } else {
            super.sendEvent(event)
        }
    }
}

@MainActor
private final class Probe {
    private struct Surface {
        let window: InputWindow
        let layer: CAMetalLayer
        let generation: Int
        var revision: Int
    }

    private struct Request: Sendable {
        let phase: Phase
        let window: Int
        let nativeWindow: Int
        let generation: Int
        let revision: Int
    }

    private let mode: Mode
    private let device: any MTLDevice
    private let commands: any MTLCommandQueue
    private let producer = DispatchQueue(label: "octara.probe.input", qos: .userInteractive)
    private let worker = DispatchQueue(label: "octara.probe.acquire", qos: .userInitiated)
    private var surfaces: [Int: Surface] = [:]
    private var generation = 0
    private var acquiring: Request?
    private var inputs: [InputObservation] = []
    private var attempts: [[String: Any]] = []
    private var inputAcquisition: (started: Double, returned: Double, revision: Int)?
    private var settlingRetries: [Int: Int] = [:]

    init(mode: Mode, device: any MTLDevice, commands: any MTLCommandQueue) {
        self.mode = mode
        self.device = device
        self.commands = commands
    }

    func start() {
        open(0)
        open(1)
        NSApplication.shared.activate()
        DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(150)) { self.run(.warmupFirst, window: 0) }
        DispatchQueue.main.asyncAfter(deadline: .now() + .seconds(10)) { self.finish("timed out waiting for a drawable or display callback") }
    }

    private func open(_ index: Int) {
        let screen = NSScreen.main?.visibleFrame ?? NSRect(x: 0, y: 0, width: 800, height: 600)
        let rect = NSRect(x: screen.minX + 20 + CGFloat(index) * 365, y: screen.midY - 105, width: 300, height: 210)
        let window = InputWindow(contentRect: rect, styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.title = "Octara Metal Probe \(index + 1)"
        window.contentView = NSView(frame: NSRect(origin: .zero, size: rect.size))
        window.contentView?.wantsLayer = true
        window.receive = { [weak self] event in self?.handle(event, window: index) }
        attach(to: window, index: index, replacing: nil, revision: 0)
        window.makeKeyAndOrderFront(nil)
    }

    private func attach(to window: InputWindow, index: Int, replacing old: CAMetalLayer?, revision: Int) {
        guard let view = window.contentView, let parent = view.layer else { finish("missing window layer") }
        let layer = CAMetalLayer()
        layer.device = device
        layer.pixelFormat = .bgra8Unorm
        layer.allowsNextDrawableTimeout = true
        layer.frame = view.bounds
        layer.contentsScale = window.backingScaleFactor
        let size = view.convertToBacking(view.bounds).size
        layer.drawableSize = CGSize(width: size.width.rounded(), height: size.height.rounded())
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        old?.removeFromSuperlayer()
        parent.addSublayer(layer)
        CATransaction.commit()
        generation += 1
        surfaces[index] = Surface(window: window, layer: layer, generation: generation, revision: revision)
    }

    private func run(_ phase: Phase, window index: Int) {
        guard acquiring == nil, let surface = surfaces[index] else { finish("acquisition already in flight or window unavailable") }
        let request = Request(
            phase: phase, window: index, nativeWindow: surface.window.windowNumber,
            generation: surface.generation, revision: surface.revision
        )
        acquiring = request
        let actions: [Event]
        switch phase {
        case .input:
            actions = [
                Event(window: surfaces[0]!.window.windowNumber, action: .edit, delayMs: 2),
                Event(window: surfaces[1]!.window.windowNumber, action: .edit, delayMs: 4),
                Event(window: surfaces[0]!.window.windowNumber, action: .edit, delayMs: 6),
            ]
        case .resizeRace:
            actions = [Event(window: surface.window.windowNumber, action: .resize, delayMs: 4)]
        case .closeRace:
            actions = [Event(window: surface.window.windowNumber, action: .close, delayMs: 4)]
        default:
            actions = []
        }

        // Keep an acquired drawable until the UI confirms it has resized or replaced its
        // window. The input comparison instead injects the same delay before nextDrawable.
        let held = phase == .resizeRace || phase == .closeRace ? DispatchSemaphore(value: 0) : nil
        let sendEvents: @Sendable () -> Void = { [producer, weak self] in
            for event in actions {
                producer.asyncAfter(deadline: .now() + .milliseconds(event.delayMs)) { [weak self] in
                    let sent = uptime()
                    DispatchQueue.main.async { [weak self] in
                        self?.deliver(event, phase: phase, sent: sent)
                        held?.signal()
                    }
                }
            }
        }
        let source = DrawableSource(surface.layer)
        let take: @Sendable () -> Acquisition = {
            let started = uptime()
            if phase == .input {
                sendEvents()
                Thread.sleep(forTimeInterval: 0.020)
            }
            let enteredDrawable = uptime()
            let drawable = source.take()
            let drawableReturned = uptime()
            if let held {
                sendEvents()
                _ = held.wait(timeout: .now() + .seconds(2))
            }
            return Acquisition(
                started: started, enteredDrawable: enteredDrawable, drawableReturned: drawableReturned,
                returned: uptime(), drawable: drawable
            )
        }
        switch mode {
        case .main:
            didAcquire(take(), request: request)
        case .worker:
            worker.async { [weak self] in
                let result = take()
                DispatchQueue.main.async { [weak self] in self?.didAcquire(result, request: request) }
            }
        }
    }

    private func deliver(_ event: Event, phase: Phase, sent: Double) {
        guard let window = NSApplication.shared.window(withWindowNumber: event.window) as? InputWindow,
              let input = NSEvent.otherEvent(
                with: .applicationDefined, location: .zero, modifierFlags: [], timestamp: sent,
                windowNumber: event.window, context: nil, subtype: inputSubtype, data1: event.action.rawValue, data2: phase.rawValue
              )
        else { finish("synthetic event lost its AppKit window") }
        window.sendEvent(input)
    }

    private func handle(_ event: NSEvent, window index: Int) {
        let handled = uptime()
        guard let action = Action(rawValue: event.data1), let phase = Phase(rawValue: event.data2),
              let surface = surfaces[index], surface.window.windowNumber == event.windowNumber
        else { finish("synthetic event reached the wrong window") }
        switch action {
        case .edit:
            surfaces[index]?.revision += 1
        case .resize:
            let size = NSSize(width: 340, height: 245)
            surface.window.setContentSize(size)
            guard surface.window.contentView?.bounds.size == size else { finish("AppKit did not apply the requested content size") }
            attach(to: surface.window, index: index, replacing: surface.layer, revision: surface.revision + 1)
        case .close:
            surface.window.close()
            guard !surface.window.isVisible else { finish("AppKit left the old window visible") }
            open(index)
            guard let successor = surfaces[index], successor.window !== surface.window else { finish("missing successor window") }
        }
        inputs.append(InputObservation(
            phase: phase, action: action, window: index + 1,
            sent: event.timestamp, handled: handled, finished: uptime()
        ))
    }

    private func didAcquire(_ result: Acquisition, request: Request) {
        acquiring = nil
        let phase = request.phase
        let current = surfaces[request.window]
        let stale = current?.generation != request.generation
        var row: [String: Any] = [
            "phase": String(describing: phase), "window": request.window + 1, "native_window": request.nativeWindow,
            "generation": request.generation, "started": result.started, "returned": result.returned,
            "requested_revision": request.revision, "acquire_ms": (result.returned - result.started) * 1000,
            "next_drawable_ms": (result.drawableReturned - result.enteredDrawable) * 1000,
            "acquired": result.drawable != nil, "discarded": stale,
        ]
        if stale {
            attempts.append(row)
            guard let input = inputs.last, input.phase == phase, result.drawable != nil,
                  result.drawableReturned < input.handled, input.finished < result.returned,
                  phase == .resizeRace || phase == .closeRace
            else { finish("window change did not retire the surface during acquisition") }
            // Move to a separate turn so the stale drawable is released before the next acquisition.
            let next: Phase = phase == .resizeRace ? .resized : .reopened
            DispatchQueue.main.async { self.run(next, window: request.window) }
            return
        }
        guard phase != .resizeRace && phase != .closeRace else { finish("an obsolete surface remained current") }
        guard let current, let drawable = result.drawable, let buffer = commands.makeCommandBuffer()
        else { finish("no drawable or command buffer for \(phase)") }
        let expected = current.layer.drawableSize
        guard drawable.texture.width == Int(expected.width), drawable.texture.height == Int(expected.height)
        else { finish("drawable dimensions differ from the current surface") }
        let revision = current.revision
        row["selected_revision"] = revision
        row["pixels"] = [drawable.texture.width, drawable.texture.height]
        let rowIndex = attempts.count
        attempts.append(row)
        if phase == .input { inputAcquisition = (result.started, result.returned, revision) }

        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = drawable.texture
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].storeAction = .store
        pass.colorAttachments[0].clearColor = MTLClearColor(red: 0.15 + Double(revision) * 0.2, green: 0.25, blue: 0.65, alpha: 1)
        guard let encoder = buffer.makeRenderCommandEncoder(descriptor: pass) else { finish("could not encode a Metal clear") }
        encoder.endEncoding()
        drawable.addPresentedHandler { [weak self] shown in
            let presented = shown.presentedTime
            DispatchQueue.main.async { [weak self] in self?.didPresent(request, rowIndex: rowIndex, at: presented) }
        }
        buffer.present(drawable)
        buffer.commit()
    }

    private func didPresent(_ request: Request, rowIndex: Int, at presented: Double) {
        let phase = request.phase
        // The compositor can skip the first drawable of a layer that was just attached.
        let newLayer = phase == .warmupFirst || phase == .warmupSecond || phase == .resized || phase == .reopened
        if presented == 0, newLayer, settlingRetries[request.generation, default: 0] < 3 {
            settlingRetries[request.generation, default: 0] += 1
            attempts[rowIndex]["skipped"] = true
            DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(20)) {
                self.run(phase, window: request.window)
            }
            return
        }
        guard presented > 0 else { finish("Metal did not display the \(phase) frame") }
        attempts[rowIndex]["presented_time"] = presented
        switch phase {
        case .warmupFirst: run(.warmupSecond, window: 1)
        case .warmupSecond: run(.input, window: 0)
        case .input: run(.secondWindow, window: 1)
        case .secondWindow where mode == .worker: run(.resizeRace, window: 0)
        case .resized: run(.closeRace, window: 1)
        case .secondWindow, .reopened: validate()
        case .resizeRace, .closeRace: finish("a retired drawable was presented")
        }
    }

    private func validate() -> Never {
        guard let inputAcquisition else { finish("no acquisition for the input checkpoint") }
        let input = inputs.filter { $0.phase == .input }
        guard input.map(\.window) == [1, 2, 1], input.allSatisfy({ $0.sent >= inputAcquisition.started && $0.sent < inputAcquisition.returned })
        else { finish("independent producer missed the acquisition interval") }
        let handledDuring = input.filter { $0.handled < inputAcquisition.returned }
        if mode == .worker {
            guard handledDuring.count == 3, inputAcquisition.revision == 2 else { finish("worker acquisition did not let both inputs update the latest drawing") }
        } else {
            guard handledDuring.isEmpty, inputAcquisition.revision == 0 else { finish("main-thread acquisition did not hold queued input") }
        }
        finish(nil)
    }

    private func finish(_ error: String?) -> Never {
        let report: [String: Any] = [
            "mode": mode.rawValue, "ok": error == nil, "error": error as Any? ?? NSNull(), "attempts": attempts,
            "input": inputs.map { input in
                ["phase": String(describing: input.phase), "action": String(describing: input.action), "window": input.window, "sent": input.sent,
                 "handled": input.handled, "finished": input.finished, "latency_ms": input.latencyMs]
            },
        ]
        if let data = try? JSONSerialization.data(withJSONObject: report, options: [.sortedKeys]) {
            print(String(decoding: data, as: UTF8.self))
        }
        for surface in surfaces.values { surface.window.close() }
        exit(error == nil ? 0 : 1)
    }
}

@main
private struct Main {
    @MainActor static func main() {
        guard CommandLine.arguments.count == 2, let mode = Mode(rawValue: CommandLine.arguments[1]) else {
            FileHandle.standardError.write(Data("Usage: octara-metal-acquire main|worker\n".utf8))
            exit(2)
        }
        guard let device = MTLCreateSystemDefaultDevice(), let commands = device.makeCommandQueue() else {
            FileHandle.standardError.write(Data("Metal device unavailable\n".utf8))
            exit(1)
        }
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        let probe = Probe(mode: mode, device: device, commands: commands)
        DispatchQueue.main.async { probe.start() }
        app.run()
    }
}
