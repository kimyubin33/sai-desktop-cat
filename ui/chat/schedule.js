// 내 작업 시간표 카드: 협업 탭과 "일정 조정하기" 대화에서 함께 쓴다.
(function () {
  const api = window.sai;
  const DAYS = ['일', '월', '화', '수', '목', '금', '토'];

  function el(tag, attrs = {}, ...children) {
    const n = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (k === 'class') n.className = v;
      else if (k === 'text') n.textContent = v;
      else if (k.startsWith('on')) n.addEventListener(k.slice(2), v);
      else if (v !== false && v != null) n.setAttribute(k, v === true ? '' : v);
    }
    for (const c of children.flat()) if (c != null && c !== false) n.append(c);
    return n;
  }
  const errMsg = (e) => String((e && e.message) || e).replace(/^Error invoking remote method '[^']+': (Error: )?/, '');
  const p2 = (n) => String(n).padStart(2, '0');
  const hm = (t) => {
    const d = new Date(t);
    return `${p2(d.getHours())}:${p2(d.getMinutes())}`;
  };
  const dayLabel = (t) => {
    const d = new Date(t);
    const today = new Date();
    today.setHours(0, 0, 0, 0);
    const diff = Math.round((new Date(d).setHours(0, 0, 0, 0) - today.getTime()) / 86400000);
    const rel = diff === 0 ? '오늘' : diff === 1 ? '내일' : diff === 2 ? '모레' : '';
    return `${d.getMonth() + 1}월 ${d.getDate()}일 (${DAYS[d.getDay()]})${rel ? ` · ${rel}` : ''}`;
  };
  const localInput = (t) => {
    const d = new Date(t);
    return `${d.getFullYear()}-${p2(d.getMonth() + 1)}-${p2(d.getDate())}T${p2(d.getHours())}:${p2(d.getMinutes())}`;
  };
  const fromInput = (s) => (s ? new Date(s).getTime() : NaN);
  const hours = (ms) => Math.round((ms / 3600000) * 10) / 10;

  /**
   * container 안에 시간표를 그린다.
   * opts: { onView(view) 새 보기 받았을 때, onChat() 대화로 고치기, compact, say(text, bad) }
   */
  function render(container, view, opts = {}) {
    container.textContent = '';
    const say = opts.say || (() => {});
    let busy = false;
    const run = async (fn, okText) => {
      if (busy) return;
      busy = true;
      container.classList.add('busy');
      try {
        const r = await fn();
        const v = r && r.blocks ? r : r && r.view ? r.view : null;
        if (v) {
          render(container, v, opts);
          if (opts.onView) opts.onView(v);
        }
        if (okText) say(typeof okText === 'function' ? okText(r) : okText);
      } catch (e) {
        say(errMsg(e), true);
      } finally {
        busy = false;
        container.classList.remove('busy');
      }
    };

    const card = el('div', { class: `sched ${opts.compact ? 'compact' : ''}` });
    container.append(card);

    // 머리
    const synced = view.blocks.length && !view.pending;
    card.append(
      el(
        'div',
        { class: 'sched-head' },
        el('strong', { text: '내 작업 시간표' }),
        el('span', {
          class: `sched-state ${view.pending ? 'pending' : synced ? 'ok' : ''}`,
          text: !view.blocks.length ? '' : view.pending ? `캘린더 반영 전 ${view.pending}건` : '캘린더와 같아요',
        }),
      ),
    );

    if (!view.tasks.length) {
      card.append(el('p', { class: 'hint', text: '맡은 작업이 생기면 캘린더의 빈 시간을 찾아 "몇 시부터 몇 시까지" 할지 짜 드려요. 협업 탭에서 배정된 작업에 "맡을게요"를 눌러 주세요.' }));
      return;
    }
    if (!view.google.ready) {
      card.append(
        el(
          'p',
          { class: 'hint warn' },
          'Google 캘린더에 로그인하지 않아 다른 일정은 피하지 못했어요. ',
          el('button', { class: 'link', text: '설정 → 외부 연결', onclick: () => document.querySelector('.tabs [data-tab="settings"]').click() }),
        ),
      );
    }
    for (const w of view.warnings.filter((x) => x.kind !== 'calendar')) card.append(el('p', { class: 'hint warn', text: w.text }));

    // 블록 (날짜별)
    const list = el('div', { class: 'sched-list' });
    let lastDay = '';
    for (const b of view.blocks) {
      const dl = dayLabel(b.start);
      if (dl !== lastDay) {
        list.append(el('div', { class: 'sched-day', text: dl }));
        lastDay = dl;
      }
      const row = el('div', { class: `sched-row ${b.late ? 'late' : ''} ${b.running ? 'running' : ''}` });
      const tags = [b.pinned && '직접 정함', b.late && '마감 뒤', b.running && '지금', !b.synced && '반영 전'].filter(Boolean);
      row.append(
        el('span', { class: 'sched-no', text: `${b.no}` }),
        el('span', { class: 'sched-time', text: `${hm(b.start)}–${hm(b.end)}` }),
        el('span', { class: 'sched-title', text: b.title }),
        el('span', { class: 'sched-tags' }, tags.map((t) => el('span', { class: `tag ${t === '반영 전' ? 'soft' : ''}`, text: t }))),
      );
      if (!opts.compact) {
        const editBox = el('div', { class: 'sched-edit hidden' });
        const s = el('input', { type: 'datetime-local', value: localInput(b.start), step: 1800 });
        const e = el('input', { type: 'datetime-local', value: localInput(b.end), step: 1800 });
        editBox.append(
          s,
          el('span', { text: '~' }),
          e,
          el('button', {
            class: 'primary',
            text: '저장',
            onclick: () => run(() => api.schedEdit({ type: 'move', ref: b.id, start: fromInput(s.value), end: fromInput(e.value) }), '옮기고 고정했어요. 나머지는 다시 짰어요.'),
          }),
        );
        row.append(
          el(
            'span',
            { class: 'sched-actions' },
            el('button', { class: 'link', text: '시간 바꾸기', onclick: () => editBox.classList.toggle('hidden') }),
            el('button', { class: 'link', text: '빼기', title: '이 시간은 비우고 다른 빈 시간에 다시 잡아요', onclick: () => run(() => api.schedEdit({ type: 'remove', ref: b.id }), '그 시간은 비우고 다른 시간에 다시 잡았어요.') }),
          ),
          editBox,
        );
      }
      list.append(row);
    }
    if (!view.blocks.length) list.append(el('p', { class: 'hint', text: '잡힌 시간이 없어요. 위의 알림을 확인해 주세요.' }));
    card.append(list);

    // 버튼
    const actions = el('div', { class: 'inline sched-buttons' });
    actions.append(
      el('button', {
        class: 'primary',
        text: view.pending ? `캘린더·할 일에 넣기 (${view.pending})` : '캘린더·할 일 다시 맞추기',
        disabled: !view.blocks.length && !view.pending,
        onclick: () => run(() => api.schedApply(), (r) => r.text),
      }),
      el('button', { text: '다시 짜기', onclick: () => run(() => api.schedReplan(), '캘린더를 다시 읽고 짰어요.') }),
    );
    if (opts.onChat) actions.append(el('button', { text: '대화로 고치기', onclick: opts.onChat }));
    card.append(actions);
    if (opts.compact) return;

    // 자세히: 작업별 시간, 안 되는 시간, 작업 시간대
    const more = el('details', { class: 'sched-more' }, el('summary', { text: '작업 시간 · 안 되는 시간 · 작업 시간대' }));
    const tasks = el('div', { class: 'sched-sub' }, el('b', { text: '작업별 필요 시간' }));
    for (const t of view.tasks) {
      const inp = el('input', { type: 'number', min: 0, max: 200, step: 0.5, value: t.need, class: 'num' });
      tasks.append(
        el(
          'div',
          { class: 'inline' },
          el('span', { class: 'grow', text: `${t.title}${t.project ? ` · ${t.project}` : ''}${t.deadline ? ` · 마감 ${new Date(t.deadline).getMonth() + 1}/${new Date(t.deadline).getDate()}` : ''}` }),
          inp,
          el('span', { text: `시간 (잡힘 ${t.placed})` }),
          el('button', { text: '바꾸기', onclick: () => run(() => api.schedEdit({ type: 'hours', taskId: t.taskId, hours: Number(inp.value) }), '필요 시간을 바꾸고 다시 짰어요.') }),
        ),
      );
    }
    const un = el('div', { class: 'sched-sub' }, el('b', { text: '안 되는 시간' }));
    for (const u of view.unavailable) {
      un.append(
        el(
          'div',
          { class: 'inline' },
          el('span', { class: 'grow', text: `${u.label}${u.note ? ` · ${u.note}` : ''}` }),
          el('button', { class: 'link', text: '지우기', onclick: () => run(() => api.schedEdit({ type: 'remove-unavailable', id: u.id }), '지우고 다시 짰어요.') }),
        ),
      );
    }
    const now = Date.now();
    const us = el('input', { type: 'datetime-local', value: localInput(Math.ceil(now / 3600000) * 3600000), step: 1800 });
    const ue = el('input', { type: 'datetime-local', value: localInput(Math.ceil(now / 3600000) * 3600000 + 2 * 3600000), step: 1800 });
    const un2 = el('input', { type: 'text', placeholder: '메모 (예: 수업)', maxlength: 40 });
    un.append(
      el(
        'div',
        { class: 'inline' },
        us,
        el('span', { text: '~' }),
        ue,
        un2,
        el('button', { text: '추가', onclick: () => run(() => api.schedEdit({ type: 'unavailable', start: fromInput(us.value), end: fromInput(ue.value), note: un2.value }), '그 시간은 비우고 다시 짰어요.') }),
      ),
    );
    const p = view.prefs;
    const f = {
      dayStart: el('input', { type: 'number', min: 0, max: 23, value: p.dayStart, class: 'num' }),
      dayEnd: el('input', { type: 'number', min: 1, max: 24, value: p.dayEnd, class: 'num' }),
      dailyMaxHours: el('input', { type: 'number', min: 0.5, max: 16, step: 0.5, value: p.dailyMaxHours, class: 'num' }),
      maxBlockHours: el('input', { type: 'number', min: 0.5, max: 8, step: 0.5, value: p.maxBlockHours, class: 'num' }),
    };
    const pr = el(
      'div',
      { class: 'sched-sub' },
      el('b', { text: '작업 시간대' }),
      el(
        'div',
        { class: 'inline wrap' },
        f.dayStart,
        el('span', { text: '시 ~' }),
        f.dayEnd,
        el('span', { text: '시, 하루 최대' }),
        f.dailyMaxHours,
        el('span', { text: '시간, 한 번에 최대' }),
        f.maxBlockHours,
        el('span', { text: '시간' }),
        el('button', {
          text: '저장',
          onclick: () =>
            run(
              () => api.schedEdit({ type: 'prefs', patch: Object.fromEntries(Object.entries(f).map(([k, i]) => [k, Number(i.value)])) }),
              '작업 시간대를 바꾸고 다시 짰어요.',
            ),
        }),
      ),
      el('button', { class: 'link', text: '직접 정한 시간·뺀 시간 모두 풀고 처음부터', onclick: () => confirm('직접 옮기거나 뺀 시간을 모두 풀고 다시 짤까요?') && run(() => api.schedEdit({ type: 'reset' }), '처음부터 다시 짰어요.') }),
    );
    more.append(tasks, un, pr);
    card.append(more);
  }

  window.SaiSchedule = { render, hours };
})();
