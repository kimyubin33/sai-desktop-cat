//! 공유 승인 전의 대화와 AI 초안은 이 사용자의 private/conversations.json 안에만 둔다.
use super::Chat;
use crate::counseling::Turn;
use crate::safety::{check_safety, safety_message};
use crate::team::{backend::random_code, Team};
use crate::time::now_ms;
use crate::util::{arr_mut, J};
use crate::{err, Result};
use serde_json::{json, Value};

struct Context {
    team_id: String,
    uid: String,
    kind: String,
    session: String,
}

impl Context {
    fn new(team: &Team, args: &Value) -> Result<Self> {
        let session = args.s("sessionId");
        let kind = args.s("kind");
        if session.is_empty() || session.len() > 100 || !matches!(kind.as_str(), "complaint" | "promise")
            || (kind == "promise") != (session == "promise") {
            return err("대화를 다시 열어 주세요.");
        }
        Ok(Self { team_id: team.id.clone().unwrap_or_default(), uid: team.my_uid(), session, kind })
    }

    fn matches(&self, row: &Value) -> bool {
        row["teamId"] == self.team_id && row["uid"] == self.uid && row["sessionId"] == self.session && row["kind"] == self.kind
    }

    fn check_team(&self, team: &Team) -> Result<()> {
        if self.uid.is_empty() || team.hub.active().id.as_ref() != Some(&self.team_id) || team.my_uid() != self.uid || !team.is_connected() {
            return err("팀 연결을 확인하고 해당 팀에서 다시 열어 주세요.");
        }
        Ok(())
    }

    fn initial(&self) -> Value {
        json!({"teamId":self.team_id,"uid":self.uid,"sessionId":self.session,"kind":self.kind,"turns":[]})
    }
}

impl Chat {
    fn delivery_source(&self, context: &Context) -> Result<Vec<Turn>> {
        if context.kind == "promise" { return Ok(Vec::new()); }
        let session = self.find(&context.session).filter(|s| s.mode == "complaint").ok_or("원래 상담 기록을 찾을 수 없어요.")?;
        if session.safety_hold.is_some() { return err("안전 확인 중인 상담은 공유할 수 없어요."); }
        Ok(session.turns)
    }

    fn delivery_record(&self, context: &Context) -> Value {
        self.conv.read(|data| data["deliveryDrafts"].as_array().and_then(|rows| rows.iter().find(|r| context.matches(r))).cloned().unwrap_or(Value::Null))
    }

    fn delivery_view(&self, team: &Team, ctx: &Context) -> Value {
        let row = self.delivery_record(ctx);
        let mut members = team.members(true);
        for member in &mut members { member["me"] = json!(member["uid"] == ctx.uid); }
        json!({"teamId":ctx.team_id,"teamName":team.name(),"isConnected":team.is_connected() && !ctx.uid.is_empty(),
            "members":members,"hasClaudeKey":self.ai.has_project_claude_key(team),"hasOpenaiKey":self.ai.has_openai_key(),
            "kind":ctx.kind,"sessionId":ctx.session,"turns":row.arr("turns"),"draft":row["draft"],
            "question":row["question"],"recommendation":row["recommendation"],"canRetry":row.b("canRetry"),
            "approved":row["approvedId"].is_string(),"approvedId":row["approvedId"]})
    }

    pub async fn delivery(&self, team: &Team, command: &str, args: &Value) -> Result<Value> {
        let ctx = Context::new(team, args)?;
        if command == "delivery:view" { return Ok(self.delivery_view(team, &ctx)); }
        ctx.check_team(team)?;
        match command {
            "delivery:discard" => {
                self.conv.update(|d| arr_mut(d, "deliveryDrafts").retain(|r| !ctx.matches(r)));
            }
            "delivery:generate" => self.generate_delivery(team, &ctx, args).await?,
            "delivery:approve" => self.approve_delivery(team, &ctx, args)?,
            _ => return err("지원하지 않는 대화 동작이에요."),
        }
        Ok(self.delivery_view(team, &ctx))
    }

    async fn generate_delivery(&self, team: &Team, ctx: &Context, args: &Value) -> Result<()> {
        let provider = args.s("provider");
        match provider.as_str() {
            "claude" if self.ai.has_project_claude_key(team) => {}
            "openai" if self.ai.has_openai_key() => {}
            _ => return err("새 문안을 만들려면 설정에서 사용할 Claude 또는 GPT API를 연결해 주세요."),
        }
        let mut source = self.delivery_source(ctx)?;
        let text = args.s("text");
        let text = text.trim();
        if text.chars().count() > 6000 { return err("한 번에 6,000자까지 입력할 수 있어요."); }
        if let Some(risk) = check_safety(text) { return err(safety_message(risk)); }
        let request = random_code(24);
        let now = now_ms();
        let row = self.conv.update(|data| -> Result<Value> {
            let rows = arr_mut(data, "deliveryDrafts");
            let i = rows.iter().position(|r| ctx.matches(r)).unwrap_or(rows.len());
            let mut row = rows.get(i).cloned().unwrap_or_else(|| ctx.initial());
            if row["approvedId"].is_string() { return err("이미 보낸 제안이에요. 새 대화를 시작해 주세요."); }
            let history = arr_mut(&mut row, "turns");
            if !text.is_empty() && !(history.last().is_some_and(|t| t["role"] == "user" && t["text"] == text)) {
                if history.len() >= 60 { return err("대화가 길어졌어요. 새 대화에서 다시 시작해 주세요."); }
                history.push(json!({"role":"user","text":text,"at":now}));
            }
            if !source.iter().any(|t| t.role == "user") && !history.iter().any(|t| t["role"] == "user") {
                return err("먼저 팀에 전하고 싶은 이야기를 입력해 주세요.");
            }
            row["revision"] = json!(request);
            row["updatedAt"] = json!(now);
            row["canRetry"] = json!(true);
            if i == rows.len() { rows.push(row.clone()); } else { rows[i] = row.clone(); }
            Ok(row)
        })?;
        source.extend(serde_json::from_value::<Vec<Turn>>(row["turns"].clone()).map_err(|_| "저장된 초안을 읽지 못했어요.")?);
        let mut reply = self.ai.delivery_draft(team, &provider, &ctx.kind, &source).await?;
        ctx.check_team(team)?;
        self.delivery_source(ctx)?;
        self.conv.update(|d| {
            let row = arr_mut(d, "deliveryDrafts").iter_mut().find(|r| ctx.matches(r) && r["revision"] == request)
                .ok_or("대화가 바뀌었어요. 현재 초안을 확인해 주세요.")?;
            row["canRetry"] = json!(false);
            row["updatedAt"] = json!(now_ms());
            row["question"] = Value::Null;
            row["recommendation"] = Value::Null;
            row["draft"] = Value::Null;
            match reply.s("kind").as_str() {
                "draft" => {
                    reply["id"] = json!(random_code(28));
                    row["draft"] = reply;
                }
                "question" | "skip" => {
                    let message = reply.s("text");
                    let field = if reply["kind"] == "question" { "question" } else { "recommendation" };
                    row[field] = json!(message);
                    arr_mut(row, "turns").push(json!({"role":"cat","at":now_ms(),"text":message,"ai":true}));
                }
                _ => return err("AI 문안 형식을 확인하지 못했어요."),
            }
            Ok(())
        })
    }

    fn approve_delivery(&self, team: &Team, ctx: &Context, args: &Value) -> Result<()> {
        let draft_id = args.s("draftId");
        let mut turns = self.delivery_source(ctx)?;
        self.conv.update(|data| {
            let row = arr_mut(data, "deliveryDrafts").iter_mut().find(|r| ctx.matches(r)).ok_or("검토할 초안을 다시 열어 주세요.")?;
            let draft = &row["draft"];
            if draft_id.is_empty() || draft["id"] != draft_id { return err("초안이 바뀌었어요. 내용을 다시 확인해 주세요."); }
            if row["approvedId"].is_string() { return Ok(()); }
            turns.extend(serde_json::from_value::<Vec<Turn>>(row["turns"].clone()).map_err(|_| "저장된 초안을 읽지 못했어요.")?);
            self.ai.check_delivery_copy(team, &ctx.kind, &turns, &draft.s("title"), &draft.s("text"))?;
            let receipt = team.queue_ai_delivery(&draft_id, draft, args["toMember"].as_str())?;
            row["approvedId"] = json!(receipt);
            row["revision"] = Value::Null;
            row["canRetry"] = json!(false);
            row["updatedAt"] = json!(now_ms());
            Ok(())
        })
    }
}
