const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('cat', {
  onState: (fn) => ipcRenderer.on('cat:state', (_e, s) => fn(s)),
  onBubble: (fn) => ipcRenderer.on('cat:bubble', (_e, b) => fn(b)),
  onHideBubble: (fn) => ipcRenderer.on('cat:hide-bubble', () => fn()),
  setInteractive: (on) => ipcRenderer.send('cat:interactive', on),
  dragStart: () => ipcRenderer.send('cat:drag-start'),
  dragEnd: () => ipcRenderer.send('cat:drag-end'),
  contextMenu: () => ipcRenderer.send('cat:context-menu'),
  bubbleAction: (id, action) => ipcRenderer.send('cat:bubble-action', { id, action }),
  bubbleHover: (on) => ipcRenderer.send('cat:bubble-hover', on),
});
