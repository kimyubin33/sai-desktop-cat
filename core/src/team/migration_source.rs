//! 이전 미리보기는 공개 shares만 새로 읽는다. 화면 캐시·비공개 문안·연동 문서를 읽지 않는다.
use super::*;
use crate::team::mesh::migration::{Snapshot, Source, MAX_BYTES, MAX_ROWS};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

fn segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
impl Firebase {
    fn migration_decode(&self, doc: &Value, path: &str, collection: bool) -> Result<Value> {
        let root = self
            .docs_base
            .split_once("/v1/")
            .ok_or("기존 서버의 문서 경로를 확인하지 못했어요.")?
            .1;
        let expected = format!("{root}/{path}");
        let name = doc
            .get("name")
            .and_then(Value::as_str)
            .ok_or("기존 문서의 원본 경로가 없어요.")?;
        let valid = if collection {
            name.strip_prefix(&format!("{expected}/"))
                .is_some_and(segment)
        } else {
            name == expected
        };
        if !valid || doc.get("fields").is_some_and(|f| !f.is_object()) {
            return err("조회한 팀·공개 목록과 다른 문서가 포함됐어요.");
        }
        // decode_doc가 덮어쓰는 별도 id 필드로 원본 ID를 혼동하지 않게 한다.
        if doc.get("fields").is_some_and(|f| f.get("id").is_some()) {
            return err("기존 문서의 ID 필드를 확인해 주세요.");
        }
        decode_doc(doc).ok_or_else(|| "기존 팀 문서 형식이 손상됐어요.".into())
    }
    async fn migration_get(&self, path: &str, query: &str) -> Result<Option<Value>> {
        let token = self.token().await?;
        let url = format!("{}{}{}", self.docs_base, request_path("GET", path), query);
        let mut request = super::super::desktop::client(&url)
            .get(&url)
            .bearer_auth(token);
        if let Some(id) = &self.server_id {
            request = request.header("X-Sai-Server-Id", id);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| "기존 팀 서버에 연결하지 못했어요.")?;
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|n| n > 2 * 1024 * 1024)
        {
            return err("이전 조회 응답의 크기 한도를 넘었어요.");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "기존 팀 목록을 읽는 중 연결이 끊겼어요.")?
        {
            if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                return err("이전 조회 응답의 크기 한도를 넘었어요.");
            }
            bytes.extend_from_slice(&chunk);
        }
        if status.as_u16() == 404 {
            return Ok(None);
        }
        if !status.is_success() {
            return err(format!(
                "기존 팀 이전 조회 권한 또는 연결을 확인해 주세요 ({status})."
            ));
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| "기존 팀 서버 응답 형식이 올바르지 않아요.".into())
    }
}
impl Backend {
    pub(crate) fn migration_scope(&self) -> String {
        let raw = match self {
            Backend::Firebase(f) => format!(
                "{}\0{}\0{}",
                self.kind(),
                f.docs_base,
                f.server_id.as_deref().unwrap_or("")
            ),
            Backend::Memory(m) => match &m.file {
                Some(path) => format!("local-file\0{}", path.to_string_lossy()),
                None => format!("memory\0{:p}", Arc::as_ptr(&m.data)),
            },
        };
        format!(
            "{:x}",
            Sha256::digest(format!("sai-mesh/legacy-source/1\0{raw}"))
        )
    }
    async fn migration_document(&self, path: &str) -> Result<Option<Value>> {
        match self {
            Backend::Firebase(f) => f
                .migration_get(&format!("/{path}"), "")
                .await?
                .map(|d| f.migration_decode(&d, path, false))
                .transpose(),
            Backend::Memory(_) => self.get_doc(path).await,
        }
    }
    pub(crate) async fn migration_snapshot(&self, team: &str) -> Result<Snapshot> {
        if !segment(team) {
            return err("기존 팀 ID를 확인해 주세요.");
        }
        let uid = self.sign_in().await?;
        if !segment(&uid) {
            return err("기존 팀 계정을 확인해 주세요.");
        }
        let verify = async {
            let member = self
                .migration_document(&format!("teams/{team}/members/{uid}"))
                .await?;
            let admin = self
                .migration_document(&format!("teams/{team}/admins/{uid}"))
                .await?;
            if member.is_none() || admin.is_none() {
                return err("기존 팀의 현재 관리자 계정으로 연결해 주세요.");
            }
            Ok(())
        };
        verify.await?;
        let root = self
            .migration_document(&format!("teams/{team}"))
            .await?
            .ok_or("기존 팀을 찾지 못했어요.")?;
        if root.b("closed") {
            return err("해산한 팀은 이 화면에서 이전하지 않아요.");
        }
        let name = root.s("name");
        if name.trim().is_empty() || name.chars().count() > 200 {
            return err("기존 팀 이름을 확인하지 못했어요.");
        }
        let mut rows = Vec::new();
        let mut total = 0usize;
        match self {
            Backend::Firebase(f) => {
                let mut page = String::new();
                let mut seen = BTreeSet::new();
                loop {
                    if seen.len() >= 16 {
                        return err("이전 목록 페이지 한도를 넘었어요.");
                    }
                    let query = format!(
                        "?pageSize=100{}",
                        if page.is_empty() {
                            String::new()
                        } else {
                            format!("&pageToken={}", urlencoding::encode(&page))
                        }
                    );
                    let response = f
                        .migration_get(&format!("/teams/{team}/shares"), &query)
                        .await?
                        .ok_or("기존 공개 톡 목록을 찾지 못했어요.")?;
                    if !response.is_object() {
                        return err("기존 공개 톡 목록 형식이 올바르지 않아요.");
                    }
                    match response.get("documents") {
                        Some(Value::Array(docs)) => {
                            for doc in docs {
                                let row =
                                    f.migration_decode(doc, &format!("teams/{team}/shares"), true)?;
                                total = total
                                    .checked_add(row.to_string().len())
                                    .ok_or("이전 자료가 너무 커요.")?;
                                if rows.len() >= MAX_ROWS || total > MAX_BYTES {
                                    return err(
                                        "이번 이전 조회는 공개 문서 1,000개·4 MiB까지 지원해요.",
                                    );
                                }
                                rows.push(row);
                            }
                        }
                        None => {}
                        _ => return err("기존 공개 톡 목록이 손상됐어요."),
                    }
                    page = match response.get("nextPageToken") {
                        None => String::new(),
                        Some(Value::String(s)) if s.len() <= 4096 => s.clone(),
                        _ => return err("다음 이전 페이지 정보가 올바르지 않아요."),
                    };
                    if page.is_empty() {
                        break;
                    }
                    if !seen.insert(page.clone()) {
                        return err("이전 목록의 페이지가 반복돼요.");
                    }
                }
            }
            Backend::Memory(_) => {
                rows = self.list_shares(team).await?;
                total = rows.iter().map(|r| r.to_string().len()).sum();
            }
        }
        if rows.len() > MAX_ROWS || total > MAX_BYTES {
            return err("이번 이전 조회는 공개 문서 1,000개·4 MiB까지 지원해요.");
        }
        if self.sign_in().await? != uid
            || self
                .migration_document(&format!("teams/{team}/members/{uid}"))
                .await?
                .is_none()
            || self
                .migration_document(&format!("teams/{team}/admins/{uid}"))
                .await?
                .is_none()
        {
            return err("조회 중 기존 계정 또는 관리자 권한이 바뀌었어요.");
        }
        Snapshot::from_rows(
            Source {
                scope: self.migration_scope(),
                kind: self.kind().into(),
                team_id: team.into(),
                team_name: name,
                reader_uid: uid,
            },
            rows,
            crate::time::now_ms(),
        )
    }
}

#[cfg(test)]
#[path = "migration_source_tests.rs"]
mod tests;
