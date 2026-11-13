// 대화창과 메인 프로세스 사이의 통신
const { TOPICS } = require('../core/topics');
const { buildPreview, createOutboxItems, mergeIntoOutbox, leaksRawText } = require('../core/privacy');

function registerChatIpc(ipcMain, { stores, chat, ai, chatWindow, onSettingsChanged }) {
  const handle = (ch, fn) =>
    ipcMain.handle(ch, (e, ...args) => {
      if (!chatWindow.isOwn(e)) throw new Error('허용되지 않은 요청이에요.');
      return fn(...args);
    });

  handle('chat:start', (mode) => chat.start(mode));
  handle('chat:send', ({ id, text, skip }) => chat.send(id, String(text || '').slice(0, 4000), !!skip));
  handle('chat:summarize-now', (id) => chat.summarizeNow(id));
  handle('chat:update-summary', ({ id, patch }) => chat.updateSummary(id, patch || {}));
  handle('chat:list', () => chat.list());
  handle('chat:get', (id) => chat.get(id));
  handle('chat:delete', (id) => chat.delete(id));
  handle('chat:delete-all', () => chat.deleteAll());

  handle('topics:list', () => Object.entries(TOPICS).map(([id, t]) => ({ id, label: t.label })));

  handle('share:preview', ({ id }) => {
    const s = chat.find(id);
    if (!s || !s.summary) return { items: [] };
    return buildPreview(s.summary, { key: s.id });
  });

  handle('share:save', ({ id, choices }) => {
    const s = chat.find(id);
    if (!s || !s.summary || s.safetyHold) return { saved: 0 };
    const raw = s.turns.filter((t) => t.role === 'user').map((t) => t.text);
    let items = createOutboxItems(s.summary, choices, { key: s.id });
    // 혹시라도 원문 조각이 섞였다면 다른 일반 문안으로 바꾸고, 그래도 겹치면 저장하지 않는다
    items = items
      .map((it) => {
        if (!leaksRawText(it, raw, 12)) return it;
        const alt = TOPICS[it.topic].advice.find((a) => !leaksRawText({ ...it, advice: a }, raw, 12));
        return alt ? { ...it, advice: alt } : null;
      })
      .filter(Boolean);
    stores.sharedOutbox.update((d) => {
      d.items = mergeIntoOutbox(d.items, items);
    });
    return { saved: items.length };
  });

  handle('share:outbox', () => stores.sharedOutbox.get().items);
  handle('share:remove', (itemId) =>
    stores.sharedOutbox.update((d) => {
      d.items = d.items.filter((x) => x.id !== itemId);
    }) && true,
  );

  const publicSettings = () => {
    const s = stores.settings.get();
    return {
      catName: s.catName,
      frequency: s.frequency,
      quiet: s.quiet,
      dnd: s.dnd,
      retentionDays: s.retentionDays,
      ai: s.ai,
      hasApiKey: ai.hasKey(),
    };
  };

  handle('settings:get', publicSettings);
  handle('settings:set', (patch) => {
    stores.settings.update((s) => {
      if (['off', 'low', 'normal', 'high'].includes(patch.frequency)) s.frequency = patch.frequency;
      if ('quiet' in patch) s.quiet = !!patch.quiet;
      if ('dnd' in patch) s.dnd = patch.dnd && patch.dnd.start && patch.dnd.end ? { start: patch.dnd.start, end: patch.dnd.end } : null;
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
