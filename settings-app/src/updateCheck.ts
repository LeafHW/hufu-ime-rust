/* 【检查更新共享 2026-11-01】概览/关于两页共用：真实调 /api/update/check。 */
import { getJson } from './api';

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
