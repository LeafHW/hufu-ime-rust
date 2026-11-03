import { useEffect, useState } from 'react';
import type { MouseEvent as ReactMouseEvent } from 'react';
import { Icon } from './Icon';
import type { About, EngineState, Route, ThemeMode } from '../types';

export const PAGE_TITLE: Record<Route, string> = {
  overview: '概览',
  schemes: '方案',
  input: '输入',
  keymap: '键位编辑',
  skin: '皮肤工作室',
  buddy: '候选挂件',
  sentence: '整句模型',
  data: '数据与备份',
  sync: '云同步',
  about: '关于',
  donate: '打赏支持',
};

const GROUPS: { label: string; items: { key: Route; label: string; icon: string }[] }[] = [
  {
    label: '常用',
    items: [
      { key: 'overview', label: '概览', icon: 'home' },
      { key: 'schemes', label: '方案', icon: 'grid' },
      { key: 'input', label: '输入', icon: 'keyboard' },
    ],
  },
  {
    label: '外观',
    items: [
      { key: 'skin', label: '皮肤', icon: 'palette' },
      { key: 'buddy', label: '挂件', icon: 'image' },
    ],
  },
  {
    label: '进阶',
    items: [
      { key: 'sentence', label: '整句模型', icon: 'spark' },
      { key: 'keymap', label: '键位', icon: 'keyboard' },
      { key: 'data', label: '数据与备份', icon: 'db' },
      { key: 'sync', label: '云同步', icon: 'cloud' },
    ],
  },
  {
    label: '其他',
    items: [
      { key: 'about', label: '关于', icon: 'info' },
      { key: 'donate', label: '打赏支持', icon: 'heart' },
    ],
  },
];

/**
 * 关窗统一入口（P0）：壳内走 WebView2 postMessage('win:close')，浏览器预览
 * 退回 window.close()。App 的未保存守卫确认后调用这里，保证不会绕过脏状态检查。
 */
export function requestWindowClose() {
  const webview = (
    window as unknown as {
      chrome?: { webview?: { postMessage?: (msg: string) => void } };
    }
  ).chrome?.webview;
  if (
    typeof document !== 'undefined' &&
    document.body.classList.contains('webview') &&
    webview?.postMessage
  ) {
    webview.postMessage('win:close');
    return;
  }
  try {
    window.close();
  } catch {
    /* 浏览器预览下可能被浏览器拒绝，忽略 */
  }
}

export function TitleBar({
  title,
  onClose,
  onRequestClose,
}: {
  title: string;
  onClose?: () => void;
  /** 由 App 注入的未保存守卫：有脏数据时先弹确认，不直接发 win:close。 */
  onRequestClose?: () => void;
}) {
  const [maximized, setMaximized] = useState(false);
  const [snapOpen, setSnapOpen] = useState(false);

  const inShell = () =>
    typeof document !== 'undefined' && document.body.classList.contains('webview');

  const postWindow = (action: string) => {
    if (!inShell()) return;
    const webview = (
      window as unknown as {
        chrome?: { webview?: { postMessage?: (msg: string) => void } };
      }
    ).chrome?.webview;
    if (webview?.postMessage) webview.postMessage(`win:${action}`);
  };

  useEffect(() => {
    const webview = (
      window as unknown as {
        chrome?: { webview?: { postMessage?: (msg: string) => void } };
      }
    ).chrome?.webview;
    if (!webview?.postMessage) return;
    /* 【2026-11-02】只发一次 ready，且等两帧 + 80ms：让合成器先出首帧，
       壳侧收到 ready 再推迟 100ms 显示窗口，避免露出 WebView2 空白表面。 */
    let sent = false;
    const send = () => {
      if (sent) return;
      sent = true;
      webview.postMessage?.('win:ready');
    };
    let inner = 0;
    const raf = requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        inner = window.setTimeout(send, 80);
      }),
    );
    return () => {
      cancelAnimationFrame(raf);
      window.clearTimeout(inner);
    };
  }, []);

  useEffect(() => {
    const webview = (
      window as unknown as {
        chrome?: {
          webview?: {
            addEventListener?: (type: string, cb: (e: MessageEvent) => void) => void;
            removeEventListener?: (type: string, cb: (e: MessageEvent) => void) => void;
          };
        };
      }
    ).chrome?.webview;
    if (!webview?.addEventListener) return;
    const onMessage = (e: MessageEvent) => {
      if (e.data === 'win:maximized') setMaximized(true);
      else if (e.data === 'win:restored') setMaximized(false);
    };
    webview.addEventListener('message', onMessage);
    return () => webview.removeEventListener?.('message', onMessage);
  }, []);

  const onBarMouseDown = (e: ReactMouseEvent<HTMLDivElement>) => {
    if (e.button !== 0 || !inShell()) return;
    if ((e.target as HTMLElement).closest('button, a, input, .no-drag')) return;
    const sx = e.clientX;
    const sy = e.clientY;
    let moved = false;
    const cleanup = () => {
      window.removeEventListener('mousemove', move);
      window.removeEventListener('mouseup', up);
    };
    const move = (ev: MouseEvent) => {
      if (moved) return;
      if (Math.abs(ev.clientX - sx) > 4 || Math.abs(ev.clientY - sy) > 4) {
        moved = true;
        cleanup();
        postWindow('drag');
      }
    };
    const up = () => cleanup();
    window.addEventListener('mousemove', move);
    window.addEventListener('mouseup', up, { once: true });
  };

  const onDoubleClick = (e: ReactMouseEvent<HTMLDivElement>) => {
    if ((e.target as HTMLElement).closest('button, a, input')) return;
    postWindow('maximize');
  };

  return (
    <div className="titlebar" onMouseDown={onBarMouseDown} onDoubleClick={onDoubleClick}>
      <div className="tb-title">虎符设置</div>
      <div className="tb-sep" />
      <div className="tb-page" style={{ display: 'inline' }}>{title}</div>
      <div className="tb-spacer" />
      <div className="win-btns no-drag">
        <button type="button" className="win-btn" aria-label="最小化" title="最小化" onClick={() => postWindow('minimize')}>
          <svg className="wm" viewBox="0 0 10 10"><path d="M1 5h8" stroke="currentColor" strokeWidth="1.1" /></svg>
        </button>
        <div
          className="win-max-wrap"
          onMouseEnter={() => setSnapOpen(true)}
          onMouseLeave={() => setSnapOpen(false)}
        >
          <button
            type="button"
            className="win-btn"
            aria-label={maximized ? '还原' : '最大化'}
            title={maximized ? '还原' : '最大化'}
            onClick={() => postWindow('maximize')}
          >
            {maximized ? (
              <svg className="wm" viewBox="0 0 10 10">
                <rect x="2.2" y="1.2" width="6.6" height="6.6" rx=".8" fill="none" stroke="currentColor" strokeWidth="1.05" />
                <path d="M1.2 3.2v5.6h5.6" fill="none" stroke="currentColor" strokeWidth="1.05" />
              </svg>
            ) : (
              <svg className="wm" viewBox="0 0 10 10"><rect x="1.2" y="1.2" width="7.6" height="7.6" rx="1" fill="none" stroke="currentColor" strokeWidth="1.1" /></svg>
            )}
          </button>
          {snapOpen ? (
            <div className="snap-flyout no-drag" role="menu" aria-label="窗口布局">
              <div className="snap-title">窗口布局</div>
              <div className="snap-grid">
                {(
                  [
                    ['left', '左半屏', '▥'],
                    ['right', '右半屏', '▥'],
                    ['tl', '左上', '◰'],
                    ['tr', '右上', '◳'],
                    ['bl', '左下', '◱'],
                    ['br', '右下', '◲'],
                  ] as const
                ).map(([zone, label, glyph]) => (
                  <button
                    key={zone}
                    type="button"
                    className={`snap-cell snap-${zone}`}
                    title={label}
                    onClick={() => {
                      postWindow(`snap-${zone}`);
                      setSnapOpen(false);
                    }}
                  >
                    <span>{glyph}</span>
                  </button>
                ))}
              </div>
              <div className="snap-hint">点击铺满对应区域</div>
            </div>
          ) : null}
        </div>
        <button
          type="button"
          className="win-btn close"
          aria-label="关闭"
          title="关闭"
          onClick={() => {
            /* 优先交给 App 的关窗守卫（P0）：脏数据时弹确认，而不是直接关。 */
            if (onRequestClose) {
              onRequestClose();
              return;
            }
            if (inShell()) postWindow('close');
            else onClose?.();
          }}
        >
          <svg className="wm" viewBox="0 0 10 10"><path d="m1.5 1.5 7 7m0-7-7 7" stroke="currentColor" strokeWidth="1.1" /></svg>
        </button>
      </div>
    </div>
  );
}

export function Sidebar({
  route,
  onNavigate,
  about,
  state,
  theme,
  onTheme,
  online,
}: {
  route: Route;
  onNavigate: (r: Route) => void;
  about: About;
  state: EngineState;
  theme: ThemeMode;
  onTheme: (m: ThemeMode) => void;
  online: boolean;
}) {
  const status = online ? (state.model_present ? '引擎运行正常' : '模型未加载') : '离线预览';
  const dotClass = !online ? 'dot muted' : state.model_present ? 'dot' : 'dot gold';
  return (
    <aside className="sidebar">
      <div className="brand">
        <img src="/hufu-icon-q-256.png" alt="" />
        <div className="brand-copy">
          <div className="brand-head">
            <div className="bn">虎<em>符</em></div>
            <div className="be"><span className="be-text">HUFU</span></div>
          </div>
          <div className="bs">输入法设置</div>
        </div>
      </div>
      <nav className="nav">
        {GROUPS.map((group) => (
          <div key={group.label}>
            <div className="nav-group">{group.label}</div>
            {group.items.map((item) => (
              <div
                key={item.key}
                className={`nav-item${route === item.key ? ' on' : ''}`}
                title={item.label}
                role="button"
                tabIndex={0}
                onClick={() => onNavigate(item.key)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') onNavigate(item.key);
                }}
              >
                <Icon name={item.icon} />
                {item.label}
              </div>
            ))}
          </div>
        ))}
      </nav>
      <div className="side-bottom">
        <div className="theme-seg" role="group" aria-label="主题">
          {(
            [
              ['system', 'monitor'],
              ['dark', 'moon'],
              ['light', 'sun'],
            ] as const
          ).map(([mode, icon]) => (
            <button
              key={mode}
              type="button"
              className={`ts${theme === mode ? ' on' : ''}`}
              title={mode === 'system' ? '跟随系统' : mode === 'dark' ? '深色' : '浅色'}
              aria-label={mode === 'system' ? '跟随系统' : mode === 'dark' ? '深色' : '浅色'}
              aria-pressed={theme === mode}
              onClick={() => onTheme(mode)}
            >
              <Icon name={icon} />
            </button>
          ))}
        </div>
        <div className="side-meta">
          <span className={dotClass} />v{about.version} · {status}
        </div>
      </div>
    </aside>
  );
}
