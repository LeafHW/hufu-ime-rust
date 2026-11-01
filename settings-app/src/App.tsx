import { useCallback, useEffect, useRef, useState } from 'react';
import { loadBootstrap } from './api';
import { PAGE_TITLE, requestWindowClose, Sidebar, TitleBar } from './components/Shell';
import { Toast } from './components/ui';
import { AboutPage } from './pages/AboutPage';
import { DonatePage } from './pages/DonatePage';
import { Overview } from './pages/Overview';
import { SkinStudio } from './pages/SkinStudio';
import { Sync } from './pages/Sync';
import { SchemesPage } from './pages/Schemes';
import { DataPage } from './pages/Data';
import { InputPage } from './pages/Input';
import { KeymapPage } from './pages/Keymap';
import { SentencePage } from './pages/Sentence';
import { BuddyPage } from './pages/Buddy';
import { fallbackBootstrap, type Bootstrap, type Route, type ThemeMode } from './types';
import { getDirtyPages, resetDirty, subscribeDirty } from './dirtyGuard';
import './styles/app.css';

const ROUTES: Route[] = ['overview', 'schemes', 'input', 'keymap', 'skin', 'buddy', 'sentence', 'data', 'sync', 'about', 'donate'];

function readRoute(): Route {
  const h = (location.hash || '').replace('#', '').replace(/^\//, '').split('/')[0];
  return (ROUTES as string[]).includes(h) ? (h as Route) : 'overview';
}

declare global {
  interface Window {
    /** 壳侧 WM_CLOSE 前的同步查询口：true=无未保存修改，可直接关。 */
    __hufuCanClose?: () => boolean;
  }
}

/* 离线判据（P1）：只有核心三端点失败才算离线。
   api.loadBootstrap 的 online 是「任一请求失败即离线」，skins / sync 抖动
   也会把整站标成离线；api.ts 由别人维护，这里在 App 侧按错误标签纠正。
   错误串格式见 api.ts safe()：`${tag}: ${message}`。 */
const CORE_TAGS = ['about', 'state', 'config'];
function normalizeBootstrap(b: Bootstrap): Bootstrap {
  const online = !b.errors.some((e) => CORE_TAGS.some((tag) => e.startsWith(`${tag}: `)));
  return online === b.online ? b : { ...b, online };
}

export default function App() {
  const [route, setRoute] = useState<Route>(readRoute);
  const [data, setData] = useState<Bootstrap>(fallbackBootstrap);
  /* RT-13：?theme= 参数只一次性生效，不写回 localStorage；
     skipPersist 在「当前主题来自 URL 参数」期间保持 true。 */
  const themeParam = (() => {
    const q = new URLSearchParams(location.search).get('theme');
    return q === 'light' || q === 'dark' || q === 'system' ? (q as ThemeMode) : null;
  })();
  const [theme, setTheme] = useState<ThemeMode>(() => themeParam ?? (() => {
    const saved = localStorage.getItem('hufu.theme');
    return saved === 'light' || saved === 'dark' || saved === 'system' ? saved : 'system';
  })());
  const skipPersist = useRef<boolean>(themeParam !== null);
  const [toast, setToast] = useState<string | null>(null);
  const toastTimer = useRef<number>(0);
  const [navConfirm, setNavConfirm] = useState<{ to: Route; sub?: string } | null>(null);
  const [dirtyPages, setDirtyPages] = useState(() => getDirtyPages());
  /* 关窗确认弹层（P0）：脏数据存在时不直接发 win:close。 */
  const [closeConfirm, setCloseConfirm] = useState(false);
  /* 【2026-11-02】放弃修改后常驻挂载的页面不会卸载，dirty 与本地编辑会赖着；
     用 epoch 变更 <main> 的 key 让全部页面重挂载（等于回到服务端快照）。 */
  const [epoch, setEpoch] = useState(0);

  /* 全局轮询（P1）：
     - loadSeq 单调递增，只让「最新一轮」的响应落库，慢响应不会覆盖新数据；
     - inFlight 单飞标记，请求未回来不发起下一轮（轮询与页面手刷共用）；
     - document.hidden 时跳过，visibilitychange 回前台立即补一轮。 */
  const loadSeq = useRef(0);
  const inFlight = useRef(false);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const refresh = useCallback(async () => {
    const seq = ++loadSeq.current;
    inFlight.current = true;
    try {
      const next = normalizeBootstrap(await loadBootstrap());
      /* 只有最新一轮的响应能覆盖 state；旧轮次直接丢弃。 */
      if (mounted.current && seq === loadSeq.current) setData(next);
      return next;
    } finally {
      /* 旧轮次不放闸：它可能晚于新一轮启动，提前清标记会让两轮叠加。 */
      if (seq === loadSeq.current) inFlight.current = false;
    }
  }, []);

  useEffect(() => {
    const poll = () => {
      /* 后台标签/隐藏窗口不轮询：省电，也避免无意义的请求堆积。 */
      if (document.hidden) return;
      /* 上一轮没回来就跳过，不叠加并发请求。 */
      if (inFlight.current) return;
      void refresh().catch(() => {
        /* 单轮失败保留上一轮数据，等下个轮询 */
      });
    };
    poll();
    const timer = window.setInterval(poll, 10000);
    /* 回前台立刻补一轮，避免隐藏期间展示旧数据。 */
    const onVisibility = () => {
      if (!document.hidden) poll();
    };
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [refresh]);

  useEffect(() => {
    const onHash = () => setRoute(readRoute());
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);

  useEffect(() => {
    document.body.dataset.screen = route === 'about' || route === 'donate' ? 'other' : route;
    document.title = `${PAGE_TITLE[route]} · 虎符设置`;
  }, [route]);

  useEffect(() => {
    const apply = () => {
      const light =
        theme === 'light' ||
        (theme === 'system' && window.matchMedia('(prefers-color-scheme: light)').matches);
      document.body.classList.toggle('light', light);
    };
    apply();
    if (!skipPersist.current) localStorage.setItem('hufu.theme', theme);
    const mq = window.matchMedia('(prefers-color-scheme: light)');
    const onSystem = () => {
      if (theme === 'system') apply();
    };
    mq.addEventListener?.('change', onSystem);
    return () => mq.removeEventListener?.('change', onSystem);
  }, [theme]);

  const onTheme = (m: ThemeMode) => {
    skipPersist.current = false;
    setTheme(m);
  };

  /* OV-B1：订阅全局脏状态 —— 关窗（beforeunload）与切路由统一拦截。 */
  useEffect(() => subscribeDirty(() => setDirtyPages(getDirtyPages())), []);
  useEffect(() => {
    const onBeforeUnload = (e: BeforeUnloadEvent) => {
      if (getDirtyPages().length) {
        e.preventDefault();
        e.returnValue = '';
      }
    };
    window.addEventListener('beforeunload', onBeforeUnload);
    return () => window.removeEventListener('beforeunload', onBeforeUnload);
  }, []);

  /* P0：壳侧 WM_CLOSE 不会触发 beforeunload，暴露同步查询口给宿主以后调用；
     没有脏数据时返回 true 可直接关，false 时壳侧应改为走页面内关闭流程。 */
  useEffect(() => {
    window.__hufuCanClose = () => getDirtyPages().length === 0;
    return () => {
      delete window.__hufuCanClose;
    };
  }, []);

  /* 关窗守卫：标题栏 ✕ 不再直发 win:close，先问 App 是否有未保存修改。
     有脏数据 → 弹确认（复用导航确认弹层样式）；无脏数据 / 已确认 → 交给壳关窗。 */
  const onRequestClose = () => {
    if (getDirtyPages().length) {
      setNavConfirm(null);
      setCloseConfirm(true);
      return;
    }
    requestWindowClose();
  };

  const doNavigate = (r: Route, sub?: string) => {
    const target = sub ? `${r}/${sub}` : r;
    try {
      history.replaceState(null, '', `#${target}`);
    } catch {
      /* file:// */
    }
    setRoute(r);
    window.dispatchEvent(new Event('hashchange'));
  };

  const navigate = (r: Route, sub?: string): boolean => {
    if (navConfirm) return false;
    if (r !== route && getDirtyPages().length) {
      setCloseConfirm(false);
      setNavConfirm({ to: r, sub });
      return false;
    }
    doNavigate(r, sub);
    return true;
  };

  /* Ctrl+K 全局聚焦搜索（原先只挂在概览页，其余页面无反应）。 */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        if (route === 'overview' || navigate('overview')) {
          window.dispatchEvent(new CustomEvent('hufu:focus-search'));
        }
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [route, navConfirm]);

  /* 导航 / 关窗确认弹层：Esc 取消。 */
  useEffect(() => {
    if (!navConfirm && !closeConfirm) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        /* 只关最上层：关窗确认优先，其次导航确认。 */
        if (closeConfirm) setCloseConfirm(false);
        else setNavConfirm(null);
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [navConfirm, closeConfirm]);

  const notify = (msg: string) => {
    setToast(msg);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 2600);
  };

  return (
    <div className="window">
      <TitleBar title={PAGE_TITLE[route]} onRequestClose={onRequestClose} />
      <div className="shell">
        <Sidebar
          route={route}
          onNavigate={navigate}
          about={data.about}
          state={data.state}
          theme={theme}
          onTheme={onTheme}
          online={data.online}
        />
        <main className="main" key={epoch}>
          <section className="screen" id="s-overview">
            <Overview data={data} notify={notify} navigate={navigate} />
          </section>
          <section className="screen" id="s-skin">
            <SkinStudio data={data} notify={notify} refresh={refresh} />
          </section>
          <section className="screen" id="s-sync">
            <Sync data={data} notify={notify} />
          </section>
          <section className="screen" id="s-schemes">
            {route === 'schemes' ? <SchemesPage data={data} notify={notify} /> : null}
          </section>
          <section className="screen" id="s-input">
            {route === 'input' ? <InputPage data={data} notify={notify} /> : null}
          </section>
          <section className="screen" id="s-keymap">
            {route === 'keymap' ? <KeymapPage data={data} notify={notify} /> : null}
          </section>
          <section className="screen" id="s-sentence">
            {route === 'sentence' ? <SentencePage data={data} notify={notify} /> : null}
          </section>
          <section className="screen" id="s-buddy">
            {route === 'buddy' ? <BuddyPage data={data} notify={notify} /> : null}
          </section>
          <section className="screen" id="s-data">
            {route === 'data' ? <DataPage data={data} notify={notify} /> : null}
          </section>
          <section className="screen" id="s-other">
            {route === 'about' ? <AboutPage about={data.about} /> : null}
            {route === 'donate' ? <DonatePage /> : null}
          </section>
        </main>
      </div>
      <Toast message={toast} />

      {navConfirm ? (
        <div
          className="help-sheet"
          role="dialog"
          aria-modal="true"
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) setNavConfirm(null);
          }}
        >
          <div className="card help-card" style={{ maxWidth: 460 }}>
            <div className="help-head">
              <div>
                <h3>有未保存的修改</h3>
                <div className="sub">离开后这些改动会被丢弃（各页保存前只存在本页内存里）</div>
              </div>
            </div>
            <div className="help-body">
              <p style={{ margin: 0 }}>
                {dirtyPages.map((d) => d.label).join('、')} 有未保存的修改。
                要放弃修改并离开吗？
              </p>
            </div>
            <div className="help-foot">
              <span />
              <div style={{ display: 'flex', gap: 8 }}>
                <button type="button" className="btn ghost" onClick={() => setNavConfirm(null)}>
                  留在本页
                </button>
                <button
                  type="button"
                  className="btn primary"
                  onClick={() => {
                    const target = navConfirm;
                    setNavConfirm(null);
                    /* 真正丢弃：清脏表并重挂载页面（常驻挂载的 Overview/Skin/Sync 不会自动卸载）。 */
                    resetDirty();
                    setEpoch((v) => v + 1);
                    if (target) doNavigate(target.to, target.sub);
                  }}
                >
                  放弃修改并离开
                </button>
              </div>
            </div>
          </div>
        </div>
      ) : null}

      {/* 关窗确认（P0）：样式/文案沿用上面的导航确认弹层，避免壳内关窗丢改动。 */}
      {closeConfirm ? (
        <div
          className="help-sheet"
          role="dialog"
          aria-modal="true"
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) setCloseConfirm(false);
          }}
        >
          <div className="card help-card" style={{ maxWidth: 460 }}>
            <div className="help-head">
              <div>
                <h3>有未保存的修改</h3>
                <div className="sub">有未保存的修改，关闭后这些改动会丢弃（各页保存前只存在本页内存里）</div>
              </div>
            </div>
            <div className="help-body">
              <p style={{ margin: 0 }}>
                {dirtyPages.map((d) => d.label).join('、')} 有未保存的修改。要放弃修改并关闭窗口吗？
              </p>
            </div>
            <div className="help-foot">
              <span />
              <div style={{ display: 'flex', gap: 8 }}>
                <button type="button" className="btn ghost" onClick={() => setCloseConfirm(false)}>
                  留在本页
                </button>
                <button
                  type="button"
                  className="btn primary"
                  onClick={() => {
                    setCloseConfirm(false);
                    requestWindowClose();
                  }}
                >
                  放弃修改并关闭
                </button>
              </div>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
