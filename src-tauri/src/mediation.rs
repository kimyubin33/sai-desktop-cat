//! 팀원 사이 중재 전달: 화면 요청 처리와 받은 말 확인.
//! 규칙은 `sai_core::mediation`, Drive 입출력은 `Google`, 판단·문안·검토는 사용자의 Claude 키로 한다.
use crate::app::App;
use sai_core::counseling::Turn;
use sai_core::mediation as book;
use sai_core::time::now_ms;
use sai_core::util::J;
use serde_json::{json, Value};
use std::sync::Arc;

type R = Result<Value, String>;

const MAX_NOTE: usize = 2000;

pub fn read(a: &App) -> Value {
    a.stores.private_conv.read(|d| d["mediation"].clone())
}

pub fn update<T>(a: &App, f: impl FnOnce(&mut Value) -> T) -> T {
    a.stores.private_conv.update(|d| f(&mut d["mediation"]))
}

pub fn save(a: &App) -> Result<(), String> {
    a.stores
        .private_conv
        .try_flush()
        .map_err(|_| "기기에 저장하지 못했어요. 저장 공간과 파일 상태를 확인해 주세요.".into())
}

pub fn cat_name(a: &App) -> String {
    a.stores.settings.read(|s| s.s("catName"))
}

async fn status(a: &App) -> Result<(u64, Value), String> {
    let st = a.google.team_status().await?;
    if st["signedIn"] != true {
        return Err("Google 계정에 로그인하면 팀원에게 말을 전할 수 있어요.".into());
    }
    let epoch = st["epoch"].as_u64().ok_or("계정 연결을 확인해 주세요.")?;
    Ok((epoch, st))
}

fn epoch_arg(x: &Value) -> Result<u64, String> {
    x["epoch"].as_u64().ok_or_else(|| "팀 목록을 다시 열어 주세요.".into())
}

/// 상담 대화(개인 AI 대화)와 사용자가 덧붙인 요점. 안전 확인 중인 대화는 쓰지 않는다.
fn turns(a: &App, x: &Value) -> Result<Vec<Turn>, String> {
    let session = a
        .chat
        .find(&x.s("sessionId"))
        .filter(|s| s.mode == "free")
        .ok_or("전할 말을 정리할 상담 대화를 다시 열어 주세요.")?;
    if session.safety_hold.is_some() {
        return Err("안전 확인이 필요한 대화는 팀원에게 전할 수 없어요.".into());
    }
    let mut turns = session.turns;
    let note = x.s("note");
    let note = note.trim();
    if note.chars().count() > MAX_NOTE {
        return Err("덧붙이는 말은 2,000자까지 쓸 수 있어요.".into());
    }
    if !note.is_empty() {
        turns.push(Turn { role: "user".into(), text: note.into(), at: now_ms(), ..Default::default() });
    }
    Ok(turns)
}

pub fn view(a: &App) -> Value {
    let mut v = book::view(&read(a));
    v["catName"] = json!(cat_name(a));
    v
}

/// 나와 공유된 사이 메시지를 확인해 기록에 넣는다. 반환: (새 부탁, 새 반응, 거른 수)
pub async fn sync(a: &App) -> Result<(usize, usize, usize), String> {
    let (epoch, st) = status(a).await?;
    if st["driveFull"] != true {
        return Err("받은 말을 확인하려면 팀 화면에서 Google 연결을 갱신해 Drive 접근을 허용해 주세요.".into());
    }
    let seen = book::seen(&read(a));
    let got = a.google.fetch_mediated(epoch, &seen).await?;
    let items = got["items"].as_array().cloned().unwrap_or_default();
    let rejected = got.strs("rejected");
    let counts = update(a, |b| book::accept(b, &items, &rejected, now_ms()));
    save(a)?;
    Ok(counts)
}

/// 받은 부탁에 반응한다. 반응은 보낸 사람 한 명에게만 공유하는 파일로 돌아간다.
pub async fn react(a: &App, id: &str, reaction: &str) -> Result<(), String> {
    if book::reaction_label(reaction).is_none() {
        return Err("반응을 다시 골라 주세요.".into());
    }
    let row = book::find(&read(a), "inbox", id).ok_or("받은 말을 다시 확인해 주세요.")?;
    if row["reaction"].is_string() {
        return Err("이미 답한 말이에요.".into());
    }
    let (epoch, _) = status(a).await?;
    a.google
        .react_mediated(epoch, &row.s("team"), &row.s("from"), id, reaction)
        .await?;
    update(a, |b| {
        book::mark(b, "inbox", id, "reaction", json!(reaction));
        book::mark(b, "inbox", id, "delivered", json!(true));
    });
    save(a)
}

pub async fn handle(a: &Arc<App>, ch: &str, x: &Value) -> R {
    if let Some(e) = a.stores.private_conv.load_error() {
        return Err(e);
    }
    match ch {
        "mediation:targets" => {
            let (epoch, _) = status(a).await?;
            a.google.mediation_targets(epoch).await
        }
        "mediation:draft" => {
            let epoch = epoch_arg(x)?;
            let person = a
                .google
                .mediation_recipient(epoch, &x.s("folderId"), &x.s("email"))
                .await?;
            let turns = turns(a, x)?;
            let mut reply = a.ai.mediation_draft(&turns, &person.strs("names")).await?;
            reply["toName"] = person["name"].clone();
            reply["teamName"] = person["teamName"].clone();
            Ok(reply)
        }
        "mediation:send" => {
            let epoch = epoch_arg(x)?;
            let to = x.s("email");
            let text = x.s("text");
            let text = text.trim();
            book::can_send(&read(a), &to, now_ms())?;
            let person = a.google.mediation_recipient(epoch, &x.s("folderId"), &to).await?;
            let turns = turns(a, x)?;
            // 사용자가 고친 문안도 같은 규칙으로 검사하고, 대화 원문 없이 한 번 더 검토한다.
            a.ai.check_mediation_copy(&turns, &person.strs("names"), text)?;
            a.ai.review_mediation(text).await?;
            let sent = a.google.send_mediated(epoch, &x.s("folderId"), &to, text).await?;
            update(a, |b| book::record_sent(b, sent));
            save(a)?;
            Ok(view(a))
        }
        "mediation:book" => Ok(view(a)),
        "mediation:check" => {
            let (requests, reactions, held) = sync(a).await?;
            let mut v = view(a);
            v["checked"] = json!({"requests":requests,"reactions":reactions,"held":held});
            Ok(v)
        }
        "mediation:react" => {
            react(a, &x.s("id"), &x.s("reaction")).await?;
            Ok(view(a))
        }
        "mediation:mute" => {
            update(a, |b| book::set_muted(b, &x.s("kind"), &x.s("value"), x.b("on")))?;
            save(a)?;
            Ok(view(a))
        }
        _ => Err("지원하지 않는 기능이에요.".into()),
    }
}
