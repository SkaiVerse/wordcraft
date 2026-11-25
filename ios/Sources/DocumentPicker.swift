import UIKit
import UniformTypeIdentifiers

/// Manages native iPadOS Files integration for opening and saving documents in place.
final class DocumentPickerManager: NSObject, UIDocumentPickerDelegate {
    private weak var presentingViewController: UIViewController?
    private let hostPointer: OpaquePointer
    private var activeSecurityScopedURL: URL?

    init(presentingViewController: UIViewController, hostPointer: OpaquePointer) {
        self.presentingViewController = presentingViewController
        self.hostPointer = hostPointer
        super.init()
    }

    deinit {
        releaseSecurityScope()
    }

    /// Releases any currently active security-scoped resource access.
    func releaseSecurityScope() {
        if let url = activeSecurityScopedURL {
            url.stopAccessingSecurityScopedResource()
            activeSecurityScopedURL = nil
        }
    }

    /// Presents document picker to open an existing .docx document in place.
    func presentOpenPicker() {
        let contentTypes: [UTType] = [
            UTType(filenameExtension: "docx") ?? .data,
            .plainText
        ]
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: contentTypes, asCopy: false)
        picker.delegate = self
        picker.allowsMultipleSelection = false
        presentingViewController?.present(picker, animated: true)
    }

    /// Presents document picker to export/save the current document to a chosen destination.
    func presentSavePicker(suggestedName: String = "Document.docx") {
        var len: Int = 0
        guard let bytesPtr = wordcraft_get_document_bytes(hostPointer, &len), len > 0 else {
            return
        }
        let data = Data(bytes: bytesPtr, count: len)
        wordcraft_free_bytes(bytesPtr, len)

        let tempURL = FileManager.default.temporaryDirectory.appendingPathComponent(suggestedName)
        do {
            try data.write(to: tempURL, options: .atomic)
            let picker = UIDocumentPickerViewController(forExporting: [tempURL], asCopy: false)
            picker.delegate = self
            presentingViewController?.present(picker, animated: true)
        } catch {
            print("Failed to stage temporary document for export: \(error)")
        }
    }

    // MARK: - UIDocumentPickerDelegate

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        guard let url = urls.first else { return }

        // Securely access the file outside the app sandbox (iCloud Drive / USB storage)
        let didAccess = url.startAccessingSecurityScopedResource()
        if didAccess {
            releaseSecurityScope()
            activeSecurityScopedURL = url
        }

        let path = url.path
        path.withCString { cPath in
            let success = wordcraft_open_document_path(hostPointer, cPath)
            if success == 0 {
                // Fallback to reading raw file bytes if path access is restricted
                if let data = try? Data(contentsOf: url) {
                    url.lastPathComponent.withCString { cName in
                        data.withUnsafeBytes { rawBuffer in
                            if let baseAddress = rawBuffer.bindMemory(to: UInt8.self).baseAddress {
                                _ = wordcraft_open_document_bytes(hostPointer, cName, baseAddress, data.count)
                            }
                        }
                    }
                }
            }
        }
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        controller.dismiss(animated: true)
    }
}
