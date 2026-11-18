//! 자동 업데이트: GitHub Releases 에 새 버전이 올라오면 설치 파일을 내려받아 두고, 고양이가 적용할지 묻는다.
//!  - 설치 파일(Sai-Setup-*.exe)의 SHA-256 을 릴리스의 SHA256SUMS.txt 와 맞춰 본 뒤에만 실행한다.
//!  - 설치 버전: 조용히 설치하고 다시 켠다.
//!  - 포터블(설치 없이 쓰던 exe): 설치 화면을 띄워 설치 버전으로 바꾼다.
//!  - "나중에"를 고르면 다음에 프로그램을 끌 때 적용된다.
mod release;

use crate::app::App;
use futures_util::StreamExt;
use sai_core::http::client;
use sai_core::time::now_ms;
use sai_core::util::J;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Emitter;

const REPO: &str = "kimyubin33/sai-desktop-cat";
const CHECK_EVERY: Duration = Duration::from_secs(3 * 60 * 60);

#[derive(Default)]
struct St {
    status: String, // idle | checking | downloading | ready | latest | error | unsupported
    latest: Option<String>,
    progress: u32,
    last_check: i64,
    error: Option<String>,
    pending: Option<Installer>,
}

struct Installer {
    path: PathBuf,
    sha256: String,
}

pub struct Updater {
    st: Mutex<St>,
    checking: Arc<tokio::sync::Mutex<()>>,
    pub current: String,
}

/// 이 컴퓨터의 CPU 가 ARM64 인지 (x64 판이 ARM 에서 에뮬레이션으로 돌고 있어도 참)
pub fn native_arm64() -> bool {
    if let Ok(v) = std::env::var("SAI_UPDATE_ARCH") {
        return v == "arm64";
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, IsWow64Process2};
        let (mut process, mut native) = (0u16, 0u16);
        if IsWow64Process2(GetCurrentProcess(), &mut process, &mut native) != 0 {
            return native == 0xAA64; // IMAGE_FILE_MACHINE_ARM64
        }
    }
    cfg!(target_arch = "aarch64")
}

pub fn supported() -> bool {
    if crate::preview::isolated() {
        return false;
    }
    // 개발용: SAI_UPDATE_TEST 가 있으면 다른 운영체제·개발 실행에서도 확인·내려받기까지 해 본다 (설치는 하지 않음)
    (cfg!(windows) && !cfg!(debug_assertions)) || std::env::var("SAI_UPDATE_TEST").is_ok()
}

impl Updater {
    pub fn new() -> Updater {
        let mut st = St {
            status: "idle".into(),
            ..Default::default()
        };
        if !supported() {
            st.status = "unsupported".into();
            st.error = Some(
                if crate::preview::isolated() {
                    "미리보기에서는 새 검증판을 직접 설치해 줘."
                } else {
                    "자동 업데이트는 Windows 설치 버전에서 쓸 수 있어."
                }
                .into(),
            );
        }
        let current = std::env::var("SAI_UPDATE_TEST")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
        Updater {
            st: Mutex::new(st),
            checking: Arc::new(tokio::sync::Mutex::new(())),
            current,
        }
    }

    pub fn state(&self) -> Value {
        let s = self.st.lock().unwrap();
        json!({ "current": self.current, "version": self.current, "status": s.status, "latest": s.latest, "progress": s.progress, "lastCheck": s.last_check, "error": s.error, "build": if cfg!(target_arch = "aarch64") { "ARM64" } else { "x64" }, "armPc": native_arm64() })
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

    fn publish(&self, a: &Arc<App>) {
        let _ = a.handle.emit_to("chat", "update:state", self.state());
        crate::menu::refresh_tray(&a.handle, a);
    }

    fn begin_check(&self) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        let guard = self.checking.clone().try_lock_owned().ok()?;
        self.set(|s| {
            if s.pending.is_none() {
                s.status = "checking".into();
            }
            s.error = None;
        });
        Some(guard)
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
        let Some(_guard) = self.begin_check() else {
            return self.state();
        };
        self.finish_check(a, manual).await
    }

    async fn finish_check(&self, a: &Arc<App>, manual: bool) -> Value {
        self.publish(a);
        match self.check_inner(a).await {
            Ok(None) => {
                self.set(|s| {
                    if s.pending.is_none() {
                        s.status = "latest".into();
                        s.latest = None;
                        s.progress = 0;
                    }
                    s.last_check = now_ms();
                });
                if manual {
                    let message = if self.is_ready() {
                        format!(
                            "새 버전 {}을 적용할 수 있어.",
                            self.latest().unwrap_or_default()
                        )
                    } else {
                        format!("지금 쓰는 {} 버전이 최신이야.", self.current)
                    };
                    a.advisor.info(a, &message);
                }
            }
            Ok(Some(v)) => {
                if manual {
                    a.advisor
                        .info(a, &format!("새 버전 {v} 다운로드가 끝났어."));
                }
                a.advisor.notify_update(a, &v);
            }
            Err(e) => {
                self.set(|s| {
                    s.status = if s.pending.is_some() {
                        "ready"
                    } else {
                        "error"
                    }
                    .into();
                    s.error = Some(e.clone());
                    s.progress = if s.pending.is_some() { 100 } else { 0 };
                    s.last_check = now_ms();
                });
                if manual {
                    a.advisor
                        .info(a, &format!("업데이트를 확인하지 못했어: {e}"));
                }
            }
        }
        self.publish(a);
        self.state()
    }

    /// 새 버전이 있으면 내려받아 확인까지 마치고 버전을 돌려준다
    async fn check_inner(&self, a: &Arc<App>) -> Result<Option<String>, String> {
        let installed_or_ready = self.latest().filter(|_| self.is_ready());
        let Some(latest) =
            release::latest_tag(installed_or_ready.as_deref().unwrap_or(&self.current)).await?
        else {
            return Ok(None);
        };
        let tag = format!("v{latest}");
        self.set(|s| s.last_check = now_ms());
        let base = format!("https://github.com/{REPO}/releases/download/{tag}");
        let sums_res = client()
            .get(format!("{base}/SHA256SUMS.txt"))
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| format!("확인용 파일을 받지 못했어: {e}"))?;
        if !sums_res.status().is_success() {
            return Err(format!(
                "새 버전에 확인용 파일(SHA256SUMS.txt)이 없어 (응답 {}).",
                sums_res.status()
            ));
        }
        let sums = sums_res.text().await.map_err(|e| e.to_string())?;
        let hash_of = |name: &str| {
            sums.lines().find_map(|l| {
                let mut p = l.split_whitespace();
                let h = p.next()?;
                let n = p.next()?.trim_start_matches('*');
                (n == name && h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
                    .then(|| h.to_ascii_lowercase())
            })
        };
        // ARM 컴퓨터(예: 스냅드래곤 노트북)는 ARM 전용 설치 파일을 받는다. 지금 x64 판이 에뮬레이션으로 돌고 있어도 넘어간다
        let x64_name = format!("Sai-Setup-{latest}.exe");
        let arm_name = format!("Sai-Setup-{latest}-arm64.exe");
        let (setup_name, want) = match (native_arm64(), hash_of(&arm_name)) {
            (true, Some(h)) => (arm_name, h),
            _ => (
                x64_name.clone(),
                hash_of(&x64_name).ok_or("확인용 파일에 설치 파일 정보가 없어.")?,
            ),
        };

        self.set(|s| {
            s.status = "downloading".into();
            s.progress = 0;
        });
        self.publish(a);
        let dir = a.data_dir.join("updates");
        std::fs::create_dir_all(&dir).map_err(|e| format!("업데이트 폴더를 만들지 못했어: {e}"))?;
        let path = dir.join(&setup_name);
        let res = client()
            .get(format!("{base}/{setup_name}"))
            .send()
            .await
            .map_err(|e| format!("설치 파일을 받지 못했어: {e}"))?;
        if !res.status().is_success() {
            return Err(format!("설치 파일을 받지 못했어 (응답 {}).", res.status()));
        }
        let total = res.content_length().unwrap_or(0);
        let mut stream = res.bytes_stream();
        let mut hasher = Sha256::new();
        let mut buf = Vec::new();
        loop {
            // 30초 동안 아무것도 오지 않으면 멈춘 것으로 본다
            let next = tokio::time::timeout(Duration::from_secs(30), stream.next())
                .await
                .map_err(|_| "30초 동안 내려받기가 멈춰서 중단했어. 다시 눌러 줘.".to_string())?;
            let Some(chunk) = next else { break };
            let chunk = chunk.map_err(|e| format!("내려받다가 끊겼어: {e}"))?;
            hasher.update(&chunk);
            buf.extend_from_slice(&chunk);
            let p = if total > 0 {
                (buf.len() as u64 * 100 / total).min(99) as u32
            } else {
                0
            };
            self.set(|s| s.progress = p);
        }
        let got = format!("{:x}", hasher.finalize());
        if got != want {
            return Err("내려받은 설치 파일이 확인용 값과 달라. 다시 눌러 줘.".into());
        }
        std::fs::write(&path, &buf).map_err(|e| format!("설치 파일을 저장하지 못했어: {e}"))?;
        self.set(|s| {
            s.status = "ready".into();
            s.progress = 100;
            s.latest = Some(latest.clone());
            s.pending = Some(Installer { path, sha256: want });
        });
        Ok(Some(latest))
    }

    /// 화면의 "업데이트 확인": 바로 돌아오고, 확인·내려받기는 뒤에서 한다.
    pub fn check_in_background(&self, a: &Arc<App>) -> Value {
        if supported() {
            if let Some(guard) = self.begin_check() {
                let state = self.state();
                let a2 = a.clone();
                tauri::async_runtime::spawn(async move {
                    let _guard = guard;
                    a2.updater.finish_check(&a2, false).await;
                });
                return state;
            }
        }
        self.state()
    }

    /// 설치 버전(설치 폴더에 제거 프로그램이 있음)인지
    fn installed() -> bool {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("uninstall.exe").exists()))
            .unwrap_or(false)
    }

    /// 받아 둔 설치 파일을 실행하고 사이를 끈다 (설치가 끝나면 다시 켠다)
    pub fn install(&self, a: &Arc<App>) {
        match self.start_installer(false) {
            Ok(true) => {
                a.stores.flush_all();
                a.handle.exit(0);
            }
            Ok(false) => {}
            Err(error) => {
                self.publish(a);
                a.advisor.info(a, &error);
            }
        }
    }

    /// 트레이에서 종료할 때 설치판에 준비된 업데이트를 적용한다.
    pub fn install_on_quit(&self) {
        if let Err(e) = self.start_installer(true) {
            eprintln!("{e}");
        }
    }

    fn start_installer(&self, quitting: bool) -> Result<bool, String> {
        if !supported() || std::env::var_os("SAI_UPDATE_TEST").is_some() {
            return Ok(false);
        }
        let installed = Self::installed();
        if quitting && !installed {
            return Ok(false);
        }
        let mut s = self.st.lock().unwrap();
        let Some(installer) = s.pending.take() else {
            return Ok(false);
        };
        let result = verify_installer(&installer)
            .and_then(|()| Self::spawn(&installer.path, installed, installed && !quitting));
        if let Err(error) = result {
            s.status = "error".into();
            s.error = Some(error.clone());
            s.latest = None;
            s.progress = 0;
            return Err(error);
        }
        Ok(true)
    }

    #[cfg(windows)]
    fn spawn(file: &std::path::Path, silent: bool, relaunch: bool) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        const DETACHED: u32 = 0x0000_0008 | 0x0800_0000;
        let args = if silent { "/S" } else { "" };
        let restart = if relaunch {
            r#" & if not errorlevel 1 start "" "%SAI_UPDATE_APP%""#
        } else {
            ""
        };
        let script = format!(
            r#"/D /V:OFF /S /C ""%SystemRoot%\System32\ping.exe" -n 4 127.0.0.1 >nul & start "" /wait "%SAI_UPDATE_FILE%" {args}{restart}""#
        );
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        std::process::Command::new("cmd.exe")
            .env("SAI_UPDATE_FILE", file)
            .env("SAI_UPDATE_APP", exe)
            .raw_arg(script)
            .creation_flags(DETACHED)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("설치 파일을 실행하지 못했어: {e}"))
    }

    #[cfg(not(windows))]
    fn spawn(_: &std::path::Path, _: bool, _: bool) -> Result<(), String> {
        Err("자동 설치는 Windows에서 쓸 수 있어.".into())
    }
}

fn verify_installer(installer: &Installer) -> Result<(), String> {
    let mut file = std::fs::File::open(&installer.path)
        .map_err(|e| format!("저장된 설치 파일을 열지 못했어: {e}"))?;
    let mut digest = Sha256::new();
    std::io::copy(&mut file, &mut digest)
        .map_err(|e| format!("설치 파일을 확인하지 못했어: {e}"))?;
    if format!("{:x}", digest.finalize()) != installer.sha256 {
        return Err("저장된 설치 파일이 확인용 값과 달라. 다시 받아 줘.".into());
    }
    Ok(())
}
