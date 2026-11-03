'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');

// ---- a tiny fake of the Chrome (callback style) extension API --------------
function makeChrome(opts = {}) {
  const calls = { native: [], cancel: [], erase: [], cookies: [] };
  const store = Object.assign({}, opts.storage || {});
  const chrome = {
    runtime: {
      id: 'test-id',
      lastError: null,
      sendNativeMessage(host, msg, cb) {
        calls.native.push({ host, msg });
        setImmediate(() => {
          if (opts.nativeError) { chrome.runtime.lastError = { message: opts.nativeError }; cb(undefined); chrome.runtime.lastError = null; }
          else cb(opts.reply || { ok: true, version: '0.7.0' });
        });
      },
    },
    storage: { local: {
      get(defaults, cb) { cb(Object.assign({}, defaults, store)); },
      set(patch, cb) { Object.assign(store, patch); cb && cb(); },
    } },
    cookies: { getAll(q, cb) { calls.cookies.push(q); cb(opts.cookies || []); } },
    downloads: {
      cancel(id, cb) { calls.cancel.push(id); cb(); },
      erase(q, cb) { calls.erase.push(q); cb([q.id]); },
    },
  };
  return { chrome, calls, store };
}

function load(opts) {
  const env = makeChrome(opts);
  global.chrome = env.chrome;
  delete global.browser;
  Object.defineProperty(globalThis, 'navigator', { value: { userAgent: 'TestAgent/1.0' }, configurable: true, writable: true });
  delete require.cache[require.resolve('../common.js')];
  const V = require('../common.js');
  return Object.assign({ V }, env);
}

test('hands an http download to SDM, then cancels and erases the browser download', async () => {
  const { V, calls } = load({ cookies: [{ name: 'sid', value: 'abc' }, { name: 'x', value: '1' }] });
  const ok = await V.captureDownload({ id: 7, url: 'https://example.com/file.zip', referrer: 'https://example.com/page', filename: '/home/u/Downloads/file.zip', fileSize: 5000000, mime: 'application/zip' });
  assert.equal(ok, true);
  assert.equal(calls.native.length, 1);
  const m = calls.native[0];
  assert.equal(m.host, 'com.sdm.host');
  assert.equal(m.msg.type, 'add');
  assert.equal(m.msg.url, 'https://example.com/file.zip');
  assert.equal(m.msg.headers.Cookie, 'sid=abc; x=1');
  assert.equal(m.msg.headers.Referer, 'https://example.com/page');
  assert.equal(m.msg.headers['User-Agent'], 'TestAgent/1.0');
  assert.equal(m.msg.filename, 'file.zip');
  assert.deepEqual(calls.cancel, [7]);
  assert.deepEqual(calls.erase, [{ id: 7 }]);
});

test('never cancels the browser download when SDM is unreachable', async () => {
  const { V, calls, store } = load({ nativeError: 'Specified native messaging host not found.' });
  const ok = await V.captureDownload({ id: 1, url: 'https://example.com/a.iso' });
  assert.equal(ok, false);
  assert.deepEqual(calls.cancel, []);
  assert.deepEqual(calls.erase, []);
  assert.equal(store.lastStatus.ok, false);
  assert.match(store.lastStatus.error, /not found/);
});

test('never cancels when SDM answers with an error', async () => {
  const { V, calls, store } = load({ reply: { ok: false, error: 'Not a web link' } });
  assert.equal(await V.captureDownload({ id: 2, url: 'https://example.com/a.iso' }), false);
  assert.deepEqual(calls.cancel, []);
  assert.equal(store.lastStatus.error, 'Not a web link');
});

test('leaves blob:, data: and file: downloads to the browser', async () => {
  const { V, calls } = load();
  for (const url of ['blob:https://x/1', 'data:text/plain,hi', 'file:///tmp/a', 'ftp://x/a']) {
    assert.equal(await V.captureDownload({ id: 3, url }), false, url);
  }
  assert.equal(calls.native.length, 0);
});

test('does nothing while capture is switched off', async () => {
  const { V, calls } = load({ storage: { enabled: false } });
  assert.equal(await V.captureDownload({ id: 4, url: 'https://example.com/a.zip' }), false);
  assert.equal(calls.native.length, 0);
});

test('skips files below the minimum size, but not unknown sizes', async () => {
  const { V, calls } = load({ storage: { minSizeMB: 10 } });
  assert.equal(await V.captureDownload({ id: 5, url: 'https://example.com/small.zip', fileSize: 2 * 1048576 }), false);
  assert.equal(calls.native.length, 0);
  assert.equal(await V.captureDownload({ id: 6, url: 'https://example.com/unknown.zip', fileSize: -1 }), true);
  assert.equal(await V.captureDownload({ id: 8, url: 'https://example.com/big.zip', fileSize: 50 * 1048576 }), true);
});

test('right-click "Download with SDM" sends the link with the page as referer', async () => {
  const { V, calls } = load();
  assert.equal(await V.captureLink('https://example.com/f.bin', 'https://example.com/downloads'), true);
  assert.equal(calls.native[0].msg.headers.Referer, 'https://example.com/downloads');
  assert.deepEqual(calls.cancel, []);
  assert.equal(await V.captureLink('javascript:alert(1)', ''), false);
});

test('ping reports the version, or the reason it failed', async () => {
  let env = load();
  assert.deepEqual(await env.V.ping(), { ok: true, version: '0.7.0' });
  env = load({ nativeError: 'host not found' });
  const r = await env.V.ping();
  assert.equal(r.ok, false);
  assert.match(r.error, /host not found/);
});

test('Firefox-style promise API works too', async () => {
  const sent = [];
  global.browser = {
    runtime: { id: 'ff', sendNativeMessage: async (h, m) => { sent.push(m); return { ok: true, version: 'x' }; } },
    storage: { local: { get: async (d) => d, set: async () => {} } },
    cookies: { getAll: async () => [{ name: 'a', value: 'b' }] },
    downloads: { cancel: async () => {}, erase: async () => [] },
  };
  delete global.chrome;
  delete require.cache[require.resolve('../common.js')];
  const V = require('../common.js');
  assert.equal(await V.captureDownload({ id: 9, url: 'https://example.com/z.zip' }), true);
  assert.equal(sent[0].headers.Cookie, 'a=b');
  delete global.browser;
});
