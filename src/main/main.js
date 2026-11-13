const { app, ipcMain, Menu, Tray, nativeImage } = require('electron');
const path = require('path');
const { createStores } = require('./store');
const { CatController } = require('./catWindow');
const { Advisor } = require('./advisor');
const { ChatWindow } = require('./chatWindow');
const { ChatService } = require('./chatService');
const { AiHelper } = require('./ai');
const { registerChatIpc } = require('./ipc');
const { runSnapshots } = require('./devSnapshot');

if (!app.requestSingleInstanceLock()) {
  app.quit();
}

let stores;
let cat;
let advisor;
let tray;
let chatWindow;
let ai;
let chat;

const FREQ_LABELS = { off: '먼저 말 걸지 않기', low: '가끔', normal: '보통', high: '자주' };

function openChat(tab = 'chat') {
  cat.hideBubble();
  chatWindow.open(tab);
}

function onSettingsChanged() {
  const s = stores.settings.get();
  cat.setCalm(!!s.quiet);
  advisor.schedule();
  refreshTray();
}

function setSetting(fn) {
  stores.settings.update(fn);
  onSettingsChanged();
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
    { label: '정리함', click: () => openChat('notes') },
    { label: '설정', click: () => openChat('settings') },
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

  ai = new AiHelper(stores.settings);
  chat = new ChatService({ stores, ai });
  chatWindow = new ChatWindow();
  registerChatIpc(ipcMain, { stores, chat, ai, chatWindow, onSettingsChanged });

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
    ...chatDemoSteps(),
  ];
}

function chatDemoSteps() {
  const js = (code) => () => chatWindow.win.webContents.executeJavaScript(code);
  const w = () => chatWindow.win;
  return [
    { name: 'chat-start', wait: 300, action: () => openChat(), window: w, settle: 1500 },
    { name: 'chat-mode', wait: 300, action: js("__saiDemo.startMode('complaint')"), window: w },
    { name: 'chat-1', wait: 300, action: js("__saiDemo.send('시간도 안 지키고 맡은 일도 제대로 안 해서 너무 화나요. 진짜 무책임해요')"), window: w },
    { name: 'chat-2', wait: 300, action: js("__saiDemo.send('어제 같이 검토하기로 했는데 연락이 없었어요')"), window: w },
    { name: 'chat-3', wait: 300, action: js("__saiDemo.send('사정은 모르겠어요. 물어보진 않았어요')"), window: w },
    { name: 'chat-4', wait: 300, action: js("__saiDemo.send('제 다음 작업이 하루 밀렸어요')"), window: w },
    { name: 'chat-5', wait: 300, action: js("__saiDemo.send('미리 알려주기만 했어도 괜찮았을 것 같아요')"), window: w, settle: 800 },
    {
      name: 'chat-share',
      wait: 300,
      action: js("document.querySelector('.card .share').click()"),
      window: w,
      settle: 800,
    },
    {
      name: 'chat-saved',
      wait: 300,
      action: js("document.querySelector('.share-panel .card-actions .primary').click()"),
      window: w,
      settle: 800,
    },
    { name: 'notes', wait: 300, action: js("__saiDemo.showTab('notes')"), window: w },
    { name: 'settings', wait: 300, action: js("__saiDemo.showTab('settings')"), window: w },
  ];
}

app.on('window-all-closed', (e) => e.preventDefault());

app.on('before-quit', () => {
  if (advisor) advisor.stop();
  if (!stores) return;
  for (const s of Object.values(stores)) s.saveNow();
});
