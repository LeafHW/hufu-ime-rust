import { useCallback, useEffect, useRef, useState } from 'react';
import { loadBootstrap } from './api';
import { PAGE_TITLE, Sidebar, TitleBar } from './components/Shell';
import { Toast } from './components/ui';
import { AboutPage } from './pages/AboutPage';
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
import { getDirtyPages, subscribeDirty } from './dirtyGuard';
import './styles/app.css';

const ROUTES: Route[] = ['overview', 'schemes', 'input', 'keymap', 'skin', 'buddy', 'sentence', 'data', 'sync', 'about'];

function readRoute(): Route {
  const h = (location.hash || '').replace('#', '').replace(/^\//, '').split('/')[0];
  return (ROUTES as string[]).includes(h) ? (h as Route) : 'overview';
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

  const refresh = useCallback(async () => {
    const next = await loadBootstrap();
    setData(next);
    return next;
  }, []);

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const next = await loadBootstrap();
        if (alive) setData(next);
      } catch {
        /* 单轮失败保留上一轮数据，等下个轮询 */
      }
    };
    void load();
    const timer = window.setInterval(() => void load(), 10000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    const onHash = () => setRoute(readRoute());
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);

  useEffect(() => {
    document.body.dataset.screen = route === 'about' ? 'other' : route;
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

  /* 导航确认弹层：Esc 取消。 */
  useEffect(() => {
    if (!navConfirm) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        setNavConfirm(null);
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [navConfirm]);

  const notify = (msg: string) => {
    setToast(msg);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 2600);
  };

  return (
    <div className="window">
      <TitleBar title={PAGE_TITLE[route]} />
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
        <main className="main">
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
    </div>
  );
}
