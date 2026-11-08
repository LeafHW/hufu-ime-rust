import { fetchWithTimeout } from './api';

export interface TrainerStatus {
  enabled: boolean;
  installed: boolean;
}

/* 【整改10 2026-11-08】全局老板键契约：小写加号串（如 "ctrl+alt+g"），
   修饰键 ctrl/alt/shift/win 至少一个 + 主键 a-z/0-9/f1-f12，空串=禁用。
   服务端缺省值 ctrl+alt+g；老 server 不回该字段时按缺省展示。 */
export const DEFAULT_TRAINER_HOTKEY = 'ctrl+alt+g';

/** "ctrl+alt+g" → "Ctrl + Alt + G"；空串/非法输入返回 ''。 */
export function prettyHotkey(value: string | undefined | null): string {
  const raw = (value ?? '').trim().toLowerCase();
  if (!raw) return '';
  return raw
    .split('+')
    .filter(Boolean)
    .map((part) => (part === 'ctrl' || part === 'alt' || part === 'shift' || part === 'win'
      ? part === 'win' ? 'Win' : part.charAt(0).toUpperCase() + part.slice(1)
      : part.toUpperCase()))
    .join(' + ');
}

/** keydown 事件 → 主键（a-z / 0-9 / f1-f12，小写）；不支持返回 ''。
    Shift 按住时字母键 e.key 是大写，统一转小写。 */
export function hotkeyMainKey(e: KeyboardEvent): string {
  const key = e.key;
  if (/^[a-z]$/i.test(key)) return key.toLowerCase();
  if (/^[0-9]$/.test(key)) return key;
  if (/^f([1-9]|1[0-2])$/i.test(key)) return key.toLowerCase();
  return '';
}

export class TrainerUnsupportedError extends Error {
  constructor() {
    super('当前运行的虎符服务尚不支持跟打器，请更新完整安装包并重启虎符服务。');
    this.name = 'TrainerUnsupportedError';
  }
}

async function requestTrainer(path: string, method: 'GET' | 'POST') {
  const response = await fetchWithTimeout(`/api/trainer/${path}`, { method, cache: 'no-store' });
  // Older servers may return either JSON or an HTML 404 page.
  if (response.status === 404) throw new TrainerUnsupportedError();
  const body = await response.json().catch(() => ({})) as Record<string, unknown>;
  if (!response.ok) throw new Error(typeof body.error === 'string' ? body.error : `跟打器请求失败（${response.status}）`);
  return body;
}

export async function getTrainerStatus(): Promise<TrainerStatus> {
  const body = await requestTrainer('status', 'GET');
  if (typeof body.enabled !== 'boolean' || typeof body.installed !== 'boolean') {
    throw new Error('跟打器状态不完整，请更新完整安装包并重启虎符服务。');
  }
  return { enabled: body.enabled, installed: body.installed };
}

export async function openTrainer(): Promise<void> {
  await requestTrainer('open', 'POST');
}
