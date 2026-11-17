use std::path::PathBuf;
use tauri::WebviewWindow;

pub async fn choose(
    window: &WebviewWindow,
    save_name: Option<String>,
) -> Result<Option<PathBuf>, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let target = window.clone();
    window
        .run_on_main_thread(move || {
            if !sender.is_closed() {
                let _ = sender.send(native(&target, save_name.as_deref()));
            }
        })
        .map_err(|_| "파일 선택창을 열지 못했어요.")?;
    receiver
        .await
        .map_err(|_| String::from("파일 선택창과의 연결이 끝났어요."))?
}
#[cfg(windows)]
fn native(window: &WebviewWindow, save: Option<&str>) -> Result<Option<PathBuf>, String> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::UI::Controls::Dialogs::*;
    let mut buffer = vec![0u16; 32768];
    if let Some(name) = save {
        for (out, c) in buffer.iter_mut().zip(name.encode_utf16()) {
            *out = c;
        }
    }
    let filter: Vec<u16> = "모든 파일 (*.*)\0*.*\0\0".encode_utf16().collect();
    let title: Vec<u16> = if save.is_some() {
        "파일 저장\0"
    } else {
        "팀에 올릴 파일 선택 · 최대 25MB\0"
    }
    .encode_utf16()
    .collect();
    let mut options: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    options.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    options.hwndOwner = window.hwnd().map_err(|_| "앱 창을 확인하지 못했어요.")?.0 as _;
    options.lpstrFilter = filter.as_ptr();
    options.nFilterIndex = 1;
    options.lpstrFile = buffer.as_mut_ptr();
    options.nMaxFile = buffer.len() as u32;
    options.lpstrTitle = title.as_ptr();
    options.Flags = OFN_EXPLORER
        | OFN_NOCHANGEDIR
        | OFN_PATHMUSTEXIST
        | OFN_NODEREFERENCELINKS
        | OFN_HIDEREADONLY
        | if save.is_none() { OFN_FILEMUSTEXIST } else { 0 };
    let ok = unsafe {
        if save.is_some() {
            GetSaveFileNameW(&mut options)
        } else {
            GetOpenFileNameW(&mut options)
        }
    };
    if ok == 0 {
        return if unsafe { CommDlgExtendedError() } == 0 {
            Ok(None)
        } else {
            Err("파일 선택창을 완료하지 못했어요. 다시 시도해 주세요.".into())
        };
    }
    let len = buffer
        .iter()
        .position(|c| *c == 0)
        .ok_or("파일 경로를 확인하지 못했어요.")?;
    if len == 0 {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(std::ffi::OsString::from_wide(
        &buffer[..len],
    ))))
}
#[cfg(not(windows))]
fn native(_: &WebviewWindow, _: Option<&str>) -> Result<Option<PathBuf>, String> {
    Err("파일 선택은 Windows 앱에서 사용할 수 있어요.".into())
}
