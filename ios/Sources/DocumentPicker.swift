import UIKit
import UniformTypeIdentifiers

/// Handles native iPadOS Files picker presentation (`UIDocumentPickerViewController`) for
/// opening, saving, and exporting documents from iCloud Drive, On My iPad, and external drives,
/// managing security-scoped resource URLs across read/write operations.
class DocumentPickerManager: NSObject, UIDocumentPickerDelegate {
    private weak var hostPtr: AnyObject?
    private var rawHost: OpaquePointer?
    private var activePickId: UInt64?
    private var pendingTempExportURL: URL?
    private var securityScopedURLs: [String: URL] = [:]

    deinit {
        releaseAllSecurityScopes()
    }

    func releaseAllSecurityScopes() {
        for (_, url) in securityScopedURLs {
            url.stopAccessingSecurityScopedResource()
        }
        securityScopedURLs.removeAll()
    }

    /// Presents a native `UIDocumentPickerViewController` driven by a Rust `file.pick` platform action.
    func presentPickRequest(host: OpaquePointer, json: [String: Any], from presenter: UIViewController) {
        self.rawHost = host
        let pickId = (json["pick_id"] as? NSNumber)?.uint64Value ?? 0
        let kind = (json["kind"] as? String) ?? "open"
        let fileName = (json["file_name"] as? String) ?? "Document.docx"
        let exts = (json["extensions"] as? [String]) ?? []

        // If another picker is somehow still registered, cancel it cleanly first.
        if let prevId = activePickId {
            activePickId = nil
            wordcraft_complete_file_pick(host, prevId, nil)
        }
        activePickId = pickId

        if kind == "save" {
            let safeName = fileName.isEmpty ? "Document.docx" : fileName
            let tempURL = FileManager.default.temporaryDirectory.appendingPathComponent(safeName)
            try? FileManager.default.removeItem(at: tempURL)

            // Provide valid initial DOCX payload when saving a Word document so Files previews cleanly;
            // Rust overwrites the final destination with the exact target format once chosen.
            if safeName.lowercased().hasSuffix(".docx") {
                var len: Int = 0
                if let ptr = wordcraft_get_document_bytes(host, &len) {
                    let data = Data(bytes: ptr, count: len)
                    wordcraft_free_bytes(ptr, len)
                    try? data.write(to: tempURL)
                } else {
                    FileManager.default.createFile(atPath: tempURL.path, contents: Data())
                }
            } else {
                FileManager.default.createFile(atPath: tempURL.path, contents: Data())
            }

            pendingTempExportURL = tempURL
            let picker = UIDocumentPickerViewController(forExporting: [tempURL], asCopy: false)
            picker.delegate = self
            picker.shouldShowFileExtensions = true
            presenter.present(picker, animated: true)
            return
        }

        let types: [UTType]
        if kind == "folder" {
            types = [.folder]
        } else {
            types = Self.resolveContentTypes(for: exts)
        }

        let picker = UIDocumentPickerViewController(forOpeningContentTypes: types, asCopy: false)
        picker.delegate = self
        picker.allowsMultipleSelection = (kind == "open_many")
        picker.shouldShowFileExtensions = true
        presenter.present(picker, animated: true)
    }

    private static func resolveContentTypes(for extensions: [String]) -> [UTType] {
        var resolved: [UTType] = []
        for ext in extensions {
            let clean = ext.trimmingCharacters(in: CharacterSet(charactersIn: "."))
            if clean.isEmpty || clean == "*" {
                return [.item]
            }
            if let ut = UTType(filenameExtension: clean) {
                resolved.append(ut)
            }
        }
        if resolved.isEmpty {
            let docx = UTType(filenameExtension: "docx") ?? .data
            return [docx, .plainText, .pdf, .rtf, .image, .item]
        }
        return resolved
    }

    // MARK: - UIDocumentPickerDelegate

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        defer { cleanupPendingTemp() }
        guard let host = rawHost else { return }

        var chosenPaths: [String] = []
        for url in urls {
            if url.startAccessingSecurityScopedResource() {
                securityScopedURLs[url.path] = url
            }
            chosenPaths.append(url.path)
        }

        if let pickId = activePickId {
            activePickId = nil
            if let data = try? JSONSerialization.data(withJSONObject: chosenPaths),
               let jsonStr = String(data: data, encoding: .utf8) {
                jsonStr.withCString { cStr in
                    wordcraft_complete_file_pick(host, pickId, cStr)
                }
            } else {
                wordcraft_complete_file_pick(host, pickId, nil)
            }
        } else if let first = urls.first {
            // Direct OS launch / external open
            first.path.withCString { cPath in
                _ = wordcraft_open_document_path(host, cPath)
            }
        }
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        cleanupPendingTemp()
        if let host = rawHost, let pickId = activePickId {
            activePickId = nil
            wordcraft_complete_file_pick(host, pickId, nil)
        }
    }

    private func cleanupPendingTemp() {
        if let temp = pendingTempExportURL {
            pendingTempExportURL = nil
            try? FileManager.default.removeItem(at: temp)
        }
    }
}
