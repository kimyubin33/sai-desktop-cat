// 설치 파일을 만들 때 config/google-client.json 이 있으면 Google 연결 정보(데스크톱 OAuth 클라이언트)를 함께 넣는다.
// GitHub 의 비밀 정보 차단 때문에 이 파일은 저장소에 올리지 않는다 (GitHub Actions 는 시크릿에서 만든다).
fn main() {
    println!("cargo:rerun-if-env-changed=SAI_TEAM_AI_ENDPOINT");
    if let Ok(endpoint) = std::env::var("SAI_TEAM_AI_ENDPOINT") {
        if !endpoint.is_empty() {
            let host = endpoint
                .strip_prefix("https://")
                .unwrap_or("")
                .trim_end_matches('/');
            assert!(
                endpoint.len() <= 2048
                    && host.contains('.')
                    && !host.is_empty()
                    && host
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b)),
                "SAI_TEAM_AI_ENDPOINT에는 비밀 값 없이 HTTPS 서비스 주소만 넣어 주세요."
            );
        }
    }
    let path = std::path::Path::new("../config/google-client.json");
    println!("cargo:rerun-if-changed=../config/google-client.json");
    if let Ok(raw) = std::fs::read_to_string(path) {
        let one_line: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        println!("cargo:rustc-env=SAI_GOOGLE_CLIENT={one_line}");
    }
    tauri_build::build()
}
