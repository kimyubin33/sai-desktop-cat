// 개인 대화 세션을 관리한다. 대화 원문은 private/conversations.json 에만 저장된다.
const { createSession, handleUser, buildSummary, MODES } = require('../core/counseling');

const DAY = 24 * 60 * 60 * 1000;

class ChatService {
  constructor({ stores, ai }) {
    this.stores = stores;
    this.ai = ai;
    this.purgeOld();
  }

  sessions() {
    return this.stores.privateConv.get().sessions;
  }

  find(id) {
    return this.sessions().find((s) => s.id === id);
  }

  save(session) {
    this.stores.privateConv.update((d) => {
      const i = d.sessions.findIndex((s) => s.id === session.id);
      if (i >= 0) d.sessions[i] = session;
      else d.sessions.push(session);
    });
  }

  purgeOld() {
    const days = this.stores.settings.get().retentionDays;
    if (!days) return;
    const cutoff = Date.now() - days * DAY;
    this.stores.privateConv.update((d) => {
      d.sessions = d.sessions.filter((s) => (s.turns.at(-1)?.at || s.startedAt) >= cutoff);
    });
  }

  async polish(session, replies) {
    const out = [];
    for (const r of replies) {
      if (r.fixed || !r.intent) {
        out.push(r);
        continue;
      }
      const better = await this.ai.polishReply(session, r.intent, r.text);
      if (better) {
        // 세션 기록의 마지막 고양이 말도 다듬은 문장으로 바꾼다
        const last = [...session.turns].reverse().find((t) => t.role === 'cat' && t.text === r.text);
        if (last) last.text = better;
        out.push({ ...r, text: better, ai: true });
      } else out.push(r);
    }
    return out;
  }

  async start(mode, topic = null) {
    const { session, replies } = createSession(mode, { topic });
    const polished = await this.polish(session, replies);
    this.save(session);
    return { session: this.publicView(session), replies: polished };
  }

  async send(id, text, skip = false, onText = null) {
    const session = this.find(id);
    if (!session) throw new Error('대화를 찾을 수 없어요.');
    const before = session.turns.length;
    const result = handleUser(session, text, { skip });
    let replies;
    if (result.safety) replies = result.replies;
    else if (session.mode === 'free' && this.ai.hasKey()) {
      // 자유 대화는 정해진 흐름 대신 Claude 와 그대로 이야기한다 (안전 확인은 위에서 먼저 한다)
      const ruleTurns = session.turns.splice(before).filter((t) => t.role === 'user');
      session.turns.push(...ruleTurns);
      const answer = await this.ai.freeChat(session, onText || (() => {}));
      if (answer) {
        session.turns.push({ role: 'cat', text: answer, at: Date.now(), ai: true });
        replies = [{ role: 'cat', text: answer, ai: true, streamed: !!onText }];
      } else {
        const fallback = result.replies.map((r) => ({ ...r }));
        for (const r of fallback) session.turns.push({ role: 'cat', text: r.text, at: Date.now() });
        const why = this.ai.lastError ? ` (AI 연결 오류: ${this.ai.lastError.slice(0, 120)})` : '';
        replies = fallback.map((r, i) => (i === fallback.length - 1 ? { ...r, text: r.text + why } : r));
      }
    } else replies = await this.polish(session, result.replies);
    if (result.summary) {
      session.summary = await this.ai.enrichSummary(session, result.summary);
    }
    this.save(session);
    return {
      session: this.publicView(session),
      replies,
      summary: result.summary ? session.summary : null,
      safety: result.safety || null,
    };
  }

  /** 사용자가 정리 카드를 직접 고친 경우 */
  updateSummary(id, patch) {
    const session = this.find(id);
    if (!session || !session.summary) return null;
    const allowed = ['reportedEvent', 'agreement', 'impact', 'needs', 'actionRequests', 'toConfirm', 'topics', 'situation', 'reason', 'nextTime'];
    for (const k of allowed) if (k in patch) session.summary[k] = patch[k];
    session.summary.editedByUser = true;
    this.save(session);
    return session.summary;
  }

  summarizeNow(id) {
    const session = this.find(id);
    if (!session || session.mode === 'free') return null;
    session.step = 'done';
    session.summary = buildSummary(session);
    this.save(session);
    return session.summary;
  }

  list() {
    return this.sessions()
      .map((s) => ({
        id: s.id,
        mode: s.mode,
        modeLabel: MODES[s.mode]?.label,
        startedAt: s.startedAt,
        preview: (s.turns.find((t) => t.role === 'user')?.text || '(아직 이야기 없음)').slice(0, 40),
        hasSummary: !!s.summary,
        safetyHold: !!s.safetyHold,
      }))
      .sort((a, b) => b.startedAt - a.startedAt);
  }

  get(id) {
    const s = this.find(id);
    return s ? this.publicView(s, true) : null;
  }

  publicView(s, withTurns = false) {
    return {
      id: s.id,
      mode: s.mode,
      step: s.step,
      startedAt: s.startedAt,
      summary: s.summary,
      safetyHold: s.safetyHold,
      turns: withTurns ? s.turns : undefined,
    };
  }

  delete(id) {
    this.stores.privateConv.update((d) => {
      d.sessions = d.sessions.filter((s) => s.id !== id);
    });
  }

  deleteAll() {
    this.stores.privateConv.update((d) => {
      d.sessions = [];
    });
  }
}

module.exports = { ChatService };
