// FirebaseBackend 와 같은 인터페이스의 시험용 백엔드.
// - 테스트: 여러 인스턴스가 하나의 hub 객체를 공유한다.
// - 로컬 시험 모드: file 경로를 주면 한 컴퓨터에서 앱 두 개를 띄워 팀처럼 시험할 수 있다.
// 권한 검사도 firestore.rules 와 같은 규칙으로 흉내 낸다.
const fs = require('fs');
const path = require('path');
const { randomCode } = require('./firebase-backend');

function emptyHub() {
  return { seq: 0, joinCodes: {}, teams: {} };
}

class MemoryBackend {
  constructor({ hub, file, uid, rand = Math.random } = {}) {
    this.file = file || null;
    this.hub = hub || (this.file ? null : emptyHub());
    this.rand = rand;
    this.uidValue = uid || `local-${randomCode(10, rand).toLowerCase()}`;
  }

  get kind() {
    return this.file ? 'local-file' : 'memory';
  }

  load() {
    if (!this.file) return this.hub;
    try {
      return JSON.parse(fs.readFileSync(this.file, 'utf8'));
    } catch {
      return emptyHub();
    }
  }

  save(hub) {
    if (!this.file) return;
    fs.mkdirSync(path.dirname(this.file), { recursive: true });
    const tmp = `${this.file}.${process.pid}.tmp`;
    fs.writeFileSync(tmp, JSON.stringify(hub, null, 2));
    fs.renameSync(tmp, this.file);
  }

  tx(fn) {
    const hub = this.load();
    const out = fn(hub);
    this.save(hub);
    return out;
  }

  nextId(hub) {
    hub.seq += 1;
    return `d${hub.seq.toString(36)}${randomCode(4, this.rand).toLowerCase()}`;
  }

  get uid() {
    return this.uidValue;
  }

  async signIn() {
    return this.uidValue;
  }

  team(hub, teamId) {
    const t = hub.teams[teamId];
    if (!t) throw new Error('팀을 찾을 수 없어요.');
    return t;
  }

  member(hub, teamId) {
    const t = this.team(hub, teamId);
    if (!t.members[this.uidValue]) {
      const e = new Error('팀원이 아니에요.');
      e.status = 403;
      throw e;
    }
    return t;
  }

  async createTeam({ name, displayName }) {
    return this.tx((hub) => {
      let joinCode;
      do joinCode = randomCode(6, this.rand);
      while (hub.joinCodes[joinCode]);
      const teamId = this.nextId(hub);
      hub.teams[teamId] = { name, joinCode, createdBy: this.uidValue, members: {}, signals: {}, proposals: {} };
      hub.joinCodes[joinCode] = { teamId, teamName: name };
      hub.teams[teamId].members[this.uidValue] = { displayName, joinCode };
      return { teamId, joinCode, name };
    });
  }

  async joinTeam({ joinCode, displayName }) {
    return this.tx((hub) => {
      const code = String(joinCode || '').trim().toUpperCase();
      const ref = hub.joinCodes[code];
      if (!ref) throw new Error('참여 코드를 찾을 수 없어요.');
      this.team(hub, ref.teamId).members[this.uidValue] = { displayName, joinCode: code };
      return { teamId: ref.teamId, joinCode: code, name: ref.teamName };
    });
  }

  async leaveTeam(teamId) {
    this.tx((hub) => {
      delete this.team(hub, teamId).members[this.uidValue];
    });
  }

  async listMembers(teamId) {
    const t = this.member(this.load(), teamId);
    return Object.entries(t.members).map(([uid, m]) => ({ uid, displayName: m.displayName }));
  }

  async createSignal(teamId, { topic, toMember, week }) {
    return this.tx((hub) => {
      const t = this.member(hub, teamId);
      if (toMember === this.uidValue) throw Object.assign(new Error('자기 자신에게는 보낼 수 없어요.'), { status: 403 });
      if (!t.members[toMember]) throw Object.assign(new Error('받는 사람이 팀원이 아니에요.'), { status: 403 });
      const id = this.nextId(hub);
      t.signals[id] = { topic, toMember, week, held: false };
      return id;
    });
  }

  async getSignal(teamId, id) {
    const t = this.member(this.load(), teamId);
    return t.signals[id] ? { id, ...t.signals[id] } : null;
  }

  async listSignalsForMe(teamId) {
    const t = this.member(this.load(), teamId);
    return Object.entries(t.signals)
      .filter(([, s]) => s.toMember === this.uidValue)
      .map(([id, s]) => ({ id, ...s }));
  }

  async holdSignal(teamId, id) {
    this.tx((hub) => {
      const s = this.member(hub, teamId).signals[id];
      if (!s || s.toMember !== this.uidValue) throw Object.assign(new Error('보류할 수 없어요.'), { status: 403 });
      s.held = true;
    });
  }

  async createProposal(teamId, { topic, week }) {
    return this.tx((hub) => {
      const t = this.member(hub, teamId);
      const id = this.nextId(hub);
      t.proposals[id] = { topic, week, responses: {} };
      return id;
    });
  }

  async listProposals(teamId) {
    const t = this.member(this.load(), teamId);
    return Object.entries(t.proposals).map(([id, p]) => ({ id, ...p }));
  }

  async respondProposal(teamId, id, response) {
    this.tx((hub) => {
      const p = this.member(hub, teamId).proposals[id];
      if (!p) throw new Error('제안을 찾을 수 없어요.');
      p.responses[this.uidValue] = response;
    });
  }
}

module.exports = { MemoryBackend, emptyHub };
