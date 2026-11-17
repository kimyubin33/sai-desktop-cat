use super::{MAX_BYTES, MAX_PENDING};
use crate::google::profile::model::id;
use crate::{err, store::Store, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::PathBuf,
    sync::Mutex,
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Upload {
    pub operation: String,
    pub folder_id: String,
    pub file_id: Option<String>,
    pub name: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ledger {
    version: u32,
    subject: String,
    uploads: Vec<Upload>,
}
pub struct Outbox {
    root: PathBuf,
    stores: Mutex<BTreeMap<String, Store>>,
}
impl Outbox {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            stores: Mutex::new(BTreeMap::new()),
        }
    }
    fn dir(&self, subject: &str) -> PathBuf {
        self.root
            .join(format!("{:x}", Sha256::digest(subject.as_bytes())))
    }
    fn resolved_dir(&self, subject: &str) -> Result<PathBuf> {
        let root = self
            .root
            .canonicalize()
            .map_err(|_| "전송 보관 폴더를 읽지 못했어요.")?;
        let dir = self
            .dir(subject)
            .canonicalize()
            .map_err(|_| "계정의 전송 보관 폴더를 읽지 못했어요.")?;
        if dir.parent() != Some(root.as_path()) {
            return err("전송 보관 폴더가 다른 위치를 가리켜 중단했어요.");
        }
        Ok(dir)
    }
    fn ledger<T>(
        &self,
        subject: &str,
        save: bool,
        f: impl FnOnce(&mut Ledger) -> Result<T>,
    ) -> Result<T> {
        let mut stores = self.stores.lock().unwrap();
        let store = stores.entry(subject.into()).or_insert_with(|| {
            let path = self.dir(subject).join("uploads.json");
            let defaults = if path.exists() {
                Value::Null
            } else {
                serde_json::to_value(Ledger {
                    version: 1,
                    subject: subject.into(),
                    uploads: vec![],
                })
                .unwrap()
            };
            Store::open(path, defaults)
        });
        if let Some(error) = store.load_error() {
            return Err(error);
        }
        let mut ledger: Ledger = serde_json::from_value(store.get())
            .map_err(|_| "전송 대기 정보를 읽지 못했어요. 원본을 유지했어요.")?;
        let unique: BTreeSet<_> = ledger.uploads.iter().map(|u| &u.operation).collect();
        if ledger.version != 1
            || ledger.subject != subject
            || ledger.uploads.len() > MAX_PENDING
            || unique.len() != ledger.uploads.len()
            || !ledger.uploads.iter().all(|u| {
                id(&u.operation)
                    && id(&u.folder_id)
                    && u.file_id.as_deref().is_none_or(id)
                    && valid_name(&u.name)
                    && u.size <= MAX_BYTES as u64
                    && hash(&u.sha256)
            })
        {
            return err("전송 대기 정보의 형식을 확인하지 못했어요. 기존 자료는 유지했어요.");
        }
        let result = f(&mut ledger)?;
        if save {
            let next =
                serde_json::to_value(ledger).map_err(|_| "전송 대기 정보를 준비하지 못했어요.")?;
            store
                .commit(|v| *v = next)
                .map_err(|_| "전송 대기 정보를 기기에 저장하지 못했어요.")?;
        }
        Ok(result)
    }
    pub fn pending(&self, subject: &str, folder: Option<&str>) -> Result<Vec<Upload>> {
        self.ledger(subject, false, |l| {
            Ok(l.uploads
                .iter()
                .filter(|u| folder.is_none_or(|f| u.folder_id == f))
                .cloned()
                .collect())
        })
    }
    pub fn get(&self, subject: &str, operation: &str) -> Result<Upload> {
        if !id(operation) {
            return err("전송 대기 항목을 확인하지 못했어요.");
        }
        self.ledger(subject, false, |l| {
            l.uploads
                .iter()
                .find(|u| u.operation == operation)
                .cloned()
                .ok_or(
                    "이미 처리했거나 현재 계정에 없는 대기 항목이에요. 목록을 새로 확인해 주세요."
                        .into(),
                )
        })
    }
    pub fn stage(&self, subject: &str, folder: &str, name: &str, bytes: &[u8]) -> Result<Upload> {
        if !id(folder) || !valid_name(name) || bytes.len() > MAX_BYTES {
            return err("파일 이름과 25MB 전송 한도를 확인해 주세요.");
        }
        let dir = self.dir(subject);
        std::fs::create_dir_all(&dir).map_err(|_| "전송 사본 폴더를 준비하지 못했어요.")?;
        let dir = self.resolved_dir(subject)?;
        let upload = Upload {
            operation: crate::google::oauth::random_token(),
            folder_id: folder.into(),
            file_id: None,
            name: name.into(),
            size: bytes.len() as u64,
            sha256: digest(bytes),
        };
        let mut created = false;
        let result = self.ledger(subject, true, |l| {
            if l.uploads.len() >= MAX_PENDING {
                return err(
                    "전송 대기 파일은 8개까지 보관할 수 있어요. 기존 항목을 먼저 처리해 주세요.",
                );
            }
            let path = dir.join(format!("{}.bin", upload.operation));
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|_| "파일의 전송 사본을 만들지 못했어요.")?;
            created = true;
            if file.write_all(bytes).and_then(|_| file.sync_all()).is_err() {
                drop(file);
                let _ = std::fs::remove_file(path);
                return err("전송 사본을 끝까지 보관하지 못했어요.");
            }
            l.uploads.push(upload.clone());
            Ok(())
        });
        if result.is_err() && created {
            let _ = std::fs::remove_file(dir.join(format!("{}.bin", upload.operation)));
        }
        result?;
        Ok(upload)
    }
    pub fn file_id(&self, subject: &str, operation: &str, fid: &str) -> Result<()> {
        if !id(fid) {
            return err("Drive 저장 주소를 확인하지 못했어요.");
        }
        self.ledger(subject, true, |l| {
            let u = l
                .uploads
                .iter_mut()
                .find(|u| u.operation == operation)
                .ok_or("전송 대기 항목이 바뀌었어요.")?;
            if u.file_id.as_deref().is_some_and(|old| old != fid) {
                return err("전송 저장 주소가 이미 정해져 있어요.");
            }
            u.file_id = Some(fid.into());
            Ok(())
        })
    }
    pub(super) fn blob(&self, subject: &str, operation: &str) -> Result<PathBuf> {
        if !id(operation) {
            return err("전송 사본 주소를 확인하지 못했어요.");
        }
        let dir = self.resolved_dir(subject)?;
        let path = dir.join(format!("{operation}.bin"));
        let canonical = path
            .canonicalize()
            .map_err(|_| "전송 사본을 찾지 못했어요. 원본 파일을 다시 선택해 주세요.")?;
        if canonical.parent() != Some(dir.as_path())
            || std::fs::symlink_metadata(&path)
                .map_err(|_| "전송 사본을 읽지 못했어요.")?
                .file_type()
                .is_symlink()
        {
            return err("전송 사본이 보관 폴더 밖을 가리켜 읽기를 중단했어요.");
        }
        Ok(canonical)
    }
    pub fn bytes(&self, subject: &str, upload: &Upload) -> Result<Vec<u8>> {
        use std::io::Read;
        let path = self.blob(subject, &upload.operation)?;
        let file = std::fs::File::open(path).map_err(|_| "전송 사본을 읽지 못했어요.")?;
        let mut bytes = vec![];
        file.take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "전송 사본을 끝까지 읽지 못했어요.")?;
        if bytes.len() as u64 != upload.size || digest(&bytes) != upload.sha256 {
            return err("보관한 사본의 내용이 달라요. 다시 올리지 않고 대기 정보를 유지했어요.");
        }
        Ok(bytes)
    }
    pub fn remove(&self, subject: &str, operation: &str) -> Result<()> {
        let _upload = self.get(subject, operation)?;
        let path = self.resolved_dir(subject)?.join(format!("{operation}.bin"));
        match std::fs::remove_file(path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return err("전송 사본을 지우지 못했어요. 대기 정보를 유지했어요."),
        }
        self.ledger(subject, true, |l| {
            l.uploads.retain(|u| u.operation != operation);
            Ok(())
        })?;
        Ok(())
    }
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
pub fn valid_name(s: &str) -> bool {
    !s.trim().is_empty()
        && s.chars().count() <= 255
        && !s.chars().any(|c| c.is_control() || matches!(c, '/' | '\\'))
}

pub fn safe_name(name: &str) -> String {
    let mut units = 0;
    let mut s = String::new();
    for c in name.chars() {
        let c = if c.is_control() || "<>:\"/\\|?*".contains(c) {
            '_'
        } else {
            c
        };
        units += c.len_utf16();
        if units > 180 {
            break;
        }
        s.push(c);
    }
    let s = s.trim().trim_end_matches(['.', ' ']);
    if s.is_empty() {
        return "download.bin".into();
    }
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    let port = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    let reserved = ["CON", "PRN", "AUX", "NUL", "CLOCK$", "CONIN$", "CONOUT$"]
        .contains(&stem.as_str())
        || port.is_some_and(|p| {
            ["1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³"].contains(&p)
        });
    if reserved {
        format!("_{s}")
    } else {
        s.into()
    }
}
