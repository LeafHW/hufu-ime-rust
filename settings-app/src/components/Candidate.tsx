import type { Skin } from '../types';
import { fallbackSkin } from '../types';

function withAlpha(hex: string | undefined, alpha: number): string {
  const h = (hex || '').replace('#', '');
  if (h.length !== 6 && h.length !== 8) return `rgba(20,22,26,${alpha})`;
  const r = parseInt(h.slice(0, 2), 16);
  const g = parseInt(h.slice(2, 4), 16);
  const b = parseInt(h.slice(4, 6), 16);
  const a = h.length === 8 ? parseInt(h.slice(6, 8), 16) / 255 : 1;
  return `rgba(${r},${g},${b},${(a * alpha).toFixed(3)})`;
}

function labelFor(n: number, style?: string): string {
  if (style === 'zh') return ['一', '二', '三', '四', '五', '六', '七', '八', '九', '十'][n - 1] || String(n);
  if (style === 'roman') return ['Ⅰ', 'Ⅱ', 'Ⅲ', 'Ⅳ', 'Ⅴ', 'Ⅵ', 'Ⅶ', 'Ⅷ', 'Ⅸ', 'Ⅹ'][n - 1] || String(n);
  return n === 10 ? '0' : String(n);
}

export function Candidate({
  skin,
  size = 'sm',
  className = '',
  ab = false,
  horizontal = false,
  fontScales,
  width,
}: {
  skin?: Skin | null;
  size?: 'sm' | 'lg';
  className?: string;
  ab?: boolean;
  horizontal?: boolean;
  /** 逐候选字号（appearance.per_cand_font 体系）：开=首选按 first 放大、其余按 other 缩小；序号/编码行不受影响。 */
  fontScales?: { enabled: boolean; first: number; other: number };
  /** 覆盖竖排窗口宽度（px）；缺省 sm=310 / lg=180。挂件页预览用它贴近真实候选窗宽。 */
  width?: number;
}) {
  const s = skin || fallbackSkin;
  const c = s.colors || {};
  const layout = s.layout || {};
  const material = s.material || {};
  const master = material.master_alpha ?? 0.94;
  const hilite = material.hilite_alpha ?? 1;
  const shadow = material.shadow_alpha ?? 0.55;
  const font = Math.max(12, Number(layout.font_point) || 17);
  const radius = Number(layout.corner_radius) || 10;
  const padX = Number(layout.margin_x) || 8;
  const padY = Number(layout.margin_y) || 6;
  const gap = Number(layout.candidate_spacing) || 6;
  const rowPad = Number(layout.hilite_padding) || 6;
  const borderW = Number(material.border_width ?? layout.border_width ?? 1);

  const bg = withAlpha(c.back_color, master);
  const border = borderW > 0 ? `${borderW}px solid ${withAlpha(c.border_color || material.border_color, 0.8)}` : 'none';
  const normalText = c.candidate_text_color || '#ECE9E2';
  const comment = c.comment_text_color || '#8D8980';
  const label = c.label_color || '#8D8980';
  const hlBack = withAlpha(c.hilited_candidate_back_color || '#E7B75C', hilite);
  const hlText = c.hilited_candidate_text_color || '#1A1408';
  const hlLabel = c.hilited_candidate_label_color || hlText;
  const preeditColor = c.text_color || c.hilited_text_color || '#E7B75C';
  const hlOn = material.hilite_on !== false;
  const fontFace = layout.font_face || '"Segoe UI Variable Text","Microsoft YaHei UI",sans-serif';
  /* 逐候选字号：i=0 用 first，其余用 other；未启用恒为 1。 */
  const scaleOf = (i: number) =>
    fontScales?.enabled ? Math.max(0.3, i === 0 ? fontScales.first : fontScales.other) : 1;

  const rows = [
    { t: '我们', cmt: 'wǒ men' },
    { t: '我', cmt: 'wǒ' },
    { t: '我军', cmt: 'wǒ jūn' },
    { t: 'War', cmt: '' },
  ];

  if (horizontal) {
    return (
      <div
        className={`cand horizontal${ab ? ' ab' : ''} ${className}`}
        style={{
          background: bg,
          border,
          borderRadius: radius,
          padding: `${padY}px ${padX}px`,
          boxShadow: `0 ${Number(layout.shadow_offset_y) || 0}px ${Number(layout.shadow_radius) || 24}px ${withAlpha(c.shadow_color, shadow)}`,
          fontFamily: fontFace,
          display: 'flex',
          alignItems: 'center',
          gap: Math.max(4, gap),
          minWidth: 300,
          maxWidth: '100%',
          width: 'auto',
        }}
      >
        {layout.inline_preedit === false && <span className="preedit-h" style={{ color: preeditColor, fontSize: Math.max(10, font - 4) }}>tuja</span>}
        {rows.map((row, i) => {
          const active = i === 0 && hlOn;
          const sc = scaleOf(i);
          return (
            <span
              key={row.t}
              className="row-h"
              style={{
                height: Math.round(font * 1.85 * sc),
                fontSize: font * sc,
                gap: Math.round(font * 0.36),
                padding: `0 ${Math.max(6, rowPad)}px`,
                borderRadius: Number(layout.hilited_corner_radius) || 7,
                background: active ? hlBack : 'transparent',
                color: active ? hlText : normalText,
              }}
            >
              <span className="idx" style={{ color: active ? hlLabel : label, fontSize: Math.max(9, font - 5) }}>
                {labelFor(i + 1, layout.label_style)}
                {String(layout.label_format || '%s.').replace('%s', '')}
              </span>
              {row.t}
              {row.cmt ? (
                <span className="cmt" style={{ color: active ? hlText : comment, fontSize: Math.max(9, (font - 6) * sc), opacity: 0.78 }}>
                  {row.cmt}
                </span>
              ) : null}
            </span>
          );
        })}
      </div>
    );
  }

  return (
    <div
      className={`cand${size === 'lg' ? ' lg' : ''}${ab ? ' ab' : ''} ${className}`}
      style={{
        background: bg,
        border,
        borderRadius: radius,
        padding: `${padY}px ${padX}px ${padY + 2}px`,
        boxShadow: `0 ${Number(layout.shadow_offset_y) || 0}px ${Number(layout.shadow_radius) || 24}px ${withAlpha(c.shadow_color, shadow)}`,
        fontFamily: fontFace,
        minWidth: width ?? (size === 'lg' ? 180 : 310),
        width: width ?? (size === 'lg' ? 180 : 310),
      }}
    >
      {layout.inline_preedit === false && <div
        className="preedit"
        style={{
          color: preeditColor,
          fontSize: Math.max(10, font - 4),
          borderBottomColor: withAlpha(c.border_color, 0.18),
        }}
      >
        tuja
      </div>}
      {rows.map((row, i) => {
        const active = i === 0 && hlOn;
        const sc = scaleOf(i);
        return (
          <div
            key={row.t}
            className={`row${active ? ' active' : ''}`}
            style={{
              height: Math.round(font * 1.85 * sc),
              fontSize: font * sc,
              gap: Math.round(font * 0.45),
              padding: `0 ${Math.max(6, rowPad)}px`,
              borderRadius: Number(layout.hilited_corner_radius) || 7,
              background: active ? hlBack : 'transparent',
              color: active ? hlText : normalText,
            }}
          >
            <span className="idx" style={{ color: active ? hlLabel : label, fontSize: Math.max(9, font - 5) }}>
              {labelFor(i + 1, layout.label_style)}
              {String(layout.label_format || '%s.').replace('%s', '')}
            </span>
            {row.t}
            {row.cmt ? (
              <span className="cmt" style={{ color: active ? withAlpha(hlText, 0.7) : comment, fontSize: Math.max(9, (font - 5) * sc) }}>
                {row.cmt}
              </span>
            ) : null}
          </div>
        );
      })}
      <div className="cand-gap" style={{ height: gap }} />
    </div>
  );
}
