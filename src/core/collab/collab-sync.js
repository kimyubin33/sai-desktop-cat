// 협업(4단계) 데이터를 팀 백엔드와 주고받고, 배분 제안·진행 확인을 계산한다.
// TeamSync 의 팀 연결과 백엔드를 그대로 쓴다.
const { plan } = require('./planner');
const { nextNudge } = require('./progress');

const STATUSES = ['todo', 'doing', 'blocked', 'done'];
const OBJECTION_REASONS = ['가능 시간이 부족해요', '역량이 맞지 않아요', '다른 일과 겹쳐요', '기타'];

const str = (v, max) => String(v ?? '').replace(/\s+/g, ' ').trim().slice(0, max);
const list = (v, maxItems = 10, maxLen = 30) =>
  (Array.isArray(v) ? v : String(v || '').split(','))
    .map((x) => str(x, maxLen))
    .filter(Boolean)
    .slice(0, maxItems);
const num = (v, min, max, dflt) => {
  const n = Number(v);
  return Number.isFinite(n) ? Math.min(max, Math.max(min, n)) : dflt;
};

function cleanProfile(p) {
  const skills = {};
  for (const s of Array.isArray(p.skills) ? p.skills : Object.entries(p.skills || {}).map(([name, level]) => ({ name, level }))) {
    const name = str(s.name, 30);
    if (name) skills[name] = num(s.level, 0, 3, 2);
  }
  return {
    pr: str(p.pr, 500),
    skills,
    learn: list(p.learn),
    weeklyHours: num(p.weeklyHours, 0, 80, 0),
    days: list(p.days, 7, 3),
  };
}

function cleanTask(t) {
  return {
    title: str(t.title, 80) || '이름 없는 작업',
    estimateHours: num(t.estimateHours, 0, 200, 1),
    difficulty: num(t.difficulty, 1, 3, 2),
    skills: list(t.skills),
    dependsOn: (Array.isArray(t.dependsOn) ? t.dependsOn : []).map((x) => str(x, 40)).filter(Boolean).slice(0, 10),
    doneCriteria: str(t.doneCriteria, 200),
    prep: (Array.isArray(t.prep) ? t.prep : list(t.prep, 10, 60).map((text) => ({ text, done: false })))
      .map((x) => ({ text: str(x.text, 60), done: !!x.done }))
      .filter((x) => x.text)
      .slice(0, 10),
    reviewer: t.reviewer || null,
  };
}

class CollabSync {
  /** onEvent({ type: 'assigned' | 'blocked', ... }) — Telegram 알림 등 바깥으로 알릴 때 */
  constructor({ team, cache, now = () => Date.now(), onEvent = () => {} }) {
    this.team = team;
    this.cache = cache;
    this.now = now;
    this.onEvent = onEvent;
  }

  data() {
    const c = this.cache.get();
    if (!c.collab) this.cache.update((x) => (x.collab = { profiles: [], projects: [], tasks: {}, activeProjectId: null, nudges: {} }));
    return this.cache.get().collab;
  }

  update(fn) {
    this.data();
    this.cache.update((c) => fn(c.collab));
  }

  ctx() {
    const t = this.team.team();
    if (!t || !t.teamId) throw new Error('먼저 팀에 연결해 주세요 (설정 → 팀 연결).');
    return { teamId: t.teamId, b: this.team.getBackend() };
  }

  myUid() {
    return this.cache.get().myUid;
  }

  async sync() {
    if (!this.team.isConnected()) return;
    const { teamId, b } = this.ctx();
    const [profiles, projects] = await Promise.all([b.listProfiles(teamId), b.listProjects(teamId)]);
    const tasks = {};
    for (const p of projects.slice(0, 10)) tasks[p.id] = await b.listTasks(teamId, p.id);
    this.update((d) => {
      d.profiles = profiles;
      d.projects = projects;
      d.tasks = tasks;
      if (!projects.some((p) => p.id === d.activeProjectId)) d.activeProjectId = projects[0] ? projects[0].id : null;
    });
  }

  // ---------- 자기소개 ----------
  myProfile() {
    return this.data().profiles.find((p) => p.uid === this.myUid()) || null;
  }

  async saveProfile(input) {
    const { teamId, b } = this.ctx();
    const p = cleanProfile(input);
    await b.setProfile(teamId, p);
    this.update((d) => {
      d.profiles = [...d.profiles.filter((x) => x.uid !== this.myUid()), { uid: this.myUid(), ...p }];
    });
    return p;
  }

  // ---------- 프로젝트 ----------
  async createProject(input) {
    const { teamId, b } = this.ctx();
    const p = {
      title: str(input.title, 80) || '새 프로젝트',
      goal: str(input.goal, 300),
      deliverable: str(input.deliverable, 200),
      deadline: /^\d{4}-\d{2}-\d{2}$/.test(input.deadline || '') ? input.deadline : '',
    };
    const id = await b.createProject(teamId, p);
    this.update((d) => {
      d.projects.push({ id, ...p });
      d.tasks[id] = [];
      d.activeProjectId = id;
    });
    return id;
  }

  async updateProject(id, input) {
    const { teamId, b } = this.ctx();
    const patch = {};
    for (const k of ['title', 'goal', 'deliverable']) if (k in input) patch[k] = str(input[k], k === 'title' ? 80 : 300);
    if ('deadline' in input) patch.deadline = /^\d{4}-\d{2}-\d{2}$/.test(input.deadline || '') ? input.deadline : '';
    await b.updateProject(teamId, id, patch);
    this.update((d) => Object.assign(d.projects.find((p) => p.id === id) || {}, patch));
  }

  setActive(id) {
    this.update((d) => (d.activeProjectId = id));
  }

  active() {
    const d = this.data();
    return d.projects.find((p) => p.id === d.activeProjectId) || null;
  }

  tasksOf(projectId) {
    return this.data().tasks[projectId] || [];
  }

  // ---------- 작업 ----------
  async addTask(projectId, input) {
    const { teamId, b } = this.ctx();
    const t = { ...cleanTask(input), status: 'todo', assignee: null, accepted: false, objections: {} };
    const id = await b.createTask(teamId, projectId, t);
    this.update((d) => (d.tasks[projectId] = [...(d.tasks[projectId] || []), { id, ...t }]));
    return id;
  }

  async editTask(projectId, taskId, input) {
    const patch = cleanTask(input);
    return this.patchTask(projectId, taskId, patch);
  }

  async patchTask(projectId, taskId, patch) {
    const { teamId, b } = this.ctx();
    await b.updateTask(teamId, projectId, taskId, patch);
    this.update((d) => Object.assign((d.tasks[projectId] || []).find((t) => t.id === taskId) || {}, patch));
  }

  async deleteTask(projectId, taskId) {
    const { teamId, b } = this.ctx();
    await b.deleteTask(teamId, projectId, taskId);
    this.update((d) => {
      d.tasks[projectId] = (d.tasks[projectId] || []).filter((t) => t.id !== taskId);
      for (const t of d.tasks[projectId]) t.dependsOn = (t.dependsOn || []).filter((x) => x !== taskId);
    });
  }

  findTask(taskId) {
    for (const [projectId, ts] of Object.entries(this.data().tasks)) {
      const t = ts.find((x) => x.id === taskId);
      if (t) return { projectId, task: t };
    }
    return null;
  }

  async setStatus(taskId, status) {
    if (!STATUSES.includes(status)) throw new Error('알 수 없는 상태예요.');
    const f = this.findTask(taskId);
    if (!f) return;
    const patch = { status };
    if (status === 'doing' && !f.task.startedAt) patch.startedAt = this.now();
    if (status === 'done') patch.doneAt = this.now();
    await this.patchTask(f.projectId, taskId, patch);
    if (status === 'blocked' && f.task.status !== 'blocked') {
      const project = this.data().projects.find((p) => p.id === f.projectId);
      const me = this.members().find((m) => m.uid === this.myUid());
      this.emit({ type: 'blocked', task: f.task.title, project: project ? project.title : '', who: me ? me.name : '팀원' });
    }
  }

  async accept(taskId) {
    const f = this.findTask(taskId);
    if (!f || f.task.assignee !== this.myUid()) throw new Error('나에게 배정된 작업만 수락할 수 있어요.');
    await this.patchTask(f.projectId, taskId, { accepted: true });
  }

  /** 이의 제기: 배정을 풀고, 다음 배분 제안에서 이 사람은 이 작업에서 빠진다 */
  async object(taskId, reason) {
    const f = this.findTask(taskId);
    if (!f) return;
    const r = OBJECTION_REASONS.includes(reason) ? reason : '기타';
    const objections = { ...(f.task.objections || {}), [this.myUid()]: r };
    const patch = { objections };
    if (f.task.assignee === this.myUid()) Object.assign(patch, { assignee: null, accepted: false });
    await this.patchTask(f.projectId, taskId, patch);
  }

  async togglePrep(taskId, index) {
    const f = this.findTask(taskId);
    if (!f) return;
    const prep = (f.task.prep || []).map((p, i) => (i === index ? { ...p, done: !p.done } : p));
    await this.patchTask(f.projectId, taskId, { prep });
  }

  // ---------- 배분 ----------
  members() {
    const profiles = new Map(this.data().profiles.map((p) => [p.uid, p]));
    return this.team.members({ includeMe: true }).map((m) => {
      const p = profiles.get(m.uid) || {};
      return { uid: m.uid, name: m.displayName, weeklyHours: p.weeklyHours || 0, skills: p.skills || {}, learn: p.learn || [], pr: p.pr || '', hasProfile: !!profiles.get(m.uid) };
    });
  }

  plan(projectId) {
    const project = this.data().projects.find((p) => p.id === projectId);
    if (!project) return null;
    return plan({ members: this.members(), tasks: this.tasksOf(projectId), deadline: project.deadline, now: this.now() });
  }

  /** 제안을 실제 배정으로 옮긴다. 수락된 배정은 건드리지 않는다. 각자 수락하거나 이의를 제기할 수 있다.
   *  돌려주는 값: 새로 배정한 목록 [{ taskId, uid, title, hours }] */
  async applyPlan(projectId, assignments) {
    const applied = [];
    for (const a of assignments) {
      if (a.fixed || !a.uid) continue;
      const t = this.tasksOf(projectId).find((x) => x.id === a.taskId);
      if (!t || t.accepted || t.status === 'done' || t.assignee === a.uid) continue;
      await this.patchTask(projectId, a.taskId, { assignee: a.uid, accepted: false });
      applied.push({ taskId: a.taskId, uid: a.uid, title: t.title, hours: a.effectiveHours || t.estimateHours });
    }
    if (applied.length) {
      const project = this.data().projects.find((p) => p.id === projectId);
      this.emit({ type: 'assigned', list: applied, project: project ? project.title : '' });
    }
    return applied;
  }

  emit(evt) {
    try {
      Promise.resolve(this.onEvent(evt)).catch(() => {});
    } catch {
      /* 알림 실패가 작업 자체를 막지 않도록 */
    }
  }

  // ---------- 진행 확인 ----------
  nudge() {
    if (!this.team.isConnected()) return null;
    const project = this.active();
    if (!project) return null;
    const tasks = this.tasksOf(project.id);
    if (!tasks.length) return null;
    const p = this.plan(project.id);
    const effective = Object.fromEntries((p ? p.assignments : []).map((a) => [a.taskId, a.effectiveHours]));
    return nextNudge({
      myUid: this.myUid(),
      project,
      tasks,
      profile: this.myProfile(),
      planWarnings: p ? p.warnings : [],
      effective,
      now: this.now(),
      recent: this.data().nudges || {},
    });
  }

  markNudged(key) {
    this.update((d) => (d.nudges = { ...(d.nudges || {}), [key]: this.now() }));
  }
}

module.exports = { CollabSync, cleanProfile, cleanTask, OBJECTION_REASONS, STATUSES };
