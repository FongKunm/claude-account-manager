use crate::model::AppResult;
#[cfg(not(target_os = "macos"))]
use base64::{engine::general_purpose::STANDARD, Engine};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

#[cfg(not(target_os = "macos"))]
pub const STORAGE_TAG: &str = "dpapi";
#[cfg(target_os = "macos")]
pub const STORAGE_TAG: &str = "keychain";
pub fn fingerprint(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

// DPAPI CurrentUser: never LOCAL_MACHINE, never a plaintext fallback.
#[cfg(windows)]
fn crypt(input: &[u8], encrypt: bool) -> AppResult<Zeroizing<Vec<u8>>> {
    use std::ffi::c_void;
    #[repr(C)]
    struct Blob {
        len: u32,
        bytes: *mut u8,
    }
    #[link(name = "crypt32")]
    unsafe extern "system" {
        fn CryptProtectData(
            input: *const Blob,
            description: *const u16,
            entropy: *const Blob,
            reserved: *mut c_void,
            prompt: *const c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
        fn CryptUnprotectData(
            input: *const Blob,
            description: *mut *mut u16,
            entropy: *const Blob,
            reserved: *mut c_void,
            prompt: *const c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }
    if input.len() > 16 * 1024 * 1024 {
        return Err("加密数据过大。".into());
    }
    let entropy = b"ClaudeAccountManager/DPAPI/v2";
    let source = Blob {
        len: input.len() as u32,
        bytes: input.as_ptr() as *mut u8,
    };
    let purpose = Blob {
        len: entropy.len() as u32,
        bytes: entropy.as_ptr() as *mut u8,
    };
    let mut result = Blob {
        len: 0,
        bytes: std::ptr::null_mut(),
    };
    // SAFETY: input buffers live through the call. Copy and wipe the Windows
    // allocation before releasing it with LocalFree.
    unsafe {
        let success = if encrypt {
            CryptProtectData(
                &source,
                std::ptr::null(),
                &purpose,
                std::ptr::null_mut(),
                std::ptr::null(),
                1,
                &mut result,
            )
        } else {
            CryptUnprotectData(
                &source,
                std::ptr::null_mut(),
                &purpose,
                std::ptr::null_mut(),
                std::ptr::null(),
                1,
                &mut result,
            )
        };
        if success == 0 {
            return Err("Windows 凭据加解密失败；请使用保存令牌的 Windows 用户。".into());
        }
        let bytes = std::slice::from_raw_parts_mut(result.bytes, result.len as usize);
        let owned = Zeroizing::new(bytes.to_vec());
        zeroize::Zeroize::zeroize(bytes);
        LocalFree(result.bytes.cast());
        Ok(owned)
    }
}
#[cfg(not(any(windows, target_os = "macos")))]
fn crypt(_: &[u8], _: bool) -> AppResult<Zeroizing<Vec<u8>>> {
    Err("此版本仅支持 Windows DPAPI。".into())
}
#[cfg(not(target_os = "macos"))]
pub fn seal(bytes: &[u8]) -> AppResult<String> {
    Ok(format!(
        "dpapi-v1:{}",
        STANDARD.encode(crypt(bytes, true)?.as_slice())
    ))
}
#[cfg(not(target_os = "macos"))]
pub fn unseal(value: &str) -> AppResult<Zeroizing<Vec<u8>>> {
    let raw = value
        .strip_prefix("dpapi-v1:")
        .ok_or("凭据加密格式不正确。")?;
    let bytes = STANDARD.decode(raw).map_err(|_| "凭据加密格式不正确。")?;
    crypt(&bytes, false)
}
#[cfg(target_os = "macos")]
pub fn seal(bytes: &[u8]) -> AppResult<String> {
    crate::macos_support::seal(bytes)
}
#[cfg(target_os = "macos")]
pub fn unseal(value: &str) -> AppResult<Zeroizing<Vec<u8>>> {
    crate::macos_support::unseal(value)
}
pub fn token(value: &str) -> AppResult<Zeroizing<String>> {
    let bytes = unseal(value)?;
    String::from_utf8(bytes.to_vec())
        .map(Zeroizing::new)
        .map_err(|_| "凭据格式不正确。".into())
}
pub fn validate_token(value: &str) -> AppResult<Zeroizing<String>> {
    let value = value.trim();
    if !value.starts_with("sk-ant-oat01-")
        || value.len() < 32
        || value.len() > 4096
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err("请输入完整的 setup token（以 sk-ant-oat01- 开头）。".into());
    }
    Ok(Zeroizing::new(value.to_owned()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip_and_tampering() {
        let secret = b"only-a-fixture-never-a-real-token";
        let encrypted = seal(secret).unwrap();
        assert!(!encrypted.contains("only-a-fixture"));
        assert_eq!(unseal(&encrypted).unwrap().as_slice(), secret);
        assert!(unseal("plaintext").is_err());
        let mut raw = STANDARD
            .decode(encrypted.strip_prefix("dpapi-v1:").unwrap())
            .unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        assert!(unseal(&format!("dpapi-v1:{}", STANDARD.encode(raw))).is_err());
    }
    #[test]
    fn rejects_header_injection() {
        assert!(validate_token("sk-ant-oat01-aaaaaaaaaaaaaaaaaaaa\r\nInjected: yes").is_err());
        assert!(validate_token("sk-ant-oat01-aaaaaaaaaaaaaaaaaaaa").is_ok());
    }
}
