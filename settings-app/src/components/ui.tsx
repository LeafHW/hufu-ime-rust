import { useRef, type ReactNode } from 'react';
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
}: {
  value: number;
  min?: number;
  max?: number;
  step?: number;
  width?: number;
  onChange: (v: number) => void;
  label?: string;
  disabled?: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const pct = Math.max(0, Math.min(100, ((value - min) / (max - min)) * 100));

  const apply = (clientX: number, rect: DOMRect) => {
    const p = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
    const raw = min + (max - min) * p;
    const next = Math.round(raw / step) * step;
    onChange(Number(next.toFixed(2)));
  };

  return (
    <div
      ref={ref}
      className={`slider${disabled ? ' disabled' : ''}`}
      style={{ width }}
      role="slider"
      tabIndex={disabled ? -1 : 0}
      aria-label={label}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuenow={value}
      aria-disabled={disabled || undefined}
      onPointerDown={(e) => {
        if (disabled) return;
        const rect = e.currentTarget.getBoundingClientRect();
        try {
          e.currentTarget.setPointerCapture(e.pointerId);
        } catch {
          /* synthetic events */
        }
        apply(e.clientX, rect);
        const move = (ev: PointerEvent) => apply(ev.clientX, rect);
        const up = () => {
          window.removeEventListener('pointermove', move);
          window.removeEventListener('pointerup', up);
        };
        window.addEventListener('pointermove', move);
        window.addEventListener('pointerup', up, { once: true });
      }}
      onKeyDown={(e) => {
        if (disabled) return;
        if (e.key === 'ArrowLeft') onChange(Math.max(min, value - step));
        if (e.key === 'ArrowRight') onChange(Math.min(max, value + step));
      }}
    >
      <i style={{ width: `${pct}%` }} />
      <b style={{ left: `${pct}%` }} />
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
