import UIKit
import GameController

/// Main root ViewController hosting WordCraft's GPU `MetalView` and bridging iPadOS system events
/// (fullscreen status-bar suppression, keyboard insets, Hold-Cmd UIKeyCommands,
/// file pickers, pasteboard sync, and CoreText fonts).
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

        documentPicker.onFinished = { [weak self] in
            self?.restoreKeyboardResponder()
        }

        let layerPtr = Unmanaged.passUnretained(metalView.metalLayer).toOpaque()
        wordcraft_set_surface(createdHost, layerPtr)

        keyCommandsManager = KeyCommandsManager(host: createdHost, viewController: self)
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
        restoreKeyboardResponder()
    }

    private func restoreKeyboardResponder() {
        if presentedViewController == nil && !metalView.isFirstResponder {
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
        case "file.pick", "file.save_bytes":
            documentPicker.presentPickRequest(host: host, json: dict, from: self)
        case "clipboard.set":
            let text = (dict["text"] as? String) ?? ""
            UIPasteboard.general.string = text
        case "keyboard.hide":
            keyboardHeight = 0
            updateSafeInsets()
            // Hide the on-screen keyboard while keeping MetalView as firstResponder for physical keyboards
            metalView.setSoftwareKeyboardVisible(false)
        default:
            break
        }
    }

    private func observeSystemEvents() {
        let nc = NotificationCenter.default
        nc.addObserver(self, selector: #selector(keyboardWillShow(_:)), name: UIResponder.keyboardWillShowNotification, object: nil)
        nc.addObserver(self, selector: #selector(keyboardWillHide(_:)), name: UIResponder.keyboardWillHideNotification, object: nil)
        nc.addObserver(self, selector: #selector(appDidBecomeActive(_:)), name: UIApplication.didBecomeActiveNotification, object: nil)
        nc.addObserver(self, selector: #selector(appWillResignActive(_:)), name: UIApplication.willResignActiveNotification, object: nil)
        nc.addObserver(self, selector: #selector(windowDidBecomeKey(_:)), name: UIWindow.didBecomeKeyNotification, object: nil)
        nc.addObserver(self, selector: #selector(hardwareKeyboardDidConnect(_:)), name: NSNotification.Name.GCKeyboardDidConnect, object: nil)
        nc.addObserver(self, selector: #selector(hardwareKeyboardDidDisconnect(_:)), name: NSNotification.Name.GCKeyboardDidDisconnect, object: nil)
    }

    @objc private func appDidBecomeActive(_ notification: Notification) {
        syncSystemPasteboard()
        metalView.resetHardwareKeyboardState()
        restoreKeyboardResponder()
    }

    @objc private func appWillResignActive(_ notification: Notification) {
        metalView.resetHardwareKeyboardState()
    }

    @objc private func windowDidBecomeKey(_ notification: Notification) {
        restoreKeyboardResponder()
    }

    @objc private func hardwareKeyboardDidDisconnect(_ notification: Notification) {
        metalView.resetHardwareKeyboardState()
    }

    @objc private func hardwareKeyboardDidConnect(_ notification: Notification) {
        metalView.resetHardwareKeyboardState()
        guard presentedViewController == nil else { return }
        // Re-bind responder & UITextInput after Bluetooth multi-host switch (PC -> iPad)
        // so `pressesBegan` and `UIKeyCommand` resume immediately without needing a screen tap.
        if metalView.isFirstResponder {
            metalView.resignFirstResponder()
        }
        metalView.becomeFirstResponder()
        metalView.reloadInputViews()
    }

    // Fallback: if UIKit moves focus off MetalView after keyboard reconnect/system UI, re-claim responder and forward keys.
    override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if presentedViewController == nil && !metalView.isFirstResponder {
            metalView.becomeFirstResponder()
            if metalView.forwardHardwarePressesBegan(presses) {
                return
            }
        }
        super.pressesBegan(presses, with: event)
    }

    override func pressesChanged(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if presentedViewController == nil && !metalView.isFirstResponder && metalView.forwardHardwarePressesChanged(presses) {
            return
        }
        super.pressesChanged(presses, with: event)
    }

    override func pressesEnded(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        if presentedViewController == nil && !metalView.isFirstResponder && metalView.forwardHardwarePressesEnded(presses) {
            return
        }
        super.pressesEnded(presses, with: event)
    }

    override func pressesCancelled(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
        metalView.resetHardwareKeyboardState()
        super.pressesCancelled(presses, with: event)
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
