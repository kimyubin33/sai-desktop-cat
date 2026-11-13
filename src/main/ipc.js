// 대화창과 메인 프로세스 사이의 통신
const { TOPICS } = require('../core/topics');
const { dndSpanMinutes, MAX_DND_MINUTES } = require('../core/advice-scheduler');
const { buildPreview, createOutboxItems, mergeIntoOutbox, leaksRawText } = require('../core/privacy');

function registerChatIpc(ipcMain, { stores, chat, ai, chatWindow, onSettingsChanged, team }) {
  const handle = (ch, fn) =>
    ipcMain.handle(ch, (e, ...args) => {
      if (!chatWindow.isOwn(e)) throw new Error('허용되지 않은 요청이에요.');
      return fn(...args);
    });

  handle('chat:start', (arg) => (typeof arg === 'string' ? chat.start(arg) : chat.start(arg.mode, arg.topic)));
  handle('chat:send', ({ id, text, skip }) => chat.send(id, String(text || '').slice(0, 4000), !!skip));
  handle('chat:summarize-now', (id) => chat.summarizeNow(id));
  handle('chat:update-summary', ({ id, patch }) => chat.updateSummary(id, patch || {}));
  handle('chat:list', () => chat.list());
  handle('chat:get', (id) => chat.get(id));
  handle('chat:delete', (id) => chat.delete(id));
  handle('chat:delete-all', () => chat.deleteAll());

  handle('topics:list', () => Object.entries(TOPICS).map(([id, t]) => ({ id, label: t.label })));

  // ---------- 공유 (여러 팀) ----------
  const hub = team.hub;
  const teamOfItem = (itemId) => {
    const it = stores.sharedOutbox.get().items.find((x) => x.id === itemId);
    return it && hub.bundle(it.teamId || null);
  };

  handle('share:preview', async ({ id }) => {
    const s = chat.find(id);
    if (!s || !s.summary) return { items: [] };
    // 받을 팀원 목록이 오래됐으면 먼저 새로 받아 온다
    const stale = hub.all().some((b) => Date.now() - (b.sync.cache.get().lastSync || 0) > 2 * 60 * 1000);
    if (hub.all().length && stale) await team.syncNow();
    const preview = buildPreview(s.summary, { key: s.id });
    const teams = hub.all().map((b) => ({
      teamId: b.teamId,
      name: b.name(),
      memberCount: b.sync.members({ includeMe: true }).length,
      members: b.sync.members(),
    }));
    const active = hub.active();
    const activeInfo = teams.find((t) => t.teamId === active.teamId) || null;
    return {
      ...preview,
      teams,
      defaultTeamId: active.teamId || null,
      // 예전 화면과의 호환 (보고 있는 팀)
      team: activeInfo ? { name: activeInfo.name, memberCount: activeInfo.memberCount } : null,
      members: activeInfo ? activeInfo.members : [],
      // 팀 규모와 상관없이 안내하되, 작은 팀이면 더 분명하게
      smallTeamWarning: activeInfo && activeInfo.memberCount > 3 ? null : preview.smallTeamWarning,
    };
  });

  handle('share:save', ({ id, choices, teamId }) => {
    const s = chat.find(id);
    if (!s || !s.summary || s.safetyHold) return { saved: 0 };
    const b = hub.bundle(teamId || hub.active().teamId || null) || hub.active();
    const raw = s.turns.filter((t) => t.role === 'user').map((t) => t.text);
    let items = createOutboxItems(s.summary, choices, { key: s.id });
    const recipients = new Map((choices || []).map((c) => [c.topic, c.toMember || null]));
    items = items.map((it) => ({ ...it, toMember: it.target === 'person' ? recipients.get(it.topic) : null }));
    // 혹시라도 원문 조각이 섞였다면 다른 일반 문안으로 바꾸고, 그래도 겹치면 저장하지 않는다
    items = items
      .map((it) => {
        if (!leaksRawText(it, raw, 12)) return it;
        const alt = TOPICS[it.topic].advice.find((a) => !leaksRawText({ ...it, advice: a }, raw, 12));
        return alt ? { ...it, advice: alt } : null;
      })
      .filter(Boolean);
    items = b.sync.prepareItems(items).map((it) => ({ ...it, teamId: b.teamId || null }));
    stores.sharedOutbox.update((d) => {
      d.items = mergeIntoOutbox(d.items, items);
    });
    if (b.sync.isConnected()) team.syncNow();
    return { saved: items.length, connected: b.sync.isConnected(), teamName: b.name() };
  });
  handle('share:confirm', ({ itemId, toMember }) => {
    const b = teamOfItem(itemId);
    if (b) b.sync.confirmReview(itemId, toMember || null);
    team.syncNow();
    return true;
  });
  handle('share:set-recipient', ({ itemId, toMember }) => {
    const b = teamOfItem(itemId);
    if (b) b.sync.setRecipient(itemId, toMember);
    team.syncNow();
    return true;
  });

  // ---------- 팀 ----------
  const teamView = () => {
    const b = hub.active();
    const c = b.sync.cache.get();
    return {
      ...team.status(),
      teams: team.teams(),
      activeTeamId: b.teamId || null,
      members: b.sync.members({ includeMe: true }).map((m) => ({ ...m, me: m.uid === c.myUid })),
      proposals: b.sync.proposals(),
      inboxCount: (c.inbox || []).length,
    };
  };
  const teamCall = (fn) => async (...args) => {
    await fn(...args);
    onSettingsChanged();
    return teamView();
  };
  handle('team:status', teamView);
  handle('team:configure', teamCall((text) => team.configure(text)));
  handle('team:create', teamCall(({ name, displayName }) => team.createTeam(name, displayName)));
  handle('team:join', teamCall(({ code, displayName }) => team.joinTeam(code, displayName)));
  handle('team:leave', teamCall((teamId) => team.leaveTeam(teamId)));
  handle('team:switch', teamCall((teamId) => team.setActive(teamId)));
  handle('team:sync', teamCall(() => team.syncNow()));
  handle('team:respond', teamCall(({ proposalId, response }) => hub.teamFacade().respond(proposalId, response)));
  handle('team:hold-topic', async (topic) => {
    const n = await hub.teamFacade().holdTopic(topic);
    return { held: n };
  });
  handle('team:propose', (topic) => {
    const ids = hub.teamFacade().proposeTeam(topic);
    if (ids.length) team.syncNow();
    return { queued: ids.length > 0, connected: hub.all().length > 0 };
  });

  handle('share:outbox', () => {
    const names = new Map(hub.teams().map((t) => [t.teamId, t.name]));
    return stores.sharedOutbox.get().items.map((x) => ({ ...x, teamName: x.teamId ? names.get(x.teamId) || '나간 팀' : null }));
  });
  handle('share:remove', (itemId) =>
    stores.sharedOutbox.update((d) => {
      d.items = d.items.filter((x) => x.id !== itemId);
    }) && true,
  );

  // ---------- 협업 (4단계) ----------
  // 보고 있는 팀이 바뀔 수 있으므로 매번 꺼내 쓴다
  const C = () => team.collab;
  const collabView = () => {
    const connected = team.sync.isConnected();
    if (!connected) return { connected: false };
    const d = C().data();
    const members = C().members();
    const nameOf = new Map(members.map((m) => [m.uid, m.name]));
    const project = C().active();
    return {
      connected,
      teamId: hub.active().teamId,
      teamName: hub.active().name(),
      teams: team.teams(),
      myUid: C().myUid(),
      members,
      myProfile: C().myProfile(),
      projects: d.projects,
      project,
      tasks: project
        ? C().tasksOf(project.id).map((t) => ({
            ...t,
            assigneeName: t.assignee ? nameOf.get(t.assignee) || '나간 팀원' : null,
            objectionNames: Object.keys(t.objections || {}).map((u) => nameOf.get(u) || '팀원'),
          }))
        : [],
      objectionReasons: require('../core/collab/collab-sync').OBJECTION_REASONS,
    };
  };
  const collabCall = (fn) => async (arg) => {
    await fn(arg || {});
    return collabView();
  };
  handle('collab:view', async ({ refresh } = {}) => {
    if (refresh && team.sync.isConnected()) await team.syncNow();
    return collabView();
  });
  handle('collab:save-profile', collabCall((p) => C().saveProfile(p)));
  handle('collab:create-project', collabCall((p) => C().createProject(p)));
  handle('collab:update-project', collabCall(({ id, ...p }) => C().updateProject(id, p)));
  handle('collab:set-active', collabCall(({ id }) => C().setActive(id)));
  handle('collab:add-task', collabCall(({ projectId, ...t }) => C().addTask(projectId, t)));
  handle('collab:edit-task', collabCall(({ projectId, id, ...t }) => C().editTask(projectId, id, t)));
  handle('collab:delete-task', collabCall(({ projectId, id }) => C().deleteTask(projectId, id)));
  handle('collab:accept', collabCall(({ id }) => C().accept(id)));
  handle('collab:object', collabCall(({ id, reason }) => C().object(id, reason)));
  handle('collab:status', collabCall(({ id, status }) => C().setStatus(id, status)));
  handle('collab:toggle-prep', collabCall(({ id, index }) => C().togglePrep(id, index)));
  handle('collab:plan', async ({ projectId }) => {
    if (team.sync.isConnected()) await team.syncNow();
    const p = C().plan(projectId);
    if (!p) return null;
    const names = new Map(C().members().map((m) => [m.uid, m.name]));
    const titles = new Map(C().tasksOf(projectId).map((t) => [t.id, t.title]));
    return {
      ...p,
      assignments: p.assignments.map((a) => ({ ...a, name: a.uid ? names.get(a.uid) : null, title: titles.get(a.taskId) })),
      missingProfiles: C().members().filter((m) => !m.hasProfile).map((m) => m.name),
    };
  });
  handle('collab:apply-plan', async ({ projectId, assignments }) => {
    const applied = await C().applyPlan(projectId, assignments || []);
    team.syncNow();
    return { applied: applied.length, view: collabView() };
  });

  // ---------- 외부 연결 (5단계) ----------
  const { shell } = require('electron');
  const extView = () => ({
    teamConnected: team.sync.isConnected(),
    telegram: team.notifier.status(),
    telegramAway: !!stores.settings.get().telegramAway,
    google: team.google.status(),
  });
  const extCall = (fn) => async (arg) => {
    await fn(arg);
    return extView();
  };
  handle('ext:status', extView);
  handle('tg:set-bot', extCall(async (token) => {
    await team.notifier.setBot(token);
    await team.notifier.sync();
  }));
  handle('tg:start-link', async () => {
    const { url } = team.notifier.startLink();
    shell.openExternal(url);
    return { url, view: extView() };
  });
  handle('tg:check-link', async () => ({ ...(await team.notifier.checkLink()), view: extView() }));
  handle('tg:team-alerts', extCall((on) => team.notifier.setTeamAlerts(on)));
  handle('tg:away', extCall((on) => stores.settings.update((s) => (s.telegramAway = !!on))));
  handle('tg:unlink', extCall(() => team.notifier.unlink()));
  handle('tg:test', extCall(() => team.notifier.test()));
  handle('g:configure', extCall((text) => team.google.configure(text)));
  handle('g:login', extCall(() => team.google.login()));
  handle('g:logout', extCall(() => team.google.logout()));
  handle('g:sync', async () => ({ result: await team.google.syncTodo(), view: extView() }));
  handle('g:availability', ({ projectId } = {}) => team.google.availability(projectId));
  handle('g:schedule-task', ({ id }) => team.google.scheduleTask(id));

  const publicSettings = () => {
    const s = stores.settings.get();
    return {
      catName: s.catName,
      frequency: s.frequency,
      quietUntil: s.quietUntil,
      dnd: s.dnd,
      retentionDays: s.retentionDays,
      ai: s.ai,
      hasApiKey: ai.hasKey(),
    };
  };

  handle('settings:get', publicSettings);
  handle('settings:set', (patch) => {
    stores.settings.update((s) => {
      if (['low', 'normal', 'high'].includes(patch.frequency)) s.frequency = patch.frequency;
      if ('dnd' in patch) {
        const d = patch.dnd && patch.dnd.start && patch.dnd.end ? { start: patch.dnd.start, end: patch.dnd.end } : null;
        // 방해 금지는 잠·휴식 시간용: 하루 10시간까지만 (그 이상이면 사실상 끄기가 되므로)
        if (d && dndSpanMinutes(d) > MAX_DND_MINUTES) throw new Error('방해 금지 시간은 하루 10시간까지 정할 수 있어요.');
        s.dnd = d;
      }
      if ([0, 7, 30, 90].includes(patch.retentionDays)) s.retentionDays = patch.retentionDays;
      if (patch.ai) s.ai = { enabled: !!patch.ai.enabled, model: String(patch.ai.model || s.ai.model).slice(0, 80) };
    });
    chat.purgeOld();
    onSettingsChanged();
    return publicSettings();
  });
  handle('settings:reset-dampened', () => {
    stores.settings.update((s) => {
      s.dampenedTopics = {};
    });
    return true;
  });
  handle('ai:set-key', (key) => {
    ai.setKey(key);
    return true;
  });
  handle('ai:test', () => ai.test());
}

module.exports = { registerChatIpc };
