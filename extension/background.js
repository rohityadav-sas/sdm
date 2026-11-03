// Chromium (MV3 service worker) loads common.js through importScripts;
// Firefox lists common.js before this file in its manifest.
if (typeof importScripts === 'function' && typeof SdmCommon === 'undefined') {
  importScripts('common.js');
}

(function () {
  'use strict';
  const api = typeof browser !== 'undefined' ? browser : chrome;
  const V = SdmCommon;

  // Every download the browser starts is offered to SDM first.
  api.downloads.onCreated.addListener((item) => {
    V.captureDownload(item).catch(() => {});
  });

  function setupMenu() {
    try {
      api.contextMenus.removeAll(() => {
        api.contextMenus.create({
          id: 'sdm-download',
          title: 'Download with SDM',
          contexts: ['link', 'video', 'audio', 'image'],
        });
      });
    } catch (_) { /* menus unavailable */ }
  }
  if (api.runtime.onInstalled) api.runtime.onInstalled.addListener(setupMenu);
  if (api.runtime.onStartup) api.runtime.onStartup.addListener(setupMenu);

  if (api.contextMenus && api.contextMenus.onClicked) {
    api.contextMenus.onClicked.addListener((info) => {
      if (info.menuItemId !== 'sdm-download') return;
      V.captureLink(info.linkUrl || info.srcUrl, info.pageUrl).catch(() => {});
    });
  }
})();
