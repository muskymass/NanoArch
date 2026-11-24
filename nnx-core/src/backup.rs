//! Encrypted project backup (`.nnxbak`): the bytes of a saved `.nnx` project sealed with a passphrase the user types.
//!
//! - **Cipher:** XChaCha20-Poly1305 (authenticated: a wrong passphrase or a changed byte is detected, never decrypted to garbage).
//! - **Key:** PBKDF2-HMAC-SHA256 from the passphrase and a fresh random 16-byte salt. The iteration count is stored in the
//!   header so a file keeps opening if the default is raised later.
//! - **Nothing is invented:** the passphrase comes from the user (at least [`MIN_PASSPHRASE_CHARS`] characters, no default), and
//!   is never stored. Losing it means the backup cannot be opened; there is no recovery.
//!
//! File layout: `magic(8) | iterations u32 LE (4) | salt (16) | nonce (24) | ciphertext+tag`. The whole header is the
//! associated data, so changing the iteration count, salt or nonce also fails authentication.

use crate::Model;
use crate::persist::{self, PersistError, Settings};
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"NNXBAK01";
pub const SALT_LEN: usize = 16;
pub const NONCE_LEN: usize = 24;
pub const HEADER_LEN: usize = 8 + 4 + SALT_LEN + NONCE_LEN;
const TAG_LEN: usize = 16;
/// PBKDF2-HMAC-SHA256 work factor used when the user does not choose one (OWASP Password Storage Cheat Sheet, 2023 guidance).
pub const DEFAULT_ITERATIONS: u32 = 600_000;
/// Below this a stored or requested count is refused as too weak; above [`MAX_ITERATIONS`] as an unreasonable wait.
pub const MIN_ITERATIONS: u32 = 100_000;
pub const MAX_ITERATIONS: u32 = 10_000_000;
pub const MIN_PASSPHRASE_CHARS: usize = 12;

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("the passphrase must have at least {MIN_PASSPHRASE_CHARS} characters (it has {0})")]
    ShortPassphrase(usize),
    #[error("iterations must be between {MIN_ITERATIONS} and {MAX_ITERATIONS} (got {0})")]
    Iterations(u32),
    #[error("this is not a NanoArch backup (bad header)")]
    NotABackup,
    #[error("the backup is shorter than its header and authentication tag: it is truncated")]
    Truncated,
    #[error("wrong passphrase, or the backup was changed after it was made")]
    Authentication,
    #[error("could not get random bytes from the operating system: {0}")]
    Random(String),
    #[error("the project could not be read or written: {0}")]
    Project(#[from] PersistError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

fn check_passphrase(p: &str) -> Result<(), BackupError> {
    let n = p.chars().count();
    if n < MIN_PASSPHRASE_CHARS { Err(BackupError::ShortPassphrase(n)) } else { Ok(()) }
}

fn derive_key(passphrase: &str, salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), salt, iterations, &mut key);
    key
}

fn header(iterations: u32, salt: &[u8; SALT_LEN], nonce: &[u8; NONCE_LEN]) -> Vec<u8> {
    let mut h = Vec::with_capacity(HEADER_LEN);
    h.extend_from_slice(MAGIC);
    h.extend_from_slice(&iterations.to_le_bytes());
    h.extend_from_slice(salt);
    h.extend_from_slice(nonce);
    h
}

/// Seals `plain` with a fresh random salt and nonce.
pub fn seal(plain: &[u8], passphrase: &str, iterations: u32) -> Result<Vec<u8>, BackupError> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut salt).map_err(|e| BackupError::Random(e.to_string()))?;
    getrandom::fill(&mut nonce).map_err(|e| BackupError::Random(e.to_string()))?;
    seal_with(plain, passphrase, iterations, &salt, &nonce)
}

/// Seals with a given salt and nonce. For tests and callers that drew them from the OS: reusing a nonce with the same key
/// breaks the cipher.
pub fn seal_with(plain: &[u8], passphrase: &str, iterations: u32, salt: &[u8; SALT_LEN], nonce: &[u8; NONCE_LEN]) -> Result<Vec<u8>, BackupError> {
    check_passphrase(passphrase)?;
    if !(MIN_ITERATIONS..=MAX_ITERATIONS).contains(&iterations) {
        return Err(BackupError::Iterations(iterations));
    }
    let key = derive_key(passphrase, salt, iterations);
    let cipher = XChaCha20Poly1305::new_from_slice(&key).map_err(|_| BackupError::Authentication)?;
    let mut out = header(iterations, salt, nonce);
    let ct = cipher.encrypt(nonce.into(), Payload { msg: plain, aad: &out }).map_err(|_| BackupError::Authentication)?;
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Opens a sealed backup. A wrong passphrase and a modified file give the same error on purpose.
pub fn open(blob: &[u8], passphrase: &str) -> Result<Vec<u8>, BackupError> {
    if blob.len() < 8 || &blob[..8] != MAGIC {
        return Err(BackupError::NotABackup);
    }
    if blob.len() < HEADER_LEN + TAG_LEN {
        return Err(BackupError::Truncated);
    }
    check_passphrase(passphrase)?;
    let iterations = u32::from_le_bytes([blob[8], blob[9], blob[10], blob[11]]);
    if !(MIN_ITERATIONS..=MAX_ITERATIONS).contains(&iterations) {
        return Err(BackupError::Iterations(iterations));
    }
    let salt = &blob[12..12 + SALT_LEN];
    let nonce: [u8; NONCE_LEN] = blob[12 + SALT_LEN..HEADER_LEN].try_into().map_err(|_| BackupError::Truncated)?;
    let key = derive_key(passphrase, salt, iterations);
    let cipher = XChaCha20Poly1305::new_from_slice(&key).map_err(|_| BackupError::Authentication)?;
    cipher.decrypt((&nonce).into(), Payload { msg: &blob[HEADER_LEN..], aad: &blob[..HEADER_LEN] }).map_err(|_| BackupError::Authentication)
}

/// Lower-case hex SHA-256 of `bytes` (a checksum to compare a backup after it is copied somewhere).
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn scratch(tag: &str) -> Result<std::path::PathBuf, BackupError> {
    let mut id = [0u8; 8];
    getrandom::fill(&mut id).map_err(|e| BackupError::Random(e.to_string()))?;
    let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
    Ok(std::env::temp_dir().join(format!("nanoarch-{tag}-{}-{hex}.nnx", std::process::id())))
}

/// The bytes of the project file exactly as `persist` would save it.
pub fn project_bytes(model: &Model, settings: &Settings) -> Result<Vec<u8>, BackupError> {
    let tmp = scratch("bak")?;
    let r = persist::save_with_settings(model, settings, &tmp).map_err(BackupError::from).and_then(|()| std::fs::read(&tmp).map_err(BackupError::from));
    let _ = std::fs::remove_file(&tmp);
    r
}

/// Reads a project from the bytes of a `.nnx` file.
pub fn project_from_bytes(bytes: &[u8]) -> Result<(Model, Settings), BackupError> {
    let tmp = scratch("rst")?;
    std::fs::write(&tmp, bytes)?;
    let r = persist::load_with_settings(&tmp).map_err(BackupError::from);
    let _ = std::fs::remove_file(&tmp);
    r
}

/// Encrypts the project into `path` (temporary file first, then renamed, like a normal save).
/// Returns the SHA-256 (hex) of the file written and its size.
pub fn write_backup(model: &Model, settings: &Settings, passphrase: &str, iterations: u32, path: &Path) -> Result<(String, usize), BackupError> {
    let blob = seal(&project_bytes(model, settings)?, passphrase, iterations)?;
    let tmp = path.with_extension("nnxbak.tmp");
    std::fs::write(&tmp, &blob)?;
    std::fs::rename(&tmp, path)?;
    Ok((sha256_hex(&blob), blob.len()))
}

/// Opens an encrypted backup file and reads the project inside it.
pub fn read_backup(path: &Path, passphrase: &str) -> Result<(Model, Settings), BackupError> {
    let blob = std::fs::read(path)?;
    project_from_bytes(&open(&blob, passphrase)?)
}
