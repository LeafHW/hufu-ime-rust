import { useState } from 'react';
import type { About } from '../types';
import { fetchUpdateCheck, type UpdateState } from '../updateCheck';
import { UpdateDownloadCard, useUpdateDownload } from '../components/UpdateDownload';
import { PageHead } from '../components/ui';

export function AboutPage({ about }: { about: About }) {
  const [upd, setUpd] = useState<UpdateState>({ kind: 'idle' });
  /* 【更新下载 2026-11-06】进度卡片：准备/下载/校验/完成/失败五态，
     百分比、速度、剩余、落点、取消、运行安装包、打开文件夹都在卡片里。
     状态来自 server 后台任务，关掉设置页重开也能接上。 */
  const dl = useUpdateDownload();

  const checkUpdate = async () => {
    setUpd({ kind: 'checking' });
    setUpd(await fetchUpdateCheck(about.github, { fresh: true }));
  };

  const pageUrl = upd.kind === 'update' ? upd.pageUrl : about.github;

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
                <span className="update-note has-update">发现新版本 v{upd.latest}</span>
                {!dl.busy && !dl.done && (
                  /* 【审计 P2-D 2026-11-05】校验式下载：server 中转 + sha256
                     比对通过才落「下载」文件夹；下载过程走 UpdateDownloadCard
                     的进度条，不再是一个等 10 分钟的阻塞请求。 */
                  <button
                    type="button"
                    className="btn"
                    onClick={() => void dl.start()}
                    title="下载到「下载」文件夹，sha256 校验通过才算完成"
                  >
                    下载安装包（自动校验）
                  </button>
                )}
              </>
            )}
            {upd.kind === 'latest' && <span className="update-note ok">已是最新（v{upd.current}）</span>}
            {upd.kind === 'error' && <span className="update-note err">检查失败：{upd.msg}</span>}
          </div>
        </div>
      </div>
      <UpdateDownloadCard api={dl} pageUrl={pageUrl} />
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
