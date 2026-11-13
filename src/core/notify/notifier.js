// Telegram 알림 연결과 보내기. TeamSync 의 팀 연결·백엔드를 쓴다.
//
// 누가 무엇을 받는가
//   - 배정 제안: 배정된 팀원 (팀 알림을 켠 경우)
//   - 막힘 도움 요청: 다른 팀원 모두 (팀 알림을 켠 경우)
//   - 팀 약속 제안: 다른 팀원 모두 (제안한 사람은 드러내지 않음)
//   - 진행 확인: 나에게만, 컴퓨터에서 자리를 비웠을 때
const { TelegramClient, findStartCode, notifyText } = require('./telegram');
const { randomCode } = require('../team/firebase-backend');

const LINK_TTL_MS = 30 * 60 * 1000;

class Notifier {
  constructor({ team, cache, makeClient = (token) => new TelegramClient({ token }), now = () => Date.now(), rand = Math.random }) {
    this.team = team;
    this.cache = cache;
    this.makeClient = makeClient;
    this.now = now;
    this.rand = rand;
  }

  state() {
    const c = this.cache.get();
    if (!c.telegram) this.cache.update((x) => (x.telegram = { bot: null, links: [], pending: null, lastResult: null, lastError: null }));
    return this.cache.get().telegram;
  }

  update(fn) {
    this.state();
    this.cache.update((c) => fn(c.telegram));
  }

  ctx() {
    const t = this.team.team();
    if (!t || !t.teamId) throw new Error('먼저 팀에 연결해 주세요.');
    return { teamId: t.teamId, b: this.team.getBackend() };
  }

  myUid() {
    return this.cache.get().myUid;
  }

  record(ok, text) {
    this.update((s) => {
      s.lastResult = { ok, text, at: this.now() };
      if (!ok) s.lastError = text;
      else s.lastError = null;
    });
  }

  // ---------- 봇 ----------
  /** 팀 봇 등록 (한 명만 하면 된다). 토큰이 맞는지 getMe 로 확인한 뒤 저장 */
  async setBot(token) {
    const client = this.makeClient(token);
    const me = await client.getMe();
    const { teamId, b } = this.ctx();
    await b.setIntegration(teamId, 'telegram', { botToken: client.token, botUsername: me.username });
    this.update((s) => (s.bot = { username: me.username }));
    return me.username;
  }

  async sync() {
    if (!this.team.isConnected()) return;
    const { teamId, b } = this.ctx();
    const bot = await b.getIntegration(teamId, 'telegram');
    const links = bot ? await b.listTelegramLinks(teamId) : [];
    this.token = bot ? bot.botToken : null;
    this.update((s) => {
      s.bot = bot ? { username: bot.botUsername } : null;
      s.links = links.map(({ uid, teamAlerts }) => ({ uid, teamAlerts })); // chatId 는 화면 캐시에 두지 않는다
    });
    this.chatIds = new Map(links.map((l) => [l.uid, l]));
    if (this.state().pending) await this.checkLink().catch(() => {});
  }

  client() {
    if (!this.token) throw new Error('팀 Telegram 봇이 아직 없어요. 설정 → 외부 연결에서 봇 토큰을 넣어 주세요.');
    return this.makeClient(this.token);
  }

  // ---------- 내 채팅 연결 ----------
  startLink() {
    const bot = this.state().bot;
    if (!bot) throw new Error('팀 Telegram 봇이 아직 없어요.');
    const code = `sai${randomCode(10, this.rand).toLowerCase()}`;
    this.update((s) => (s.pending = { code, at: this.now() }));
    return { code, url: `https://t.me/${bot.username}?start=${code}` };
  }

  /** 봇에 들어온 /start <코드> 를 찾아 내 채팅으로 연결 */
  async checkLink() {
    const p = this.state().pending;
    if (!p) return { linked: this.isLinked() };
    if (this.now() - p.at > LINK_TTL_MS) {
      this.update((s) => (s.pending = null));
      throw new Error('연결 코드가 만료됐어요. 다시 시작해 주세요.');
    }
    const found = findStartCode(await this.client().getUpdates(), p.code);
    if (!found) return { linked: false, waiting: true };
    const { teamId, b } = this.ctx();
    await b.setTelegramLink(teamId, { chatId: found.chatId, teamAlerts: true });
    this.chatIds = this.chatIds || new Map();
    this.chatIds.set(this.myUid(), { chatId: found.chatId, teamAlerts: true });
    this.update((s) => {
      s.pending = null;
      s.links = [...(s.links || []).filter((l) => l.uid !== this.myUid()), { uid: this.myUid(), teamAlerts: true }];
    });
    await this.client().sendMessage(found.chatId, notifyText('linked'));
    this.record(true, '연결 확인 메시지를 보냈어요.');
    return { linked: true };
  }

  isLinked() {
    return (this.state().links || []).some((l) => l.uid === this.myUid());
  }

  async setTeamAlerts(on) {
    const mine = this.chatIds && this.chatIds.get(this.myUid());
    if (!mine) throw new Error('먼저 Telegram 을 연결해 주세요.');
    const { teamId, b } = this.ctx();
    await b.setTelegramLink(teamId, { chatId: mine.chatId, teamAlerts: !!on });
    mine.teamAlerts = !!on;
    this.update((s) => (s.links = s.links.map((l) => (l.uid === this.myUid() ? { ...l, teamAlerts: !!on } : l))));
  }

  async unlink() {
    const { teamId, b } = this.ctx();
    await b.removeTelegramLink(teamId);
    if (this.chatIds) this.chatIds.delete(this.myUid());
    this.update((s) => (s.links = (s.links || []).filter((l) => l.uid !== this.myUid())));
  }

  // ---------- 보내기 ----------
  async sendTo(uids, kind, data, { requireTeamAlerts = true } = {}) {
    if (!this.token || !this.chatIds) return 0;
    const text = notifyText(kind, data);
    let sent = 0;
    for (const uid of new Set(uids)) {
      const l = this.chatIds.get(uid);
      if (!l || (requireTeamAlerts && !l.teamAlerts)) continue;
      try {
        await this.client().sendMessage(l.chatId, text);
        sent++;
      } catch (e) {
        this.record(false, String(e.message || e));
      }
    }
    if (sent) this.record(true, `${sent}명에게 알림을 보냈어요.`);
    return sent;
  }

  others() {
    return this.team.members({ includeMe: false }).map((m) => m.uid);
  }

  assigned(list, projectTitle) {
    return Promise.all(
      list
        .filter((a) => a.uid && a.uid !== this.myUid())
        .map((a) => this.sendTo([a.uid], 'assigned', { task: a.title, project: projectTitle, hours: a.hours })),
    );
  }

  blocked(task, projectTitle, who) {
    return this.sendTo(this.others(), 'blocked', { task, project: projectTitle, who });
  }

  proposal(text) {
    return this.sendTo(this.others(), 'proposal', { text });
  }

  /** 자리를 비웠을 때 내 진행 확인을 나에게 */
  nudgeMe(text, projectTitle) {
    return this.sendTo([this.myUid()], 'nudge', { text, project: projectTitle }, { requireTeamAlerts: false });
  }

  async test() {
    const n = await this.sendTo([this.myUid()], 'test', {}, { requireTeamAlerts: false });
    if (!n) throw new Error('보내지 못했어요. Telegram 연결을 먼저 해 주세요.');
    return n;
  }

  status() {
    const s = this.state();
    return {
      bot: s.bot,
      linked: this.isLinked(),
      teamAlerts: !!((s.links || []).find((l) => l.uid === this.myUid()) || {}).teamAlerts,
      linkedCount: (s.links || []).length,
      pending: !!s.pending,
      lastResult: s.lastResult,
      lastError: s.lastError,
    };
  }
}

module.exports = { Notifier };
