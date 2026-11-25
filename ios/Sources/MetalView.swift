import UIKit
import Metal
import QuartzCore

/// Direct CAMetalLayer view driving WordCraft's wgpu/egui surface, 1-finger touch scrolling,
/// multi-touch gestures, Apple Pencil Scribble, virtual keyboard (`UIKeyInput`),
/// external trackpad/mouse hover, and physical keyboard (`pressesBegan` / Mac chords).
class MetalView: UIView, UIGestureRecognizerDelegate, UIKeyInput, UIPointerInteractionDelegate, UIScribbleInteractionDelegate {
    var host: OpaquePointer?
    var onPlatformAction: ((String) -> Void)?

    private var displayLink: CADisplayLink?
    private var lastPanTranslation: CGPoint = .zero
    private var lastRenderTime: CFTimeInterval = 0
    private var scrollVelocity: CGPoint = .zero
    private var lastPinchScale: CGFloat = 1.0
    private var panRecognizer: UIPanGestureRecognizer?

    // Map active UITouch object pointers to stable 64-bit touch IDs for Rust
    private var touchIdMap = [ObjectIdentifier: UInt64]()
    private var nextTouchId: UInt64 = 1

    // MARK: - UITextInputTraits & UIKeyInput (Virtual Keyboard)

    override var canBecomeFirstResponder: Bool { true }

    var hasText: Bool { true }

    var autocapitalizationType: UITextAutocapitalizationType = .none
    var autocorrectionType: UITextAutocorrectionType = .no
    var spellCheckingType: UITextSpellCheckingType = .no
    var smartQuotesType: UITextSmartQuotesType = .no
    var smartDashesType: UITextSmartDashesType = .no
    var smartInsertDeleteType: UITextSmartInsertDeleteType = .no
    var keyboardType: UIKeyboardType = .default
    var returnKeyType: UIReturnKeyType = .default

    func insertText(_ text: String) {
        guard let host = host, !text.isEmpty else { return }
        if text == "\n" || text == "\r" {
            // Return key in USB HID = 40
            wordcraft_send_key(host, 40, 0, true, nil)
            wordcraft_send_key(host, 40, 0, false, nil)
        } else if text == "\t" {
            // Tab key in USB HID = 43
            wordcraft_send_key(host, 43, 0, true, nil)
            wordcraft_send_key(host, 43, 0, false, nil)
        } else {
            text.withCString { cStr in
                _ = wordcraft_insert_text_at_selection(host, cStr)
            }
        }
    }

    func deleteBackward() {
        guard let host = host else { return }
        wordcraft_send_backspace(host)
    }

    // MARK: - Layer Setup

    override class var layerClass: AnyClass {
        return CAMetalLayer.self
    }

    var metalLayer: CAMetalLayer {
        return layer as! CAMetalLayer
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        commonInit()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        commonInit()
    }

    private func commonInit() {
        isMultipleTouchEnabled = true
        contentScaleFactor = UIScreen.main.scale

        metalLayer.pixelFormat = .bgra8Unorm
        metalLayer.framebufferOnly = true
        metalLayer.contentsScale = contentScaleFactor
        metalLayer.isOpaque = true

        setupGestures()
        setupInteractions()
    }

    private func setupGestures() {
        // Two-finger pinch-to-zoom
        let pinch = UIPinchGestureRecognizer(target: self, action: #selector(handlePinch(_:)))
        pinch.delegate = self
        addGestureRecognizer(pinch)

        // One-finger pan / scroll over document and scrollable UI panels
        let pan = UIPanGestureRecognizer(target: self, action: #selector(handlePan(_:)))
        pan.minimumNumberOfTouches = 1
        pan.maximumNumberOfTouches = 1
        pan.cancelsTouchesInView = true
        pan.delegate = self
        addGestureRecognizer(pan)
        self.panRecognizer = pan

        // Three-finger swipe Left/Right -> Undo/Redo
        let swipeLeft = UISwipeGestureRecognizer(target: self, action: #selector(handleThreeFingerSwipe(_:)))
        swipeLeft.direction = .left
        swipeLeft.numberOfTouchesRequired = 3
        addGestureRecognizer(swipeLeft)

        let swipeRight = UISwipeGestureRecognizer(target: self, action: #selector(handleThreeFingerSwipe(_:)))
        swipeRight.direction = .right
        swipeRight.numberOfTouchesRequired = 3
        addGestureRecognizer(swipeRight)

        // Two-finger tap -> Context Menu
        let twoFingerTap = UITapGestureRecognizer(target: self, action: #selector(handleTwoFingerTap(_:)))
        twoFingerTap.numberOfTouchesRequired = 2
        twoFingerTap.numberOfTapsRequired = 1
        addGestureRecognizer(twoFingerTap)
    }

    private func setupInteractions() {
        // Trackpad & Mouse hover tracking (iPadOS 13.4+)
        let pointerInteraction = UIPointerInteraction(delegate: self)
        addInteraction(pointerInteraction)

        // Apple Pencil Scribble integration (iPadOS 14+)
        let scribble = UIScribbleInteraction(delegate: self)
        addInteraction(scribble)
    }

    // MARK: - DisplayLink Render Loop

    func startRenderLoop() {
        stopRenderLoop()
        lastRenderTime = CACurrentMediaTime()
        let link = CADisplayLink(target: self, selector: #selector(renderLoop(_:)))
        link.add(to: .main, forMode: .common)
        displayLink = link
    }

    func stopRenderLoop() {
        displayLink?.invalidate()
        displayLink = nil
    }

    @objc private func renderLoop(_ link: CADisplayLink) {
        guard let host = host else { return }

        let now = link.timestamp > 0 ? link.timestamp : CACurrentMediaTime()
        let dt = lastRenderTime > 0 ? max(0.0, min(now - lastRenderTime, 0.05)) : (1.0 / 60.0)
        lastRenderTime = now

        // Apply native inertial scroll decay between frames
        let speed = hypot(scrollVelocity.x, scrollVelocity.y)
        if speed > 15.0 {
            let dx = Float(scrollVelocity.x * dt)
            let dy = Float(scrollVelocity.y * dt)
            wordcraft_send_pan_gesture(host, dx, dy)
            // Exponential damping matched to craft-ui-canvas FlickPan feel
            let damping = CGFloat(exp(-dt / 0.18))
            scrollVelocity.x *= damping
            scrollVelocity.y *= damping
        } else if scrollVelocity != .zero {
            scrollVelocity = .zero
        }

        wordcraft_render_frame(host)

        // Pump outgoing UIKit actions (file pickers, pasteboard sync, keyboard dismiss)
        while let actionPtr = wordcraft_poll_platform_action(host) {
            let json = String(cString: actionPtr)
            wordcraft_free_string(actionPtr)
            onPlatformAction?(json)
        }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let scale = window?.screen.scale ?? UIScreen.main.scale
        contentScaleFactor = scale
        metalLayer.contentsScale = scale
        metalLayer.drawableSize = CGSize(
            width: bounds.width * scale,
            height: bounds.height * scale
        )
        if let host = host {
            wordcraft_resize(host, Float(bounds.width), Float(bounds.height), Float(scale))
        }
    }

    // MARK: - UIGestureRecognizerDelegate

    override func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
        if gestureRecognizer === panRecognizer {
            // Yield to active selection handle, object corner, table border, or Apple Pencil drag
            if let host = host, wordcraft_is_touch_drag_active(host) != 0 {
                return false
            }
            return true
        }
        return true
    }

    func gestureRecognizer(
        _ gestureRecognizer: UIGestureRecognizer,
        shouldRecognizeSimultaneouslyWith otherGestureRecognizer: UIGestureRecognizer
    ) -> Bool {
        return false
    }

    // MARK: - Touch Handling & Palm Rejection

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        // Arrest inertial glide immediately on touch down
        scrollVelocity = .zero

        // If an Apple Pencil stroke begins over a pan recognizer, do not scroll
        for touch in touches {
            if touch.type == .pencil {
                scrollVelocity = .zero
            }
            sendTouch(touch, phase: 0) // TouchPhase::Began
        }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches {
            sendTouch(touch, phase: 1) // TouchPhase::Moved
        }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        var openKeyboard = false
        for touch in touches {
            sendTouch(touch, phase: 3) // TouchPhase::Ended
            if let host = host, touch.type == .direct {
                let loc = touch.location(in: self)
                if wordcraft_is_point_in_canvas(host, Float(loc.x), Float(loc.y)) != 0 {
                    openKeyboard = true
                }
            }
            let objId = ObjectIdentifier(touch)
            touchIdMap.removeValue(forKey: objId)
        }

        if openKeyboard && !isFirstResponder {
            becomeFirstResponder()
        }
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches {
            sendTouch(touch, phase: 4) // TouchPhase::Cancelled
            let objId = ObjectIdentifier(touch)
            touchIdMap.removeValue(forKey: objId)
        }
    }

    private func sendTouch(_ touch: UITouch, phase: Int32) {
        guard let host = host else { return }
        let objId = ObjectIdentifier(touch)
        let id: UInt64
        if let existing = touchIdMap[objId] {
            id = existing
        } else {
            id = nextTouchId
            nextTouchId += 1
            touchIdMap[objId] = id
        }

        let loc = touch.location(in: self)
        let force = touch.maximumPossibleForce > 0 ? Float(touch.force / touch.maximumPossibleForce) : 0.5
        let pointerType: Int32
        switch touch.type {
        case .pencil:
            pointerType = 1 // ApplePencil
        case .indirect, .indirectPointer:
            pointerType = 2 // IndirectPointer
        default:
            pointerType = 0 // DirectTouch
        }

        wordcraft_send_touch(host, phase, Float(loc.x), Float(loc.y), id, force, pointerType)
    }

    // MARK: - Physical Keyboard Forwarding (`pressesBegan` / `pressesEnded`)

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        guard let host = host else {
            super.pressesBegan(presses, with: event)
            return
        }

        var handledAny = false
        for press in presses {
            guard let key = press.key else { continue }
            handledAny = true

            let mods = modifierBitmask(key.modifierFlags)
            if (mods & 8) != 0 && key.charactersIgnoringModifiers.lowercased() == "v" {
                syncPasteboard(host)
            }

            let text = key.characters
            if text.isEmpty {
                wordcraft_send_key(host, Int32(key.keyCode.rawValue), mods, true, nil)
            } else {
                text.withCString { cText in
                    wordcraft_send_key(host, Int32(key.keyCode.rawValue), mods, true, cText)
                }
            }
        }

        if !handledAny {
            super.pressesBegan(presses, with: event)
        }
    }

    override func pressesEnded(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        guard let host = host else {
            super.pressesEnded(presses, with: event)
            return
        }

        var handledAny = false
        for press in presses {
            guard let key = press.key else { continue }
            handledAny = true
            let mods = modifierBitmask(key.modifierFlags)
            wordcraft_send_key(host, Int32(key.keyCode.rawValue), mods, false, nil)
        }

        if !handledAny {
            super.pressesEnded(presses, with: event)
        }
    }

    private func modifierBitmask(_ flags: UIKeyModifierFlags) -> UInt32 {
        var bits: UInt32 = 0
        if flags.contains(.alternate) { bits |= 1 }
        if flags.contains(.control) { bits |= 2 }
        if flags.contains(.shift) { bits |= 4 }
        if flags.contains(.command) { bits |= 8 }
        return bits
    }

    private func syncPasteboard(_ host: OpaquePointer) {
        if let text = UIPasteboard.general.string {
            text.withCString { cStr in
                wordcraft_sync_pasteboard_text(host, cStr)
            }
        } else {
            wordcraft_sync_pasteboard_text(host, nil)
        }
    }

    // MARK: - Gesture Handlers

    @objc private func handlePinch(_ gesture: UIPinchGestureRecognizer) {
        guard let host = host else { return }
        if gesture.state == .began {
            lastPinchScale = 1.0
        } else if gesture.state == .changed {
            let deltaScale = Float(gesture.scale / max(lastPinchScale, 0.01))
            lastPinchScale = gesture.scale
            wordcraft_send_pinch_gesture(host, deltaScale, Float(gesture.velocity))
        }
    }

    @objc private func handlePan(_ gesture: UIPanGestureRecognizer) {
        guard let host = host else { return }
        let translation = gesture.translation(in: self)

        switch gesture.state {
        case .began:
            scrollVelocity = .zero
            lastPanTranslation = .zero
        case .changed:
            let dx = Float(translation.x - lastPanTranslation.x)
            let dy = Float(translation.y - lastPanTranslation.y)
            lastPanTranslation = translation
            wordcraft_send_pan_gesture(host, dx, dy)
        case .ended:
            let v = gesture.velocity(in: self)
            if hypot(v.x, v.y) > 60.0 {
                scrollVelocity = v
            }
            lastPanTranslation = .zero
        case .cancelled, .failed:
            scrollVelocity = .zero
            lastPanTranslation = .zero
        default:
            break
        }
    }

    @objc private func handleThreeFingerSwipe(_ gesture: UISwipeGestureRecognizer) {
        guard let host = host else { return }
        let dir: Int32 = gesture.direction == .left ? 0 : 1
        wordcraft_send_three_finger_swipe(host, dir)
    }

    @objc private func handleTwoFingerTap(_ gesture: UITapGestureRecognizer) {
        guard let host = host, gesture.state == .ended else { return }
        let loc = gesture.location(in: self)
        syncPasteboard(host)
        wordcraft_send_context_menu(host, Float(loc.x), Float(loc.y))
    }

    // MARK: - UIPointerInteractionDelegate (Mouse / Trackpad Hover & Scroll)

    func pointerInteraction(_ interaction: UIPointerInteraction, regionFor request: UIPointerRegionRequest, defaultRegion: UIPointerRegion) -> UIPointerRegion? {
        if let host = host {
            wordcraft_send_mouse(host, Float(request.location.x), Float(request.location.y), -1, false, 0, 0)
        }
        return defaultRegion
    }

    // MARK: - UIScribbleInteractionDelegate (Apple Pencil Handwriting-to-Text)

    func scribbleInteractionShouldDelayFocus(_ interaction: UIScribbleInteraction) -> Bool {
        return false
    }

    func scribbleInteractionWillBeginWriting(_ interaction: UIScribbleInteraction) {
        if let host = host {
            wordcraft_set_input_mode(host, 2) // Hybrid mode when Pencil writes
        }
    }
}
