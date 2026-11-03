import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Icon } from './Icon';

export function Badge({
  children,
  tone = 'gold',
  dot = false,
}: {
  children: ReactNode;
  tone?: 'gold' | 'green' | 'gray';
  dot?: boolean;
}) {
  const cls = tone === 'green' ? 'badge green' : tone === 'gray' ? 'badge gray' : 'badge';
  return (
    <span className={cls}>
      {dot ? <span className="dot" /> : null}
      {children}
    </span>
  );
}

export function Switch({
  on,
  onChange,
  title,
}: {
  on: boolean;
  onChange?: (v: boolean) => void;
  title?: string;
}) {
  return (
    <button
      type="button"
      className={`switch${on ? ' on' : ''}`}
      title={title}
      aria-pressed={on}
      onClick={() => onChange?.(!on)}
    />
  );
}

export function Slider({
  value,
  min = 0,
  max = 100,
  step = 1,
  width = 92,
  onChange,
  label,
  disabled,
  suffix,
}: {
  value: number;
  min?: number;
  max?: number;
  step?: number;
  width?: number;
  onChange: (v: number) => void;
  label?: string;
  disabled?: boolean;
  /** 点击输入时跟在数字后的单位（如 % / px / pt） */
  suffix?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  /** 非 null=正在输入精确数值（单击滑杆打开）；字符串保留用户编辑中的原文。 */
  const [editing, setEditing] = useState<string | null>(null);
  const moved = useRef(false);
  const pct = Math.max(0, Math.min(100, ((value - min) / (max - min)) * 100));

  useEffect(() => {
    if (editing === null) return;
    const el = inputRef.current;
    if (!el) return;
    el.focus();
    el.select();
  }, [editing]);

  const quantize = (raw: number) => {
    const stepped = Math.round(raw / step) * step;
    return Number(Math.max(min, Math.min(max, stepped)).toFixed(2));
  };
  const apply = (clientX: number, rect: DOMRect) => {
    if (rect.width <= 0) return;
    const p = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
    onChange(quantize(min + (max - min) * p));
  };
  const commit = () => {
    if (editing === null) return;
    const raw = Number(editing.replace(/[^\d.+-]/g, ''));
    setEditing(null);
    if (Number.isFinite(raw) && editing.trim() !== '') onChange(quantize(raw));
  };

  return (
    <div
      ref={ref}
      className={`slider${disabled ? ' disabled' : ''}${editing !== null ? ' editing' : ''}`}
      style={{ width }}
      role="slider"
      tabIndex={disabled ? -1 : 0}
      title={label ? `${label} · 拖动调节，单击输入` : '拖动调节，单击输入'}
      aria-label={label}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuenow={value}
      aria-disabled={disabled || undefined}
      onPointerDown={(e) => {
        if (disabled || editing !== null) return;
        const rect = e.currentTarget.getBoundingClientRect();
        /* 【2026-11-03】单击不再立即跳值：按下只记起点，位移超过 3px 才进入拖动，
           原地松手=打开数字输入（精细调节）；拖动逻辑与旧版一致。 */
        moved.current = false;
        try {
          e.currentTarget.setPointerCapture(e.pointerId);
        } catch {
          /* synthetic events */
        }
        const startX = e.clientX;
        const startY = e.clientY;
        const move = (ev: PointerEvent) => {
          if (!moved.current) {
            if (Math.hypot(ev.clientX - startX, ev.clientY - startY) <= 3) return;
            moved.current = true;
          }
          apply(ev.clientX, rect);
        };
        const up = () => {
          window.removeEventListener('pointermove', move);
          window.removeEventListener('pointerup', up);
          if (!moved.current) setEditing(String(Number(value.toFixed(2))));
        };
        window.addEventListener('pointermove', move);
        window.addEventListener('pointerup', up, { once: true });
      }}
      onKeyDown={(e) => {
        if (disabled) return;
        if (e.key === 'ArrowLeft') onChange(quantize(value - step));
        else if (e.key === 'ArrowRight') onChange(quantize(value + step));
        else if (e.key === 'Enter') {
          e.preventDefault();
          setEditing(String(Number(value.toFixed(2))));
        }
      }}
    >
      <i style={{ width: `${pct}%` }} />
      <b style={{ left: `${pct}%` }} />
      {editing !== null ? (
        <span className="slider-edit" onPointerDown={(e) => e.stopPropagation()}>
          <input
            ref={inputRef}
            value={editing}
            inputMode="decimal"
            aria-label={label ? `${label}：输入数值` : '输入数值'}
            onChange={(e) => setEditing(e.target.value)}
            onKeyDown={(e) => {
              /* 输入框里吃掉按键：避免 Ctrl+K / 空格等触发页面级快捷键 */
              e.stopPropagation();
              if (e.key === 'Enter') commit();
              else if (e.key === 'Escape') setEditing(null);
            }}
            onBlur={commit}
          />
          {suffix ? <em>{suffix}</em> : null}
        </span>
      ) : null}
    </div>
  );
}

export function PageHead({
  title,
  sub,
  actions,
}: {
  title: string;
  sub: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className="page-head">
      <div>
        <h1>{title}</h1>
        <div className="sub">{sub}</div>
      </div>
      {actions ? <div className="head-actions">{actions}</div> : null}
    </div>
  );
}

export function IconButton({ name, title, onClick }: { name: string; title?: string; onClick?: () => void }) {
  return (
    <button type="button" className="icon-btn" title={title} aria-label={title} onClick={onClick}>
      <Icon name={name} />
    </button>
  );
}

export function Toast({ message }: { message: string | null }) {
  return (
    <div className={`toast${message ? ' show' : ''}`} role="status" aria-live="polite">
      {message}
    </div>
  );
}
