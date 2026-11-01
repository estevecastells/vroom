import AppKit
import SwiftUI
import VroomCapture
import VroomKit

public struct MenuBarLabel: View {
    let model: AppModel

    public init(model: AppModel) { self.model = model }

    public var body: some View {
        switch model.phase {
        case .recording(_, nil, _): Image(systemName: "record.circle.fill").symbolRenderingMode(.multicolor)
        case .recording: Image(systemName: "pause.circle")
        case .preparing, .saving: Image(systemName: "ellipsis.circle")
        case .idle: Image(systemName: model.syncing ? "icloud.and.arrow.up" : "video.circle")
        }
    }
}

@MainActor
public struct MenuContent: View {
    @Bindable var model: AppModel
    @Environment(\.openWindow) private var openWindow

    @State private var cameras: [Device] = []
    @State private var microphones: [Device] = []
    @State private var displays: [DisplayInfo] = []

    public init(model: AppModel) { self.model = model }

    public var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Vroom").font(.headline)
                Spacer()
                if model.syncing { Label("Subiendo…", systemImage: "arrow.up.circle").font(.caption).foregroundStyle(.secondary) }
            }
            recordButton
            if !model.canRecordScreen && model.phase == .idle { permissionHint }
            if model.phase == .idle { sources }
            if let error = model.lastError {
                HStack(alignment: .top) {
                    Text(error).font(.caption).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true)
                    Spacer()
                    Button { model.clearError() } label: { Image(systemName: "xmark") }.buttonStyle(.borderless)
                }
            }
            if let toast = model.toast { Text(toast).font(.caption).foregroundStyle(.secondary) }
            if !model.cloudReady {
                Button { show("settings") } label: {
                    Label("Conecta Cloudflare R2 para guardar copia en la nube", systemImage: "icloud.slash")
                        .font(.caption)
                }
                .buttonStyle(.link)
            }
            Divider()
            recent
            Divider()
            HStack {
                Button("Biblioteca") { show("library") }
                Spacer()
                Button("Ajustes…") { show("settings") }
                Spacer()
                Button("Salir") { NSApplication.shared.terminate(nil) }
            }
            .controlSize(.small)
        }
        .padding(14)
        .frame(width: 320)
        .onAppear {
            model.checkPermissions()
            cameras = Devices.cameras()
            microphones = Devices.microphones()
            displays = Devices.displays()
        }
    }

    private var recordButton: some View {
        Button { model.toggleRecording() } label: {
            HStack {
                Image(systemName: model.isRecording ? "stop.fill" : "record.circle")
                Text(recordTitle)
                Spacer()
                Text("⌥⇧R").font(.caption).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity)
        }
        .controlSize(.large)
        .buttonStyle(.borderedProminent)
        .tint(model.isRecording ? .red : .accentColor)
        .disabled(model.phase == .saving || model.cli == nil)
    }

    private var permissionHint: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label("Falta el permiso de Grabación de pantalla y audio del sistema", systemImage: "lock.shield")
                .font(.callout.weight(.medium))
            Text("Actívalo para Vroom en Ajustes del Sistema y luego reabre la app: macOS solo lo aplica al reiniciarla.")
                .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            HStack {
                Button("Abrir Ajustes del Sistema") { model.openScreenRecordingSettings() }
                Button("Reabrir Vroom") { model.relaunch() }
            }
            .controlSize(.small)
        }
        .padding(10)
        .background(.orange.opacity(0.12), in: RoundedRectangle(cornerRadius: 8))
    }

    private var recordTitle: String {
        switch model.phase {
        case .idle: "Grabar"
        case .preparing: "Cancelar"
        case .recording: "Terminar"
        case .saving: "Guardando…"
        }
    }

    private var sources: some View {
        VStack(alignment: .leading, spacing: 6) {
            Picker("Qué grabar", selection: $model.prefs.captureMode) {
                Label("Pantalla", systemImage: "display").tag(CaptureMode.screen)
                Label("Ventana", systemImage: "macwindow").tag(CaptureMode.window)
                Label("Área", systemImage: "rectangle.dashed").tag(CaptureMode.region)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .padding(.bottom, 2)
            if model.prefs.captureMode == .screen {
                Picker(selection: $model.prefs.displayID) {
                    Text("Pantalla bajo el ratón").tag(CGDirectDisplayID?.none)
                    ForEach(displays) { Text($0.name).tag(Optional($0.id)) }
                } label: { Image(systemName: "display").frame(width: 18) }
            } else {
                Text(model.prefs.captureMode == .window
                     ? "Al pulsar Grabar, haz clic en la ventana que quieras."
                     : "Al pulsar Grabar, arrastra para elegir el área.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Picker(selection: $model.prefs.cameraID) {
                Text("Sin cámara").tag(String?.none)
                ForEach(cameras) { Text($0.name).tag(Optional($0.id)) }
            } label: { Image(systemName: "video").frame(width: 18) }
            Picker(selection: $model.prefs.microphoneID) {
                Text("Sin micrófono").tag(String?.none)
                ForEach(microphones) { Text($0.name).tag(Optional($0.id)) }
            } label: { Image(systemName: "mic").frame(width: 18) }
            Toggle(isOn: $model.prefs.systemAudio) {
                Label("Audio del sistema", systemImage: "speaker.wave.2")
            }
            .toggleStyle(.checkbox)
            .help("Graba lo que suena en el Mac (llamadas, vídeos, otras apps), sin los sonidos de Vroom")
        }
        .pickerStyle(.menu)
        .controlSize(.small)
    }

    @ViewBuilder
    private var recent: some View {
        let items = model.entries.filter { $0.status != .recording }.prefix(4)
        if items.isEmpty {
            Text("Aún no hay grabaciones.").font(.callout).foregroundStyle(.secondary)
        } else {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(items)) { entry in
                    HStack(spacing: 8) {
                        Button { model.play(entry) } label: {
                            VStack(alignment: .leading, spacing: 1) {
                                Text(entry.title).lineLimit(1)
                                Text(Format.subtitle(entry)).font(.caption2).foregroundStyle(.secondary)
                            }
                        }
                        .buttonStyle(.plain)
                        Spacer()
                        StatusIcon(entry: entry, progress: model.transfers[entry.id])
                        Button { model.copyLink(entry) } label: { Image(systemName: "link") }
                            .buttonStyle(.borderless)
                            .disabled(!model.cloudReady)
                            .help("Copiar enlace")
                    }
                }
            }
        }
    }

    /// A menu bar app's windows only come to the front while it is a regular app.
    private func show(_ id: String) {
        NSApp.setActivationPolicy(.regular)
        openWindow(id: id)
        NSApp.activate(ignoringOtherApps: true)
        DispatchQueue.main.async {
            NSApp.windows.first { $0.identifier?.rawValue.contains(id) == true }?.makeKeyAndOrderFront(nil)
        }
    }
}

struct StatusIcon: View {
    let entry: Entry
    let progress: Double?

    var body: some View {
        Group {
            if let p = progress ?? (entry.status == .uploading ? entry.progress : nil) {
                ProgressView(value: p).progressViewStyle(.circular).controlSize(.mini)
            } else {
                switch entry.status {
                case .recording: Image(systemName: "record.circle").foregroundStyle(.red)
                case .local: Image(systemName: entry.error == nil ? "internaldrive" : "exclamationmark.icloud")
                    .foregroundStyle(entry.error == nil ? Color.secondary : Color.orange)
                case .uploading: Image(systemName: "icloud.and.arrow.up").foregroundStyle(.secondary)
                case .synced: Image(systemName: "checkmark.icloud").foregroundStyle(.green)
                case .cloud: Image(systemName: "icloud").foregroundStyle(.blue)
                }
            }
        }
        .frame(width: 16, height: 16)
        .help(Format.status(entry))
    }
}

enum Format {
    static let bytes: ByteCountFormatter = {
        let f = ByteCountFormatter()
        f.countStyle = .file
        return f
    }()

    static func subtitle(_ e: Entry) -> String {
        let date = e.createdAt.formatted(.dateTime.day().month(.abbreviated).hour().minute())
        return "\(date) · \(Elapsed.format(e.durationSecs))"
    }

    static func status(_ e: Entry) -> String {
        switch e.status {
        case .recording: return "Grabando"
        case .local: return e.error.map { "Solo en este Mac. Último error: \($0)" } ?? "Solo en este Mac"
        case .uploading: return "Subiendo a R2" + (e.error.map { ". Último error: \($0)" } ?? "")
        case .synced: return "En este Mac y en R2"
        case .cloud: return "Solo en R2"
        }
    }
}
