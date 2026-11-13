// Google OAuth 2.0 — 데스크톱 앱용 (루프백 주소 + PKCE)
// https://developers.google.com/identity/protocols/oauth2/native-app
const crypto = require('crypto');

const AUTH_URL = 'https://accounts.google.com/o/oauth2/v2/auth';
const TOKEN_URL = 'https://oauth2.googleapis.com/token';
const REVOKE_URL = 'https://oauth2.googleapis.com/revoke';

// 필요한 만큼만: 바쁜 시간(제목·내용은 못 봄), 사이가 만든 일정 추가, 할 일, 계정 이메일 표시
const SCOPES = [
  'openid',
  'email',
  'https://www.googleapis.com/auth/calendar.freebusy',
  'https://www.googleapis.com/auth/calendar.events',
  'https://www.googleapis.com/auth/tasks',
];

const b64url = (buf) => buf.toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');

function pkce() {
  const verifier = b64url(crypto.randomBytes(32));
  const challenge = b64url(crypto.createHash('sha256').update(verifier).digest());
  return { verifier, challenge };
}

function authUrl({ clientId, redirectUri, challenge, state, scopes = SCOPES }) {
  const q = new URLSearchParams({
    client_id: clientId,
    redirect_uri: redirectUri,
    response_type: 'code',
    scope: scopes.join(' '),
    code_challenge: challenge,
    code_challenge_method: 'S256',
    access_type: 'offline',
    prompt: 'consent',
    state,
  });
  return `${AUTH_URL}?${q}`;
}

async function postForm(fetchImpl, url, params) {
  const res = await fetchImpl(url, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams(params).toString(),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    const err = new Error(`Google 로그인 실패: ${data.error_description || data.error || res.status}`);
    err.code = data.error;
    throw err;
  }
  return data;
}

/** id_token 의 이메일만 꺼낸다 (화면 표시용 — 서명 검증은 하지 않으므로 권한 판단에 쓰지 않는다) */
function emailFromIdToken(idToken) {
  try {
    const payload = JSON.parse(Buffer.from(idToken.split('.')[1].replace(/-/g, '+').replace(/_/g, '/'), 'base64').toString('utf8'));
    return payload.email || null;
  } catch {
    return null;
  }
}

/** Google Cloud 콘솔에서 받은 클라이언트 JSON 또는 "ID / 비밀번호" 텍스트에서 값을 뽑는다 */
function parseGoogleClient(text) {
  const t = String(text || '');
  try {
    const j = JSON.parse(t);
    const c = j.installed || j.web || j;
    if (c.client_id && c.client_secret) return { clientId: c.client_id, clientSecret: c.client_secret };
  } catch {
    /* JSON 이 아니면 아래에서 */
  }
  const id = /([0-9]+-[a-z0-9]+\.apps\.googleusercontent\.com)/.exec(t);
  const secret = /(GOCSPX-[A-Za-z0-9_-]+)/.exec(t);
  return id && secret ? { clientId: id[1], clientSecret: secret[1] } : null;
}

class GoogleAuth {
  /**
   * client: { clientId, clientSecret }
   * loadTokens/saveTokens: refresh token 등을 암호화해 저장하는 함수
   */
  constructor({ client, loadTokens = () => null, saveTokens = () => {}, fetchImpl = fetch, now = () => Date.now() }) {
    this.client = client;
    this.loadTokens = loadTokens;
    this.saveTokens = saveTokens;
    this.fetch = fetchImpl;
    this.now = now;
    this.access = null;
  }

  tokens() {
    return this.loadTokens();
  }

  isSignedIn() {
    const t = this.tokens();
    return !!(t && t.refreshToken);
  }

  async exchangeCode({ code, verifier, redirectUri }) {
    const d = await postForm(this.fetch, TOKEN_URL, {
      code,
      client_id: this.client.clientId,
      client_secret: this.client.clientSecret,
      redirect_uri: redirectUri,
      grant_type: 'authorization_code',
      code_verifier: verifier,
    });
    if (!d.refresh_token) throw new Error('Google 이 오래 쓰는 로그인 정보를 주지 않았어요. 다시 로그인해 주세요.');
    const email = emailFromIdToken(d.id_token || '');
    this.saveTokens({ refreshToken: d.refresh_token, email, scope: d.scope });
    this.access = { token: d.access_token, exp: this.now() + (d.expires_in || 3600) * 1000 };
    return { email };
  }

  async accessToken() {
    if (this.access && this.now() < this.access.exp - 60000) return this.access.token;
    const t = this.tokens();
    if (!t || !t.refreshToken) throw new Error('Google 에 로그인되어 있지 않아요.');
    try {
      const d = await postForm(this.fetch, TOKEN_URL, {
        client_id: this.client.clientId,
        client_secret: this.client.clientSecret,
        refresh_token: t.refreshToken,
        grant_type: 'refresh_token',
      });
      this.access = { token: d.access_token, exp: this.now() + (d.expires_in || 3600) * 1000 };
      return this.access.token;
    } catch (e) {
      if (e.code === 'invalid_grant') {
        // 테스트 상태의 앱은 약 7일 뒤 로그인이 만료된다
        this.saveTokens(null);
        throw new Error('Google 로그인이 만료됐어요. 설정 → 외부 연결에서 다시 로그인해 주세요.');
      }
      throw e;
    }
  }

  /** Google API 호출 */
  async api(method, url, body) {
    const res = await this.fetch(url, {
      method,
      headers: { authorization: `Bearer ${await this.accessToken()}`, 'content-type': 'application/json' },
      body: body ? JSON.stringify(body) : undefined,
    });
    if (res.status === 204) return null;
    const data = await res.json().catch(() => null);
    if (!res.ok) {
      const msg = (data && data.error && (data.error.message || data.error)) || res.status;
      const err = new Error(`Google API 실패 (${res.status}): ${msg}`);
      err.status = res.status;
      throw err;
    }
    return data;
  }

  async signOut() {
    const t = this.tokens();
    this.saveTokens(null);
    this.access = null;
    if (t && t.refreshToken) {
      await this.fetch(`${REVOKE_URL}?token=${encodeURIComponent(t.refreshToken)}`, { method: 'POST' }).catch(() => {});
    }
  }
}

module.exports = { GoogleAuth, pkce, authUrl, emailFromIdToken, parseGoogleClient, SCOPES };
