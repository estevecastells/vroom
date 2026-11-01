import SwiftUI
import VroomUI

@main
@MainActor
struct VroomApp: App {
    @State private var model = AppModel()

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
