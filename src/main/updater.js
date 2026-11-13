// 자동 업데이트: GitHub Releases 에 새 버전이 올라오면 내려받아 두고, 고양이가 적용할지 묻는다.
// - 설치 버전(Sai-Setup-*.exe 로 설치)에서 동작한다.
// - zip 버전으로 쓰던 경우에도 새 설치 파일을 받아 설치하므로, 그 뒤로는 설치 버전이 된다.
// - 포터블(Sai-Portable-*.exe)은 스스로 바꿀 수 없어 새 버전이 있다는 안내만 한다.
// - "나중에"를 고르면 다음에 프로그램을 끌 때 적용된다.
const { app, shell } = require('electron');

const CHECK_EVERY_MS = 3 * 60 * 60 * 1000;
const RELEASES_URL = 'https://github.com/kimyubin33/sai-desktop-cat/releases/latest';

function createUpdater({ onReady, onInfo = () => {} }) {
  const state = {
    current: app.getVersion(),
    status: 'idle', // idle | checking | downloading | ready | latest | error | unsupported
    latest: null,
    progress: 0,
    lastCheck: 0,
    error: null,
  };
  const portable = !!process.env.PORTABLE_EXECUTABLE_DIR;
  let autoUpdater = null;

  if (!app.isPackaged) {
    state.status = 'unsupported';
    state.error = '개발 실행(npm start)에서는 자동 업데이트를 하지 않아요.';
  } else {
    try {
      ({ autoUpdater } = require('electron-updater'));
      // 설치 파일에만 들어가는 app-update.yml 대신 코드에서 직접 지정 → zip 으로 쓰던 경우에도 업데이트된다
      autoUpdater.setFeedURL({ provider: 'github', owner: 'kimyubin33', repo: 'sai-desktop-cat', releaseType: 'release' });
      autoUpdater.autoDownload = !portable;
      autoUpdater.autoInstallOnAppQuit = true;
      autoUpdater.allowPrerelease = false;
      autoUpdater.on('checking-for-update', () => Object.assign(state, { status: 'checking', error: null }));
      autoUpdater.on('update-not-available', () => Object.assign(state, { status: 'latest', lastCheck: Date.now() }));
      autoUpdater.on('update-available', (info) => {
        Object.assign(state, { status: portable ? 'ready' : 'downloading', latest: info.version, lastCheck: Date.now() });
        if (portable) onInfo(`새 버전 ${info.version}이 나왔어요. 포터블 버전은 새 파일을 받아서 바꿔 주세요.`, true);
      });
      autoUpdater.on('download-progress', (p) => (state.progress = Math.round(p.percent || 0)));
      autoUpdater.on('update-downloaded', (info) => {
        Object.assign(state, { status: 'ready', latest: info.version, progress: 100 });
        onReady(info.version);
      });
      autoUpdater.on('error', (e) => Object.assign(state, { status: 'error', error: String((e && e.message) || e).split('\n')[0], lastCheck: Date.now() }));
    } catch (e) {
      state.status = 'unsupported';
      state.error = String(e.message || e);
    }
  }

  async function check({ manual = false } = {}) {
    if (!autoUpdater) {
      if (manual) onInfo(state.error || '이 실행 방식에서는 자동 업데이트를 할 수 없어요.');
      return state;
    }
    try {
      await autoUpdater.checkForUpdates();
      if (manual && state.status === 'latest') onInfo(`지금이 최신 버전(${state.current})이에요.`);
      if (manual && state.status === 'downloading') onInfo(`새 버전 ${state.latest}을 받고 있어요. 다 받으면 알려 드릴게요.`);
    } catch (e) {
      Object.assign(state, { status: 'error', error: String(e.message || e).split('\n')[0], lastCheck: Date.now() });
      if (manual) onInfo(`업데이트를 확인하지 못했어요: ${state.error}`);
    }
    return state;
  }

  function install() {
    if (portable) return shell.openExternal(RELEASES_URL);
    if (autoUpdater && state.status === 'ready') {
      // 조용히 설치하고 바로 다시 켠다
      setImmediate(() => autoUpdater.quitAndInstall(true, true));
    }
  }

  let timer = null;
  function start() {
    if (!autoUpdater) return;
    setTimeout(() => check(), 15 * 1000);
    timer = setInterval(() => check(), CHECK_EVERY_MS);
  }

  function stop() {
    clearInterval(timer);
  }

  return { state, check, install, start, stop, releasesUrl: RELEASES_URL };
}

module.exports = { createUpdater };
