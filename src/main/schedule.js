// 내 작업 시간표를 앱에 붙인다: 맡은 작업 모으기, Google 캘린더 바쁜 시간 읽기, 캘린더·할 일 반영,
// 그리고 대화로 고치기 (Claude API 키가 있으면 AI가 도구로 고치고, 없으면 규칙으로 읽는다).
const { ScheduleBook } = require('../core/schedule/schedule-book');
const { parseCommand } = require('../core/schedule/commands');
const { parseDate } = require('../core/collab/planner');
const { fmtRange, parseLocalIso, toLocalIso, WEEKDAYS } = require('../core/schedule/korean-time');
const { callClaudeMessages } = require('./ai');

const MAX_TURNS = 30;

function createScheduleService({ stores, team, ai }) {
  const google = team.google;
  const collabs = () => team.hub.all().map((b) => b.collab).filter((c) => c.team.isConnected());

  function collectItems() {
    const out = [];
    let order = 0;
    for (const c of collabs()) {
      const me = c.myUid();
      const teamInfo = c.team.team() || {};
      for (const p of c.data().projects) {
        const tasks = c.tasksOf(p.id);
        const mine = tasks.filter((t) => t.assignee === me && t.accepted && t.status !== 'done');
        if (!mine.length) continue;
        let plan = null;
        try {
          plan = c.plan(p.id);
        } catch {
          /* 배분 계산이 실패해도 시간표는 예상 시간으로 짠다 */
        }
        const asg = new Map(((plan && plan.assignments) || []).map((a) => [a.taskId, a]));
        const byId = new Map(tasks.map((t) => [t.id, t]));
        for (const t of mine) {
          const a = asg.get(t.id);
          const openDeps = (t.dependsOn || []).filter((d) => byId.has(d) && byId.get(d).status !== 'done');
          // 다른 사람이 맡은 선행 작업이 남아 있으면, 팀 일정 추정상 시작 가능한 때부터
          const othersDeps = openDeps.filter((d) => byId.get(d).assignee !== me || !byId.get(d).accepted);
          out.push({
            taskId: t.id,
            projectId: p.id,
            projectTitle: p.title,
            teamId: teamInfo.teamId,
            title: t.title,
            hours: (a && a.effectiveHours) || Number(t.estimateHours) || 1,
            deadline: parseDate(p.deadline),
            deps: openDeps,
            after: othersDeps.length && a && a.startAt ? a.startAt : null,
            order: order++,
          });
        }
      }
    }
    return out;
  }

  const book = new ScheduleBook({
    store: stores.schedule,
    getItems: collectItems,
    getBusy: (r) => google.busyBetween(r),
  });

  const sig = () =>
    JSON.stringify(collectItems().map((i) => [i.taskId, i.title, i.hours, i.deadline, i.deps]));

  /** 맡은 작업이 바뀌었으면 다시 짠다 */
  async function view({ refresh = false } = {}) {
    const st = book.state();
    const s = sig();
    if (refresh || st.sig !== s || !st.lastPlanAt) {
      await book.replan({ fresh: refresh });
      book.update((x) => (x.sig = s));
    }
    return decorate(book.view());
  }

  function decorate(v) {
    const g = google.status();
    return {
      ...v,
      google: { ready: google.isReady(), configured: g.configured, email: g.email },
      ai: !!ai.config().apiKey,
      chat: (book.state().chat || []).slice(-MAX_TURNS),
    };
  }

  async function replan() {
    await book.replan({ fresh: true });
    book.update((x) => (x.sig = sig()));
    return decorate(book.view());
  }

  async function apply() {
    if (!google.isReady()) throw new Error('먼저 설정 → 외부 연결에서 Google에 로그인해 주세요.');
    const r = await book.apply(google.calendarOps);
    let todo = null;
    try {
      todo = await google.syncTodo();
    } catch (e) {
      todo = `할 일 동기화 실패: ${e.message || e}`;
    }
    const out = { result: r, todo, view: decorate(book.view()) };
    out.text = applyText(out);
    return out;
  }

  const applyText = (r) => {
    const x = r.result;
    if (!x.created && !x.patched && !x.removed && !x.failed) return '캘린더는 이미 지금 시간표와 같아요.';
    const parts = [];
    if (x.created) parts.push(`${x.created}개 추가`);
    if (x.patched) parts.push(`${x.patched}개 시간 변경`);
    if (x.removed) parts.push(`${x.removed}개 삭제`);
    let t = `캘린더에 반영했어요 (${parts.join(', ')}).`;
    if (x.failed) t += ` ${x.failed}개는 실패했어요: ${x.errors[0]}`;
    if (r.todo) t += ` Google 할 일: ${r.todo}.`;
    return t;
  };

  // ---------- 직접 고치기 (화면 버튼) ----------
  async function edit(op) {
    switch (op.type) {
      case 'move':
        await book.moveBlock(op.ref, op.start, op.end);
        break;
      case 'remove':
        await book.removeBlock(op.ref);
        break;
      case 'hours':
        await book.setHours(op.taskId, op.hours);
        break;
      case 'unavailable':
        await book.addUnavailable(op.start, op.end, op.note);
        break;
      case 'remove-unavailable':
        await book.removeUnavailable(op.id);
        break;
      case 'prefs':
        await book.setPrefs(op.patch);
        break;
      case 'reset':
        await book.reset();
        break;
      default:
        throw new Error('알 수 없는 수정이에요.');
    }
    return decorate(book.view());
  }

  // ---------- 대화로 고치기 ----------
  function remember(role, text) {
    book.update((s) => {
      s.chat = [...(s.chat || []), { role, text, at: Date.now() }].slice(-MAX_TURNS);
    });
  }

  async function chat(text) {
    text = String(text || '').trim().slice(0, 20000);
    if (!text) throw new Error('하실 말을 적어 주세요.');
    await view();
    remember('user', text);
    let reply = null;
    let via = 'rules';
    const cfg = ai.config();
    if (cfg.apiKey) {
      try {
        reply = await aiChat(text, cfg);
        via = 'ai';
      } catch (e) {
        ai.lastError = String(e.message || e);
        reply = null;
      }
    }
    if (!reply) {
      const before = new Set((book.state().warnings || []).map((w) => w.text));
      reply = await ruleChat(text, !!cfg.apiKey);
      const fresh = (book.state().warnings || []).filter((w) => !before.has(w.text) && w.kind !== 'calendar');
      if (fresh.length) reply += ` 다만 ${fresh.map((w) => w.text).join(' ')}`;
    }
    remember('cat', reply);
    return { reply, via, view: decorate(book.view()) };
  }

  async function ruleChat(text, aiFailed) {
    const v = book.view();
    const [cmd] = parseCommand(text, { blocks: v.blocks, tasks: v.tasks, prefs: v.prefs, now: Date.now() });
    try {
      switch (cmd.type) {
        case 'apply':
          return applyText(await apply());
        case 'replan':
          await replan();
          return `다시 짰어요. ${book.view().blocks.length}개 시간으로 나눴어요.`;
        case 'reset':
          await book.reset();
          return '직접 정한 시간과 빼 둔 시간을 풀고 처음부터 다시 짰어요.';
        case 'show':
          return book.view().blocks.length ? '지금 시간표는 아래와 같아요.' : '아직 잡힌 시간이 없어요. 협업 탭에서 작업을 맡으면 시간표를 짜 드려요.';
        case 'move': {
          const b = await book.moveBlock(cmd.ref, cmd.start, cmd.end);
          return `${b.title}을 ${fmtRange(b.start, b.end)}로 옮기고 고정했어요. 겹치는 일정은 다른 시간으로 옮겼어요.`;
        }
        case 'remove': {
          const b = await book.removeBlock(cmd.ref);
          return `${fmtRange(b.start, b.end)} ${b.title}은 비워 두고, 그만큼 다른 빈 시간에 다시 잡았어요. 작업 시간 자체를 줄이려면 "${b.title} N시간이면 돼"라고 말해 주세요.`;
        }
        case 'add':
          await book.addBlock(cmd.taskId, cmd.start, cmd.end);
          return `${fmtRange(cmd.start, cmd.end)}에 넣었어요. 전체 시간은 그대로라 다른 날 시간이 그만큼 줄었어요.`;
        case 'hours': {
          await book.setHours(cmd.taskId, cmd.hours);
          const t = book.view().tasks.find((x) => x.taskId === cmd.taskId);
          return `${t ? t.title : '그 작업'}을 ${round1(cmd.hours)}시간으로 바꾸고 다시 짰어요.`;
        }
        case 'unavailable':
          await book.addUnavailable(cmd.start, cmd.end, cmd.note);
          return `${fmtRange(cmd.start, cmd.end)}은 비워 두고 다시 짰어요.`;
        case 'prefs': {
          await book.setPrefs(cmd.patch);
          const p = book.view().prefs;
          return `작업 시간대를 ${p.dayStart}시~${p.dayEnd}시, 하루 최대 ${p.dailyMaxHours}시간, 한 번에 최대 ${p.maxBlockHours}시간으로 맞춰 다시 짰어요.`;
        }
        default: {
          const help =
            '이렇게 말해 주시면 고칠게요: "2번 목요일 3시로 옮겨줘", "1번 빼줘", "수요일은 안 돼", "내일 2시부터 5시까지 수업 있어", "자료조사 3시간이면 돼", "하루에 4시간만", "캘린더에 넣어줘".';
          const tip = aiFailed ? ' (AI 연결이 안 돼서 정해진 표현만 알아들을 수 있어요.)' : ai.config().apiKey ? '' : ' 설정에서 Claude API 키를 넣으면 더 자유롭게 말해도 알아들어요.';
          return `${cmd.hint ? cmd.hint + ' ' : '무엇을 바꿀지 잘 모르겠어요. '}${cmd.hint ? '' : help}${tip}`;
        }
      }
    } catch (e) {
      return `고치지 못했어요: ${e.message || e}`;
    }
  }

  // ---------- AI (도구 사용) ----------
  const TOOLS = [
    {
      name: 'move_block',
      description: '시간표의 블록 하나를 다른 시간으로 옮긴다. 옮긴 블록은 고정되고 나머지는 자동으로 다시 배치된다. end 를 생략하면 길이를 유지한다.',
      input_schema: { type: 'object', properties: { no: { type: 'integer', description: '블록 번호' }, start: { type: 'string', description: 'YYYY-MM-DDTHH:MM (현지 시각)' }, end: { type: 'string' } }, required: ['no', 'start'] },
    },
    {
      name: 'remove_block',
      description: '블록 하나를 뺀다. 그 시간은 비워 두고, 작업 시간은 다른 빈 시간에 다시 잡힌다. 작업 시간 자체를 줄이려면 set_task_hours 를 쓴다.',
      input_schema: { type: 'object', properties: { no: { type: 'integer' } }, required: ['no'] },
    },
    {
      name: 'add_block',
      description: '특정 작업을 특정 시간에 하도록 고정 블록을 추가한다. 전체 필요 시간은 그대로라 다른 블록이 줄어든다.',
      input_schema: { type: 'object', properties: { task_id: { type: 'string' }, start: { type: 'string' }, end: { type: 'string' } }, required: ['task_id', 'start', 'end'] },
    },
    {
      name: 'set_task_hours',
      description: '작업에 필요한 전체 시간(시간 단위)을 바꾸고 다시 배치한다.',
      input_schema: { type: 'object', properties: { task_id: { type: 'string' }, hours: { type: 'number' } }, required: ['task_id', 'hours'] },
    },
    {
      name: 'add_unavailable',
      description: '사용자가 작업할 수 없는 시간(약속, 수업 등)을 추가하고 다시 배치한다.',
      input_schema: { type: 'object', properties: { start: { type: 'string' }, end: { type: 'string' }, note: { type: 'string' } }, required: ['start', 'end'] },
    },
    {
      name: 'remove_unavailable',
      description: '안 되는 시간 하나를 지운다.',
      input_schema: { type: 'object', properties: { id: { type: 'string' } }, required: ['id'] },
    },
    {
      name: 'set_prefs',
      description: '작업 시간대(시작·끝 시각, 0~24), 하루 최대 작업 시간, 한 번에 최대 작업 시간을 바꾸고 다시 배치한다.',
      input_schema: { type: 'object', properties: { day_start: { type: 'number' }, day_end: { type: 'number' }, daily_max_hours: { type: 'number' }, max_block_hours: { type: 'number' } } },
    },
    { name: 'replan', description: '고정한 블록은 두고 나머지를 처음부터 다시 배치한다.', input_schema: { type: 'object', properties: {} } },
    {
      name: 'add_task',
      description: '새 작업을 팀 프로젝트에 추가하고 사용자가 맡은 것으로 한다 (팀원 모두에게 보인다). 사용자가 새 할 일을 추가해 달라고 할 때만 쓴다.',
      input_schema: {
        type: 'object',
        properties: { project_id: { type: 'string' }, title: { type: 'string' }, hours: { type: 'number' }, done_criteria: { type: 'string' } },
        required: ['project_id', 'title', 'hours'],
      },
    },
    {
      name: 'rename_task',
      description: '작업 이름을 바꾼다 (팀원 모두에게 보인다).',
      input_schema: { type: 'object', properties: { task_id: { type: 'string' }, title: { type: 'string' } }, required: ['task_id', 'title'] },
    },
    {
      name: 'complete_task',
      description: '작업을 완료로 표시한다. 남은 블록은 시간표에서 빠진다.',
      input_schema: { type: 'object', properties: { task_id: { type: 'string' } }, required: ['task_id'] },
    },
    {
      name: 'apply_to_calendar',
      description: '지금 시간표를 Google 캘린더 일정과 Google 할 일에 반영한다. 사용자가 반영·넣기·확정을 요청했을 때만 쓴다.',
      input_schema: { type: 'object', properties: {} },
    },
  ];

  function projectsList() {
    const out = [];
    for (const c of collabs()) {
      const t = c.team.team() || {};
      for (const p of c.data().projects) out.push({ id: p.id, title: p.title, team: t.name || '', deadline: p.deadline || '' });
    }
    return out;
  }

  function contextText() {
    const v = book.view();
    const now = new Date();
    const lines = [];
    lines.push(`지금: ${toLocalIso(now)} (${WEEKDAYS[now.getDay()]}요일)`);
    lines.push(`작업 시간대: ${v.prefs.dayStart}~${v.prefs.dayEnd}시, 하루 최대 ${v.prefs.dailyMaxHours}시간, 한 번에 최대 ${v.prefs.maxBlockHours}시간`);
    lines.push(`Google 캘린더: ${google.isReady() ? '연결됨(바쁜 시간은 피해서 배치됨)' : '연결 안 됨'}`);
    lines.push('\n내가 맡은 작업:');
    for (const t of v.tasks) lines.push(`- task_id=${t.taskId} "${t.title}" [${t.project}] 필요 ${t.need}시간, 배치 ${t.placed}시간${t.deadline ? `, 마감 ${toLocalIso(t.deadline).slice(0, 10)}` : ''}`);
    if (!v.tasks.length) lines.push('(없음)');
    lines.push('\n시간표 (번호. 시작 ~ 끝 작업):');
    for (const b of v.blocks) lines.push(`${b.no}. ${toLocalIso(b.start)} ~ ${toLocalIso(b.end).slice(11)} ${b.title}${b.pinned ? ' (고정)' : ''}${b.late ? ' (마감 뒤)' : ''}${b.synced ? '' : ' (캘린더 미반영)'}`);
    if (!v.blocks.length) lines.push('(없음)');
    if (v.unavailable.length) {
      lines.push('\n안 되는 시간:');
      for (const u of v.unavailable) lines.push(`- id=${u.id} ${toLocalIso(u.start)} ~ ${toLocalIso(u.end)} ${u.note || ''}`);
    }
    if (v.warnings.length) lines.push(`\n알림: ${v.warnings.map((w) => w.text).join(' / ')}`);
    const ps = projectsList();
    if (ps.length) lines.push(`\n프로젝트: ${ps.map((p) => `project_id=${p.id} "${p.title}"(${p.team})`).join(', ')}`);
    return lines.join('\n');
  }

  const findCollabFor = (taskId) => collabs().find((c) => c.findTask(taskId));
  const findCollabForProject = (projectId) => collabs().find((c) => c.data().projects.some((p) => p.id === projectId));

  async function runTool(name, input) {
    const t = (s) => {
      const ms = parseLocalIso(s);
      if (ms == null) throw new Error(`시각 형식이 맞지 않아요: ${s}`);
      return ms;
    };
    switch (name) {
      case 'move_block': {
        const start = t(input.start);
        const b = await book.moveBlock(input.no, start, input.end ? t(input.end) : 0);
        return `옮김: ${fmtRange(b.start, b.end)} ${b.title}`;
      }
      case 'remove_block': {
        const b = await book.removeBlock(input.no);
        return `뺌: ${fmtRange(b.start, b.end)} ${b.title} (그 시간은 비워 둠)`;
      }
      case 'add_block':
        await book.addBlock(input.task_id, t(input.start), t(input.end));
        return '추가함';
      case 'set_task_hours':
        await book.setHours(input.task_id, input.hours);
        return `필요 시간 ${input.hours}시간으로 바꿈`;
      case 'add_unavailable':
        await book.addUnavailable(t(input.start), t(input.end), input.note || '');
        return '안 되는 시간 추가함';
      case 'remove_unavailable':
        await book.removeUnavailable(input.id);
        return '안 되는 시간 지움';
      case 'set_prefs': {
        const patch = {};
        if (input.day_start != null) patch.dayStart = input.day_start;
        if (input.day_end != null) patch.dayEnd = input.day_end;
        if (input.daily_max_hours != null) patch.dailyMaxHours = input.daily_max_hours;
        if (input.max_block_hours != null) patch.maxBlockHours = input.max_block_hours;
        await book.setPrefs(patch);
        return '설정 바꿈';
      }
      case 'replan':
        await replan();
        return '다시 배치함';
      case 'add_task': {
        const c = findCollabForProject(input.project_id);
        if (!c) throw new Error('그 프로젝트를 찾지 못했어요.');
        const id = await c.addTask(input.project_id, { title: input.title, estimateHours: input.hours, doneCriteria: input.done_criteria || '' });
        await c.patchTask(input.project_id, id, { assignee: c.myUid(), accepted: true });
        await replan();
        return `작업 추가함 (task_id=${id})`;
      }
      case 'rename_task': {
        const c = findCollabFor(input.task_id);
        if (!c) throw new Error('그 작업을 찾지 못했어요.');
        const f = c.findTask(input.task_id);
        await c.patchTask(f.projectId, input.task_id, { title: String(input.title).slice(0, 80) });
        await replan();
        return '이름 바꿈';
      }
      case 'complete_task': {
        const c = findCollabFor(input.task_id);
        if (!c) throw new Error('그 작업을 찾지 못했어요.');
        await c.setStatus(input.task_id, 'done');
        await replan();
        return '완료로 표시함';
      }
      case 'apply_to_calendar':
        return applyText(await apply());
      default:
        throw new Error(`알 수 없는 도구: ${name}`);
    }
  }

  async function aiChat(text, cfg) {
    const system =
      `당신은 사용자의 바탕화면에 사는 고양이 "${cfg.name || '사이'}"이고, 사용자의 작업 시간표를 함께 고칩니다.\n` +
      '- 사용자가 시간이나 할 일을 바꿔 달라고 하면 도구로 직접 고칩니다. 확인을 묻기보다 합리적으로 해석해 바로 고치고, 무엇을 했는지 짧게 말합니다.\n' +
      '- 시각은 현지 시각 YYYY-MM-DDTHH:MM 형식으로 넘깁니다. "목요일", "내일 오후 3시" 같은 말은 아래의 "지금"을 기준으로 계산합니다. 오전/오후가 없으면 낮 시간으로 봅니다.\n' +
      '- 블록 번호는 고칠 때마다 바뀔 수 있으니, 도구 결과에 붙은 최신 시간표를 기준으로 합니다.\n' +
      '- 캘린더 반영(apply_to_calendar)은 사용자가 넣어 달라·반영해 달라·확정한다고 할 때만 합니다. 고친 뒤에는 반영하려면 말해 달라고 안내합니다.\n' +
      '- 할 수 없는 일(다른 사람의 캘린더 수정 등)은 솔직히 말합니다.\n' +
      '- 부드러운 존댓말로 2~4문장, 이모지·마크다운·목록 기호 없이 답합니다. 시간표 전체를 다시 나열하지 않습니다 (화면에 따로 보입니다).';
    const history = (book.state().chat || []).slice(-11, -1).map((m) => ({ role: m.role === 'user' ? 'user' : 'assistant', content: m.text }));
    // 같은 역할이 이어지지 않게 정리
    const msgs = [];
    for (const m of history) {
      if (msgs.length && msgs[msgs.length - 1].role === m.role) msgs[msgs.length - 1].content += `\n${m.content}`;
      else msgs.push({ ...m });
    }
    while (msgs.length && msgs[0].role !== 'user') msgs.shift();
    const userMsg = { role: 'user', content: `[현재 상태]\n${contextText()}\n\n[사용자 말]\n${text}` };
    if (msgs.length && msgs[msgs.length - 1].role === 'user') msgs.pop();
    msgs.push(userMsg);

    for (let i = 0; i < 8; i++) {
      const data = await callClaudeMessages({ apiKey: cfg.apiKey, model: cfg.model, system, messages: msgs, tools: TOOLS });
      const content = data.content || [];
      msgs.push({ role: 'assistant', content });
      const uses = content.filter((b) => b.type === 'tool_use');
      if (!uses.length || data.stop_reason !== 'tool_use') {
        const out = content.filter((b) => b.type === 'text').map((b) => b.text).join('').trim();
        return out || '고쳤어요.';
      }
      const results = [];
      for (const u of uses) {
        let res;
        let isError = false;
        try {
          res = await runTool(u.name, u.input || {});
        } catch (e) {
          res = `실패: ${e.message || e}`;
          isError = true;
        }
        results.push({ type: 'tool_result', tool_use_id: u.id, content: `${res}\n\n[최신 상태]\n${contextText()}`, is_error: isError });
      }
      msgs.push({ role: 'user', content: results });
    }
    return '여러 번 고쳐 봤는데 마무리하지 못했어요. 아래 시간표를 확인해 주세요.';
  }

  function clearChat() {
    book.update((s) => (s.chat = []));
  }

  return { view, replan, apply, edit, chat, clearChat, book };
}

const round1 = (x) => Math.round(x * 10) / 10;

module.exports = { createScheduleService };
