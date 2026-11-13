const { contextBridge, ipcRenderer } = require('electron');

const invoke = (ch) => (...args) => ipcRenderer.invoke(ch, ...args);

contextBridge.exposeInMainWorld('sai', {
  start: invoke('chat:start'),
  send: invoke('chat:send'),
  summarizeNow: invoke('chat:summarize-now'),
  updateSummary: invoke('chat:update-summary'),
  list: invoke('chat:list'),
  get: invoke('chat:get'),
  remove: invoke('chat:delete'),
  removeAll: invoke('chat:delete-all'),

  topics: invoke('topics:list'),
  sharePreview: invoke('share:preview'),
  shareSave: invoke('share:save'),
  outbox: invoke('share:outbox'),
  outboxRemove: invoke('share:remove'),

  getSettings: invoke('settings:get'),
  setSettings: invoke('settings:set'),
  setApiKey: invoke('ai:set-key'),
  testAi: invoke('ai:test'),
  resetDampened: invoke('settings:reset-dampened'),

  onShowTab: (fn) => ipcRenderer.on('chat:show-tab', (_e, tab) => fn(tab)),
});
