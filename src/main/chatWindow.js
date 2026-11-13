// 고양이를 클릭하면 열리는 대화창 (대화 · 정리함 · 설정)
const { BrowserWindow } = require('electron');
const path = require('path');

class ChatWindow {
  constructor() {
    this.win = null;
  }

  /** extra.explain: 주제 id — "내 사정도 있어요"에서 바로 내 사정 이야기를 시작 */
  open(tab = 'chat', extra = {}) {
    if (this.win && !this.win.isDestroyed()) {
      if (this.win.isMinimized()) this.win.restore();
      this.win.show();
      this.win.focus();
      this.win.webContents.send('chat:show-tab', tab);
      if (extra.explain) this.win.webContents.send('chat:start-explain', extra.explain);
      if (extra.task) this.win.webContents.send('chat:focus-task', extra.task);
      return this.win;
    }
    this.win = new BrowserWindow({
      width: 520,
      height: 700,
      minWidth: 380,
      minHeight: 520,
      title: '사이와 이야기',
      icon: path.join(__dirname, '..', '..', 'assets', 'icon.png'),
      backgroundColor: '#fffaf2',
      autoHideMenuBar: true,
      show: false,
      webPreferences: {
        preload: path.join(__dirname, '..', 'preload', 'chat-preload.js'),
        contextIsolation: true,
        nodeIntegration: false,
        sandbox: true,
      },
    });
    this.win.setMenu(null);
    this.win.loadFile(path.join(__dirname, '..', 'renderer', 'chat', 'index.html'), { query: { tab, ...(extra.explain ? { explain: extra.explain } : {}), ...(extra.task ? { task: extra.task } : {}) } });
    this.win.once('ready-to-show', () => this.win.show());
    this.win.on('closed', () => {
      this.win = null;
    });
    return this.win;
  }

  isOwn(e) {
    return this.win && !this.win.isDestroyed() && e.sender === this.win.webContents;
  }
}

module.exports = { ChatWindow };
