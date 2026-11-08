import { useCallback, useEffect, useRef, useState } from 'react';
import type { Bootstrap } from '../types';
import { updateConfig } from '../api';
import { getTrainerStatus, openTrainer, TrainerUnsupportedError, type TrainerStatus } from '../trainerApi';
import { Icon } from './Icon';

export function TrainerSettings({ data, notify, refresh }: {
  data: Bootstrap;
  notify: (message: string) => void;
  refresh: () => Promise<unknown>;
}) {
  const [status, setStatus] = useState<TrainerStatus | null>(null);
  const [error, setError] = useState('');
  const [unsupported, setUnsupported] = useState(false);
  const [checking, setChecking] = useState(false);
  const [saving, setSaving] = useState(false);
  const [opening, setOpening] = useState(false);
  const operation = useRef(false);
  const checkSequence = useRef(0);

  const checkStatus = useCallback(async () => {
    const sequence = ++checkSequence.current;
    if (!data.online) {
      setStatus(null);
      setError('虎符服务未连接，连接后可设置跟打器。');
      setChecking(false);
      return;
    }
    setChecking(true);
    try {
      const next = await getTrainerStatus();
      if (sequence !== checkSequence.current) return;
      setStatus(next);
      setUnsupported(false);
      setError('');
    } catch (e) {
      if (sequence !== checkSequence.current) return;
      setStatus(null);
      setUnsupported(e instanceof TrainerUnsupportedError);
      setError((e as Error).message);
    } finally {
      if (sequence === checkSequence.current) setChecking(false);
    }
  }, [data.online]);

  useEffect(() => {
    void checkStatus();
    return () => { checkSequence.current += 1; };
  }, [checkStatus, data.config.trainer?.enabled]);

  const toggle = async () => {
    if (!status || operation.current || !data.online) return;
    operation.current = true;
    setSaving(true);
    ++checkSequence.current;
    const enabled = !status.enabled;
    try {
      await updateConfig({ trainer: { enabled } });
      setStatus({ ...status, enabled });
      notify(enabled ? '跟打器内置入口已启用' : '跟打器内置入口已关闭，练习数据已保留');
      await refresh().catch(() => undefined);
      await checkStatus();
    } catch (e) {
      notify(`跟打器设置未保存：${(e as Error).message}`);
    } finally {
      operation.current = false;
      setSaving(false);
    }
  };

  const launch = async () => {
    if (!status?.enabled || !status.installed || operation.current) return;
    operation.current = true;
    setOpening(true);
    try {
      await openTrainer();
    } catch (e) {
      notify((e as Error).message);
      await checkStatus();
    } finally {
      operation.current = false;
      setOpening(false);
    }
  };

  const busy = saving || opening;
  return (
    <section className="card" aria-label="跟打器设置" style={{ padding: '18px 20px', marginBottom: 16 }}>
      <div className="card-head" style={{ gap: 16, flexWrap: 'wrap' }}>
        <div>
          <div className="card-title" style={{ display: 'flex', alignItems: 'center', gap: 8 }}><Icon name="keyboard" /> 跟打练习</div>
          <div style={{ color: 'var(--text3)', fontSize: 12, marginTop: 6 }}>
            关闭后停用虎符内置启动入口与快捷键，保留练习数据；已打开的窗口可继续使用，也可从开始菜单独立打开。
          </div>
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: 12, flexWrap: 'wrap' }}>
          <span style={{ color: 'var(--text2)', fontSize: 12 }}>{saving ? '保存中…' : '启用跟打器'}</span>
          <button type="button" className={`switch${status?.enabled ? ' on' : ''}`}
            role="switch" aria-checked={status?.enabled ?? false} aria-label="启用跟打器"
            disabled={!status || !data.online || busy || checking} onClick={() => void toggle()} />
          <button type="button" className="btn primary" title="Ctrl+Alt+T"
            disabled={!status?.enabled || !status.installed || !data.online || busy || checking}
            onClick={() => void launch()}>{opening ? '正在打开…' : '打开跟打器'}</button>
        </div>
      </div>
      <div role="status" aria-live="polite" style={{ color: error ? 'var(--red)' : 'var(--text3)', fontSize: 12, marginTop: 10 }}>
        {checking ? '正在检查跟打器…' : error || (!status?.installed
          ? '跟打器组件尚未安装，请更新完整安装包。'
          : status.enabled ? '已启用 · Ctrl+Alt+T 打开练习窗口 · 开关立即保存' : '虎符内置入口已关闭 · Ctrl+Alt+T 已停用 · 练习数据保留')}
        {!checking && error && data.online && !unsupported ? <button type="button" className="btn ghost" style={{ marginLeft: 8 }} onClick={() => void checkStatus()}>重新检查</button> : null}
      </div>
      <div style={{ color: 'var(--text3)', fontSize: 12, marginTop: 8 }}>
        内置练习语料源自 zhouzhou-typing（Apache-2.0）。
      </div>
    </section>
  );
}
