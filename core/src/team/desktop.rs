//! 데스크톱 서버는 팀 문서만 맡는다. 로그인 및 토큰 갱신은 기존 Firebase 계정을 사용한다.
use crate::{err, Result};
use once_cell::sync::Lazy;
use serde_json::Value;
use std::time::Duration;
use url::{Host, Url};

pub const PROJECT: &str = "saigajoa";

fn document_client(loopback: bool) -> reqwest::Client {
    let builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("sai-desktop-cat/", env!("CARGO_PKG_VERSION")));
    // 이 PC의 서버에는 회사/시스템 프록시를 거치지 않고 직접 연결한다.
    let builder = if loopback { builder.no_proxy() } else { builder };
    builder.build().expect("team document client")
}

static DOC_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| document_client(false));
static LOCAL_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| document_client(true));

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        Some(Host::Domain("localhost")) => true,
        _ => false,
    }
}

pub(super) fn client(address: &str) -> &'static reqwest::Client {
    if Url::parse(address).is_ok_and(|url| is_loopback(&url)) { &LOCAL_CLIENT } else { &DOC_CLIENT }
}

/// 설정에 저장하는 값은 경로나 쿼리를 붙이지 않은 서버의 origin이다.
pub fn origin(input: &str) -> Result<String> {
    let input = input.trim();
    if input.contains(['\\', '\n', '\r', '\t']) || input.len() > 512 {
        return err("팀 서버 주소를 확인해 주세요.");
    }
    let url = Url::parse(input).map_err(|_| "https:// 로 시작하는 팀 서버 주소를 입력해 주세요.")?;
    if url.host().is_none() || !url.username().is_empty() || url.password().is_some()
        || url.query().is_some() || url.fragment().is_some() || url.path() != "/" {
        return err("사용자 정보나 경로가 없는 팀 서버 주소를 입력해 주세요.");
    }
    if url.scheme() != "https" && !(url.scheme() == "http" && is_loopback(&url)) {
        return err("다른 컴퓨터에는 HTTPS 주소가 필요해요. 이 PC에서는 http://127.0.0.1:8765 를 쓸 수 있어요.");
    }
    Ok(url.origin().ascii_serialization())
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 36 && id.bytes().enumerate().all(|(i, b)| {
        if [8, 13, 18, 23].contains(&i) { b == b'-' } else { b.is_ascii_hexdigit() && !b.is_ascii_uppercase() }
    })
}

/// 신뢰 여부를 결정하기 전의 진단에는 로그인 정보와 팀 ID를 보내지 않는다.
pub async fn health(input: &str) -> Result<(String, String)> {
    let address = origin(input)?;
    let mut response = client(&address).get(format!("{address}/health"))
        .timeout(Duration::from_secs(8)).send().await.map_err(|_| {
            "데스크톱 서버에 연결할 수 없어요. 서버 실행 상태와 주소를 확인해 주세요."
        })?;
    if !response.status().is_success() {
        return err(format!("팀 서버 연결 확인에 실패했어요 ({}).", response.status().as_u16()));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "팀 서버 응답을 읽지 못했어요.")? {
        if chunk.len() > 4_096 - bytes.len() { return err("사이 팀 서버의 응답을 확인할 수 없어요."); }
        bytes.extend_from_slice(&chunk);
    }
    let body: Value = serde_json::from_slice(&bytes).map_err(|_| "사이 팀 서버 응답을 확인할 수 없어요.")?;
    let id = body["serverId"].as_str().unwrap_or_default();
    if body["service"] != "sai-team-server" || body["protocol"] != 1 || body["projectId"] != PROJECT || !valid_id(id) {
        return err("이 주소에 호환되는 사이 팀 서버가 있는지 확인해 주세요.");
    }
    Ok((address, id.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https_or_loopback_origins_only() {
        for address in ["http://192.168.0.5:8765", "javascript:abc", "file:///x", "https://a.test/team", "https://a.test/?q=x", "https://a.test/#id", "https://u:p@a.test", "http://localhost.evil.test"] {
            assert!(origin(address).is_err(), "{address}");
        }
        assert_eq!(origin(" https://Example.org/ ").unwrap(), "https://example.org");
        assert_eq!(origin("http://[::1]:8765").unwrap(), "http://[::1]:8765");
        assert!(origin("http://127.0.0.1:8765").is_ok());
    }

    #[test]
    fn server_id_is_safe_as_a_backup_filename() {
        assert!(valid_id("5608a7b4-f9da-4ad7-a98f-646be56ac567"));
        assert!(!valid_id("00000000-0000-0000-0000-00000000000/"));
    }

    #[test]
    fn only_loopback_requests_bypass_the_proxy() {
        for address in ["http://127.0.0.1:8765/health", "http://[::1]:8765/v1/documents", "https://localhost/"] {
            assert!(std::ptr::eq(client(address), &*LOCAL_CLIENT));
        }
        for address in ["https://team.example/health", "https://firestore.googleapis.com/v1/", "https://localhost.example/"] {
            assert!(std::ptr::eq(client(address), &*DOC_CLIENT));
        }
    }

    #[tokio::test]
    async fn health_checks_local_server_without_sending_credentials() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0u8; 1024];
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0 && request.len() < 8192);
                request.extend_from_slice(&buffer[..n]);
                if request.windows(4).any(|w| w == b"\r\n\r\n") { break; }
            }
            let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
            assert!(request.starts_with("get /health http/1.1\r\n"));
            assert!(!request.contains("authorization:"));
            let body = r#"{"service":"sai-team-server","protocol":1,"projectId":"saigajoa","serverId":"5608a7b4-f9da-4ad7-a98f-646be56ac567"}"#;
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let result = health(&address).await;
        if result.is_err() { server.abort(); }
        assert_eq!(result.unwrap(), (address, "5608a7b4-f9da-4ad7-a98f-646be56ac567".into()));
        server.await.unwrap();
    }
}
