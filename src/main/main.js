const { app, ipcMain, Menu, Tray, nativeImage } = require('electron');
const path = require('path');
const { createStores } = require('./store');
const { CatController } = require('./catWindow');
const { Advisor } = require('./advisor');
const { runSnapshots } = require('./devSnapshot');

if (!app.requestSingleInstanceLock()) {
  app.quit();
}

let stores;
let cat;
let advisor;
let tray;

const FREQ_LABELS = { off: '먼저 말 걸지 않기', low: '가끔', normal: '보통', high: '자주' };

function openChat() {
  // 대화창은 다음 단계에서 연결된다
}

function setSetting(fn) {
  stores.settings.update(fn);
  const s = stores.settings.get();
  cat.setCalm(!!s.quiet);
  advisor.schedule();
  refreshTray();
}

function focusFor(minutes) {
  setSetting((s) => {
    s.focusUntil = minutes ? Date.now() + minutes * 60 * 1000 : 0;
  });
  if (minutes) setTimeout(refreshTray, minutes * 60 * 1000 + 1000);
}

function buildMenu() {
  const s = stores.settings.get();
  const focusing = s.focusUntil > Date.now();
  const until = focusing ? new Date(s.focusUntil).toLocaleTimeString('ko-KR', { hour: '2-digit', minute: '2-digit' }) : '';
  return Menu.buildFromTemplate([
    { label: `${s.catName}와 이야기하기`, click: openChat },
    { label: '조언 한마디 듣기', click: () => advisor.speakNow() },
    { type: 'separator' },
    {
      label: '조용한 모드 (먼저 말 걸지 않기)',
      type: 'checkbox',
      checked: !!s.quiet,
      click: (i) => setSetting((x) => (x.quiet = i.checked)),
    },
    {
      label: focusing ? `집중 모드 중 (${until}까지)` : '집중 모드',
      submenu: [
        { label: '30분', click: () => focusFor(30) },
        { label: '1시간', click: () => focusFor(60) },
        { label: '2시간', click: () => focusFor(120) },
        { label: '집중 모드 끝내기', enabled: focusing, click: () => focusFor(0) },
      ],
    },
    {
      label: '말 거는 빈도',
      submenu: Object.entries(FREQ_LABELS).map(([k, label]) => ({
        label,
        type: 'radio',
        checked: s.frequency === k,
        click: () => setSetting((x) => (x.frequency = k)),
      })),
    },
    { type: 'separator' },
    { label: `${s.catName} 보이기`, type: 'checkbox', checked: cat.isVisible(), click: (i) => cat.setVisible(i.checked) },
    { label: '종료', role: 'quit' },
  ]);
}

function refreshTray() {
  if (tray) tray.setContextMenu(buildMenu());
}

app.whenReady().then(() => {
  stores = createStores(app.getPath('userData'));
  if (process.platform === 'win32') app.setAppUserModelId('com.kimyubin33.sai');

  cat = new CatController({
    settings: stores.settings,
    onClick: () => openChat(),
    onContextMenu: (win) => buildMenu().popup({ window: win }),
    onBubbleAction: (id, action) => advisor.handleAction(id, action),
  });
  cat.bindIpc(ipcMain);
  cat.create();

  advisor = new Advisor({ stores, cat, onChat: () => openChat() });

  tray = new Tray(nativeImage.createFromPath(path.join(__dirname, '..', '..', 'assets', 'tray.png')));
  tray.setToolTip('사이 — 고양이 협업 도우미');
  tray.on('click', () => openChat());
  refreshTray();

  if (!runSnapshots(app, devSnapshotSteps())) advisor.start();
});

function devSnapshotSteps() {
  const force = (state) => () => {
    cat.hideBubble();
    cat.behavior.enter(state);
    cat.sendState(true);
  };
  return [
    ...['sit', 'walk', 'loaf', 'sleep', 'drag'].map((state) => ({
      name: `cat-${state}`,
      wait: 1000,
      action: force(state),
      window: () => cat.win,
    })),
    {
      name: 'cat-greet',
      wait: 600,
      action: () => {
        force('sit')();
        advisor.greet();
      },
      window: () => cat.win,
    },
    {
      name: 'cat-advice',
      wait: 600,
      action: () => {
        cat.hideBubble();
        advisor.speakNow();
      },
      window: () => cat.win,
    },
  ];
}

app.on('window-all-closed', (e) => e.preventDefault());

app.on('before-quit', () => {
  if (advisor) advisor.stop();
  if (!stores) return;
  for (const s of Object.values(stores)) s.saveNow();
});
