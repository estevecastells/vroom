import AppKit
import SwiftUI
import VroomUI

@main
@MainActor
struct VroomApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model: AppModel

    init() {
        let model = AppModel()
        _model = State(initialValue: model)
        AppDelegate.model = model
    }

    var body: some Scene {
        MenuBarExtra {
            MenuContent(model: model)
        } label: {
            MenuBarLabel(model: model)
        }
        .menuBarExtraStyle(.window)

        Window("Biblioteca de Vroom", id: "library") {
            LibraryView(model: model)
        }
        .defaultSize(width: 900, height: 620)

        Window("Ajustes de Vroom", id: "settings") {
            SettingsView(model: model)
        }
        .windowResizability(.contentSize)
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    static var model: AppModel?

    /// Opening Vroom again (Finder, Spotlight, Launchpad, `open`) while it runs shows the
    /// library. Without this nothing visible happened, which looks broken whenever the menu
    /// bar icon is out of sight: hidden by a menu bar manager such as Thaw, or under the notch.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        if !hasVisibleWindows { Self.model?.openLibrary() }
        return true
    }
}
