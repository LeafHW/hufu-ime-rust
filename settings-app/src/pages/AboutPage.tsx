import { useState } from 'react';
import type { About } from '../types';
import { getJson } from '../api';
import { PageHead } from '../components/ui';

type UpdateState =
  | { kind: 'idle' }
  | { kind: 'checking' }
  | { kind: 'latest'; current: string }
  | { kind: 'update'; latest: string; downloadUrl: string | null; pageUrl: string }
  | { kind: 'error'; msg: string };

export function AboutPage({ about }: { about: About }) {
  const [upd, setUpd] = useState<UpdateState>({ kind: 'idle' });

  const checkUpdate = async () => {
    setUpd({ kind: 'checking' });
    try {
      const r = (await getJson<{ ok: boolean; current: string; latest?: string; has_update?: boolean; page_url?: string; download_url?: string | null; error?: string }>('/api/update/check')) as {
        ok: boolean; current: string; latest?: string; has_update?: boolean; page_url?: string; download_url?: string | null; error?: string;
      };
      if (!r.ok) setUpd({ kind: 'error', msg: r.error || '检查失败' });
      else if (r.has_update)
        setUpd({ kind: 'update', latest: r.latest || '?', downloadUrl: r.download_url || null, pageUrl: r.page_url || about.github });
      else setUpd({ kind: 'latest', current: r.current });
    } catch (e) {
      setUpd({ kind: 'error', msg: (e as Error).message });
    }
  };

  return (
    <>
      <PageHead title="关于" sub="虎符输入法 · Rust 引擎 + TSF / IMK 薄壳" />
      <div className="card about-hero">
        <img src="/hufu-icon-q-256.png" alt="" />
        <div>
          <h2>虎符 HuFu</h2>
          <div className="ver">v{about.version} · Windows 设置窗口预览</div>
          <div style={{ marginTop: 12, display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
            <button type="button" className="btn primary" onClick={() => window.open(about.github, '_blank', 'noopener')}>
              打开 GitHub
            </button>
            <button type="button" className="btn" onClick={checkUpdate} disabled={upd.kind === 'checking'}>
              {upd.kind === 'checking' ? '检查中…' : '检查更新'}
            </button>
            {upd.kind === 'update' && (
              <>
                <span className="update-note has-update">
                  发现新版本 v{upd.latest}
                </span>
                <button
                  type="button"
                  className="btn"
                  onClick={() => window.open(upd.downloadUrl || upd.pageUrl, '_blank', 'noopener')}
                  title={upd.downloadUrl ? '浏览器直接下载安装包' : '未找到安装包，打开下载页'}
                >
                  {upd.downloadUrl ? '直接下载安装包' : '打开下载页'}
                </button>
              </>
            )}
            {upd.kind === 'latest' && <span className="update-note ok">已是最新（v{upd.current}）</span>}
            {upd.kind === 'error' && <span className="update-note err">检查失败：{upd.msg}</span>}
          </div>
        </div>
      </div>
      <div className="about-grid">
        <div className="card about-card">
          <div className="label">安装目录</div>
          <code>{about.exe_dir}</code>
        </div>
        <div className="card about-card">
          <div className="label">数据目录</div>
          <code>{about.data_dir}</code>
        </div>
        <div className="card about-card">
          <div className="label">开源仓库</div>
          <code>{about.github}</code>
        </div>
        <div className="card about-card">
          <div className="label">致谢</div>
          <code>虎码 / 夜莺 / 虎爪 / 霞鹜文楷 / OpenCC</code>
        </div>
      </div>
    </>
  );
}
