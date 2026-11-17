//! Google와 직접 통신한 HTTPS 토큰 응답 전용. 외부에서 받은 JWT의 서명 검증기로 사용하지 않는다.
//! 신원은 고정된 Google UserInfo 엔드포인트의 응답과도 대조한다.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub subject: String,
    pub email: String,
}

pub(super) fn valid_subject(sub: &str) -> bool {
    !sub.is_empty() && sub.len() <= 255 && sub.bytes().all(|b| b.is_ascii_graphic())
}

pub(super) fn claims_from_google_response(
    token: &str,
    client_id: &str,
    nonce: Option<&str>,
    access: &str,
    now: i64,
) -> Result<String> {
    let fail = || "Google 계정 확인 정보가 올바르지 않아요. 다시 로그인해 주세요.".to_owned();
    if token.len() > 16384 {
        return Err(fail());
    }
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
        return Err(fail());
    }
    let decode = |part: &str| -> Result<Value> {
        serde_json::from_slice(&B64URL.decode(part).map_err(|_| fail())?).map_err(|_| fail())
    };
    let header = decode(parts[0])?;
    if header["alg"] != "RS256" {
        return Err(fail());
    }
    let c = decode(parts[1])?;
    if !["accounts.google.com", "https://accounts.google.com"].contains(&c.s("iss").as_str()) {
        return Err(fail());
    }
    let audience = match &c["aud"] {
        Value::String(id) => id == client_id,
        Value::Array(ids) => {
            ids.iter().all(Value::is_string)
                && ids.iter().any(|id| id.as_str() == Some(client_id))
                && c["azp"] == client_id
        }
        _ => false,
    };
    if !audience
        || c.get("azp")
            .is_some_and(|azp| azp.as_str() != Some(client_id))
    {
        return Err(fail());
    }
    let issued = c["iat"].as_i64().ok_or_else(fail)?;
    let expires = c["exp"].as_i64().ok_or_else(fail)?;
    if issued > now.saturating_add(60) || expires <= now || expires <= issued {
        return Err(fail());
    }
    if nonce.is_some_and(|expected| c["nonce"].as_str() != Some(expected)) {
        return Err(fail());
    }
    if let Some(hash) = c.get("at_hash") {
        let digest = Sha256::digest(access.as_bytes());
        if hash.as_str() != Some(B64URL.encode(&digest[..16]).as_str()) {
            return Err(fail());
        }
    }
    let subject = c.s("sub");
    if !valid_subject(&subject) {
        return Err(fail());
    }
    Ok(subject)
}

pub(super) fn user_identity(info: &Value, expected_subject: &str) -> Result<Identity> {
    let subject = info.s("sub");
    if !valid_subject(&subject) || subject != expected_subject {
        return err("Google 계정 확인 정보가 서로 달라요. 다시 로그인해 주세요.");
    }
    let verified = info["email_verified"] == true || info["email_verified"] == "true";
    let email = info.s("email");
    if !verified || email.is_empty() || email.len() > 320 || email.chars().any(char::is_control) {
        return err("Google에서 확인한 이메일 정보를 받지 못했어요. 이메일 권한을 확인해 주세요.");
    }
    Ok(Identity { subject, email })
}
