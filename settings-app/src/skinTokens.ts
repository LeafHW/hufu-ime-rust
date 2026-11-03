import type { Skin } from './types';
import { fallbackSkin } from './types';

/** 把 #RRGGBB / #RRGGBBAA 叠到透明度 a 上（非 hex 时给 fallback）。 */
export function withAlpha(hex: string | undefined, a: number, fallback = '#14161a'): string {
  const h = (hex || '').replace('#', '');
  if (h.length !== 6 && h.length !== 8) return fallback;
  const r = parseInt(h.slice(0, 2), 16);
  const g = parseInt(h.slice(2, 4), 16);
  const b = parseInt(h.slice(4, 6), 16);
  const alpha = h.length === 8 ? parseInt(h.slice(6, 8), 16) / 255 : 1;
  return `rgba(${r},${g},${b},${(alpha * a).toFixed(3)})`;
}

/** 演示用的候选窗配色：跟着用户当前皮肤走，看起来就是自己那只候选窗。 */
export type SkinTokens = {
  bg: string;
  border: string;
  text: string;
  comment: string;
  label: string;
  preedit: string;
  hlBg: string;
  hlText: string;
  radius: number;
  face: string;
  font: number;
  rowPad: number;
  gap: number;
};

export function skinTokens(skin: Skin | null | undefined): SkinTokens {
  const s = skin || fallbackSkin;
  const c = s.colors || {};
  const l = s.layout || {};
  const m = s.material || {};
  const master = Number.isFinite(Number(m.master_alpha)) ? Number(m.master_alpha) : 0.94;
  const hilite = Number.isFinite(Number(m.hilite_alpha)) ? Number(m.hilite_alpha) : 1;
  const font = Math.min(19, Math.max(13, Number(l.font_point) || 16));
  return {
    bg: withAlpha(c.back_color, master, 'rgba(20,22,26,.94)'),
    border: withAlpha(c.border_color, 0.55, 'rgba(255,255,255,.12)'),
    text: c.candidate_text_color || '#ECE9E2',
    comment: c.comment_text_color || '#8D8980',
    label: c.label_color || '#8D8980',
    preedit: c.text_color || c.hilited_text_color || '#E7B75C',
    hlBg: withAlpha(c.hilited_candidate_back_color || '#E7B75C', hilite, 'rgba(231,183,92,.9)'),
    hlText: c.hilited_candidate_text_color || '#1A1408',
    radius: Math.min(14, Number(l.corner_radius) || 9),
    face: l.font_face || '"Segoe UI Variable Text","Microsoft YaHei UI",sans-serif',
    font,
    rowPad: 4,
    gap: Math.max(3, Math.min(10, Number(l.candidate_spacing) || 5)),
  };
}
