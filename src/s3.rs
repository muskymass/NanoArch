//! Upload and download of an ENCRYPTED backup to an S3-compatible store the user names (AWS S3, MinIO, R2, B2, Wasabi...).
//!
//! - **Only sealed files travel.** The caller passes the path of a `.nnxbak` file; the bytes are checked to be a NanoArch backup
//!   (magic header) before anything leaves the computer, so a plain project can never be uploaded by mistake.
//! - **Nothing is assumed.** Endpoint, bucket, key, region and both credentials come from the user for that one call. The
//!   credentials are signed into the request (SigV4, `nnx_core::s3sign`) and are neither stored nor returned.
//! - **Transport.** `https` to any host; plain `http` only to this computer or an address on a private network, since a
//!   signed request over the open internet without TLS would expose the object name and the account's request pattern.
//! - Path-style URLs (`endpoint/bucket/key`), which every S3-compatible server accepts.

use nnx_core::backup::MAGIC;
use nnx_core::s3sign::{self, Request};
use serde_json::{Value, json};
use std::net::IpAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Largest object this client will download into memory.
const MAX_DOWNLOAD: u64 = 1 << 30;
const TIMEOUT: Duration = Duration::from_secs(600);

pub struct Target {
    pub https: bool,
    /// Host header value (`host` or `host:port`, default ports dropped).
    pub host: String,
    pub bucket: String,
    pub key: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
}

/// Debug output never contains the credentials.
impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target").field("host", &self.host).field("bucket", &self.bucket).field("key", &self.key).field("access_key", &s3sign::mask(&self.access_key)).field("secret_key", &"****").finish()
    }
}

fn text<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    let v = args.get(name).and_then(Value::as_str).map(str::trim).unwrap_or("");
    if v.is_empty() { Err(format!("{name} is required (no default is assumed)")) } else { Ok(v) }
}

fn is_private(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.trim_matches(|c| c == '[' || c == ']').parse::<IpAddr>() {
        Ok(IpAddr::V4(a)) => a.is_loopback() || a.is_private() || a.is_link_local(),
        Ok(IpAddr::V6(a)) => a.is_loopback() || (a.segments()[0] & 0xfe00) == 0xfc00,
        Err(_) => false,
    }
}

/// Reads and checks the target fields of a backup command.
pub fn parse_target(args: &Value) -> Result<Target, String> {
    let endpoint = text(args, "endpoint")?;
    let (https, rest) = match endpoint.split_once("://") {
        Some(("https", r)) => (true, r),
        Some(("http", r)) => (false, r),
        Some((s, _)) => return Err(format!("endpoint scheme must be http or https, not '{s}'")),
        None => return Err("endpoint must start with https:// (or http:// for this computer or a private network)".into()),
    };
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() || rest.contains('/') || rest.contains('?') || rest.contains('@') {
        return Err("endpoint must be just scheme://host[:port] (the bucket and key are separate fields)".into());
    }
    let (hostname, port) = match rest.rsplit_once(':') {
        Some((h, p)) if !rest.ends_with(']') && !h.is_empty() => (h, Some(p.parse::<u16>().map_err(|_| format!("'{p}' is not a port number"))?)),
        _ => (rest, None),
    };
    if !https && !is_private(hostname) {
        return Err(format!("plain http is only allowed for this computer or a private network address, not '{hostname}': use https://"));
    }
    let default = if https { 443 } else { 80 };
    let host = match port {
        Some(p) if p != default => format!("{hostname}:{p}"),
        _ => hostname.to_string(),
    };
    let bucket = text(args, "bucket")?;
    let valid_bucket = (3..=63).contains(&bucket.len()) && bucket.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.');
    if !valid_bucket {
        return Err(format!("bucket '{bucket}' must be 3-63 characters of lower-case letters, digits, '-' and '.'"));
    }
    let key = text(args, "key")?;
    s3sign::check_key(key).map_err(|e| e.to_string())?;
    Ok(Target {
        https,
        host,
        bucket: bucket.to_string(),
        key: key.to_string(),
        region: text(args, "region")?.to_string(),
        access_key: text(args, "access_key")?.to_string(),
        secret_key: text(args, "secret_key")?.to_string(),
    })
}

/// Current UTC time as `YYYYMMDDTHHMMSSZ`.
pub fn amz_now() -> Result<String, String> {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| "the system clock is before 1970")?.as_secs();
    Ok(amz_stamp(secs))
}

pub fn amz_stamp(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil-from-days (H. Hinnant), proleptic Gregorian.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn call(t: &Target, method: &str, body: Option<&[u8]>, stamp: &str) -> Result<(u16, Vec<u8>), String> {
    let path = format!("/{}/{}", t.bucket, s3sign::uri_encode(&t.key, true));
    let payload = s3sign::sha256_hex(body.unwrap_or(&[]));
    let signed = s3sign::sign(&Request {
        method,
        host: &t.host,
        path: &path,
        query: &[],
        region: &t.region,
        access_key: &t.access_key,
        secret_key: &t.secret_key,
        amz_date: stamp,
        payload_sha256: &payload,
        extra_headers: &[],
    })
    .map_err(|e| e.to_string())?;
    let url = format!("{}://{}{}", if t.https { "https" } else { "http" }, t.host, path);
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(TIMEOUT)).build().into();
    let resp = match (method, body) {
        ("PUT", Some(b)) => agent.put(&url).header("x-amz-date", stamp).header("x-amz-content-sha256", &payload).header("Authorization", &signed.authorization).send(b),
        _ => agent.get(&url).header("x-amz-date", stamp).header("x-amz-content-sha256", &payload).header("Authorization", &signed.authorization).call(),
    }
    .map_err(|e| format!("could not reach the storage server: {e}"))?;
    let status = resp.status().as_u16();
    let bytes = resp.into_body().with_config().limit(MAX_DOWNLOAD).read_to_vec().map_err(|e| format!("could not read the server's answer: {e}"))?;
    Ok((status, bytes))
}

fn failure(action: &str, status: u16, body: &[u8]) -> String {
    let b = String::from_utf8_lossy(body);
    let code = b.split("<Code>").nth(1).and_then(|s| s.split("</Code>").next()).unwrap_or("");
    let msg = b.split("<Message>").nth(1).and_then(|s| s.split("</Message>").next()).unwrap_or("");
    format!("{action} was refused by the server (HTTP {status}{}{}{})", if code.is_empty() { "" } else { ", " }, code, if msg.is_empty() { String::new() } else { format!(": {msg}") })
}

/// Uploads the sealed backup at `file`. Returns what the server was given, for the user to compare.
pub fn put_backup(t: &Target, file: &std::path::Path, stamp: &str) -> Result<Value, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    if bytes.len() < MAGIC.len() || &bytes[..MAGIC.len()] != MAGIC {
        return Err(format!("{} is not an encrypted NanoArch backup: only sealed backups are uploaded", file.display()));
    }
    let (status, body) = call(t, "PUT", Some(&bytes), stamp)?;
    if !(200..300).contains(&status) {
        return Err(failure("the upload", status, &body));
    }
    Ok(json!({ "uploaded": true, "bucket": t.bucket, "key": t.key, "bytes": bytes.len(), "sha256": nnx_core::backup::sha256_hex(&bytes), "http_status": status,
        "access_key": s3sign::mask(&t.access_key) }))
}

/// Downloads `key` into `dest` (refused if `dest` exists and `overwrite` is false); the bytes must be a sealed backup.
pub fn get_backup(t: &Target, dest: &std::path::Path, overwrite: bool, stamp: &str) -> Result<Value, String> {
    if dest.exists() && !overwrite {
        return Err(format!("{} already exists; pass overwrite: true to replace it", dest.display()));
    }
    let (status, body) = call(t, "GET", None, stamp)?;
    if !(200..300).contains(&status) {
        return Err(failure("the download", status, &body));
    }
    if body.len() < MAGIC.len() || &body[..MAGIC.len()] != MAGIC {
        return Err("the object is not an encrypted NanoArch backup, so it was not saved".into());
    }
    let tmp = dest.with_extension("nnxbak.part");
    std::fs::write(&tmp, &body).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, dest).map_err(|e| format!("cannot move the download into {}: {e}", dest.display()))?;
    Ok(json!({ "downloaded": true, "path": dest.display().to_string(), "bytes": body.len(), "sha256": nnx_core::backup::sha256_hex(&body), "http_status": status }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn args(endpoint: &str) -> Value {
        json!({ "endpoint": endpoint, "bucket": "nano-backups", "key": "villa/a b.nnxbak", "region": "us-east-1", "access_key": "AKIAIOSFODNN7EXAMPLE", "secret_key": "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY" })
    }

    /// A one-shot S3 stand-in: stores the object of a PUT and serves it for a GET; returns what it saw.
    fn mock(store: Vec<u8>) -> (u16, std::thread::JoinHandle<(String, Vec<u8>)>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let (head_end, len) = loop {
                let n = s.read(&mut chunk).unwrap();
                buf.extend_from_slice(&chunk[..n]);
                if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..p]).to_lowercase();
                    let len = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
                    break (p + 4, len);
                }
            };
            while buf.len() < head_end + len {
                let n = s.read(&mut chunk).unwrap();
                buf.extend_from_slice(&chunk[..n]);
            }
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let is_get = head.starts_with("GET");
            let out = if is_get { store } else { Vec::new() };
            write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", out.len()).unwrap();
            s.write_all(&out).unwrap();
            (head, buf[head_end..head_end + len].to_vec())
        });
        (port, h)
    }

    fn sealed() -> Vec<u8> {
        nnx_core::backup::seal(b"project", "correct horse battery", nnx_core::backup::MIN_ITERATIONS).unwrap()
    }

    #[test]
    fn stamp_is_utc_calendar_time() {
        assert_eq!(amz_stamp(0), "19700101T000000Z");
        assert_eq!(amz_stamp(1_369_353_600), "20130524T000000Z");
        assert_eq!(amz_stamp(951_782_400 + 86_399), "20000229T235959Z");
    }

    #[test]
    fn target_fields_and_endpoint_are_checked_by_name() {
        let t = parse_target(&args("http://127.0.0.1:9000")).unwrap();
        assert_eq!((t.https, t.host.as_str()), (false, "127.0.0.1:9000"));
        assert_eq!(parse_target(&args("https://s3.example.com:443/")).unwrap().host, "s3.example.com");
        assert!(parse_target(&args("http://192.168.1.20:9000")).is_ok());
        assert!(parse_target(&args("http://s3.example.com")).unwrap_err().contains("use https"));
        assert!(parse_target(&args("http://8.8.8.8")).unwrap_err().contains("use https"));
        assert!(parse_target(&args("ftp://x")).unwrap_err().contains("scheme"));
        assert!(parse_target(&args("https://x.com/bucket")).unwrap_err().contains("just scheme"));
        assert!(parse_target(&args("s3.example.com")).unwrap_err().contains("https://"));
        for (field, want) in [("region", "region"), ("access_key", "access_key"), ("secret_key", "secret_key"), ("bucket", "bucket"), ("key", "key")] {
            let mut a = args("https://s3.example.com");
            a.as_object_mut().unwrap().remove(field);
            assert!(parse_target(&a).unwrap_err().contains(want), "{field}");
        }
        let mut a = args("https://s3.example.com");
        a["bucket"] = json!("Bad_Bucket");
        assert!(parse_target(&a).unwrap_err().contains("bucket"));
        a["bucket"] = json!("ok-bucket");
        a["key"] = json!("../x");
        assert!(parse_target(&a).is_err());
    }

    #[test]
    fn put_signs_the_request_and_sends_exactly_the_sealed_bytes() {
        let dir = std::env::temp_dir().join(format!("nnx-s3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.nnxbak");
        let blob = sealed();
        std::fs::write(&file, &blob).unwrap();
        let (port, h) = mock(Vec::new());
        let t = parse_target(&args(&format!("http://127.0.0.1:{port}"))).unwrap();
        let r = put_backup(&t, &file, "20130524T000000Z").unwrap();
        let (head, body) = h.join().unwrap();
        assert_eq!(body, blob);
        assert!(head.starts_with("PUT /nano-backups/villa/a%20b.nnxbak HTTP/1.1"), "{head}");
        let low = head.to_lowercase();
        assert!(low.contains(&format!("x-amz-content-sha256: {}", nnx_core::backup::sha256_hex(&blob))));
        assert!(low.contains("x-amz-date: 20130524t000000z"));
        assert!(head.contains("AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature="), "{head}");
        assert!(!head.contains("wJalrXUtnFEMI"), "the secret key must never be sent");
        assert_eq!(r["bytes"], blob.len());
        assert_eq!(r["access_key"], "AKIA****");
        assert!(!r.to_string().contains("wJalrXUtnFEMI"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plain_file_is_never_uploaded() {
        let file = std::env::temp_dir().join(format!("nnx-s3-plain-{}.nnx", std::process::id()));
        std::fs::write(&file, b"SQLite format 3\0 pretend project").unwrap();
        let t = parse_target(&args("http://127.0.0.1:9")).unwrap();
        assert!(put_backup(&t, &file, "20130524T000000Z").unwrap_err().contains("not an encrypted NanoArch backup"));
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn get_saves_a_sealed_backup_and_refuses_anything_else_or_overwriting() {
        let dir = std::env::temp_dir().join(format!("nnx-s3g-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let blob = sealed();
        let (port, h) = mock(blob.clone());
        let t = parse_target(&args(&format!("http://127.0.0.1:{port}"))).unwrap();
        let dest = dir.join("back.nnxbak");
        let r = get_backup(&t, &dest, false, "20130524T000000Z").unwrap();
        assert!(h.join().unwrap().0.starts_with("GET /nano-backups/villa/a%20b.nnxbak"));
        assert_eq!(std::fs::read(&dest).unwrap(), blob);
        assert_eq!(r["sha256"], nnx_core::backup::sha256_hex(&blob));
        assert!(get_backup(&t, &dest, false, "20130524T000000Z").unwrap_err().contains("already exists"));
        let (port, h) = mock(b"<html>not a backup</html>".to_vec());
        let t = parse_target(&args(&format!("http://127.0.0.1:{port}"))).unwrap();
        let other = dir.join("other.nnxbak");
        assert!(get_backup(&t, &other, false, "20130524T000000Z").unwrap_err().contains("not an encrypted"));
        h.join().unwrap();
        assert!(!other.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
