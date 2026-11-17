// Actual Worker entrypoint and SQLite Durable Objects; every outbound request is intercepted.
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Miniflare, convertV4MiniflareOptions, Response } from 'miniflare';

const ROOT = fileURLToPath(new URL('../../', import.meta.url));
const TARGET = join(ROOT, 'target');
const WORKER = fileURLToPath(new URL('./worker.mjs', import.meta.url));
const CONFIG = JSON.parse(await readFile(new URL('./wrangler.jsonc', import.meta.url), 'utf8'));
const SOURCE = '2026-11-20까지 검증용 문서를 검토해 주세요.';
const ACCESS = 'GOOGLE_RUNTIME_TEST_ONLY_1234567890';
const KEY = 'CLAUDE_RUNTIME_TEST_ONLY_1234567890';
const HMAC = 'HMAC_RUNTIME_TEST_ONLY_12345678901234567890';
const json = (value, status = 200) => new Response(JSON.stringify(value), {
  status, headers: { 'content-type': 'application/json' },
});

async function fixture(t, { teamLimit = 2, globalLimit = 10, model = 'claude-runtime-test-only', pagedPermissions = false, accessCheck = false } = {}) {
  await mkdir(TARGET, { recursive: true });
  const statePath = await mkdtemp(join(TARGET, 'team-ai-runtime-'));
  const state = { calls: 0, outbound: 0, revoked: false, revokeAfterCall: false,
    redirectGoogle: false, redirectClaude: false, unexpected: [], permissionPages: [] };
  const policy = { version: 1, dailyRequests: globalLimit, teams: {
    'runtime-team': { ownerPermissionId: 'runtime-owner', model, dailyRequests: teamLimit, maxOutputTokens: 512 },
  } };
  const outbound = async request => {
    state.outbound++;
    const url = new URL(request.url);
    if (!['https://api.anthropic.com', 'https://www.googleapis.com', 'https://openidconnect.googleapis.com'].includes(url.origin)) {
      state.unexpected.push(`${url.origin}${url.pathname}`);
      throw new Error('Unexpected outbound request was blocked.');
    }
    if (url.origin === 'https://api.anthropic.com') {
      assert.equal(url.pathname, '/v1/messages');
      assert.equal(request.headers.get('authorization'), `Bearer ${KEY}`);
      assert.equal(request.headers.get('anthropic-version'), '2026-10-21');
      const value = await request.json();
      assert.equal(value.messages[0].content, SOURCE);
      assert.equal(value.model, model);
      assert.equal(value.max_tokens, 512);
      if (model === 'claude-sonnet-5-5') {
        assert.deepEqual(value.thinking, { type: 'between_tools' });
        assert.deepEqual(value.output_config, { effort: 'low' });
      }
      assert.equal(value.tools, undefined);
      assert.ok(!JSON.stringify(value).includes(ACCESS));
      state.calls++;
      if (state.redirectClaude) return new Response('', { status: 307,
        headers: { location: 'https://unexpected.example.invalid/provider' } });
      return json({ stop_reason: 'end_turn', content: [{ type: 'text', text: JSON.stringify({
        summary: '검증용 문서 검토', candidates: [{ title: '검증용 문서 검토', assignee: '',
          dueDate: '2026-11-20', evidence: SOURCE, uncertain: false }],
      }) }] });
    }
    assert.equal(request.headers.get('authorization'), `Bearer ${ACCESS}`);
    assert.equal(request.headers.get('x-api-key'), null);
    if (state.redirectGoogle) return new Response('', { status: 302,
      headers: { location: 'https://unexpected.example.invalid/google' } });
    if (state.revoked || state.revokeAfterCall && state.calls > 0) return json({}, 403);
    if (request.url === 'https://openidconnect.googleapis.com/v1/userinfo') {
      return json({ sub: 'runtime-subject', email_verified: true });
    }
    if (request.url === 'https://www.googleapis.com/drive/v3/about?fields=user(permissionId)') {
      return json({ user: { permissionId: 'runtime-member' } });
    }
    if (url.origin === 'https://www.googleapis.com' && url.pathname === '/drive/v3/files/runtime-team/permissions') {
      const page = url.searchParams.get('pageToken'); state.permissionPages.push(page);
      if (page === '') return json({}, 403);
      if (pagedPermissions) {
        if (page === null) return json({ permissions: [], nextPageToken: 'runtime page+/=' });
        assert.equal(page, 'runtime page+/=');
      }
      return json({ permissions: [{ id: 'runtime-member', type: 'user', role: 'reader' }] });
    }
    if (url.origin === 'https://www.googleapis.com' && url.pathname === '/drive/v3/files/runtime-team') {
      return json({ id: 'runtime-team', mimeType: 'application/vnd.google-apps.folder', trashed: false,
        owners: [{ permissionId: 'runtime-owner' }], properties: { saiTeam: '1' }, capabilities: { canListChildren: true } });
    }
    state.unexpected.push(`${url.origin}${url.pathname}`);
    throw new Error('Unexpected outbound request was blocked.');
  };
  const options = () => convertV4MiniflareOptions({
    name: CONFIG.name,
    modules: [
      ...(accessCheck ? [{ type: 'ESModule', path: join(dirname(WORKER), 'access-check-worker.mjs') }] : []),
      { type: 'ESModule', path: WORKER },
      { type: 'ESModule', path: join(dirname(WORKER), 'gateway.mjs') },
    ],
    modulesRoot: dirname(WORKER),
    compatibilityDate: CONFIG.compatibility_date,
    host: '127.0.0.1', port: 0,
    cf: false,
    telemetry: { enabled: false },
    resourcePersistencePath: statePath,
    durableObjects: { BUDGETS: { className: 'TeamBudget', useSQLite: true } },
    bindings: { TEAM_AI_POLICY: JSON.stringify(policy), AI_HMAC_KEY: HMAC, ANTHROPIC_API_KEY: KEY },
    outboundService: outbound,
  });
  let runtime;
  t.after(async () => {
    await runtime?.dispose();
    // This path was created here, and must remain inside this workspace's target directory.
    assert.equal(dirname(resolve(statePath)), resolve(TARGET));
    assert.ok(basename(statePath).startsWith('team-ai-runtime-'));
    await rm(statePath, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
  });
  const start = async () => { runtime = new Miniflare(options()); await runtime.ready; };
  await start();
  const createdAt = Date.now();
  const dispatch = (path, value) => runtime.dispatchFetch(`https://gateway.example.invalid${path}`, {
    method: 'POST', headers: { authorization: `Bearer ${ACCESS}`, 'content-type': 'application/json' },
    body: JSON.stringify(value),
  });
  return {
    state,
    status: () => dispatch('/v1/status', { folderId: 'runtime-team' }),
    analyze: id => dispatch('/v1/analyze', { folderId: 'runtime-team', requestId: id.repeat(40), createdAt, text: SOURCE }),
    restart: async () => { await runtime.dispose(); runtime = undefined; await start(); },
    inspectPersistence: async () => {
      await runtime.dispose(); runtime = undefined;
      const files = (await readdir(statePath, { recursive: true })).filter(p => p.endsWith('.sqlite'));
      assert.ok(files.length > 0, 'Actual SQLite persistence must exist.');
      for (const path of files) {
        const bytes = await readFile(join(statePath, path));
        assert.equal(bytes.subarray(0, 16).toString(), 'SQLite format 3\0');
        for (const secret of [SOURCE, ACCESS, KEY, HMAC, 'runtime-subject']) assert.ok(!bytes.includes(Buffer.from(secret)));
      }
    },
  };
}

test('Worker entrypoint uses real SQLite for concurrent deduplication and restart persistence', { timeout: 60000 }, async t => {
  const f = await fixture(t);
  const response = await f.status();
  const status = await response.json();
  assert.equal(response.status, 200, JSON.stringify({ ...status, outbound: f.state.outbound }));
  assert.equal(status.ready, true);
  assert.equal(status.used, 0);
  assert.equal(f.state.calls, 0);
  const replies = await Promise.all(Array.from({ length: 8 }, () => f.analyze('a')));
  assert.equal(replies.filter(r => r.status === 200).length, 1);
  assert.equal(replies.filter(r => r.status === 409).length, 7);
  const body = await replies.find(r => r.status === 200).text();
  assert.ok(body.includes('검증용 문서 검토'));
  for (const secret of [ACCESS, KEY, HMAC]) assert.ok(!body.includes(secret));
  assert.equal(f.state.calls, 1);
  await f.restart();
  assert.equal((await f.analyze('a')).status, 409);
  assert.equal((await (await f.status()).json()).used, 1);
  assert.equal(f.state.calls, 1);
  assert.deepEqual(f.state.unexpected, []);
  await f.inspectPersistence();
});

test('real SQLite serializes distinct requests against team and global attempt caps', { timeout: 60000 }, async t => {
  for (const limits of [{ teamLimit: 1, globalLimit: 10 }, { teamLimit: 10, globalLimit: 1 }]) {
    const f = await fixture(t, limits);
    const replies = await Promise.all(Array.from({ length: 8 }, (_, i) => f.analyze(String(i))));
    assert.equal(replies.filter(r => r.status === 200).length, 1);
    assert.equal(replies.filter(r => r.status === 429).length, 7);
    assert.equal(f.state.calls, 1);
    assert.deepEqual(f.state.unexpected, []);
  }
});

test('discarding a response never permits another provider call after a runtime restart', { timeout: 60000 }, async t => {
  const f = await fixture(t);
  const discarded = await f.analyze('b');
  assert.equal(discarded.status, 200);
  await discarded.body.cancel();
  await f.restart();
  assert.equal((await f.analyze('b')).status, 409);
  assert.equal(f.state.calls, 1);
  assert.deepEqual(f.state.unexpected, []);
});

test('revoked membership before or after processing hides the analysis in the actual Worker', { timeout: 60000 }, async t => {
  const f = await fixture(t);
  f.state.revoked = true;
  assert.equal((await f.analyze('c')).status, 403);
  assert.equal(f.state.calls, 0);
  f.state.revoked = false;
  f.state.revokeAfterCall = true;
  const reply = await f.analyze('d');
  assert.equal(reply.status, 403);
  assert.ok(!(await reply.text()).includes('검증용 문서 검토'));
  assert.equal(f.state.calls, 1);
  assert.deepEqual(f.state.unexpected, []);
});

test('Google and Claude redirects are rejected by workerd without forwarding either credential', { timeout: 60000 }, async t => {
  const f = await fixture(t);
  f.state.redirectGoogle = true;
  assert.equal((await f.status()).status, 403);
  assert.equal(f.state.calls, 0);
  f.state.redirectGoogle = false;
  f.state.redirectClaude = true;
  assert.equal((await f.analyze('e')).status, 502);
  assert.equal(f.state.calls, 1);
  assert.deepEqual(f.state.unexpected, []);
});

test('Sonnet 5.5 extraction sends its bounded text-only configuration through the actual Worker', { timeout: 60000 }, async t => {
  const f = await fixture(t, { model: 'claude-sonnet-5-5' });
  assert.equal((await f.analyze('f')).status, 200);
  assert.equal(f.state.calls, 1);
  assert.deepEqual(f.state.unexpected, []);
});

test('actual Worker omits an absent Google page token and follows the encoded next page', { timeout: 60000 }, async t => {
  const f = await fixture(t, { pagedPermissions: true });
  assert.equal((await f.analyze('g')).status, 200);
  assert.deepEqual(f.state.permissionPages, [null, 'runtime page+/=', null, 'runtime page+/=']);
  assert.equal(f.state.calls, 1);
  assert.deepEqual(f.state.unexpected, []);
});

test('permission-only Worker verifies members but blocks analysis before outbound requests or budget reservation', { timeout: 60000 }, async t => {
  const f = await fixture(t, { accessCheck: true });
  const initial = await f.status();
  assert.equal(initial.status, 200);
  const status = await initial.json();
  assert.equal(status.owner, false);
  assert.equal(status.ready, false);
  assert.equal(status.analysisDisabled, true);
  assert.equal(status.used, 0);
  const outboundBefore = f.state.outbound;
  const blocked = await f.analyze('h');
  assert.equal(blocked.status, 403);
  assert.deepEqual(await blocked.json(), { error: 'analysis_disabled' });
  assert.equal(f.state.outbound, outboundBefore);
  assert.equal((await (await f.status()).json()).used, 0);
  f.state.revoked = true;
  assert.equal((await f.status()).status, 403);
  assert.equal(f.state.calls, 0);
  assert.deepEqual(f.state.unexpected, []);
});
