import { useEffect, useRef, useState } from 'react';

/**
 * 【2026-11-03】动效预览（原皮肤页 M5 演示，迁到「动效」页）：
 * 固定内部 token fast=80ms 的高亮滑动演示，只受全局动效开关/速度影响，
 * 不向皮肤暴露任何参数。点「播放」走一遍：选中从第 1 项滑到第 2 项再回位。
 */
export function MotionDemo({ speed, enabled }: { speed: number; enabled: boolean }) {
  const [sel, setSel] = useState(0);
  const [playing, setPlaying] = useState(false);
  const timers = useRef<number[]>([]);
  const clear = () => {
    timers.current.forEach((t) => window.clearTimeout(t));
    timers.current = [];
  };
  useEffect(() => clear, []);
  const play = () => {
    clear();
    setPlaying(true);
    setSel(1);
    timers.current.push(
      window.setTimeout(() => {
        setSel(0);
        setPlaying(false);
      }, 780),
    );
  };
  const dur = enabled ? Math.max(0, 80 / Math.max(0.05, speed)) : 0;
  return (
    <div className="motion-demo">
      <div className="md-rows" role="img" aria-label="候选窗高亮滑动预览">
        <div
          className="md-pill"
          style={{ transform: `translateY(${sel * 26}px)`, transitionDuration: `${Math.round(dur)}ms` }}
        />
        {['我们', '我', '我军'].map((t, i) => (
          <div key={t} className={`md-row${sel === i ? ' on' : ''}`}>
            {i + 1}. {t}
          </div>
        ))}
      </div>
      <button type="button" className="btn sm outline" onClick={play} disabled={playing}>
        {playing ? '播放中…' : '播放'}
      </button>
    </div>
  );
}
