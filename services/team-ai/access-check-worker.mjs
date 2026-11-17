// Temporary operator verification entrypoint. It never delegates analysis requests.
// Normal deployment continues to use worker.mjs; permissions are checked by that same handler.
import worker from './worker.mjs';
export { TeamBudget } from './worker.mjs';

const headers = { 'content-type': 'application/json; charset=utf-8', 'cache-control': 'no-store',
  'x-content-type-options': 'nosniff' };
export default {
  async fetch(request, env) {
    if (request.method !== 'POST' || new URL(request.url).pathname !== '/v1/status') {
      return new Response(JSON.stringify({ error: 'analysis_disabled' }), { status: 403, headers });
    }
    const response = await worker.fetch(request, env);
    if (!response.ok) return response;
    const status = await response.json();
    return new Response(JSON.stringify({ ...status, ready: false, analysisDisabled: true }), { headers });
  },
};
