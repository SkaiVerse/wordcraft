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
/// trackpad & mouse hover/click/right-click/scroll, physical keyboard (`pressesBegan` + repeat),
/// full `UITextInput` (spacebar floating-cursor trackpad, selection, marked text), and Apple Pencil.
class MetalView: UIView, UIGestureRecognizerDelegate, UITextInput, UIPointerInteractionDelegate, UIScribbleInteractionDelegate {
    var host: OpaquePointer?
    var onPlatformAction: ((String) -> Void)?

    private var displayLink: CADisplayLink?
    private var lastPanTranslation: CGPoint = .zero
    private var lastTrackpadTranslation: CGPoint = .zero
    private var panOrigin: CGPoint = .zero
    private var lastRenderTime: CFTimeInterval = 0
    private var scrollVelocity: CGPoint = .zero
    private var isPanInertialGlide: Bool = false
    private var lastPinchScale: CGFloat = 1.0
    private var panRecognizer: UIPanGestureRecognizer?
    private var trackpadScrollRecognizer: UIPanGestureRecognizer?
    private var longPressRecognizer: UILongPressGestureRecognizer?

    // Keep MetalView as firstResponder even when the software keyboard is hidden so physical keyboards
    // (`pressesBegan` / `UIKeyCommand`) keep working at all times.
    private let hiddenInputPlaceholder = UIView(frame: .zero)
    private var customEmptyInputView: UIView?

    override var inputView: UIView? {
        return customEmptyInputView
    }

    func setSoftwareKeyboardVisible(_ visible: Bool) {
        let wantsHiddenView = !visible
        let currentlyHidden = (customEmptyInputView != nil)
        if wantsHiddenView != currentlyHidden {
            customEmptyInputView = visible ? nil : hiddenInputPlaceholder
            if isFirstResponder {
                reloadInputViews()
            }
        }
        if !isFirstResponder {
            becomeFirstResponder()
        }
    }

    // Track hardware key hold for native key-repeat on arrows, Backspace, typing keys, etc.
    private struct HeldKey {
        let keyCode: Int32
        let modifiers: UInt32
        let text: String?
        let downAt: CFTimeInterval
        var lastRepeatAt: CFTimeInterval
    }
    private var heldKeyForRepeat: HeldKey?

    // Active touch/mouse tracking
    private var touchIdMap = [ObjectIdentifier: UInt64]()
    private var nextTouchId: UInt64 = 1
    private var indirectButtonMap = [ObjectIdentifier: Int32]()
    private var immediateTouchIds = Set<ObjectIdentifier>()
    private var pendingTapTouch: ObjectIdentifier?
    private var pendingTapLocation: CGPoint = .zero

    // MARK: - UITextInput State

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
            wordcraft_send_key_tap(host, 40, 0)
        } else if text == "\t" {
            wordcraft_send_key_tap(host, 43, 0)
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

    // MARK: - UITextInput Protocol

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
            _ = wordcraft_ime_set_selection(host, range.startPos.offset, range.endPos.offset)
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
        let ns = fetchImeSnapshot().text as NSString
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
        case .left, .up: next = max(0, pos.offset - offset)
        case .right, .down: next = min(len, pos.offset + offset)
        @unknown default: next = pos.offset
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
        case .left, .up: return WordTextRange(start: 0, end: pos.offset)
        default: return WordTextRange(start: pos.offset, end: len)
        }
    }

    func baseWritingDirection(for position: UITextPosition, in direction: UITextStorageDirection) -> NSWritingDirection {
        return .natural
    }

    func setBaseWritingDirection(_ writingDirection: NSWritingDirection, for range: UITextRange) {}

    func caretRect(for position: UITextPosition) -> CGRect { fetchImeSnapshot().caretRect }
    func firstRect(for range: UITextRange) -> CGRect { fetchImeSnapshot().caretRect }
    func selectionRects(for range: UITextRange) -> [UITextSelectionRect] { [WordSelectionRect(rect: fetchImeSnapshot().caretRect)] }

    func closestPosition(to point: CGPoint) -> UITextPosition? {
        if let host = host {
            let off = wordcraft_ime_offset_at_point(host, Float(point.x), Float(point.y))
            if off >= 0 { return WordTextPosition(Int(off)) }
        }
        return WordTextPosition(fetchImeSnapshot().selEnd)
    }

    func closestPosition(to point: CGPoint, within range: UITextRange) -> UITextPosition? {
        guard let wr = range as? WordTextRange, let pos = closestPosition(to: point) as? WordTextPosition else {
            return range.start
        }
        return WordTextPosition(max(wr.startPos.offset, min(wr.endPos.offset, pos.offset)))
    }

    func characterRange(at point: CGPoint) -> UITextRange? {
        guard let pos = closestPosition(to: point) as? WordTextPosition else { return nil }
        return WordTextRange(start: pos.offset, end: pos.offset)
    }

    // MARK: - Spacebar Trackpad Floating Cursor Callbacks

    func beginFloatingCursor(at point: CGPoint) {
        floatingCursorLastPoint = point
        inputDelegate?.selectionWillChange(self)
        if let host = host { wordcraft_ime_reset_floating_cursor(host) }
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
        if let host = host { wordcraft_ime_reset_floating_cursor(host) }
        inputDelegate?.selectionWillChange(self)
        inputDelegate?.selectionDidChange(self)
    }

    // MARK: - Layer Setup

    override class var layerClass: AnyClass { CAMetalLayer.self }
    var metalLayer: CAMetalLayer { layer as! CAMetalLayer }

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
        // Two-finger pinch-to-zoom (supports both direct fingers and trackpad pinch)
        let pinch = UIPinchGestureRecognizer(target: self, action: #selector(handlePinch(_:)))
        pinch.delegate = self
        addGestureRecognizer(pinch)

        // Trackpad two-finger scroll & mouse wheel (`allowedScrollTypesMask = .all`, indirect scroll events only)
        let trackpadPan = UIPanGestureRecognizer(target: self, action: #selector(handleTrackpadScroll(_:)))
        trackpadPan.allowedScrollTypesMask = .all
        trackpadPan.minimumNumberOfTouches = 0
        trackpadPan.maximumNumberOfTouches = 0
        trackpadPan.cancelsTouchesInView = false
        trackpadPan.delegate = self
        addGestureRecognizer(trackpadPan)
        self.trackpadScrollRecognizer = trackpadPan

        // One-finger direct touch scroll (excludes indirect mouse/trackpad drags so drag-selection works)
        let pan = UIPanGestureRecognizer(target: self, action: #selector(handlePan(_:)))
        pan.allowedTouchTypes = [NSNumber(value: UITouch.TouchType.direct.rawValue)]
        pan.minimumNumberOfTouches = 1
        pan.maximumNumberOfTouches = 1
        pan.cancelsTouchesInView = true
        pan.delegate = self
        addGestureRecognizer(pan)
        self.panRecognizer = pan

        // Long Press -> Right Click Context Menu (direct finger press)
        let longPress = UILongPressGestureRecognizer(target: self, action: #selector(handleLongPress(_:)))
        longPress.allowedTouchTypes = [NSNumber(value: UITouch.TouchType.direct.rawValue)]
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
        // Continuous trackpad & mouse cursor movement tracking (iPadOS 13.4+)
        let hover = UIHoverGestureRecognizer(target: self, action: #selector(handleHover(_:)))
        addGestureRecognizer(hover)

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

        // Hardware keyboard auto-repeat (0.4s delay, 35ms interval)
        if var held = heldKeyForRepeat {
            if now - held.downAt >= 0.40 && now - held.lastRepeatAt >= 0.035 {
                held.lastRepeatAt = now
                heldKeyForRepeat = held
                if let txt = held.text, !txt.isEmpty {
                    txt.withCString { cStr in
                        wordcraft_send_key(host, held.keyCode, held.modifiers, true, cStr)
                    }
                } else {
                    wordcraft_send_key(host, held.keyCode, held.modifiers, true, nil)
                }
            }
        }

        // Inertial decay for direct-touch scrolling
        if isPanInertialGlide {
            let speed = hypot(scrollVelocity.x, scrollVelocity.y)
            if speed > 20.0 {
                let dx = Float(scrollVelocity.x * dt)
                let dy = Float(scrollVelocity.y * dt)
                wordcraft_send_pan_gesture(host, Float(panOrigin.x), Float(panOrigin.y), dx, dy)
                let damping = CGFloat(exp(-dt / 0.14))
                scrollVelocity.x *= damping
                scrollVelocity.y *= damping
            } else {
                scrollVelocity = .zero
                isPanInertialGlide = false
                wordcraft_send_hover_gone(host)
            }
        }

        wordcraft_render_frame(host)

        // Pump outgoing UIKit actions
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
        metalLayer.drawableSize = CGSize(width: bounds.width * scale, height: bounds.height * scale)
        if let host = host {
            wordcraft_resize(host, Float(bounds.width), Float(bounds.height), Float(scale))
        }
    }

    // MARK: - UIGestureRecognizerDelegate

    override func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
        if gestureRecognizer === panRecognizer || gestureRecognizer === longPressRecognizer {
            if !immediateTouchIds.isEmpty { return false }
            if let host = host, wordcraft_is_touch_drag_active(host) != 0 { return false }
        }
        return true
    }

    func gestureRecognizer(
        _ gestureRecognizer: UIGestureRecognizer,
        shouldRecognizeSimultaneouslyWith otherGestureRecognizer: UIGestureRecognizer
    ) -> Bool {
        return false
    }

    // MARK: - Touch & Indirect Mouse/Trackpad Handling

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        if isPanInertialGlide {
            scrollVelocity = .zero
            isPanInertialGlide = false
        }
        guard let host = host else { return }

        // Ensure MetalView holds first responder cleanly on any click/tap
        if !isFirstResponder {
            becomeFirstResponder()
        }

        for touch in touches {
            let objId = ObjectIdentifier(touch)
            let loc = touch.location(in: self)

            // Route external Mouse / Magic Trackpad clicks cleanly via mouse API (preserving buttonMask + drag)
            if touch.type == .indirect || touch.type == .indirectPointer {
                let isSecondary = (event?.buttonMask.contains(.secondary) == true)
                let button: Int32 = isSecondary ? 1 : 0
                indirectButtonMap[objId] = button
                if isSecondary { syncPasteboard(host) }
                wordcraft_send_mouse(host, Float(loc.x), Float(loc.y), button, true, 0, 0)
                continue
            }

            let isImmediate = (touch.type == .pencil)
                || (touches.count > 1)
                || (wordcraft_is_near_touch_target(host, Float(loc.x), Float(loc.y)) != 0)

            if isImmediate {
                pendingTapTouch = nil
                immediateTouchIds.insert(objId)
                sendTouch(touch, phase: 0) // Began
            } else {
                pendingTapTouch = objId
                pendingTapLocation = loc
                wordcraft_send_hover(host, Float(loc.x), Float(loc.y))
            }
        }
    }

    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
        guard let host = host else { return }
        for touch in touches {
            let objId = ObjectIdentifier(touch)
            if touch.type == .indirect || touch.type == .indirectPointer {
                let loc = touch.location(in: self)
                wordcraft_send_mouse(host, Float(loc.x), Float(loc.y), -1, false, 0, 0)
            } else if immediateTouchIds.contains(objId) {
                sendTouch(touch, phase: 1) // Moved
            }
        }
    }

    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        guard let host = host else { return }
        var openKeyboard = false

        for touch in touches {
            let objId = ObjectIdentifier(touch)
            let loc = touch.location(in: self)

            if touch.type == .indirect || touch.type == .indirectPointer {
                let button = indirectButtonMap.removeValue(forKey: objId) ?? 0
                wordcraft_send_mouse(host, Float(loc.x), Float(loc.y), button, false, 0, 0)
                continue
            }

            if immediateTouchIds.contains(objId) {
                immediateTouchIds.remove(objId)
                sendTouch(touch, phase: 3) // Ended
            } else if pendingTapTouch == objId {
                pendingTapTouch = nil
                wordcraft_send_tap(host, Float(pendingTapLocation.x), Float(pendingTapLocation.y))
                if touch.type == .direct && wordcraft_is_point_in_canvas(host, Float(loc.x), Float(loc.y)) != 0 {
                    openKeyboard = true
                }
            }
            touchIdMap.removeValue(forKey: objId)
        }

        if openKeyboard {
            setSoftwareKeyboardVisible(true)
        }
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        guard let host = host else { return }
        for touch in touches {
            let objId = ObjectIdentifier(touch)
            if touch.type == .indirect || touch.type == .indirectPointer {
                let loc = touch.location(in: self)
                let button = indirectButtonMap.removeValue(forKey: objId) ?? 0
                wordcraft_send_mouse(host, Float(loc.x), Float(loc.y), button, false, 0, 0)
                continue
            }

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
        let pointerType: Int32 = (touch.type == .pencil) ? 1 : 0
        wordcraft_send_touch(host, phase, Float(loc.x), Float(loc.y), id, force, pointerType)
    }

    // MARK: - Physical Keyboard Forwarding (`pressesBegan` / `pressesEnded` / Repeat)

    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        guard let host = host else {
            super.pressesBegan(presses, with: event)
            return
        }

        var handledAny = false
        for press in presses {
            guard let key = press.key else { continue }
            handledAny = true

            let hid = Int32(key.keyCode.rawValue)
            let mods = effectiveModifierBitmask(from: key)

            if (mods & 8) != 0 && key.charactersIgnoringModifiers.lowercased() == "v" {
                syncPasteboard(host)
            }

            let text = key.characters
            if text.isEmpty {
                wordcraft_send_key(host, hid, mods, true, nil)
            } else {
                text.withCString { cText in
                    wordcraft_send_key(host, hid, mods, true, cText)
                }
            }

            // Track non-modifier keys for smooth hardware key-repeat (arrows, backspace, typing)
            if hid < 224 || hid > 231 {
                let now = CACurrentMediaTime()
                heldKeyForRepeat = HeldKey(keyCode: hid, modifiers: mods, text: text.isEmpty ? nil : text, downAt: now, lastRepeatAt: now)
            }
        }

        if !handledAny {
            super.pressesBegan(presses, with: event)
        }
    }

    override func pressesChanged(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        guard let host = host else {
            super.pressesChanged(presses, with: event)
            return
        }
        var handledAny = false
        for press in presses {
            guard let key = press.key else { continue }
            handledAny = true
            let hid = Int32(key.keyCode.rawValue)
            let mods = effectiveModifierBitmask(from: key)
            wordcraft_send_key(host, hid, mods, true, nil)
        }
        if !handledAny { super.pressesChanged(presses, with: event) }
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
            let hid = Int32(key.keyCode.rawValue)
            let mods = releaseModifierBitmask(from: key)

            if heldKeyForRepeat?.keyCode == hid {
                heldKeyForRepeat = nil
            }
            wordcraft_send_key(host, hid, mods, false, nil)
        }

        if !handledAny {
            super.pressesEnded(presses, with: event)
        }
    }

    override func pressesCancelled(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        heldKeyForRepeat = nil
        if let host = host {
            wordcraft_clear_modifiers(host)
        }
        super.pressesCancelled(presses, with: event)
    }

    private func modifierBitmask(_ flags: UIKeyModifierFlags) -> UInt32 {
        var bits: UInt32 = 0
        if flags.contains(.alternate) { bits |= 1 }
        if flags.contains(.control) { bits |= 2 }
        if flags.contains(.shift) { bits |= 4 }
        if flags.contains(.command) { bits |= 8 }
        return bits
    }

    private func effectiveModifierBitmask(from key: UIKey) -> UInt32 {
        var bits = modifierBitmask(key.modifierFlags)
        // Include modifier bit for the modifier key's own press (HID 224..231)
        switch key.keyCode.rawValue {
        case 224, 228: bits |= 2 // Control
        case 225, 229: bits |= 4 // Shift
        case 226, 230: bits |= 1 // Option/Alt
        case 227, 231: bits |= 8 // Command
        default: break
        }
        return bits
    }

    private func releaseModifierBitmask(from key: UIKey) -> UInt32 {
        var bits = modifierBitmask(key.modifierFlags)
        switch key.keyCode.rawValue {
        case 224, 228: bits &= ~2
        case 225, 229: bits &= ~4
        case 226, 230: bits &= ~1
        case 227, 231: bits &= ~8
        default: break
        }
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

    // MARK: - Gesture Handlers (Hover, Trackpad Scroll, Touch Pan, Pinch, Long Press)

    @objc private func handleHover(_ gesture: UIHoverGestureRecognizer) {
        guard let host = host else { return }
        switch gesture.state {
        case .began, .changed:
            let loc = gesture.location(in: self)
            wordcraft_send_mouse(host, Float(loc.x), Float(loc.y), -1, false, 0, 0)
        case .ended, .cancelled:
            wordcraft_send_hover_gone(host)
        default:
            break
        }
    }

    @objc private func handleTrackpadScroll(_ gesture: UIPanGestureRecognizer) {
        guard let host = host else { return }
        let translation = gesture.translation(in: self)
        let loc = gesture.location(in: self)

        switch gesture.state {
        case .began:
            lastTrackpadTranslation = .zero
            wordcraft_send_mouse(host, Float(loc.x), Float(loc.y), -1, false, 0, 0)
        case .changed:
            let dx = Float(translation.x - lastTrackpadTranslation.x)
            let dy = Float(translation.y - lastTrackpadTranslation.y)
            lastTrackpadTranslation = translation
            wordcraft_send_mouse(host, Float(loc.x), Float(loc.y), -1, false, dx, dy)
        case .ended, .cancelled, .failed:
            lastTrackpadTranslation = .zero
        default:
            break
        }
    }

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
            if hypot(v.x, v.y) > 120.0 {
                scrollVelocity = CGPoint(x: v.x * 0.65, y: v.y * 0.65)
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

    // MARK: - UIPointerInteractionDelegate

    func pointerInteraction(_ interaction: UIPointerInteraction, regionFor request: UIPointerRegionRequest, defaultRegion: UIPointerRegion) -> UIPointerRegion? {
        if let host = host {
            wordcraft_send_mouse(host, Float(request.location.x), Float(request.location.y), -1, false, 0, 0)
        }
        return defaultRegion
    }

    // MARK: - UIScribbleInteractionDelegate

    func scribbleInteractionShouldDelayFocus(_ interaction: UIScribbleInteraction) -> Bool {
        return false
    }

    func scribbleInteractionWillBeginWriting(_ interaction: UIScribbleInteraction) {
        if let host = host {
            wordcraft_set_input_mode(host, 2)
        }
    }
}
