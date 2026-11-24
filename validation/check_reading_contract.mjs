// Historical reading compatibility; malformed populated events still reject.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
const require = createRequire(new URL('../web/package.json', import.meta.url));
const { build } = require('esbuild');
const result = await build({ entryPoints: [fileURLToPath(new URL('../web/src/api.ts', import.meta.url))], bundle: true, write: false, format: 'esm', platform: 'browser' });
const { interpretationSchema, workspaceSchema, querySchema } = await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].text).toString('base64')}`);
const old = { id:'fiction:reading',language:'en',translation:null,headline:'Fictional statement',kind:'statement',claim_status:'reported',topic:'other',claims:[],locations:[],location_label:null,location_precision:null,geometry:null,related_count:0,related_key:null,model:'fictional',created_at:'2026-09-20T00:00:00Z' };
for (const reading of [old,{...old,events:null},{...old,events:[]}]) assert.deepEqual(interpretationSchema.parse(reading).events,[]);
for (const events of [{},'invalid',[{type:'invented-event'}]]) assert.equal(interpretationSchema.safeParse({...old,events}).success,false);
console.log(JSON.stringify({outcome:'passed',legacy_missing_events:true,legacy_null_events:true,invalid_populated_events_rejected:true}));

if (process.argv.includes('--live-local')) {
  const base = 'http://127.0.0.1:8790/api/v1';
  async function get(path, body) {
    const response = await fetch(base+path, body ? { method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body),signal:AbortSignal.timeout(15000) } : {signal:AbortSignal.timeout(15000)});
    assert.equal(response.ok,true,`Local endpoint failed: ${path}`);
    return response.json();
  }
  workspaceSchema.parse(await get('/workspace'));
  const first = querySchema.parse(await get('/query',{scope:{content_view:'all'},generation:1,limit:1}));
  let cursor=null, checked=0, done=false;
  for (let page=0;page<100;page++) {
    const response = querySchema.parse(await get('/query',{scope:{content_view:'all',publication_id:first.publication_id},generation:1,limit:100,cursor}));
    checked += response.items.length;
    cursor=response.next_cursor;
    if (!cursor) { done=true; assert.equal(checked,response.total); break; }
  }
  assert.equal(done,true,'Local contract scan reached its 10,000-message bound');
  console.log(JSON.stringify({outcome:'passed',live_local_workspace:true,frozen_publication_messages_checked:checked,provider_requests:0}));
}
