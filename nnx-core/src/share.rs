//! A shareable link to a tour page without any cloud: a tiny HTTP server (std only) that serves ONE page, the snapshot taken when sharing
//! started, at an unguessable path `/t/<token>`. `Scope::Local` listens on loopback only (this machine); `Scope::Lan` listens on every
//! interface so a phone or a client's laptop on the same network can open the link. Nothing leaves the network: there is no relay, no
//! account and no third party.
//!
//! The link stops working when the owner stops it, when the lifetime the owner entered runs out, or when the `ShareServer` is dropped.
//! Every other path, method or token answers 404 / 405 with no detail.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Longest lifetime a link may be given (seven days). Longer sharing is not offered: restart it deliberately.
pub const MAX_LIFETIME: Duration = Duration::from_secs(7 * 24 * 3600);
const MAX_REQUEST: usize = 8192;
const MAX_CONNECTIONS: usize = 32;

/// Who can reach the link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Loopback only: this machine.
    Local,
    /// Every network interface: devices on the same network can open it.
    Lan,
}

/// A running share. Dropping it (or `stop`) closes the listener.
pub struct ShareServer {
    addr: SocketAddr,
    scope: Scope,
    token: String,
    stop: Arc<AtomicBool>,
    views: Arc<AtomicUsize>,
    deadline: Instant,
    worker: Option<JoinHandle<()>>,
}

/// A 148-bit random path token (two UUIDv7 values: the random parts of both are used).
fn new_token() -> String {
    let a = uuid::Uuid::now_v7().simple().to_string();
    let b = uuid::Uuid::now_v7().simple().to_string();
    // The first 12 hex digits of a v7 UUID are the timestamp: they carry no secrecy and are dropped.
    format!("{}{}", &a[12..], &b[12..])
}

/// Constant-time comparison so the token cannot be recovered by timing.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// The address other devices on the network can use to reach this machine. A UDP "connect" only asks the OS which interface would route
/// to the address; no packet is sent (192.0.2.1 is the documentation range, TEST-NET-1).
pub fn lan_address() -> Result<IpAddr, String> {
    let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(|e| format!("share: cannot inspect the network: {e}"))?;
    probe.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).map_err(|_| "share: this machine has no network connection, so no link for other devices can be made (use scope local)".to_string())?;
    let ip = probe.local_addr().map_err(|e| format!("share: cannot read the local address: {e}"))?.ip();
    if ip.is_unspecified() || ip.is_loopback() {
        return Err("share: this machine has no network address other than loopback (use scope local)".into());
    }
    Ok(ip)
}

impl ShareServer {
    /// Start serving `html` for `lifetime`. `port` 0 lets the OS choose a free one.
    pub fn start(html: String, scope: Scope, port: u16, lifetime: Duration) -> Result<ShareServer, String> {
        if html.trim().is_empty() {
            return Err("share: there is no page to share".into());
        }
        if lifetime.is_zero() || lifetime > MAX_LIFETIME {
            return Err(format!("share: the lifetime must be between 1 second and {} minutes", MAX_LIFETIME.as_secs() / 60));
        }
        let bind: IpAddr = match scope {
            Scope::Local => Ipv4Addr::LOCALHOST.into(),
            Scope::Lan => Ipv4Addr::UNSPECIFIED.into(),
        };
        let listener = TcpListener::bind((bind, port)).map_err(|e| format!("share: cannot listen on port {port}: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| format!("share: cannot configure the listener: {e}"))?;
        let addr = listener.local_addr().map_err(|e| format!("share: cannot read the listening address: {e}"))?;
        let token = new_token();
        let stop = Arc::new(AtomicBool::new(false));
        let views = Arc::new(AtomicUsize::new(0));
        let deadline = Instant::now() + lifetime;
        let page = Arc::new(html);
        let active = Arc::new(AtomicUsize::new(0));
        let worker = {
            let (stop, views, token, page) = (stop.clone(), views.clone(), token.clone(), page.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) && Instant::now() < deadline {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if active.load(Ordering::Acquire) >= MAX_CONNECTIONS {
                                let _ = stream.shutdown(Shutdown::Both);
                                continue;
                            }
                            active.fetch_add(1, Ordering::AcqRel);
                            let (token, page, views, active) = (token.clone(), page.clone(), views.clone(), active.clone());
                            std::thread::spawn(move || {
                                handle(stream, &token, &page, &views, deadline);
                                active.fetch_sub(1, Ordering::AcqRel);
                            });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
                        Err(_) => std::thread::sleep(Duration::from_millis(100)),
                    }
                }
            })
        };
        Ok(ShareServer { addr, scope, token, stop, views, deadline, worker: Some(worker) })
    }

    pub fn scope(&self) -> Scope {
        self.scope
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// The path part of the link (`/t/<token>`).
    pub fn path(&self) -> String {
        format!("/t/{}", self.token)
    }

    /// The link as other devices should use it: the loopback address for `Local`, this machine's network address for `Lan`.
    pub fn url(&self) -> Result<String, String> {
        let host = match self.scope {
            Scope::Local => "127.0.0.1".to_string(),
            Scope::Lan => lan_address()?.to_string(),
        };
        Ok(format!("http://{host}:{}{}", self.port(), self.path()))
    }

    /// How many times the page has been served.
    pub fn views(&self) -> usize {
        self.views.load(Ordering::Acquire)
    }

    /// Whole seconds left before the link expires (0 once it has).
    pub fn seconds_left(&self) -> u64 {
        self.deadline.saturating_duration_since(Instant::now()).as_secs()
    }

    /// True while the link still answers.
    pub fn is_live(&self) -> bool {
        !self.stop.load(Ordering::Acquire) && Instant::now() < self.deadline
    }

    /// Close the listener and wait for the accept loop to end.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

impl Drop for ShareServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn reply(stream: &mut TcpStream, status: &str, extra: &str, body: &[u8], with_body: bool) {
    let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n{extra}\r\n", body.len());
    let _ = stream.write_all(head.as_bytes());
    if with_body {
        let _ = stream.write_all(body);
    }
    let _ = stream.flush();
    // Close our side first, then drain what the client is still sending (bounded): closing with unread data would reset the connection
    // and the client could lose the answer.
    let _ = stream.shutdown(Shutdown::Write);
    let mut sink = [0u8; 1024];
    let mut left = 4 * MAX_REQUEST;
    while left > 0 {
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(n) => left = left.saturating_sub(n),
        }
    }
}

fn handle(mut stream: TcpStream, token: &str, page: &str, views: &AtomicUsize, deadline: Instant) {
    // The accepted socket may inherit the listener's non-blocking mode on some platforms.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_REQUEST {
                    return reply(&mut stream, "431 Request Header Fields Too Large", "", b"", true);
                }
            }
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let mut parts = text.lines().next().unwrap_or("").split(' ');
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if method != "GET" && method != "HEAD" {
        return reply(&mut stream, "405 Method Not Allowed", "Allow: GET, HEAD\r\n", b"", true);
    }
    let path = target.split(['?', '#']).next().unwrap_or("");
    let given = path.strip_prefix("/t/").unwrap_or("");
    if Instant::now() >= deadline || !same(given.as_bytes(), token.as_bytes()) {
        return reply(&mut stream, "404 Not Found", "", b"", true);
    }
    if method == "GET" {
        views.fetch_add(1, Ordering::AcqRel);
    }
    // The tour is one self-contained page: inline script and style, WebGL, a PNG download from a data URL. Nothing may be fetched.
    let extra = "Content-Type: text/html; charset=utf-8\r\nContent-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; connect-src 'none'\r\n";
    reply(&mut stream, "200 OK", extra, page.as_bytes(), method == "GET");
}
