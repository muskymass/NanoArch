//! Phone preview and site comments on the local network, no cloud (tracker item 96).
//!
//! Two parts:
//! 1. [`SiteReview`], the comments stored in the project (`meta.site_review`, so they follow undo/redo and the `.nnx` file) and the
//!    report over them;
//! 2. [`ReviewServer`], a small HTTP server (std only) that serves ONE responsive page (a plan snapshot, with a web-app manifest so a phone
//!    can add it to the home screen) at an unguessable path `/r/<token>/`, and receives comments from the phone into an in-memory inbox.
//!
//! Nothing reaches the project by itself. The inbox is only a queue: the user reads it and imports chosen comments through the command
//! stack (one undoable edit). Nothing is defaulted or guessed:
//! - a comment needs an author (typed by the person on the phone), its text, and a real `YYYY-MM-DD` date (the phone's own calendar day);
//! - a pin is a tap on the plan; the server converts the tap to plan millimetres with the frame of the plan it served, and refuses a tap
//!   outside the drawing. A tapped element must be one of the elements of the served level or the comment is refused by name;
//! - a comment is resolved only with who resolved it and when.
//!
//! Limits, stated: the link has no login, so anyone on the network who has it can read the plan and add comments until it expires or is
//! stopped (the inbox is capped and every comment is reviewed before it enters the project); traffic is plain HTTP, so use a trusted network;
//! a phone opens the page in its browser, and because plain HTTP on a network address is not a "secure context", browsers will not register
//! an offline service worker for it: the page needs the connection to the machine. The page is a snapshot of the plan taken at start.

use crate::asbuilt::resolve_ref;
use crate::execution::{esc, parse_date};
use crate::model::Model;
use crate::share::{Scope, lan_address, new_token, same};
use crate::structx::schedule_names;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const MAX_LIFETIME: Duration = Duration::from_secs(7 * 24 * 3600);
/// Most comments the inbox holds before it refuses more (the user must import or clear some).
pub const MAX_INBOX: usize = 500;
/// Most comments one network address may send during one run of the server.
pub const MAX_PER_ADDRESS: usize = 100;
pub const MAX_TEXT_CHARS: usize = 2000;
pub const MAX_AUTHOR_CHARS: usize = 80;
const MAX_HEAD: usize = 8192;
const MAX_BODY: usize = 8192;
const MAX_CONNECTIONS: usize = 32;

// ---------------------------------------------------------------------------------------------------------------------------------
// Stored comments
// ---------------------------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SiteComment {
    pub id: String,
    /// `YYYY-MM-DD`, the day the commenter wrote it.
    pub date: String,
    /// Who wrote it, as they typed it.
    pub author: String,
    pub text: String,
    /// The name of the level the plan showed.
    pub level: Option<String>,
    /// The tapped element (an element id), when the tap landed on one.
    pub element: Option<String>,
    /// The pin, in plan millimetres (both or neither).
    pub x_mm: Option<i64>,
    pub y_mm: Option<i64>,
    /// Who closed the comment and on which day (both or neither).
    pub resolved_by: Option<String>,
    pub resolved_on: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SiteReview {
    pub comments: Vec<SiteComment>,
}

impl SiteComment {
    /// What can be checked on one comment alone.
    pub fn check(&self) -> Result<(), String> {
        let id = &self.id;
        if id.trim().is_empty() {
            return Err("a site comment needs an id".into());
        }
        if self.author.trim().is_empty() {
            return Err(format!("site comment '{id}' needs its author"));
        }
        if self.author.chars().count() > MAX_AUTHOR_CHARS {
            return Err(format!("site comment '{id}': the author name is longer than {MAX_AUTHOR_CHARS} characters"));
        }
        if self.text.trim().is_empty() {
            return Err(format!("site comment '{id}' needs its text"));
        }
        if self.text.chars().count() > MAX_TEXT_CHARS {
            return Err(format!("site comment '{id}': the text is longer than {MAX_TEXT_CHARS} characters"));
        }
        let day = parse_date(&format!("site comment '{id}' date"), &self.date)?.to_days();
        if self.x_mm.is_some() != self.y_mm.is_some() {
            return Err(format!("site comment '{id}': a pin needs both x_mm and y_mm"));
        }
        if self.element.as_deref().is_some_and(|e| e.trim().is_empty()) {
            return Err(format!("site comment '{id}' has an empty element name"));
        }
        match (self.resolved_by.as_deref(), self.resolved_on.as_deref()) {
            (None, None) => {}
            (Some(by), Some(on)) => {
                if by.trim().is_empty() {
                    return Err(format!("site comment '{id}': resolved_by is blank (who closed it?)"));
                }
                if parse_date(&format!("site comment '{id}' resolved_on"), on)?.to_days() < day {
                    return Err(format!("site comment '{id}' is resolved on {on}, before it was written ({})", self.date));
                }
            }
            _ => return Err(format!("site comment '{id}': resolved_by and resolved_on go together (who closed it, and when)")),
        }
        Ok(())
    }
}

impl SiteReview {
    pub fn check(&self) -> Result<(), String> {
        let mut ids = BTreeSet::new();
        for c in &self.comments {
            c.check()?;
            if !ids.insert(c.id.as_str()) {
                return Err(format!("the site comment id '{}' is used twice", c.id));
            }
        }
        Ok(())
    }
}

/// Refuses, by comment and name, every element reference that matches nothing in the model.
pub fn validate_links(model: &Model, review: &SiteReview) -> Result<(), String> {
    let names = schedule_names(model, None);
    for c in &review.comments {
        if let Some(e) = c.element.as_deref()
            && resolve_ref(model, &names, e.trim()).is_none()
        {
            return Err(format!("site comment '{}': '{e}' matches no element in the model", c.id));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct CommentRow {
    pub id: String,
    pub date: String,
    pub author: String,
    pub text: String,
    pub level: Option<String>,
    pub x_mm: Option<i64>,
    pub y_mm: Option<i64>,
    /// `open` or `resolved`.
    pub status: String,
    pub resolved_by: Option<String>,
    pub resolved_on: Option<String>,
    pub element_id: Option<String>,
    /// The element reference no longer matches anything (deleted after the comment was recorded).
    pub unresolved: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthorCount {
    pub author: String,
    pub total: usize,
    pub open: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SiteReviewReport {
    /// Oldest first; comments of the same day keep their stored order.
    pub comments: Vec<CommentRow>,
    pub total: usize,
    pub open: usize,
    pub resolved: usize,
    pub by_author: Vec<AuthorCount>,
    pub notes: Vec<String>,
}

pub fn report(model: &Model, review: &SiteReview) -> SiteReviewReport {
    let names = schedule_names(model, None);
    let mut rep = SiteReviewReport::default();
    let mut order: Vec<(i64, usize)> = Vec::new();
    for (i, c) in review.comments.iter().enumerate() {
        match parse_date("site comment date", &c.date) {
            Ok(d) => order.push((d.to_days(), i)),
            Err(e) => rep.notes.push(format!("site comment '{}': {e}", c.id)),
        }
    }
    order.sort();
    let mut by: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (_, i) in &order {
        let c = &review.comments[*i];
        let resolved = c.resolved_by.is_some();
        let (mut element_id, mut unresolved) = (None, None);
        if let Some(e) = c.element.as_deref() {
            match resolve_ref(model, &names, e.trim()) {
                Some(id) => element_id = Some(id.to_string()),
                None => {
                    unresolved = Some(e.to_string());
                    rep.notes.push(format!("site comment '{}': '{e}' no longer matches an element", c.id));
                }
            }
        }
        let n = by.entry(c.author.trim().to_string()).or_default();
        n.0 += 1;
        if !resolved {
            n.1 += 1;
        }
        rep.comments.push(CommentRow {
            id: c.id.clone(),
            date: c.date.clone(),
            author: c.author.clone(),
            text: c.text.clone(),
            level: c.level.clone(),
            x_mm: c.x_mm,
            y_mm: c.y_mm,
            status: if resolved { "resolved".into() } else { "open".into() },
            resolved_by: c.resolved_by.clone(),
            resolved_on: c.resolved_on.clone(),
            element_id,
            unresolved,
        });
    }
    rep.total = rep.comments.len();
    rep.resolved = rep.comments.iter().filter(|r| r.status == "resolved").count();
    rep.open = rep.total - rep.resolved;
    rep.by_author = by.into_iter().map(|(author, (total, open))| AuthorCount { author, total, open }).collect();
    if review.comments.is_empty() {
        rep.notes.push("no site comments have been recorded".into());
    }
    rep
}

impl SiteReviewReport {
    pub fn csv(&self) -> String {
        let cell = |s: &str| if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
        let mut o = String::from("id,date,author,status,level,x_mm,y_mm,element_id,text,resolved_by,resolved_on\n");
        for r in &self.comments {
            let _ = writeln!(
                o,
                "{},{},{},{},{},{},{},{},{},{},{}",
                cell(&r.id),
                r.date,
                cell(&r.author),
                r.status,
                cell(r.level.as_deref().unwrap_or("")),
                r.x_mm.map(|v| v.to_string()).unwrap_or_default(),
                r.y_mm.map(|v| v.to_string()).unwrap_or_default(),
                cell(r.element_id.as_deref().unwrap_or("")),
                cell(&r.text),
                cell(r.resolved_by.as_deref().unwrap_or("")),
                r.resolved_on.as_deref().unwrap_or("")
            );
        }
        o
    }

    pub fn html(&self) -> String {
        let mut o = String::from("<!doctype html><meta charset=\"utf-8\"><title>Site comments</title><h1>Site comments</h1>");
        let _ = write!(o, "<p>{} comments ({} open, {} resolved)</p>", self.total, self.open, self.resolved);
        o.push_str("<table border=\"1\" cellpadding=\"4\"><tr><th>Date</th><th>Author</th><th>Level</th><th>Comment</th><th>Status</th></tr>");
        for r in &self.comments {
            let status = match (&r.resolved_by, &r.resolved_on) {
                (Some(by), Some(on)) => format!("resolved by {} on {}", esc(by), esc(on)),
                _ => "open".to_string(),
            };
            let _ = write!(o, "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", esc(&r.date), esc(&r.author), esc(r.level.as_deref().unwrap_or("")), esc(&r.text), status);
        }
        o.push_str("</table>");
        if !self.by_author.is_empty() {
            o.push_str("<h2>By author</h2><ul>");
            for c in &self.by_author {
                let _ = write!(o, "<li>{}: {} ({} open)</li>", esc(&c.author), c.total, c.open);
            }
            o.push_str("</ul>");
        }
        o
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------------------------------------------------------------------

/// How a tap on the served SVG maps to plan millimetres: `px = margin + (x - min_x) * px_per_mm`, `py = margin + (max_y - y) * px_per_mm`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanFrame {
    pub min_x_mm: f64,
    pub max_y_mm: f64,
    pub px_per_mm: f64,
    pub margin_px: f64,
    /// The drawing's size in SVG user units.
    pub width_px: f64,
    pub height_px: f64,
}

impl PlanFrame {
    /// The plan millimetres under an SVG point, or `None` when the point is off the sheet or the frame is unusable.
    pub fn to_mm(&self, px: f64, py: f64) -> Option<(i64, i64)> {
        if !(self.px_per_mm.is_finite() && self.px_per_mm > 0.0) || !px.is_finite() || !py.is_finite() {
            return None;
        }
        if px < 0.0 || py < 0.0 || px > self.width_px || py > self.height_px {
            return None;
        }
        let x = self.min_x_mm + (px - self.margin_px) / self.px_per_mm;
        let y = self.max_y_mm - (py - self.margin_px) / self.px_per_mm;
        Some((x.round() as i64, y.round() as i64))
    }
}

/// What the server serves. Everything is the user's choice; the plan snapshot is taken by the caller.
#[derive(Debug, Clone)]
pub struct ReviewConfig {
    pub title: String,
    pub level_name: Option<String>,
    pub svg: String,
    pub frame: PlanFrame,
    /// The ids of the elements drawn on the served level: a tapped element must be one of them.
    pub element_ids: BTreeSet<String>,
    /// `true` for an Arabic page first (the page can switch language).
    pub arabic: bool,
}

/// A comment received from a phone, waiting for the user.
#[derive(Debug, Clone, Serialize)]
pub struct Received {
    pub seq: u64,
    pub from: String,
    pub comment: SiteComment,
}

#[derive(Default)]
struct Inbox {
    items: Vec<Received>,
    next: u64,
    per_address: BTreeMap<IpAddr, usize>,
}

struct Shared {
    token: String,
    page: String,
    manifest: String,
    cfg: ReviewConfig,
    inbox: Mutex<Inbox>,
    views: AtomicUsize,
    deadline: Instant,
}

pub struct ReviewServer {
    addr: SocketAddr,
    scope: Scope,
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl ReviewServer {
    /// Start serving. `port` 0 lets the OS choose a free one.
    pub fn start(cfg: ReviewConfig, scope: Scope, port: u16, lifetime: Duration) -> Result<ReviewServer, String> {
        if cfg.svg.trim().is_empty() {
            return Err("site review: there is no plan to show".into());
        }
        if lifetime.is_zero() || lifetime > MAX_LIFETIME {
            return Err(format!("site review: the lifetime must be between 1 second and {} minutes", MAX_LIFETIME.as_secs() / 60));
        }
        let bind: IpAddr = match scope {
            Scope::Local => Ipv4Addr::LOCALHOST.into(),
            Scope::Lan => Ipv4Addr::UNSPECIFIED.into(),
        };
        let listener = TcpListener::bind((bind, port)).map_err(|e| format!("site review: cannot listen on port {port}: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| format!("site review: cannot configure the listener: {e}"))?;
        let addr = listener.local_addr().map_err(|e| format!("site review: cannot read the listening address: {e}"))?;
        let deadline = Instant::now() + lifetime;
        let shared = Arc::new(Shared { token: new_token(), page: page(&cfg), manifest: manifest(&cfg), cfg, inbox: Mutex::new(Inbox::default()), views: AtomicUsize::new(0), deadline });
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicUsize::new(0));
        let worker = {
            let (stop, shared) = (stop.clone(), shared.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) && Instant::now() < shared.deadline {
                    match listener.accept() {
                        Ok((stream, peer)) => {
                            if active.load(Ordering::Acquire) >= MAX_CONNECTIONS {
                                let _ = stream.shutdown(Shutdown::Both);
                                continue;
                            }
                            active.fetch_add(1, Ordering::AcqRel);
                            let (shared, active) = (shared.clone(), active.clone());
                            std::thread::spawn(move || {
                                handle(stream, peer.ip(), &shared);
                                active.fetch_sub(1, Ordering::AcqRel);
                            });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
                        Err(_) => std::thread::sleep(Duration::from_millis(100)),
                    }
                }
            })
        };
        Ok(ReviewServer { addr, scope, stop, shared, worker: Some(worker) })
    }

    pub fn scope(&self) -> Scope {
        self.scope
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn path(&self) -> String {
        format!("/r/{}/", self.shared.token)
    }

    /// The link a phone should open: the loopback address for `Local`, this machine's network address for `Lan`.
    pub fn url(&self) -> Result<String, String> {
        let host = match self.scope {
            Scope::Local => "127.0.0.1".to_string(),
            Scope::Lan => lan_address()?.to_string(),
        };
        Ok(format!("http://{host}:{}{}", self.port(), self.path()))
    }

    pub fn views(&self) -> usize {
        self.shared.views.load(Ordering::Acquire)
    }

    pub fn seconds_left(&self) -> u64 {
        self.shared.deadline.saturating_duration_since(Instant::now()).as_secs()
    }

    pub fn is_live(&self) -> bool {
        !self.stop.load(Ordering::Acquire) && Instant::now() < self.shared.deadline
    }

    /// The comments received and not yet taken, oldest first.
    pub fn inbox(&self) -> Vec<Received> {
        self.shared.inbox.lock().map(|i| i.items.clone()).unwrap_or_default()
    }

    /// The received comments with these sequence numbers (a copy; they stay in the inbox until [`ReviewServer::discard`]).
    pub fn pick(&self, seqs: &[u64]) -> Result<Vec<Received>, String> {
        let inbox = self.shared.inbox.lock().map_err(|_| "site review: the inbox is not available".to_string())?;
        let mut out = Vec::with_capacity(seqs.len());
        let mut seen = BTreeSet::new();
        for s in seqs {
            if !seen.insert(*s) {
                return Err(format!("site review: inbox number {s} is listed twice"));
            }
            out.push(inbox.items.iter().find(|r| r.seq == *s).cloned().ok_or_else(|| format!("site review: inbox number {s} is not in the inbox"))?);
        }
        Ok(out)
    }

    /// Removes received comments (after they were imported, or to drop them). Returns how many were removed.
    pub fn discard(&self, seqs: &[u64]) -> usize {
        let Ok(mut inbox) = self.shared.inbox.lock() else { return 0 };
        let before = inbox.items.len();
        inbox.items.retain(|r| !seqs.contains(&r.seq));
        before - inbox.items.len()
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

impl Drop for ReviewServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// An error in both languages, sent to the phone.
struct Refusal {
    status: &'static str,
    en: String,
    ar: String,
}

fn refuse(status: &'static str, en: &str, ar: &str) -> Refusal {
    Refusal { status, en: en.into(), ar: ar.into() }
}

/// Turns the JSON a phone sent into a comment (without the id), or says which part is wrong.
fn parse_comment(body: &[u8], cfg: &ReviewConfig) -> Result<SiteComment, Refusal> {
    #[derive(Deserialize)]
    struct Body {
        author: Option<String>,
        text: Option<String>,
        date: Option<String>,
        px: Option<f64>,
        py: Option<f64>,
        element: Option<String>,
    }
    let b: Body = serde_json::from_slice(body).map_err(|_| refuse("400 Bad Request", "the comment could not be read", "تعذّرت قراءة التعليق"))?;
    let author = b.author.unwrap_or_default().trim().to_string();
    if author.is_empty() {
        return Err(refuse("422 Unprocessable Entity", "write your name", "اكتب اسمك"));
    }
    if author.chars().count() > MAX_AUTHOR_CHARS {
        return Err(refuse("422 Unprocessable Entity", "the name is too long", "الاسم طويل جداً"));
    }
    let text = b.text.unwrap_or_default().trim().to_string();
    if text.is_empty() {
        return Err(refuse("422 Unprocessable Entity", "write the comment", "اكتب التعليق"));
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(refuse("422 Unprocessable Entity", "the comment is too long", "التعليق طويل جداً"));
    }
    let date = b.date.unwrap_or_default().trim().to_string();
    if parse_date("date", &date).is_err() {
        return Err(refuse("422 Unprocessable Entity", "the date from this device is not valid", "تاريخ الجهاز غير صالح"));
    }
    let (x_mm, y_mm) = match (b.px, b.py) {
        (None, None) => (None, None),
        (Some(px), Some(py)) => match cfg.frame.to_mm(px, py) {
            Some((x, y)) => (Some(x), Some(y)),
            None => return Err(refuse("422 Unprocessable Entity", "the pin is outside the drawing", "الدبوس خارج الرسم")),
        },
        _ => return Err(refuse("422 Unprocessable Entity", "a pin needs both coordinates", "الدبوس يحتاج إحداثيين")),
    };
    let element = b.element.map(|e| e.trim().to_string()).filter(|e| !e.is_empty());
    if let Some(e) = &element
        && !cfg.element_ids.contains(e)
    {
        return Err(refuse("422 Unprocessable Entity", "the tapped element is not on this plan", "العنصر المضغوط ليس في هذه الخريطة"));
    }
    Ok(SiteComment { id: String::new(), date, author, text, level: cfg.level_name.clone(), element, x_mm, y_mm, resolved_by: None, resolved_on: None })
}

fn new_comment_id() -> String {
    let a = uuid::Uuid::now_v7().simple().to_string();
    // The first 12 hex digits are the timestamp, the rest is random.
    format!("sc-{}", &a[20..])
}

fn receive(shared: &Shared, from: IpAddr, body: &[u8]) -> Result<SiteComment, Refusal> {
    let mut comment = parse_comment(body, &shared.cfg)?;
    comment.id = new_comment_id();
    let mut inbox = shared.inbox.lock().map_err(|_| refuse("500 Internal Server Error", "the inbox is not available", "صندوق الوارد غير متاح"))?;
    if inbox.items.len() >= MAX_INBOX {
        return Err(refuse("429 Too Many Requests", "the inbox is full; ask the engineer to clear it", "صندوق الوارد ممتلئ؛ اطلب من المهندس تفريغه"));
    }
    let sent = inbox.per_address.entry(from).or_insert(0);
    if *sent >= MAX_PER_ADDRESS {
        return Err(refuse("429 Too Many Requests", "this device sent too many comments", "أرسل هذا الجهاز تعليقات كثيرة"));
    }
    *sent += 1;
    let seq = inbox.next;
    inbox.next += 1;
    inbox.items.push(Received { seq, from: from.to_string(), comment: comment.clone() });
    Ok(comment)
}

fn send(stream: &mut TcpStream, status: &str, extra: &str, body: &[u8], with_body: bool) {
    let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n{extra}\r\n", body.len());
    let _ = stream.write_all(head.as_bytes());
    if with_body {
        let _ = stream.write_all(body);
    }
    let _ = stream.flush();
    // Close our side first and drain what the client still sends (bounded): closing with unread data would reset the connection.
    let _ = stream.shutdown(Shutdown::Write);
    let mut sink = [0u8; 1024];
    let mut left = 4 * MAX_HEAD;
    while left > 0 {
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(n) => left = left.saturating_sub(n),
        }
    }
}

const PAGE_CSP: &str = "Content-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; manifest-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'\r\n";
const JSON_CSP: &str = "Content-Type: application/json; charset=utf-8\r\nContent-Security-Policy: default-src 'none'\r\n";

fn handle(mut stream: TcpStream, from: IpAddr, shared: &Shared) {
    // The accepted socket may inherit the listener's non-blocking mode on some platforms.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_HEAD + MAX_BODY {
                    return send(&mut stream, "431 Request Header Fields Too Large", "", b"", true);
                }
            }
        }
    };
    if head_end > MAX_HEAD {
        return send(&mut stream, "431 Request Header Fields Too Large", "", b"", true);
    }
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or("").split(' ');
    let (method, target) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
    if method != "GET" && method != "POST" {
        return send(&mut stream, "405 Method Not Allowed", "Allow: GET, POST\r\n", b"", true);
    }
    let path = target.split(['?', '#']).next().unwrap_or("");
    let rest = path.strip_prefix("/r/").unwrap_or("");
    let (given, tail) = rest.split_once('/').unwrap_or((rest, ""));
    if Instant::now() >= shared.deadline || !same(given.as_bytes(), shared.token.as_bytes()) {
        return send(&mut stream, "404 Not Found", "", b"", true);
    }
    match (method, tail) {
        ("GET", "") => {
            shared.views.fetch_add(1, Ordering::AcqRel);
            send(&mut stream, "200 OK", &format!("Content-Type: text/html; charset=utf-8\r\n{PAGE_CSP}"), shared.page.as_bytes(), true);
        }
        ("GET", "manifest.webmanifest") => send(&mut stream, "200 OK", "Content-Type: application/manifest+json; charset=utf-8\r\n", shared.manifest.as_bytes(), true),
        ("GET", "icon.svg") => send(&mut stream, "200 OK", "Content-Type: image/svg+xml\r\n", ICON.as_bytes(), true),
        ("GET", "received.json") => {
            let n = shared.inbox.lock().map(|i| i.items.len()).unwrap_or(0);
            send(&mut stream, "200 OK", JSON_CSP, format!("{{\"received\":{n}}}").as_bytes(), true);
        }
        ("POST", "comment") => {
            // A cross-site form cannot send JSON without a pre-flight, which this server never approves.
            let ctype = head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-type")).map(|(_, v)| v.trim().to_ascii_lowercase()));
            if !ctype.is_some_and(|c| c.starts_with("application/json")) {
                return send(&mut stream, "415 Unsupported Media Type", JSON_CSP, br#"{"error":"send application/json"}"#, true);
            }
            let len = head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.trim().parse::<usize>().ok()));
            let Some(len) = len else { return send(&mut stream, "411 Length Required", JSON_CSP, br#"{"error":"length required"}"#, true) };
            if len > MAX_BODY {
                return send(&mut stream, "413 Payload Too Large", JSON_CSP, br#"{"error":"too large"}"#, true);
            }
            let mut body = buf[head_end + 4..].to_vec();
            while body.len() < len {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => body.extend_from_slice(&chunk[..n]),
                }
            }
            body.truncate(len);
            match receive(shared, from, &body) {
                Ok(c) => send(&mut stream, "201 Created", JSON_CSP, serde_json::json!({ "ok": true, "id": c.id }).to_string().as_bytes(), true),
                Err(r) => send(&mut stream, r.status, JSON_CSP, serde_json::json!({ "ok": false, "error": r.en, "error_ar": r.ar }).to_string().as_bytes(), true),
            }
        }
        _ => send(&mut stream, "404 Not Found", "", b"", true),
    }
}

const ICON: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 64 64\"><rect width=\"64\" height=\"64\" rx=\"12\" fill=\"#1f3a4d\"/><path d=\"M12 50V22l20-10 20 10v28z\" fill=\"none\" stroke=\"#fff\" stroke-width=\"4\" stroke-linejoin=\"round\"/><circle cx=\"32\" cy=\"34\" r=\"7\" fill=\"#f2b705\"/></svg>";

fn manifest(cfg: &ReviewConfig) -> String {
    serde_json::json!({
        "name": cfg.title,
        "short_name": "NanoArch",
        "start_url": "./",
        "scope": "./",
        "display": "standalone",
        "orientation": "any",
        "background_color": "#ffffff",
        "theme_color": "#1f3a4d",
        "lang": if cfg.arabic { "ar" } else { "en" },
        "dir": if cfg.arabic { "rtl" } else { "ltr" },
        "icons": [{ "src": "icon.svg", "sizes": "any", "type": "image/svg+xml", "purpose": "any" }],
    })
    .to_string()
}

/// The responsive page: the plan (pan with scroll, zoom with the buttons), a tap places a pin, and a form sends the comment.
fn page(cfg: &ReviewConfig) -> String {
    let lang = if cfg.arabic { "ar" } else { "en" };
    let dir = if cfg.arabic { "rtl" } else { "ltr" };
    PAGE.replace("__LANG__", lang).replace("__DIR__", dir).replace("__TITLE__", &esc(&cfg.title)).replace("__SVG__", &cfg.svg)
}

const PAGE: &str = r##"<!doctype html>
<html lang="__LANG__" dir="__DIR__"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<meta name="theme-color" content="#1f3a4d">
<link rel="manifest" href="manifest.webmanifest"><link rel="icon" href="icon.svg" type="image/svg+xml">
<title>__TITLE__</title>
<style>
:root{--ink:#16232d;--bg:#f4f6f8;--bar:#1f3a4d;--acc:#f2b705}
*{box-sizing:border-box}
html,body{height:100%;margin:0}
body{display:flex;flex-direction:column;font:16px/1.4 "Segoe UI",Tahoma,Arial,sans-serif;color:var(--ink);background:var(--bg)}
header{display:flex;align-items:center;gap:8px;padding:8px 12px;background:var(--bar);color:#fff}
header h1{flex:1;margin:0;font-size:1rem;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
button{font:inherit;min-height:44px;min-width:44px;padding:0 14px;border:1px solid #9aa7b1;border-radius:8px;background:#fff;color:var(--ink)}
button:hover{background:#e8eef2}
button.primary{background:var(--bar);border-color:var(--bar);color:#fff}
header button{background:transparent;border-color:#fff8;color:#fff}
main{flex:1;display:flex;flex-direction:column;min-height:0}
#sheet{flex:1;overflow:auto;background:#fff;touch-action:pan-x pan-y pinch-zoom;border-bottom:1px solid #c7d0d6}
#sheet svg{display:block;height:auto}
.pin{fill:var(--acc);stroke:#7a5c00;stroke-width:2}
.sent{fill:#2a9d8f;stroke:#14524b;stroke-width:2}
#panel{padding:10px 12px;max-height:46vh;overflow:auto}
label{display:block;margin:6px 0 2px;font-size:.9rem}
input,textarea{width:100%;font:inherit;padding:10px;border:1px solid #9aa7b1;border-radius:8px}
textarea{min-height:84px;resize:vertical}
.row{display:flex;gap:8px;align-items:center;margin-top:8px;flex-wrap:wrap}
#msg{min-height:1.4em;margin:6px 0 0;font-size:.95rem}
#msg.err{color:#b3261e}#msg.ok{color:#14684f}
ul{margin:8px 0 0;padding-inline-start:20px}
@media (min-width:900px){main{flex-direction:row}#sheet{border-bottom:0;border-inline-end:1px solid #c7d0d6}#panel{width:380px;max-height:none}}
@media (prefers-color-scheme:dark){:root{--ink:#e8edf1;--bg:#10181e}input,textarea,button{background:#1b2730;color:var(--ink);border-color:#51636f}#sheet{background:#fff}}
</style></head><body>
<header><h1 id="ttl">__TITLE__</h1><button id="zout" aria-label="-">−</button><button id="zin" aria-label="+">+</button><button id="lang">EN</button></header>
<main>
<div id="sheet">__SVG__</div>
<section id="panel">
<p id="hint"></p>
<label for="who" id="lwho"></label><input id="who" maxlength="80" autocomplete="name">
<label for="txt" id="ltxt"></label><textarea id="txt" maxlength="2000"></textarea>
<div class="row"><button class="primary" id="send"></button><button id="clear"></button></div>
<p id="msg" role="status"></p>
<ul id="mine"></ul>
</section></main>
<script>
(function(){
var T={ar:{hint:"اضغط على المخطط لوضع دبوس ثم اكتب تعليقك. يمكنك الإرسال بلا دبوس.",who:"الاسم",txt:"التعليق",send:"إرسال",clear:"إزالة الدبوس",pin:"دبوس موضوع",nopin:"بلا دبوس",sent:"أُرسل. سيراجعه المهندس قبل إضافته للمشروع.",fail:"تعذّر الإرسال. تحقق من الاتصال بالجهاز.",lang:"EN",dir:"rtl"},
en:{hint:"Tap the plan to drop a pin, then write your comment. You can also send without a pin.",who:"Name",txt:"Comment",send:"Send",clear:"Remove pin",pin:"pin placed",nopin:"no pin",sent:"Sent. The engineer reviews it before it enters the project.",fail:"Could not send. Check the connection to the computer.",lang:"ع",dir:"ltr"}};
var L=document.documentElement.lang==="en"?"en":"ar";
var $=function(i){return document.getElementById(i)};
var svg=document.querySelector("#sheet svg"),pin=null,zoom=1,NS="http://www.w3.org/2000/svg";
function apply(){var t=T[L];document.documentElement.lang=L;document.documentElement.dir=t.dir;
$("hint").textContent=t.hint;$("lwho").textContent=t.who;$("ltxt").textContent=t.txt;$("send").textContent=t.send;$("clear").textContent=t.clear;$("lang").textContent=t.lang}
function fit(){if(!svg)return;svg.removeAttribute("width");svg.removeAttribute("height");svg.style.width=(zoom*100)+"%"}
fit();apply();
try{$("who").value=localStorage.getItem("na_who")||""}catch(e){}
$("lang").onclick=function(){L=L==="ar"?"en":"ar";apply()};
$("zin").onclick=function(){zoom=Math.min(zoom*1.4,8);fit()};
$("zout").onclick=function(){zoom=Math.max(zoom/1.4,1);fit()};
function mark(x,y,cls){var c=document.createElementNS(NS,"circle");c.setAttribute("cx",x);c.setAttribute("cy",y);c.setAttribute("r",9);c.setAttribute("class",cls);svg.appendChild(c);return c}
if(svg){svg.addEventListener("click",function(ev){
var pt=svg.createSVGPoint();pt.x=ev.clientX;pt.y=ev.clientY;var m=svg.getScreenCTM();if(!m)return;var p=pt.matrixTransform(m.inverse());
var host=ev.target.closest&&ev.target.closest("[data-id]");
if(pin&&pin.node)svg.removeChild(pin.node);
pin={px:p.x,py:p.y,element:host?host.getAttribute("data-id"):null,node:mark(p.x,p.y,"pin")};$("msg").textContent=T[L].pin;$("msg").className=""})}
$("clear").onclick=function(){if(pin&&pin.node)svg.removeChild(pin.node);pin=null;$("msg").textContent=T[L].nopin;$("msg").className=""};
function today(){var d=new Date();function z(n){return(n<10?"0":"")+n}return d.getFullYear()+"-"+z(d.getMonth()+1)+"-"+z(d.getDate())}
$("send").onclick=function(){
var body={author:$("who").value,text:$("txt").value,date:today()};
if(pin){body.px=pin.px;body.py=pin.py;if(pin.element)body.element=pin.element}
$("send").disabled=true;
fetch("comment",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(body)})
.then(function(r){return r.json().then(function(j){return{ok:r.ok,j:j}})})
.then(function(x){var m=$("msg");
if(x.ok&&x.j.ok){m.textContent=T[L].sent;m.className="ok";try{localStorage.setItem("na_who",$("who").value)}catch(e){}
var li=document.createElement("li");li.textContent=body.text;$("mine").appendChild(li);
if(pin&&pin.node){pin.node.setAttribute("class","sent");pin=null}$("txt").value=""}
else{m.textContent=(L==="ar"?x.j.error_ar:x.j.error)||T[L].fail;m.className="err"}})
.catch(function(){$("msg").textContent=T[L].fail;$("msg").className="err"})
.then(function(){$("send").disabled=false})};
})();
</script></body></html>"##;
