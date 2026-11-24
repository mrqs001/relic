// No provider requests. Countdown expiry and timezone-equivalent timestamps.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {createRequire} from 'node:module';
const require=createRequire(new URL('../web/package.json',import.meta.url));
const {transform}=require('esbuild');
const {code}=await transform(readFileSync(new URL('../web/src/components/connection-retry.ts',import.meta.url),'utf8'),{loader:'ts',format:'esm'});
const {retrySeconds,retryLabel}=await import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);
const now=Date.parse('2026-09-12T21:00:00Z');
assert.equal(retrySeconds(null,now),0);
assert.equal(retrySeconds('2026-09-12T21:00:00Z',now),0);
assert.equal(retrySeconds('2026-09-12T20:59:59Z',now),0);
assert.equal(retrySeconds('2026-09-12T21:00:59.500Z',now),60);
assert.equal(retrySeconds('2026-09-12T22:00:59.500+01:00',now),60);
assert.equal(retrySeconds('invalid',now),0);
assert.equal(retryLabel(15),'Retry in 15s');
assert.equal(retryLabel(61),'Retry in 2m');
assert.equal(retryLabel(3600),'Retry in 1h');
console.log(JSON.stringify({outcome:'passed',countdown_and_expiry:true,timezone_equivalence:true,provider_requests:0}));
