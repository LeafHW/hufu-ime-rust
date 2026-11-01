import { useEffect, useMemo, useRef, useState } from 'react';
import type { Bootstrap, Config } from '../types';
import { Icon } from '../components/Icon';
import { Badge, PageHead } from '../components/ui';
import '../styles/keymap.css';

import { fetchWithTimeout } from '../api';
/**
 * 键位编辑器 —— 对应旧 settings-ui「自定义按键（全键盘 · 两态映射）」窗口。
 *
 * 真源：GET /api/config 的 keymap.map / keymap.map_idle /
 *       keymap.shift_symbols / keymap.shift_letters_always，
 * 保存：POST /api/config（先 GET 最新配置，只合并 keymap 字段）。
 *
 * 键名与 server parse_key、engine keymap_key_name 同口径：
 *   单字符原样；backspace/tab/enter/space/capslock/shiftleft/shiftright/
 *   pageup/pagedown/home/end/delete/up/down/left/right/f1..f12；
 *   小键盘 num0..num9 / numadd / numsub / nummul / numdiv / numdot / numenter。
 * 功能 id 与 engine run_keymap 同口径：
 *   select1..select10 / repeat / repeat2..4 / undo / clear / pageup /
 *   pagedown / switch / top / s2t / emoji / text:<字串>。
 */

type Dict = Record<string, unknown>;
type PageId = 'main' | 'num' | 'shift';

const asDict = (v: unknown): Dict =>
  v && typeof v === 'object' && !Array.isArray(v) ? (v as Dict) : {};
const str = (v: unknown, d = '') => (typeof v === 'string' ? v : d);
const bool = (v: unknown, d = false) => (typeof v === 'boolean' ? v : d);
const strMap = (v: unknown): Record<string, string> => {
  const src = asDict(v);
  const out: Record<string, string> = {};
  Object.keys(src).forEach((k) => {
    if (typeof src[k] === 'string') out[k] = src[k] as string;
  });
  return out;
};
const strList = (v: unknown): string[] =>
  Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
const hasOwn = (o: object, k: string) => Object.prototype.hasOwnProperty.call(o, k);

interface KeymapDraft {
  map: Record<string, string>;
  map_idle: Record<string, string>;
  shift_symbols: Record<string, string>;
  shift_letters_always: string[];
}

function normalizeKeymap(cfg: Config): KeymapDraft {
  const km = asDict(cfg.keymap);
  return {
    map: strMap(km.map),
    map_idle: strMap(km.map_idle),
    shift_symbols: strMap(km.shift_symbols),
    shift_letters_always: Array.from(new Set(strList(km.shift_letters_always))),
  };
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

/* ── 键帽 ───────────────────────────────────────────────────────────── */

interface KeyCap {
  cap: string;
  name?: string;
  w?: number;
  ro?: boolean;
  hint?: string;
}
interface KeyRow {
  keys: KeyCap[];
}
const k = (cap: string, name?: string, opts: { w?: number; ro?: boolean; hint?: string } = {}): KeyCap => ({
  cap,
  name,
  w: opts.w ?? 1,
  ro: opts.ro,
  hint: opts.hint,
});

const MAIN_ROWS: KeyRow[] = [
  {
    keys: [
      k('Esc', undefined, { ro: true, hint: 'Esc 固定双行为（编码中清屏 / 空态撤回），不可映射' }),
      k('`', '`', { hint: '反引号：常被反查引导/超集编码占用，映射前先确认冲突' }),
      ...Array.from('1234567890', (c) => k(c, c)),
      k('-', '-'),
      k('=', '='),
      k('Backspace', 'backspace', { w: 2 }),
    ],
  },
  {
    keys: [
      k('Tab', 'tab', { w: 1.5 }),
      ...Array.from('qwertyuiop', (c) => k(c, c)),
      k('[', '['),
      k(']', ']'),
      k('\\', '\\', { w: 1.5 }),
    ],
  },
  {
    keys: [
      k('Caps', 'capslock', { w: 1.8, hint: 'Caps 固定由「输入 → Caps 行为」控制，也可在此映射覆盖' }),
      ...Array.from('asdfghjkl', (c) => k(c, c)),
      k(';', ';'),
      k("'", "'"),
      k('Enter', 'enter', { w: 2.2 }),
    ],
  },
  {
    keys: [
      k('LShift', 'shiftleft', { w: 2.4, hint: '左右 Shift 独立：映射后该侧单击不再切换中英' }),
      ...Array.from('zxcvbnm', (c) => k(c, c)),
      k(',', ','),
      k('.', '.'),
      k('/', '/'),
      k('RShift', 'shiftright', { w: 2.4, hint: '左右 Shift 独立：映射后该侧单击不再切换中英' }),
    ],
  },
  {
    keys: [
      k('Ctrl', undefined, { w: 1.2, ro: true, hint: 'Ctrl 组合是应用快捷键，不可映射' }),
      k('Win', undefined, { w: 1.2, ro: true, hint: 'Win 组合是系统快捷键，不可映射' }),
      k('Alt', undefined, { w: 1.2, ro: true, hint: 'Alt 组合是应用快捷键，不可映射' }),
      k('空格', 'space', { w: 5.6 }),
      k('Alt', undefined, { w: 1.2, ro: true }),
      k('Win', undefined, { w: 1.2, ro: true }),
      k('Ctrl', undefined, { w: 1.2, ro: true }),
    ],
  },
  {
    keys: [
      k('↑', 'up'),
      k('↓', 'down'),
      k('←', 'left'),
      k('→', 'right'),
      k('PageUp', 'pageup'),
      k('PageDown', 'pagedown'),
      k('Home', 'home'),
      k('End', 'end'),
      k('Delete', 'delete'),
    ],
  },
  {
    keys: Array.from({ length: 12 }, (_, i) => k(`F${i + 1}`, `f${i + 1}`)),
  },
];

const NUM_ROWS: KeyRow[] = [
  {
    keys: [
      k('NumLock', undefined, { ro: true, hint: 'NumLock 决定小键盘发数字还是方向，不可映射' }),
      k('Num /', 'numdiv'),
      k('Num *', 'nummul'),
      k('Num -', 'numsub'),
    ],
  },
  {
    keys: [k('Num 7', 'num7'), k('Num 8', 'num8'), k('Num 9', 'num9'), k('Num +', 'numadd')],
  },
  {
    keys: [k('Num 4', 'num4'), k('Num 5', 'num5'), k('Num 6', 'num6'), k('', undefined)],
  },
  {
    keys: [k('Num 1', 'num1'), k('Num 2', 'num2'), k('Num 3', 'num3'), k('Num回车', 'numenter')],
  },
  {
    keys: [k('Num 0', 'num0', { w: 2 }), k('', undefined), k('Num .', 'numdot'), k('', undefined)],
  },
];

const SHIFT_ROWS: KeyRow[] = [
  { keys: Array.from('`1234567890-=', (c) => k(c, c)) },
  { keys: [...Array.from('qwertyuiop', (c) => k(c, c)), k('[', '['), k(']', ']'), k('\\', '\\')] },
  { keys: [...Array.from('asdfghjkl', (c) => k(c, c)), k(';', ';'), k("'", "'")] },
  { keys: [...Array.from('zxcvbnm', (c) => k(c, c)), k(',', ','), k('.', '.'), k('/', '/')] },
];

const PAGE_TITLE: Record<PageId, string> = { main: '主键盘', num: '小键盘', shift: 'Shift+符号' };
const PAGE_ROWS: Record<Exclude<PageId, 'shift'>, KeyRow[]> = { main: MAIN_ROWS, num: NUM_ROWS };

function pageNames(page: Exclude<PageId, 'shift'>): string[] {
  const out: string[] = [];
  PAGE_ROWS[page].forEach((row) => row.keys.forEach((key) => key.name && out.push(key.name)));
  return Array.from(new Set(out));
}

function assignedOnPage(d: KeymapDraft, page: Exclude<PageId, 'shift'>): number {
  return pageNames(page).filter((name) => d.map[name] || d.map_idle[name]).length;
}

/* ── 功能目录（与 engine run_keymap 同口径）────────────────────────── */

interface FnOption {
  id: string;
  label: string;
}

const FUNCS: FnOption[] = [
  { id: '', label: '（清除映射 · 恢复该键默认行为）' },
  ...Array.from({ length: 10 }, (_, i) => ({
    id: `select${i + 1}`,
    label: `第 ${i + 1} 选重键${i === 0 ? '（首选）' : ''}`,
  })),
  { id: 'repeat', label: '重复上屏（最近 1 条）' },
  { id: 'repeat2', label: '重复上屏（倒数第 2 条）' },
  { id: 'repeat3', label: '重复上屏（倒数第 3 条）' },
  { id: 'repeat4', label: '重复上屏（倒数第 4 条）' },
  { id: 'undo', label: '撤回上次上屏（回删）' },
  { id: 'clear', label: '清屏（清编码与候选）' },
  { id: 'pageup', label: '候选上一页' },
  { id: 'pagedown', label: '候选下一页' },
  { id: 'switch', label: '切换中英文' },
  { id: 'top', label: '顶屏（上屏当前高亮候选）' },
  { id: 's2t', label: '简繁转换开关（按一下切换）' },
  { id: 'emoji', label: 'Emoji 注解候选开关（按一下切换）' },
];

const IDLE_FUNCS: FnOption[] = [
  { id: '__same__', label: '（跟随有候选时的设置）' },
  { id: '', label: '（清除 · 无候选走默认行为）' },
  { id: 'repeat', label: '重复上屏（最近 1 条）' },
  { id: 'repeat2', label: '重复上屏（倒数第 2 条）' },
  { id: 'repeat3', label: '重复上屏（倒数第 3 条）' },
  { id: 'repeat4', label: '重复上屏（倒数第 4 条）' },
  { id: 'undo', label: '撤回上次上屏（回删）' },
  { id: 'switch', label: '切换中英文' },
  { id: 's2t', label: '简繁转换开关（按一下切换）' },
  { id: 'emoji', label: 'Emoji 注解候选开关（按一下切换）' },
];

function fullLabel(id: string): string {
  if (!id) return '（默认）';
  if (id.startsWith('text:')) return `直出「${id.slice(5)}」`;
  return (
    FUNCS.find((f) => f.id === id)?.label ||
    IDLE_FUNCS.find((f) => f.id === id)?.label ||
    id
  );
}

function shortLabel(id: string): string {
  if (!id) return '';
  if (id.startsWith('text:')) {
    const t = id.slice(5);
    return `“${t.length > 4 ? `${t.slice(0, 4)}…` : t}”`;
  }
  if (/^select\d+$/.test(id)) return `选${id.slice(6)}`;
  const map: Record<string, string> = {
    repeat: '重复',
    repeat2: '重复2',
    repeat3: '重复3',
    repeat4: '重复4',
    undo: '撤回',
    clear: '清屏',
    pageup: '上页',
    pagedown: '下页',
    switch: '中英',
    top: '顶屏',
    s2t: '简繁',
    emoji: 'Emoji',
  };
  return map[id] ?? id;
}

function truncate(s: string, n: number): string {
  return s.length > n ? `${s.slice(0, n)}…` : s;
}

/* ── 冲突检测（旧页口径）──────────────────────────────────────────── */

interface ConflictCtx {
  alphabet: string;
  paging: string;
  second: string;
  third: string;
  customSelect: string[];
  reverseEnabled: boolean;
}

function computeConflicts(d: KeymapDraft, ctx: ConflictCtx): string[] {
  const out: string[] = [];
  const keys = new Set([...Object.keys(d.map), ...Object.keys(d.map_idle)]);
  const seen = new Map<string, string>();
  keys.forEach((key) => {
    const act = d.map[key] || (hasOwn(d.map_idle, key) ? d.map_idle[key] : '');
    if (!act) return;
    const prev = seen.get(act);
    if (prev) out.push(`同一功能「${fullLabel(act)}」同时绑在「${prev}」与「${key}」上`);
    else seen.set(act, key);

    const idx = ctx.customSelect.indexOf(key);
    if (idx >= 0) out.push(`「${key}」同时是第 ${idx + 1} 选重键（内建选重优先）`);
    const isText =
      (d.map[key] || '').startsWith('text:') || (d.map_idle[key] || '').startsWith('text:');
    if (ctx.alphabet.includes(key) && !isText) {
      out.push(`「${key}」在编码字母表里——编码延续时优先当编码字符，映射只在无延续时生效`);
    }
    if (key === ctx.second || key === ctx.third) out.push(`「${key}」是次选/三选键（内建选重优先）`);
    if (ctx.paging.includes(key)) out.push(`「${key}」是翻页键——功能映射优先会令翻页失效`);
    if (key === '`' && ctx.reverseEnabled) out.push('「`」是反查引导键——反查开启时引导优先');
    if (key === 'shiftleft' || key === 'shiftright') {
      out.push(
        `「${key === 'shiftleft' ? '左 Shift' : '右 Shift'}」已映射：该侧单击不再切换中英（清映射即恢复）`,
      );
    }
  });
  return out;
}

/* ── 键帽按钮 ─────────────────────────────────────────────────────── */

function KeyButton({
  cap,
  draft,
  page,
  disabled,
  onOpen,
  onShift,
}: {
  cap: KeyCap;
  draft: KeymapDraft;
  page: PageId;
  disabled: boolean;
  onOpen: (name: string, cap: string) => void;
  onShift: (base: string) => void;
}) {
  if (!cap.cap) {
    return <div className="km-key blank" style={{ flexGrow: cap.w ?? 1 }} aria-hidden="true" />;
  }

  if (page === 'shift') {
    const base = cap.name || cap.cap;
    const isLetter = /^[a-z]$/.test(base);
    const storeKey = isLetter ? base.toUpperCase() : base;
    const custom = draft.shift_symbols[storeKey] || '';
    const always = isLetter && draft.shift_letters_always.includes(storeKey);
    const title =
      `Shift+${base}` +
      (custom ? ` 自定义直出「${custom}」` : ' 使用内置输出') +
      (isLetter ? (always ? ' · 都生效' : ' · 只在有候选时') : '');
    return (
      <button
        type="button"
        className={`km-key km-shift${custom ? ' assigned' : ''}`}
        style={{ flexGrow: cap.w ?? 1 }}
        title={title}
        aria-label={`Shift+${base}`}
        disabled={disabled}
        onClick={() => onShift(base)}
      >
        <span className="km-cap">⇧+{base}</span>
        {custom ? <span className="km-fn">{truncate(custom, 6)}</span> : null}
        {always ? <span className="km-idle">全域</span> : null}
      </button>
    );
  }

  const name = cap.name || '';
  const act = name ? draft.map[name] || '' : '';
  const hasIdle = name ? hasOwn(draft.map_idle, name) : false;
  const idle = hasIdle ? draft.map_idle[name] || '' : '';
  const cls = [
    'km-key',
    act ? 'assigned' : '',
    !act && hasIdle && idle ? 'idle-only' : '',
    cap.ro || !name ? 'readonly' : '',
  ]
    .filter(Boolean)
    .join(' ');
  const title = cap.hint
    ? cap.hint
    : !name
      ? cap.cap
      : `按键「${cap.cap}」${act ? ` · 有候选：${fullLabel(act)}` : ''}${
          hasIdle ? ` · 无候选：${idle ? fullLabel(idle) : '默认行为'}` : ' · 无候选：跟随有候选'
        }`;
  return (
    <button
      type="button"
      className={cls}
      style={{ flexGrow: cap.w ?? 1 }}
      title={title}
      aria-label={cap.cap}
      disabled={disabled || cap.ro || !name}
      onClick={() => name && onOpen(name, cap.cap)}
    >
      <span className="km-cap">{cap.cap}</span>
      {act ? <span className="km-fn">{shortLabel(act)}</span> : null}
      {hasIdle && idle ? <span className="km-idle">{shortLabel(idle)}</span> : null}
    </button>
  );
}

/* ── 功能设置弹窗 ─────────────────────────────────────────────────── */

function KeyDialog({
  cap,
  name,
  act,
  idle,
  conflicts,
  saving,
  onCancel,
  onApply,
}: {
  cap: string;
  name: string;
  act: string;
  idle: string;
  conflicts: string[];
  saving: boolean;
  onCancel: () => void;
  onApply: (act: string, idle: string) => void;
}) {
  const initialCustom = act.startsWith('text:') ? act.slice(5) : '';
  const [actSel, setActSel] = useState(initialCustom ? '' : act);
  const [actText, setActText] = useState(initialCustom);
  const follow = idle === '__same__';
  const idleCustom = !follow && idle.startsWith('text:') ? idle.slice(5) : '';
  const [idleSel, setIdleSel] = useState(follow ? '__same__' : idleCustom ? '' : idle);
  const [idleText, setIdleText] = useState(idleCustom);

  /* RT-05：Esc 关闭弹层。 */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        /* 保存中不许关：关掉就会连刚填的值一起丢，失败后还得重打 */
        if (saving) return;
        onCancel();
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [onCancel, saving]);

  const actOptions =
    actSel && !FUNCS.some((f) => f.id === actSel)
      ? [...FUNCS, { id: actSel, label: actSel }]
      : FUNCS;
  const idleOptions =
    idleSel && idleSel !== '__same__' && !IDLE_FUNCS.some((f) => f.id === idleSel)
      ? [...IDLE_FUNCS, { id: idleSel, label: idleSel }]
      : IDLE_FUNCS;

  return (
    <div
      className="km-modal-backdrop"
      onMouseDown={(e) => {
        /* 保存中点背板不关闭（同 Esc）：别把在途保存的输入一起取消掉 */
        if (!saving && e.target === e.currentTarget) onCancel();
      }}
    >
      <div className="km-modal" role="dialog" aria-modal="true" aria-label={`按键 ${cap} 设置`}>
        <h3>
          按键「{cap}」<span className="km-modal-keyname">{name}</span>
        </h3>
        <div className="sub">
          功能仅在中文输入态、无 Shift/Ctrl/Alt 组合时生效；编码延续中的字母键仍优先当编码字符。
        </div>

        <div className="km-modal-sec first">
          <div className="km-modal-sec-title">
            <span className="dot" /> 有候选时（打字中）
          </div>
          <div className="km-modal-row">
            <label htmlFor="km-act">功能</label>
            <select
              id="km-act"
              className="km-select"
              value={actSel}
              onChange={(e) => {
                setActSel(e.target.value);
                if (e.target.value) setActText('');
              }}
            >
              {actOptions.map((f) => (
                <option key={f.id || '__clear'} value={f.id}>
                  {f.label}
                </option>
              ))}
            </select>
          </div>
          <div className="km-modal-row km-modal-custom">
            <label htmlFor="km-act-text">自定义直出</label>
            <input
              id="km-act-text"
              className="km-input"
              type="text"
              value={actText}
              placeholder="如：？ 或 、（填写后覆盖上方选择）"
              onChange={(e) => {
                setActText(e.target.value);
                if (e.target.value) setActSel('');
              }}
            />
          </div>
        </div>

        <div className="km-modal-sec">
          <div className="km-modal-sec-title">
            <span className="dot gold" /> 无候选时（空态）
          </div>
          <div className="km-modal-row">
            <label htmlFor="km-idle">功能</label>
            <select
              id="km-idle"
              className="km-select"
              value={idleSel}
              onChange={(e) => {
                setIdleSel(e.target.value);
                if (e.target.value) setIdleText('');
              }}
            >
              {idleOptions.map((f) => (
                <option key={f.id || '__clear'} value={f.id}>
                  {f.label}
                </option>
              ))}
            </select>
          </div>
          <div className="km-modal-row km-modal-custom">
            <label htmlFor="km-idle-text">自定义直出</label>
            <input
              id="km-idle-text"
              className="km-input"
              type="text"
              value={idleText}
              placeholder="如：！ 或 、（填写后覆盖上方选择）"
              onChange={(e) => {
                setIdleText(e.target.value);
                if (e.target.value) setIdleSel('');
              }}
            />
          </div>
        </div>

        {conflicts.length ? (
          <div className="km-modal-cf">
            {conflicts.map((c, i) => (
              <div key={i}>⚠ {c}</div>
            ))}
          </div>
        ) : null}

        <div className="km-modal-actions">
          <button type="button" className="btn outline" onClick={onCancel}>
            取消
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={saving}
            onClick={() => onApply(actText ? `text:${actText}` : actSel, idleText ? `text:${idleText}` : idleSel)}
          >
            {saving ? '保存中…' : '保存并应用'}
          </button>
        </div>
      </div>
    </div>
  );
}

/* ── Shift+符号弹窗 ────────────────────────────────────────────────── */

interface ShiftTarget {
  base: string;
  key: string;
  isLetter: boolean;
  builtin: string;
  loading: boolean;
}

function ShiftDialog({
  target,
  initial,
  mode,
  saving,
  onCancel,
  onApply,
}: {
  target: ShiftTarget;
  initial: string;
  mode: 'compose' | 'always';
  saving: boolean;
  onCancel: () => void;
  onApply: (value: string, mode: 'compose' | 'always') => void;
}) {
  const [value, setValue] = useState(initial);
  const [scope, setScope] = useState<'compose' | 'always'>(mode);

  /* RT-05：Esc 关闭弹层。 */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        /* 保存中不许关：关掉就会连刚填的值一起丢，失败后还得重打 */
        if (saving) return;
        onCancel();
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [onCancel, saving]);

  return (
    <div
      className="km-modal-backdrop"
      onMouseDown={(e) => {
        /* 保存中点背板不关闭（同 Esc）：别把在途保存的输入一起取消掉 */
        if (!saving && e.target === e.currentTarget) onCancel();
      }}
    >
      <div className="km-modal" role="dialog" aria-modal="true" aria-label={`Shift+${target.base} 设置`}>
        <h3>
          Shift+{target.base}
          {target.isLetter ? <span className="km-modal-keyname">字母键</span> : null}
        </h3>
        <div className="sub">
          {target.isLetter
            ? '字母键可设「只在有候选时」（空态=正常大写字母）或「都生效」（空态直接出串）。'
            : '有候选时=候选先顶屏 + 此串；空态=直接出此串。'}
        </div>

        <div className="km-builtin">
          内置默认：<b>{target.loading ? '读取中…' : target.builtin || '—'}</b>
          {initial ? (
            <>
              　· 当前自定义：<b>{initial}</b>
            </>
          ) : null}
        </div>

        <div className="km-modal-row">
          <label htmlFor="km-shift-text">替换为</label>
          <input
            id="km-shift-text"
            className="km-input"
            type="text"
            value={value}
            placeholder="留空 = 恢复内置输出"
            autoFocus
            onChange={(e) => setValue(e.target.value)}
          />
        </div>

        {target.isLetter ? (
          <div className="km-radio">
            <label>
              <input
                type="radio"
                name="km-scope"
                checked={scope === 'compose'}
                onChange={() => setScope('compose')}
              />
              只在有候选时
            </label>
            <label>
              <input
                type="radio"
                name="km-scope"
                checked={scope === 'always'}
                onChange={() => setScope('always')}
              />
              都生效（空态也出串）
            </label>
          </div>
        ) : null}

        <div className="km-modal-actions">
          <button type="button" className="btn outline" onClick={onCancel}>
            取消
          </button>
          {initial ? (
            <button type="button" className="btn ghost" disabled={saving} onClick={() => onApply('', scope)}>
              恢复内置
            </button>
          ) : null}
          <button type="button" className="btn primary" disabled={saving} onClick={() => onApply(value, scope)}>
            {saving ? '保存中…' : '保存并应用'}
          </button>
        </div>
      </div>
    </div>
  );
}

/* ── 页面 ───────────────────────────────────────────────────────────── */

export function KeymapPage({ data, notify }: { data: Bootstrap; notify: (msg: string) => void }) {
  /* 审计 2026-11-01：draft/saved 恒同值（所有编辑入口都即时 persist），
     头部「撤销/保存并应用/未保存徽标」与横幅为死控件，已移除；
     弹窗内保存失败时 notify 报错、编辑不落 draft（重试入口=重新编辑）。 */
  const [draft, setDraft] = useState<KeymapDraft>(() => normalizeKeymap(data.config));
  const [page, setPage] = useState<PageId>('main');
  const [saving, setSaving] = useState(false);
  const [editing, setEditing] = useState<{ name: string; cap: string } | null>(null);
  const [shiftTarget, setShiftTarget] = useState<ShiftTarget | null>(null);
  const [confirmReset, setConfirmReset] = useState<PageId | null>(null);
  const ignorePropsUntil = useRef(0);

  const propKey = useMemo(() => JSON.stringify(normalizeKeymap(data.config)), [data.config]);

  useEffect(() => {
    let alive = true;
    getConfig()
      .then((cfg) => {
        if (!alive) return;
        setDraft(normalizeKeymap(cfg));
      })
      .catch(() => {
        /* 离线预览：保留 Bootstrap 值，保存时仍会重新 GET */
      });
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    if (!data.online) return;
    /* 保存后 15s 内忽略在途的旧 Bootstrap 轮询。 */
    if (Date.now() < ignorePropsUntil.current) return;
    setDraft(normalizeKeymap(data.config));
  }, [propKey, data.online]);

  const cfg = data.config;
  const inputCfg = asDict(cfg.input);
  const candCfg = asDict(cfg.candidates);
  const reverseCfg = asDict(cfg.reverse);
  const generalCfg = asDict(cfg.general);
  const ctx: ConflictCtx = {
    alphabet: str(inputCfg.alphabet),
    paging: str(candCfg.paging_keys, '-='),
    second: str(candCfg.second_select, ';'),
    third: str(candCfg.third_select, "'"),
    customSelect: strList(candCfg.custom_select_keys),
    reverseEnabled: bool(reverseCfg.enabled, true),
  };
  const conflicts = useMemo(() => computeConflicts(draft, ctx), [
    draft,
    ctx.alphabet,
    ctx.paging,
    ctx.second,
    ctx.third,
    ctx.customSelect.join('\u0000'),
    ctx.reverseEnabled,
  ]);

  const mappedKeys = useMemo(
    () =>
      Array.from(new Set([...Object.keys(draft.map), ...Object.keys(draft.map_idle)])).filter(
        (key) => draft.map[key] || draft.map_idle[key],
      ),
    [draft],
  );
  const mappedCount = mappedKeys.length;

  const entries = useMemo(
    () =>
      mappedKeys
        .slice()
        .sort((a, b) => a.localeCompare(b, 'zh-CN'))
        .map((key) => ({
          key,
          act: draft.map[key] || '',
          hasIdle: hasOwn(draft.map_idle, key),
          idle: draft.map_idle[key] || '',
        })),
    [draft, mappedKeys],
  );

  /* 返回是否写入成功：调用方要等这一票再关弹窗。此前 applyKey/applyShift 先
     setEditing(null) 再 await，保存失败时弹窗已关、用户刚输入的新值一起消失，
     只能在报错后再从头打一遍。 */
  const persist = async (next: KeymapDraft, okMsg: string): Promise<boolean> => {
    setSaving(true);
    try {
      const fresh = await getConfig();
      const base: Dict = { ...(fresh as unknown as Dict) };
      const freshKm = asDict(fresh.keymap);
      base.keymap = {
        ...freshKm,
        map: next.map,
        map_idle: next.map_idle,
        shift_symbols: next.shift_symbols,
        shift_letters_always: next.shift_letters_always,
      };
      await postConfig(base);
      setDraft(next);
      ignorePropsUntil.current = Date.now() + 15000;
      notify(okMsg);
      return true;
    } catch (e) {
      notify(`保存失败：${(e as Error).message}`);
      return false;
    } finally {
      setSaving(false);
    }
  };

  const openKey = (name: string, cap: string) => {
    if (saving) return;
    setConfirmReset(null);
    setEditing({ name, cap });
  };

  const applyKey = async (name: string, cap: string, act: string, idle: string) => {
    const next: KeymapDraft = {
      ...draft,
      map: { ...draft.map },
      map_idle: { ...draft.map_idle },
    };
    if (act) next.map[name] = act;
    else delete next.map[name];
    if (idle === '__same__') delete next.map_idle[name];
    else next.map_idle[name] = idle;
    /* 成功才关弹窗：失败时保留弹窗与用户刚填的值，可直接重试。
       弹窗留着也才渲染得到 saving 分支（按钮 disabled + 「保存中…」）。 */
    const ok = await persist(next, `按键「${cap}」已保存并应用`);
    if (ok) setEditing(null);
  };

  const openShift = async (base: string) => {
    if (saving) return;
    setConfirmReset(null);
    const isLetter = /^[a-z]$/.test(base);
    const key = isLetter ? base.toUpperCase() : base;
    setShiftTarget({ base, key, isLetter, builtin: '', loading: true });
    try {
      const r = await getJson<{ builtin?: unknown; custom?: unknown }>(
        `/api/shift_symbol?base=${encodeURIComponent(base)}`,
      );
      setShiftTarget((prev) =>
        prev && prev.key === key ? { ...prev, builtin: str(r.builtin, ''), loading: false } : prev,
      );
    } catch {
      setShiftTarget((prev) => (prev && prev.key === key ? { ...prev, loading: false } : prev));
    }
  };

  const applyShift = async (value: string, mode: 'compose' | 'always') => {
    if (!shiftTarget) return;
    const target = shiftTarget;
    const next: KeymapDraft = {
      ...draft,
      shift_symbols: { ...draft.shift_symbols },
      shift_letters_always: [...draft.shift_letters_always],
    };
    if (value) next.shift_symbols[target.key] = value;
    else delete next.shift_symbols[target.key];
    next.shift_letters_always = next.shift_letters_always.filter((key) => key !== target.key);
    if (value && target.isLetter && mode === 'always') next.shift_letters_always.push(target.key);
    if (target.isLetter) {
      try {
        /* 与旧设置页同一 localStorage 键：字母档位偏好在两代设置窗之间延续。 */
        localStorage.setItem('hufuSSLetterMode', mode);
      } catch {
        /* 存储被禁用时忽略 */
      }
    }
    /* 同 applyKey：保存失败要保留弹窗和新值（失败先把弹窗关掉=用户输入白填） */
    const ok = await persist(next, value ? `Shift+${target.base} 已保存并应用` : `Shift+${target.base} 已恢复内置`);
    if (ok) setShiftTarget(null);
  };

  const askReset = (targetPage: PageId) => {
    const count =
      targetPage === 'shift' ? Object.keys(draft.shift_symbols).length : assignedOnPage(draft, targetPage);
    if (count === 0) {
      notify(`「${PAGE_TITLE[targetPage]}」已经是默认（无自定义）`);
      return;
    }
    setConfirmReset(targetPage);
  };

  const doReset = async (targetPage: PageId) => {
    const next: KeymapDraft = {
      ...draft,
      map: { ...draft.map },
      map_idle: { ...draft.map_idle },
      shift_symbols: { ...draft.shift_symbols },
      shift_letters_always: [...draft.shift_letters_always],
    };
    if (targetPage === 'shift') {
      next.shift_symbols = {};
      next.shift_letters_always = [];
    } else {
      pageNames(targetPage).forEach((name) => {
        delete next.map[name];
        delete next.map_idle[name];
      });
      delete next.map.escape;
      delete next.map_idle.escape;
    }
    /* 与上面两处同类：失败时保留确认条，避免「提示失败但条目已消失」的假象 */
    const ok = await persist(next, `已恢复「${PAGE_TITLE[targetPage]}」默认并应用`);
    if (ok) setConfirmReset(null);
  };

  const shiftInitial = shiftTarget ? draft.shift_symbols[shiftTarget.key] || '' : '';
  const shiftMode: 'compose' | 'always' = (() => {
    if (!shiftTarget) return 'compose';
    if (draft.shift_letters_always.includes(shiftTarget.key)) return 'always';
    try {
      return localStorage.getItem('hufuSSLetterMode') === 'always' ? 'always' : 'compose';
    } catch {
      return 'compose';
    }
  })();

  const rows =
    page === 'shift' ? SHIFT_ROWS : page === 'num' ? NUM_ROWS : MAIN_ROWS;
  const pageAssigned =
    page === 'shift' ? Object.keys(draft.shift_symbols).length : assignedOnPage(draft, page);
  const pageNote =
    page === 'shift'
      ? `${pageAssigned} 项自定义 · 字母可设「只在有候选时 / 都生效」`
      : `${pageAssigned} 键有自定义 · 键名与引擎 parse_key 同口径`;
  const resetCount = confirmReset
    ? confirmReset === 'shift'
      ? Object.keys(draft.shift_symbols).length
      : assignedOnPage(draft, confirmReset)
    : 0;

  const editAct = editing ? draft.map[editing.name] || '' : '';
  const editIdle =
    editing && hasOwn(draft.map_idle, editing.name) ? draft.map_idle[editing.name] || '' : '__same__';
  const editConflicts = editing
    ? conflicts.filter((c) => c.includes(`「${editing.name}」`))
    : [];

  return (
    <>
      <PageHead
        title="键位编辑"
        sub={
          <>
            <span className={data.online ? 'dot' : 'dot muted'} />
            {data.online ? '引擎已连接' : '离线预览'}
            <span style={{ color: 'var(--text3)' }}>·</span>
            全键盘两态映射
            <span style={{ color: 'var(--text3)' }}>·</span>
            {mappedCount} 键自定义
            <span style={{ color: 'var(--text3)' }}>·</span>
            {conflicts.length ? `${conflicts.length} 条冲突提示` : '无冲突'}
          </>
        }
      />

      <section className="card km-kb-card">
        <div className="km-toolbar">
          <div className="km-tabs" role="tablist" aria-label="键盘分页">
            {(['main', 'num', 'shift'] as PageId[]).map((p) => (
              <button
                key={p}
                type="button"
                role="tab"
                aria-selected={page === p}
                className={page === p ? 'on' : ''}
                onClick={() => {
                  setPage(p);
                  setConfirmReset(null);
                }}
              >
                {PAGE_TITLE[p]}
              </button>
            ))}
          </div>
          <span className="km-toolbar-spacer" />
          <span className="km-page-note">{pageNote}</span>
          <button type="button" className="btn sm outline" disabled={saving} onClick={() => askReset(page)}>
            恢复本页默认
          </button>
        </div>

        {confirmReset ? (
          <div className="km-confirm">
            <span>
              将清除「{PAGE_TITLE[confirmReset]}」本页 {resetCount} 项自定义并立即应用，其他分页保留。
            </span>
            <button type="button" className="btn sm primary" disabled={saving} onClick={() => doReset(confirmReset)}>
              确认恢复
            </button>
            <button type="button" className="btn sm ghost" onClick={() => setConfirmReset(null)}>
              取消
            </button>
          </div>
        ) : null}

        {page === 'shift' ? (
          <div className="km-shift-note">
            点亮键 = 可改 Shift 输出；点键后输入替换串。数字/符号键有候选时先顶屏再出串；字母键可选「只在有候选时 / 都生效」。
          </div>
        ) : null}

        <div className="km-kb-scroll">
          <div
            className={`km-kb${page === 'num' ? ' km-numpad' : ''}${page === 'shift' ? ' km-shift-kb' : ''}`}
          >
            {rows.map((row, ri) => (
              <div className="km-row" key={`${page}-${ri}`}>
                {row.keys.map((cap, ki) => (
                  <KeyButton
                    key={`${page}-${ri}-${ki}-${cap.name || cap.cap || 'blank'}`}
                    cap={cap}
                    draft={draft}
                    page={page}
                    disabled={saving}
                    onOpen={openKey}
                    onShift={openShift}
                  />
                ))}
              </div>
            ))}
          </div>
        </div>

        <div className="km-legend">
          <span>
            <i className="g" />
            有候选映射
          </span>
          <span>
            <i className="o" />
            仅无候选映射
          </span>
          <span>
            <i className="d" />
            默认 / 不可映射
          </span>
          <span>点击键帽编辑；Esc / Ctrl / Alt / Win / NumLock 固定不可映射</span>
        </div>
      </section>

      <div className="km-grid">
        <section className="card km-card">
          <div className="km-card-h">
            <span className="km-card-title">当前功能映射</span>
            <Badge tone="gray">{mappedCount} 键</Badge>
          </div>
          {entries.length ? (
            <div className="km-list">
              {entries.map((e) => (
                <div className="km-entry" key={e.key}>
                  <code>{e.key}</code>
                  <span className="fn">{e.act ? fullLabel(e.act) : '（有候选：默认）'}</span>
                  {e.hasIdle ? (
                    <span className="idle">空态：{e.idle ? fullLabel(e.idle) : '默认行为'}</span>
                  ) : (
                    <span className="idle follow">空态：跟随</span>
                  )}
                </div>
              ))}
            </div>
          ) : (
            <div className="km-empty">（空 = 全部按键默认行为）</div>
          )}
        </section>

        <section className="card km-card">
          <div className="km-card-h">
            <span className="km-card-title">冲突检测</span>
            <Badge tone={conflicts.length ? 'gray' : 'green'}>
              {conflicts.length ? `${conflicts.length} 条` : '通过'}
            </Badge>
          </div>
          {conflicts.length ? (
            <div className="km-cf">
              {conflicts.map((c, i) => (
                <div className="km-cf-item" key={i}>
                  <span>⚠</span>
                  <span>{c}</span>
                </div>
              ))}
            </div>
          ) : (
            <div className="km-cf-ok">✓ 未检测到冲突</div>
          )}
          <div className="km-note">
            检测口径同旧设置页：编码字母表、翻页键、次选/三选、重复功能分配、反查引导键、左右 Shift 单击切换。冲突只提示，不阻止保存。
          </div>
        </section>
      </div>

      <section className="card km-card km-coverage">
        <div className="km-card-h">
          <span className="km-card-title">覆盖范围与未覆盖项</span>
          <Badge tone="gray">真读真写</Badge>
        </div>
        <ul className="km-note-list">
          <li>
            <b>已覆盖</b>：主键盘全键 —— 反引号、数字/字母/符号、Tab/Caps/Enter/Backspace、左右 Shift 独立、空格、方向键逐个、PageUp/PageDown/Home/End/Delete、F1–F12；保存到{' '}
            <code>keymap.map</code> / <code>keymap.map_idle</code>。
          </li>
          <li>
            <b>已覆盖</b>：小键盘 num0–num9、+ - * / . 与 NumEnter（<code>num*</code> 键名，与主排数字身份分开）；NumLock 仅展示。
          </li>
          <li>
            <b>已覆盖</b>：Shift+数字/符号/字母输出 —— 保存到 <code>keymap.shift_symbols</code>，字母的「都生效」档保存到 <code>keymap.shift_letters_always</code>。
          </li>
          <li>
            <b>未覆盖</b>：调频按住键 <code>candidates.freq_key</code>、删词按住键 <code>candidates.delete_key</code>、导出码表快捷键 <code>general.export_key</code> —— 旧页为「录制」式组合键录入，本编辑器不含录制 UI，当前值只读展示：
            <span className="km-kv-inline">
              freq=<code>{str(candCfg.freq_key, 'ctrl') || '停用'}</code> · delete=
              <code>{str(candCfg.delete_key, 'ctrl+shift') || '停用'}</code> · export=
              <code>{str(generalCfg.export_key, '') || '未设置'}</code>
            </span>
          </li>
          <li>
            <b>未覆盖</b>：老配置里通用的 <code>"shift"</code> 兜底键（两侧同效）不在键盘图内，保存时原样保留；左右 Shift 独立键可编辑。
          </li>
          <li>
            <b>已下线</b>：Alt+符号页（引擎字段已删）；旧「选重键页」已删（由 select1–select10 功能覆盖）。
          </li>
          <li>
            <b>说明</b>：恢复默认按当前分页清除（主键盘 / 小键盘 / Shift+符号），不做全局一键清空，与旧页一致。
          </li>
          <li>
            <b>说明</b>：引擎 keymap 只认裸键，Shift/Ctrl/Alt/Win 组合不写入映射（组合键是应用快捷键领地）；左右 Shift 本身可映射。
          </li>
        </ul>
      </section>

      {editing ? (
        <KeyDialog
          key={editing.name}
          cap={editing.cap}
          name={editing.name}
          act={editAct}
          idle={editIdle}
          conflicts={editConflicts}
          saving={saving}
          onCancel={() => setEditing(null)}
          onApply={(act, idle) => {
            const target = editing;
            if (target) void applyKey(target.name, target.cap, act, idle);
          }}
        />
      ) : null}

      {shiftTarget ? (
        <ShiftDialog
          key={shiftTarget.key}
          target={shiftTarget}
          initial={shiftInitial}
          mode={shiftMode}
          saving={saving}
          onCancel={() => setShiftTarget(null)}
          onApply={(value, mode) => void applyShift(value, mode)}
        />
      ) : null}
    </>
  );
}
