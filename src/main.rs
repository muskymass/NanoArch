//! NanoArch: Windows desktop shell. A native window (tao) hosting the interface (WebView2 via wry).
//! The interface sends `{id, cmd, args}` messages. Each runs on its own worker thread so the window never
//! freezes during generation or a file dialog; `backend::App` answers; the reply returns through the event loop.

#![cfg_attr(not(test), windows_subsystem = "windows")]

mod backend;
mod editing;
mod levels;
mod s3;
mod library;
mod secret;
mod ollama;
mod dwg;
mod voice;

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Icon, Window, WindowBuilder};
use wry::{PageLoadEvent, WebView, WebViewBuilder};

const HTML: &str = include_str!("../assets/index.html");

/// IBM Plex Sans Arabic (SIL Open Font License 1.1, see assets/vendor/fonts/plex-sans-arabic-OFL-LICENSE.txt), four weights, each as an Arabic
/// and a Latin subset (woff2). The page has no origin to fetch a file from, so the faces are written into it as data URLs at start-up.
/// Where the page's own `/*NN_FONTS*/` marker stands the style rules are inserted; no network is ever used.
const FONT_FACES: [(&str, u16, &str, &[u8]); 8] = [
    ("arabic", 400, ARABIC_RANGE, include_bytes!("../assets/vendor/fonts/plex-sans-arabic-arabic-400.woff2")),
    ("arabic", 500, ARABIC_RANGE, include_bytes!("../assets/vendor/fonts/plex-sans-arabic-arabic-500.woff2")),
    ("arabic", 600, ARABIC_RANGE, include_bytes!("../assets/vendor/fonts/plex-sans-arabic-arabic-600.woff2")),
    ("arabic", 700, ARABIC_RANGE, include_bytes!("../assets/vendor/fonts/plex-sans-arabic-arabic-700.woff2")),
    ("latin", 400, LATIN_RANGE, include_bytes!("../assets/vendor/fonts/plex-sans-arabic-latin-400.woff2")),
    ("latin", 500, LATIN_RANGE, include_bytes!("../assets/vendor/fonts/plex-sans-arabic-latin-500.woff2")),
    ("latin", 600, LATIN_RANGE, include_bytes!("../assets/vendor/fonts/plex-sans-arabic-latin-600.woff2")),
    ("latin", 700, LATIN_RANGE, include_bytes!("../assets/vendor/fonts/plex-sans-arabic-latin-700.woff2")),
];
/// Unicode ranges of the two subsets (from the font package's own unicode.json).
const ARABIC_RANGE: &str = "U+0600-06FF,U+0750-077F,U+0870-088E,U+0890-0891,U+0897-08E1,U+08E3-08FF,U+200C-200E,U+2010-2011,U+204F,U+2E41,U+FB50-FDFF,U+FE70-FE74,U+FE76-FEFC";
const LATIN_RANGE: &str = "U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,U+FEFF,U+FFFD";

/// The `@font-face` rules for the bundled faces.
fn font_face_css() -> String {
    FONT_FACES
        .iter()
        .map(|(_, weight, range, bytes)| {
            format!(
                "@font-face{{font-family:\"IBM Plex Sans Arabic\";font-style:normal;font-weight:{weight};font-display:block;src:url(data:font/woff2;base64,{}) format(\"woff2\");unicode-range:{range}}}",
                nnx_core::xlsx::base64_encode(bytes)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The file that keeps the user's interface choices. The page has no origin, so the browser's own storage is closed to it; the choices live in a
/// small JSON file of their own. Next to the library folder when `NANOARCH_LIBRARY_DIR` names one (so a test's own library folder keeps its own
/// choices and never touches the user's), otherwise `%APPDATA%\NanoArch\ui-prefs.json`.
fn ui_prefs_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("NANOARCH_LIBRARY_DIR") {
        let dir = PathBuf::from(dir);
        let mut name = dir.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
        name.push(".ui-prefs.json");
        return dir.with_file_name(name);
    }
    std::env::var_os("APPDATA").map_or_else(|| PathBuf::from("NanoArch-ui-prefs.json"), |a| PathBuf::from(a).join("NanoArch").join("ui-prefs.json"))
}

const UI_PREFS_MAX_BYTES: usize = 16 * 1024;

/// The saved choices as JSON text for the page: `{}` when there is no file yet. A file that exists but is not a JSON object is NOT ignored
/// silently: the page gets `{"_error": <path>}` and tells the user that the choices were reset (the next save replaces the file).
fn ui_prefs_json(path: &Path) -> String {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return "{}".to_string(),
        Err(_) => return json!({ "_error": path.display().to_string() }).to_string().replace('<', "\\u003c"),
    };
    match serde_json::from_str::<Value>(&text) {
        // `<` is escaped so no stored text can close the script element it is written into.
        Ok(Value::Object(mut m)) => {
            // The page reads these two as maps. Anything else under their names (a hand edit, a half-written file) is dropped and reported,
            // because the page would stop on it before it could show a single label.
            let mut dropped = false;
            for key in ["sizes", "open"] {
                if m.get(key).is_some_and(|v| !v.is_object()) {
                    m.remove(key);
                    dropped = true;
                }
            }
            if dropped {
                m.insert("_error".to_string(), Value::String(path.display().to_string()));
            }
            Value::Object(m).to_string().replace('<', "\\u003c")
        }
        _ => json!({ "_error": path.display().to_string() }).to_string().replace('<', "\\u003c"),
    }
}

fn ui_prefs_set(args: &Value) -> Result<Value, String> {
    let prefs = args.get("prefs").filter(|p| p.is_object()).ok_or("ui_prefs_set: 'prefs' must be an object")?;
    let text = prefs.to_string();
    if text.len() > UI_PREFS_MAX_BYTES {
        return Err(format!("ui_prefs_set: the choices are {} bytes, over the {UI_PREFS_MAX_BYTES} byte limit", text.len()));
    }
    let path = ui_prefs_path();
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("the interface choices folder {} cannot be made: {e}", parent.display()))?;
    }
    // Written beside the file and renamed over it, so a crash in the middle never leaves half a file.
    let mut tmp = path.clone().into_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text).map_err(|e| format!("the interface choices cannot be saved to {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("the interface choices cannot be saved to {}: {e}", path.display()))?;
    Ok(json!({ "saved": path.display().to_string() }))
}

/// The interface page with the bundled fonts and the saved interface choices written in.
fn page_html() -> String {
    HTML.replacen("/*NN_FONTS*/", &font_face_css(), 1).replacen("/*NN_PREFS*/{}", &ui_prefs_json(&ui_prefs_path()), 1)
}

/// The application icon as raw RGBA, 64 x 64 (made from `assets/icon.png`, see `assets/icon.ico` for the file icon).
const ICON_RGBA: &[u8] = include_bytes!("../assets/icon-window.rgba");
const ICON_SIZE: u32 = 64;

fn window_icon() -> Icon {
    Icon::from_rgba(ICON_RGBA.to_vec(), ICON_SIZE, ICON_SIZE).expect("the embedded icon is a 64 x 64 RGBA image")
}

/// Messages to the event loop (the only thread that may touch windows and web views).
enum Ui {
    /// Script to run in the main page (the reply to a command).
    Js(String),
    /// Print this HTML document to a PDF at `path`, then answer message `id`.
    Pdf { id: u64, html: String, path: PathBuf, size_mm: (f64, f64) },
    /// The document of printer `n` has finished loading: print it now.
    PrintNow(usize),
    /// Printer `n` is done; `error` is `None` on success.
    PdfDone { n: usize, error: Option<String> },
}

/// A hidden window holding the document being printed.
struct Printer {
    _window: Window,
    webview: WebView,
    id: u64,
    path: PathBuf,
    html_file: PathBuf,
    /// Page size in millimetres (width, height).
    size_mm: (f64, f64),
    /// Printing has been requested (the page-load event can fire more than once).
    started: bool,
}

type Shared = Arc<Mutex<backend::App>>;

/// The application state, even if a worker thread panicked while holding it.
fn lock(app: &Shared) -> MutexGuard<'_, backend::App> {
    app.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs one message and returns the JavaScript that delivers the result to the page.
fn respond(app: &Shared, body: &str) -> String {
    let msg: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let id = msg.get("id").and_then(Value::as_u64).unwrap_or(0);
    let cmd = msg.get("cmd").and_then(Value::as_str).unwrap_or("");
    let args = msg.get("args").cloned().unwrap_or(Value::Null);
    let result = match cmd {
        "export" => export(&args),
        "save_project" => save_project(app),
        "open_project" => open_project(app),
        "pick_sketch" => pick_sketch(),
        "pick_plans" => pick_plans(),
        // A project folder: the user picks it, the engine reads its files, and the page may then open the drawings inside that folder only.
        "pick_folder" => pick_folder(),
        "folder_import" => {
            let r = lock(app).handle(cmd, &args);
            if r.is_ok() && let Some(p) = args.get("path").and_then(Value::as_str) {
                *IMPORT_FOLDER.lock().unwrap_or_else(|e| e.into_inner()) = std::fs::canonicalize(p).ok();
            }
            r
        }
        "folder_file" => folder_file(&args),
        // A DWG of the imported folder converted to DXF by a converter installed here; it may take a while, so it runs outside the lock.
        "dwg_to_dxf" => dwg_to_dxf_file(&args),
        "sketch_pdfjs" => Ok(json!({ "lib": PDFJS, "worker": PDFJS_WORKER })),
        // The code library (the Saudi Building Code PDFs): the page reads a file only to look a clause up while the user works.
        "code_library_list" => code_library_list(),
        "code_library_file" => code_library_file(&args),
        // The reference library the user fills with PDFs and links: stored in its own folder, independent of any project.
        "library_list" | "library_add" | "library_remove" | "library_search" | "library_page" | "library_pick" | "library_fetch" => library::handle(cmd, &args),
        // The local model assistant talks to a server and may take a while: it runs outside the lock.
        "ai_status" | "ai_brief" | "ai_sketch" | "ai_deed" | "ai_ocr" | "ai_key_set" | "ai_key_clear" | "ai_key_status" => ollama::handle(cmd, &args),
        // A recording is transcribed by a program on this computer; it may take a while, so it runs outside the lock.
        "voice_transcribe" => voice::handle(&args),
        // A real DWG: the DXF comes from the engine under the lock, then a converter program on this computer runs outside it.
        "export_dwg" => export_dwg(app, &args),
        // The user's appearance and layout choices (mode, accent, density, font size, column widths): a small file of their own, not the project.
        "ui_prefs_set" => ui_prefs_set(&args),
        // The lock is held only while the engines run, never while a dialog is open.
        _ => lock(app).handle(cmd, &args),
    };
    let payload = match result {
        Ok(data) => json!({ "ok": true, "data": data }),
        Err(error) => json!({ "ok": false, "error": error }),
    };
    format!("window.__resolve({id}, {payload});")
}

/// `export_dwg`: the plan's DXF (same arguments and gate as `export_dxf`) converted to DWG by the converter named in `args`.
fn export_dwg(app: &Shared, args: &Value) -> Result<Value, String> {
    let dxf = lock(app).handle("export_dxf", args)?;
    let text = dxf.get("dxf").and_then(Value::as_str).ok_or("the plan produced no DXF")?;
    dwg::convert(text, args)
}

/// Where an export goes. Normally the user picks it in a Save dialog. Setting the environment variable
/// `NANOARCH_EXPORT_DIR` skips the dialog and saves into that folder: a seam for automated end-to-end tests of the
/// real application (a dialog cannot be driven from outside).
fn pick_save_path(name: &str) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("NANOARCH_EXPORT_DIR") {
        return Some(Path::new(&dir).join(name));
    }
    rfd::FileDialog::new().set_file_name(name).save_file()
}

/// Asks where to save a text export (CSV, SVG) and writes it. `saved` is null if the user cancels.
fn export(args: &Value) -> Result<Value, String> {
    let name = args.get("name").and_then(Value::as_str).unwrap_or("export.txt");
    // Text goes in `data`; a binary file (xlsx) goes in `b64`.
    let bytes: Vec<u8> = match (args.get("data").and_then(Value::as_str), args.get("b64").and_then(Value::as_str)) {
        (Some(d), _) => d.as_bytes().to_vec(),
        (None, Some(b)) => nnx_core::xlsx::base64_decode(b).map_err(|e| format!("export: {e}"))?,
        _ => return Err("nothing to export".into()),
    };
    match pick_save_path(name) {
        Some(path) => {
            std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
            Ok(json!({ "saved": path.display().to_string() }))
        }
        None => Ok(json!({ "saved": null })),
    }
}

/// pdf.js (Apache-2.0, see assets/vendor/pdfjs-LICENSE.txt) draws a PDF sketch onto a canvas. It is large, so the page
/// asks for it only when a PDF is imported.
const PDFJS: &str = include_str!("../assets/vendor/pdf.min.js");
const PDFJS_WORKER: &str = include_str!("../assets/vendor/pdf.worker.min.js");
const SKETCH_MAX_BYTES: u64 = 40 * 1024 * 1024;
/// The largest code PDF served to the page (the full code volumes are about 18 MB).
const CODE_LIBRARY_MAX_BYTES: u64 = 60 * 1024 * 1024;

/// (kind, mime) of a sketch file by extension, or None if it is not a supported type.
fn sketch_kind(path: &Path) -> Option<(&'static str, &'static str)> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "pdf" => Some(("pdf", "application/pdf")),
        "png" => Some(("image", "image/png")),
        "jpg" | "jpeg" => Some(("image", "image/jpeg")),
        "bmp" => Some(("image", "image/bmp")),
        "webp" => Some(("image", "image/webp")),
        _ => None,
    }
}

/// Lets the user pick a draft drawing (image or PDF) and returns its bytes for the page to draw. The sketch is only
/// a reference to trace over, so nothing is stored in the project. `NANOARCH_SKETCH_FILE` skips the dialog (test seam).
/// Folder of the code library: `NANOARCH_CODE_LIBRARY` (the seam of the tests), otherwise the workspace folder named in the project rules.
fn code_library_dir() -> PathBuf {
    std::env::var_os("NANOARCH_CODE_LIBRARY").map_or_else(|| PathBuf::from(r"C:\Projects\البناء\الكود السعودي"), PathBuf::from)
}

/// The PDF files of a library folder with their sizes, sorted by name.
fn library_files(dir: &Path) -> Result<Vec<(String, u64)>, String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("the code library folder {} cannot be read: {e}", dir.display()))?;
    let mut out: Vec<(String, u64)> = rd
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")))
        .filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), e.metadata().ok()?.len())))
        .collect();
    out.sort();
    Ok(out)
}

fn code_library_list() -> Result<Value, String> {
    let dir = code_library_dir();
    let files = library_files(&dir)?;
    Ok(json!({ "dir": dir.display().to_string(), "files": files.iter().map(|(n, s)| json!({ "name": n, "bytes": s })).collect::<Vec<_>>() }))
}

/// One library file as base64. Only a name that the folder listing itself contains is served, so no path can leave the folder.
fn code_library_file(args: &Value) -> Result<Value, String> {
    let name = args.get("name").and_then(Value::as_str).ok_or("code_library_file: 'name' is required")?;
    serve_library_file(&code_library_dir(), name)
}

fn serve_library_file(dir: &Path, name: &str) -> Result<Value, String> {
    let (found, size) = library_files(dir)?.into_iter().find(|(n, _)| n == name).ok_or_else(|| format!("'{name}' is not a PDF of the code library"))?;
    if size > CODE_LIBRARY_MAX_BYTES {
        return Err(format!("'{found}' is {} MB; the limit is {} MB", size / 1_048_576, CODE_LIBRARY_MAX_BYTES / 1_048_576));
    }
    let bytes = std::fs::read(dir.join(&found)).map_err(|e| format!("{found}: {e}"))?;
    Ok(json!({ "name": found, "b64": nnx_core::xlsx::base64_encode(&bytes) }))
}

/// The folder the last successful `folder_import` read: the only place `folder_file` serves drawings from.
static IMPORT_FOLDER: Mutex<Option<PathBuf>> = Mutex::new(None);

/// `pick_folder`: the user's choice of a project folder (`NANOARCH_IMPORT_FOLDER` skips the dialog: the seam of the end-to-end test).
fn pick_folder() -> Result<Value, String> {
    let dir = match std::env::var_os("NANOARCH_IMPORT_FOLDER") {
        Some(p) => Some(PathBuf::from(p)),
        None => rfd::FileDialog::new().pick_folder(),
    };
    Ok(json!({ "picked": dir.map(|d| d.display().to_string()) }))
}

/// One drawing (image or PDF) of the imported folder, in the same shape `pick_sketch` returns, so the sketch tab can open it.
fn folder_file(args: &Value) -> Result<Value, String> {
    let wanted = args.get("path").and_then(Value::as_str).ok_or("folder file: the path is missing")?;
    let root = IMPORT_FOLDER.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or("folder file: no folder has been imported")?;
    let path = std::fs::canonicalize(wanted).map_err(|e| format!("folder file: cannot read {wanted}: {e}"))?;
    if !path.starts_with(&root) {
        return Err("folder file: that file is outside the imported folder".into());
    }
    // A Word file is served as it is: the page unzips it to read its text.
    if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("docx")) {
        let size = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.len();
        if size > SKETCH_MAX_BYTES {
            return Err(format!("the file is {} MB; the limit is {} MB", size / 1_048_576, SKETCH_MAX_BYTES / 1_048_576));
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        return Ok(json!({ "picked": { "name": path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), "kind": "docx", "b64": nnx_core::xlsx::base64_encode(&bytes) } }));
    }
    Ok(json!({ "picked": read_sketch(&path)? }))
}

/// `dwg_to_dxf`: `{ path }` of a DWG inside the imported folder. `{ dxf: text }` when a converter is installed, `{ dxf: null }` when none is.
fn dwg_to_dxf_file(args: &Value) -> Result<Value, String> {
    let wanted = args.get("path").and_then(Value::as_str).ok_or("dwg: the path is missing")?;
    let root = IMPORT_FOLDER.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or("dwg: no folder has been imported")?;
    let path = std::fs::canonicalize(wanted).map_err(|e| format!("dwg: cannot read {wanted}: {e}"))?;
    if !path.starts_with(&root) || !path.extension().is_some_and(|x| x.eq_ignore_ascii_case("dwg")) {
        return Err("dwg: that is not a DWG of the imported folder".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("dwg: {}: {e}", path.display()))?;
    Ok(json!({ "dxf": dwg::dwg_to_dxf(&bytes)? }))
}

/// Several plan images at once (one per level) for a plan import. The user picks them in a dialog; `NANOARCH_PLAN_FILES` (paths separated
/// by `;`) is the seam an end-to-end test of the real window uses instead.
fn pick_plans() -> Result<Value, String> {
    let paths: Vec<PathBuf> = match std::env::var_os("NANOARCH_PLAN_FILES") {
        Some(p) => std::env::split_paths(&p).collect::<Vec<_>>().into_iter().flat_map(|q| q.to_string_lossy().split(';').filter(|x| !x.is_empty()).map(PathBuf::from).collect::<Vec<_>>()).collect(),
        None => rfd::FileDialog::new().add_filter("Plans (png, jpg, bmp, webp)", &["png", "jpg", "jpeg", "bmp", "webp"]).pick_files().unwrap_or_default(),
    };
    let mut out = Vec::new();
    for path in &paths {
        if sketch_kind(path).is_some_and(|(kind, _)| kind == "pdf") {
            return Err(format!("{}: a PDF is not read here: export the page as an image first", path.display()));
        }
        out.push(read_sketch(path)?);
    }
    Ok(json!({ "picked": out }))
}

fn pick_sketch() -> Result<Value, String> {
    let path = match std::env::var_os("NANOARCH_SKETCH_FILE") {
        Some(p) => Some(PathBuf::from(p)),
        None => rfd::FileDialog::new().add_filter("Sketch (png, jpg, bmp, webp, pdf)", &["png", "jpg", "jpeg", "bmp", "webp", "pdf"]).pick_file(),
    };
    let Some(path) = path else { return Ok(json!({ "picked": null })) };
    Ok(json!({ "picked": read_sketch(&path)? }))
}

fn read_sketch(path: &Path) -> Result<Value, String> {
    let path = path.to_path_buf();
    let (kind, mime) = sketch_kind(&path).ok_or("unsupported file type: use png, jpg, bmp, webp or pdf")?;
    let size = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    if size > SKETCH_MAX_BYTES {
        return Err(format!("the file is {} MB; the limit is {} MB", size / 1_048_576, SKETCH_MAX_BYTES / 1_048_576));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(json!({ "name": name, "kind": kind, "mime": mime, "b64": nnx_core::xlsx::base64_encode(&bytes) }))
}

/// Where a project file is saved or opened. The user picks it in a dialog; `NANOARCH_EXPORT_DIR` (the seam of `pick_save_path`) makes it
/// `project.nnx` in that folder, so an end-to-end test of the real window can save and reopen a project.
fn project_path(save: bool) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("NANOARCH_EXPORT_DIR") {
        return Some(Path::new(&dir).join("project.nnx"));
    }
    let dialog = rfd::FileDialog::new().add_filter("NanoArch project", &["nnx"]);
    if save { dialog.set_file_name("project.nnx").save_file() } else { dialog.pick_file() }
}

/// Saves the open plan as `.nnx`, then records that state as saved so the close warning goes away.
fn save_project(app: &Shared) -> Result<Value, String> {
    let (model, settings) = {
        let guard = lock(app);
        (guard.model()?.clone(), guard.settings())
    };
    match project_path(true) {
        Some(path) => {
            nnx_core::persist::save_with_settings(&model, &settings, &path).map_err(|e| e.to_string())?;
            lock(app).mark_saved();
            Ok(json!({ "saved": path.display().to_string() }))
        }
        None => Ok(json!({ "saved": null })),
    }
}

/// Opens a saved `.nnx` project, replacing the current plan. The page is told to ask first if there are
/// unsaved changes (it checks `status`), so this only reads the file.
fn open_project(app: &Shared) -> Result<Value, String> {
    let Some(path) = project_path(false) else {
        return Ok(json!({ "opened": null }));
    };
    let (model, settings) = nnx_core::persist::load_with_settings(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut result = lock(app).open(model, &settings)?;
    result["opened"] = json!(path.display().to_string());
    Ok(result)
}

fn confirm_discard() -> bool {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("NanoArch")
        .set_description("There are unsaved changes. Close without saving?\nيوجد عمل غير محفوظ. هل تغلق دون حفظ؟")
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes
}

/// Asks where to save the PDF (on a worker thread), then hands the document to the event loop to print.
fn start_pdf(msg: &Value, proxy: &EventLoopProxy<Ui>) {
    let id = msg.get("id").and_then(Value::as_u64).unwrap_or(0);
    let args = msg.get("args").cloned().unwrap_or(Value::Null);
    let reply = |payload: Value| {
        let _ = proxy.send_event(Ui::Js(format!("window.__resolve({id}, {payload});")));
    };
    let Some(html) = args.get("html").and_then(Value::as_str) else {
        return reply(json!({ "ok": false, "error": "nothing to print" }));
    };
    match pick_save_path(args.get("name").and_then(Value::as_str).unwrap_or("plan.pdf")) {
        Some(path) => {
            // A3 for plan sheets (the default), A4 for documents such as the bill of quantities.
            let size_mm = if args.get("size").and_then(Value::as_str) == Some("A4") { (210.0, 297.0) } else { (297.0, 420.0) };
            let _ = proxy.send_event(Ui::Pdf { id, html: html.to_string(), path, size_mm });
        }
        None => reply(json!({ "ok": true, "data": { "saved": null } })),
    }
}

/// A hidden window that loads the document from a temporary file (a web view cannot take a large page as a string).
fn open_printer(target: &EventLoopWindowTarget<Ui>, proxy: &EventLoopProxy<Ui>, n: usize, id: u64, html: &str, path: PathBuf, size_mm: (f64, f64)) -> Result<Printer, String> {
    let html_file = std::env::temp_dir().join(format!("nanoarch-print-{}-{n}.html", std::process::id()));
    std::fs::write(&html_file, html).map_err(|e| e.to_string())?;
    let window = WindowBuilder::new().with_title("NanoArch print").with_visible(false).with_inner_size(LogicalSize::new(1200.0, 900.0)).build(target).map_err(|e| e.to_string())?;
    let p = proxy.clone();
    let url = format!("file:///{}", html_file.display().to_string().replace('\\', "/"));
    let webview = WebViewBuilder::new()
        .with_url(&url)
        .with_on_page_load_handler(move |event, _| {
            if matches!(event, PageLoadEvent::Finished) {
                let _ = p.send_event(Ui::PrintNow(n));
            }
        })
        .build(&window)
        .map_err(|e| e.to_string())?;
    Ok(Printer { _window: window, webview, id, path, html_file, size_mm, started: false })
}

/// Prints the loaded document to `path` with WebView2 on a portrait page of `size_mm`. `done` is called once, when
/// the PDF is written or has failed.
#[cfg(windows)]
fn print_to_pdf(webview: &WebView, path: &Path, size_mm: (f64, f64), done: impl FnOnce(Result<(), String>) + 'static) -> Result<(), String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{COREWEBVIEW2_PRINT_ORIENTATION_PORTRAIT, ICoreWebView2, ICoreWebView2Environment6, ICoreWebView2PrintSettings, ICoreWebView2_7};
    use webview2_com::PrintToPdfCompletedHandler;
    use windows::core::{HSTRING, Interface};
    use wry::WebViewExtWindows;
    let core: ICoreWebView2 = webview.webview();
    let core7: ICoreWebView2_7 = core.cast().map_err(|e| format!("this WebView2 runtime cannot print to PDF: {e}"))?;
    // Portrait page of the requested size with 12 mm margins and backgrounds: the web engine ignores the CSS page size.
    // If the runtime is too old for print settings, its default page is used instead.
    let settings: Option<ICoreWebView2PrintSettings> = webview.environment().cast::<ICoreWebView2Environment6>().ok().and_then(|env| unsafe { env.CreatePrintSettings() }.ok()).and_then(|s| {
        let a3 = (|| unsafe {
            s.SetOrientation(COREWEBVIEW2_PRINT_ORIENTATION_PORTRAIT)?;
            s.SetPageWidth(size_mm.0 / 25.4)?;
            s.SetPageHeight(size_mm.1 / 25.4)?;
            for set in [ICoreWebView2PrintSettings::SetMarginTop, ICoreWebView2PrintSettings::SetMarginBottom, ICoreWebView2PrintSettings::SetMarginLeft, ICoreWebView2PrintSettings::SetMarginRight] {
                set(&s, 12.0 / 25.4)?;
            }
            s.SetShouldPrintBackgrounds(true)?;
            Ok::<(), windows::core::Error>(())
        })();
        a3.ok().map(|()| s)
    });
    let handler = PrintToPdfCompletedHandler::create(Box::new(move |error, ok| {
        done(match error {
            Err(e) => Err(e.to_string()),
            Ok(()) if ok => Ok(()),
            Ok(()) => Err("printing to PDF failed".to_string()),
        });
        Ok(())
    }));
    unsafe { core7.PrintToPdf(&HSTRING::from(path.as_os_str()), settings.as_ref(), &handler) }.map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn print_to_pdf(_: &WebView, _: &Path, _: (f64, f64), done: impl FnOnce(Result<(), String>) + 'static) -> Result<(), String> {
    done(Err("PDF export needs Windows".into()));
    Ok(())
}

/// The interface needs the Microsoft Edge WebView2 runtime; without it the window cannot be built. Tell the user what to
/// install instead of closing with no message (a GUI app has no console to print a panic to).
fn webview_missing(detail: &str) {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("NanoArch")
        .set_description(format!(
            "NanoArch needs the Microsoft Edge WebView2 Runtime, which was not found on this computer.
             Install it from https://developer.microsoft.com/microsoft-edge/webview2/ then start NanoArch again.

             يحتاج NanoArch إلى مكوّن Microsoft Edge WebView2 Runtime ولم يُعثر عليه على هذا الجهاز.
             ثبّته من https://developer.microsoft.com/microsoft-edge/webview2/ ثم شغّل البرنامج مرة أخرى.

{detail}"
        ))
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

fn main() {
    let event_loop = EventLoopBuilder::<Ui>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let ui_proxy = proxy.clone();
    let window = WindowBuilder::new()
        .with_title("NanoArch - NanoNexus")
        .with_window_icon(Some(window_icon()))
        .with_inner_size(LogicalSize::new(1440.0, 900.0))
        .build(&event_loop)
        .expect("window");

    let app: Shared = Arc::new(Mutex::new(backend::App::default()));
    let handler_app = Arc::clone(&app);
    let webview = WebViewBuilder::new()
        .with_html(page_html())
        .with_ipc_handler(move |req| {
            let body = req.body().clone();
            let (app, proxy) = (Arc::clone(&handler_app), proxy.clone());
            std::thread::spawn(move || {
                let msg: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                if msg.get("cmd").and_then(Value::as_str) == Some("save_pdf") {
                    start_pdf(&msg, &proxy);
                } else {
                    let _ = proxy.send_event(Ui::Js(respond(&app, &body)));
                }
            });
        })
        .build(&window);
    let webview = match webview {
        Ok(w) => w,
        Err(e) => {
            webview_missing(&e.to_string());
            return;
        }
    };

    let mut printers: Vec<Option<Printer>> = Vec::new();
    event_loop.run(move |event, target, flow| {
        *flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(Ui::Js(js)) => {
                let _ = webview.evaluate_script(&js);
            }
            Event::UserEvent(Ui::Pdf { id, html, path, size_mm }) => match open_printer(target, &ui_proxy, printers.len(), id, &html, path, size_mm) {
                Ok(p) => printers.push(Some(p)),
                Err(e) => {
                    let _ = webview.evaluate_script(&format!("window.__resolve({id}, {});", json!({ "ok": false, "error": e })));
                }
            },
            Event::UserEvent(Ui::PrintNow(n)) => {
                if let Some(Some(p)) = printers.get_mut(n).filter(|p| p.as_ref().is_some_and(|p| !p.started)) {
                    p.started = true;
                    let proxy = ui_proxy.clone();
                    if let Err(e) = print_to_pdf(&p.webview, &p.path, p.size_mm, move |r| {
                        let _ = proxy.send_event(Ui::PdfDone { n, error: r.err() });
                    }) {
                        let _ = ui_proxy.send_event(Ui::PdfDone { n, error: Some(e) });
                    }
                }
            }
            Event::UserEvent(Ui::PdfDone { n, error }) => {
                if let Some(p) = printers.get_mut(n).and_then(Option::take) {
                    let payload = match error {
                        None => json!({ "ok": true, "data": { "saved": p.path.display().to_string() } }),
                        Some(e) => json!({ "ok": false, "error": e }),
                    };
                    let _ = webview.evaluate_script(&format!("window.__resolve({}, {payload});", p.id));
                    let _ = std::fs::remove_file(&p.html_file);
                }
            }
            // Close at once when nothing is unsaved; otherwise only if the user confirms.
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } if !lock(&app).is_dirty() || confirm_discard() => {
                *flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_code_library_serves_only_pdfs_listed_in_its_own_folder() {
        let dir = std::env::temp_dir().join(format!("nanoarch_lib_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.pdf"), b"%PDF-1.4 test").unwrap();
        std::fs::write(dir.join("notes.txt"), b"not a pdf").unwrap();
        let names: Vec<String> = library_files(&dir).unwrap().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["a.pdf".to_string()], "only PDFs are listed");
        let served = serve_library_file(&dir, "a.pdf").unwrap();
        assert_eq!(served["name"], "a.pdf");
        assert_eq!(served["b64"], nnx_core::xlsx::base64_encode(b"%PDF-1.4 test"));
        for bad in ["notes.txt", "../a.pdf", "..\\a.pdf", "missing.pdf"] {
            assert!(serve_library_file(&dir, bad).unwrap_err().contains("not a PDF of the code library"), "{bad} is refused");
        }
        assert!(library_files(&dir.join("absent")).unwrap_err().contains("cannot be read"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sketch_types_are_recognised_by_extension_only_when_supported() {
        let k = |n: &str| sketch_kind(Path::new(n));
        assert_eq!(k("a.PDF"), Some(("pdf", "application/pdf")));
        assert_eq!(k("plan.jpeg"), Some(("image", "image/jpeg")));
        assert_eq!(k("x.png"), Some(("image", "image/png")));
        assert_eq!(k("x.dwg"), None);
        assert_eq!(k("noext"), None);
    }

    #[test]
    fn the_window_icon_is_a_valid_64_pixel_rgba_image_that_is_not_blank() {
        assert_eq!(ICON_RGBA.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        let _ = window_icon();
        let opaque = ICON_RGBA.chunks(4).filter(|p| p[3] == 255).count();
        let transparent = ICON_RGBA.chunks(4).filter(|p| p[3] == 0).count();
        assert!(opaque > 2000, "the rounded square is drawn: {opaque}");
        assert!(transparent > 100, "its corners are transparent: {transparent}");
        let colours: std::collections::BTreeSet<[u8; 3]> = ICON_RGBA.chunks(4).filter(|p| p[3] == 255).map(|p| [p[0] / 16, p[1] / 16, p[2] / 16]).collect();
        assert!(colours.len() > 20, "a shaded 3D drawing, not a flat fill: {} colours", colours.len());
    }

    #[test]
    fn the_icon_files_are_a_multi_size_ico_and_a_png() {
        let ico = include_bytes!("../assets/icon.ico");
        assert_eq!(&ico[..4], &[0, 0, 1, 0], "ICO header");
        let n = u16::from_le_bytes([ico[4], ico[5]]) as usize;
        assert_eq!(n, 7);
        let mut sizes = Vec::new();
        for i in 0..n {
            let e = &ico[6 + 16 * i..6 + 16 * (i + 1)];
            let (w, len, off) = (if e[0] == 0 { 256 } else { e[0] as u32 }, u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize, u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize);
            assert_eq!(&ico[off..off + 8], b"\x89PNG\r\n\x1a\n", "entry {i} is a PNG");
            assert!(off + len <= ico.len());
            sizes.push(w);
        }
        assert_eq!(sizes, vec![16, 24, 32, 48, 64, 128, 256]);
        let png = include_bytes!("../assets/icon.png");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        // The PNG header holds its size: 512 x 512.
        assert_eq!((u32::from_be_bytes([png[16], png[17], png[18], png[19]]), u32::from_be_bytes([png[20], png[21], png[22], png[23]])), (512, 512));
    }

    #[test]
    fn the_page_carries_the_bundled_fonts_and_no_network_font() {
        let html = page_html();
        assert!(!html.contains("/*NN_FONTS*/"), "the marker must be replaced");
        assert_eq!(HTML.matches("/*NN_FONTS*/").count(), 1, "the page must carry the font marker once");
        assert_eq!(HTML.matches("/*NN_PREFS*/{}").count(), 1, "the page must carry the preferences marker once");
        assert!(!html.contains("/*NN_PREFS*/"), "the preferences marker must be replaced");
        // WebView2's NavigateToString refuses a document over 2 MB (2 * 1024 * 1024 bytes; Microsoft Learn, ICoreWebView2::NavigateToString).
        // Counted as UTF-16 (two bytes a character), the worst reading of "total size", with a tenth kept free.
        let bytes = html.encode_utf16().count() * 2;
        assert!(bytes < 2 * 1024 * 1024 * 9 / 10, "the page is {bytes} bytes (UTF-16), too close to the 2 MB limit of NavigateToString");
        assert_eq!(html.matches("@font-face").count(), 8);
        assert!(html.contains("font-weight:700") && html.contains("IBM Plex Sans Arabic"));
        // Every face is a data URL: the page names no http(s) font and no font file path.
        assert!(!html.contains("fonts.googleapis") && !html.contains("fonts.gstatic"));
        assert!(FONT_FACES.iter().all(|(_, _, _, b)| b.len() > 10_000 && &b[..4] == b"wOF2"));
    }

    #[test]
    fn interface_choices_round_trip_through_their_own_file() {
        let dir = std::env::temp_dir().join(format!("nanoarch-ui-prefs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("p.ui-prefs.json");
        assert_eq!(ui_prefs_json(&file), "{}", "no file yet: no choices");
        std::fs::write(&file, r#"{"theme":"dark","accent":"teal","sizes":{"--w-res":512},"note":"</script>"}"#).unwrap();
        let json = ui_prefs_json(&file);
        assert!(json.contains(r#""theme":"dark""#) && json.contains("512"));
        assert!(!json.contains('<'), "a stored '<' must not be able to close the script element: {json}");
        std::fs::write(&file, "[1,2]").unwrap();
        assert!(ui_prefs_json(&file).contains("_error"), "a damaged file is reported to the page, not ignored");
        std::fs::write(&file, "{not json").unwrap();
        assert!(ui_prefs_json(&file).contains("_error"));
        // A valid object whose maps are not maps would stop the page: the bad parts are dropped and reported, the rest is kept.
        std::fs::write(&file, r#"{"theme":"dark","sizes":null,"open":[1]}"#).unwrap();
        let json = ui_prefs_json(&file);
        assert!(json.contains("_error") && json.contains(r#""theme":"dark""#), "{json}");
        assert!(!json.contains("sizes") && !json.contains("open"), "{json}");
        assert!(ui_prefs_set(&json!({ "prefs": [1] })).is_err());
        assert!(ui_prefs_set(&json!({ "prefs": { "k": "x".repeat(UI_PREFS_MAX_BYTES) } })).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_pdf_engine_is_bundled_and_looks_like_pdfjs() {
        assert!(PDFJS.contains("pdfjsLib") && PDFJS.len() > 100_000);
        assert!(PDFJS_WORKER.len() > 500_000);
    }
}
