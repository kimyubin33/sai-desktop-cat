// 팀 연결 로직 (Electron 의존성 없음 — 백엔드와 저장소를 주입받는다)
//
// 공유 대기함 항목의 상태
//   review          보내기 전에 내가 한 번 더 확인해야 함 ("검토 후 공유")
//   needs-recipient 받을 팀원을 아직 고르지 않음
//   queued          보낼 준비 완료. sendAfter 이후 전송 (보낸 시각으로 발언자를 짐작하기 어렵게 늦춰 보냄)
//   sent            전송됨 (remoteId 로 상태 확인)
//   held            받은 사람이 "내 사정도 있어요"로 보류함
//
// 서버로는 주제 id, 받는 사람 uid, 주(週)만 간다. 문안은 받는 쪽 앱의 일반 문안을 쓴다.
const { isoWeek } = require('../privacy');
const { TOPICS } = require('../topics');

const DAY = 24 * 60 * 60 * 1000;
const BOOST_WINDOW_DAYS = 21;
const DEFAULT_DELAY_MIN = [20, 180];

class TeamSync {
  /**
   * makeBackend: () => backend (FirebaseBackend | MemoryBackend) 또는 null(설정 없음)
   * state: { get(), set(v) } 팀 연결 정보 { teamId, name, joinCode, displayName }
   * outbox: { get(), update(fn) } — { items: [] }
   * cache:  { get(), update(fn) } — 팀원·받은 신호·제안 캐시
   */
  constructor({ makeBackend, state, outbox, cache, now = () => Date.now(), rand = Math.random, delayMin = DEFAULT_DELAY_MIN, delayMs = null }) {
    this.makeBackend = makeBackend;
    this.state = state;
    this.outbox = outbox;
    this.cache = cache;
    this.now = now;
    this.rand = rand;
    this.delayMin = delayMin;
    this.delayMs = delayMs;
    this.backend = null;
    this.syncing = null;
  }

  team() {
    return this.state.get();
  }

  isConnected() {
    return !!(this.team() && this.team().teamId);
  }

  getBackend() {
    if (!this.backend) this.backend = this.makeBackend();
    if (!this.backend) throw new Error('팀 서버(Firebase) 설정이 아직 없어요. 설정에서 먼저 입력해 주세요.');
    return this.backend;
  }

  resetBackend() {
    this.backend = null;
  }

  // ---------- 팀 만들기·참여·나가기 ----------
  async createTeam(name, displayName) {
    const b = this.getBackend();
    const t = await b.createTeam({ name: clean(name, 60) || '우리 팀', displayName: clean(displayName, 30) || '팀원' });
    this.state.set({ teamId: t.teamId, name: t.name, joinCode: t.joinCode, displayName: clean(displayName, 30) || '팀원' });
    this.resetCache();
    await this.sync();
    return this.team();
  }

  async joinTeam(code, displayName) {
    const b = this.getBackend();
    const t = await b.joinTeam({ joinCode: code, displayName: clean(displayName, 30) || '팀원' });
    this.state.set({ teamId: t.teamId, name: t.name, joinCode: t.joinCode, displayName: clean(displayName, 30) || '팀원' });
    this.resetCache();
    await this.sync();
    return this.team();
  }

  async leaveTeam() {
    const t = this.team();
    if (t && t.teamId) {
      try {
        await this.getBackend().leaveTeam(t.teamId);
      } catch (e) {
        // 서버에 닿지 않아도 이 기기에서는 연결을 끊는다
        this.cache.update((c) => (c.lastError = String(e.message || e)));
      }
    }
    this.state.set(null);
    this.resetCache();
    // 아직 보내지 않은 항목은 남기고, 이 팀으로 보낸 기록은 지운다
    this.outbox.update((d) => {
      d.items = d.items.filter((x) => !['sent', 'held'].includes(x.status));
      for (const x of d.items) if (x.target === 'person') x.toMember = null;
    });
  }

  resetCache() {
    this.cache.update((c) => {
      c.members = [];
      c.inbox = [];
      c.proposals = [];
      c.lastSync = 0;
      c.lastError = null;
      c.myUid = null;
    });
  }

  // ---------- 공유 대기함 ----------
  sendAfter() {
    if (this.delayMs != null) return this.now() + this.delayMs;
    const [a, b] = this.delayMin;
    return this.now() + Math.round((a + this.rand() * (b - a)) * 60 * 1000);
  }

  /** 새로 저장된 공유 항목에 상태를 붙인다 */
  prepareItems(items) {
    return items.map((it) => {
      const base = { ...it, toMember: it.target === 'person' ? it.toMember || null : null };
      if (it.scope === 'review') return { ...base, status: 'review' };
      if (it.target === 'person' && !base.toMember) return { ...base, status: 'needs-recipient' };
      return { ...base, status: 'queued', sendAfter: this.sendAfter() };
    });
  }

  confirmReview(itemId, toMember) {
    this.updateItem(itemId, (x) => {
      if (toMember) x.toMember = toMember;
      if (x.target === 'person' && !x.toMember) x.status = 'needs-recipient';
      else {
        x.status = 'queued';
        x.sendAfter = this.sendAfter();
      }
    });
  }

  setRecipient(itemId, toMember) {
    this.updateItem(itemId, (x) => {
      x.toMember = toMember;
      if (x.status === 'needs-recipient') {
        x.status = 'queued';
        x.sendAfter = this.sendAfter();
      }
    });
  }

  updateItem(itemId, fn) {
    this.outbox.update((d) => {
      const x = d.items.find((i) => i.id === itemId);
      if (x) fn(x);
    });
  }

  /** "내 사정"을 들은 뒤, 개인 조언 대신 팀 약속으로 맞춰 보자고 제안 */
  proposeTeam(topic) {
    if (!TOPICS[topic]) return null;
    const exists = this.outbox.get().items.find((x) => x.topic === topic && x.target === 'team' && x.status !== 'sent');
    if (exists) return exists.id;
    const id = `o${this.now().toString(36)}${Math.floor(this.rand() * 1e6).toString(36)}`;
    this.outbox.update((d) => {
      d.items.push({
        id,
        topic,
        label: TOPICS[topic].label,
        scope: 'auto',
        target: 'team',
        advice: TOPICS[topic].advice[0],
        teamProposal: TOPICS[topic].teamProposal,
        week: isoWeek(this.now()),
        toMember: null,
        status: 'queued',
        sendAfter: this.sendAfter(),
      });
    });
    return id;
  }

  async sendDue() {
    const t = this.team();
    if (!t) return 0;
    const b = this.getBackend();
    const due = this.outbox.get().items.filter((x) => x.status === 'queued' && (x.sendAfter || 0) <= this.now());
    let sent = 0;
    const memberIds = new Set((this.cache.get().members || []).map((m) => m.uid));
    for (const it of due) {
      try {
        let remoteId;
        const week = isoWeek(this.now());
        if (it.target === 'team') {
          // 같은 주제의 제안이 이미 있으면 새로 만들지 않고 동의만 한다 (여러 명의 의견처럼 부풀리지 않기)
          const existing = (this.cache.get().proposals || []).find((p) => p.topic === it.topic && p.week === week);
          if (existing) {
            await b.respondProposal(t.teamId, existing.id, 'agree');
            remoteId = existing.id;
          } else {
            remoteId = await b.createProposal(t.teamId, { topic: it.topic, week });
            await b.respondProposal(t.teamId, remoteId, 'agree');
          }
        } else {
          if (memberIds.size && !memberIds.has(it.toMember)) {
            this.updateItem(it.id, (x) => {
              x.status = 'needs-recipient';
              x.toMember = null;
            });
            continue;
          }
          remoteId = await b.createSignal(t.teamId, { topic: it.topic, toMember: it.toMember, week });
        }
        this.updateItem(it.id, (x) => {
          x.status = 'sent';
          x.remoteId = remoteId;
          x.week = week;
          delete x.sendAfter;
        });
        sent++;
      } catch (e) {
        this.updateItem(it.id, (x) => (x.lastError = String(e.message || e)));
      }
    }
    return sent;
  }

  // ---------- 동기화 ----------
  async sync() {
    if (this.syncing) return this.syncing;
    this.syncing = this._sync().finally(() => {
      this.syncing = null;
    });
    return this.syncing;
  }

  async _sync() {
    const t = this.team();
    if (!t || !t.teamId) return { ok: false, reason: 'not-connected' };
    try {
      const b = this.getBackend();
      const myUid = await b.signIn();
      const members = await b.listMembers(t.teamId);
      const proposals = await b.listProposals(t.teamId);
      this.cache.update((c) => {
        c.members = members;
        c.proposals = proposals;
        c.myUid = myUid;
      });
      if (!members.some((m) => m.uid === myUid)) {
        throw new Error('이 기기는 더 이상 팀원으로 등록되어 있지 않아요. 참여 코드로 다시 참여해 주세요.');
      }
      await this.sendDue();
      const inbox = await b.listSignalsForMe(t.teamId);
      // 내가 보낸 신호가 보류되었는지 확인
      for (const it of this.outbox.get().items.filter((x) => x.status === 'sent' && x.target === 'person' && x.remoteId)) {
        const s = await b.getSignal(t.teamId, it.remoteId).catch(() => null);
        if (s && s.held) this.updateItem(it.id, (x) => (x.status = 'held'));
      }
      this.cache.update((c) => {
        c.inbox = inbox.map(({ id, topic, week, held }) => ({ id, topic, week, held }));
        c.proposals = proposals;
        c.lastSync = this.now();
        c.lastError = null;
      });
      return { ok: true };
    } catch (e) {
      this.cache.update((c) => (c.lastError = String(e.message || e)));
      return { ok: false, error: String(e.message || e) };
    }
  }

  // ---------- 받은 쪽 ----------
  /** 최근 3주 동안 받은 (보류되지 않은) 주제 → 평소 조언에서 조금 더 자주 다룸 */
  boosts() {
    if (!this.isConnected()) return {};
    const cutoff = isoWeek(this.now() - BOOST_WINDOW_DAYS * DAY);
    const out = {};
    for (const s of this.cache.get().inbox || []) {
      if (!s.held && TOPICS[s.topic] && s.week >= cutoff) out[s.topic] = true;
    }
    return out;
  }

  /** "내 사정도 있어요": 이 주제로 받은 신호를 보류한다. 받은 신호가 없으면 아무것도 하지 않는다. */
  async holdTopic(topic) {
    const t = this.team();
    if (!t) return 0;
    const targets = (this.cache.get().inbox || []).filter((s) => s.topic === topic && !s.held);
    let n = 0;
    for (const s of targets) {
      try {
        await this.getBackend().holdSignal(t.teamId, s.id);
        n++;
      } catch (e) {
        this.cache.update((c) => (c.lastError = String(e.message || e)));
      }
    }
    if (n)
      this.cache.update((c) => {
        for (const s of c.inbox) if (s.topic === topic) s.held = true;
      });
    return n;
  }

  // ---------- 팀 약속 ----------
  proposals() {
    const c = this.cache.get();
    const memberIds = new Set((c.members || []).map((m) => m.uid));
    return (c.proposals || [])
      .filter((p) => TOPICS[p.topic])
      .map((p) => {
        const responses = Object.entries(p.responses || {}).filter(([uid]) => memberIds.has(uid));
        const agree = responses.filter(([, r]) => r === 'agree').length;
        return {
          id: p.id,
          topic: p.topic,
          label: TOPICS[p.topic].label,
          text: TOPICS[p.topic].teamProposal,
          week: p.week,
          agree,
          memberCount: memberIds.size,
          agreedByAll: memberIds.size > 1 && agree >= memberIds.size,
          mine: (p.responses || {})[c.myUid] || null,
        };
      })
      .sort((a, b) => (a.week < b.week ? 1 : -1));
  }

  pendingProposalsForMe() {
    return this.proposals().filter((p) => !p.mine);
  }

  async respond(proposalId, response) {
    const t = this.team();
    if (!t) return;
    await this.getBackend().respondProposal(t.teamId, proposalId, response);
    this.cache.update((c) => {
      const p = (c.proposals || []).find((x) => x.id === proposalId);
      if (p) p.responses = { ...(p.responses || {}), [c.myUid]: response };
    });
  }

  // ---------- 화면용 ----------
  members({ includeMe = false } = {}) {
    const c = this.cache.get();
    return (c.members || []).filter((m) => includeMe || m.uid !== c.myUid);
  }

  status() {
    const t = this.team();
    const c = this.cache.get();
    return {
      connected: this.isConnected(),
      team: t,
      memberCount: (c.members || []).length,
      lastSync: c.lastSync || 0,
      lastError: c.lastError || null,
      backendKind: this.backend ? this.backend.kind : null,
    };
  }
}

function clean(s, max) {
  return String(s || '')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, max);
}

module.exports = { TeamSync, BOOST_WINDOW_DAYS };
