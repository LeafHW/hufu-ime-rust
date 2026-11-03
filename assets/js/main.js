/* ==========================================================================
   虎符 HuFu — site interactions
   ========================================================================== */
(function () {
  'use strict';

  var root = document.documentElement;
  var body = document.body;
  var reduceMotion = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  /* 低端机/省流量：跳过 View Transition 整页截图（主题切换仍即时生效）。 */
  var weakDevice = (navigator.hardwareConcurrency && navigator.hardwareConcurrency <= 4) ||
    !!(navigator.connection && navigator.connection.saveData);
  var $ = function (sel, ctx) { return (ctx || document).querySelector(sel); };
  var $$ = function (sel, ctx) { return Array.prototype.slice.call((ctx || document).querySelectorAll(sel)); };
  var sleep = function (ms) { return new Promise(function (resolve) { setTimeout(resolve, ms); }); };

  /* ---------- Theme ---------- */
  var metaTheme = $('#meta-theme-color');
  var themeToggle = $('[data-theme-toggle]');

  function currentTheme() {
    return root.getAttribute('data-theme') === 'dark' ? 'dark' : 'light';
  }

  function syncThemeImages(theme) {
    $$('img[data-theme-img]').forEach(function (img) {
      var next = theme === 'dark' ? img.getAttribute('data-dark') : img.getAttribute('data-light');
      if (next && img.getAttribute('src') !== next) img.setAttribute('src', next);
    });
  }

  function applyTheme(theme) {
    root.setAttribute('data-theme', theme);
    if (metaTheme) metaTheme.setAttribute('content', theme === 'dark' ? '#0d0f11' : '#f6f1e6');
    if (themeToggle) {
      themeToggle.setAttribute('aria-label', theme === 'dark' ? '切换到浅色模式' : '切换到深色模式');
      themeToggle.setAttribute('title', theme === 'dark' ? '切换到浅色模式' : '切换到深色模式');
    }
    syncThemeImages(theme);
  }

  function setTheme(theme, persist) {
    applyTheme(theme);
    if (persist !== false) {
      try { localStorage.setItem('hufu-theme', theme); } catch (e) {}
    }
  }

  applyTheme(currentTheme());

  /* ---------- Theme switch: circular reveal from the toggle ----------
     与设置页同一套：View Transition root 层 + clip-path 圆扩散。
     半径按按钮中心到四角的最远距离算，避免用 120vmax 提前铺满。 */
  var themeVtBusy = false;
  function themeReveal(fromButton) {
    var r = fromButton.getBoundingClientRect();
    var x = r.left + r.width / 2;
    var y = r.top + r.height / 2;
    var far = Math.max(
      Math.sqrt(x * x + y * y),
      Math.sqrt((window.innerWidth - x) * (window.innerWidth - x) + y * y),
      Math.sqrt(x * x + (window.innerHeight - y) * (window.innerHeight - y)),
      Math.sqrt((window.innerWidth - x) * (window.innerWidth - x) + (window.innerHeight - y) * (window.innerHeight - y))
    ) + 28;
    root.style.setProperty('--theme-x', x.toFixed(1) + 'px');
    root.style.setProperty('--theme-y', y.toFixed(1) + 'px');
    root.style.setProperty('--theme-r', far.toFixed(1) + 'px');
  }

  if (themeToggle) {
    themeToggle.addEventListener('click', function () {
      var next = currentTheme() === 'dark' ? 'light' : 'dark';
      if (reduceMotion || weakDevice || !document.startViewTransition || themeVtBusy) {
        setTheme(next);
        return;
      }
      themeVtBusy = true;
      themeReveal(themeToggle);
      root.classList.add('theme-vt');
      var vt = document.startViewTransition(function () { setTheme(next); });
      var done = function () {
        themeVtBusy = false;
        root.classList.remove('theme-vt');
      };
      vt.finished.then(done, done);
    });
  }

  if (window.matchMedia) {
    var scheme = window.matchMedia('(prefers-color-scheme: dark)');
    var onScheme = function (e) {
      var saved = null;
      try { saved = localStorage.getItem('hufu-theme'); } catch (err) {}
      if (!saved) setTheme(e.matches ? 'dark' : 'light', false);
    };
    if (scheme.addEventListener) scheme.addEventListener('change', onScheme);
    else if (scheme.addListener) scheme.addListener(onScheme);
  }

  // Warm the other theme's screenshots after load.
  window.addEventListener('load', function () {
    setTimeout(function () {
      $$('img[data-theme-img]').forEach(function (img) {
        var other = currentTheme() === 'dark' ? img.getAttribute('data-light') : img.getAttribute('data-dark');
        if (other) { var pre = new Image(); pre.src = other; }
      });
    }, 1200);
  });

  /* ---------- Scroll progress / header ---------- */
  var progress = $('[data-scroll-progress]');
  var header = $('[data-header]');
  var ticking = false;

  function onScrollFrame() {
    ticking = false;
    var y = window.scrollY || window.pageYOffset;
    var max = Math.max(1, document.documentElement.scrollHeight - window.innerHeight);
    if (progress) progress.style.transform = 'scaleX(' + Math.min(1, Math.max(0, y / max)) + ')';
    if (header) header.classList.toggle('is-scrolled', y > 10);
  }

  function requestScroll() {
    if (!ticking) {
      ticking = true;
      window.requestAnimationFrame(onScrollFrame);
    }
  }

  window.addEventListener('scroll', requestScroll, { passive: true });
  onScrollFrame();

  /* ---------- Mobile menu ---------- */
  var menuToggle = $('[data-menu-toggle]');
  var mobileNav = $('[data-mobile-nav]');

  function setMenu(open) {
    if (!menuToggle || !mobileNav) return;
    menuToggle.setAttribute('aria-expanded', open ? 'true' : 'false');
    menuToggle.setAttribute('aria-label', open ? '关闭菜单' : '打开菜单');
    mobileNav.hidden = !open;
    body.classList.toggle('menu-open', open);
    if (open) {
      var first = mobileNav.querySelector('a');
      if (first) setTimeout(function () { first.focus(); }, 60);
    } else {
      menuToggle.focus();
    }
  }

  if (menuToggle && mobileNav) {
    menuToggle.addEventListener('click', function () {
      setMenu(menuToggle.getAttribute('aria-expanded') !== 'true');
    });
    $$('a', mobileNav).forEach(function (a) {
      a.addEventListener('click', function () { setMenu(false); });
    });
    document.addEventListener('keydown', function (e) {
      if (e.key === 'Escape' && menuToggle.getAttribute('aria-expanded') === 'true') setMenu(false);
    });
    window.addEventListener('resize', function () {
      if (window.innerWidth > 820 && menuToggle.getAttribute('aria-expanded') === 'true') setMenu(false);
    });
  }

  /* ---------- Active nav ---------- */
  var navLinks = $$('[data-nav-link]');
  var navSections = navLinks.map(function (a) { return document.getElementById(a.getAttribute('href').slice(1)); }).filter(Boolean);

  if ('IntersectionObserver' in window && navSections.length) {
    var navObserver = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (!entry.isIntersecting) return;
        var id = entry.target.id;
        navLinks.forEach(function (a) {
          var active = a.getAttribute('href') === '#' + id;
          a.classList.toggle('is-active', active);
          if (active) a.setAttribute('aria-current', 'location');
          else a.removeAttribute('aria-current');
        });
      });
    }, { rootMargin: '-42% 0px -52% 0px', threshold: 0 });
    navSections.forEach(function (s) { navObserver.observe(s); });
    window.addEventListener('scroll', function () {
      if ((window.scrollY || 0) < 260) {
        navLinks.forEach(function (a) {
          a.classList.remove('is-active');
          a.removeAttribute('aria-current');
        });
      }
    }, { passive: true });
  }

  /* ---------- Reveal on scroll ---------- */
  var revealEls = $$('[data-reveal]');

  // Stagger siblings in repeating groups.
  ['.bento', '.shot-row', '.faq-list', '.stats'].forEach(function (sel) {
    var group = $(sel);
    if (!group) return;
    $$('[data-reveal]', group).forEach(function (el, i) {
      el.style.setProperty('--d', String(i % 5));
    });
  });

  if (reduceMotion || !('IntersectionObserver' in window)) {
    revealEls.forEach(function (el) { el.classList.add('is-in'); });
  } else {
    var revealObserver = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (entry.isIntersecting) {
          entry.target.classList.add('is-in');
          revealObserver.unobserve(entry.target);
        }
      });
    }, { rootMargin: '0px 0px -8% 0px', threshold: 0.08 });
    revealEls.forEach(function (el) { revealObserver.observe(el); });
  }

  /* ---------- 装饰动画离屏暂停 ---------- */
  // 页面有 13 个 infinite 动画（脉冲/旋转/漂浮/跑马灯等）。只让视口内
  // 的持续跑：滚出视口即 animation-play-state:paused，减少合成器常驻。
  if (!reduceMotion && 'IntersectionObserver' in window) {
    var decoEls = [];
    $$('*').forEach(function (el) {
      var cs = window.getComputedStyle(el);
      var names = cs.animationName || '';
      var counts = cs.animationIterationCount || '';
      if (names && names !== 'none' && counts.indexOf('infinite') >= 0) decoEls.push(el);
    });
    if (decoEls.length) {
      var decoObs = new IntersectionObserver(function (entries) {
        entries.forEach(function (entry) {
          entry.target.style.animationPlayState = entry.isIntersecting ? 'running' : 'paused';
        });
      }, { rootMargin: '120px 0px' });
      decoEls.forEach(function (el) {
        el.style.animationPlayState = 'paused';
        decoObs.observe(el);
      });
    }
  }
  document.addEventListener('visibilitychange', function () {
    document.body.classList.toggle('site-hidden', document.hidden);
  });

  /* ---------- Hero typing demo ---------- */
  var demoText = $('[data-demo-text]');
  var demoEditor = demoText ? demoText.closest('.editor') : null;
  var candidate = $('[data-candidate]');
  var candCode = $('[data-cand-code]');
  var candItems = candidate ? $$('[data-cand-item]', candidate) : [];
  var candKeys = $$('[data-code-key]');
  var heroStage = $('.hero-stage');
  var demoVisible = true;
  var demoSkinFlip = false;
  var DEMO_CODE = 'syfdgvwihuvnagj';
  var DEMO_PREFILL = '';
  var DEMO_SENTENCE = '让中文收指成句';

  if (candidate && candItems.length) {
    candItems.forEach(function (li, i) { li.style.setProperty('--n', String(i)); });
  }

  function setActiveCandidate(index) {
    candItems.forEach(function (li, i) {
      li.classList.toggle('is-active', i === index);
    });
  }

  function resetDemo() {
    if (demoText) demoText.textContent = DEMO_PREFILL;
    if (demoEditor) demoEditor.classList.add('is-typing');
    if (candidate) {
      candidate.classList.add('is-hidden');
      candidate.classList.remove('is-open', 'is-committing', 'is-typing');
    }
    if (candCode) candCode.textContent = '';
    candKeys.forEach(function (k) { k.classList.remove('is-pressed'); });
    setActiveCandidate(0);
  }

  var demoFirst = true;
  var demoRunning = false;

  function showCandidates() {
    if (!candidate) return;
    candidate.classList.remove('is-hidden', 'is-typing', 'is-committing');
    candidate.classList.add('is-open');
  }

  async function demoCycle() {
    if (demoFirst) {
      demoFirst = false;
      if (demoText) demoText.textContent = DEMO_PREFILL;
      if (demoEditor) demoEditor.classList.add('is-typing');
      showCandidates();
      if (candCode) candCode.textContent = DEMO_CODE;
      setActiveCandidate(0);
      await sleep(2300);
    } else {
      resetDemo();
      if (candidate) {
        candidate.classList.remove('is-hidden');
        candidate.classList.add('is-typing');
      }
      await sleep(260);

      var code = DEMO_CODE;
      for (var i = 0; i < code.length; i++) {
        if (!demoVisible || document.hidden) return;
        if (candCode) candCode.textContent += code[i];
        var key = null;
        for (var k = 0; k < candKeys.length; k++) {
          if (candKeys[k].getAttribute('data-code-key') === code[i]) { key = candKeys[k]; break; }
        }
        if (key) key.classList.add('is-pressed');
        await sleep(code[i] === ' ' ? 70 : 105);
      }

      await sleep(180);
      if (!demoVisible || document.hidden) return;
      if (candidate) {
        candidate.classList.remove('is-typing');
        candidate.classList.add('is-open');
      }
      setActiveCandidate(0);
      await sleep(1250);

      if (!demoVisible || document.hidden) return;
      setActiveCandidate(1);
      await sleep(520);
      setActiveCandidate(0);
      await sleep(440);
    }

    if (!demoVisible || document.hidden) return;
    if (candidate) candidate.classList.add('is-committing');
    await sleep(260);

    if (candidate) candidate.classList.add('is-hidden');
    if (demoText) {
      demoText.textContent = DEMO_SENTENCE;
      demoText.classList.remove('flash');
      void demoText.offsetWidth;
      demoText.classList.add('flash');
    }
    if (demoEditor) demoEditor.classList.add('is-typing');
    await sleep(1650);

    if (!demoVisible || document.hidden) return;

    demoSkinFlip = !demoSkinFlip;
    if (candidate) candidate.classList.toggle('skin-hupo', demoSkinFlip);
    if (demoText) demoText.textContent = DEMO_PREFILL;
    if (demoEditor) demoEditor.classList.add('is-typing');
    if (candidate) {
      candidate.classList.add('is-hidden');
      candidate.classList.remove('is-committing', 'is-open');
    }
    if (candCode) candCode.textContent = '';
    candKeys.forEach(function (k) { k.classList.remove('is-pressed'); });
    await sleep(620);
  }

  async function runDemo() {
    if (reduceMotion || !demoText || demoRunning) return;
    if (!demoVisible || document.hidden) return;
    demoRunning = true;
    try {
      await demoCycle();
    } catch (e) {
      /* keep the site usable even if a demo frame fails */
    }
    demoRunning = false;
    if (demoVisible && !document.hidden) setTimeout(runDemo, 260);
  }

  if (heroStage && 'IntersectionObserver' in window) {
    var demoObserver = new IntersectionObserver(function (entries) {
      demoVisible = entries[0].isIntersecting;
      if (demoVisible && !document.hidden) runDemo();
    }, { threshold: 0.18 });
    demoObserver.observe(heroStage);
  } else {
    runDemo();
  }

  document.addEventListener('visibilitychange', function () {
    if (!document.hidden && demoVisible) runDemo();
  });

  if (reduceMotion && demoText) {
    demoText.textContent = DEMO_SENTENCE;
    if (candCode) candCode.textContent = DEMO_CODE;
    if (candidate) {
      candidate.classList.remove('is-hidden', 'is-typing', 'is-committing');
      candidate.classList.add('is-open');
    }
    setActiveCandidate(0);
  }

  /* ---------- Code strip typewriter ---------- */
  var codeTypeEls = $$('[data-code-type]');

  function prepareCode(el) {
    var textEl = el.querySelector('[data-code-text]');
    if (!textEl) return;
    var full = textEl.textContent;
    textEl.setAttribute('data-full', full);
    if (reduceMotion) {
      el.classList.add('is-typed');
      return;
    }
    textEl.textContent = '';
  }

  function typeCode(el) {
    if (!el || el.getAttribute('data-typed') === '1') return;
    var textEl = el.querySelector('[data-code-text]');
    if (!textEl) return;
    var full = textEl.getAttribute('data-full') || textEl.textContent;
    textEl.setAttribute('data-full', full);
    if (reduceMotion) {
      textEl.textContent = full;
      el.classList.add('is-typed');
      return;
    }
    el.setAttribute('data-typed', '1');
    el.classList.add('is-typing');
    textEl.textContent = '';
    var i = 0;
    var timer = setInterval(function () {
      i += 1;
      textEl.textContent = full.slice(0, i);
      if (i >= full.length) {
        clearInterval(timer);
        el.classList.remove('is-typing');
        el.classList.add('is-typed');
      }
    }, 76);
  }

  codeTypeEls.forEach(prepareCode);
  var heroCode = $('[data-code-type="hero"]');
  if (heroCode) setTimeout(function () { typeCode(heroCode); }, 240);
  var featureCode = $('[data-code-type="feature"]');
  if (featureCode) {
    if ('IntersectionObserver' in window) {
      var codeObserver = new IntersectionObserver(function (entries) {
        entries.forEach(function (entry) {
          if (entry.isIntersecting) {
            typeCode(entry.target);
            codeObserver.unobserve(entry.target);
          }
        });
      }, { threshold: 0.35 });
      codeObserver.observe(featureCode);
    } else {
      typeCode(featureCode);
    }
  }

  /* ---------- Stats counters ---------- */
  var statsBlock = $('.stats');
  var statNumbers = $$('[data-count]');
  var statsDone = false;

  function formatCount(value, decimals) {
    return decimals > 0 ? value.toFixed(decimals) : String(Math.round(value));
  }

  function animateStats() {
    if (statsDone) return;
    statsDone = true;
    statNumbers.forEach(function (el) {
      var target = parseFloat(el.getAttribute('data-count')) || 0;
      var decimals = parseInt(el.getAttribute('data-decimals') || '0', 10);
      if (reduceMotion) {
        el.textContent = formatCount(target, decimals);
        return;
      }
      var start = performance.now();
      var duration = 1300;
      function frame(now) {
        var t = Math.min(1, (now - start) / duration);
        var eased = 1 - Math.pow(1 - t, 4);
        el.textContent = t < 1 ? formatCount(target * eased, decimals) : formatCount(target, decimals);
        if (t < 1) window.requestAnimationFrame(frame);
      }
      window.requestAnimationFrame(frame);
    });
  }

  if (statsBlock && 'IntersectionObserver' in window) {
    var statsObserver = new IntersectionObserver(function (entries) {
      if (entries[0].isIntersecting) {
        animateStats();
        statsObserver.disconnect();
      }
    }, { threshold: 0.25 });
    statsObserver.observe(statsBlock);
  } else {
    animateStats();
  }

  /* ---------- Hero pointer parallax ---------- */
  if (heroStage && window.matchMedia && window.matchMedia('(pointer: fine)').matches && !reduceMotion) {
    var paraRaf = 0;
    var targetX = 0;
    var targetY = 0;
    heroStage.addEventListener('pointermove', function (e) {
      var rect = heroStage.getBoundingClientRect();
      targetX = (e.clientX - rect.left) / rect.width - 0.5;
      targetY = (e.clientY - rect.top) / rect.height - 0.5;
      if (!paraRaf) {
        paraRaf = window.requestAnimationFrame(function () {
          paraRaf = 0;
          heroStage.style.setProperty('--mx', (targetX * 1.2).toFixed(3));
          heroStage.style.setProperty('--my', (targetY * 1.2).toFixed(3));
        });
      }
    });
    heroStage.addEventListener('pointerleave', function () {
      heroStage.style.setProperty('--mx', '0');
      heroStage.style.setProperty('--my', '0');
    });
  }

  /* ---------- Skin lab ---------- */
  var skinWindow = $('[data-skin-window]');
  var skinName = $('[data-skin-name]');
  var skinButtons = $$('[data-skin]');
  var skinNames = {
    moyan: '墨岩',
    canghai: '沧海',
    chenwu: '晨雾',
    huguang: '湖光',
    hupo: '琥珀',
    mushan: '暮山紫'
  };
  var skinAuto = null;
  var skinTouched = false;
  var skinPaused = false;
  var skinIndex = 0;

  function setSkin(id) {
    if (!skinWindow) return;
    var safe = skinNames[id] ? id : 'moyan';
    skinWindow.classList.remove('skin-moyan', 'skin-canghai', 'skin-chenwu', 'skin-huguang', 'skin-hupo', 'skin-mushan');
    skinWindow.classList.add('skin-' + safe);
    if (skinName) skinName.textContent = skinNames[safe];
    skinButtons.forEach(function (btn) {
      var active = btn.getAttribute('data-skin') === safe;
      btn.classList.toggle('is-active', active);
      btn.setAttribute('aria-pressed', active ? 'true' : 'false');
    });
    skinIndex = Object.keys(skinNames).indexOf(safe);
    if (!reduceMotion) {
      skinWindow.classList.remove('is-open');
      skinWindow.classList.add('is-typing');
      window.requestAnimationFrame(function () {
        window.setTimeout(function () {
          skinWindow.classList.remove('is-typing');
          skinWindow.classList.add('is-open');
        }, 40);
      });
    }
  }

  function startSkinAuto() {
    if (reduceMotion || skinTouched || skinAuto) return;
    skinAuto = window.setInterval(function () {
      if (skinPaused || document.hidden) return;
      var ids = Object.keys(skinNames);
      var next = ids[(skinIndex + 1) % ids.length];
      setSkin(next);
    }, 4600);
  }

  function stopSkinAuto() {
    if (skinAuto) {
      window.clearInterval(skinAuto);
      skinAuto = null;
    }
  }

  if (skinWindow && skinButtons.length) {
    skinButtons.forEach(function (btn) {
      btn.addEventListener('click', function () {
        skinTouched = true;
        stopSkinAuto();
        setSkin(btn.getAttribute('data-skin'));
      });
    });

    var lab = $('.skin-lab');
    if (lab) {
      lab.addEventListener('pointerenter', function () { skinPaused = true; });
      lab.addEventListener('pointerleave', function () { skinPaused = false; });
      lab.addEventListener('focusin', function () { skinPaused = true; });
      lab.addEventListener('focusout', function () { skinPaused = false; });
    }

    var skinObs = new IntersectionObserver(function (entries) {
      if (entries[0].isIntersecting) startSkinAuto();
      else stopSkinAuto();
    }, { threshold: 0.25 });
    skinObs.observe(skinWindow);
    setSkin('moyan');
  }

  /* ---------- OS tabs ---------- */
  var osTabs = $$('[data-os-tab]');
  var osPanels = $$('[data-os-panel]');

  function selectOs(id, focusTab) {
    osTabs.forEach(function (tab) {
      var active = tab.getAttribute('data-os-tab') === id;
      tab.classList.toggle('is-active', active);
      tab.setAttribute('aria-selected', active ? 'true' : 'false');
      tab.setAttribute('tabindex', active ? '0' : '-1');
      if (active && focusTab) tab.focus();
    });
    osPanels.forEach(function (panel) {
      panel.hidden = panel.getAttribute('data-os-panel') !== id;
    });
    $$('[data-os-steps]').forEach(function (steps) {
      steps.hidden = steps.getAttribute('data-os-steps') !== id;
    });
  }

  if (osTabs.length) {
    osTabs.forEach(function (tab, index) {
      tab.addEventListener('click', function () { selectOs(tab.getAttribute('data-os-tab')); });
      tab.addEventListener('keydown', function (e) {
        if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft' && e.key !== 'Home' && e.key !== 'End') return;
        e.preventDefault();
        var next = index;
        if (e.key === 'ArrowRight') next = (index + 1) % osTabs.length;
        if (e.key === 'ArrowLeft') next = (index - 1 + osTabs.length) % osTabs.length;
        if (e.key === 'Home') next = 0;
        if (e.key === 'End') next = osTabs.length - 1;
        selectOs(osTabs[next].getAttribute('data-os-tab'), true);
      });
    });

    var ua = navigator.userAgent || '';
    var os = 'windows';
    if (/Macintosh|Mac OS X/i.test(ua)) os = 'macos';
    else if (/Linux/i.test(ua) && !/Android/i.test(ua)) os = 'linux';
    selectOs(os);
  }

  /* ---------- Latest release (progressive enhancement) ---------- */
  var releaseVersion = $$('[data-release-version]');
  var releaseDate = $$('[data-release-date]');
  var exeLinks = $$('[data-exe-link]');
  var exeVersions = $$('[data-exe-version]');
  var exeSizes = $$('[data-exe-size]');
  var exeMeta = $$('[data-exe-meta]');
  var zipLink = $('[data-zip-link]');

  function formatMB(bytes) {
    return (bytes / 1048576).toFixed(1) + ' MB';
  }

  function formatDate(iso) {
    if (!iso) return '';
    var d = new Date(iso);
    if (isNaN(d.getTime())) return '';
    var p = function (n) { return (n < 10 ? '0' : '') + n; };
    return d.getFullYear() + '-' + p(d.getMonth() + 1) + '-' + p(d.getDate());
  }

  function applyRelease(data) {
    if (!data || !data.tag_name) return;
    var tag = data.tag_name;
    var version = tag.replace(/^v/i, '');
    releaseVersion.forEach(function (el) { el.textContent = tag; });
    exeVersions.forEach(function (el) { el.textContent = version; });
    var date = formatDate(data.published_at);
    if (date) releaseDate.forEach(function (el) { el.textContent = date; });

    var assets = Array.isArray(data.assets) ? data.assets : [];
    var exe = assets.filter(function (a) { return /\.exe$/i.test(a.name); }).sort(function (a, b) {
      return (/setup/i.test(b.name) ? 1 : 0) - (/setup/i.test(a.name) ? 1 : 0);
    })[0];
    var zip = assets.filter(function (a) { return /\.zip$/i.test(a.name); }).sort(function (a, b) {
      return (/no-model/i.test(b.name) ? 1 : 0) - (/no-model/i.test(a.name) ? 1 : 0);
    })[0];

    if (exe) {
      exeLinks.forEach(function (a) { a.setAttribute('href', exe.browser_download_url); });
      exeSizes.forEach(function (el) { el.textContent = formatMB(exe.size); });
      exeVersions.forEach(function (el) { el.textContent = version; });
      exeMeta.forEach(function (el) { el.textContent = exe.name + ' · ' + formatMB(exe.size); });
    }
    if (zip && zipLink) {
      zipLink.hidden = false;
      zipLink.setAttribute('href', zip.browser_download_url);
      zipLink.textContent = '绿色版 ' + zip.name + ' · ' + formatMB(zip.size);
    }
  }

  function loadFromGitHub() {
    var cached = null;
    try {
      cached = JSON.parse(sessionStorage.getItem('hufu-release-v1') || 'null');
    } catch (e) {}
    if (cached && cached.time && Date.now() - cached.time < 10 * 60 * 1000 && cached.data) {
      applyRelease(cached.data);
      return;
    }
    if (!window.fetch) return;
    var controller = window.AbortController ? new AbortController() : null;
    var timer = controller ? setTimeout(function () { controller.abort(); }, 7000) : null;
    fetch('https://api.github.com/repos/LeafHW/hufu-ime-rust/releases/latest', {
      headers: { Accept: 'application/vnd.github+json' },
      signal: controller ? controller.signal : undefined
    }).then(function (res) {
      if (!res.ok) throw new Error('HTTP ' + res.status);
      return res.json();
    }).then(function (data) {
      if (timer) clearTimeout(timer);
      applyRelease(data);
      try { sessionStorage.setItem('hufu-release-v1', JSON.stringify({ time: Date.now(), data: data })); } catch (e) {}
    }).catch(function () {
      if (timer) clearTimeout(timer);
      /* keep static fallback */
    });
  }

  /* 清单驱动：./update.json 由发版脚本生成（同源、约 1 KB、不占 GitHub API 配额）。
     清单不可达时才回退 GitHub API（loadFromGitHub）。 */
  function applyManifest(m) {
    if (!m || !m.latest) return false;
    var version = String(m.latest).replace(/^v/i, '');
    var src = (m.sources && m.sources[0]) || {};
    releaseVersion.forEach(function (el) { el.textContent = 'v' + version; });
    exeVersions.forEach(function (el) { el.textContent = version; });
    if (m.date) releaseDate.forEach(function (el) { el.textContent = m.date; });
    if (src.url) exeLinks.forEach(function (a) { a.setAttribute('href', src.url); });
    if (m.size) {
      exeSizes.forEach(function (el) { el.textContent = formatMB(m.size); });
      exeMeta.forEach(function (el) {
        el.textContent = 'HuFuSetup-' + version + '.exe · ' + formatMB(m.size) + ' · 国内加速';
      });
    }
    return true;
  }

  function loadRelease() {
    var cached = null;
    try {
      cached = JSON.parse(sessionStorage.getItem('hufu-manifest-v1') || 'null');
    } catch (e) {}
    if (cached && cached.time && Date.now() - cached.time < 10 * 60 * 1000 && cached.data) {
      if (applyManifest(cached.data)) return;
    }
    if (!window.fetch) return;
    fetch('./update.json', { cache: 'no-store' }).then(function (res) {
      if (!res.ok) throw new Error('HTTP ' + res.status);
      return res.json();
    }).then(function (m) {
      if (!applyManifest(m)) throw new Error('清单格式无效');
      try { sessionStorage.setItem('hufu-manifest-v1', JSON.stringify({ time: Date.now(), data: m })); } catch (e) {}
    }).catch(function () {
      loadFromGitHub();
    });
  }

  if (exeLinks.length) {
    if ('requestIdleCallback' in window) window.requestIdleCallback(loadRelease, { timeout: 3000 });
    else setTimeout(loadRelease, 1200);
  }

  /* ---------- Copy buttons ---------- */
  $$('[data-copy]').forEach(function (btn) {
    btn.addEventListener('click', function () {
      var text = btn.getAttribute('data-copy') || '';
      var label = btn.querySelector('span');
      var old = label ? label.textContent : '';
      var done = function () {
        btn.classList.add('is-copied');
        if (label) label.textContent = '已复制';
        setTimeout(function () {
          btn.classList.remove('is-copied');
          if (label) label.textContent = old;
        }, 1600);
      };
      var fallback = function () {
        var ta = document.createElement('textarea');
        ta.value = text;
        ta.setAttribute('readonly', '');
        ta.style.position = 'fixed';
        ta.style.left = '-9999px';
        document.body.appendChild(ta);
        ta.select();
        try { document.execCommand('copy'); done(); } catch (e) {}
        document.body.removeChild(ta);
      };
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text).then(done).catch(fallback);
      } else {
        fallback();
      }
    });
  });

  /* ---------- Lightbox ---------- */
  var lightbox = $('#lightbox');
  var lbImg = $('[data-lb-img]');
  var lbCaption = $('[data-lb-caption]');
  var lbButtons = $$('[data-lightbox]');
  var lbIndex = 0;

  function showShot(index) {
    if (!lightbox || !lbImg || !lbButtons.length) return;
    lbIndex = (index + lbButtons.length) % lbButtons.length;
    var btn = lbButtons[lbIndex];
    var thumb = btn.querySelector('img');
    var full = btn.getAttribute('data-src') || (thumb ? (thumb.currentSrc || thumb.getAttribute('src')) : '');
    var caption = btn.getAttribute('data-caption') || (thumb ? thumb.getAttribute('alt') : '');
    if (full) lbImg.setAttribute('src', full);
    lbImg.setAttribute('alt', caption);
    if (lbCaption) lbCaption.textContent = caption;
    if (!lightbox.open) {
      if (typeof lightbox.showModal === 'function') lightbox.showModal();
      else lightbox.setAttribute('open', '');
    }
  }

  if (lightbox && lbButtons.length) {
    lbButtons.forEach(function (btn, i) {
      btn.addEventListener('click', function () { showShot(i); });
    });
    var closeBtn = $('[data-lb-close]', lightbox);
    var prevBtn = $('[data-lb-prev]', lightbox);
    var nextBtn = $('[data-lb-next]', lightbox);
    if (closeBtn) closeBtn.addEventListener('click', function () { lightbox.close ? lightbox.close() : lightbox.removeAttribute('open'); });
    if (prevBtn) prevBtn.addEventListener('click', function () { showShot(lbIndex - 1); });
    if (nextBtn) nextBtn.addEventListener('click', function () { showShot(lbIndex + 1); });
    lightbox.addEventListener('click', function (e) {
      if (e.target === lightbox) {
        if (lightbox.close) lightbox.close();
        else lightbox.removeAttribute('open');
      }
    });
    lightbox.addEventListener('keydown', function (e) {
      if (e.key === 'ArrowLeft') { e.preventDefault(); showShot(lbIndex - 1); }
      if (e.key === 'ArrowRight') { e.preventDefault(); showShot(lbIndex + 1); }
    });
    lightbox.addEventListener('cancel', function () {
      /* native Esc close */
    });
  }

  /* ---------- FAQ: close others ---------- */
  $$('.faq-list details').forEach(function (detail) {
    detail.addEventListener('toggle', function () {
      if (!detail.open) return;
      $$('.faq-list details').forEach(function (other) {
        if (other !== detail) other.open = false;
      });
    });
  });

  /* ---------- Smooth anchor offset & external links ---------- */
  document.addEventListener('click', function (e) {
    var a = e.target.closest ? e.target.closest('a[href^="#"]') : null;
    if (!a) return;
    var href = a.getAttribute('href');
    if (!href || href === '#') return;
    var target = document.getElementById(href.slice(1));
    if (!target) return;
    e.preventDefault();
    var top = target.getBoundingClientRect().top + window.scrollY - 78;
    window.scrollTo({ top: top, behavior: reduceMotion ? 'auto' : 'smooth' });
    if (history.replaceState) history.replaceState(null, '', href);
  });
})();
