import { useState } from 'react';
import type { About } from '../types';
import { fetchUpdateCheck, verifiedDownloadUpdate, type UpdateState } from '../updateCheck';
import { PageHead } from '../components/ui';

export function AboutPage({ about }: { about: About }) {
  const [upd, setUpd] = useState<UpdateState>({ kind: 'idle' });
  const [dlBusy, setDlBusy] = useState(false);
  const [dlMsg, setDlMsg] = useState('');

  const checkUpdate = async () => {
    setUpd({ kind: 'checking' });
    setUpd(await fetchUpdateCheck(about.github));
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
                  /* 【审计 P2-D 2026-11-05】校验式下载：server 中转 + sha256
                     比对，通过才落「下载」文件夹；端点不可用回退浏览器直开。
                     【F4 复审修复】进度上屏（此前只 console.info，用户无感知
                     重复点击触发 409）；下载中禁用防重复。 */
                  disabled={dlBusy}
                  onClick={async () => {
                    if (dlBusy) return;
                    setDlBusy(true);
                    try {
                      await verifiedDownloadUpdate(upd.downloadUrl || upd.pageUrl, setDlMsg);
                    } finally {
                      setDlBusy(false);
                    }
                  }}
                  title={upd.downloadUrl ? '下载并校验安装包（sha256 通过才保存）' : '未找到安装包，打开下载页'}
                >
                  {dlBusy ? '下载中…' : upd.downloadUrl ? '下载安装包（自动校验）' : '打开下载页'}
                </button>
                {dlMsg && <span className="update-note err">{dlMsg}</span>}
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
