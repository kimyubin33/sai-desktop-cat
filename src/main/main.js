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
const { createUpdater } = require('./updater');
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
let updater;

const FREQ_LABELS = { low: '가끔', normal: '보통', high: '자주' };

function openChat(tab = 'chat', extra = {}) {
  cat.hideBubble();
  chatWindow.open(tab, extra);
}

function onSettingsChanged() {
  const s = stores.settings.get();
  cat.setCalm(s.quietUntil > Date.now());
  advisor.schedule();
  refreshTray();
}

function setSetting(fn) {
  stores.settings.update(fn);
  onSettingsChanged();
}

/** 잠시 조용히: 최대 1시간, 지나면 저절로 다시 말을 건다 */
function quietFor(minutes) {
  setSetting((s) => {
    s.quietUntil = minutes ? Date.now() + Math.min(minutes, 60) * 60 * 1000 : 0;
  });
  if (minutes) setTimeout(onSettingsChanged, Math.min(minutes, 60) * 60 * 1000 + 1000);
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
  const quieting = s.quietUntil > Date.now();
  const quietUntil = quieting ? new Date(s.quietUntil).toLocaleTimeString('ko-KR', { hour: '2-digit', minute: '2-digit' }) : '';
  return Menu.buildFromTemplate([
    { label: `${s.catName}와 이야기하기`, click: openChat },
    { label: '조언 한마디 듣기', click: () => advisor.speakNow() },
    { type: 'separator' },
    {
      label: quieting ? `잠시 조용히 하는 중 (${quietUntil}까지)` : '잠시 조용히 (1시간)',
      click: () => quietFor(quieting ? 0 : 60),
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
    {
      label: updater && updater.state.status === 'ready' ? `새 버전 ${updater.state.latest} 적용하기` : `업데이트 확인 (지금 ${app.getVersion()})`,
      click: () => (updater.state.status === 'ready' ? updater.install() : updater.check({ manual: true })),
    },
    { type: 'separator' },
    {
      label: cat.isVisible() ? `${s.catName} 1시간 숨기기` : `${s.catName} 다시 보이기`,
      click: () => hideFor(cat.isVisible() ? 60 : 0),
    },
    { label: '종료', role: 'quit' },
  ]);
}

/** 숨기기도 1시간 뒤 저절로 다시 나타난다 */
let hideTimer = null;
function hideFor(minutes) {
  clearTimeout(hideTimer);
  cat.setVisible(!minutes);
  if (minutes) hideTimer = setTimeout(() => (cat.setVisible(true), refreshTray()), minutes * 60 * 1000);
  refreshTray();
}

function refreshTray() {
  if (tray) tray.setContextMenu(buildMenu());
}

app.whenReady().then(() => {
  stores = createStores(app.getPath('userData'));
  // 예전 버전에서 "먼저 말 걸지 않기"·"조용한 모드"를 켜 둔 경우: 끄는 기능을 없앴으므로 기본값으로 돌린다
  stores.settings.update((s) => {
    if (!['low', 'normal', 'high'].includes(s.frequency)) s.frequency = 'low';
    if ('quiet' in s) delete s.quiet;
    s.dampenedTopics = {};
  });
  if (process.platform === 'win32') app.setAppUserModelId('com.kimyubin33.sai');

  cat = new CatController({
    settings: stores.settings,
    onClick: () => openChat(),
    onContextMenu: () => cat.popupMenu(buildMenu()),
    onBubbleAction: (id, action) => advisor.handleAction(id, action),
  });
  cat.bindIpc(ipcMain);
  cat.create();

  team = createTeamService({ stores, onChange: () => refreshTray() });
  advisor = new Advisor({
    stores,
    cat,
    // 고양이 말풍선은 모든 팀을 함께 본다
    team: team.hub.teamFacade(),
    collab: team.hub.collabFacade(),
    notifier: team.hub.notifierFacade(),
    // 10분 넘게 입력이 없거나 화면이 잠겨 있으면 자리를 비운 것으로 본다
    isAway: () => powerMonitor.getSystemIdleState(600) !== 'active',
    onCollab: (taskId) => openChat('collab', taskId ? { task: taskId } : {}),
    onChat: () => openChat(),
    onExplain: (topic) => openChat('chat', { explain: topic }),
  });

  ai = new AiHelper(stores.settings);
  chat = new ChatService({ stores, ai });
  chatWindow = new ChatWindow();
  registerChatIpc(ipcMain, { stores, chat, ai, chatWindow, onSettingsChanged, team, getUpdater: () => updater });

  tray = new Tray(nativeImage.createFromPath(path.join(__dirname, '..', '..', 'assets', 'tray.png')));
  tray.setToolTip('사이 — 고양이 협업 도우미');
  tray.on('click', () => openChat());
  refreshTray();

  updater = createUpdater({
    onReady: (version) => {
      advisor.notifyUpdate(version);
      refreshTray();
    },
    onInfo: (text) => advisor.info(text),
  });
  advisor.onUpdateInstall = () => updater.install();

  team.start();
  if (!runSnapshots(app, { cat, advisor, chatWindow, openChat, team, stores, updater })) {
    advisor.start();
    updater.start();
  }
});

app.on('window-all-closed', (e) => e.preventDefault());

app.on('before-quit', () => {
  if (advisor) advisor.stop();
  if (team) team.stop();
  if (updater) updater.stop();
  if (!stores) return;
  for (const s of Object.values(stores)) s.saveNow();
});
