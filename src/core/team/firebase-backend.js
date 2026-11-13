// Firebase(익명 로그인 + Firestore)를 REST로 쓰는 팀 백엔드.
// SDK 대신 REST를 쓰는 이유: Electron 메인 프로세스에서 의존성 없이 동작하고,
// 익명 계정의 refresh token 을 직접 암호화 저장해 앱을 다시 켜도 같은 팀원으로 남기 위해서다.
//
// 데이터 구조 (자세한 권한은 firestore.rules):
//   joinCodes/{code}              { teamId, teamName }
//   teams/{teamId}                { name, joinCode, createdBy }
//   teams/{teamId}/members/{uid}  { displayName, joinCode }
//   teams/{teamId}/signals/{id}   { topic, toMember, week, held }      — 보낸 사람 정보 없음
//   teams/{teamId}/proposals/{id} { topic, week, responses: {uid: 'agree'|'later'} }
// 문안(text)은 서버에 올리지 않는다. 받는 쪽이 자기 앱에 있는 주제별 일반 문안으로 말한다.
const { encodeFields, decodeDoc } = require('./codec');

const CODE_CHARS = 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789';

function randomCode(len = 6, rand = Math.random) {
  let s = '';
  for (let i = 0; i < len; i++) s += CODE_CHARS[Math.floor(rand() * CODE_CHARS.length)];
  return s;
}

class FirebaseBackend {
  /**
   * config: { apiKey, projectId, emulator?: { auth: 'http://127.0.0.1:9099', firestore: 'http://127.0.0.1:8080' } }
   * loadSession/saveSession: refresh token 등을 영구 저장하는 함수
   */
  constructor({ config, loadSession = () => null, saveSession = () => {}, fetchImpl = fetch }) {
    if (!config || !config.apiKey || !config.projectId) throw new Error('Firebase 설정(apiKey, projectId)이 필요해요.');
    this.config = config;
    this.loadSession = loadSession;
    this.saveSession = saveSession;
    this.fetch = fetchImpl;
    this.session = loadSession() || null; // { uid, refreshToken }
    this.idToken = null;
    this.idTokenExp = 0;
    const emu = config.emulator || {};
    this.authBase = emu.auth ? `${emu.auth}/identitytoolkit.googleapis.com/v1` : 'https://identitytoolkit.googleapis.com/v1';
    this.tokenUrl = emu.auth ? `${emu.auth}/securetoken.googleapis.com/v1/token` : 'https://securetoken.googleapis.com/v1/token';
    this.docsBase = `${emu.firestore || 'https://firestore.googleapis.com'}/v1/projects/${config.projectId}/databases/(default)/documents`;
  }

  get kind() {
    return 'firebase';
  }

  // ---------- 인증 ----------
  async signIn() {
    await this.token();
    return this.session.uid;
  }

  get uid() {
    return this.session && this.session.uid;
  }

  async token() {
    if (this.idToken && Date.now() < this.idTokenExp - 60000) return this.idToken;
    if (this.session && this.session.refreshToken) {
      const res = await this.fetch(`${this.tokenUrl}?key=${this.config.apiKey}`, {
        method: 'POST',
        headers: { 'content-type': 'application/x-www-form-urlencoded' },
        body: `grant_type=refresh_token&refresh_token=${encodeURIComponent(this.session.refreshToken)}`,
      });
      if (res.ok) {
        const d = await res.json();
        this.setToken(d.id_token, d.expires_in, d.user_id, d.refresh_token);
        return this.idToken;
      }
      // refresh token 이 무효하면 새 익명 계정을 만든다 (팀에는 다시 참여해야 함)
      this.session = null;
    }
    const res = await this.fetch(`${this.authBase}/accounts:signUp?key=${this.config.apiKey}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ returnSecureToken: true }),
    });
    if (!res.ok) throw new Error(`익명 로그인 실패 (${res.status}): ${await safeText(res)}`);
    const d = await res.json();
    this.setToken(d.idToken, d.expiresIn, d.localId, d.refreshToken);
    return this.idToken;
  }

  setToken(idToken, expiresIn, uid, refreshToken) {
    this.idToken = idToken;
    this.idTokenExp = Date.now() + Number(expiresIn || 3600) * 1000;
    const changed = !this.session || this.session.uid !== uid || this.session.refreshToken !== refreshToken;
    this.session = { uid, refreshToken };
    if (changed) this.saveSession(this.session);
  }

  // ---------- Firestore REST ----------
  async req(method, path, body, query = '') {
    const res = await this.fetch(`${this.docsBase}${path}${query}`, {
      method,
      headers: { 'content-type': 'application/json', authorization: `Bearer ${await this.token()}` },
      body: body ? JSON.stringify(body) : undefined,
    });
    if (res.status === 404) return null;
    if (!res.ok) {
      const err = new Error(`Firestore ${method} ${path} 실패 (${res.status}): ${await safeText(res)}`);
      err.status = res.status;
      throw err;
    }
    return res.status === 204 ? {} : res.json();
  }

  async createDoc(collectionPath, id, data) {
    const q = id ? `?documentId=${encodeURIComponent(id)}` : '';
    return decodeDoc(await this.req('POST', `/${collectionPath}`, { fields: encodeFields(data) }, q));
  }

  async getDoc(path) {
    return decodeDoc(await this.req('GET', `/${path}`));
  }

  async patchDoc(path, data, fieldPaths) {
    const mask = fieldPaths.map((f) => `updateMask.fieldPaths=${encodeURIComponent(f)}`).join('&');
    const q = `?${mask}&currentDocument.exists=true`;
    return decodeDoc(await this.req('PATCH', `/${path}`, { fields: encodeFields(data) }, q));
  }

  async deleteDoc(path) {
    await this.req('DELETE', `/${path}`);
  }

  async listDocs(collectionPath) {
    const out = [];
    let pageToken = '';
    do {
      const q = `?pageSize=100${pageToken ? `&pageToken=${encodeURIComponent(pageToken)}` : ''}`;
      const d = await this.req('GET', `/${collectionPath}`, null, q);
      for (const doc of (d && d.documents) || []) out.push(decodeDoc(doc));
      pageToken = d && d.nextPageToken;
    } while (pageToken);
    return out;
  }

  async queryEq(parentPath, collectionId, field, value) {
    const body = {
      structuredQuery: {
        from: [{ collectionId }],
        where: { fieldFilter: { field: { fieldPath: field }, op: 'EQUAL', value: { stringValue: value } } },
        limit: 200,
      },
    };
    const rows = (await this.req('POST', `/${parentPath}:runQuery`, body)) || [];
    return rows.filter((r) => r.document).map((r) => decodeDoc(r.document));
  }

  // ---------- 팀 ----------
  async createTeam({ name, displayName }) {
    const uid = await this.signIn();
    for (let attempt = 0; attempt < 5; attempt++) {
      const joinCode = randomCode();
      if (await this.getDoc(`joinCodes/${joinCode}`).catch(() => null)) continue;
      const team = await this.createDoc('teams', null, { name, joinCode, createdBy: uid });
      await this.createDoc('joinCodes', joinCode, { teamId: team.id, teamName: name });
      await this.createDoc(`teams/${team.id}/members`, uid, { displayName, joinCode });
      return { teamId: team.id, joinCode, name };
    }
    throw new Error('참여 코드를 만들지 못했어요. 다시 시도해 주세요.');
  }

  async joinTeam({ joinCode, displayName }) {
    const uid = await this.signIn();
    const code = String(joinCode || '').trim().toUpperCase();
    const ref = await this.getDoc(`joinCodes/${code}`);
    if (!ref) throw new Error('참여 코드를 찾을 수 없어요.');
    const existing = await this.getDoc(`teams/${ref.teamId}/members/${uid}`).catch(() => null);
    if (existing) await this.patchDoc(`teams/${ref.teamId}/members/${uid}`, { displayName }, ['displayName']);
    else await this.createDoc(`teams/${ref.teamId}/members`, uid, { displayName, joinCode: code });
    return { teamId: ref.teamId, joinCode: code, name: ref.teamName };
  }

  async leaveTeam(teamId) {
    const uid = await this.signIn();
    await this.deleteDoc(`teams/${teamId}/members/${uid}`);
  }

  async listMembers(teamId) {
    const docs = await this.listDocs(`teams/${teamId}/members`);
    return docs.map((d) => ({ uid: d.id, displayName: d.displayName }));
  }

  // ---------- 조언 주제 신호 ----------
  async createSignal(teamId, { topic, toMember, week }) {
    const d = await this.createDoc(`teams/${teamId}/signals`, null, { topic, toMember, week, held: false });
    return d.id;
  }

  async getSignal(teamId, id) {
    return this.getDoc(`teams/${teamId}/signals/${id}`);
  }

  async listSignalsForMe(teamId) {
    const uid = await this.signIn();
    return this.queryEq(`teams/${teamId}`, 'signals', 'toMember', uid);
  }

  async holdSignal(teamId, id) {
    await this.patchDoc(`teams/${teamId}/signals/${id}`, { held: true }, ['held']);
  }

  // ---------- 팀 약속 제안 ----------
  async createProposal(teamId, { topic, week }) {
    const d = await this.createDoc(`teams/${teamId}/proposals`, null, { topic, week, responses: {} });
    return d.id;
  }

  async listProposals(teamId) {
    return this.listDocs(`teams/${teamId}/proposals`);
  }

  async respondProposal(teamId, id, response) {
    const uid = await this.signIn();
    const path = `responses.\`${uid}\``;
    await this.patchDoc(`teams/${teamId}/proposals/${id}`, { responses: { [uid]: response } }, [path]);
  }
}

async function safeText(res) {
  try {
    return (await res.text()).slice(0, 300);
  } catch {
    return '';
  }
}

/** Firebase 콘솔에서 복사한 설정(JS 객체 또는 JSON)에서 apiKey, projectId 를 뽑는다 */
function parseFirebaseConfig(text) {
  const t = String(text || '');
  const get = (k) => {
    const m = new RegExp(`["']?${k}["']?\\s*:\\s*["']([^"']+)["']`).exec(t);
    return m ? m[1].trim() : null;
  };
  const apiKey = get('apiKey');
  const projectId = get('projectId');
  if (!apiKey || !projectId) return null;
  return { apiKey, projectId };
}

module.exports = { FirebaseBackend, parseFirebaseConfig, randomCode };
