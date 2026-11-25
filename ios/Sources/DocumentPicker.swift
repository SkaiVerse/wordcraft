import UIKit
import UniformTypeIdentifiers

/// Handles native iPadOS Files picker presentation (`UIDocumentPickerViewController`) using
/// sandbox-safe copy/export flows so Open, Save, Save As, Export PDF/PNG, and Insert Picture
/// work reliably under iCloud Drive, On My iPad, USB drives, and third-party providers.
class DocumentPickerManager: NSObject, UIDocumentPickerDelegate {
    private var rawHost: OpaquePointer?
    private var activePickId: UInt64?
    private var activeIsSave: Bool = false
    private var activeSaveName: String = "Document.docx"
    private var pendingTempExportURL: URL?

    /// Presents a native `UIDocumentPickerViewController` driven by a Rust `file.pick` or `file.save_bytes` action.
    func presentPickRequest(host: OpaquePointer, json: [String: Any], from presenter: UIViewController) {
        self.rawHost = host
        let pickId = (json["pick_id"] as? NSNumber)?.uint64Value ?? 0
        let action = (json["action"] as? String) ?? "file.pick"

        if let prevId = activePickId {
            activePickId = nil
            if activeIsSave {
                wordcraft_complete_file_save(host, prevId, false, nil)
            } else {
                wordcraft_finish_open_pick(host, prevId)
            }
        }
        activePickId = pickId

        if action == "file.save_bytes" {
            let fileName = (json["file_name"] as? String) ?? "Document.docx"
            let stagingPath = (json["staging_path"] as? String) ?? ""
            let stagingURL = URL(fileURLWithPath: stagingPath)

            activeIsSave = true
            activeSaveName = fileName
            pendingTempExportURL = stagingURL

            let picker = UIDocumentPickerViewController(forExporting: [stagingURL], asCopy: true)
            picker.delegate = self
            picker.shouldShowFileExtensions = true
            presenter.present(picker, animated: true)
            return
        }

        let kind = (json["kind"] as? String) ?? "open"
        activeIsSave = false
        pendingTempExportURL = nil

        let types: [UTType] = (kind == "folder") ? [.folder] : [.item]
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: types, asCopy: true)
        picker.delegate = self
        picker.allowsMultipleSelection = (kind == "open_many")
        picker.shouldShowFileExtensions = true
        presenter.present(picker, animated: true)
    }

    // MARK: - UIDocumentPickerDelegate

    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        defer { cleanupPendingTemp() }
        guard let host = rawHost else { return }

        if activeIsSave {
            if let pickId = activePickId {
                let name = activeSaveName
                activePickId = nil
                activeIsSave = false
                name.withCString { cName in
                    wordcraft_complete_file_save(host, pickId, true, cName)
                }
            }
            return
        }

        if let pickId = activePickId {
            activePickId = nil
            for url in urls {
                let scoped = url.startAccessingSecurityScopedResource()
                defer {
                    if scoped {
                        url.stopAccessingSecurityScopedResource()
                    }
                }
                if let data = try? Data(contentsOf: url) {
                    let name = url.lastPathComponent
                    name.withCString { cName in
                        data.withUnsafeBytes { rawBuf in
                            if let ptr = rawBuf.baseAddress?.assumingMemoryBound(to: UInt8.self) {
                                wordcraft_push_picked_file_bytes(host, pickId, cName, ptr, data.count)
                            }
                        }
                    }
                }
            }
            wordcraft_finish_open_pick(host, pickId)
        } else if let first = urls.first {
            let scoped = first.startAccessingSecurityScopedResource()
            defer {
                if scoped {
                    first.stopAccessingSecurityScopedResource()
                }
            }
            if let data = try? Data(contentsOf: first) {
                first.lastPathComponent.withCString { cName in
                    data.withUnsafeBytes { rawBuf in
                        if let ptr = rawBuf.baseAddress?.assumingMemoryBound(to: UInt8.self) {
                            _ = wordcraft_open_document_bytes(host, cName, ptr, data.count)
                        }
                    }
                }
            }
        }
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        cleanupPendingTemp()
        guard let host = rawHost, let pickId = activePickId else { return }
        activePickId = nil
        if activeIsSave {
            activeIsSave = false
            wordcraft_complete_file_save(host, pickId, false, nil)
        } else {
            wordcraft_finish_open_pick(host, pickId)
        }
    }

    private func cleanupPendingTemp() {
        if let temp = pendingTempExportURL {
            pendingTempExportURL = nil
            try? FileManager.default.removeItem(at: temp)
        }
    }
}
