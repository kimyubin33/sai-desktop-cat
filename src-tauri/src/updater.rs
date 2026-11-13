//! 자동 업데이트: GitHub Releases 에 새 버전이 올라오면 설치 파일을 내려받아 두고, 고양이가 적용할지 묻는다.
//!  - 설치 파일(Sai-Setup-*.exe)의 SHA-256 을 릴리스의 SHA256SUMS.txt 와 맞춰 본 뒤에만 실행한다.
//!  - 설치 버전: 조용히 설치하고 다시 켠다.
//!  - 포터블(설치 없이 쓰던 exe): 설치 화면을 띄워 설치 버전으로 바꾼다.
//!  - "나중에"를 고르면 다음에 프로그램을 끌 때 적용된다.
use crate::app::App;
use futures_util::StreamExt;
use sai_core::http::client;
use sai_core::time::now_ms;
use sai_core::util::J;
use serde_json::{json, Value};
#[allow(unused_imports)]
use sai_core::util::J as _J;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const REPO: &str = "kimyubin33/sai-desktop-cat";
pub const RELEASES_URL: &str = "https://github.com/kimyubin33/sai-desktop-cat/releases/latest";
const CHECK_EVERY: Duration = Duration::from_secs(3 * 60 * 60);

#[derive(Default)]
struct St {
    status: String, // idle | checking | downloading | ready | latest | error | unsupported
    latest: Option<String>,
    progress: u32,
    last_check: i64,
    error: Option<String>,
    file: Option<PathBuf>,
}

pub struct Updater {
    st: Mutex<St>,
    pub current: String,
}

pub fn supported() -> bool {
    // 개발용: SAI_UPDATE_TEST 가 있으면 다른 운영체제·개발 실행에서도 확인·내려받기까지 해 본다 (설치는 하지 않음)
    (cfg!(windows) && !cfg!(debug_assertions)) || std::env::var("SAI_UPDATE_TEST").is_ok()
}

impl Updater {
    pub fn new() -> Updater {
        let mut st = St { status: "idle".into(), ..Default::default() };
        if !supported() {
            st.status = "unsupported".into();
            st.error = Some("이 실행 방식에서는 자동 업데이트를 하지 않아요 (Windows 설치 버전에서 동작).".into());
        }
        let current = std::env::var("SAI_UPDATE_TEST").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
        Updater { st: Mutex::new(st), current }
    }

    pub fn state(&self) -> Value {
        let s = self.st.lock().unwrap();
        json!({ "current": self.current, "version": self.current, "status": s.status, "latest": s.latest, "progress": s.progress, "lastCheck": s.last_check, "error": s.error })
    }

    pub fn is_ready(&self) -> bool {
        self.st.lock().unwrap().status == "ready"
    }

    pub fn latest(&self) -> Option<String> {
        self.st.lock().unwrap().latest.clone()
    }

    fn set(&self, f: impl FnOnce(&mut St)) {
        f(&mut self.st.lock().unwrap());
    }

    pub fn start(&self, a: &Arc<App>) {
        if !supported() {
            return;
        }
        let a2 = a.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(15)).await;
            loop {
                a2.updater.check(&a2, false).await;
                tokio::time::sleep(CHECK_EVERY).await;
            }
        });
    }

    pub async fn check(&self, a: &Arc<App>, manual: bool) -> Value {
        if !supported() {
            if manual {
                a.advisor.info(a, &self.state().s("error"));
            }
            return self.state();
        }
        if self.is_ready() || self.st.lock().unwrap().status == "downloading" {
            if manual {
                a.advisor.info(a, &format!("새 버전 {}을 이미 받고 있거나 받아 두었어요.", self.latest().unwrap_or_default()));
            }
            return self.state();
        }
        self.set(|s| {
            s.status = "checking".into();
            s.error = None;
        });
        match self.check_inner(a).await {
            Ok(None) => {
                self.set(|s| {
                    s.status = "latest".into();
                    s.last_check = now_ms();
                });
                if manual {
                    a.advisor.info(a, &format!("지금이 최신 버전({})이에요.", self.current));
                }
            }
            Ok(Some(v)) => {
                if manual {
                    a.advisor.info(a, &format!("새 버전 {v}을 받았어요."));
                }
                a.advisor.notify_update(a, &v);
            }
            Err(e) => {
                self.set(|s| {
                    s.status = "error".into();
                    s.error = Some(e.clone());
                    s.last_check = now_ms();
                });
                if manual {
                    a.advisor.info(a, &format!("업데이트를 확인하지 못했어요: {e}"));
                }
            }
        }
        self.state()
    }

    /// 새 버전이 있으면 내려받아 확인까지 마치고 버전을 돌려준다
    async fn check_inner(&self, _a: &Arc<App>) -> Result<Option<String>, String> {
        // GitHub API 는 시간당 호출 수 제한이 있어서, 최신 릴리스 주소가 넘어가는 곳(…/releases/tag/vX)으로 버전을 알아낸다
        let res = client()
            .get(RELEASES_URL)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("GitHub 에 연결하지 못했어요: {e}"))?;
        let final_url = res.url().to_string();
        let tag = final_url.rsplit("/tag/").next().filter(|t| *t != final_url).map(String::from);
        let Some(tag) = tag else { return Err(format!("최신 버전을 알아내지 못했어요 (응답 {})", res.status())) };
        let latest = tag.trim_start_matches('v').trim_end_matches('/').to_string();
        let newer = match (semver::Version::parse(&latest), semver::Version::parse(&self.current)) {
            (Ok(l), Ok(c)) => l > c,
            _ => false,
        };
        self.set(|s| s.last_check = now_ms());
        if !newer {
            return Ok(None);
        }
        let base = format!("https://github.com/{REPO}/releases/download/{tag}");
        let setup_name = format!("Sai-Setup-{latest}.exe");
        let sums_res = client()
            .get(format!("{base}/SHA256SUMS.txt"))
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| format!("확인용 파일을 받지 못했어요: {e}"))?;
        if !sums_res.status().is_success() {
            return Err(format!("새 버전에 확인용 파일(SHA256SUMS.txt)이 없어요 (응답 {}).", sums_res.status()));
        }
        let sums = sums_res.text().await.map_err(|e| e.to_string())?;
        let want = sums
            .lines()
            .find_map(|l| {
                let mut p = l.split_whitespace();
                let h = p.next()?;
                let n = p.next()?.trim_start_matches('*');
                (n == setup_name).then(|| h.to_lowercase())
            })
            .ok_or("확인용 파일에 설치 파일 정보가 없어요.")?;

        self.set(|s| {
            s.status = "downloading".into();
            s.latest = Some(latest.clone());
            s.progress = 0;
        });
        let dir = std::env::temp_dir().join("sai-update");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(&setup_name);
        let res = client()
            .get(format!("{base}/{setup_name}"))
            .send()
            .await
            .map_err(|e| format!("설치 파일을 받지 못했어요: {e}"))?;
        if !res.status().is_success() {
            return Err(format!("설치 파일을 받지 못했어요 (응답 {}).", res.status()));
        }
        let total = res.content_length().unwrap_or(0);
        let mut stream = res.bytes_stream();
        let mut hasher = Sha256::new();
        let mut buf: Vec<u8> = Vec::with_capacity(total as usize);
        loop {
            // 30초 동안 아무것도 오지 않으면 멈춘 것으로 본다
            let next = tokio::time::timeout(Duration::from_secs(30), stream.next()).await.map_err(|_| "내려받기가 30초 동안 멈춰 있어서 그만뒀어요. 다시 눌러 주세요.".to_string())?;
            let Some(chunk) = next else { break };
            let chunk = chunk.map_err(|e| format!("내려받다가 끊겼어요: {e}"))?;
            hasher.update(&chunk);
            buf.extend_from_slice(&chunk);
            let p = if total > 0 { (buf.len() as u64 * 100 / total).min(99) as u32 } else { 0 };
            self.set(|s| s.progress = p);
        }
        let got = format!("{:x}", hasher.finalize());
        if got != want {
            return Err("내려받은 설치 파일이 확인용 값과 달라요. 다시 눌러 주세요.".into());
        }
        std::fs::write(&path, &buf).map_err(|e| format!("설치 파일을 저장하지 못했어요: {e}"))?;
        self.set(|s| {
            s.status = "ready".into();
            s.progress = 100;
            s.file = Some(path);
        });
        Ok(Some(latest))
    }

    /// 화면의 "업데이트 확인": 바로 돌아오고, 확인·내려받기는 뒤에서 한다 (화면은 상태를 자주 읽어 % 를 보여 준다)
    pub fn check_in_background(&self, a: &Arc<App>) -> Value {
        if supported() {
            let busy = matches!(self.st.lock().unwrap().status.as_str(), "checking" | "downloading" | "ready");
            if !busy {
                self.set(|s| {
                    s.status = "checking".into();
                    s.error = None;
                });
                let a2 = a.clone();
                tauri::async_runtime::spawn(async move {
                    a2.updater.check(&a2, false).await;
                    crate::menu::refresh_tray(&a2.handle, &a2);
                });
            }
        }
        self.state()
    }

    /// 설치 버전(설치 폴더에 제거 프로그램이 있음)인지
    fn installed() -> bool {
        std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("uninstall.exe").exists())).unwrap_or(false)
    }

    /// 받아 둔 설치 파일을 실행하고 사이를 끈다 (설치가 끝나면 다시 켠다)
    pub fn install(&self, a: &Arc<App>) {
        let Some(file) = self.st.lock().unwrap().file.clone() else {
            let _ = open::that_detached(RELEASES_URL);
            return;
        };
        self.run_installer(&file);
        a.stores.flush_all();
        a.handle.exit(0);
    }

    /// 프로그램을 끌 때: 받아 둔 새 버전이 있으면 조용히 설치해 둔다 (다시 켜지는 않음)
    pub fn install_on_quit(&self) {
        if let Some(file) = self.st.lock().unwrap().file.clone() {
            if Self::installed() {
                self.spawn(&file, &["/S"], false);
            }
        }
    }

    fn run_installer(&self, file: &PathBuf) {
        if Self::installed() {
            self.spawn(file, &["/S"], true);
        } else {
            // 포터블로 쓰던 경우: 설치 화면을 띄운다
            self.spawn(file, &[], false);
        }
        // 파일은 다음 확인 때 다시 받는다
        self.set(|s| s.file = None);
    }

    #[allow(unused_variables)]
    fn spawn(&self, file: &PathBuf, args: &[&str], relaunch: bool) {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const DETACHED: u32 = 0x0000_0008 | 0x0800_0000; // DETACHED_PROCESS | CREATE_NO_WINDOW
            let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
            // 사이가 꺼질 시간을 조금 준 뒤 설치하고, 필요하면 다시 켠다
            let mut script = format!("timeout /t 2 /nobreak >nul & start \"\" /wait \"{}\" {}", file.display(), args.join(" "));
            if relaunch {
                script.push_str(&format!(" & start \"\" \"{exe}\""));
            }
            let _ = std::process::Command::new("cmd").raw_arg(format!("/S /C \"{script}\"")).creation_flags(DETACHED).spawn();
        }
    }
}
