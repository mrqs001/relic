// Bounded, fictional-only sustained import/read measurement. No model/provider calls.
import assert from 'node:assert/strict';
import {readFile, writeFile} from 'node:fs/promises';
import os from 'node:os';
const base = new URL(process.argv[2]);
assert.ok(['127.0.0.1','localhost'].includes(base.hostname));
const output = process.argv[3];
const duration = 120000;
const samples = [], failures = [];
const sleep = ms => new Promise(r => setTimeout(r,ms));
async function api(path, body) {
 const started=performance.now();
 const response=await fetch(new URL('/api/v1'+path,base),{method:body===undefined?'GET':'POST',headers:{'Content-Type':'application/json',Origin:base.origin},...(body===undefined?{}:{body:JSON.stringify(body)}),signal:AbortSignal.timeout(30000)});
 const headers=performance.now();const text=await response.text();const decoded=performance.now();const value=JSON.parse(text);
 samples.push({path:path.split('?')[0],status:response.status,headers_ms:headers-started,body_ms:decoded-headers,parse_ms:performance.now()-decoded,total_ms:performance.now()-started,bytes:Buffer.byteLength(text)});
 assert.ok(response.ok,`${path}: HTTP ${response.status}`);return value;
}
const workspace=await api('/workspace');
assert.ok(workspace.sources.length>=20 && workspace.sources.every(s=>s.fictional),'Disposable fictional workspace required');
const scope={source_ids:[],include_unlocated:true,query:'selene-workload',time_basis:'publication_time',from:null,to:null,bbox:null,publication_id:null};
const baseline=await api('/query',{scope,generation:0,limit:100});assert.equal(baseline.total,10000);
const e=baseline.items[0];
const detailPath=`/evidence?id=${encodeURIComponent(e.id)}&revision=${encodeURIComponent(e.revision)}`;
const original=await api(detailPath);
const seed=JSON.parse(await readFile(new URL('../state/workload/batch-000.json',import.meta.url),'utf8'));
const started=performance.now();let imported=0,round=0;
async function reader(kind) {
 while(performance.now()-started<duration) {
  try {
   if(kind==='query') {const q=await api('/query',{scope,generation:++round,limit:100});assert.equal(q.total,10000);assert.equal(q.located,8000);assert.equal(q.unlocated,2000);}
   else {const d=await api(detailPath);assert.equal(d.text,original.text);assert.deepEqual(d.citation,original.citation);}
  } catch(e) {failures.push({kind,message:String(e)});break;}
  await sleep(250);
 }
}
async function importer() {
 let batch=0;
 while(performance.now()-started<duration) {
  const evidence_revisions=seed.evidence_revisions.slice(0,100).map((e,n)=>{e=structuredClone(e);e.id=`sustained:20260923:${batch}:${n}`;e.text=e.text.replace('selene-workload','sustained-workload');e.citation.evidence_id=e.id;e.citation.end_character=[...e.text].length;return e;});
  try {const r=await api('/imports',{...seed,title:'Fictional sustained load',evidence_revisions});imported+=r.accepted;} catch(e){failures.push({kind:'import',message:String(e)});break;}
  batch++;await sleep(1000);
 }
}
await Promise.all([reader('query'),reader('evidence'),importer()]);
const final=await api('/query',{scope,generation:++round,limit:100});assert.equal(final.total,10000);
const quantile=(xs,p)=>[...xs].sort((a,b)=>a-b)[Math.min(xs.length-1,Math.floor(xs.length*p))];
const measured=Object.fromEntries(['/query','/evidence','/imports'].map(path=>{const rows=samples.filter(s=>s.path===path);return [path,{samples:rows.length,p95_ms:quantile(rows.map(r=>r.total_ms),.95),max_ms:Math.max(...rows.map(r=>r.total_ms)),headers_p95_ms:quantile(rows.map(r=>r.headers_ms),.95),parse_p95_ms:quantile(rows.map(r=>r.parse_ms),.95)}]}));
const report={timestamp:new Date().toISOString(),duration_ms:performance.now()-started,environment:{cpu:os.cpus()[0].model,memory_bytes:os.totalmem(),node:process.version},fictional:true,initial_revisions:50000,matching_reports:10000,imported,measured,failures,exact_original_preserved:!failures.some(f=>f.kind==='evidence'),limitations:['Two-minute HTTP/import soak, not complete release acceptance.','No live provider or model processing workload.','Browser rendering and cancellation measured separately.']};
await writeFile(output,JSON.stringify(report,null,2)+'\n');console.log(JSON.stringify(report));if(failures.length)process.exitCode=1;
