//! API 키·로그인 토큰을 암호화해 설정 파일에 보관한다.
//! Windows 에서는 DPAPI(현재 사용자 계정으로만 풀 수 있음)를 쓰고, 다른 운영체제에서는 base64 로만 감싼다.
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde_json::Value;

pub fn encrypt(plain: &str) -> Option<String> {
    if plain.is_empty() {
        return None;
    }
    #[cfg(windows)]
    {
        if let Some(enc) = dpapi::protect(plain.as_bytes()) {
            return Some(format!("dpapi:{}", B64.encode(enc)));
        }
    }
    Some(format!("raw:{}", B64.encode(plain.as_bytes())))
}

pub fn encrypt_json(v: &Value) -> Option<String> {
    if v.is_null() {
        return None;
    }
    encrypt(&v.to_string())
}

pub fn decrypt(stored: Option<&str>) -> Option<String> {
    let s = stored?;
    if let Some(rest) = s.strip_prefix("raw:") {
        return String::from_utf8(B64.decode(rest).ok()?).ok();
    }
    #[cfg(windows)]
    if let Some(rest) = s.strip_prefix("dpapi:") {
        let bytes = dpapi::unprotect(&B64.decode(rest).ok()?)?;
        return String::from_utf8(bytes).ok();
    }
    None
}

pub fn decrypt_json(stored: Option<&str>) -> Option<Value> {
    serde_json::from_str(&decrypt(stored)?).ok()
}

/// 설정 값(Value)에서 암호화된 문자열을 꺼내 푼다
pub fn decrypt_field(settings: &Value, key: &str) -> Option<String> {
    decrypt(settings.get(key).and_then(|v| v.as_str()))
}

pub fn decrypt_json_field(settings: &Value, key: &str) -> Option<Value> {
    decrypt_json(settings.get(key).and_then(|v| v.as_str()))
}

#[cfg(windows)]
mod dpapi {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};

    fn run(input: &[u8], protect: bool) -> Option<Vec<u8>> {
        unsafe {
            let mut inp = CRYPT_INTEGER_BLOB { cbData: input.len() as u32, pbData: input.as_ptr() as *mut u8 };
            let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
            let ok = if protect {
                CryptProtectData(&mut inp, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0, &mut out)
            } else {
                CryptUnprotectData(&mut inp, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0, &mut out)
            };
            if ok == 0 || out.pbData.is_null() {
                return None;
            }
            let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(out.pbData as _);
            Some(v)
        }
    }

    pub fn protect(b: &[u8]) -> Option<Vec<u8>> {
        run(b, true)
    }
    pub fn unprotect(b: &[u8]) -> Option<Vec<u8>> {
        run(b, false)
    }
}
