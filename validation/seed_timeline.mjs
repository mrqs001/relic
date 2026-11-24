// Fictional, disposable UI/load fixture. Never seed a real workspace.
import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
const base = new URL(process.argv[2] ?? 'http://127.0.0.1:8793');
assert.ok(['localhost','127.0.0.1','[::1]'].includes(base.hostname));
const request = async (path, body) => {
  const r = await fetch(new URL(`/api/v1${path}`,base), { ...(body ? {method:'POST',body:JSON.stringify(body),headers:{'Content-Type':'application/json',Origin:base.origin}} : {}), signal:AbortSignal.timeout(60000) });
  assert.ok(r.ok,`Fixture request ${path}: HTTP ${r.status}`); return r.json();
};
const workspace=await request('/workspace');
assert.ok(workspace.empty, 'Choose a NEW disposable workspace. No live or existing evidence may be mixed with this seed.');
const anchor=process.argv[3] ? Date.parse(process.argv[3]) : Date.now();
assert.ok(Number.isFinite(anchor),'Use an ISO timestamp for the optional fixture clock.');
const sources=Array.from({length:3},(_,i)=>({id:`timeline:source-${i}`,label:`Fictional desk ${i+1}`,family:'authored_report'}));
for (let batch=0;batch<20;batch++) {
  const rows=Array.from({length:500},(_,i)=>{
    const n=batch*500+i,id=`timeline:message-${n}`,text=`Fictional message ${n}. Exercise notice near the demonstration harbour.`;
    return {id,revision:'v1',source:sources[n%3].id,published_at:n%101===0?null:new Date(anchor-n*60000).toISOString(),acquired_at:new Date(anchor-n*60000+30000).toISOString(),text,
      geometry:n%5===0?null:{type:'Point',coordinates:[-30+(n%20)*.1,12+(n%13)*.1]},location_precision:n%5===0?'unresolved':'approximate_locality',origin_group:null,dependence:'unknown',language:'en',fictional:true,availability:'available',citation:{evidence_id:id,revision:'v1',start_character:0,end_character:[...text].length}};
  });
  await request('/imports',{schema_version:1,fictional:true,title:'Fictional rolling timeline',sources,evidence_revisions:rows});
}
const latencies=[];
let last;
for(let i=0;i<24;i++) {
  const started=performance.now();
  last=await request('/query',{scope:{content_view:'all'},generation:i,limit:100});
  latencies.push(performance.now()-started);
  assert.equal(last.total,10000); assert.equal(last.located,8000); assert.equal(last.loaded,100);
  assert.equal(last.timeline.buckets.reduce((n,b)=>n+b.count,0)+last.timeline.undated,10000);
  assert.ok(last.timeline.buckets.length<=36);
}
latencies.sort((a,b)=>a-b);
const result={timestamp:new Date().toISOString(),fictional:true,anchor:new Date(anchor).toISOString(),total:last.total,located:last.located,undated:last.timeline.undated,buckets:last.timeline.buckets.length,queries:latencies.length,p50_ms:latencies[12],p95_ms:latencies[22],max_ms:latencies.at(-1),provider_requests:0,model_requests:0};
await writeFile(new URL('./timeline-load.json',import.meta.url),JSON.stringify(result,null,2)+'\n');
console.log(JSON.stringify(result));
