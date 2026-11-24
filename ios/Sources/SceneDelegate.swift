import UIKit

class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?

    func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let windowScene = (scene as? UIWindowScene) else { return }

        let window = UIWindow(windowScene: windowScene)
        let viewController = ViewController()
        window.rootViewController = viewController
        self.window = window
        window.makeKeyAndVisible()

        // Handle opening a document passed on launch (e.g. from Files app)
        if let urlContext = connectionOptions.urlContexts.first {
            handleOpenURL(urlContext.url, in: viewController)
        }
    }

    func scene(_ scene: UIScene, openURLContexts URLContexts: Set<UIOpenURLContext>) {
        guard let url = URLContexts.first?.url,
              let viewController = window?.rootViewController as? ViewController else {
            return
        }
        handleOpenURL(url, in: viewController)
    }

    private func handleOpenURL(_ url: URL, in viewController: ViewController) {
        guard let host = viewController.hostPointer else { return }
        let didAccess = url.startAccessingSecurityScopedResource()
        defer {
            if didAccess {
                url.stopAccessingSecurityScopedResource()
            }
        }
        let path = url.path
        path.withCString { cPath in
            _ = wordcraft_open_document_path(host, cPath)
        }
    }
}
