// Verify the browser media boundary without contacting any provider.
// node validation/check_evidence_media.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';

const require = createRequire(new URL('../web/package.json', import.meta.url));
const { transform } = require('esbuild');
const { code } = await transform(readFileSync(new URL('../web/src/components/evidence-media.ts', import.meta.url), 'utf8'), { loader: 'ts', format: 'esm' });
const { controlledMediaUrl, mediaPresentation, mediaLabel } = await import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);
const identity = { id: 'x:post:18446744073709551000', revision: 'sha256:exact-revision', index: 0 };
const query = new URLSearchParams({ id: identity.id, revision: identity.revision, index: String(identity.index) }).toString();
const local = `/api/v1/media?${query}`;
assert.equal(controlledMediaUrl(local, identity), local);
const rejected = [
  `https://pbs.twimg.com/photo.jpg`, `https://example.test${local}`, `//pbs.twimg.com/photo.jpg`,
  `data:image/png;base64,AAAA`, `blob:http://relic.local/file`, `javascript:alert(1)`,
  `/\\pbs.twimg.com/photo.jpg`, `/api/v1/objects/hash?${query}`, `${local}#other`,
  `${local}&index=0`, `${local.replace('index=0', 'index=1')}`, `${local.replace('exact-revision', 'other-revision')}`,
  `/api/v1/media?${new URLSearchParams({ ...identity, id: 'x:post:123', index: '0' })}`,
];
for (const url of rejected) assert.throws(() => controlledMediaUrl(url, identity), Error, url);
for (const [mime, expected] of [
  ['image/jpeg', 'image'], ['image/png; charset=binary', 'image'], ['video/mp4', 'video'],
  ['audio/ogg', 'audio'], ['image/svg+xml', 'download'], ['text/html', 'download'],
  ['application/pdf', 'download'], ['application/octet-stream', 'download'],
]) assert.equal(mediaPresentation(mime), expected);
assert.equal(mediaLabel({ kind: 'photo', preview_url: 'https://pbs.twimg.com/photo.jpg' }), 'Image');
assert.equal(mediaLabel({ kind: 'document', mime_type: 'video/mp4' }), 'Video');
assert.equal(mediaLabel(null), 'Media');
console.log(JSON.stringify({ outcome: 'passed', unsafe_or_mismatched_urls_rejected: rejected.length, type_cases: 8, provider_requests: 0 }));

const { resolvedMediaReceipt } = await import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);
const telegram = { id: 'telegram:100:-1000000000200:42', revision: `sha256:${'a'.repeat(64)}`, index: 0 };
const newRevision = `sha256:${'b'.repeat(64)}`;
const resolvedUrl = `/api/v1/media?${new URLSearchParams({id:telegram.id, revision:newRevision, index:'0'})}`;
assert.equal(resolvedMediaReceipt({url:resolvedUrl,revision:newRevision},telegram).revision,newRevision);
assert.throws(() => controlledMediaUrl(resolvedUrl,telegram)); // New bytes must never render as the old revision.
assert.throws(() => resolvedMediaReceipt({url:resolvedUrl},telegram));
assert.throws(() => resolvedMediaReceipt({url:resolvedUrl,revision:newRevision},identity));
assert.throws(() => resolvedMediaReceipt({url:resolvedUrl,revision:'malformed'},telegram));
assert.throws(() => resolvedMediaReceipt({url:'https://telegram.example.test/photo.jpg',revision:newRevision},telegram));
assert.equal(resolvedMediaReceipt({url:local},identity).revision,identity.revision);
console.log(JSON.stringify({outcome:'passed',telegram_revision_recovery:true,old_revision_substitution_rejected:true,provider_requests:0}));
