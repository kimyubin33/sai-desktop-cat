//! Bounded binary transfers to fixed Google endpoints, using one captured account.
use super::{GoogleAuth, Lease, Transport};
use crate::google::files::{Progress, MAX_BYTES};
use serde_json::Value;
use std::{sync::Arc, time::Duration};
type ApiResult<T> = std::result::Result<T, (u16, String)>;
impl GoogleAuth {
    pub(in crate::google) async fn preview_text_bytes(
        &self,
        lease: &Lease,
        file_id: &str,
        export: bool,
    ) -> ApiResult<Vec<u8>> {
        self.media_current(lease)?;
        if !crate::google::profile::model::id(file_id) {
            return Err((0, "파일 주소가 올바르지 않아요.".into()));
        }
        let suffix = if export {
            "/export?mimeType=text%2Fplain"
        } else {
            "?alt=media"
        };
        let mut response = self
            .transport
            .http
            .get(format!(
                "{}/drive/v3/files/{file_id}{suffix}",
                self.transport.api
            ))
            .bearer_auth(&lease.access.token)
            .timeout(Duration::from_secs(45))
            .header("accept-encoding", "identity")
            .send()
            .await
            .map_err(|_| (0, "분석할 파일 내용을 받지 못했어요.".into()))?;
        if response.status().as_u16() != 200 {
            return Err((
                response.status().as_u16(),
                "파일을 텍스트로 읽을 권한과 형식을 확인해 주세요.".into(),
            ));
        }
        const LIMIT: usize = 24 * 1024;
        if response.content_length().is_some_and(|n| n > LIMIT as u64) {
            return Err((
                0,
                "분석할 텍스트는 24KB까지 지원해요. 필요한 부분을 직접 입력해 주세요.".into(),
            ));
        }
        let mut bytes = vec![];
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| (0, "파일 내용을 끝까지 받지 못했어요.".into()))?
        {
            self.media_current(lease)?;
            if bytes.len().saturating_add(chunk.len()) > LIMIT {
                return Err((0,"텍스트가 24KB보다 커서 일부만 보내지 않았어요. 필요한 부분을 직접 입력해 주세요.".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        self.media_current(lease)?;
        Ok(bytes)
    }
    fn media_current(&self, lease: &Lease) -> ApiResult<()> {
        if self.lease_current(lease) {
            Ok(())
        } else {
            Err((
                0,
                "계정 연결이 바뀌었어요. 전송 결과는 현재 계정에 반영하지 않았어요.".into(),
            ))
        }
    }
    pub(in crate::google) async fn upload_media(
        &self,
        lease: &Lease,
        boundary: &str,
        body: Vec<u8>,
        progress: Progress,
    ) -> ApiResult<Value> {
        self.media_current(lease)?;
        if body.len() > MAX_BYTES + 65536 {
            return Err((0, "전송할 파일이 25MB보다 커요.".into()));
        }
        let total = body.len();
        let send_progress = progress.clone();
        let stream = futures_util::stream::unfold((body, 0usize), move |(data, offset)| {
            let progress = send_progress.clone();
            async move {
                if offset == data.len() {
                    return None;
                }
                let end = (offset + 64 * 1024).min(data.len());
                let chunk = data[offset..end].to_vec();
                progress("sending", end as u64, total as u64);
                Some((Ok::<_, std::io::Error>(chunk), (data, end)))
            }
        });
        let response=self.transport.http.post(format!("{}/upload/drive/v3/files?uploadType=multipart&fields=id,name,mimeType,size,parents,trashed,version,sha256Checksum,appProperties,capabilities(canDownload)",self.transport.api))
            .bearer_auth(&lease.access.token).timeout(Duration::from_secs(180))
            .header("content-type",format!("multipart/related; boundary={boundary}"))
            .header("content-length",total).body(reqwest::Body::wrap_stream(stream)).send().await
            .map_err(|_|(0,"파일 전송을 마치지 못했어요. 대기 목록에서 다시 확인할 수 있어요.".into()))?;
        let status = response.status().as_u16();
        let (_, value) = Transport::body(response).await.map_err(|e| (status, e))?;
        self.media_current(lease)?;
        if !(200..300).contains(&status) {
            return Err((
                status,
                format!("Drive 파일 전송을 확인하지 못했어요 ({status})."),
            ));
        }
        progress("verifying", total as u64, total as u64);
        Ok(value)
    }
    pub(in crate::google) async fn download_media(
        &self,
        lease: &Lease,
        file_id: &str,
        progress: Progress,
    ) -> ApiResult<Vec<u8>> {
        self.media_current(lease)?;
        if !crate::google::profile::model::id(file_id) {
            return Err((0, "파일 주소가 올바르지 않아요.".into()));
        }
        let mut response = self
            .transport
            .http
            .get(format!(
                "{}/drive/v3/files/{file_id}?alt=media",
                self.transport.api
            ))
            .bearer_auth(&lease.access.token)
            .timeout(Duration::from_secs(180))
            .header("accept-encoding", "identity")
            .send()
            .await
            .map_err(|_| {
                (
                    0,
                    "파일을 내려받지 못했어요. 저장된 원본은 바꾸지 않았어요.".into(),
                )
            })?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err((
                status,
                format!("Drive 파일을 내려받지 못했어요 ({status})."),
            ));
        }
        let total = response.content_length().unwrap_or(0);
        if total > MAX_BYTES as u64 {
            return Err((0, "25MB보다 큰 파일은 Drive에서 열어 주세요.".into()));
        }
        let mut bytes = Vec::with_capacity((total as usize).min(MAX_BYTES));
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| (0, "파일을 끝까지 받지 못했어요. 다시 시도해 주세요.".into()))?
        {
            self.media_current(lease)?;
            if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
                return Err((0, "파일 크기가 전송 한도를 넘었어요.".into()));
            }
            bytes.extend_from_slice(&chunk);
            progress("receiving", bytes.len() as u64, total);
        }
        self.media_current(lease)?;
        Ok(bytes)
    }
}
pub(in crate::google) fn silent() -> Progress {
    Arc::new(|_, _, _| {})
}
