// macOS 14+. Build with swiftc -swift-version 6 -parse-as-library.
import AppKit
import Metal
import QuartzCore

// The layer stays attached and configured for the duration of the link. The worker reads it
// through CAMetalDisplayLink; only the main thread touches its view and window.
private struct LayerHandle: @unchecked Sendable {
    let layer: CAMetalLayer
}

// The worker hands the callback's retained drawable and timing to one main-thread consumer.
private struct DrawableUpdate: @unchecked Sendable {
    let update: CAMetalDisplayLink.Update
}

private final class LinkWorker: @unchecked Sendable {
    private let lock = NSLock()
    private var active = true
    private var posted = false

    var running: Bool { lock.withLock { active } }

    func stop() {
        lock.withLock { active = false }
    }

    func reserve() -> Bool {
        lock.withLock {
            guard active, !posted else { return false }
            posted = true
            return true
        }
    }

    func consume() {
        lock.withLock { posted = false }
    }
}

private final class LinkDelegate: NSObject, CAMetalDisplayLinkDelegate {
    let receive: (CAMetalDisplayLink.Update) -> Void

    init(receive: @escaping (CAMetalDisplayLink.Update) -> Void) { self.receive = receive }

    func metalDisplayLink(_ link: CAMetalDisplayLink, needsUpdate update: CAMetalDisplayLink.Update) {
        receive(update)
    }
}

private func displayLink(_ layer: CAMetalLayer, delegate: any CAMetalDisplayLinkDelegate) -> CAMetalDisplayLink {
    let link = CAMetalDisplayLink(metalLayer: layer)
    link.delegate = delegate
    link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 60, preferred: 60)
    link.preferredFrameLatency = 1
    return link
}

@MainActor
final class DisplayProbe: NSObject, @preconcurrency CAMetalDisplayLinkDelegate {
    let window: NSWindow
    let layer = CAMetalLayer()
    let queue: MTLCommandQueue
    var link: CAMetalDisplayLink?
    private var worker: LinkWorker?
    var held: [any CAMetalDrawable] = []
    var callbacks = 0
    var peakHeld = 0
    var afterPressure = 0
    var pressureReleased = false
    var dispatchDelays: [Double] = []
    var dispatchesUnderPressure = 0
    var presents: [Double] = []
    var renderBudgetMs: Double?

    override init() {
        let device = MTLCreateSystemDefaultDevice()!
        queue = device.makeCommandQueue()!
        window = NSWindow(contentRect: NSRect(x: 180, y: 180, width: 440, height: 280),
                          styleMask: [.titled, .closable], backing: .buffered, defer: false)
        super.init()
        window.title = "Octara Metal Display Link Probe"
        window.isReleasedWhenClosed = false
        let view = NSView(frame: window.contentView!.bounds)
        window.contentView = view
        view.wantsLayer = true
        layer.device = device
        layer.pixelFormat = .bgra8Unorm
        layer.frame = view.bounds
        layer.contentsScale = window.backingScaleFactor
        layer.drawableSize = CGSize(width: 440 * window.backingScaleFactor, height: 280 * window.backingScaleFactor)
        layer.maximumDrawableCount = 3
        layer.allowsNextDrawableTimeout = true
        view.layer!.addSublayer(layer)
    }

    func start(background: Bool) {
        window.makeKeyAndOrderFront(nil)
        NSApp.activate()
        if background {
            startWorker()
        } else {
            let link = displayLink(layer, delegate: self)
            self.link = link
            link.add(to: .main, forMode: .common)
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
            if let worker = self.worker {
                worker.stop()
            } else {
                self.link?.invalidate()
                self.finish()
            }
        }
    }

    private func startWorker() {
        let source = LayerHandle(layer: layer)
        let worker = LinkWorker()
        self.worker = worker
        let thread = Thread {
            let delegate = LinkDelegate { update in
                if worker.reserve() {
                    let frame = DrawableUpdate(update: update)
                    DispatchQueue.main.async {
                        defer { worker.consume() }
                        if worker.running {
                            self.draw(frame.update)
                        }
                    }
                }
            }
            let link = displayLink(source.layer, delegate: delegate)
            let loop = RunLoop.current
            link.add(to: loop, forMode: .default)
            withExtendedLifetime(delegate) {
                while worker.running {
                    autoreleasepool { _ = loop.run(mode: .default, before: Date(timeIntervalSinceNow: 0.05)) }
                }
            }
            link.invalidate()
            DispatchQueue.main.async { self.finish() }
        }
        thread.qualityOfService = .userInteractive
        thread.start()
    }

    func metalDisplayLink(_ link: CAMetalDisplayLink, needsUpdate update: CAMetalDisplayLink.Update) {
        draw(update)
    }

    private func draw(_ update: CAMetalDisplayLink.Update) {
        callbacks += 1
        if renderBudgetMs == nil {
            renderBudgetMs = (update.targetPresentationTimestamp - update.targetTimestamp) * 1000
        }
        let drawable = update.drawable
        if callbacks >= 12, !pressureReleased {
            held.append(drawable)
            peakHeld = max(peakHeld, held.count)
            if callbacks == 12 {
                for sample in 1...24 {
                    DispatchQueue.global(qos: .userInteractive).asyncAfter(deadline: .now() + .milliseconds(10 * sample)) {
                        let sent = CACurrentMediaTime()
                        DispatchQueue.main.async {
                            self.dispatchDelays.append((CACurrentMediaTime() - sent) * 1000)
                            if !self.pressureReleased { self.dispatchesUnderPressure += 1 }
                        }
                    }
                }
                DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(260)) {
                    self.pressureReleased = true
                    self.held.removeAll()
                }
            }
            return
        }
        if pressureReleased {
            afterPressure += 1
        }
        let pass = MTLRenderPassDescriptor()
        let color = pass.colorAttachments[0]!
        color.texture = drawable.texture
        color.loadAction = .clear
        color.storeAction = .store
        color.clearColor = MTLClearColorMake(0.08, 0.2, pressureReleased ? 0.65 : 0.35, 1)
        guard let buffer = queue.makeCommandBuffer(), let encoder = buffer.makeRenderCommandEncoder(descriptor: pass) else {
            return
        }
        encoder.endEncoding()
        drawable.addPresentedHandler { result in
            let displayedAt = result.presentedTime
            DispatchQueue.main.async {
                if displayedAt > 0 {
                    self.presents.append(displayedAt)
                }
            }
        }
        buffer.commit()
        drawable.present()
    }

    func finish() {
        window.close()
        let ordered = presents.sorted()
        let gaps = zip(ordered, ordered.dropFirst()).map { ($1 - $0) * 1000 }
        let medianGap = gaps.sorted().dropFirst(gaps.count / 2).first ?? 0
        print("mode=\(worker == nil ? "main" : "worker") callbacks=\(callbacks) held_peak=\(peakHeld) resumed=\(afterPressure) physical_presents=\(presents.count) median_gap_ms=\(medianGap) main_dispatch_samples=\(dispatchDelays.count) max_dispatch_ms=\(dispatchDelays.max() ?? 0)")
        print("dispatches_under_pressure=\(dispatchesUnderPressure) render_deadline_to_target_ms=\(renderBudgetMs ?? 0)")
        let ok = peakHeld == 3 && afterPressure > 0 && presents.count > 11
            && dispatchDelays.count == 24 && dispatchesUnderPressure == 24
        exit(ok ? 0 : 1)
    }
}

@main
struct Probe {
    @MainActor static func main() {
        let mode = CommandLine.arguments.dropFirst().first ?? "main"
        guard CommandLine.arguments.count <= 2, mode == "main" || mode == "worker" else {
            FileHandle.standardError.write(Data("Usage: metal-display-link [main|worker]\n".utf8))
            exit(2)
        }
        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        let probe = DisplayProbe()
        DispatchQueue.main.async { probe.start(background: mode == "worker") }
        app.run()
    }
}
