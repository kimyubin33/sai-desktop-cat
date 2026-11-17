import test from 'node:test';
import assert from 'node:assert/strict';
import { handle, BudgetLedger, validateResult } from './gateway.mjs';
const ACCESS = 'GOOGLE_ACCESS_TEST_ONLY_1234567890';
const KEY = 'CLAUDE_KEY_TEST_ONLY_1234567890';
const SOURCE = '기획서를 검토하고 의견을 남겨 주세요.';
const asJson = v => new Response(JSON.stringify(v), { headers: { 'content-type': 'application/json' } });
class Storage {
  data = new Map(); alarm = null; tail = Promise.resolve();
  async get(k) { return structuredClone(this.data.get(k)); }
  async put(k, v) { this.data.set(k, structuredClone(v)); }
  async delete(k) { for (const key of Array.isArray(k) ? k : [k]) this.data.delete(key); }
  async list({ prefix }) { return new Map([...this.data].filter(([k]) => k.startsWith(prefix)).map(([k, v]) => [k, structuredClone(v)])); }
  async getAlarm() { return this.alarm; }
  async setAlarm(at) { this.alarm = at; }
  transaction(f) {
    const next = this.tail.then(async () => { const backup = structuredClone(this.data); try { return await f(this); } catch (e) { this.data = backup; throw e; } });
    this.tail = next.catch(() => {}); return next;
  }
}
function fixture() {
  let clock = Date.parse('2026-11-17T23:59:00Z');
  const stores = new Map(); const objects = new Map();
  const f = { calls: 0, googleCalls: 0, caller: 'member', owner: 'owner', member: true, revoked: false, revokedAfter: false, unknown: false, truncated: false, badEvidence: false, upstreamError: false };
  const policy = { version: 1, dailyRequests: 20, teams: { 'team-folder': { ownerPermissionId: 'owner', model: 'claude-test-model', dailyRequests: 5, maxOutputTokens: 1024 } } };
  const env = { TEAM_AI_POLICY: JSON.stringify(policy), AI_HMAC_KEY: 'HMAC_TEST_ONLY_123456789012345678901234567890', ANTHROPIC_API_KEY: KEY, BUDGETS: {
    idFromName: n => n,
    get: n => { if (!stores.has(n)) stores.set(n, new Storage()); if (!objects.has(n)) objects.set(n, new BudgetLedger(stores.get(n), () => clock)); return { fetch: (u, init) => objects.get(n).fetch(new Request(u, init)) }; },
  } };
  const send = async (u, init) => {
    if (u.startsWith('https://api.anthropic.com/')) {
      f.calls++; assert.equal(u, 'https://api.anthropic.com/v1/messages'); assert.equal(init.headers.authorization, `Bearer ${KEY}`); assert.equal(init.headers['anthropic-version'],'2026-10-21'); assert.ok(!JSON.stringify(init).includes(ACCESS)); assert.equal(init.redirect, 'manual');
      const b = JSON.parse(init.body); assert.equal(b.model, policy.teams['team-folder'].model); assert.equal(b.max_tokens, 1024); assert.equal(b.messages[0].content, SOURCE); assert.equal(b.tools, undefined); assert.equal(b.messages.length, 1);
      if (f.unknown) throw new Error('lost after sending');
      if (f.upstreamError) return new Response(KEY, { status: 500 });
      const output = { summary: '기획서 검토 요청', candidates: [{ title: '기획서 검토', assignee: '', dueDate: '', evidence: f.badEvidence ? 'invented quote' : SOURCE, uncertain: true }] };
      return asJson({ stop_reason: f.truncated ? 'max_tokens' : 'end_turn', content: [{ type: 'text', text: JSON.stringify(output) }] });
    }
    f.googleCalls++; assert.equal(init.headers.authorization, `Bearer ${ACCESS}`); assert.equal(init.headers['x-api-key'], undefined); assert.equal(init.redirect, 'manual');
    if (f.revoked || f.revokedAfter && f.calls > 0) return new Response('{}', { status: 403 });
    if (u === 'https://openidconnect.googleapis.com/v1/userinfo') return asJson({ sub: 'google-sub-a', email_verified: true });
    if (u === 'https://www.googleapis.com/drive/v3/about?fields=user(permissionId)') return asJson({ user: { permissionId: f.caller } });
    if (u.includes('/permissions?')) return asJson({ permissions: f.member ? [{ id: f.caller, type: 'user', role: f.caller === 'owner' ? 'owner' : 'reader' }] : [{ id: 'anyoneWithLink', type: 'anyone', role: 'reader' }] });
    if (u.startsWith('https://www.googleapis.com/drive/v3/files/team-folder?')) return asJson({ id: 'team-folder', mimeType: 'application/vnd.google-apps.folder', trashed: false, properties: { saiTeam: '1' }, owners: [{ permissionId: f.owner }], capabilities: { canListChildren: true } });
    throw new Error('Unexpected outbound URL');
  };
  const request = (path = '/v1/analyze', extra = {}, headers = {}) => new Request(`https://gateway.test${path}`, { method: 'POST', headers: { authorization: `Bearer ${ACCESS}`, 'content-type': 'application/json', ...headers }, body: JSON.stringify(path === '/v1/status' ? { folderId: 'team-folder', ...extra } : { folderId: 'team-folder', requestId: 'r'.repeat(40), createdAt: clock, text: SOURCE, ...extra }) });
  return { f, env, policy, stores, objects, send, request, now: () => clock, advance: n => { clock += n; }, run: (r) => handle(r, env, send, () => clock) };
}
test('an explicit Google member receives validated proposals and no credentials', async () => {
  const x = fixture(); const response = await x.run(x.request()); assert.equal(response.status, 200); const body = await response.text(); assert.ok(body.includes('기획서 검토')); assert.ok(!body.includes(KEY)); assert.ok(!body.includes(ACCESS)); assert.equal(x.f.calls, 1);
  const saved = JSON.stringify([...x.stores.values()].map(s => [...s.data])); for (const secret of [SOURCE, KEY, ACCESS, 'google-sub-a']) assert.ok(!saved.includes(secret));
});
test('missing login, injected roles and arbitrary teams cannot cause paid calls', async () => {
  for (const scenario of [x => x.request('/v1/analyze', {}, { authorization: '' }), x => x.request('/v1/analyze', { subject: 'owner', owner: true }), x => x.request('/v1/analyze', { folderId: 'not-enabled' })]) { const x = fixture(); assert.notEqual((await x.run(scenario(x))).status, 200); assert.equal(x.f.calls, 0); }
  const x=fixture();assert.equal((await x.run(new Request('http://gateway.test/v1/status',{method:'POST',body:'{}'}))).status,400);assert.equal(x.f.googleCalls,0);
});
test('public-link access is not explicit team membership; transferred ownership disables AI', async () => {
  for (const change of [f => { f.member = false; }, f => { f.owner = 'new-owner'; }, f => { f.revoked = true; }]) { const x = fixture(); change(x.f); assert.equal((await x.run(x.request())).status, 403); assert.equal(x.f.calls, 0); }
});
test('status derives owner role from Google, exposes only configuration and counts', async () => {
  const x = fixture(); let v = await (await x.run(x.request('/v1/status'))).json(); assert.equal(v.owner, false); assert.equal(v.ready, true); assert.equal(v.used, 0); x.f.caller = 'owner'; v = await (await x.run(x.request('/v1/status'))).json(); assert.equal(v.owner, true); assert.equal(x.f.calls, 0);
});
test('parallel duplicates and worker restarts never repeat a charged request', async () => {
  const x = fixture(); const results = await Promise.all(Array.from({ length: 8 }, () => x.run(x.request()))); assert.equal(results.filter(r => r.status === 200).length, 1); assert.equal(x.f.calls, 1);
  x.objects.clear(); assert.equal((await x.run(x.request())).status, 409); assert.equal(x.f.calls, 1);
});
test('team and global caps hold for parallel distinct requests', async () => {
  for (const teamLimit of [true, false]) { const x = fixture(); if (teamLimit) x.policy.teams['team-folder'].dailyRequests = 1; else x.policy.dailyRequests = 1; x.env.TEAM_AI_POLICY = JSON.stringify(x.policy);
    const results = await Promise.all(Array.from({ length: 8 }, (_, i) => x.run(x.request('/v1/analyze', { requestId: `${i}`.repeat(40) })))); assert.equal(results.filter(r => r.status === 200).length, 1); assert.equal(x.f.calls, 1); assert.ok(results.some(r => r.status === 429));
  }
});
test('uncertain upstream response is never retried with the same request ID', async () => {
  const x = fixture(); x.f.unknown = true; assert.equal((await x.run(x.request())).status, 503); x.f.unknown = false; assert.equal((await x.run(x.request())).status, 409); assert.equal(x.f.calls, 1);
});
test('changed source under the same ID is rejected and upstream error bodies stay private', async () => {
  const x = fixture(); x.f.upstreamError = true; const r = await x.run(x.request()); assert.equal(r.status, 502); assert.ok(!(await r.text()).includes(KEY)); const next = await x.run(x.request('/v1/analyze', { text: 'different' })); assert.equal(next.status, 409); assert.equal(x.f.calls, 1);
});
test('revocation after processing hides the result', async () => { const x = fixture(); x.f.revokedAfter = true; const r = await x.run(x.request()); assert.equal(r.status, 403); assert.ok(!(await r.text()).includes('기획서 검토')); assert.equal(x.f.calls, 1); });
test('truncated output and invented evidence are not successful results', async () => { for (const key of ['truncated', 'badEvidence']) { const x = fixture(); x.f[key] = true; assert.equal((await x.run(x.request())).status, 502); } });
test('large or old requests are rejected before provider use', async () => {
  for (const extra of [{ text: 'a'.repeat(24577) }, { createdAt: 1 }, { text: '\u0000' }, { requestId: 'short' }]) { const x = fixture(); assert.equal((await x.run(x.request('/v1/analyze', extra))).status, 400); assert.equal(x.f.calls, 0); }
});
test('unconfigured deployments fail closed without a provider request', async () => { const x = fixture(); delete x.env.TEAM_AI_POLICY; assert.equal((await x.run(x.request())).status, 503); assert.equal(x.f.calls, 0); });
test('UTC rollover resets limits but does not permit replay of an earlier attempt', async () => {
  const x = fixture(); x.policy.teams['team-folder'].dailyRequests = 1; x.env.TEAM_AI_POLICY = JSON.stringify(x.policy); const old = x.now(); assert.equal((await x.run(x.request())).status, 200); x.advance(120000);
  assert.equal((await x.run(x.request('/v1/analyze', { createdAt: old }))).status, 409); assert.equal((await x.run(x.request('/v1/analyze', { requestId: 'new'.repeat(14) }))).status, 200); assert.equal(x.f.calls, 2);
});
test('quota alarms are not postponed by status traffic and expire old metadata', async () => {
  const x = fixture(); await x.run(x.request()); const storage = x.stores.get('global'); const alarm = storage.alarm; x.advance(30000); await x.objects.get('global').fetch(new Request('https://budget/status', { method: 'POST', body: '{}' })); assert.equal(storage.alarm, alarm);
  x.advance(32 * 86400000); await x.objects.get('global').alarm(); assert.equal(storage.data.size, 0);
});
test('result schema rejects invalid dates and unsolicited actions', () => {
  const c = { title: 'task', assignee: '', dueDate: '2026-02-30', evidence: SOURCE, uncertain: true }; assert.throws(() => validateResult({ summary: 'x', candidates: [c] }, SOURCE)); c.dueDate = ''; c.evidence=SOURCE+'\nSecond line';assert.equal(validateResult({summary:'x',candidates:[c]},c.evidence).candidates[0].evidence,c.evidence);c.action = 'share'; assert.throws(() => validateResult({ summary: 'x', candidates: [c] }, SOURCE));
});
test('rotating the fingerprint secret never permits a duplicate paid attempt', async () => { const x=fixture();assert.equal((await x.run(x.request())).status,200);x.env.AI_HMAC_KEY='ROTATED_TEST_ONLY_123456789012345678901234567890';assert.equal((await x.run(x.request())).status,409);assert.equal(x.f.calls,1); });
test('missing verified Google identifiers and missing policy owner fail closed', async () => {
  for (const urlPart of ['/userinfo','/about?']) {const x=fixture();const original=x.send;const send=(url,init)=>url.includes(urlPart)?Promise.resolve(asJson(urlPart==='/userinfo'?{email_verified:true}:{user:{}})):original(url,init);const r=await handle(x.request(),x.env,send,x.now);assert.notEqual(r.status,200);assert.equal(x.f.calls,0);}
  const x=fixture();delete x.policy.teams['team-folder'].ownerPermissionId;x.env.TEAM_AI_POLICY=JSON.stringify(x.policy);assert.equal((await x.run(x.request())).status,503);assert.equal(x.f.calls,0);
});

test('Sonnet 5.5 extraction disables up-front thinking without enabling tools or changing the output cap', async () => {
  const x = fixture(); x.policy.teams['team-folder'].model = 'claude-sonnet-5-5';
  x.env.TEAM_AI_POLICY = JSON.stringify(x.policy);
  const send = async (url, init) => {
    if (url === 'https://api.anthropic.com/v1/messages') {
      const body = JSON.parse(init.body);
      assert.deepEqual(body.thinking, { type: 'between_tools' });
      assert.deepEqual(body.output_config, { effort: 'low' });
      assert.equal(body.max_tokens, 1024);
      assert.equal(body.tools, undefined);
    }
    return x.send(url, init);
  };
  assert.equal((await handle(x.request(), x.env, send, x.now)).status, 200);
  assert.equal(x.f.calls, 1);
});

test('Google permission listing omits the first page token and encodes returned continuation tokens', async () => {
  const x = fixture(); const pages = []; const continuation = 'next page+/=';
  const send = async (url, init) => {
    const parsed = new URL(url);
    if (parsed.pathname === '/drive/v3/files/team-folder/permissions') {
      const page = parsed.searchParams.get('pageToken'); pages.push(page);
      if (page === '') return new Response('{}', { status: 403 });
      if (page === null) return asJson({ permissions: [], nextPageToken: continuation });
      assert.equal(page, continuation);
      return asJson({ permissions: [{ id: 'member', type: 'user', role: 'reader' }] });
    }
    return x.send(url, init);
  };
  assert.equal((await handle(x.request(), x.env, send, x.now)).status, 200);
  assert.deepEqual(pages, [null, continuation, null, continuation]);
  assert.equal(x.f.calls, 1);
});
