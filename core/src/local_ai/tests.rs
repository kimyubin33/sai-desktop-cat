use super::character::{clean, prompt, Moment, Trigger};
use super::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Write as _;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("sai-local-ai-test-{}", crate::google::oauth::random_token()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        assert!(self.0.file_name().unwrap().to_string_lossy().starts_with("sai-local-ai-test-"));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

type Routes = Arc<HashMap<String, (u16, Vec<u8>)>>;
/// Minimal HTTP/1.1 server: serves fixed bodies, honours `Range: bytes=N-`, records request heads.
async fn serve(routes: Routes) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(vec![]));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let routes = routes.clone();
            let log = log.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16384];
                let mut n = 0;
                while !buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    let m = sock.read(&mut buf[n..]).await.unwrap_or(0);
                    if m == 0 {
                        return;
                    }
                    n += m;
                }
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                log.lock().unwrap().push(head.clone());
                let path = head.split_whitespace().nth(1).unwrap_or("").to_owned();
                let (status, body) = routes.get(&path).cloned().unwrap_or((404, b"{}".to_vec()));
                let start = head
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(|r| r.trim_end_matches('-').parse::<usize>().unwrap()));
                let (status, body) = match start {
                    Some(s) if status == 200 => (206, body[s..].to_vec()),
                    _ => (status, body),
                };
                let reply = format!("HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
                let _ = sock.write_all(reply.as_bytes()).await;
                let _ = sock.write_all(&body).await;
            });
        }
    });
    (base, seen)
}

fn runtime_zip(extra: Option<&str>) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("llama-server.exe", opts).unwrap();
    w.write_all(b"not really an exe").unwrap();
    w.start_file("ggml.dll", opts).unwrap();
    w.write_all(b"dll").unwrap();
    if let Some(name) = extra {
        w.start_file(name, opts).unwrap();
        w.write_all(b"evil").unwrap();
    }
    w.finish().unwrap().into_inner()
}
fn asset(name: &'static str, base: &str, path: &str, body: &[u8]) -> Asset {
    Asset {
        name,
        url: format!("{base}{path}"),
        sha256: Box::leak(format!("{:x}", Sha256::digest(body)).into_boxed_str()),
        size: body.len() as u64,
    }
}

#[tokio::test]
async fn install_verifies_resumes_and_unpacks_then_remove_cleans_up() {
    let temp = Temp::new();
    let zip = runtime_zip(None);
    let model: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let routes: Routes = Arc::new(HashMap::from([
        ("/rt.zip".to_owned(), (200, zip.clone())),
        ("/model.gguf".to_owned(), (200, model.clone())),
    ]));
    let (base, seen) = serve(routes).await;
    let rt = asset("rt.zip", &base, "/rt.zip", &zip);
    let md = asset("model.gguf", &base, "/model.gguf", &model);
    let settings = Store::memory(json!({}));
    let ai = LocalAi::with_assets(temp.0.clone(), settings.clone(), Some(rt.clone()), md.clone());
    assert!(!ai.installed());
    assert!(ai.set_enabled(true).is_err());

    // A previous attempt stopped half way: the rest is requested with Range, and the whole file is hashed.
    std::fs::create_dir_all(temp.0.join("models")).unwrap();
    std::fs::write(temp.0.join("models").join("model.gguf.part"), &model[..100_000]).unwrap();
    let events = Arc::new(Mutex::new(0));
    let counter = events.clone();
    ai.install(&move |_| *counter.lock().unwrap() += 1).await.unwrap();
    assert!(*events.lock().unwrap() > 0);
    assert!(ai.installed());
    assert!(seen.lock().unwrap().iter().any(|h| h.starts_with("GET /model.gguf") && h.to_ascii_lowercase().contains("range: bytes=100000-")));
    assert!(temp.0.join("runtime").join(rt.sha256).join("ggml.dll").is_file());
    // The runtime archive is not kept after unpacking.
    assert!(!temp.0.join("downloads").join("rt.zip").exists());

    // A second install is a no-op: nothing is downloaded again.
    let before = seen.lock().unwrap().len();
    ai.install(&|_| {}).await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), before);

    ai.set_enabled(true).unwrap();
    assert!(ai.ready());
    assert_eq!(ai.status()["installed"], true);
    ai.remove().await.unwrap();
    assert!(!ai.installed());
    assert!(!ai.enabled());
    assert!(!temp.0.join("models").exists());
}

#[tokio::test]
async fn tampered_downloads_and_unsafe_archives_are_rejected() {
    let temp = Temp::new();
    let good = runtime_zip(None);
    let evil = runtime_zip(Some("../escape.txt"));
    let routes: Routes = Arc::new(HashMap::from([
        ("/rt.zip".to_owned(), (200, b"tampered bytes".to_vec())),
        ("/evil.zip".to_owned(), (200, evil.clone())),
        ("/model.gguf".to_owned(), (200, b"model".to_vec())),
    ]));
    let (base, _) = serve(routes).await;
    // Pinned hash is for the good archive, server sends something else.
    let mut rt = asset("rt.zip", &base, "/rt.zip", &good);
    rt.size = b"tampered bytes".len() as u64;
    let md = asset("model.gguf", &base, "/model.gguf", b"model");
    let ai = LocalAi::with_assets(temp.0.clone(), Store::memory(json!({})), Some(rt), md.clone());
    assert!(ai.install(&|_| {}).await.is_err());
    assert!(!ai.installed());
    assert!(ai.status()["error"].is_string());
    assert!(!temp.0.join("downloads").join("rt.zip").exists());
    assert!(!temp.0.join("downloads").join("rt.zip.part").exists());

    // Zip-slip: an entry escaping the folder stops unpacking and leaves nothing behind.
    let evil_asset = asset("evil.zip", &base, "/evil.zip", &evil);
    let ai = LocalAi::with_assets(temp.0.join("b"), Store::memory(json!({})), Some(evil_asset), md);
    assert!(ai.install(&|_| {}).await.is_err());
    assert!(!temp.0.join("b").join("runtime").join("escape.txt").exists());
    assert!(!temp.0.join("escape.txt").exists());
    assert!(!ai.installed());
}

fn moment(trigger: Trigger) -> Moment {
    Moment {
        name: "사이".into(),
        now_ms: 1_763_445_600_000,
        trigger,
        theme: Some("물 한 잔 마시고 올래?".into()),
        recent: vec!["오늘도 천천히 가 보자.".into()],
    }
}

#[test]
fn prompt_carries_only_the_moment() {
    let (system, user) = prompt(&moment(Trigger::Returned(25)));
    assert!(system.contains("사이") && system.contains("반말"));
    assert!(user.contains("돌아왔어") && user.contains("물 한 잔") && user.contains("오늘도 천천히"));
}

#[test]
fn only_short_plain_korean_lines_survive() {
    let m = moment(Trigger::Timer);
    assert_eq!(clean("\"사이: 다시 왔구나. 잠깐 기지개 켜고 시작할까?\"\n\n(설명)", &m).as_deref(), Some("다시 왔구나. 잠깐 기지개 켜고 시작할까?"));
    assert_eq!(clean("사이: 물 한 잔 하고 오자", &m).as_deref(), Some("물 한 잔 하고 오자"));
    // A trailing emoji is dropped, not the line.
    assert_eq!(clean("어깨 한번 펴 보자 😊", &m).as_deref(), Some("어깨 한번 펴 보자"));
    // Near-repeats of recent lines are dropped.
    let mut r = moment(Trigger::Timer);
    r.recent = vec!["물 한 잔 마시고 어깨 한번 펴 보자.".into()];
    assert!(clean("물 한 잔 마시고 어깨 한번 펴 보자. 잠깐 쉬어 갈까?", &r).is_none());
    assert!(clean("제일 작은 일 하나만 먼저 끝내 볼까?", &r).is_some());
    for bad in [
        "",
        "응",
        "- 목록으로 말하면 안 돼",
        "🔥🔥",
        "고마워, 팀원이야 ٩(◕‿◕｡)۶ 아침 기분 잘 나왔어?",
        "팀원에게 고맙다고 말해 보는 건 어때요?",
        "야구 보고 있구나? 시간이 흘러가네",
        "밤새 조용히 놀았나봐?",
        "Let's take a short break together, okay?",
        "여기 들어가 봐 https://example.com",
        "오늘도 천천히 가 보자.",
        &"아주 긴 말이야 ".repeat(20),
    ] {
        assert!(clean(bad, &m).is_none(), "{bad}");
    }
}

#[tokio::test]
async fn completion_uses_the_session_key_and_reads_the_reply() {
    let reply = json!({"choices":[{"message":{"role":"assistant","content":"잠깐 쉬어 가자. 어깨 한 번 풀어 볼래?"}}]});
    let routes: Routes = Arc::new(HashMap::from([("/v1/chat/completions".to_owned(), (200, reply.to_string().into_bytes()))]));
    let (base, seen) = serve(routes).await;
    let temp = Temp::new();
    let ai = LocalAi::with_assets(temp.0.clone(), Store::memory(json!({})), None, model_asset());
    *ai.fake.lock().unwrap() = Some(Endpoint { base, key: "session-key".into() });
    let text = ai.complete("system", "user", 50).await.unwrap();
    assert_eq!(text, "잠깐 쉬어 가자. 어깨 한 번 풀어 볼래?");
    assert!(seen.lock().unwrap()[0].to_ascii_lowercase().contains("authorization: bearer session-key"));
    // Not enabled / not installed: the caller falls back to canned advice.
    assert!(ai.speak(&moment(Trigger::Asked)).await.is_err());
}

#[test]
fn pinned_assets_are_https_and_hashed() {
    for a in runtime_asset().into_iter().chain([model_asset()]) {
        assert!(a.url.starts_with("https://"));
        assert_eq!(a.sha256.len(), 64);
        assert!(a.size > 1_000_000);
    }
}

/// Runs the real runtime and model. Downloads ~2.5GB on first run into SAI_LIVE_DIR.
/// `SAI_LIVE_DIR=... cargo test -p sai-core live_model_speaks -- --ignored --nocapture`
#[tokio::test]
#[ignore]
async fn live_model_speaks() {
    let root = PathBuf::from(std::env::var("SAI_LIVE_DIR").expect("SAI_LIVE_DIR"));
    let ai = LocalAi::new(root, Store::memory(json!({})));
    let started = Instant::now();
    ai.install(&|v| {
        let p = &v["progress"];
        if p["total"].as_u64().unwrap_or(0) > 0 && p["done"].as_u64().unwrap_or(0) % (256 << 20) < (1 << 20) {
            eprintln!("progress {} {}/{}", p["phase"], p["done"], p["total"]);
        }
    })
    .await
    .unwrap();
    eprintln!("installed in {:?}; status {}", started.elapsed(), ai.status());
    ai.set_enabled(true).unwrap();
    let now = chrono::Local::now().timestamp_millis();
    let themes = [
        Some("물 한 잔 마시고 어깨 한번 펴 보자."),
        Some("오늘 할 일 중에 제일 작은 것부터 하나만 끝내 볼까?"),
        None,
        Some("팀원에게 고맙다는 말 한마디 건네 보는 건 어때?"),
    ];
    let triggers = [Trigger::Asked, Trigger::Timer, Trigger::Returned(25), Trigger::Returned(180)];
    let mut recent: Vec<String> = vec![];
    let (mut ok, mut total) = (0, 0);
    for round in 0..3 {
        for (i, trigger) in triggers.iter().enumerate() {
            let m = Moment {
                name: "사이".into(),
                now_ms: now + round * 3 * 3_600_000,
                trigger: trigger.clone(),
                theme: themes[i].map(str::to_owned),
                recent: recent.clone(),
            };
            let (system, user) = prompt(&m);
            let t = Instant::now();
            let raw = ai.complete(&system, &user, 120).await.unwrap();
            let line = clean(&raw, &m);
            total += 1;
            eprintln!("[{:?}] {:?}\n  raw: {:?}\n  -> {:?}", t.elapsed(), trigger, raw, line);
            if let Some(l) = line {
                ok += 1;
                recent.push(l);
            }
        }
    }
    eprintln!("accepted {ok}/{total}");
    ai.stop().await;
}
