(function () {
  'use strict';

  var API = 'https://api.github.com/repos/LeafHW/hufu-ime-rust/releases?per_page=100';
  var CACHE_KEY = 'hufu-releases-v2';
  var CACHE_TTL = 30 * 60 * 1000;

  var latestSlot = document.getElementById('latest-slot');
  var releaseList = document.getElementById('release-list');
  var loadState = document.getElementById('load-state');
  var lastSync = document.getElementById('last-sync');
  var historyCount = document.getElementById('history-count');
  var refreshBtn = document.querySelector('[data-refresh]');
  var copyBtn = document.querySelector('[data-copy]');
  var themeBtn = document.querySelector('[data-theme-toggle]');
  var statusDot = document.querySelector('.hero-status .status-dot');

  var loading = false;
  var currentData = null;

  function escapeHTML(value) {
    return String(value == null ? '' : value)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  function safeUrl(value) {
    return /^https?:\/\//i.test(String(value || '')) ? String(value) : '';
  }

  function formatDate(iso) {
    if (!iso) return '';
    var d = new Date(iso);
    if (isNaN(d.getTime())) return '';
    try {
      return new Intl.DateTimeFormat('zh-CN', {
        year: 'numeric', month: '2-digit', day: '2-digit', timeZone: 'Asia/Shanghai'
      }).format(d).replace(/\//g, '-');
    } catch (e) {
      return iso.slice(0, 10);
    }
  }

  function formatTime(date) {
    var p = function (n) { return (n < 10 ? '0' : '') + n; };
    return p(date.getHours()) + ':' + p(date.getMinutes());
  }

  function formatSize(bytes) {
    if (!bytes || bytes < 0) return '';
    if (bytes >= 1048576) return (bytes / 1048576).toFixed(1) + ' MB';
    if (bytes >= 1024) return Math.round(bytes / 1024) + ' KB';
    return bytes + ' B';
  }

  function inline(text) {
    var s = escapeHTML(text);
    s = s.replace(/`([^`\n]+)`/g, '<code>$1</code>');
    s = s.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, function (_, label, url) {
      return '<a href="' + url + '" target="_blank" rel="noopener">' + label + '</a>';
    });
    s = s.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
    s = s.replace(/(^|[\s(])(https?:\/\/[^\s<)]+)/g, function (_, lead, url) {
      return lead + '<a href="' + url + '" target="_blank" rel="noopener">' + url + '</a>';
    });
    return s;
  }

  function markdown(md) {
    if (!md) return '<p class="md-empty">本次更新暂无详细说明。</p>';
    var lines = String(md).replace(/\r\n?/g, '\n').trim().split('\n');
    if (lines.length && /^#\s+/.test(lines[0])) lines.shift();

    var out = [];
    var para = [];
    var list = '';
    var inCode = false;
    var code = [];

    function flushPara() {
      if (para.length) {
        out.push('<p>' + para.join('<br>') + '</p>');
        para = [];
      }
    }

    function closeList() {
      if (list) {
        out.push('</' + list + '>');
        list = '';
      }
    }

    lines.forEach(function (line) {
      if (inCode) {
        if (/^```/.test(line)) {
          out.push('<pre><code>' + escapeHTML(code.join('\n')) + '</code></pre>');
          inCode = false;
          code = [];
        } else {
          code.push(line);
        }
        return;
      }

      if (/^```/.test(line)) {
        flushPara();
        closeList();
        inCode = true;
        code = [];
        return;
      }

      if (!line.trim()) {
        flushPara();
        closeList();
        return;
      }

      var heading = line.match(/^(#{1,4})\s+(.*)$/);
      if (heading) {
        flushPara();
        closeList();
        var level = Math.min(5, heading[1].length + 2);
        out.push('<h' + level + '>' + inline(heading[2]) + '</h' + level + '>');
        return;
      }

      if (/^---+$/.test(line)) {
        flushPara();
        closeList();
        out.push('<hr>');
        return;
      }

      var ul = line.match(/^[-*]\s+(.*)$/);
      if (ul) {
        flushPara();
        if (list !== 'ul') {
          closeList();
          out.push('<ul>');
          list = 'ul';
        }
        out.push('<li>' + inline(ul[1]) + '</li>');
        return;
      }

      var ol = line.match(/^\d+\.\s+(.*)$/);
      if (ol) {
        flushPara();
        if (list !== 'ol') {
          closeList();
          out.push('<ol>');
          list = 'ol';
        }
        out.push('<li>' + inline(ol[1]) + '</li>');
        return;
      }

      var quote = line.match(/^>\s?(.*)$/);
      if (quote) {
        flushPara();
        closeList();
        out.push('<blockquote>' + inline(quote[1]) + '</blockquote>');
        return;
      }

      para.push(inline(line));
    });

    if (inCode) out.push('<pre><code>' + escapeHTML(code.join('\n')) + '</code></pre>');
    flushPara();
    closeList();
    return out.join('\n');
  }

  function pickAssets(release) {
    var assets = Array.isArray(release.assets) ? release.assets : [];
    var exe = assets.filter(function (a) { return /\.exe$/i.test(a.name); }).sort(function (a, b) {
      return (/setup/i.test(b.name) ? 1 : 0) - (/setup/i.test(a.name) ? 1 : 0);
    })[0];
    var zip = assets.filter(function (a) { return /\.zip$/i.test(a.name); }).sort(function (a, b) {
      return (/no-model/i.test(b.name) ? 1 : 0) - (/no-model/i.test(a.name) ? 1 : 0);
    })[0];
    return { exe: exe, zip: zip };
  }

  function assetLink(asset, label) {
    if (!asset) return '';
    var url = safeUrl(asset.browser_download_url);
    if (!url) return '';
    return '<a class="asset primary" href="' + escapeHTML(url) + '" target="_blank" rel="noopener">' + escapeHTML(label || asset.name) + ' · ' + escapeHTML(formatSize(asset.size)) + '</a>';
  }

  function renderLatest(release) {
    var tag = escapeHTML(release.tag_name || '');
    var name = escapeHTML(release.name || release.tag_name || '未命名版本');
    var date = escapeHTML(formatDate(release.published_at));
    var assets = pickAssets(release);
    var exe = assets.exe;
    var zip = assets.zip;
    var notes = markdown(release.body || '');
    var releaseUrl = safeUrl(release.html_url) || 'https://github.com/LeafHW/hufu-ime-rust/releases';

    var primary = exe
      ? '<a class="dl-main" href="' + escapeHTML(safeUrl(exe.browser_download_url) || releaseUrl) + '" target="_blank" rel="noopener"><span class="dl-main-icon">↓</span><span class="dl-main-copy"><b>Windows 安装包</b><em>' + escapeHTML(exe.name) + ' · ' + escapeHTML(formatSize(exe.size)) + '</em></span><span class="dl-main-arrow">↗</span></a>'
      : '<a class="dl-main" href="' + escapeHTML(releaseUrl) + '" target="_blank" rel="noopener"><span class="dl-main-icon">↗</span><span class="dl-main-copy"><b>打开 GitHub 下载页</b><em>查看本次发布的所有文件</em></span><span class="dl-main-arrow">↗</span></a>';

    var sub = zip
      ? '<a class="dl-sub" href="' + escapeHTML(safeUrl(zip.browser_download_url)) + '" target="_blank" rel="noopener">绿色版 zip · ' + escapeHTML(formatSize(zip.size)) + ' ↗</a>'
      : '';

    return '<article class="latest-card">' +
      '<div class="latest-top">' +
        '<span class="badge">最新版</span>' +
        '<span class="latest-tag">' + tag + '</span>' +
        '<span class="latest-date">' + date + '</span>' +
        (release.prerelease ? '<span class="badge pre">预发布</span>' : '') +
      '</div>' +
      '<h2 class="latest-title">' + name + '</h2>' +
      '<div class="latest-grid">' +
        '<div class="latest-notes md">' + notes + '</div>' +
        '<aside class="download-panel">' +
          '<p class="dl-label">DOWNLOAD / 下载</p>' +
          primary +
          sub +
          '<a class="dl-all" href="' + escapeHTML(releaseUrl) + '" target="_blank" rel="noopener">在 GitHub 查看全部文件 ↗</a>' +
          '<p class="dl-note">Windows 10/11 · 旧版原地升级，码表、设置与学习记录保留。<br>安装包来自 GitHub Releases，下载链接随发布自动更新。</p>' +
        '</aside>' +
      '</div>' +
    '</article>';
  }

  function renderHistory(release) {
    var tag = escapeHTML(release.tag_name || '');
    var name = escapeHTML(release.name || release.tag_name || '未命名版本');
    var date = escapeHTML(formatDate(release.published_at));
    var assets = pickAssets(release);
    var exe = assets.exe;
    var zip = assets.zip;
    var body = release.body ? markdown(release.body) : '';
    var releaseUrl = safeUrl(release.html_url) || 'https://github.com/LeafHW/hufu-ime-rust/releases';
    var download = '';

    if (exe) {
      download += assetLink(exe, 'Windows 安装包');
    }
    if (zip) {
      download += assetLink(zip, '绿色版 zip');
    }
    if (!exe && !zip) {
      download += '<a class="asset" href="' + escapeHTML(releaseUrl) + '" target="_blank" rel="noopener">打开版本页 ↗</a>';
    }
    download += '<a class="asset asset-gh" href="' + escapeHTML(releaseUrl) + '" target="_blank" rel="noopener">全部文件 ↗</a>';

    var notes = body
      ? '<details class="release-notes"><summary>查看更新说明</summary><div class="md">' + body + '</div></details>'
      : '';

    return '<article class="release-row">' +
      '<div class="release-main">' +
        '<div class="release-head">' +
          '<span class="release-version">' + tag + '</span>' +
          '<span class="release-date">' + date + '</span>' +
          (release.prerelease ? '<span class="badge pre">预发布</span>' : '') +
        '</div>' +
        '<h3 class="release-title">' + name + '</h3>' +
        notes +
        '<div class="release-actions">' + download + '</div>' +
      '</div>' +
    '</article>';
  }

  function setStatus(text, sync) {
    if (loadState) loadState.textContent = text;
    if (lastSync && sync != null) lastSync.textContent = sync;
  }

  function setBusy(busy) {
    loading = busy;
    if (refreshBtn) {
      refreshBtn.disabled = busy;
      refreshBtn.classList.toggle('is-busy', busy);
    }
    if (latestSlot) latestSlot.setAttribute('aria-busy', busy ? 'true' : 'false');
    if (releaseList) releaseList.setAttribute('aria-busy', busy ? 'true' : 'false');
    if (statusDot) statusDot.classList.toggle('is-loading', busy);
  }

  function render(data) {
    var releases = (Array.isArray(data) ? data : []).filter(function (r) { return r && !r.draft; });
    if (!releases.length) throw new Error('empty');

    var latest = releases.filter(function (r) { return !r.prerelease; })[0] || releases[0];
    var others = releases.filter(function (r) { return r.id !== latest.id; });

    currentData = data;
    if (latestSlot) latestSlot.innerHTML = renderLatest(latest);
    if (releaseList) {
      releaseList.innerHTML = others.length
        ? others.map(renderHistory).join('')
        : '<div class="notice">这是目前唯一的版本。后续发布的新版本会自动出现在这里。</div>';
    }
    if (historyCount) historyCount.textContent = others.length ? '共 ' + others.length + ' 个历史版本' : '暂无更早版本';
    setStatus('已同步 · 共 ' + releases.length + ' 个版本', '最后更新 ' + formatTime(new Date()));

    document.title = '虎符更新日志 · ' + (latest.tag_name || '最新版');
  }

  function renderError(message) {
    var text = message || 'GitHub API 暂时不可用（网络限制或限流）。';
    if (latestSlot) {
      latestSlot.innerHTML =
        '<div class="error-card">' +
          '<p class="kicker"><span class="kicker-dot"></span> RELEASES / TEMPORARILY UNAVAILABLE</p>' +
          '<h2>暂时读不到最新版本。</h2>' +
          '<p>' + escapeHTML(text) + ' 你仍然可以直接打开 GitHub Releases 查看最新版本与历史版本。</p>' +
          '<div class="error-actions">' +
            '<a class="btn" href="https://github.com/LeafHW/hufu-ime-rust/releases/latest" target="_blank" rel="noopener">打开 GitHub 最新版本 ↗</a>' +
            '<button class="btn btn-ghost" type="button" data-retry>重试</button>' +
          '</div>' +
        '</div>';
    }
    if (releaseList) releaseList.innerHTML = '';
    if (historyCount) historyCount.textContent = '';
    setStatus('读取失败', '');
    document.title = '虎符更新日志 · 最新版与历史版本下载';
  }

  function readCache() {
    try {
      var raw = sessionStorage.getItem(CACHE_KEY);
      if (!raw) return null;
      var parsed = JSON.parse(raw);
      if (!parsed || !parsed.time || Date.now() - parsed.time > CACHE_TTL) return null;
      return parsed.data;
    } catch (e) {
      return null;
    }
  }

  function writeCache(data) {
    try {
      sessionStorage.setItem(CACHE_KEY, JSON.stringify({ time: Date.now(), data: data }));
    } catch (e) {}
  }

  function load(force) {
    if (loading) return;
    var cached = force ? null : readCache();

    if (cached) {
      try { render(cached); } catch (e) {}
    }

    setBusy(true);
    if (!cached) setStatus('正在从 GitHub 读取版本…');

    var controller = window.AbortController ? new AbortController() : null;
    var timer = controller ? setTimeout(function () { controller.abort(); }, 12000) : null;

    fetch(API, {
      headers: { 'Accept': 'application/vnd.github+json' },
      cache: 'no-store',
      signal: controller ? controller.signal : undefined
    }).then(function (res) {
      if (timer) clearTimeout(timer);
      if (res.status === 403) throw new Error('GitHub API 触发了访问频率限制。');
      if (!res.ok) throw new Error('GitHub API 返回了 HTTP ' + res.status + '。');
      return res.json();
    }).then(function (data) {
      writeCache(data);
      render(data);
      setBusy(false);
    }).catch(function (err) {
      if (timer) clearTimeout(timer);
      setBusy(false);
      // Fallback: the site hosts the most recent snapshot, so the page still
      // works on networks where api.github.com is blocked or rate-limited.
      fetch('./cache.json', { cache: 'no-store' }).then(function (res) {
        if (!res.ok) throw new Error('snapshot unavailable');
        return res.json();
      }).then(function (payload) {
        var data = payload && payload.data ? payload.data : payload;
        render(data);
        var when = payload && payload.time ? formatDate(payload.time) : '';
        setStatus('GitHub API 不可达 · 显示最近同步快照', when ? '快照 ' + when : '最近同步');
      }).catch(function () {
        if (cached) {
          render(cached);
          setStatus('GitHub API 不可达 · 显示上次缓存', '显示缓存版本');
          return;
        }
        renderError(err && err.name === 'AbortError' ? '请求 GitHub 超时。' : (err && err.message) || '网络请求失败。');
      });
    });
  }

  /* theme */
  function applyTheme(theme) {
    document.documentElement.setAttribute('data-theme', theme);
    try { localStorage.setItem('hufu-theme', theme); } catch (e) {}
    if (themeBtn) {
      themeBtn.setAttribute('aria-label', theme === 'dark' ? '切换到浅色模式' : '切换到深色模式');
      themeBtn.setAttribute('title', theme === 'dark' ? '切换到浅色模式' : '切换到深色模式');
    }
  }

  if (themeBtn) {
    themeBtn.addEventListener('click', function () {
      var current = document.documentElement.getAttribute('data-theme') === 'dark' ? 'dark' : 'light';
      applyTheme(current === 'dark' ? 'light' : 'dark');
    });
    applyTheme(document.documentElement.getAttribute('data-theme') === 'dark' ? 'dark' : 'light');
  }

  /* copy */
  function copyText(text) {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      return navigator.clipboard.writeText(text);
    }
    return new Promise(function (resolve, reject) {
      var ta = document.createElement('textarea');
      ta.value = text;
      ta.setAttribute('readonly', '');
      ta.style.position = 'fixed';
      ta.style.left = '-9999px';
      document.body.appendChild(ta);
      ta.select();
      try {
        document.execCommand('copy');
        resolve();
      } catch (e) {
        reject(e);
      }
      document.body.removeChild(ta);
    });
  }

  if (copyBtn) {
    copyBtn.addEventListener('click', function () {
      var label = copyBtn.querySelector('span');
      var old = label ? label.textContent : '';
      copyText(window.location.href).then(function () {
        if (label) label.textContent = '已复制';
        copyBtn.classList.add('is-busy');
        setTimeout(function () {
          if (label) label.textContent = old;
          copyBtn.classList.remove('is-busy');
        }, 1800);
      }).catch(function () {});
    });
  }

  if (refreshBtn) {
    refreshBtn.addEventListener('click', function () { load(true); });
  }

  document.addEventListener('click', function (e) {
    var retry = e.target.closest ? e.target.closest('[data-retry]') : null;
    if (retry) load(true);
  });

  load(false);
})();
