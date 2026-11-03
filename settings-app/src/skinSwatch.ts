/**
 * 皮肤缩略图配色（2026-11-03）。
 *
 * 服务端 GET /api/skins 给每个 JSON 皮肤返回两个代表色（swatch=back_color、
 * accent=hilited_candidate_back_color）。概览「当前皮肤」和皮肤列表缩略图都
 * 走这里，保证同一只皮肤在两处是同一个图标；浅色主题下统一把深色混向浅纸，
 * 不再出现黑块。SSF 皮肤没有这两个字段，仍用 CSS 的 .skin-thumb.ssf 样式。
 */
type RGB = [number, number, number];

const FALLBACK_BACK = '#2E323A';
const FALLBACK_ACCENT = '#E7B75C';
const PAPER = '#F7F2E8';

function parseHex(hex?: string): RGB | null {
  if (!hex) return null;
  const h = hex.replace('#', '');
  if (h.length !== 6 && h.length !== 8) return null;
  const v = (i: number) => parseInt(h.slice(i * 2, i * 2 + 2), 16);
  if (!Number.isFinite(v(0)) || !Number.isFinite(v(1)) || !Number.isFinite(v(2))) return null;
  return [v(0), v(1), v(2)];
}

function hex(rgb: RGB): string {
  return `#${rgb.map((n) => Math.max(0, Math.min(255, Math.round(n))).toString(16).padStart(2, '0')).join('')}`;
}

function mix(a: RGB, b: RGB, t: number): RGB {
  return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
}

function luminance([r, g, b]: RGB): number {
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export type SwatchStyle = {
  /** 缩略图渐变（直接给 background） */
  background: string;
  /** 底部小色条颜色（也通过 --sw-bar 传给 CSS） */
  bar: string;
  /** 边框色 */
  border: string;
};

/** 把颜色亮度夹到可见区间：浅色主题下既不能是一块黑，也不能白到看不见。
    保持色相，只调明度（深色往浅纸提、浅色往暖褐压）。 */
function clampLum(rgb: RGB, lo: number, hi: number): RGB {
  const l = luminance(rgb);
  if (l > hi) return mix(rgb, [120, 105, 82], Math.min(0.62, ((l - hi) / (255 - hi)) * 1.1));
  if (l < lo) return mix(rgb, [247, 242, 232], Math.min(0.72, ((lo - l) / lo) * 0.95));
  return rgb;
}

export function skinSwatch(backHex?: string, accentHex?: string, light = false): SwatchStyle {
  const back = parseHex(backHex) || parseHex(FALLBACK_BACK)!;
  const accent = parseHex(accentHex) || parseHex(FALLBACK_ACCENT)!;
  const paper = parseHex(PAPER)!;
  if (light) {
    /* 浅色主题：先夹亮度再混纸。若直接混纸，本身就很浅的皮肤会白成一片、
       和卡片底分不开；夹到中明度后色相还在，缩略图才认得出是哪只皮肤。 */
    const b = clampLum(back, 100, 180);
    const a = clampLum(accent, 72, 165);
    return {
      background: `linear-gradient(135deg, ${hex(b)}, ${hex(mix(b, paper, 0.42))} 58%, ${hex(a)})`,
      bar: hex(a),
      border: 'rgba(120,90,40,.22)',
    };
  }
  return {
    background: `linear-gradient(135deg, ${hex(back)}, #15171B 58%, ${hex(accent)})`,
    bar: hex(luminance(accent) < 70 ? mix(accent, [243, 200, 120], 0.6) : accent),
    border: 'rgba(255,255,255,.12)',
  };
}
