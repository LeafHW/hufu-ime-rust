import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { Bootstrap, Route } from '../types';
import { MotionDemo } from '../components/MotionDemo';
import { skinTokens, withAlpha, type SkinTokens } from '../skinTokens';
import { useScreenActive } from '../hooks';
import { Badge, PageHead, Slider, Switch } from '../components/ui';
import '../styles/motion.css';

/** 系统「减少动画」探测器：与引擎同一个系统口径。 */
function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(
    typeof window !== 'undefined' && window.matchMedia?.('(prefers-reduced-motion: reduce)').matches === true,
  );
  useEffect(() => {
    const mq = window.matchMedia('(prefers-reduced-motion: reduce)');
    const on = () => setReduced(mq.matches);
    mq.addEventListener?.('change', on);
    on();
    return () => mq.removeEventListener?.('change', on);
  }, []);
  return reduced;
}

/** 演示里自动轮播选中项：每到下一项停 intervalMs。 */
function useCycle(count: number, intervalMs: number, playing: boolean) {
  const [sel, setSel] = useState(0);
  useEffect(() => {
    if (!playing) return;
    const t = window.setInterval(() => setSel((v) => (v + 1) % Math.max(1, count)), intervalMs);
    return () => window.clearInterval(t);
  }, [count, intervalMs, playing]);
  return [sel, setSel] as const;
}

/* ── 迷你候选窗（演示共同底子）───────────────────────────────────────── */

const DEMO_ROWS = [
  { t: '我们', cmt: 'wǒ men' },
  { t: '我', cmt: 'wǒ' },
  { t: '我军', cmt: 'wǒ jūn' },
  { t: 'War', cmt: '' },
];

function MiniCand({
  tokens,
  rows = DEMO_ROWS,
  sel = 0,
  scales,
  scaleMode = 'first',
  durMs = 0,
  className = '',
  onPick,
  style,
  flash,
}: {
  tokens: SkinTokens;
  rows?: { t: string; cmt: string }[];
  sel?: number;
  /** 逐候选字号比例：开=选中/首选放大、其余缩小。 */
  scales?: { first: number; other: number; enabled: boolean };
  scaleMode?: 'first' | 'follow';
  durMs?: number;
  className?: string;
  onPick?: (i: number) => void;
  style?: React.CSSProperties;
  flash?: boolean;
}) {
  const k = (i: number) => {
    if (!scales?.enabled) return 1;
    const big = scaleMode === 'follow' ? i === sel : i === 0;
    return big ? scales.first : scales.other;
  };
  return (
    <div
      className={`mo-cand ${className}`}
      style={{
        background: tokens.bg,
        border: `1px solid ${tokens.border}`,
        borderRadius: tokens.radius,
        fontFamily: tokens.face,
        ...style,
      }}
    >
      <div className="mo-pre" style={{ color: tokens.preedit, fontSize: Math.max(10, tokens.font - 4) }}>
        tuja
      </div>
      <div className="mo-rows" style={{ gap: `${tokens.gap}px` }}>
        {rows.map((r, i) => {
          const on = i === sel;
          return (
            <div
              key={r.t}
              className={`mo-row${on ? ' on' : ''}${on && flash ? ' flash' : ''}`}
              style={{
                background: on ? tokens.hlBg : 'transparent',
                color: on ? tokens.hlText : tokens.text,
                borderRadius: Math.max(4, tokens.radius - 2),
                fontSize: `${Math.round(tokens.font * k(i) * 100) / 100}px`,
                padding: `${tokens.rowPad}px ${Math.max(6, tokens.rowPad + 3)}px`,
                transition: durMs
                  ? `font-size ${durMs}ms cubic-bezier(.2,.75,.2,1), background ${Math.round(durMs * 0.7)}ms ease, color ${Math.round(durMs * 0.7)}ms ease`
                  : 'none',
                cursor: onPick ? 'pointer' : 'default',
              }}
              onMouseDown={onPick ? () => onPick(i) : undefined}
            >
              <span className="mo-idx" style={{ color: on ? tokens.hlText : tokens.label, fontSize: Math.max(9, tokens.font - 5) }}>
                {i + 1}.
              </span>
              <span className="mo-word">{r.t}</span>
              {r.cmt ? (
                <span className="mo-cmt" style={{ color: on ? withAlpha(tokens.hlText, 0.72) : tokens.comment }}>
                  {r.cmt}
                </span>
              ) : null}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/* ── 各动效演示 ───────────────────────────────────────────────────────── */

/** 高亮跟随放大：选中项变大、其余缩小，可点行手动切换。 */
function FollowDemo({
  tokens,
  scales,
  durMs,
  playing,
}: {
  tokens: SkinTokens;
  scales: { first: number; other: number; enabled: boolean };
  durMs: number;
  playing: boolean;
}) {
  const [sel, setSel] = useCycle(4, 1550, playing);
  return (
    <div className="mo-stage">
      <MiniCand
        tokens={tokens}
        sel={sel}
        scales={scales}
        scaleMode="follow"
        durMs={durMs}
        onPick={(i) => setSel(i)}
      />
    </div>
  );
}

/** 出现 / 收起：每轮重挂载窗口，重播「淡入上浮」；不留下长时间空舞台。 */
function RevealDemo({ tokens, durMs, active }: { tokens: SkinTokens; durMs: number; active: boolean }) {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    if (!active) return;
    const t = window.setInterval(() => setTick((v) => v + 1), 2100);
    return () => window.clearInterval(t);
  }, [active]);
  return (
    <div className="mo-stage">
      <div
        key={tick}
        className="mo-reveal"
        style={{ animationDuration: durMs ? `${durMs}ms` : '0ms' }}
      >
        <MiniCand tokens={tokens} />
      </div>
    </div>
  );
}

/** 选重暂留：选中那一刻高亮闪一下；开启时窗口多停留一会儿并有一圈余辉。 */
function LingerDemo({ tokens, durMs, linger, active }: { tokens: SkinTokens; durMs: number; linger: boolean; active: boolean }) {
  const [sel, setSel] = useState(0);
  const [flash, setFlash] = useState(false);
  const [holding, setHolding] = useState(false);
  useEffect(() => {
    if (!active) return;
    let timers: number[] = [];
    let alive = true;
    const cycle = () => {
      if (!alive) return;
      setSel(0);
      setFlash(false);
      setHolding(false);
      timers.push(
        window.setTimeout(() => {
          if (!alive) return;
          setSel(1);
          setFlash(true);
        }, 620),
      );
      timers.push(
        window.setTimeout(() => {
          if (!alive) return;
          setFlash(false);
          if (linger) setHolding(true);
        }, 980),
      );
      timers.push(
        window.setTimeout(() => {
          if (!alive) return;
          setHolding(false);
        }, 1680),
      );
      timers.push(window.setTimeout(cycle, 2300));
    };
    cycle();
    return () => {
      alive = false;
      timers.forEach((t) => window.clearTimeout(t));
      timers = [];
    };
  }, [linger, active]);
  return (
    <div className="mo-stage">
      <div className={`mo-linger${holding ? ' holding' : ''}`}>
        <MiniCand tokens={tokens} sel={sel} durMs={durMs} flash={flash} />
        {linger && holding ? <span className="mo-linger-tag">暂留</span> : null}
      </div>
    </div>
  );
}

/* ── 卡片外壳 ─────────────────────────────────────────────────────────── */

function MotionCard({
  eyebrow,
  title,
  desc,
  on,
  onToggle,
  disabled,
  note,
  foot,
  children,
}: {
  eyebrow: string;
  title: string;
  desc: string;
  on?: boolean;
  onToggle?: (v: boolean) => void;
  disabled?: boolean;
  note?: React.ReactNode;
  foot?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className={`card mo-card${on === false ? ' off' : ''}${disabled ? ' muted' : ''}`}>
      <div className="mo-card-head">
        <div className="mo-card-copy">
          <div className="mo-eyebrow">{eyebrow}</div>
          <h3>
            {title}
            {on === false ? <Badge tone="gray">未启用</Badge> : null}
          </h3>
          <p>{desc}</p>
        </div>
        {onToggle ? <Switch on={on === true} title={title} onChange={onToggle} /> : null}
      </div>
      {note ? <div className="mo-note-line">{note}</div> : null}
      {children}
      {foot ? <div className="mo-card-foot">{foot}</div> : null}
    </section>
  );
}

/* ── 页面 ─────────────────────────────────────────────────────────────── */

export function MotionPage({
  data,
  notify,
  navigate,
}: {
  data: Bootstrap;
  notify: (msg: string) => void;
  navigate: (r: Route) => void;
}) {
  const app = data.config.appearance || {};
  const reduced = useReducedMotion();
  const screenActive = useScreenActive('motion');
  const tokens = useMemo(() => skinTokens(data.skin), [data.skin]);

  const [anim, setAnim] = useState(app.anim !== false);
  const [flash, setFlash] = useState(app.anim_flash !== false);
  const [hl, setHl] = useState(app.hl_font_scale === true);
  const [speedPct, setSpeedPct] = useState<number | null>(null);
  const speed = Math.max(0.05, (speedPct ?? Math.round((app.anim_speed ?? 1) * 100)) / 100);

  /* config 轮询回来后同步（页面可能在别处被改过）。 */
  useEffect(() => setAnim(app.anim !== false), [app.anim]);
  useEffect(() => setFlash(app.anim_flash !== false), [app.anim_flash]);
  useEffect(() => setHl(app.hl_font_scale === true), [app.hl_font_scale]);

  /* 读-改-写 /api/config：单飞 + 合并排队，开关连点不互相覆盖。 */
  const writing = useRef(false);
  const queued = useRef<Record<string, unknown> | null>(null);
  const writeAppearance = useCallback(async (patch: Record<string, unknown>): Promise<void> => {
    if (writing.current) {
      queued.current = { ...(queued.current || {}), ...patch };
      return;
    }
    writing.current = true;
    let current: Record<string, unknown> | null = patch;
    try {
      while (current) {
        const r = await fetch('/api/config', { cache: 'no-store' });
        if (!r.ok) throw new Error(`读取配置失败（${r.status}）`);
        const fresh = (await r.json()) as Record<string, unknown>;
        const appearance = { ...(fresh.appearance as Record<string, unknown> | undefined), ...current };
        const p = await fetch('/api/config', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ ...fresh, appearance }),
        });
        const body = (await p.json().catch(() => ({}))) as { error?: string };
        if (!p.ok) throw new Error(body.error || `保存失败（${p.status}）`);
        current = queued.current;
        queued.current = null;
      }
    } finally {
      writing.current = false;
    }
  }, []);

  const speedDebounce = useRef<ReturnType<typeof setTimeout> | null>(null);
  const commitSpeed = (pct: number) => {
    setSpeedPct(pct);
    if (speedDebounce.current) clearTimeout(speedDebounce.current);
    speedDebounce.current = setTimeout(() => {
      void writeAppearance({ anim_speed: Number((pct / 100).toFixed(2)) }).catch((e) => {
        setSpeedPct(null);
        notify(`保存失败：${(e as Error).message}`);
      });
    }, 320);
  };

  /* 演示时长：总开关关掉/系统减动画 → 0（瞬时），否则按速度缩放。 */
  const animates = anim && !reduced;
  const dur = (base: number) => (animates ? Math.max(16, Math.round(base / speed)) : 0);
  /* 演示舞台始终按两档比例预览（开关关着也让人看到打开后的效果）；
     比例取用户当前配置，引擎侧 pcf || hl 任一开就用这两档。 */
  const scales = {
    enabled: true,
    first: Math.max(0.5, app.first_font_scale ?? 1),
    other: Math.max(0.5, Math.min(1, app.other_font_scale ?? 0.7)),
  };

  const toggleAnim = (v: boolean) => {
    setAnim(v);
    void writeAppearance({ anim: v }).catch((e) => {
      setAnim(!v);
      notify(`保存失败：${(e as Error).message}`);
    });
  };
  const toggleFlash = (v: boolean) => {
    setFlash(v);
    void writeAppearance({ anim_flash: v }).catch((e) => {
      setFlash(!v);
      notify(`保存失败：${(e as Error).message}`);
    });
  };
  const toggleHl = (v: boolean) => {
    setHl(v);
    void writeAppearance({ hl_font_scale: v }).catch((e) => {
      setHl(!v);
      notify(`保存失败：${(e as Error).message}`);
    });
  };

  const speedText = `${Math.round(speed * 100)}%`;
  const rhythm = [80, 180, 300].map((base) => ({ base, ms: animates ? Math.max(16, Math.round(base / speed)) : 0 }));

  return (
    <>
      <PageHead
        title="动效"
        sub={
          <>
            候选窗与设置页共用的节奏 · 改完立即生效
            {reduced ? <Badge tone="gray">系统已开启「减少动画」</Badge> : null}
          </>
        }
      />
      <div className="mo-wrap">
        <section className="card mo-hero">
          <div className="mo-hero-l">
            <div className="mo-eyebrow">MASTER</div>
            <h2>
              候选窗动效
              <Badge tone={anim ? 'gold' : 'gray'} dot={anim}>
                {anim ? '已开启' : '已关闭'}
              </Badge>
            </h2>
            <p>
              高亮滑动、出现/收起、选重暂留与下面的字号强调都归它管。关掉后候选窗只剩「瞬时切换」，
              打字、上屏、调频逻辑都不受影响。
            </p>
            <div className="mo-hero-switch">
              <Switch on={anim} title="候选窗动效总开关" onChange={toggleAnim} />
              <span>{anim ? '动效开启' : '动效关闭'}</span>
            </div>
          </div>
          <div className="mo-hero-r">
            <div className="mo-speed-head">
              <span>动效速度</span>
              <b>{speedText}</b>
            </div>
            <Slider
              value={speedPct ?? Math.round((app.anim_speed ?? 1) * 100)}
              min={40}
              max={200}
              step={5}
              width={190}
              label="动效速度"
              suffix="%"
              disabled={!anim}
              onChange={commitSpeed}
            />
            <div className="mo-speed-scale">
              <span>慢</span>
              <i />
              <span>快</span>
            </div>
            <div className="mo-rhythm" aria-hidden="true">
              {rhythm.map((r) => (
                <div key={r.base} className="mo-rhythm-row">
                  <span className="mo-rhythm-ms">{r.ms ? `${r.ms}ms` : '—'}</span>
                  <span className="mo-rhythm-track">
                    <i
                      className={animates && screenActive ? 'run' : ''}
                      style={{ animationDuration: `${Math.max(120, (r.base / speed) * 3.2)}ms` }}
                    />
                  </span>
                  <span className="mo-rhythm-tag">{r.base === 80 ? '快' : r.base === 180 ? '中' : '慢'}</span>
                </div>
              ))}
            </div>
          </div>
        </section>

        <div className="mo-grid">
          <MotionCard
            eyebrow="FOLLOW"
            title="高亮跟随放大"
            desc="选中项放大、其余候选缩小，强调跟着光标走。比例沿用皮肤页的「首选放大 / 其余缩小」；关掉则维持原来的「第 1 候选放大」。"
            on={hl}
            onToggle={toggleHl}
            note={
              <>
                矢量皮肤按比例换字号 · 位图皮肤（.ssf）跟随高亮行高 · 当前比例{' '}
                <b>
                  {Math.round(scales.first * 100)}% / {Math.round(scales.other * 100)}%
                </b>{' '}
                <button type="button" className="link-more" onClick={() => navigate('skin')}>
                  去皮肤页调整 →
                </button>
              </>
            }
            foot={
              <>
                <span className="mo-hint">点候选项可手动切换，观察字号与高亮一起落位。</span>
                <span className={`mo-live${animates ? ' on' : ''}`}>预览</span>
              </>
            }
          >
            <FollowDemo tokens={tokens} scales={scales} durMs={dur(220)} playing={animates && screenActive} />
          </MotionCard>

          <MotionCard
            eyebrow="GLIDE"
            title="高亮滑动"
            desc="选中项变化时，高亮底块从旧位置滑到新位置，而不是瞬间跳过去。速度受上面的总速度影响。"
            on={anim}
            onToggle={toggleAnim}
            foot={<span className="mo-hint">固定内部节奏 fast = 80ms，不随皮肤参数变化。</span>}
          >
            <div className="mo-stage mo-stage-slide">
              <MotionDemo tokens={tokens} speed={speed} enabled={animates} />
            </div>
          </MotionCard>

          <MotionCard
            eyebrow="LINGER"
            title="选重暂留"
            desc="选中候选后，候选窗多停留一小会儿再收起，让上屏结果看得见；关闭则选完立即收起。"
            on={flash}
            onToggle={toggleFlash}
            disabled={!anim}
            note={!anim ? '总开关关闭时此项不生效。' : undefined}
          >
            <LingerDemo tokens={tokens} durMs={dur(140)} linger={animates && flash} active={screenActive} />
          </MotionCard>

          <MotionCard
            eyebrow="REVEAL"
            title="出现 / 收起"
            desc="候选窗淡入上浮出现、淡出下沉收起；关闭总开关后直接出现、直接收起。"
            on={anim}
            onToggle={toggleAnim}
          >
            <RevealDemo tokens={tokens} durMs={dur(210)} active={screenActive} />
          </MotionCard>
        </div>

        <section className="card mo-tip">
          <div className="mo-tip-l">
            <div className="mo-eyebrow">NOTE</div>
            <h3>几个说明</h3>
          </div>
          <ul className="mo-tip-list">
            <li>
              系统「减少动画」开启时，候选窗与设置页的动效都会停下——这里只做说明，不需要再手动关。
            </li>
            <li>
              「高亮跟随放大」只改字号强调跟随谁，不额外加逐帧字号动画（历史上那种做法会卡）；
              高亮块本身仍走上面的滑动动效。
            </li>
            <li>所有开关都写回原来的配置键（anim / anim_flash / anim_speed / hl_font_scale），老配置不用重设。</li>
          </ul>
        </section>
      </div>
    </>
  );
}
