//! vroom: the recording library and its Cloudflare R2 copy, from the terminal.
//! The apps run it for everything except capture; `--json` prints one event per line.

mod loom;
mod out;
mod secret;
mod transcribe;

use out::Out;
use std::path::PathBuf;
use std::process::ExitCode;
use time::OffsetDateTime;
use vroom_cloud::{sync, R2};
use vroom_core::config::{default_config_path, Config, R2Config};
use vroom_core::protocol::{Event, PROTOCOL_VERSION};
use vroom_core::Library;

const USAGE: &str = "\
vroom: grabaciones locales con copia en Cloudflare R2

  vroom list                        grabaciones, de la más nueva a la más antigua
  vroom sync                        sube lo pendiente y libera copias locales caducadas
  vroom upload <id>... | --pending  sube grabaciones a R2 (reanuda las interrumpidas)
  vroom share <id> [--before-upload] enlace para compartir
  vroom stream-url <id>             URL temporal para reproducir desde R2
  vroom download <id>               baja el vídeo de una grabación que solo está en R2
  vroom restore                     reconstruye la biblioteca desde R2 (ordenador nuevo)
  vroom rename <id> <título>
  vroom move <id> [carpeta/sub]     mueve a una carpeta (sin carpeta: al nivel superior)
  vroom folders                     lista las carpetas
  vroom folder create <ruta> | rename <ruta> <nueva> | delete <ruta>
                                    (al borrar, las grabaciones pasan a la carpeta de arriba)
  vroom transcribe <id>... | --missing [--language es] [--model ruta.bin]
                                    subtítulos con Whisper en local (usa los modelos de MacWhisper)
  vroom free <id>                   borra el vídeo local de una grabación ya subida
  vroom delete <id> [--keep-cloud]  borra la grabación (también de R2, salvo --keep-cloud)

  vroom config                      muestra la configuración
  vroom config r2 --account-id <id> --bucket <b> --access-key-id <k> [--public-url <u>]
                  [--prefix <p>] [--jurisdiction eu]   (el secret se lee de stdin)
  vroom config set <clave> <valor>  library, auto_upload, keep_local_days
  vroom check                       prueba la conexión con R2

  vroom loom import [--no-upload] [--free-local] [--limit N]
                                    importa tus vídeos de Loom (cookie connect.sid por stdin
                                    o en VROOM_LOOM_COOKIE)
  vroom loom list                   lista lo que se importaría

  Para la app: new [--title t] [--folder f], finalize <id> --duration s --width w --height h, recover, version.
  --json en cualquier comando imprime eventos JSON, uno por línea.";

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let json = take_flag(&mut args, "--json");
    let out = Out::new(json);
    match run(&out, args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            out.event(&Event::Error { message: message.clone() }, || format!("error: {message}"));
            ExitCode::FAILURE
        }
    }
}

struct Ctx {
    config_path: std::path::PathBuf,
    config: Config,
    lib: Library,
}

impl Ctx {
    fn load() -> Result<Ctx, String> {
        let config_path = default_config_path();
        let config = Config::load(&config_path)?;
        let lib =
            Library::open(config.library_dir()).map_err(|e| format!("{}: {e}", config.library_dir().display()))?;
        Ok(Ctx { config_path, config, lib })
    }

    fn r2(&self) -> Result<R2, String> {
        let cfg = self.config.r2.as_ref().ok_or("R2 no está configurado: vroom config r2 …")?;
        let secret = secret::get(&cfg.access_key_id)?.ok_or("falta el Secret Access Key de R2")?;
        R2::new(cfg, &secret)
    }

    /// Only one process uploads or imports at a time; the others say so and stop.
    fn lock(&self) -> Result<std::fs::File, String> {
        let path = self.lib.root().join(".vroom.lock");
        let f = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        match f.try_lock() {
            Ok(()) => Ok(f),
            Err(std::fs::TryLockError::WouldBlock) => Err("ya hay una sincronización en marcha".into()),
            Err(std::fs::TryLockError::Error(e)) => Err(e.to_string()),
        }
    }
}

fn run(out: &Out, mut args: Vec<String>) -> Result<(), String> {
    if args.is_empty() || take_flag(&mut args, "--help") || take_flag(&mut args, "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    let cmd = args.remove(0);
    if cmd == "version" || cmd == "--version" {
        let version = env!("CARGO_PKG_VERSION").to_string();
        out.event(&Event::Version { version: version.clone(), protocol: PROTOCOL_VERSION }, || {
            format!("vroom {version}")
        });
        return Ok(());
    }
    let mut ctx = Ctx::load()?;
    match cmd.as_str() {
        "list" | "ls" => {
            let entries = ctx.lib.list()?;
            out.event(&Event::Entries { entries: entries.clone() }, || out::table(&entries));
            Ok(())
        }
        "new" => {
            let title = take_opt(&mut args, "--title");
            let folder = take_opt(&mut args, "--folder");
            // The app writes the video; without --owner-pid this process "owns" it.
            let owner = take_opt(&mut args, "--owner-pid").and_then(|p| p.parse().ok()).unwrap_or(std::process::id());
            let mut entry = ctx.lib.create(title, now_local(), owner)?;
            if folder.is_some() {
                entry = ctx.lib.set_folder(&entry.meta.id, folder.as_deref())?;
            }
            out.entry(&entry);
            Ok(())
        }
        "finalize" => {
            let id = positional(&mut args, "id")?;
            let duration = take_opt(&mut args, "--duration").and_then(|v| v.parse().ok()).unwrap_or(0.0);
            let width = take_opt(&mut args, "--width").and_then(|v| v.parse().ok()).unwrap_or(0);
            let height = take_opt(&mut args, "--height").and_then(|v| v.parse().ok()).unwrap_or(0);
            let entry = ctx.lib.finalize(&id, duration, width, height)?;
            out.entry(&entry);
            Ok(())
        }
        "recover" => {
            // An import or sync elsewhere is writing; its folders are not leftovers.
            let Ok(_lock) = ctx.lock() else {
                out.ok("biblioteca en uso; no se recupera nada".into());
                return Ok(());
            };
            let ids = ctx.lib.recover()?;
            out.ok(format!("{} grabaciones recuperadas", ids.len()));
            Ok(())
        }
        "rename" => {
            let id = positional(&mut args, "id")?;
            let title = rest(&args);
            let entry = ctx.lib.rename(&id, &title)?;
            out.entry(&entry);
            Ok(())
        }
        "folders" => {
            let folders = ctx.lib.folders()?;
            out.event(&Event::Folders { folders: folders.clone() }, || folders.join("\n"));
            Ok(())
        }
        "folder" => {
            let sub = positional(&mut args, "create|rename|delete")?;
            match sub.as_str() {
                "create" | "new" => {
                    ctx.lib.create_folder(&rest(&args))?;
                }
                "rename" | "move" => {
                    let from = positional(&mut args, "carpeta")?;
                    ctx.lib.rename_folder(&from, &rest(&args))?;
                }
                "delete" | "rm" => {
                    ctx.lib.delete_folder(&rest(&args))?;
                }
                other => return Err(format!("vroom folder: subcomando desconocido {other}")),
            }
            let folders = ctx.lib.folders()?;
            out.event(&Event::Folders { folders: folders.clone() }, || folders.join("\n"));
            Ok(())
        }
        "move" | "mv" => {
            let id = positional(&mut args, "id")?;
            let folder = rest(&args);
            let entry = ctx.lib.set_folder(&id, Some(&folder))?;
            out.entry(&entry);
            Ok(())
        }
        "free" => {
            let id = positional(&mut args, "id")?;
            let entry = ctx.lib.remove_local_video(&id)?;
            out.entry(&entry);
            Ok(())
        }
        "delete" | "rm" => {
            let keep_cloud = take_flag(&mut args, "--keep-cloud");
            let id = positional(&mut args, "id")?;
            if !ctx.lib.contains(&id) {
                return Err(format!("no existe la grabación {id}"));
            }
            let in_cloud = ctx.lib.state(&id).uploaded_at.is_some() || ctx.lib.state(&id).multipart.is_some();
            if !keep_cloud && in_cloud {
                sync::delete_remote(&ctx.r2()?, &ctx.lib, &id)?;
            }
            ctx.lib.remove(&id)?;
            out.ok(format!("{id} borrada"));
            Ok(())
        }
        "upload" => {
            let pending = take_flag(&mut args, "--pending");
            let _lock = ctx.lock()?;
            let r2 = ctx.r2()?;
            let ids = if pending { ctx.lib.pending_upload()? } else { args };
            if ids.is_empty() && !pending {
                return Err("¿qué grabación? vroom upload <id> o --pending".into());
            }
            upload_all(out, &ctx, &r2, &ids)
        }
        "sync" => {
            let _lock = ctx.lock()?;
            ctx.lib.recover()?;
            let r2 = ctx.r2()?;
            if let Err(e) = sync::push_folders(&r2, &ctx.lib) {
                out.warning(None, &format!("carpetas: {e}"));
            }
            upload_all(out, &ctx, &r2, &ctx.lib.pending_upload()?)?;
            for id in ctx.lib.expired_local(ctx.config.keep_local_days, OffsetDateTime::now_utc())? {
                ctx.lib.remove_local_video(&id)?;
                out.done(&id, "freed");
            }
            Ok(())
        }
        "restore" => {
            let _lock = ctx.lock()?;
            let r2 = ctx.r2()?;
            let restored = sync::restore(&r2, &ctx.lib, |id| out.done(id, "restored"))?;
            out.ok(format!(
                "{} grabaciones recuperadas de R2 ({} ya estaban aquí)",
                restored.added.len(),
                restored.already_here
            ));
            Ok(())
        }
        "download" => {
            let id = positional(&mut args, "id")?;
            let r2 = ctx.r2()?;
            sync::download(&r2, &ctx.lib, &id, |done, total| out.progress(&id, done, total))?;
            out.entry(&ctx.lib.entry(&id)?);
            Ok(())
        }
        "share" => {
            // The app copies the link the moment a recording stops, while it uploads.
            let early = take_flag(&mut args, "--before-upload");
            let id = positional(&mut args, "id")?;
            if !ctx.lib.contains(&id) {
                return Err(format!("no existe la grabación {id}"));
            }
            if !early && ctx.lib.state(&id).uploaded_at.is_none() {
                return Err("la grabación aún no está subida a R2".into());
            }
            let (url, expires_at) = sync::share_link(&ctx.r2()?, &id);
            out.event(&Event::Link { id, url: url.clone(), expires_at }, || url.clone());
            Ok(())
        }
        "stream-url" => {
            let id = positional(&mut args, "id")?;
            let url = sync::stream_url(&ctx.r2()?, &id);
            out.event(&Event::Link { id, url: url.clone(), expires_at: None }, || url.clone());
            Ok(())
        }
        "check" => {
            ctx.r2()?.check()?;
            out.ok("R2 responde y las credenciales funcionan".into());
            Ok(())
        }
        "config" => config(out, &mut ctx, args),
        "loom" => loom::run(out, &ctx, args),
        "transcribe" => transcribe::run(out, &ctx, args),
        other => Err(format!("comando desconocido: {other}\n\n{USAGE}")),
    }
}

/// Upload each recording; one failure is reported and the rest still go up.
fn upload_all(out: &Out, ctx: &Ctx, r2: &R2, ids: &[String]) -> Result<(), String> {
    let mut failed = 0;
    for id in ids {
        match sync::upload(r2, &ctx.lib, id, |done, total| out.progress(id, done, total)) {
            Ok(()) => out.done(id, "uploaded"),
            Err(message) => {
                failed += 1;
                out.warning(Some(id), &message);
            }
        }
    }
    if failed > 0 {
        return Err(format!("{failed} de {} subidas fallaron; se reintentarán", ids.len()));
    }
    Ok(())
}

fn config(out: &Out, ctx: &mut Ctx, mut args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None | Some("show") => {}
        Some("r2") => {
            args.remove(0);
            let cfg = R2Config {
                account_id: take_opt(&mut args, "--account-id").ok_or("falta --account-id")?.trim().to_string(),
                bucket: take_opt(&mut args, "--bucket").ok_or("falta --bucket")?.trim().to_string(),
                access_key_id: take_opt(&mut args, "--access-key-id")
                    .ok_or("falta --access-key-id")?
                    .trim()
                    .to_string(),
                public_url: take_opt(&mut args, "--public-url")
                    .map(|u| u.trim().trim_end_matches('/').to_string())
                    .filter(|u| !u.is_empty()),
                prefix: take_opt(&mut args, "--prefix").map(normalize_prefix).unwrap_or_default(),
                jurisdiction: take_opt(&mut args, "--jurisdiction").filter(|j| !j.is_empty()),
                endpoint: take_opt(&mut args, "--endpoint").filter(|e| !e.is_empty()),
            };
            cfg.validate()?;
            let secret = read_stdin_line()?;
            if !secret.is_empty() {
                secret::set(&cfg.access_key_id, &secret)?;
            } else if secret::get(&cfg.access_key_id)?.is_none() {
                return Err("falta el Secret Access Key (por stdin)".into());
            }
            R2::new(&cfg, &secret::get(&cfg.access_key_id)?.unwrap_or_default())?.check()?;
            if let Some(old) = &ctx.config.r2 {
                if old.access_key_id != cfg.access_key_id {
                    secret::delete(&old.access_key_id);
                }
            }
            ctx.config.r2 = Some(cfg);
            ctx.config.save(&ctx.config_path)?;
        }
        Some("disconnect") => {
            if let Some(old) = ctx.config.r2.take() {
                secret::delete(&old.access_key_id);
            }
            ctx.config.save(&ctx.config_path)?;
        }
        Some("set") => {
            let key = args.get(1).ok_or("vroom config set <clave> <valor>")?.clone();
            let value = args.get(2).cloned().unwrap_or_default();
            match key.as_str() {
                "library" => ctx.config.library = (!value.is_empty()).then(|| value.into()),
                "auto_upload" => ctx.config.auto_upload = parse_bool(&value)?,
                "keep_local_days" => ctx.config.keep_local_days = value.parse().map_err(|_| "un número de días")?,
                _ => return Err(format!("clave desconocida: {key}")),
            }
            ctx.config.save(&ctx.config_path)?;
        }
        Some(other) => return Err(format!("vroom config: subcomando desconocido {other}")),
    }
    let has_secret = match &ctx.config.r2 {
        Some(r2) => secret::get(&r2.access_key_id).ok().flatten().is_some(),
        None => false,
    };
    let event = Event::Config {
        config: ctx.config.clone(),
        path: ctx.config_path.display().to_string(),
        library: ctx.config.library_dir().display().to_string(),
        has_secret,
    };
    out.event(&event, || {
        let mut s =
            format!("config:     {}\nbiblioteca: {}\n", ctx.config_path.display(), ctx.config.library_dir().display());
        s += &format!("auto-subida: {}\n", if ctx.config.auto_upload { "sí" } else { "no" });
        s += &format!(
            "copia local: {}\n",
            match ctx.config.keep_local_days {
                0 => "se conserva siempre".into(),
                d => format!("se borra {d} días después de subirla"),
            }
        );
        match &ctx.config.r2 {
            Some(r2) => {
                s += &format!(
                    "R2:         {} / bucket {}{}\n",
                    r2.account_id,
                    r2.bucket,
                    if has_secret { "" } else { " (falta el secret)" }
                );
                if let Some(u) = &r2.public_url {
                    s += &format!("público:    {u}\n");
                }
            }
            None => s += "R2:         sin configurar\n",
        }
        s
    });
    Ok(())
}

fn normalize_prefix(p: String) -> String {
    let p = p.trim().trim_matches('/');
    if p.is_empty() {
        String::new()
    } else {
        format!("{p}/")
    }
}

fn parse_bool(v: &str) -> Result<bool, String> {
    match v {
        "true" | "1" | "yes" | "on" | "si" | "sí" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(format!("se esperaba true o false, no {v}")),
    }
}

pub(crate) fn read_stdin_line() -> Result<String, String> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        eprint!("Pega el valor y pulsa Intro: ");
    }
    let mut s = String::new();
    std::io::stdin().read_line(&mut s).map_err(|e| e.to_string())?;
    Ok(s.trim().to_string())
}

/// A program from `$env`, next to vroom itself (how the Windows zip ships whisper
/// and ffmpeg), the PATH, or Homebrew (apps launched from Finder get a minimal PATH).
pub(crate) fn find_tool(name: &str, env: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(env).map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let name = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let mut dirs: Vec<PathBuf> =
        std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)).into_iter().collect();
    dirs.extend(std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default());
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    dirs.into_iter().map(|d| d.join(&name)).find(|p| p.is_file())
}

fn now_local() -> OffsetDateTime {
    OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc())
}

/// Flags are only looked for before `--`, so a title like "-h" can follow it.
pub(crate) fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    match args[..end].iter().position(|a| a == flag) {
        Some(i) => {
            args.remove(i);
            true
        }
        None => false,
    }
}

pub(crate) fn take_opt(args: &mut Vec<String>, name: &str) -> Option<String> {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let i = args[..end].iter().position(|a| a == name)?;
    args.remove(i);
    (i < args.len()).then(|| args.remove(i))
}

/// What follows the options, joined; a leading `--` is dropped.
fn rest(args: &[String]) -> String {
    args.iter().filter(|a| *a != "--").cloned().collect::<Vec<_>>().join(" ")
}

fn positional(args: &mut Vec<String>, what: &str) -> Result<String, String> {
    if args.first().is_some_and(|a| a == "--") {
        args.remove(0);
    }
    if args.is_empty() || args[0].starts_with("--") {
        return Err(format!("falta el argumento <{what}>"));
    }
    Ok(args.remove(0))
}
