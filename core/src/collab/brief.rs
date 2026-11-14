//! 프로젝트에 연결한 Markdown/텍스트 기획서. 서버에서 공유할 수 없는 이전 규칙에서는
//! 프로젝트 접근 권한을 확인한 뒤 기기 안에만 보관하고 그 상태를 화면에 돌려준다.
use crate::team::{backend::denied, Team};
use crate::{err, Result};
use serde_json::{json, Value};

const MAX_BYTES: usize = 48 * 1024;

fn check_project_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 128 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')) {
        return err("올바른 프로젝트를 선택해 주세요.");
    }
    Ok(())
}

fn validate<'a>(name: &str, text: &'a str) -> Result<&'a str> {
    let ext = name.rsplit_once('.').filter(|(stem, _)| !stem.trim().is_empty()).map(|(_, ext)| ext);
    if name.chars().count() > 200 || name.contains(['/', '\\']) || name.chars().any(char::is_control)
        || !ext.is_some_and(|s| s.eq_ignore_ascii_case("md") || s.eq_ignore_ascii_case("txt"))
    {
        return err(".md 또는 .txt 파일을 선택해 주세요.");
    }
    if text.len() > MAX_BYTES {
        return err("기획서는 UTF-8 텍스트 48KB까지 올릴 수 있어요.");
    }
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() || text.chars().any(|ch| ch.is_control() && !matches!(ch, '\r' | '\n' | '\t')) {
        return err("비어 있거나 읽을 수 없는 기획서예요. UTF-8 텍스트 파일을 선택해 주세요.");
    }
    Ok(text)
}

fn view(doc: &Value, shared: bool) -> Result<Value> {
    let name = doc["name"].as_str().unwrap_or("");
    let text = validate(name, doc["text"].as_str().unwrap_or(""))?;
    Ok(json!({ "name": name, "text": text, "updatedAt": doc["updatedAt"], "shared": shared }))
}

impl Team {
    fn local_project_brief(&self, project: &str, uid: &str) -> Result<Option<Value>> {
        self.cache_get()["localBriefs"][uid].get(project).map(|doc| view(doc, false)).transpose()
    }

    pub async fn get_project_brief(&self, project: &str) -> Result<Option<Value>> {
        check_project_id(project)?;
        let id = self.team_id()?;
        let backend = self.backend()?;
        match backend.get_brief(&id, project).await {
            Ok(Some(doc)) => return Ok(Some(view(&doc, true)?)),
            Ok(None) => {}
            Err(e) if denied(&e) => {
                backend.check_brief_project(&id, project).await?;
            }
            Err(e) => return Err(e),
        }
        self.local_project_brief(project, &backend.sign_in().await?)
    }

    pub async fn save_project_brief(&self, project: &str, name: &str, text: &str) -> Result<Value> {
        check_project_id(project)?;
        let text = validate(name, text)?;
        let id = self.team_id()?;
        let backend = self.backend()?;
        let uid = backend.sign_in().await?;
        let doc = json!({ "name": name, "text": text, "updatedAt": self.now(), "by": uid });
        match backend.put_brief(&id, project, doc.clone()).await {
            Ok(()) => {
                self.cache_update(|cache| {
                    if let Some(local) = cache["localBriefs"][&uid].as_object_mut() {
                        local.remove(project);
                    }
                });
                view(&doc, true)
            }
            Err(e) if denied(&e) => {
                // 실제 팀 탈퇴나 쓰기 전용 제한을 구형 규칙으로 잘못 판단하지 않는다.
                backend.check_brief_project(&id, project).await?;
                if !matches!(backend.get_brief(&id, project).await, Err(ref read) if denied(read)) {
                    return Err(e);
                }
                self.cache_update(|cache| cache["localBriefs"][&uid][project] = doc.clone());
                view(&doc, false)
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Stores;
    use crate::team::{Backend, Hub};
    use crate::util::J;
    use std::sync::{Arc, Mutex};

    #[test]
    fn validates_text_without_damaging_markdown() {
        let md = "# 기획서\r\n\t팀 과제\n";
        assert_eq!(validate("발표.MD", &format!("\u{feff}{md}")).unwrap(), md);
        assert!(validate("과제.txt", &"가".repeat(MAX_BYTES / 3)).is_ok());
        assert!(validate("과제.txt", &"가".repeat(MAX_BYTES / 3 + 1)).is_err());
        for (name, text) in [("a.md", " \t "), ("a.txt", "a\0b"), ("../a.txt", "안녕"), ("plan.txt.exe", "hi")] {
            assert!(validate(name, text).is_err());
        }
        assert!(check_project_id("a/../b").is_err());
    }

    fn hub(data: Arc<Mutex<Value>>, uid: &'static str) -> Hub {
        let stores = Stores::memory();
        Hub::new(stores.settings, stores.shared_outbox, stores.team_cache,
            Box::new(move || Some(Backend::memory_shared(data.clone(), uid))), Some(0), Arc::new(crate::time::now_ms))
    }

    #[tokio::test]
    async fn two_members_share_and_replace_one_project_brief() {
        let data = Backend::new_shared_memory();
        let a = hub(data.clone(), "a");
        let b = hub(data, "b");
        let setup = a.create_team("팀", "유빈").await.unwrap();
        b.join_team(&setup.s("joinCode"), "재연").await.unwrap();
        let t = a.active();
        let p = t.create_project(&json!({"title":"발표"})).await.unwrap();
        let other = t.create_project(&json!({"title":"다른 프로젝트"})).await.unwrap();
        assert!(b.active().get_project_brief(&p).await.unwrap().is_none());
        assert!(t.save_project_brief(&p, "기획.md", "# 발표\n준비").await.unwrap().b("shared"));
        assert_eq!(b.active().get_project_brief(&p).await.unwrap().unwrap().s("text"), "# 발표\n준비");
        b.active().save_project_brief(&p, "수정.txt", "새로운 기획").await.unwrap();
        assert_eq!(t.get_project_brief(&p).await.unwrap().unwrap().s("name"), "수정.txt");
        assert!(t.get_project_brief(&other).await.unwrap().is_none());
        assert!(t.save_project_brief("missing", "기획.md", "x").await.is_err());
    }
}
