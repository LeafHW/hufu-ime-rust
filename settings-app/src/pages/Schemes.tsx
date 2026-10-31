import { useEffect, useMemo, useRef, useState } from 'react';
import type { Bootstrap } from '../types';
import { useDirtyGuard } from '../dirtyGuard';
import { Icon } from '../components/Icon';
import { Badge, PageHead } from '../components/ui';
import '../styles/schemes.css';

/* ────────────────────────────────────────────────────────────────────────────
 * 方案页（真实读写）——数据源全部来自 hufu-server：
 *   GET  /api/schemas          码表目录下的方案名（实时列目录）
 *   GET  /api/state            当前方案 / 整句激活态
 *   GET  /api/config           schema.export_formats（导出格式记忆）
 *                              reverse.scheme / reverse.prefix /
 *                              reverse.disabled_output / candidates.split_scheme
 *   GET  /api/assets           数据\拼音反查\*.txt、数据\拆分\*.拆分
 *   POST /api/schema           { name } 切换方案
 *   POST /api/open_schema_dir  { name } 打开该方案码表文件夹
 *   POST /api/export_schema    { name, format } 导出码表（合并用户调整）
 *                              { name, format, lines_only: true } 只记忆格式
 *   POST /api/config           整份配置写回（本页只改上面两节）
 * 说明：码表根目录走 POST /api/open_dir（kind:"schema"）；
 *   单个方案的码表文件夹仍用 /api/open_schema_dir 传方案名。
 * ────────────────────────────────────────────────────────────────────────── */

interface SchemaSection {
  dir?: string;
  current?: string;
  export_formats?: Record<string, string>;
  [key: string]: unknown;
}
interface ReverseSection {
  scheme?: string;
  prefix?: string;
  disabled_output?: string;
  [key: string]: unknown;
}
interface CandidatesSection {
  split_scheme?: string;
  [key: string]: unknown;
}
interface RawConfig {
  schema?: SchemaSection;
  reverse?: ReverseSection;
  candidates?: CandidatesSection;
  [key: string]: unknown;
}

interface Form {
  reverseScheme: string;
  prefix: string;
  disabledOutput: string;
  splitScheme: string;
}

interface SchemaListResponse {
  schemas?: string[];
}
interface StateResponse {
  current_schema?: string;
  sentence_active?: boolean;
  model_present?: boolean;
}
interface AssetsResponse {
  reverse?: string[];
  split?: string[];
}
interface ExportResponse {
  ok?: boolean;
  path?: string;
  lines?: number;
  remembered?: string;
}
interface OpenDirResponse {
  ok?: boolean;
  path?: string;
}

type Status = { tone: 'ok' | 'err' | 'info'; text: string; path?: string };

/** 旧页 /api/export_schema 的六种格式（顺序与旧页下拉一致）。 */
const FORMATS = [
  { id: 'sentence', label: '虎整句', full: '虎整句格式' },
  { id: 'native', label: 'HuFu 原生', full: 'HuFu 原生' },
  { id: 'rime', label: 'Rime', full: 'Rime dict.yaml' },
  { id: 'duoduo', label: '多多', full: '多多' },
  { id: 'qq', label: 'QQ五笔', full: 'QQ五笔' },
  { id: 'robot', label: '机器人', full: '机器人专用（码<Tab>词）' },
] as const;

const formatLabel = (id: string) => FORMATS.find((f) => f.id === id)?.label ?? id;
const formatFull = (id: string) => FORMATS.find((f) => f.id === id)?.full ?? id;

function formFrom(cfg: RawConfig | null): Form {
  return {
    reverseScheme: typeof cfg?.reverse?.scheme === 'string' ? cfg.reverse.scheme : '',
    prefix: typeof cfg?.reverse?.prefix === 'string' ? cfg.reverse.prefix : '`',
    disabledOutput: typeof cfg?.reverse?.disabled_output === 'string' ? cfg.reverse.disabled_output : '·',
    splitScheme: typeof cfg?.candidates?.split_scheme === 'string' ? cfg.candidates.split_scheme : '',
  };
}

function withExportFormat(cfg: RawConfig, name: string, format: string): RawConfig {
  return {
    ...cfg,
    schema: {
      ...(cfg.schema ?? {}),
      export_formats: { ...(cfg.schema?.export_formats ?? {}), [name]: format },
    },
  };
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

async function readBody(r: Response): Promise<unknown> {
  try {
    return (await r.json()) as unknown;
  } catch {
    return null;
  }
}

function httpError(body: unknown, url: string, status: number): string {
  if (body && typeof body === 'object' && 'error' in body) {
    const msg = (body as { error?: unknown }).error;
    if (typeof msg === 'string' && msg) return msg;
  }
  return `${url} -> HTTP ${status}`;
}

async function getJson<T>(url: string): Promise<T> {
  const r = await fetch(url, { cache: 'no-store' });
  const body = await readBody(r);
  if (!r.ok) throw new Error(httpError(body, url, r.status));
  return body as T;
}

async function postJson<T>(url: string, payload: unknown): Promise<T> {
  const r = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  });
  const body = await readBody(r);
  if (!r.ok) throw new Error(httpError(body, url, r.status));
  return body as T;
}

export function SchemesPage({ data, notify }: { data: Bootstrap; notify: (msg: string) => void }) {
  const initialConfig = data.config as unknown as RawConfig;

  const [schemas, setSchemas] = useState<string[]>(() => data.state.schemas ?? []);
  const [current, setCurrent] = useState<string>(data.state.current_schema || '');
  const [sentence, setSentence] = useState<boolean>(!!data.state.sentence_active);
  const [modelPresent, setModelPresent] = useState<boolean>(!!data.state.model_present);

  const [cfg, setCfg] = useState<RawConfig | null>(null);
  const [formats, setFormats] = useState<Record<string, string>>(
    () => initialConfig.schema?.export_formats ?? {},
  );
  const [form, setForm] = useState<Form>(() => formFrom(initialConfig));
  const [assets, setAssets] = useState<AssetsResponse>({ reverse: [], split: [] });
  const [dirty, setDirty] = useState(false);
  /* OV-B1：脏状态上报全局守卫。 */
  useDirtyGuard('schemes', '方案', dirty);

  const [filter, setFilter] = useState('');
  const [busy, setBusy] = useState('');
  const [status, setStatus] = useState<Status | null>(null);

  const busyRef = useRef(false);
  const dirtyRef = useRef(false);

  const markDirty = (next: boolean) => {
    dirtyRef.current = next;
    setDirty(next);
  };

  const fail = (text: string) => {
    setStatus({ tone: 'err', text });
    notify(text);
  };

  const applyConfig = (next: RawConfig) => {
    setCfg(next);
    setFormats(next.schema?.export_formats ?? {});
    if (!dirtyRef.current) setForm(formFrom(next));
  };

  /* App 每 10s 拉一次 /api/state：外部切换（托盘 / Ctrl+M / 另一窗口）也跟着走 */
  useEffect(() => {
    setCurrent(data.state.current_schema || '');
    setSentence(!!data.state.sentence_active);
    setModelPresent(!!data.state.model_present);
  }, [data.state.current_schema, data.state.sentence_active, data.state.model_present]);

  useEffect(() => {
    if (Array.isArray(data.state.schemas) && data.state.schemas.length) setSchemas(data.state.schemas);
  }, [data.state.schemas]);

  const refreshAll = async (announce = false) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy('load');
    try {
      const [list, st, cfgAll, assetList] = await Promise.all([
        getJson<SchemaListResponse>('/api/schemas'),
        getJson<StateResponse>('/api/state'),
        getJson<RawConfig>('/api/config'),
        getJson<AssetsResponse>('/api/assets').catch(() => ({ reverse: [], split: [] })),
      ]);
      setSchemas(list.schemas ?? []);
      setCurrent(st.current_schema ?? '');
      setSentence(!!st.sentence_active);
      if (typeof st.model_present === 'boolean') setModelPresent(st.model_present);
      setAssets({ reverse: assetList.reverse ?? [], split: assetList.split ?? [] });
      applyConfig(cfgAll);
      setStatus({ tone: 'info', text: `已刷新：${(list.schemas ?? []).length} 个方案 · 当前 ${st.current_schema || '—'}` });
      if (announce) notify('方案列表已刷新');
    } catch (e) {
      fail(`读取方案列表失败：${errText(e)}`);
    } finally {
      busyRef.current = false;
      setBusy('');
    }
  };

  useEffect(() => {
    void refreshAll();
    // 仅首挂载拉一次；之后靠「刷新列表」与 App 的 10s 轮询
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const switchSchema = async (name: string) => {
    if (busyRef.current) return;
    if (name === current) {
      notify(`「${name}」已经是当前方案`);
      return;
    }
    busyRef.current = true;
    setBusy(`schema:${name}`);
    try {
      await postJson<{ ok?: boolean; current?: string }>('/api/schema', { name });
      /* 关键（旧页同样处理）：切换后必须重取配置快照，否则之后 POST /api/config
         会把旧方案名写回，服务端视为又切回旧方案。 */
      const [st, cfgAll] = await Promise.all([
        getJson<StateResponse>('/api/state'),
        getJson<RawConfig>('/api/config'),
      ]);
      setCurrent(st.current_schema ?? name);
      setSentence(!!st.sentence_active);
      if (typeof st.model_present === 'boolean') setModelPresent(st.model_present);
      applyConfig(cfgAll);
      const extra = st.sentence_active ? '（整句已激活）' : name.includes('整句') ? '（整句未激活，检查模型）' : '';
      setStatus({ tone: 'ok', text: `已切换到「${name}」${extra}` });
      notify(`已切换到 ${name}`);
    } catch (e) {
      fail(`切换方案失败：${errText(e)}`);
    } finally {
      busyRef.current = false;
      setBusy('');
    }
  };

  const openDir = async (name: string, root = false) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(`open:${name}`);
    try {
      const r = root
        ? await postJson<OpenDirResponse>('/api/open_dir', { kind: 'schema' })
        : await postJson<OpenDirResponse>('/api/open_schema_dir', { name });
      setStatus({
        tone: 'ok',
        text: root ? '已在资源管理器打开码表根目录' : `已在资源管理器打开「${name}」的码表文件夹`,
        path: r.path,
      });
      notify(root ? '已打开码表目录' : `已打开 ${name} 的文件夹`);
    } catch (e) {
      fail(`打开${root ? '码表目录' : '方案文件夹'}失败：${errText(e)}`);
    } finally {
      busyRef.current = false;
      setBusy('');
    }
  };

  const rememberFormat = async (name: string, format: string) => {
    if (busyRef.current) return;
    const prevFormat = formats[name] ?? 'sentence';
    busyRef.current = true;
    setBusy(`fmt:${name}`);
    setFormats((p) => ({ ...p, [name]: format }));
    setCfg((p) => (p ? withExportFormat(p, name, format) : p));
    try {
      await postJson<ExportResponse>('/api/export_schema', { name, format, lines_only: true });
      setStatus({
        tone: 'info',
        text: `已记住「${name}」的导出格式：${formatFull(format)}；托盘 / 语言栏导出用同一记忆`,
      });
    } catch (e) {
      setFormats((p) => ({ ...p, [name]: prevFormat }));
      setCfg((p) => (p ? withExportFormat(p, name, prevFormat) : p));
      fail(`记忆导出格式失败：${errText(e)}`);
    } finally {
      busyRef.current = false;
      setBusy('');
    }
  };

  const exportSchema = async (name: string) => {
    if (busyRef.current) return;
    const format = formats[name] ?? 'sentence';
    busyRef.current = true;
    setBusy(`export:${name}`);
    try {
      const r = await postJson<ExportResponse>('/api/export_schema', { name, format });
      if (r.remembered) {
        setFormats((p) => ({ ...p, [name]: r.remembered as string }));
        setCfg((p) => (p ? withExportFormat(p, name, r.remembered as string) : p));
      }
      setStatus({
        tone: 'ok',
        text: `已按「${formatFull(r.remembered ?? format)}」导出 ${r.lines ?? 0} 行，已打开导出目录`,
        path: r.path,
      });
      notify(`已导出 ${r.lines ?? 0} 行码表`);
    } catch (e) {
      fail(`导出码表失败：${errText(e)}`);
    } finally {
      busyRef.current = false;
      setBusy('');
    }
  };

  const saveConfig = async () => {
    if (busyRef.current) return;
    if (!cfg) {
      fail('配置还没读取完成，请先点「刷新列表」');
      return;
    }
    if (form.prefix.length !== 1) {
      fail('反查引导前缀必须是 1 个字符');
      return;
    }
    busyRef.current = true;
    setBusy('save');
    try {
      /* 落盘前重取最新整份配置：本页可能停留很久，期间别的页/别的窗口
         可能改过皮肤、按键等——用旧快照写回会把它们洗掉；只补本页两节。 */
      const base = await getJson<RawConfig>('/api/config');
      const next: RawConfig = {
        ...base,
        reverse: {
          ...(base.reverse ?? {}),
          scheme: form.reverseScheme,
          prefix: form.prefix,
          disabled_output: form.disabledOutput,
        },
        candidates: {
          ...(base.candidates ?? {}),
          split_scheme: form.splitScheme,
        },
      };
      await postJson<{ ok?: boolean }>('/api/config', next);
      markDirty(false);
      const fresh = await getJson<RawConfig>('/api/config');
      applyConfig(fresh);
      setStatus({ tone: 'ok', text: '反查 / 拆分设置已写入 config.json（POST /api/config）' });
      notify('反查 / 拆分设置已保存');
    } catch (e) {
      fail(`保存失败：${errText(e)}`);
    } finally {
      busyRef.current = false;
      setBusy('');
    }
  };

  const revertForm = () => {
    setForm(formFrom(cfg));
    markDirty(false);
    setStatus({ tone: 'info', text: '已还原为磁盘上的配置值' });
  };

  const shown = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const list = q ? schemas.filter((n) => n.toLowerCase().includes(q)) : schemas;
    return [...list].sort((a, b) => a.localeCompare(b, 'zh'));
  }, [schemas, filter]);

  const reverseOptions = assets.reverse ?? [];
  const splitOptions = assets.split ?? [];
  const unknownReverse = form.reverseScheme !== '' && !reverseOptions.includes(form.reverseScheme);
  const unknownSplit = form.splitScheme !== '' && !splitOptions.includes(form.splitScheme);
  const disabledOutputKnown = ['·', '`', ''].includes(form.disabledOutput);
  const busyAny = busy !== '';

  return (
    <>
      <PageHead
        title="方案"
        sub={
          <>
            <span className={data.online ? 'dot' : 'dot muted'} />
            {schemas.length} 个方案
            <span style={{ color: 'var(--text3)' }}>·</span>
            当前 {current || '—'}
            <span style={{ color: 'var(--text3)' }}>·</span>
            {sentence ? '整句已激活' : modelPresent ? '码表模式 · 模型已安装' : '码表模式'}
          </>
        }
        actions={
          <>
            <button
              type="button"
              className="btn outline"
              onClick={() => void refreshAll(true)}
              disabled={busyAny}
            >
              <Icon name="refresh" />
              {busy === 'load' ? '刷新中…' : '刷新列表'}
            </button>
            <button
              type="button"
              className="btn ghost"
              onClick={() => void openDir('.', true)}
              disabled={busyAny}
            >
              <Icon name="folder" />
              打开码表目录
            </button>
          </>
        }
      />

      <div className="sch-layout">
        <div className="card sch-list-card">
          <div className="sch-head">
            <div className="search">
              <Icon name="search" />
              <input
                aria-label="筛选方案"
                placeholder="筛选方案…"
                value={filter}
                onChange={(e) => setFilter(e.target.value)}
              />
            </div>
            <span className="sch-count">
              显示 <b>{shown.length}</b> / {schemas.length}
            </span>
          </div>

          <div className="sch-rows">
            {shown.map((name) => {
              const isCurrent = name === current;
              const fmt = formats[name] ?? 'sentence';
              return (
                <div
                  key={name}
                  className={`sch-row${isCurrent ? ' on' : ''}`}
                  title={isCurrent ? `${name}（当前方案）` : `点击切换到「${name}」`}
                  onClick={() => void switchSchema(name)}
                >
                  <div className="sch-mark">
                    <Icon name="grid" />
                  </div>
                  <div className="sch-info">
                    <div className="sch-name">
                      <span className="sch-name-text" title={name}>
                        {name}
                      </span>
                      {name.includes('整句') ? <Badge>整句</Badge> : null}
                      {isCurrent ? (
                        <Badge tone="green" dot>
                          当前
                        </Badge>
                      ) : null}
                    </div>
                    <div className="sch-sub">
                      {isCurrent ? '当前生效 · ' : ''}
                      导出格式：{formatFull(fmt)}
                      {isCurrent && !sentence && name.includes('整句') ? ' · 整句未激活' : ''}
                    </div>
                  </div>

                  {/* 行内控件全部 stopPropagation，避免误切换 */}
                  <div className="sch-ctl" onClick={(e) => e.stopPropagation()}>
                    <select
                      className="sch-select"
                      aria-label={`${name} 的导出格式`}
                      title={`导出格式（随方案记忆；托盘导出同格式）——当前 ${formatFull(fmt)}`}
                      value={fmt}
                      disabled={busyAny}
                      onChange={(e) => void rememberFormat(name, e.target.value)}
                    >
                      {FORMATS.map((f) => (
                        <option key={f.id} value={f.id}>
                          {f.label}
                        </option>
                      ))}
                    </select>
                    <button
                      type="button"
                      className="btn sm outline"
                      title={`把用户调整（置顶 / 隐藏 / 加词 / 调频）合并进原始码表，导出到 码表导出\\${name}\\`}
                      disabled={busyAny}
                      onClick={() => void exportSchema(name)}
                    >
                      <Icon name="download" />
                      {busy === `export:${name}` ? '导出中…' : '导出'}
                    </button>
                    <button
                      type="button"
                      className="icon-btn sch-open"
                      title={`打开「${name}」的码表文件夹`}
                      aria-label={`打开 ${name} 的码表文件夹`}
                      disabled={busyAny}
                      onClick={() => void openDir(name)}
                    >
                      <Icon name="folder" />
                    </button>
                  </div>
                </div>
              );
            })}
            {shown.length === 0 ? (
              <div className="sch-empty">
                {schemas.length === 0
                  ? '码表文件夹里还没有方案目录。新建方案文件夹后点「刷新列表」即可出现。'
                  : `没有匹配「${filter}」的方案。`}
              </div>
            ) : null}
          </div>

          {status ? (
            <div className={`sch-status ${status.tone}`}>
              <span
                className={`dot ${status.tone === 'err' ? 'red' : status.tone === 'ok' ? '' : 'muted'}`}
              />
              <span className="sch-status-text" title={status.text}>
                {status.text}
              </span>
              {status.path ? (
                <code className="sch-status-path" title={status.path}>
                  {status.path}
                </code>
              ) : null}
            </div>
          ) : null}

          <div className="sch-foot">
            <span>
              当前方案：<b>{current || '—'}</b>
              {sentence ? '（整句激活）' : ''}
            </span>
            <span className="sch-foot-note">点击行切换 · 码表文件夹里新建目录后刷新即可</span>
          </div>
        </div>

        <div className="sch-side">
          <div className="card sch-cfg">
            <div className="card-head">
              <span className="card-title">反查与拆分</span>
              <Badge tone={dirty ? 'gold' : 'green'}>{dirty ? '未保存' : '已同步'}</Badge>
            </div>

            <div className="sch-field">
              <span className="label">反查方案（数据\拼音反查\*.txt）</span>
              <select
                className="sch-select block"
                aria-label="反查方案"
                value={form.reverseScheme}
                disabled={busyAny}
                onChange={(e) => {
                  setForm((p) => ({ ...p, reverseScheme: e.target.value }));
                  markDirty(true);
                }}
              >
                <option value="">（关闭反查）</option>
                {reverseOptions.map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
                {unknownReverse ? (
                  <option value={form.reverseScheme}>{form.reverseScheme}（数据\拼音反查 未找到）</option>
                ) : null}
              </select>
            </div>

            <div className="sch-duo">
              <div className="sch-field">
                <span className="label">引导前缀</span>
                <input
                  className="field-input sch-input"
                  aria-label="反查引导前缀"
                  title="反查引导前缀，默认 `；必须是 1 个字符"
                  maxLength={1}
                  value={form.prefix}
                  disabled={busyAny}
                  onChange={(e) => {
                    setForm((p) => ({ ...p, prefix: e.target.value.slice(0, 1) }));
                    markDirty(true);
                  }}
                />
              </div>
              <div className="sch-field">
                <span className="label">关闭反查时按引导键</span>
                <select
                  className="sch-select block"
                  aria-label="关闭反查时按引导键"
                  value={form.disabledOutput}
                  disabled={busyAny}
                  onChange={(e) => {
                    setForm((p) => ({ ...p, disabledOutput: e.target.value }));
                    markDirty(true);
                  }}
                >
                  <option value="·">直出「·」（默认）</option>
                  <option value="`">直出「`」</option>
                  <option value="">交给系统</option>
                  {!disabledOutputKnown ? (
                    <option value={form.disabledOutput}>当前值：{form.disabledOutput}</option>
                  ) : null}
                </select>
              </div>
            </div>

            <div className="sch-field">
              <span className="label">拆分方案（数据\拆分\*.拆分）</span>
              <select
                className="sch-select block"
                aria-label="拆分方案"
                value={form.splitScheme}
                disabled={busyAny}
                onChange={(e) => {
                  setForm((p) => ({ ...p, splitScheme: e.target.value }));
                  markDirty(true);
                }}
              >
                <option value="">（关闭拆分）</option>
                {splitOptions.map((n) => (
                  <option key={n} value={n}>
                    {n}
                  </option>
                ))}
                {unknownSplit ? (
                  <option value={form.splitScheme}>{form.splitScheme}（数据\拆分 未找到）</option>
                ) : null}
              </select>
            </div>

            <div className="sch-cfg-actions">
              <button
                type="button"
                className="btn primary sm"
                disabled={!dirty || busyAny || !cfg}
                onClick={() => void saveConfig()}
              >
                {busy === 'save' ? '保存中…' : '保存到 config.json'}
              </button>
              <button
                type="button"
                className="btn ghost sm"
                disabled={!dirty || busyAny}
                onClick={revertForm}
              >
                还原
              </button>
            </div>

            <div className="sch-note">
              保存走 <code>POST /api/config</code>：先读整份配置 → 只改
              <code> reverse</code> / <code>candidates.split_scheme</code> → 原样写回，不动其它页设置。
            </div>
          </div>

          <div className="card sch-tips">
            <div className="sch-tip">
              <Icon name="grid" />
              <span>方案 = 码表目录的子文件夹，切换后立即生效；最近方案对可用 Ctrl+M 来回切。</span>
            </div>
            <div className="sch-tip">
              <Icon name="folder" />
              <span>「打开文件夹」在资源管理器里定位该方案；表头「打开码表目录」定位码表根目录。</span>
            </div>
            <div className="sch-tip">
              <Icon name="download" />
              <span>
                「导出」把用户调整（置顶 / 隐藏 / 加词 / 调频）合并进原始码表，写到
                <code>{' 码表导出\\<方案>\\ '}</code>并自动打开目录。
              </span>
            </div>
            <div className="sch-tip">
              <Icon name="info" />
              <span>导出格式随方案记忆（config.schema.export_formats），托盘 / 语言栏导出用同一格式。</span>
            </div>
          </div>
        </div>
      </div>
    </>
  );
}
