// Telegram 봇 API (https://core.telegram.org/bots/api) 를 쓰는 최소 클라이언트와 알림 문안.
//
// 원칙: Telegram 으로는 "업무" 알림만 보낸다 — 배정, 도움 요청, 팀 약속 제안, 내 작업 진행 확인.
// 개인 대화, 정리 카드, 받은 조언 주제처럼 상담에서 나온 내용은 어떤 경우에도 보내지 않는다.
// (Telegram 서버를 거치는 순간 이 앱의 개인정보 분리 원칙 밖으로 나가기 때문)

const API = 'https://api.telegram.org';

class TelegramClient {
  constructor({ token, fetchImpl = fetch }) {
    if (!/^\d+:[\w-]{20,}$/.test(String(token || '').trim())) throw new Error('봇 토큰 형식이 아니에요. @BotFather 가 준 "숫자:문자" 형태의 토큰을 넣어 주세요.');
    this.token = String(token).trim();
    this.fetch = fetchImpl;
  }

  async call(method, body) {
    const res = await this.fetch(`${API}/bot${this.token}/${method}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body || {}),
    });
    let data = null;
    try {
      data = await res.json();
    } catch {
      /* 아래에서 처리 */
    }
    if (!data || !data.ok) {
      const desc = (data && data.description) || `HTTP ${res.status}`;
      const err = new Error(`Telegram ${method} 실패: ${desc}`);
      err.code = data && data.error_code;
      throw err;
    }
    return data.result;
  }

  getMe() {
    return this.call('getMe');
  }

  sendMessage(chatId, text) {
    return this.call('sendMessage', { chat_id: chatId, text, disable_web_page_preview: true });
  }

  /**
   * 대기 중인 메시지를 읽기만 한다 (offset 을 넘기지 않아 다른 팀원 앱의 연결 코드도 지워지지 않음).
   * Telegram 은 확인되지 않은 업데이트를 최대 24시간 보관한다.
   */
  getUpdates() {
    return this.call('getUpdates', { allowed_updates: ['message'], timeout: 0, limit: 100 });
  }
}

/** /start <코드> 메시지를 찾아 그 채팅을 돌려준다 */
function findStartCode(updates, code) {
  for (const u of updates || []) {
    const m = u.message;
    if (!m || !m.text || !m.chat) continue;
    const parts = m.text.trim().split(/\s+/);
    if (parts[0].startsWith('/start') && parts[1] === code) {
      return { chatId: m.chat.id, name: m.chat.first_name || m.chat.username || '' };
    }
  }
  return null;
}

// ---------- 알림 문안 (업무 알림만 허용) ----------
const KINDS = {
  linked: () => '사이와 연결됐어요. 팀 업무 알림과, 자리를 비웠을 때의 진행 확인을 여기로 보내 드릴게요. 개인 대화 내용은 보내지 않아요.',
  assigned: ({ task, project, hours }) =>
    `[사이 · ${project}] "${task}" 배정 제안이 왔어요 (약 ${hours}시간). 사이 앱의 협업 탭에서 맡거나 이의를 남겨 주세요.`,
  blocked: ({ task, project, who }) =>
    `[사이 · ${project}] ${who}님이 "${task}"에서 도움이 필요하대요. 같이 볼 수 있다면 한마디 건네 주세요.`,
  proposal: ({ text }) => `[사이] 팀에 약속을 맞춰 보자는 제안이 있어요: "${text}" 사이 앱에서 동의하거나 나중에 볼 수 있어요.`,
  nudge: ({ text, project }) => `[사이 · ${project}] ${text}`,
  test: () => '사이 알림 시험이에요. 이 메시지가 보이면 연결이 잘 된 거예요.',
};

function notifyText(kind, data = {}) {
  const f = KINDS[kind];
  if (!f) throw new Error(`보낼 수 없는 알림 종류예요: ${kind}`);
  return f(data);
}

module.exports = { TelegramClient, findStartCode, notifyText, NOTIFY_KINDS: Object.keys(KINDS) };
