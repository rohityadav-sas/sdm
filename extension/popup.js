(async function () {
  const V = SdmCommon;
  const $ = (id) => document.getElementById(id);

  const s = await V.getSettings();
  $('enabled').checked = !!s.enabled;
  $('min').value = s.minSizeMB || 0;
  $('enabled').addEventListener('change', () => V.setSettings({ enabled: $('enabled').checked }));
  $('min').addEventListener('change', () => V.setSettings({ minSizeMB: Math.max(0, Number($('min').value) || 0) }));

  async function check() {
    const box = $('status');
    box.className = 'status';
    box.textContent = 'Checking connection…';
    const r = await V.ping();
    if (r.ok) {
      box.className = 'status ok';
      box.textContent = 'Connected to SDM' + (r.version ? ' ' + r.version : '') + '.';
    } else {
      box.className = 'status bad';
      box.textContent = 'Can’t reach SDM: ' + r.error + ' — open SDM → Settings → Browser extension → Set up.';
    }
  }
  $('test').addEventListener('click', check);
  check();
})();
