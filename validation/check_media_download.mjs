// Protocol and cancellation checks with a local synthetic response, no provider calls.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
const require = createRequire(new URL('../web/package.json', import.meta.url));
const { build } = require('esbuild');
const { outputFiles } = await build({ entryPoints: [new URL('../web/src/components/media-download.ts', import.meta.url).pathname], bundle: true, write: false, format: 'esm', platform: 'node' });
const { downloadMedia } = await import(`data:text/javascript;base64,${Buffer.from(outputFiles[0].contents).toString('base64')}`);
const identity = { id: 'telegram:100:-1000000000200:42', revision: `sha256:${'a'.repeat(64)}`, index: 0 };
const receipt = { url: `/api/v1/media?${new URLSearchParams({...identity,index:'0'})}`, revision: identity.revision, bytes: 1024, media_type: 'video/mp4' };
const event = v => JSON.stringify(v) + '\n';
const progress = (downloaded, total, phase='downloading') => ({kind:'progress', phase, downloaded, total});
let cancelled = 0;
let options;
const originalFetch = globalThis.fetch;
function response(parts, open=false) {
  globalThis.fetch = async (url, opts) => {
    assert.equal(url, '/api/v1/media/download'); options=opts;
    return new Response(new ReadableStream({
      start(controller) {
        for (const part of parts) controller.enqueue(new TextEncoder().encode(part));
        if (!open) controller.close();
        opts.signal.addEventListener('abort', () => controller.error(opts.signal.reason), {once:true});
      },
      cancel() { cancelled++; },
    }), {headers:{'content-type':'application/x-ndjson'}});
  };
}
try {
  const events = event(progress(0,null)) + event(progress(512,1024)) + event(progress(1024,1024,'verifying')) + event({kind:'ready',receipt});
  response([...events]); // Split JSON across arbitrary byte chunks.
  const updates=[]; const controller=new AbortController();
  assert.deepEqual(await downloadMedia(identity,controller.signal,v=>updates.push(v)),receipt);
  assert.equal(options.signal,controller.signal);
  assert.deepEqual(JSON.parse(options.body), identity);
  assert.deepEqual(updates.map(v=>v.downloaded),[0,512,1024]);
  assert.equal(updates[0].total,null);
  assert.equal(updates.at(-1).phase,'verifying');
  response([event(progress(512,1024))],true);
  const stop = new AbortController();
  const pending=downloadMedia(identity,stop.signal,()=>stop.abort());
  await assert.rejects(pending,{name:'AbortError'});
  response([event(progress(512,1024))]);
  await assert.rejects(downloadMedia(identity,new AbortController().signal,()=>{}), /ended before/);
  response([event({kind:'error',code:'media_unavailable',message:'Unavailable'})]);
  await assert.rejects(downloadMedia(identity,new AbortController().signal,()=>{}), /Unavailable/);
  for(const malformed of [event(progress(1025,1024)), event(progress(-1,1024)), 'x'.repeat(17000), event({kind:'ready',receipt:{...receipt,url:'https://provider.invalid/movie.mp4'}})]) {
    response([malformed],true);
    const before=cancelled;
    await assert.rejects(downloadMedia(identity,new AbortController().signal,()=>{}));
    assert.equal(cancelled,before+1); // Contract failures also stop the server transfer.
  }
  response([event({kind:'ready',receipt})],true);
  const before=cancelled;
  await downloadMedia(identity,new AbortController().signal,()=>{});
  assert.equal(cancelled,before+1);
  console.log(JSON.stringify({outcome:'passed',byte_progress:true,unknown_size:true,split_frames:true,abort:true,incomplete_rejected:true,malformed_cancelled:4,exact_local_receipt:true,provider_requests:0}));
} finally {globalThis.fetch=originalFetch;}
