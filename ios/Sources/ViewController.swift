import UIKit
import GameController

/// Main view controller hosting WordCraft on iPadOS.
final class ViewController: UIViewController {
    private(set) var hostPointer: OpaquePointer?
    private var metalView: MetalView?
    private var keyCommandsManager: KeyCommandsManager?
    private var documentPickerManager: DocumentPickerManager?
    private var scribbleCoordinator: ScribbleAndPencilCoordinator?

    override var keyCommands: [UIKeyCommand]? {
        return keyCommandsManager?.buildKeyCommands()
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground

        let scale = Float(view.window?.windowScene?.screen.scale ?? UIScreen.main.scale)
        let w = Float(view.bounds.width)
        let h = Float(view.bounds.height)

        // Initialize pure-Rust WordCraft engine host
        guard let host = wordcraft_create(scale, w, h) else {
            fatalError("Failed to initialize WordCraft engine host")
        }
        self.hostPointer = host

        // Setup managers
        let scribble = ScribbleAndPencilCoordinator(targetView: view, hostPointer: host)
        self.scribbleCoordinator = scribble
        self.keyCommandsManager = KeyCommandsManager(hostController: self, hostPointer: host)
        self.documentPickerManager = DocumentPickerManager(presentingViewController: self, hostPointer: host)

        // Setup Metal rendering view
        let metal = MetalView(frame: view.bounds, hostPointer: host, scribbleCoordinator: scribble)
        metal.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(metal)
        self.metalView = metal

        setupKeyboardObservers()
        setupGameControllers()
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
        if let host = hostPointer {
            wordcraft_destroy(host)
            hostPointer = nil
        }
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        updateSafeInsets(keyboardHeight: 0.0)
    }

    // MARK: - Safe Area & Virtual Keyboard Bridge

    private func setupKeyboardObservers() {
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(keyboardWillShow(_:)),
            name: UIResponder.keyboardWillShowNotification,
            object: nil
        )
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(keyboardWillHide(_:)),
            name: UIResponder.keyboardWillHideNotification,
            object: nil
        )
    }

    @objc private func keyboardWillShow(_ notification: Notification) {
        guard let userInfo = notification.userInfo,
              let keyboardFrame = userInfo[UIResponder.keyboardFrameEndUserInfoKey] as? CGRect,
              let host = hostPointer else { return }

        let keyboardInView = view.convert(keyboardFrame, from: nil)
        let intersection = view.bounds.intersection(keyboardInView)
        let keyboardHeight = Float(intersection.height)

        updateSafeInsets(keyboardHeight: keyboardHeight)
    }

    @objc private func keyboardWillHide(_ notification: Notification) {
        updateSafeInsets(keyboardHeight: 0.0)
    }

    private func updateSafeInsets(keyboardHeight: Float) {
        guard let host = hostPointer else { return }
        let insets = view.safeAreaInsets
        wordcraft_set_safe_insets(
            host,
            Float(insets.top),
            Float(insets.bottom),
            Float(insets.left),
            Float(insets.right),
            keyboardHeight
        )
    }

    // MARK: - GameController (GCMouse & GCKeyboard)

    private func setupGameControllers() {
        NotificationCenter.default.addObserver(
            forName: .GCMouseDidConnect,
            object: nil,
            queue: .main
        ) { [weak self] note in
            if let mouse = note.object as? GCMouse {
                self?.configureMouse(mouse)
            }
        }

        if let currentMouse = GCMouse.current {
            configureMouse(currentMouse)
        }
    }

    private func configureMouse(_ mouse: GCMouse) {
        guard let mouseInput = mouse.mouseInput else { return }
        var currentX: Float = Float(view.bounds.midX)
        var currentY: Float = Float(view.bounds.midY)

        mouseInput.mouseMovedHandler = { [weak self] _, deltaX, deltaY in
            guard let self = self, let host = self.hostPointer else { return }
            currentX = min(max(currentX + deltaX, 0), Float(self.view.bounds.width))
            currentY = min(max(currentY - deltaY, 0), Float(self.view.bounds.height))
            let isDown = mouseInput.leftButton.isPressed
            let button: Int32 = if mouseInput.rightButton?.isPressed == true {
                1
            } else if isDown {
                0
            } else {
                -1
            }
            wordcraft_send_mouse(host, currentX, currentY, button, isDown, 0, 0)
        }

        mouseInput.scroll.valueChangedHandler = { [weak self] _, scrollX, scrollY in
            guard let self = self, let host = self.hostPointer else { return }
            wordcraft_send_mouse(host, currentX, currentY, -1, false, scrollX * 10.0, scrollY * 10.0)
        }
    }

    // MARK: - Window Resizing (Split View / Stage Manager)

    override func viewWillTransition(to size: CGSize, with coordinator: UIViewControllerTransitionCoordinator) {
        super.viewWillTransition(to: size, with: coordinator)
        coordinator.animate { _ in
            if let host = self.hostPointer {
                let scale = Float(self.view.window?.windowScene?.screen.scale ?? UIScreen.main.scale)
                wordcraft_resize(host, Float(size.width), Float(size.height), scale)
            }
        }
    }

    // MARK: - Document Actions

    func openDocument() {
        documentPickerManager?.presentOpenPicker()
    }

    func saveDocument() {
        documentPickerManager?.presentSavePicker()
    }
}
