import UIKit

/// Coordinates Apple Pencil Scribble handwriting recognition and ink annotations.
final class ScribbleAndPencilCoordinator: NSObject, UIScribbleInteractionDelegate {
    private weak var targetView: UIView?
    private let hostPointer: OpaquePointer
    private var scribbleInteraction: UIScribbleInteraction?

    // Active pencil stroke buffer
    private var isRecordingStroke = false
    private var currentStrokePoints: [Float] = []
    private var activePage: Int32 = 1

    init(targetView: UIView, hostPointer: OpaquePointer) {
        self.targetView = targetView
        self.hostPointer = hostPointer
        super.init()

        // Attach iPadOS Scribble interaction
        let interaction = UIScribbleInteraction(delegate: self)
        targetView.addInteraction(interaction)
        self.scribbleInteraction = interaction
    }

    // MARK: - Apple Pencil Scribble (Handwriting-to-Text)

    /// Inserts text recognized from Apple Pencil handwriting directly into WordCraft's selection.
    func insertScribbleText(_ text: String) {
        text.withCString { cText in
            _ = wordcraft_insert_text_at_selection(hostPointer, cText)
        }
    }

    // MARK: - UIScribbleInteractionDelegate

    func scribbleInteraction(_ interaction: UIScribbleInteraction, shouldBeginAt location: CGPoint) -> Bool {
        // Allow Scribble handwriting whenever touching document text canvas
        return true
    }

    // MARK: - Apple Pencil Ink Strokes

    /// Begins recording an Apple Pencil freehand ink stroke.
    func handlePencilBegan(at point: CGPoint, force: CGFloat, page: Int32) {
        isRecordingStroke = true
        activePage = page
        currentStrokePoints = [Float(point.x), Float(point.y)]
    }

    /// Appends point to the active pencil stroke.
    func handlePencilMoved(to point: CGPoint, force: CGFloat) {
        guard isRecordingStroke else { return }
        currentStrokePoints.append(Float(point.x))
        currentStrokePoints.append(Float(point.y))
    }

    /// Finishes recording and dispatches the ink stroke to WordCraft's draw engine.
    func handlePencilEnded(at point: CGPoint, tool: String = "pen", color: String = "000000", width: Float = 2.0) {
        guard isRecordingStroke else { return }
        currentStrokePoints.append(Float(point.x))
        currentStrokePoints.append(Float(point.y))

        let count = currentStrokePoints.count / 2
        currentStrokePoints.withUnsafeBufferPointer { buffer in
            if let baseAddress = buffer.baseAddress {
                tool.withCString { cTool in
                    color.withCString { cColor in
                        _ = wordcraft_send_pencil_stroke(
                            hostPointer,
                            activePage,
                            baseAddress,
                            count,
                            cTool,
                            cColor,
                            width
                        )
                    }
                }
            }
        }

        isRecordingStroke = false
        currentStrokePoints.removeAll()
    }

    func handlePencilCancelled() {
        isRecordingStroke = false
        currentStrokePoints.removeAll()
    }
}
