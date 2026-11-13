// 화면(HTML)과 앱(Rust) 사이의 다리. 예전 window.sai / window.cat 형식을 그대로 제공한다.
(function () {
  const T = window.__TAURI__;
  const invoke = T.core.invoke;
  const listen = (name, fn) => T.event.listen(name, (e) => fn(e.payload));
  const call = (ch) => (args) => invoke('call', { ch, args: args === undefined ? null : args });

  if (document.documentElement.dataset.window === 'cat') {
    const ev = (name, args) => invoke('cat_event', { ev: name, args: args === undefined ? null : args }).catch(() => {});
    window.cat = {
      onState: (fn) => listen('cat:state', fn),
      onBubble: (fn) => listen('cat:bubble', fn),
      onHideBubble: (fn) => listen('cat:hide-bubble', () => fn()),
      setInteractive: () => {},
      regions: (list) => ev('regions', list),
      dragStart: () => ev('drag-start'),
      dragEnd: () => ev('drag-end'),
      contextMenu: () => ev('context-menu'),
      bubbleAction: (id, action) => ev('bubble-action', { id, action }),
      bubbleHover: (on) => ev('bubble-hover', !!on),
      ready: () => ev('ready'),
    };
    return;
  }

  const names = {
    start: 'chat:start', send: 'chat:send', summarizeNow: 'chat:summarize-now', updateSummary: 'chat:update-summary',
    list: 'chat:list', get: 'chat:get', remove: 'chat:delete', removeAll: 'chat:delete-all',
    topics: 'topics:list', sharePreview: 'share:preview', shareSave: 'share:save', outbox: 'share:outbox', outboxRemove: 'share:remove',
    getSettings: 'settings:get', setSettings: 'settings:set', setApiKey: 'ai:set-key', testAi: 'ai:test', resetDampened: 'settings:reset-dampened',
    shareConfirm: 'share:confirm', shareSetRecipient: 'share:set-recipient',
    teamStatus: 'team:status', teamConfigure: 'team:configure', teamCreate: 'team:create', teamJoin: 'team:join', teamLeave: 'team:leave',
    teamSwitch: 'team:switch', teamSync: 'team:sync', teamRespond: 'team:respond', teamHoldTopic: 'team:hold-topic', teamPropose: 'team:propose',
    collabView: 'collab:view', collabSaveProfile: 'collab:save-profile', collabCreateProject: 'collab:create-project',
    collabUpdateProject: 'collab:update-project', collabSetActive: 'collab:set-active', collabAddTask: 'collab:add-task',
    collabEditTask: 'collab:edit-task', collabDeleteTask: 'collab:delete-task', collabAccept: 'collab:accept', collabObject: 'collab:object',
    collabStatus: 'collab:status', collabTogglePrep: 'collab:toggle-prep', collabPlan: 'collab:plan', collabApplyPlan: 'collab:apply-plan',
    updateStatus: 'update:status', updateCheck: 'update:check', updateInstall: 'update:install',
    extStatus: 'ext:status', tgSetBot: 'tg:set-bot', tgStartLink: 'tg:start-link', tgCheckLink: 'tg:check-link', tgTeamAlerts: 'tg:team-alerts',
    tgAway: 'tg:away', tgUnlink: 'tg:unlink', tgTest: 'tg:test', gConfigure: 'g:configure', gLogin: 'g:login', gLogout: 'g:logout',
    gSync: 'g:sync', gAvailability: 'g:availability',
    schedView: 'sched:view', schedReplan: 'sched:replan', schedApply: 'sched:apply', schedEdit: 'sched:edit', schedChat: 'sched:chat',
    schedClearChat: 'sched:clear-chat',
  };
  const sai = {};
  for (const [k, ch] of Object.entries(names)) sai[k] = call(ch);
  sai.onStream = (fn) => listen('chat:stream', fn);
  sai.onShowTab = (fn) => listen('chat:show-tab', fn);
  sai.onStartExplain = (fn) => listen('chat:start-explain', fn);
  sai.onFocusTask = (fn) => listen('chat:focus-task', fn);
  window.sai = sai;
})();
