// Real production request guards/API with controllable promises and a loopback
// server. No installed HuFu process, production files or external network.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import ts from 'typescript';

const moduleUrl = (source) => `data:text/javascript;base64,${Buffer.from(source).toString('base64')}`;
const compile = (source) => ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
}).outputText;
const source = async (path) => compile(await readFile(new URL(path, import.meta.url), 'utf8'));
const types = moduleUrl(await source('../src/types.ts'));
const apiUrl = moduleUrl((await source('../src/api.ts')).replace(/from ['"]\.\/types['"]/, `from '${types}'`));
const { fetchWithTimeout } = await import(apiUrl);
const { createSnapshotRequest } = await import(moduleUrl(await source('../src/snapshotRequest.ts')));
const { fetchDownloadState, cancelUpdateDownload } = await import(moduleUrl(
  (await source('../src/updateCheck.ts')).replace(/from ['"]\.\/api['"]/, `from '${apiUrl}'`),
));
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

let next = deferred();
let calls = 0;
const values = [], errors = [];
const reader = createSnapshotRequest(() => { calls += 1; return next.promise; }, (v) => values.push(v), (e) => errors.push(e));
const first = reader.refresh(true);
for (let i = 0; i < 50; i += 1) assert.equal(reader.refresh(true), first);
await Promise.resolve();
assert.equal(calls, 1, 'slow polling/focus events must not accumulate requests');
reader.invalidate(); // start/cancel completed after this read started
next.resolve({ phase: 'downloading' });
await first;
assert.deepEqual(values, [], 'an old read cannot overwrite a newer action result');
next = deferred();
const fresh = reader.refresh(true);
next.resolve({ phase: 'done' });
await fresh;
assert.deepEqual(values, [{ phase: 'done' }], 'fresh state is still published');
next = deferred();
const staleError = reader.refresh(true);
reader.invalidate();
next.reject(new Error('old request'));
await staleError;
assert.deepEqual(errors, [], 'obsolete failures cannot replace a successful action');
next = deferred();
const detached = reader.refresh(true);
reader.dispose();
next.resolve({ phase: 'downloading' });
await detached;
assert.equal(values.length, 1, 'unmounted readers cannot publish');
await reader.refresh(true);
assert.equal(calls, 4, 'disposed readers cannot issue requests');

const nativeFetch = globalThis.fetch;
let wire = deferred();
let wireCalls = 0;
globalThis.fetch = async (url) => {
  assert.equal(url, '/api/update/download/status');
  wireCalls += 1;
  return wire.promise;
};
const overview = fetchDownloadState(), about = fetchDownloadState();
assert.equal(overview, about, 'two mounted pages share the same pending read');
wire.resolve(Response.json({ phase: 'downloading', downloaded: 10 }));
assert.equal((await overview).downloaded, 10);
await about;
assert.equal(wireCalls, 1);
wire = deferred();
const failed = fetchDownloadState();
wire.reject(new Error('offline'));
await assert.rejects(failed, /offline/);
wire = deferred();
const retried = fetchDownloadState();
wire.resolve(Response.json({ phase: 'done' }));
assert.equal((await retried).phase, 'done', 'a failed read must not poison later polling');
globalThis.fetch = async () => new Response('', { status: 503 });
await assert.rejects(cancelUpdateDownload(), /503/, 'HTTP cancellation failures must surface');
globalThis.fetch = nativeFetch;

let bodyStarted;
let cancelled = 0;
const server = createServer((req, res) => {
  if (req.url === '/slow') {
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.write('{"phase":');
    bodyStarted?.();
    req.on('close', () => { cancelled += 1; });
    // Intentionally never finish the response body.
  } else if (req.url === '/binary') {
    res.writeHead(206, { 'Content-Type': 'application/octet-stream', 'X-Fixture': 'yes' });
    res.end(Buffer.from([0, 255, 128, 65]));
  } else {
    res.writeHead(Number(req.url.slice(1)));
    res.end();
  }
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const base = `http://127.0.0.1:${server.address().port}`;
try {
  const started = Date.now();
  await assert.rejects(fetchWithTimeout(`${base}/slow`, undefined, 150), /请求超时/);
  assert.ok(Date.now() - started < 2000, 'receiving headers must not disable the body deadline');
  const binary = await fetchWithTimeout(`${base}/binary`);
  assert.equal(binary.status, 206);
  assert.equal(binary.headers.get('X-Fixture'), 'yes');
  assert.deepEqual([...new Uint8Array(await binary.arrayBuffer())], [0, 255, 128, 65]);
  for (const status of [204, 205, 304]) {
    const empty = await fetchWithTimeout(`${base}/${status}`);
    assert.equal(empty.status, status);
    assert.equal(empty.body, null, `${status} must retain its null body`);
    assert.equal(await empty.text(), '');
  }
  const ctrl = new AbortController();
  const atHeaders = deferred();
  bodyStarted = atHeaders.resolve;
  const active = fetchWithTimeout(`${base}/slow`, { signal: ctrl.signal }, 5000);
  await atHeaders.promise;
  ctrl.abort();
  await assert.rejects(active, (e) => e.name === 'AbortError' && !e.message.includes('请求超时'));
  const already = new AbortController();
  already.abort();
  await assert.rejects(fetchWithTimeout(`${base}/binary`, { signal: already.signal }), { name: 'AbortError' });
} finally {
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
}
assert.ok(cancelled >= 1, 'aborted body readers must close their connection');
console.log('request lifecycle: polling, stale results, coalescing, failures, body deadline, binary/null-body and abort checks passed');
