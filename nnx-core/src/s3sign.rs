//! AWS Signature Version 4 for S3-compatible object stores (AWS S3, MinIO, Cloudflare R2, Backblaze B2, Wasabi...).
//!
//! Pure functions: they build the signature and send nothing. The caller passes the credentials for that one request;
//! nothing here stores or logs them.

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SignError {
    #[error("{0} is required")]
    Missing(&'static str),
    #[error("the timestamp must look like 20130524T000000Z (got '{0}')")]
    BadStamp(String),
    #[error("the object key '{0}' must not be empty or contain '..' or a backslash")]
    BadKey(String),
}

pub struct Request<'a> {
    pub method: &'a str,
    /// Host header value, with the port when it is not the scheme's default (`localhost:9000`).
    pub host: &'a str,
    /// Already URI-encoded absolute path (`/bucket/key`).
    pub path: &'a str,
    pub query: &'a [(&'a str, &'a str)],
    pub region: &'a str,
    pub access_key: &'a str,
    pub secret_key: &'a str,
    /// `YYYYMMDDTHHMMSSZ`, UTC.
    pub amz_date: &'a str,
    pub payload_sha256: &'a str,
    /// Extra headers to sign besides host, x-amz-content-sha256 and x-amz-date.
    pub extra_headers: &'a [(&'a str, &'a str)],
}

pub struct Signed {
    pub authorization: String,
    pub signed_headers: String,
    pub canonical_request: String,
}

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    // HMAC accepts keys of any length, so construction cannot fail.
    let mut m = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

/// RFC 3986 percent-encoding as SigV4 wants it (`/` kept only when `keep_slash`).
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut o = String::new();
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            b'/' if keep_slash => o.push('/'),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

/// An object key is refused when it is empty or could climb out of its prefix.
pub fn check_key(key: &str) -> Result<(), SignError> {
    if key.is_empty() || key.contains('\\') || key.split('/').any(|p| p == "..") {
        return Err(SignError::BadKey(key.to_string()));
    }
    Ok(())
}

pub fn sign(r: &Request) -> Result<Signed, SignError> {
    for (v, n) in [(r.host, "host"), (r.region, "region"), (r.access_key, "access key"), (r.secret_key, "secret key"), (r.payload_sha256, "payload hash")] {
        if v.trim().is_empty() {
            return Err(SignError::Missing(n));
        }
    }
    let d = r.amz_date.as_bytes();
    let stamp_ok = d.len() == 16 && d[8] == b'T' && d[15] == b'Z' && d[..8].iter().all(u8::is_ascii_digit) && d[9..15].iter().all(u8::is_ascii_digit);
    if !stamp_ok {
        return Err(SignError::BadStamp(r.amz_date.to_string()));
    }
    let day = &r.amz_date[..8];
    let mut headers: Vec<(String, String)> = vec![
        ("host".into(), r.host.trim().into()),
        ("x-amz-content-sha256".into(), r.payload_sha256.into()),
        ("x-amz-date".into(), r.amz_date.into()),
    ];
    headers.extend(r.extra_headers.iter().map(|(k, v)| (k.to_lowercase(), v.trim().to_string())));
    headers.sort();
    let canon_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");
    let mut q: Vec<(String, String)> = r.query.iter().map(|(k, v)| (uri_encode(k, false), uri_encode(v, false))).collect();
    q.sort();
    let canon_query = q.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
    let canonical_request = format!("{}\n{}\n{}\n{}\n{}\n{}", r.method, r.path, canon_query, canon_headers, signed_headers, r.payload_sha256);
    let scope = format!("{day}/{}/s3/aws4_request", r.region);
    let to_sign = format!("AWS4-HMAC-SHA256\n{}\n{scope}\n{}", r.amz_date, sha256_hex(canonical_request.as_bytes()));
    let k = hmac(format!("AWS4{}", r.secret_key).as_bytes(), day.as_bytes());
    let k = hmac(&k, r.region.as_bytes());
    let k = hmac(&k, b"s3");
    let k = hmac(&k, b"aws4_request");
    let sig: String = hmac(&k, to_sign.as_bytes()).iter().map(|x| format!("{x:02x}")).collect();
    let authorization = format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={sig}", r.access_key);
    Ok(Signed { authorization, signed_headers, canonical_request })
}

/// First four characters then `****` for a key of 16+ characters, else `****` and its length (the workspace rule for showing secrets).
pub fn mask(secret: &str) -> String {
    let n = secret.chars().count();
    if n >= 16 { format!("{}****", secret.chars().take(4).collect::<String>()) } else { format!("**** ({n} chars)") }
}
