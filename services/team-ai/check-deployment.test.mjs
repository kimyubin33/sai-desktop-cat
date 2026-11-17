import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, mkdir, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { inspectDeployment } from './check-deployment.mjs';

const base = JSON.parse(await readFile(new URL('./wrangler.jsonc', import.meta.url), 'utf8'));
function fixture() {
  return {
    env: {
      SAI_TEAM_AI_ENDPOINT: 'https://ai.example.invalid',
      CLOUDFLARE_ACCOUNT_ID: 'a'.repeat(32),
      ANTHROPIC_API_KEY: 'CLAUDE_KEY_TEST_ONLY_1234567890',
      AI_HMAC_KEY: 'HMAC_TEST_ONLY_123456789012345678901234567890',
      TEAM_AI_POLICY: JSON.stringify({ version: 1, dailyRequests: 10, teams: {
        'team-test-only': { ownerPermissionId: 'owner-test-only', model: 'claude-test-only', dailyRequests: 2, maxOutputTokens: 512 },
      } }),
    },
    worker: { ...structuredClone(base), routes: [{ pattern: 'ai.example.invalid', custom_domain: true }] },
  };
}

test('empty operator environment reports missing setup without contacting services', () => {
  const report = inspectDeployment({}, base);
  assert.equal(report.localConfigurationValid, false);
  assert.equal(report.networkRequests, 0);
  assert.equal(report.teamCount, 0);
  assert.equal(report.checks.find(c => c.id === 'sqlite_budget_binding').ready, true);
});

test('valid local setup remains distinct from live authentication and model access', () => {
  const { env, worker } = fixture();
  const report = inspectDeployment(env, worker);
  assert.equal(report.localConfigurationValid, true);
  assert.equal(report.teamCount, 1);
  assert.equal(report.dailyAttemptLimit, 10);
  assert.equal(report.liveAuthenticationChecked, false);
  assert.equal(report.providerModelAccessChecked, false);
  for (const value of [...Object.values(env), 'team-test-only', 'owner-test-only']) {
    assert.ok(!JSON.stringify(report).includes(value));
  }
});

test('unsafe endpoint, mismatched public route and missing SQLite migration cannot pass', () => {
  for (const raw of ['http://ai.example.invalid', 'https://localhost', 'https://127.0.0.1', 'https://[::1]',
    'https://user:password@ai.example.invalid', 'https://ai.example.invalid/token', 'https://ai.example.invalid/?key=private']) {
    const { env, worker } = fixture(); env.SAI_TEAM_AI_ENDPOINT = raw;
    assert.equal(inspectDeployment(env, worker).localConfigurationValid, false);
  }
  for (const mutate of [w => { w.routes = []; }, w => { w.migrations = []; },
    w => { w.durable_objects.bindings[0].script_name = 'another-worker'; }, w => { w.secrets.required = []; }]) {
    const { env, worker } = fixture(); mutate(worker);
    assert.equal(inspectDeployment(env, worker).localConfigurationValid, false);
  }
});

test('invalid policy, empty allowlist and missing secrets fail without exposing their values', () => {
  for (const change of [e => { e.TEAM_AI_POLICY = 'PRIVATE_INVALID_POLICY'; },
    e => { e.TEAM_AI_POLICY = JSON.stringify({ version: 1, dailyRequests: 1, teams: {} }); },
    e => { delete e.AI_HMAC_KEY; }, e => { delete e.ANTHROPIC_API_KEY; }, e => { delete e.CLOUDFLARE_ACCOUNT_ID; }]) {
    const { env, worker } = fixture(); change(env);
    const report = inspectDeployment(env, worker);
    assert.equal(report.localConfigurationValid, false);
    assert.ok(!JSON.stringify(report).includes('PRIVATE_INVALID_POLICY'));
  }
});

test('an explicit operator draft supplies its account while an environment override remains authoritative', () => {
  const { env, worker } = fixture();
  worker.account_id = env.CLOUDFLARE_ACCOUNT_ID;
  delete env.CLOUDFLARE_ACCOUNT_ID;
  assert.equal(inspectDeployment(env, worker).localConfigurationValid, true);
  env.CLOUDFLARE_ACCOUNT_ID = 'INVALID_ACCOUNT_TEST_ONLY';
  assert.equal(inspectDeployment(env, worker).localConfigurationValid, false);
});

test('CLI checks an explicit local draft and keeps bad paths, contents and arguments private', async t => {
  const target = fileURLToPath(new URL('../../target/', import.meta.url));
  await mkdir(target, { recursive: true });
  const dir = await mkdtemp(join(target, 'team-ai-deployment-test-'));
  t.after(async () => {
    assert.equal(dirname(resolve(dir)), resolve(target));
    assert.ok(basename(dir).startsWith('team-ai-deployment-test-'));
    await rm(dir, { recursive: true, force: true });
  });
  const script = fileURLToPath(new URL('./check-deployment.mjs', import.meta.url));
  const { env, worker } = fixture();
  worker.account_id = env.CLOUDFLARE_ACCOUNT_ID;
  delete env.CLOUDFLARE_ACCOUNT_ID;
  const config = join(dir, 'operator.jsonc');
  await writeFile(config, JSON.stringify(worker));
  const report = JSON.parse(execFileSync(process.execPath, [script, '--config', config], { env, encoding: 'utf8' }));
  assert.equal(report.localConfigurationValid, true);
  const privateText = 'PRIVATE_BAD_CONFIG_TEST_ONLY';
  const badConfig = join(dir, `${privateText}.jsonc`);
  await writeFile(badConfig, privateText);
  for (const args of [['--config', badConfig], ['--config', join(dir, 'missing.jsonc')], ['--unexpected', privateText]]) {
    let failure;
    try { execFileSync(process.execPath, [script, ...args], { env, encoding: 'utf8', stdio: 'pipe' }); }
    catch (error) { failure = error; }
    assert.equal(failure?.status, 1);
    const output = `${failure.stdout}${failure.stderr}`;
    for (const secret of [privateText, dir, env.ANTHROPIC_API_KEY, env.AI_HMAC_KEY]) assert.ok(!output.includes(secret));
  }
});
