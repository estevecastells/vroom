import AppKit
import SwiftUI
import VroomCapture
import VroomKit

@MainActor
public struct SettingsView: View {
    @Bindable var model: AppModel

    public init(model: AppModel) { self.model = model }

    public var body: some View {
        TabView {
            RecordingSettings(model: model).tabItem { Label("Grabación", systemImage: "record.circle") }
            CloudSettings(model: model).tabItem { Label("Cloudflare R2", systemImage: "icloud") }
            LibrarySettings(model: model).tabItem { Label("Biblioteca", systemImage: "folder") }
            TranscriptionSettings(model: model).tabItem { Label("Transcripción", systemImage: "captions.bubble") }
            LoomImport(model: model).tabItem { Label("Importar de Loom", systemImage: "square.and.arrow.down") }
        }
        .frame(width: 560)
        .padding(20)
        .task { await model.reloadConfig() }
    }
}

@MainActor
private struct RecordingSettings: View {
    @Bindable var model: AppModel

    var body: some View {
        Form {
            Picker("Resolución", selection: $model.prefs.maxHeight) {
                Text("720p").tag(720)
                Text("1080p").tag(1080)
                Text("1440p").tag(1440)
                Text("Nativa (Retina)").tag(0)
            }
            Picker("Fotogramas", selection: $model.prefs.fps) {
                Text("30 fps").tag(30)
                Text("60 fps").tag(60)
            }
            Picker("Códec", selection: $model.prefs.codec) {
                Text("H.264 (se ve en cualquier navegador)").tag(RecorderSettings.Codec.h264)
                Text("HEVC (archivos ~40 % más pequeños)").tag(RecorderSettings.Codec.hevc)
            }
            Toggle("Cuenta atrás de 3 segundos", isOn: $model.prefs.countdown)
            Toggle("Abrir al iniciar sesión", isOn: Binding(get: { model.openAtLogin }, set: { model.setOpenAtLogin($0) }))
            Text("⌥⇧R empieza y termina una grabación desde cualquier app. La cámara aparece como una burbuja que puedes arrastrar; la barra de control no sale en el vídeo.")
                .font(.caption).foregroundStyle(.secondary)
        }
        .formStyle(.grouped)
    }
}

@MainActor
private struct CloudSettings: View {
    let model: AppModel
    @State private var accountID = ""
    @State private var bucket = "vroom"
    @State private var accessKeyID = ""
    @State private var secret = ""
    @State private var publicURL = ""
    @State private var prefix = ""
    @State private var busy = false
    @State private var message: String?
    @State private var failed = false
    @State private var restoreMessage: String?

    var body: some View {
        Form {
            Section {
                TextField("Account ID", text: $accountID, prompt: Text("32 caracteres, en el panel de R2"))
                TextField("Bucket", text: $bucket)
                TextField("Access Key ID", text: $accessKeyID)
                SecureField("Secret Access Key", text: $secret,
                            prompt: Text(model.config?.hasSecret == true ? "guardado en el llavero" : ""))
                TextField("URL pública (opcional)", text: $publicURL, prompt: Text("https://videos.tudominio.com"))
                TextField("Prefijo (opcional)", text: $prefix, prompt: Text("p. ej. esteve/"))
            } header: {
                Text("Conexión")
            } footer: {
                Text("Crea el bucket y un token de API de R2 con permiso «Object Read & Write» en Cloudflare › R2 › Manage API tokens. Con una URL pública (dominio propio o r2.dev) los enlaces no caducan y abren una página con el vídeo; sin ella son enlaces firmados de 7 días. El secret se guarda en el llavero de macOS.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            HStack {
                if let message {
                    Text(message).font(.caption).foregroundStyle(failed ? .red : .green)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Spacer()
                if model.config?.config.r2 != nil {
                    Button("Desconectar") { Task { await model.disconnect() } }
                }
                Button(busy ? "Comprobando…" : "Conectar") { connect() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(busy || accountID.isEmpty || bucket.isEmpty || accessKeyID.isEmpty
                              || (secret.isEmpty && model.config?.hasSecret != true))
            }
            Section {
                HStack {
                    Text("¿Ordenador nuevo? Recupera la lista de grabaciones desde R2; los vídeos se descargan cuando los abras.")
                        .font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Button("Restaurar desde R2") {
                        Task { restoreMessage = await model.restore() }
                    }
                    .disabled(!model.cloudReady)
                }
                if let restoreMessage { Text(restoreMessage).font(.caption) }
            }
        }
        .formStyle(.grouped)
        .onAppear(perform: load)
        .onChange(of: model.config) { load() }
    }

    private func load() {
        guard let r2 = model.config?.config.r2 else { return }
        accountID = r2.accountId
        bucket = r2.bucket
        accessKeyID = r2.accessKeyId
        publicURL = r2.publicUrl ?? ""
        prefix = r2.prefix ?? ""
    }

    private func connect() {
        busy = true
        message = nil
        Task {
            let error = await model.connect(
                accountID: accountID.trimmingCharacters(in: .whitespaces), bucket: bucket.trimmingCharacters(in: .whitespaces),
                accessKeyID: accessKeyID.trimmingCharacters(in: .whitespaces), secret: secret,
                publicURL: publicURL.trimmingCharacters(in: .whitespaces), prefix: prefix.trimmingCharacters(in: .whitespaces))
            busy = false
            failed = error != nil
            message = error ?? "Conectado: R2 responde y las credenciales funcionan."
            if error == nil { secret = "" }
        }
    }
}

@MainActor
private struct TranscriptionSettings: View {
    @Bindable var model: AppModel

    var body: some View {
        Form {
            Section {
                Toggle("Transcribir al terminar cada grabación", isOn: $model.prefs.transcribe)
                    .disabled(!model.canTranscribe)
                Text("Los subtítulos se generan en este Mac con Whisper: el audio no sale de aquí. Se ven en la página para compartir y puedes rehacerlos desde la biblioteca (clic derecho → Transcribir).")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("Modelo") {
                if let m = model.transcriberModel {
                    LabeledContent("En uso") {
                        Text(URL(fileURLWithPath: m).lastPathComponent).foregroundStyle(.secondary)
                    }
                    if m.contains("/MacWhisper/") {
                        Text("Es el modelo de MacWhisper; Vroom lo usa sin copiarlo.").font(.caption).foregroundStyle(.secondary)
                    }
                } else {
                    Text("No hay ningún modelo todavía.").foregroundStyle(.secondary)
                }
                if let p = model.modelDownload {
                    ProgressView(value: p) { Text("Descargando modelo…") }
                } else if !(model.transcriberModel?.contains("/Vroom/models/") ?? false) {
                    Button("Descargar Whisper large-v3-turbo (574 MB)") { model.downloadModel() }
                    Text("Modelo abierto de OpenAI, del repositorio de whisper.cpp en Hugging Face. Se comprueba su huella SHA-256 antes de usarlo; es más rápido que el de MacWhisper con la misma calidad.")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            Section("whisper.cpp") {
                if let w = model.whisperPath {
                    LabeledContent("Programa") { Text(w).foregroundStyle(.secondary) }
                } else {
                    Text("Falta whisper.cpp. Instálalo desde Terminal:")
                    Text("brew install whisper-cpp ffmpeg").font(.system(.body, design: .monospaced)).textSelection(.enabled)
                    Button("Comprobar de nuevo") { Task { await model.reloadTranscriber() } }
                }
            }
        }
        .formStyle(.grouped)
        .task { await model.reloadTranscriber() }
    }
}

@MainActor
private struct LibrarySettings: View {
    let model: AppModel

    var body: some View {
        Form {
            if let c = model.config {
                LabeledContent("Carpeta") {
                    HStack {
                        Text(c.library).lineLimit(1).truncationMode(.middle).foregroundStyle(.secondary)
                        Button("Cambiar…") { chooseFolder() }
                        Button("Abrir") { NSWorkspace.shared.open(URL(fileURLWithPath: c.library)) }
                    }
                }
                Toggle("Subir a R2 al terminar cada grabación", isOn: Binding(
                    get: { c.config.autoUpload }, set: { model.set("auto_upload", $0 ? "true" : "false") }))
                Picker("Copia local", selection: Binding(
                    get: { c.config.keepLocalDays }, set: { model.set("keep_local_days", String($0)) })) {
                    Text("Conservar siempre").tag(0)
                    Text("Borrar 7 días después de subirla").tag(7)
                    Text("Borrar 30 días después de subirla").tag(30)
                    Text("Borrar 90 días después de subirla").tag(90)
                }
                Text("Solo se borran vídeos que ya están en R2; siguen en la biblioteca y se descargan al abrirlos. Si pierdes o te roban el Mac, todo lo subido sigue en tu bucket.")
                    .font(.caption).foregroundStyle(.secondary)
            } else {
                ProgressView()
            }
        }
        .formStyle(.grouped)
    }

    private func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.prompt = "Usar esta carpeta"
        if panel.runModal() == .OK, let url = panel.url { model.set("library", url.path) }
    }
}

@MainActor
private struct LoomImport: View {
    let model: AppModel
    @State private var cookie = ""
    @State private var running = false
    @State private var log: [String] = []
    @State private var progress: (id: String, value: Double)?
    @State private var task: Task<Void, Never>?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Loom no ofrece exportación masiva ni API pública de descargas. Vroom usa la sesión de tu navegador para listar tus vídeos (también los de carpetas), los descarga con sus subtítulos y los sube a R2. Si lo interrumpes, al volver a lanzarlo sigue donde lo dejó.")
                .font(.callout).fixedSize(horizontal: false, vertical: true)
            Text("1. Entra en loom.com con tu cuenta y abre tu biblioteca.\n2. Abre las herramientas de desarrollo (⌥⌘I) › Network, recarga y haz clic en cualquier petición a «graphql».\n3. En Request Headers, copia el valor completo de «cookie» y pégalo aquí (también vale solo el valor de connect.sid).")
                .font(.caption).foregroundStyle(.secondary)
            SecureField("Cookie de loom.com", text: $cookie)
            HStack {
                if !model.cloudReady {
                    Text("Sin R2 conectado se importa solo a este Mac.").font(.caption).foregroundStyle(.orange)
                }
                Spacer()
                if running {
                    Button("Detener") { task?.cancel() }
                } else {
                    Button("Ver cuántos hay") { start(["loom", "list"]) }.disabled(cookie.isEmpty)
                    Button("Importar todo") {
                        start(["loom", "import"] + (model.cloudReady ? [] : ["--no-upload"]))
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(cookie.isEmpty)
                }
            }
            if let progress { ProgressView(value: progress.value) }
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(Array(log.enumerated()), id: \.offset) { i, line in
                            Text(line).font(.caption.monospaced()).textSelection(.enabled).id(i)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(height: 170)
                .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: 6))
                .onChange(of: log.count) { proxy.scrollTo(log.count - 1) }
            }
        }
    }

    private func start(_ args: [String]) {
        guard let cli = model.cli else { return }
        running = true
        log = []
        let cookie = cookie
        task = Task {
            do {
                try await cli.stream(args, stdin: cookie) { event in
                    Task { @MainActor in handle(event) }
                }
            } catch is CancellationError {
                log.append("Detenido.")
            } catch {
                log.append("Error: \(error.localizedDescription)")
            }
            running = false
            progress = nil
            await model.refresh()
        }
    }

    private func handle(_ event: Event) {
        switch event {
        case let .progress(id, done, total) where total > 0:
            progress = (id, Double(done) / Double(total))
        case let .done(id, what):
            let title = model.entries.first { $0.id == id }?.title ?? id
            log.append("\(what == "uploaded" ? "↑ subido" : "✓ importado"): \(title)")
            if what == "imported" { Task { await model.refresh() } }
        case let .warning(_, message): log.append(message)
        case let .ok(message): log.append(message)
        case let .error(message): log.append("Error: \(message)")
        default: break
        }
    }
}
