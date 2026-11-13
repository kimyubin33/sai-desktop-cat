const { app, ipcMain, Menu, Tray, nativeImage, powerMonitor } = require('electron');
const path = require('path');
const { createStores } = require('./store');
const { CatController } = require('./catWindow');
const { Advisor } = require('./advisor');
const { ChatWindow } = require('./chatWindow');
const { ChatService } = require('./chatService');
const { AiHelper } = require('./ai');
const { registerChatIpc } = require('./ipc');
const { createTeamService } = require('./team');
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
let team;

const FREQ_LABELS = { off: '먼저 말 걸지 않기', low: '가끔', normal: '보통', high: '자주' };

function openChat(tab = 'chat', extra = {}) {
  cat.hideBubble();
  chatWindow.open(tab, extra);
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
    { label: '협업 (업무·일정)', click: () => openChat('collab') },
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

  team = createTeamService({ stores, onChange: () => refreshTray() });
  advisor = new Advisor({
    stores,
    cat,
    team: team.sync,
    collab: team.collab,
    notifier: team.notifier,
    // 10분 넘게 입력이 없거나 화면이 잠겨 있으면 자리를 비운 것으로 본다
    isAway: () => powerMonitor.getSystemIdleState(600) !== 'active',
    onCollab: (taskId) => openChat('collab', taskId ? { task: taskId } : {}),
    onChat: () => openChat(),
    onExplain: (topic) => openChat('chat', { explain: topic }),
  });

  ai = new AiHelper(stores.settings);
  chat = new ChatService({ stores, ai });
  chatWindow = new ChatWindow();
  registerChatIpc(ipcMain, { stores, chat, ai, chatWindow, onSettingsChanged, team });

  tray = new Tray(nativeImage.createFromPath(path.join(__dirname, '..', '..', 'assets', 'tray.png')));
  tray.setToolTip('사이 — 고양이 협업 도우미');
  tray.on('click', () => openChat());
  refreshTray();

  team.start();
  if (!runSnapshots(app, { cat, advisor, chatWindow, openChat, team, stores })) advisor.start();
});

app.on('window-all-closed', (e) => e.preventDefault());

app.on('before-quit', () => {
  if (advisor) advisor.stop();
  if (team) team.stop();
  if (!stores) return;
  for (const s of Object.values(stores)) s.saveNow();
});
