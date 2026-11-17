use sai_core::google::{safe_download_name, MAX_FILE_BYTES};
use std::{
    io::{Read, Write},
    path::Path,
};

pub fn source(path: &Path) -> Result<(String, Vec<u8>), String> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("파일 이름을 읽지 못했어요.")?
        .to_owned();
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ);
    }
    let file = options
        .open(path)
        .map_err(|_| "파일을 읽지 못했어요. 열려 있는 파일을 닫거나 다른 사본을 선택해 주세요.")?;
    let meta = file.metadata().map_err(|_| "파일 정보를 읽지 못했어요.")?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES as u64 {
        return Err("일반 파일을 선택해 주세요. 첫 버전의 전송 한도는 25MB예요.".into());
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "파일을 끝까지 읽지 못했어요.")?;
    if bytes.len() as u64 != meta.len() {
        return Err("읽는 중 파일 크기가 바뀌었어요. 원본을 확인하고 다시 선택해 주세요.".into());
    }
    Ok((name, bytes))
}
pub fn destination(path: &Path) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("저장 파일 이름을 확인하지 못했어요.")?;
    if name != safe_download_name(name)
        || !path.is_absolute()
        || !path.parent().is_some_and(|p| p.is_dir())
    {
        return Err("사용할 수 있는 저장 경로와 파일 이름을 선택해 주세요.".into());
    }
    if path
        .try_exists()
        .map_err(|_| "저장 위치를 확인하지 못했어요.")?
    {
        return Err(
            "같은 이름의 파일이 있어요. 덮어쓰지 않았으니 다른 이름을 선택해 주세요.".into(),
        );
    }
    Ok(())
}
pub fn save(path: &Path, bytes: &[u8]) -> Result<String, String> {
    destination(path)?;
    let parent = path.parent().ok_or("저장 폴더를 확인하지 못했어요.")?;
    let temporary = parent.join(format!(
        ".sai-download-{}.tmp",
        sai_core::google::oauth::random_token()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| "저장할 임시 파일을 만들지 못했어요.")?;
    let result = file.write_all(bytes).and_then(|_| file.sync_all());
    drop(file);
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
        return Err("받은 파일을 끝까지 저장하지 못했어요. 기존 파일은 유지했어요.".into());
    }
    let result = publish(&temporary, path);
    let _ = std::fs::remove_file(&temporary);
    result?;
    Ok(path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned())
}
#[cfg(windows)]
fn publish(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // No REPLACE_EXISTING flag: a destination created during download must also be preserved.
    if unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0)
    } == 0
    {
        return Err(
            "파일을 저장 위치로 옮기지 못했어요. 같은 이름의 파일과 저장 권한을 확인해 주세요."
                .into(),
        );
    }
    Ok(())
}
#[cfg(not(windows))]
fn publish(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::hard_link(from, to)
        .map_err(|_| "파일을 저장하지 못했어요. 기존 파일은 유지했어요.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
            assert!(self
                .0
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("sai-file-save-test-"));
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn save_never_overwrites_and_preserves_complete_bytes() {
        let t = Temp(std::env::temp_dir().join(format!(
            "sai-file-save-test-{}",
            sai_core::google::oauth::random_token()
        )));
        std::fs::create_dir(&t.0).unwrap();
        let path = t.0.join("file.bin");
        save(&path, b"complete bytes").unwrap();
        assert!(save(&path, b"replacement").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"complete bytes");
        let staged = t.0.join("temp.bin");
        std::fs::write(&staged, b"other").unwrap();
        assert!(publish(&staged, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"complete bytes");
    }
}
