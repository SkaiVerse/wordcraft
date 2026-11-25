import UIKit

/// Model representing a command discoverable in iPadOS Hold-Cmd shortcut overlay.
struct WordCraftKeyCommand: Decodable {
    let id: String
    let title: String
    let input: String
    let modifiers: UInt32
}

/// Manages registration and dispatch of native iPadOS `UIKeyCommand`s.
final class KeyCommandsManager: NSObject {
    private weak var hostController: UIViewController?
    private let hostPointer: OpaquePointer

    init(hostController: UIViewController, hostPointer: OpaquePointer) {
        self.hostController = hostController
        self.hostPointer = hostPointer
        super.init()
    }

    /// Builds the list of native UIKeyCommands from WordCraft's central command registry.
    func buildKeyCommands() -> [UIKeyCommand] {
        guard let cString = wordcraft_get_key_commands_json(hostPointer) else {
            return []
        }
        defer { wordcraft_free_string(cString) }

        let jsonString = String(cString: cString)
        guard let data = jsonString.data(using: .utf8),
              let commands = try? JSONDecoder().decode([WordCraftKeyCommand].self, from: data) else {
            return []
        }

        return commands.map { cmd in
            var flags: UIKeyModifierFlags = []
            if (cmd.modifiers & 1) != 0 { flags.insert(.alternate) }
            if (cmd.modifiers & 2) != 0 { flags.insert(.control) }
            if (cmd.modifiers & 4) != 0 { flags.insert(.shift) }
            if (cmd.modifiers & 8) != 0 { flags.insert(.command) }

            let action = #selector(handleKeyCommand(_:))
            let keyCommand = UIKeyCommand(
                title: cmd.title,
                image: nil,
                action: action,
                input: cmd.input,
                modifierFlags: flags,
                propertyList: cmd.id
            )
            keyCommand.discoverabilityTitle = cmd.title
            return keyCommand
        }
    }

    /// Handles execution of a selected key command.
    @objc func handleKeyCommand(_ sender: UIKeyCommand) {
        guard let cmdId = sender.propertyList as? String else { return }
        cmdId.withCString { cCmd in
            "{}" .withCString { cParams in
                if let response = wordcraft_dispatch_command(hostPointer, cCmd, cParams) {
                    wordcraft_free_string(response)
                }
            }
        }
    }
}
