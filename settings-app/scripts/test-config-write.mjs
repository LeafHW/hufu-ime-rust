// Exercises the real TypeScript API against an in-memory HTTP contract.
// No live HuFu service or user configuration is accessed.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import ts from 'typescript';

const moduleUrl = (source) => `data:text/javascript;base64,${Buffer.from(source).toString('base64')}`;
const compile = (source) => ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
}).outputText;
const types = moduleUrl(compile(await readFile(new URL('../src/types.ts', import.meta.url), 'utf8')));
const api = compile(await readFile(new URL('../src/api.ts', import.meta.url), 'utf8'))
  .replace(/from ['"]\.\/types['"]/, `from '${types}'`);
const { updateConfig } = await import(moduleUrl(api));

let state;
let failNext = false;
let conflictNext = false;
let allowConflict = true;
globalThis.window = { confirm: () => allowConflict };
globalThis.fetch = async (url, init = {}) => {
  assert.equal(url, '/api/config');
  if (init.method !== 'POST') return Response.json(structuredClone(state));
  const body = JSON.parse(init.body);
  if (failNext) {
    failNext = false;
    return Response.json({ error: 'disk unavailable' }, { status: 500 });
  }
  if (conflictNext) {
    conflictNext = false;
    state.appearance.skin = 'external-skin';
    state.rev += 1;
  }
  if (body.rev !== state.rev) {
    return Response.json({ error: 'conflict' }, { status: 409 });
  }
  state = { ...body, rev: state.rev + 1 };
  return Response.json({ ok: true, rev: state.rev });
};

const reset = () => {
  state = {
    rev: 1,
    appearance: { anim: true, skin: 'initial' },
    sound: { enabled: true, volume: 70 },
    keymap: {
      map: { F1: 'old', F2: 'keep' },
      map_idle: { F1: 'old-idle' },
      shift_symbols: { a: 'old-symbol' },
    },
  };
};

reset();
await updateConfig((fresh) => ({
  ...fresh,
  keymap: { ...fresh.keymap, map: { F2: 'keep' }, map_idle: {}, shift_symbols: {} },
}));
assert.deepEqual(state.keymap.map, { F2: 'keep' }, 'deleted mapping must stay deleted');
assert.deepEqual(state.keymap.map_idle, {}, 'empty map replaces previous map');
assert.deepEqual(state.keymap.shift_symbols, {}, 'resetting symbols removes custom symbols');
assert.equal(state.sound.volume, 70, 'unrelated configuration remains intact');

reset();
const animation = updateConfig({ appearance: { anim: false } });
const keymap = updateConfig((fresh) => ({
  ...fresh,
  keymap: { ...fresh.keymap, map: {} },
}));
await Promise.all([animation, keymap]);
assert.equal(state.appearance.anim, false, 'queued replacement must use the preceding successful write');
assert.deepEqual(state.keymap.map, {});
assert.equal(state.rev, 3);

reset();
const patch = await updateConfig({ sound: { enabled: false } });
assert.equal(state.sound.volume, 70, 'ordinary patch keeps nested fields');
assert.equal(patch.rev, state.rev, 'returned snapshot carries the committed revision');

reset();
conflictNext = true;
const refreshed = await updateConfig((fresh) => ({ ...fresh, sound: { ...fresh.sound, enabled: false } }));
assert.equal(refreshed.appearance.skin, 'external-skin', 'confirmed CAS retry recomputes from fresh state');
assert.equal(state.sound.enabled, false);

reset();
conflictNext = true;
allowConflict = false;
await assert.rejects(updateConfig({ sound: { enabled: false } }), /未保存/);
assert.equal(state.sound.enabled, true, 'cancelled conflict does not apply the edit');
allowConflict = true;
await updateConfig({ sound: { enabled: false } });
assert.equal(state.sound.enabled, false, 'cancelled write does not poison the queue');

reset();
failNext = true;
await assert.rejects(updateConfig({ sound: { enabled: false } }), /disk unavailable/);
await updateConfig({ sound: { volume: 25 } });
assert.equal(state.sound.enabled, true, 'failed write is not applied');
assert.equal(state.sound.volume, 25, 'failed write does not poison subsequent writes');

console.log('PASS: mapping deletion/reset, queued fresh updates, nested patches, revision, CAS retry/cancel, failure recovery');
