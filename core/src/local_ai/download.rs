//! Resumable, hash-pinned downloads and safe unpacking of the llama.cpp runtime.
use super::Asset;
use crate::{err, Result};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Unpacked runtime must stay small; the archive is pinned, this only guards against surprises.
const MAX_UNPACKED: u64 = 400 * 1024 * 1024;

pub fn marker(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".verified");
    path.with_file_name(name)
}
fn partial(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

/// A finished download is trusted only with a marker that names the expected hash and a matching size.
pub fn verified(path: &Path, asset: &Asset) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() == asset.size)
        && std::fs::read_to_string(marker(path)).is_ok_and(|s| s.trim() == asset.sha256)
}

pub async fn fetch(
    http: &reqwest::Client,
    asset: &Asset,
    dest: &Path,
    progress: &(dyn Fn(u64, u64) + Sync),
    cancel: &AtomicBool,
) -> Result<()> {
    if verified(dest, asset) {
        return Ok(());
    }
    let dir = dest.parent().ok_or("저장할 폴더를 확인하지 못했어요.")?;
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|_| "캐릭터 AI를 저장할 폴더를 만들지 못했어요.")?;
    let part = partial(dest);
    let mut have = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
    if have > asset.size {
        let _ = tokio::fs::remove_file(&part).await;
        have = 0;
    }
    // Resuming: the hash must cover the bytes already on disk too.
    let mut hasher = Sha256::new();
    if have > 0 {
        let mut f = tokio::fs::File::open(&part).await.map_err(|_| "받던 파일을 읽지 못했어요.")?;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf).await.map_err(|_| "받던 파일을 읽지 못했어요.")?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
    }
    if have < asset.size {
        let mut req = http.get(&asset.url);
        if have > 0 {
            req = req.header("range", format!("bytes={have}-"));
        }
        let res = req
            .send()
            .await
            .map_err(|_| "캐릭터 AI를 받지 못했어요. 인터넷 연결을 확인해 주세요.")?;
        let status = res.status().as_u16();
        let append = match status {
            206 if have > 0 => true,
            200 => {
                have = 0;
                hasher = Sha256::new();
                false
            }
            _ => return Err(format!("캐릭터 AI를 받지 못했어요 ({status}).")),
        };
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(&part)
            .await
            .map_err(|_| "받은 파일을 저장하지 못했어요. 저장 공간을 확인해 주세요.")?;
        let mut stream = res.bytes_stream();
        let mut last = 0u64;
        progress(have, asset.size);
        while let Some(chunk) = stream.next().await {
            if cancel.load(Ordering::SeqCst) {
                let _ = file.flush().await;
                return err("받기를 멈췄어요. 다시 받으면 이어서 받아요.");
            }
            let chunk = chunk.map_err(|_| "받는 중에 연결이 끊겼어요. 다시 받으면 이어서 받아요.")?;
            have += chunk.len() as u64;
            if have > asset.size {
                drop(file);
                let _ = tokio::fs::remove_file(&part).await;
                return err("받은 파일 크기가 예상과 달라 지웠어요.");
            }
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|_| "받은 파일을 저장하지 못했어요. 저장 공간을 확인해 주세요.")?;
            if have - last >= 4 << 20 || have == asset.size {
                last = have;
                progress(have, asset.size);
            }
        }
        file.flush().await.map_err(|_| "받은 파일을 저장하지 못했어요.")?;
    }
    if have != asset.size {
        return err("받는 중에 연결이 끊겼어요. 다시 받으면 이어서 받아요.");
    }
    let digest = format!("{:x}", hasher.finalize());
    if digest != asset.sha256 {
        let _ = tokio::fs::remove_file(&part).await;
        return err("받은 파일이 원본과 달라 지웠어요. 다시 받아 주세요.");
    }
    let _ = tokio::fs::remove_file(dest).await;
    tokio::fs::rename(&part, dest)
        .await
        .map_err(|_| "받은 파일을 옮기지 못했어요.")?;
    tokio::fs::write(marker(dest), asset.sha256)
        .await
        .map_err(|_| "받은 파일 확인 기록을 저장하지 못했어요.")?;
    Ok(())
}

/// Unpacks into a fresh sibling folder and swaps it in only when `llama-server.exe` is present.
pub async fn extract(zip: &Path, dest: &Path) -> Result<()> {
    let zip = zip.to_owned();
    let dest = dest.to_owned();
    tokio::task::spawn_blocking(move || extract_blocking(&zip, &dest))
        .await
        .map_err(|_| "실행 파일을 풀지 못했어요.")?
}
fn extract_blocking(zip: &Path, dest: &Path) -> Result<()> {
    let tmp = dest.with_extension("unpacking");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|_| "실행 파일을 풀 폴더를 만들지 못했어요.")?;
    let file = std::fs::File::open(zip).map_err(|_| "받은 실행 파일을 열지 못했어요.")?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| "받은 실행 파일 형식이 올바르지 않아요.")?;
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|_| "받은 실행 파일을 풀지 못했어요.")?;
        // enclosed_name rejects absolute paths and `..` (zip-slip).
        let Some(rel) = entry.enclosed_name() else {
            return err("받은 실행 파일에 허용되지 않은 경로가 있어요.");
        };
        let out = tmp.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|_| "실행 파일을 풀지 못했어요.")?;
            continue;
        }
        total += entry.size();
        if total > MAX_UNPACKED {
            return err("받은 실행 파일이 예상보다 커서 멈췄어요.");
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|_| "실행 파일을 풀지 못했어요.")?;
        }
        let mut f = std::fs::File::create(&out).map_err(|_| "실행 파일을 풀지 못했어요.")?;
        std::io::copy(&mut (&mut entry).take(MAX_UNPACKED), &mut f).map_err(|_| "실행 파일을 풀지 못했어요.")?;
    }
    if !tmp.join("llama-server.exe").is_file() {
        let _ = std::fs::remove_dir_all(&tmp);
        return err("받은 실행 파일에 캐릭터 AI 서버가 없어요.");
    }
    let _ = std::fs::remove_dir_all(dest);
    std::fs::rename(&tmp, dest).map_err(|_| "풀어 둔 실행 파일을 옮기지 못했어요.")?;
    Ok(())
}
use std::io::Read as _;
