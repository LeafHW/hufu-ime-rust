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

/* 统一请求超时。设置页所有请求都打本机 hufu-server：端口被占/引擎卡死时
   fetch 会无限期挂着，UI 就一直「保存中…」、按钮永久 disabled（只能刷新
   页面）。15s 足够本机磁盘写盘，超时给明确文案而不是静默挂住。 */
const REQ_TIMEOUT_MS = 15000;

/** fetch 的可超时包装：AbortController + setTimeout，调用方签名不变。
 *  只有「我们自己 abort 过」才报超时——其它网络错误（server 未启动等）
 *  仍原样抛出，避免把连接失败误报成超时。 */
export async function fetchWithTimeout(
  url: string,
  init?: RequestInit,
  timeoutMs: number = REQ_TIMEOUT_MS,
): Promise<Response> {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), timeoutMs);
  try {
    return await fetch(url, { ...init, signal: ctrl.signal });
  } catch (e) {
    if (ctrl.signal.aborted) {
      throw new Error(`请求超时：${url}（${Math.round(timeoutMs / 1000)}s 无响应）`);
    }
    throw e;
  } finally {
    clearTimeout(timer);
  }
}

export async function getJson<T>(url: string): Promise<T> {
  const r = await fetchWithTimeout(url, { cache: 'no-store' });
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
  const r = await fetchWithTimeout(url, {
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
  const r = await fetchWithTimeout('/api/skin', {
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
  const r = await fetchWithTimeout(`/api/skin?kind=ssf&id=${encodeURIComponent(id)}`, { method: 'DELETE' });
  const body = await readErrBody(r);
  if (!r.ok) throw new Error(body.error || `删除失败（${r.status}）`);
}

export async function syncNow(): Promise<void> {
  const r = await fetchWithTimeout('/api/sync/now', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
  if (!r.ok) throw new Error(`立即同步失败（${r.status}）`);
}

/** 本机已安装的简体中文字体表（皮肤页字体下拉用）。
 *  server 实际返回 [{family, source}] 对象数组；TS 标 string[] 曾致
 *  React #31（对象渲染进 <option>）整页白屏。此处统一映射成 family。 */
export async function loadFonts(): Promise<{ fonts: string[]; default: string }> {
  const r = await fetchWithTimeout('/api/fonts', { cache: 'no-store' });
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
  const r = await fetchWithTimeout(`/api/skin/ssf-settings?id=${encodeURIComponent(id)}`, { cache: 'no-store' });
  const body = await readErrBody(r);
  if (!r.ok) throw new Error(body.error || `读取皮肤参数失败（${r.status}）`);
  return body as SsfSettingsResp;
}

export async function saveSsfSettings(id: string, overrides: SsfOverrides): Promise<void> {
  await postJson('/api/skin/ssf-settings', { id, overrides });
}

/* ── 【P1-15 2026-11-04】配置写入统一入口 ──────────────────────────────
   后端 /api/config 现在返回 rev（每次写入自增），POST 带 rev 不符即 409，
   不再允许「读旧快照 → 整份写回」静默洗掉别人刚保存的配置。
   updateConfig 的语义：
     1. GET 最新配置 + rev；
     2. 深度合并 patch（对象递归、数组/标量整体替换）；
     3. 带 rev POST；
     4. 409（其他页面/窗口已改）→ 询问用户：确定=按最新 rev 重新应用
        本次修改；取消=放弃本次修改。绝不静默覆盖别人的写入。
   同一窗口内的写入串行化（单飞链）：快速连点开关、多个常驻页面同时写
   都不会互相覆盖（沿用动效页原有写队列思路，收敛成全局唯一入口）。 */
type JsonDict = Record<string, unknown>;

function isPlainDict(v: unknown): v is JsonDict {
  return !!v && typeof v === 'object' && !Array.isArray(v);
}

/** 深度合并：patch 里的对象递归合并，数组/标量整体替换。 */
function deepMerge(base: JsonDict, patch: JsonDict): JsonDict {
  const out: JsonDict = { ...base };
  for (const [k, v] of Object.entries(patch)) {
    out[k] = isPlainDict(v) && isPlainDict(out[k]) ? deepMerge(out[k] as JsonDict, v) : v;
  }
  return out;
}

let configWriteChain: Promise<unknown> = Promise.resolve();

/** 统一配置写入：成功后返回合并后的配置快照；失败抛错（409 用户取消也抛错）。 */
export function updateConfig(patch: JsonDict): Promise<Config> {
  const run = () => writeConfigOnce(patch);
  const p = configWriteChain.then(run, run);
  // 链本身吞错，避免一次失败卡死后续写入（错误仍抛给各自调用方）
  configWriteChain = p.catch(() => undefined);
  return p;
}

async function writeConfigOnce(patch: JsonDict): Promise<Config> {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const fresh = (await getJson<Config & { rev?: number }>('/api/config')) as Config & {
      rev?: number;
    };
    const rev = typeof fresh.rev === 'number' ? fresh.rev : undefined;
    const merged = deepMerge(fresh as unknown as JsonDict, patch);
    const body: JsonDict = { ...merged };
    if (rev !== undefined) body.rev = rev;
    const r = await fetchWithTimeout('/api/config', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    const respBody = (await r.json().catch(() => ({}))) as { error?: string };
    if (r.status === 409) {
      const retry = window.confirm(
        '其他页面/窗口已修改并保存了配置。\n\n' +
          '点「确定」：按最新配置重新应用本次修改（推荐）\n' +
          '点「取消」：放弃本次修改（当前页面保持原样）',
      );
      if (!retry) throw new Error('配置已被其他页面修改，本次修改未保存');
      continue; // 重新 GET 最新 rev，再把本次 patch 合并上去
    }
    if (!r.ok) throw new Error(respBody.error || `保存失败（${r.status}）`);
    return merged as unknown as Config;
  }
  throw new Error('配置已被其他页面持续修改，保存失败（请稍后重试）');
}
