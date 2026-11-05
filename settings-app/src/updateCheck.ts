/* 【检查更新共享 2026-11-01】概览/关于两页共用：真实调 /api/update/check。 */
import { getJson, fetchWithTimeout } from './api';

export type UpdateState =
  | { kind: 'idle' }
  | { kind: 'checking' }
  | { kind: 'latest'; current: string }
  | { kind: 'update'; latest: string; downloadUrl: string | null; pageUrl: string }
  | { kind: 'error'; msg: string };

export async function fetchUpdateCheck(fallbackPageUrl: string): Promise<UpdateState> {
  try {
    const r = await getJson<{
      ok: boolean; current: string; latest?: string; has_update?: boolean;
      page_url?: string; download_url?: string | null; error?: string;
    }>('/api/update/check');
    if (!r.ok) return { kind: 'error', msg: r.error || '检查失败' };
    if (r.has_update)
      return { kind: 'update', latest: r.latest || '?', downloadUrl: r.download_url || null, pageUrl: r.page_url || fallbackPageUrl };
    return { kind: 'latest', current: r.current };
  } catch (e) {
    return { kind: 'error', msg: (e as Error).message };
  }
}

/* ── 【更新下载 2026-11-06】后台任务 + 轮询快照 ────────────────────────
   旧端点 /api/update/download 是一次阻塞请求：前端最长等 10 分钟，中间
   只能显示一句静态文案（用户实测 v1.9.9.3：没有进度、不知道存到哪）。
   现由 server 后台线程下载，前端 400ms 拉 /api/update/download/status：
   百分比、已下/总量、速度、落点、完成后的运行/定位都来自真实状态。 */

export type DownloadPhase =
  | 'idle'
  | 'preparing'
  | 'downloading'
  | 'verifying'
  | 'done'
  | 'error'
  | 'cancelled';

export interface DownloadState {
  ok?: boolean;
  phase: DownloadPhase;
  /** 清单里的目标版本号（如 1.9.9.4） */
  latest?: string;
  /** 安装包文件名 */
  file?: string;
  /** 安装包完整落盘路径 */
  dest?: string;
  /** 下载目录（启动即有，用于「下到哪」） */
  dir?: string;
  /** 总字节数；0=服务器未给 Content-Length 也不在清单里（进度不确定） */
  total?: number;
  downloaded?: number;
  elapsed_ms?: number;
  error?: string;
  sha256_expect?: string;
  sha256_got?: string;
  active?: boolean;
}

export const isDownloadActive = (p?: DownloadPhase): boolean =>
  p === 'preparing' || p === 'downloading' || p === 'verifying';

/** 启动下载。409（已有任务）不抛错：返回 started=false + 当前快照，前端
 *  直接接力轮询，避免「重复点击 → 一条死文案」的旧体验。 */
export async function startUpdateDownload(): Promise<{
  started: boolean;
  state: DownloadState;
}> {
  const r = await fetchWithTimeout(
    '/api/update/download/start',
    { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' },
    20000,
  );
  let v: DownloadState & { error?: string } = { phase: 'idle' };
  try {
    v = (await r.json()) as DownloadState;
  } catch {
    /* 非 JSON（老 server 404 页等）：按失败处理 */
  }
  if (r.status === 404) {
    throw new Error('当前 server 不支持后台下载（请先更新虎符）');
  }
  if (!r.ok && r.status !== 409) {
    throw new Error(v.error || `启动下载失败（${r.status}）`);
  }
  return { started: r.ok, state: v };
}

export async function fetchDownloadState(): Promise<DownloadState> {
  return await getJson<DownloadState>('/api/update/download/status');
}

export async function cancelUpdateDownload(): Promise<void> {
  await fetchWithTimeout('/api/update/download/cancel', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: '{}',
  });
}

/** 打开刚下载的安装包（installer）或在资源管理器定位（folder）。
 *  路径由 server 自己的下载状态决定，前端不传路径。 */
export async function openDownloaded(target: 'installer' | 'folder'): Promise<void> {
  const r = await fetchWithTimeout(
    '/api/update/open',
    { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ target }) },
    30000,
  );
  if (!r.ok) {
    let msg = `打开失败（${r.status}）`;
    try {
      const b = (await r.json()) as { error?: string };
      if (b?.error) msg = b.error;
    } catch {
      /* ignore */
    }
    throw new Error(msg);
  }
}
