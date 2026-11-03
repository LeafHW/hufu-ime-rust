import { useEffect, useRef, useState } from 'react';
import type { SkinTokens } from '../skinTokens';

const ROWS = ['我们', '我', '我军'];
const ROW_H = 28;

/**
 * 【动效预览】高亮滑动演示：固定内部节奏 fast=80ms（只受总开关/速度影响），
 * 外观跟「动效」页其它演示一致——用当前皮肤的真实配色画一只迷你候选窗，
 * 高亮底块在候选之间滑过去，而不是设置页自己的米色小控件。
 */
export function MotionDemo({
  tokens,
  speed,
  enabled,
}: {
  tokens: SkinTokens;
  speed: number;
  enabled: boolean;
}) {
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
  const dur = enabled ? Math.max(16, Math.round(80 / Math.max(0.05, speed))) : 0;
  const gap = tokens.gap;
  return (
    <div className="motion-demo">
      <div
        className="md-cand"
        style={{
          background: tokens.bg,
          borderColor: tokens.border,
          borderRadius: tokens.radius + 2,
          fontFamily: tokens.face,
        }}
      >
        <div className="md-pre" style={{ color: tokens.preedit, fontSize: Math.max(10, tokens.font - 4) }}>
          tuja
        </div>
        <div className="md-rows" style={{ gap }}>
          <span
            className="md-pill"
            style={{
              height: ROW_H,
              borderRadius: Math.max(4, tokens.radius - 2),
              background: tokens.hlBg,
              transform: `translateY(${sel * (ROW_H + gap)}px)`,
              transitionDuration: `${dur}ms`,
            }}
          />
          {ROWS.map((t, i) => {
            const on = i === sel;
            return (
              <div
                key={t}
                className={`md-row${on ? ' on' : ''}`}
                style={{
                  height: ROW_H,
                  lineHeight: `${ROW_H}px`,
                  borderRadius: Math.max(4, tokens.radius - 2),
                  color: on ? tokens.hlText : tokens.text,
                  fontSize: Math.round(tokens.font * 0.84),
                  padding: `0 ${Math.max(8, tokens.rowPad + 4)}px`,
                  transitionDuration: `${Math.round(dur * 0.7)}ms`,
                }}
              >
                <span className="md-idx" style={{ color: on ? tokens.hlText : tokens.label }}>
                  {i + 1}.
                </span>
                {t}
              </div>
            );
          })}
        </div>
      </div>
      <button type="button" className="btn sm outline" onClick={play} disabled={playing}>
        {playing ? '播放中…' : '播放'}
      </button>
    </div>
  );
}
