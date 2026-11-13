// 컴퓨터를 켜면 사이가 자동으로 켜지게 한다 (Windows 시작 프로그램 등록).
// 끄는 설정은 두지 않고, 켤 때마다 다시 등록한다 → 작업 관리자에서 꺼도 다음 실행 때 되살아난다.
// 설치 버전은 설치된 exe, 포터블은 원래 exe 파일 위치, zip 은 압축 푼 exe 를 등록한다.
const { app } = require('electron');

const NAME = 'Sai';

function exePath() {
  return process.env.PORTABLE_EXECUTABLE_FILE || process.execPath;
}

function ensureAutoStart() {
  if (!app.isPackaged || process.platform !== 'win32') return { enabled: false, reason: 'unsupported' };
  try {
    const opts = { path: exePath(), args: [], name: NAME };
    app.setLoginItemSettings({ ...opts, openAtLogin: true, enabled: true });
    const s = app.getLoginItemSettings(opts);
    return { enabled: !!(s.openAtLogin && s.executableWillLaunchAtLogin !== false) };
  } catch (e) {
    return { enabled: false, reason: String(e.message || e) };
  }
}

module.exports = { ensureAutoStart, AUTOSTART_NAME: NAME };
