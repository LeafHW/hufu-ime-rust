import { useRef, useState, type ChangeEvent } from 'react';
import type { Bootstrap } from '../types';
import { Icon } from '../components/Icon';
import { Badge, PageHead } from '../components/ui';
import '../styles/data.css';

/* ────────────────────────────────────────────────────────────────────────────
 * 数据与备份页（真实读写）——接口全部对照旧设置页 settings-ui/index.html：
 *   GET  /api/settings_export   打包 bundle v3（config + 皮肤 + .ssf + overrides），
 *                               服务端已剥掉整句模型参数与同步凭据
 *   POST /api/settings_import   body = 导出产物；语义「叠加到默认值」
 *   GET  /api/about             数据目录 / 安装目录 / 版本
 *   GET  /api/state             当前方案（码表目录 = 码表根\<方案>）
 *   POST /api/open_schema_dir   { name } 打开码表文件夹；name="." 即码表根目录
 *   POST /api/open_dir          { kind: "data"|"exe"|"schema" } 受控打开目录
 * 旧页一致行为：导出优先 File System Access API 原生「另存为」，不支持则 <a download>；
 *   导入先本地解析 + 摘要确认，再 POST；无 alert/confirm/prompt。
 * ────────────────────────────────────────────────────────────────────────── */

interface ImportPreview {
  fileName: string;
  fileSize: number;
  /** 0 = 旧版裸 config / 无 bundle 标记 */
  version: number;
  skins: number;
  ssfSkins: number;
  ssfSettings: number;
  exportedAt: string;
  raw: Record<string, unknown>;
}

interface ImportApplied {
  fileName: string;
  skins: number;
  ssfSkins: number;
  ssfSettings: number;
}

interface ImportResponse {
  ok?: boolean;
  skins_applied?: number;
  ssf_skins_applied?: number;
  ssf_settings_applied?: number;
}

interface OpenDirResponse {
  ok?: boolean;
  path?: string;
}

type Status = { tone: 'ok' | 'err' | 'info'; text: string };

interface SaveFileHandle {
  createWritable: () => Promise<{
    write: (data: Blob) => Promise<void>;
    close: () => Promise<void>;
  }>;
}
type SavePicker = (opts: {
  suggestedName?: string;
  types?: { description?: string; accept: Record<string, string[]> }[];
}) => Promise<SaveFileHandle>;

const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === 'object' && v !== null && !Array.isArray(v);

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function isCancel(e: unknown): boolean {
  return (e as { name?: string } | null)?.name === 'AbortError';
}

function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return '0 B';
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(2)} MB`;
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

function suggestedExportName(): string {
  const d = new Date();
  const p2 = (n: number) => String(n).padStart(2, '0');
  return `hufu-config-${d.getFullYear()}${p2(d.getMonth() + 1)}${p2(d.getDate())}-${p2(
    d.getHours(),
  )}${p2(d.getMinutes())}.json`;
}

/** 写文件：优先 Chromium 原生另存为对话框（旧页同款），否则退回下载。
 *  用户点「取消」→ AbortError 原样抛出；其它失败（无用户激活 / 浏览器不支持）→ 退回下载。 */
async function saveBlob(blob: Blob, name: string): Promise<'picker' | 'download'> {
  const picker = (window as unknown as { showSaveFilePicker?: SavePicker }).showSaveFilePicker;
  let handle: SaveFileHandle | null = null;
  if (typeof picker === 'function') {
    try {
      handle = await picker.call(window, {
        suggestedName: name,
        types: [{ description: '虎符设置文件', accept: { 'application/json': ['.json'] } }],
      });
    } catch (e) {
      if ((e as { name?: string } | null)?.name === 'AbortError') throw e;
      handle = null; // 无激活 / 权限受限 → 走下载兜底
    }
  }
  if (handle) {
    const writable = await handle.createWritable();
    await writable.write(blob);
    await writable.close();
    return 'picker';
  }
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.rel = 'noopener';
  document.body.appendChild(a);
  a.click();
  a.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 5000);
  return 'download';
}

async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    /* 落到 execCommand 兜底 */
  }
  try {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.left = '-9999px';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand('copy');
    ta.remove();
    return ok;
  } catch {
    return false;
  }
}

/** 本地预校验，规则与旧页 + 服务端 /api/settings_import 的 bundle 白名单一致。 */
function makePreview(fileName: string, fileSize: number, parsed: unknown): ImportPreview {
  if (!isObject(parsed)) throw new Error('文件内容不是 JSON 对象');
  const mark = parsed.hufu_settings_bundle;
  let bundle: Record<string, unknown>;
  let version = 0;
  if (typeof mark === 'number' && (mark === 1 || mark === 2 || mark === 3)) {
    bundle = parsed;
    version = mark;
  } else if (mark === undefined) {
    const cfg = 'config' in parsed ? parsed.config : parsed;
    if (!isObject(cfg)) throw new Error('文件里没有 config 节');
    bundle = { config: cfg };
  } else {
    throw new Error(`不是虎符设置文件（hufu_settings_bundle 标记缺失或版本不识别：${String(mark)}）`);
  }
  if (!isObject(bundle.config)) throw new Error('文件里没有 config 节');
  const count = (v: unknown) => (isObject(v) ? Object.keys(v).length : 0);
  return {
    fileName,
    fileSize,
    version,
    skins: count(bundle.skins),
    ssfSkins: count(bundle.ssf_skins),
    ssfSettings: count(bundle.ssf_settings),
    exportedAt: typeof bundle.exported_at === 'string' ? bundle.exported_at : '',
    raw: bundle,
  };
}

export function DataPage({ data, notify }: { data: Bootstrap; notify: (msg: string) => void }) {
  const [exporting, setExporting] = useState(false);
  const [importing, setImporting] = useState(false);
  const [opening, setOpening] = useState('');
  const [pending, setPending] = useState<ImportPreview | null>(null);
  const [imported, setImported] = useState<ImportApplied | null>(null);
  const [lastExport, setLastExport] = useState<{
    name: string;
    bytes: number;
    how: 'picker' | 'download';
  } | null>(null);
  const [status, setStatus] = useState<Status | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  const current = data.state.current_schema || '';
  const loadingAny = exporting || importing || opening !== '';

  const fail = (text: string) => {
    setStatus({ tone: 'err', text });
    notify(text);
  };

  /* ── 导出：GET /api/settings_export → 存盘 ── */
  const exportSettings = async () => {
    if (exporting) return;
    setExporting(true);
    try {
      const r = await fetch('/api/settings_export', { cache: 'no-store' });
      if (!r.ok) {
        const body = await readBody(r);
        throw new Error(httpError(body, '/api/settings_export', r.status));
      }
      const blob = await r.blob();
      if (blob.size === 0) throw new Error('服务端返回了空文件');
      const name = suggestedExportName();
      const how = await saveBlob(blob, name);
      setLastExport({ name, bytes: blob.size, how });
      setStatus({
        tone: 'ok',
        text:
          how === 'picker'
            ? `已导出全部设置（含皮肤与按键）：${name} · ${formatBytes(blob.size)}`
            : `已导出：${name} · ${formatBytes(
                blob.size,
              )}（未弹出保存对话框，已退回浏览器默认下载位置）`,
      });
      notify('已导出全部设置');
    } catch (e) {
      if (isCancel(e)) {
        setStatus({ tone: 'info', text: '已取消导出（没有写文件）' });
        notify('已取消导出');
      } else {
        fail(`导出失败：${errText(e)}`);
      }
    } finally {
      setExporting(false);
    }
  };

  /* ── 导入：本地解析 + 摘要 → 确认 → POST /api/settings_import ── */
  const pickImport = () => {
    if (importing) return;
    fileRef.current?.click();
  };

  const onFile = async (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = '';
    if (!file) return;
    try {
      const text = await file.text();
      let parsed: unknown;
      try {
        parsed = JSON.parse(text) as unknown;
      } catch (er) {
        throw new Error(`JSON 解析失败：${errText(er)}`);
      }
      const preview = makePreview(file.name, file.size, parsed);
      setPending(preview);
      setImported(null);
      setStatus({
        tone: 'info',
        text: `已读取「${file.name}」：bundle ${
          preview.version ? `v${preview.version}` : '（旧版裸 config）'
        } · 皮肤 ${preview.skins} 张；核对摘要后点「确认导入」`,
      });
    } catch (er) {
      setPending(null);
      fail(`读取设置文件失败：${errText(er)}`);
    }
  };

  const confirmImport = async () => {
    if (!pending || importing) return;
    setImporting(true);
    try {
      const r = await postJson<ImportResponse>('/api/settings_import', pending.raw);
      const applied: ImportApplied = {
        fileName: pending.fileName,
        skins: r.skins_applied ?? 0,
        ssfSkins: r.ssf_skins_applied ?? 0,
        ssfSettings: r.ssf_settings_applied ?? 0,
      };
      setImported(applied);
      setPending(null);
      setStatus({
        tone: 'ok',
        text: `导入成功：皮肤 ${applied.skins} 张、.ssf ${applied.ssfSkins} 款、调参 ${applied.ssfSettings} 组；配置已应用。建议重新载入界面查看全部变化。`,
      });
      notify('设置已导入并应用，建议重新载入界面');
    } catch (e) {
      fail(`导入失败：${errText(e)}`);
    } finally {
      setImporting(false);
    }
  };

  const cancelImport = () => {
    if (importing) return;
    setPending(null);
    setStatus({ tone: 'info', text: '已取消导入（没有写入任何设置）' });
  };

  /* ── 目录 ── */
  const copyPath = async (label: string, text: string) => {
    if (!text) {
      fail(`${label}路径为空，无法复制`);
      return;
    }
    const ok = await copyText(text);
    if (ok) {
      setStatus({ tone: 'ok', text: `已复制${label}路径：${text}` });
      notify(`已复制${label}路径`);
    } else {
      fail(`复制失败，请手动选择路径：${text}`);
    }
  };

  const openDir = async (kind: 'data' | 'exe' | 'schema', label: string) => {
    if (opening !== '') return;
    setOpening(kind);
    try {
      const r = await postJson<OpenDirResponse>('/api/open_dir', { kind });
      setStatus({ tone: 'ok', text: `已在资源管理器打开${label}${r.path ? `：${r.path}` : ''}` });
      notify(`已打开${label}`);
    } catch (e) {
      fail(`打开${label}失败：${errText(e)}`);
    } finally {
      setOpening('');
    }
  };

  const openSchemaDir = async (name: string, label: string) => {
    if (opening !== '') return;
    setOpening(name);
    try {
      const r = await postJson<OpenDirResponse>('/api/open_schema_dir', { name });
      setStatus({ tone: 'ok', text: `已在资源管理器打开${label}${r.path ? `：${r.path}` : ''}` });
      notify(`已打开${label}`);
    } catch (e) {
      fail(`打开${label}失败：${errText(e)}`);
    } finally {
      setOpening('');
    }
  };

  return (
    <>
      <PageHead
        title="数据与备份"
        sub={
          <>
            <span className={data.online ? 'dot' : 'dot muted'} />
            {data.online ? '引擎在线' : '离线预览'}
            <span style={{ color: 'var(--text3)' }}>·</span>
            一个 JSON 带走全部设置
            <span style={{ color: 'var(--text3)' }}>·</span>
            v{data.about.version}
          </>
        }
      />

      <div className="dt-layout">
        <div className="dt-main">
          <div className="card dt-io-card">
            {/* 导出 */}
            <section className="dt-sec">
              <div className="dt-sec-head">
                <div className="dt-ico">
                  <Icon name="download" />
                </div>
                <div className="dt-sec-title">
                  <b>导出全部设置</b>
                  <span>GET /api/settings_export</span>
                </div>
                <Badge tone="gray">bundle v3</Badge>
              </div>
              <p className="dt-desc">
                配置项、皮肤（含你调过 / 自定义的）、自定义按键（键位映射、Shift+符号 / 字母）全部在内。
                升版本 / 换机器时导入即还原；新版本新增的设置项自动按默认值补齐，导不进的老物件自动忽略。
              </p>
              <div className="dt-actions">
                <button
                  type="button"
                  className="btn primary"
                  disabled={exporting}
                  onClick={() => void exportSettings()}
                >
                  <Icon name="download" />
                  {exporting ? '正在打包…' : '导出设置…'}
                </button>
                {lastExport ? (
                  <span className="dt-meta" title={lastExport.name}>
                    最近导出 · {lastExport.name} · {formatBytes(lastExport.bytes)}
                    {lastExport.how === 'download' ? '（默认下载处）' : ''}
                  </span>
                ) : (
                  <span className="dt-meta">导出时弹「另存为」选位置；文件名带日期时间</span>
                )}
              </div>
            </section>

            <div className="dt-sep" />

            {/* 导入 */}
            <section className="dt-sec">
              <div className="dt-sec-head">
                <div className="dt-ico">
                  <Icon name="upload" />
                </div>
                <div className="dt-sec-title">
                  <b>导入设置</b>
                  <span>POST /api/settings_import</span>
                </div>
                <Badge tone="gray">叠加到默认值</Badge>
              </div>
              <p className="dt-desc">
                选择之前导出的 JSON：缺失字段按默认值补齐（新版新增项）、未知字段忽略（老物件不炸），
                皮肤逐张校验后落盘。<b>导入不会改动本机的整句模型参数。</b>
              </p>
              <div className="dt-actions">
                <button type="button" className="btn outline" disabled={importing} onClick={pickImport}>
                  <Icon name="upload" />
                  选择设置文件…
                </button>
                <input
                  ref={fileRef}
                  type="file"
                  accept=".json,application/json"
                  className="dt-file"
                  aria-label="选择虎符设置文件"
                  onChange={(e) => void onFile(e)}
                />
                <span className="dt-meta">
                  {pending ? `待确认：${pending.fileName}` : '选文件后先看摘要，确认才写入'}
                </span>
              </div>

              {pending ? (
                <div className="dt-pending">
                  <div className="dt-pending-head">
                    <b title={pending.fileName}>{pending.fileName}</b>
                    <span>{formatBytes(pending.fileSize)}</span>
                    <Badge tone={pending.version ? 'gold' : 'gray'}>
                      {pending.version ? `bundle v${pending.version}` : '旧版 / 裸 config'}
                    </Badge>
                  </div>
                  <div className="dt-chips">
                    <span className="chip">
                      皮肤 <b>{pending.skins}</b>
                    </span>
                    <span className="chip">
                      .ssf <b>{pending.ssfSkins}</b>
                    </span>
                    <span className="chip">
                      皮肤调参 <b>{pending.ssfSettings}</b>
                    </span>
                    {pending.exportedAt ? (
                      <span className="chip">
                        导出于 <b>{pending.exportedAt}</b>
                      </span>
                    ) : null}
                  </div>
                  <div className="dt-warn">
                    确认后立即应用配置并覆盖同名皮肤 / .ssf；不确定时先「导出设置…」留一份当前备份。
                  </div>
                  <div className="dt-actions">
                    <button
                      type="button"
                      className="btn primary"
                      disabled={importing || loadingAny}
                      onClick={() => void confirmImport()}
                    >
                      {importing ? '导入中…' : '确认导入'}
                    </button>
                    <button
                      type="button"
                      className="btn ghost"
                      disabled={importing}
                      onClick={cancelImport}
                    >
                      取消
                    </button>
                  </div>
                </div>
              ) : null}

              {imported ? (
                <div className="dt-done">
                  <div className="dt-done-head">
                    <span className="dot" />
                    <b>导入成功</b>
                    <span title={imported.fileName}>{imported.fileName}</span>
                  </div>
                  <div className="dt-done-body">
                    皮肤 {imported.skins} 张 · .ssf {imported.ssfSkins} 款 · 调参{' '}
                    {imported.ssfSettings} 组；配置已应用。
                  </div>
                  <div className="dt-actions">
                    <button type="button" className="btn outline sm" onClick={() => location.reload()}>
                      <Icon name="refresh" />
                      立即重新载入界面
                    </button>
                    <span className="dt-meta">界面每 10 秒自动拉取最新状态，也可以继续使用</span>
                  </div>
                </div>
              ) : null}
            </section>

            <div className="dt-note">
              <Icon name="shield" />
              <span>
                建议把导出的 JSON 存进网盘或移动硬盘：同一份文件可用于换机迁移，也可作为升级前的回滚点。
              </span>
            </div>

            <div className={`dt-status${status ? ` ${status.tone}` : ''}`}>
              <span
                className={`dot ${status?.tone === 'err' ? 'red' : status?.tone === 'ok' ? '' : 'muted'}`}
              />
              <span>{status ? status.text : '导出 / 导入都是本机与引擎之间的真实调用，文件不会上传到任何服务器。'}</span>
            </div>
          </div>
        </div>

        <div className="dt-side">
          <div className="card dt-dirs">
            <div className="card-head" style={{ marginBottom: 4 }}>
              <span className="card-title">本机目录</span>
              <Badge tone="gray">只读</Badge>
            </div>

            <div className="dt-dir">
              <div className="dt-dir-ico">
                <Icon name="db" />
              </div>
              <div className="dt-dir-main">
                <div className="dt-dir-name">
                  数据目录<span>config.json / 转换词典 / 音效 / 同步.json</span>
                </div>
                <code className="dt-path" title={data.about.data_dir}>
                  {data.about.data_dir || '—'}
                </code>
              </div>
              <div className="dt-dir-actions">
                <button
                  type="button"
                  className="btn sm"
                  disabled={!data.about.data_dir || opening !== ''}
                  onClick={() => void openDir('data', '数据目录')}
                >
                  打开
                </button>
                <button
                  type="button"
                  className="btn sm outline"
                  disabled={!data.about.data_dir}
                  onClick={() => void copyPath('数据目录', data.about.data_dir)}
                >
                  复制
                </button>
              </div>
            </div>

            <div className="dt-dir">
              <div className="dt-dir-ico">
                <Icon name="monitor" />
              </div>
              <div className="dt-dir-main">
                <div className="dt-dir-name">
                  安装目录<span>exe / version.txt / 码表 / 皮肤缓存</span>
                </div>
                <code className="dt-path" title={data.about.exe_dir}>
                  {data.about.exe_dir || '—'}
                </code>
              </div>
              <div className="dt-dir-actions">
                <button
                  type="button"
                  className="btn sm"
                  disabled={!data.about.exe_dir || opening !== ''}
                  onClick={() => void openDir('exe', '安装目录')}
                >
                  打开
                </button>
                <button
                  type="button"
                  className="btn sm outline"
                  disabled={!data.about.exe_dir}
                  onClick={() => void copyPath('安装目录', data.about.exe_dir)}
                >
                  复制
                </button>
              </div>
            </div>

            <div className="dt-dir">
              <div className="dt-dir-ico">
                <Icon name="grid" />
              </div>
              <div className="dt-dir-main">
                <div className="dt-dir-name">
                  码表目录<span>安装根\码表 优先，回退 数据\码表 · 当前方案 {current || '—'}</span>
                </div>
                <code className="dt-path" title={`码表\\${current || '（当前方案未知）'}`}>
                  {`码表\\${current || '—'}`}
                </code>
                <div className="dt-dir-acts">
                  <button
                    type="button"
                    className="btn sm goldline"
                    disabled={!current || opening !== ''}
                    onClick={() => void openSchemaDir(current, '当前方案文件夹')}
                  >
                    <Icon name="folder" />
                    打开当前方案
                  </button>
                  <button
                    type="button"
                    className="btn sm ghost"
                    disabled={opening !== ''}
                    onClick={() => void openSchemaDir('.', '码表根目录')}
                  >
                    <Icon name="folder" />
                    打开根目录
                  </button>
                </div>
              </div>
            </div>

            <div className="dt-dir-note">
              <Icon name="info" />
              <span>
                数据目录 / 安装目录 / 码表根都通过 <code>POST /api/open_dir</code>（kind=data/exe/schema）打开；
                也可复制路径。码表方案文件夹仍走 <code>/api/open_schema_dir</code>。
              </span>
            </div>
          </div>

          <div className="card dt-exclude">
            <div className="dt-exclude-h">
              <Icon name="shield" />
              导出<b>不包含</b>
            </div>
            <ul>
              <li>
                整句模型参数（config.sentence 整节）：导入永不改动本机模型参数，保证新算法升级生效。
              </li>
              <li>
                云同步账号 / 应用密码：只存本机 <code>数据\同步.json</code>，绝不进导出文件。
              </li>
            </ul>
          </div>
        </div>
      </div>
    </>
  );
}
