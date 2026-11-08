import type { Bootstrap } from '../types';
import { TrainerHotkeySettings, TrainerSettings } from '../components/TrainerSettings';
import { PageHead } from '../components/ui';
import '../styles/input.css';

/* 【整改10 2026-11-08】独立「跟打器」导航页：
   概览页原「跟打练习」卡整体挪到这里（启用开关保持单处），
   并新增全局老板键录制卡（trainer.hotkey，支持录制/保存/清除）。 */
export function Trainer({
  data,
  notify,
  refresh,
}: {
  data: Bootstrap;
  notify: (msg: string) => void;
  refresh: () => Promise<unknown>;
}) {
  return (
    <>
      <PageHead
        title="跟打器"
        sub={
          <>
            <span className={data.online ? 'dot' : 'dot muted'} />
            {data.online ? '打字练习 · 成绩统计' : '离线预览'}
            <span style={{ color: 'var(--text3)' }}>·</span>
            启用内置入口并设置全局老板键
          </>
        }
      />
      <TrainerSettings data={data} notify={notify} refresh={refresh} />
      <TrainerHotkeySettings data={data} notify={notify} refresh={refresh} />
    </>
  );
}
