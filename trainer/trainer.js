(function () {
  'use strict';

  var btn = document.querySelector('[data-theme-toggle]');
  var root = document.documentElement;

  function paint(theme) {
    root.setAttribute('data-theme', theme);
    if (!btn) return;
    var to = theme === 'dark' ? '浅色' : '深色';
    btn.setAttribute('aria-label', '切换到' + to + '模式');
    btn.setAttribute('title', '切换到' + to + '模式');
    var meta = document.querySelector('meta[name="theme-color"]');
    if (meta) meta.setAttribute('content', theme === 'dark' ? '#0d0f11' : '#f5efe3');
  }

  paint(root.getAttribute('data-theme') === 'dark' ? 'dark' : 'light');

  if (!btn) return;

  btn.addEventListener('click', function () {
    var next = root.getAttribute('data-theme') === 'dark' ? 'light' : 'dark';
    paint(next);
    try { localStorage.setItem('hufu-theme', next); } catch (e) {}
  });
})();
