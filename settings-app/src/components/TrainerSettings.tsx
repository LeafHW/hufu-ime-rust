import { useCallback, useEffect, useRef, useState } from 'react';
import type { Bootstrap } from '../types';
import { updateConfig } from '../api';
import { getTrainerStatus, openTrainer, TrainerUnsupportedError, type TrainerStatus } from '../trainerApi';
import { DEFAULT_TRAINER_HOTKEY, hotkeyMainKey, prettyHotkey } from '../trainerApi';
import { Icon } from './Icon';

/** 当前生效的全局老板键（config 未回该字段时按服务端默认 ctrl+alt+g 展示）。 */
function effectiveHotkey(data: Bootstrap): string {
  const raw = data.config.trainer?.hotkey;
  return typeof raw === 'string' ? raw : DEFAULT_TRAINER_HOTKEY;
}

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
  const hotkeyPretty = prettyHotkey(effectiveHotkey(data));
  return (
    <section className="card" aria-label="跟打器设置" style={{ padding: '18px 20px', marginBottom: 16 }}>
      <div className="card-head" style={{ gap: 16, flexWrap: 'wrap' }}>
        <div>
          <div className="card-title" style={{ display: 'flex', alignItems: 'center', gap: 8 }}><Icon name="keyboard" /> 跟打练习</div>
          <div style={{ color: 'var(--text3)', fontSize: 12, marginTop: 6 }}>
            关闭后停用虎符内置启动入口与全局老板键，保留练习数据；已打开的窗口可继续使用，也可从开始菜单独立打开。
          </div>
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: 12, flexWrap: 'wrap' }}>
          <span style={{ color: 'var(--text2)', fontSize: 12 }}>{saving ? '保存中…' : '启用跟打器'}</span>
          <button type="button" className={`switch${status?.enabled ? ' on' : ''}`}
            role="switch" aria-checked={status?.enabled ?? false} aria-label="启用跟打器"
            disabled={!status || !data.online || busy || checking} onClick={() => void toggle()} />
          <button type="button" className="btn primary" title={hotkeyPretty ? `全局老板键 ${hotkeyPretty}` : '未设置全局老板键'}
            disabled={!status?.enabled || !status.installed || !data.online || busy || checking}
            onClick={() => void launch()}>{opening ? '正在打开…' : '打开跟打器'}</button>
        </div>
      </div>
      <div role="status" aria-live="polite" style={{ color: error ? 'var(--red)' : 'var(--text3)', fontSize: 12, marginTop: 10 }}>
        {checking ? '正在检查跟打器…' : error || (!status?.installed
          ? '跟打器组件尚未安装，请更新完整安装包。'
          : status.enabled
            ? (hotkeyPretty ? `已启用 · ${hotkeyPretty} 打开练习窗口 · 开关立即保存` : '已启用 · 全局老板键未设置 · 开关立即保存')
            : '虎符内置入口已关闭 · 全局老板键已停用 · 练习数据保留')}
        {!checking && error && data.online && !unsupported ? <button type="button" className="btn ghost" style={{ marginLeft: 8 }} onClick={() => void checkStatus()}>重新检查</button> : null}
      </div>
      <div style={{ color: 'var(--text3)', fontSize: 12, marginTop: 8 }}>
        内置练习语料源自 zhouzhou-typing（Apache-2.0）。
      </div>
    </section>
  );
}

/* ── 【整改10 2026-11-08】全局老板键（trainer.hotkey）───────────────────
   录制器：点「录制热键」进入监听（捕获 window keydown，按住修饰键时实时
   预览当前组合），Esc 取消；有效组合 = 修饰键 ≥1（Ctrl/Alt/Shift/Win，
   Win 键被 OS 截获、组合里的 Meta 映射为 win）+ 主键 a-z/0-9/f1-f12。
   保存 = updateConfig 深合并 trainer.hotkey（小写加号串）；
   清除 = 置空串保存（=禁用老板键）。 */
export function TrainerHotkeySettings({ data, notify, refresh }: {
  data: Bootstrap;
  notify: (message: string) => void;
  refresh: () => Promise<unknown>;
}) {
  const effective = effectiveHotkey(data);
  const [listening, setListening] = useState(false);
  /** 监听中/已捕获的实时显示（如 "Ctrl + Alt + G"）。 */
  const [preview, setPreview] = useState('');
  /** 已捕获待保存的小写加号串；'' = 无待保存内容。 */
  const [pending, setPending] = useState('');
  const [hint, setHint] = useState('');
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!listening) return;
    const onKeyDown = (e: KeyboardEvent) => {
      /* 录制态独占按键：拦掉默认行为与页面级快捷键（Ctrl+K 等）。 */
      e.preventDefault();
      e.stopPropagation();
      if (e.key === 'Escape') {
        setListening(false);
        setPreview('');
        setHint('');
        return;
      }
      const mods: string[] = [];
      if (e.ctrlKey) mods.push('ctrl');
      if (e.altKey) mods.push('alt');
      if (e.shiftKey) mods.push('shift');
      if (e.metaKey) mods.push('win');
      const main = hotkeyMainKey(e);
      if (!main) {
        /* 只按着修饰键（或按到不支持的键）：实时预览当前按住的组合。 */
        setPreview(mods.length ? `${prettyHotkey(mods.join('+'))} + …` : '…');
        setHint('');
        return;
      }
      if (!mods.length) {
        setPreview(prettyHotkey(main));
        setHint('需至少搭配一个修饰键（Ctrl / Alt / Shift / Win）');
        return;
      }
      const value = [...mods, main].join('+');
      setPending(value);
      setPreview(prettyHotkey(value));
      setHint('');
      setListening(false);
    };
    /* 窗口失焦（如按 Win 键弹出开始菜单）时取消录制。 */
    const onBlur = () => {
      setListening(false);
      setPreview('');
      setHint('');
    };
    window.addEventListener('keydown', onKeyDown, true);
    window.addEventListener('blur', onBlur);
    return () => {
      window.removeEventListener('keydown', onKeyDown, true);
      window.removeEventListener('blur', onBlur);
    };
  }, [listening]);

  const save = async () => {
    if (!pending || saving) return;
    setSaving(true);
    try {
      await updateConfig({ trainer: { hotkey: pending } });
      notify(`全局老板键已保存：${prettyHotkey(pending)}`);
      setPending('');
      setPreview('');
      setHint('');
      await refresh().catch(() => undefined);
    } catch (e) {
      notify(`全局老板键未保存：${(e as Error).message}`);
    } finally {
      setSaving(false);
    }
  };

  const clear = async () => {
    if (saving) return;
    setSaving(true);
    try {
      await updateConfig({ trainer: { hotkey: '' } });
      notify('全局老板键已清除，组合键不再唤出跟打器');
      setListening(false);
      setPending('');
      setPreview('');
      setHint('');
      await refresh().catch(() => undefined);
    } catch (e) {
      notify(`全局老板键未保存：${(e as Error).message}`);
    } finally {
      setSaving(false);
    }
  };

  const effectivePretty = prettyHotkey(effective);
  const status = listening
    ? `监听中：${preview || '等待按键…'}（Esc 取消）`
    : pending
      ? (hint || `已捕获：${preview}，点「保存」后生效`)
      : hint || (effectivePretty
        ? `全局老板键：按一次打开跟打器，再按一次关闭。当前生效 ${effectivePretty}`
        : '未设置全局老板键；点「录制热键」录入组合键后保存');

  return (
    <section className="card" aria-label="全局老板键设置" style={{ padding: '18px 20px' }}>
      <div className="card-head" style={{ gap: 16, flexWrap: 'wrap' }}>
        <div>
          <div className="card-title" style={{ display: 'flex', alignItems: 'center', gap: 8 }}><Icon name="play" /> 全局老板键</div>
          <div style={{ color: 'var(--text3)', fontSize: 12, marginTop: 6 }}>
            全局老板键：按一次打开跟打器，再按一次关闭。组合需含 Ctrl / Alt / Shift / Win 至少一个修饰键，主键支持字母、数字、F1–F12。
          </div>
        </div>
      </div>
      <div style={{ display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap', marginTop: 12 }}>
        <span style={{ color: 'var(--text2)', fontSize: 12, flex: 'none' }}>当前生效</span>
        <code className="ip-code" style={{ minWidth: 110, textAlign: 'center' }}>
          {listening || pending ? (preview || '…') : (effectivePretty || '未设置')}
        </code>
        <button
          type="button"
          className={`btn sm${listening ? ' goldline' : ' outline'}`}
          disabled={saving}
          onClick={() => {
            if (listening) {
              setListening(false);
              setPreview('');
              setHint('');
              return;
            }
            setPending('');
            setPreview('');
            setHint('');
            setListening(true);
          }}
        >
          {listening ? '按下组合键…' : '录制热键'}
        </button>
        <button type="button" className="btn sm primary" disabled={!pending || saving || !data.online} onClick={() => void save()}>保存</button>
        <button type="button" className="btn sm ghost" disabled={saving || (!effective && !pending) || !data.online} onClick={() => void clear()}>清除</button>
      </div>
      <div role="status" aria-live="polite" style={{ color: 'var(--text3)', fontSize: 12, marginTop: 10 }}>
        {data.online ? status : '虎符服务未连接，连接后可设置全局老板键。'}
      </div>
    </section>
  );
}
