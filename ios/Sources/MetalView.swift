import UIKit
import Metal
import QuartzCore

/// Metal rendering view hosting WordCraft's GPU surface and touch/pencil/pointer input pipeline.
final class MetalView: UIView, UIGestureRecognizerDelegate, UIPointerInteractionDelegate {
    private let hostPointer: OpaquePointer
    private var displayLink: CADisplayLink?
    private weak var scribbleCoordinator: ScribbleAndPencilCoordinator?

    override class var layerClass: AnyClass {
        return CAMetalLayer.self
    }

    var metalLayer: CAMetalLayer {
        return layer as! CAMetalLayer
    }

    init(frame: CGRect, hostPointer: OpaquePointer, scribbleCoordinator: ScribbleAndPencilCoordinator?) {
        self.hostPointer = hostPointer
        self.scribbleCoordinator = scribbleCoordinator
        super.init(frame: frame)

        isMultipleTouchEnabled = true
        backgroundColor = .systemBackground

        // Configure CAMetalLayer
        metalLayer.device = MTLCreateSystemDefaultDevice()
        metalLayer.pixelFormat = .bgra8Unorm
        metalLayer.framebufferOnly = false
        metalLayer.contentsScale = UIScreen.main.scale

        // Register Metal surface with WordCraft host
        let layerPtr = Unmanaged.passUnretained(metalLayer).toOpaque()
        wordcraft_set_surface(hostPointer, layerPtr)

        setupGestures()
        setupPointerInteraction()
        setupDisplayLink()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    deinit {
        displayLink?.invalidate()
    }

    // MARK: - Display Link Render Loop

    private func setupDisplayLink() {
        let link = CADisplayLink(target: self, selector: #selector(renderLoop))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
        link.add(to: .main, forMode: .common)
        self.displayLink = link
    }

    @objc private func renderLoop() {
        _ = wordcraft_render_frame(hostPointer)
    }

    // MARK: - Layout & Resize

    override func layoutSubviews() {
        super.layoutSubviews()
        let scale = Float(window?.windowScene?.screen.scale ?? UIScreen.main.scale)
        let w = Float(bounds.width)
        let h = Float(bounds.height)
        metalLayer.drawableSize = CGSize(width: bounds.width * CGFloat(scale), height: bounds.height * CGFloat(scale))
        wordcraft_resize(hostPointer, w, h, scale)
    }

    // MARK: - Multi-Touch & Apple Pencil Input

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        forwardTouches(touches, phase: 0) // 0: Began
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        forwardTouches(touches, phase: 1) // 1: Moved
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        forwardTouches(touches, phase: 3) // 3: Ended
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        forwardTouches(touches, phase: 4) // 4: Cancelled
    }

    private func forwardTouches(_ touches: Set<UITouch>, phase: Int32) {
        for touch in touches {
            let loc = touch.location(in: self)
            let touchId = UInt64(touch.hash)
            let force = Float(touch.force)

            let pointerType: Int32
            switch touch.type {
            case .pencil:
                pointerType = 1 // Apple Pencil
                handlePencilTouch(touch, at: loc, phase: phase)
            case .indirectPointer, .indirect:
                pointerType = 2 // External Mouse/Trackpad
            default:
                pointerType = 0 // Direct Touch
            }

            wordcraft_send_touch(
                hostPointer,
                phase,
                Float(loc.x),
                Float(loc.y),
                touchId,
                force,
                pointerType
            )
        }
    }

    private func handlePencilTouch(_ touch: UITouch, at loc: CGPoint, phase: Int32) {
        guard let coordinator = scribbleCoordinator else { return }
        switch phase {
        case 0:
            coordinator.handlePencilBegan(at: loc, force: touch.force, page: 1)
        case 1:
            coordinator.handlePencilMoved(to: loc, force: touch.force)
        case 3:
            coordinator.handlePencilEnded(at: loc)
        default:
            coordinator.handlePencilCancelled()
        }
    }

    // MARK: - Trackpad & Pointer Hover (UIPointerInteraction)

    private func setupPointerInteraction() {
        let interaction = UIPointerInteraction(delegate: self)
        addInteraction(interaction)
    }

    func pointerInteraction(_ interaction: UIPointerInteraction, regionFor request: UIPointerRegionRequest, defaultRegion: UIPointerRegion) -> UIPointerRegion? {
        let loc = request.location
        wordcraft_send_mouse(hostPointer, Float(loc.x), Float(loc.y), -1, false, 0, 0)
        return defaultRegion
    }

    // MARK: - Native Gesture Recognizers

    private func setupGestures() {
        // Pinch-to-zoom (2 fingers)
        let pinch = UIPinchGestureRecognizer(target: self, action: #selector(handlePinch(_:)))
        pinch.delegate = self
        addGestureRecognizer(pinch)

        // Inertial pan / scroll (2 fingers)
        let pan = UIPanGestureRecognizer(target: self, action: #selector(handlePan(_:)))
        pan.minimumNumberOfTouches = 2
        pan.maximumNumberOfTouches = 2
        pan.delegate = self
        addGestureRecognizer(pan)

        // Double-tap to toggle Fit Page Width / 100% zoom
        let doubleTap = UITapGestureRecognizer(target: self, action: #selector(handleDoubleTap(_:)))
        doubleTap.numberOfTapsRequired = 2
        doubleTap.numberOfTouchesRequired = 1
        doubleTap.delegate = self
        addGestureRecognizer(doubleTap)

        // Two-finger tap for Context Menu
        let twoFingerTap = UITapGestureRecognizer(target: self, action: #selector(handleTwoFingerTap(_:)))
        twoFingerTap.numberOfTouchesRequired = 2
        twoFingerTap.numberOfTapsRequired = 1
        twoFingerTap.delegate = self
        addGestureRecognizer(twoFingerTap)

        // Three-finger swipe left (Undo)
        let swipeLeft = UISwipeGestureRecognizer(target: self, action: #selector(handleSwipeLeft(_:)))
        swipeLeft.numberOfTouchesRequired = 3
        swipeLeft.direction = .left
        swipeLeft.delegate = self
        addGestureRecognizer(swipeLeft)

        // Three-finger swipe right (Redo)
        let swipeRight = UISwipeGestureRecognizer(target: self, action: #selector(handleSwipeRight(_:)))
        swipeRight.numberOfTouchesRequired = 3
        swipeRight.direction = .right
        swipeRight.delegate = self
        addGestureRecognizer(swipeRight)
    }

    @objc private func handlePinch(_ recognizer: UIPinchGestureRecognizer) {
        let scale = Float(recognizer.scale)
        let velocity = Float(recognizer.velocity)
        wordcraft_send_pinch_gesture(hostPointer, scale, velocity)
        recognizer.scale = 1.0
    }

    @objc private func handlePan(_ recognizer: UIPanGestureRecognizer) {
        let translation = recognizer.translation(in: self)
        wordcraft_send_pan_gesture(hostPointer, Float(translation.x), Float(translation.y))
        recognizer.setTranslation(.zero, in: self)
    }

    @objc private func handleDoubleTap(_ recognizer: UITapGestureRecognizer) {
        let loc = recognizer.location(in: self)
        wordcraft_send_double_tap(hostPointer, Float(loc.x), Float(loc.y))
    }

    @objc private func handleTwoFingerTap(_ recognizer: UITapGestureRecognizer) {
        let loc = recognizer.location(in: self)
        wordcraft_send_context_menu(hostPointer, Float(loc.x), Float(loc.y))
    }

    @objc private func handleSwipeLeft(_ recognizer: UISwipeGestureRecognizer) {
        wordcraft_send_three_finger_swipe(hostPointer, 0) // 0: Undo
    }

    @objc private func handleSwipeRight(_ recognizer: UISwipeGestureRecognizer) {
        wordcraft_send_three_finger_swipe(hostPointer, 1) // 1: Redo
    }

    // MARK: - UIGestureRecognizerDelegate

    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith otherGestureRecognizer: UIGestureRecognizer) -> Bool {
        // Allow pinch and pan to coexist smoothly
        if (gestureRecognizer is UIPinchGestureRecognizer && otherGestureRecognizer is UIPanGestureRecognizer) ||
           (gestureRecognizer is UIPanGestureRecognizer && otherGestureRecognizer is UIPinchGestureRecognizer) {
            return true
        }
        return false
    }
}
