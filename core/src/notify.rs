//! Telegram 알림 연결과 보내기.
//!
//! 원칙: Telegram 으로는 "업무" 알림만 보낸다 — 배정, 도움 요청, 팀 약속 제안, 내 작업 진행 확인.
//! 개인 대화, 정리 카드, 받은 조언 주제처럼 상담에서 나온 내용은 어떤 경우에도 보내지 않는다.
//!
//! 누가 무엇을 받는가
//!   - 배정 제안: 배정된 팀원 (팀 알림을 켠 경우)
//!   - 막힘 도움 요청: 다른 팀원 모두 (팀 알림을 켠 경우)
//!   - 팀 약속 제안: 다른 팀원 모두 (제안한 사람은 드러내지 않음)
//!   - 진행 확인: 나에게만, 컴퓨터에서 자리를 비웠을 때
use crate::http::{client, json_body};
use crate::team::{backend::random_code, Team, TgRuntime};
use crate::util::{arr_mut, J};
use crate::{err, Result};
use serde_json::{json, Value};

const API: &str = "https://api.telegram.org";
const LINK_TTL_MS: i64 = 30 * 60 * 1000;

pub fn valid_token(token: &str) -> bool {
    let t = token.trim();
    let Some((a, b)) = t.split_once(':') else { return false };
    !a.is_empty() && a.chars().all(|c| c.is_ascii_digit()) && b.len() >= 20 && b.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub async fn tg_call(token: &str, method: &str, body: Value) -> Result<Value> {
    let res = client().post(format!("{API}/bot{token}/{method}")).json(&body).send().await.map_err(|e| format!("Telegram 에 연결하지 못했어요: {e}"))?;
    let (status, d) = json_body(res).await;
    if !d.b("ok") {
        let desc = d.os("description").unwrap_or_else(|| format!("HTTP {status}"));
        return err(format!("Telegram {method} 실패: {desc}"));
    }
    Ok(d["result"].clone())
}

/// /start <코드> 메시지를 찾아 그 채팅 id 를 돌려준다
pub fn find_start_code(updates: &Value, code: &str) -> Option<Value> {
    for u in updates.as_array()? {
        let m = &u["message"];
        let text = m.s("text");
        if text.is_empty() || m["chat"].is_null() {
            continue;
        }
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.first().map(|p| p.starts_with("/start")).unwrap_or(false) && parts.get(1) == Some(&code) {
            return Some(m["chat"]["id"].clone());
        }
    }
    None
}

/// 알림 문안 (업무 알림만 허용)
pub fn notify_text(kind: &str, d: &Value) -> Result<String> {
    Ok(match kind {
        "linked" => "사이와 연결됐어요. 팀 업무 알림과, 자리를 비웠을 때의 진행 확인을 여기로 보내 드릴게요. 개인 대화 내용은 보내지 않아요.".into(),
        "assigned" => format!("[사이 · {}] \"{}\" 배정 제안이 왔어요 (약 {}시간). 사이 앱의 협업 탭에서 맡거나 이의를 남겨 주세요.", d.s("project"), d.s("task"), crate::util::fmt_h(d.n("hours"))),
        "blocked" => format!("[사이 · {}] {}님이 \"{}\"에서 도움이 필요하대요. 같이 볼 수 있다면 한마디 건네 주세요.", d.s("project"), d.s("who"), d.s("task")),
        "proposal" => format!("[사이] 팀에 약속을 맞춰 보자는 제안이 있어요: \"{}\" 사이 앱에서 동의하거나 나중에 볼 수 있어요.", d.s("text")),
        "nudge" => format!("[사이 · {}] {}", d.s("project"), d.s("text")),
        "test" => "사이 알림 시험이에요. 이 메시지가 보이면 연결이 잘 된 거예요.".into(),
        _ => return err(format!("보낼 수 없는 알림 종류예요: {kind}")),
    })
}

impl Team {
    fn tg_state(&self) -> Value {
        let c = self.cache_get();
        match c.get("telegram") {
            Some(v) if v.is_object() => v.clone(),
            _ => json!({ "bot": null, "links": [], "pending": null, "lastResult": null, "lastError": null }),
        }
    }

    fn tg_update(&self, f: impl FnOnce(&mut Value)) {
        self.cache_update(|c| {
            if !c["telegram"].is_object() {
                c["telegram"] = json!({ "bot": null, "links": [], "pending": null, "lastResult": null, "lastError": null });
            }
            f(&mut c["telegram"]);
        });
    }

    fn tg_rt(&self) -> TgRuntime {
        self.hub.0.tg.lock().unwrap().get(&self.id.clone().unwrap_or_default()).cloned().unwrap_or_default()
    }

    fn tg_set_rt(&self, f: impl FnOnce(&mut TgRuntime)) {
        let mut g = self.hub.0.tg.lock().unwrap();
        f(g.entry(self.id.clone().unwrap_or_default()).or_default());
    }

    pub fn tg_record(&self, ok: bool, text: &str) {
        let now = self.now();
        self.tg_update(|s| {
            s["lastResult"] = json!({ "ok": ok, "text": text, "at": now });
            s["lastError"] = if ok { Value::Null } else { json!(text) };
        });
    }

    /// 팀 봇 등록 (한 명만 하면 된다). 토큰이 맞는지 getMe 로 확인한 뒤 저장
    pub async fn tg_set_bot(&self, token: &str) -> Result<String> {
        let token = token.trim().to_string();
        if !valid_token(&token) {
            return err("봇 토큰 형식이 아니에요. @BotFather 가 준 \"숫자:문자\" 형태의 토큰을 넣어 주세요.");
        }
        let me = tg_call(&token, "getMe", json!({})).await?;
        let team_id = self.team_id()?;
        let username = me.s("username");
        self.backend()?.set_integration(&team_id, "telegram", json!({ "botToken": token, "botUsername": username })).await?;
        self.tg_update(|s| s["bot"] = json!({ "username": username }));
        self.tg_set_rt(|r| r.token = Some(token));
        Ok(username)
    }

    pub async fn tg_sync(&self) -> Result<()> {
        if !self.is_connected() {
            return Ok(());
        }
        let team_id = self.team_id()?;
        let b = self.backend()?;
        let bot = b.get_integration(&team_id, "telegram").await?;
        let links = if bot.is_some() { b.list_telegram_links(&team_id).await? } else { vec![] };
        self.tg_update(|s| {
            s["bot"] = bot.as_ref().map(|b| json!({ "username": b.s("botUsername") })).unwrap_or(Value::Null);
            // chatId 는 화면 캐시에 두지 않는다
            s["links"] = json!(links.iter().map(|l| json!({ "uid": l.s("uid"), "teamAlerts": l.b("teamAlerts") })).collect::<Vec<_>>());
        });
        self.tg_set_rt(|r| {
            r.token = bot.as_ref().map(|b| b.s("botToken"));
            r.chat_ids = links.iter().map(|l| (l.s("uid"), (l["chatId"].clone(), l.b("teamAlerts")))).collect();
        });
        if !self.tg_state()["pending"].is_null() {
            let _ = self.tg_check_link().await;
        }
        Ok(())
    }

    fn tg_token(&self) -> Result<String> {
        self.tg_rt().token.ok_or_else(|| "팀 Telegram 봇이 아직 없어요. 설정 → 외부 연결에서 봇 토큰을 넣어 주세요.".to_string())
    }

    // ---------- 내 채팅 연결 ----------
    pub fn tg_start_link(&self) -> Result<Value> {
        let bot = self.tg_state()["bot"].clone();
        if bot.is_null() {
            return err("팀 Telegram 봇이 아직 없어요.");
        }
        let code = format!("sai{}", random_code(10).to_lowercase());
        let now = self.now();
        self.tg_update(|s| s["pending"] = json!({ "code": code, "at": now }));
        Ok(json!({ "code": code, "url": format!("https://t.me/{}?start={}", bot.s("username"), code) }))
    }

    /// 봇에 들어온 /start <코드> 를 찾아 내 채팅으로 연결
    pub async fn tg_check_link(&self) -> Result<Value> {
        let p = self.tg_state()["pending"].clone();
        if p.is_null() {
            return Ok(json!({ "linked": self.tg_is_linked() }));
        }
        if self.now() - p.i("at") > LINK_TTL_MS {
            self.tg_update(|s| s["pending"] = Value::Null);
            return err("연결 코드가 만료됐어요. 다시 시작해 주세요.");
        }
        let token = self.tg_token()?;
        // 읽기만 한다 (offset 을 넘기지 않아 다른 팀원 앱의 연결 코드도 지워지지 않음)
        let updates = tg_call(&token, "getUpdates", json!({ "allowed_updates": ["message"], "timeout": 0, "limit": 100 })).await?;
        let Some(chat_id) = find_start_code(&updates, &p.s("code")) else { return Ok(json!({ "linked": false, "waiting": true })) };
        let team_id = self.team_id()?;
        self.backend()?.set_telegram_link(&team_id, json!({ "chatId": chat_id, "teamAlerts": true })).await?;
        let me = self.my_uid();
        self.tg_set_rt(|r| {
            r.chat_ids.insert(me.clone(), (chat_id.clone(), true));
        });
        self.tg_update(|s| {
            s["pending"] = Value::Null;
            let links = arr_mut(s, "links");
            links.retain(|l| l.s("uid") != me);
            links.push(json!({ "uid": me, "teamAlerts": true }));
        });
        tg_call(&token, "sendMessage", json!({ "chat_id": chat_id, "text": notify_text("linked", &json!({}))?, "disable_web_page_preview": true })).await?;
        self.tg_record(true, "연결 확인 메시지를 보냈어요.");
        Ok(json!({ "linked": true }))
    }

    pub fn tg_is_linked(&self) -> bool {
        let me = self.my_uid();
        self.tg_state().arr("links").iter().any(|l| l.s("uid") == me)
    }

    pub async fn tg_set_team_alerts(&self, on: bool) -> Result<()> {
        let me = self.my_uid();
        let Some((chat_id, _)) = self.tg_rt().chat_ids.get(&me).cloned() else { return err("먼저 Telegram 을 연결해 주세요.") };
        let team_id = self.team_id()?;
        self.backend()?.set_telegram_link(&team_id, json!({ "chatId": chat_id, "teamAlerts": on })).await?;
        self.tg_set_rt(|r| {
            if let Some(x) = r.chat_ids.get_mut(&me) {
                x.1 = on;
            }
        });
        self.tg_update(|s| {
            for l in arr_mut(s, "links") {
                if l.s("uid") == me {
                    l["teamAlerts"] = json!(on);
                }
            }
        });
        Ok(())
    }

    pub async fn tg_unlink(&self) -> Result<()> {
        let team_id = self.team_id()?;
        self.backend()?.remove_telegram_link(&team_id).await?;
        let me = self.my_uid();
        self.tg_set_rt(|r| {
            r.chat_ids.remove(&me);
        });
        self.tg_update(|s| arr_mut(s, "links").retain(|l| l.s("uid") != me));
        Ok(())
    }

    // ---------- 보내기 ----------
    async fn tg_send_to(&self, uids: &[String], kind: &str, data: Value, require_team_alerts: bool) -> usize {
        let rt = self.tg_rt();
        let Some(token) = rt.token.clone() else { return 0 };
        let Ok(text) = notify_text(kind, &data) else { return 0 };
        let mut sent = 0;
        let mut seen = std::collections::HashSet::new();
        for uid in uids {
            if !seen.insert(uid.clone()) {
                continue;
            }
            let Some((chat, alerts)) = rt.chat_ids.get(uid) else { continue };
            if require_team_alerts && !alerts {
                continue;
            }
            match tg_call(&token, "sendMessage", json!({ "chat_id": chat, "text": text, "disable_web_page_preview": true })).await {
                Ok(_) => sent += 1,
                Err(e) => self.tg_record(false, &e),
            }
        }
        if sent > 0 {
            self.tg_record(true, &format!("{sent}명에게 알림을 보냈어요."));
        }
        sent
    }

    fn others(&self) -> Vec<String> {
        self.members(false).iter().map(|m| m.s("uid")).collect()
    }

    /// (uid, 작업 이름, 시간)
    pub async fn tg_assigned(&self, list: &[(String, String, f64)], project: &str) -> usize {
        let me = self.my_uid();
        let mut n = 0;
        for (uid, title, hours) in list.iter().filter(|a| a.0 != me) {
            n += self.tg_send_to(std::slice::from_ref(uid), "assigned", json!({ "task": title, "project": project, "hours": hours }), true).await;
        }
        n
    }

    pub async fn tg_blocked(&self, task: &str, project: &str, who: &str) -> usize {
        self.tg_send_to(&self.others(), "blocked", json!({ "task": task, "project": project, "who": who }), true).await
    }

    pub async fn tg_proposal(&self, text: &str) -> usize {
        self.tg_send_to(&self.others(), "proposal", json!({ "text": text }), true).await
    }

    /// 자리를 비웠을 때 내 진행 확인을 나에게
    pub async fn tg_nudge_me(&self, text: &str, project: &str) -> usize {
        self.tg_send_to(&[self.my_uid()], "nudge", json!({ "text": text, "project": project }), false).await
    }

    pub async fn tg_test(&self) -> Result<usize> {
        let n = self.tg_send_to(&[self.my_uid()], "test", json!({}), false).await;
        if n == 0 {
            return err("보내지 못했어요. Telegram 연결을 먼저 해 주세요.");
        }
        Ok(n)
    }

    pub fn tg_status(&self) -> Value {
        let s = self.tg_state();
        let me = self.my_uid();
        json!({
            "bot": s["bot"],
            "linked": self.tg_is_linked(),
            "teamAlerts": s.arr("links").iter().find(|l| l.s("uid") == me).map(|l| l.b("teamAlerts")).unwrap_or(false),
            "linkedCount": s.arr("links").len(),
            "pending": !s["pending"].is_null(),
            "lastResult": s["lastResult"],
            "lastError": s["lastError"],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_code_and_texts() {
        let ups = json!([{ "message": { "text": "/start saiabc", "chat": { "id": 42 } } }]);
        assert_eq!(find_start_code(&ups, "saiabc"), Some(json!(42)));
        assert_eq!(find_start_code(&ups, "other"), None);
        assert!(notify_text("summary", &json!({})).is_err());
        assert!(valid_token("123456:ABCDEFGHIJKLMNOPQRSTU_-x"));
        assert!(!valid_token("nope"));
    }
}
