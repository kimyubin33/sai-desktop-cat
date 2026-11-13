const { app, ipcMain, Menu, Tray, nativeImage } = require('electron');
const path = require('path');
const { createStores } = require('./store');
const { CatController } = require('./catWindow');
const { runSnapshots } = require('./devSnapshot');

if (!app.requestSingleInstanceLock()) {
  app.quit();
}

let stores;
let cat;
let tray;

function buildMenu() {
  const s = stores.settings.get();
  return Menu.buildFromTemplate([
    { label: `${s.catName} 보이기`, type: 'checkbox', checked: cat.isVisible(), click: (i) => cat.setVisible(i.checked) },
    { type: 'separator' },
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
    onClick: () => {},
    onContextMenu: (win) => buildMenu().popup({ window: win }),
    onBubbleAction: () => {},
  });
  cat.bindIpc(ipcMain);
  cat.create();

  tray = new Tray(nativeImage.createFromPath(path.join(__dirname, '..', '..', 'assets', 'tray.png')));
  tray.setToolTip('사이 — 고양이 협업 도우미');
  refreshTray();

  runSnapshots(app, devSnapshotSteps());
});

function devSnapshotSteps() {
  const force = (state) => () => {
    cat.behavior.enter(state);
    cat.sendState(true);
  };
  return ['sit', 'walk', 'loaf', 'sleep', 'drag'].map((state) => ({
    name: `cat-${state}`,
    wait: 1200,
    action: force(state),
    window: () => cat.win,
  }));
}

app.on('window-all-closed', (e) => e.preventDefault());

app.on('before-quit', () => {
  if (!stores) return;
  for (const s of Object.values(stores)) s.saveNow();
});
