use nnx_core::share::{Scope, ShareServer};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const PAGE: &str = "<!doctype html><title>t</title><canvas id=\"gl\"></canvas>";

fn request(port: u16, raw: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    String::from_utf8_lossy(&out).into_owned()
}

fn get(port: u16, path: &str) -> String {
    request(port, &format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n"))
}

fn local(life: Duration) -> ShareServer {
    ShareServer::start(PAGE.into(), Scope::Local, 0, life).unwrap()
}

#[test]
fn the_token_path_serves_the_page_and_counts_the_view() {
    let s = local(Duration::from_secs(60));
    let r = get(s.port(), &s.path());
    assert!(r.starts_with("HTTP/1.1 200 OK"), "{r}");
    assert!(r.ends_with(PAGE), "{r}");
    assert!(r.contains("Content-Type: text/html; charset=utf-8") && r.contains("default-src 'none'") && r.contains("Cache-Control: no-store"));
    assert_eq!(s.views(), 1);
    // A query string does not change which page it is.
    assert!(get(s.port(), &format!("{}?x=1", s.path())).starts_with("HTTP/1.1 200"));
    assert_eq!(s.views(), 2);
    assert!(s.url().unwrap().starts_with("http://127.0.0.1:") && s.url().unwrap().ends_with(&s.path()));
}

#[test]
fn anything_but_the_exact_token_is_a_404_with_no_page() {
    let s = local(Duration::from_secs(60));
    let wrong = format!("{}0", s.path());
    for p in ["/", "/t/", "/t/abc", wrong.as_str(), "/../t", "/T/x", "//t/x"] {
        let r = get(s.port(), p);
        assert!(r.starts_with("HTTP/1.1 404"), "{p}: {r}");
        assert!(!r.contains("canvas"), "{p}");
    }
    let short = &s.path()[..s.path().len() - 1];
    assert!(get(s.port(), short).starts_with("HTTP/1.1 404"));
    assert_eq!(s.views(), 0);
}

#[test]
fn only_get_and_head_are_answered_and_head_has_no_body() {
    let s = local(Duration::from_secs(60));
    let post = request(s.port(), &format!("POST {} HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n", s.path()));
    assert!(post.starts_with("HTTP/1.1 405") && post.contains("Allow: GET, HEAD"), "{post}");
    let head = request(s.port(), &format!("HEAD {} HTTP/1.1\r\nHost: x\r\n\r\n", s.path()));
    assert!(head.starts_with("HTTP/1.1 200") && !head.contains("canvas") && head.contains(&format!("Content-Length: {}", PAGE.len())), "{head}");
    assert_eq!(s.views(), 0);
    let big = request(s.port(), &format!("GET /{} HTTP/1.1\r\n", "a".repeat(9000)));
    assert!(big.starts_with("HTTP/1.1 431"), "{big}");
}

#[test]
fn every_share_has_its_own_long_random_token() {
    let a = local(Duration::from_secs(60));
    let b = local(Duration::from_secs(60));
    assert_ne!(a.path(), b.path());
    assert_eq!(a.path().len(), "/t/".len() + 40);
    assert!(a.path()[3..].chars().all(|c| c.is_ascii_hexdigit()));
    // The other share's token does not open this one.
    assert!(get(a.port(), &b.path()).starts_with("HTTP/1.1 404"));
}

#[test]
fn the_link_stops_when_stopped_and_when_the_lifetime_runs_out() {
    let mut s = local(Duration::from_secs(60));
    let port = s.port();
    assert!(s.is_live());
    s.stop();
    assert!(!s.is_live());
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err(), "the listener must be closed after stop");

    let e = local(Duration::from_millis(300));
    let path = e.path();
    assert!(get(e.port(), &path).starts_with("HTTP/1.1 200"));
    std::thread::sleep(Duration::from_millis(600));
    assert!(!e.is_live() && e.seconds_left() == 0);
}

#[test]
fn dropping_the_server_closes_the_port() {
    let port = {
        let s = local(Duration::from_secs(60));
        s.port()
    };
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
}

#[test]
fn a_silent_client_does_not_block_other_visitors() {
    let s = local(Duration::from_secs(60));
    let _idle = TcpStream::connect(("127.0.0.1", s.port())).unwrap();
    assert!(get(s.port(), &s.path()).starts_with("HTTP/1.1 200"));
}

#[test]
fn bad_inputs_are_refused_by_name() {
    let err = |html: &str, life: Duration| ShareServer::start(html.into(), Scope::Local, 0, life).err().unwrap();
    assert!(err("  ", Duration::from_secs(5)).contains("no page"));
    assert!(err(PAGE, Duration::ZERO).contains("lifetime"));
    assert!(err(PAGE, Duration::from_secs(7 * 24 * 3600 + 1)).contains("lifetime"));
    let taken = local(Duration::from_secs(60));
    assert!(ShareServer::start(PAGE.into(), Scope::Local, taken.port(), Duration::from_secs(5)).err().unwrap().contains("cannot listen"));
}

#[test]
fn a_lan_share_listens_on_the_network_and_its_url_names_this_machine() {
    let s = ShareServer::start(PAGE.into(), Scope::Lan, 0, Duration::from_secs(60)).unwrap();
    // Reachable through loopback too (it listens on every interface).
    assert!(get(s.port(), &s.path()).starts_with("HTTP/1.1 200"));
    match s.url() {
        Ok(u) => assert!(u.starts_with("http://") && !u.starts_with("http://127.0.0.1") && u.ends_with(&s.path()), "{u}"),
        // A machine with no network gets a named refusal, never a made-up address.
        Err(e) => assert!(e.contains("no network"), "{e}"),
    }
}
