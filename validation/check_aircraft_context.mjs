import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {fileURLToPath} from 'node:url';
// Aircraft facts read from the live feed alone (no request): what a callsign
// and Mode S address name, roles only for provider-listed military aircraft,
// climb rates and trails only from real successive reports. Fictional data.
const require=createRequire(new URL('../web/package.json',import.meta.url));
const {build}=require('esbuild');
const bundle=await build({entryPoints:[fileURLToPath(new URL('../web/src/components/live/aircraft.ts',import.meta.url))],bundle:true,write:false,format:'esm',platform:'browser'});
const m=await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString('base64')}`);

// Operators: state services before airlines; a prefix must be followed by digits.
assert.equal(m.operatorOf('RCH870').name,'US Air Force Air Mobility Command');
assert.equal(m.operatorOf('RCH870').country,'US');
assert.equal(m.operatorOf('BAW12A').name,'British Airways');
assert.equal(m.operatorOf('BAWX'),null,'letters after the prefix are not a flight');
assert.equal(m.operatorOf('NATO01').name,'NATO');
assert.equal(m.operatorOf('ZZZ123'),null,'unknown designators name nobody');
assert.equal(m.operatorOf(null),null);
// Mode S blocks: the state of registry, never a guess outside the table.
assert.equal(m.registeredIn('43C7A4').code,'GB');
assert.equal(m.registeredIn('020036').code,'MA','Morocco, not Egypt');
assert.equal(m.registeredIn('789123').code,'HK','Hong Kong before the wider China block');
assert.equal(m.registeredIn('AE1238').code,'US');
assert.equal(m.registeredIn('F00000'),null);
assert.equal(m.registeredIn('XYZ'),null);
assert.equal(m.typeName('k35r'),'Boeing KC-135R Stratotanker');
assert.equal(m.typeName('ZZZZ'),null);
// Roles only for provider-listed military or government aircraft, and only from the type or silhouette.
assert.equal(m.roleOf('K35R','narrow',true),'tanker');
assert.equal(m.roleOf('K35R','narrow',false),null,'a civil aircraft gets no role');
assert.equal(m.roleOf('B350','turboprop',true),null,'no role is guessed for multi-purpose types');
assert.equal(m.roleOf('EC45','heli',true),'helicopter');
assert.equal(m.roleCount('helicopter',7),'7 helicopters');
assert.equal(m.roleCount('fighter',1),'1 fast jet');
const d=m.describe({callsign:'PCT610',flight_number:'GNY610',icao_hex:'3F9AE8',aircraft_type:'D228',military:true,source:1,icon:6});
assert.equal(d.operator.name,'German Navy');assert.equal(d.operator.basis,'flight number');assert.equal(d.country.code,'DE');assert.equal(d.role,null);assert.equal(d.source,'MLAT');
assert.equal(m.sourceLabel(4),'Estimated');assert.ok(m.estimated(4)&&!m.estimated(0));
assert.equal(m.fpmText(1234),'+1,200 ft/min');assert.equal(m.fpmText(-250),'Level');assert.equal(m.climb(null),null);

// Climb and trails from successive reports.
const feature=(id,t,lon,alt,extra={})=>({type:'Feature',geometry:{type:'Point',coordinates:[lon,51]},properties:{flight_id:id,observed_at_ms:t,altitude_ft:alt,on_ground:false,tone:'civil',...extra}});
const h=new m.AircraftHistory();
const t0=1_700_000_000_000;
let first=feature(1,t0,0,30000,{trail_from:[-0.01,51,t0-8000]});
let trails=h.update([first],t0);
assert.equal(first.properties.vs_fpm,null,'one report gives no climb rate');
assert.equal(trails.length,1,'the buffered point starts a trail at once');
const second=feature(1,t0+12000,0.03,30600);
trails=h.update([second],t0+12000);
assert.equal(second.properties.vs_fpm,3000,'600 ft in 12 s');
assert.equal(trails[0].geometry.coordinates.length,3);
const repeat=feature(1,t0+12000,0.03,30600);
h.update([repeat],t0+15000);
assert.equal(repeat.properties.vs_fpm,3000,'a repeated report keeps the last rate');
const late=feature(1,t0+12000+180000,1.5,31000);
trails=h.update([late],t0+200000);
assert.equal(late.properties.vs_fpm,null,'a gap of three minutes restarts the trail');
assert.equal(trails.length,0);
const jump=feature(2,t0,0,10000);h.update([jump],t0);
const far=feature(2,t0+10000,20,10000);trails=h.update([far],t0+10000);
assert.equal(trails.length,0,'an impossible jump is never joined');
const ground=feature(3,t0,0,0,{on_ground:true});h.update([ground],t0);
const ground2=feature(3,t0+10000,0.0001,0,{on_ground:true});trails=h.update([ground2],t0+10000);
assert.equal(ground2.properties.vs_fpm,null);

// Who is up: provider-listed military only, unattributed last.
const pic=m.militaryPicture([
  {properties:{military:true,callsign:'RRR1',aircraft_type:'A400',shape:'heavy'}},
  {properties:{military:true,icao_hex:'43C7A4',aircraft_type:'EUFI',shape:'fighter'}},
  {properties:{military:true,aircraft_type:'H60',shape:'heli'}},
  {properties:{military:false,callsign:'BAW1'}},
  {properties:{military:null,callsign:'RCH1'}},
]);
assert.equal(pic.total,3);
assert.deepEqual(pic.rows.map(r=>[r.code,r.count]),[['GB',2],['',1]]);
assert.deepEqual(pic.rows[0].roles.map(([r])=>r).sort(),['airlift','fighter']);
console.log('Passed: aircraft operators, Mode S states, type names and roles from Relic tables without guesses; climb rates and trails from successive reports only; military picture from the provider category.');
