import { DurableObject } from 'cloudflare:workers';
import { handle, BudgetLedger } from './gateway.mjs';
export default { fetch: (request, env) => handle(request, env) };
export class TeamBudget extends DurableObject {
  constructor(ctx, env) { super(ctx, env); this.ledger = new BudgetLedger(ctx.storage); }
  fetch(request) { return this.ledger.fetch(request); }
  alarm() { return this.ledger.alarm(); }
}
