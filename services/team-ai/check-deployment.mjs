// Offline operator check. Never contacts a service, decrypts desktop keys, or prints values.
import { readFile } from 'node:fs/promises';
import { isIP } from 'node:net';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { configuration, providerConfigured } from './gateway.mjs';

const SECRETS = ['ANTHROPIC_API_KEY', 'AI_HMAC_KEY', 'TEAM_AI_POLICY'];

function endpoint(raw) {
  if (typeof raw !== 'string' || raw.length > 2048) return null;
  try {
    const url = new URL(raw);
    const host = url.hostname.replace(/^\[|\]$/g, '');
    if (url.protocol !== 'https:' || !host || host === 'localhost' || isIP(host)
      || url.username || url.password || url.search || url.hash || url.pathname !== '/') return null;
    return url;
  } catch { return null; }
}

export function inspectDeployment(env, worker) {
  const url = endpoint(env.SAI_TEAM_AI_ENDPOINT);
  const budgets = worker?.durable_objects?.bindings;
  const storage = Array.isArray(budgets)
    && budgets.filter(b => b.name === 'BUDGETS').length === 1
    && budgets.some(b => b.name === 'BUDGETS' && b.class_name === 'TeamBudget' && !b.script_name)
    && Array.isArray(worker?.migrations)
    && worker.migrations.some(m => Array.isArray(m.new_sqlite_classes)
      && m.new_sqlite_classes.includes('TeamBudget'));
  let policy = null;
  try { policy = configuration({ ...env, BUDGETS: storage ? {} : null }); } catch { /* Values stay private. */ }
  const routes = Array.isArray(worker?.routes) ? worker.routes : [];
  const routed = !!url && (routes.some(r => {
    const pattern = typeof r === 'string' ? r : r?.pattern;
    return [url.hostname, `${url.hostname}/*`, `${url.origin}/*`].includes(pattern);
  }) || worker?.workers_dev === true && url.hostname.endsWith('.workers.dev')
    && url.hostname.startsWith(`${worker.name}.`));
  const declared = Array.isArray(worker?.secrets?.required)
    && SECRETS.every(name => worker.secrets.required.includes(name));
  const checks = [
    { id: 'https_endpoint', ready: !!url },
    { id: 'cloudflare_account', ready: /^[a-f0-9]{32}$/i.test(env.CLOUDFLARE_ACCOUNT_ID || worker?.account_id || '') },
    { id: 'matching_public_route', ready: routed },
    { id: 'sqlite_budget_binding', ready: !!storage },
    { id: 'required_secrets_declared', ready: declared },
    { id: 'team_policy_and_hmac', ready: !!policy && Object.keys(policy.teams).length > 0 },
    { id: 'claude_key_format', ready: providerConfigured(env) },
  ];
  return {
    mode: 'offline',
    localConfigurationValid: checks.every(c => c.ready),
    checks,
    teamCount: policy ? Object.keys(policy.teams).length : 0,
    dailyAttemptLimit: policy?.dailyRequests ?? null,
    liveAuthenticationChecked: false,
    providerModelAccessChecked: false,
    networkRequests: 0,
  };
}

export async function main(env = process.env, args = process.argv.slice(2)) {
  try {
    if (args.length !== 0 && !(args.length === 2 && args[0] === '--config' && args[1].trim())) {
      throw new Error('Unsupported configuration arguments.');
    }
    const configPath = args.length ? resolve(args[1]) : new URL('./wrangler.jsonc', import.meta.url);
    const worker = JSON.parse(await readFile(configPath, 'utf8'));
    const report = inspectDeployment(env, worker);
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
    return report.localConfigurationValid ? 0 : 1;
  } catch {
    process.stderr.write('Unable to check the Worker configuration. No configuration values were printed.\n');
    return 1;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  process.exitCode = await main();
}
