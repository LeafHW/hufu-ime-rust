import { memo, useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { loadBootstrap } from './api';
import { PAGE_TITLE, requestWindowClose, Sidebar, TitleBar } from './components/Shell';
import { Toast } from './components/ui';
import { AboutPage } from './pages/AboutPage';
import { DonatePage } from './pages/DonatePage';
import { Overview } from './pages/Overview';
import { SkinStudio } from './pages/SkinStudio';
import { MotionPage } from './pages/Motion';
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

const ROUTES: Route[] = ['overview', 'schemes', 'input', 'keymap', 'skin', 'motion', 'buddy', 'sentence', 'data', 'sync', 'about', 'donate'];

/* 【2026-11-03】页面全部常驻挂载，切页只改 body[data-screen]。
   用 memo 包一层：切页/轮询引起的 App state 变化不再把 11 个页面整树重渲染，
   切页时主线程只剩「新页首次布局」，转场起点更快（memo 依赖 props 引用稳定：
   notify / navigate / refresh 已 useCallback，data 只在轮询返回时换新对象）。 */
const OverviewView = memo(Overview);
const SchemesView = memo(SchemesPage);
const InputView = memo(InputPage);
const KeymapView = memo(KeymapPage);
const SkinView = memo(SkinStudio);
const MotionView = memo(MotionPage);
const BuddyView = memo(BuddyPage);
const SentenceView = memo(SentencePage);
const DataView = memo(DataPage);
const SyncView = memo(Sync);
const AboutView = memo(AboutPage);
const DonateView = memo(DonatePage);

/* 【2026-11-03】页间转场时序：
   主路径 = View Transitions（::view-transition-old/new(hufu-page) 的 CSS 动画，
   时长写在 styles/app.css，和这里只是注释级对应）；退回路径 = Web Animations
   的「先退后进」。只动 opacity/transform，全程走合成器；
   系统「减少动画」时直接换页、不排队。 */
const PAGE_OUT_MS = 90;
const PAGE_IN_MS = 220;
const PAGE_EASE = 'cubic-bezier(.2,.7,.2,1)';
const PAGE_OUT_EASE = 'cubic-bezier(.4,0,.9,.4)';

type VtDoc = Document & {
  startViewTransition?: (cb: () => void) => { ready?: Promise<unknown>; finished?: Promise<unknown> };
};
const supportsViewTransition = () =>
  typeof (document as VtDoc).startViewTransition === 'function';

function readRoute(): Route {
  const h = (location.hash || '').replace('#', '').replace(/^\/?/, '').split('/')[0];
  return (ROUTES as string[]).includes(h) ? (h as Route) : 'overview';
}

const reducedMotion = () =>
  typeof window !== 'undefined' &&
  window.matchMedia?.('(prefers-reduced-motion: reduce)').matches === true;

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
  /** 当前页（侧栏高亮/标题跟随它）。页面全部常驻挂载，换页只切 display：
      不卸载 = 各页未保存的草稿不会丢，切换也没有重建成本。 */
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
  const [dirtyPages, setDirtyPages] = useState(() => getDirtyPages());
  /* 关窗确认弹层（P0）：脏数据存在时不直接发 win:close。 */
  const [closeConfirm, setCloseConfirm] = useState(false);

  const mainRef = useRef<HTMLElement | null>(null);
  /** 真正显示的页（DOM 的 body[data-screen] 跟随它），与 view 解耦是为了先退场再换页。 */
  const shownRef = useRef<Route>(readRoute());
  /** 连点导航时以最后一次点击为目标。 */
  const targetRef = useRef<Route>(readRoute());
  const outGuard = useRef(0);
  /** 当前在跑的那条转场动画（退场/入场共用一条，切页时先取消再接力）。 */
  const animRef = useRef<Animation | null>(null);
  /** idle=静止，out=旧页正在退场，in=新页正在入场。 */
  const phaseRef = useRef<'idle' | 'out' | 'in'>('idle');
  /** View Transition 进行中：此时再点导航只记目标，等这一轮结束接力。 */
  const vtBusy = useRef(false);
  /** Ctrl+K 从别的页回概览：等新页真正显示出来再聚焦搜索框（隐藏时 focus 无效）。 */
  const focusSearchPending = useRef(false);

  /* 卸载时掐掉可能在跑的转场动画（StrictMode 开发期会双挂载）。 */
  useEffect(
    () => () => {
      animRef.current?.cancel();
      animRef.current = null;
      window.clearTimeout(outGuard.current);
    },
    [],
  );

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

  /* 首帧直接把当前页显示出来（CSS 靠 body[data-screen] 决定哪一节 display:block）；
     全部页面常驻挂载，深链 #skin 不需要再滚动定位。 */
  useLayoutEffect(() => {
    const r = shownRef.current;
    document.body.dataset.screen = r;
    if (mainRef.current) mainRef.current.scrollTop = 0;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** 换页的 DOM 落点：切显示哪一节 + 回到页顶（切换后从页头开始，和侧栏选中一致）。 */
  const swapTo = useCallback((r: Route) => {
    shownRef.current = r;
    document.body.dataset.screen = r;
    const m = mainRef.current;
    if (m) m.scrollTop = 0;
    /* 新页这时才 display:block，聚焦/滚动到搜索框才有意义。 */
    if (focusSearchPending.current) {
      focusSearchPending.current = false;
      requestAnimationFrame(() =>
        window.dispatchEvent(new CustomEvent('hufu:focus-search')),
      );
    }
  }, []);

  /** 退回方案（内核不支持 View Transitions）：旧页先退场，换页后再入场。 */
  const commitRoute = useCallback((r: Route, animate = true) => {
    const m = mainRef.current;
    swapTo(r);
    /* 旧页马上 display:none：同一任务内取消退场动画，不会闪回 */
    animRef.current?.cancel();
    animRef.current = null;
    phaseRef.current = 'idle';
    window.clearTimeout(outGuard.current);
    m?.classList.remove('nav-out');
    if (!m) return;
    const el = document.getElementById(`s-${r}`);
    if (!animate || reducedMotion() || !el || typeof el.animate !== 'function') {
      m.classList.remove('nav-in');
      return;
    }
    const a = el.animate(
      [
        { opacity: 0, transform: 'translate3d(0,12px,0) scale(.998)' },
        { opacity: 1, transform: 'none' },
      ],
      { duration: PAGE_IN_MS, easing: PAGE_EASE },
    );
    animRef.current = a;
    phaseRef.current = 'in';
    m.classList.add('nav-in');
    a.onfinish = () => {
      if (animRef.current === a) {
        animRef.current = null;
        phaseRef.current = 'idle';
        m.classList.remove('nav-in');
      }
    };
  }, [swapTo]);

  /** View Transitions 主路径：旧页快照继续留在屏上，新页在快照后面完成布局与绘制，
      然后「快退快进」地淡出淡入（伪元素动画写在 styles/app.css）。
      转场期间来的点击只记目标，当前转场结束后接力，避免半路截断闪跳。 */
  const runVt = useCallback(
    (r: Route) => {
      const doc = document as VtDoc;
      const start = doc.startViewTransition;
      if (typeof start !== 'function') {
        commitRoute(r, true);
        return;
      }
      let vt: { ready?: Promise<unknown>; finished?: Promise<unknown> };
      try {
        vtBusy.current = true;
        vt = start.call(doc, () => swapTo(r));
      } catch {
        vtBusy.current = false;
        commitRoute(r, true);
        return;
      }
      vt.ready?.catch(() => {});
      Promise.resolve(vt.finished)
        .catch(() => {})
        .then(() => {
          vtBusy.current = false;
          if (focusSearchPending.current) {
            focusSearchPending.current = false;
            window.dispatchEvent(new CustomEvent('hufu:focus-search'));
          }
          const next = targetRef.current;
          if (next === shownRef.current) return;
          runVt(next);
        });
    },
    [commitRoute, swapTo],
  );

  /** 换页主入口：优先 View Transitions，不支持时退回 Web Animations「先退后进」。 */
  const go = useCallback(
    (r: Route) => {
      const m = mainRef.current;
      const prev = shownRef.current;
      if (r === targetRef.current) return;
      targetRef.current = r;
      setView(r); // 侧栏高亮 / 标题栏立刻响应点击
      if (!m || reducedMotion()) {
        commitRoute(r, false);
        return;
      }
      if (supportsViewTransition()) {
        if (vtBusy.current) return; // 这一轮结束后由 runVt 接力
        /* 掐掉退回路径可能还在跑的动画，避免两套转场叠加 */
        animRef.current?.cancel();
        animRef.current = null;
        window.clearTimeout(outGuard.current);
        m.classList.remove('nav-in', 'nav-out');
        phaseRef.current = 'idle';
        runVt(r);
        return;
      }

      const cur = document.getElementById(`s-${prev}`);
      if (!cur || typeof cur.animate !== 'function') {
        commitRoute(r, false);
        return;
      }
      /* 退场途中又点回当前页：撤掉退场，从当前透明度淡回。 */
      if (r === prev) {
        const cs = getComputedStyle(cur);
        animRef.current?.cancel();
        animRef.current = null;
        window.clearTimeout(outGuard.current);
        phaseRef.current = 'in';
        m.classList.remove('nav-out');
        m.classList.add('nav-in');
        const back = cur.animate(
          [{ opacity: cs.opacity, transform: cs.transform }, { opacity: 1, transform: 'none' }],
          { duration: 160, easing: PAGE_EASE },
        );
        animRef.current = back;
        back.onfinish = () => {
          if (animRef.current === back) {
            animRef.current = null;
            phaseRef.current = 'idle';
            m.classList.remove('nav-in');
          }
        };
        return;
      }
      if (phaseRef.current === 'out') return;
      /* 正常换页：先取当前透明度/变换再取消可能在跑的入场，保证从眼前这一刻接上。 */
      const cs = getComputedStyle(cur);
      animRef.current?.cancel();
      animRef.current = null;
      phaseRef.current = 'out';
      m.classList.remove('nav-in');
      m.classList.add('nav-out');
      const a = cur.animate(
        [
          { opacity: cs.opacity, transform: cs.transform },
          { opacity: 0, transform: 'translate3d(0,-6px,0) scale(.998)' },
        ],
        { duration: PAGE_OUT_MS, easing: PAGE_OUT_EASE, fill: 'forwards' },
      );
      animRef.current = a;
      const commit = () => {
        window.clearTimeout(outGuard.current);
        if (phaseRef.current !== 'out') return;
        commitRoute(targetRef.current, true);
      };
      a.onfinish = commit;
      /* 兜底：窗口隐藏时帧可能不推进，onfinish 不保证按时到。 */
      outGuard.current = window.setTimeout(commit, PAGE_OUT_MS + 200);
    },
    [commitRoute, runVt],
  );

  useEffect(() => {
    const onHash = () => go(readRoute());
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, [go]);

  useEffect(() => {
    document.title = `${PAGE_TITLE[view]} · 虎符设置`;
  }, [view]);

  useEffect(() => {
    const apply = () => {
      const light =
        theme === 'light' ||
        (theme === 'system' && window.matchMedia('(prefers-color-scheme: light)').matches);
      document.body.classList.toggle('light', light);
      /* 【2026-11-03 浅色黑边】把解析后的主题色同步给窗壳：
         窗口那圈 7px 非客户区边框由壳绘制，浅色主题必须跟着变浅，
         否则圆角处会露出系统深色窗框（黑边）。 */
      const webview = (
        window as unknown as {
          chrome?: { webview?: { postMessage?: (msg: string) => void } };
        }
      ).chrome?.webview;
      if (document.body.classList.contains('webview') && webview?.postMessage) {
        const cs = getComputedStyle(document.body);
        const bg = cs.getPropertyValue('--bg').trim();
        const side = cs.getPropertyValue('--bg2').trim();
        if (bg && side) {
          webview.postMessage(`win:theme:${bg}:${side}:${light ? 'light' : 'dark'}`);
        }
      }
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

  /* OV-B1：订阅全局脏状态 —— 关窗（beforeunload）统一拦截。 */
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
      setCloseConfirm(true);
      return;
    }
    requestWindowClose();
  };

  const navigate = useCallback(
    (r: Route, sub?: string): boolean => {
      const target = sub ? `${r}/${sub}` : r;
      try {
        history.replaceState(null, '', `#${target}`);
      } catch {
        /* file:// */
      }
      go(r);
      /* replaceState 不触发 hashchange：输入页/同步页靠它同步页签与引导层。 */
      window.dispatchEvent(new Event('hashchange'));
      return true;
    },
    [go],
  );

  /* Ctrl+K 全局聚焦搜索（原先只挂在概览页，其余页面无反应）。 */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        if (shownRef.current === 'overview') {
          window.dispatchEvent(new CustomEvent('hufu:focus-search'));
          return;
        }
        focusSearchPending.current = true;
        navigate('overview');
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [navigate]);

  /* 关窗确认弹层：Esc 取消。 */
  useEffect(() => {
    if (!closeConfirm) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        setCloseConfirm(false);
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [closeConfirm]);

  /* 引用稳定：下面 11 个 memo 页面组件靠它跳过无关重渲染。 */
  const notify = useCallback((msg: string) => {
    setToast(msg);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 2600);
  }, []);

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
        <main className="main" ref={mainRef}>
          <section className="screen" id="s-overview">
            <OverviewView data={data} notify={notify} navigate={navigate} />
          </section>
          <section className="screen" id="s-schemes">
            <SchemesView data={data} notify={notify} />
          </section>
          <section className="screen" id="s-input">
            <InputView data={data} notify={notify} />
          </section>
          <section className="screen" id="s-keymap">
            <KeymapView data={data} notify={notify} />
          </section>
          <section className="screen" id="s-skin">
            <SkinView data={data} notify={notify} refresh={refresh} navigate={navigate} />
          </section>
          <section className="screen" id="s-motion">
            <MotionView data={data} notify={notify} navigate={navigate} />
          </section>
          <section className="screen" id="s-buddy">
            <BuddyView data={data} notify={notify} />
          </section>
          <section className="screen" id="s-sentence">
            <SentenceView data={data} notify={notify} />
          </section>
          <section className="screen" id="s-data">
            <DataView data={data} notify={notify} />
          </section>
          <section className="screen" id="s-sync">
            <SyncView data={data} notify={notify} />
          </section>
          <section className="screen" id="s-about">
            <AboutView about={data.about} />
          </section>
          <section className="screen" id="s-donate">
            <DonateView />
          </section>
        </main>
      </div>
      <Toast message={toast} />

      {/* 关窗确认（P0）：样式/文案沿用导航确认弹层，避免壳内关窗丢改动。 */}
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
