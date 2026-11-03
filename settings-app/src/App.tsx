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
/** 页面向下滚动的完整旅程顺序（与侧栏一致，最后到打赏）。 */
const JOURNEY: Route[] = ['overview', 'schemes', 'input', 'skin', 'buddy', 'sentence', 'keymap', 'data', 'sync', 'about', 'donate'];

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
  /** 侧栏/标题跟随滚动位置；页面常驻挂载，不再区分「当前路由」与「当前站点」。 */
  const [view, setView] = useState<Route>(readRoute);
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
  const mainRef = useRef<HTMLElement | null>(null);
  /** 主动导航/深链期间锁住滚动高亮，避免长距离平滑滚动时标题在中间页闪烁。 */
  const navLockUntil = useRef(0);
  const [scrollProgress, setScrollProgress] = useState(0);

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
    const onHash = () => {
      const r = readRoute();
      setView(r);
      requestAnimationFrame(() => scrollToRoute(r));
    };
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);

  useEffect(() => {
    document.body.dataset.screen = view === 'about' || view === 'donate' ? 'other' : view;
    document.title = `${PAGE_TITLE[view]} · 虎符设置`;
  }, [view]);

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

  /* 动效减负：窗口隐藏/最小化时暂停常驻 CSS 动画（同步页辉光/圈环等），
     避免后台常驻动画持续占用合成器。 */
  useEffect(() => {
    const apply = () =>
      document.documentElement.classList.toggle('anim-paused', document.hidden);
    apply();
    document.addEventListener('visibilitychange', apply);
    return () => document.removeEventListener('visibilitychange', apply);
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

  const scrollToRoute = (r: Route, smooth = true) => {
    const m = mainRef.current;
    const el = document.getElementById(`s-${r}`);
    if (!m || !el) return;
    const top = el.getBoundingClientRect().top - m.getBoundingClientRect().top + m.scrollTop;
    m.scrollTo({ top: Math.max(0, top - 8), behavior: smooth ? 'smooth' : 'auto' });
  };

  /* 滚动旅程：滚动位置决定侧栏/标题高亮（页面常驻，不写路由状态）。 */
  useEffect(() => {
    const m = mainRef.current;
    if (!m) return;
    let raf = 0;
    const onScroll = () => {
      if (raf) return;
      raf = requestAnimationFrame(() => {
        raf = 0;
        const probe = m.scrollTop + Math.min(180, m.clientHeight * 0.24);
        let cur: Route = JOURNEY[0];
        for (const r of JOURNEY) {
          const el = document.getElementById(`s-${r}`);
          if (!el) continue;
          const top = el.getBoundingClientRect().top - m.getBoundingClientRect().top + m.scrollTop;
          if (top <= probe) cur = r;
        }
        if (Date.now() >= navLockUntil.current) setView(cur);
        const max = Math.max(1, m.scrollHeight - m.clientHeight);
        setScrollProgress(Math.min(1, Math.max(0, m.scrollTop / max)));
      });
    };
    m.addEventListener('scroll', onScroll, { passive: true });
    onScroll();

    /* 元素入场：页面进入视口时给直属内容加 .in（CSS 统一做 stagger）。 */
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (e.isIntersecting) e.target.classList.add('in');
        }
      },
      { root: m, threshold: 0.06, rootMargin: '0px 0px -6% 0px' },
    );
    m.querySelectorAll('.screen').forEach((el) => io.observe(el));
    return () => {
      m.removeEventListener('scroll', onScroll);
      if (raf) cancelAnimationFrame(raf);
      io.disconnect();
    };
  }, [epoch]);

  const doNavigate = (r: Route, sub?: string) => {
    const target = sub ? `${r}/${sub}` : r;
    try {
      history.replaceState(null, '', `#${target}`);
    } catch {
      /* file:// */
    }
    navLockUntil.current = Date.now() + 1100;
    setView(r);
    requestAnimationFrame(() => scrollToRoute(r));
    window.dispatchEvent(new Event('hashchange'));
  };

  const navigate = (r: Route, sub?: string): boolean => {
    if (navConfirm) return false;
    // 【2026-11-03】旅程模式所有页面常驻挂载：站间导航只是滚动，不卸载
    // 页面、不丢任何未保存内容，因此不再弹「放弃修改并离开」。
    // 旧逻辑会让 Ctrl+K（内部 navigate('overview')）在任意脏页时直接弹
    // 丢弃确认，搜索完全不可用；脏数据守卫只保留在关窗路径。
    doNavigate(r, sub);
    return true;
  };

  /* 深链 #skin/#donate 首屏落到对应站点。页面全部挂载后图片/字体/折叠区
     会继续改变高度，单次 rAF 可能被后续布局挤回上一页，这里做几次短重试。 */
  useEffect(() => {
    const r = readRoute();
    if (r === 'overview') return;
    navLockUntil.current = Date.now() + 1700;
    let tries = 0;
    let cancelled = false;
    let timer = 0;
    const tick = () => {
      if (cancelled) return;
      scrollToRoute(r, false);
      tries += 1;
      if (tries < 4) timer = window.setTimeout(tick, 260);
    };
    timer = window.setTimeout(tick, 120);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /* Ctrl+K 全局聚焦搜索（原先只挂在概览页，其余页面无反应）。 */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        // 无论当前滚到哪一站，Ctrl+K 都回到概览并聚焦搜索框；页面常驻
        // 挂载，聚焦事件不需要等滚动动画完成。
        navigate('overview');
        window.dispatchEvent(new CustomEvent('hufu:focus-search'));
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [navConfirm]);

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
      <TitleBar title={PAGE_TITLE[view]} onRequestClose={onRequestClose} />
      <div className="shell">
        <Sidebar
          route={view}
          onNavigate={navigate}
          about={data.about}
          state={data.state}
          theme={theme}
          onTheme={onTheme}
          online={data.online}
        />
        <main className="main journey" key={epoch} ref={mainRef}>
          <div className="journey-progress" aria-hidden="true">
            <i style={{ transform: `scaleX(${scrollProgress})` }} />
          </div>
          <section className="screen" id="s-overview" data-next="方案">
            <div className="screen-inner"><Overview data={data} notify={notify} navigate={navigate} /></div>
          </section>
          <section className="screen" id="s-schemes" data-next="输入">
            <div className="screen-inner"><SchemesPage data={data} notify={notify} /></div>
          </section>
          <section className="screen" id="s-input" data-next="皮肤工作室">
            <div className="screen-inner"><InputPage data={data} notify={notify} /></div>
          </section>
          <section className="screen" id="s-skin" data-next="候选挂件">
            <div className="screen-inner"><SkinStudio data={data} notify={notify} refresh={refresh} /></div>
          </section>
          <section className="screen" id="s-buddy" data-next="整句模型">
            <div className="screen-inner"><BuddyPage data={data} notify={notify} /></div>
          </section>
          <section className="screen" id="s-sentence" data-next="键位编辑">
            <div className="screen-inner"><SentencePage data={data} notify={notify} /></div>
          </section>
          <section className="screen" id="s-keymap" data-next="数据与备份">
            <div className="screen-inner"><KeymapPage data={data} notify={notify} /></div>
          </section>
          <section className="screen" id="s-data" data-next="云同步">
            <div className="screen-inner"><DataPage data={data} notify={notify} /></div>
          </section>
          <section className="screen" id="s-sync" data-next="关于">
            <div className="screen-inner"><Sync data={data} notify={notify} /></div>
          </section>
          <section className="screen" id="s-about" data-next="打赏支持">
            <div className="screen-inner"><AboutPage about={data.about} /></div>
          </section>
          <section className="screen" id="s-donate">
            <div className="screen-inner"><DonatePage /></div>
          </section>
        </main>
        <nav className="journey-rail" aria-label="页面旅程">
          {JOURNEY.map((r, i) => (
            <button
              key={r}
              type="button"
              className={`jr-dot${view === r ? ' on' : ''}`}
              title={PAGE_TITLE[r]}
              aria-label={`${String(i + 1).padStart(2, '0')} ${PAGE_TITLE[r]}`}
              aria-current={view === r ? 'true' : undefined}
              onClick={() => navigate(r)}
            >
              <span>{String(i + 1).padStart(2, '0')}</span>
            </button>
          ))}
        </nav>

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
