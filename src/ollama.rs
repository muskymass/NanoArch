//! The optional local brief assistant: a client for an Ollama server on THIS computer (`http://localhost:11434` by default).
//!
//! - **Local only.** The endpoint must be `localhost`, `127.0.0.1` or `::1` over plain HTTP; anything else is refused before any
//!   connection, because the description of a client's project is not sent to another machine.
//! - **Optional.** If the server is not running, has no model, is too slow or answers with something that is not the JSON asked for,
//!   the brief is filled from the keywords of the text instead (`nnx_core::briefai::from_keywords`) and the answer says why.
//! - **Checked.** The model's JSON goes through `briefai::from_model_json` and `briefai::reconcile`: what the text states wins over
//!   what the model says, and room programs the model proposed are marked as assumed.
//!
//! The HTTP is plain `std::net` (one request per connection, `Connection: close`), so no extra dependency is needed.

use nnx_core::briefai;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

pub const DEFAULT_ENDPOINT: &str = "http://localhost:11434";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_READ_TIMEOUT_S: u64 = 120;

/// `(host, port)` of an endpoint like `http://localhost:11434`, `localhost:11434` or `127.0.0.1`, if it is on this computer.
pub fn parse_endpoint(s: &str) -> Result<(String, u16), String> {
    let s = s.trim();
    let rest = match s.split_once("://") {
        Some(("http", r)) => r,
        Some((scheme, _)) => return Err(format!("the model server must be plain http on this computer, not '{scheme}'")),
        None => s,
    };
    let hostport = rest.split('/').next().unwrap_or("");
    let (host, port) = if let Some(r) = hostport.strip_prefix('[') {
        let (h, p) = r.split_once(']').ok_or("bad IPv6 address")?;
        (h.to_string(), p.strip_prefix(':').map(str::to_string))
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), Some(p.to_string())),
            None => (hostport.to_string(), None),
        }
    };
    if !["localhost", "127.0.0.1", "::1"].contains(&host.to_lowercase().as_str()) {
        return Err(format!("only a model server on this computer (localhost, 127.0.0.1 or ::1) is allowed, not '{host}': your project text is not sent to another machine"));
    }
    let port = match port {
        Some(p) => p.parse::<u16>().map_err(|_| format!("bad port '{p}'"))?,
        None => 11434,
    };
    Ok((host, port))
}

/// Decodes a `Transfer-Encoding: chunked` body.
fn dechunk(mut b: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let eol = b.windows(2).position(|w| w == b"\r\n").ok_or("bad chunked body")?;
        let size = usize::from_str_radix(std::str::from_utf8(&b[..eol]).map_err(|_| "bad chunk size")?.split(';').next().unwrap_or("").trim(), 16).map_err(|_| "bad chunk size")?;
        b = &b[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if b.len() < size + 2 {
            return Err("the chunked body is cut short".into());
        }
        out.extend_from_slice(&b[..size]);
        b = &b[size + 2..];
    }
}

/// One request to the server: `(status, body)`.
fn request(host: &str, port: u16, method: &str, path: &str, body: Option<&str>, read_timeout: Duration) -> Result<(u16, String), String> {
    // IPv4 first: Ollama listens on 127.0.0.1 by default, and on Windows a refused connection to `::1` (the first address
    // `localhost` resolves to) takes about two seconds before the next address is tried.
    let mut addrs: Vec<_> = (host, port).to_socket_addrs().map_err(|e| format!("cannot resolve {host}: {e}"))?.collect();
    addrs.sort_by_key(|a| a.is_ipv6());
    let mut stream = None;
    let mut last = String::from("no address");
    for a in addrs {
        match TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = e.to_string(),
        }
    }
    let mut stream = stream.ok_or_else(|| format!("cannot connect to {host}:{port}: {last}"))?;
    stream.set_read_timeout(Some(read_timeout)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
    let payload = body.unwrap_or("");
    let req = format!("{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}", payload.len());
    stream.write_all(req.as_bytes()).map_err(|e| format!("cannot send the request: {e}"))?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut { format!("the model did not answer within {} s", read_timeout.as_secs()) } else { format!("cannot read the answer: {e}") })?;
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("the server's answer is not HTTP")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let status: u16 = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse().ok()).ok_or("the server's answer has no status")?;
    let chunked = head.lines().any(|l| l.to_lowercase().starts_with("transfer-encoding:") && l.to_lowercase().contains("chunked"));
    let body = &raw[split + 4..];
    let body = if chunked { dechunk(body)? } else { body.to_vec() };
    Ok((status, String::from_utf8_lossy(&body).to_string()))
}

/// The names of the models installed on the server.
pub fn models(endpoint: &str) -> Result<Vec<String>, String> {
    let (host, port) = parse_endpoint(endpoint)?;
    let (status, body) = request(&host, port, "GET", "/api/tags", None, Duration::from_secs(10))?;
    if status != 200 {
        return Err(format!("the server answered {status}: {}", body.chars().take(200).collect::<String>()));
    }
    let v: Value = serde_json::from_str(&body).map_err(|_| "the model list is not JSON (is this an Ollama server?)")?;
    Ok(v["models"].as_array().map(|a| a.iter().filter_map(|m| m["name"].as_str().map(str::to_string)).collect()).unwrap_or_default())
}

/// Asks `model` to answer with JSON following `schema`. Returns the parsed JSON of its reply.
pub fn chat_json(endpoint: &str, model: &str, system: &str, user: &str, schema: &Value, timeout_s: u64) -> Result<Value, String> {
    chat_json_images(endpoint, model, system, user, &[], schema, timeout_s)
}

/// Like `chat_json`, with images (base64) attached to the user's message for a vision model.
pub fn chat_json_images(endpoint: &str, model: &str, system: &str, user: &str, images_b64: &[String], schema: &Value, timeout_s: u64) -> Result<Value, String> {
    let (host, port) = parse_endpoint(endpoint)?;
    let mut user_msg = json!({ "role": "user", "content": user });
    if !images_b64.is_empty() {
        user_msg["images"] = json!(images_b64);
    }
    let body = json!({
        "model": model, "stream": false, "format": schema, "options": { "temperature": 0 },
        "messages": [{ "role": "system", "content": system }, user_msg]
    })
    .to_string();
    let (status, resp) = request(&host, port, "POST", "/api/chat", Some(&body), Duration::from_secs(timeout_s.max(1)))?;
    let v: Value = serde_json::from_str(&resp).map_err(|_| format!("the server's answer is not JSON ({status})"))?;
    if status != 200 {
        return Err(format!("the server answered {status}: {}", v["error"].as_str().unwrap_or("no message")));
    }
    let content = v["message"]["content"].as_str().ok_or("the model's answer has no content")?;
    serde_json::from_str(content.trim()).map_err(|_| "the model did not return JSON".to_string())
}

fn endpoint_of(args: &Value) -> String {
    args.get("endpoint").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_ENDPOINT).to_string()
}

/// `ai_status` and `ai_brief`. They run outside the application's lock: a slow model never freezes the other commands.
pub fn handle(cmd: &str, args: &Value) -> Result<Value, String> {
    let endpoint = endpoint_of(args);
    match cmd {
        "ai_status" => {
            parse_endpoint(&endpoint)?;
            Ok(match models(&endpoint) {
                Ok(m) => json!({ "online": true, "endpoint": endpoint, "models": m }),
                Err(e) => json!({ "online": false, "endpoint": endpoint, "models": [], "error": e }),
            })
        }
        "ai_brief" => {
            let text = args.get("text").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).ok_or("describe the project first")?;
            parse_endpoint(&endpoint)?;
            let timeout = args.get("timeout_s").and_then(Value::as_u64).unwrap_or(DEFAULT_READ_TIMEOUT_S);
            let fallback = |why: String| -> Value {
                let mut d = briefai::from_keywords(text);
                d.notes.insert(0, format!("the local model was not used ({why}): filled from the keywords found in the text"));
                json!({ "draft": d, "online": false, "model": null })
            };
            let installed = match models(&endpoint) {
                Ok(m) => m,
                Err(e) => return Ok(fallback(format!("Ollama is not reachable at {endpoint}: {e}"))),
            };
            let wanted = args.get("model").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
            let (model, chosen_note) = match (wanted, installed.first()) {
                (Some(m), _) if installed.iter().any(|i| i == m) => (m.to_string(), None),
                (Some(m), Some(first)) => (first.clone(), Some(format!("the model '{m}' is not installed: used '{first}'"))),
                (None, Some(first)) => (first.clone(), None),
                (_, None) => return Ok(fallback("no model is installed: run `ollama pull <model>`".into())),
            };
            match chat_json(&endpoint, &model, &briefai::system_prompt(), text, &briefai::schema(), timeout).and_then(|v| briefai::from_model_json(&v, &model)) {
                Ok(draft) => {
                    let mut d = briefai::reconcile(draft, text);
                    if let Some(n) = chosen_note {
                        d.notes.insert(0, n);
                    }
                    Ok(json!({ "draft": d, "online": true, "model": model }))
                }
                Err(e) => Ok(fallback(format!("{model}: {e}"))),
            }
        }
        "ai_sketch" => ai_sketch(&endpoint, args),
        other => Err(format!("unknown command '{other}'")),
    }
}

/// Name fragments of Ollama models that accept images.
const VISION_HINTS: [&str; 10] = ["llava", "vision", "-vl", "vl:", "minicpm-v", "gemma3", "moondream", "bakllava", "granite3.2-vision", "mllama"];

fn is_vision(name: &str) -> bool {
    let n = name.to_lowercase();
    VISION_HINTS.iter().any(|h| n.contains(h))
}

fn arg_f64(args: &Value, k: &str) -> Result<f64, String> {
    args.get(k).and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| format!("sketch: '{k}' is missing or not a number"))
}

fn arg_point(args: &Value, k: &str) -> Result<(f64, f64), String> {
    let a = args.get(k).and_then(Value::as_array).filter(|a| a.len() == 2).ok_or_else(|| format!("sketch: '{k}' must be [x, y] in image pixels"))?;
    match (a[0].as_f64(), a[1].as_f64()) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Ok((x, y)),
        _ => Err(format!("sketch: '{k}' must be two numbers")),
    }
}

/// `ai_sketch`: a photographed or scanned hand sketch read into unconfirmed rooms by a vision model on this computer.
/// Arguments: `image_b64` (or `path`), `scale_p1` and `scale_p2` (pixels) with `scale_real_mm`, `origin_px` (the south-west corner of
/// the buildable area, in pixels), optional `tolerance_pct`, `model`, `endpoint`, `timeout_s`. There is no keyword fallback: a sketch
/// cannot be read without the model, so an unreachable server or a model that is not a vision model is an error that says so.
fn ai_sketch(endpoint: &str, args: &Value) -> Result<Value, String> {
    use nnx_core::sketchread::{self, Frame, Scale};
    let bytes = match (args.get("image_b64").and_then(Value::as_str).filter(|t| !t.trim().is_empty()), args.get("path").and_then(Value::as_str).filter(|p| !p.trim().is_empty())) {
        (Some(b), _) => nnx_core::xlsx::base64_decode(b).map_err(|e| format!("sketch: {e}"))?,
        (None, Some(p)) => std::fs::read(p).map_err(|e| format!("sketch: cannot read {p}: {e}"))?,
        (None, None) => return Err("sketch: give the image's path, or its content as image_b64".into()),
    };
    let (width_px, height_px) = sketchread::image_size(&bytes).map_err(|e| format!("sketch: {e}"))?;
    let scale = Scale::from_reference(arg_point(args, "scale_p1")?, arg_point(args, "scale_p2")?, arg_f64(args, "scale_real_mm")?).map_err(|e| format!("sketch: {e}"))?;
    let tolerance_pct = match args.get("tolerance_pct") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_f64().ok_or("sketch: tolerance_pct must be a number")?),
    };
    let frame = Frame { width_px, height_px, scale, origin_px: arg_point(args, "origin_px")?, tolerance_pct };
    parse_endpoint(endpoint)?;
    let installed = models(endpoint).map_err(|e| format!("sketch: the local model server is not reachable at {endpoint}: {e}. A sketch cannot be read without it; the manual tracing tool on the Draft sketch tab still works"))?;
    let wanted = args.get("model").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let model = match wanted {
        Some(m) if installed.iter().any(|i| i == m) => m.to_string(),
        Some(m) => return Err(format!("sketch: the model '{m}' is not installed (installed: {})", if installed.is_empty() { "none".to_string() } else { installed.join(", ") })),
        None => installed.iter().find(|m| is_vision(m)).cloned().ok_or_else(|| format!("sketch: no installed model is a vision model (installed: {}): run `ollama pull qwen2.5vl` or name one in `model`", if installed.is_empty() { "none".to_string() } else { installed.join(", ") }))?,
    };
    let timeout = args.get("timeout_s").and_then(Value::as_u64).unwrap_or(DEFAULT_READ_TIMEOUT_S * 3);
    let b64 = nnx_core::xlsx::base64_encode(&bytes);
    let v = chat_json_images(endpoint, &model, &sketchread::system_prompt(width_px, height_px), "Read the rooms of this floor plan.", &[b64], &sketchread::schema(), timeout).map_err(|e| format!("sketch: {model}: {e}"))?;
    let reading = sketchread::from_model_json(&v, &model, &frame).map_err(|e| format!("sketch: {model}: {e}"))?;
    Ok(json!({ "reading": reading, "model": model, "image": { "width_px": width_px, "height_px": height_px }, "mm_per_px": scale.mm_per_px }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    /// Answers each connection with the next canned response and returns the requests it saw. `None`: accept and never answer.
    fn serve(responses: Vec<Option<Vec<u8>>>) -> (u16, JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for resp in responses {
                let (mut s, _) = listener.accept().unwrap();
                s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let n = s.read(&mut tmp).unwrap_or(0);
                    buf.extend_from_slice(&tmp[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(i) = text.find("\r\n\r\n") {
                        let len = text[..i].lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                        if buf.len() >= i + 4 + len {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                seen.push(String::from_utf8_lossy(&buf).to_string());
                match resp {
                    Some(r) => {
                        let _ = s.write_all(&r);
                    }
                    None => std::thread::sleep(Duration::from_secs(3)),
                }
            }
            seen
        });
        (port, h)
    }

    fn ok(body: &str) -> Option<Vec<u8>> {
        Some(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes())
    }

    fn chunked(body: &str) -> Option<Vec<u8>> {
        let (a, b) = body.split_at(body.len() / 2);
        Some(format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n", a.len(), b.len()).into_bytes())
    }

    fn status(code: u16, body: &str) -> Option<Vec<u8>> {
        Some(format!("HTTP/1.1 {code} X\r\nContent-Length: {}\r\n\r\n{body}", body.len()).into_bytes())
    }

    fn chat_answer(content: &Value) -> String {
        json!({ "message": { "role": "assistant", "content": content.to_string() }, "done": true }).to_string()
    }

    const TEXT: &str = "فيلا دورين وملحق في الجبيل على أرض 600م طراز نجدي حديث";

    #[test]
    fn only_endpoints_on_this_computer_are_accepted() {
        assert_eq!(parse_endpoint("http://localhost:11434").unwrap(), ("localhost".into(), 11434));
        assert_eq!(parse_endpoint("127.0.0.1:8080").unwrap(), ("127.0.0.1".into(), 8080));
        assert_eq!(parse_endpoint("http://LOCALHOST").unwrap(), ("LOCALHOST".into(), 11434));
        assert_eq!(parse_endpoint("http://[::1]:11434/api").unwrap(), ("::1".into(), 11434));
        for bad in ["http://192.168.1.5:11434", "http://example.com", "https://localhost:11434", "ftp://localhost", "http://localhost.evil.com:1", "http://localhost:notaport"] {
            assert!(parse_endpoint(bad).is_err(), "{bad}");
        }
        // The refusal happens before any connection, with the reason.
        let e = handle("ai_brief", &json!({ "text": TEXT, "endpoint": "http://10.0.0.7:11434" })).unwrap_err();
        assert!(e.contains("not sent to another machine"), "{e}");
        assert!(handle("ai_status", &json!({ "endpoint": "http://example.com" })).is_err());
    }

    #[test]
    fn the_status_lists_the_installed_models_and_an_unreachable_server_is_offline_not_an_error() {
        let (port, srv) = serve(vec![ok(r#"{"models":[{"name":"qwen2.5:7b"},{"name":"llama3.1:8b"}]}"#)]);
        let r = handle("ai_status", &json!({ "endpoint": format!("http://localhost:{port}") })).unwrap();
        assert_eq!((r["online"].as_bool(), r["models"].clone()), (Some(true), json!(["qwen2.5:7b", "llama3.1:8b"])));
        assert!(srv.join().unwrap()[0].starts_with("GET /api/tags HTTP/1.1"));
        // A port nobody listens on.
        let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let r = handle("ai_status", &json!({ "endpoint": format!("127.0.0.1:{closed}") })).unwrap();
        assert_eq!(r["online"], false);
        assert!(r["error"].as_str().unwrap().contains("cannot connect"));
    }

    #[test]
    fn a_model_answer_fills_the_brief_and_the_text_wins_where_they_disagree() {
        let answer = json!({ "title": "فيلا", "building_type": "villa", "style": "neo_classical", "city": "Jubail", "floors": 3, "annex": true, "plot_area_m2": 600,
            "rooms": [{ "func": "majlis", "name": "المجلس", "area_m2": 40, "floor": 1, "assumed": false }, { "func": "bedroom", "area_m2": 18, "floor": 2, "assumed": true }] });
        let (port, srv) = serve(vec![ok(r#"{"models":[{"name":"qwen2.5:7b"}]}"#), ok(&chat_answer(&answer))]);
        let r = handle("ai_brief", &json!({ "text": TEXT, "endpoint": format!("localhost:{port}"), "model": "qwen2.5:7b" })).unwrap();
        assert_eq!((r["online"].as_bool(), r["model"].as_str()), (Some(true), Some("qwen2.5:7b")));
        let d = &r["draft"];
        assert_eq!(d["source"], "ollama:qwen2.5:7b");
        assert_eq!((d["building_type"].as_str(), d["floors"].as_u64(), d["style"].as_str()), (Some("villa"), Some(2), Some("modern_najdi")), "floors and style come from the text, not the model");
        assert_eq!(d["plot_area_m2"], 600.0);
        assert_eq!(d["climate_zone"], 1);
        assert!(d["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("kept the text")));
        // The model was sent the text, the schema and temperature 0, to the local server only.
        let seen = srv.join().unwrap();
        assert!(seen[1].starts_with("POST /api/chat HTTP/1.1") && seen[1].contains("\"temperature\":0") && seen[1].contains("\"stream\":false") && seen[1].contains("\"format\""));
        assert!(seen[1].contains("الجبيل") && seen[1].contains("Host: localhost:"));
    }

    #[test]
    fn a_chunked_answer_is_decoded() {
        let answer = json!({ "building_type": "house", "floors": 1, "annex": false, "rooms": [] });
        let (port, srv) = serve(vec![chunked(r#"{"models":[{"name":"m1"}]}"#), chunked(&chat_answer(&answer))]);
        let r = handle("ai_brief", &json!({ "text": "منزل دور واحد", "endpoint": format!("127.0.0.1:{port}") })).unwrap();
        assert_eq!(r["online"], true);
        assert_eq!(r["model"], "m1", "no model named: the first installed one");
        assert_eq!(r["draft"]["building_type"], "house");
        srv.join().unwrap();
    }

    #[test]
    fn when_the_server_is_off_or_useless_the_keywords_fill_the_brief_and_the_reason_is_given() {
        // Offline.
        let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let r = handle("ai_brief", &json!({ "text": TEXT, "endpoint": format!("localhost:{closed}") })).unwrap();
        assert_eq!(r["online"], false);
        let d = &r["draft"];
        assert_eq!((d["source"].as_str(), d["building_type"].as_str(), d["floors"].as_u64(), d["plot_area_m2"].as_f64()), (Some("keywords"), Some("villa"), Some(2), Some(600.0)));
        assert!(d["notes"][0].as_str().unwrap().contains("not reachable") && d["notes"][0].as_str().unwrap().contains("filled from the keywords"));
        // No model installed.
        let (port, srv) = serve(vec![ok(r#"{"models":[]}"#)]);
        let r = handle("ai_brief", &json!({ "text": TEXT, "endpoint": format!("localhost:{port}") })).unwrap();
        assert!(r["draft"]["notes"][0].as_str().unwrap().contains("ollama pull"));
        srv.join().unwrap();
        // The model answers with something that is not JSON.
        let (port, srv) = serve(vec![ok(r#"{"models":[{"name":"m"}]}"#), ok(&json!({ "message": { "content": "Sure! Here is your villa." } }).to_string())]);
        let r = handle("ai_brief", &json!({ "text": TEXT, "endpoint": format!("localhost:{port}") })).unwrap();
        assert_eq!(r["online"], false);
        assert!(r["draft"]["notes"][0].as_str().unwrap().contains("did not return JSON"));
        srv.join().unwrap();
        // An error from the server (model not found).
        let (port, srv) = serve(vec![ok(r#"{"models":[{"name":"m"}]}"#), status(404, r#"{"error":"model 'm' not found"}"#)]);
        let r = handle("ai_brief", &json!({ "text": TEXT, "endpoint": format!("localhost:{port}") })).unwrap();
        assert!(r["draft"]["notes"][0].as_str().unwrap().contains("404") && r["draft"]["notes"][0].as_str().unwrap().contains("not found"));
        srv.join().unwrap();
        // A model that is not installed is replaced by the first one, and that is said.
        let (port, srv) = serve(vec![ok(r#"{"models":[{"name":"installed"}]}"#), ok(&chat_answer(&json!({ "building_type": "villa", "floors": 2, "annex": false, "rooms": [] })))]);
        let r = handle("ai_brief", &json!({ "text": TEXT, "endpoint": format!("localhost:{port}"), "model": "missing" })).unwrap();
        assert_eq!(r["model"], "installed");
        assert!(r["draft"]["notes"][0].as_str().unwrap().contains("'missing' is not installed"));
        srv.join().unwrap();
        // Nothing to read.
        assert!(handle("ai_brief", &json!({ "text": "   " })).unwrap_err().contains("describe the project first"));
        assert!(handle("nope", &json!({})).is_err());
    }

    #[test]
    fn a_model_that_never_answers_ends_with_the_keywords_after_the_timeout() {
        let (port, srv) = serve(vec![ok(r#"{"models":[{"name":"slow"}]}"#), None]);
        let started = std::time::Instant::now();
        let r = handle("ai_brief", &json!({ "text": TEXT, "endpoint": format!("localhost:{port}"), "timeout_s": 1 })).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        assert_eq!(r["online"], false);
        assert!(r["draft"]["notes"][0].as_str().unwrap().contains("did not answer within 1 s"), "{}", r["draft"]["notes"][0]);
        assert_eq!(r["draft"]["floors"], 2);
        srv.join().unwrap();
    }

    /// A PNG header for an image of 1000 x 800 px, base64 (the model server is a mock, so the pixels do not matter).
    fn png_b64() -> String {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        png.extend_from_slice(&1000u32.to_be_bytes());
        png.extend_from_slice(&800u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0]);
        nnx_core::xlsx::base64_encode(&png)
    }

    fn sketch_args(port: u16) -> Value {
        json!({ "image_b64": png_b64(), "endpoint": format!("localhost:{port}"), "scale_p1": [0, 0], "scale_p2": [100, 0], "scale_real_mm": 2000, "origin_px": [100, 700] })
    }

    #[test]
    fn a_sketch_is_read_by_the_vision_model_and_comes_back_unconfirmed() {
        let answer = json!({ "rooms": [{ "label": "مجلس", "func": "majlis", "bbox_px": [150, 500, 400, 700] }] });
        let (port, srv) = serve(vec![ok(r#"{"models":[{"name":"llama3:8b"},{"name":"qwen2.5vl:7b"}]}"#), ok(&chat_answer(&answer))]);
        let r = handle("ai_sketch", &sketch_args(port)).unwrap();
        assert_eq!(r["model"], "qwen2.5vl:7b", "the first vision model is chosen, not the first model");
        assert_eq!(r["reading"]["rooms"][0]["w_mm"], 5000);
        assert_eq!(r["reading"]["rooms"][0]["status"], "unconfirmed");
        assert_eq!(r["reading"]["needs_review"], true);
        assert_eq!(r["image"]["width_px"], 1000);
        let seen = srv.join().unwrap();
        assert!(seen[1].contains("\"images\":[\""), "the image is attached to the request");
        assert!(seen[1].contains("1000 x 800 pixels"));
    }

    #[test]
    fn a_sketch_without_a_vision_model_or_a_server_is_an_error_not_a_guess() {
        let (port, srv) = serve(vec![ok(r#"{"models":[{"name":"llama3:8b"}]}"#)]);
        let e = handle("ai_sketch", &sketch_args(port)).unwrap_err();
        assert!(e.contains("no installed model is a vision model") && e.contains("llama3:8b"), "{e}");
        srv.join().unwrap();
        let (port, srv) = serve(vec![ok(r#"{"models":[{"name":"llava"}]}"#)]);
        let mut a = sketch_args(port);
        a["model"] = json!("missing");
        assert!(handle("ai_sketch", &a).unwrap_err().contains("'missing' is not installed"));
        srv.join().unwrap();
        let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let e = handle("ai_sketch", &sketch_args(closed)).unwrap_err();
        assert!(e.contains("not reachable") && e.contains("manual tracing"), "{e}");
    }

    #[test]
    fn a_sketch_needs_the_scale_the_origin_and_an_image_it_can_read() {
        let mut a = sketch_args(1);
        a.as_object_mut().unwrap().remove("scale_real_mm");
        assert!(handle("ai_sketch", &a).unwrap_err().contains("'scale_real_mm' is missing"));
        let mut a = sketch_args(1);
        a.as_object_mut().unwrap().remove("origin_px");
        assert!(handle("ai_sketch", &a).unwrap_err().contains("'origin_px' must be [x, y]"));
        let mut a = sketch_args(1);
        a["scale_p2"] = json!([0, 0]);
        assert!(handle("ai_sketch", &a).unwrap_err().contains("same point"));
        let mut a = sketch_args(1);
        a["image_b64"] = json!(nnx_core::xlsx::base64_encode(b"%PDF-1.7 xxxx"));
        assert!(handle("ai_sketch", &a).unwrap_err().contains("render the page"));
        assert!(handle("ai_sketch", &json!({})).unwrap_err().contains("image's path"));
        let mut a = sketch_args(1);
        a["endpoint"] = json!("http://10.0.0.7:11434");
        assert!(handle("ai_sketch", &a).unwrap_err().contains("only a model server on this computer"));
    }

    #[test]
    fn a_vision_answer_with_no_usable_room_is_reported() {
        let answer = json!({ "rooms": [{ "label": "x", "func": "kitchen", "bbox_px": [900, 500, 1100, 600] }] });
        let (port, srv) = serve(vec![ok(r#"{"models":[{"name":"llava"}]}"#), ok(&chat_answer(&answer))]);
        let e = handle("ai_sketch", &sketch_args(port)).unwrap_err();
        assert!(e.contains("llava") && e.contains("no room could be read"), "{e}");
        srv.join().unwrap();
    }
}
