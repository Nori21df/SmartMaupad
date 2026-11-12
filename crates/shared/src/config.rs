//! Lưu cấu hình dạng TOML ở `%APPDATA%\SmartMaupad`.
//!
//! Mật khẩu được mã hoá bằng DPAPI theo tài khoản Windows hiện tại trước khi
//! ghi ra file, nên file config chép sang máy/tài khoản khác sẽ không giải mã được.

use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::{fs, io, slice};

use serde::de::DeserializeOwned;
use serde::Serialize;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
};

const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

fn config_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("SmartMaupad")
}

/// Đọc config; file thiếu hoặc hỏng thì trả giá trị mặc định.
pub fn load<T: DeserializeOwned + Default>(file_name: &str) -> T {
    fs::read_to_string(config_dir().join(file_name))
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save<T: Serialize>(file_name: &str, value: &T) -> io::Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    let text = toml::to_string_pretty(value).map_err(io::Error::other)?;
    fs::write(dir.join(file_name), text)
}

/// Mã hoá bằng DPAPI, trả chuỗi hex. Trả chuỗi rỗng nếu đầu vào rỗng hoặc lỗi.
pub fn protect(plain: &str) -> String {
    if plain.is_empty() {
        return String::new();
    }
    dpapi(plain.as_bytes(), true).map(|bytes| to_hex(&bytes)).unwrap_or_default()
}

/// Ngược lại với [`protect`]. `None` nếu không giải mã được.
pub fn unprotect(hex: &str) -> Option<String> {
    let bytes = from_hex(hex)?;
    if bytes.is_empty() {
        return None;
    }
    String::from_utf8(dpapi(&bytes, false)?).ok()
}

fn dpapi(input: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    let data_in = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(input.len()).ok()?,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut data_out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: null_mut() };
    // SAFETY: data_in trỏ vào `input` còn sống suốt lời gọi; DPAPI chỉ đọc nó.
    // data_out do DPAPI cấp phát bằng LocalAlloc và được giải phóng ngay bên dưới.
    unsafe {
        let ok = if encrypt {
            CryptProtectData(
                &data_in,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut data_out,
            )
        } else {
            CryptUnprotectData(
                &data_in,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut data_out,
            )
        };
        if ok == 0 || data_out.pbData.is_null() {
            return None;
        }
        let bytes = slice::from_raw_parts(data_out.pbData, data_out.cbData as usize).to_vec();
        LocalFree(data_out.pbData.cast());
        Some(bytes)
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) || !hex.is_ascii() {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let bytes = [0u8, 1, 0x7f, 0x80, 0xff];
        assert_eq!(from_hex(&to_hex(&bytes)).unwrap(), bytes);
        assert!(from_hex("abc").is_none());
        assert!(from_hex("zz").is_none());
    }

    #[test]
    fn dpapi_round_trip() {
        let secret = "mật khẩu bí mật 🔑";
        let stored = protect(secret);
        assert!(!stored.is_empty());
        assert!(!stored.contains("secret"));
        assert_eq!(unprotect(&stored).as_deref(), Some(secret));
    }

    #[test]
    fn unprotect_rejects_garbage() {
        assert_eq!(unprotect(""), None);
        assert_eq!(unprotect("00112233"), None);
        assert_eq!(protect(""), "");
    }
}
