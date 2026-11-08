/* 【更新下载 2026-11-06】共享下载状态 hook + 进度卡片。
   关于页（完整卡片）与概览页（行内迷你进度）共用：server 侧任务是进程
   级的，设置页关掉重开、切换页面都能接着看进度。 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { Icon } from './Icon';
import { Badge } from './ui';
import { createSnapshotRequest } from '../snapshotRequest';
import {
  cancelUpdateDownload,
  fetchDownloadState,
  isDownloadActive,
  openDownloaded,
  startUpdateDownload,
  type DownloadPhase,
  type DownloadState,
} from '../updateCheck';

export interface UpdateDownloadApi {
  state: DownloadState | null;
  /** 请求级错误（启动/轮询/open 失败）；下载本身的失败在 state.error */
  error: string;
  openError: string;
  cancelling: boolean;
  /** 瞬时速度（字节/秒，指数平滑）；0=未知 */
  speed: number;
  /** 预计剩余秒数；null=未知 */
  etaSec: number | null;
  busy: boolean;
  done: boolean;
  start: () => Promise<void>;
  cancel: () => Promise<void>;
  open: (target: 'installer' | 'folder') => Promise<void>;
}

/** 本次页面会话里「点了下载」才在完成时自动定位文件；别的页面/上次会话
 *  直接看到完成态时不弹资源管理器（避免莫名其妙弹窗）。 */
let pendingReveal = false;

export function useUpdateDownload(): UpdateDownloadApi {
  const [state, setState] = useState<DownloadState | null>(null);
  const [error, setError] = useState('');
  const [openError, setOpenError] = useState('');
  const [cancelling, setCancelling] = useState(false);
  const [speed, setSpeed] = useState(0);
  const requests = useRef<ReturnType<typeof createSnapshotRequest<DownloadState>> | null>(null);
  const mounted = useRef(false);
  const refreshState = useCallback((reportError = false) => {
    void requests.current?.refresh(reportError);
  }, []);

  /* 所有快照入口共用一个请求；慢请求不会按 400ms 周期堆积。
     启动/取消前后的代际失效防止旧状态覆盖操作结果。 */
  useEffect(() => {
    mounted.current = true;
    const reader = createSnapshotRequest(fetchDownloadState, (s) => {
      setState(s);
      setError('');
    }, (e) => setError((e as Error).message));
    requests.current = reader;
    refreshState();
    const refresh = () => {
      if (!document.hidden) refreshState();
    };
    window.addEventListener('hashchange', refresh);
    window.addEventListener('focus', refresh);
    document.addEventListener('visibilitychange', refresh);
    return () => {
      mounted.current = false;
      reader.dispose();
      if (requests.current === reader) requests.current = null;
      window.removeEventListener('hashchange', refresh);
      window.removeEventListener('focus', refresh);
      document.removeEventListener('visibilitychange', refresh);
    };
  }, [refreshState]);

  const busy = isDownloadActive(state?.phase);
  const done = state?.phase === 'done';

  /* 任务进行中每 400ms 拉一次快照；结束自动停 */
  useEffect(() => {
    if (!busy) return;
    const id = window.setInterval(() => {
      if (!document.hidden) refreshState(true);
    }, 400);
    return () => {
      clearInterval(id);
    };
  }, [busy, refreshState]);

  /* 完成时自动在资源管理器定位安装包（仅本次点击发起的下载） */
  const prevPhase = useRef<DownloadPhase | undefined>(undefined);
  useEffect(() => {
    const p = state?.phase;
    const before = prevPhase.current;
    prevPhase.current = p;
    if (p === 'done' && pendingReveal && isDownloadActive(before)) {
      pendingReveal = false;
      openDownloaded('folder').catch((e: unknown) => {
        if (mounted.current) setOpenError((e as Error).message);
      });
    }
    if (!isDownloadActive(p)) setCancelling(false);
  }, [state?.phase]);

  /* 速度：相邻两次快照的字节差 / 时间差，指数平滑防跳字 */
  const sampleRef = useRef<{ t: number; bytes: number } | null>(null);
  useEffect(() => {
    if (state?.phase !== 'downloading' || state.downloaded == null) {
      sampleRef.current = null;
      setSpeed(0);
      return;
    }
    const now = Date.now();
    const bytes = state.downloaded;
    const prev = sampleRef.current;
    if (prev && now > prev.t && bytes >= prev.bytes) {
      const inst = ((bytes - prev.bytes) * 1000) / (now - prev.t);
      setSpeed((s) => (s > 0 ? s * 0.7 + inst * 0.3 : inst));
    }
    sampleRef.current = { t: now, bytes };
  }, [state?.phase, state?.downloaded]);

  const start = useCallback(async () => {
    requests.current?.invalidate();
    setError('');
    setOpenError('');
    try {
      const r = await startUpdateDownload();
      if (!mounted.current) return;
      requests.current?.invalidate();
      if (r.started) pendingReveal = true;
      setState(r.state);
    } catch (e) {
      if (mounted.current) setError((e as Error).message);
    }
  }, []);

  const cancel = useCallback(async () => {
    requests.current?.invalidate();
    setCancelling(true);
    try {
      await cancelUpdateDownload();
      if (!mounted.current) return;
      requests.current?.invalidate();
      refreshState(true);
    } catch (e) {
      if (!mounted.current) return;
      setError((e as Error).message);
      setCancelling(false);
    }
  }, [refreshState]);

  const open = useCallback(async (target: 'installer' | 'folder') => {
    setOpenError('');
    try {
      await openDownloaded(target);
    } catch (e) {
      if (!mounted.current) return;
      setOpenError((e as Error).message);
      /* 文件被删/移走后按钮才会报错：立刻重拉状态，让卡片翻成「已不在」 */
      requests.current?.invalidate();
      refreshState();
    }
  }, [refreshState]);

  const total = state?.total ?? 0;
  const downloaded = state?.downloaded ?? 0;
  const etaSec =
    state?.phase === 'downloading' && total > 0 && speed > 0
      ? Math.max(0, (total - downloaded) / speed)
      : null;

  return {
    state,
    error,
    openError,
    cancelling,
    speed,
    etaSec,
    busy,
    done,
    start,
    cancel,
    open,
  };
}

const fmtSize = (n?: number): string => (n && n > 0 ? `${(n / 1024 / 1024).toFixed(1)} MB` : '未知大小');

const fmtSpeed = (bps: number): string => (bps > 0 ? `${(bps / 1024 / 1024).toFixed(1)} MB/s` : '—');

const fmtEta = (sec: number): string =>
  sec >= 60 ? `${Math.floor(sec / 60)} 分 ${Math.round(sec % 60)} 秒` : `${Math.max(1, Math.round(sec))} 秒`;

/** 进度卡片：准备 / 下载（百分比+速度+剩余+落点+取消）/ 校验 / 完成
 *  （运行安装包·打开文件夹）/ 失败或取消（重试）。 */
export function UpdateDownloadCard({
  api,
  pageUrl,
}: {
  api: UpdateDownloadApi;
  /** 下载失败时可退回的下载页（清单不可达时仍能给用户一条出路） */
  pageUrl?: string;
}) {
  const st = api.state;
  if (!st || st.phase === 'idle') return null;
  const phase = st.phase;
  const total = st.total ?? 0;
  const got = st.downloaded ?? 0;
  const pct =
    phase === 'done' ? 100 : total > 0 ? Math.min(100, Math.round((got / total) * 100)) : null;
  const active = isDownloadActive(phase);
  const ok = phase === 'done';
  const bad = phase === 'error';

  const title = ok
    ? `虎符 v${st.latest || ''} 安装包已就绪`
    : bad || phase === 'cancelled'
      ? '安装包下载'
      : `正在下载虎符 v${st.latest || ''} 安装包`;

  const sub =
    phase === 'preparing'
      ? '正在获取更新信息…'
      : phase === 'downloading'
        ? '正在下载（sha256 校验通过才会保存）'
        : phase === 'verifying'
          ? '正在校验 sha256，通过后自动保存'
          : phase === 'done'
            ? `${st.file || '安装包'} · ${fmtSize(got)}`
            : phase === 'cancelled'
              ? '已取消，可重新下载'
              : st.error || '下载失败';

  return (
    <div className={`card upd-card${ok ? ' ok' : ''}${bad ? ' bad' : ''}`}>
      <div className="upd-head">
        <span className={`upd-ico${ok ? ' ok' : ''}${bad ? ' err' : ''}`}>
          <Icon name={ok ? 'check' : bad ? 'close' : 'download'} />
        </span>
        <div className="upd-titles">
          <b>{title}</b>
          <span>{sub}</span>
        </div>
        {phase === 'downloading' && pct !== null && <span className="upd-pct">{pct}%</span>}
        {ok && (
          <Badge tone="green" dot>
            校验通过
          </Badge>
        )}
      </div>

      {(active || ok) && (
        <div className={`upd-bar${pct === null ? ' indet' : ''}`}>
          <i style={pct === null ? undefined : { width: `${pct}%` }} />
        </div>
      )}

      {phase === 'downloading' && (
        <div className="upd-meta">
          <span>
            {fmtSize(got)} / {fmtSize(total)}
          </span>
          <span>{fmtSpeed(api.speed)}</span>
          {api.etaSec != null && <span>剩余约 {fmtEta(api.etaSec)}</span>}
        </div>
      )}

      {/* 【P3 扫尾 2026-11-07】多源回退提示：attempt>1 = server 已自动换过
          attempt-1 次下载源（此时进度归零是换源重下，不是卡死/bug）；
          老 server 无该字段或回落 1 时不显示。样式沿用 upd-hint。 */}
      {active && st.attempt != null && st.attempt > 1 && (
        <div className="upd-hint">网络不佳，已自动切换备用下载源（第 {st.attempt - 1} 次）</div>
      )}

      {(st.dest || st.dir) && phase !== 'error' && phase !== 'cancelled' && (
        <div className="upd-path">
          <span>保存到</span>
          <code title={st.dest || st.dir}>{st.dest || st.dir}</code>
        </div>
      )}

      {ok && st.sha256_got && (
        <div className="upd-sha">sha256 {st.sha256_got.slice(0, 24)}… 已校验，安全可运行</div>
      )}

      {(api.error || api.openError) && <div className="upd-err">{api.error || api.openError}</div>}

      <div className="upd-actions">
        {active && (
          <button
            type="button"
            className="btn outline"
            disabled={api.cancelling}
            onClick={() => void api.cancel()}
          >
            {api.cancelling ? '正在取消…' : '取消下载'}
          </button>
        )}
        {phase === 'verifying' && <span className="upd-hint">校验通过后自动保存，请稍候…</span>}
        {ok && (
          <>
            <button type="button" className="btn primary" onClick={() => void api.open('installer')}>
              <Icon name="play" />
              运行安装包
            </button>
            <button type="button" className="btn" onClick={() => void api.open('folder')}>
              <Icon name="folder" />
              打开所在文件夹
            </button>
            <button
              type="button"
              className="btn ghost"
              title="文件被移动/删除或想重新校验时，可再下一次"
              onClick={() => void api.start()}
            >
              重新下载
            </button>
            <span className="upd-hint">运行后按安装向导完成后重启即可</span>
          </>
        )}
        {(phase === 'error' || phase === 'cancelled') && (
          <>
            <button type="button" className="btn primary" onClick={() => void api.start()}>
              重新下载
            </button>
            {pageUrl && (
              <button
                type="button"
                className="btn ghost"
                onClick={() => window.open(pageUrl, '_blank', 'noopener')}
              >
                打开下载页
              </button>
            )}
          </>
        )}
      </div>
    </div>
  );
}
