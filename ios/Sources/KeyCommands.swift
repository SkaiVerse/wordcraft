import UIKit

/// Bridges WordCraft's Mac command catalog to iPadOS `UIKeyCommand` for native Hold-Cmd HUD discovery.
class KeyCommandsManager: NSObject {
    private weak var hostViewController: UIViewController?
    private var hostPtr: OpaquePointer?

    private struct CommandEntry: Decodable {
        let id: String
        let title: String
        let input: String
        let hid: Int32
        let modifiers: UInt32
    }

    init(host: OpaquePointer, viewController: UIViewController) {
        self.hostPtr = host
        self.hostViewController = viewController
        super.init()
    }

    /// Generates `[UIKeyCommand]` array mapped dynamically from WordCraft's command registry.
    func buildKeyCommands() -> [UIKeyCommand] {
        guard let host = hostPtr,
              let jsonPtr = wordcraft_get_key_commands_json(host) else {
            return []
        }
        let jsonString = String(cString: jsonPtr)
        wordcraft_free_string(jsonPtr)

        guard let data = jsonString.data(using: .utf8),
              let entries = try? JSONDecoder().decode([CommandEntry].self, from: data) else {
            return []
        }

        return entries.map { entry in
            var flags: UIKeyModifierFlags = []
            if (entry.modifiers & 1) != 0 { flags.insert(.alternate) }
            if (entry.modifiers & 2) != 0 { flags.insert(.control) }
            if (entry.modifiers & 4) != 0 { flags.insert(.shift) }
            if (entry.modifiers & 8) != 0 { flags.insert(.command) }

            let payload: [String: Any] = [
                "id": entry.id,
                "hid": Int(entry.hid),
                "modifiers": Int(entry.modifiers),
                "input": entry.input,
            ]
            let cmd = UIKeyCommand(
                title: entry.title,
                image: nil,
                action: #selector(ViewController.handleKeyCommand(_:)),
                input: entry.input,
                modifierFlags: flags,
                propertyList: payload
            )
            cmd.wantsPriorityOverSystemBehavior = true
            return cmd
        }
    }

    func executeKeyCommand(_ sender: UIKeyCommand) {
        guard let host = hostPtr,
              let payload = sender.propertyList as? [String: Any],
              let hid = payload["hid"] as? Int,
              let mods = payload["modifiers"] as? Int else { return }

        let input = (payload["input"] as? String) ?? ""
        if (mods & 8) != 0 && input.lowercased() == "v" {
            if let text = UIPasteboard.general.string {
                text.withCString { cText in
                    wordcraft_sync_pasteboard_text(host, cText)
                }
            } else {
                wordcraft_sync_pasteboard_text(host, nil)
            }
        }

        wordcraft_send_key_tap(host, Int32(hid), UInt32(mods))
    }
}
