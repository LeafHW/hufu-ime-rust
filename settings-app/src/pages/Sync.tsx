import { useEffect, useRef, useState } from 'react';
import type { Bootstrap } from '../types';
import { syncNow } from '../api';
import { Icon } from '../components/Icon';
import { Badge, PageHead, Switch } from '../components/ui';
import { useDirtyGuard } from '../dirtyGuard';
import '../styles/sync-sheet.css';

import { fetchWithTimeout } from '../api';
type Json = Record<string, any>;
type SheetKind = 'cloud' | 'trash';
type ConfirmState = { title: string; run: () => Promise<void> } | null;

function fmtTime(ts: number): string {
  if (!ts) return '尚未同步';
  try {
    return new Date(ts * 1000).toLocaleString('zh-CN', {
      month: 'numeric',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  } catch {
    return '—';
  }
}

function fmtMs(ms: number): string {
  if (!ms) return '—';
  try {
    return new Date(ms).toLocaleString('zh-CN', {
      month: 'numeric',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  } catch {
    return '—';
  }
}

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

/* 假状态清理①：进度文案沿用旧设置页公式（progress.cur/total + up/down 字节数）。 */
function fmtMB(b: unknown): string {
  const n = Number(b) || 0;
  return n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : `${(n / 1024).toFixed(1)} KB`;
}

async function getJson(path: string): Promise<Json> {
  const res = await fetchWithTimeout(path);
  const body = (await res.json().catch(() => ({}))) as Json;
  if (!res.ok) throw new Error(body.error || `HTTP ${res.status}`);
  return body;
}

async function postJson(path: string, payload: Json, timeoutMs?: number): Promise<Json> {
  const res = await fetchWithTimeout(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  }, timeoutMs);
  const body = (await res.json().catch(() => ({}))) as Json;
  if (!res.ok) throw new Error(body.error || `HTTP ${res.status}`);
  if (body.ok === false) throw new Error(body.msg || '操作失败');
  return body;
}

function countFiles(entry: unknown): number {
  if (!entry || typeof entry !== 'object') return 0;
  const files = (entry as Json).files;
  return files && typeof files === 'object' ? Object.keys(files).length : 0;
}

export function Sync({ data, notify }: { data: Bootstrap; notify: (msg: string) => void }) {
  const { sync, syncSchemes } = data;
  const [selected, setSelected] = useState<Set<string>>(
    new Set(syncSchemes.schemes.filter((s) => s.selected).map((s) => s.name)),
  );
  const [running, setRunning] = useState(false);
  /* 【互斥修复 2026-11】原实现是单个 busy 字符串：保存并测试（busy='save-test'）
     飞行中点「云端文件」，loadSheet 会把 busy 覆盖成 'sheet'，读完后
     setBusy(null) 把保存中的锁一并清空 → 保存按钮复活、可重复提交。
     改为按操作 key 的集合，finally 只清自己那个 key，互不影响。 */
  const [busyKeys, setBusyKeys] = useState<ReadonlySet<string>>(() => new Set());
  /* state 要到下一次渲染才更新，同一轮里的连续操作会都看到空集合；
     用 ref 镜像做同步判断，保证真正互斥。 */
  const busyRef = useRef<Set<string>>(new Set());
  const syncBusyKeys = () => setBusyKeys(new Set(busyRef.current));
  const beginBusy = (key: string) => {
    busyRef.current.add(key);
    syncBusyKeys();
  };
  const endBusy = (key: string) => {
    /* 只删自己那个 key：其它正在进行的操作（如 'action'）必须保持锁定。 */
    busyRef.current.delete(key);
    syncBusyKeys();
  };
  /** 无 key = 是否有任何操作在飞行中；带 key = 该操作是否在飞行中。 */
  const isBusy = (key?: string) => (key ? busyRef.current.has(key) : busyRef.current.size > 0);
  /** 所有异步操作统一入口：任一操作飞行中直接拒绝，finally 只释放自己。 */
  const withBusy = async (key: string, fn: () => Promise<void>) => {
    if (isBusy()) {
      notify('上一个操作还在进行，请稍候');
      return;
    }
    beginBusy(key);
    try {
      await fn();
    } finally {
      endBusy(key);
    }
  };
  /** 任一操作进行中：UI 禁用/文案判断沿用旧语义，数据源从字符串换成集合。 */
  const busy = busyKeys.size > 0;
  const [server, setServer] = useState(sync.url);
  const [user, setUser] = useState(sync.user);
  const [device, setDevice] = useState(sync.device);
  const [password, setPassword] = useState('');
  const [enabled, setEnabled] = useState(sync.enabled);
  const [sheet, setSheet] = useState<SheetKind | null>(null);
  const [sheetData, setSheetData] = useState<Json | null>(null);
  const [confirm, setConfirm] = useState<ConfirmState>(null);
  const [guideOpen, setGuideOpen] = useState(false);

  /* 【合符动效 2026-11 用户定稿】状态卡虎符要区分「分符 / 合符」：
     · 未配置 = 分符：两半分开、无辉光、圆环不转；
     · 保存并测试成功后 → 两半先合拢 → 再浮现辉光 → 圆环才开始转（分三段播放）；
     · 页面打开时就已配置（刷新/重进）：直接合符终态，不重放动画；
     · 退出帐户/配置被清空 → 反向拆开回去。
     另外两处「用户没想到」的细节一并处理：连接异常时辉光转琥珀且圆环停转；
     离线时整体压暗停转（避免"在转但其实没连上"的误导）。 */
  type TallyPhase = 'split' | 'merging' | 'glow' | 'joined';
  const [tallyPhase, setTallyPhase] = useState<TallyPhase>(sync.configured ? 'joined' : 'split');
  const tallyTimers = useRef<number[]>([]);
  const clearTallyTimers = () => {
    tallyTimers.current.forEach((t) => window.clearTimeout(t));
    tallyTimers.current = [];
  };
  useEffect(() => clearTallyTimers, []);
  useEffect(() => {
    if (!sync.configured) {
      clearTallyTimers();
      setTallyPhase('split');
    } else {
      // 已经是合符就不打断正在播的动画（外部状态变化只兜底分符→合符）
      setTallyPhase((p) => (p === 'split' ? 'joined' : p));
    }
  }, [sync.configured]);
  /* 【分符/合符文字动效 2026-11】文字跟着图标一起动：
     合符 → 两个字从两侧向中间合拢；分符 → 两个字从中间裂开。
     旧标签叠一层播出场动画（绝对定位，不撑布局），520ms 后移除。 */
  const labelText = tallyPhase === 'split' ? '分符 · 待连接' : '合符 · 双机同步';
  const [labelOld, setLabelOld] = useState<string | null>(null);
  const labelPrev = useRef(labelText);
  useEffect(() => {
    if (labelText === labelPrev.current) return;
    setLabelOld(labelPrev.current);
    labelPrev.current = labelText;
    const t = window.setTimeout(() => setLabelOld(null), 520);
    return () => window.clearTimeout(t);
  }, [labelText]);
  const renderLabel = (text: string, layer: 'old' | 'new') => {
    const core = text.slice(0, 2);
    const suffix = text.slice(2);
    const dir = text.startsWith('分') ? 'split' : 'join';
    return (
      <span className={`lab ${layer} dir-${dir}`} key={text + layer} aria-hidden={layer === 'old' || undefined}>
        <span className="core">
          {[core[0], core[1]].map((c, i) => (
            <span className="ch" key={i}>{c}</span>
          ))}
        </span>
        <span className="suffix">{suffix}</span>
      </span>
    );
  };

  const playMerge = () => {
    clearTallyTimers();
    setTallyPhase('merging');
    tallyTimers.current.push(window.setTimeout(() => setTallyPhase('glow'), 660));
    tallyTimers.current.push(window.setTimeout(() => setTallyPhase('joined'), 1300));
  };

  useEffect(() => {
    const syncGuide = () => {
      const h = (location.hash || '').replace('#', '').replace(/^\//, '');
      const parts = h.split('/');
      if (parts[0] === 'sync' && parts[1] === 'guide') setGuideOpen(true);
    };
    syncGuide();
    window.addEventListener('hashchange', syncGuide);
    return () => window.removeEventListener('hashchange', syncGuide);
  }, []);

  /* RT-05：Esc 关闭弹层（确认框 > 抽屉 > 教程，逐层退）。 */
  useEffect(() => {
    if (!confirm && !sheet && !guideOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      e.stopPropagation();
      if (confirm) setConfirm(null);
      else if (sheet) setSheet(null);
      else setGuideOpen(false);
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [confirm, sheet, guideOpen]);

  const formSig = useRef('');
  const selectedSig = useRef('');
  /* 【脏状态修复 2026-11】表单基线 = 服务端已确认的连接三字段。若直接拿 props
     比较，保存成功后 Bootstrap 轮询（10s）回来之前会一直判定为脏、切页时误弹
     确认；因此保存成功时把基线前移到刚提交的值。 */
  const [formBase, setFormBase] = useState(() => ({ url: sync.url, user: sync.user, device: sync.device }));
  /* 表单脏判断：连接三字段与已确认基线不一致（切页守卫 + 「只测试已保存配置」提示用）。 */
  const formDirty = server !== formBase.url || user !== formBase.user || device !== formBase.device;
  /* 【云同步表单进脏守卫】此前服务器/账号/密码/设备名改完不保存直接切页会静默
     丢弃；密码框不在 formDirty 里（不回显），有输入也算脏。 */
  useDirtyGuard('sync', '云同步', formDirty || password.length > 0);

  /* 表单回灌：只在用户没有未保存修改时把 props 灌回输入框。
     【缺陷修复】旧实现只看 props 签名变化就回灌：任一端点失败时 api 层会回退
     空值，签名一变就把用户正在敲的服务器/账号/密码清空。 */
  useEffect(() => {
    const sig = [sync.url, sync.user, sync.device].join('|');
    if (sig === formSig.current) return;
    formSig.current = sig;
    if (formDirty) return;
    /* 端点失败时 api 层回退空值，不能把空串灌进表单（会清掉用户已存的账号）。 */
    if (!sync.url && !sync.user) return;
    setServer(sync.url);
    setUser(sync.user);
    setDevice(sync.device);
    setFormBase({ url: sync.url, user: sync.user, device: sync.device });
  }, [sync.url, sync.user, sync.device, formDirty]);

  useEffect(() => {
    setEnabled(sync.enabled);
  }, [sync.enabled]);

  useEffect(() => {
    const sig = syncSchemes.schemes.map((s) => `${s.name}:${s.selected}`).join('|');
    if (sig === selectedSig.current) return;
    selectedSig.current = sig;
    setSelected(new Set(syncSchemes.schemes.filter((s) => s.selected).map((s) => s.name)));
  }, [syncSchemes]);

  const selectedSchemes = syncSchemes.schemes.filter((s) => selected.has(s.name));
  const files = selectedSchemes.reduce((a, b) => a + b.files, 0);
  /* SY-03：离线（api 层 online=false）时不再把 fallback 假数据当真实状态。 */
  const offline = !data.online;
  /* 合符动画一开跑，标题/状态点立即按"已配置"显示——否则图标已合符、
     文案还停在「未配置」，要等 10s 轮询才更新（用户可见的不一致）。 */
  const configuredNow = sync.configured || tallyPhase !== 'split';
  const status = offline
    ? '离线预览'
    : !configuredNow
      ? '未配置'
      : sync.last_error
        ? '同步异常'
        : sync.stopped
          ? '已停止'
          : sync.phase === 'syncing'
            ? '同步中'
            : '已同步';
  const dot = offline ? 'dot muted' : !configuredNow ? 'dot muted' : sync.last_error || sync.stopped ? 'dot red' : 'dot';

  /* 假状态清理①：真实进度（total=0 时按旧页公式给 3%/50% 占位），完成态写成绩单。 */
  const p = sync.progress;
  const moved = (Number(p?.up_files) || 0) + (Number(p?.down_files) || 0);
  const syncingNow = !offline && (sync.phase === 'syncing' || p?.active === true);
  const pct = syncingNow
    ? Number(p?.total) > 0
      ? Math.min(100, Math.round((Number(p?.cur) * 100) / Number(p?.total)))
      : moved
        ? 50
        : 3
    : 100;
  const progText = offline
    ? '离线'
    : syncingNow
      ? `同步中 ${p?.cur ?? 0}/${p?.total || '?'}（${pct}%）${p?.cur_name ? ` · ${p.cur_name}` : ''}` +
        (moved ? `（↑${p?.up_files} 个 ${fmtMB(p?.up_bytes)} / ↓${p?.down_files} 个 ${fmtMB(p?.down_bytes)}）` : '')
      : moved
        ? p && p.down_files > 0
          ? `✓ 同步完毕｜↑ ${p.up_files} 个（${fmtMB(p.up_bytes)}）↓ ${p.down_files} 个（${fmtMB(p.down_bytes)}）`
          : `✓ 上传完毕｜↑ ${p?.up_files} 个（${fmtMB(p?.up_bytes)}）`
        : sync.last_sync
          ? '✓ 同步完毕｜无文件改动'
          : '等待首轮同步';

  const run = async () => {
    if (running || offline || isBusy()) return;
    setRunning(true);
    try {
      await syncNow();
      notify('已触发同步');
    } catch (e) {
      notify((e as Error).message);
    } finally {
      setRunning(false);
    }
  };

  const saveAndTest = async (testOnly: boolean) => {
    if (offline) return;
    /* 【2026-11-02】空服务器/空账号绝不能提交：pass 走 __keep__ 时服务端会
       认为「保留密码」，配合空 user 会把已存账号清掉（端点抖动回灌空值后
       用户手快就会踩到）。 */
    if (!server.trim() || !user.trim()) {
      notify('请先填写服务器地址与账号');
      return;
    }
    /* 保存/测试共用一个 withBusy 包裹：无论成功失败，只有它自己释放锁；
       期间其它端点（云端文件/勾选保存等）一律被互斥挡回。 */
    await withBusy(testOnly ? 'test' : 'save-test', async () => {
      try {
        if (!testOnly) {
          await postJson('/api/sync/save', {
            url: server,
            user,
            pass: password || '__keep__',
            device,
          });
          setPassword('');
          /* 保存成功即与服务端对齐：基线前移到刚提交的值，脏守卫立即归位
             （Bootstrap 轮询回来前不再被误判为未保存）。 */
          setFormBase({ url: server, user, device });
          notify('连接设置已保存');
        }
        const result = await postJson('/api/sync/test', {}, 20000);
        const testedOk = result.ok !== false;
        notify(result.msg || (result.ok === false ? '连接失败' : '连接正常'));
        // 【合符动效】保存并测试成功 → 分符合拢，播完辉光/圆环再进入合符终态
        if (testedOk && !testOnly) playMerge();
      } catch (e) {
        notify((e as Error).message);
      }
    });
  };

  const toggleEnabled = async (next: boolean) => {
    if (offline) {
      notify('离线预览：无法修改同步设置');
      return;
    }
    await withBusy('enable', async () => {
      setEnabled(next);
      try {
        await postJson('/api/sync/enable', { enabled: next });
        notify(next ? '自动同步已开启' : '自动同步已暂停');
      } catch (e) {
        setEnabled(!next);
        notify((e as Error).message);
      }
    });
  };

  const saveSelection = async (andSync: boolean) => {
    if (offline) {
      notify('离线预览：无法保存同步方案');
      return;
    }
    await withBusy('selection', async () => {
      try {
        await postJson('/api/sync/select', { selected: [...selected] });
        selectedSig.current = syncSchemes.schemes.map((s) => `${s.name}:${selected.has(s.name)}`).join('|');
        notify('同步方案勾选已保存');
        if (andSync) {
          await syncNow();
          notify('已保存勾选并触发同步');
        }
      } catch (e) {
        notify((e as Error).message);
      }
    });
  };

  /* 只负责「取数并落状态」：读取指示占用 'sheet' key，但不做入口互斥判断——
     确认动作（'action'）里刷新抽屉时锁还在自己手上，不能被自己挡住。 */
  const fetchSheet = async (kind: SheetKind) => {
    setSheetData(null);
    beginBusy('sheet');
    try {
      const result = await getJson(kind === 'cloud' ? '/api/sync/cloud' : '/api/sync/trash');
      setSheetData(result);
    } catch (e) {
      setSheetData({ ok: false, msg: (e as Error).message });
    } finally {
      /* 只清 'sheet'：外层 'action'/其它 key 的锁保持到各自 finally。 */
      endBusy('sheet');
    }
  };

  const loadSheet = async (kind: SheetKind) => {
    if (!data.online) {
      notify('离线预览：无法读取云端数据');
      return;
    }
    /* 互斥入口：保存/测试等操作飞行中不再叠加云端读取。
       旧实现正是在这里把 busy 覆盖成 'sheet'，读完后无条件清空，把保存锁
       释放掉，导致保存按钮复活、可重复提交。 */
    if (isBusy()) return;
    setSheet(kind);
    await fetchSheet(kind);
  };

  const ask = (title: string, action: () => Promise<void>) => setConfirm({ title, run: action });

  const runConfirm = async () => {
    if (!confirm) return;
    const action = confirm.run;
    setConfirm(null);
    await withBusy('action', async () => {
      try {
        await action();
      } catch (e) {
        notify((e as Error).message);
      }
    });
  };

  const deleteCloud = (scheme?: string) =>
    ask(scheme ? `删除云端方案「${scheme}」？本机文件不会删除。` : '删除云端全部同步数据？本机文件不会删除。', async () => {
      /* SY-02：server 删除超 30s 时返回 202 {pending:true, msg}——须如实提示，不谎报已删除。 */
      const r = (await postJson('/api/sync/cloud/delete', scheme ? { scheme } : {})) as { pending?: boolean; msg?: string };
      notify(r.pending ? r.msg || '删除仍在进行，稍候刷新查看' : scheme ? `云端方案「${scheme}」已删除` : '云端同步数据已删除');
      if (sheet === 'cloud') await fetchSheet('cloud');
    });

  const logout = () =>
    ask('退出同步帐户？云端数据会保留，本机文件不会删除。', async () => {
      const result = await postJson('/api/sync/logout', {});
      notify(result.msg || '已退出同步帐户');
      setSheet(null);
      /* 退出后本机凭据已清空：表单与基线一起清空，避免残留「未保存修改」
         在切页时误弹确认（服务器侧的值由下一次 Bootstrap 回灌）。 */
      setPassword('');
      setServer('');
      setUser('');
      setDevice('');
      setFormBase({ url: '', user: '', device: '' });
    });

  /* SY-04：还原是破坏性动作（覆盖本机同名文件 + 传播到云端），必须确认。 */
  const restoreTrash = (scheme: string, trash: string) =>
    ask(
      `将回收站文件「${trash}」还原到方案「${scheme}」？\n会覆盖本机同名文件，并以还原后的新改动同步到云端。`,
      async () => {
        const result = await postJson('/api/sync/trash/restore', { scheme, trash });
        notify(result.msg || '已还原');
        if (sheet === 'trash') await fetchSheet('trash');
      },
    );

  const clearTrash = () =>
    ask('清空同步回收站？此操作不可恢复。', async () => {
      const result = await postJson('/api/sync/trash/clear', {});
      notify(result.msg || '回收站已清空');
      if (sheet === 'trash') await fetchSheet('trash');
    });

  const sheetTitle = sheet === 'cloud' ? '云端文件' : '同步回收站';

  return (
    <>
      <PageHead
        title="云同步"
        sub={
          offline ? (
            <>离线预览 · 无法连接设置服务</>
          ) : (
            <>
              WebDAV · {hostOf(sync.url)}<span style={{ color: 'var(--text3)' }}>·</span>设备「{device || '—'}」
            </>
          )
        }
        actions={
          <>
            <button type="button" className="btn outline" onClick={() => setGuideOpen(true)}>
              <Icon name="help" />
              教程
            </button>
            <button type="button" className="btn ghost" disabled={offline || busy} onClick={() => loadSheet('cloud')}>
              <Icon name="sliders" />
              管理
            </button>
            <button type="button" className="btn primary" disabled={offline || busy} onClick={run}>
              <Icon name="refresh" />
              {running ? '同步中…' : '立即同步'}
            </button>
          </>
        }
      />

      <div className="card sync-hero">
        <div
          className={
            'sync-face is-' +
            tallyPhase +
            (offline ? ' is-offline' : '') +
            (sync.configured && (sync.last_error || sync.stopped) ? ' is-trouble' : '')
          }
          aria-label={tallyPhase === 'split' ? '虎符分符（尚未连接）' : '虎符合符（已连接）'}
        >
          <span className="sync-halo" />
          <span className="sync-ring" />
          <span className="sync-flash" />
          <svg
            className="sync-tally"
            viewBox="0 0 80 140"
            role="img"
            aria-label={tallyPhase === 'split' ? '虎符分符' : '虎符合符'}
          >
            <defs>
              <linearGradient id="tallyGold" x1="0" y1="0" x2="1" y2="1">
                <stop offset="0" stopColor="#f9e4a8" />
                <stop offset="0.28" stopColor="#e2b45a" />
                <stop offset="0.55" stopColor="#c58a2c" />
                <stop offset="0.8" stopColor="#f0c978" />
                <stop offset="1" stopColor="#9a681d" />
              </linearGradient>
              <linearGradient id="tallyEdge" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0" stopColor="#fff1c9" />
                <stop offset="1" stopColor="#8f5f18" />
              </linearGradient>
              <filter id="tallyGlow" x="-60%" y="-30%" width="220%" height="160%">
                <feGaussianBlur stdDeviation="2.2" result="b" />
                <feMerge><feMergeNode in="b" /><feMergeNode in="SourceGraphic" /></feMerge>
              </filter>
            </defs>
            {/* 左半：分符时向左分开；合符时归位。横纹只画到接缝为止，分开后不穿帮 */}
            <g className="tally-half tally-left" filter={tallyPhase === 'split' ? 'none' : 'url(#tallyGlow)'}>
              <path d="M40 8 H30 Q22 8 22 16 V36 L14 44 L22 52 V72 L14 80 L22 88 V112 Q22 132 30 132 H40 Z"
                fill="url(#tallyGold)" stroke="url(#tallyEdge)" strokeWidth="1.4" />
              <path d="M24 25 H40 M24 70 H40 M24 115 H40" stroke="#6e4713" strokeWidth="2" opacity="0.5" />
              <path d="M28 33 H39.2 M28 104 H39.2" stroke="#fff0bd" strokeWidth="1" opacity="0.45" />
            </g>
            {/* 右半 */}
            <g className="tally-half tally-right" filter={tallyPhase === 'split' ? 'none' : 'url(#tallyGlow)'}>
              <path d="M40 8 H50 Q58 8 58 16 V36 L66 44 L58 52 V72 L66 80 L58 88 V112 Q58 132 50 132 H40 Z"
                fill="url(#tallyGold)" stroke="url(#tallyEdge)" strokeWidth="1.4" />
              <path d="M40 25 H56 M40 70 H56 M40 115 H56" stroke="#6e4713" strokeWidth="2" opacity="0.5" />
              <path d="M40.8 33 H52 M40.8 104 H52" stroke="#fff0bd" strokeWidth="1" opacity="0.45" />
            </g>
            {/* 接缝：分符时隐藏（两半是断开的），合符时出现 */}
            <g className="tally-seam">
              <line x1="40" y1="10" x2="40" y2="130" stroke="#64400f" strokeWidth="2" />
              <line x1="41" y1="12" x2="41" y2="128" stroke="#ffe5a0" strokeWidth="0.8" opacity="0.55" />
              <circle cx="40" cy="8" r="2.4" fill="#ffe5a0" opacity="0.8" />
              <circle cx="40" cy="132" r="2.4" fill="#ffe5a0" opacity="0.8" />
            </g>
          </svg>
        </div>
        <div className="hero-copy">
          <div
            className={`k tally-label${tallyPhase === 'glow' ? ' is-glow' : ''}`}
            data-joined={tallyPhase !== 'split'}
          >
            {labelOld ? renderLabel(labelOld, 'old') : null}
            {renderLabel(labelText, 'new')}
          </div>
          <h2>
            {status}{' '}
            {offline ? (
              <Badge tone="gray">离线</Badge>
            ) : (
              <Badge tone={sync.last_error ? 'gray' : 'green'}>{sync.last_error ? '检查连接' : '连接正常'}</Badge>
            )}
          </h2>
          <div className="sub" title={sync.last_error || undefined}>
            {sync.last_error ? <><span className="dot red" /> {sync.last_error} · </> : null}
            上次 {fmtTime(sync.last_sync)} · 每 2 分钟自动一次 · {offline ? '等待服务恢复' : enabled ? '自动同步已开启' : '自动同步已关闭'}
          </div>
        </div>
        <div className="hero-stats">
          <div className="hero-stat"><span className="k">设备</span><span className="v">{offline ? '—' : device || '—'}</span></div>
          <div className="hero-stat"><span className="k">方案</span><span className="v">{offline ? '—' : <>{selected.size} <small>/ {syncSchemes.schemes.length}</small></>}</span></div>
          <div className="hero-stat"><span className="k">同步文件</span><span className="v">{offline ? '—' : <>{files} 个 <small>· {selectedSchemes.length} 个方案</small></>}</span></div>
        </div>
        <div className="hero-foot">
          <span className={dot} />
          <span>自动同步 · 每 2 分钟</span>
          <div className="hbar"><i style={{ width: `${offline ? 0 : pct}%` }} /></div>
          <span title={progText}>{progText}</span>
        </div>
      </div>

      <div className="sync-grid">
        <div className="card sync-card">
          <div className="card-head">
            <span className="card-title">WebDAV 连接</span>
            <div style={{ display: 'flex', alignItems: 'center', gap: 7 }}>
              <span className="more">自动同步</span>
              <Switch on={enabled} onChange={toggleEnabled} title="自动同步" />
              <Badge tone={offline ? 'gray' : sync.last_error ? 'gray' : 'green'}>{offline ? '离线' : sync.last_error ? '异常' : '连接正常'}</Badge>
            </div>
          </div>
          {offline ? (
            <div className="sync-note" style={{ marginTop: 10 }}>
              <Icon name="info" />
              离线预览：无法连接设置服务，当前显示的不是真实配置；保存、测试与同步均已禁用。
            </div>
          ) : null}
          <div className="form-grid">
            <div className="form-cell" style={{ gridColumn: 'span 2' }}>
              <span className="label">服务器</span>
              <input className="field-input" value={server} disabled={offline} onChange={(e) => setServer(e.target.value)} />
            </div>
            <div className="form-cell">
              <span className="label">账号</span>
              <input className="field-input" value={user} disabled={offline} onChange={(e) => setUser(e.target.value)} />
            </div>
            <div className="form-cell">
              <span className="label">应用密码</span>
              <input
                className="field-input"
                type="password"
                value={password}
                placeholder="已保存（重新输入以修改）"
                disabled={offline}
                onChange={(e) => setPassword(e.target.value)}
              />
            </div>
            <div className="form-cell">
              <span className="label">设备名</span>
              <input className="field-input" value={device} disabled={offline} onChange={(e) => setDevice(e.target.value)} />
            </div>
            <div className="form-cell">
              <span className="label">远端目录</span>
              <input className="field-input" value="hufu-sync/ · 自动" readOnly />
            </div>
          </div>
          <div className="sync-note">
            <Icon name="lock" />
            应用密码不是登录密码；
            <span style={{ color: 'var(--gold2)' }} onClick={() => setGuideOpen(true)}>
              如何获取？
            </span>
          </div>
          <div className="sync-actions">
            <button type="button" className="btn primary" disabled={busy || offline} onClick={() => saveAndTest(false)}>
              {busyKeys.has('save-test') ? '保存中…' : '保存并测试'}
            </button>
            <button
              type="button"
              className="btn"
              disabled={busy || offline}
              title={formDirty ? '表单已修改：本按钮只测已保存的配置，请先「保存并测试」' : '测试已保存配置的连接'}
              onClick={() => saveAndTest(true)}
            >
              {busyKeys.has('test') ? '测试中…' : '只测试已保存配置'}
            </button>
          </div>
          <div className="sync-tip">
            <Icon name="shield" />
            <div>
              <b>数据只经你自己的网盘</b>
              <span>虎符官方不设服务器；同步范围：方案文件夹 + 设置 + 自带皮肤调参。</span>
            </div>
          </div>
          <div style={{ marginTop: 'auto', paddingTop: 14, borderTop: '1px solid var(--line)', display: 'flex', gap: 16, fontSize: 10, color: 'var(--text3)' }}>
            <span onClick={() => loadSheet('cloud')} title="设备列表在云端文件面板（每台设备 30 天未同步自动移出）"><b style={{ color: 'var(--text2)', fontWeight: 500 }}>管理设备</b></span>
            <span
              onClick={async () => {
                try {
                  const r = await fetch('/api/sync/backup', { cache: 'no-store' });
                  if (!r.ok) throw new Error(String(r.status));
                  const b = (await r.json()) as { path?: string; rounds?: string[] | number };
                  const n = Array.isArray(b.rounds) ? b.rounds.length : (b.rounds ?? 0);
                  notify(`每次同步前自动整轮备份（保留 10 轮）：${b.path || '数据\\同步备份\\'}（当前 ${n} 轮）`);
                } catch (e) {
                  notify(`备份信息读取失败：${(e as Error).message}`);
                }
              }}
              title="查看本地自动备份（每次同步前整轮备份，保留 10 轮）"
            ><b style={{ color: 'var(--text2)', fontWeight: 500 }}>本地备份</b></span>
            <span onClick={() => loadSheet('cloud')}><b style={{ color: 'var(--text2)', fontWeight: 500 }}>云端文件</b></span>
            <span onClick={() => loadSheet('trash')}><b style={{ color: 'var(--text2)', fontWeight: 500 }}>回收站</b> · 30 天</span>
          </div>
        </div>

        <div className="card sync-card">
          <div className="card-head">
            <span className="card-title">同步方案</span>
            <span className="more">{selected.size} / {syncSchemes.schemes.length} 已勾选</span>
          </div>
          <div style={{ marginTop: 6 }}>
            {syncSchemes.schemes.map((s) => {
              const on = selected.has(s.name);
              return (
                <div className="scheme-row" key={s.name}>
                  <button
                    type="button"
                    className={`checkbox${on ? ' on' : ''}`}
                    aria-pressed={on}
                    disabled={offline}
                    onClick={() => {
                      const next = new Set(selected);
                      if (on) next.delete(s.name);
                      else next.add(s.name);
                      setSelected(next);
                    }}
                  >
                    {on ? <Icon name="check" /> : null}
                  </button>
                  <div>
                    <div className="scheme-name" style={{ color: on ? undefined : 'var(--text3)' }}>{s.name}</div>
                    <div className="scheme-sub">
                      {on ? `${s.files} 个文件 · ${s.cloud ? '云端已有' : '仅本机'}` : '未参与同步'}
                    </div>
                  </div>
                  {s.cloud ? <Badge tone={on ? 'green' : 'gray'}>{on ? '已同步' : '云端'}</Badge> : null}
                </div>
              );
            })}
          </div>
          <div className="scheme-foot">
            <button type="button" className="btn primary" disabled={busy || offline} onClick={() => saveSelection(true)}>
              {busyKeys.has('selection') ? '保存中…' : '保存勾选并同步'}
            </button>
            <div className="mini-links">
              <span onClick={() => loadSheet('cloud')}><b>管理</b> 设备与文件</span>
              <span onClick={() => loadSheet('trash')}><b>回收站</b> 还原旧文件</span>
            </div>
          </div>
        </div>
      </div>

      {sheet ? (
        <div
          className="sync-sheet"
          role="dialog"
          aria-modal="true"
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) setSheet(null);
          }}
        >
          <div className="card sync-sheet-card">
            <div className="sync-sheet-head">
              <div>
                <h3>{sheetTitle}</h3>
                <div className="sub">{busyKeys.has('sheet') ? '读取中…' : sheet === 'cloud' ? '只读云端，不触碰本机文件' : '30 天内的旧文件备份'}</div>
              </div>
              <button type="button" className="btn ghost" onClick={() => setSheet(null)}>关闭</button>
            </div>
            <div className="sync-sheet-body">
              {!sheetData ? (
                <div className="empty">读取中…</div>
              ) : sheetData.error ? (
                <div className="empty">{sheetData.error}</div>
              ) : sheetData.ok === false ? (
                <div className="empty">{sheetData.msg || '读取失败'}</div>
              ) : sheet === 'cloud' ? (
                !sheetData.exists ? (
                  <div className="empty">云端还没有同步数据</div>
                ) : (
                  <>
                    <div className="sheet-section">
                      <h4>设备</h4>
                      <div className="sheet-chips">
                        {Object.entries(sheetData.devices || {}).map(([name, ts]) => (
                          <span className="chip" key={name}>{name} · {fmtTime(Number(ts) || 0)}</span>
                        ))}
                        {Object.keys(sheetData.devices || {}).length === 0 ? <span className="empty">暂无</span> : null}
                      </div>
                    </div>
                    <div className="sheet-section">
                      <h4>云端方案</h4>
                      {Object.entries(sheetData.schemes || {}).map(([name, entry]) => (
                        <div className="sheet-row" key={name}>
                          <div>
                            <b>{name}</b>
                            <span>{countFiles(entry)} 个文件</span>
                          </div>
                          <div className="sheet-actions">
                            <button type="button" className="btn danger sm" disabled={busy} onClick={() => deleteCloud(name)}>删除</button>
                          </div>
                        </div>
                      ))}
                      {Object.keys(sheetData.schemes || {}).length === 0 ? <div className="empty">云端没有方案</div> : null}
                    </div>
                    <div className="sheet-foot">
                      <button type="button" className="btn danger" disabled={busy} onClick={() => deleteCloud()}>删除云端全部</button>
                      <button type="button" className="btn ghost" disabled={busy} onClick={logout}>退出帐户</button>
                    </div>
                  </>
                )
              ) : (
                <>
                  {Object.entries(sheetData.schemes || {}).map(([name, items]) => (
                    <div className="sheet-section" key={name}>
                      <h4>{name}</h4>
                      {(items as Json[]).map((item) => (
                        <div className="sheet-row" key={item.trash}>
                          <div>
                            <b>{item.name}</b>
                            <span>{fmtMs(Number(item.saved_ms) || 0)} · {Math.ceil((Number(item.size) || 0) / 1024)} KB</span>
                          </div>
                          <div className="sheet-actions">
                            <button type="button" className="btn sm" disabled={busy} onClick={() => restoreTrash(name, item.trash)}>还原</button>
                          </div>
                        </div>
                      ))}
                    </div>
                  ))}
                  {Object.keys(sheetData.schemes || {}).length === 0 ? <div className="empty">回收站是空的</div> : null}
                  <div className="sheet-foot">
                    <button type="button" className="btn danger" disabled={busy} onClick={clearTrash}>清空回收站</button>
                  </div>
                </>
              )}
            </div>
          </div>
        </div>
      ) : null}

      {guideOpen ? (
        <div
          className="sync-sheet"
          role="dialog"
          aria-modal="true"
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) setGuideOpen(false);
          }}
        >
          <div className="card sync-sheet-card sync-guide-card">
            <div className="sync-sheet-head">
              <div>
                <h3>云同步教程</h3>
                <div className="sub">坚果云客户端只用于取应用密码，取到后可卸载 · WebDAV 双机同步</div>
              </div>
              <button type="button" className="btn ghost" onClick={() => setGuideOpen(false)}>关闭</button>
            </div>
            <div className="sync-sheet-body sync-guide-body">
              <ol className="sync-guide-steps">
                <li>
                  <b>1. 获取坚果云应用密码</b>
                  <p>
                    坚果云目前需要先安装 Windows 客户端才能生成应用密码，网页端不直接提供。打开
                    <a className="sync-guide-link" href="https://www.jianguoyun.com/" target="_blank" rel="noreferrer">坚果云官网 · 网页版</a>
                    （<code>www.jianguoyun.com</code>）下载并登录客户端，进入 账户 → 安全选项 → 添加应用，生成一个<strong>应用密码</strong>。
                  </p>
                  <p className="sync-guide-tip">拿到密码后先保存好，客户端可以直接卸载；后续同步只需要服务器地址 + 账号 + 这串应用密码，与客户端是否安装无关。</p>
                </li>
                <li>
                  <b>2. 填写并测试连接</b>
                  <p>服务器填 <code>https://dav.jianguoyun.com/dav/</code>，账号填坚果云登录邮箱，密码填上一步的应用密码（不是登录密码）；设备名随意。点“保存并测试”，通过后自动同步会开启，远端目录固定为 <code>hufu-sync/</code>。</p>
                </li>
                <li>
                  <b>3. 勾选要同步的方案</b>
                  <p>在“同步方案”里勾选需要的方案（方案下的码表/用户词/用户调整会整案同步），点“保存勾选并同步”。未勾选的方案不会上传，也不会被云端覆盖。</p>
                </li>
                <li>
                  <b>4. 第二台设备</b>
                  <p>在另一台机器用同一账号登录，勾选相同方案，点“立即同步”拉取。冲突按文件 mtime 新者优先；首次接入建议先备份本机数据。</p>
                </li>
                <li>
                  <b>5. 查看与管理</b>
                  <p>“云端文件”可看远端方案和设备；“回收站”保留 30 天内被覆盖/删除的旧文件，可还原；“退出帐户”会清空本机凭据但保留云端与本机文件。</p>
                </li>
              </ol>
              <div className="sync-guide-note">
                <Icon name="shield" />
                <div>
                  <b>数据只经你自己的网盘</b>
                  <span>虎符没有官方同步服务器；凭据只存本机 config，导出设置时不会包含同步账号/密码。</span>
                </div>
              </div>
              <div className="sync-guide-actions">
                <button type="button" className="btn primary" onClick={() => setGuideOpen(false)}>知道了</button>
              </div>
            </div>
          </div>
        </div>
      ) : null}

      {confirm ? (
        <div className="sync-confirm" role="dialog" aria-modal="true">
          <div className="card sync-confirm-card">
            <h3>确认操作</h3>
            <p>{confirm.title}</p>
            <div className="sync-confirm-actions">
              <button type="button" className="btn ghost" onClick={() => setConfirm(null)}>取消</button>
              <button type="button" className="btn danger" onClick={runConfirm}>确认</button>
            </div>
          </div>
        </div>
      ) : null}
    </>
  );
}
