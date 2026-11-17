//! 개발용 읽기 전용 검사. 명시한 설치 자료만 확인하며 본문·비밀 값은 출력하지 않는다.
use sai_core::store::legacy::Archive;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn fingerprints(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>, Box<dyn std::error::Error>> {
    let mut paths = vec![
        "settings.json",
        "advice-history.json",
        "shared/team-cache.json",
        "shared/outbox.json",
        "private/conversations.json",
        "private/schedule.json",
        "mesh/identity.sqlite3",
        "mesh/identity.sqlite3-wal",
        "mesh/identity.sqlite3-shm",
    ]
    .into_iter()
    .map(|name| root.join(name))
    .collect::<Vec<_>>();
    if let Ok(entries) = fs::read_dir(root.join("shared/servers")) {
        for entry in entries {
            let path = entry?.path();
            if path.is_dir() {
                for name in ["team-cache.json", "outbox.json"] {
                    paths.push(path.join(name));
                }
            }
        }
    }
    let mut result = BTreeMap::new();
    for path in paths {
        if path.exists() {
            if !fs::canonicalize(&path)?.starts_with(root) {
                return Err("Source outside app folder".into());
            }
            let digest = Sha256::digest(fs::read(&path)?).to_vec();
            result.insert(path, digest);
        }
    }
    Ok(result)
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arg = std::env::args_os()
        .nth(1)
        .ok_or("Pass an existing app data directory explicitly")?;
    let root = fs::canonicalize(arg)?;
    let before = fingerprints(&root)?;
    let archive = Archive::read(&root)?;
    if before != fingerprints(&root)? {
        return Err("Source changed during read-only check; result is inconclusive".into());
    }
    println!(
        "Readonly source check passed ({} files); summary: {}",
        before.len(),
        serde_json::to_string(&archive.summary)?
    );
    Ok(())
}
