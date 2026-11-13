// 협업 탭: 자기소개 · 프로젝트 · 작업 · 배분 제안 · 진행 상태
(function () {
  const root = document.getElementById('collabRoot');
  const api = window.sai;
  let view = null;
  let planResult = null;
  let focusTaskId = null;
  let editingTaskId = null;
  let busy = false;

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
  // DOM append 는 null 을 "null" 글자로 넣으므로 걸러서 붙인다
  const add = (parent, ...kids) => parent.append(...kids.flat().filter((k) => k != null && k !== false));
  const errMsg = (e) => String((e && e.message) || e).replace(/^Error invoking remote method '[^']+': (Error: )?/, '');
  const LEVELS = [
    [1, '배우는 중'],
    [2, '할 수 있음'],
    [3, '능숙'],
  ];
  const STATUS = { todo: '할 일', doing: '하는 중', blocked: '막힘', done: '완료' };
  const DIFF = { 1: '쉬움', 2: '보통', 3: '어려움' };

  let msgTimer;
  function flash(text, bad = false) {
    const m = document.getElementById('collabMsg');
    if (!m) return;
    m.textContent = text;
    m.classList.toggle('bad', bad);
    clearTimeout(msgTimer);
    msgTimer = setTimeout(() => (m.textContent = ''), 4000);
  }

  async function act(fn, ok) {
    if (busy) return;
    busy = true;
    try {
      const v = await fn();
      if (v && v.connected !== undefined) view = v;
      else if (v && v.view) view = v.view;
      draw();
      if (ok) flash(ok);
    } catch (e) {
      flash(errMsg(e), true);
    } finally {
      busy = false;
    }
  }

  // 저장된 내용을 먼저 보여 주고, 필요하면 서버와 동기화한 뒤 다시 그린다
  async function render(refresh = false) {
    try {
      view = await api.collabView({ refresh: false });
      draw();
      if (refresh && view.connected) {
        view = await api.collabView({ refresh: true });
        draw();
      }
    } catch (e) {
      view = { connected: false, error: errMsg(e) };
      draw();
    }
  }

  function draw() {
    root.textContent = '';
    if (!view || !view.connected) {
      add(root, 
        el(
          'div',
          { class: 'empty-state' },
          el('p', { text: '협업 기능은 팀에 연결한 뒤 쓸 수 있어요.' }),
          el('p', { class: 'hint', text: '설정 → 팀 연결에서 팀을 만들거나 참여 코드로 들어와 주세요.' }),
          el('button', { class: 'primary', text: '설정 열기', onclick: () => document.querySelector('.tabs [data-tab="settings"]').click() }),
        ),
      );
      return;
    }
    add(root, el('div', { id: 'collabMsg', class: 'status collab-msg' }));
    add(root, profileSection(), projectSection());
    if (view.project) add(root, planSection(), taskSection(), taskForm());
    if (focusTaskId) {
      const card = root.querySelector(`[data-task="${focusTaskId}"]`);
      if (card) {
        card.classList.add('focus');
        card.scrollIntoView({ block: 'center' });
      }
      focusTaskId = null;
    }
  }

  // ---------- 자기소개 ----------
  function profileSection() {
    const p = view.myProfile || {};
    const skills = Object.entries(p.skills || {}).map(([name, level]) => ({ name, level }));
    if (!skills.length) skills.push({ name: '', level: 2 });
    const rows = el('div', { class: 'skill-rows' });
    const addRow = (s) => {
      const row = el(
        'div',
        { class: 'inline skill-row' },
        el('input', { type: 'text', class: 'skill-name', value: s.name, placeholder: '역량 (예: 영상편집)', maxlength: 30 }),
        el('select', { class: 'skill-level' }, LEVELS.map(([v, l]) => el('option', { value: v, text: l, selected: Number(s.level) === v }))),
        el('button', { type: 'button', class: 'link', text: '빼기', onclick: () => row.remove() }),
      );
      add(rows, row);
    };
    skills.forEach(addRow);

    const form = el(
      'form',
      {
        class: 'settings',
        onsubmit: (e) => {
          e.preventDefault();
          const f = e.target;
          const sk = [...rows.querySelectorAll('.skill-row')].map((r) => ({ name: r.querySelector('.skill-name').value, level: Number(r.querySelector('.skill-level').value) }));
          act(() => api.collabSaveProfile({ pr: f.pr.value, skills: sk, learn: f.learn.value, weeklyHours: f.weeklyHours.value }), '자기소개를 저장했어요. 팀원 모두 볼 수 있어요.');
        },
      },
      el('label', {}, '자기소개 (할 수 있는 일, 해 본 경험)', el('textarea', { name: 'pr', rows: 3, maxlength: 500, text: p.pr || '' })),
      el('div', { class: 'label', text: '역량 — 본인이 적고 언제든 고칠 수 있어요' }),
      rows,
      el('button', { type: 'button', class: 'link', text: '+ 역량 추가', onclick: () => addRow({ name: '', level: 2 }) }),
      el('label', {}, '배우고 싶은 일 (쉼표로 구분)', el('input', { type: 'text', name: 'learn', value: (p.learn || []).join(', ') })),
      el('label', {}, '이번 주에 이 프로젝트에 쓸 수 있는 시간', el('div', { class: 'inline' }, el('input', { type: 'number', name: 'weeklyHours', min: 0, max: 80, step: 0.5, value: p.weeklyHours ?? '' }), '시간 / 주')),
      el('div', { class: 'inline' }, el('button', { type: 'submit', class: 'primary', text: '자기소개 저장' })),
    );

    const team = el(
      'div',
      { class: 'list' },
      view.members.map((m) =>
        el(
          'div',
          { class: 'item' },
          el(
            'div',
            { class: 'main' },
            el('div', { class: 'title', text: `${m.name}${m.uid === view.myUid ? ' (나)' : ''} · ${m.hasProfile ? `주 ${m.weeklyHours}시간` : '자기소개 아직 없음'}` }),
            m.pr ? el('div', { class: 'sub', text: m.pr, title: m.pr }) : null,
            el(
              'div',
              { class: 'chips' },
              Object.entries(m.skills).map(([k, v]) => el('span', { class: 'chip', text: `${k} · ${LEVELS.find((l) => l[0] === v)?.[1] || ''}` })),
              (m.learn || []).map((k) => el('span', { class: 'chip learn', text: `배우고 싶음: ${k}` })),
            ),
          ),
        ),
      ),
    );

    return el(
      'details',
      { class: 'block', open: !view.myProfile },
      el('summary', {}, el('strong', { text: '자기소개 · 역량 · 가능 시간' }), view.myProfile ? null : el('span', { class: 'status-pill review', text: '입력 필요' })),
      form,
      el('div', { class: 'label', text: '팀원' }),
      team,
    );
  }

  // ---------- 프로젝트 ----------
  function projectSection() {
    const p = view.project;
    const sel = el(
      'select',
      { onchange: (e) => act(() => api.collabSetActive({ id: e.target.value })) },
      view.projects.map((x) => el('option', { value: x.id, text: x.title, selected: p && x.id === p.id })),
    );
    const newForm = el(
      'form',
      {
        class: 'settings',
        onsubmit: (e) => {
          e.preventDefault();
          const f = e.target;
          act(() => api.collabCreateProject({ title: f.title.value, goal: f.goal.value, deliverable: f.deliverable.value, deadline: f.deadline.value }), '프로젝트를 만들었어요.');
        },
      },
      el('label', {}, '프로젝트 이름', el('input', { type: 'text', name: 'title', required: true, maxlength: 80 })),
      el('label', {}, '목표', el('input', { type: 'text', name: 'goal', maxlength: 300 })),
      el('label', {}, '결과물', el('input', { type: 'text', name: 'deliverable', maxlength: 200 })),
      el('label', {}, '마감일', el('input', { type: 'date', name: 'deadline' })),
      el('button', { type: 'submit', class: 'primary', text: '만들기' }),
    );
    const box = el('div', { class: 'block' });
    if (!view.projects.length) {
      add(box, el('strong', { text: '프로젝트' }), el('p', { class: 'hint', text: '아직 프로젝트가 없어요. 목표와 마감일부터 적어 주세요.' }), newForm);
      return box;
    }
    const dl = p && p.deadline ? Math.ceil((new Date(p.deadline + 'T23:59') - Date.now()) / 86400000) : null;
    add(box, 
      el('div', { class: 'inline' }, el('strong', { text: '프로젝트' }), sel),
      p
        ? el(
            'div',
            { class: 'project-meta' },
            p.goal ? el('div', { text: `목표: ${p.goal}` }) : null,
            p.deliverable ? el('div', { text: `결과물: ${p.deliverable}` }) : null,
            el('div', { text: p.deadline ? `마감: ${p.deadline} (${dl >= 0 ? `${dl}일 남음` : `${-dl}일 지남`})` : '마감일 없음' }),
          )
        : null,
      el('details', {}, el('summary', { text: '새 프로젝트 만들기' }), newForm),
    );
    return box;
  }

  // ---------- 배분 제안 ----------
  function planSection() {
    const box = el('div', { class: 'block plan' });
    add(box, 
      el(
        'div',
        { class: 'inline' },
        el('strong', { text: '배분 제안' }),
        el('button', {
          class: 'primary',
          text: planResult ? '다시 계산' : '배분 제안 받기',
          onclick: async () => {
            try {
              planResult = await api.collabPlan({ projectId: view.project.id });
              draw();
            } catch (e) {
              flash(errMsg(e), true);
            }
          },
        }),
      ),
      el('p', { class: 'hint', text: '같은 개수가 아니라 가능 시간·역량·어려운 일 쏠림·선행 작업·검토 여유를 함께 봐요. 적용해도 각자 수락하거나 이의를 제기할 수 있어요.' }),
    );
    if (!planResult) return box;
    const r = planResult;
    if (r.missingProfiles && r.missingProfiles.length) add(box, el('div', { class: 'warn', text: `자기소개가 없는 팀원: ${r.missingProfiles.join(', ')} — 가능 시간이 0으로 계산돼요.` }));
    for (const w of r.warnings) add(box, el('div', { class: 'warn', text: w.text }));
    add(box, 
      el(
        'div',
        { class: 'loads' },
        r.members.map((m) => {
          const pct = m.ratio == null ? 0 : Math.min(150, Math.round(m.ratio * 100));
          return el(
            'div',
            { class: 'load-row' },
            el('span', { class: 'load-name', text: m.name }),
            el('div', { class: 'bar' }, el('div', { class: `fill ${m.ratio > 1 ? 'over' : ''}`, style: `width:${Math.min(100, pct)}%` })),
            el('span', { class: 'load-num', text: m.usable ? `${m.load}/${m.usable}h` : '시간 없음' }),
          );
        }),
      ),
      el('div', { class: 'hint', text: `마감까지 약 ${r.weeks}주 · 막대는 검토·수정 몫 20%를 뺀 시간 대비 부담` }),
    );
    const rows = el(
      'div',
      { class: 'list' },
      r.assignments.map((a) =>
        el(
          'details',
          { class: 'item plan-row' },
          el('summary', {}, el('span', { class: 'title', text: a.title }), ' → ', el('b', { text: a.name || '배정 못 함' }), a.fixed ? el('span', { class: 'status-pill sent', text: '수락됨' }) : null),
          el('ul', {}, a.reasons.map((x) => el('li', { text: x }))),
        ),
      ),
    );
    add(box, rows, el('p', { class: 'hint', text: r.note }));
    add(box, 
      el('button', {
        class: 'primary',
        text: '이대로 배정 제안하기',
        onclick: () =>
          act(async () => {
            const res = await api.collabApplyPlan({ projectId: view.project.id, assignments: r.assignments });
            planResult = null;
            flash(`${res.applied}개 작업을 제안했어요. 각자 수락하거나 이의를 제기할 수 있어요.`);
            return res.view;
          }),
      }),
    );
    return box;
  }

  // ---------- 작업 ----------
  function taskSection() {
    const box = el('div', { class: 'block' }, el('strong', { text: '작업' }));
    const tasks = view.tasks;
    if (!tasks.length) add(box, el('p', { class: 'hint', text: '작업을 잘게 나눠 적어 주세요. 예상 시간, 필요한 역량, 먼저 끝나야 하는 작업, 완료 기준이 있으면 배분이 정확해져요.' }));
    const titleOf = new Map(tasks.map((t) => [t.id, t.title]));
    const order = { blocked: 0, doing: 1, todo: 2, done: 3 };
    for (const t of [...tasks].sort((a, b) => order[a.status] - order[b.status])) {
      const mine = t.assignee === view.myUid;
      const card = el('div', { class: `task ${t.status}`, 'data-task': t.id });
      add(card, 
        el(
          'div',
          { class: 'task-head' },
          el('b', { text: t.title }),
          el(
            'select',
            { class: `status-select ${t.status}`, onchange: (e) => act(() => api.collabStatus({ id: t.id, status: e.target.value })) },
            Object.entries(STATUS).map(([k, v]) => el('option', { value: k, text: v, selected: t.status === k })),
          ),
        ),
        el(
          'div',
          { class: 'task-meta' },
          `${t.estimateHours}시간 · ${DIFF[t.difficulty] || '보통'}`,
          t.assigneeName ? ` · 담당 ${t.assigneeName}${mine ? '(나)' : ''}${t.accepted ? '' : ' — 확인 기다림'}` : ' · 미배정',
        ),
        t.skills && t.skills.length ? el('div', { class: 'chips' }, t.skills.map((s) => el('span', { class: 'chip', text: s }))) : null,
        t.dependsOn && t.dependsOn.length ? el('div', { class: 'hint', text: `먼저 끝나야 할 작업: ${t.dependsOn.map((d) => titleOf.get(d) || '삭제된 작업').join(', ')}` }) : null,
        t.doneCriteria ? el('div', { class: 'hint', text: `완료 기준: ${t.doneCriteria}` }) : null,
        t.prep && t.prep.length
          ? el(
              'div',
              { class: 'prep' },
              t.prep.map((p, i) =>
                el('label', { class: 'check' }, el('input', { type: 'checkbox', checked: p.done, onchange: () => act(() => api.collabTogglePrep({ id: t.id, index: i })) }), ` ${p.text}`),
              ),
            )
          : null,
        t.objectionNames && t.objectionNames.length ? el('div', { class: 'hint', text: `이의 제기: ${t.objectionNames.join(', ')} — 다음 배분 제안에서 빠져요` }) : null,
      );
      const actions = el('div', { class: 'inline task-actions' });
      if (mine && !t.accepted && t.status !== 'done') {
        const reason = el('select', {}, view.objectionReasons.map((r) => el('option', { value: r, text: r })));
        actions.append(
          el('button', { class: 'primary', text: '맡을게요', onclick: () => act(() => api.collabAccept({ id: t.id }), '수락했어요.') }),
          reason,
          el('button', { text: '이의 있어요', onclick: () => act(() => api.collabObject({ id: t.id, reason: reason.value }), '이의를 남겼어요. 다시 배분할 때 반영돼요.') }),
        );
      } else if (mine && t.accepted && t.status !== 'done') {
        const reason = el('select', {}, view.objectionReasons.map((r) => el('option', { value: r, text: r })));
        actions.append(
          el('details', { class: 'inline-details' }, el('summary', { text: '못 하게 됐어요' }), reason, el('button', { text: '배정 풀기', onclick: () => act(() => api.collabObject({ id: t.id, reason: reason.value }), '배정을 풀었어요. 팀이 다시 나눌 수 있어요.') })),
        );
      }
      actions.append(
        el('button', { class: 'link', text: '고치기', onclick: () => ((editingTaskId = t.id), draw(), document.getElementById('taskForm').scrollIntoView()) }),
        el('button', {
          class: 'link',
          text: '삭제',
          onclick: () => confirm(`"${t.title}" 작업을 지울까요?`) && act(() => api.collabDeleteTask({ projectId: view.project.id, id: t.id }), '지웠어요.'),
        }),
      );
      add(card, actions);
      add(box, card);
    }
    return box;
  }

  function taskForm() {
    const t = editingTaskId ? view.tasks.find((x) => x.id === editingTaskId) : null;
    const others = view.tasks.filter((x) => !t || x.id !== t.id);
    const form = el(
      'form',
      {
        id: 'taskForm',
        class: 'settings',
        onsubmit: (e) => {
          e.preventDefault();
          const f = e.target;
          const data = {
            projectId: view.project.id,
            title: f.title.value,
            estimateHours: f.estimateHours.value,
            difficulty: f.difficulty.value,
            skills: f.skills.value,
            dependsOn: [...f.querySelectorAll('input[name="dep"]:checked')].map((x) => x.value),
            doneCriteria: f.doneCriteria.value,
            prep: f.prep.value
              .split(',')
              .map((x) => x.trim())
              .filter(Boolean)
              .map((text) => ({ text, done: !!(t && (t.prep || []).find((p) => p.text === text && p.done)) })),
          };
          if (t) act(() => api.collabEditTask({ ...data, id: t.id }).then((v) => ((editingTaskId = null), v)), '고쳤어요.');
          else act(() => api.collabAddTask(data), '작업을 추가했어요.');
        },
      },
      el('label', {}, '작업 이름', el('input', { type: 'text', name: 'title', required: true, maxlength: 80, value: t ? t.title : '' })),
      el(
        'div',
        { class: 'inline' },
        el('label', {}, '예상 시간', el('input', { type: 'number', name: 'estimateHours', min: 0.5, max: 200, step: 0.5, value: t ? t.estimateHours : 2 })),
        el('label', {}, '난이도', el('select', { name: 'difficulty' }, Object.entries(DIFF).map(([k, v]) => el('option', { value: k, text: v, selected: Number(t ? t.difficulty : 2) === Number(k) })))),
      ),
      el('label', {}, '필요한 역량 (쉼표로 구분)', el('input', { type: 'text', name: 'skills', value: t ? (t.skills || []).join(', ') : '', placeholder: '예: 디자인, 영상편집' })),
      others.length
        ? el(
            'div',
            {},
            el('div', { class: 'label', text: '먼저 끝나야 하는 작업' }),
            el('div', { class: 'deps' }, others.map((o) => el('label', { class: 'check' }, el('input', { type: 'checkbox', name: 'dep', value: o.id, checked: !!(t && (t.dependsOn || []).includes(o.id)) }), ` ${o.title}`))),
          )
        : null,
      el('label', {}, '완료 기준 (어디까지 하면 끝인지)', el('input', { type: 'text', name: 'doneCriteria', maxlength: 200, value: t ? t.doneCriteria || '' : '' })),
      el('label', {}, '준비물·참고자료 (쉼표로 구분)', el('input', { type: 'text', name: 'prep', value: t ? (t.prep || []).map((p) => p.text).join(', ') : '' })),
      el(
        'div',
        { class: 'inline' },
        el('button', { type: 'submit', class: 'primary', text: t ? '고친 내용 저장' : '작업 추가' }),
        t ? el('button', { type: 'button', class: 'link', text: '취소', onclick: () => ((editingTaskId = null), draw()) }) : null,
      ),
    );
    return el('details', { class: 'block', open: !!t || !view.tasks.length }, el('summary', {}, el('strong', { text: t ? `작업 고치기: ${t.title}` : '작업 추가' })), form);
  }

  window.SaiCollab = {
    render,
    focus: (id) => {
      focusTaskId = id;
    },
  };
  api.onFocusTask((id) => {
    focusTaskId = id;
    document.querySelector('.tabs [data-tab="collab"]').click();
  });
})();
