//! The reference library commands: import a PDF or a text/HTML file the user picks, or a web page/PDF from a link, keep the page texts in
//! the library folder and search them while the user works. No engineering logic lives here: the engine (`nnx_core::library`) stores and
//! searches; this shell only opens dialogs and fetches the link the user typed.
//!
//! A link is fetched over HTTPS only, from a public address (not this computer or a private network), up to 60 MB, and only on the user's
//! action. Nothing about the project is sent: the request is a plain GET of the address typed.

use nnx_core::library::{Kind, Library, NewEntry, Origin, html_to_pages};
use nnx_core::xlsx::base64_encode;
use serde_json::{Value, json};
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

const MAX_BYTES: u64 = 60 * 1024 * 1024;

fn dir() -> PathBuf {
    if let Some(d) = std::env::var_os("NANOARCH_LIBRARY_DIR") {
        return PathBuf::from(d);
    }
    std::env::var_os("APPDATA").map_or_else(|| PathBuf::from("NanoArch-library"), |a| PathBuf::from(a).join("NanoArch").join("library"))
}

fn open() -> Result<Library, String> {
    Library::open(dir()).map_err(|e| e.to_string())
}

fn text<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).map(str::trim).unwrap_or("")
}

pub fn handle(cmd: &str, args: &Value) -> Result<Value, String> {
    match cmd {
        "library_list" => {
            let entries = open()?.list().map_err(|e| e.to_string())?;
            Ok(json!({ "dir": dir().display().to_string(), "entries": entries }))
        }
        "library_add" => add(args),
        "library_remove" => {
            open()?.remove(text(args, "id")).map_err(|e| e.to_string())?;
            Ok(json!({ "removed": true }))
        }
        "library_search" => {
            let kind = match text(args, "kind") {
                "" | "all" => None,
                k => Some(Kind::parse(k).map_err(|e| e.to_string())?),
            };
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20).clamp(1, 100) as usize;
            let hits = open()?.search(text(args, "query"), kind, limit).map_err(|e| e.to_string())?;
            Ok(json!({ "hits": hits }))
        }
        "library_page" => {
            let page = args.get("page").and_then(Value::as_u64).ok_or("library_page: 'page' is required")? as usize;
            let (entry, body) = open()?.page(text(args, "id"), page).map_err(|e| e.to_string())?;
            Ok(json!({ "citation": entry.citation(Some(page)), "title": entry.title, "page": page, "text": body }))
        }
        "library_pick" => pick(),
        "library_fetch" => fetch(text(args, "url")),
        _ => Err(format!("unknown library command '{cmd}'")),
    }
}

fn add(args: &Value) -> Result<Value, String> {
    let kind = Kind::parse(text(args, "kind")).map_err(|e| e.to_string())?;
    let pages: Vec<String> = args.get("pages").and_then(Value::as_array).ok_or("library_add: 'pages' (the text of each page) is required")?.iter().map(|p| p.as_str().unwrap_or("").to_string()).collect();
    let origin = match args.get("origin") {
        Some(o) if o.get("type").and_then(Value::as_str) == Some("link") => Origin::Link { url: o.get("url").and_then(Value::as_str).unwrap_or("").to_string() },
        Some(o) => Origin::File { name: o.get("name").and_then(Value::as_str).unwrap_or("").to_string() },
        None => return Err("library_add: 'origin' is required".into()),
    };
    let entry = open()?.add(NewEntry { kind, title: text(args, "title").into(), issuer: text(args, "issuer").into(), edition: text(args, "edition").into(), origin, pages }).map_err(|e| e.to_string())?;
    Ok(json!({ "entry": entry }))
}

/// The user picks a PDF, text, Markdown or HTML file. A PDF is returned as base64 for the page to read with pdf.js; the others come back as pages.
/// `NANOARCH_LIBRARY_FILE` skips the dialog (the seam of the end-to-end tests).
fn pick() -> Result<Value, String> {
    let path = match std::env::var_os("NANOARCH_LIBRARY_FILE") {
        Some(p) => Some(PathBuf::from(p)),
        None => rfd::FileDialog::new().add_filter("PDF / text / HTML", &["pdf", "txt", "md", "html", "htm"]).pick_file(),
    };
    let Some(path) = path else { return Ok(json!({ "picked": null })) };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let size = std::fs::metadata(&path).map_err(|e| format!("{name}: {e}"))?.len();
    if size > MAX_BYTES {
        return Err(format!("{name} is {} MB; the limit is {} MB", size / 1_048_576, MAX_BYTES / 1_048_576));
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("{name}: {e}"))?;
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let picked = match ext.as_str() {
        "pdf" => json!({ "name": name, "kind": "pdf", "b64": base64_encode(&bytes) }),
        "html" | "htm" => {
            let (title, pages) = html_to_pages(&String::from_utf8_lossy(&bytes));
            json!({ "name": name, "kind": "pages", "title": title, "pages": pages })
        }
        _ => {
            let s = String::from_utf8(bytes).map_err(|_| format!("{name} is not UTF-8 text"))?;
            json!({ "name": name, "kind": "pages", "title": null, "pages": split_text(&s) })
        }
    };
    Ok(json!({ "picked": picked }))
}

/// Plain text split into pages at form feeds, or into blocks of about 3000 characters at line ends.
fn split_text(s: &str) -> Vec<String> {
    if s.contains('\u{c}') {
        return s.split('\u{c}').map(str::to_string).collect();
    }
    let mut pages = Vec::new();
    let mut cur = String::new();
    for line in s.lines() {
        if cur.chars().count() + line.chars().count() > 3000 && !cur.is_empty() {
            pages.push(std::mem::take(&mut cur));
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.trim().is_empty() {
        pages.push(cur);
    }
    pages
}

/// `Ok(host)` for an https address on the public internet; otherwise the reason it is refused.
fn check_url(url: &str) -> Result<String, String> {
    let rest = url.strip_prefix("https://").ok_or("only https:// links are accepted")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err("a link with a user name or password in it is not accepted".into());
    }
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.rsplit_once(':').map_or(authority, |(h, port)| if port.chars().all(|c| c.is_ascii_digit()) { h } else { authority }),
    };
    if host.is_empty() {
        return Err("the link has no host".into());
    }
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") || lower.ends_with(".local") || lower.ends_with(".internal") {
        return Err("a link to this computer or a private network is not accepted".into());
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        let private = match ip {
            IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local() || v.is_unspecified() || v.is_broadcast(),
            IpAddr::V6(v) => v.is_loopback() || v.is_unspecified() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80,
        };
        if private {
            return Err("a link to this computer or a private network is not accepted".into());
        }
    }
    Ok(host.to_string())
}

fn fetch(url: &str) -> Result<Value, String> {
    check_url(url)?;
    let agent: ureq::Agent = ureq::Agent::config_builder().https_only(true).http_status_as_error(false).timeout_global(Some(Duration::from_secs(60))).build().into();
    let mut resp = agent.get(url).header("User-Agent", "NanoArch-library").call().map_err(|e| format!("cannot reach {url}: {e}"))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!("{url} answered HTTP {status}"));
    }
    let ctype = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    let bytes = resp.body_mut().with_config().limit(MAX_BYTES).read_to_vec().map_err(|e| format!("{url}: {e} (limit {} MB)", MAX_BYTES / 1_048_576))?;
    let name = url.split(['?', '#']).next().unwrap_or(url).rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("link").to_string();
    if ctype.contains("pdf") || bytes.starts_with(b"%PDF") {
        return Ok(json!({ "fetched": { "url": url, "name": name, "kind": "pdf", "b64": base64_encode(&bytes) } }));
    }
    if ctype.contains("html") || ctype.contains("text/plain") || ctype.contains("xml") || ctype.is_empty() {
        let body = String::from_utf8_lossy(&bytes);
        let (title, pages) = if ctype.contains("plain") { (None, split_text(&body)) } else { html_to_pages(&body) };
        return Ok(json!({ "fetched": { "url": url, "name": name, "kind": "pages", "title": title, "pages": pages } }));
    }
    Err(format!("{url} is '{ctype}': only a PDF, a web page or text can be imported"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_https_links_are_fetched() {
        assert!(check_url("https://example.org/code.pdf").is_ok());
        assert!(check_url("https://example.org:8443/a?b=1").is_ok());
        for bad in ["http://example.org/a", "https://localhost/a", "https://127.0.0.1/a", "https://192.168.1.5/a", "https://10.0.0.2:80/a", "https://[::1]/a", "https://user:pw@example.org/a", "ftp://x/a", "https://", "https://printer.local/a", "https://169.254.1.1/"] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_commands_import_search_cite_and_remove() {
        let dir = std::env::temp_dir().join(format!("nnx-libcmd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: this is the only test that sets the variable, and it does so before any library command runs.
        unsafe { std::env::set_var("NANOARCH_LIBRARY_DIR", &dir) };
        let add = handle("library_add", &json!({ "kind": "spec", "title": "Spec 7", "edition": "2020", "origin": { "type": "link", "url": "https://example.org/s" }, "pages": ["cover 40 mm", "slump 100 mm"] })).unwrap();
        let id = add["entry"]["id"].as_str().unwrap().to_string();
        let hits = handle("library_search", &json!({ "query": "slump", "kind": "spec" })).unwrap();
        assert_eq!(hits["hits"][0]["citation"], "Spec 7 (2020), p. 2");
        assert_eq!(handle("library_page", &json!({ "id": id, "page": 1 })).unwrap()["text"], "cover 40 mm");
        assert!(handle("library_add", &json!({ "kind": "spec", "title": "x", "pages": [] })).is_err(), "origin missing");
        assert_eq!(handle("library_list", &json!({})).unwrap()["entries"].as_array().unwrap().len(), 1);
        handle("library_remove", &json!({ "id": id })).unwrap();
        assert!(handle("library_list", &json!({})).unwrap()["entries"].as_array().unwrap().is_empty());
        assert!(split_text("a\nb").len() == 1 && split_text("p1\u{c}p2").len() == 2);
    }
}
