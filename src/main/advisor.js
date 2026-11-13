// 말풍선 조언을 실제 고양이 창과 연결한다.
const { LIBRARY, sourceLabel } = require('../core/advice-library');
const { silenceReason, nextDelayMs, pickAdvice, rememberShown } = require('../core/advice-scheduler');

const LATER_MS = 20 * 60 * 1000;
const BUSY_RETRY_MS = 60 * 1000;
const PROPOSAL_REPEAT_MS = 6 * 60 * 60 * 1000;

class Advisor {
  /**
   * team: 팀 연결 (없으면 null)
   *   { isConnected(), boosts(), pendingProposalsForMe(), respond(id, r) }
   * onExplain(topic): "내 사정도 있어요" → 대화창의 내 사정 이야기로
   */
  constructor({ stores, cat, onChat, onExplain = () => {}, team = null, collab = null, onCollab = () => {}, notifier = null, isAway = () => false }) {
    this.notifier = notifier;
    this.isAway = isAway;
    this.collab = collab;
    this.onCollab = onCollab;
    this.stores = stores;
    this.cat = cat;
    this.onChat = onChat;
    this.onExplain = onExplain;
    this.team = team;
    this.timer = null;
    this.laterTimer = null;
    this.shown = new Map(); // bubble id → entry
    this.seq = 0;
  }

  start() {
    const s = this.stores.settings.get();
    if (!s.onboarded) {
      setTimeout(() => this.greet(), 2500);
    }
    this.schedule();
  }

  schedule(delay) {
    clearTimeout(this.timer);
    const s = this.stores.settings.get();
    const ms = delay ?? nextDelayMs(s);
    this.timer = setTimeout(() => this.onTimer(), ms);
  }

  onTimer() {
    const s = this.stores.settings.get();
    if (silenceReason(s)) return this.schedule();
    if (this.cat.isBusy() || !this.cat.isVisible()) return this.schedule(BUSY_RETRY_MS);
    if (!this.maybeShowProposal() && !this.maybeShowProgress()) this.speakNow();
    this.schedule();
  }

  greet() {
    const name = this.stores.settings.get().catName;
    this.show({
      id: 'greet',
      kind: 'system',
      topic: null,
      text: `안녕하세요, 저는 ${name}예요. 가끔 말을 걸게요. 저를 클릭하면 편하게 이야기할 수 있고, 끌어서 옮길 수도 있어요.`,
      source: null,
    });
  }

  teamConnected() {
    return !!(this.team && this.team.isConnected());
  }

  /** 사용자가 메뉴에서 직접 요청했을 때는 조용한 모드여도 말한다 */
  speakNow() {
    const s = this.stores.settings.get();
    const hist = this.stores.adviceHistory;
    const entry = pickAdvice(LIBRARY, {
      recent: hist.get().recent,
      // 주제 줄이기는 없앴다 (업무 조언을 하나씩 꺼 버리면 결국 다 꺼지기 때문)
      boosts: this.teamConnected() ? this.team.boosts() : {},
    });
    hist.update((h) => {
      h.recent = rememberShown(h.recent, entry.id);
    });
    this.show(entry);
  }

  /** 아직 답하지 않은 팀 약속 제안이 있으면 (같은 제안은 6시간에 한 번만) 말풍선으로 묻는다 */
  maybeShowProposal() {
    if (!this.teamConnected()) return false;
    const cache = this.stores.teamCache;
    const shown = cache.get().shownProposals || {};
    const now = Date.now();
    const p = this.team.pendingProposalsForMe().find((x) => !shown[x.id] || now - shown[x.id] > PROPOSAL_REPEAT_MS);
    if (!p) return false;
    cache.update((c) => {
      c.shownProposals = { ...(c.shownProposals || {}), [p.id]: now };
    });
    this.show({
      id: `proposal-${p.id}`,
      kind: 'proposal',
      proposalId: p.id,
      topic: null,
      text: `팀에 이런 약속을 맞춰 보자는 제안이 있어요. "${p.text}"`,
      source: { type: 'team', label: `${p.teamName ? `${p.teamName} · ` : ''}팀 약속 제안 · 지금 ${p.agree}/${p.memberCount}명 동의` },
    });
    return true;
  }

  /** 업무 진행 확인 (배정 확인, 시작, 막힘, 준비물, 마감) */
  maybeShowProgress() {
    if (!this.collab || !this.teamConnected()) return false;
    let n = null;
    try {
      n = this.collab.nudge();
    } catch {
      return false;
    }
    if (!n) return false;
    this.collab.markNudged(n.key);
    // 자리를 비웠으면 말풍선 대신 Telegram 으로 (연결했고, 설정에서 켜 둔 경우)
    if (this.isAway() && this.notifier && this.notifier.isLinked() && this.stores.settings.get().telegramAway) {
      const project = this.collab.active();
      this.notifier.nudgeMe(n.text, project ? project.title : '').catch(() => {});
      return true;
    }
    this.show({
      id: `progress-${n.key}`,
      kind: 'progress',
      taskId: n.taskId,
      actions: n.actions,
      topic: null,
      text: n.text,
      source: { type: 'collab', label: n.source },
    });
    return true;
  }

  actionsFor(entry) {
    if (entry.kind === 'progress') return entry.actions;
    if (entry.kind === 'proposal') {
      return [
        { id: 'later', label: '나중에' },
        { id: 'agree', label: '좋아요', primary: true },
      ];
    }
    if (entry.kind === 'system') {
      return [
        { id: 'later', label: '나중에' },
        { id: 'chat', label: '이야기하기', primary: true },
      ];
    }
    const actions = [{ id: 'later', label: '나중에' }];
    // 받은 조언인지 아닌지 드러나지 않도록, 팀에 연결되어 있으면 모든 협업 조언에 같은 버튼을 단다
    if (entry.kind === 'teamwork' && this.teamConnected()) actions.push({ id: 'mine', label: '내 사정도 있어요' });
    actions.push({ id: 'chat', label: '이야기하기', primary: true });
    return actions;
  }

  show(entry, text) {
    const id = `b${++this.seq}`;
    this.shown.set(id, entry);
    return this.cat.showBubble({
      id,
      text: text || entry.text,
      sourceLabel: entry.source ? sourceLabel(entry) : '',
      topic: entry.topic,
      kind: entry.kind,
      actions: this.actionsFor(entry),
    });
  }

  /** 대화창 등 다른 곳에서 고양이가 직접 한마디 하게 할 때 */
  say(text) {
    this.show({ id: 'say', kind: 'system', topic: null, source: null }, text);
  }

  handleAction(id, action) {
    const entry = this.shown.get(id);
    this.shown.delete(id);
    if (!entry) return;
    if (entry.id === 'greet') {
      this.stores.settings.update((s) => {
        s.onboarded = true;
      });
    }
    switch (action) {
      case 'later':
        clearTimeout(this.laterTimer);
        this.laterTimer = setTimeout(() => {
          if (!silenceReason(this.stores.settings.get()) && !this.cat.isBusy()) this.show(entry);
        }, LATER_MS);
        break;
      case 'less':
        if (entry.topic) {
          this.stores.settings.update((s) => {
            s.dampenedTopics[entry.topic] = (s.dampenedTopics[entry.topic] || 0) + 1;
          });
        }
        break;
      case 'mine':
        if (entry.topic) this.onExplain(entry.topic);
        break;
      case 'agree':
        if (entry.proposalId && this.team) this.team.respond(entry.proposalId, 'agree').catch(() => {});
        break;
      case 'chat':
        this.onChat();
        break;
      case 'open-collab':
      case 'task-object':
        this.onCollab(entry.taskId || null);
        break;
      case 'task-accept':
        this.runTask(() => this.collab.accept(entry.taskId), '고마워요. 시작할 수 있을 때 알려 드릴게요.');
        break;
      case 'task-start':
        this.runTask(() => this.collab.setStatus(entry.taskId, 'doing'), '좋아요. 막히면 언제든 말해 주세요.');
        break;
      case 'task-blocked':
        this.runTask(() => this.collab.setStatus(entry.taskId, 'blocked'), '팀에 도움이 필요하다고 알려 둘게요. 어디서 막혔는지 협업 탭에 적어 두면 더 빨라요.');
        break;
      case 'task-done':
        this.runTask(() => this.collab.setStatus(entry.taskId, 'done'), '수고했어요! 완료로 표시해 둘게요.');
        break;
      case 'task-ok':
        break;
      default:
        break;
    }
  }

  runTask(fn, thanks) {
    if (!this.collab) return;
    fn()
      .then(() => setTimeout(() => this.say(thanks), 400))
      .catch((e) => setTimeout(() => this.say(`처리하지 못했어요: ${e.message}`), 400));
  }

  stop() {
    clearTimeout(this.timer);
    clearTimeout(this.laterTimer);
  }
}

module.exports = { Advisor };
