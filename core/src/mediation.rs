//! 팀원 사이 중재 전달의 기기 쪽 기록과 규칙. Drive 입출력은 `google::mediation`이 맡는다.
//!
//! 받는 쪽 한도는 받는 사람의 앱이 직접 건다. 보내는 쪽 앱을 고쳐도 우회할 수 없다.
//! 기록(book)은 이 PC의 개인 대화 저장소 안 `mediation` 키에만 둔다.
use crate::time::iso_week;
use crate::util::J;
use crate::{err, Result};
use serde_json::{json, Value};

/// 보낸 사람 한 명에게서 한 주에 받는 부탁 수.
pub const RECEIVE_LIMIT: usize = 2;
/// 한 사람에게 한 주에 보내는 부탁 수. 받는 쪽 한도와 같게 둬서 보내고도 버려지는 일을 줄인다.
pub const SEND_LIMIT: usize = 2;
pub const MAX_TEXT: usize = 400;
pub const REACTIONS: &[(&str, &str)] = &[("ok", "알겠어"), ("talk", "이야기해 보고 싶어"), ("unsure", "잘 모르겠어")];
const KEEP_ROWS: usize = 100;
const KEEP_SEEN: usize = 1000;

pub fn reaction_label(id: &str) -> Option<&'static str> {
    REACTIONS.iter().find(|(k, _)| *k == id).map(|(_, l)| *l)
}

fn same_email(a: &str, b: &str) -> bool {
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}

fn rows<'a>(book: &'a mut Value, key: &str) -> &'a mut Vec<Value> {
    if !book.is_object() {
        *book = json!({});
    }
    if !book[key].is_array() {
        book[key] = json!([]);
    }
    book[key].as_array_mut().unwrap()
}

fn trim(list: &mut Vec<Value>, keep: usize) {
    let extra = list.len().saturating_sub(keep);
    list.drain(..extra);
}

/// 보낼 문안: 비어 있지 않고 400자 이하, 줄바꿈 말고는 제어 문자나 방향 바꿈 문자가 없다.
pub fn valid_text(text: &str) -> bool {
    !text.trim().is_empty()
        && text.chars().count() <= MAX_TEXT
        && !text.chars().any(|c| {
            (c.is_control() && c != '\n')
                || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
        })
}

pub fn is_muted(book: &Value, from: &str, team: &str) -> bool {
    book["muted"]["people"].as_array().is_some_and(|p| p.iter().any(|e| same_email(e.as_str().unwrap_or(""), from)))
        || book["muted"]["teams"].as_array().is_some_and(|t| t.iter().any(|f| f == team))
}

pub fn set_muted(book: &mut Value, kind: &str, value: &str, on: bool) -> Result<()> {
    let key = match kind {
        "person" => "people",
        "team" => "teams",
        _ => return err("끌 대상을 확인하지 못했어요."),
    };
    if value.is_empty() || value.len() > 254 {
        return err("끌 대상을 확인하지 못했어요.");
    }
    if !book.is_object() {
        *book = json!({});
    }
    if !book["muted"].is_object() {
        book["muted"] = json!({});
    }
    if !book["muted"][key].is_array() {
        book["muted"][key] = json!([]);
    }
    let list = book["muted"][key].as_array_mut().unwrap();
    list.retain(|v| !same_email(v.as_str().unwrap_or(""), value) && v != value);
    if on {
        list.push(json!(value));
    }
    Ok(())
}

/// 이번 주에 이 사람에게 더 보낼 수 있는지.
pub fn can_send(book: &Value, to: &str, now: i64) -> Result<()> {
    let week = iso_week(now);
    let sent = book["sent"]
        .as_array()
        .map(|r| r.iter().filter(|s| same_email(&s.s("to"), to) && iso_week(s.i("at")) == week).count())
        .unwrap_or(0);
    if sent >= SEND_LIMIT {
        return err("이번 주에는 이 팀원에게 더 보낼 수 없어요. 한 사람에게 한 주에 두 번까지 전할 수 있어요.");
    }
    Ok(())
}

pub fn record_sent(book: &mut Value, sent: Value) {
    let list = rows(book, "sent");
    list.push(sent);
    trim(list, KEEP_ROWS);
}

pub fn seen(book: &Value) -> Vec<String> {
    book.strs("seen")
}

fn mark_seen(book: &mut Value, file_id: &str) {
    let list = rows(book, "seen");
    if !list.iter().any(|v| v == file_id) {
        list.push(json!(file_id));
    }
    trim(list, KEEP_SEEN);
}

/// Drive에서 확인한 항목(이미 팀 명단 확인을 마친 것)을 기록에 넣는다.
/// 받은 부탁은 한도와 끄기를 적용하고, 반응은 내가 보낸 부탁의 받는 사람이 보낸 것만 받는다.
/// 반환: 새로 받은 부탁 수, 새로 받은 반응 수, 한도·끄기로 거른 수.
pub fn accept(book: &mut Value, items: &[Value], rejected: &[String], now: i64) -> (usize, usize, usize) {
    let (mut requests, mut reactions, mut held) = (0, 0, 0);
    for file_id in rejected {
        mark_seen(book, file_id);
    }
    for item in items {
        let file_id = item.s("fileId");
        if file_id.is_empty() || seen(book).contains(&file_id) {
            continue;
        }
        mark_seen(book, &file_id);
        let from = item.s("from");
        let team = item.s("team");
        match item.s("type").as_str() {
            "request" => {
                let week = iso_week(now);
                let inbox = book["inbox"].as_array().cloned().unwrap_or_default();
                let this_week = inbox
                    .iter()
                    .filter(|r| same_email(&r.s("from"), &from) && iso_week(r.i("receivedAt")) == week)
                    .count();
                if is_muted(book, &from, &team) || this_week >= RECEIVE_LIMIT || inbox.iter().any(|r| r["id"] == item["id"]) {
                    held += 1;
                    continue;
                }
                let mut row = item.clone();
                row["receivedAt"] = json!(now);
                row["delivered"] = json!(false);
                row["reaction"] = Value::Null;
                let list = rows(book, "inbox");
                list.push(row);
                trim(list, KEEP_ROWS);
                requests += 1;
            }
            "reaction" => {
                let reaction = item.s("reaction");
                if reaction_label(&reaction).is_none() {
                    continue;
                }
                let Some(sent) = rows(book, "sent").iter_mut().find(|s| {
                    s["id"] == item["re"] && same_email(&s.s("to"), &from) && s["team"] == item["team"] && s["reaction"].is_null()
                }) else {
                    continue;
                };
                sent["reaction"] = json!(reaction);
                sent["reactionAt"] = json!(now);
                sent["noticed"] = json!(false);
                reactions += 1;
            }
            _ => {}
        }
    }
    (requests, reactions, held)
}

/// 캐릭터가 아직 건네지 않은 부탁 하나.
pub fn next_request(book: &Value) -> Option<Value> {
    book["inbox"].as_array()?.iter().find(|r| r["delivered"] == false).cloned()
}

/// 보낸 사람에게 아직 알리지 않은 반응 하나.
pub fn next_reaction(book: &Value) -> Option<Value> {
    book["sent"].as_array()?.iter().find(|r| r["noticed"] == false && r["reaction"].is_string()).cloned()
}

pub fn mark(book: &mut Value, key: &str, id: &str, field: &str, value: Value) {
    if let Some(row) = rows(book, key).iter_mut().find(|r| r["id"] == id) {
        row[field] = value;
    }
}

pub fn find(book: &Value, key: &str, id: &str) -> Option<Value> {
    book[key].as_array()?.iter().find(|r| r["id"] == id).cloned()
}

fn name_of(row: &Value, name_key: &str, email_key: &str) -> String {
    let name = row.s(name_key);
    if !name.trim().is_empty() {
        return name.trim().to_owned();
    }
    row.s(email_key).split('@').next().unwrap_or("").to_owned()
}

/// 받는 사람에게 보여 줄 말풍선. 누가 보낸 말인지, 사이가 다듬었다는 것을 먼저 밝힌다.
pub fn request_bubble(row: &Value, cat: &str) -> String {
    let cat = if cat.trim().is_empty() { "사이" } else { cat.trim() };
    format!(
        "{}님이 {cat}에게 부탁한 말이야. {cat}가 부드럽게 다듬었어.\n\n“{}”",
        name_of(row, "fromName", "from"),
        row.s("text")
    )
}

pub fn reaction_bubble(row: &Value) -> String {
    format!(
        "{}님이 전한 부탁에 “{}”라고 답했어.",
        name_of(row, "toName", "to"),
        reaction_label(&row.s("reaction")).unwrap_or("답")
    )
}

/// 화면에 보여 줄 기록. 내부 파일 번호는 빼고 보낸다.
pub fn view(book: &Value) -> Value {
    let pick = |key: &str, fields: &[&str]| -> Vec<Value> {
        book[key]
            .as_array()
            .map(|r| {
                r.iter()
                    .rev()
                    .map(|row| {
                        let mut out = json!({});
                        for f in fields {
                            out[*f] = row[*f].clone();
                        }
                        out
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    json!({
        "inbox": pick("inbox", &["id", "from", "fromName", "team", "teamName", "text", "createdAt", "receivedAt", "reaction"]),
        "sent": pick("sent", &["id", "to", "toName", "team", "teamName", "text", "at", "reaction"]),
        "muted": book["muted"].clone(),
        "receiveLimit": RECEIVE_LIMIT,
        "sendLimit": SEND_LIMIT,
    })
}

#[cfg(test)]
mod tests;
