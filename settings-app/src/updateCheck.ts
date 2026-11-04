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

/* 【审计 P2-D 2026-11-05】校验式下载：server 中转拉取 + sha256 比对通过
   才落「下载」文件夹。此前 window.open 直接下载即提示运行——清单
   sha256 有传无验，更新源失陷 = 供应链直通。端点不可用（老 server）时
   回退 window.open 浏览器直下（保可用性）。 */
export async function verifiedDownloadUpdate(
  fallbackUrl: string,
  onProgress: (msg: string) => void,
): Promise<void> {
  onProgress('正在下载并校验安装包（校验通过才会保存）…');
  try {
    const r = await fetchWithTimeout(
      '/api/update/download',
      { cache: 'no-store' },
      10 * 60 * 1000,
    );
    const v = await r.json();
    if (v && v.ok) {
      onProgress(`已校验并保存到「下载」文件夹：${v.path}`);
      return;
    }
    onProgress(`校验式下载失败：${v?.error || r.status}，改用浏览器直接下载`);
  } catch {
    onProgress('校验式下载不可用，改用浏览器直接下载');
  }
  window.open(fallbackUrl, '_blank', 'noopener');
}
