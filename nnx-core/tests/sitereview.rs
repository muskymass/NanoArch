//! Phone preview and site comments (tracker item 96). Every value is SYNTHETIC test data; the server runs on loopback.

use nnx_core::share::Scope;
use nnx_core::sitereview::{PlanFrame, ReviewConfig, ReviewServer, SiteComment, SiteReview, report, validate_links};
use nnx_core::typed::{Column, element_of};
use nnx_core::*;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

fn model() -> (Model, String) {
    let col = element_of(&Column { x: 1000, y: 0, b: 300, h: 300, grid_ref: "A1".into(), axial_service_kn: None, extra: Default::default() }, Source::User);
    let id = col.id.to_string();
    let mut doc = Document::default();
    doc.commit("m", Origin::User, vec![Command::Create(col)]).unwrap_or_else(|e| panic!("{e}"));
    (doc.model, id)
}

fn comment(id: &str, date: &str, by: &str) -> SiteComment {
    SiteComment { id: id.into(), date: date.into(), author: by.into(), text: format!("text {id}"), ..Default::default() }
}

#[test]
fn a_comment_without_author_text_date_or_with_half_a_pin_or_resolution_is_refused_by_name() {
    let ok = comment("a", "2026-03-01", "Site engineer");
    for (bad, msg) in [
        (SiteComment { id: " ".into(), ..ok.clone() }, "needs an id"),
        (SiteComment { author: " ".into(), ..ok.clone() }, "its author"),
        (SiteComment { text: "".into(), ..ok.clone() }, "its text"),
        (SiteComment { text: "x".repeat(2001), ..ok.clone() }, "longer than 2000"),
        (SiteComment { author: "x".repeat(81), ..ok.clone() }, "longer than 80"),
        (SiteComment { date: "2026-02-30".into(), ..ok.clone() }, "not a date"),
        (SiteComment { x_mm: Some(1), ..ok.clone() }, "both x_mm and y_mm"),
        (SiteComment { element: Some(" ".into()), ..ok.clone() }, "empty element"),
        (SiteComment { resolved_by: Some("Ali".into()), ..ok.clone() }, "go together"),
        (SiteComment { resolved_on: Some("2026-03-02".into()), ..ok.clone() }, "go together"),
        (SiteComment { resolved_by: Some(" ".into()), resolved_on: Some("2026-03-02".into()), ..ok.clone() }, "who closed it"),
        (SiteComment { resolved_by: Some("Ali".into()), resolved_on: Some("2026-02-28".into()), ..ok.clone() }, "before it was written"),
    ] {
        let e = SiteReview { comments: vec![bad] }.check().unwrap_err();
        assert!(e.contains(msg), "{e}");
    }
    let dup = SiteReview { comments: vec![ok.clone(), ok.clone()] }.check().unwrap_err();
    assert!(dup.contains("used twice"), "{dup}");
    let closed = SiteComment { id: "b".into(), resolved_by: Some("Ali".into()), resolved_on: Some("2026-03-01".into()), x_mm: Some(5), y_mm: Some(6), ..ok.clone() };
    assert!(SiteReview { comments: vec![ok, closed] }.check().is_ok());
    assert!(SiteReview::default().check().is_ok());
}

#[test]
fn element_links_must_exist_and_a_deleted_element_is_reported_not_dropped() {
    let (m, id) = model();
    let linked = SiteComment { element: Some(id.clone()), ..comment("a", "2026-03-01", "Ali") };
    assert!(validate_links(&m, &SiteReview { comments: vec![linked.clone()] }).is_ok());
    let by_name = SiteComment { element: Some("A1".into()), ..comment("b", "2026-03-02", "Sara") };
    let bad = SiteComment { element: Some("NOPE-9".into()), ..comment("c", "2026-03-01", "Ali") };
    assert!(validate_links(&m, &SiteReview { comments: vec![bad.clone()] }).unwrap_err().contains("NOPE-9"));
    let rep = report(&m, &SiteReview { comments: vec![linked, by_name, bad] });
    assert!(rep.comments.iter().filter(|r| r.element_id.is_some()).count() >= 1);
    assert_eq!(rep.comments.iter().filter(|r| r.unresolved.as_deref() == Some("NOPE-9")).count(), 1);
    assert!(rep.notes.iter().any(|n| n.contains("NOPE-9")));
}

#[test]
fn the_report_orders_by_date_counts_open_and_resolved_and_writes_csv_and_html() {
    let (m, _) = model();
    let closed = SiteComment { resolved_by: Some("Owner".into()), resolved_on: Some("2026-03-05".into()), text: "a, \"quoted\" <b>".into(), ..comment("b", "2026-03-02", "Sara") };
    let rep = report(&m, &SiteReview { comments: vec![comment("a", "2026-03-09", "Ali"), closed, comment("c", "2026-03-02", "Ali")] });
    assert_eq!(rep.comments.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["b", "c", "a"]);
    assert_eq!((rep.total, rep.open, rep.resolved), (3, 2, 1));
    let ali = rep.by_author.iter().find(|a| a.author == "Ali").unwrap();
    assert_eq!((ali.total, ali.open), (2, 2));
    let csv = rep.csv();
    assert!(csv.starts_with("id,date,author,status,"));
    assert!(csv.contains("\"a, \"\"quoted\"\" <b>\""), "{csv}");
    let html = rep.html();
    assert!(html.contains("a, &quot;quoted&quot; &lt;b&gt;") && html.contains("resolved by Owner on 2026-03-05"), "{html}");
    assert!(report(&m, &SiteReview::default()).notes.iter().any(|n| n.contains("no site comments")));
}

#[test]
fn a_tap_maps_to_plan_millimetres_and_a_tap_off_the_sheet_is_refused() {
    let f = PlanFrame { min_x_mm: 1000.0, max_y_mm: 9000.0, px_per_mm: 0.05, margin_px: 90.0, width_px: 590.0, height_px: 590.0 };
    // The top-left corner of the drawing area is (min x, max y).
    assert_eq!(f.to_mm(90.0, 90.0), Some((1000, 9000)));
    // 100 px right and 100 px down is 2000 mm east and 2000 mm south at 0.05 px/mm.
    assert_eq!(f.to_mm(190.0, 190.0), Some((3000, 7000)));
    assert_eq!(f.to_mm(-1.0, 10.0), None);
    assert_eq!(f.to_mm(10.0, 591.0), None);
    assert_eq!(f.to_mm(f64::NAN, 10.0), None);
    assert_eq!(PlanFrame { px_per_mm: 0.0, ..f }.to_mm(10.0, 10.0), None);
}

fn config(ids: &[&str]) -> ReviewConfig {
    ReviewConfig {
        title: "Villa <test>".into(),
        level_name: Some("Ground".into()),
        svg: "<svg viewBox=\"0 0 590 590\"><rect data-id=\"e1\" width=\"50\" height=\"50\"/></svg>".into(),
        frame: PlanFrame { min_x_mm: 0.0, max_y_mm: 10000.0, px_per_mm: 0.05, margin_px: 90.0, width_px: 590.0, height_px: 590.0 },
        element_ids: ids.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
        arabic: true,
    }
}

/// One raw HTTP exchange; returns the status line and the whole response text.
fn http(port: u16, request: &str) -> (String, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).into_owned();
    (text.lines().next().unwrap_or("").to_string(), text)
}

fn post(port: u16, path: &str, ctype: &str, body: &str) -> (String, String) {
    http(port, &format!("POST {path}comment HTTP/1.1\r\nHost: x\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\r\n{body}", body.len()))
}

#[test]
fn the_server_serves_the_page_receives_comments_and_refuses_bad_ones() {
    let srv = ReviewServer::start(config(&["e1"]), Scope::Local, 0, Duration::from_secs(60)).unwrap();
    let (port, path) = (srv.port(), srv.path());
    assert!(srv.url().unwrap().starts_with("http://127.0.0.1:"));

    // The page is the plan snapshot with the title escaped and no network reference.
    let (st, page) = http(port, &format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n"));
    assert_eq!(st, "HTTP/1.1 200 OK");
    assert!(page.contains("data-id=\"e1\"") && page.contains("Villa &lt;test&gt;") && page.contains("manifest.webmanifest") && page.contains("name=\"viewport\""));
    assert!(page.contains("connect-src 'self'") && page.contains("default-src 'none'") && !page.contains("https://"));
    assert_eq!(srv.views(), 1);
    let (st, man) = http(port, &format!("GET {path}manifest.webmanifest HTTP/1.1\r\n\r\n"));
    assert_eq!(st, "HTTP/1.1 200 OK");
    assert!(man.contains("\"display\":\"standalone\"") && man.contains("\"dir\":\"rtl\""), "{man}");
    assert!(http(port, &format!("GET {path}icon.svg HTTP/1.1\r\n\r\n")).1.contains("<svg"));

    // Wrong token, other paths and other methods say nothing.
    assert_eq!(http(port, "GET /r/wrong/ HTTP/1.1\r\n\r\n").0, "HTTP/1.1 404 Not Found");
    assert_eq!(http(port, "GET / HTTP/1.1\r\n\r\n").0, "HTTP/1.1 404 Not Found");
    assert_eq!(http(port, &format!("GET {path}nothing HTTP/1.1\r\n\r\n")).0, "HTTP/1.1 404 Not Found");
    assert_eq!(http(port, &format!("DELETE {path} HTTP/1.1\r\n\r\n")).0, "HTTP/1.1 405 Method Not Allowed");
    assert_eq!(post(port, "/r/wrong/", "application/json", "{}").0, "HTTP/1.1 404 Not Found");

    // A good comment with a pin on an element.
    let good = r#"{"author":" Sara ","text":"crack in the wall","date":"2026-03-01","px":190,"py":190,"element":"e1"}"#;
    let (st, body) = post(port, &path, "application/json; charset=utf-8", good);
    assert_eq!(st, "HTTP/1.1 201 Created", "{body}");
    assert!(body.contains("\"ok\":true"));
    let inbox = srv.inbox();
    assert_eq!(inbox.len(), 1);
    let c = &inbox[0].comment;
    assert_eq!((c.author.as_str(), c.level.as_deref(), c.element.as_deref(), c.x_mm, c.y_mm), ("Sara", Some("Ground"), Some("e1"), Some(2000), Some(8000)));
    assert!(c.id.starts_with("sc-") && c.check().is_ok(), "an inbox comment is a valid stored comment");
    assert!(http(port, &format!("GET {path}received.json HTTP/1.1\r\n\r\n")).1.contains("\"received\":1"));

    // A comment without a pin is fine.
    assert_eq!(post(port, &path, "application/json", r#"{"author":"Ali","text":"ok","date":"2026-03-02"}"#).0, "HTTP/1.1 201 Created");

    // Every refusal names the problem in both languages and stores nothing.
    let before = srv.inbox().len();
    for (body, en) in [
        (r#"{"text":"x","date":"2026-03-01"}"#, "write your name"),
        (r#"{"author":"A","text":"  ","date":"2026-03-01"}"#, "write the comment"),
        (r#"{"author":"A","text":"x","date":"2026-02-30"}"#, "date from this device"),
        (r#"{"author":"A","text":"x","date":"2026-03-01","px":9999,"py":10}"#, "outside the drawing"),
        (r#"{"author":"A","text":"x","date":"2026-03-01","px":100}"#, "both coordinates"),
        (r#"{"author":"A","text":"x","date":"2026-03-01","element":"zzz"}"#, "not on this plan"),
        (r#"not json"#, "could not be read"),
    ] {
        let (st, resp) = post(port, &path, "application/json", body);
        assert!(st.starts_with("HTTP/1.1 4"), "{body}: {st}");
        assert!(resp.contains(en) && resp.contains("error_ar"), "{body}: {resp}");
    }
    assert_eq!(post(port, &path, "text/plain", good).0, "HTTP/1.1 415 Unsupported Media Type");
    assert_eq!(post(port, &path, "application/x-www-form-urlencoded", "a=b").0, "HTTP/1.1 415 Unsupported Media Type");
    assert_eq!(http(port, &format!("POST {path}comment HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{{}}")).0, "HTTP/1.1 411 Length Required");
    let huge = format!(r#"{{"author":"A","text":"{}","date":"2026-03-01"}}"#, "x".repeat(9000));
    assert_eq!(post(port, &path, "application/json", &huge).0, "HTTP/1.1 413 Payload Too Large");
    assert_eq!(srv.inbox().len(), before);
}

#[test]
fn the_inbox_is_picked_by_number_discarded_after_import_and_capped_per_address() {
    let srv = ReviewServer::start(config(&[]), Scope::Local, 0, Duration::from_secs(60)).unwrap();
    let (port, path) = (srv.port(), srv.path());
    for i in 0..3 {
        let body = format!(r#"{{"author":"A","text":"n{i}","date":"2026-03-01"}}"#);
        assert_eq!(post(port, &path, "application/json", &body).0, "HTTP/1.1 201 Created");
    }
    let all = srv.inbox();
    assert_eq!(all.iter().map(|r| r.seq).collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(srv.pick(&[2, 0]).unwrap().iter().map(|r| r.comment.text.as_str()).collect::<Vec<_>>(), ["n2", "n0"]);
    assert!(srv.pick(&[7]).unwrap_err().contains("not in the inbox"));
    assert!(srv.pick(&[1, 1]).unwrap_err().contains("twice"));
    assert_eq!(srv.discard(&[0, 2, 9]), 2);
    assert_eq!(srv.inbox().iter().map(|r| r.seq).collect::<Vec<_>>(), [1]);

    // One address may send at most 100 comments during a run (the three above count): the 101st is refused even after a discard.
    let mut last = String::new();
    for i in 3..100 {
        last = post(port, &path, "application/json", &format!(r#"{{"author":"A","text":"n{i}","date":"2026-03-01"}}"#)).0;
    }
    assert_eq!(last, "HTTP/1.1 201 Created");
    let (st, resp) = post(port, &path, "application/json", r#"{"author":"A","text":"one more","date":"2026-03-01"}"#);
    assert_eq!(st, "HTTP/1.1 429 Too Many Requests", "{resp}");
    assert_eq!(srv.inbox().len(), 1 + 97);
}

#[test]
fn the_link_stops_when_stopped_or_expired_and_the_lifetime_is_bounded() {
    assert!(ReviewServer::start(config(&[]), Scope::Local, 0, Duration::ZERO).err().unwrap().contains("lifetime"));
    assert!(ReviewServer::start(config(&[]), Scope::Local, 0, Duration::from_secs(8 * 24 * 3600)).err().unwrap().contains("lifetime"));
    let empty = ReviewConfig { svg: " ".into(), ..config(&[]) };
    assert!(ReviewServer::start(empty, Scope::Local, 0, Duration::from_secs(5)).err().unwrap().contains("no plan"));

    let mut srv = ReviewServer::start(config(&[]), Scope::Local, 0, Duration::from_secs(60)).unwrap();
    let (port, path) = (srv.port(), srv.path());
    assert!(srv.is_live());
    srv.stop();
    assert!(!srv.is_live());
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err() || http(port, &format!("GET {path} HTTP/1.1\r\n\r\n")).1.is_empty());

    let short = ReviewServer::start(config(&[]), Scope::Local, 0, Duration::from_millis(300)).unwrap();
    let (port, path) = (short.port(), short.path());
    std::thread::sleep(Duration::from_millis(600));
    assert!(!short.is_live() && short.seconds_left() == 0);
    // Once expired the listener is gone (refused) or, if a late connection is accepted, it is never served the page.
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err() || http(port, &format!("GET {path} HTTP/1.1\r\n\r\n")).0 != "HTTP/1.1 200 OK");
}
