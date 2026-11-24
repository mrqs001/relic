// Read-only, bounded check of published message interpretations through Relic.
import assert from 'node:assert/strict';
const origin = new URL(process.env.RELIC_TEST_URL || 'http://127.0.0.1:8790');
assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(origin.hostname), 'Local Relic only');
async function api(path, body) {
  const response = await fetch(new URL(`/api/v1${path}`,origin), body ? {method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)} : {});
  assert.equal(response.ok,true,`API status ${response.status}`);
  return response.json();
}
const all = await api('/query',{scope:{},generation:1,limit:500});
assert.ok(all.total<=500,'Narrow this bounded check before using a larger archive');
assert.equal(all.items.length,all.total);
const scope={publication_id:all.publication_id};
const processed=all.items.filter(item=>item.interpretation);
let claims=0, locations=0;
for(const item of processed){
  const reading=item.interpretation;
  for(const support of [...reading.claims,...reading.locations]){
    const citation=support.citation;
    assert.equal(citation.evidence_id,item.id);
    assert.equal(citation.revision,item.revision);
    assert.equal(Array.from(item.text).slice(citation.start_character,citation.end_character).join(''),support.quote);
  }
  claims+=reading.claims.length;locations+=reading.locations.length;
  if(reading.geometry){ assert.equal(reading.kind,'event');assert.equal(reading.claim_status,'reported');assert.equal(reading.location_precision,'approximate_city'); }
}
assert.equal(all.located+all.unlocated,all.total);
assert.equal(all.map.features.length,all.located);
const signals=await api('/query',{scope:{...scope,content_view:'signals'},generation:2,limit:500});
assert.equal(signals.total,all.items.filter(i=>!['promotion','other'].includes(i.interpretation?.kind)).length);
let relatedChecked=false;
const related=processed.find(i=>i.interpretation.related_key && i.interpretation.related_count>0);
if(related){const key=related.interpretation.related_key;const q=await api('/query',{scope:{...scope,related_key:key},generation:3,limit:500});assert.equal(q.total,all.items.filter(i=>i.interpretation?.related_key===key).length);assert.ok(q.items.every(i=>i.interpretation?.related_key===key));relatedChecked=true;}
const sources=[...new Set(processed.map(i=>i.source))];
for(const source of sources.slice(0,5)){
 const item=processed.find(i=>i.source===source);
 const detail=await api(`/evidence?id=${encodeURIComponent(item.id)}&revision=${encodeURIComponent(item.revision)}&publication_id=${encodeURIComponent(all.publication_id)}`);
 assert.equal(detail.text,item.text);assert.equal(detail.interpretation.id,item.interpretation.id);
}
console.log(JSON.stringify({checked_at:new Date().toISOString(),outcome:'passed',records:all.total,processed:processed.length,exact_claim_quotes:claims,exact_location_quotes:locations,located:all.located,unlocated:all.unlocated,signals:signals.total,related_scope_checked:relatedChecked,processed_sources:sources.length,limitations:'Bounded retained-output and shared-scope checks; not semantic accuracy or complete source coverage.'},null,2));
