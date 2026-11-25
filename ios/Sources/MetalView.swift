import UIKit
import Metal
import QuartzCore

private final class WordTextPosition: UITextPosition {
    let offset: Int
    init(_ offset: Int) {
        self.offset = offset
        super.init()
    }
}

private final class WordTextRange: UITextRange {
    let startPos: WordTextPosition
    let endPos: WordTextPosition

    init(start: Int, end: Int) {
        let lo = min(start, end)
        let hi = max(start, end)
        self.startPos = WordTextPosition(lo)
        self.endPos = WordTextPosition(hi)
        super.init()
    }

    override var start: UITextPosition { startPos }
    override var end: UITextPosition { endPos }
    override var isEmpty: Bool { startPos.offset == endPos.offset }
}

private final class WordSelectionRect: UITextSelectionRect {
    private let _rect: CGRect
    private let _containsStart: Bool
    private let _containsEnd: Bool

    init(rect: CGRect, containsStart: Bool = true, containsEnd: Bool = true) {
        self._rect = rect
        self._containsStart = containsStart
        self._containsEnd = containsEnd
        super.init()
    }

    override var rect: CGRect { _rect }
    override var writingDirection: NSWritingDirection { .leftToRight }
    override var containsStart: Bool { _containsStart }
    override var containsEnd: Bool { _containsEnd }
    override var isVertical: Bool { false }
}

/// Direct CAMetalLayer view driving WordCraft's wgpu/egui surface, 1-finger touch scrolling,
/// multi-touch gestures, long-press right-click, Apple Pencil Scribble, full `UITextInput`
/// (spacebar floating-cursor trackpad, 2-finger selection, marked text), mouse/trackpad hover,
/// and physical keyboard (`pressesBegan` / Mac chords).
class MetalView: UIView, UIGestureRecognizerDelegate, UITextInput, UIPointerInteractionDelegate, UIScribbleInteractionDelegate {
    var host: OpaquePointer?
    var onPlatformAction: ((String) -> Void)?

    private var displayLink: CADisplayLink?
    private var lastPanTranslation: CGPoint = .zero
    private var panOrigin: CGPoint = .zero
    private var lastRenderTime: CFTimeInterval = 0
    private var scrollVelocity: CGPoint = .zero
    private var isPanInertialGlide: Bool = false
    private var lastPinchScale: CGFloat = 1.0
    private var panRecognizer: UIPanGestureRecognizer?
    private var longPressRecognizer: UILongPressGestureRecognizer?

    // Map active UITouch object pointers to stable 64-bit touch IDs for Rust
    private var touchIdMap = [ObjectIdentifier: UInt64]()
    private var nextTouchId: UInt64 = 1
    // Direct touches hitting interactive handles or Apple Pencil go straight to Rust immediately;
    // normal finger touches wait in `pendingTapTouch` until tap lift or pan/long-press wins.
    private var immediateTouchIds = Set<ObjectIdentifier>()
    private var pendingTapTouch: ObjectIdentifier?
    private var pendingTapLocation: CGPoint = .zero

    // MARK: - UITextInput State (Virtual Keyboard, Spacebar Floating Cursor & Selection)

    private struct ImeSnapshot {
        let available: Bool
        let text: String
        let selStart: Int
        let selEnd: Int
        let caretRect: CGRect
        let wantsKeyboard: Bool
    }

    private var currentMarkedText: String?
    private var currentMarkedStart: Int = 0
    private var floatingCursorLastPoint: CGPoint?

    weak var inputDelegate: UITextInputDelegate?
    var markedTextStyle: [NSAttributedString.Key: Any]?
    lazy var tokenizer: UITextInputTokenizer = UITextInputStringTokenizer(textInput: self)

    override var canBecomeFirstResponder: Bool { true }

    var hasText: Bool {
        let snap = fetchImeSnapshot()
        return !snap.text.isEmpty || snap.available
    }

    var autocapitalizationType: UITextAutocapitalizationType = .none
    var autocorrectionType: UITextAutocorrectionType = .no
    var spellCheckingType: UITextSpellCheckingType = .no
    var smartQuotesType: UITextSmartQuotesType = .no
    var smartDashesType: UITextSmartDashesType = .no
    var smartInsertDeleteType: UITextSmartInsertDeleteType = .no
    var keyboardType: UIKeyboardType = .default
    var returnKeyType: UIReturnKeyType = .default

    private func fetchImeSnapshot() -> ImeSnapshot {
        guard let host = host,
              let ptr = wordcraft_get_ime_state_json(host) else {
            return ImeSnapshot(available: false, text: "", selStart: 0, selEnd: 0, caretRect: CGRect(x: 20, y: 120, width: 2, height: 18), wantsKeyboard: false)
        }
        let jsonStr = String(cString: ptr)
        wordcraft_free_string(ptr)

        guard let data = jsonStr.data(using: .utf8),
              let dict = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return ImeSnapshot(available: false, text: "", selStart: 0, selEnd: 0, caretRect: CGRect(x: 20, y: 120, width: 2, height: 18), wantsKeyboard: false)
        }

        let available = (dict["available"] as? Bool) ?? false
        let text = (dict["text"] as? String) ?? ""
        let selStart = (dict["sel_start"] as? Int) ?? 0
        let selEnd = (dict["sel_end"] as? Int) ?? 0
        let wantsKeyboard = (dict["wants_keyboard"] as? Bool) ?? false
        let arr = (dict["caret"] as? [NSNumber]) ?? []
        let caretRect: CGRect
        if arr.count == 4 {
            caretRect = CGRect(x: arr[0].doubleValue, y: arr[1].doubleValue, width: arr[2].doubleValue, height: arr[3].doubleValue)
        } else {
            caretRect = CGRect(x: 20, y: 120, width: 2, height: 18)
        }
        return ImeSnapshot(available: available, text: text, selStart: selStart, selEnd: selEnd, caretRect: caretRect, wantsKeyboard: wantsKeyboard)
    }

    // MARK: - UIKeyInput

    func insertText(_ text: String) {
        guard let host = host, !text.isEmpty else { return }
        if currentMarkedText != nil {
            currentMarkedText = nil
            wordcraft_ime_unmark_text(host, nil)
        }
        if text == "\n" || text == "\r" {
            wordcraft_send_key(host, 40, 0, true, nil)
            wordcraft_send_key(host, 40, 0, false, nil)
        } else if text == "\t" {
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

    // MARK: - UITextInput Text & Selection Protocol

    var selectedTextRange: UITextRange? {
        get {
            let snap = fetchImeSnapshot()
            if let marked = currentMarkedText {
                let len = (marked as NSString).length
                let pos = min(snap.selEnd, currentMarkedStart + len)
                return WordTextRange(start: pos, end: pos)
            }
            return WordTextRange(start: snap.selStart, end: snap.selEnd)
        }
        set {
            guard let host = host, let range = newValue as? WordTextRange else { return }
            let start = range.startPos.offset
            let end = range.endPos.offset
            _ = wordcraft_ime_set_selection(host, start, end)
        }
    }

    var markedTextRange: UITextRange? {
        guard let marked = currentMarkedText else { return nil }
        return WordTextRange(start: currentMarkedStart, end: currentMarkedStart + (marked as NSString).length)
    }

    func setMarkedText(_ markedText: String?, selectedRange: NSRange) {
        guard let host = host else { return }
        if currentMarkedText == nil {
            currentMarkedStart = fetchImeSnapshot().selStart
        }
        let text = markedText ?? ""
        currentMarkedText = text.isEmpty ? nil : text
        let selStart = Int32(selectedRange.location)
        let selEnd = Int32(selectedRange.location + selectedRange.length)
        text.withCString { cStr in
            wordcraft_ime_set_marked_text(host, cStr, selStart, selEnd)
        }
    }

    func unmarkText() {
        guard let host = host else { return }
        let committed = currentMarkedText
        currentMarkedText = nil
        if let committed = committed, !committed.isEmpty {
            committed.withCString { cStr in
                wordcraft_ime_unmark_text(host, cStr)
            }
        } else {
            wordcraft_ime_unmark_text(host, nil)
        }
    }

    func text(in range: UITextRange) -> String? {
        guard let wr = range as? WordTextRange else { return nil }
        let snap = fetchImeSnapshot()
        let ns = snap.text as NSString
        let lo = max(0, min(wr.startPos.offset, ns.length))
        let hi = max(lo, min(wr.endPos.offset, ns.length))
        return ns.substring(with: NSRange(location: lo, length: hi - lo))
    }

    func replace(_ range: UITextRange, withText text: String) {
        guard let host = host, let wr = range as? WordTextRange else { return }
        currentMarkedText = nil
        text.withCString { cStr in
            _ = wordcraft_ime_replace_range(host, wr.startPos.offset, wr.endPos.offset, cStr)
        }
    }

    var beginningOfDocument: UITextPosition { WordTextPosition(0) }

    var endOfDocument: UITextPosition {
        let ns = fetchImeSnapshot().text as NSString
        return WordTextPosition(ns.length)
    }

    func textRange(from fromPosition: UITextPosition, to toPosition: UITextPosition) -> UITextRange? {
        guard let from = fromPosition as? WordTextPosition,
              let to = toPosition as? WordTextPosition else { return nil }
        return WordTextRange(start: from.offset, end: to.offset)
    }

    func position(from position: UITextPosition, offset: Int) -> UITextPosition? {
        guard let pos = position as? WordTextPosition else { return nil }
        let len = (fetchImeSnapshot().text as NSString).length
        let target = pos.offset + offset
        if target < 0 || target > len { return nil }
        return WordTextPosition(target)
    }

    func position(from position: UITextPosition, in direction: UITextLayoutDirection, offset: Int) -> UITextPosition? {
        guard let pos = position as? WordTextPosition else { return nil }
        let len = (fetchImeSnapshot().text as NSString).length
        let next: Int
        switch direction {
        case .left, .up:
            next = max(0, pos.offset - offset)
        case .right, .down:
            next = min(len, pos.offset + offset)
        @unknown default:
            next = pos.offset
        }
        return WordTextPosition(next)
    }

    func compare(_ position: UITextPosition, to other: UITextPosition) -> ComparisonResult {
        guard let a = position as? WordTextPosition, let b = other as? WordTextPosition else { return .orderedSame }
        if a.offset < b.offset { return .orderedAscending }
        if a.offset > b.offset { return .orderedDescending }
        return .orderedSame
    }

    func offset(from: UITextPosition, to toPosition: UITextPosition) -> Int {
        guard let a = from as? WordTextPosition, let b = toPosition as? WordTextPosition else { return 0 }
        return b.offset - a.offset
    }

    func position(within range: UITextRange, farthestIn direction: UITextLayoutDirection) -> UITextPosition? {
        guard let wr = range as? WordTextRange else { return nil }
        switch direction {
        case .left, .up: return wr.startPos
        default: return wr.endPos
        }
    }

    func characterRange(byExtending position: UITextPosition, in direction: UITextLayoutDirection) -> UITextRange? {
        guard let pos = position as? WordTextPosition else { return nil }
        let len = (fetchImeSnapshot().text as NSString).length
        switch direction {
        case .left, .up:
            return WordTextRange(start: 0, end: pos.offset)
        default:
            return WordTextRange(start: pos.offset, end: len)
        }
    }

    func baseWritingDirection(for position: UITextPosition, in direction: UITextStorageDirection) -> NSWritingDirection {
        return .natural
    }

    func setBaseWritingDirection(_ writingDirection: NSWritingDirection, for range: UITextRange) {}

    func caretRect(for position: UITextPosition) -> CGRect {
        return fetchImeSnapshot().caretRect
    }

    func firstRect(for range: UITextRange) -> CGRect {
        return fetchImeSnapshot().caretRect
    }

    func selectionRects(for range: UITextRange) -> [UITextSelectionRect] {
        return [WordSelectionRect(rect: fetchImeSnapshot().caretRect)]
    }

    func closestPosition(to point: CGPoint) -> UITextPosition? {
        if let host = host {
            let off = wordcraft_ime_offset_at_point(host, Float(point.x), Float(point.y))
            if off >= 0 {
                return WordTextPosition(Int(off))
            }
        }
        return WordTextPosition(fetchImeSnapshot().selEnd)
    }

    func closestPosition(to point: CGPoint, within range: UITextRange) -> UITextPosition? {
        guard let wr = range as? WordTextRange, let pos = closestPosition(to: point) as? WordTextPosition else {
            return range.start
        }
        let clamped = max(wr.startPos.offset, min(wr.endPos.offset, pos.offset))
        return WordTextPosition(clamped)
    }

    func characterRange(at point: CGPoint) -> UITextRange? {
        guard let pos = closestPosition(to: point) as? WordTextPosition else { return nil }
        return WordTextRange(start: pos.offset, end: pos.offset)
    }

    // MARK: - Spacebar Trackpad Floating Cursor Callbacks

    func beginFloatingCursor(at point: CGPoint) {
        floatingCursorLastPoint = point
        inputDelegate?.selectionWillChange(self)
        if let host = host {
            wordcraft_ime_reset_floating_cursor(host)
        }
        inputDelegate?.selectionDidChange(self)
    }

    func updateFloatingCursor(at point: CGPoint) {
        guard let host = host, let last = floatingCursorLastPoint else { return }
        let dx = Float(point.x - last.x)
        let dy = Float(point.y - last.y)
        floatingCursorLastPoint = point

        inputDelegate?.selectionWillChange(self)
        _ = wordcraft_ime_move_caret_by_delta(host, dx, dy, false)
        inputDelegate?.selectionDidChange(self)
    }

    func endFloatingCursor() {
        floatingCursorLastPoint = nil
        if let host = host {
            wordcraft_ime_reset_floating_cursor(host)
        }
        inputDelegate?.selectionWillChange(self)
        inputDelegate?.selectionDidChange(self)
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

        // Long Press -> Right Click Context Menu
        let longPress = UILongPressGestureRecognizer(target: self, action: #selector(handleLongPress(_:)))
        longPress.minimumPressDuration = 0.45
        longPress.allowableMovement = 12.0
        longPress.cancelsTouchesInView = true
        longPress.delegate = self
        addGestureRecognizer(longPress)
        self.longPressRecognizer = longPress

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
        let pointerInteraction = UIPointerInteraction(delegate: self)
        addInteraction(pointerInteraction)

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

        // Apply native inertial scroll decay keeping pointer_hover_pos over panOrigin
        if isPanInertialGlide {
            let speed = hypot(scrollVelocity.x, scrollVelocity.y)
            if speed > 15.0 {
                let dx = Float(scrollVelocity.x * dt)
                let dy = Float(scrollVelocity.y * dt)
                wordcraft_send_pan_gesture(host, Float(panOrigin.x), Float(panOrigin.y), dx, dy)
                let damping = CGFloat(exp(-dt / 0.18))
                scrollVelocity.x *= damping
                scrollVelocity.y *= damping
            } else {
                scrollVelocity = .zero
                isPanInertialGlide = false
                wordcraft_send_hover_gone(host)
            }
        }

        wordcraft_render_frame(host)

        // Pump outgoing UIKit actions (file pickers, export sheet, pasteboard sync, keyboard dismiss)
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
        if gestureRecognizer === panRecognizer || gestureRecognizer === longPressRecognizer {
            // Yield when a selection handle, object corner, table border, or Apple Pencil stroke is active
            if !immediateTouchIds.isEmpty {
                return false
            }
            if let host = host, wordcraft_is_touch_drag_active(host) != 0 {
                return false
            }
        }
        return true
    }

    func gestureRecognizer(
        _ gestureRecognizer: UIGestureRecognizer,
        shouldRecognizeSimultaneouslyWith otherGestureRecognizer: UIGestureRecognizer
    ) -> Bool {
        return false
    }

    // MARK: - Touch Handling & Tap / Scroll / Handle Arbitration

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        // Arrest any active inertial scroll on finger down
        if isPanInertialGlide {
            scrollVelocity = .zero
            isPanInertialGlide = false
        }

        guard let host = host else { return }

        for touch in touches {
            let objId = ObjectIdentifier(touch)
            let loc = touch.location(in: self)

            let isImmediate = (touch.type != .direct)
                || (touches.count > 1)
                || (wordcraft_is_near_touch_target(host, Float(loc.x), Float(loc.y)) != 0)

            if isImmediate {
                pendingTapTouch = nil
                immediateTouchIds.insert(objId)
                sendTouch(touch, phase: 0) // Began
            } else {
                pendingTapTouch = objId
                pendingTapLocation = loc
                // Keep hover pointer alive at finger position without pressing Primary yet
                wordcraft_send_hover(host, Float(loc.x), Float(loc.y))
            }
        }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches {
            let objId = ObjectIdentifier(touch)
            if immediateTouchIds.contains(objId) {
                sendTouch(touch, phase: 1) // Moved
            }
        }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        var openKeyboard = false
        for touch in touches {
            let objId = ObjectIdentifier(touch)
            let loc = touch.location(in: self)

            if immediateTouchIds.contains(objId) {
                immediateTouchIds.remove(objId)
                sendTouch(touch, phase: 3) // Ended
            } else if pendingTapTouch == objId, let host = host {
                pendingTapTouch = nil
                wordcraft_send_tap(host, Float(pendingTapLocation.x), Float(pendingTapLocation.y))
                if touch.type == .direct && wordcraft_is_point_in_canvas(host, Float(loc.x), Float(loc.y)) != 0 {
                    openKeyboard = true
                }
            }
            touchIdMap.removeValue(forKey: objId)
        }

        if openKeyboard && !isFirstResponder {
            becomeFirstResponder()
        }
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        for touch in touches {
            let objId = ObjectIdentifier(touch)
            if immediateTouchIds.contains(objId) {
                immediateTouchIds.remove(objId)
                sendTouch(touch, phase: 4) // Cancelled
            } else if pendingTapTouch == objId {
                pendingTapTouch = nil
            }
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
            pendingTapTouch = nil
            scrollVelocity = .zero
            isPanInertialGlide = false
            lastPanTranslation = .zero
            panOrigin = gesture.location(in: self)
            wordcraft_send_hover(host, Float(panOrigin.x), Float(panOrigin.y))
        case .changed:
            let dx = Float(translation.x - lastPanTranslation.x)
            let dy = Float(translation.y - lastPanTranslation.y)
            lastPanTranslation = translation
            wordcraft_send_pan_gesture(host, Float(panOrigin.x), Float(panOrigin.y), dx, dy)
        case .ended:
            let v = gesture.velocity(in: self)
            if hypot(v.x, v.y) > 50.0 {
                scrollVelocity = v
                isPanInertialGlide = true
            } else {
                scrollVelocity = .zero
                isPanInertialGlide = false
                wordcraft_send_hover_gone(host)
            }
            lastPanTranslation = .zero
        case .cancelled, .failed:
            scrollVelocity = .zero
            isPanInertialGlide = false
            lastPanTranslation = .zero
            wordcraft_send_hover_gone(host)
        default:
            break
        }
    }

    @objc private func handleLongPress(_ gesture: UILongPressGestureRecognizer) {
        guard let host = host, gesture.state == .began else { return }
        pendingTapTouch = nil
        scrollVelocity = .zero
        isPanInertialGlide = false
        let loc = gesture.location(in: self)
        syncPasteboard(host)
        wordcraft_send_context_menu(host, Float(loc.x), Float(loc.y))
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
