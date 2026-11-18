//! 운영체제와 닿는 작은 기능: 시작 프로그램 등록, 자리 비움 확인, 링크 열기.

/// 컴퓨터를 켜면 사이가 자동으로 켜지게 한다 (Windows 시작 프로그램 등록).
/// 끄는 설정은 두지 않고, 켤 때마다 다시 등록한다 → 작업 관리자에서 꺼도 다음 실행 때 되살아난다.
pub fn ensure_autostart() {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let Ok(exe) = std::env::current_exe() else { return };
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok((run, _)) = hkcu.create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run") {
            let _ = run.set_value("Sai", &format!("\"{}\"", exe.display()));
        }
        // 작업 관리자에서 "사용 안 함"으로 바꿨다면 다시 켠다 (첫 바이트 2 = 사용)
        if let Ok(appr) = hkcu.open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run", winreg::enums::KEY_ALL_ACCESS) {
            let on: Vec<u8> = vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            let _ = appr.set_raw_value("Sai", &winreg::RegValue { bytes: on, vtype: winreg::enums::RegType::REG_BINARY });
        }
    }
}

/// 마지막 키보드·마우스 입력 뒤 지난 시간(ms). 확인할 수 없으면 None.
pub fn idle_ms() -> Option<u64> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::GetTickCount;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
        let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        if unsafe { GetLastInputInfo(&mut info) } == 0 {
            return None;
        }
        // Both counters wrap at the same point, so wrapping subtraction stays correct.
        Some(unsafe { GetTickCount() }.wrapping_sub(info.dwTime) as u64)
    }
    #[cfg(not(windows))]
    None
}

pub fn open_url(url: &str) {
    let _ = open::that_detached(url);
}
