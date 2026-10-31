import {
  fallbackBootstrap,
  fallbackSkin,
  type About,
  type Bootstrap,
  type Config,
  type EngineState,
  type Skin,
  type SkinList,
  type SyncSchemes,
  type SyncStatus,
} from './types';

export async function getJson<T>(url: string): Promise<T> {
  const r = await fetch(url, { cache: 'no-store' });
  if (!r.ok) throw new Error(`${url} -> ${r.status}`);
  return (await r.json()) as T;
}

/** 读响应体里的 error/msg 字段（后端 Response::err 约定为 {error}）。 */
async function readErrBody(r: Response): Promise<{ error?: string; msg?: string }> {
  try {
    return (await r.json()) as { error?: string; msg?: string };
  } catch {
    return {};
  }
}

async function postJson<T>(url: string, payload: unknown): Promise<T> {
  const r = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  });
  const body = await readErrBody(r);
  if (!r.ok) throw new Error(body.error || body.msg || `${url} -> ${r.status}`);
  return body as T;
}

export async function loadBootstrap(): Promise<Bootstrap> {
  const errors: string[] = [];
  const safe = async <T>(url: string, fallback: T, tag: string): Promise<T> => {
    try {
      return await getJson<T>(url);
    } catch (e) {
      errors.push(`${tag}: ${(e as Error).message}`);
      return fallback;
    }
  };

  const [about, state, config, skins, sync, syncSchemes] = await Promise.all([
    safe<About>('/api/about', fallbackBootstrap.about, 'about'),
    safe<EngineState>('/api/state', fallbackBootstrap.state, 'state'),
    safe<Config>('/api/config', fallbackBootstrap.config, 'config'),
    safe<SkinList>('/api/skins', fallbackBootstrap.skins, 'skins'),
    safe<SyncStatus>('/api/sync/status', fallbackBootstrap.sync, 'sync'),
    safe<SyncSchemes>('/api/sync/schemes', fallbackBootstrap.syncSchemes, 'sync-schemes'),
  ]);

  let skin: Skin | null = null;
  if (skins.current && skins.current_kind === 'json') {
    try {
      skin = await loadSkin(skins.current);
    } catch (e) {
      errors.push(`skin: ${(e as Error).message}`);
    }
  }
  if (!skin && skins.current_kind === 'json') skin = fallbackSkin;

  return {
    about,
    state,
    config,
    skins,
    skin,
    sync,
    syncSchemes,
    online: errors.length === 0,
    errors,
  };
}

/**
 * 读取指定皮肤并校验返回体的 id 与请求一致（A-2）。
 * 后端在皮肤文件损坏时曾返回 200 + 默认皮肤（id=hufu-default），
 * 不校验会让后续保存覆盖官方皮肤文件。
 */
export async function loadSkin(id: string): Promise<Skin> {
  const s = await getJson<Skin>(`/api/skin?id=${encodeURIComponent(id)}`);
  if (!s || typeof s.id !== 'string' || s.id !== id) {
    throw new Error(`皮肤「${id}」读取异常：服务器返回了别的皮肤（可能文件已损坏）`);
  }
  return s;
}

/**
 * 保存皮肤。默认 select:false —— 保存 ≠ 切换当前皮肤（P0-1）；
 * 后端已按 body.select 决定是否顺带设为当前（未上线前该字段被忽略，
 * 行为与旧版一致）。
 */
export async function saveSkin(
  skin: Skin,
  opts?: { select?: boolean },
): Promise<{ ok: boolean; id?: string; error?: string }> {
  const select = opts?.select ?? false;
  const r = await fetch('/api/skin', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ ...skin, select }),
  });
  let body: { ok?: boolean; id?: string; error?: string } = {};
  try {
    body = await r.json();
  } catch {
    /* ignore */
  }
  if (!r.ok) throw new Error(body.error || `保存失败（${r.status}）`);
  return { ...body, ok: body.ok ?? true };
}

/** 显式切换当前皮肤（P0-1）：kind=ssf 时后端会先校验可解包才切换。 */
export async function selectSkin(id: string, kind: 'json' | 'ssf'): Promise<void> {
  await postJson<{ ok?: boolean }>('/api/skin/select', { id, kind });
}

/** 官方皮肤恢复出厂；非官方 id 返回 404。成功返回恢复后的皮肤全量。 */
export async function resetSkin(id: string): Promise<Skin> {
  const s = await postJson<Skin>('/api/skin/reset', { id });
  if (!s || typeof s.id !== 'string' || s.id !== id) {
    throw new Error('恢复出厂返回数据异常');
  }
  return s;
}

/** 删除 .ssf 皮肤（源文件 + 缓存目录）；删的是当前皮肤时后端回退默认。 */
export async function deleteSsfSkin(id: string): Promise<void> {
  const r = await fetch(`/api/skin?kind=ssf&id=${encodeURIComponent(id)}`, { method: 'DELETE' });
  const body = await readErrBody(r);
  if (!r.ok) throw new Error(body.error || `删除失败（${r.status}）`);
}

export async function syncNow(): Promise<void> {
  const r = await fetch('/api/sync/now', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
  if (!r.ok) throw new Error(`立即同步失败（${r.status}）`);
}

/** 本机已安装的简体中文字体表（皮肤页字体下拉用）。
 *  server 实际返回 [{family, source}] 对象数组；TS 标 string[] 曾致
 *  React #31（对象渲染进 <option>）整页白屏。此处统一映射成 family。 */
export async function loadFonts(): Promise<{ fonts: string[]; default: string }> {
  const r = await fetch('/api/fonts', { cache: 'no-store' });
  const body = await readErrBody(r);
  if (!r.ok) throw new Error(body.error || `字体列表失败（${r.status}）`);
  const raw = (body as { fonts?: Array<string | { family: string; source?: string }> }).fonts || [];
  return {
    fonts: raw.map((x) => (typeof x === 'string' ? x : x.family)),
    default: (body as { default?: string }).default || '',
  };
}

/** SSF 皮肤调参（overrides + 皮肤原值 base）。 */
export interface SsfOverrides {
  font_pt?: number | null;
  font_face?: string | null;
  horizontal?: boolean | null;
  show_index?: boolean | null;
  cand_y?: number | null;
  code_color?: string | null;
  first_color?: string | null;
  text_color?: string | null;
  annotation_color?: string | null;
  overlay?: unknown;
}

export interface SsfSettingsResp {
  id: string;
  overrides: SsfOverrides;
  base: { font_pt?: number; font_face?: string; code_color?: string; first_color?: string; text_color?: string; annotation_color?: string };
}

export async function loadSsfSettings(id: string): Promise<SsfSettingsResp> {
  const r = await fetch(`/api/skin/ssf-settings?id=${encodeURIComponent(id)}`, { cache: 'no-store' });
  const body = await readErrBody(r);
  if (!r.ok) throw new Error(body.error || `读取皮肤参数失败（${r.status}）`);
  return body as SsfSettingsResp;
}

export async function saveSsfSettings(id: string, overrides: SsfOverrides): Promise<void> {
  await postJson('/api/skin/ssf-settings', { id, overrides });
}
