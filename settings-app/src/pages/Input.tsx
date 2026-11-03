import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import type { Bootstrap, Config } from '../types';
import { useDirtyGuard } from '../dirtyGuard';
import { Icon } from '../components/Icon';
import { Badge, PageHead, Slider, Switch } from '../components/ui';
import '../styles/input.css';

import { fetchWithTimeout } from '../api';
/**
 * 输入页 —— 对应旧 settings-ui「输入与候选」页。
 *
 * 真源：GET /api/config（hufu_config::Config），保存 POST /api/config。
 * 后端 apply_config 是整份替换，因此保存时先 GET 最新配置，只合并本页
 * 负责的字段再 POST —— 避免把 Lead 正在改的其它页字段覆盖回旧值。
 */

type Dict = Record<string, unknown>;

const asDict = (v: unknown): Dict =>
  v && typeof v === 'object' && !Array.isArray(v) ? (v as Dict) : {};
const str = (v: unknown, d = '') => (typeof v === 'string' ? v : d);
const num = (v: unknown, d = 0) => (typeof v === 'number' && Number.isFinite(v) ? v : d);
const bool = (v: unknown, d = false) => (typeof v === 'boolean' ? v : d);
const clampInt = (v: number, lo: number, hi: number) =>
  Number.isFinite(v) ? Math.min(hi, Math.max(lo, Math.round(v))) : lo;
const chars = (v: string) => Array.from(v);

type CapsKey = 'Caps' | 'Clear' | 'Switch' | 'None';

interface InputDraft {
  general: {
    shift_switch: boolean;
    ctrl_space_switch: boolean;
    caps_action: CapsKey;
    switch_recent_schema: boolean;
    esc_undo: boolean;
    repeat_include_symbols: boolean;
    commit_raw_on_en_switch: boolean;
  };
  input: {
    alphabet: string;
    max_code_length: number;
    auto_push: boolean;
    auto_select_unique: boolean;
    auto_clear_empty: boolean;
    enter_clear: boolean;
    tab_clear: boolean;
    mixed_input: boolean;
    ascii_punct: boolean;
    slash_dunhao: boolean;
    backslash_dunhao: boolean;
    semicolon_guide: boolean;
    code_disguise: string;
    wildcard_enabled: boolean;
    wildcard_char: string;
  };
  candidates: {
    page_size: number;
    paging_keys: string;
    second_select: string;
    third_select: string;
    show_pinyin_comment: boolean;
    show_unicode_comment: boolean;
    show_split: boolean;
    split_scheme: string;
    diff_hl: boolean;
    delay_show_ms: number;
    delay_comment_ms: number;
  };
  punct: {
    full_shape: boolean;
    pair_brackets: boolean;
  };
  sound: {
    enabled: boolean;
    volume: number;
    pack: string;
  };
  opencc: {
    enabled: boolean;
    to_traditional: boolean;
    emoji: boolean;
  };
  clipboard: {
    enabled: boolean;
    whitelist: string;
  };
  /** 录制式组合键；参与保存。 */
  readOnly: {
    freq_key: string;
    delete_key: string;
    export_key: string;
  };
}

type SectionKey = 'general' | 'input' | 'candidates' | 'punct' | 'sound' | 'opencc' | 'clipboard';
type InputTab = 'switch' | 'input' | 'candidates' | 'sound' | 'advanced';

const TAB_FROM_HASH: Record<string, InputTab> = {
  switch: 'switch',
  input: 'input',
  candidates: 'candidates',
  sound: 'sound',
  advanced: 'advanced',
};

function tabFromHash(): InputTab {
  const h = (location.hash || '').replace('#', '').replace(/^\//, '');
  const sub = h.split('/')[1] || '';
  return TAB_FROM_HASH[sub] || 'switch';
}

const CAPS_VALUES: CapsKey[] = ['Caps', 'Clear', 'Switch', 'None'];

function normalize(cfg: Config): InputDraft {
  const g = asDict(cfg.general);
  const i = asDict(cfg.input);
  const c = asDict(cfg.candidates);
  const p = asDict(cfg.punct);
  const s = asDict(cfg.sound);
  const o = asDict(cfg.opencc);
  const cb = asDict(cfg.clipboard);
  const rawCaps = str(g.caps_action, 'Caps');
  const caps = (CAPS_VALUES as string[]).includes(rawCaps) ? (rawCaps as CapsKey) : 'Caps';
  return {
    general: {
      shift_switch: bool(g.shift_switch, true),
      ctrl_space_switch: bool(g.ctrl_space_switch, true),
      caps_action: caps,
      switch_recent_schema: bool(g.switch_recent_schema, true),
      esc_undo: bool(g.esc_undo, false),
      repeat_include_symbols: bool(g.repeat_include_symbols, false),
      commit_raw_on_en_switch: bool(g.commit_raw_on_en_switch, true),
    },
    input: {
      alphabet: str(i.alphabet, ";'zyxwvutsrqponmlkjihgfedcba"),
      max_code_length: num(i.max_code_length, 4),
      auto_push: bool(i.auto_push, true),
      auto_select_unique: bool(i.auto_select_unique, false),
      auto_clear_empty: bool(i.auto_clear_empty, false),
      enter_clear: bool(i.enter_clear, false),
      tab_clear: bool(i.tab_clear, true),
      mixed_input: bool(i.mixed_input, false),
      ascii_punct: bool(i.ascii_punct, false),
      slash_dunhao: bool(i.slash_dunhao, false),
      backslash_dunhao: bool(i.backslash_dunhao, false),
      semicolon_guide: bool(i.semicolon_guide, true),
      code_disguise: str(i.code_disguise, ''),
      wildcard_enabled: bool(i.wildcard_enabled, false),
      wildcard_char: str(i.wildcard_char, '\\'),
    },
    candidates: {
      page_size: num(c.page_size, 4),
      paging_keys: str(c.paging_keys, '-='),
      second_select: str(c.second_select, ';'),
      third_select: str(c.third_select, "'"),
      show_pinyin_comment: bool(c.show_pinyin_comment, false),
      show_unicode_comment: bool(c.show_unicode_comment, false),
      show_split: bool(c.show_split, false),
      split_scheme: str(c.split_scheme, '虎码'),
      diff_hl: bool(c.diff_hl, false),
      delay_show_ms: num(c.delay_show_ms, 0),
      delay_comment_ms: num(c.delay_comment_ms, 0),
    },
    punct: {
      full_shape: bool(p.full_shape, true),
      pair_brackets: bool(p.pair_brackets, true),
    },
    sound: {
      enabled: bool(s.enabled, false),
      volume: num(s.volume, 50),
      pack: str(s.pack, ''),
    },
    opencc: {
      enabled: bool(o.enabled, false),
      to_traditional: bool(o.to_traditional, true),
      emoji: bool(o.emoji, false),
    },
    clipboard: {
      enabled: bool(cb.enabled, false),
      whitelist: Array.isArray(cb.whitelist)
        ? cb.whitelist.filter((x): x is string => typeof x === 'string').join(', ')
        : '',
    },
    readOnly: {
      freq_key: str(c.freq_key, 'ctrl'),
      delete_key: str(c.delete_key, 'ctrl+shift'),
      export_key: str(g.export_key, ''),
    },
  };
}

/**
 * 按字段合并：用户动过的字段保留 draft，没动过的字段跟随服务端。
 *
 * base 是「上一次与服务端对齐的草稿」（saved），prev 是当前草稿，server 是新
 * 快照。用于忽略窗结束后补同步、以及保存前对齐 fresh 配置：这样外部改动
 * （例如另一处把 OpenCC 切了）不会被旧草稿写回，正在编辑的输入框也不会被打断。
 */
function mergeUntouched(prev: InputDraft, base: InputDraft, server: InputDraft): InputDraft {
  const out = {} as InputDraft;
  (Object.keys(server) as (keyof InputDraft)[]).forEach((section) => {
    const p = prev[section] as unknown as Dict;
    const b = base[section] as unknown as Dict;
    const s = server[section] as unknown as Dict;
    const merged: Dict = {};
    for (const k of Object.keys(s)) merged[k] = p[k] !== b[k] ? p[k] : s[k];
    /* 防御：server 里缺失但草稿里存在的键原样保留（后端裁剪/字段新增时不丢输入）。 */
    for (const k of Object.keys(p)) if (!(k in merged)) merged[k] = p[k];
    (out as unknown as Dict)[section] = merged;
  });
  return out;
}

function validate(d: InputDraft, reversePrefix: string): string | null {
  const alphabet = d.input.alphabet;
  const letters = chars(alphabet);
  if (!alphabet) return '编码字母表不能为空';
  if (new Set(letters).size !== letters.length) return '编码字母表存在重复字符';
  if (/\s/.test(alphabet)) return '编码字母表不能包含空白字符';

  if (!Number.isInteger(d.input.max_code_length) || d.input.max_code_length < 1 || d.input.max_code_length > 10) {
    return '最大码长需为 1–10 的整数';
  }
  if (!Number.isInteger(d.candidates.page_size) || d.candidates.page_size < 1 || d.candidates.page_size > 10) {
    return '每页候选数需为 1–10 的整数';
  }
  if (!d.candidates.paging_keys) return '翻页键不能为空';
  if (chars(d.candidates.paging_keys).length > 8) return '翻页键最多 8 个字符';
  if (chars(d.candidates.second_select).length !== 1) return '次选键需为单个字符';
  if (chars(d.candidates.third_select).length !== 1) return '三选键需为单个字符';
  if (d.candidates.delay_show_ms < 0 || d.candidates.delay_show_ms > 2000) return '候选延时需在 0–2000 毫秒';
  if (d.candidates.delay_comment_ms < 0 || d.candidates.delay_comment_ms > 2000) {
    return '注释延时需在 0–2000 毫秒';
  }
  if (d.sound.volume < 0 || d.sound.volume > 100) return '音量需在 0–100';
  const wildcardChars = chars(d.input.wildcard_char);
  if (wildcardChars.length !== 1) return '通配字符需为单个字符（引擎字段为 char）';
  if (d.input.wildcard_enabled) {
    const wc = wildcardChars;
    if (letters.includes(wc[0])) return '通配字符不能出现在编码字母表里';
    if (wc[0] === reversePrefix) return '通配字符不能与反查引导键相同';
    if (wc[0] === d.candidates.second_select || wc[0] === d.candidates.third_select) {
      return '通配字符不能与次选/三选键相同';
    }
    if (d.candidates.paging_keys.includes(wc[0])) return '通配字符不能与翻页键相同';
  }
  return null;
}

function parseList(v: string): string[] {
  return v
    .split(/[,，;；\s]+/)
    .map((x) => x.trim())
    .filter(Boolean);
}

async function getJson<T>(url: string): Promise<T> {
  const r = await fetchWithTimeout(url, { cache: 'no-store' });
  if (!r.ok) throw new Error(`${url} -> ${r.status}`);
  return (await r.json()) as T;
}

async function getConfig(): Promise<Config> {
  return getJson<Config>('/api/config');
}

async function postConfig(cfg: unknown): Promise<void> {
  const r = await fetchWithTimeout('/api/config', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(cfg),
  });
  const body: { error?: string } = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(body.error || `保存失败（HTTP ${r.status}）`);
}

function countDiff(a: InputDraft, b: InputDraft): number {
  let n = 0;
  (['general', 'input', 'candidates', 'punct', 'sound', 'opencc', 'clipboard'] as SectionKey[]).forEach((sec) => {
    const aa = a[sec] as unknown as Dict;
    const bb = b[sec] as unknown as Dict;
    new Set([...Object.keys(aa), ...Object.keys(bb)]).forEach((k) => {
      if (aa[k] !== bb[k]) n += 1;
    });
  });
  (Object.keys(a.readOnly) as (keyof InputDraft['readOnly'])[]).forEach((k) => {
    if (a.readOnly[k] !== b.readOnly[k]) n += 1;
  });
  return n;
}

/* ── 小组件 ─────────────────────────────────────────────────────────── */

function Card({
  title,
  sub,
  badge,
  children,
  category,
}: {
  title: string;
  sub?: ReactNode;
  badge?: ReactNode;
  children: ReactNode;
  category?: InputTab;
}) {
  return (
    <section className="card ip-card" data-cat={category}>
      <div className="ip-card-head">
        <span className="ip-card-title">{title}</span>
        {badge}
      </div>
      {sub ? <div className="ip-card-sub">{sub}</div> : null}
      {children}
    </section>
  );
}

function Row({ label, hint, children }: { label: ReactNode; hint?: ReactNode; children: ReactNode }) {
  return (
    <div className="ip-row">
      <div className="ip-row-main">
        <span className="ip-label">{label}</span>
        {hint ? <span className="ip-hint">{hint}</span> : null}
      </div>
      <div className="ip-control">{children}</div>
    </div>
  );
}

function ToggleRow({
  label,
  hint,
  on,
  onChange,
}: {
  label: string;
  hint?: ReactNode;
  on: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <Row label={label} hint={hint}>
      <Switch on={on} onChange={onChange} title={label} />
    </Row>
  );
}

function Field({
  label,
  hint,
  wide,
  children,
}: {
  label: string;
  hint?: ReactNode;
  wide?: boolean;
  children: ReactNode;
}) {
  return (
    <div className={`ip-field${wide ? ' wide' : ''}`}>
      <span className="ip-label">{label}</span>
      {children}
      {hint ? <span className="ip-hint">{hint}</span> : null}
    </div>
  );
}

function Seg({
  value,
  options,
  onChange,
  className,
}: {
  value: string;
  options: { v: string; l: string }[];
  onChange: (v: string) => void;
  className?: string;
}) {
  return (
    <div className={`ip-seg${className ? ` ${className}` : ''}`} role="group">
      {options.map((o) => (
        <button
          key={o.v}
          type="button"
          className={o.v === value ? 'on' : ''}
          aria-pressed={o.v === value}
          onClick={() => onChange(o.v)}
        >
          {o.l}
        </button>
      ))}
    </div>
  );
}

/* ── 页面 ───────────────────────────────────────────────────────────── */

export function InputPage({ data, notify }: { data: Bootstrap; notify: (msg: string) => void }) {
  const [draft, setDraft] = useState<InputDraft>(() => normalize(data.config));
  const [saved, setSaved] = useState<InputDraft>(() => normalize(data.config));
  const [saving, setSaving] = useState(false);
  const [packs, setPacks] = useState<string[]>([]);
  const [splitSchemes, setSplitSchemes] = useState<string[]>([]);
  const dirtyRef = useRef(false);
  const ignorePropsUntil = useRef(0);
  /* 【忽略窗漏同步修复 2026-11】刚保存后 15s 内会忽略可能仍在途的旧 Bootstrap
     快照，避免刚保存的值被上一轮 GET 的旧值覆盖。但旧实现是「直接 return」：
     这一次 props 变化被彻底丢掉，而 propKey 此时已经变了、之后可能再也不会变
     ——外部改动（真实事故：保存后 15s 内按 s2t 键切了 OpenCC）就永远补不进
     draft，下一次保存会把它写回旧值。这里把被忽略的 propKey 记账，窗口结束后
     的下一次 props 变化（或保存前的 fresh GET）再做一次合并同步。 */
  const pendingPropKey = useRef<string | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const [recorder, setRecorder] = useState<keyof InputDraft['readOnly'] | null>(null);
  const [recordPreview, setRecordPreview] = useState('');
  /* 【组合键开关 2026-11-01】调频/删词/导出功能开关：关=写空串（引擎
     parse_key_combo 空集恒 false=停用），重开恢复上次值（ref 记忆）。 */
  const lastCombo = useRef<Record<string, string>>({});
  useEffect(() => {
    for (const k of ['freq_key', 'delete_key', 'export_key'] as const) {
      const v = draft.readOnly[k];
      if (v) lastCombo.current[k] = v;
    }
    // 仅挂载时播种记忆（draft 引用不稳定，不进依赖）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const toggleCombo = (key: 'freq_key' | 'delete_key' | 'export_key', on: boolean | string) => {
    if (on) {
      const fallback = key === 'freq_key' ? 'ctrl' : key === 'delete_key' ? 'ctrl+shift' : '';
      const v = lastCombo.current[key] || fallback;
      setDraft((p) => ({ ...p, readOnly: { ...p.readOnly, [key]: v } }));
    } else {
      const cur = draft.readOnly[key];
      if (cur) lastCombo.current[key] = cur;
      setDraft((p) => ({ ...p, readOnly: { ...p.readOnly, [key]: '' } }));
    }
  };
  const [tab, setTab] = useState<InputTab>(tabFromHash);
  const tabsRef = useRef<HTMLDivElement | null>(null);
  const [tabInk, setTabInk] = useState({ x: 0, w: 0 });
  const [slideDir, setSlideDir] = useState(0);
  const slideTimer = useRef(0);
  const TAB_ORDER: InputTab[] = ['switch', 'input', 'candidates', 'sound', 'advanced'];
  const switchTab = (key: InputTab) => {
    if (key === tab) return;
    const from = TAB_ORDER.indexOf(tab);
    const to = TAB_ORDER.indexOf(key);
    setSlideDir(to > from ? 1 : -1);
    setTab(key);
    try {
      history.replaceState(null, '', `#input/${key}`);
    } catch {
      /* file:// */
    }
    window.clearTimeout(slideTimer.current);
    slideTimer.current = window.setTimeout(() => setSlideDir(0), 380);
  };
  useEffect(() => () => window.clearTimeout(slideTimer.current), []);
  useEffect(() => {
    const root = tabsRef.current;
    if (!root) return;
    const sync = () => {
      const active = root.querySelector<HTMLElement>('.ip-tab.on');
      if (!active) return;
      const rr = root.getBoundingClientRect();
      const ar = active.getBoundingClientRect();
      setTabInk({ x: ar.left - rr.left + root.scrollLeft, w: ar.width });
    };
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(root);
    return () => ro.disconnect();
  }, [tab]);

  const dirty = useMemo(() => JSON.stringify(draft) !== JSON.stringify(saved), [draft, saved]);
  useEffect(() => {
    dirtyRef.current = dirty;
  }, [dirty]);

  /* OV-B1：脏状态上报全局守卫（切路由/关窗前 App 统一拦截确认）。 */
  useDirtyGuard('input', '输入', dirty);

  const reversePrefix = useMemo(() => str(asDict(data.config.reverse).prefix, '`'), [data.config]);
  const propKey = useMemo(() => JSON.stringify(normalize(data.config)), [data.config]);

  /* 首次进入拉一次完整配置（Bootstrap 已带，但保证拿到全量字段）。 */
  useEffect(() => {
    let alive = true;
    getConfig()
      .then((cfg) => {
        if (!alive || dirtyRef.current) return;
        const next = normalize(cfg);
        setDraft(next);
        setSaved(next);
      })
      .catch(() => {
        /* 离线预览：保留 Bootstrap/默认值，保存时仍会重新 GET */
      });
    return () => {
      alive = false;
    };
  }, []);

  /* App 每 10s 刷新 Bootstrap；未改动时跟随，改动中不覆盖用户编辑。 */
  useEffect(() => {
    if (!data.online) return;
    if (Date.now() < ignorePropsUntil.current) {
      /* 忽略窗内不能把这个 propKey 扔掉：它已经变化且不会再变，丢掉就永远补
         不回来。先记账，窗口结束后的下一次 props 变化做合并同步。 */
      pendingPropKey.current = propKey;
      return;
    }
    if (pendingPropKey.current !== null) {
      /* 窗口已过且有积压的外部改动：把服务端值合进 draft 一次，然后清账。
         用户已经改过的字段（draft ≠ saved）保持原样，只有没动过的字段跟随
         服务端，所以即便正在编辑也不会被打断。 */
      pendingPropKey.current = null;
      const next = normalize(data.config);
      setDraft((prev) => mergeUntouched(prev, saved, next));
      setSaved(next);
      return;
    }
    if (dirtyRef.current) return;
    const next = normalize(data.config);
    setDraft(next);
    setSaved(next);
  }, [propKey, data.online]);

  useEffect(() => {
    let alive = true;
    getJson<{ packs?: unknown }>('/api/sound_packs')
      .then((r) => {
        if (!alive) return;
        setPacks(Array.isArray(r.packs) ? r.packs.filter((x): x is string => typeof x === 'string') : []);
      })
      .catch(() => {});
    getJson<{ split?: unknown }>('/api/assets')
      .then((r) => {
        if (!alive) return;
        setSplitSchemes(Array.isArray(r.split) ? r.split.filter((x): x is string => typeof x === 'string') : []);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  const patch = (section: SectionKey, key: string, value: unknown) => {
    setDraft((prev) => ({
      ...prev,
      [section]: { ...(prev[section] as unknown as Dict), [key]: value },
    }) as InputDraft);
  };

  const discard = () => {
    setDraft(saved);
    notify('已撤销本页未保存的修改');
  };

  const save = async () => {
    const invalid = validate(draft, reversePrefix);
    if (invalid) {
      notify(invalid);
      return;
    }
    setSaving(true);
    try {
      const fresh = await getConfig();
      /* 【保存前对齐 2026-11】忽略窗里积压的外部改动可能还没机会补进 draft
         （窗口结束后如果没有任何新的 props 变化，上面的补同步不会触发）。
         这里用刚取回的 fresh 做同一套合并：用户改过的字段以 draft 为准，没动
         过的字段取服务端最新值 —— 既不改写外部改动，也不覆盖本页编辑。
         合并成功后再清 pendingPropKey，失败时保留账继续等下一轮补同步。 */
      const merged = mergeUntouched(draft, saved, normalize(fresh));
      const base: Dict = { ...(fresh as unknown as Dict) };
      base.general = Object.assign({}, asDict(fresh.general), merged.general);
      base.input = Object.assign({}, asDict(fresh.input), merged.input, {
        max_code_length: clampInt(merged.input.max_code_length, 1, 10),
      });
      base.candidates = Object.assign({}, asDict(fresh.candidates), merged.candidates, {
        page_size: clampInt(merged.candidates.page_size, 1, 10),
        delay_show_ms: clampInt(merged.candidates.delay_show_ms, 0, 2000),
        delay_comment_ms: clampInt(merged.candidates.delay_comment_ms, 0, 2000),
        freq_key: merged.readOnly.freq_key,
        delete_key: merged.readOnly.delete_key,
      });
      base.general = Object.assign({}, asDict(fresh.general), merged.general, {
        export_key: merged.readOnly.export_key,
      });
      base.punct = Object.assign({}, asDict(fresh.punct), merged.punct);
      base.sound = Object.assign({}, asDict(fresh.sound), merged.sound, {
        volume: clampInt(merged.sound.volume, 0, 100),
      });
      base.opencc = Object.assign({}, asDict(fresh.opencc), merged.opencc);
      base.clipboard = Object.assign({}, asDict(fresh.clipboard), {
        enabled: merged.clipboard.enabled,
        whitelist: parseList(merged.clipboard.whitelist),
      });
      await postConfig(base);

      const applied: InputDraft = {
        ...merged,
        input: { ...merged.input, max_code_length: clampInt(merged.input.max_code_length, 1, 10) },
        candidates: {
          ...merged.candidates,
          page_size: clampInt(merged.candidates.page_size, 1, 10),
          delay_show_ms: clampInt(merged.candidates.delay_show_ms, 0, 2000),
          delay_comment_ms: clampInt(merged.candidates.delay_comment_ms, 0, 2000),
        },
        sound: { ...merged.sound, volume: clampInt(merged.sound.volume, 0, 100) },
      };
      setDraft(applied);
      setSaved(applied);
      pendingPropKey.current = null;
      ignorePropsUntil.current = Date.now() + 15000;
      notify(
        `输入设置已保存并应用 · 码长 ${applied.input.max_code_length} · 每页 ${applied.candidates.page_size} 个候选`,
      );
    } catch (e) {
      notify(`保存失败：${(e as Error).message}`);
    } finally {
      setSaving(false);
    }
  };

  const previewSound = (tag: 'key' | 'select' | 'commit' | 'page', label: string) => {
    try {
      audioRef.current?.pause();
      const url = `/api/sound?tag=${tag}&pack=${encodeURIComponent(draft.sound.pack)}`;
      const audio = new Audio(url);
      audio.volume = Math.max(0, Math.min(1, draft.sound.volume / 100));
      audioRef.current = audio;
      void audio.play().catch(() => notify(`试听失败：音效包「${draft.sound.pack || '根目录'}」缺少 ${label} 音频`));
    } catch {
      notify('试听失败：当前环境无法播放音频');
    }
  };

  const engine = asDict(data.state.state);
  const chinese = engine.chinese !== false;
  const fullShape = engine.full_shape === true;
  const capsLock = engine.caps_lock === true;
  const mode = str(engine.mode, '');
  const scheme = data.state.current_schema || '—';

  const keymapCfg = asDict(data.config.keymap);
  const keymapMap = asDict(keymapCfg.map);
  const keymapIdle = asDict(keymapCfg.map_idle);
  const mappedKeys = new Set([...Object.keys(keymapMap), ...Object.keys(keymapIdle)]).size;
  const shiftSymbols = asDict(keymapCfg.shift_symbols);
  const shiftCount = Object.keys(shiftSymbols).length;

  const splitOptions = useMemo(() => {
    const names = new Set(splitSchemes);
    if (draft.candidates.split_scheme) names.add(draft.candidates.split_scheme);
    return ['', ...Array.from(names).sort((a, b) => a.localeCompare(b, 'zh-CN'))];
  }, [splitSchemes, draft.candidates.split_scheme]);

  const packOptions = useMemo(() => {
    const names = new Set(packs);
    if (draft.sound.pack) names.add(draft.sound.pack);
    return ['', ...Array.from(names)];
  }, [packs, draft.sound.pack]);

  const wildcardIssue = useMemo(() => {
    if (!draft.input.wildcard_enabled) return null;
    const wc = chars(draft.input.wildcard_char);
    if (wc.length !== 1) return '通配字符需为单个字符';
    if (chars(draft.input.alphabet).includes(wc[0])) return '通配字符在编码字母表内，引擎会按关闭处理';
    if (wc[0] === reversePrefix) return '与反查引导键相同，反查优先，通配符不会生效';
    if (wc[0] === draft.candidates.second_select || wc[0] === draft.candidates.third_select) {
      return '与次选/三选键冲突，选重优先';
    }
    if (draft.candidates.paging_keys.includes(wc[0])) return '与翻页键冲突，翻页优先';
    return null;
  }, [
    draft.input.wildcard_enabled,
    draft.input.wildcard_char,
    draft.input.alphabet,
    draft.candidates.second_select,
    draft.candidates.third_select,
    draft.candidates.paging_keys,
    reversePrefix,
  ]);

  /* 录制式组合键：调频/删词/导出。只改本地 draft，点“保存并应用”才写盘。 */
  useEffect(() => {
    if (!recorder) return;
    let timer = 0;
    let finished = false;
    const mods = (e: KeyboardEvent) => {
      const out: string[] = [];
      if (e.ctrlKey) out.push('ctrl');
      if (e.altKey) out.push('alt');
      if (e.shiftKey) out.push('shift');
      if (e.metaKey) out.push('win');
      return out;
    };
    const finish = (value: string) => {
      if (finished) return;
      finished = true;
      window.clearTimeout(timer);
      setDraft((prev) => ({ ...prev, readOnly: { ...prev.readOnly, [recorder]: value } }));
      setRecorder(null);
      setRecordPreview('');
      notify(value ? `已录制：${value}（保存后生效）` : '已清空该组合键');
    };
    /* 【单键可录 2026-11-01】旧逻辑 bug：录单修饰键（如只按 Alt）时，
       keydown 起 450ms 计时，用户正常速度松开 → keyup 后按「当前按住
       集」（已空）录成空串=停用——单键永远设不上，只有组合能录成。
       修法：本会话按过的键记入 history，keyup 收尾时录「按过并集」
       （按过即录）：单击 Alt → 'alt'；Ctrl+Shift 按下再全松 →
       'ctrl+shift'；普通键立即录不变。 */
    const history = new Set<string>();
    const recordMods = (e: KeyboardEvent) => {
      if (e.ctrlKey) history.add('ctrl');
      if (e.altKey) history.add('alt');
      if (e.shiftKey) history.add('shift');
      if (e.metaKey) history.add('win');
    };
    const scheduleModifier = (e: KeyboardEvent) => {
      const value = mods(e).join('+');
      setRecordPreview(value);
      window.clearTimeout(timer);
      timer = window.setTimeout(() => finish(value), 450);
    };
    const onKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === 'Escape') {
        finished = true;
        window.clearTimeout(timer);
        setRecorder(null);
        setRecordPreview('');
        return;
      }
      if (e.key === 'Backspace' && !e.ctrlKey && !e.altKey && !e.shiftKey && !e.metaKey) {
        finish('');
        return;
      }
      const raw = e.key;
      if (['Control', 'Shift', 'Alt', 'Meta', 'OS'].includes(raw)) {
        recordMods(e);
        scheduleModifier(e);
        return;
      }
      window.clearTimeout(timer);
      const key = raw === ' ' ? 'space' : /^Arrow/.test(raw) ? raw.slice(5).toLowerCase() : raw.toLowerCase();
      finish([...mods(e), key].join('+'));
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (['Control', 'Shift', 'Alt', 'Meta', 'OS'].includes(e.key)) {
        /* 【单键可录 2026-11-01】收尾值取「本会话按过并集」而非当前
           按住集——松开最后一个键时按住集已空，旧行为会录成空串。 */
        window.clearTimeout(timer);
        timer = window.setTimeout(() => finish([...history].join('+')), 300);
      }
    };
    window.addEventListener('keydown', onKeyDown, true);
    window.addEventListener('keyup', onKeyUp, true);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener('keydown', onKeyDown, true);
      window.removeEventListener('keyup', onKeyUp, true);
    };
  }, [recorder]);

  /* OV-B4：切页签即取消组合键录制——页签只是 CSS 过滤，组件不卸载，
     不取消的话 recorder 会继续在 window 捕获阶段抢键。 */
  useEffect(() => {
    setRecorder(null);
    setRecordPreview('');
  }, [tab]);

  useEffect(() => {
    const sync = () => {
      const h = (location.hash || '').replace('#', '').replace(/^\//, '');
      if (h.split('/')[0] === 'input') setTab(tabFromHash());
    };
    window.addEventListener('hashchange', sync);
    return () => window.removeEventListener('hashchange', sync);
  }, []);

  const diff = countDiff(draft, saved);
  const validationError = validate(draft, reversePrefix);

  return (
    <>
      <PageHead
        title="输入"
        sub={
          <>
            <span className={data.online ? 'dot' : 'dot muted'} />
            {data.online ? '引擎已连接' : '离线预览'}
            <span style={{ color: 'var(--text3)' }}>·</span>
            {scheme}
            <span style={{ color: 'var(--text3)' }}>·</span>
            {chinese ? '中文' : '英文'}
            {fullShape ? ' · 全角' : ' · 半角'}
          </>
        }
      />

      {dirty ? (
        <div className="ip-dirty">
          <span className="dot gold" />
          <span>本页有 {diff} 项修改尚未保存；保存后引擎约 2.5 秒内热更新。</span>
          <button type="button" className="btn sm primary" disabled={saving} onClick={save}>
            {saving ? '保存中…' : '立即保存'}
          </button>
        </div>
      ) : null}

      {dirty && validationError ? <div className="ip-warn ip-warn-top">无法保存：{validationError}</div> : null}

      <div className="ip-summary">
        <span className="chip">
          方案 <b>{scheme}</b>
        </span>
        <span className="chip">
          候选 <b>{draft.candidates.page_size}</b>/页
        </span>
        <span className="chip">
          码长 <b>{draft.input.max_code_length}</b>
        </span>
        <span className={`chip${draft.opencc.enabled ? ' gold' : ''}`}>
          简繁 <b>{draft.opencc.enabled ? (draft.opencc.to_traditional ? '简→繁' : '繁→简') : '关'}</b>
        </span>
        <span className={`chip${draft.sound.enabled ? ' gold' : ''}`}>
          音效 <b>{draft.sound.enabled ? `${draft.sound.volume}%` : '关'}</b>
        </span>
        <span className="chip">
          键位映射 <b>{mappedKeys}</b>
        </span>
      </div>

      <div
        className="ip-tabs"
        role="tablist"
        aria-label="输入设置分类"
        ref={tabsRef}
        onKeyDown={(e) => {
          const i = TAB_ORDER.indexOf(tab);
          if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
            e.preventDefault();
            const n = e.key === 'ArrowRight' ? (i + 1) % TAB_ORDER.length : (i - 1 + TAB_ORDER.length) % TAB_ORDER.length;
            switchTab(TAB_ORDER[n]);
          } else if (e.key === 'Home') {
            e.preventDefault();
            switchTab(TAB_ORDER[0]);
          } else if (e.key === 'End') {
            e.preventDefault();
            switchTab(TAB_ORDER[TAB_ORDER.length - 1]);
          }
        }}
      >
        {([
          ['switch', '切换', '中英切换与 Caps'],
          ['input', '编码', '编码、上屏与混输'],
          ['candidates', '候选', '候选、翻页、标点'],
          ['sound', '声音', '音效、繁简与 Emoji'],
          ['advanced', '高级', '剪贴板、状态与组合键'],
        ] as const).map(([key, label, hint]) => (
          <button
            key={key}
            type="button"
            role="tab"
            aria-selected={tab === key}
            className={`ip-tab${tab === key ? ' on' : ''}`}
            title={hint}
            onClick={() => switchTab(key)}
          >
            {label}
          </button>
        ))}
        <span
          className="ip-tab-ink"
          aria-hidden="true"
          style={{ transform: `translateX(${tabInk.x}px)`, width: tabInk.w }}
        />
      </div>

      <div
        className="ip-grid"
        data-tab={tab}
        data-slide={slideDir === 1 ? 'right' : slideDir === -1 ? 'left' : undefined}
      >
        <div className="ip-col">
          <Card
            category="switch"
            title="中英切换"
            badge={<Badge tone="gray">general</Badge>}
            sub="仅影响引擎自有切换路径；英文态 Caps 恒为系统大小写。"
          >
            <ToggleRow
              label="Shift 切换中英"
              hint="单击 Shift 在中/英之间切换；已映射的左右 Shift 不再触发切换。"
              on={draft.general.shift_switch}
              onChange={(v) => patch('general', 'shift_switch', v)}
            />
            <ToggleRow
              label="Ctrl+空格 切换中英"
              on={draft.general.ctrl_space_switch}
              onChange={(v) => patch('general', 'ctrl_space_switch', v)}
            />
            <Row label="Caps 行为" hint="仅中文态生效；英文态 Caps 按系统默认大小写。">
              <Seg
                className="caps"
                value={draft.general.caps_action}
                onChange={(v) => patch('general', 'caps_action', v as CapsKey)}
                options={[
                  { v: 'Caps', l: '锁定大写' },
                  { v: 'Clear', l: '清屏' },
                  { v: 'Switch', l: '切英文' },
                  { v: 'None', l: '无操作' },
                ]}
              />
            </Row>
            <ToggleRow
              label="Ctrl+M 切换最近方案"
              hint="在当前方案与上一方案之间来回切换。"
              on={draft.general.switch_recent_schema}
              onChange={(v) => patch('general', 'switch_recent_schema', v)}
            />
            <ToggleRow
              label="Esc 撤回上次上屏"
              hint="空态 + 焦点有光标时生效，连按撤更早（最多 8 条）；编码中 Esc 仍清屏。"
              on={draft.general.esc_undo}
              onChange={(v) => patch('general', 'esc_undo', v)}
            />
            <ToggleRow
              label="重复上屏包括符号"
              hint="关=只记中英文词组；开=标点/符号整段入历史，可回放「中，」。"
              on={draft.general.repeat_include_symbols}
              onChange={(v) => patch('general', 'repeat_include_symbols', v)}
            />
            <ToggleRow
              label="切换英文时保留已输入编码并上屏"
              hint="Shift / Caps / Ctrl+空格 / 自定义 switch 同一口径；语言栏点击始终清空。"
              on={draft.general.commit_raw_on_en_switch}
              onChange={(v) => patch('general', 'commit_raw_on_en_switch', v)}
            />
          </Card>

          <Card
            category="input"
            title="编码"
            badge={<Badge tone="gray">input</Badge>}
            sub="字母表、最大码长、自动上屏与标点直出；改动保存后下一键生效。"
          >
            <Field
              label="编码字母表"
              wide
              hint={
                <>
                  默认 27 码元；当前 <b>{chars(draft.input.alphabet).length}</b> 个。字母表内字符不可作通配符。
                </>
              }
            >
              <input
                className="ip-input"
                type="text"
                spellCheck={false}
                autoComplete="off"
                value={draft.input.alphabet}
                onChange={(e) => patch('input', 'alphabet', e.target.value)}
              />
            </Field>

            <div className="ip-cols" style={{ marginTop: 10 }}>
              <Field label="最大码长" hint="1–10；超出后按下一项决定是否顶屏。">
                <input
                  className="ip-input num"
                  type="number"
                  min={1}
                  max={10}
                  step={1}
                  value={draft.input.max_code_length}
                  onChange={(e) => patch('input', 'max_code_length', e.target.value === '' ? 0 : Number(e.target.value))}
                  onBlur={() => patch('input', 'max_code_length', clampInt(draft.input.max_code_length, 1, 10))}
                />
              </Field>
              <Field label="Tab 键" hint="清屏=当前行为；导航=高亮下一候选，空格上屏。">
                <Seg
                  value={draft.input.tab_clear ? 'clear' : 'nav'}
                  onChange={(v) => patch('input', 'tab_clear', v === 'clear')}
                  options={[
                    { v: 'clear', l: '清屏' },
                    { v: 'nav', l: '选重导航' },
                  ]}
                />
              </Field>
            </div>

            <div style={{ marginTop: 8 }}>
              <ToggleRow
                label="超最大码长自动上屏"
                on={draft.input.auto_push}
                onChange={(v) => patch('input', 'auto_push', v)}
              />
              <ToggleRow
                label="满最大码长唯一自动上屏"
                on={draft.input.auto_select_unique}
                onChange={(v) => patch('input', 'auto_select_unique', v)}
              />
              <ToggleRow
                label="空码自动清屏"
                hint="满码 +1 键仍无解才清前码，末键保留为新输入。"
                on={draft.input.auto_clear_empty}
                onChange={(v) => patch('input', 'auto_clear_empty', v)}
              />
              <ToggleRow
                label="回车清屏"
                on={draft.input.enter_clear}
                onChange={(v) => patch('input', 'enter_clear', v)}
              />
              <ToggleRow
                label="中英混输（保留大小写）"
                on={draft.input.mixed_input}
                onChange={(v) => patch('input', 'mixed_input', v)}
              />
              <ToggleRow
                label="中文态使用英文标点"
                on={draft.input.ascii_punct}
                onChange={(v) => patch('input', 'ascii_punct', v)}
              />
              <ToggleRow
                label="/=顿号直出"
                hint="开=空态按 / 直接上屏「、」并屏蔽码表内 / 命名空间；关=进符号命名空间。"
                on={draft.input.slash_dunhao}
                onChange={(v) => patch('input', 'slash_dunhao', v)}
              />
              <ToggleRow
                label="\=顿号直出"
                hint="开=空态按 \ 直接上屏「、」；关=弹「、」候选，空格确认。"
                on={draft.input.backslash_dunhao}
                onChange={(v) => patch('input', 'backslash_dunhao', v)}
              />
              <ToggleRow
                label=";=引导快符"
                hint="方案里有 ; 编码/快符时弹「：/；」候选；无 ; 数据时按普通符号键。"
                on={draft.input.semicolon_guide}
                onChange={(v) => patch('input', 'semicolon_guide', v)}
              />
            </div>

            <div className="ip-cols" style={{ marginTop: 8 }}>
              <Field label="编码伪装前缀" hint="显示在编码前，如 *；留空=关闭。">
                <input
                  className="ip-input char"
                  type="text"
                  maxLength={3}
                  value={draft.input.code_disguise}
                  onChange={(e) => patch('input', 'code_disguise', e.target.value)}
                />
              </Field>
              <Field label="通配字符" hint="默认 \；单字符，不能与字母表/反查/选重/翻页冲突。">
                <input
                  className="ip-input char"
                  type="text"
                  maxLength={2}
                  disabled={!draft.input.wildcard_enabled}
                  value={draft.input.wildcard_char}
                  onChange={(e) => patch('input', 'wildcard_char', e.target.value)}
                />
              </Field>
            </div>
            <ToggleRow
              label="编码通配符"
              hint="打 d\x = 全部 3 码（首 d 末 x）词条，注释位显示完整编码；搜索语义下不自动上屏/清屏。"
              on={draft.input.wildcard_enabled}
              onChange={(v) => patch('input', 'wildcard_enabled', v)}
            />
            {wildcardIssue ? <div className="ip-warn">{wildcardIssue}</div> : null}
          </Card>

          <Card
            category="candidates"
            title="候选"
            badge={<Badge tone="gray">candidates</Badge>}
            sub="候选窗行为与注释来源；竖排/序号属皮肤单一真源，在皮肤工作室调整。"
          >
            <div className="ip-cols">
              <Field label="每页候选数" hint="1–10，默认 4。">
                <input
                  className="ip-input num"
                  type="number"
                  min={1}
                  max={10}
                  step={1}
                  value={draft.candidates.page_size}
                  onChange={(e) => patch('candidates', 'page_size', e.target.value === '' ? 0 : Number(e.target.value))}
                  onBlur={() => patch('candidates', 'page_size', clampInt(draft.candidates.page_size, 1, 10))}
                />
              </Field>
              <Field label="翻页键" hint="逐字符生效，如 -= 或 ,.。">
                <input
                  className="ip-input"
                  type="text"
                  maxLength={8}
                  value={draft.candidates.paging_keys}
                  onChange={(e) => patch('candidates', 'paging_keys', e.target.value)}
                />
              </Field>
              <Field label="次选键" hint="单字符，默认 ;。">
                <input
                  className="ip-input char"
                  type="text"
                  maxLength={1}
                  value={draft.candidates.second_select}
                  onChange={(e) => patch('candidates', 'second_select', e.target.value)}
                />
              </Field>
              <Field label="三选键" hint="单字符，默认 '。">
                <input
                  className="ip-input char"
                  type="text"
                  maxLength={1}
                  value={draft.candidates.third_select}
                  onChange={(e) => patch('candidates', 'third_select', e.target.value)}
                />
              </Field>
              <Field label="候选延时（毫秒）" hint="0=立即显示，上限 2000。">
                <input
                  className="ip-input num"
                  type="number"
                  min={0}
                  max={2000}
                  step={10}
                  value={draft.candidates.delay_show_ms}
                  onChange={(e) => patch('candidates', 'delay_show_ms', e.target.value === '' ? 0 : Number(e.target.value))}
                  onBlur={() => patch('candidates', 'delay_show_ms', clampInt(draft.candidates.delay_show_ms, 0, 2000))}
                />
              </Field>
              <Field label="注释延时（毫秒）" hint="延时展开拼音/拆分注释。">
                <input
                  className="ip-input num"
                  type="number"
                  min={0}
                  max={2000}
                  step={10}
                  value={draft.candidates.delay_comment_ms}
                  onChange={(e) => patch('candidates', 'delay_comment_ms', e.target.value === '' ? 0 : Number(e.target.value))}
                  onBlur={() => patch('candidates', 'delay_comment_ms', clampInt(draft.candidates.delay_comment_ms, 0, 2000))}
                />
              </Field>
            </div>

            <div style={{ marginTop: 8 }}>
              <ToggleRow
                label="显示拼音注释"
                hint="数据\注释\拼音.注释；整句方案下暂不生效。"
                on={draft.candidates.show_pinyin_comment}
                onChange={(v) => patch('candidates', 'show_pinyin_comment', v)}
              />
              <ToggleRow
                label="显示 unicode 注释"
                hint="数据\注释\unicode.注释；整句方案下暂不生效。"
                on={draft.candidates.show_unicode_comment}
                onChange={(v) => patch('candidates', 'show_unicode_comment', v)}
              />
              <ToggleRow
                label="显示拆分"
                hint="数据\拆分\<方案>.拆分；整句方案下暂不生效。"
                on={draft.candidates.show_split}
                onChange={(v) => patch('candidates', 'show_split', v)}
              />
              <ToggleRow
                label="同码异字高亮"
                hint="同编码且 ≥3 字的候选互标差异字；默认关，整句下恒关。"
                on={draft.candidates.diff_hl}
                onChange={(v) => patch('candidates', 'diff_hl', v)}
              />
            </div>

            <Field
              label="拆分方案"
              wide
              hint="来自 数据\拆分\*.拆分；关闭=不显示拆分（与上方开关同时生效）。"
            >
              <select
                className="ip-select"
                value={draft.candidates.split_scheme}
                onChange={(e) => patch('candidates', 'split_scheme', e.target.value)}
              >
                {splitOptions.map((name) => (
                  <option key={name || '__off'} value={name}>
                    {name || '（关闭 / 使用默认）'}
                  </option>
                ))}
              </select>
            </Field>
          </Card>
        </div>

        <div className="ip-col">
          <Card category="candidates" title="标点与全半角" badge={<Badge tone="gray">punct</Badge>}>
            <ToggleRow
              label="全角标点"
              hint="中文态标点使用全角形态；关=半角。"
              on={draft.punct.full_shape}
              onChange={(v) => patch('punct', 'full_shape', v)}
            />
            <ToggleRow
              label="成对引号自动配对"
              hint="输入左引号/括号时自动补右半。"
              on={draft.punct.pair_brackets}
              onChange={(v) => patch('punct', 'pair_brackets', v)}
            />
            <div className="ip-note">
              引擎当前：{chinese ? '中文' : '英文'} · {fullShape ? '全角' : '半角'}。标点映射表本身由方案/快符数据决定，本页只控制全局形态开关。
            </div>
          </Card>

          <Card
            category="sound"
            title="音效"
            badge={<Badge tone="gray">sound</Badge>}
            sub="编码 / 选词 / 上屏 / 翻页四类提示音；音效包在 数据\音效\ 下整组替换。"
          >
            <ToggleRow
              label="启用按键音"
              on={draft.sound.enabled}
              onChange={(v) => patch('sound', 'enabled', v)}
            />
            <Row label="音量" hint={`${draft.sound.volume} / 100`}>
              <Slider
                value={draft.sound.volume}
                min={0}
                max={100}
                step={1}
                width={140}
                label="音效音量"
                onChange={(v) => patch('sound', 'volume', v)}
              />
            </Row>
            <Field
              label="音效包"
              wide
              hint="留空=使用 数据\音效\ 根目录四件；包目录需含 key/select/commit/page.wav。"
            >
              <select
                className="ip-select"
                value={draft.sound.pack}
                onChange={(e) => patch('sound', 'pack', e.target.value)}
              >
                {packOptions.map((name) => (
                  <option key={name || '__root'} value={name}>
                    {name || '（根目录四件 · 自定义）'}
                  </option>
                ))}
              </select>
            </Field>
            <div className="ip-audio">
              <button type="button" onClick={() => previewSound('key', '按键音')}>
                ▶ 编码
              </button>
              <button type="button" onClick={() => previewSound('select', '选词音')}>
                ▶ 选词
              </button>
              <button type="button" onClick={() => previewSound('commit', '上屏音')}>
                ▶ 上屏
              </button>
              <button type="button" onClick={() => previewSound('page', '翻页音')}>
                ▶ 翻页
              </button>
            </div>
            <div className="ip-note">试听走 GET /api/sound?tag=…&amp;pack=…，按当前音量播放选中音效包；缺文件时提示且不改配置。</div>
          </Card>

          <Card
            category="sound"
            title="繁简与 Emoji"
            badge={<Badge tone="gray">opencc</Badge>}
            sub="词典位于 数据\转换词典\；Emoji 注解候选可独立于 OpenCC 开启。"
          >
            <ToggleRow
              label="启用 OpenCC 转换"
              on={draft.opencc.enabled}
              onChange={(v) => patch('opencc', 'enabled', v)}
            />
            <Row label="转换方向">
              <Seg
                value={draft.opencc.to_traditional ? 's2t' : 't2s'}
                onChange={(v) => patch('opencc', 'to_traditional', v === 's2t')}
                options={[
                  { v: 's2t', l: '简 → 繁' },
                  { v: 't2s', l: '繁 → 简' },
                ]}
              />
            </Row>
            <ToggleRow
              label="Emoji 注解候选"
              hint="勾选的词各自成条候选（如 手机 → 📱/📲）；无需开启 OpenCC。"
              on={draft.opencc.emoji}
              onChange={(v) => patch('opencc', 'emoji', v)}
            />
          </Card>

          <Card
            category="advanced"
            title="剪贴板上屏"
            badge={<Badge tone="gray">clipboard</Badge>}
            sub="Ctrl+Shift+V 经输入法通道插入剪贴板文本，不走系统粘贴。"
          >
            <ToggleRow
              label="启用 Ctrl+Shift+V 剪贴板上屏"
              on={draft.clipboard.enabled}
              onChange={(v) => patch('clipboard', 'enabled', v)}
            />
            <Field
              label="进程白名单"
              wide
              hint="exe 名，逗号分隔；留空=所有进程。例：notepad.exe, WINWORD.EXE"
            >
              <input
                className="ip-input"
                type="text"
                value={draft.clipboard.whitelist}
                onChange={(e) => patch('clipboard', 'whitelist', e.target.value)}
              />
            </Field>
          </Card>

          <Card
            category="advanced"
            title="运行状态与键位"
            badge={<Badge tone="gray">部分只读</Badge>}
            sub="引擎实时状态只读；调频/删词/导出组合键可直接录制，键位映射请到键位编辑器。"
          >
            <div className="ip-kv">
              <span>当前方案</span>
              <b>{scheme}</b>
              <span>输入态</span>
              <b>
                {chinese ? '中文' : '英文'} · {fullShape ? '全角' : '半角'}
                {capsLock ? ' · Caps 锁定' : ''}
              </b>
              <span>引擎模式</span>
              <b>{mode || '—'}</b>
              <span>键位映射</span>
              <b>{mappedKeys} 个键已自定义</b>
              <span>Shift+符号</span>
              <b>{shiftCount} 项自定义</b>
            </div>
            <a className="btn sm outline ip-link-btn" href="#keymap">
              <Icon name="keyboard" />
              打开键位编辑器
              <Icon name="chev" />
            </a>
            <div className="ip-recorders">
              {([
                ['freq_key', '调频按住键'],
                ['delete_key', '删词按住键'],
                ['export_key', '导出码表快捷键'],
              ] as const).map(([key, label]) => (
                <div className="ip-recorder" key={key}>
                  <span>{label}</span>
                  <code className="ip-code">{draft.readOnly[key] || '已停用'}</code>
                  <button
                    type="button"
                    className={`btn sm${recorder === key ? ' goldline' : ''}`}
                    disabled={recorder !== null && recorder !== key}
                    onClick={() => {
                      setRecordPreview('');
                      setRecorder(recorder === key ? null : key);
                    }}
                  >
                    {recorder === key ? (recordPreview ? `录制中 ${recordPreview}` : '按下任意键…') : '录制'}
                  </button>
                  <Switch
                    on={!!draft.readOnly[key]}
                    onChange={(v) => toggleCombo(key, v)}
                    title={draft.readOnly[key] ? '点击停用该功能' : '点击启用（恢复上次键位）'}
                  />
                </div>
              ))}
            </div>
            <div className="ip-note">
              单键或组合都可以：单击 Alt 录成单键「alt」，按住 Ctrl+Shift 再松开录成「ctrl+shift」，普通键（如 Tab）单击即录。
              开关关闭 = 该功能停用（键位保留，重开即恢复）。录制后点「保存并应用」才写入 config；Esc 取消，Backspace 清空。
            </div>
          </Card>
        </div>
      </div>

      <div className="ip-foot">
        <span className={validationError && dirty ? 'ip-foot-warn' : ''}>
          {validationError && dirty
            ? validationError
            : dirty
              ? `${diff} 项修改尚未保存`
              : '所有修改已保存并生效'}
        </span>
        <button type="button" className="btn outline" disabled={!dirty || saving} onClick={discard}>
          撤销
        </button>
        <button type="button" className="btn primary" disabled={!dirty || saving} onClick={save}>
          <Icon name="check" />
          {saving ? '保存中…' : '保存并应用'}
        </button>
      </div>
    </>
  );
}
