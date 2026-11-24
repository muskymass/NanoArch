use nnx_core::backup::*;
use nnx_core::s3sign::{self, Request, SignError};
use nnx_core::{Command, Discipline, Document, Element, Origin, Source};
use serde_json::json;

const PASS: &str = "correct horse battery";

fn fast(plain: &[u8]) -> Vec<u8> {
    seal(plain, PASS, MIN_ITERATIONS).unwrap()
}

#[test]
fn round_trip_and_fresh_randomness() {
    let a = fast(b"project bytes");
    let b = fast(b"project bytes");
    assert_ne!(a, b, "salt and nonce are random per backup");
    assert_eq!(open(&a, PASS).unwrap(), b"project bytes");
    assert!(!a.windows(7).any(|w| w == b"project"), "plaintext must not appear in the file");
    assert_eq!(a.len(), HEADER_LEN + 13 + 16);
}

#[test]
fn wrong_passphrase_and_every_changed_byte_are_refused() {
    let blob = fast(b"secret plan");
    assert!(matches!(open(&blob, "another passphrase!"), Err(BackupError::Authentication)));
    for i in (12..HEADER_LEN + 2).chain(blob.len() - 2..blob.len()) {
        let mut t = blob.clone();
        t[i] ^= 1;
        assert!(matches!(open(&t, PASS), Err(BackupError::Authentication)), "byte {i}");
    }
}

#[test]
fn iteration_count_is_authenticated_and_bounded() {
    let mut blob = fast(b"x");
    blob[8..12].copy_from_slice(&(MIN_ITERATIONS + 1).to_le_bytes());
    assert!(matches!(open(&blob, PASS), Err(BackupError::Authentication)));
    blob[8..12].copy_from_slice(&5u32.to_le_bytes());
    assert!(matches!(open(&blob, PASS), Err(BackupError::Iterations(5))));
    assert!(matches!(seal(b"x", PASS, 10), Err(BackupError::Iterations(10))));
    assert!(matches!(seal(b"x", PASS, MAX_ITERATIONS + 1), Err(BackupError::Iterations(_))));
}

#[test]
fn bad_inputs_are_named() {
    assert!(matches!(seal(b"x", "short", MIN_ITERATIONS), Err(BackupError::ShortPassphrase(5))));
    assert!(matches!(open(b"hello world, not a backup at all", PASS), Err(BackupError::NotABackup)));
    let blob = fast(b"x");
    assert!(matches!(open(&blob[..HEADER_LEN + 3], PASS), Err(BackupError::Truncated)));
    assert!(matches!(open(&blob, "short"), Err(BackupError::ShortPassphrase(5))));
}

#[test]
fn sha256_known_vector() {
    assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

#[test]
fn project_file_round_trips_through_an_encrypted_backup() {
    let dir = std::env::temp_dir().join(format!("nnx-bak-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut doc = Document::lenient();
    let col = Element::new("Column", Discipline::Str, Source::Generated).with_prop("section", json!({ "b": 300, "h": 600 }));
    doc.commit("seed", Origin::User, vec![Command::Create(col)]).unwrap();
    let mut settings = nnx_core::persist::Settings::new();
    settings.insert("note".into(), json!("kept"));
    let file = dir.join("p.nnxbak");
    let (hash, size) = write_backup(&doc.model, &settings, PASS, MIN_ITERATIONS, &file).unwrap();
    assert_eq!(size as u64, std::fs::metadata(&file).unwrap().len());
    assert_eq!(hash, sha256_hex(&std::fs::read(&file).unwrap()));
    let (back, s) = read_backup(&file, PASS).unwrap();
    assert_eq!(back, doc.model);
    assert_eq!(s.get("note"), Some(&json!("kept")));
    assert!(read_backup(&file, "wrong passphrase!").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

// The worked GET Object example from the AWS Signature Version 4 documentation for S3.
#[test]
fn sigv4_matches_the_aws_documented_example() {
    let empty = s3sign::sha256_hex(b"");
    assert_eq!(empty, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    let r = Request {
        method: "GET",
        host: "examplebucket.s3.amazonaws.com",
        path: "/test.txt",
        query: &[],
        region: "us-east-1",
        access_key: "AKIAIOSFODNN7EXAMPLE",
        secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        amz_date: "20130524T000000Z",
        payload_sha256: &empty,
        extra_headers: &[("range", "bytes=0-9")],
    };
    let s = s3sign::sign(&r).unwrap();
    assert_eq!(s.signed_headers, "host;range;x-amz-content-sha256;x-amz-date");
    assert!(s.authorization.ends_with("Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"), "{}", s.authorization);
}

#[test]
fn sigv4_inputs_are_checked_and_secrets_masked() {
    let base = Request { method: "PUT", host: "h", path: "/b/k", query: &[], region: "r", access_key: "a", secret_key: "s", amz_date: "20130524T000000Z", payload_sha256: "00", extra_headers: &[] };
    assert!(s3sign::sign(&base).is_ok());
    assert_eq!(s3sign::sign(&Request { region: " ", ..base }).err(), Some(SignError::Missing("region")));
    assert!(matches!(s3sign::sign(&Request { amz_date: "2013-05-24", ..base }), Err(SignError::BadStamp(_))));
    assert!(s3sign::check_key("a/b.nnxbak").is_ok());
    assert!(s3sign::check_key("a/../b").is_err() && s3sign::check_key("").is_err() && s3sign::check_key("a\\b").is_err());
    assert_eq!(s3sign::uri_encode("a b/ü", true), "a%20b/%C3%BC");
    assert_eq!(s3sign::mask("AKIAIOSFODNN7EXAMPLE"), "AKIA****");
    assert_eq!(s3sign::mask("short"), "**** (5 chars)");
}
