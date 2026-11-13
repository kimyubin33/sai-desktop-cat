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

  shareConfirm: invoke('share:confirm'),
  shareSetRecipient: invoke('share:set-recipient'),

  teamStatus: invoke('team:status'),
  teamConfigure: invoke('team:configure'),
  teamCreate: invoke('team:create'),
  teamJoin: invoke('team:join'),
  teamLeave: invoke('team:leave'),
  teamSync: invoke('team:sync'),
  teamRespond: invoke('team:respond'),
  teamHoldTopic: invoke('team:hold-topic'),
  teamPropose: invoke('team:propose'),

  collabView: invoke('collab:view'),
  collabSaveProfile: invoke('collab:save-profile'),
  collabCreateProject: invoke('collab:create-project'),
  collabUpdateProject: invoke('collab:update-project'),
  collabSetActive: invoke('collab:set-active'),
  collabAddTask: invoke('collab:add-task'),
  collabEditTask: invoke('collab:edit-task'),
  collabDeleteTask: invoke('collab:delete-task'),
  collabAccept: invoke('collab:accept'),
  collabObject: invoke('collab:object'),
  collabStatus: invoke('collab:status'),
  collabTogglePrep: invoke('collab:toggle-prep'),
  collabPlan: invoke('collab:plan'),
  collabApplyPlan: invoke('collab:apply-plan'),
  onFocusTask: (fn) => ipcRenderer.on('chat:focus-task', (_e, id) => fn(id)),

  onShowTab: (fn) => ipcRenderer.on('chat:show-tab', (_e, tab) => fn(tab)),
  onStartExplain: (fn) => ipcRenderer.on('chat:start-explain', (_e, topic) => fn(topic)),
});
