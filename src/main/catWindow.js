// 바탕화면 고양이 창: 작은 투명 창 자체가 화면 아래쪽(작업 표시줄 위)을 따라 움직인다.
const { BrowserWindow, screen } = require('electron');
const path = require('path');
const { CatBehavior } = require('../core/behavior');

const WIN_W = 260;
const WIN_H = 240;
const TICK_MS = 33;
const BUBBLE_MS = 15000;

class CatController {
  constructor({ settings, onClick, onContextMenu, onBubbleAction }) {
    this.settings = settings;
    this.onClick = onClick;
    this.onContextMenu = onContextMenu;
    this.onBubbleAction = onBubbleAction;
    this.win = null;
    this.behavior = null;
    this.display = null;
    this.y = 0;
    this.lastSent = '';
    this.drag = null;
    this.fall = null;
    this.bubble = null;
    this.bubbleTimer = null;
    this.bubbleHovered = false;
  }

  create() {
    const saved = this.settings.get().position;
    this.display =
      (saved && screen.getAllDisplays().find((d) => d.id === saved.displayId)) || screen.getPrimaryDisplay();
    const { minX, maxX } = this.bounds();
    const startX = saved ? Math.min(Math.max(saved.x, minX), maxX) : Math.round(minX + (maxX - minX) * 0.7);
    this.behavior = new CatBehavior({ minX, maxX, x: startX });
    this.behavior.calm = !!this.settings.get().quiet;

    this.win = new BrowserWindow({
      width: WIN_W,
      height: WIN_H,
      x: startX,
      y: this.y,
      transparent: true,
      backgroundColor: '#00000000',
      frame: false,
      resizable: false,
      movable: false,
      skipTaskbar: true,
      hasShadow: false,
      alwaysOnTop: true,
      focusable: false,
      fullscreenable: false,
      show: false,
      webPreferences: {
        preload: path.join(__dirname, '..', 'preload', 'cat-preload.js'),
        contextIsolation: true,
        nodeIntegration: false,
        sandbox: true,
      },
    });
    this.win.setAlwaysOnTop(true, 'floating');
    this.win.setVisibleOnAllWorkspaces(true, { visibleOnFullScreen: false });
    this.win.setIgnoreMouseEvents(true, { forward: true });
    this.win.loadFile(path.join(__dirname, '..', 'renderer', 'cat', 'index.html'));
    this.win.once('ready-to-show', () => {
      this.win.showInactive();
      this.sendState(true);
    });

    screen.on('display-metrics-changed', () => this.refreshBounds());
    screen.on('display-removed', () => {
      this.display = screen.getPrimaryDisplay();
      this.refreshBounds();
    });

    this.lastTick = Date.now();
    this.loop = setInterval(() => this.tick(), TICK_MS);
  }

  /** 고양이 창이 다닐 수 있는 가로 범위와 바닥 높이 */
  bounds() {
    const wa = this.display.workArea;
    this.y = wa.y + wa.height - WIN_H;
    return { minX: wa.x, maxX: wa.x + wa.width - WIN_W };
  }

  refreshBounds() {
    if (!this.behavior) return;
    const { minX, maxX } = this.bounds();
    this.behavior.setBounds(minX, maxX);
    if (!this.drag && !this.fall) this.place();
  }

  place(x = this.behavior.x, y = this.y) {
    if (this.win && !this.win.isDestroyed()) this.win.setPosition(Math.round(x), Math.round(y));
  }

  tick() {
    const now = Date.now();
    const dt = Math.min(100, now - this.lastTick);
    this.lastTick = now;

    if (this.drag) {
      const cur = screen.getCursorScreenPoint();
      const dx = cur.x - this.drag.cursor.x;
      const dy = cur.y - this.drag.cursor.y;
      if (!this.drag.moved && Math.hypot(dx, dy) > 5) {
        this.drag.moved = true;
        this.hideBubble();
        this.behavior.startDrag();
      }
      if (this.drag.moved) this.place(this.drag.win.x + dx, this.drag.win.y + dy);
    } else if (this.fall) {
      // 놓으면 바닥까지 살짝 떨어져 앉는다
      this.fall.vy += 0.0025 * dt;
      this.fall.y = Math.min(this.y, this.fall.y + this.fall.vy * dt);
      this.place(this.fall.x, this.fall.y);
      if (this.fall.y >= this.y) {
        this.behavior.endDrag(this.fall.x);
        this.fall = null;
        this.place();
        this.savePosition();
      }
    } else if (this.behavior.tick(dt)) {
      this.place();
    }
    this.sendState();
  }

  sendState(force = false) {
    const s = this.behavior.snapshot();
    const key = s.state + s.facing;
    if (!force && key === this.lastSent) return;
    this.lastSent = key;
    this.send('cat:state', s);
  }

  send(ch, payload) {
    if (this.win && !this.win.isDestroyed()) this.win.webContents.send(ch, payload);
  }

  setInteractive(on) {
    if (this.drag && !on) return;
    this.win.setIgnoreMouseEvents(!on, { forward: true });
  }

  dragStart() {
    const [x, y] = this.win.getPosition();
    this.fall = null;
    this.drag = { cursor: screen.getCursorScreenPoint(), win: { x, y }, at: Date.now(), moved: false };
  }

  dragEnd() {
    const d = this.drag;
    if (!d) return;
    this.drag = null;
    if (!d.moved) {
      if (Date.now() - d.at < 600) this.onClick();
      return;
    }
    // 커서가 있는 모니터로 옮겨간다
    const cur = screen.getCursorScreenPoint();
    this.display = screen.getDisplayNearestPoint(cur);
    const { minX, maxX } = this.bounds();
    this.behavior.setBounds(minX, maxX);
    const [x, y] = this.win.getPosition();
    this.fall = { x: Math.min(Math.max(x, minX), maxX), y: Math.min(y, this.y), vy: 0 };
    this.setInteractive(false);
  }

  savePosition() {
    this.settings.update((s) => {
      s.position = { x: Math.round(this.behavior.x), displayId: this.display.id };
    });
  }

  setCalm(on) {
    if (this.behavior) this.behavior.calm = on;
  }

  isBusy() {
    return !!(this.drag || this.fall || this.bubble);
  }

  showBubble(b) {
    if (this.drag || this.fall) return false;
    this.bubble = b;
    this.behavior.startTalking();
    this.send('cat:bubble', b);
    this.armBubbleTimer();
    return true;
  }

  armBubbleTimer() {
    clearTimeout(this.bubbleTimer);
    this.bubbleTimer = setTimeout(() => {
      if (this.bubbleHovered) return this.armBubbleTimer();
      const b = this.bubble;
      this.hideBubble();
      if (b) this.onBubbleAction(b.id, 'timeout');
    }, BUBBLE_MS);
  }

  hideBubble() {
    clearTimeout(this.bubbleTimer);
    if (!this.bubble) return;
    this.bubble = null;
    this.bubbleHovered = false;
    this.behavior.stopTalking();
    this.send('cat:hide-bubble');
    this.setInteractive(false);
  }

  setVisible(on) {
    if (!this.win) return;
    if (on) this.win.showInactive();
    else {
      this.hideBubble();
      this.win.hide();
    }
  }

  isVisible() {
    return this.win && this.win.isVisible();
  }

  bindIpc(ipcMain) {
    const own = (e) => this.win && e.sender === this.win.webContents;
    ipcMain.on('cat:interactive', (e, on) => own(e) && this.setInteractive(!!on));
    ipcMain.on('cat:drag-start', (e) => own(e) && this.dragStart());
    ipcMain.on('cat:drag-end', (e) => own(e) && this.dragEnd());
    ipcMain.on('cat:context-menu', (e) => own(e) && this.onContextMenu(this.win));
    ipcMain.on('cat:bubble-hover', (e, on) => {
      if (!own(e)) return;
      this.bubbleHovered = !!on;
    });
    ipcMain.on('cat:bubble-action', (e, { id, action }) => {
      if (!own(e) || !this.bubble || this.bubble.id !== id) return;
      this.hideBubble();
      this.onBubbleAction(id, action);
    });
  }
}

module.exports = { CatController, WIN_W, WIN_H };
