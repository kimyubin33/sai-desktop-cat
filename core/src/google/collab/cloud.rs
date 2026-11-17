use super::{
    model::{Record, LIMIT},
    Remote,
};
use crate::{
    err,
    google::{
        oauth::{GoogleAuth, Lease},
        profile::model::id,
    },
    Result,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
const FIELDS: &str = "id,name,mimeType,parents,trashed,version,properties";
fn name(op: &str) -> String {
    format!("sai-work-v1-{op}.json")
}
fn meta(value: &Value, folder: &str) -> Result<(String, String)> {
    let fid = value["id"]
        .as_str()
        .filter(|s| id(s))
        .ok_or("협업 파일 주소가 올바르지 않아요.")?;
    let revision = value["version"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_digit()))
        .ok_or("협업 파일 변경 번호를 확인하지 못했어요.")?;
    if value["trashed"] != false
        || value["mimeType"] != "application/json"
        || value["properties"]["saiCollab"] != "1"
        || !value["parents"]
            .as_array()
            .is_some_and(|v| v.iter().any(|s| s == folder))
    {
        return err("선택한 팀의 협업 파일만 사용할 수 있어요.");
    }
    Ok((fid.into(), revision.into()))
}
async fn content(auth: &GoogleAuth, lease: &Lease, fid: &str, folder: &str) -> Result<Record> {
    let value = auth
        .profile_request(
            lease,
            "GET",
            &format!("/drive/v3/files/{fid}?alt=media"),
            None,
        )
        .await
        .map_err(|e| e.1)?;
    let record: Record =
        serde_json::from_value(value).map_err(|_| "협업 파일의 내용을 읽지 못했어요.")?;
    if !record.valid(folder) {
        return err("협업 기록의 팀·버전·내용이 올바르지 않아요.");
    }
    Ok(record)
}
pub async fn read(
    auth: &GoogleAuth,
    lease: &Lease,
    folder: &str,
    cache: &[Remote],
) -> Result<Vec<Remote>> {
    let cached: BTreeMap<_, _> = cache.iter().map(|r| (r.file_id.as_str(), r)).collect();
    let q=format!("'{folder}' in parents and trashed = false and properties has {{ key='saiCollab' and value='1' }}");
    let mut rows = vec![];
    let mut seen = BTreeSet::new();
    let mut ops = BTreeSet::new();
    let mut pages = BTreeSet::new();
    let mut page = String::new();
    loop {
        let list=auth.profile_request(lease,"GET",&format!("/drive/v3/files?spaces=drive&pageSize=50&fields=nextPageToken,incompleteSearch,files({FIELDS})&q={}&pageToken={}",urlencoding::encode(&q),urlencoding::encode(&page)),None).await.map_err(|e|e.1)?;
        if list["incompleteSearch"] == true {
            return err("협업 목록을 끝까지 확인하지 못했어요.");
        }
        for value in list["files"]
            .as_array()
            .filter(|v| v.len() <= 50)
            .ok_or("협업 파일 목록 형식이 올바르지 않아요.")?
        {
            let (fid, revision) = meta(value, folder)?;
            if !seen.insert(fid.clone()) || seen.len() > LIMIT {
                return err("협업 목록이 중복되거나 보관 한도를 넘었어요.");
            }
            let record = match cached.get(fid.as_str()) {
                Some(old) if old.revision == revision => old.record.clone(),
                old => {
                    let r = content(auth, lease, &fid, folder).await?;
                    if old.is_some_and(|old| old.record != r) {
                        return err(
                            "이미 확인한 협업 기록이 Drive에서 바뀌었어요. 덮어쓰지 않았어요.",
                        );
                    }
                    r
                }
            };
            if value["name"] != name(&record.op_id) || !ops.insert(record.op_id.clone()) {
                return err("협업 기록의 이름이나 중복을 확인해 주세요.");
            }
            rows.push(Remote {
                file_id: fid,
                revision,
                record,
            });
        }
        match list.get("nextPageToken") {
            None => break,
            Some(Value::String(token))
                if !token.is_empty() && token.len() <= 2048 && pages.insert(token.clone()) =>
            {
                page = token.clone()
            }
            _ => return err("협업 목록의 다음 페이지를 확인하지 못했어요."),
        }
    }
    if cache.iter().any(|r| !seen.contains(&r.file_id)) {
        return err("이전에 확인한 협업 기록 일부가 보이지 않아요. 삭제·공유 권한·동기화 상태를 확인해 주세요.");
    }
    Ok(rows)
}
pub async fn generate(auth: &GoogleAuth, lease: &Lease) -> Result<String> {
    let value = auth
        .profile_request(
            lease,
            "GET",
            "/drive/v3/files/generateIds?count=1&space=drive&type=files",
            None,
        )
        .await
        .map_err(|e| e.1)?;
    value["ids"]
        .as_array()
        .filter(|a| a.len() == 1)
        .and_then(|a| a[0].as_str())
        .filter(|s| id(s))
        .map(str::to_owned)
        .ok_or("협업 저장 주소를 준비하지 못했어요.".into())
}
async fn existing(
    auth: &GoogleAuth,
    lease: &Lease,
    fid: &str,
    record: &Record,
) -> Result<Option<Remote>> {
    let value = match auth
        .profile_request(
            lease,
            "GET",
            &format!("/drive/v3/files/{fid}?fields={FIELDS}"),
            None,
        )
        .await
    {
        Ok(v) => v,
        Err((404, _)) => return Ok(None),
        Err(e) => return Err(e.1),
    };
    let (got, revision) = meta(&value, &record.folder_id)?;
    if got != fid
        || value["name"] != name(&record.op_id)
        || content(auth, lease, fid, &record.folder_id).await? != *record
    {
        return err("같은 저장 주소에 다른 협업 내용이 있어요. 덮어쓰지 않았어요.");
    }
    Ok(Some(Remote {
        file_id: got,
        revision,
        record: record.clone(),
    }))
}
pub async fn write(auth: &GoogleAuth, lease: &Lease, fid: &str, record: &Record) -> Result<Remote> {
    if let Some(found) = existing(auth, lease, fid, record).await? {
        return Ok(found);
    }
    let boundary = format!("sai_{}", crate::google::oauth::random_token());
    let metadata = json!({"id":fid,"name":name(&record.op_id),"mimeType":"application/json","parents":[record.folder_id],"properties":{"saiCollab":"1"}});
    let content = serde_json::to_string(record).map_err(|_| "협업 내용을 준비하지 못했어요.")?;
    let body=format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Type: application/json\r\n\r\n{content}\r\n--{boundary}--\r\n");
    match auth
        .profile_request(
            lease,
            "POST",
            "/upload/drive/v3/files?uploadType=multipart&fields=id",
            Some((
                format!("multipart/related; boundary={boundary}"),
                body.into_bytes(),
            )),
        )
        .await
    {
        Ok(value) if value["id"] == fid => (),
        Err((409, _)) => (),
        Err(e) => return Err(e.1),
        _ => return err("협업 저장 결과를 확인하지 못했어요. 대기 기록을 유지했어요."),
    }
    existing(auth, lease, fid, record)
        .await?
        .ok_or("협업 내용의 저장 결과를 확인하지 못했어요. 대기 기록을 유지했어요.".into())
}
