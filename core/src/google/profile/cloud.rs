use super::model::{id, Record, LIMIT};
use crate::google::oauth::{GoogleAuth, Lease};
use crate::{err, Result};
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn error(e: (u16, String)) -> String {
    e.1
}
fn name(op: &str) -> String {
    format!("sai-profile-v1-{op}.json")
}
fn parse(value: Value, lease: &Lease) -> Result<Record> {
    let r: Record = serde_json::from_value(value)
        .map_err(|_| "Google 설정 자료의 형식을 확인하지 못했어요.")?;
    if !r.valid(lease.subject()) {
        return err("Google 설정 자료의 계정이나 버전을 확인하지 못했어요.");
    }
    Ok(r)
}
pub async fn read(auth: &GoogleAuth, lease: &Lease) -> Result<Vec<Record>> {
    let mut records = vec![];
    let mut page = String::new();
    let mut pages = BTreeSet::new();
    let mut files = BTreeSet::new();
    loop {
        let query = urlencoding::encode(
            "trashed = false and appProperties has { key='saiProfile' and value='1' }",
        );
        let path = format!("/drive/v3/files?spaces=appDataFolder&pageSize=100&fields=nextPageToken,incompleteSearch,files(id,name)&q={query}&pageToken={}", urlencoding::encode(&page));
        let list = auth
            .profile_request(lease, "GET", &path, None)
            .await
            .map_err(error)?;
        if list["incompleteSearch"] == true {
            return err("Google 설정 목록을 끝까지 확인하지 못했어요.");
        }
        let rows = list["files"]
            .as_array()
            .ok_or("Google 설정 목록의 형식이 올바르지 않아요.")?;
        for file in rows {
            let fid = file["id"]
                .as_str()
                .filter(|s| id(s))
                .ok_or("Google 설정 파일의 주소가 올바르지 않아요.")?;
            if !files.insert(fid.to_owned()) || files.len() > LIMIT {
                return err("Google 설정 목록의 크기나 중복을 확인해 주세요.");
            }
            let value = auth
                .profile_request(
                    lease,
                    "GET",
                    &format!("/drive/v3/files/{fid}?alt=media"),
                    None,
                )
                .await
                .map_err(error)?;
            let record = parse(value, lease)?;
            if file["name"] != name(&record.op_id) {
                return err("Google 설정 파일의 이름과 내용이 일치하지 않아요.");
            }
            records.push(record);
        }
        match list.get("nextPageToken") {
            None => break,
            Some(Value::String(token))
                if !token.is_empty() && token.len() <= 2048 && pages.insert(token.clone()) =>
            {
                page = token.clone()
            }
            _ => return err("Google 설정 목록의 다음 페이지를 확인하지 못했어요."),
        }
    }
    Ok(records)
}
pub async fn generate_id(auth: &GoogleAuth, lease: &Lease) -> Result<String> {
    let value = auth
        .profile_request(
            lease,
            "GET",
            "/drive/v3/files/generateIds?count=1&space=appDataFolder&type=files",
            None,
        )
        .await
        .map_err(error)?;
    value["ids"]
        .as_array()
        .filter(|a| a.len() == 1)
        .and_then(|a| a[0].as_str())
        .filter(|s| id(s))
        .map(str::to_owned)
        .ok_or_else(|| "Google 설정 파일의 저장 주소를 준비하지 못했어요.".into())
}
async fn existing(auth: &GoogleAuth, lease: &Lease, fid: &str, record: &Record) -> Result<bool> {
    match auth
        .profile_request(
            lease,
            "GET",
            &format!("/drive/v3/files/{fid}?alt=media"),
            None,
        )
        .await
    {
        Ok(value) => {
            if parse(value, lease)? == *record {
                Ok(true)
            } else {
                err("같은 주소에 다른 설정 자료가 있어요. 덮어쓰지 않았어요.")
            }
        }
        Err((404, _)) => Ok(false),
        Err(e) => Err(e.1),
    }
}
pub async fn write(auth: &GoogleAuth, lease: &Lease, fid: &str, record: &Record) -> Result<()> {
    if !id(fid) || !record.valid(lease.subject()) {
        return err("저장할 Google 설정을 확인하지 못했어요.");
    }
    if existing(auth, lease, fid, record).await? {
        return Ok(());
    }
    let boundary = format!("sai_{}", crate::google::oauth::random_token());
    let metadata = json!({"id":fid,"name":name(&record.op_id),"mimeType":"application/json","parents":["appDataFolder"],"appProperties":{"saiProfile":"1"}});
    let content = serde_json::to_string(record).map_err(|_| "설정 내용을 준비하지 못했어요.")?;
    let body = format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Type: application/json\r\n\r\n{content}\r\n--{boundary}--\r\n");
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
        Ok(v) if v["id"] == fid => Ok(()),
        Err((409, _)) if existing(auth, lease, fid, record).await? => Ok(()),
        Err(e) => Err(e.1),
        _ => err("Google 설정의 저장 결과를 확인하지 못했어요. 다음에 다시 확인할게요."),
    }
}
