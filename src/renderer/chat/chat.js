(function () {
  const $ = (sel, root = document) => root.querySelector(sel);
  const messages = $('#messages');
  const input = $('#input');
  const avatar = $('#avatar');

  const state = { session: null, topics: [], busy: false, settings: null, talkingUntil: 0 };

  // ---------- 작은 도우미 ----------
  function el(tag, attrs = {}, ...children) {
    const n = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (k === 'class') n.className = v;
      else if (k === 'text') n.textContent = v;
      else if (k.startsWith('on')) n.addEventListener(k.slice(2), v);
      else if (v !== false && v != null) n.setAttribute(k, v === true ? '' : v);
    }
    for (const c of children.flat()) if (c != null) n.append(c);
    return n;
  }

  const fmtDate = (t) =>
    new Date(t).toLocaleString('ko-KR', { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });

  const topicLabel = (id) => state.topics.find((t) => t.id === id)?.label || id;

  // ---------- 아바타 ----------
  let frame = 0;
  setInterval(() => {
    frame++;
    const talking = Date.now() < state.talkingUntil || state.busy;
    window.SaiSprite.render(avatar, talking ? 'talk' : 'sit', frame, -1, { eyes: frame % 9 === 0 ? 'closed' : 'open' });
  }, 220);

  // ---------- 탭 ----------
  function showTab(tab) {
    document.querySelectorAll('.tabs button').forEach((b) => b.classList.toggle('active', b.dataset.tab === tab));
    document.querySelectorAll('.tab').forEach((s) => s.classList.toggle('active', s.id === `tab-${tab}`));
    if (tab === 'notes') renderNotes();
    if (tab === 'settings') loadSettings();
  }
  document.querySelector('.tabs').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-tab]');
    if (b) showTab(b.dataset.tab);
  });
  window.sai.onShowTab(showTab);

  // ---------- 대화 ----------
  function addMsg(role, text, extra = '') {
    const m = el('div', { class: `msg ${role} ${extra}`.trim(), text });
    messages.append(m);
    messages.scrollTop = messages.scrollHeight;
    if (role === 'cat') state.talkingUntil = Date.now() + Math.min(4000, 400 + text.length * 40);
    return m;
  }

  function setBusy(on) {
    state.busy = on;
    $('#sendBtn').disabled = on;
    $('#skipBtn').disabled = on;
    $('#summarizeBtn').disabled = on;
  }

  async function startMode(mode) {
    messages.textContent = '';
    $('#modePicker').classList.add('hidden');
    $('#composer').classList.remove('hidden');
    setBusy(true);
    const typing = addMsg('cat', '…', 'typing');
    try {
      const res = await window.sai.start(mode);
      state.session = res.session;
      typing.remove();
      res.replies.forEach((r) => addMsg('cat', r.text));
      updateHelpers();
    } catch (e) {
      typing.textContent = '대화를 시작하지 못했어요: ' + e.message;
    } finally {
      setBusy(false);
      input.focus();
    }
  }

  function updateHelpers() {
    const s = state.session;
    const guided = s && s.mode !== 'free' && s.step !== 'done';
    $('#skipBtn').classList.toggle('hidden', !guided);
    $('#summarizeBtn').classList.toggle('hidden', !guided);
  }

  async function send({ skip = false } = {}) {
    if (state.busy || !state.session) return;
    const text = input.value.trim();
    if (!text && !skip) return;
    input.value = '';
    if (text) addMsg('user', text);
    setBusy(true);
    const typing = addMsg('cat', '…', 'typing');
    try {
      const res = await window.sai.send({ id: state.session.id, text, skip });
      typing.remove();
      state.session = res.session;
      res.replies.forEach((r) => addMsg('cat', r.text, r.safety ? 'safety' : ''));
      if (res.summary) renderSummary(res.summary, state.session.id, messages);
      updateHelpers();
    } catch (e) {
      typing.textContent = '답하지 못했어요: ' + e.message;
    } finally {
      setBusy(false);
      input.focus();
    }
  }

  $('#modePicker').addEventListener('click', (e) => {
    const b = e.target.closest('button[data-mode]');
    if (b) startMode(b.dataset.mode);
  });
  $('#sendBtn').addEventListener('click', () => send());
  $('#skipBtn').addEventListener('click', () => send({ skip: true }));
  $('#summarizeBtn').addEventListener('click', async () => {
    if (!state.session) return;
    const summary = await window.sai.summarizeNow(state.session.id);
    if (summary) {
      state.session.step = 'done';
      addMsg('cat', '지금까지 들은 내용으로 정리해 봤어요. 비어 있는 부분은 나중에 채워도 괜찮아요.');
      renderSummary(summary, state.session.id, messages);
      updateHelpers();
    }
  });
  $('#newBtn').addEventListener('click', () => {
    state.session = null;
    messages.textContent = '';
    $('#composer').classList.add('hidden');
    $('#modePicker').classList.remove('hidden');
  });
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      send();
    }
  });

  // ---------- 정리 카드 ----------
  const COMPLAINT_FIELDS = [
    ['내가 이야기한 사건', 'reportedEvent', 'text'],
    ['함께 정한 약속이었는지', 'agreement', 'text'],
    ['', 'agreementNote', 'note'],
    ['공동 기록', 'sharedRecord', 'note'],
    ['감정', 'emotions', 'join'],
    ['실제 영향', 'impact', 'text'],
    ['기대·필요', 'needs', 'text'],
    ['가능한 행동 요청', 'actionRequests', 'list'],
    ['사람 대신 행동으로 바꿔 말하면', 'reframes', 'reframes'],
    ['아직 확인할 점', 'toConfirm', 'list'],
  ];
  const EXPLAIN_FIELDS = [
    ['상황', 'situation', 'text'],
    ['그때의 사정', 'reason', 'text'],
    ['다음엔 이렇게', 'nextTime', 'text'],
    ['감정', 'emotions', 'join'],
    ['', 'note', 'note'],
  ];

  function renderSummary(summary, sessionId, container, { readOnly = false } = {}) {
    const card = $('#summaryTpl').content.firstElementChild.cloneNode(true);
    const dl = $('.fields', card);
    const fields = summary.mode === 'explain' ? EXPLAIN_FIELDS : COMPLAINT_FIELDS;
    for (const [label, key, kind] of fields) {
      const v = summary[key];
      if (v == null || (Array.isArray(v) && !v.length) || v === '') continue;
      if (label) dl.append(el('dt', { text: label }));
      let dd;
      if (kind === 'list') dd = el('dd', {}, el('ul', {}, v.map((x) => el('li', { text: x }))));
      else if (kind === 'reframes')
        dd = el('dd', {}, el('ul', {}, v.map((r) => el('li', { text: `${r.from} → ${r.to}` }))));
      else if (kind === 'join') dd = el('dd', { text: v.join(', ') });
      else dd = el('dd', { text: v, 'data-key': kind === 'text' ? key : null, class: kind === 'note' ? 'hint' : '' });
      dl.append(dd);
    }
    if (summary.profanity) {
      dl.append(el('dd', { class: 'hint', text: '거친 표현은 정리에서 뺐어요. 그만큼 힘들었다는 마음만 남겨 둘게요.' }));
    }
    if (summary.sensitive) {
      dl.append(
        el('dd', { class: 'hint', text: '건강·가족·평가처럼 민감할 수 있는 내용이 보여서, 자동 반영은 기본으로 꺼 둘게요.' }),
      );
    }
    if (summary.aiAssisted) dl.append(el('dd', { class: 'hint', text: 'AI가 정리를 도왔어요. 틀린 부분은 고쳐 주세요.' }));

    // 주제 칩
    const chips = $('.chips', card);
    const selected = new Set(summary.topics || []);
    for (const t of state.topics) {
      const chip = el('button', { class: `chip ${selected.has(t.id) ? 'on' : ''}`, text: t.label, disabled: readOnly });
      chip.addEventListener('click', async () => {
        if (selected.has(t.id)) selected.delete(t.id);
        else selected.add(t.id);
        chip.classList.toggle('on');
        summary.topics = [...selected];
        await window.sai.updateSummary({ id: sessionId, patch: { topics: summary.topics } });
      });
      chips.append(chip);
    }

    const editBtn = $('.edit', card);
    const shareBtn = $('.share', card);
    if (readOnly) {
      editBtn.remove();
    } else {
      editBtn.addEventListener('click', async () => {
        const editing = editBtn.dataset.editing === '1';
        const dds = card.querySelectorAll('dd[data-key]');
        if (!editing) {
          dds.forEach((d) => d.setAttribute('contenteditable', 'true'));
          editBtn.textContent = '고친 내용 저장';
          editBtn.dataset.editing = '1';
          dds[0]?.focus();
        } else {
          const patch = {};
          dds.forEach((d) => {
            d.removeAttribute('contenteditable');
            patch[d.dataset.key] = d.textContent.trim();
          });
          Object.assign(summary, patch);
          await window.sai.updateSummary({ id: sessionId, patch });
          editBtn.textContent = '내용 고치기';
          editBtn.dataset.editing = '0';
        }
      });
    }
    if (summary.mode !== 'complaint') shareBtn.remove();
    else shareBtn.addEventListener('click', () => openSharePanel(sessionId, summary, card));

    container.append(card);
    container.scrollTop = container.scrollHeight;
    return card;
  }

  // ---------- 공유 범위 ----------
  const SCOPES = [
    ['private', '비공개로 두기', '이 주제는 어디에도 반영하지 않아요.'],
    ['review', '검토 후 공유', '보내기 전에 일반화된 문안을 한 번 더 확인해요.'],
    ['auto', '허용한 주제로 자동 반영', '상대의 고양이가 평소 조언 속에 자연스럽게 섞어 말해요.'],
  ];
  const TARGETS = [
    ['person', '관련된 팀원의 고양이에게'],
    ['team', '팀 공통 약속으로 제안'],
  ];

  async function openSharePanel(sessionId, summary, card) {
    const old = card.querySelector('.share-panel');
    if (old) return old.remove();
    if (!summary.topics || !summary.topics.length) {
      card.append(el('p', { class: 'hint share-panel', text: '먼저 위에서 반영할 일반 주제를 하나 이상 골라 주세요.' }));
      return;
    }
    const preview = await window.sai.sharePreview({ id: sessionId });
    const panel = el('div', { class: 'share-panel' });
    panel.append(
      el('p', {
        class: 'hint',
        text: '전달되는 건 아래의 일반적인 문안뿐이에요. 내가 쓴 원문, 이름, 날짜, 구체적인 사건은 들어가지 않아요.',
      }),
    );
    const choices = {};
    for (const p of preview.items) {
      choices[p.topic] = { scope: p.defaultScope, target: 'person' };
      const opt = el('div', { class: 'option' }, el('div', { class: 'topic-name', text: p.label }));
      const scopes = el('div', { class: 'scopes' });
      for (const [val, label, desc] of SCOPES) {
        const disabled = val === 'auto' && p.sensitive;
        scopes.append(
          el(
            'label',
            { title: desc },
            el('input', {
              type: 'radio',
              name: `scope-${p.topic}`,
              value: val,
              checked: p.defaultScope === val,
              disabled,
              onchange: () => (choices[p.topic].scope = val),
            }),
            ` ${label}`,
            disabled ? ' (민감할 수 있어 제외)' : '',
          ),
        );
      }
      const targets = el('div', { class: 'scopes' });
      for (const [val, label] of TARGETS) {
        targets.append(
          el(
            'label',
            {},
            el('input', {
              type: 'radio',
              name: `target-${p.topic}`,
              value: val,
              checked: val === 'person',
              onchange: () => (choices[p.topic].target = val),
            }),
            ` ${label}`,
          ),
        );
      }
      opt.append(scopes, el('div', { class: 'label', text: '전달 방식' }), targets);
      opt.append(el('div', { class: 'preview', text: `상대의 고양이가 이렇게 말할 수 있어요: “${p.advice}”` }));
      opt.append(el('div', { class: 'preview', text: `팀 공통 제안으로는: “${p.teamProposal}”` }));
      if (p.sensitive) opt.append(el('div', { class: 'warn', text: '민감할 수 있는 내용이 있어 자동 반영은 할 수 없어요.' }));
      panel.append(opt);
    }
    if (preview.smallTeamWarning) panel.append(el('div', { class: 'warn', text: preview.smallTeamWarning }));
    const save = el('button', { class: 'primary', text: '공유 대기함에 저장' });
    save.addEventListener('click', async () => {
      const res = await window.sai.shareSave({
        id: sessionId,
        choices: Object.entries(choices).map(([topic, c]) => ({ topic, ...c })),
      });
      panel.remove();
      addMsg(
        'cat',
        res.saved
          ? `${res.saved}개 주제를 공유 대기함에 넣어 두었어요. 원문과 이름은 들어가지 않았어요. 팀 연결이 열리면 그때 반영돼요.`
          : '모두 비공개로 두었어요. 언제든 정리함에서 다시 정할 수 있어요.',
      );
    });
    panel.append(el('div', { class: 'card-actions' }, el('span'), save));
    card.append(panel);
    panel.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
  }

  // ---------- 정리함 ----------
  const SCOPE_TEXT = { review: '검토 후 공유', auto: '자동 반영 허용' };
  const TARGET_TEXT = { person: '관련 팀원의 고양이', team: '팀 공통 제안' };

  async function renderNotes() {
    const [outbox, sessions] = await Promise.all([window.sai.outbox(), window.sai.list()]);
    const ob = $('#outbox');
    ob.textContent = '';
    if (!outbox.length) ob.append(el('div', { class: 'empty', text: '아직 공유하기로 한 주제가 없어요.' }));
    for (const it of outbox) {
      ob.append(
        el(
          'div',
          { class: 'item' },
          el(
            'div',
            { class: 'main' },
            el('div', { class: 'title', text: `${it.label} · ${SCOPE_TEXT[it.scope]} · ${TARGET_TEXT[it.target]}` }),
            el('div', { class: 'sub', text: it.target === 'team' ? it.teamProposal : it.advice, title: it.advice }),
          ),
          el('button', {
            class: 'link',
            text: '빼기',
            onclick: async () => {
              await window.sai.outboxRemove(it.id);
              renderNotes();
            },
          }),
        ),
      );
    }

    const list = $('#sessions');
    list.textContent = '';
    if (!sessions.length) list.append(el('div', { class: 'empty', text: '저장된 대화가 없어요.' }));
    for (const s of sessions) {
      list.append(
        el(
          'div',
          { class: 'item' },
          el(
            'div',
            { class: 'main' },
            el('div', { class: 'title', text: `${s.modeLabel} · ${fmtDate(s.startedAt)}${s.hasSummary ? ' · 정리됨' : ''}` }),
            el('div', { class: 'sub', text: s.preview }),
          ),
          el('button', { text: '보기', onclick: () => viewSession(s.id) }),
          el('button', {
            class: 'link',
            text: '삭제',
            onclick: async () => {
              if (!confirm('이 대화를 지울까요? 되돌릴 수 없어요.')) return;
              await window.sai.remove(s.id);
              $('#sessionView').classList.add('hidden');
              renderNotes();
            },
          }),
        ),
      );
    }
  }

  async function viewSession(id) {
    const s = await window.sai.get(id);
    const view = $('#sessionView');
    view.textContent = '';
    view.classList.remove('hidden');
    const box = el('div', { class: 'messages' });
    for (const t of s.turns) box.append(el('div', { class: `msg ${t.role} ${t.safety ? 'safety' : ''}`, text: t.text }));
    view.append(el('h2', { text: fmtDate(s.startedAt) }), box);
    if (s.summary) renderSummary(s.summary, s.id, view);
    view.scrollIntoView({ behavior: 'smooth' });
  }

  // ---------- 설정 ----------
  const form = $('#settingsForm');

  async function loadSettings() {
    const s = await window.sai.getSettings();
    state.settings = s;
    form.frequency.value = s.frequency;
    form.quiet.checked = !!s.quiet;
    form.dndOn.checked = !!s.dnd;
    if (s.dnd) {
      form.dndStart.value = s.dnd.start;
      form.dndEnd.value = s.dnd.end;
    }
    form.retentionDays.value = String(s.retentionDays);
    form.aiEnabled.checked = !!s.ai.enabled;
    form.model.value = s.ai.model;
    form.apiKey.placeholder = s.hasApiKey ? '저장된 키가 있어요 (바꿀 때만 입력)' : 'sk-ant-...';
    updateEngine(s);
  }

  function updateEngine(s) {
    $('#catName').textContent = s.catName;
    $('#engine').textContent = s.ai.enabled && s.hasApiKey ? 'AI 대화 사용 중' : '기본 상담 흐름';
  }

  form.addEventListener('submit', async (e) => {
    e.preventDefault();
    if (form.apiKey.value.trim()) {
      await window.sai.setApiKey(form.apiKey.value.trim());
      form.apiKey.value = '';
    }
    const s = await window.sai.setSettings({
      frequency: form.frequency.value,
      quiet: form.quiet.checked,
      dnd: form.dndOn.checked ? { start: form.dndStart.value, end: form.dndEnd.value } : null,
      retentionDays: Number(form.retentionDays.value),
      ai: { enabled: form.aiEnabled.checked, model: form.model.value.trim() || 'claude-sonnet-5-5' },
    });
    state.settings = s;
    updateEngine(s);
    form.apiKey.placeholder = s.hasApiKey ? '저장된 키가 있어요 (바꿀 때만 입력)' : 'sk-ant-...';
    $('#saveStatus').textContent = '저장했어요.';
    setTimeout(() => ($('#saveStatus').textContent = ''), 2000);
  });

  $('#testAi').addEventListener('click', async () => {
    if (form.apiKey.value.trim()) {
      await window.sai.setApiKey(form.apiKey.value.trim());
      form.apiKey.value = '';
    }
    $('#aiStatus').textContent = '확인하는 중…';
    const r = await window.sai.testAi();
    $('#aiStatus').textContent = r.ok ? '연결됐어요.' : `연결하지 못했어요: ${r.error}`;
  });

  $('#deleteAll').addEventListener('click', async () => {
    if (!confirm('모든 대화 기록을 지울까요? 공유 대기함은 그대로 남아요.')) return;
    await window.sai.removeAll();
    $('#saveStatus').textContent = '대화 기록을 모두 지웠어요.';
  });

  $('#resetDampened').addEventListener('click', async () => {
    await window.sai.resetDampened();
    $('#saveStatus').textContent = '줄인 주제 기록을 초기화했어요.';
  });

  // ---------- 시작 ----------
  (async function init() {
    state.topics = await window.sai.topics();
    const s = await window.sai.getSettings();
    state.settings = s;
    updateEngine(s);
    const tab = new URLSearchParams(location.search).get('tab') || 'chat';
    showTab(tab);
  })();

  // 개발용 스냅숏에서 대화 흐름을 재현할 때 사용
  window.__saiDemo = { startMode, send: (text) => ((input.value = text), send()), showTab };
})();
