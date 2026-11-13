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
  const errMsg = (e) => String((e && e.message) || e).replace(/^Error invoking remote method '[^']+': (Error: )?/, '');

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
    if (tab === 'collab' && window.SaiCollab) window.SaiCollab.render(true);
    if (tab === 'settings') {
      loadSettings();
      renderTeam();
      if (window.SaiExt) window.SaiExt.render();
      renderUpdate().catch(() => {});
    }
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
    // 보내기 버튼은 막지 않는다 — 답을 기다리는 중에 보낸 말은 줄을 세워 두었다가 보낸다
    $('#skipBtn').disabled = on;
    $('#summarizeBtn').disabled = on;
  }

  // ---------- 일정 조정 ----------
  let schedCard = null;
  function showSchedCard(view) {
    if (schedCard) schedCard.remove();
    schedCard = el('div', { class: 'msg-card' });
    window.SaiSchedule.render(schedCard, view, {
      compact: true,
      say: (t, bad) => addMsg('cat', t, bad ? 'safety' : ''),
      onView: (v) => (state.schedView = v),
    });
    messages.append(schedCard);
    messages.scrollTop = messages.scrollHeight;
  }

  async function startSchedule() {
    showTab('chat');
    queue.length = 0;
    messages.textContent = '';
    schedCard = null;
    state.session = { id: 'schedule', mode: 'schedule', step: 'talk' };
    $('#modePicker').classList.add('hidden');
    $('#composer').classList.remove('hidden');
    updateHelpers();
    setBusy(true);
    const typing = addMsg('cat', '…', 'typing');
    try {
      const v = await window.sai.schedView();
      state.schedView = v;
      typing.remove();
      for (const m of v.chat.slice(-8)) addMsg(m.role === 'user' ? 'user' : 'cat', m.text);
      if (!v.tasks.length) addMsg('cat', '아직 맡은 작업이 없어요. 협업 탭에서 작업을 맡으면 몇 시부터 몇 시까지 할지 짜 드릴게요.');
      else
        addMsg(
          'cat',
          `맡은 작업 ${v.tasks.length}개를 ${v.blocks.length}번에 나눠 잡아 봤어요. 틀린 곳이 있으면 편하게 말해 주세요. 예: "2번 목요일 3시로 옮겨줘", "수요일은 안 돼", "자료조사 3시간이면 돼". 괜찮으면 "캘린더에 넣어줘"라고 해 주세요.` +
            (v.ai ? '' : ' (Claude API 키를 넣으면 더 자유롭게 말해도 알아들어요.)'),
        );
      showSchedCard(v);
    } catch (e) {
      typing.textContent = '시간표를 불러오지 못했어요: ' + errMsg(e);
    } finally {
      setBusy(false);
      input.focus();
      drainQueue();
    }
  }

  async function sendSchedule(text, shown = false) {
    if (!shown) addMsg('user', text);
    setBusy(true);
    const typing = addMsg('cat', '…', 'typing');
    try {
      const r = await window.sai.schedChat({ text });
      typing.remove();
      addMsg('cat', r.reply);
      state.schedView = r.view;
      showSchedCard(r.view);
    } catch (e) {
      typing.textContent = '고치지 못했어요: ' + errMsg(e);
    } finally {
      setBusy(false);
      input.focus();
      drainQueue();
    }
  }
  window.SaiChat = { startSchedule };

  async function startMode(mode, topic = null) {
    if (mode === 'schedule') return startSchedule();
    queue.length = 0;
    messages.textContent = '';
    $('#modePicker').classList.add('hidden');
    $('#composer').classList.remove('hidden');
    setBusy(true);
    const typing = addMsg('cat', '…', 'typing');
    try {
      const res = await window.sai.start({ mode, topic });
      state.session = res.session;
      typing.remove();
      res.replies.forEach((r) => addMsg('cat', r.text));
      updateHelpers();
    } catch (e) {
      typing.textContent = '대화를 시작하지 못했어요: ' + e.message;
    } finally {
      setBusy(false);
      input.focus();
      drainQueue();
    }
  }

  function updateHelpers() {
    const s = state.session;
    const guided = s && s.mode !== 'free' && s.mode !== 'schedule' && s.step !== 'done';
    $('#skipBtn').classList.toggle('hidden', !guided);
    $('#summarizeBtn').classList.toggle('hidden', !guided);
  }

  // 답을 기다리는 동안 보낸 말은 줄을 세워 두었다가 차례로 보낸다 (입력이 씹히지 않도록)
  const queue = [];
  let streamSeq = 0;
  let streaming = null; // { id, bubble }
  window.sai.onStream(({ streamId, text }) => {
    if (!streaming || streaming.id !== streamId) return;
    const b = streaming.bubble;
    b.classList.remove('typing');
    b.textContent = text;
    state.talkingUntil = Date.now() + 600;
    const nearBottom = messages.scrollHeight - messages.scrollTop - messages.clientHeight < 120;
    if (nearBottom) messages.scrollTop = messages.scrollHeight;
  });

  async function send({ skip = false } = {}) {
    if (!state.session) return;
    const text = input.value.trim();
    if (!text && !skip) return;
    input.value = '';
    if (state.busy) {
      if (text) {
        queue.push(text);
        addMsg('user', text, 'queued');
      }
      return;
    }
    if (state.session.mode === 'schedule') return sendSchedule(text);
    if (text) addMsg('user', text);
    return sendNow(text, skip);
  }

  async function sendNow(text, skip = false) {
    setBusy(true);
    const typing = addMsg('cat', '…', 'typing');
    const streamId = `s${++streamSeq}`;
    streaming = { id: streamId, bubble: typing };
    try {
      const res = await window.sai.send({ id: state.session.id, text, skip, streamId });
      streaming = null;
      state.session = res.session;
      if (res.replies.length === 1 && res.replies[0].streamed) {
        typing.classList.remove('typing');
        typing.textContent = res.replies[0].text;
      } else {
        typing.remove();
        res.replies.forEach((r) => addMsg('cat', r.text, r.safety ? 'safety' : ''));
      }
      if (res.summary) renderSummary(res.summary, state.session.id, messages);
      updateHelpers();
    } catch (e) {
      streaming = null;
      typing.classList.remove('typing');
      typing.textContent = '답하지 못했어요: ' + errMsg(e);
    } finally {
      setBusy(false);
      input.focus();
      drainQueue();
    }
  }

  function drainQueue() {
    if (state.busy || !queue.length || !state.session) return;
    const next = queue.shift();
    messages.querySelector('.msg.user.queued')?.classList.remove('queued');
    if (state.session.mode === 'schedule') sendSchedule(next, true);
    else sendNow(next);
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
    if (summary.mode === 'explain') {
      shareBtn.remove();
      if (!readOnly) addExplainActions(summary, card);
    } else if (summary.mode !== 'complaint') shareBtn.remove();
    else shareBtn.addEventListener('click', () => openSharePanel(sessionId, summary, card));

    container.append(card);
    container.scrollTop = container.scrollHeight;
    return card;
  }

  // ---------- 내 사정 정리 후 할 수 있는 일 ----------
  function addExplainActions(summary, card) {
    const topic = summary.relatedTopic || (summary.topics || [])[0];
    if (!topic) return;
    const box = el('div', { class: 'card-actions' });
    if (summary.relatedTopic) {
      box.append(
        el('button', {
          text: '이 주제 조언 잠시 멈추기',
          onclick: async (e) => {
            e.target.disabled = true;
            const r = await window.sai.teamHoldTopic(topic).catch(() => ({ held: 0 }));
            addMsg(
              'cat',
              r.held
                ? `"${topicLabel(topic)}" 조언은 잠시 멈출게요. 사정을 모른 채 같은 말을 반복하지 않도록요.`
                : `"${topicLabel(topic)}" 조언을 줄여 둘게요. (팀에서 받은 관련 주제가 없어서 내 쪽에서만 줄였어요)`,
            );
          },
        }),
      );
    }
    box.append(
      el('button', {
        class: 'primary',
        text: '팀 약속으로 맞춰 보자고 제안',
        onclick: async (e) => {
          e.target.disabled = true;
          const r = await window.sai.teamPropose(topic);
          addMsg(
            'cat',
            r.connected
              ? `"${topicLabel(topic)}"에 대해 팀이 함께 맞출 약속을 제안해 둘게요. 누가 제안했는지는 보이지 않아요.`
              : '팀에 연결되면 이 제안이 전달돼요. 설정에서 팀을 연결해 주세요.',
          );
        },
      }),
    );
    card.append(box);
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
    let shareTeamId = preview.defaultTeamId;
    const teamById = new Map((preview.teams || []).map((t) => [t.teamId, t]));
    if (preview.teams && preview.teams.length > 1) {
      // 팀플이 여럿이면 어느 팀 이야기인지 고른다. 받을 팀원 목록도 그 팀으로 바뀐다
      panel.append(
        el('div', { class: 'label', text: '어느 팀의 이야기인가요?' }),
        el(
          'select',
          {
            class: 'team-pick',
            onchange: (e) => {
              shareTeamId = e.target.value;
              const t = teamById.get(shareTeamId);
              for (const sel of panel.querySelectorAll('select.recipient')) {
                sel.textContent = '';
                sel.append(el('option', { value: '', text: '받을 팀원 고르기' }), ...t.members.map((m) => el('option', { value: m.uid, text: m.displayName })));
              }
              for (const c of Object.values(choices)) c.toMember = null;
            },
          },
          preview.teams.map((t) => el('option', { value: t.teamId, text: `${t.name} (${t.memberCount}명)`, selected: t.teamId === shareTeamId })),
        ),
      );
    } else if (preview.team) {
      panel.append(el('p', { class: 'hint', text: `연결된 팀: ${preview.team.name} (${preview.team.memberCount}명)` }));
    } else {
      panel.append(el('div', { class: 'warn', text: '아직 팀에 연결되지 않았어요. 저장해 두면 팀에 연결한 뒤 받을 사람을 골라 보낼 수 있어요.' }));
    }
    for (const p of preview.items) {
      choices[p.topic] = { scope: p.defaultScope, target: 'person', toMember: null };
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
      // 팀에 연결돼 있으면 항상 그린다 (고른 팀에 따라 목록이 바뀌므로, 지금 팀에 다른 팀원이 없어도)
      if ((preview.teams || []).length) {
        const current = teamById.get(shareTeamId);
        const ms = current ? current.members : preview.members || [];
        const sel = el(
          'select',
          { class: 'recipient', onchange: (e) => (choices[p.topic].toMember = e.target.value || null) },
          el('option', { value: '', text: ms.length ? '받을 팀원 고르기' : '이 팀에는 아직 다른 팀원이 없어요' }),
          ms.map((m) => el('option', { value: m.uid, text: m.displayName })),
        );
        opt.append(el('div', { class: 'label', text: '관련된 팀원 (상대에게 내 이름은 보이지 않아요)' }), sel);
      }
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
        teamId: shareTeamId,
        choices: Object.entries(choices).map(([topic, c]) => ({ topic, ...c })),
      });
      panel.remove();
      addMsg(
        'cat',
        !res.saved
          ? '모두 비공개로 두었어요. 언제든 정리함에서 다시 정할 수 있어요.'
          : res.connected
            ? `${res.teamName ? `"${res.teamName}" 팀으로 ` : ''}${res.saved}개 주제를 공유 대기함에 넣었어요. 원문과 이름은 들어가지 않고, 시간을 조금 두고 전달돼요. "검토 후 공유"는 정리함에서 확인한 뒤에 보내져요.`
            : `${res.saved}개 주제를 공유 대기함에 넣어 두었어요. 팀에 연결되면 전달돼요.`,
      );
    });
    panel.append(el('div', { class: 'card-actions' }, el('span'), save));
    card.append(panel);
    panel.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
  }

  // ---------- 정리함 ----------
  const SCOPE_TEXT = { review: '검토 후 공유', auto: '자동 반영 허용' };
  const TARGET_TEXT = { person: '관련 팀원의 고양이', team: '팀 공통 제안' };
  const STATUS_TEXT = {
    review: '확인 기다림',
    'needs-recipient': '받을 팀원 고르기',
    queued: '곧 전달돼요',
    sent: '전달됨',
    held: '상대가 사정을 이야기함',
  };

  async function renderNotes() {
    const [outbox, sessions, teamInfo] = await Promise.all([window.sai.outbox(), window.sai.list(), window.sai.teamStatus()]);
    const others = (teamInfo.members || []).filter((m) => !m.me);
    const nameOf = (uid) => (teamInfo.members || []).find((m) => m.uid === uid)?.displayName;

    // 팀 약속
    const pl = $('#proposals');
    pl.textContent = '';
    if (!teamInfo.connected) pl.append(el('div', { class: 'empty', text: '팀에 연결하면 여기에 팀 약속 제안이 보여요.' }));
    else if (!teamInfo.proposals.length) pl.append(el('div', { class: 'empty', text: '아직 팀 약속 제안이 없어요.' }));
    for (const p of teamInfo.proposals || []) {
      const respond = (r) => async () => {
        await window.sai.teamRespond({ proposalId: p.id, response: r });
        renderNotes();
      };
      pl.append(
        el(
          'div',
          { class: `item ${p.agreedByAll ? 'agreed' : ''}` },
          el(
            'div',
            { class: 'main' },
            el('div', { class: 'title', text: `${p.label} · ${p.agreedByAll ? '팀 약속' : `동의 ${p.agree}/${p.memberCount}명`}` }),
            el('div', { class: 'sub', text: p.text, title: p.text }),
          ),
          p.mine === 'agree'
            ? el('span', { class: 'status-pill sent', text: '동의함' })
            : el('button', { class: 'primary', text: '좋아요', onclick: respond('agree') }),
          p.mine ? null : el('button', { class: 'link', text: '나중에', onclick: respond('later') }),
        ),
      );
    }

    // 공유 대기함
    const ob = $('#outbox');
    ob.textContent = '';
    if (!outbox.length) ob.append(el('div', { class: 'empty', text: '아직 공유하기로 한 주제가 없어요.' }));
    for (const it of outbox) {
      const status = it.status || (it.scope === 'review' ? 'review' : 'queued');
      // 받을 팀원 고르기·보내기는 그 팀을 보고 있을 때만 (팀원 목록이 팀마다 다르므로)
      const here = teamInfo.connected && (!it.teamId || it.teamId === teamInfo.activeTeamId);
      const controls = el('div', { class: 'controls' });
      const recipientSelect = () =>
        el(
          'select',
          { 'data-for': it.id },
          el('option', { value: '', text: '받을 팀원' }),
          others.map((m) => el('option', { value: m.uid, text: m.displayName, selected: m.uid === it.toMember })),
        );
      if (here && status === 'review') {
        const sel = it.target === 'person' ? recipientSelect() : null;
        if (sel) controls.append(sel);
        controls.append(
          el('button', {
            class: 'primary',
            text: '이 문안으로 보내기',
            onclick: async () => {
              await window.sai.shareConfirm({ itemId: it.id, toMember: sel ? sel.value : null });
              renderNotes();
            },
          }),
        );
      } else if (here && status === 'needs-recipient') {
        const sel = recipientSelect();
        controls.append(
          sel,
          el('button', {
            text: '정하기',
            onclick: async () => {
              if (!sel.value) return;
              await window.sai.shareSetRecipient({ itemId: it.id, toMember: sel.value });
              renderNotes();
            },
          }),
        );
      } else if (status === 'held') {
        controls.append(
          el('button', {
            text: '팀 약속으로 맞춰 보자고 제안',
            onclick: async () => {
              await window.sai.teamPropose(it.topic);
              renderNotes();
            },
          }),
        );
      }
      const to = it.target === 'person' && it.toMember && nameOf(it.toMember) ? ` → ${nameOf(it.toMember)}` : '';
      const teamTag = (teamInfo.teams || []).length > 1 && it.teamName ? `[${it.teamName}] ` : '';
      ob.append(
        el(
          'div',
          { class: 'item' },
          el(
            'div',
            { class: 'main' },
            el('div', { class: 'title', text: `${teamTag}${it.label} · ${TARGET_TEXT[it.target]}${to}` }),
            el('div', { class: 'sub', text: it.target === 'team' ? it.teamProposal : it.advice, title: it.advice }),
            status === 'held'
              ? el('div', { class: 'hint', text: '상대가 자기 사정을 이야기해서 이 조언은 멈췄어요. 한쪽 이야기만으로 교정하지 않도록, 팀 약속으로 함께 맞춰 보는 걸 추천해요.' })
              : null,
            controls.childNodes.length ? controls : null,
          ),
          el('span', { class: `status-pill ${status}`, text: teamInfo.connected || status !== 'queued' ? STATUS_TEXT[status] : '팀 연결 전' }),
          ['sent', 'held'].includes(status)
            ? null
            : el('button', {
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

  // ---------- 팀 연결 ----------
  // 머리말의 팀 고르기 (팀이 둘 이상일 때)
  function refreshTeamSwitch(info) {
    const sel = $('#teamSwitch');
    const teams = info.teams || [];
    sel.textContent = '';
    for (const t of teams) sel.append(el('option', { value: t.teamId, text: t.name, selected: t.active }));
    sel.classList.toggle('hidden', teams.length < 2);
  }

  $('#teamSwitch').addEventListener('change', async (e) => {
    const info = await window.sai.teamSwitch(e.target.value);
    refreshTeamSwitch(info);
    const current = document.querySelector('.tabs button.active');
    showTab(current ? current.dataset.tab : 'chat');
  });

  async function renderTeam(info) {
    info = info || (await window.sai.teamStatus());
    refreshTeamSwitch(info);
    const list = $('#teamList');
    list.textContent = '';
    for (const t of info.teams || []) {
      list.append(
        el(
          'div',
          { class: `item ${t.active ? 'agreed' : ''}` },
          el(
            'div',
            { class: 'main' },
            el('div', { class: 'title', text: `${t.name} · 팀원 ${t.memberCount}명` }),
            el('div', { class: 'sub', text: `참여 코드 ${t.joinCode} · 내 이름 ${t.displayName}${t.lastError ? ' · 연결 문제 있음' : ''}` }),
          ),
          t.active
            ? el('span', { class: 'status-pill sent', text: '보는 중' })
            : el('button', { text: '보기', onclick: teamAction(() => window.sai.teamSwitch(t.teamId), `${t.name}을 보고 있어요.`) }),
        ),
      );
    }
    state.activeTeamName = ((info.teams || []).find((t) => t.active) || {}).name;
    $('#teamJoinTitle').textContent = (info.teams || []).length ? '다른 팀 추가하기 (만들기 또는 참여 코드)' : '';
    const st = $('#teamStatus');
    st.textContent = '';
    const fmtTime = (t) => (t ? new Date(t).toLocaleTimeString('ko-KR', { hour: '2-digit', minute: '2-digit' }) : '아직 없음');
    if (!info.configured) {
      st.append(el('div', { text: '팀 서버가 아직 설정되지 않았어요.' }));
    } else if (!info.connected) {
      st.append(el('div', { text: `Firebase: ${info.projectId} · 팀에 아직 참여하지 않았어요.` }));
    } else {
      st.append(
        el('div', {}, el('b', { text: info.team.name }), ` · 팀원 ${info.memberCount}명 · 마지막 동기화 ${fmtTime(info.lastSync)}`),
        el('div', {}, '참여 코드 ', el('span', { class: 'code-big', text: info.team.joinCode }), ' — 팀원에게 알려 주세요'),
        el('div', { class: 'hint', text: info.localMode ? '로컬 시험 모드' : `Firebase: ${info.projectId}` }),
      );
    }
    if (info.lastError) st.append(el('div', { class: 'err', text: `연결 문제: ${info.lastError}` }));

    const editing = $('#teamConfig').dataset.editing === '1';
    $('#teamConfig').classList.toggle('hidden', info.localMode || (info.configured && !editing));
    // 팀이 있어도 다른 팀을 더 만들거나 참여할 수 있다
    $('#teamJoin').classList.toggle('hidden', !info.configured);
    $('#teamConnected').classList.toggle('hidden', !info.connected);
    $('#fbEdit').classList.toggle('hidden', !!info.localMode);
    const ml = $('#memberList');
    ml.textContent = '';
    for (const m of info.members || []) {
      ml.append(el('div', { class: 'item' }, el('div', { class: 'main' }, el('div', { class: 'title', text: m.displayName + (m.me ? ' (나)' : '') }))));
    }
  }

  function teamAction(fn, okText) {
    return async () => {
      const msg = $('#teamMsg');
      msg.textContent = '처리하는 중…';
      try {
        const info = await fn();
        msg.textContent = okText || '';
        await renderTeam(info);
        $('#teamName').value = '';
        $('#joinCode').value = '';
      } catch (e) {
        msg.textContent = errMsg(e);
      }
    };
  }

  $('#fbSave').addEventListener(
    'click',
    teamAction(async () => {
      const info = await window.sai.teamConfigure($('#fbConfig').value);
      $('#teamConfig').dataset.editing = '0';
      $('#fbConfig').value = '';
      return info;
    }, 'Firebase 설정을 저장했어요.'),
  );
  $('#fbEdit').addEventListener('click', () => {
    $('#teamConfig').dataset.editing = '1';
    $('#teamConfig').classList.remove('hidden');
  });
  $('#teamCreate').addEventListener(
    'click',
    teamAction(
      () => window.sai.teamCreate({ name: $('#teamName').value, displayName: $('#displayName').value }),
      '팀을 만들었어요. 참여 코드를 팀원에게 알려 주세요.',
    ),
  );
  $('#teamJoinBtn').addEventListener(
    'click',
    teamAction(() => window.sai.teamJoin({ code: $('#joinCode').value, displayName: $('#displayName').value }), '팀에 참여했어요.'),
  );
  $('#teamSyncBtn').addEventListener('click', teamAction(() => window.sai.teamSync(), '동기화했어요.'));
  $('#teamLeave').addEventListener('click', () => {
    const name = state.activeTeamName || '이 팀';
    if (!confirm(`"${name}"에서 나갈까요? 이 팀으로 보낸 기록이 지워지고, 다시 참여하려면 참여 코드가 필요해요. 다른 팀은 그대로예요.`)) return;
    teamAction(() => window.sai.teamLeave(), '팀에서 나왔어요.')();
  });

  // ---------- 버전 · 업데이트 ----------
  const UPDATE_TEXT = {
    idle: '아직 확인하지 않았어요.',
    checking: '확인하는 중이에요…',
    downloading: '새 버전을 받는 중이에요',
    ready: '새 버전이 준비됐어요.',
    latest: '최신 버전이에요.',
    error: '확인하지 못했어요',
    unsupported: '',
  };
  async function renderUpdate(st) {
    st = st || (await window.sai.updateStatus());
    const box = $('#updateInfo');
    box.textContent = '';
    box.append(el('div', {}, '지금 버전 ', el('b', { text: st.version || st.current || '' })));
    let line = UPDATE_TEXT[st.status] || '';
    if (st.status === 'downloading') line += ` (${st.progress || 0}%) — ${st.latest}`;
    if (st.status === 'ready') line = `새 버전 ${st.latest}이 준비됐어요.`;
    if (st.status === 'error' || st.status === 'unsupported') line = `${line ? `${line}: ` : ''}${st.error || ''}`;
    if (line) box.append(el('div', { class: st.status === 'error' ? 'err' : 'hint', text: line }));
    $('#updateInstall').classList.toggle('hidden', st.status !== 'ready');
  }
  $('#updateCheck').addEventListener('click', async () => {
    $('#updateInfo').textContent = '확인하는 중이에요…';
    try {
      await renderUpdate(await window.sai.updateCheck());
    } catch (e) {
      $('#updateInfo').textContent = errMsg(e);
    }
  });
  $('#updateInstall').addEventListener('click', () => window.sai.updateInstall());

  // ---------- 설정 ----------
  const form = $('#settingsForm');

  async function loadSettings() {
    const s = await window.sai.getSettings();
    state.settings = s;
    form.frequency.value = ['low', 'normal', 'high'].includes(s.frequency) ? s.frequency : 'low';
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
    $('#engine').textContent = s.hasApiKey ? 'Claude 연결됨' : '기본 상담 흐름';
  }

  form.addEventListener('submit', async (e) => {
    e.preventDefault();
    if (form.apiKey.value.trim()) {
      await window.sai.setApiKey(form.apiKey.value.trim());
      form.apiKey.value = '';
    }
    let s;
    try {
      s = await window.sai.setSettings({
        frequency: form.frequency.value,
        dnd: form.dndOn.checked ? { start: form.dndStart.value, end: form.dndEnd.value } : null,
        retentionDays: Number(form.retentionDays.value),
        ai: { enabled: form.aiEnabled.checked, model: form.model.value.trim() || 'claude-sonnet-5-5' },
      });
    } catch (err) {
      $('#saveStatus').textContent = errMsg(err);
      return;
    }
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


  // ---------- 시작 ----------
  (async function init() {
    state.topics = await window.sai.topics();
    const s = await window.sai.getSettings();
    state.settings = s;
    updateEngine(s);
    window.sai.teamStatus().then(refreshTeamSwitch).catch(() => {});
    const params = new URLSearchParams(location.search);
    if (params.get('task')) window.SaiCollab && window.SaiCollab.focus(params.get('task'));
    showTab(params.get('tab') || 'chat');
    if (params.get('explain')) startMode('explain', params.get('explain'));
  })();

  window.sai.onStartExplain((topic) => {
    showTab('chat');
    startMode('explain', topic);
  });

  // 개발용 스냅숏에서 대화 흐름을 재현할 때 사용
  window.__saiDemo = { startMode, startSchedule, send: (text) => ((input.value = text), send()), showTab, renderTeam };
})();
