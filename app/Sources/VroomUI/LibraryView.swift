import AppKit
import SwiftUI
import VroomKit

@MainActor
public struct LibraryView: View {
    let model: AppModel
    @State private var search = ""
    @State private var renaming: Entry?
    @State private var newTitle = ""
    @State private var deleting: Entry?
    @State private var folder: FolderFilter = .all
    @State private var moving: Entry?
    @State private var newFolder = ""
    /// The "new folder" / "rename folder" sheet: (parent or folder being renamed, is rename).
    @State private var folderSheet: FolderSheet?
    @State private var folderName = ""
    @State private var deletingFolder: String?

    public init(model: AppModel) { self.model = model }

    private var filtered: [Entry] {
        let q = search.trimmingCharacters(in: .whitespaces)
        return model.entries.filter { e in
            folder.contains(e) && (q.isEmpty || e.title.localizedCaseInsensitiveContains(q))
        }
    }

    private var sidebar: some View {
        List(selection: $folder) {
            Label("Todas", systemImage: "video").tag(FolderFilter.all)
            Label("Sin carpeta", systemImage: "tray").tag(FolderFilter.none)
                .dropDestination(for: String.self) { ids, _ in drop(ids, into: "") }
            Section {
                ForEach(model.folders, id: \.self) { path in
                    let depth = path.split(separator: "/").count - 1
                    Label(String(path.split(separator: "/").last ?? ""), systemImage: "folder")
                        .padding(.leading, CGFloat(depth) * 14)
                        .tag(FolderFilter.folder(path))
                        .dropDestination(for: String.self) { ids, _ in drop(ids, into: path) }
                        .contextMenu {
                            Button("Nueva subcarpeta…") { startFolderSheet(.create(parent: path)) }
                            Button("Renombrar…") { startFolderSheet(.rename(path)) }
                            Divider()
                            Button("Eliminar carpeta…", role: .destructive) { deletingFolder = path }
                        }
                }
            } header: {
                HStack {
                    Text("Carpetas")
                    Spacer()
                    Button { startFolderSheet(.create(parent: nil)) } label: { Image(systemName: "folder.badge.plus") }
                        .buttonStyle(.borderless)
                        .help("Nueva carpeta")
                }
            }
        }
        .navigationSplitViewColumnWidth(min: 180, ideal: 220)
        .sheet(item: $folderSheet) { sheet in
            VStack(alignment: .leading, spacing: 12) {
                Text(sheet.title).font(.headline)
                TextField("Nombre", text: $folderName).frame(width: 320).onSubmit { commitFolder(sheet) }
                HStack {
                    Spacer()
                    Button("Cancelar") { folderSheet = nil }.keyboardShortcut(.cancelAction)
                    Button(sheet.isRename ? LocalizedStringKey("Renombrar") : LocalizedStringKey("Crear")) { commitFolder(sheet) }.keyboardShortcut(.defaultAction)
                }
            }
            .padding(20)
        }
        .confirmationDialog(
            "¿Eliminar la carpeta «\(deletingFolder ?? "")»?",
            isPresented: Binding(get: { deletingFolder != nil }, set: { if !$0 { deletingFolder = nil } }),
            presenting: deletingFolder
        ) { path in
            let inside = recordings(in: path)
            if inside.isEmpty {
                Button("Eliminar carpeta", role: .destructive) { deleteFolder(path) }
            } else if inside.contains(where: { $0.isInCloud || $0.status == .uploading }) {
                Button("Eliminar carpeta y grabaciones de este Mac y de R2", role: .destructive) {
                    deleteFolder(path, recordings: true, keepCloud: false)
                }
                Button("Eliminar carpeta y grabaciones solo de este Mac") { deleteFolder(path, recordings: true, keepCloud: true) }
                Button("Eliminar solo la carpeta") { deleteFolder(path) }
            } else {
                Button("Eliminar carpeta y grabaciones", role: .destructive) { deleteFolder(path, recordings: true, keepCloud: true) }
                Button("Eliminar solo la carpeta") { deleteFolder(path) }
            }
        } message: { path in
            let count = recordings(in: path).count
            if count == 0 {
                Text("La carpeta está vacía.")
            } else {
                Text(count == 1 ? LocalizedStringKey("Contiene 1 grabación.") : LocalizedStringKey("Contiene \(count) grabaciones."))
                    + Text(verbatim: " ") + Text("Si eliminas solo la carpeta, las grabaciones pasan a la carpeta de arriba.")
            }
        }
    }

    /// Recordings in a folder or its subfolders.
    private func recordings(in path: String) -> [Entry] {
        model.entries.filter { $0.folder == path || $0.folder?.hasPrefix(path + "/") == true }
    }

    private func deleteFolder(_ path: String, recordings: Bool = false, keepCloud: Bool = false) {
        if case let .folder(selected) = folder, selected == path || selected.hasPrefix(path + "/") { folder = .all }
        model.deleteFolder(path, recordings: recordings, keepCloud: keepCloud)
    }

    private var grid: some View {
        ScrollView {
            if model.entries.isEmpty {
                ContentUnavailableView("Sin grabaciones", systemImage: "video",
                                       description: Text("Pulsa ⌥⇧R o el botón Grabar para empezar."))
                    .padding(.top, 80)
            } else {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 230, maximum: 320), spacing: 16)], spacing: 18) {
                    ForEach(filtered) { entry in
                        Card(entry: entry, progress: model.transfers[entry.id], transcribing: model.transcribing[entry.id],
                             cloudReady: model.cloudReady, copyLink: { model.copyLink(entry) })
                            .draggable(entry.id)
                            .onTapGesture(count: 2) { model.play(entry) }
                            .contextMenu { menu(for: entry) }
                    }
                }
                .padding(18)
            }
        }
    }

    public var body: some View {
        NavigationSplitView {
            sidebar
        } detail: {
            grid
        }
        .navigationTitle(folder.title)
        .searchable(text: $search, placement: .toolbar, prompt: "Buscar")
        .toolbar {
            ToolbarItemGroup {
                if let toast = model.toast { Text(toast).font(.caption).foregroundStyle(.secondary) }
                if model.syncing {
                    ProgressView().controlSize(.small)
                } else if model.cloudReady {
                    Button { model.syncAll() } label: { Label("Sincronizar", systemImage: "arrow.triangle.2.circlepath.icloud") }
                        .help("Sube lo pendiente ahora")
                }
                Button { model.toggleRecording() } label: {
                    Label(model.isRecording ? LocalizedStringKey("Terminar") : LocalizedStringKey("Grabar"), systemImage: model.isRecording ? "stop.fill" : "record.circle")
                }
            }
        }
        .overlay(alignment: .bottom) {
            if let error = model.lastError {
                HStack {
                    Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                    Text(error).font(.callout).lineLimit(3)
                    Button { model.clearError() } label: { Image(systemName: "xmark") }.buttonStyle(.borderless)
                }
                .padding(10)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10))
                .padding()
            }
        }
        .task { await model.refresh() }
        .sheet(item: $renaming) { entry in
            VStack(alignment: .leading, spacing: 12) {
                Text("Renombrar").font(.headline)
                TextField("Título", text: $newTitle).frame(width: 340).onSubmit { commitRename(entry) }
                HStack {
                    Spacer()
                    Button("Cancelar") { renaming = nil }.keyboardShortcut(.cancelAction)
                    Button("Guardar") { commitRename(entry) }.keyboardShortcut(.defaultAction)
                }
            }
            .padding(20)
        }
        .sheet(item: $moving) { entry in
            VStack(alignment: .leading, spacing: 12) {
                Text("Mover a carpeta").font(.headline)
                HStack {
                    TextField("Carpeta (usa / para subcarpetas)", text: $newFolder)
                        .frame(width: 300)
                        .onSubmit { commitMove(entry) }
                    if !model.folders.isEmpty {
                        Menu {
                            ForEach(model.folders, id: \.self) { f in Button(f) { newFolder = f } }
                        } label: { Image(systemName: "folder") }
                        .fixedSize()
                    }
                }
                Text("Déjalo vacío para sacarla de cualquier carpeta.").font(.caption).foregroundStyle(.secondary)
                HStack {
                    Spacer()
                    Button("Cancelar") { moving = nil }.keyboardShortcut(.cancelAction)
                    Button("Mover") { commitMove(entry) }.keyboardShortcut(.defaultAction)
                }
            }
            .padding(20)
        }
        .confirmationDialog(
            "¿Eliminar «\(deleting?.title ?? "")»?", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
            presenting: deleting
        ) { entry in
            if entry.isInCloud || entry.status == .uploading {
                Button("Eliminar de este Mac y de R2", role: .destructive) { model.delete(entry, keepCloud: false) }
                Button("Eliminar solo de este Mac") { model.delete(entry, keepCloud: true) }
            } else {
                Button("Eliminar", role: .destructive) { model.delete(entry, keepCloud: true) }
            }
        } message: { entry in
            Text(entry.isInCloud ? LocalizedStringKey("Los enlaces compartidos dejarán de funcionar si la borras de R2.")
                 : LocalizedStringKey("Es la única copia: no está en la nube."))
        }
        .frame(minWidth: 560, minHeight: 400)
    }

    @ViewBuilder
    private func menu(for entry: Entry) -> some View {
        Button("Reproducir") { model.play(entry) }
        Button("Copiar enlace") { model.copyLink(entry) }.disabled(!model.cloudReady)
        Button("Renombrar…") {
            newTitle = entry.title
            renaming = entry
        }
        Button("Mover a carpeta…") {
            newFolder = entry.folder ?? ""
            moving = entry
        }
        if entry.videoURL != nil { Button("Mostrar en Finder") { model.reveal(entry) } }
        if entry.videoURL != nil && entry.status != .recording {
            Button(entry.captions == nil ? LocalizedStringKey("Transcribir") : LocalizedStringKey("Volver a transcribir")) { model.transcribe(entry) }
                .disabled(!model.canTranscribe || model.transcribing[entry.id] != nil)
        }
        Divider()
        switch entry.status {
        case .local, .uploading:
            Button("Subir ahora") { model.upload(entry) }.disabled(!model.cloudReady)
        case .synced:
            Button("Liberar espacio en este Mac") { model.freeLocal(entry) }
        case .cloud:
            Button("Descargar a este Mac") { model.download(entry) }
        case .recording:
            EmptyView()
        }
        Divider()
        Button("Eliminar…", role: .destructive) { deleting = entry }
    }

    private func startFolderSheet(_ sheet: FolderSheet) {
        if case let .rename(path) = sheet { folderName = String(path.split(separator: "/").last ?? "") } else { folderName = "" }
        folderSheet = sheet
    }

    private func commitFolder(_ sheet: FolderSheet) {
        let name = folderName.replacingOccurrences(of: "/", with: "-").trimmingCharacters(in: .whitespacesAndNewlines)
        folderSheet = nil
        guard !name.isEmpty else { return }
        switch sheet {
        case let .create(parent):
            let path = parent.map { "\($0)/\(name)" } ?? name
            model.createFolder(path)
            folder = .folder(path)
        case let .rename(path):
            let parent = path.split(separator: "/").dropLast().joined(separator: "/")
            let new = parent.isEmpty ? name : "\(parent)/\(name)"
            if new != path {
                model.renameFolder(path, to: new)
                folder = .folder(new)
            }
        }
    }

    private func drop(_ ids: [String], into path: String) -> Bool {
        let moving = model.entries.filter { ids.contains($0.id) && ($0.folder ?? "") != path }
        moving.forEach { model.move($0, to: path) }
        return !moving.isEmpty
    }

    private func commitMove(_ entry: Entry) {
        let f = newFolder.trimmingCharacters(in: .whitespacesAndNewlines)
        if f != (entry.folder ?? "") { model.move(entry, to: f) }
        moving = nil
    }

    private func commitRename(_ entry: Entry) {
        let t = newTitle.trimmingCharacters(in: .whitespacesAndNewlines)
        if !t.isEmpty, t != entry.title { model.rename(entry, to: t) }
        renaming = nil
    }
}

enum FolderSheet: Identifiable {
    case create(parent: String?)
    case rename(String)

    var id: String {
        switch self {
        case let .create(parent): "create:\(parent ?? "")"
        case let .rename(path): "rename:\(path)"
        }
    }

    var isRename: Bool { if case .rename = self { true } else { false } }

    var title: String {
        switch self {
        case .create(nil): return String(localized: "Nueva carpeta")
        case let .create(parent?): return String(localized: "Nueva carpeta dentro de «\(parent)»")
        case let .rename(path):
            let name = String(path.split(separator: "/").last ?? "")
            return String(localized: "Renombrar «\(name)»")
        }
    }
}

enum FolderFilter: Hashable {
    case all, none
    case folder(String)

    /// A folder shows its subfolders' recordings too.
    func contains(_ e: Entry) -> Bool {
        switch self {
        case .all: return true
        case .none: return e.folder == nil
        case let .folder(path): return e.folder == path || e.folder?.hasPrefix(path + "/") == true
        }
    }

    var title: String {
        switch self {
        case .all: return String(localized: "Biblioteca")
        case .none: return String(localized: "Sin carpeta")
        case let .folder(path): return path.replacingOccurrences(of: "/", with: " › ")
        }
    }
}

private struct Card: View {
    let entry: Entry
    let progress: Double?
    let transcribing: Double?
    let cloudReady: Bool
    let copyLink: () -> Void
    @State private var hover = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ZStack(alignment: .bottomTrailing) {
                Thumb(url: entry.thumbURL)
                    .aspectRatio(16 / 10, contentMode: .fill)
                    .frame(maxWidth: .infinity)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                Text(Elapsed.format(entry.durationSecs))
                    .font(.caption2.monospacedDigit())
                    .padding(.horizontal, 5).padding(.vertical, 2)
                    .background(.black.opacity(0.65), in: RoundedRectangle(cornerRadius: 4))
                    .foregroundStyle(.white)
                    .padding(6)
                if hover && cloudReady {
                    Button(action: copyLink) { Label("Copiar enlace", systemImage: "link") }
                        .controlSize(.small)
                        .padding(6)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
                }
                if let t = transcribing {
                    Text(t > 0 ? LocalizedStringKey("Transcribiendo \(Int(t * 100)) %") : LocalizedStringKey("Transcripción en cola"))
                        .font(.caption2.monospacedDigit())
                        .padding(.horizontal, 5).padding(.vertical, 2)
                        .background(.black.opacity(0.65), in: RoundedRectangle(cornerRadius: 4))
                        .foregroundStyle(.white)
                        .padding(6)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                }
                if let p = progress ?? (entry.status == .uploading ? entry.progress : nil) {
                    ProgressView(value: p).tint(.white).padding(8)
                        .frame(maxHeight: .infinity, alignment: .bottom)
                }
            }
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(entry.title).font(.callout.weight(.medium)).lineLimit(2)
                    Text(entry.createdAt.formatted(date: .abbreviated, time: .shortened)
                         + (entry.sizeBytes > 0 ? " · " + Format.bytes.string(fromByteCount: entry.sizeBytes) : ""))
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer(minLength: 4)
                if entry.captions != nil {
                    Image(systemName: "captions.bubble").font(.caption2).foregroundStyle(.secondary).help("Con subtítulos")
                }
                if entry.source == .loom { Text(verbatim: "Loom").font(.caption2).foregroundStyle(.secondary) }
                StatusIcon(entry: entry, progress: nil)
            }
        }
        .contentShape(Rectangle())
        .onHover { hover = $0 }
    }
}

/// Thumbnails are small JPEGs; decode them off the main thread and keep a few in memory.
private struct Thumb: View {
    let url: URL?
    @State private var image: NSImage?

    var body: some View {
        Rectangle()
            .fill(.quaternary)
            .overlay {
                if let image {
                    Image(nsImage: image).resizable().scaledToFill()
                } else {
                    Image(systemName: "video").font(.title).foregroundStyle(.tertiary)
                }
            }
            .clipped()
            .task(id: url) { image = await ThumbCache.shared.image(for: url)?.image }
    }
}

/// NSImage is immutable once loaded; this lets it cross from the loading actor.
struct LoadedImage: @unchecked Sendable { let image: NSImage }

actor ThumbCache {
    static let shared = ThumbCache()
    private let cache: NSCache<NSURL, NSImage> = {
        let c = NSCache<NSURL, NSImage>()
        c.countLimit = 200
        return c
    }()

    func image(for url: URL?) -> LoadedImage? {
        guard let url else { return nil }
        if let hit = cache.object(forKey: url as NSURL) { return LoadedImage(image: hit) }
        guard let img = NSImage(contentsOf: url) else { return nil }
        cache.setObject(img, forKey: url as NSURL)
        return LoadedImage(image: img)
    }
}
