//! 팀 연결. 여러 팀(팀플)을 동시에 쓴다.
//!
//!   settings.teams        [{ teamId, name, joinCode, displayName }]
//!   settings.activeTeamId 화면(협업·정리함·공유 기본값)에서 보고 있는 팀
//!   teamCache.teams[id]   팀별 캐시 (팀원·받은 주제·제안·협업·Telegram)
//!   outbox.items[].teamId 공유 항목이 어느 팀으로 가는지
//! 고양이의 말풍선(받은 조언 주제, 팀 약속 제안, 진행 확인)은 모든 팀을 함께 본다.
//! 서버 계정(익명 로그인)은 하나를 모든 팀이 같이 쓴다.
//!
//! 공유 대기함 항목의 상태
//!   review          보내기 전에 내가 한 번 더 확인해야 함 ("검토 후 공유")
//!   needs-recipient 받을 팀원을 아직 고르지 않음
//!   queued          보낼 준비 완료. sendAfter 이후 전송 (보낸 시각으로 발언자를 짐작하기 어렵게 늦춰 보냄)
//!   sent            전송됨 (remoteId 로 상태 확인)
//!   held            받은 사람이 "내 사정도 있어요"로 보류함
//! 서버로는 주제 id, 받는 사람 uid, 주(週)만 간다. 문안은 받는 쪽 앱의 일반 문안을 쓴다.
pub mod backend;
pub mod codec;

pub use backend::Backend;

use crate::store::Store;
use crate::time::{iso_week, DAY};
use crate::topics::topic;
use crate::util::{arr_mut, clean, to_base36, J};
use crate::{err, Result};
use rand::Rng;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub const BOOST_WINDOW_DAYS: i64 = 21;

pub type MakeBackend = Box<dyn Fn() -> Option<Backend> + Send + Sync>;
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Telegram 봇 토큰과 팀원 채팅 id 는 화면 캐시(파일)에 두지 않고 실행 중에만 기억한다
#[derive(Default, Clone)]
pub struct TgRuntime {
    pub token: Option<String>,
    pub chat_ids: HashMap<String, (Value, bool)>,
}

pub struct HubInner {
    pub settings: Store,
    pub outbox: Store,
    pub team_cache: Store,
    make_backend: MakeBackend,
    backend: Mutex<Option<Arc<Backend>>>,
    pub delay_ms: Option<i64>,
    pub now: Clock,
    sync_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    pub tg: Mutex<HashMap<String, TgRuntime>>,
    pub last_nudge_team: Mutex<Option<String>>,
}

#[derive(Clone)]
pub struct Hub(pub Arc<HubInner>);

/// 팀 하나 (id 가 None 이면 아직 팀이 없는 상태: 만들기·참여용)
#[derive(Clone)]
pub struct Team {
    pub hub: Hub,
    pub id: Option<String>,
}

impl Hub {
    pub fn new(settings: Store, outbox: Store, team_cache: Store, make_backend: MakeBackend, delay_ms: Option<i64>, now: Clock) -> Hub {
        settings.update(|s| {
            if !s["teams"].is_array() {
                s["teams"] = json!([]);
            }
        });
        team_cache.update(|c| {
            if !c["teams"].is_object() {
                c["teams"] = json!({});
            }
        });
        Hub(Arc::new(HubInner {
            settings,
            outbox,
            team_cache,
            make_backend,
            backend: Mutex::new(None),
            delay_ms,
            now,
            sync_locks: Mutex::new(HashMap::new()),
            tg: Mutex::new(HashMap::new()),
            last_nudge_team: Mutex::new(None),
        }))
    }

    pub fn now(&self) -> i64 {
        (self.0.now)()
    }

    // ---------- 백엔드 (공유) ----------
    pub fn backend(&self) -> Result<Arc<Backend>> {
        let mut g = self.0.backend.lock().unwrap();
        if g.is_none() {
            *g = (self.0.make_backend)().map(Arc::new);
        }
        g.clone().ok_or_else(|| "팀 서버(Firebase) 설정이 아직 없어요. 설정에서 먼저 입력해 주세요.".to_string())
    }

    pub fn backend_kind(&self) -> Option<&'static str> {
        self.0.backend.lock().unwrap().as_ref().map(|b| b.kind())
    }

    pub fn reset_backend(&self) {
        *self.0.backend.lock().unwrap() = None;
    }

    // ---------- 팀 목록 ----------
    pub fn teams(&self) -> Vec<Value> {
        self.0.settings.read(|s| s.arr("teams"))
    }

    pub fn team(&self, id: Option<&str>) -> Option<Team> {
        match id {
            None => Some(Team { hub: self.clone(), id: None }),
            Some(id) => self.teams().iter().any(|t| t.s("teamId") == id).then(|| Team { hub: self.clone(), id: Some(id.to_string()) }),
        }
    }

    pub fn detached(&self) -> Team {
        Team { hub: self.clone(), id: None }
    }

    pub fn all(&self) -> Vec<Team> {
        self.teams().iter().map(|t| Team { hub: self.clone(), id: Some(t.s("teamId")) }).collect()
    }

    pub fn active(&self) -> Team {
        let active = self.0.settings.read(|s| s.s("activeTeamId"));
        let teams = self.teams();
        let id = if teams.iter().any(|t| t.s("teamId") == active) { Some(active) } else { teams.first().map(|t| t.s("teamId")) };
        Team { hub: self.clone(), id }
    }

    pub fn set_active(&self, team_id: &str) -> Result<()> {
        if !self.teams().iter().any(|t| t.s("teamId") == team_id) {
            return err("그 팀을 찾을 수 없어요.");
        }
        self.0.settings.update(|s| s["activeTeamId"] = json!(team_id));
        Ok(())
    }

    fn add_team(&self, t: Value) {
        let first = self.teams().is_empty();
        let id = t.s("teamId");
        self.0.settings.update(|s| {
            let teams = arr_mut(s, "teams");
            teams.retain(|x| x.s("teamId") != id);
            teams.push(t);
            s["activeTeamId"] = json!(id);
        });
        // 팀이 하나도 없을 때 저장해 둔 공유 항목은 처음 연결한 팀으로 보낸다
        if first {
            self.0.outbox.update(|d| {
                for it in arr_mut(d, "items") {
                    if it.os("teamId").is_none() {
                        it["teamId"] = json!(id);
                    }
                }
            });
        }
    }

    fn remove_team(&self, team_id: &str) {
        self.0.settings.update(|s| {
            arr_mut(s, "teams").retain(|x| x.s("teamId") != team_id);
            if s.s("activeTeamId") == team_id {
                s["activeTeamId"] = s["teams"].get(0).map(|t| t["teamId"].clone()).unwrap_or(Value::Null);
            }
        });
        self.0.team_cache.update(|c| {
            if let Some(o) = c["teams"].as_object_mut() {
                o.remove(team_id);
            }
        });
        self.0.tg.lock().unwrap().remove(team_id);
    }

    // ---------- 만들기·참여·나가기 ----------
    pub async fn create_team(&self, name: &str, display_name: &str) -> Result<Value> {
        let b = self.backend()?;
        let display = non_empty(clean(display_name, 30), "팀원");
        let t = b.create_team(&non_empty(clean(name, 60), "우리 팀"), &display).await?;
        self.add_team(json!({ "teamId": t.s("teamId"), "name": t.s("name"), "joinCode": t.s("joinCode"), "displayName": display }));
        let team = self.active();
        self.sync_team(&team).await;
        Ok(team.state().unwrap_or(Value::Null))
    }

    pub async fn join_team(&self, code: &str, display_name: &str) -> Result<Value> {
        let c = code.trim().to_uppercase();
        if let Some(existing) = self.teams().iter().find(|t| t.s("joinCode") == c) {
            return err(format!("이미 참여한 팀이에요: {}", existing.s("name")));
        }
        let b = self.backend()?;
        let display = non_empty(clean(display_name, 30), "팀원");
        let t = b.join_team(&c, &display).await?;
        self.add_team(json!({ "teamId": t.s("teamId"), "name": t.s("name"), "joinCode": t.s("joinCode"), "displayName": display }));
        let team = self.active();
        self.sync_team(&team).await;
        Ok(team.state().unwrap_or(Value::Null))
    }

    pub async fn leave_team(&self, team_id: &str) -> Result<()> {
        if let Ok(b) = self.backend() {
            if let Err(e) = b.leave_team(team_id).await {
                // 서버에 닿지 않아도 이 기기에서는 연결을 끊는다
                let _ = e;
            }
        }
        // 이 팀으로 가려던 공유 항목과 캐시도 함께 지운다
        self.0.outbox.update(|d| arr_mut(d, "items").retain(|x| x.s("teamId") != team_id));
        self.remove_team(team_id);
        Ok(())
    }

    // ---------- 동기화 ----------
    pub async fn sync_team(&self, t: &Team) -> Value {
        let r = t.sync().await;
        if r.b("ok") {
            if let Err(e) = t.collab_sync().await {
                t.cache_update(|c| c["lastError"] = json!(e));
            }
            if let Err(e) = t.tg_sync().await {
                t.tg_record(false, &e);
            }
        }
        r
    }

    pub async fn sync_all(&self) -> Vec<Value> {
        let mut out = vec![];
        for t in self.all() {
            out.push(self.sync_team(&t).await);
        }
        out
    }

    fn lock_for(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.0.sync_locks.lock().unwrap().entry(key.to_string()).or_default().clone()
    }

    // ---------- 모든 팀을 함께 보는 창구 (고양이 말풍선용) ----------
    pub fn any_connected(&self) -> bool {
        self.all().iter().any(|t| t.is_connected())
    }

    pub fn boosts(&self) -> HashMap<String, bool> {
        let mut out = HashMap::new();
        for t in self.all() {
            out.extend(t.boosts());
        }
        out
    }

    pub fn pending_proposals_for_me(&self) -> Vec<Value> {
        let mut out = vec![];
        for t in self.all() {
            let name = t.name();
            for mut p in t.pending_proposals_for_me() {
                p["teamId"] = json!(t.id);
                p["teamName"] = json!(name);
                out.push(p);
            }
        }
        out
    }

    pub async fn respond(&self, proposal_id: &str, response: &str) -> Result<()> {
        if let Some(t) = self.all().into_iter().find(|t| t.proposals().iter().any(|p| p.s("id") == proposal_id)) {
            t.respond(proposal_id, response).await?;
        }
        Ok(())
    }

    pub async fn hold_topic(&self, topic_id: &str) -> usize {
        let mut n = 0;
        for t in self.all() {
            n += t.hold_topic(topic_id).await;
        }
        n
    }

    /// "내 사정" 뒤 팀 약속 제안: 그 주제를 받은 팀들에, 없으면 지금 보고 있는 팀에
    pub fn propose_team(&self, topic_id: &str) -> Vec<String> {
        let got: Vec<Team> = self.all().into_iter().filter(|t| t.cache_get().arr("inbox").iter().any(|s| s.s("topic") == topic_id)).collect();
        let targets = if got.is_empty() { vec![self.active()].into_iter().filter(|t| t.id.is_some()).collect() } else { got };
        targets.iter().filter_map(|t| t.propose_team(topic_id)).collect()
    }

    pub fn find_task_team(&self, task_id: &str) -> Option<Team> {
        self.all().into_iter().find(|t| t.find_task(task_id).is_some())
    }
}

fn non_empty(s: String, d: &str) -> String {
    if s.is_empty() {
        d.into()
    } else {
        s
    }
}

impl Team {
    pub fn now(&self) -> i64 {
        self.hub.now()
    }

    fn key(&self) -> String {
        self.id.clone().unwrap_or_else(|| "_none".into())
    }

    /// 팀 연결 정보 { teamId, name, joinCode, displayName }
    pub fn state(&self) -> Option<Value> {
        let id = self.id.as_ref()?;
        self.hub.teams().into_iter().find(|t| &t.s("teamId") == id)
    }

    pub fn name(&self) -> String {
        self.state().map(|s| s.s("name")).unwrap_or_default()
    }

    pub fn team_id(&self) -> Result<String> {
        self.state().map(|s| s.s("teamId")).filter(|s| !s.is_empty()).ok_or_else(|| "먼저 팀에 연결해 주세요 (설정 → 팀 연결).".to_string())
    }

    pub fn is_connected(&self) -> bool {
        self.state().map(|s| !s.s("teamId").is_empty()).unwrap_or(false)
    }

    pub fn backend(&self) -> Result<Arc<Backend>> {
        self.hub.backend()
    }

    // ---------- 팀별 캐시 ----------
    pub fn cache_get(&self) -> Value {
        let k = self.key();
        self.hub.0.team_cache.read(|c| c["teams"].get(&k).cloned()).unwrap_or_else(|| json!({}))
    }

    pub fn cache_update<R>(&self, f: impl FnOnce(&mut Value) -> R) -> R {
        let k = self.key();
        self.hub.0.team_cache.update(|c| {
            if !c["teams"].is_object() {
                c["teams"] = json!({});
            }
            let e = c["teams"].as_object_mut().unwrap().entry(k).or_insert_with(|| json!({}));
            f(e)
        })
    }

    pub fn my_uid(&self) -> String {
        self.cache_get().s("myUid")
    }

    // ---------- 팀별 공유 대기함 ----------
    pub fn outbox_items(&self) -> Vec<Value> {
        let id = self.id.clone();
        self.hub.0.outbox.read(|d| d.arr("items").into_iter().filter(|x| x.os("teamId") == id).collect())
    }

    pub fn outbox_update<R>(&self, f: impl FnOnce(&mut Vec<Value>) -> R) -> R {
        let id = self.id.clone();
        self.hub.0.outbox.update(|d| {
            let all = arr_mut(d, "items");
            let (mut mine, others): (Vec<Value>, Vec<Value>) = all.drain(..).partition(|x| x.os("teamId") == id);
            let r = f(&mut mine);
            for x in mine.iter_mut() {
                x["teamId"] = json!(id);
            }
            all.extend(others);
            all.extend(mine);
            r
        })
    }

    fn update_item(&self, item_id: &str, f: impl FnOnce(&mut Value)) {
        self.outbox_update(|items| {
            if let Some(x) = items.iter_mut().find(|i| i.s("id") == item_id) {
                f(x);
            }
        });
    }

    // ---------- 공유 대기함 ----------
    pub fn send_after(&self) -> i64 {
        if let Some(d) = self.hub.0.delay_ms {
            return self.now() + d;
        }
        let m = 20.0 + rand::thread_rng().gen::<f64>() * 160.0;
        self.now() + (m * 60_000.0).round() as i64
    }

    /// 새로 저장된 공유 항목에 상태를 붙인다
    pub fn prepare_items(&self, items: Vec<Value>) -> Vec<Value> {
        items
            .into_iter()
            .map(|mut it| {
                if it.s("target") != "person" {
                    it["toMember"] = Value::Null;
                } else if it.os("toMember").map(|s| s.is_empty()).unwrap_or(true) {
                    it["toMember"] = Value::Null;
                }
                if it.s("scope") == "review" {
                    it["status"] = json!("review");
                } else if it.s("target") == "person" && it["toMember"].is_null() {
                    it["status"] = json!("needs-recipient");
                } else {
                    it["status"] = json!("queued");
                    it["sendAfter"] = json!(self.send_after());
                }
                it
            })
            .collect()
    }

    pub fn confirm_review(&self, item_id: &str, to_member: Option<String>) {
        let after = self.send_after();
        self.update_item(item_id, |x| {
            if let Some(m) = to_member.filter(|m| !m.is_empty()) {
                x["toMember"] = json!(m);
            }
            if x.s("target") == "person" && x.os("toMember").is_none() {
                x["status"] = json!("needs-recipient");
            } else {
                x["status"] = json!("queued");
                x["sendAfter"] = json!(after);
            }
        });
    }

    pub fn set_recipient(&self, item_id: &str, to_member: &str) {
        let after = self.send_after();
        self.update_item(item_id, |x| {
            x["toMember"] = json!(to_member);
            if x.s("status") == "needs-recipient" {
                x["status"] = json!("queued");
                x["sendAfter"] = json!(after);
            }
        });
    }

    /// "내 사정"을 들은 뒤, 개인 조언 대신 팀 약속으로 맞춰 보자고 제안
    pub fn propose_team(&self, topic_id: &str) -> Option<String> {
        let t = topic(topic_id)?;
        if let Some(x) = self.outbox_items().iter().find(|x| x.s("topic") == topic_id && x.s("target") == "team" && x.s("status") != "sent") {
            return Some(x.s("id"));
        }
        let id = format!("o{}{}", to_base36(self.now() as u64), to_base36(rand::thread_rng().gen_range(0..1_000_000)));
        let after = self.send_after();
        let week = iso_week(self.now());
        self.outbox_update(|items| {
            items.push(json!({
                "id": id, "topic": topic_id, "label": t.label, "scope": "auto", "target": "team",
                "advice": t.advice[0], "teamProposal": t.team_proposal, "week": week,
                "toMember": null, "status": "queued", "sendAfter": after
            }))
        });
        Some(id)
    }

    pub async fn send_due(&self) -> Result<usize> {
        let team_id = self.team_id()?;
        let b = self.backend()?;
        let now = self.now();
        let due: Vec<Value> = self.outbox_items().into_iter().filter(|x| x.s("status") == "queued" && x.i("sendAfter") <= now).collect();
        let cache = self.cache_get();
        let member_ids: HashSet<String> = cache.arr("members").iter().map(|m| m.s("uid")).collect();
        let mut sent = 0;
        for it in due {
            let week = iso_week(self.now());
            let r: Result<String> = async {
                if it.s("target") == "team" {
                    // 같은 주제의 제안이 이미 있으면 새로 만들지 않고 동의만 한다 (여러 명의 의견처럼 부풀리지 않기)
                    let existing = self.cache_get().arr("proposals").into_iter().find(|p| p.s("topic") == it.s("topic") && p.s("week") == week);
                    if let Some(p) = existing {
                        b.respond_proposal(&team_id, &p.s("id"), "agree").await?;
                        Ok(p.s("id"))
                    } else {
                        let id = b.create_proposal(&team_id, &it.s("topic"), &week).await?;
                        b.respond_proposal(&team_id, &id, "agree").await?;
                        // 새 팀 약속 제안을 팀원에게 알림 (제안한 사람은 드러내지 않음)
                        if let Some(t) = topic(&it.s("topic")) {
                            let _ = self.tg_proposal(t.team_proposal).await;
                        }
                        Ok(id)
                    }
                } else {
                    let to = it.s("toMember");
                    if !member_ids.is_empty() && !member_ids.contains(&to) {
                        return Err("__recipient".to_string());
                    }
                    b.create_signal(&team_id, &it.s("topic"), &to, &week).await
                }
            }
            .await;
            match r {
                Ok(remote) => {
                    self.update_item(&it.s("id"), |x| {
                        x["status"] = json!("sent");
                        x["remoteId"] = json!(remote);
                        x["week"] = json!(week);
                        x.as_object_mut().unwrap().remove("sendAfter");
                    });
                    sent += 1;
                }
                Err(e) if e == "__recipient" => self.update_item(&it.s("id"), |x| {
                    x["status"] = json!("needs-recipient");
                    x["toMember"] = Value::Null;
                }),
                Err(e) => self.update_item(&it.s("id"), |x| x["lastError"] = json!(e)),
            }
        }
        Ok(sent)
    }

    // ---------- 동기화 ----------
    pub async fn sync(&self) -> Value {
        let lock = self.hub.lock_for(&self.key());
        let _g = lock.lock().await;
        match self.sync_inner().await {
            Ok(()) => json!({ "ok": true }),
            Err(e) => {
                self.cache_update(|c| c["lastError"] = json!(e));
                json!({ "ok": false, "error": e })
            }
        }
    }

    async fn sync_inner(&self) -> Result<()> {
        let team_id = self.team_id().map_err(|_| "not-connected".to_string())?;
        let b = self.backend()?;
        let my_uid = b.sign_in().await?;
        let members = b.list_members(&team_id).await?;
        let proposals = b.list_proposals(&team_id).await?;
        self.cache_update(|c| {
            c["members"] = json!(members);
            c["proposals"] = json!(proposals);
            c["myUid"] = json!(my_uid);
        });
        if !members.iter().any(|m| m.s("uid") == my_uid) {
            return err("이 기기는 더 이상 팀원으로 등록되어 있지 않아요. 참여 코드로 다시 참여해 주세요.");
        }
        self.send_due().await?;
        let inbox = b.list_signals_for_me(&team_id).await?;
        // 내가 보낸 신호가 보류되었는지 확인
        for it in self.outbox_items().into_iter().filter(|x| x.s("status") == "sent" && x.s("target") == "person" && !x.s("remoteId").is_empty()) {
            if let Ok(Some(s)) = b.get_signal(&team_id, &it.s("remoteId")).await {
                if s.b("held") {
                    self.update_item(&it.s("id"), |x| x["status"] = json!("held"));
                }
            }
        }
        let now = self.now();
        self.cache_update(|c| {
            c["inbox"] = json!(inbox.iter().map(|s| json!({ "id": s.s("id"), "topic": s.s("topic"), "week": s.s("week"), "held": s.b("held") })).collect::<Vec<_>>());
            c["lastSync"] = json!(now);
            c["lastError"] = Value::Null;
        });
        Ok(())
    }

    // ---------- 받은 쪽 ----------
    /// 최근 3주 동안 받은 (보류되지 않은) 주제 → 평소 조언에서 조금 더 자주 다룸
    pub fn boosts(&self) -> HashMap<String, bool> {
        let mut out = HashMap::new();
        if !self.is_connected() {
            return out;
        }
        let cutoff = iso_week(self.now() - BOOST_WINDOW_DAYS * DAY);
        for s in self.cache_get().arr("inbox") {
            if !s.b("held") && topic(&s.s("topic")).is_some() && s.s("week") >= cutoff {
                out.insert(s.s("topic"), true);
            }
        }
        out
    }

    /// "내 사정도 있어요": 이 주제로 받은 신호를 보류한다
    pub async fn hold_topic(&self, topic_id: &str) -> usize {
        let Ok(team_id) = self.team_id() else { return 0 };
        let Ok(b) = self.backend() else { return 0 };
        let targets: Vec<Value> = self.cache_get().arr("inbox").into_iter().filter(|s| s.s("topic") == topic_id && !s.b("held")).collect();
        let mut n = 0;
        for s in targets {
            match b.hold_signal(&team_id, &s.s("id")).await {
                Ok(()) => n += 1,
                Err(e) => self.cache_update(|c| c["lastError"] = json!(e)),
            }
        }
        if n > 0 {
            self.cache_update(|c| {
                for s in arr_mut(c, "inbox") {
                    if s.s("topic") == topic_id {
                        s["held"] = json!(true);
                    }
                }
            });
        }
        n
    }

    // ---------- 팀 약속 ----------
    pub fn proposals(&self) -> Vec<Value> {
        let c = self.cache_get();
        let member_ids: HashSet<String> = c.arr("members").iter().map(|m| m.s("uid")).collect();
        let my = c.s("myUid");
        let mut out: Vec<Value> = c
            .arr("proposals")
            .iter()
            .filter_map(|p| {
                let t = topic(&p.s("topic"))?;
                let responses = p.obj("responses");
                let agree = responses.iter().filter(|(uid, r)| member_ids.contains(*uid) && r.as_str() == Some("agree")).count();
                Some(json!({
                    "id": p.s("id"), "topic": t.id, "label": t.label, "text": t.team_proposal, "week": p.s("week"),
                    "agree": agree, "memberCount": member_ids.len(),
                    "agreedByAll": member_ids.len() > 1 && agree >= member_ids.len(),
                    "mine": responses.get(&my).cloned().unwrap_or(Value::Null),
                }))
            })
            .collect();
        out.sort_by(|a, b| b.s("week").cmp(&a.s("week")));
        out
    }

    pub fn pending_proposals_for_me(&self) -> Vec<Value> {
        self.proposals().into_iter().filter(|p| p["mine"].is_null()).collect()
    }

    pub async fn respond(&self, proposal_id: &str, response: &str) -> Result<()> {
        let team_id = self.team_id()?;
        self.backend()?.respond_proposal(&team_id, proposal_id, response).await?;
        self.cache_update(|c| {
            let my = c.s("myUid");
            for p in arr_mut(c, "proposals") {
                if p.s("id") == proposal_id {
                    if !p["responses"].is_object() {
                        p["responses"] = json!({});
                    }
                    p["responses"][&my] = json!(response);
                }
            }
        });
        Ok(())
    }

    // ---------- 화면용 ----------
    pub fn members(&self, include_me: bool) -> Vec<Value> {
        let c = self.cache_get();
        let me = c.s("myUid");
        c.arr("members").into_iter().filter(|m| include_me || m.s("uid") != me).collect()
    }

    pub fn status(&self) -> Value {
        let c = self.cache_get();
        json!({
            "connected": self.is_connected(),
            "team": self.state(),
            "memberCount": c.arr("members").len(),
            "lastSync": c.i("lastSync"),
            "lastError": c.get("lastError").cloned().unwrap_or(Value::Null),
            "backendKind": self.hub.backend_kind(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Stores;

    pub fn hub_for(shared: Arc<Mutex<Value>>, uid: &str) -> (Hub, Stores) {
        let st = Stores::memory();
        let u = uid.to_string();
        let hub = Hub::new(
            st.settings.clone(),
            st.shared_outbox.clone(),
            st.team_cache.clone(),
            Box::new(move || Some(Backend::memory_shared(shared.clone(), &u))),
            Some(0),
            Arc::new(crate::time::now_ms),
        );
        (hub, st)
    }

    #[tokio::test]
    async fn signal_goes_only_to_recipient_and_can_be_held() {
        let shared = Backend::new_shared_memory();
        let (a, _) = hub_for(shared.clone(), "ua");
        let (b, _) = hub_for(shared.clone(), "ub");
        let t = a.create_team("BION", "유빈").await.unwrap();
        b.join_team(&t.s("joinCode"), "민수").await.unwrap();
        let ta = a.active();
        ta.sync().await;
        assert_eq!(ta.members(true).len(), 2);
        // A → B 조언 주제
        let items = ta.prepare_items(vec![json!({ "id": "o1", "topic": "time_promise", "scope": "auto", "target": "person", "toMember": "ub", "week": "x" })]);
        a.0.outbox.update(|d| arr_mut(d, "items").extend(items.into_iter().map(|mut i| {
            i["teamId"] = json!(ta.id);
            i
        })));
        ta.sync().await;
        assert_eq!(ta.outbox_items()[0].s("status"), "sent");
        let tb = b.active();
        tb.sync().await;
        assert_eq!(tb.boosts().get("time_promise"), Some(&true));
        // B 가 보류하면 A 쪽 항목도 held
        assert_eq!(tb.hold_topic("time_promise").await, 1);
        ta.sync().await;
        assert_eq!(ta.outbox_items()[0].s("status"), "held");
        // 팀 약속 제안
        ta.propose_team("workload").unwrap();
        ta.sync().await;
        tb.sync().await;
        let p = tb.pending_proposals_for_me();
        assert_eq!(p.len(), 1);
        tb.respond(&p[0].s("id"), "agree").await.unwrap();
        ta.sync().await;
        assert_eq!(ta.proposals()[0]["agreedByAll"], true);
    }
}
