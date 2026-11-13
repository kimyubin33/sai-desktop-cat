// 여러 팀(팀플)을 동시에 쓰기 위한 묶음. Electron 의존성 없음.
//
// 팀마다 TeamSync · CollabSync · Notifier 를 하나씩 두고, 저장소는 팀별로 나눠 쓴다.
//   settings.teams        [{ teamId, name, joinCode, displayName }]
//   settings.activeTeamId 화면(협업·정리함·공유 기본값)에서 보고 있는 팀
//   teamCache.teams[id]   팀별 캐시 (팀원·받은 주제·제안·협업·Telegram)
//   outbox.items[].teamId 공유 항목이 어느 팀으로 가는지
// 고양이의 말풍선(받은 조언 주제, 팀 약속 제안, 진행 확인)은 모든 팀을 함께 본다.
// 서버 계정(익명 로그인)은 하나를 모든 팀이 같이 쓴다.
const { TeamSync } = require('./team-sync');
const { CollabSync } = require('../collab/collab-sync');
const { Notifier } = require('../notify/notifier');
const { TOPICS } = require('../topics');

class TeamHub {
  /**
   * settings / outbox / teamCache: { get(), update(fn) } 저장소
   * makeBackend: () => 백엔드 (모든 팀이 같은 인스턴스를 쓴다)
   */
  constructor({ settings, outbox, teamCache, makeBackend, delayMs = null, makeTelegram, now = () => Date.now() }) {
    this.settings = settings;
    this.outbox = outbox;
    this.teamCache = teamCache;
    this.makeBackendRaw = makeBackend;
    this.backend = null;
    this.delayMs = delayMs;
    this.makeTelegram = makeTelegram;
    this.now = now;
    this.bundles = new Map();
    this.migrate();
    this.detached = this.makeBundle(null);
  }

  // ---------- 예전(팀 하나) 저장 형식에서 옮기기 ----------
  migrate() {
    const s = this.settings.get();
    if (!Array.isArray(s.teams)) this.settings.update((x) => (x.teams = []));
    const old = this.settings.get().team;
    if (old && old.teamId) {
      this.settings.update((x) => {
        if (!x.teams.some((t) => t.teamId === old.teamId)) x.teams.push(old);
        x.activeTeamId = x.activeTeamId || old.teamId;
        x.team = null;
      });
      this.teamCache.update((c) => {
        const moved = {};
        for (const k of Object.keys(c)) if (k !== 'teams') (moved[k] = c[k]), delete c[k];
        c.teams = { ...(c.teams || {}), [old.teamId]: moved };
      });
      this.outbox.update((d) => {
        for (const it of d.items) if (!it.teamId) it.teamId = old.teamId;
      });
    }
    if (!this.teamCache.get().teams) this.teamCache.update((c) => (c.teams = {}));
  }

  // ---------- 백엔드 (공유) ----------
  getBackend() {
    if (!this.backend) this.backend = this.makeBackendRaw();
    return this.backend;
  }

  resetBackend() {
    this.backend = null;
  }

  // ---------- 팀별 저장소 ----------
  scopedCache(id) {
    const key = id || '_none';
    const hub = this;
    return {
      get() {
        const all = hub.teamCache.get().teams || {};
        if (!all[key]) hub.teamCache.update((c) => (c.teams = { ...(c.teams || {}), [key]: {} }));
        return hub.teamCache.get().teams[key];
      },
      update(fn) {
        this.get();
        hub.teamCache.update((c) => fn(c.teams[key]));
        return hub.teamCache.get().teams[key];
      },
    };
  }

  scopedOutbox(id) {
    const hub = this;
    const mine = (it) => (it.teamId || null) === (id || null);
    return {
      get() {
        return { items: hub.outbox.get().items.filter(mine) };
      },
      update(fn) {
        hub.outbox.update((d) => {
          const view = { items: d.items.filter(mine) };
          fn(view);
          d.items = [...d.items.filter((x) => !mine(x)), ...view.items.map((x) => ({ ...x, teamId: id || null }))];
        });
      },
    };
  }

  makeBundle(teamId) {
    const hub = this;
    const state = teamId
      ? {
          get: () => hub.settings.get().teams.find((t) => t.teamId === teamId) || null,
          set: (v) => (v ? hub.replaceTeam(teamId, v) : hub.removeTeam(teamId)),
        }
      : { get: () => null, set: (v) => v && hub.addTeam(v) };
    const cache = this.scopedCache(teamId);
    const notifier = new Notifier({ team: null, cache, ...(this.makeTelegram ? { makeClient: this.makeTelegram } : {}) });
    const sync = new TeamSync({
      makeBackend: () => hub.getBackend(),
      state,
      outbox: this.scopedOutbox(teamId),
      cache,
      delayMs: this.delayMs,
      now: this.now,
      onProposal: (topic) => notifier.proposal(TOPICS[topic] ? TOPICS[topic].teamProposal : ''),
    });
    // 공유 백엔드를 쓰도록 TeamSync 의 백엔드 재설정을 허브로 돌린다
    sync.resetBackend = () => hub.resetBackend();
    sync.getBackend = () => {
      const b = hub.getBackend();
      if (!b) throw new Error('팀 서버(Firebase) 설정이 아직 없어요.');
      return b;
    };
    notifier.team = sync;
    const collab = new CollabSync({
      team: sync,
      cache,
      now: this.now,
      onEvent: (e) => {
        if (e.type === 'assigned') return notifier.assigned(e.list, e.project);
        if (e.type === 'blocked') return notifier.blocked(e.task, e.project, e.who);
        return null;
      },
    });
    return { teamId, sync, collab, notifier, name: () => (state.get() || {}).name || '' };
  }

  // ---------- 팀 목록 ----------
  teams() {
    return this.settings.get().teams || [];
  }

  bundle(teamId) {
    if (!teamId) return this.detached;
    if (!this.teams().some((t) => t.teamId === teamId)) return null;
    if (!this.bundles.has(teamId)) this.bundles.set(teamId, this.makeBundle(teamId));
    return this.bundles.get(teamId);
  }

  all() {
    return this.teams().map((t) => this.bundle(t.teamId));
  }

  active() {
    const s = this.settings.get();
    const id = this.teams().some((t) => t.teamId === s.activeTeamId) ? s.activeTeamId : this.teams()[0] && this.teams()[0].teamId;
    return id ? this.bundle(id) : this.detached;
  }

  setActive(teamId) {
    if (!this.teams().some((t) => t.teamId === teamId)) throw new Error('그 팀을 찾을 수 없어요.');
    this.settings.update((s) => (s.activeTeamId = teamId));
  }

  addTeam(t) {
    const first = this.teams().length === 0;
    this.settings.update((s) => {
      s.teams = [...(s.teams || []).filter((x) => x.teamId !== t.teamId), t];
      s.activeTeamId = t.teamId;
    });
    // 팀이 하나도 없을 때 저장해 둔 공유 항목은 처음 연결한 팀으로 보낸다
    if (first) {
      this.outbox.update((d) => {
        for (const it of d.items) if (!it.teamId) it.teamId = t.teamId;
      });
    }
  }

  replaceTeam(teamId, t) {
    this.settings.update((s) => (s.teams = s.teams.map((x) => (x.teamId === teamId ? t : x))));
  }

  removeTeam(teamId) {
    this.settings.update((s) => {
      s.teams = s.teams.filter((x) => x.teamId !== teamId);
      if (s.activeTeamId === teamId) s.activeTeamId = s.teams[0] ? s.teams[0].teamId : null;
    });
    this.teamCache.update((c) => delete c.teams[teamId]);
    this.bundles.delete(teamId);
  }

  // ---------- 만들기·참여·나가기 ----------
  async createTeam(name, displayName) {
    await this.detached.sync.createTeam(name, displayName);
    const b = this.active();
    await this.syncTeam(b);
    return b.sync.team();
  }

  async joinTeam(code, displayName) {
    const c = String(code || '').trim().toUpperCase();
    const existing = this.teams().find((t) => t.joinCode === c);
    if (existing) throw new Error(`이미 참여한 팀이에요: ${existing.name}`);
    await this.detached.sync.joinTeam(code, displayName);
    const b = this.active();
    await this.syncTeam(b);
    return b.sync.team();
  }

  async leaveTeam(teamId) {
    const b = this.bundle(teamId);
    if (!b) return;
    await b.sync.leaveTeam();
    // 이 팀으로 가려던 공유 항목과 캐시도 함께 지운다
    this.outbox.update((d) => (d.items = d.items.filter((x) => x.teamId !== teamId)));
    this.teamCache.update((c) => delete c.teams[teamId]);
    this.bundles.delete(teamId);
  }

  // ---------- 동기화 ----------
  async syncTeam(b) {
    const r = await b.sync.sync();
    if (r.ok) {
      try {
        await b.collab.sync();
      } catch (e) {
        b.sync.cache.update((c) => (c.lastError = String(e.message || e)));
      }
      await b.notifier.sync().catch((e) => b.notifier.record(false, String(e.message || e)));
    }
    return r;
  }

  async syncAll() {
    const results = [];
    for (const b of this.all()) results.push(await this.syncTeam(b));
    return results;
  }

  // ---------- 모든 팀을 함께 보는 창구 (고양이 말풍선용) ----------
  /** advisor 의 team 자리 */
  teamFacade() {
    const hub = this;
    return {
      isConnected: () => hub.all().some((b) => b.sync.isConnected()),
      boosts: () => Object.assign({}, ...hub.all().map((b) => b.sync.boosts())),
      pendingProposalsForMe: () =>
        hub.all().flatMap((b) => b.sync.pendingProposalsForMe().map((p) => ({ ...p, teamId: b.teamId, teamName: b.name(), id: p.id }))),
      respond: async (proposalId, response) => {
        const b = hub.all().find((x) => x.sync.proposals().some((p) => p.id === proposalId));
        if (b) await b.sync.respond(proposalId, response);
      },
      holdTopic: async (topic) => {
        let n = 0;
        for (const b of hub.all()) n += await b.sync.holdTopic(topic);
        return n;
      },
      /** "내 사정" 뒤 팀 약속 제안: 그 주제를 받은 팀들에, 없으면 지금 보고 있는 팀에 */
      proposeTeam: (topic) => {
        const got = hub.all().filter((b) => (b.sync.cache.get().inbox || []).some((s) => s.topic === topic));
        const targets = got.length ? got : [hub.active()].filter((b) => b.teamId);
        return targets.map((b) => b.sync.proposeTeam(topic)).filter(Boolean);
      },
    };
  }

  /** advisor 의 collab 자리 — 모든 팀의 진행 확인 중 먼저 할 말 하나 */
  collabFacade() {
    if (this._collabFacade) return this._collabFacade;
    const hub = this;
    let last = null;
    const ofTask = (taskId) => hub.all().find((b) => b.collab.findTask(taskId));
    const facade = {
      nudge: () => {
        for (const b of [hub.active(), ...hub.all().filter((x) => x !== hub.active())]) {
          if (!b.teamId) continue;
          const n = b.collab.nudge();
          if (n) {
            last = b;
            const multi = hub.teams().length > 1;
            return { ...n, teamId: b.teamId, source: multi ? `${b.name()} · ${n.source}` : n.source };
          }
        }
        return null;
      },
      markNudged: (key) => last && last.collab.markNudged(key),
      active: () => (last || hub.active()).collab.active(),
      accept: (taskId) => ofTask(taskId).collab.accept(taskId),
      setStatus: (taskId, status) => ofTask(taskId).collab.setStatus(taskId, status),
      findTask: (taskId) => {
        const b = ofTask(taskId);
        return b ? b.collab.findTask(taskId) : null;
      },
      lastNotifier: () => (last || hub.active()).notifier,
    };
    this._collabFacade = facade;
    return facade;
  }

  /** advisor 의 notifier 자리 — 진행 확인을 낸 팀의 Telegram 으로, 그 팀이 연결 안 됐으면 연결된 다른 팀 봇으로 */
  notifierFacade() {
    const hub = this;
    const collab = this.collabFacade();
    return {
      isLinked: () => hub.all().some((b) => b.notifier.isLinked()),
      nudgeMe: async (text, project) => {
        const own = collab.lastNotifier();
        if (own && own.isLinked()) return own.nudgeMe(text, project);
        const other = hub.all().find((b) => b.notifier.isLinked());
        return other ? other.notifier.nudgeMe(text, project) : 0;
      },
    };
  }
}

module.exports = { TeamHub };
