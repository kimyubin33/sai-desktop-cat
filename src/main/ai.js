// 선택 기능: Claude API로 고양이의 말을 더 자연스럽게 다듬고, 정리 카드를 보강한다.
// API 키가 없거나 호출이 실패하면 규칙 기반 흐름(core/counseling.js)이 그대로 동작한다.
// 대화 단계·안전 확인·공유 범위 결정은 항상 이 앱의 규칙이 담당하고, AI는 문장과 정리 품질만 돕는다.
const { encryptSecret, decryptSecret } = require('./secret');
const { TOPICS } = require('../core/topics');

const API_URL = 'https://api.anthropic.com/v1/messages';
const TIMEOUT_MS = 60000;

const PERSONA = `당신은 팀원 각자의 바탕화면에 사는 고양이 "{name}"입니다. 지금은 한 사용자와 단둘이 비공개로 이야기하고 있습니다.
지켜야 할 원칙:
- 항상 부드러운 존댓말로 말하고, 질문은 한 번에 하나만 합니다. 보통은 짧게 말하되 필요하면 충분히 설명해도 됩니다.
- 사용자가 겪은 불편과 감정은 인정하되, 사용자 편만 들거나 상대를 비난하지 않습니다.
- 확인되지 않은 추측을 사실처럼 말하지 않고, 상대의 잘못을 확정하지 않습니다.
- 사람에 대한 평가(무능하다, 게으르다 등)는 관찰 가능한 행동과 필요한 변화로 바꿔 말합니다.
- 사용자의 말을 다른 사람에게 그대로 전하겠다고 약속하지 않습니다. 원문과 이름은 다른 팀원에게 가지 않습니다.
- 심리 치료 효과를 약속하지 않고, 진단하지 않습니다.
- 이모지, 마크다운, 목록 기호를 쓰지 않습니다.`;

const encryptKey = (key) => encryptSecret(key);
const decryptKey = (stored) => decryptSecret(stored);

async function callClaude({ apiKey, model, system, user, maxTokens = 400 }) {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), TIMEOUT_MS);
  try {
    const res = await fetch(API_URL, {
      method: 'POST',
      signal: ctrl.signal,
      headers: {
        'content-type': 'application/json',
        'x-api-key': apiKey,
        'anthropic-version': '2023-06-01',
      },
      body: JSON.stringify({
        model,
        max_tokens: maxTokens,
        system,
        messages: [{ role: 'user', content: user }],
      }),
    });
    if (!res.ok) {
      const body = await res.text().catch(() => '');
      throw new Error(`API ${res.status}: ${body.slice(0, 200)}`);
    }
    const data = await res.json();
    if (data.stop_reason === 'max_tokens') throw new Error('답이 너무 길어 중간에 끊겼어요.');
    return (data.content || [])
      .filter((b) => b.type === 'text')
      .map((b) => b.text)
      .join('')
      .trim();
  } finally {
    clearTimeout(timer);
  }
}

/** 도구(tool use)를 쓰는 호출. 응답 전체를 돌려준다 */
async function callClaudeMessages({ apiKey, model, system, messages, tools, maxTokens = 1200, timeoutMs = 45000 }) {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), timeoutMs);
  try {
    const res = await fetch(API_URL, {
      method: 'POST',
      signal: ctrl.signal,
      headers: { 'content-type': 'application/json', 'x-api-key': apiKey, 'anthropic-version': '2023-06-01' },
      body: JSON.stringify({ model, max_tokens: maxTokens, system, messages, tools }),
    });
    if (!res.ok) {
      const body = await res.text().catch(() => '');
      throw new Error(`API ${res.status}: ${body.slice(0, 200)}`);
    }
    return await res.json();
  } finally {
    clearTimeout(timer);
  }
}

/**
 * 스트리밍 호출: 글자가 오는 대로 onText(지금까지의 전체 글) 를 부른다.
 * 응답이 max_tokens 에서 끊기면 이어서 쓰게 해서 끝까지 받는다 (최대 maxContinue 번).
 * 60초 동안 아무것도 오지 않을 때만 멈춘다 (전체 시간 제한 없음).
 */
async function streamClaude({ apiKey, model, system, messages, maxTokens = 8192, onText = () => {}, idleMs = 60000, maxContinue = 4 }) {
  let full = '';
  for (let round = 0; round <= maxContinue; round++) {
    const msgs = full ? [...messages, { role: 'assistant', content: full }] : messages;
    const ctrl = new AbortController();
    let idle = setTimeout(() => ctrl.abort(), idleMs);
    const bump = () => {
      clearTimeout(idle);
      idle = setTimeout(() => ctrl.abort(), idleMs);
    };
    let stop = null;
    try {
      const res = await fetch(API_URL, {
        method: 'POST',
        signal: ctrl.signal,
        headers: { 'content-type': 'application/json', 'x-api-key': apiKey, 'anthropic-version': '2023-06-01' },
        body: JSON.stringify({ model, max_tokens: maxTokens, system, messages: msgs, stream: true }),
      });
      if (!res.ok) {
        const body = await res.text().catch(() => '');
        throw new Error(`API ${res.status}: ${body.slice(0, 200)}`);
      }
      const reader = res.body.getReader();
      const dec = new TextDecoder();
      let buf = '';
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        bump();
        buf += dec.decode(value, { stream: true });
        let i;
        while ((i = buf.indexOf('\n')) >= 0) {
          const line = buf.slice(0, i).trim();
          buf = buf.slice(i + 1);
          if (!line.startsWith('data:')) continue;
          let ev;
          try {
            ev = JSON.parse(line.slice(5));
          } catch {
            continue;
          }
          if (ev.type === 'content_block_delta' && ev.delta && ev.delta.type === 'text_delta') {
            full += ev.delta.text;
            onText(full);
          } else if (ev.type === 'message_delta' && ev.delta) {
            stop = ev.delta.stop_reason || stop;
          } else if (ev.type === 'error') {
            throw new Error(`API 오류: ${(ev.error && ev.error.message) || 'unknown'}`);
          }
        }
      }
    } catch (e) {
      if (e.name === 'AbortError') {
        if (full) return { text: full.trim(), stopReason: 'timeout' };
        throw new Error('AI가 60초 동안 응답하지 않았어요.');
      }
      throw e;
    } finally {
      clearTimeout(idle);
    }
    if (stop !== 'max_tokens') return { text: full.trim(), stopReason: stop };
    // 이어 쓰기: 끝의 공백은 API 가 받지 않는다
    full = full.replace(/\s+$/, '');
  }
  return { text: full.trim(), stopReason: 'max_tokens' };
}

const FREE_PERSONA = `당신은 사용자의 바탕화면에 사는 고양이 "{name}"입니다. Anthropic의 Claude를 바탕으로 동작하며, 사용자와 자유롭게 대화합니다.
- 일반 대화, 질문, 공부, 글쓰기, 고민 상담 등 무엇이든 성실하게 돕습니다. 필요한 만큼 충분히 길게 답해도 됩니다.
- 부드러운 존댓말을 쓰고, 고양이다운 따뜻함은 살짝만 드러냅니다.
- 팀원에 대한 이야기가 나오면 사용자 편만 들거나 상대를 단정해 비난하지 않고, 관찰 가능한 행동과 필요한 변화로 바꿔 생각하도록 돕습니다.
- 심리 치료 효과를 약속하거나 진단하지 않습니다. 위급해 보이면 전문 도움을 권합니다.
- 화면이 마크다운을 그리지 않으므로 **, #, 표 같은 기호 대신 줄바꿈과 "1." 같은 번호로 정리합니다.`;

function transcript(session, limit = 12) {
  return session.turns
    .filter((t) => !t.safety)
    .slice(-limit)
    .map((t) => `${t.role === 'user' ? '사용자' : '고양이'}: ${t.text}`)
    .join('\n');
}

class AiHelper {
  constructor(settingsStore) {
    this.settings = settingsStore;
    this.lastError = null;
  }

  config() {
    const s = this.settings.get();
    const apiKey = decryptKey(s.apiKeyEnc);
    return { enabled: !!(s.ai && s.ai.enabled && apiKey), apiKey, model: (s.ai && s.ai.model) || 'claude-sonnet-5-5', name: s.catName };
  }

  setKey(key) {
    this.settings.update((s) => {
      s.apiKeyEnc = encryptKey((key || '').trim());
    });
  }

  hasKey() {
    return !!decryptKey(this.settings.get().apiKeyEnc);
  }

  async test() {
    const c = this.config();
    if (!c.apiKey) return { ok: false, error: 'API 키가 없어요.' };
    try {
      const text = await callClaude({ ...c, system: '짧게 답하세요.', user: '"연결됐어요"라고만 답해 주세요.', maxTokens: 20 });
      return { ok: true, text };
    } catch (e) {
      return { ok: false, error: String(e.message || e) };
    }
  }

  /** "그냥 이야기할래요": 일반 Claude 대화처럼 전체 대화를 보고 길이 제한 없이 답한다 (스트리밍) */
  async freeChat(session, onText) {
    const c = this.config();
    if (!c.apiKey) return null;
    const msgs = [];
    for (const t of session.turns.filter((x) => !x.safety && !x.pending).slice(-200)) {
      const role = t.role === 'user' ? 'user' : 'assistant';
      if (msgs.length && msgs[msgs.length - 1].role === role) msgs[msgs.length - 1].content += `\n\n${t.text}`;
      else msgs.push({ role, content: t.text });
    }
    while (msgs.length && msgs[0].role !== 'user') msgs.shift();
    if (!msgs.length || msgs[msgs.length - 1].role !== 'user') return null;
    try {
      const r = await streamClaude({ apiKey: c.apiKey, model: c.model, system: FREE_PERSONA.replace('{name}', c.name), messages: msgs, onText });
      this.lastError = null;
      return r.text || null;
    } catch (e) {
      this.lastError = String(e.message || e);
      return null;
    }
  }

  /** 단계별 의도는 그대로 두고 문장만 자연스럽게 다듬는다. 실패하면 null */
  async polishReply(session, intent, fallback) {
    const c = this.config();
    if (!c.enabled || !intent) return null;
    const user =
      `지금까지의 대화:\n${transcript(session)}\n\n` +
      `이번에 고양이가 할 일: ${intent}\n` +
      `참고 문장(이 뜻을 유지하되 대화에 맞게 자연스럽게): ${fallback}\n\n` +
      `고양이의 다음 말만 출력하세요.`;
    try {
      const text = await callClaude({ ...c, system: PERSONA.replace('{name}', c.name), user, maxTokens: 1500 });
      this.lastError = null;
      return text || null;
    } catch (e) {
      this.lastError = String(e.message || e);
      return null;
    }
  }

  /** 규칙 기반 정리를 AI가 보강한다. 주제 id 는 정해진 목록 안에서만 받는다. */
  async enrichSummary(session, base) {
    const c = this.config();
    if (!c.enabled || base.mode !== 'complaint') return base;
    const topicList = Object.entries(TOPICS)
      .map(([id, t]) => `${id}: ${t.label}`)
      .join('\n');
    const user =
      `다음 비공개 대화를 정리해 JSON 하나만 출력하세요. 다른 말은 쓰지 마세요.\n\n${transcript(session, 20)}\n\n` +
      `형식:\n{"reportedEvent": "사용자가 보고한 사건(사용자 관점이라는 것이 드러나게)", "emotions": ["감정"], "impact": "실제 영향", ` +
      `"needs": "기대·필요", "actionRequests": ["상대에게 요청할 수 있는 구체적 행동"], ` +
      `"reframes": [{"from": "사람에 대한 평가", "to": "행동과 필요한 변화로 바꾼 표현"}], ` +
      `"toConfirm": ["아직 확인되지 않은 점"], "topics": ["아래 목록의 id"]}\n\n` +
      `규칙: 추측을 사실로 쓰지 말 것. 상대의 잘못을 확정하지 말 것. 욕설·이름은 옮기지 말 것.\n주제 목록:\n${topicList}`;
    try {
      const raw = await callClaude({ ...c, system: PERSONA.replace('{name}', c.name), user, maxTokens: 3000 });
      const json = JSON.parse(raw.slice(raw.indexOf('{'), raw.lastIndexOf('}') + 1));
      const merged = { ...base, aiAssisted: true };
      for (const k of ['reportedEvent', 'impact', 'needs']) if (typeof json[k] === 'string' && json[k].trim()) merged[k] = json[k].trim();
      for (const k of ['emotions', 'actionRequests', 'toConfirm'])
        if (Array.isArray(json[k]) && json[k].length) merged[k] = json[k].filter((x) => typeof x === 'string').slice(0, 6);
      if (Array.isArray(json.reframes))
        merged.reframes = json.reframes.filter((r) => r && r.from && r.to).slice(0, 4);
      if (Array.isArray(json.topics)) {
        const valid = json.topics.filter((id) => TOPICS[id]);
        merged.topics = [...new Set([...base.topics, ...valid])];
      }
      this.lastError = null;
      return merged;
    } catch (e) {
      this.lastError = String(e.message || e);
      return base;
    }
  }
}

module.exports = { AiHelper, encryptKey, decryptKey, callClaudeMessages, streamClaude };
