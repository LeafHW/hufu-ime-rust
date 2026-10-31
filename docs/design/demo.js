(function () {
  const $ = (q, el = document) => el.querySelector(q);
  const $$ = (q, el = document) => Array.from(el.querySelectorAll(q));

  const screens = { overview: '概览', skin: '皮肤工作室', sync: '云同步' };
  const skins = {
    moyan: { name: '墨言', rgb: [27, 30, 36], hl1: '#f2c66d', hl2: '#e0aa48', radius: 12, accent: '#e7b75c' },
    yuzhu: { name: '雨竹', rgb: [24, 38, 35], hl1: '#b7d8c6', hl2: '#8fb8a1', radius: 10, accent: '#9fc7b4' },
    jingyu: { name: '鲸语', rgb: [27, 28, 45], hl1: '#c9bcf5', hl2: '#a797e8', radius: 14, accent: '#b8a9ef' }
  };

  const cand = $('.stage .cand');
  const stage = $('.stage');
  const inspName = $('.insp-name b');
  const capName = $('.stage-cap b');

  let currentSkin = 'moyan';
  let alpha = 0.92;
  let radius = 12;
  let shadow = 32;
  let ab = false;
  let toastTimer = 0;

  function toast(msg) {
    const t = $('#toast');
    if (!t) return;
    t.textContent = msg;
    t.classList.add('show');
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => t.classList.remove('show'), 2400);
  }

  function setScreen(s, push) {
    if (!screens[s]) {
      toast('这页在视觉稿里还没展开');
      return;
    }
    document.body.dataset.screen = s;
    $$('.nav-item').forEach(el => el.classList.toggle('on', el.dataset.nav === s));
    $$('.tb-page').forEach(el => { el.style.display = 'none'; });
    const pn = $('.pn-' + s);
    if (pn) pn.style.display = 'inline';
    if (push) {
      try { history.replaceState(null, '', '#' + s); } catch (_) {}
    }
  }

  function setSlider(sl, val) {
    const min = +sl.dataset.min;
    const max = +sl.dataset.max;
    const p = Math.max(0, Math.min(1, (val - min) / (max - min)));
    sl.querySelector('i').style.width = (p * 100) + '%';
    sl.querySelector('b').style.left = (p * 100) + '%';
    sl.dataset.val = val;
  }

  function syncSliders() {
    $$('.slider[data-var]').forEach(sl => {
      if (sl.dataset.var === 'alpha') setSlider(sl, alpha * 100);
      if (sl.dataset.var === 'radius') setSlider(sl, radius);
      if (sl.dataset.var === 'shadow') setSlider(sl, shadow);
    });
    const vr = $('#v-radius');
    if (vr) vr.textContent = Math.round(radius) + ' px';
  }

  function paintCand() {
    const sk = skins[currentSkin];
    cand.style.background = 'rgba(' + sk.rgb.join(',') + ',' + alpha + ')';
    cand.style.borderRadius = radius + 'px';
    cand.style.boxShadow = '0 ' + Math.round(shadow * 0.5) + 'px ' + Math.round(shadow * 1.6) + 'px rgba(0,0,0,.55)';
    cand.style.setProperty('--cand-hl1', sk.hl1);
    cand.style.setProperty('--cand-hl2', sk.hl2);
    cand.style.setProperty('--cand-accent', sk.accent);
  }

  function applySkin(key) {
    const sk = skins[key];
    if (!sk) return;
    currentSkin = key;
    alpha = 0.92;
    radius = sk.radius;
    shadow = 32;
    ab = false;
    cand.classList.remove('ab');
    const abBtn = $('#ab-btn');
    if (abBtn) abBtn.textContent = 'A/B';
    inspName.textContent = sk.name;
    capName.textContent = sk.name;
    $$('.skin-item').forEach(el => el.classList.toggle('on', el.dataset.skin === key));
    paintCand();
    syncSliders();
  }

  function bindSliders() {
    $$('.slider[data-var]').forEach(sl => {
      const apply = clientX => {
        if (ab) return;
        const r = sl.getBoundingClientRect();
        const p = Math.max(0, Math.min(1, (clientX - r.left) / r.width));
        const min = +sl.dataset.min;
        const max = +sl.dataset.max;
        const v = min + (max - min) * p;
        setSlider(sl, v);
        if (sl.dataset.var === 'alpha') {
          alpha = v / 100;
          paintCand();
        }
        if (sl.dataset.var === 'radius') {
          radius = v;
          paintCand();
          const vr = $('#v-radius');
          if (vr) vr.textContent = Math.round(v) + ' px';
        }
        if (sl.dataset.var === 'shadow') {
          shadow = v;
          paintCand();
        }
      };
      sl.addEventListener('pointerdown', e => {
        if (ab) return;
        sl.setPointerCapture(e.pointerId);
        apply(e.clientX);
        const move = ev => apply(ev.clientX);
        const up = () => {
          sl.removeEventListener('pointermove', move);
          sl.removeEventListener('pointerup', up);
        };
        sl.addEventListener('pointermove', move);
        sl.addEventListener('pointerup', up, { once: true });
      });
    });
  }

  function setTheme(mode) {
    let light = mode === 'light';
    if (mode === 'system') light = matchMedia('(prefers-color-scheme: light)').matches;
    document.body.classList.toggle('light', light);
    $$('.theme-seg .ts').forEach(el => el.classList.toggle('on', el.dataset.theme === mode));
  }

  // 侧栏导航
  $$('.nav-item').forEach(el => {
    el.style.cursor = 'pointer';
    el.addEventListener('click', () => setScreen(el.dataset.nav, true));
  });

  // 主题三态
  $$('.theme-seg .ts').forEach((el, i) => {
    el.dataset.theme = ['system', 'dark', 'light'][i];
    el.style.cursor = 'pointer';
    el.addEventListener('click', () => setTheme(el.dataset.theme));
  });

  // 皮肤列表
  $$('.skin-item[data-skin]').forEach(el => {
    el.style.cursor = 'pointer';
    el.addEventListener('click', () => {
      if (!ab) applySkin(el.dataset.skin);
    });
  });

  // 预览背景
  const bgNames = ['board', 'dark', 'light', 'wall'];
  $$('.stage-toolbar .seg .sg').forEach((el, i) => {
    el.dataset.bg = bgNames[i];
    el.style.cursor = 'pointer';
    el.addEventListener('click', () => {
      $$('.stage-toolbar .seg .sg').forEach(x => x.classList.remove('on'));
      el.classList.add('on');
      stage.classList.remove('bg-board', 'bg-dark', 'bg-light', 'bg-wall');
      stage.classList.add('bg-' + el.dataset.bg);
    });
  });
  stage.classList.add('bg-wall');

  // A/B
  const abBtn = $('#ab-btn');
  if (abBtn) {
    abBtn.addEventListener('click', () => {
      ab = !ab;
      cand.classList.toggle('ab', ab);
      abBtn.textContent = ab ? '回到编辑' : 'A/B';
      toast(ab ? 'A/B：显示已保存版本' : 'A/B：回到编辑版本');
    });
  }

  // 真机试打
  const tryBtn = $('#try-btn');
  if (tryBtn) {
    tryBtn.addEventListener('click', () => {
      toast('真机试打：正式版会在设置窗内直接出现真实候选窗');
    });
  }

  // 演示按钮统一反馈
  $$('.btn.primary').forEach(b => {
    b.addEventListener('click', () => toast('演示按钮：正式版会执行真实操作'));
  });

  // 初始化
  const q = new URLSearchParams(location.search);
  let s = (location.hash || '').replace('#', '') || q.get('s') || 'overview';
  if (s.endsWith('-light')) {
    s = s.slice(0, -6);
    document.body.classList.add('light');
  }
  if (q.get('theme') === 'light') document.body.classList.add('light');
  setScreen(s, false);
  setTheme(document.body.classList.contains('light') ? 'light' : 'dark');
  applySkin('moyan');
  bindSliders();
  syncSliders();
})();
