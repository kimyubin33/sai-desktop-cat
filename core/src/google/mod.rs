//! Google 캘린더·할 일 연결: 로그인 창 열기, 빈 시간 계산, 할 일 동기화, 시간표 일정 반영.
mod agenda;
pub mod calendar;
mod collab;
pub mod drive;
mod files;
mod login;
pub mod oauth;
mod profile;
mod review;
mod team_ai;
mod teams;
pub use files::{
    safe_name as safe_download_name, Download as DownloadedFile, Progress as FileProgress,
    MAX_BYTES as MAX_FILE_BYTES,
};
pub mod tasks;

use crate::schedule::time_plan::Interval;
use crate::store::Store;
use crate::team::Hub;
use crate::time::{now_ms, DAY};
use crate::util::J;
use crate::{err, Result};
use oauth::{parse_google_client, GoogleAuth};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

pub type OpenUrl = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct Google(Arc<Inner>);

struct Inner {
    settings: Store,
    team_cache: Store,
    hub: Hub,
    builtin: Option<Value>,
    auth: Mutex<Option<Arc<GoogleAuth>>>,
    open_url: OpenUrl,
    login: login::Login,
    profiles: profile::Profiles,
    teams: teams::Teams,
    files: files::Files,
    collab: collab::Collaboration,
    reviews: review::Reviews,
    agenda: agenda::Agenda,
}

impl Google {
    pub async fn calendar_preview(&self, epoch: u64, folder: &str, task: &str) -> Result<Value> {
        let auth = self.auth()?;
        self.0
            .agenda
            .preview(
                &auth,
                &self.0.profiles,
                &self.0.teams,
                &self.0.collab,
                epoch,
                folder,
                task,
            )
            .await
    }
    pub async fn connect_calendar(&self, epoch: u64) -> Result<Value> {
        self.0
            .login
            .run_for(
                self.auth()?,
                self.0.open_url.clone(),
                oauth::Purpose::Calendar,
                Some(epoch),
            )
            .await?;
        let auth = self.auth()?;
        let lease = auth.lease().await?;
        auth.with_lease(&lease, || {
            if lease.epoch() == epoch {
                Ok(json!({"epoch":epoch,"connected":auth.calendar_connected()}))
            } else {
                err("계정 연결이 바뀌었어요.")
            }
        })
        .await
    }
    pub async fn add_to_calendar(&self, epoch: u64, folder: &str, value: Value) -> Result<Value> {
        let add =
            serde_json::from_value(value).map_err(|_| "등록할 할 일과 날짜를 확인해 주세요.")?;
        let auth = self.auth()?;
        agenda::add(
            &auth,
            &self.0.profiles,
            &self.0.teams,
            &self.0.collab,
            epoch,
            folder,
            add,
        )
        .await
    }
    pub async fn change_calendar(&self, epoch: u64, folder: &str, value: Value) -> Result<Value> {
        let change = serde_json::from_value(value)
            .map_err(|_| "수정·삭제할 일정의 확인 정보를 확인해 주세요.")?;
        let auth = self.auth()?;
        self.0
            .agenda
            .change(
                &auth,
                &self.0.profiles,
                &self.0.teams,
                &self.0.collab,
                epoch,
                folder,
                change,
            )
            .await
    }
    pub async fn check_calendar_change(
        &self,
        epoch: u64,
        folder: &str,
        review: &str,
    ) -> Result<Value> {
        let auth = self.auth()?;
        self.0
            .agenda
            .check(
                &auth,
                &self.0.profiles,
                &self.0.teams,
                epoch,
                folder,
                review,
            )
            .await
    }
    pub fn forget_calendar(&self, epoch: u64, folder: &str, review: &str) {
        self.0.agenda.forget(epoch, folder, review);
    }
    pub async fn prepare_ai_review(&self, epoch: u64, folder: &str, input: Value) -> Result<Value> {
        let input =
            serde_json::from_value(input).map_err(|_| "분석할 파일이나 텍스트를 확인해 주세요.")?;
        let auth = self.auth()?;
        self.0
            .reviews
            .prepare(
                &auth,
                &self.0.profiles,
                &self.0.teams,
                &self.0.files,
                &self.0.collab,
                epoch,
                folder,
                input,
            )
            .await
    }
    pub async fn run_ai_review(
        &self,
        epoch: u64,
        folder: &str,
        review: &str,
        confirmed: bool,
        endpoint: Option<&str>,
    ) -> Result<Value> {
        let auth = self.auth()?;
        self.0
            .reviews
            .run(
                &auth,
                &self.0.profiles,
                &self.0.teams,
                &self.0.files,
                &self.0.collab,
                epoch,
                folder,
                review,
                confirmed,
                endpoint,
            )
            .await
    }
    pub async fn commit_ai_review(
        &self,
        epoch: u64,
        folder: &str,
        review: &str,
        choices: Value,
        confirmed: bool,
    ) -> Result<Value> {
        let choices =
            serde_json::from_value(choices).map_err(|_| "검토한 할 일 선택을 확인해 주세요.")?;
        let auth = self.auth()?;
        self.0
            .reviews
            .commit(
                &auth,
                &self.0.profiles,
                &self.0.teams,
                &self.0.files,
                &self.0.collab,
                epoch,
                folder,
                review,
                choices,
                confirmed,
            )
            .await
    }
    pub fn forget_ai_review(&self, epoch: u64, folder: &str, review: &str) {
        self.0.reviews.forget(epoch, folder, review);
    }
    pub async fn connect_team_ai(
        &self,
        epoch: u64,
        folder: &str,
        endpoint: Option<&str>,
    ) -> Result<Value> {
        let auth = self.auth()?;
        team_ai::Client::new(endpoint)?
            .connect(&auth, &self.0.profiles, &self.0.teams, epoch, folder)
            .await
    }
    /// Native integration only. Frontend analysis must use the explicit review session flow.
    pub async fn analyze_team_text(
        &self,
        epoch: u64,
        folder: &str,
        endpoint: Option<&str>,
        attempt: Value,
    ) -> Result<Value> {
        let attempt: team_ai::Attempt = serde_json::from_value(attempt)
            .map_err(|_| "분석할 내용과 요청 정보를 확인해 주세요.")?;
        let auth = self.auth()?;
        team_ai::Client::new(endpoint)?
            .analyze(
                &auth,
                &self.0.profiles,
                &self.0.teams,
                epoch,
                folder,
                &attempt,
            )
            .await
    }
    pub async fn team_ai_status(
        &self,
        epoch: u64,
        folder: &str,
        endpoint: Option<&str>,
    ) -> Result<Value> {
        let auth = self.auth()?;
        team_ai::Client::new(endpoint)?
            .status(&auth, &self.0.profiles, &self.0.teams, epoch, folder)
            .await
    }
    pub async fn collaboration(&self, epoch: u64, folder: &str, sync: bool) -> Result<Value> {
        let auth = self.auth()?;
        self.0
            .collab
            .load(&auth, &self.0.profiles, &self.0.teams, epoch, folder, sync)
            .await
    }
    pub async fn save_collaboration(&self, epoch: u64, folder: &str, edit: Value) -> Result<Value> {
        let auth = self.auth()?;
        self.0
            .collab
            .save(&auth, &self.0.profiles, &self.0.teams, epoch, folder, edit)
            .await
    }
    /// builtin: 설치 파일에 함께 들어간 OAuth 클라이언트 { clientId, clientSecret }
    pub fn new(
        settings: Store,
        team_cache: Store,
        hub: Hub,
        builtin: Option<Value>,
        open_url: OpenUrl,
        profile_dir: std::path::PathBuf,
    ) -> Google {
        // 앱에 들어 있던 연결 정보를 이 컴퓨터의 설정에도 남겨 둔다.
        // 나중에 연결 정보가 빠진 버전으로 업데이트되더라도 Google 연결이 끊기지 않게 하기 위해서다.
        if let Some(b) = &builtin {
            if settings.read(|s| s["googleClient"].is_null()) {
                let b = b.clone();
                settings.update(|s| s["googleClient"] = b);
            }
        }
        let files_dir = profile_dir.join("file-transfers-v1");
        let collab_dir = profile_dir.join("collaboration-v1");
        let reviews_dir = profile_dir.join("ai-reviews-v1");
        Google(Arc::new(Inner {
            settings,
            team_cache,
            hub,
            builtin,
            auth: Mutex::new(None),
            open_url,
            login: login::Login::default(),
            profiles: profile::Profiles::new(profile_dir),
            teams: teams::Teams::default(),
            files: files::Files::new(files_dir),
            collab: collab::Collaboration::new(collab_dir),
            reviews: review::Reviews::new(reviews_dir),
            agenda: agenda::Agenda::default(),
        }))
    }

    fn client(&self) -> Option<Value> {
        let c = self.0.settings.read(|s| s["googleClient"].clone());
        if c.is_object() && !c.s("clientId").is_empty() {
            Some(c)
        } else {
            self.0.builtin.clone()
        }
    }

    fn auth(&self) -> Result<Arc<GoogleAuth>> {
        let Some(c) = self.client() else {
            return err("이 빌드의 Google 연결을 준비하고 있어요. 연결 정보가 포함된 앱으로 업데이트해 주세요.");
        };
        let mut g = self.0.auth.lock().unwrap();
        if g.as_ref()
            .map(|a| a.client_id != c.s("clientId"))
            .unwrap_or(true)
        {
            *g = Some(Arc::new(GoogleAuth::new(&c, self.0.settings.clone())));
        }
        Ok(g.clone().unwrap())
    }

    fn record(&self, patch: Value) {
        self.0.team_cache.update(|c| {
            if !c["google"].is_object() {
                c["google"] = json!({});
            }
            if let Some(o) = patch.as_object() {
                for (k, v) in o {
                    c["google"][k] = v.clone();
                }
            }
        });
    }

    fn fail<T>(&self, e: String) -> Result<T> {
        self.record(json!({ "lastError": e, "lastErrorAt": now_ms() }));
        Err(e)
    }

    pub fn is_ready(&self) -> bool {
        self.client().is_some() && self.auth().map(|a| a.is_signed_in()).unwrap_or(false)
    }

    // ---------- 로그인 ----------
    pub async fn login(&self) -> Result<Value> {
        let auth = self.auth()?;
        if auth.has_session() && auth.has_full_drive() {
            return Ok(json!({"email":auth.verified_identity().await?.email}));
        }
        let result = self
            .0
            .login
            .run(self.auth()?, self.0.open_url.clone())
            .await;
        if result.is_ok() {
            self.record(json!({ "lastError": null }));
        }
        result
    }

    pub async fn cancel_login(&self) {
        self.0.login.cancel();
        let _guard = self.0.login.idle().await;
    }

    pub async fn resume(&self) -> Result<()> {
        // 계정 신원 확인만 수행한다. 이전 Firebase 팀 백업은 읽거나 갱신하지 않는다.
        self.auth()?.verified_identity().await?;
        Ok(())
    }

    pub fn profile_status(&self) -> Value {
        let Ok(auth) = self.auth() else {
            return json!({"epoch":0,"signedIn":false,"ready":false});
        };
        let (epoch, active) = auth.active_context();
        match active {
            Some(identity) => {
                let mut v = self.0.profiles.status(&identity.subject, epoch);
                v["signedIn"] = json!(true);
                v
            }
            None => json!({"epoch":epoch,"signedIn":false,"ready":false}),
        }
    }
    pub async fn sync_profile(&self) -> Result<()> {
        let auth = self.auth()?;
        self.0.profiles.sync(&auth, &self.0.settings).await
    }
    pub async fn update_profile(&self, epoch: u64, patch: Value) -> Result<()> {
        let auth = self.auth()?;
        self.0
            .profiles
            .update(&auth, &self.0.settings, epoch, patch)
            .await
    }
    pub async fn team_status(&self) -> Result<Value> {
        let auth = self.auth()?;
        let (epoch, active) = auth.active_context();
        if active.is_none() {
            return Ok(json!({"epoch":epoch,"signedIn":false,"ready":false,"teams":[]}));
        }
        self.0.teams.view(&auth, &self.0.profiles).await
    }
    pub async fn enable_team_drive(&self, epoch: u64) -> Result<()> {
        self.0
            .login
            .run_for(
                self.auth()?,
                self.0.open_url.clone(),
                oauth::Purpose::Files,
                Some(epoch),
            )
            .await?;
        Ok(())
    }
    pub async fn create_team_folder(&self, epoch: u64, name: &str) -> Result<Value> {
        let auth = self.auth()?;
        self.0
            .teams
            .create(&auth, &self.0.profiles, epoch, name)
            .await
    }
    pub async fn pick_team_folder(&self, epoch: u64) -> Result<Value> {
        let auth = self.auth()?;
        let selected = self
            .0
            .login
            .run_for(
                auth.clone(),
                self.0.open_url.clone(),
                oauth::Purpose::Folder,
                Some(epoch),
            )
            .await?;
        self.0
            .teams
            .join(&auth, &self.0.profiles, epoch, &selected.s("folderId"))
            .await
    }
    pub async fn invite_team_folder(&self, epoch: u64, folder: &str, email: &str) -> Result<Value> {
        let auth = self.auth()?;
        self.0
            .teams
            .invite(&auth, &self.0.profiles, epoch, folder, email)
            .await
    }
    pub async fn open_team_folder(&self, epoch: u64, folder: &str) -> Result<String> {
        let auth = self.auth()?;
        self.0
            .teams
            .open(&auth, &self.0.profiles, epoch, folder)
            .await
    }
    pub async fn remove_team_folder(&self, epoch: u64, folder: &str) -> Result<()> {
        let auth = self.auth()?;
        let lease = auth.lease().await?;
        if lease.epoch() != epoch {
            return err("계정 연결이 바뀌었어요.");
        }
        if self.0.files.has_pending(lease.subject(), folder)? {
            return err("이 팀에 전송 대기 파일이 있어요. 파일 보기에서 먼저 다시 올리거나 대기 사본을 지워 주세요.");
        }
        self.0
            .collab
            .remove_team(&auth, &self.0.profiles, &self.0.teams, epoch, folder)
            .await
    }
    pub async fn list_team_files(&self, epoch: u64, folder: &str, page: &str) -> Result<Value> {
        let auth = self.auth()?;
        self.0
            .files
            .list(&auth, &self.0.profiles, &self.0.teams, epoch, folder, page)
            .await
    }
    pub async fn team_file_info(&self, epoch: u64, folder: &str, file: &str) -> Result<Value> {
        let auth = self.auth()?;
        self.0
            .files
            .info(&auth, &self.0.profiles, &self.0.teams, epoch, folder, file)
            .await
    }
    pub async fn check_file_upload(&self, epoch: u64, folder: &str) -> Result<()> {
        let auth = self.auth()?;
        let (_, team) = self
            .0
            .teams
            .access(&auth, &self.0.profiles, epoch, folder)
            .await?;
        if team["canUpload"] != true {
            return err("이 팀 폴더에는 파일을 올릴 권한이 없어요.");
        }
        Ok(())
    }
    pub async fn upload_team_file(
        &self,
        epoch: u64,
        folder: &str,
        name: &str,
        bytes: &[u8],
        progress: FileProgress,
    ) -> Result<Value> {
        let auth = self.auth()?;
        let operation = self
            .0
            .files
            .stage(
                &auth,
                &self.0.profiles,
                &self.0.teams,
                epoch,
                folder,
                name,
                bytes,
            )
            .await?;
        self.0
            .files
            .retry(
                &auth,
                &self.0.profiles,
                &self.0.teams,
                epoch,
                &operation,
                progress,
            )
            .await
    }
    pub async fn retry_team_file(
        &self,
        epoch: u64,
        folder: &str,
        operation: &str,
        progress: FileProgress,
    ) -> Result<Value> {
        let auth = self.auth()?;
        self.0
            .files
            .check_pending_target(&auth, epoch, folder, operation)
            .await?;
        self.0
            .files
            .retry(
                &auth,
                &self.0.profiles,
                &self.0.teams,
                epoch,
                operation,
                progress,
            )
            .await
    }
    pub async fn discard_team_upload(
        &self,
        epoch: u64,
        folder: &str,
        operation: &str,
    ) -> Result<()> {
        let auth = self.auth()?;
        self.0
            .files
            .check_pending_target(&auth, epoch, folder, operation)
            .await?;
        self.0.files.discard(&auth, epoch, operation).await
    }
    pub async fn pick_team_files(&self, epoch: u64, folder: &str) -> Result<Value> {
        let auth = self.auth()?;
        self.0
            .teams
            .access(&auth, &self.0.profiles, epoch, folder)
            .await?;
        let chosen = self
            .0
            .login
            .run_for(
                auth.clone(),
                self.0.open_url.clone(),
                oauth::Purpose::SelectedFiles,
                Some(epoch),
            )
            .await?;
        let ids: Vec<String> = chosen["fileIds"]
            .as_array()
            .ok_or("선택한 파일을 확인하지 못했어요.")?
            .iter()
            .filter_map(|id| id.as_str().map(str::to_owned))
            .collect();
        self.0
            .files
            .selected(&auth, &self.0.profiles, &self.0.teams, epoch, folder, &ids)
            .await
    }
    pub async fn browse_drive_files(
        &self,
        epoch: u64,
        mode: &str,
        parent: &str,
        query: &str,
        page: &str,
    ) -> Result<Value> {
        let auth = self.auth()?;
        self.0
            .files
            .browse(&auth, epoch, mode, parent, query, page)
            .await
    }
    pub async fn browse_team_folders(&self, epoch: u64, page: &str) -> Result<Value> {
        let auth = self.auth()?;
        self.0
            .teams
            .browse(&auth, &self.0.profiles, epoch, page)
            .await
    }
    pub async fn connect_team_folder(&self, epoch: u64, folder: &str) -> Result<Value> {
        let auth = self.auth()?;
        self.0
            .teams
            .join(&auth, &self.0.profiles, epoch, folder)
            .await
    }
    pub async fn import_drive_files(
        &self,
        epoch: u64,
        folder: &str,
        ids: &[String],
        progress: FileProgress,
    ) -> Result<Value> {
        let auth = self.auth()?;
        self.0
            .files
            .import(
                &auth,
                &self.0.profiles,
                &self.0.teams,
                epoch,
                folder,
                ids,
                progress,
            )
            .await
    }
    pub async fn retry_drive_import(
        &self,
        epoch: u64,
        folder: &str,
        op: &str,
        progress: FileProgress,
    ) -> Result<Value> {
        let auth = self.auth()?;
        self.0
            .files
            .retry_import(
                &auth,
                &self.0.profiles,
                &self.0.teams,
                epoch,
                folder,
                op,
                progress,
            )
            .await
    }
    pub async fn discard_drive_import(&self, epoch: u64, folder: &str, op: &str) -> Result<()> {
        let auth = self.auth()?;
        self.0.files.discard_import(&auth, epoch, folder, op).await
    }
    pub async fn download_team_file(
        &self,
        epoch: u64,
        folder: &str,
        file: &str,
        progress: FileProgress,
    ) -> Result<DownloadedFile> {
        let auth = self.auth()?;
        self.0
            .files
            .download(
                &auth,
                &self.0.profiles,
                &self.0.teams,
                epoch,
                folder,
                file,
                progress,
            )
            .await
    }
    pub async fn with_current_account<T>(
        &self,
        epoch: u64,
        finish: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let auth = self.auth()?;
        let lease = auth.lease().await?;
        if lease.epoch() != epoch {
            return err("계정 연결이 바뀌어 저장하지 않았어요.");
        }
        auth.with_lease(&lease, finish).await
    }

    pub async fn logout(&self) -> Result<()> {
        self.0.login.cancel();
        let _guard = self.0.login.idle().await;
        self.auth()?.sign_out().await?;
        self.0.reviews.clear();
        self.0.agenda.clear();
        self.record(json!({ "lastSync": 0, "lastResult": null, "lastError": null }));
        Ok(())
    }

    // ---------- 다른 컴퓨터로 옮기기 (Drive 숨김 폴더) ----------
    /// 지금 버전에 필요한 Drive 권한(숨김 폴더 + 내 드라이브)을 모두 받았는지. 예전 로그인이면 다시 로그인해야 한다
    pub fn has_drive(&self) -> bool {
        self.auth()
            .ok()
            .and_then(|a| a.tokens())
            .map(|t| scope_ok(&t.s("scope")))
            .unwrap_or(false)
    }

    fn record_account(&self, patch: Value) {
        self.0.team_cache.update(|c| {
            if !c["account"].is_object() {
                c["account"] = json!({});
            }
            if let Some(o) = patch.as_object() {
                for (k, v) in o {
                    c["account"][k] = v.clone();
                }
            }
        });
    }

    /// 팀 목록이 바뀌면 Google 계정에 맡겨 둔다.
    /// 이 컴퓨터에서 한 번이라도 되찾기를 확인한 뒤에만 쓴다 (새 컴퓨터의 빈 목록으로 덮어쓰지 않도록)
    /// allow_empty: 이 컴퓨터에서 마지막 팀을 나갔을 때만 참. 그 밖에는 팀이 없는 목록으로 덮어쓰지 않는다
    pub async fn backup_account(&self, allow_empty: bool) -> Result<bool> {
        if !self.is_ready()
            || !self.has_drive()
            || !self.0.team_cache.read(|c| c["account"].b("checked"))
        {
            return Ok(false);
        }
        let Some(snap) = self.0.hub.account_snapshot() else {
            return Ok(false);
        };
        if snap.arr("teams").is_empty() && !allow_empty {
            return Ok(false);
        }
        let a = self.auth()?;
        match drive::appdata_write(&a, &self.0.hub.account_filename(), &snap).await {
            Ok(()) => {
                self.record_account(
                    json!({ "savedAt": now_ms(), "teams": snap.arr("teams").len(), "error": null }),
                );
                Ok(true)
            }
            Err(e) => {
                self.record_account(json!({ "error": e }));
                Err(e)
            }
        }
    }

    /// Google 계정에 맡겨 둔 팀을 이 컴퓨터로 가져온다. 결과 문장을 돌려준다
    pub async fn restore_account(&self) -> Result<String> {
        if !self.is_ready() {
            return err("먼저 설정 → 외부 연결에서 Google 로 로그인해 주세요.");
        }
        if !self.has_drive() {
            return err(
                "팀을 옮기려면 Google 로그인을 한 번 더 해 주세요 (Drive 권한이 새로 필요해요).",
            );
        }
        let a = self.auth()?;
        let snap = match drive::appdata_read(&a, &self.0.hub.account_filename()).await {
            Ok(s) => s,
            Err(e) => {
                self.record_account(json!({ "error": e }));
                return Err(e);
            }
        };
        let text = match snap {
            None => "Google 계정에 맡겨 둔 팀이 아직 없어요. 지금 이 컴퓨터의 팀을 맡겨 둘게요."
                .to_string(),
            // 맡겨 둔 목록에 팀이 없으면 이 컴퓨터의 팀을 그대로 둔다
            Some(snap) if snap.arr("teams").is_empty() => {
                "Google 계정에 맡겨 둔 팀이 없어요. 지금 이 컴퓨터의 팀을 맡겨 둘게요.".to_string()
            }
            Some(snap) => {
                let r = self.0.hub.restore_account(&snap).await?;
                let restored: Vec<String> = r
                    .arr("restored")
                    .iter()
                    .map(|x| x.as_str().unwrap_or("").to_string())
                    .collect();
                let rejoined: Vec<String> = r
                    .arr("rejoined")
                    .iter()
                    .map(|x| x.as_str().unwrap_or("").to_string())
                    .collect();
                let mut parts = vec![];
                if !restored.is_empty() {
                    parts.push(format!(
                        "팀 {}개를 되찾았어요: {}",
                        restored.len(),
                        restored.join(", ")
                    ));
                }
                if !rejoined.is_empty() {
                    parts.push(format!(
                        "이 컴퓨터에만 있던 {}에도 같은 계정으로 다시 들어갔어요",
                        rejoined.join(", ")
                    ));
                }
                if parts.is_empty() {
                    "이미 Google 계정의 팀과 같아요.".into()
                } else {
                    parts.join(". ") + "."
                }
            }
        };
        self.record_account(json!({ "checked": true, "restoredAt": now_ms(), "error": null }));
        let _ = self.backup_account(false).await;
        Ok(text)
    }

    pub fn account_status(&self) -> Value {
        let c = self.0.team_cache.read(|c| c["account"].clone());
        json!({ "ready": self.is_ready() && self.has_drive(), "checked": c.b("checked"), "savedAt": c.i("savedAt"), "error": c.get("error").cloned().unwrap_or(Value::Null) })
    }

    // ---------- 팀 자료 (Drive) ----------
    fn drive_auth(&self) -> Result<std::sync::Arc<oauth::GoogleAuth>> {
        if !self.is_ready() {
            return err("Google Drive 를 쓰려면 설정 → 외부 연결에서 Google 로 로그인해 주세요.");
        }
        if !self.has_drive() {
            return err(
                "Drive 를 쓰려면 Google 로그인을 한 번 더 해 주세요 (Drive 권한이 새로 필요해요).",
            );
        }
        self.auth()
    }

    pub async fn list_drive(
        &self,
        mode: &str,
        folder: &str,
        query: &str,
        page: &str,
    ) -> Result<Value> {
        let a = self.drive_auth()?;
        drive::list(&a, mode, folder, query, page).await
    }

    /// 내 드라이브(또는 공유 문서함)의 파일을 팀에 보낼 준비: 공유할 수 있으면 링크 권한을 맞춘다.
    /// 돌려주는 값: 파일 정보 + access ("edit" | "view" | "link": 공유 설정을 바꿀 수 없어 원래 권한 그대로)
    pub async fn prepare_send(&self, id: &str, edit: bool) -> Result<Value> {
        let a = self.drive_auth()?;
        let mut f = drive::get(&a, id).await?;
        let access = if f["capabilities"].b("canShare") {
            match drive::set_access(&a, id, edit).await {
                Ok(()) => {
                    if edit {
                        "edit"
                    } else {
                        "view"
                    }
                }
                Err(_) => "link",
            }
        } else {
            "link"
        };
        f["access"] = json!(access);
        Ok(f)
    }

    pub async fn copy_to_mine(&self, id: &str, name: &str) -> Result<Value> {
        let a = self.drive_auth()?;
        drive::copy_to_mine(&a, id, name).await
    }

    pub async fn set_file_access(&self, drive_id: &str, edit: bool) -> Result<()> {
        let a = self.drive_auth()?;
        drive::set_access(&a, drive_id, edit).await
    }

    pub async fn create_doc(
        &self,
        team_name: &str,
        title: &str,
        html: &str,
        edit: bool,
    ) -> Result<Value> {
        let a = self.drive_auth()?;
        drive::create_doc(&a, &format!("사이 - {team_name}"), title, html, edit).await
    }

    pub async fn upload_file(
        &self,
        team_name: &str,
        name: &str,
        mime: &str,
        bytes: Vec<u8>,
        edit: bool,
    ) -> Result<Value> {
        if !self.is_ready() {
            return err("파일을 올리려면 설정 → 외부 연결에서 Google 로 로그인해 주세요.");
        }
        if !self.has_drive() {
            return err("Drive 에 올리려면 Google 로그인을 한 번 더 해 주세요 (Drive 권한이 새로 필요해요).");
        }
        let a = self.auth()?;
        drive::upload(&a, &format!("사이 - {team_name}"), name, mime, bytes, edit).await
    }

    /// 붙여 넣은 Drive 링크의 파일 정보 (볼 수 없으면 None)
    pub async fn link_meta(&self, url: &str) -> Option<Value> {
        let id = drive::drive_id(url)?;
        if !self.is_ready() {
            return None;
        }
        let a = self.auth().ok()?;
        drive::meta(&a, &id).await
    }

    // ---------- 캘린더 ----------
    /// 이번 7일, 그리고 마감까지의 빈 시간 (9~22시 기준). 이 기기에서만 계산·표시한다
    pub async fn availability(&self, project_id: Option<&str>) -> Result<Value> {
        let a = self.auth()?;
        let now = now_ms();
        let project = project_id
            .and_then(|pid| self.0.hub.all().into_iter().find_map(|t| t.project(pid)))
            .or_else(|| self.0.hub.active().active_project());
        let deadline = project.and_then(|p| crate::time::parse_deadline(&p.s("deadline")));
        let until = (now + 7 * DAY).max(deadline.unwrap_or(0));
        match calendar::query_busy(&a, now, until).await {
            Ok(busy) => {
                let week = calendar::free_hours(&busy, now, now + 7 * DAY, 9, 22);
                let to_dl = deadline
                    .filter(|d| *d > now)
                    .map(|d| calendar::free_hours(&busy, now, d, 9, 22));
                self.record(json!({ "lastCalendar": now, "lastError": null }));
                Ok(json!({ "week": week, "toDeadline": to_dl, "window": "9시~22시" }))
            }
            Err(e) => self.fail(e),
        }
    }

    /// 시간표가 쓰는 바쁜 시간 (사이가 만든 일정 포함)
    pub async fn busy_between(&self, from: i64, to: i64) -> (Vec<Interval>, bool, Option<String>) {
        if !self.is_ready() {
            return (
                vec![],
                false,
                Some("Google 캘린더에 로그인하지 않았어요".into()),
            );
        }
        let a = match self.auth() {
            Ok(a) => a,
            Err(e) => return (vec![], false, Some(e)),
        };
        match calendar::query_busy(&a, from, to).await {
            Ok(b) => {
                self.record(json!({ "lastCalendar": now_ms(), "lastError": null }));
                (b, true, None)
            }
            Err(e) => {
                self.record(json!({ "lastError": e, "lastErrorAt": now_ms() }));
                (vec![], false, Some(e))
            }
        }
    }

    fn describe(&self, project_id: &str) -> String {
        let p = self
            .0
            .hub
            .all()
            .into_iter()
            .find_map(|t| t.project(project_id));
        let mut lines = vec!["사이가 잡은 작업 시간이에요.".to_string()];
        if let Some(p) = p {
            lines.push(format!("프로젝트: {}", p.s("title")));
        }
        lines.push("시간을 바꾸려면 사이의 협업 탭이나 대화에서 고쳐 주세요.".into());
        lines.join("\n")
    }

    pub async fn create_block_event(&self, b: &Value) -> Result<String> {
        let a = self.auth()?;
        calendar::create_event(
            &a,
            &format!("사이: {}", b.s("title")),
            &self.describe(&b.s("projectId")),
            b.i("start"),
            b.i("end"),
            &b.s("taskId"),
        )
        .await
    }

    pub async fn patch_block_event(&self, event_id: &str, b: &Value) -> Result<()> {
        let a = self.auth()?;
        calendar::patch_event(
            &a,
            event_id,
            &format!("사이: {}", b.s("title")),
            b.i("start"),
            b.i("end"),
        )
        .await
    }

    pub async fn remove_block_event(&self, event_id: &str) -> Result<()> {
        let a = self.auth()?;
        calendar::delete_event(&a, event_id).await
    }

    // ---------- 할 일 ----------
    pub async fn sync_todo(&self) -> Result<Option<String>> {
        if self.client().is_none() || !self.is_ready() {
            return Ok(None);
        }
        let teams: Vec<_> = self
            .0
            .hub
            .all()
            .into_iter()
            .filter(|t| t.is_connected())
            .collect();
        if teams.is_empty() {
            return Ok(None);
        }
        let a = self.auth()?;
        let (mut created, mut marked, mut completed) = (0, 0, 0);
        // 모든 팀의 프로젝트를 돌며, 팀마다 내 uid 기준으로 수락한 작업을 올린다
        for t in teams {
            let me = t.my_uid();
            for p in t.collab_data().arr("projects") {
                let pid = p.s("id");
                let mine: Vec<Value> = t
                    .tasks_of(&pid)
                    .into_iter()
                    .filter(|x| x.s("assignee") == me && x.b("accepted"))
                    .collect();
                let links = self.0.settings.read(|s| s["googleTaskLinks"].obj(&pid));
                if mine.is_empty() && links.is_empty() {
                    continue;
                }
                let r = async {
                    let list_id =
                        tasks::ensure_list(&a, &format!("사이 · {}", p.s("title"))).await?;
                    tasks::sync_tasks(&a, &list_id, &mine, &links, &p.s("deadline"), &p.s("title"))
                        .await
                }
                .await;
                let r = match r {
                    Ok(r) => r,
                    Err(e) => return self.fail(e),
                };
                let links = r.links.clone();
                self.0.settings.update(|s| {
                    if !s["googleTaskLinks"].is_object() {
                        s["googleTaskLinks"] = json!({});
                    }
                    s["googleTaskLinks"][&pid] = Value::Object(links);
                });
                for id in &r.mark_done {
                    let _ = t.set_status(id, "done").await;
                }
                created += r.created;
                marked += r.mark_done.len();
                completed += r.completed_in_google;
            }
        }
        let text = format!("할 일 {created}개 추가, Google 에서 완료한 {marked}개 반영, 사이에서 완료한 {completed}개 반영");
        self.record(json!({ "lastSync": now_ms(), "lastResult": text, "lastError": null }));
        Ok(Some(text))
    }

    pub fn status(&self) -> Value {
        let c = self.client();
        let auth = c.as_ref().and_then(|_| self.auth().ok());
        let t = auth.as_ref().and_then(|a| a.tokens());
        let active = auth.as_ref().and_then(|a| a.active_identity());
        let s = self.0.team_cache.read(|c| c["google"].clone());
        json!({
            "configured": c.is_some(),
            "builtin": self.0.settings.read(|s| s["googleClient"].is_null()) && self.0.builtin.is_some(),
            "signedIn": active.is_some(),
            "email": active.map(|identity| identity.email),
            "hasSession": auth.as_ref().is_some_and(|a| a.has_session()),
            "hasCredentials": auth.as_ref().is_some_and(|a| a.has_credentials()),
            "needsReauth": auth.as_ref().is_some_and(|a| a.needs_reauth()),
            "loginPending": self.0.login.busy(),
            "appData": t.as_ref().is_some_and(|t| oauth::account_scopes(&t.s("scope"))),
            "driveFile": auth.as_ref().is_some_and(|a|a.has_file_scope()),
            "driveFull": auth.as_ref().is_some_and(|a|a.has_full_drive()),
            "lastSync": s.i("lastSync"),
            "lastResult": s.get("lastResult").cloned().unwrap_or(Value::Null),
            "lastError": s.get("lastError").cloned().unwrap_or(Value::Null),
            "drive": t.as_ref().map(|t| scope_ok(&t.s("scope"))).unwrap_or(false),
        })
    }

    pub fn configure(&self, text: &str) -> Result<Value> {
        if self.0.login.busy() {
            return err("진행 중인 Google 로그인을 먼저 완료하거나 취소해 주세요.");
        }
        let Some(c) = parse_google_client(text) else {
            return err("클라이언트 ID(…apps.googleusercontent.com)와 보안 비밀번호(GOCSPX-…)를 찾지 못했어요.");
        };
        let cc = c.clone();
        self.0.settings.update(|s| {
            s["googleClient"] = cc;
            s["googleTokensEnc"] = Value::Null;
            s["googleCalendarTokensEnc"] = Value::Null;
        });
        *self.0.auth.lock().unwrap() = None;
        Ok(c)
    }
}

fn scope_ok(scope: &str) -> bool {
    let s: Vec<&str> = scope.split_whitespace().collect();
    s.contains(&"https://www.googleapis.com/auth/drive.appdata")
        && s.contains(&"https://www.googleapis.com/auth/drive")
}
