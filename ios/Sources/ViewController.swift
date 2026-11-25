import UIKit
import GameController

/// Main root ViewController hosting WordCraft's GPU `MetalView` and bridging iPadOS system events
/// (fullscreen status-bar suppression, keyboard insets, GameController mice/trackpads,
/// Hold-Cmd UIKeyCommands, file pickers, pasteboard, and CoreText fonts).
class ViewController: UIViewController {
    var metalView: MetalView!
    var host: OpaquePointer?
    var hostPointer: OpaquePointer? { host }
    var keyCommandsManager: KeyCommandsManager?
    let documentPicker = DocumentPickerManager()

    private var keyboardHeight: CGFloat = 0

    override var prefersStatusBarHidden: Bool {
        return true
    }

    override var preferredScreenEdgesDeferringSystemGestures: UIRectEdge {
        return [.top, .bottom]
    }

    override var canBecomeFirstResponder: Bool {
        return true
    }

    override var keyCommands: [UIKeyCommand]? {
        return keyCommandsManager?.buildKeyCommands()
    }

    @objc func handleKeyCommand(_ sender: UIKeyCommand) {
        keyCommandsManager?.executeKeyCommand(sender)
    }

    override func loadView() {
        metalView = MetalView(frame: UIScreen.main.bounds)
        metalView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view = metalView
    }

    override func viewDidLoad() {
        super.viewDidLoad()

        let scale = Float(view.window?.screen.scale ?? UIScreen.main.scale)
        let width = Float(view.bounds.width)
        let height = Float(view.bounds.height)

        guard let createdHost = wordcraft_create(scale, width, height) else {
            fatalError("Failed to create WordCraftHost")
        }
        self.host = createdHost
        metalView.host = createdHost
        metalView.onPlatformAction = { [weak self] json in
            self?.handlePlatformAction(json)
        }

        // Bind CAMetalLayer pointer directly to wgpu Metal surface
        let layerPtr = Unmanaged.passUnretained(metalView.metalLayer).toOpaque()
        wordcraft_set_surface(createdHost, layerPtr)

        keyCommandsManager = KeyCommandsManager(host: createdHost, viewController: self)

        // Register all iPadOS system & user-installed fonts in CoreText
        FontScanner.registerInstalledFonts(host: createdHost)

        syncSystemPasteboard()
        observeSystemEvents()
        metalView.startRenderLoop()
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        setNeedsStatusBarAppearanceUpdate()
        syncSystemPasteboard()
        updateSafeInsets()
        if !metalView.isFirstResponder {
            metalView.becomeFirstResponder()
        }
    }

    override func viewSafeAreaInsetsDidChange() {
        super.viewSafeAreaInsetsDidChange()
        updateSafeInsets()
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        updateSafeInsets()
    }

    private func updateSafeInsets() {
        guard let host = host else { return }
        let insets = view.safeAreaInsets
        wordcraft_set_safe_insets(
            host,
            Float(insets.top),
            Float(insets.bottom),
            Float(insets.left),
            Float(insets.right),
            Float(keyboardHeight)
        )
    }

    private func syncSystemPasteboard() {
        guard let host = host else { return }
        if let text = UIPasteboard.general.string {
            text.withCString { cText in
                wordcraft_sync_pasteboard_text(host, cText)
            }
        } else {
            wordcraft_sync_pasteboard_text(host, nil)
        }
    }

    private func handlePlatformAction(_ json: String) {
        guard let host = host,
              let data = json.data(using: .utf8),
              let dict = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let action = dict["action"] as? String else { return }

        switch action {
        case "file.pick":
            documentPicker.presentPickRequest(host: host, json: dict, from: self)
        case "clipboard.set":
            let text = (dict["text"] as? String) ?? ""
            UIPasteboard.general.string = text
        case "keyboard.hide":
            keyboardHeight = 0
            updateSafeInsets()
            metalView.resignFirstResponder()
        default:
            break
        }
    }

    private func observeSystemEvents() {
        let nc = NotificationCenter.default
        nc.addObserver(self, selector: #selector(keyboardWillShow(_:)), name: UIResponder.keyboardWillShowNotification, object: nil)
        nc.addObserver(self, selector: #selector(keyboardWillHide(_:)), name: UIResponder.keyboardWillHideNotification, object: nil)
        nc.addObserver(self, selector: #selector(appDidBecomeActive(_:)), name: UIApplication.didBecomeActiveNotification, object: nil)
        nc.addObserver(self, selector: #selector(mouseDidConnect(_:)), name: .GCMouseDidConnect, object: nil)

        for mouse in GCMouse.mice() {
            attachMouseHandler(mouse)
        }
    }

    @objc private func appDidBecomeActive(_ notification: Notification) {
        syncSystemPasteboard()
    }

    @objc private func keyboardWillShow(_ notification: Notification) {
        guard let frame = (notification.userInfo?[UIResponder.keyboardFrameEndUserInfoKey] as? NSValue)?.cgRectValue else { return }
        let localFrame = view.convert(frame, from: nil)
        let overlap = max(0, view.bounds.maxY - localFrame.minY)
        keyboardHeight = overlap
        updateSafeInsets()
    }

    @objc private func keyboardWillHide(_ notification: Notification) {
        keyboardHeight = 0
        updateSafeInsets()
    }

    @objc private func mouseDidConnect(_ notification: Notification) {
        if let mouse = notification.object as? GCMouse {
            attachMouseHandler(mouse)
        }
    }

    private func attachMouseHandler(_ mouse: GCMouse) {
        guard let input = mouse.mouseInput else { return }
        input.mouseMovedHandler = { [weak self] _, dx, dy in
            guard let self = self, let host = self.host else { return }
            let center = CGPoint(x: self.view.bounds.midX + CGFloat(dx), y: self.view.bounds.midY - CGFloat(dy))
            wordcraft_send_mouse(host, Float(center.x), Float(center.y), -1, false, 0, 0)
        }
        input.scroll.valueChangedHandler = { [weak self] _, xValue, yValue in
            guard let self = self, let host = self.host else { return }
            wordcraft_send_mouse(host, Float(self.view.bounds.midX), Float(self.view.bounds.midY), -1, false, xValue * 10.0, yValue * 10.0)
        }
        input.leftButton.pressedChangedHandler = { [weak self] _, _, pressed in
            guard let self = self, let host = self.host else { return }
            wordcraft_send_mouse(host, Float(self.view.bounds.midX), Float(self.view.bounds.midY), 0, pressed, 0, 0)
        }
        input.rightButton?.pressedChangedHandler = { [weak self] _, _, pressed in
            guard let self = self, let host = self.host else { return }
            wordcraft_send_mouse(host, Float(self.view.bounds.midX), Float(self.view.bounds.midY), 1, pressed, 0, 0)
        }
    }

    func openIncomingURL(_ url: URL) {
        guard let host = host else { return }
        let didAccess = url.startAccessingSecurityScopedResource()
        defer {
            if didAccess {
                url.stopAccessingSecurityScopedResource()
            }
        }
        if let data = try? Data(contentsOf: url) {
            data.withUnsafeBytes { rawBuf in
                if let ptr = rawBuf.baseAddress?.assumingMemoryBound(to: UInt8.self) {
                    _ = wordcraft_open_document_bytes(host, url.lastPathComponent, ptr, data.count)
                }
            }
        }
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
        metalView?.stopRenderLoop()
        if let host = host {
            wordcraft_destroy(host)
        }
    }
}
