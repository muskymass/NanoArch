//! The API key of the remote AI option, kept in the Windows Credential Manager (per user, encrypted by Windows).
//!
//! The key is never written to a project, a settings file or the page's storage, and it is never returned whole: the page only learns
//! whether one is stored and its masked form (first 4 characters then `****` for 16 or more, else `****` and the length).

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW, CredWriteW};
    use windows::core::{PCWSTR, PWSTR};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn store(target: &str, secret: &str) -> Result<(), String> {
        let mut name = wide(target);
        let mut user = wide("NanoArch");
        let mut blob = secret.as_bytes().to_vec();
        let cred = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(name.as_mut_ptr()),
            UserName: PWSTR(user.as_mut_ptr()),
            CredentialBlobSize: u32::try_from(blob.len()).map_err(|_| "the key is too long")?,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        // SAFETY: `cred` points at buffers that live until the call returns; CredWriteW copies them.
        unsafe { CredWriteW(&cred, 0) }.map_err(|e| format!("cannot save the key in the Windows Credential Manager: {e}"))
    }

    pub fn load(target: &str) -> Result<Option<String>, String> {
        let name = wide(target);
        let mut out: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `out` receives a buffer owned by Windows, freed below with CredFree.
        match unsafe { CredReadW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None, &mut out) } {
            Ok(()) => {
                // SAFETY: CredReadW succeeded, so `out` is valid and the blob has `CredentialBlobSize` bytes.
                let text = unsafe {
                    let c = &*out;
                    let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec();
                    CredFree(out as *const _);
                    String::from_utf8(bytes)
                };
                text.map(Some).map_err(|_| "the stored key is not text".to_string())
            }
            Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(None),
            Err(e) => Err(format!("cannot read the key from the Windows Credential Manager: {e}")),
        }
    }

    pub fn remove(target: &str) -> Result<(), String> {
        let name = wide(target);
        // SAFETY: plain call with a NUL-terminated name.
        match unsafe { CredDeleteW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
            Err(e) => Err(format!("cannot remove the key: {e}")),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn store(_: &str, _: &str) -> Result<(), String> {
        Err("storing the key needs Windows".into())
    }
    pub fn load(_: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    pub fn remove(_: &str) -> Result<(), String> {
        Ok(())
    }
}

/// One key per provider: the credential's name carries the provider's id (letters, digits, `-` and `_` only).
fn target(id: &str) -> Result<String, String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("bad provider id '{id}'"));
    }
    Ok(format!("NanoArch/ai-api-key/{id}"))
}

pub fn save_key(id: &str, key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("the key is empty".into());
    }
    imp::store(&target(id)?, key)
}

pub fn key(id: &str) -> Result<Option<String>, String> {
    imp::load(&target(id)?)
}

pub fn clear_key(id: &str) -> Result<(), String> {
    imp::remove(&target(id)?)
}

/// `AIza****`-style display: first 4 characters then `****` for a key of 16 or more, else `****` and its length.
pub fn mask(key: &str) -> String {
    let n = key.chars().count();
    if n >= 16 { format!("{}****", key.chars().take(4).collect::<String>()) } else { format!("**** ({n})") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_key_shows_four_characters_and_a_short_one_only_its_length() {
        assert_eq!(mask("sk-abcdefghijklmnop"), "sk-a****");
        assert_eq!(mask("short"), "**** (5)");
        assert_eq!(target("gemini").unwrap(), "NanoArch/ai-api-key/gemini");
        assert!(target("../x").is_err() && target("").is_err());
    }
}
