//! 이 PC에서 도는 캐릭터 AI (llama.cpp `llama-server`).
//!  - 런타임과 모델은 처음 켤 때 내려받는다. 주소·크기·sha256은 아래에 고정하고, 검증을 통과한 파일만 쓴다.
//!  - 서버는 127.0.0.1의 임의 포트에서 실행 때마다 새로 만든 API 키로만 연다. 필요할 때 켜고, 쉬면 끈다.
//!  - 앱이 비정상 종료해도 Windows 작업 개체(Job Object)가 서버 프로세스를 함께 끝낸다.
//!  - 여기서 만든 말은 이 PC 밖으로 보내지 않는다.
pub mod character;
mod download;
#[cfg(test)]
mod tests;

use crate::store::Store;
use crate::util::J;
use crate::{err, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Pinned download. `sha256` is verified before the file is used.
#[derive(Clone, Debug)]
pub struct Asset {
    pub name: &'static str,
    pub url: String,
    pub sha256: &'static str,
    pub size: u64,
}

const RUNTIME_BUILD: &str = "b12503";
const RUNTIME_X64: (&str, &str, u64) = (
    "llama-b12503-bin-win-cpu-x64.zip",
    "64bee9b6fece8e66c665c40290cbb84706af23c670041d024b56a71a39859d77",
    25_521_102,
);
const RUNTIME_ARM64: (&str, &str, u64) = (
    "llama-b12503-bin-win-cpu-arm64.zip",
    "56df5e64f4763ff9a8ca5e1a5b54a5839b4becb99e0e2f1ba8ad80dbf50faf05",
    13_838_966,
);
pub const MODEL_LABEL: &str = "Qwen3 4B Instruct (Apache-2.0)";
const MODEL_FILE: &str = "Qwen3-4B-Instruct-2507-Q4_K_M.gguf";
const MODEL_SHA256: &str = "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597";
const MODEL_SIZE: u64 = 2_497_281_120;
/// Below this much physical memory the model would push other programs into swap.
const RECOMMENDED_MEMORY_GB: u64 = 8;
const IDLE_STOP: Duration = Duration::from_secs(10 * 60);
const START_TIMEOUT: Duration = Duration::from_secs(180);
const REPLY_TIMEOUT: Duration = Duration::from_secs(60);

pub fn runtime_asset() -> Option<Asset> {
    let (name, sha256, size) = if cfg!(all(windows, target_arch = "x86_64")) {
        RUNTIME_X64
    } else if cfg!(all(windows, target_arch = "aarch64")) {
        RUNTIME_ARM64
    } else {
        return None;
    };
    Some(Asset {
        name,
        url: format!("https://github.com/ggml-org/llama.cpp/releases/download/{RUNTIME_BUILD}/{name}"),
        sha256,
        size,
    })
}
pub fn model_asset() -> Asset {
    Asset {
        name: MODEL_FILE,
        url: format!("https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/main/{MODEL_FILE}"),
        sha256: MODEL_SHA256,
        size: MODEL_SIZE,
    }
}

/// A running `llama-server`. Dropping it ends the process.
struct Server {
    child: tokio::process::Child,
    base: String,
    key: String,
    last_used: Instant,
    #[cfg(windows)]
    _job: job::Job,
}
/// Where to send chat requests. Tests point this at a fake server.
#[derive(Clone)]
struct Endpoint {
    base: String,
    key: String,
}

#[derive(Clone, Default)]
struct Progress {
    phase: &'static str,
    done: u64,
    total: u64,
}

pub struct LocalAi {
    root: PathBuf,
    settings: Store,
    http: reqwest::Client,
    runtime: Option<Asset>,
    model: Asset,
    server: tokio::sync::Mutex<Option<Server>>,
    #[cfg(test)]
    fake: Mutex<Option<Endpoint>>,
    install_gate: tokio::sync::Mutex<()>,
    cancel: AtomicBool,
    progress: Mutex<Option<Progress>>,
    last_error: Mutex<Option<String>>,
}

impl LocalAi {
    pub fn new(root: PathBuf, settings: Store) -> Self {
        Self::with_assets(root, settings, runtime_asset(), model_asset())
    }
    pub(crate) fn with_assets(root: PathBuf, settings: Store, runtime: Option<Asset>, model: Asset) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                // GitHub and Hugging Face hand downloads to their CDNs. Never follow to plain HTTP.
                if attempt.previous().len() > 8 || attempt.url().scheme() != "https" {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .user_agent(concat!("sai-desktop-cat/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("local AI http client");
        Self {
            root,
            settings,
            http,
            runtime,
            model,
            server: tokio::sync::Mutex::new(None),
            #[cfg(test)]
            fake: Mutex::new(None),
            install_gate: tokio::sync::Mutex::new(()),
            cancel: AtomicBool::new(false),
            progress: Mutex::new(None),
            last_error: Mutex::new(None),
        }
    }

    fn runtime_dir(&self) -> Option<PathBuf> {
        self.runtime.as_ref().map(|r| self.root.join("runtime").join(r.sha256))
    }
    fn model_path(&self) -> PathBuf {
        self.root.join("models").join(self.model.name)
    }
    fn server_exe(&self) -> Option<PathBuf> {
        let exe = self.runtime_dir()?.join("llama-server.exe");
        exe.is_file().then_some(exe)
    }
    pub fn installed(&self) -> bool {
        self.server_exe().is_some() && download::verified(&self.model_path(), &self.model)
    }
    pub fn enabled(&self) -> bool {
        self.settings.read(|s| s["localAi"].b("enabled"))
    }
    /// Installed, switched on, and not mid-install.
    pub fn ready(&self) -> bool {
        self.enabled() && self.installed() && self.install_gate.try_lock().is_ok()
    }

    pub fn status(&self) -> Value {
        let progress = self.progress.lock().unwrap().clone();
        let memory = memory_gb();
        let remaining = self.runtime.as_ref().map(|r| r.size).unwrap_or(0) + self.model.size;
        json!({
            "supported": self.runtime.is_some(),
            "installed": self.installed(),
            "enabled": self.enabled(),
            "installing": progress.is_some(),
            "progress": progress.map(|p| json!({"phase":p.phase,"done":p.done,"total":p.total})),
            "error": self.last_error.lock().unwrap().clone(),
            "model": MODEL_LABEL,
            "downloadBytes": remaining,
            "memoryGb": memory,
            "lowMemory": memory.is_some_and(|m| m < RECOMMENDED_MEMORY_GB),
        })
    }

    pub fn set_enabled(&self, on: bool) -> Result<()> {
        if on && !self.installed() {
            return err("먼저 캐릭터 AI를 받아 주세요.");
        }
        self.settings.update(|s| {
            if !s["localAi"].is_object() {
                s["localAi"] = json!({});
            }
            s["localAi"]["enabled"] = json!(on);
        });
        Ok(())
    }

    pub fn cancel_install(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Downloads, verifies and unpacks the runtime and model. `notify` is called as progress changes.
    pub async fn install(&self, notify: &(dyn Fn(Value) + Send + Sync)) -> Result<()> {
        let _gate = self
            .install_gate
            .try_lock()
            .map_err(|_| "캐릭터 AI를 이미 받고 있어요.")?;
        let Some(runtime) = self.runtime.clone() else {
            return err("이 PC에서는 캐릭터 AI를 아직 쓸 수 없어요.");
        };
        self.cancel.store(false, Ordering::SeqCst);
        *self.last_error.lock().unwrap() = None;
        let result = self.install_inner(&runtime, notify).await;
        *self.progress.lock().unwrap() = None;
        if let Err(e) = &result {
            *self.last_error.lock().unwrap() = Some(e.clone());
        }
        notify(self.status());
        result
    }
    async fn install_inner(&self, runtime: &Asset, notify: &(dyn Fn(Value) + Send + Sync)) -> Result<()> {
        // Stop a running server before files underneath it could change.
        self.stop().await;
        let report = |phase: &'static str, done: u64, total: u64| {
            *self.progress.lock().unwrap() = Some(Progress { phase, done, total });
            notify(self.status());
        };
        let dir = self.runtime_dir().unwrap();
        if !dir.join("llama-server.exe").is_file() {
            let zip = self.root.join("downloads").join(runtime.name);
            download::fetch(&self.http, runtime, &zip, &|d, t| report("runtime", d, t), &self.cancel).await?;
            report("extract", 0, 1);
            download::extract(&zip, &dir).await?;
            let _ = tokio::fs::remove_file(&zip).await;
            let _ = tokio::fs::remove_file(download::marker(&zip)).await;
        }
        download::fetch(&self.http, &self.model, &self.model_path(), &|d, t| report("model", d, t), &self.cancel).await?;
        Ok(())
    }

    /// Deletes the runtime and model from this PC and turns the feature off.
    pub async fn remove(&self) -> Result<()> {
        let _gate = self
            .install_gate
            .try_lock()
            .map_err(|_| "받는 중에는 지울 수 없어요. 먼저 취소해 주세요.")?;
        self.stop().await;
        self.set_enabled(false)?;
        for sub in ["runtime", "models", "downloads"] {
            let path = self.root.join(sub);
            if path.exists() {
                tokio::fs::remove_dir_all(&path)
                    .await
                    .map_err(|_| "캐릭터 AI 파일을 지우지 못했어요. 앱을 다시 켠 뒤 시도해 주세요.")?;
            }
        }
        Ok(())
    }

    pub async fn stop(&self) {
        if let Some(mut server) = self.server.lock().await.take() {
            let _ = server.child.kill().await;
        }
    }
    /// Ends the server after it has been unused for a while, returning its memory to the PC.
    pub async fn reap_idle(&self) {
        let mut guard = self.server.lock().await;
        let exited = guard
            .as_mut()
            .is_some_and(|s| !matches!(s.child.try_wait(), Ok(None)));
        if exited || guard.as_ref().is_some_and(|s| s.last_used.elapsed() > IDLE_STOP) {
            if let Some(mut server) = guard.take() {
                let _ = server.child.kill().await;
            }
        }
    }

    async fn endpoint(&self) -> Result<Endpoint> {
        #[cfg(test)]
        if let Some(fake) = self.fake.lock().unwrap().clone() {
            return Ok(fake);
        }
        if !self.installed() {
            return err("캐릭터 AI가 아직 준비되지 않았어요.");
        }
        let mut guard = self.server.lock().await;
        if let Some(server) = guard.as_mut() {
            if matches!(server.child.try_wait(), Ok(None)) {
                server.last_used = Instant::now();
                return Ok(Endpoint { base: server.base.clone(), key: server.key.clone() });
            }
            *guard = None;
        }
        let server = self.spawn().await?;
        let endpoint = Endpoint { base: server.base.clone(), key: server.key.clone() };
        *guard = Some(server);
        Ok(endpoint)
    }

    async fn spawn(&self) -> Result<Server> {
        let exe = self.server_exe().ok_or("캐릭터 AI 실행 파일을 찾지 못했어요.")?;
        let port = {
            let probe = std::net::TcpListener::bind("127.0.0.1:0").map_err(|_| "캐릭터 AI가 쓸 포트를 찾지 못했어요.")?;
            probe.local_addr().map_err(|_| "캐릭터 AI가 쓸 포트를 찾지 못했어요.")?.port()
        };
        let key = crate::google::oauth::random_token();
        // Leave cores for the user's own work; the character is never urgent.
        let threads = std::thread::available_parallelism().map(|n| (n.get() / 2).clamp(1, 8)).unwrap_or(2);
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.current_dir(exe.parent().unwrap())
            .arg("-m")
            .arg(self.model_path())
            .args(["--host", "127.0.0.1", "--port", &port.to_string(), "--api-key", &key])
            .args(["-c", "4096", "-np", "1", "-t", &threads.to_string(), "--no-webui"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
            cmd.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
        }
        let mut child = cmd.spawn().map_err(|_| "캐릭터 AI를 시작하지 못했어요.")?;
        #[cfg(windows)]
        let job = job::Job::kill_on_close(&child)?;
        // Keep only the tail of the server log, to explain a failed start without storing everything.
        let log = std::sync::Arc::new(Mutex::new(String::new()));
        if let Some(stderr) = child.stderr.take() {
            let log = log.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut l = log.lock().unwrap();
                    l.push_str(&line);
                    l.push('\n');
                    if l.len() > 4000 {
                        let cut = l.len() - 2000;
                        let cut = (cut..l.len()).find(|i| l.is_char_boundary(*i)).unwrap_or(l.len());
                        l.drain(..cut);
                    }
                }
            });
        }
        let base = format!("http://127.0.0.1:{port}");
        let started = Instant::now();
        loop {
            if let Ok(Some(_)) = child.try_wait() {
                let tail = log.lock().unwrap().lines().rev().take(3).collect::<Vec<_>>().join(" / ");
                return Err(format!("캐릭터 AI가 시작 중에 멈췄어요. {tail}"));
            }
            let health = self.http.get(format!("{base}/health")).bearer_auth(&key).timeout(Duration::from_secs(3)).send().await;
            if health.is_ok_and(|r| r.status().is_success()) {
                break;
            }
            if started.elapsed() > START_TIMEOUT {
                let _ = child.kill().await;
                return err("캐릭터 AI가 3분 안에 준비되지 않았어요. PC 메모리가 부족할 수 있어요.");
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Ok(Server {
            child,
            base,
            key,
            last_used: Instant::now(),
            #[cfg(windows)]
            _job: job,
        })
    }

    /// One short completion from the local model.
    pub async fn complete(&self, system: &str, user: &str, max_tokens: u32) -> Result<String> {
        let ep = self.endpoint().await?;
        let body = json!({
            "messages": [{"role":"system","content":system},{"role":"user","content":user}],
            "max_tokens": max_tokens,
            "temperature": 0.6,
            "top_p": 0.9,
            "stream": false,
        });
        let res = self
            .http
            .post(format!("{}/v1/chat/completions", ep.base))
            .bearer_auth(&ep.key)
            .json(&body)
            .timeout(REPLY_TIMEOUT)
            .send()
            .await
            .map_err(|_| "캐릭터 AI가 제때 답하지 않았어요.")?;
        if !res.status().is_success() {
            return Err(format!("캐릭터 AI가 답하지 못했어요 ({}).", res.status().as_u16()));
        }
        let value: Value = res.json().await.map_err(|_| "캐릭터 AI 응답을 읽지 못했어요.")?;
        if let Some(server) = self.server.lock().await.as_mut() {
            server.last_used = Instant::now();
        }
        Ok(value["choices"][0]["message"].s("content"))
    }

    /// A line for the character to say now, or an error when the caller should fall back to canned advice.
    pub async fn speak(&self, moment: &character::Moment) -> Result<String> {
        if !self.ready() {
            return err("캐릭터 AI가 꺼져 있어요.");
        }
        let (system, user) = character::prompt(moment);
        // Sampling varies, so one more try usually yields a usable line.
        for _ in 0..2 {
            let raw = self.complete(&system, &user, 120).await?;
            if let Some(line) = character::clean(&raw, moment) {
                return Ok(line);
            }
        }
        err("캐릭터 AI가 쓸 수 없는 말을 만들었어요.")
    }
}

#[cfg(windows)]
fn memory_gb() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    (unsafe { GlobalMemoryStatusEx(&mut m) } != 0).then(|| (m.ullTotalPhys + (1 << 29)) >> 30)
}
#[cfg(not(windows))]
fn memory_gb() -> Option<u64> {
    None
}

#[cfg(windows)]
mod job {
    use crate::Result;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    /// Closing the last handle to this job ends every process in it, including after a crash of the app.
    pub struct Job(HANDLE);
    // The handle is only closed once, on drop.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}
    impl Job {
        pub fn kill_on_close(child: &tokio::process::Child) -> Result<Self> {
            let process = child.raw_handle().ok_or("캐릭터 AI 프로세스를 확인하지 못했어요.")?;
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err("캐릭터 AI 프로세스를 관리하지 못했어요.".into());
                }
                let job = Job(handle);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) != 0
                    && AssignProcessToJobObject(job.0, process as HANDLE) != 0;
                if !ok {
                    return Err("캐릭터 AI 프로세스를 관리하지 못했어요.".into());
                }
                Ok(job)
            }
        }
    }
    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}
