// 말풍선 조언을 실제 고양이 창과 연결한다.
const { LIBRARY, sourceLabel } = require('../core/advice-library');
const { silenceReason, nextDelayMs, pickAdvice, rememberShown } = require('../core/advice-scheduler');

const LATER_MS = 20 * 60 * 1000;
const BUSY_RETRY_MS = 60 * 1000;

class Advisor {
  constructor({ stores, cat, onChat, getBoosts = () => ({}) }) {
    this.stores = stores;
    this.cat = cat;
    this.onChat = onChat;
    this.getBoosts = getBoosts;
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
    this.speakNow();
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

  /** 사용자가 메뉴에서 직접 요청했을 때는 조용한 모드여도 말한다 */
  speakNow() {
    const s = this.stores.settings.get();
    const hist = this.stores.adviceHistory;
    const entry = pickAdvice(LIBRARY, {
      recent: hist.get().recent,
      dampened: s.dampenedTopics,
      boosts: this.getBoosts(),
    });
    hist.update((h) => {
      h.recent = rememberShown(h.recent, entry.id);
    });
    this.show(entry);
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
      case 'chat':
        this.onChat();
        break;
      default:
        break;
    }
  }

  stop() {
    clearTimeout(this.timer);
    clearTimeout(this.laterTimer);
  }
}

module.exports = { Advisor };
