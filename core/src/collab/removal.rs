//! 공동 프로젝트 삭제와 개인별 목록 숨기기를 분리한다.
use super::brief::check_project_id;
use crate::team::Team;
use crate::util::{arr_mut, J};
use crate::{err, Result};
use serde_json::{json, Value};

pub(super) fn select_visible(d: &mut Value, uid: &str) {
    let hidden = d["hiddenProjects"][uid].as_object();
    let mut visible = d.arr("projects").into_iter().filter(|p| !hidden.is_some_and(|h| h.contains_key(&p.s("id"))));
    if !visible.clone().any(|p| p["id"] == d["activeProjectId"]) {
        d["activeProjectId"] = visible.next().map(|p| p["id"].clone()).unwrap_or(Value::Null);
    }
}

impl Team {
    pub async fn delete_my_profile(&self) -> Result<()> {
        let uid = self.backend()?.delete_own_profile(&self.team_id()?).await?;
        self.collab_update(|d| arr_mut(d, "profiles").retain(|p| p["uid"] != uid));
        Ok(())
    }

    pub fn set_project_hidden(&self, pid: &str, hide: bool) -> Result<()> {
        check_project_id(pid)?;
        let uid = self.my_uid();
        if uid.is_empty() { return err("팀에 다시 연결해 주세요."); }
        self.collab_update(|d| {
            if !d.arr("projects").iter().any(|p| p["id"] == pid) { return err("프로젝트를 찾을 수 없어요."); }
            if hide {
                d["hiddenProjects"][&uid][pid] = json!(true);
            } else if let Some(hidden) = d["hiddenProjects"][&uid].as_object_mut() {
                hidden.remove(pid);
                d["activeProjectId"] = json!(pid);
            }
            select_visible(d, &uid);
            Ok(())
        })
    }

    pub async fn delete_project(&self, pid: &str) -> Result<()> {
        check_project_id(pid)?;
        self.backend()?.delete_project_tree(&self.team_id()?, pid).await?;
        let uid = self.my_uid();
        self.cache_update(|cache| {
            if let Some(users) = cache["localBriefs"].as_object_mut() {
                for briefs in users.values_mut().filter_map(Value::as_object_mut) { briefs.remove(pid); }
            }
            let d = &mut cache["collab"];
            arr_mut(d, "projects").retain(|p| p["id"] != pid);
            for k in ["tasks", "files"] {
                if let Some(items) = d[k].as_object_mut() { items.remove(pid); }
            }
            if let Some(users) = d["hiddenProjects"].as_object_mut() {
                for hidden in users.values_mut().filter_map(Value::as_object_mut) { hidden.remove(pid); }
            }
            select_visible(d, &uid);
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Stores;
    use crate::team::{Backend, Hub};
    use std::sync::{Arc, Mutex};

    fn hub(data: Arc<Mutex<Value>>, uid: &'static str) -> Hub {
        let st = Stores::memory();
        Hub::new(st.settings, st.shared_outbox, st.team_cache, Box::new(move || Some(Backend::memory_shared(data.clone(), uid))),
            Some(0), Arc::new(crate::time::now_ms))
    }

    #[tokio::test]
    async fn own_records_removal_and_project_visibility() {
        let db = Backend::new_shared_memory();
        let a = hub(db.clone(), "a");
        let b = hub(db, "b");
        let setup = a.create_team("팀", "유빈").await.unwrap();
        b.join_team(&setup.s("joinCode"), "병헌").await.unwrap();
        let a = a.active();
        let b = b.active();
        let p = a.create_project(&json!({"title":"발표"})).await.unwrap();
        let keep = a.create_project(&json!({"title":"다른 프로젝트"})).await.unwrap();
        let b_brief = a.save_project_brief(&p,"기획.txt","첫 안").await.unwrap();
        assert!(b.delete_project_brief(&p, &b_brief).await.is_err());
        let new_brief = b.save_project_brief(&p, "기획.txt", "새 안").await.unwrap();
        assert!(a.delete_project_brief(&p, &b_brief).await.is_err());
        b.delete_project_brief(&p, &new_brief).await.unwrap();
        assert!(a.get_project_brief(&p).await.unwrap().is_none());
        b.save_project_brief(&p, "기획.txt", "최종안").await.unwrap();
        b.add_task(&p,&json!({"title":"자료 찾기"})).await.unwrap();
        b.add_file(&p,&json!({"name":"참고","url":"https://example.org/plan"})).await.unwrap();
        a.save_profile(&json!({"pr":"개발"})).await.unwrap();
        b.save_profile(&json!({"pr":"설계"})).await.unwrap();
        a.delete_my_profile().await.unwrap();
        assert!(a.my_profile().is_none());
        b.collab_sync().await.unwrap();
        assert!(b.my_profile().is_some());
        a.set_active_project(&p);
        a.set_project_hidden(&p,true).unwrap();
        a.collab_sync().await.unwrap();
        assert_eq!(a.active_project().unwrap()["id"],keep);
        assert!(b.project(&p).is_some());
        a.set_project_hidden(&p,false).unwrap();
        assert_eq!(a.active_project().unwrap()["id"],p);
        assert!(b.delete_project(&p).await.is_err());
        assert!(a.get_project_brief(&p).await.unwrap().is_some());
        a.delete_project(&p).await.unwrap();
        a.delete_project(&p).await.unwrap();
        b.collab_sync().await.unwrap();
        assert!(b.project(&p).is_none());
        assert!(b.project(&keep).is_some());
        let backend = b.backend().unwrap();
        let tid = b.team_id().unwrap();
        assert!(backend.list_tasks(&tid,&p).await.unwrap().is_empty());
        assert!(backend.list_files(&tid,&p).await.unwrap().is_empty());
    }
}
