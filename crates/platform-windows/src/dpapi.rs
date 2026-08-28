//! DPAPI（CryptProtectData）加密：用于 WebDAV 凭据本地存储
//!
//! 密文以 "dpapi:<hex>" 前缀存储在 secrets.json，只有当前用户可解密。

use std::fmt;

const PREFIX: &str = "dpapi:";

/// DPAPI 加密后的密文（十六进制编码）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedBlob(String);

impl fmt::Display for ProtectedBlob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl ProtectedBlob {
    /// 序列化成存储格式字符串
    pub fn to_storage_string(&self) -> String {
        self.0.clone()
    }
}

/// 用当前用户上下文加密数据
pub fn protect(plaintext: &str) -> std::io::Result<ProtectedBlob> {
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};
    let input = plaintext.as_bytes();
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    unsafe {
        let ok = CryptProtectData(
            &in_blob,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
            0,
            &mut out_blob,
        );
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    let bytes = unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) };
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    unsafe {
        windows_sys::Win32::Foundation::LocalFree(out_blob.pbData as *mut _ as _);
    }
    Ok(ProtectedBlob(format!("{PREFIX}{hex}")))
}

/// 解密 "dpapi:<hex>" 格式密文
pub fn unprotect(stored: &str) -> std::io::Result<String> {
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};
    let hex = stored
        .strip_prefix(PREFIX)
        .ok_or_else(|| std::io::Error::other("密文缺少 dpapi: 前缀"))?;
    let bytes: Vec<u8> = (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16))
        .collect::<Result<_, _>>()
        .map_err(|e| std::io::Error::other(format!("密文 hex 解码失败: {e}")))?;
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    unsafe {
        let ok = CryptUnprotectData(
            &in_blob,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
            0,
            &mut out_blob,
        );
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    let plain = unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize) };
    let text = String::from_utf8_lossy(plain).to_string();
    unsafe {
        windows_sys::Win32::Foundation::LocalFree(out_blob.pbData as *mut _ as _);
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpapi加解密往返() {
        let secret = "webdav-密码-test-123";
        let blob = protect(secret).unwrap();
        assert!(blob.to_storage_string().starts_with(PREFIX));
        let plain = unprotect(&blob.to_storage_string()).unwrap();
        assert_eq!(plain, secret);
    }
}
