import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
const require = createRequire(new URL('../web/package.json', import.meta.url));
const { build } = require('esbuild');
const result = await build({ entryPoints: [fileURLToPath(new URL('../web/src/time-window.ts', import.meta.url))], bundle:true,write:false,format:'esm',platform:'browser',define:{'process.env.NODE_ENV':'"production"'} });
const { resolveTimeScope, shiftInterval, timeWindows } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
const now=Date.parse('2026-09-12T12:00:00.000Z'),scope={from:null,to:null,source_ids:['fictional:one'],time_basis:'publication_time'};
for (const [preset, duration] of Object.entries(timeWindows)) {
  const first=resolveTimeScope(scope,preset,now),next=resolveTimeScope(scope,preset,now+30000);
  assert.equal(Date.parse(first.from),now-duration);
  assert.equal(Date.parse(next.from)-Date.parse(first.from),30000);
  assert.equal(Date.parse(next.to)-Date.parse(first.to),30000);
  assert.deepEqual(next.source_ids,scope.source_ids);
  assert.equal(scope.from,null);
  assert.deepEqual(resolveTimeScope(first,null,now+30000),first);
  const before=shiftInterval(first.from,first.to,-1);
  assert.equal(Date.parse(before.to)+1,Date.parse(first.from));
  assert.deepEqual(shiftInterval(before.from,before.to,1),{from:first.from,to:first.to});
}
console.log('Passed: six rolling presets, exact frozen interval, inclusive adjacent steps, shared source scope.');
