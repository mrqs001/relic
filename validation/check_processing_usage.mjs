// Read-only local contract check: usage accounting must never advertise quotas.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
const require = createRequire(new URL('../web/package.json', import.meta.url));
const { build } = require('esbuild');
const result = await build({ entryPoints: [fileURLToPath(new URL('../web/src/api.ts', import.meta.url))], bundle: true, write: false, format: 'esm', platform: 'browser' });
const { processingSchema } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
const response = await fetch('http://127.0.0.1:8790/api/v1/processing', {signal:AbortSignal.timeout(15000)});
assert.equal(response.ok, true);
const status = processingSchema.parse(await response.json());
assert.ok(status.supplemental);
for (const usage of [status.today, status.supplemental.today]) {
  assert.equal(usage.limit, null);
  assert.equal(usage.spend_limit_usd, null);
  assert.ok(usage.calls >= 0);
}
assert.equal(status.today.unknown_cost_requests > 0, status.today.cost_usd === null);
console.log(JSON.stringify({outcome:'passed',baseline_request_cap:null,supplementary_request_cap:null,spend_caps:null,cost_accounting_retained:true,provider_requests:0}));
