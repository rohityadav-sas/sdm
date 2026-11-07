// Shared by the background script and the popup. Works with both the Chrome
// (callback) and Firefox (promise) flavours of the WebExtension APIs.
(function (root) {
  'use strict';

  const HOST = 'com.sdm.host';
  const DEFAULTS = { enabled: true, minSizeMB: 0 };
  const api = () => (typeof browser !== 'undefined' ? browser : chrome);
  const isPromiseApi = () => typeof browser !== 'undefined';

  // Invoke target[method](...args) and always return a Promise.
  function invoke(target, method, ...args) {
    if (isPromiseApi()) return Promise.resolve(target[method](...args));
    return new Promise((resolve, reject) => {
      target[method](...args, (result) => {
        const err = chrome.runtime && chrome.runtime.lastError;
        if (err) reject(new Error(err.message));
        else resolve(result);
      });
    });
  }

  function call(ns, method, ...args) {
    return invoke(api()[ns], method, ...args);
  }

  function sendNative(message) {
    return invoke(api().runtime, 'sendNativeMessage', HOST, message);
  }

  async function getSettings() {
    try {
      const v = await invoke(api().storage.local, 'get', DEFAULTS);
      return Object.assign({}, DEFAULTS, v || {});
    } catch (_) {
      return Object.assign({}, DEFAULTS);
    }
  }

  async function setSettings(patch) {
    try {
      await invoke(api().storage.local, 'set', patch);
    } catch (_) { /* ignore */ }
  }

  async function cookieHeader(url) {
    try {
      const cookies = await invoke(api().cookies, 'getAll', { url });
      return (cookies || []).map((c) => c.name + '=' + c.value).join('; ');
    } catch (_) {
      return '';
    }
  }

  function basename(p) {
    if (!p) return '';
    return String(p).split(/[\\/]/).pop();
  }

  async function buildPayload(url, extra) {
    const headers = {};
    const cookie = await cookieHeader(url);
    if (cookie) headers['Cookie'] = cookie;
    if (extra && extra.referrer) headers['Referer'] = extra.referrer;
    if (typeof navigator !== 'undefined' && navigator.userAgent) headers['User-Agent'] = navigator.userAgent;
    const payload = { type: 'add', url, headers, source: 'extension' };
    if (extra && extra.filename) payload.filename = basename(extra.filename);
    if (extra && extra.size > 0) payload.size = extra.size;
    if (extra && extra.mime) payload.mime = extra.mime;
    return payload;
  }

  async function remember(status) {
    await setSettings({ lastStatus: Object.assign({ at: Date.now() }, status) });
  }

  /**
   * Hand a browser download over to SDM.
   * Returns true when SDM accepted it and the browser's own download was cancelled.
   * The browser download is only cancelled after SDM confirmed, so a missing or
   * crashed SDM never loses a download.
   */
  async function captureDownload(item) {
    const settings = await getSettings();
    if (!settings.enabled) return false;
    const url = item.url || item.finalUrl || '';
    if (!/^https?:\/\//i.test(url)) return false; // blob:, data:, file: ... stay with the browser
    if (settings.minSizeMB > 0 && item.fileSize > 0 && item.fileSize < settings.minSizeMB * 1048576) return false;

    const payload = await buildPayload(url, {
      referrer: item.referrer, filename: item.filename, size: item.fileSize, mime: item.mime,
    });
    let reply;
    try {
      reply = await sendNative(payload);
    } catch (e) {
      await remember({ ok: false, error: e && e.message ? e.message : String(e) });
      return false;
    }
    if (!reply || !reply.ok) {
      await remember({ ok: false, error: (reply && reply.error) || 'SDM did not accept the download' });
      return false;
    }
    await remember({ ok: true, url });
    try { await call('downloads', 'cancel', item.id); } catch (_) { /* already finished/cancelled */ }
    try { await call('downloads', 'erase', { id: item.id }); } catch (_) { /* ignore */ }
    return true;
  }

  /** "Download with SDM" from the right-click menu. */
  async function captureLink(url, pageUrl) {
    if (!/^https?:\/\//i.test(url || '')) return false;
    const payload = await buildPayload(url, { referrer: pageUrl });
    try {
      const reply = await sendNative(payload);
      const ok = !!(reply && reply.ok);
      await remember(ok ? { ok: true, url } : { ok: false, error: (reply && reply.error) || 'SDM did not accept the link' });
      return ok;
    } catch (e) {
      await remember({ ok: false, error: e && e.message ? e.message : String(e) });
      return false;
    }
  }

  async function ping() {
    try {
      const reply = await sendNative({ type: 'ping', source: 'extension' });
      return reply && reply.ok ? { ok: true, version: reply.version || '' } : { ok: false, error: (reply && reply.error) || 'No answer from SDM' };
    } catch (e) {
      return { ok: false, error: e && e.message ? e.message : String(e) };
    }
  }

  const exported = { HOST, DEFAULTS, getSettings, setSettings, captureDownload, captureLink, ping, buildPayload, basename };
  if (typeof module !== 'undefined' && module.exports) module.exports = exported;
  root.SdmCommon = exported;
})(typeof globalThis !== 'undefined' ? globalThis : this);
