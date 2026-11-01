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
            if !model.folders.isEmpty {
                Section("Carpetas") {
                    ForEach(model.folders, id: \.self) { path in
                        let depth = path.split(separator: "/").count - 1
                        Label(String(path.split(separator: "/").last ?? ""), systemImage: "folder")
                            .padding(.leading, CGFloat(depth) * 14)
                            .tag(FolderFilter.folder(path))
                    }
                }
            }
        }
        .navigationSplitViewColumnWidth(min: 170, ideal: 210)
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
                        Card(entry: entry, progress: model.transfers[entry.id], cloudReady: model.cloudReady,
                             copyLink: { model.copyLink(entry) })
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
                    Button { model.sync() } label: { Label("Sincronizar", systemImage: "arrow.triangle.2.circlepath.icloud") }
                        .help("Sube lo pendiente ahora")
                }
                Button { model.toggleRecording() } label: {
                    Label(model.isRecording ? "Terminar" : "Grabar", systemImage: model.isRecording ? "stop.fill" : "record.circle")
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
            Text(entry.isInCloud ? "Los enlaces compartidos dejarán de funcionar si la borras de R2."
                 : "Es la única copia: no está en la nube.")
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
        case .all: return "Biblioteca"
        case .none: return "Sin carpeta"
        case let .folder(path): return path.replacingOccurrences(of: "/", with: " › ")
        }
    }
}

private struct Card: View {
    let entry: Entry
    let progress: Double?
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
                if entry.source == .loom { Text("Loom").font(.caption2).foregroundStyle(.secondary) }
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
