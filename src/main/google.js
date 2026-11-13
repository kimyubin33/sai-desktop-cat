// Google 캘린더·할 일 연결을 앱에 붙인다: 로그인 창 열기, 빈 시간 계산, 작업 시간 잡기, 할 일 동기화.
const http = require('http');
const crypto = require('crypto');
const { shell } = require('electron');
const { GoogleAuth, pkce, authUrl, parseGoogleClient } = require('../core/google/oauth');
const { queryBusy, freeHours, findSlot, createEvent } = require('../core/google/calendar');
const { ensureList, syncTasks } = require('../core/google/tasks');
const { encryptSecret, decryptJson } = require('./secret');
const { BUILTIN_GOOGLE } = require('./firebase-config');

const DAY = 24 * 60 * 60 * 1000;
const LOGIN_TIMEOUT_MS = 5 * 60 * 1000;

function createGoogleService({ stores, collabs, activeCollab }) {
  const connected = () => collabs().filter((c) => c.team.isConnected());
  const findProject = (projectId) => {
    for (const c of connected()) {
      const p = c.data().projects.find((x) => x.id === projectId);
      if (p) return { c, p };
    }
    return null;
  };
  const settings = stores.settings;
  const client = () => settings.get().googleClient || BUILTIN_GOOGLE;
  let auth = null;

  function getAuth() {
    const c = client();
    if (!c) throw new Error('Google 연결 정보(OAuth 클라이언트)가 아직 없어요. 설정 → 외부 연결에서 넣어 주세요.');
    if (!auth || auth.client.clientId !== c.clientId) {
      auth = new GoogleAuth({
        client: c,
        loadTokens: () => decryptJson(settings.get().googleTokensEnc),
        saveTokens: (t) => settings.update((s) => (s.googleTokensEnc = t ? encryptSecret(t) : null)),
      });
    }
    return auth;
  }

  const api = (m, u, b) => getAuth().api(m, u, b);

  function state() {
    const c = stores.teamCache.get();
    return c.google || {};
  }

  function record(patch) {
    stores.teamCache.update((c) => (c.google = { ...(c.google || {}), ...patch }));
  }

  function fail(e) {
    record({ lastError: String(e.message || e), lastErrorAt: Date.now() });
    throw e;
  }

  // ---------- 로그인 ----------
  function login() {
    const a = getAuth();
    const { verifier, challenge } = pkce();
    const st = crypto.randomBytes(16).toString('hex');
    return new Promise((resolve, reject) => {
      const server = http.createServer(async (req, res) => {
        const u = new URL(req.url, 'http://127.0.0.1');
        if (u.pathname !== '/') return res.writeHead(404).end();
        const done = (ok, msg) => {
          res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
          res.end(
            `<!doctype html><meta charset="utf-8"><title>사이</title><body style="font-family:sans-serif;padding:40px;text-align:center"><h2>${ok ? '사이와 Google 연결 완료' : '연결하지 못했어요'}</h2><p>${msg}</p><p>이 창은 닫아도 돼요.</p></body>`,
          );
        };
        clearTimeout(timer);
        server.close();
        if (u.searchParams.get('state') !== st) {
          done(false, '요청이 맞지 않아요. 사이에서 다시 시도해 주세요.');
          return reject(new Error('로그인 요청이 맞지 않아요.'));
        }
        if (u.searchParams.get('error')) {
          done(false, '로그인을 취소했거나 권한을 허용하지 않았어요.');
          return reject(new Error(`Google 로그인 취소: ${u.searchParams.get('error')}`));
        }
        try {
          const r = await a.exchangeCode({ code: u.searchParams.get('code'), verifier, redirectUri });
          record({ lastError: null });
          done(true, `${r.email || 'Google 계정'}으로 연결됐어요.`);
          resolve(r);
        } catch (e) {
          done(false, String(e.message || e));
          reject(e);
        }
      });
      let redirectUri;
      const timer = setTimeout(() => {
        server.close();
        reject(new Error('5분 안에 로그인을 마치지 못했어요. 다시 시도해 주세요.'));
      }, LOGIN_TIMEOUT_MS);
      server.listen(0, '127.0.0.1', () => {
        redirectUri = `http://127.0.0.1:${server.address().port}`;
        shell.openExternal(authUrl({ clientId: a.client.clientId, redirectUri, challenge, state: st }));
      });
    });
  }

  async function logout() {
    await getAuth().signOut();
    record({ lastSync: 0, lastResult: null, lastError: null });
  }

  // ---------- 캘린더 ----------
  /** 이번 7일, 그리고 마감까지의 빈 시간 (9~22시 기준). 이 기기에서만 계산·표시한다 */
  async function availability(projectId) {
    try {
      const now = Date.now();
      const found = projectId && findProject(projectId);
      const project = found ? found.p : activeCollab().active();
      const deadline = project && project.deadline ? new Date(`${project.deadline}T23:59`).getTime() : null;
      const until = Math.max(now + 7 * DAY, deadline || 0);
      const busy = await queryBusy(api, { from: now, to: until });
      const week = freeHours(busy, { from: now, to: now + 7 * DAY });
      const toDeadline = deadline && deadline > now ? freeHours(busy, { from: now, to: deadline }) : null;
      record({ lastCalendar: now, lastError: null });
      return { week, toDeadline, window: '9시~22시' };
    } catch (e) {
      return fail(e);
    }
  }

  /** 내 작업 시간을 캘린더 첫 빈 틈에 잡는다 (최대 2시간 블록) */
  async function scheduleTask(taskId) {
    try {
      const collab = connected().find((c) => c.findTask(taskId));
      const f = collab && collab.findTask(taskId);
      if (!f) throw new Error('작업을 찾을 수 없어요.');
      const plan = collab.plan(f.projectId);
      const a = plan && plan.assignments.find((x) => x.taskId === taskId);
      const hours = (a && a.effectiveHours) || f.task.estimateHours || 1;
      const now = Date.now();
      const busy = await queryBusy(api, { from: now, to: now + 14 * DAY });
      const slot = findSlot(busy, { from: now, hours });
      if (!slot) throw new Error('앞으로 2주 안에 1시간 이상 빈 시간이 없어요.');
      const project = collab.data().projects.find((p) => p.id === f.projectId);
      await createEvent(api, {
        summary: `사이: ${f.task.title}`,
        description: [f.task.doneCriteria && `완료 기준: ${f.task.doneCriteria}`, project && `프로젝트: ${project.title}`].filter(Boolean).join('\n'),
        start: slot.start,
        end: slot.end,
        saiTaskId: taskId,
      });
      record({ lastError: null, lastResult: `"${f.task.title}" 일정을 캘린더에 넣었어요.`, lastSync: now });
      return { start: slot.start, end: slot.end, hours: (slot.end - slot.start) / 3600000, totalHours: hours };
    } catch (e) {
      return fail(e);
    }
  }

  // ---------- 할 일 ----------
  async function syncTodo() {
    if (!client() || !getAuth().isSignedIn() || !connected().length) return null;
    try {
      const results = [];
      // 모든 팀의 프로젝트를 돌며, 팀마다 내 uid 기준으로 수락한 작업을 올린다
      for (const collab of connected())
      for (const project of collab.data().projects) {
        const me = collab.myUid();
        const mine = collab.tasksOf(project.id).filter((t) => t.assignee === me && t.accepted);
        const links = (settings.get().googleTaskLinks || {})[project.id] || {};
        if (!mine.length && !Object.keys(links).length) continue;
        const listId = await ensureList(api, `사이 · ${project.title}`);
        const r = await syncTasks(api, { listId, mine, links, deadline: project.deadline, projectTitle: project.title });
        settings.update((s) => (s.googleTaskLinks = { ...(s.googleTaskLinks || {}), [project.id]: r.links }));
        for (const id of r.markDone) await collab.setStatus(id, 'done');
        results.push(r);
      }
      const sum = (k) => results.reduce((a, r) => a + (Array.isArray(r[k]) ? r[k].length : r[k]), 0);
      const text = `할 일 ${sum('created')}개 추가, Google 에서 완료한 ${sum('markDone')}개 반영, 사이에서 완료한 ${sum('completedInGoogle')}개 반영`;
      record({ lastSync: Date.now(), lastResult: text, lastError: null });
      return text;
    } catch (e) {
      return fail(e);
    }
  }

  function status() {
    const c = client();
    const t = c ? decryptJson(settings.get().googleTokensEnc) : null;
    const s = state();
    return {
      configured: !!c,
      builtin: !settings.get().googleClient && !!BUILTIN_GOOGLE,
      signedIn: !!(t && t.refreshToken),
      email: t ? t.email : null,
      lastSync: s.lastSync || 0,
      lastResult: s.lastResult || null,
      lastError: s.lastError || null,
    };
  }

  function configure(text) {
    const c = parseGoogleClient(text);
    if (!c) throw new Error('클라이언트 ID(…apps.googleusercontent.com)와 보안 비밀번호(GOCSPX-…)를 찾지 못했어요.');
    settings.update((s) => {
      s.googleClient = c;
      s.googleTokensEnc = null;
    });
    auth = null;
    return c;
  }

  return { login, logout, availability, scheduleTask, syncTodo, status, configure };
}

module.exports = { createGoogleService };
