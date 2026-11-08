import { memo, useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { flushSync } from 'react-dom';
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
import { Trainer } from './pages/Trainer';
import { KeymapPage } from './pages/Keymap';
import { SentencePage } from './pages/Sentence';
import { BuddyPage } from './pages/Buddy';
import { fallbackBootstrap, type Bootstrap, type Route, type ThemeMode } from './types';
import { getDirtyPages, subscribeDirty } from './dirtyGuard';
import './styles/app.css';

const ROUTES: Route[] = ['overview', 'schemes', 'input', 'trainer', 'keymap', 'skin', 'motion', 'buddy', 'sentence', 'data', 'sync', 'about', 'donate'];

/* 【2026-11-03】页面全部常驻挂载，切页只改 body[data-screen]。
   用 memo 包一层：切页/轮询引起的 App state 变化不再把 13 个页面整树重渲染，
   切页时主线程只剩「新页首次布局」，转场起点更快（memo 依赖 props 引用稳定：
   notify / navigate / refresh 已 useCallback，data 只在轮询返回时换新对象）。 */
const OverviewView = memo(Overview);
const SchemesView = memo(SchemesPage);
const InputView = memo(InputPage);
const TrainerView = memo(Trainer);
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
  /** 当前页（侧栏高亮/标题跟随它）。页面首访挂载后一直保留（切 display）：
      已访问页不卸载 = 草稿不丢、切页没有重建成本；未访问页等首次点击/空闲预挂载。 */
  const [view, setView] = useState<Route>(readRoute);
  /** 【2026-11-03 启动提速】页面首访才挂载：首帧只渲染当前页（概览常驻，
      Ctrl+K 的搜索监听依赖它）；其余页在首帧后用 requestIdleCallback
      一页一页预挂载，既不让启动阻塞，又保证切页时已经就绪。 */
  const [mountedPages, setMountedPages] = useState<Set<Route>>(() => {
    const first = readRoute();
    const set = new Set<Route>(['overview']);
    set.add(first);
    return set;
  });
  const mountedPagesRef = useRef<Set<Route>>(mountedPages);
  const ensureMounted = useCallback((r: Route) => {
    if (mountedPagesRef.current.has(r)) return;
    const next = new Set(mountedPagesRef.current);
    next.add(r);
    mountedPagesRef.current = next;
    /* 新页要在 View Transition 拍「新快照」前完成布局：同步提交，避免
       快照抓到空页、转场结束后内容才蹦出来。 */
    flushSync(() => setMountedPages(next));
  }, []);
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
  /** 【2026-11-03】主题切换的 VT 序号：连点切换时只让最后一次负责收尾清理。 */
  const themeVtSeq = useRef(0);
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

  const lastSig = useRef('');
  const refresh = useCallback(async () => {
    const seq = ++loadSeq.current;
    inFlight.current = true;
    try {
      const next = normalizeBootstrap(await loadBootstrap());
      /* 只有最新一轮的响应能覆盖 state；旧轮次直接丢弃。
         【2026-11-03】数据没变就不 setData：每 10s 轮询过去会换新对象，
         把 12 个 memo 页面全部重渲染一遍（切页/动画期间偶发掉帧的根因）。
         序列化对比约 0.3ms，远小于整树重渲染。 */
      if (mounted.current && seq === loadSeq.current) {
        const sig = JSON.stringify(next);
        if (sig !== lastSig.current) {
          lastSig.current = sig;
          setData(next);
        }
      }
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

  /* 首帧后按空闲分批预挂载其余页：启动不被 13 页整树渲染拖慢，
     用户真正点过去时（通常 >1s）已经挂载好，切页仍然零成本。 */
  useEffect(() => {
    const order: Route[] = ['skin', 'motion', 'input', 'trainer', 'schemes', 'sync', 'data', 'keymap', 'buddy', 'sentence', 'about', 'donate'];
    let i = 0;
    let cancelled = false;
    const idle = (cb: () => void) => {
      const ric = (window as unknown as { requestIdleCallback?: (cb: () => void, o?: { timeout: number }) => number }).requestIdleCallback;
      if (typeof ric === 'function') ric(cb, { timeout: 700 });
      else window.setTimeout(cb, 120);
    };
    const step = () => {
      if (cancelled) return;
      /* 转场进行中不挂载新页：别在动画帧里插一段重渲染。 */
      if (vtBusy.current || phaseRef.current !== 'idle') {
        idle(step);
        return;
      }
      while (i < order.length && mountedPagesRef.current.has(order[i])) i++;
      if (i >= order.length) return;
      ensureMounted(order[i++]);
      idle(step);
    };
    idle(step);
    return () => {
      cancelled = true;
    };
  }, [ensureMounted]);

  /* 首帧直接把当前页显示出来（CSS 靠 body[data-screen] 决定哪一节 display:block）；
     深链页在 mounted 初始集合里，不需要再滚动定位。 */
  useLayoutEffect(() => {
    const r = shownRef.current;
    document.body.dataset.screen = r;
    if (mainRef.current) mainRef.current.scrollTop = 0;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** 换页的 DOM 落点：切显示哪一节 + 回到页顶（切换后从页头开始，和侧栏选中一致）。 */
  const swapTo = useCallback((r: Route) => {
    ensureMounted(r);
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
  }, [ensureMounted]);

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
      /* 【2026-11-03】html.light 也要跟着切：index.html 的首帧脚本只加不减，
         深色下残留 html.light 会让根背景/圆角缝仍是浅色。 */
      document.documentElement.classList.toggle('light', light);
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

  /* 【2026-11-03】主题切换动效：以侧栏 LOGO 为圆心，新主题从那里圆形扩散
     铺满整窗（LOGO 底正好是黑/白两色，深浅切换都顺）。
     页面切换的 VT 正在跑时直接瞬切，避免两套 View Transition 打架；
     系统「减少动画」时也不做。 */
  const onTheme = (m: ThemeMode) => {
    skipPersist.current = false;
    if (m === theme) return;
    const doc = document as VtDoc;
    if (
      reducedMotion() ||
      typeof doc.startViewTransition !== 'function' ||
      vtBusy.current
    ) {
      setTheme(m);
      return;
    }
    const root = document.documentElement;
    const logo = document.querySelector('.brand img') as HTMLElement | null;
    const rect = logo?.getBoundingClientRect();
    const x = Math.round(rect ? rect.left + rect.width / 2 : 68);
    const y = Math.round(rect ? rect.top + rect.height / 2 : 82);
    root.style.setProperty('--theme-x', `${x}px`);
    root.style.setProperty('--theme-y', `${y}px`);
    /* 圆要刚好盖到最远角：半径写死 160vmax 会在 40% 进度就铺满，
       看起来像瞬切；按实际距离算，整段动画的扩散才看得见。 */
    const far = Math.hypot(
      Math.max(x, window.innerWidth - x),
      Math.max(y, window.innerHeight - y),
    );
    root.style.setProperty('--theme-r', `${Math.ceil(far + 28)}px`);
    root.classList.add('theme-vt');
    const seq = ++themeVtSeq.current;
    try {
      const vt = doc.startViewTransition(() => {
        flushSync(() => setTheme(m));
      });
      Promise.resolve(vt.finished)
        .catch(() => {})
        .finally(() => {
          if (themeVtSeq.current === seq) root.classList.remove('theme-vt');
        });
    } catch {
      if (themeVtSeq.current === seq) root.classList.remove('theme-vt');
      setTheme(m);
    }
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

  /* P0-1（2026-11-04）：Alt+F4 / 任务栏「关闭窗口」/ 系统菜单关闭在壳侧先
     ExecuteScript 问 __hufuCanClose；为 false 时**不销毁窗口**，改发
     win:close-dirty，让页面走这里同一套未保存确认弹层。用户点「放弃修改
     并关闭」→ requestWindowClose() → 壳侧 win:close → 真正销毁。 */
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
    const onShellMessage = (e: MessageEvent) => {
      if (e.data === 'win:close-dirty') setCloseConfirm(true);
    };
    webview.addEventListener('message', onShellMessage);
    return () => webview.removeEventListener?.('message', onShellMessage);
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

  /* 引用稳定：下面 13 个 memo 页面组件靠它跳过无关重渲染。 */
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
            {mountedPages.has('overview') ? <OverviewView data={data} notify={notify} navigate={navigate} refresh={refresh} /> : null}
          </section>
          <section className="screen" id="s-schemes">
            {mountedPages.has('schemes') ? <SchemesView data={data} notify={notify} /> : null}
          </section>
          <section className="screen" id="s-input">
            {mountedPages.has('input') ? <InputView data={data} notify={notify} /> : null}
          </section>
          <section className="screen" id="s-trainer">
            {mountedPages.has('trainer') ? <TrainerView data={data} notify={notify} refresh={refresh} /> : null}
          </section>
          <section className="screen" id="s-keymap">
            {mountedPages.has('keymap') ? <KeymapView data={data} notify={notify} /> : null}
          </section>
          <section className="screen" id="s-skin">
            {mountedPages.has('skin') ? <SkinView data={data} notify={notify} refresh={refresh} navigate={navigate} /> : null}
          </section>
          <section className="screen" id="s-motion">
            {mountedPages.has('motion') ? <MotionView data={data} notify={notify} navigate={navigate} /> : null}
          </section>
          <section className="screen" id="s-buddy">
            {mountedPages.has('buddy') ? <BuddyView data={data} notify={notify} /> : null}
          </section>
          <section className="screen" id="s-sentence">
            {mountedPages.has('sentence') ? <SentenceView data={data} notify={notify} /> : null}
          </section>
          <section className="screen" id="s-data">
            {mountedPages.has('data') ? <DataView data={data} notify={notify} /> : null}
          </section>
          <section className="screen" id="s-sync">
            {mountedPages.has('sync') ? <SyncView data={data} notify={notify} /> : null}
          </section>
          <section className="screen" id="s-about">
            {mountedPages.has('about') ? <AboutView about={data.about} /> : null}
          </section>
          <section className="screen" id="s-donate">
            {mountedPages.has('donate') ? <DonateView /> : null}
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
