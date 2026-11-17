use super::*;
#[derive(Clone)]
pub(in crate::google) struct TextSource {
    pub file_id: String,
    pub name: String,
    pub version: String,
    pub text: String,
}
pub(super) fn supported(meta: &Value) -> bool {
    let kind = meta["mimeType"].as_str().unwrap_or("");
    kind == "application/vnd.google-apps.document"
        || ["text/plain", "text/markdown", "text/csv"].contains(&kind)
        || {
            let name = meta["name"].as_str().unwrap_or("").to_ascii_lowercase();
            !kind.starts_with("application/vnd.google-apps.")
                && [".txt", ".md", ".csv"]
                    .iter()
                    .any(|ext| name.ends_with(ext))
        }
}
pub(in crate::google) fn validate_text(value: &str) -> Result<()> {
    if value.trim().is_empty()
        || value.len() > 24 * 1024
        || value
            .chars()
            .any(|c| c.is_control() && !['\n', '\r', '\t'].contains(&c))
    {
        return err("비어 있지 않은 UTF-8 텍스트를 24KB 이내로 준비해 주세요.");
    }
    Ok(())
}
impl Files {
    pub async fn text_source(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        fid: &str,
    ) -> Result<TextSource> {
        let lease = auth.full_drive_lease(epoch).await?;
        teams.access(auth, profiles, epoch, folder).await?;
        let before = metadata(auth, &lease, fid).await.map_err(api_error)?;
        file_view(&before, folder, Some(fid))?;
        if !supported(&before) || before["capabilities"]["canDownload"] != true {
            return err("Google 문서·TXT·MD·CSV 중 읽기 가능한 파일을 선택해 주세요. 다른 형식은 필요한 텍스트를 직접 입력할 수 있어요.");
        }
        let export = before["mimeType"] == "application/vnd.google-apps.document";
        if !export && size(&before)? > 24 * 1024 {
            return err("분석할 텍스트는 24KB까지 지원해요. 필요한 부분을 직접 입력해 주세요.");
        }
        let bytes = auth
            .preview_text_bytes(&lease, fid, export)
            .await
            .map_err(api_error)?;
        if !export {
            if bytes.len() as u64 != size(&before)?
                || checksum(&before)?.is_some_and(|expected| digest(&bytes) != expected)
            {
                return err("파일 크기나 내용 확인에 실패했어요. 텍스트를 보내지 않았어요.");
            }
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| "UTF-8 텍스트를 읽지 못했어요. 필요한 내용을 직접 입력해 주세요.")?;
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned();
        validate_text(&text)?;
        let source = TextSource {
            file_id: fid.into(),
            name: before["name"].as_str().unwrap().into(),
            version: before["version"].as_str().unwrap().into(),
            text,
        };
        self.check_text_source(auth, profiles, teams, epoch, folder, &source)
            .await?;
        auth.with_lease(&lease, || Ok(source)).await
    }
    pub async fn check_text_source(
        &self,
        auth: &GoogleAuth,
        profiles: &Profiles,
        teams: &Teams,
        epoch: u64,
        folder: &str,
        source: &TextSource,
    ) -> Result<()> {
        let lease = auth.full_drive_lease(epoch).await?;
        teams.access(auth, profiles, epoch, folder).await?;
        let meta = metadata(auth, &lease, &source.file_id)
            .await
            .map_err(api_error)?;
        file_view(&meta, folder, Some(&source.file_id))?;
        if meta["version"] != source.version
            || meta["capabilities"]["canDownload"] != true
            || !supported(&meta)
        {
            return err(
                "미리보기 이후 원본이나 읽기 권한이 바뀌었어요. 파일을 다시 선택해 주세요.",
            );
        }
        auth.with_lease(&lease, || Ok(())).await
    }
}
