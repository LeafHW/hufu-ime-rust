import { fetchWithTimeout } from './api';

export interface TrainerStatus {
  enabled: boolean;
  installed: boolean;
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
