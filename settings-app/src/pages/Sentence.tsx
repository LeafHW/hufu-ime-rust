import { useEffect, useMemo, useRef, useState } from 'react';
import type { Bootstrap, Config, EngineState } from '../types';
import { useDirtyGuard } from '../dirtyGuard';
import { Icon } from '../components/Icon';
import { Badge, PageHead, Switch } from '../components/ui';
import '../styles/sentence.css';

/* ── 整句配置（字段与 hufu-config::SentenceSection 对齐） ───────────── */

type RerankCfg = {
  enabled: boolean;
  top_k: number;
  debounce_ms: number;
  model_path?: string;
  endpoint?: string;
  timeout_ms?: number;
};

type Weights = {
  beam_width: number;
  candidate_limit: number;
  max_raw_length: number;
  rank_penalty: number;
  emitted_character_reward: number;
  isolation_lambda: number;
  isolation_threshold: number;
  confidence: number;
  dict_bias: number;
  supplement_baseline: number;
  supplement_scale: number;
  supplement_maximum: number;
  high_freq_limit: number;
  digit_codes?: boolean;
};

type SentenceCfg = {
  enabled: boolean;
  auto_enable: boolean;
  early_commit: boolean;
  empty_code_auto_commit: boolean;
  early_need: number;
  ngram_path: string;
  rerank: RerankCfg;
  weights: Weights;
  [key: string]: unknown;
};

const W_KEYS = [
  'beam_width',
  'candidate_limit',
  'max_raw_length',
  'rank_penalty',
  'emitted_character_reward',
  'isolation_lambda',
  'isolation_threshold',
  'confidence',
  'supplement_baseline',
  'supplement_scale',
  'supplement_maximum',
  'high_freq_limit',
] as const;
type WKey = (typeof W_KEYS)[number];

/** 与旧设置页 W_DEFAULTS 同源（出厂调校值）。 */
const DEFAULT_WEIGHTS: Weights = {
  beam_width: 200,
  candidate_limit: 20,
  max_raw_length: 128,
  rank_penalty: 0.03,
  emitted_character_reward: 2.0,
  isolation_lambda: 2.0,
  isolation_threshold: 3000,
  confidence: 0.99,
  dict_bias: 1.0,
  supplement_baseline: 9.0,
  supplement_scale: 2.0,
  supplement_maximum: 32.0,
  high_freq_limit: 0,
};

const DEFAULT_RERANK: RerankCfg = {
  enabled: true,
  top_k: 5,
  debounce_ms: 350,
};

const DEFAULT_SENTENCE: SentenceCfg = {
  enabled: true,
  auto_enable: true,
  early_commit: true,
  empty_code_auto_commit: true,
  early_need: 3,
  ngram_path: '模型/sentence-ngram.bin',
  rerank: { ...DEFAULT_RERANK },
  weights: { ...DEFAULT_WEIGHTS },
};

type WeightDef = {
  key: WKey;
  label: string;
  hint?: string;
  min: number;
  max: number;
  step: number;
  group: 'decode' | 'score' | 'corpus';
};

/** 旧页 12 条权重滑杆（min/max/step 与旧页一致）。 */
const WEIGHT_DEFS: WeightDef[] = [
  { key: 'beam_width', label: 'Beam 宽度', hint: '每键保留的搜索宽度', min: 10, max: 32000, step: 1, group: 'decode' },
  { key: 'candidate_limit', label: '候选上限', hint: '整句输出候选条数上限', min: 5, max: 50, step: 1, group: 'decode' },
  { key: 'max_raw_length', label: '缓冲码长上限', min: 16, max: 256, step: 4, group: 'decode' },
  { key: 'high_freq_limit', label: '整句高频字上限', hint: '0 = 不限制', min: 0, max: 4000, step: 1, group: 'decode' },
  { key: 'rank_penalty', label: '名次惩罚', hint: '× ln(码表名次)', min: 0, max: 1, step: 0.005, group: 'score' },
  { key: 'emitted_character_reward', label: '出字奖励', hint: '鼓励多出字', min: 0, max: 8, step: 0.05, group: 'score' },
  { key: 'isolation_lambda', label: '孤立生僻惩罚', min: 0, max: 20, step: 0.1, group: 'score' },
  { key: 'isolation_threshold', label: '生僻字频阈值', min: 100, max: 21000, step: 100, group: 'score' },
  { key: 'confidence', label: '提前上屏置信', min: 0.5, max: 1, step: 0.001, group: 'corpus' },
  { key: 'supplement_baseline', label: '语料奖励基准', min: 0, max: 20, step: 0.1, group: 'corpus' },
  { key: 'supplement_scale', label: '语料奖励缩放', min: 0, max: 8, step: 0.1, group: 'corpus' },
  { key: 'supplement_maximum', label: '语料奖励上限', min: 1, max: 40, step: 0.5, group: 'corpus' },
];

const WEIGHT_GROUPS: Array<{ id: WeightDef['group']; title: string; sub: string }> = [
  { id: 'decode', title: '解码规模', sub: '束宽 / 候选数 / 缓冲长度 / 高频字限制' },
  { id: 'score', title: '评分倾向', sub: '码表名次、出字奖励、孤立生僻惩罚' },
  { id: 'corpus', title: '置信与语料', sub: '提前上屏置信与补充语料奖励' },
];

type Preset = {
  id: 'stable' | 'balanced' | 'sensitive';
  name: string;
  desc: string;
  weights: Record<WKey, number>;
};

/**
 * 旧页没有落盘的三张预设表，这里按 W_DEFAULTS 与字段语义标定：
 * 稳健=置信更高、惩罚更重；均衡=出厂值；灵敏=束宽/候选更大、置信更低。
 * 三张表都只覆盖旧页的 12 个权重，dict_bias / digit_codes 原样保留。
 */
const PRESETS: Preset[] = [
  {
    id: 'stable',
    name: '稳健',
    desc: '置信更严、惩罚更重，长句更少跳字；出字略保守。',
    weights: {
      beam_width: 200,
      candidate_limit: 12,
      max_raw_length: 128,
      rank_penalty: 0.05,
      emitted_character_reward: 1.8,
      isolation_lambda: 3.0,
      isolation_threshold: 2500,
      confidence: 0.997,
      supplement_baseline: 8.0,
      supplement_scale: 1.6,
      supplement_maximum: 28,
      high_freq_limit: 0,
    },
  },
  {
    id: 'balanced',
    name: '均衡',
    desc: '出厂调校值（W_DEFAULTS），准确率与上屏率折中。',
    weights: {
      beam_width: 200,
      candidate_limit: 20,
      max_raw_length: 128,
      rank_penalty: 0.03,
      emitted_character_reward: 2.0,
      isolation_lambda: 2.0,
      isolation_threshold: 3000,
      confidence: 0.99,
      supplement_baseline: 9.0,
      supplement_scale: 2.0,
      supplement_maximum: 32,
      high_freq_limit: 0,
    },
  },
  {
    id: 'sensitive',
    name: '灵敏',
    desc: '束宽与候选更大、置信更松，出字更积极；偶发激进候选。',
    weights: {
      beam_width: 600,
      candidate_limit: 30,
      max_raw_length: 160,
      rank_penalty: 0.02,
      emitted_character_reward: 2.4,
      isolation_lambda: 1.2,
      isolation_threshold: 3400,
      confidence: 0.97,
      supplement_baseline: 10.0,
      supplement_scale: 2.4,
      supplement_maximum: 36,
      high_freq_limit: 0,
    },
  },
];

/* ── 工具 ─────────────────────────────────────────────────────────── */

const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));
const num = (v: unknown, fallback: number) =>
  typeof v === 'number' && Number.isFinite(v) ? v : fallback;

function decimalsOf(step: number): number {
  const s = String(step);
  return s.includes('.') ? s.split('.')[1].length : 0;
}

function fmt(n: number, digits = 3): string {
  return String(Number(n.toFixed(digits)));
}

function normalizeSentence(raw: unknown): SentenceCfg {
  const src = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>;
  const rr = (src.rerank && typeof src.rerank === 'object' ? src.rerank : {}) as Record<string, unknown>;
  const wr = (src.weights && typeof src.weights === 'object' ? src.weights : {}) as Record<string, unknown>;

  const weights: Weights = { ...DEFAULT_WEIGHTS };
  for (const key of W_KEYS) weights[key] = num(wr[key], DEFAULT_WEIGHTS[key]);
  weights.dict_bias = num(wr.dict_bias, DEFAULT_WEIGHTS.dict_bias);
  weights.digit_codes = wr.digit_codes === true;

  const rerank: RerankCfg = {
    enabled: rr.enabled !== false,
    top_k: Math.round(clamp(num(rr.top_k, 5), 2, 10)),
    debounce_ms: Math.round(clamp(num(rr.debounce_ms, 350), 50, 2000)),
    ...(typeof rr.model_path === 'string' ? { model_path: rr.model_path } : {}),
    ...(typeof rr.endpoint === 'string' ? { endpoint: rr.endpoint } : {}),
    ...(typeof rr.timeout_ms === 'number' ? { timeout_ms: rr.timeout_ms } : {}),
  };

  return {
    ...(src as Record<string, unknown>),
    enabled: src.enabled !== false,
    auto_enable: src.auto_enable !== false,
    early_commit: src.early_commit !== false,
    empty_code_auto_commit: src.empty_code_auto_commit !== false,
    early_need: Math.round(clamp(num(src.early_need, 3), 1, 8)),
    ngram_path:
      typeof src.ngram_path === 'string' && src.ngram_path
        ? src.ngram_path
        : DEFAULT_SENTENCE.ngram_path,
    rerank,
    weights,
  };
}

async function fetchConfig(): Promise<Config> {
  const r = await fetch('/api/config', { cache: 'no-store' });
  if (!r.ok) throw new Error(`读取配置失败（${r.status}）`);
  return (await r.json()) as Config;
}

async function fetchState(): Promise<EngineState> {
  const r = await fetch('/api/state', { cache: 'no-store' });
  if (!r.ok) throw new Error(`读取状态失败（${r.status}）`);
  return (await r.json()) as EngineState;
}

async function postConfig(cfg: Config): Promise<void> {
  const r = await fetch('/api/config', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(cfg),
  });
  let body: { error?: string } = {};
  try {
    body = (await r.json()) as { error?: string };
  } catch {
    /* 非 JSON 错误体 */
  }
  if (!r.ok) throw new Error(body.error || `保存失败（${r.status}）`);
}

const sleep = (ms: number) => new Promise<void>((resolve) => window.setTimeout(resolve, ms));

/* 【草稿先应用再组句 2026-11-02】POST /api/config 只把整句模型重载丢给后台
   线程（hufu-server/src/main.rs:1897-1900、848-1001），紧接着的
   /api/sentence_test（main.rs:2460-2475）拿到的仍是旧 decoder——试验结果对应
   用户修改之前的权重，会误导调参。服务端没有暴露 reload 完成标志（weight 类
   变更期间旧模型继续服务，/api/state 的 sentence_active 全程为 true），所以
   只能按下面三段兜底：
   ① 轮询 /api/config 直到落盘配置与本页草稿一致（确认 POST 真正生效）；
   ② 若 sentence_active 出现 false（切方案/拆旧模型）就等它回到 true；
   ③ 再留一个装载安全窗（后台 mmap 装载 + 补充语料自动机重建，热缓存下远
      小于该值），之后才允许调用 /api/sentence_test。
   返回 false 只在 ① 超时；此时调用方仍可测试，但会显式提示可能仍是旧权重。 */
const SENTENCE_RELOAD_WAIT_MS = 8000;
const SENTENCE_RELOAD_ACTIVE_WAIT_MS = 3000;
const SENTENCE_RELOAD_SETTLE_MS = 500;

async function waitSentenceReload(draft: SentenceCfg): Promise<boolean> {
  const sig = JSON.stringify(normalizeSentence(draft));
  const deadline = Date.now() + SENTENCE_RELOAD_WAIT_MS;
  let persisted = false;
  while (Date.now() < deadline) {
    try {
      const cfg = await fetchConfig();
      persisted = JSON.stringify(normalizeSentence(cfg.sentence)) === sig;
    } catch {
      /* 读不到配置：下一轮再试 */
    }
    if (persisted) break;
    await sleep(150);
  }
  if (!persisted) return false;
  /* 拆旧模型后 sentence_active 会短暂为 false：等重新激活再进安全窗，
     避免把「模型未就绪」当成「已生效」。 */
  const activeDeadline = Math.min(deadline, Date.now() + SENTENCE_RELOAD_ACTIVE_WAIT_MS);
  while (Date.now() < activeDeadline) {
    let active = true;
    try {
      active = Boolean((await fetchState()).sentence_active);
    } catch {
      /* 读不到状态：保持等待 */
    }
    if (active) break;
    await sleep(150);
  }
  await sleep(SENTENCE_RELOAD_SETTLE_MS);
  return true;
}

function joinModelPath(base: string, rel: string): string {
  return `${base.replace(/[\\/]+$/, '')}\\${rel.replace(/^[\\/]+/, '').replace(/\//g, '\\')}`;
}

function modelDirs(ngramPath: string, exeDir: string, dataDir: string): string[] {
  const raw = (ngramPath || '').trim().replace(/\\/g, '/');
  if (!raw) return [];
  if (/^[a-zA-Z]:\//.test(raw) || raw.startsWith('//')) {
    const idx = raw.lastIndexOf('/');
    return [idx > 0 ? raw.slice(0, idx).replace(/\//g, '\\') : raw.replace(/\//g, '\\')];
  }
  const idx = raw.lastIndexOf('/');
  const relDir = idx >= 0 ? raw.slice(0, idx) : '';
  const bases = [exeDir, dataDir].filter((x): x is string => Boolean(x));
  const out = bases.map((b) => (relDir ? joinModelPath(b, relDir) : b));
  return Array.from(new Set(out));
}

/* ── 精确滑杆：共享 Slider 的 toFixed(2) 会吃掉 0.005/0.001 步进 ──── */

function StepSlider({
  value,
  min,
  max,
  step,
  width = 190,
  onChange,
  label,
}: {
  value: number;
  min: number;
  max: number;
  step: number;
  width?: number;
  onChange: (v: number) => void;
  label: string;
}) {
  const decimals = Math.max(0, decimalsOf(step));
  const snap = (raw: number) => Number((Math.round(raw / step) * step).toFixed(decimals));
  const pct = clamp(((value - min) / (max - min)) * 100, 0, 100);

  const apply = (clientX: number, rect: DOMRect) => {
    const p = clamp((clientX - rect.left) / rect.width, 0, 1);
    onChange(snap(min + (max - min) * p));
  };

  return (
    <div
      className="slider sentence-range"
      style={{ width }}
      role="slider"
      tabIndex={0}
      aria-label={label}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuenow={value}
      onPointerDown={(e) => {
        const rect = e.currentTarget.getBoundingClientRect();
        try {
          e.currentTarget.setPointerCapture(e.pointerId);
        } catch {
          /* synthetic events */
        }
        apply(e.clientX, rect);
        const move = (ev: PointerEvent) => apply(ev.clientX, rect);
        const up = () => {
          window.removeEventListener('pointermove', move);
          window.removeEventListener('pointerup', up);
        };
        window.addEventListener('pointermove', move);
        window.addEventListener('pointerup', up, { once: true });
      }}
      onKeyDown={(e) => {
        if (e.key === 'ArrowLeft') onChange(snap(clamp(value - step, min, max)));
        if (e.key === 'ArrowRight') onChange(snap(clamp(value + step, min, max)));
      }}
    >
      <i style={{ width: `${pct}%` }} />
      <b style={{ left: `${pct}%` }} />
    </div>
  );
}

/* ── 页面 ─────────────────────────────────────────────────────────── */

export function SentencePage({
  data,
  notify,
}: {
  data: Bootstrap;
  notify: (msg: string) => void;
}) {
  const [sentence, setSentence] = useState<SentenceCfg>(() => normalizeSentence(data.config.sentence));
  const [remote, setRemote] = useState<EngineState>(data.state);
  const [dirty, setDirty] = useState(false);
  /* OV-B1：脏状态上报全局守卫。 */
  useDirtyGuard('sentence', '整句模型', dirty);
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testRaw, setTestRaw] = useState('tujatuja');
  const [testOut, setTestOut] = useState<string[] | null>(null);
  const [testError, setTestError] = useState('');
  const [lastTest, setLastTest] = useState<{ raw: string; n: number } | null>(null);
  const [booted, setBooted] = useState(false);
  /* 【模型真名 2026-11-01】server /api/sentence/models 真值：实际装载
     候选（config 命中或「模型\」探测，顺序=装载优先级）+ 重排实际
     picked gguf（含魔数校验）。页面显示用户真实文件名，不再显示
     config 固定默认字面量。 */
  const [models, setModels] = useState<{
    model_dir: string;
    ngram_candidates: string[];
    ngram_active: string | null;
    rerank_model: string | null;
  }>({ model_dir: '', ngram_candidates: [], ngram_active: null, rerank_model: null });
  useEffect(() => {
    let alive = true;
    fetch('/api/sentence/models')
      .then((r) => (r.ok ? r.json() : null))
      .then((d: typeof models | null) => {
        if (alive && d) setModels(d);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [sentence.ngram_path]);

  const sentenceRef = useRef(sentence);
  const dirtyRef = useRef(false);
  const savingRef = useRef(false);
  const serverCfgRef = useRef<Config | null>(null);
  const lastPolledRef = useRef(JSON.stringify(sentence));
  const configChainRef = useRef<Promise<unknown>>(Promise.resolve());
  const queueCountRef = useRef(0);
  const notifyRef = useRef(notify);
  notifyRef.current = notify;

  const setLocal = (next: SentenceCfg) => {
    sentenceRef.current = next;
    setSentence(next);
  };

  const markDirty = (v: boolean) => {
    dirtyRef.current = v;
    setDirty(v);
  };

  /* 所有整包 POST 串行：避免两次开关快速切换时后一次用旧配置覆盖前一次。 */
  function queueConfig<T>(task: () => Promise<T>): Promise<T> {
    queueCountRef.current += 1;
    savingRef.current = true;
    setSaving(true);
    const p = configChainRef.current.then(task, task);
    configChainRef.current = p.then(
      () => undefined,
      () => undefined,
    );
    return p.finally(() => {
      queueCountRef.current -= 1;
      if (queueCountRef.current === 0) {
        savingRef.current = false;
        setSaving(false);
      }
    });
  }

  /* 初次载入：直接拉服务端配置，不依赖 App 的 10s 轮询。 */
  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const cfg = await fetchConfig();
        if (!alive) return;
        serverCfgRef.current = cfg;
        const next = normalizeSentence(cfg.sentence);
        lastPolledRef.current = JSON.stringify(next);
        setLocal(next);
        setBooted(true);
      } catch {
        if (alive) setBooted(true);
      }
      try {
        const st = await fetchState();
        if (alive) setRemote(st);
      } catch {
        /* 离线预览：保留 data.state */
      }
    })();
    return () => {
      alive = false;
    };
  }, []);

  /* 状态轮询：模型加载/激活是后台异步的。 */
  useEffect(() => {
    const tick = async () => {
      try {
        setRemote(await fetchState());
      } catch {
        /* 保持上次状态 */
      }
    };
    const timer = window.setInterval(tick, 4000);
    return () => window.clearInterval(timer);
  }, []);

  /* 配置轮询：只在本页无未应用改动时跟随外部变化。 */
  useEffect(() => {
    const timer = window.setInterval(async () => {
      if (dirtyRef.current || savingRef.current) return;
      try {
        const cfg = await fetchConfig();
        const next = normalizeSentence(cfg.sentence);
        const sig = JSON.stringify(next);
        if (sig === lastPolledRef.current) {
          serverCfgRef.current = cfg;
          return;
        }
        serverCfgRef.current = cfg;
        lastPolledRef.current = sig;
        setLocal(next);
      } catch {
        /* 下轮再试 */
      }
    }, 8000);
    return () => window.clearInterval(timer);
  }, []);

  /* 全量应用当前草稿：先读服务端最新整包（保留别处改动），再整包 POST。 */
  const persistSentence = async (
    nextSentence: SentenceCfg,
    success: string,
    revert?: SentenceCfg,
  ): Promise<boolean> => {
    setLocal(nextSentence);
    markDirty(true);
    try {
      const next = await queueConfig(async () => {
        const fresh = await fetchConfig();
        const cfg: Config = { ...fresh, sentence: nextSentence };
        await postConfig(cfg);
        return cfg;
      });
      serverCfgRef.current = next;
      lastPolledRef.current = JSON.stringify(nextSentence);
      if (sentenceRef.current === nextSentence) markDirty(false);
      notifyRef.current(success);
      return true;
    } catch (e) {
      if (revert) setLocal(revert);
      notifyRef.current(`保存失败：${(e as Error).message}`);
      return false;
    }
  };

  const applyDraft = async (silent = false): Promise<boolean> => {
    const ok = await persistSentence(
      sentenceRef.current,
      silent ? '整句参数已保存' : '整句参数已保存并应用',
    );
    return ok;
  };

  const discardDraft = async () => {
    try {
      const cfg = await fetchConfig();
      serverCfgRef.current = cfg;
      const next = normalizeSentence(cfg.sentence);
      lastPolledRef.current = JSON.stringify(next);
      setLocal(next);
      markDirty(false);
      notifyRef.current('已放弃未应用的调整');
    } catch (e) {
      notifyRef.current(`读取配置失败：${(e as Error).message}`);
    }
  };

  /* 开关类即时保存：只改当前开关，不把未应用的高级权重偷偷带上去。 */
  const saveFlag = async (
    updater: (s: SentenceCfg) => SentenceCfg,
    message: string,
  ) => {
    const before = sentenceRef.current;
    const nextLocal = updater(before);
    setLocal(nextLocal);
    try {
      const { cfg, nextServer } = await queueConfig(async () => {
        const fresh = await fetchConfig();
        const serverSentence = updater(normalizeSentence(fresh.sentence));
        const next: Config = { ...fresh, sentence: serverSentence };
        await postConfig(next);
        return { cfg: next, nextServer: serverSentence };
      });
      serverCfgRef.current = cfg;
      lastPolledRef.current = JSON.stringify(nextServer);
      notifyRef.current(message);
    } catch (e) {
      setLocal(before);
      notifyRef.current(`保存失败：${(e as Error).message}`);
    }
  };

  const setWeight = (key: WKey, value: number) => {
    const next = {
      ...sentenceRef.current,
      weights: { ...sentenceRef.current.weights, [key]: value },
    };
    setLocal(next);
    markDirty(true);
  };

  const setRerank = (patch: Partial<RerankCfg>) => {
    const next = {
      ...sentenceRef.current,
      rerank: { ...sentenceRef.current.rerank, ...patch },
    };
    setLocal(next);
    markDirty(true);
  };

  const applyPreset = async (preset: Preset) => {
    if (
      dirtyRef.current &&
      !window.confirm('当前有未应用的调整。应用预设会把它们一并保存，继续？')
    ) {
      return;
    }
    const nextSentence: SentenceCfg = {
      ...sentenceRef.current,
      weights: { ...sentenceRef.current.weights, ...preset.weights },
    };
    await persistSentence(nextSentence, `已应用「${preset.name}」预设并写入 12 项权重`);
  };

  const restoreDefaults = async () => {
    if (
      !window.confirm(
        '恢复出厂整句参数？将重置 12 项权重、提前上屏键数、断供兜底与神经重排参数，并立即保存。',
      )
    ) {
      return;
    }
    const before = sentenceRef.current;
    const next: SentenceCfg = {
      ...before,
      early_need: 3,
      empty_code_auto_commit: true,
      rerank: { ...before.rerank, enabled: true, top_k: 5, debounce_ms: 350 },
      weights: {
        ...DEFAULT_WEIGHTS,
        digit_codes: before.weights.digit_codes,
      },
    };
    await persistSentence(next, '整句参数已恢复初始值并保存生效', before);
  };

  const applyGroupDefaults = (group: WeightDef['group']) => {
    const nextWeights = { ...sentenceRef.current.weights };
    for (const def of WEIGHT_DEFS) {
      if (def.group === group) nextWeights[def.key] = DEFAULT_WEIGHTS[def.key];
    }
    setLocal({ ...sentenceRef.current, weights: nextWeights });
    markDirty(true);
  };

  const runTest = async () => {
    const raw = testRaw.trim();
    if (!raw) {
      notifyRef.current('请输入编码后再试验');
      return;
    }
    setTesting(true);
    setTestError('');
    setTestOut(null);
    try {
      if (dirtyRef.current) {
        /* 有草稿：先「保存并应用」，再等后台模型重载落地才测试——否则
           /api/sentence_test 用的是旧 decoder，结果对应旧权重、误导调参。 */
        const draft = sentenceRef.current;
        const applied = await applyDraft(true);
        if (!applied) return;
        const settled = await waitSentenceReload(draft);
        if (!settled) {
          notifyRef.current('参数已保存，但模型重载未在时限内确认；本次试验可能仍用旧权重');
        }
      }
      const r = await fetch('/api/sentence_test', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ raw }),
      });
      let body: { candidates?: unknown; error?: string } = {};
      try {
        body = (await r.json()) as { candidates?: unknown; error?: string };
      } catch {
        /* 非 JSON */
      }
      if (!r.ok) throw new Error(body.error || `试验失败（${r.status}）`);
      const list = Array.isArray(body.candidates)
        ? body.candidates.filter((x): x is string => typeof x === 'string')
        : [];
      setTestOut(list);
      setLastTest({ raw, n: list.length });
      notifyRef.current(list.length ? `试验完成：${list.length} 个候选` : '试验完成：无候选');
    } catch (e) {
      setTestError((e as Error).message);
      notifyRef.current(`试验失败：${(e as Error).message}`);
    } finally {
      setTesting(false);
    }
  };

  const copyModelPath = async () => {
    const p = modelPaths[0] || sentence.ngram_path;
    try {
      await navigator.clipboard.writeText(p);
      notifyRef.current(`已复制模型目录：${p}`);
    } catch {
      notifyRef.current(`模型目录：${p}`);
    }
  };

  /* 打开模型目录走 /api/open_dir {kind:"model"}（server 受控通道）；
     旧的 open_schema_dir 传绝对路径用法已被路径穿越封堵（400）。 */
  const openModelDir = async () => {
    try {
      const r = await fetch('/api/open_dir', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ kind: 'model' }),
      });
      let body: { path?: string } = {};
      try {
        body = (await r.json()) as { path?: string };
      } catch {
        /* 非 JSON */
      }
      if (r.ok) {
        notifyRef.current(`已打开模型目录：${body.path || ''}`);
        return;
      }
    } catch {
      /* 走剪贴板兜底 */
    }
    try {
      await navigator.clipboard.writeText(modelPaths[0] || sentence.ngram_path);
      notifyRef.current(`无法直接打开，已复制模型目录：${modelPaths[0] || sentence.ngram_path}`);
    } catch {
      notifyRef.current(`无法直接打开模型目录：${modelPaths[0] || sentence.ngram_path}`);
    }
  };

  const refreshState = async () => {
    try {
      const st = await fetchState();
      setRemote(st);
      notifyRef.current(
        st.sentence_active
          ? '整句引擎已加载'
          : st.model_present
            ? '模型存在，当前未激活'
            : '未检测到整句模型文件',
      );
    } catch (e) {
      notifyRef.current(`刷新状态失败：${(e as Error).message}`);
    }
  };

  const schema = remote.current_schema || data.state.current_schema || '—';
  const schemaIsSentence = schema.includes('整句');
  const installed = Boolean(remote.model_present);
  const active = Boolean(remote.sentence_active);
  const enabled = sentence.enabled;
  const loading = enabled && installed && schemaIsSentence && !active;
  const statusText = active
    ? '整句引擎运行中'
    : !installed
      ? '整机未安装整句模型'
      : !enabled
        ? '整句已停用'
        : !schemaIsSentence
          ? '等待切换到整句方案'
          : '模型存在 · 加载中/未激活';
  const statusTone = active ? 'green' : installed ? 'gold' : 'gray';
  const modelPaths = useMemo(
    () => modelDirs(sentence.ngram_path, data.about.exe_dir, data.about.data_dir),
    [sentence.ngram_path, data.about.exe_dir, data.about.data_dir],
  );
  const activePreset = useMemo(
    () =>
      PRESETS.find((p) =>
        W_KEYS.every((k) => Math.abs(sentence.weights[k] - p.weights[k]) <= Math.max(1e-6, Math.abs(p.weights[k]) * 1e-6)),
      )?.id,
    [sentence.weights],
  );

  const weightValue = (key: WKey) => sentence.weights[key];

  return (
    <>
      <PageHead
        title="整句模型"
        sub={
          <>
            <span className={active ? 'dot' : installed ? 'dot gold' : 'dot muted'} />
            {statusText}
            <span className="sentence-sep">·</span>
            当前方案 {schema}
            <span className="sentence-sep">·</span>
            12 项组句权重
          </>
        }
        actions={
          <>
            <button type="button" className="btn ghost sm" onClick={refreshState} title="重新读取 /api/state">
              <Icon name="refresh" />
              刷新状态
            </button>
            <button
              type="button"
              className="btn ghost sm"
              onClick={restoreDefaults}
              disabled={saving}
              title="恢复 W_DEFAULTS 与出厂行为参数"
            >
              <Icon name="refresh" />
              恢复默认
            </button>
          </>
        }
      />

      <div className="sentence-page">
        <section className="card sentence-hero">
          <div className="sentence-hero-main">
            <div className="sentence-hero-top">
              <span className={`sentence-orb${active ? ' on' : loading ? ' loading' : ''}`} />
              <div>
                <div className="sentence-hero-title">
                  {statusText}
                  <Badge tone={statusTone}>
                    {active ? '已加载' : installed ? (enabled ? '待激活' : '已停用') : '未安装'}
                  </Badge>
                </div>
                <div className="sentence-hero-sub">
                  模型文件 {installed ? '已找到' : '未找到'} · 总开关 {enabled ? '开' : '关'} ·
                  自动启用 {sentence.auto_enable ? '开' : '关'} · 提前上屏{' '}
                  {sentence.early_commit ? `稳 ${sentence.early_need} 键` : '关'}
                </div>
              </div>
            </div>

            <div className="sentence-path">
              <span className="sentence-path-label">模型路径</span>
              <code title={models.ngram_active || sentence.ngram_path}>
                {models.ngram_active
                  ? models.ngram_active.split(/[\\/]/).pop()
                  : sentence.ngram_path || '（未配置）'}
              </code>
              {models.ngram_active && models.ngram_candidates.length > 1 ? (
                <span className="sentence-path-note">
                  （多个候选按新→旧试读，当前首选；另有{' '}
                  {models.ngram_candidates.length - 1} 个）
                </span>
              ) : null}
              <button type="button" className="btn outline sm" onClick={openModelDir}>
                <Icon name="folder" />
                打开目录
              </button>
              <button type="button" className="btn ghost sm" onClick={copyModelPath} title="复制解析后的模型目录">
                复制
              </button>
            </div>
            {models.ngram_candidates.length > 0 ? (
              <div className="sentence-resolved">
                实际装载候选（新→旧）：
                {models.ngram_candidates.map((p, i) => (
                  <code key={p} className={i === 0 ? 'is-active' : undefined}>
                    {i === 0 ? '✓ ' : ''}
                    {p}
                  </code>
                ))}
                <span>（文件名不限：放入「模型」目录的 .bin 自动按新→旧试读装载）</span>
              </div>
            ) : (
              <div className="sentence-resolved">
                模型目录未找到 .bin 文件——把整句模型（任意文件名的 .bin）放入模型目录即自动装载。
              </div>
            )}

            {!installed ? (
              <div className="sentence-guide">
                <div className="sentence-guide-title">
                  <Icon name="info" />
                  <b>当前安装未携带整句模型（轻量安装）</b>
                </div>
                <ol>
                  <li>
                    从{' '}
                    <a
                      href="https://github.com/LeafHW/hufu-ime-rust/releases/tag/%E6%A8%A1%E5%9E%8B"
                      target="_blank"
                      rel="noreferrer"
                    >
                      GitHub 模型发布页
                    </a>{' '}
                    下载 模型.7z；
                  </li>
                  <li>解压后把「模型」文件夹整个复制到安装目录（与程序同级）；</li>
                  <li>切换到「虎整句」方案，模型随切换自动加载，无需重启。</li>
                </ol>
              </div>
            ) : !active && !schemaIsSentence ? (
              <div className="sentence-hint">
                <Icon name="info" />
                模型已就位，但当前方案「{schema}」不是整句方案。切到「虎整句」后自动加载；也可打开上面的「整句」总开关。
              </div>
            ) : loading ? (
              <div className="sentence-hint">
                <Icon name="refresh" />
                模型存在，整句引擎正在后台装载（大模型首次加载需要数秒）；装载完成后本页状态会自动刷新。
              </div>
            ) : null}
          </div>

          <div className="sentence-hero-side">
            <div className="sentence-stat">
              <span>模型文件</span>
              <b className={installed ? 'ok' : 'muted'}>{installed ? '已找到' : '未找到'}</b>
            </div>
            <div className="sentence-stat">
              <span>整句引擎</span>
              <b className={active ? 'ok' : 'muted'}>{active ? '运行中' : loading ? '加载中' : '未运行'}</b>
            </div>
            <div className="sentence-stat">
              <span>当前方案</span>
              <b>{schema}</b>
            </div>
            <div className="sentence-stat">
              <span>参数状态</span>
              <b className={dirty ? 'warn' : 'ok'}>{dirty ? '有未应用调整' : saving ? '保存中' : '已同步'}</b>
            </div>
          </div>
        </section>

        <div className="sentence-cols">
          <div className="sentence-col">
            <section className="card sentence-card">
              <div className="card-head">
                <div>
                  <div className="card-title">整句开关</div>
                  <div className="sentence-card-sub">开关类改动即时写入 /api/config；权重与重排数字走下方「应用」。</div>
                </div>
                <Badge tone={saving ? 'gold' : 'gray'}>{saving ? '保存中' : dirty ? '有草稿' : '已同步'}</Badge>
              </div>
              <div className="sentence-toggle-grid">
                <div className="sentence-toggle">
                  <Switch
                    on={sentence.enabled}
                    onChange={(v) =>
                      void saveFlag(
                        (s) => ({ ...s, enabled: v }),
                        v ? '已启用整句（切到整句方案后加载）' : '已停用整句',
                      )
                    }
                    title="整句输入总开关"
                  />
                  <div>
                    <b>启用整句</b>
                    <span>总开关；关闭后即使模型存在也不会组句。</span>
                  </div>
                </div>
                <div className="sentence-toggle">
                  <Switch
                    on={sentence.auto_enable}
                    onChange={(v) =>
                      void saveFlag((s) => ({ ...s, auto_enable: v }), v ? '已开启方案自动启用' : '已关闭方案自动启用')
                    }
                    title="方案名含「整句」时自动启用"
                  />
                  <div>
                    <b>「整句」方案自动启用</b>
                    <span>切到虎整句时自动加载模型。</span>
                  </div>
                </div>
                <div className="sentence-toggle">
                  <Switch
                    on={sentence.early_commit}
                    onChange={(v) =>
                      void saveFlag((s) => ({ ...s, early_commit: v }), v ? '已开启提前上屏' : '已关闭提前上屏')
                    }
                    title="稳定 N 键后提前上屏"
                  />
                  <div>
                    <b>提前上屏</b>
                    <span>连续稳定 {sentence.early_need} 键后落屏，越大越稳、上屏越迟。</span>
                  </div>
                </div>
                <div className="sentence-toggle">
                  <Switch
                    on={sentence.empty_code_auto_commit}
                    onChange={(v) =>
                      void saveFlag(
                        (s) => ({ ...s, empty_code_auto_commit: v }),
                        v ? '已开启断供兜底上屏' : '已关闭断供兜底上屏',
                      )
                    }
                    title="提前上屏锁定错误前缀导致候选断供时，空格重开一段取首选"
                  />
                  <div>
                    <b>断供兜底上屏</b>
                    <span>候选断供时用剩余编码重开一段，防丢字。</span>
                  </div>
                </div>
              </div>
              <div className="sentence-inline-field">
                <label htmlFor="sentence-early-need">提前上屏稳定键数（观察窗）</label>
                <StepSlider
                  value={sentence.early_need}
                  min={1}
                  max={8}
                  step={1}
                  width={150}
                  label="提前上屏稳定键数"
                  onChange={(v) => {
                    setLocal({ ...sentenceRef.current, early_need: v });
                    markDirty(true);
                  }}
                />
                <b>{sentence.early_need} 键</b>
              </div>
            </section>

            <section className="card sentence-card">
              <div className="card-head">
                <div>
                  <div className="card-title">预设</div>
                  <div className="sentence-card-sub">点一张卡立即写入旧页 12 项权重；只覆盖权重，不动开关与模型路径。</div>
                </div>
                {activePreset ? <Badge tone="green">当前匹配</Badge> : null}
              </div>
              <div className="sentence-presets">
                {PRESETS.map((p) => (
                  <button
                    key={p.id}
                    type="button"
                    className={`sentence-preset${activePreset === p.id ? ' on' : ''}`}
                    onClick={() => void applyPreset(p)}
                    disabled={saving}
                  >
                    <div className="sentence-preset-head">
                      <b>{p.name}</b>
                      {activePreset === p.id ? <Badge tone="green">当前</Badge> : null}
                    </div>
                    <p>{p.desc}</p>
                    <span className="sentence-preset-foot">
                      写入 12 项权重
                      <Icon name="arrow" />
                    </span>
                  </button>
                ))}
              </div>
            </section>

            <section className="card sentence-card">
              <div className="card-head">
                <div>
                  <div className="card-title">高级权重</div>
                  <div className="sentence-card-sub">拖动只改本地草稿；点底部「应用并保存」才 POST /api/config。</div>
                </div>
                {dirty ? <Badge tone="gold">未应用</Badge> : <Badge tone="green">已同步</Badge>}
              </div>
              <div className="sentence-weight-groups">
                {WEIGHT_GROUPS.map((group) => (
                  <div className="sentence-weight-group" key={group.id}>
                    <div className="sentence-group-head">
                      <div>
                        <b>{group.title}</b>
                        <span>{group.sub}</span>
                      </div>
                      <button
                        type="button"
                        className="sentence-mini"
                        onClick={() => applyGroupDefaults(group.id)}
                        title="恢复本组出厂值（先进入草稿，再点应用）"
                      >
                        恢复本组
                      </button>
                    </div>
                    {WEIGHT_DEFS.filter((d) => d.group === group.id).map((def) => (
                      <div className="sentence-weight-row" key={def.key}>
                        <div className="sentence-weight-label">
                          <span>
                            {def.label}
                            {def.hint ? <em>{def.hint}</em> : null}
                          </span>
                          <b>{fmt(weightValue(def.key))}</b>
                        </div>
                        <StepSlider
                          value={weightValue(def.key)}
                          min={def.min}
                          max={def.max}
                          step={def.step}
                          label={def.label}
                          onChange={(v) => setWeight(def.key, v)}
                        />
                      </div>
                    ))}
                  </div>
                ))}
              </div>
              <div className="sentence-weight-note">
                出厂值来自旧设置页 W_DEFAULTS（束宽 200 / 候选 20 / 置信 0.99）；
                dict_bias 与 digit_codes 由引擎维护，本页不暴露。
              </div>
            </section>
          </div>

          <div className="sentence-col">
            <section className="card sentence-card">
              <div className="card-head">
                <div>
                  <div className="card-title">神经重排</div>
                  <div className="sentence-card-sub">打字停顿后用 Qwen 小模型重排；gguf 缺失时自动跳过。</div>
                </div>
                <Switch
                  on={sentence.rerank.enabled}
                  onChange={(v) =>
                    void saveFlag(
                      (s) => ({ ...s, rerank: { ...s.rerank, enabled: v } }),
                      v ? '已开启神经重排' : '已关闭神经重排',
                    )
                  }
                  title="停顿重排"
                />
              </div>
              <div className="sentence-rerank">
                <div className="sentence-rerank-row">
                  <div className="sentence-rerank-label">
                    <span>重排候选数 top_k</span>
                    <b>{sentence.rerank.top_k}</b>
                  </div>
                  <StepSlider
                    value={sentence.rerank.top_k}
                    min={2}
                    max={10}
                    step={1}
                    width={160}
                    label="重排候选数 top_k"
                    onChange={(v) => setRerank({ top_k: v })}
                  />
                </div>
                <div className="sentence-rerank-row">
                  <div className="sentence-rerank-label">
                    <span>停顿判定</span>
                    <b>{sentence.rerank.debounce_ms} ms</b>
                  </div>
                  <StepSlider
                    value={sentence.rerank.debounce_ms}
                    min={50}
                    max={2000}
                    step={50}
                    width={160}
                    label="停顿判定毫秒"
                    onChange={(v) => setRerank({ debounce_ms: v })}
                  />
                </div>
                <div className="sentence-rerank-path">
                  <span>重排模型</span>
                  <code title={models.rerank_model || undefined}>
                    {models.rerank_model
                      ? models.rerank_model.split(/[\\/]/).pop()
                      : '未找到 .gguf（放入「模型」目录任意名 .gguf 即自动装载）'}
                  </code>
                </div>
              </div>
            </section>

            <section className="card sentence-card sentence-test">
              <div className="card-head">
                <div>
                  <div className="card-title">试验区</div>
                  <div className="sentence-card-sub">有未应用草稿时先保存并等模型重载完成，再调用真实 POST /api/sentence_test。</div>
                </div>
                <Badge tone={testing ? 'gold' : 'gray'}>{testing ? '组句中' : '真实接口'}</Badge>
              </div>
              <div className="sentence-test-input">
                <input
                  value={testRaw}
                  onChange={(e) => setTestRaw(e.target.value)}
                  placeholder="例如 tujatuja，可带尾缀 ; ' 数字"
                  aria-label="整句试验编码"
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') void runTest();
                  }}
                />
                <button type="button" className="btn primary" onClick={() => void runTest()} disabled={testing || saving}>
                  <Icon name="play" />
                  组句
                </button>
              </div>
              <div className="sentence-test-meta">
                <span>接口只返回候选文本顺序，不返回分数/来源；候选按后端 decode 顺序展示。</span>
              </div>
              {testError ? (
                <div className="sentence-test-error">
                  <Icon name="info" />
                  {testError}
                </div>
              ) : null}
              {testOut ? (
                testOut.length ? (
                  <ol className="sentence-candidates">
                    {testOut.slice(0, 12).map((t, i) => (
                      <li key={`${i}-${t}`} className={i === 0 ? 'first' : ''}>
                        <span>{i + 1}</span>
                        <b>{t}</b>
                      </li>
                    ))}
                  </ol>
                ) : (
                  <div className="sentence-empty">
                    无候选。确认模型已加载、当前方案含「整句」，且编码长度足够。
                  </div>
                )
              ) : (
                <div className="sentence-empty">输入编码后点「组句」，这里会显示真实候选。</div>
              )}
              {lastTest ? (
                <div className="sentence-test-foot">
                  上次试验：<code>{lastTest.raw}</code> · {lastTest.n} 个候选
                </div>
              ) : null}
            </section>
          </div>
        </div>

        {dirty ? (
          <div className="sentence-applybar" role="status">
            <div>
              <b>有未应用的调整</b>
              <span>权重 / 提前上屏键数 / 重排数字改动需显式写入 POST /api/config。</span>
            </div>
            <div className="sentence-apply-actions">
              <button type="button" className="btn ghost" onClick={() => void discardDraft()} disabled={saving}>
                放弃
              </button>
              <button type="button" className="btn primary" onClick={() => void applyDraft()} disabled={saving}>
                {saving ? '保存中…' : '应用并保存'}
              </button>
            </div>
          </div>
        ) : null}

        {!booted ? <div className="sentence-loading">读取本机整句配置…</div> : null}
      </div>
    </>
  );
}
