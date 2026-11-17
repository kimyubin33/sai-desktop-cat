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

pub fn open_url(url: &str) {
    let _ = open::that_detached(url);
}
