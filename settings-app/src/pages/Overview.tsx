import { useEffect, useRef, useState } from 'react';
import type { Bootstrap, Route } from '../types';
import { fallbackSkin } from '../types';
import { syncNow } from '../api';
import { fetchUpdateCheck, type UpdateState } from '../updateCheck';
import { useDirtyPages } from '../dirtyGuard';
import { Icon } from '../components/Icon';
import { Badge, PageHead } from '../components/ui';
import { useBodyLight } from '../hooks';
import { SEARCH_ITEMS } from './searchIndex';

const shortName = (n: string) => n.replace(/（.*?）|\(.*?\)/g, '').trim() || n;

/** 把颜色往浅纸上混：t=0 原色、t=1 纯浅色。浅色主题的皮肤缩略图用它提亮，
    避免把深色皮肤原样画出来变成一块黑。 */
function tintToLight(hex: string, t: number, light = '#f7f2e8'): string {
  const parse = (h: string) => {
    const v = h.replace('#', '');
    if (v.length !== 6) return null;
    return [parseInt(v.slice(0, 2), 16), parseInt(v.slice(2, 4), 16), parseInt(v.slice(4, 6), 16)];
  };
  const a = parse(hex) || [42, 46, 54];
  const b = parse(light) || [247, 242, 232];
  const m = (i: number) => Math.round(a[i] * (1 - t) + b[i] * t);
  return `rgb(${m(0)}, ${m(1)}, ${m(2)})`;
}

/** 中文长词按“字符顺序子序列”兜底：候选字号可命中“候选窗…字号”。 */
function isSubsequence(needle: string, hay: string): boolean {
  let i = 0;
  for (const ch of hay) {
    if (ch === needle[i]) i += 1;
    if (i === needle.length) return true;
  }
  return needle.length === 0;
}




function formatTime(ts: number): string {
  if (!ts) return '尚未同步';
  try {
    return new Date(ts * 1000).toLocaleString('zh-CN', {
      month: 'numeric',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  } catch {
    return '—';
  }
}

export function Overview({
  data,
  notify,
  navigate,
}: {
  data: Bootstrap;
  notify: (msg: string) => void;
  navigate: (r: Route, sub?: string) => void;
}) {
  const [syncing, setSyncing] = useState(false);
  /* 【检查更新 2026-11-01】概览行与关于页同款：真实调 /api/update/check；
     有更新时该行变「直接下载」，无 exe 资产回退下载页。 */
  const [upd, setUpd] = useState<UpdateState>({ kind: 'idle' });
  const onCheckUpdateRow = async () => {
    if (upd.kind === 'checking') return;
    if (upd.kind === 'update') {
      window.open(upd.downloadUrl || upd.pageUrl, '_blank', 'noopener');
      return;
    }
    setUpd({ kind: 'checking' });
    setUpd(await fetchUpdateCheck(data.about.github));
  };
  const [query, setQuery] = useState(() => {
    try {
      return new URLSearchParams(location.search).get('q') || '';
    } catch {
      return '';
    }
  });
  const [helpOpen, setHelpOpen] = useState(false);
  const searchRef = useRef<HTMLInputElement>(null);

  const light = useBodyLight();
  const skin = data.skin || fallbackSkin;
  const currentMeta = data.skins.skins.find((s) => s.id === data.skins.current);
  const skinName = shortName(data.skin?.name || currentMeta?.name || '墨言');
  const openSchemaRoot = async () => {
    try {
      const r = await fetch('/api/open_dir', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ kind: 'schema' }),
      });
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      notify('已在资源管理器打开码表目录');
    } catch (e) {
      notify(`打开码表目录失败：${(e as Error).message}`);
    }
  };

  const results = query.trim()
    ? (() => {
        const qRaw = query.trim().toLowerCase();
        const tokens = qRaw.split(/\s+/).filter(Boolean);
        const compact = qRaw.replace(/[\s·:：/、，,。.\-—]+/g, '');
        const hit = (t: string, hay: string) => {
          if (hay.includes(t)) return true;
          if (t.length >= 3 && /[㐀-鿿]/.test(t)) return isSubsequence(t, hay);
          return false;
        };
        return SEARCH_ITEMS.filter((item) => {
          const hay = `${item.title} ${item.desc} ${item.group} ${item.keys}`.toLowerCase();
          if (tokens.every((t) => hit(t, hay))) return true;
          return hay.replace(/[\s·:：/、，,。.\-—]+/g, '').includes(compact);
        });
      })()
    : SEARCH_ITEMS.slice(0, 6);

  const focusSearch = () => {
    searchRef.current?.focus();
    searchRef.current?.scrollIntoView({ behavior: 'smooth', block: 'center' });
  };

  /* Ctrl+K 已提升到 App 层（全局快捷键）；本页只响应聚焦事件。 */
  useEffect(() => {
    const onFocus = () => focusSearch();
    window.addEventListener('hufu:focus-search', onFocus);
    return () => window.removeEventListener('hufu:focus-search', onFocus);
  }, []);

  /* 假状态清理②：真实的“设置保存”状态来自全局脏状态注册表。 */
  const dirtyPages = useDirtyPages();

  /* RT-05：Esc 关闭帮助弹层。 */
  useEffect(() => {
    if (!helpOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        setHelpOpen(false);
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [helpOpen]);

  const scheme = data.state.current_schema || '—';
  const sync = data.sync;
  const selected = sync.selected?.length ?? data.syncSchemes.schemes.filter((s) => s.selected).length;
  const totalSchemes = data.syncSchemes.schemes.length;
  const files = data.syncSchemes.schemes.filter((s) => s.selected).reduce((a, b) => a + b.files, 0);
  const colorCount = Object.keys(skin.colors || {}).length;
  const highlight = skin.colors?.hilited_candidate_back_color || '#E7B75C';
  const back = skin.colors?.back_color || '#2a2e36';

  const syncLabel = !sync.configured
    ? '未配置'
    : sync.last_error
      ? '同步异常'
      : sync.stopped
        ? '已停止'
        : sync.phase === 'syncing'
          ? '同步中'
          : '已同步';
  const syncDot = !sync.configured ? 'dot muted' : sync.last_error || sync.stopped ? 'dot red' : 'dot';

  const runSync = async () => {
    if (syncing) return;
    setSyncing(true);
    try {
      await syncNow();
      notify('已触发同步，稍候查看状态');
    } catch (e) {
      notify((e as Error).message);
    } finally {
      setSyncing(false);
    }
  };

  return (
    <>
      <PageHead
        title="概览"
        sub={
          <>
            <span className={data.online ? 'dot' : 'dot muted'} />
            {data.online ? (data.state.model_present ? '引擎运行正常' : '模型未加载') : '离线预览'}
            <span style={{ color: 'var(--text3)' }}>·</span>
            {data.state.model_present ? '模型已加载' : '未安装整句模型'}
            <span style={{ color: 'var(--text3)' }}>·</span>
            {scheme}
          </>
        }
        actions={
          <>
            <button type="button" className="icon-btn" title="搜索设置" onClick={focusSearch}>
              <Icon name="search" />
            </button>
            <button type="button" className="icon-btn" title="帮助" onClick={() => setHelpOpen(true)}>
              <Icon name="help" />
            </button>
          </>
        }
      />

      <div className="status-grid">
        <div
          className="card status-card clickable"
          role="button"
          tabIndex={0}
          aria-label="打开方案设置"
          title="点击打开方案设置"
          onClick={() => navigate('schemes')}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              navigate('schemes');
            }
          }}
        >
          <div className="sc-top">
            <div className="sc-label">当前方案</div>
            <Icon name="grid" className="icon sc-icon" />
          </div>
          <div className="sc-value">
            {scheme} <Badge>{data.state.sentence_active ? '整句激活' : '码表'}</Badge>
          </div>
          <div className="sc-sub">{data.state.schemas.length} 个方案 · 当前生效</div>
          <div className="sc-link" onClick={(e) => { e.stopPropagation(); navigate('schemes'); }}>
            切换方案 <Icon name="arrow" />
          </div>
        </div>

        <div
          className="card status-card skin clickable"
          role="button"
          tabIndex={0}
          aria-label="打开皮肤工作室"
          title="点击打开皮肤工作室"
          onClick={() => navigate('skin')}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              navigate('skin');
            }
          }}
        >
          <div className="sc-top">
            <div className="sc-label">当前皮肤</div>
            <Icon name="palette" className="icon sc-icon" />
          </div>
          <div className="sc-value">
            <span
              className="swatch-mini"
              style={{
                background: light
                  ? `linear-gradient(135deg, ${tintToLight(back, 0.46)}, ${tintToLight(back, 0.76)} 58%, ${tintToLight(highlight, 0.3)})`
                  : `linear-gradient(135deg, ${back}, #15171b 58%, ${highlight})`,
              }}
            />
            {skinName}
          </div>
          <div className="sc-sub">
            {data.skins.current_kind === 'ssf' ? '位图皮肤' : '矢量皮肤'} · {colorCount || 21} 个颜色角色
          </div>
          <div className="sc-link" onClick={(e) => { e.stopPropagation(); navigate('skin'); }}>
            打开皮肤工作室 <Icon name="arrow" />
          </div>
        </div>

        <div
          className="card status-card clickable"
          role="button"
          tabIndex={0}
          aria-label="打开云同步设置"
          title="点击打开云同步设置"
          onClick={() => navigate('sync')}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              navigate('sync');
            }
          }}
        >
          <div className="sc-top">
            <div className="sc-label">云同步</div>
            <Icon name="cloud" className="icon sc-icon" />
          </div>
          <div className="sc-value">
            <span className={syncDot} />
            {syncLabel}
          </div>
          <div className="sc-sub">
            {sync.configured ? `${selected} 个方案 · ${files} 个文件` : '尚未配置 WebDAV'}
          </div>
          <div
            className="sc-link"
            onClick={(e) => {
              e.stopPropagation();
              if (sync.configured) void runSync();
              else navigate('sync');
            }}
          >
            {syncing ? '同步中…' : sync.configured ? '立即同步' : '去配置'} <Icon name="arrow" />
          </div>
        </div>
      </div>

      <div className="overview-body">
        <div className="card overview-search">
          <div className="os-head">
            <div>
              <div className="card-title">搜索设置</div>
              <div className="os-sub">输入功能名，直接跳到对应页面；也可以按 Ctrl K。</div>
            </div>
            <kbd className="os-kbd">Ctrl K</kbd>
          </div>
          <div className="os-input">
            <Icon name="search" />
            <input
              ref={searchRef}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && results[0]) navigate(results[0].route, results[0].sub);
                if (e.key === 'Escape') setQuery('');
              }}
              placeholder="搜索：中英切换、候选数、键位、皮肤、整句、同步…"
              aria-label="搜索设置"
            />
            {query ? (
              <button type="button" className="os-clear" onClick={() => setQuery('')} aria-label="清除搜索">
                <Icon name="close" />
              </button>
            ) : null}
          </div>
          <div className="os-list" role="listbox" aria-label="搜索结果">
            {results.length === 0 ? (
              <div className="os-empty">
                <Icon name="search" />
                <b>没有找到「{query}」</b>
                <span>试试搜索“候选”“键位”“皮肤”“同步”或“声音”。</span>
              </div>
            ) : results.map((item) => (
              <button
                type="button"
                className="os-item"
                key={item.title}
                role="option"
                onClick={() => navigate(item.route, item.sub)}
              >
                <span className="os-ico"><Icon name={item.icon} /></span>
                <span className="os-main">
                  <b>{item.title}</b>
                  <span>{item.desc}</span>
                </span>
                <span className="os-badge">{item.group}</span>
                <Icon name="chev" />
              </button>
            ))}
          </div>
          <div className="os-foot">
            <span>回车打开第一个结果</span>
            <span>Esc 清空</span>
          </div>
        </div>

        <div className="card side-card">
          <div className="card-head" style={{ marginBottom: 4 }}>
            <span className="card-title">常用操作</span>
            <span className="more" onClick={() => navigate('input', 'switch')}>全部设置</span>
          </div>
          <div className="act-row" onClick={() => navigate('keymap')}>
            <div className="act-ico"><Icon name="keyboard" /></div>
            <div className="act-main"><b>键位编辑</b><span>主键盘、小键盘与 Shift+符号</span></div>
            <Icon name="chev" className="icon sm chev" />
          </div>
          <div className="act-row" onClick={() => navigate('data')}>
            <div className="act-ico"><Icon name="upload" /></div>
            <div className="act-main"><b>导入设置</b><span>从 JSON 恢复全部配置</span></div>
            <Icon name="chev" className="icon sm chev" />
          </div>
          <div className="act-row" onClick={() => void openSchemaRoot()}>
            <div className="act-ico"><Icon name="folder" /></div>
            <div className="act-main"><b>打开码表目录</b><span>当前方案的码表根目录</span></div>
            <Icon name="chev" className="icon sm chev" />
          </div>
          <div
            className={`act-row${upd.kind === 'update' ? ' has-update' : ''}`}
            onClick={() => void onCheckUpdateRow()}
          >
            <div className="act-ico"><Icon name="refresh" /></div>
            <div className="act-main">
              <b>{upd.kind === 'update' ? `发现新版本 v${upd.latest}` : '检查更新'}</b>
              <span>
                {upd.kind === 'idle' && `当前 v${data.about.version} · 点击检查`}
                {upd.kind === 'checking' && '检查中…'}
                {upd.kind === 'latest' && `已是最新（v${upd.current}）`}
                {upd.kind === 'update' && (upd.downloadUrl ? '点击直接下载安装包' : '点击打开下载页')}
                {upd.kind === 'error' && `检查失败：${upd.msg}`}
              </span>
            </div>
            <Icon name={upd.kind === 'update' ? 'arrow' : 'chev'} className="icon sm chev" />
          </div>
          <div className="side-tip">
            <b>小提示</b>
            <span>按 Ctrl+Alt+H 随时打开设置；按 Ctrl K 或顶部搜索快速跳到任意设置项。</span>
          </div>
          <div className="engine-box">
            {/* 假状态清理②：删除无真实来源的「候选窗跟随中 · ≤2.5s」；
                「设置保存」改读全局脏状态注册表。 */}
            <div className="engine-line">
              <span>输入法状态</span><b>{data.state.state?.chinese ? '中文' : '英文'} · {data.state.state?.full_shape ? '全角' : '半角'}</b>
            </div>
            <div className="engine-line">
              <span>设置保存</span>
              <b>
                {data.online
                  ? dirtyPages.length
                    ? `${dirtyPages.map((d) => d.label).join('、')} 有未保存改动`
                    : '已保存 ✓'
                  : '未知（离线）'}
              </b>
            </div>
          </div>
        </div>
      </div>

      {helpOpen ? (
        <div
          className="help-sheet"
          role="dialog"
          aria-modal="true"
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) setHelpOpen(false);
          }}
        >
          <div className="card help-card">
            <div className="help-head">
              <div>
                <h3>虎符设置 · 帮助</h3>
                <div className="sub">常用操作、快捷键与排查</div>
              </div>
              <button type="button" className="btn ghost" onClick={() => setHelpOpen(false)}>关闭</button>
            </div>
            <div className="help-body">
              <section>
                <h4>快速上手</h4>
                <ul>
                  <li>左侧按「概览 / 方案 / 输入 / 外观 / 进阶 / 其他」分组；所有改动点底部「保存并应用」才生效。</li>
                  <li>概览搜索支持 <b>Ctrl K</b>：输入“候选/音效/皮肤/同步”等直接跳到对应页面或输入页签。</li>
                  <li>输入页分「切换 / 编码 / 候选 / 声音 / 高级」五类，不再一屏铺满。</li>
                  <li>键位编辑支持点击后直接按键录制；Esc 取消，Backspace 清除。</li>
                  <li>皮肤工作室可导入 <code>.ssf</code>；新建皮肤会以当前矢量皮肤为模板。</li>
                </ul>
              </section>
              <section>
                <h4>快捷键</h4>
                <div className="help-keys">
                  <span>Ctrl K</span><b>聚焦设置搜索</b>
                  <span>Ctrl Alt H</span><b>显示/隐藏设置窗</b>
                  <span>Shift</span><b>中英切换（可关）</b>
                  <span>Ctrl M</span><b>最近方案切换</b>
                  <span>Esc</span><b>取消录制 / 清屏</b>
                </div>
              </section>
              <section>
                <h4>常见问题</h4>
                <dl>
                  <dt>改完为什么没生效？</dt><dd>输入/键位页底部点「保存并应用」；底部状态会显示未保存/已保存。</dd>
                  <dt>云同步怎么连？</dt><dd>云同步页右上「教程」有坚果云应用密码、测试连接、勾选方案、第二台设备完整步骤。</dd>
                  <dt>皮肤导入失败？</dt><dd>只支持虎符可解析的 .ssf；导入后列表约 10 秒刷新，失败会提示原因。</dd>
                  <dt>想恢复默认？</dt><dd>各页有「恢复默认」；输入页会写入旧值，需再保存并应用。</dd>
                </dl>
              </section>
            </div>
            <div className="help-foot">
              <span>更多细节见 <code>hufu/docs/设置窗口验收报告.md</code></span>
              <button type="button" className="btn primary" onClick={() => setHelpOpen(false)}>知道了</button>
            </div>
          </div>
        </div>
      ) : null}
    </>
  );
}
