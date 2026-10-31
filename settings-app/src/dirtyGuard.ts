import { useEffect, useState } from 'react';

/**
 * 全局「未保存修改」注册表（OV-B1）。
 * 各编辑页通过 useDirtyGuard 注册自己的脏状态；App 层在路由切换与
 * window 关闭前统一拦截确认，概览页用它展示真实的「设置保存」状态。
 */
export interface DirtyEntry {
  key: string;
  label: string;
}

const dirtyMap = new Map<string, string>();
const listeners = new Set<() => void>();

function emit() {
  listeners.forEach((l) => l());
}

export function setDirtyPage(key: string, dirty: boolean, label = key) {
  const had = dirtyMap.has(key);
  if (dirty && !had) {
    dirtyMap.set(key, label);
    emit();
  } else if (!dirty && had) {
    dirtyMap.delete(key);
    emit();
  }
}

export function getDirtyPages(): DirtyEntry[] {
  return Array.from(dirtyMap.entries(), ([key, label]) => ({ key, label }));
}

export function subscribeDirty(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

/** 订阅脏状态变化并返回当前快照（概览页状态卡用）。 */
export function useDirtyPages(): DirtyEntry[] {
  const [pages, setPages] = useState<DirtyEntry[]>(() => getDirtyPages());
  useEffect(() => subscribeDirty(() => setPages(getDirtyPages())), []);
  return pages;
}

/**
 * 编辑页注册脏状态；组件卸载（导航已被用户确认）时自动清除。
 * 卸载清空是预期行为：能走到卸载说明守卫已放行或页面本来就不脏。
 */
export function useDirtyGuard(key: string, label: string, dirty: boolean) {
  useEffect(() => {
    setDirtyPage(key, dirty, label);
    return () => setDirtyPage(key, false);
  }, [key, label, dirty]);
}
