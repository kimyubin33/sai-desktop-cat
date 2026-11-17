//! The installed preview has its own identity and cannot update or register the released app.
pub const BUILD: bool = cfg!(feature = "preview");
pub const TITLE: &str = "사이 미리보기";

pub fn isolated() -> bool {
    BUILD || std::env::var_os("SAI_PREVIEW_DIR").is_some()
}

pub fn validate_identity(
    preview: bool,
    identifier: &str,
    product: Option<&str>,
    binary: Option<&str>,
) -> Result<(), &'static str> {
    let configured = identifier == "com.kimyubin33.sai.preview";
    if preview != configured || preview && (product != Some(TITLE) || binary != Some("sai-preview"))
    {
        return Err("미리보기 앱의 빌드와 설치 정보가 일치하지 않습니다.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_and_release_cannot_start_with_each_others_identity() {
        assert!(validate_identity(true, "com.kimyubin33.sai", Some("사이"), Some("sai")).is_err());
        assert!(validate_identity(
            false,
            "com.kimyubin33.sai.preview",
            Some(TITLE),
            Some("sai-preview")
        )
        .is_err());
        assert!(validate_identity(false, "com.kimyubin33.sai", Some("사이"), Some("sai")).is_ok());
    }
    #[test]
    fn preview_requires_its_distinct_binary_and_installer_name() {
        let id = "com.kimyubin33.sai.preview";
        assert!(validate_identity(true, id, Some(TITLE), Some("sai")).is_err());
        assert!(validate_identity(true, id, Some("사이"), Some("sai-preview")).is_err());
        assert!(validate_identity(true, id, Some(TITLE), Some("sai-preview")).is_ok());
    }
}
