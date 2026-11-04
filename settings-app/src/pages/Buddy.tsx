import { useCallback, useEffect, useRef, useState } from 'react';
import type { ChangeEvent, PointerEvent as ReactPointerEvent, RefObject } from 'react';
import type { Bootstrap } from '../types';
import { useDirtyGuard } from '../dirtyGuard';
import { fetchWithTimeout } from '../api';
import { Candidate } from '../components/Candidate';
import { Icon } from '../components/Icon';
import { Badge, PageHead, Slider, Switch } from '../components/ui';
import '../styles/buddy.css';

/* ── 挂件数据结构（与 hufu-skin::OverlayConfig / OverlayProc 对齐） ── */

type Crop = [number, number, number, number];
type Rgb = [number, number, number];

type OverlayProc = {
  crop: Crop | null;
  key: Rgb | null;
  tol: number;
  corner: number;
  feather: number;
  cshape: 'round' | 'star';
  src_w: number;
  src_h: number;
};

type OverlayPreset = {
  name: string;
  image?: string;
  proc?: OverlayProc | null;
  [key: string]: unknown;
};

type OverlayCfg = {
  enabled: boolean;
  image: string;
  side: 'left' | 'right';
  base_height: number;
  offset_x: number;
  offset_y: number;
  gap: number;
  flip_h: boolean;
  layer: 'below' | 'above';
  opacity: number;
  v_align: 'center' | 'top' | 'bottom';
  hide_delay_ms: number;
  proc: OverlayProc | null;
  presets: OverlayPreset[];
};

type SsfPayload = {
  id?: string;
  overrides?: Record<string, unknown>;
  base?: Record<string, unknown>;
};

const EMPTY_PROC: OverlayProc = {
  crop: null,
  key: null,
  tol: 30,
  corner: 0,
  feather: 0,
  cshape: 'round',
  src_w: 0,
  src_h: 0,
};

const DEFAULT_OVERLAY: OverlayCfg = {
  enabled: false,
  image: '',
  side: 'right',
  base_height: 240,
  offset_x: 0,
  offset_y: 0,
  gap: 8,
  flip_h: false,
  layer: 'below',
  opacity: 100,
  v_align: 'center',
  hide_delay_ms: 0,
  proc: null,
  presets: [],
};

const PRESET_KEYS = [
  'enabled',
  'image',
  'side',
  'base_height',
  'offset_x',
  'offset_y',
  'gap',
  'flip_h',
  'layer',
  'opacity',
  'v_align',
  'hide_delay_ms',
] as const;

const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));
const num = (v: unknown, fallback: number) =>
  typeof v === 'number' && Number.isFinite(v) ? v : fallback;

function toCrop(v: unknown): Crop | null {
  if (!Array.isArray(v) || v.length < 4) return null;
  const n = v.slice(0, 4).map(Number);
  if (!n.every((x) => Number.isFinite(x))) return null;
  return [n[0], n[1], n[2], n[3]];
}

function toKey(v: unknown): Rgb | null {
  if (!Array.isArray(v) || v.length < 3) return null;
  const n = v.slice(0, 3).map(Number);
  if (!n.every((x) => Number.isFinite(x))) return null;
  return [
    clamp(Math.round(n[0]), 0, 255),
    clamp(Math.round(n[1]), 0, 255),
    clamp(Math.round(n[2]), 0, 255),
  ];
}

function normalizeProc(raw: unknown): OverlayProc | null {
  if (!raw || typeof raw !== 'object') return null;
  const p = raw as Record<string, unknown>;
  return {
    crop: toCrop(p.crop),
    key: toKey(p.key),
    tol: Math.round(clamp(num(p.tol, 30), 4, 120)),
    corner: Math.round(clamp(num(p.corner, 0), 0, 120)),
    feather: Math.round(clamp(num(p.feather, 0), 0, 80)),
    cshape: p.cshape === 'star' ? 'star' : 'round',
    src_w: Math.round(Math.max(0, num(p.src_w, 0))),
    src_h: Math.round(Math.max(0, num(p.src_h, 0))),
  };
}

function normalizeOverlay(raw: unknown): OverlayCfg {
  const src = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>;
  const presets = Array.isArray(src.presets)
    ? src.presets
        .filter((p): p is Record<string, unknown> => Boolean(p) && typeof p === 'object')
        .map((p) => ({
          ...p,
          name: typeof p.name === 'string' && p.name.trim() ? p.name.trim() : '未命名配置',
          image: typeof p.image === 'string' ? p.image : '',
          proc: normalizeProc(p.proc),
        }))
    : [];
  return {
    enabled: src.enabled === true,
    image: typeof src.image === 'string' ? src.image : '',
    side: src.side === 'left' ? 'left' : 'right',
    base_height: Math.round(clamp(num(src.base_height, 240), 24, 600)),
    offset_x: Math.round(clamp(num(src.offset_x, 0), -400, 400)),
    offset_y: Math.round(clamp(num(src.offset_y, 0), -400, 400)),
    gap: Math.round(clamp(num(src.gap, 8), -400, 600)),
    flip_h: src.flip_h === true,
    layer: src.layer === 'above' ? 'above' : 'below',
    opacity: Math.round(clamp(num(src.opacity, 100), 5, 100)),
    v_align:
      src.v_align === 'top' || src.v_align === 'bottom' || src.v_align === 'center'
        ? src.v_align
        : 'center',
    hide_delay_ms: Math.round(clamp(num(src.hide_delay_ms, 0), 0, 2000)),
    proc: normalizeProc(src.proc),
    presets,
  };
}

function defaultProc(img: HTMLImageElement): OverlayProc {
  return {
    ...EMPTY_PROC,
    src_w: img.naturalWidth,
    src_h: img.naturalHeight,
  };
}

function parseRatio(r: string): number | null {
  if (r === '1') return 1;
  if (r === '2:3') return 2 / 3;
  if (r === '3:4') return 3 / 4;
  if (r === '9:16') return 9 / 16;
  return null;
}

/* ── 图片工具 ─────────────────────────────────────────────────────── */

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error('图片解码失败'));
    img.src = src;
  });
}

function readFileAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const rd = new FileReader();
    rd.onload = () => resolve(String(rd.result || ''));
    rd.onerror = () => reject(new Error('图片读取失败'));
    rd.readAsDataURL(file);
  });
}

/* 【挂件图格式白名单 2026-11-02】DLL 端只按文件头魔数识别 PNG/GIF/JPEG/WEBP
   这四种（hufu-server/src/overlay.rs:251-264、hufu-tsf/src/overlaywin.rs:185-197）。
   BMP/SVG/AVIF/ICO/TIFF 浏览器能解码、旧逻辑也照单全收，但整图写进皮肤后
   实机挂件不会出现，且 DLL 静默失败没有任何报错。故前端在读取前做同一套
   魔数校验，不通过就明确提示并放弃写入。 */
type OverlayImageKind = 'png' | 'gif' | 'jpeg' | 'webp';

const OVERLAY_IMAGE_MIME: Record<OverlayImageKind, string> = {
  png: 'image/png',
  gif: 'image/gif',
  jpeg: 'image/jpeg',
  webp: 'image/webp',
};

function sniffOverlayImage(head: Uint8Array): OverlayImageKind | null {
  const at = (i: number) => head[i] ?? -1;
  /* PNG: 89 50 4E 47（完整签名后随 0D 0A 1A 0A，前 4 字节足够区分） */
  if (at(0) === 0x89 && at(1) === 0x50 && at(2) === 0x4e && at(3) === 0x47) return 'png';
  /* GIF: 47 49 46 38（"GIF8"，87a / 89a 都命中） */
  if (at(0) === 0x47 && at(1) === 0x49 && at(2) === 0x46 && at(3) === 0x38) return 'gif';
  /* JPEG: FF D8 FF */
  if (at(0) === 0xff && at(1) === 0xd8 && at(2) === 0xff) return 'jpeg';
  /* WEBP: RIFF 容器 —— 0-3 "RIFF"，8-11 "WEBP"（RIFF 头里还有 4 字节长度） */
  if (
    at(0) === 0x52 &&
    at(1) === 0x49 &&
    at(2) === 0x46 &&
    at(3) === 0x46 &&
    at(8) === 0x57 &&
    at(9) === 0x45 &&
    at(10) === 0x42 &&
    at(11) === 0x50
  ) {
    return 'webp';
  }
  return null;
}

/** 只读文件头 16 字节做魔数判定，不整读大图。 */
async function sniffOverlayImageFile(file: File): Promise<OverlayImageKind | null> {
  try {
    const head = new Uint8Array(await file.slice(0, 16).arrayBuffer());
    return sniffOverlayImage(head);
  } catch {
    return null;
  }
}

type ImageDecoderLike = {
  tracks: {
    ready: Promise<void>;
    selectedTrack: { frameCount: number } | null;
  };
};
type ImageDecoderCtor = new (init: { data: ArrayBuffer; type: string }) => ImageDecoderLike;

async function detectAnimated(dataUrl: string, mime: string): Promise<boolean> {
  const ctor = (window as unknown as { ImageDecoder?: ImageDecoderCtor }).ImageDecoder;
  if (!ctor) return mime === 'image/gif' || mime === 'image/apng';
  try {
    const buf = await (await fetchWithTimeout(dataUrl)).arrayBuffer();
    const dec = new ctor({ data: buf, type: mime || 'image/gif' });
    await dec.tracks.ready;
    return (dec.tracks.selectedTrack?.frameCount ?? 1) > 1;
  } catch {
    return false;
  }
}

async function shrinkImage(
  dataUrl: string,
  img: HTMLImageElement,
): Promise<{ url: string; img: HTMLImageElement } | null> {
  let url = dataUrl;
  for (const cap of [4096, 3072, 2048, 1536, 1024]) {
    const long = Math.max(img.naturalWidth, img.naturalHeight);
    if (long <= cap) break;
    const k = cap / long;
    const cv = document.createElement('canvas');
    cv.width = Math.max(1, Math.round(img.naturalWidth * k));
    cv.height = Math.max(1, Math.round(img.naturalHeight * k));
    const ctx = cv.getContext('2d');
    if (!ctx) return null;
    ctx.drawImage(img, 0, 0, cv.width, cv.height);
    url = cv.toDataURL('image/png');
    if (url.length <= 11_000_000) break;
  }
  if (url.length > 11_000_000) return null;
  try {
    const next = await loadImage(url);
    return { url, img: next };
  } catch {
    return null;
  }
}

function samplePixel(img: HTMLImageElement, x: number, y: number): Rgb | null {
  const cv = document.createElement('canvas');
  cv.width = 1;
  cv.height = 1;
  const ctx = cv.getContext('2d', { willReadFrequently: true });
  if (!ctx) return null;
  ctx.drawImage(img, -x, -y);
  const d = ctx.getImageData(0, 0, 1, 1).data;
  return [d[0], d[1], d[2]];
}

function autoCropBox(img: HTMLImageElement, key: Rgb | null, tol: number): Crop | null {
  const maxScan = 512;
  const k = Math.min(1, maxScan / Math.max(img.naturalWidth, img.naturalHeight));
  const w = Math.max(1, Math.round(img.naturalWidth * k));
  const h = Math.max(1, Math.round(img.naturalHeight * k));
  const cv = document.createElement('canvas');
  cv.width = w;
  cv.height = h;
  const ctx = cv.getContext('2d', { willReadFrequently: true });
  if (!ctx) return null;
  ctx.drawImage(img, 0, 0, w, h);
  const d = ctx.getImageData(0, 0, w, h).data;
  let minX = w;
  let minY = h;
  let maxX = -1;
  let maxY = -1;
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const i = (y * w + x) * 4;
      if (d[i + 3] < 8) continue;
      if (key) {
        const dist = Math.max(
          Math.abs(d[i] - key[0]),
          Math.abs(d[i + 1] - key[1]),
          Math.abs(d[i + 2] - key[2]),
        );
        if (dist <= tol) continue;
      }
      if (x < minX) minX = x;
      if (y < minY) minY = y;
      if (x > maxX) maxX = x;
      if (y > maxY) maxY = y;
    }
  }
  if (maxX < minX || maxY < minY) return null;
  const pad = 2 / k;
  const sx = Math.max(0, Math.floor(minX / k - pad));
  const sy = Math.max(0, Math.floor(minY / k - pad));
  const ex = Math.min(img.naturalWidth, Math.ceil((maxX + 1) / k + pad));
  const ey = Math.min(img.naturalHeight, Math.ceil((maxY + 1) / k + pad));
  if (ex - sx < 2 || ey - sy < 2) return null;
  return [sx, sy, ex - sx, ey - sy];
}

function processToCanvas(
  cv: HTMLCanvasElement,
  img: HTMLImageElement,
  proc: OverlayProc | null,
  flip: boolean,
  maxH: number,
) {
  const crop = proc?.crop;
  const sx = crop ? crop[0] : 0;
  const sy = crop ? crop[1] : 0;
  const sw = crop ? crop[2] : img.naturalWidth;
  const sh = crop ? crop[3] : img.naturalHeight;
  if (sw < 2 || sh < 2) return;
  const k = Math.min(1, maxH / Math.max(sw, sh));
  const w = Math.max(1, Math.round(sw * k));
  const h = Math.max(1, Math.round(sh * k));
  cv.width = w;
  cv.height = h;
  const ctx = cv.getContext('2d', { willReadFrequently: true });
  if (!ctx) return;
  ctx.clearRect(0, 0, w, h);
  if (flip) {
    ctx.translate(w, 0);
    ctx.scale(-1, 1);
  }
  ctx.drawImage(img, sx, sy, sw, sh, 0, 0, w, h);
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  if (!proc || (!proc.key && proc.corner <= 0 && proc.feather <= 0)) return;

  const key = proc.key;
  const tol = proc.tol;
  const soft = Math.max(6, tol * 0.35);
  const rr = Math.max(proc.corner, proc.feather) * (h / 240);
  const hard = proc.corner * (h / 240);
  const f = proc.feather * (h / 240);
  const halfW = w / 2;
  const halfH = h / 2;
  const id = ctx.getImageData(0, 0, w, h);
  const d = id.data;
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const i = (y * w + x) * 4;
      if (d[i + 3] === 0) continue;
      let a = d[i + 3];
      if (key) {
        const dist = Math.max(
          Math.abs(d[i] - key[0]),
          Math.abs(d[i + 1] - key[1]),
          Math.abs(d[i + 2] - key[2]),
        );
        if (dist <= tol) {
          d[i + 3] = 0;
          continue;
        }
        if (dist < tol + soft) a *= (dist - tol) / soft;
      }
      if (rr > 0) {
        const qx = Math.abs(x - halfW + 0.5) - halfW + rr;
        const qy = Math.abs(y - halfH + 0.5) - halfH + rr;
        const qxo = Math.max(qx, 0);
        const qyo = Math.max(qy, 0);
        const sd =
          proc.cshape === 'star'
            ? Math.max(Math.min(qx, qy), 0) + Math.sqrt(qxo * qxo + qyo * qyo) - rr
            : Math.min(Math.max(qx, qy), 0) + Math.sqrt(qxo * qxo + qyo * qyo) - rr;
        if (hard > 0 && sd >= 0) {
          d[i + 3] = 0;
          continue;
        }
        if (f > 0) {
          let t = -sd / f;
          t = t < 0 ? 0 : t > 1 ? 1 : t;
          a *= t;
        }
      }
      d[i + 3] = a;
    }
  }
  ctx.putImageData(id, 0, 0);
}

function cropFromPoints(
  view: { ox: number; oy: number; k: number },
  x0: number,
  y0: number,
  x1: number,
  y1: number,
  ratio: string,
  iw: number,
  ih: number,
): Crop | null {
  const sx0 = clamp((Math.min(x0, x1) - view.ox) / view.k, 0, iw);
  const sy0 = clamp((Math.min(y0, y1) - view.oy) / view.k, 0, ih);
  const sx1 = clamp((Math.max(x0, x1) - view.ox) / view.k, 0, iw);
  const sy1 = clamp((Math.max(y0, y1) - view.oy) / view.k, 0, ih);
  let w = sx1 - sx0;
  let h = sy1 - sy0;
  if (w < 2 || h < 2) return null;
  const r = parseRatio(ratio);
  if (r) {
    const cx = (sx0 + sx1) / 2;
    const cy = (sy0 + sy1) / 2;
    if (w / h > r) w = h * r;
    else h = w / r;
    if (w > iw) {
      w = iw;
      h = w / r;
    }
    if (h > ih) {
      h = ih;
      w = h * r;
    }
    const x = clamp(cx - w / 2, 0, iw - w);
    const y = clamp(cy - h / 2, 0, ih - h);
    return [Math.round(x), Math.round(y), Math.round(w), Math.round(h)];
  }
  return [Math.round(sx0), Math.round(sy0), Math.round(w), Math.round(h)];
}

function shortName(n: string): string {
  return n.replace(/（.*?）|\(.*?\)/g, '').trim() || n;
}

/* ── 页面 ─────────────────────────────────────────────────────────── */

export function BuddyPage({
  data,
  notify,
}: {
  data: Bootstrap;
  notify: (msg: string) => void;
}) {
  const [mode, setMode] = useState<'json' | 'ssf'>(data.skins.current_kind === 'ssf' ? 'ssf' : 'json');
  const [skinId, setSkinId] = useState(data.skins.current);
  const [overlay, setOverlay] = useState<OverlayCfg>(DEFAULT_OVERLAY);
  const [loaded, setLoaded] = useState(false);
  const [loadError, setLoadError] = useState('');
  const [saving, setSaving] = useState(false);
  const [dirty, setDirty] = useState(false);
  /* OV-B1：脏状态上报全局守卫（挂件自动保存有 700ms 队列，切页瞬间可能仍有未落盘改动）。 */
  useDirtyGuard('buddy', '候选挂件', dirty || saving);
  const [saveError, setSaveError] = useState('');
  const [imageName, setImageName] = useState('');
  const [imgInfo, setImgInfo] = useState<{ w: number; h: number; animated: boolean } | null>(null);
  const [imgVersion, setImgVersion] = useState(0);
  const [picking, setPicking] = useState(false);
  const [ratio, setRatio] = useState('free');
  const [draftCrop, setDraftCrop] = useState<Crop | null>(null);
  const [activePreset, setActivePreset] = useState('');

  const overlayRef = useRef(overlay);
  const dirtyRef = useRef(false);
  const lastSavedRef = useRef<OverlayCfg | null>(null);
  const imgRef = useRef<HTMLImageElement | null>(null);
  const previewRef = useRef<HTMLCanvasElement>(null);
  const workRef = useRef<HTMLCanvasElement>(null);
  const workViewRef = useRef<{ ox: number; oy: number; k: number; dw: number; dh: number } | null>(
    null,
  );
  const dragCropRef = useRef<{ x0: number; y0: number } | null>(null);
  const draftCropRef = useRef<Crop | null>(null);
  const dragLayoutRef = useRef<{ x: number; y: number; ox: number; oy: number } | null>(null);
  const dragResizeRef = useRef<{ y: number; h: number } | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const loadGenRef = useRef(0);
  const pendingRef = useRef<OverlayCfg | null>(null);
  const runningRef = useRef(false);
  const waitersRef = useRef<Array<{ resolve: (ok: boolean) => void }>>([]);
  const notifyRef = useRef(notify);
  notifyRef.current = notify;

  const setDraft = (c: Crop | null) => {
    draftCropRef.current = c;
    setDraftCrop(c);
  };

  const replaceOverlay = (next: OverlayCfg, isDirty: boolean) => {
    overlayRef.current = next;
    setOverlay(next);
    dirtyRef.current = isDirty;
    setDirty(isDirty);
  };

  const updateOverlay = (updater: (prev: OverlayCfg) => OverlayCfg) => {
    const next = updater(overlayRef.current);
    overlayRef.current = next;
    setOverlay(next);
    dirtyRef.current = true;
    setDirty(true);
  };

  /* 载入当前皮肤的挂件配置：矢量皮读 /api/skin，独立皮读 ssf-settings。 */
  const load = useCallback(async (id: string, kind: 'json' | 'ssf') => {
    const gen = ++loadGenRef.current;
    setLoaded(false);
    setLoadError('');
    setSaveError('');
    try {
      let ov: OverlayCfg;
      if (kind === 'ssf') {
        const r = await fetchWithTimeout(`/api/skin/ssf-settings?id=${encodeURIComponent(id)}`, {
          cache: 'no-store',
        });
        let body: SsfPayload & { error?: string } = {};
        try {
          body = (await r.json()) as SsfPayload & { error?: string };
        } catch {
          /* 非 JSON */
        }
        if (!r.ok) throw new Error(body.error || `读取独立皮肤设置失败（${r.status}）`);
        ov = normalizeOverlay(body.overrides?.overlay);
      } else {
        const r = await fetchWithTimeout(`/api/skin?id=${encodeURIComponent(id)}`, { cache: 'no-store' });
        let body: Record<string, unknown> = {};
        try {
          body = (await r.json()) as Record<string, unknown>;
        } catch {
          /* 非 JSON */
        }
        if (!r.ok) throw new Error(`读取皮肤失败（${r.status}）`);
        ov = normalizeOverlay(body.overlay);
      }
      if (gen !== loadGenRef.current) return;
      setMode(kind);
      setSkinId(id);
      replaceOverlay(ov, false);
      lastSavedRef.current = ov;
      setDraft(null);
      setRatio('free');
      setActivePreset('');
      setImageName(ov.image ? '当前挂件图' : '');
      setLoaded(true);
    } catch (e) {
      if (gen === loadGenRef.current) {
        setLoadError((e as Error).message);
        setLoaded(false);
      }
    }
  }, []);

  useEffect(() => {
    const kind = data.skins.current_kind === 'ssf' ? 'ssf' : 'json';
    void load(data.skins.current, kind);
  }, [data.skins.current, data.skins.current_kind, load]);

  /* 图片解码（换图/切配置后重新加载）。 */
  useEffect(() => {
    let alive = true;
    const src = overlay.image;
    // 换图/清图先让旧帧下线，避免异步解码窗口里绘制上一张图。
    imgRef.current = null;
    setImgInfo(null);
    setImgVersion((v) => v + 1);
    if (!src) return;
    (async () => {
      try {
        const img = await loadImage(src);
        if (!alive) return;
        imgRef.current = img;
        const mime = src.startsWith('data:') ? src.slice(5, src.indexOf(';')) : '';
        const animated = await detectAnimated(src, mime);
        if (!alive) return;
        setImgInfo({ w: img.naturalWidth, h: img.naturalHeight, animated });
        setImgVersion((v) => v + 1);
      } catch {
        if (!alive) return;
        imgRef.current = null;
        setImgInfo(null);
        setImgVersion((v) => v + 1);
        notifyRef.current('挂件图片解码失败，请重新选择图片');
      }
    })();
    return () => {
      alive = false;
    };
  }, [overlay.image]);

  /* 预览/处理台重绘。side 在依赖里：贴边切换会让 <BuddyWidget> 换到
     children 数组的另一位置（canvas 卸载重建），不补依赖新画布不会被
     绘制 → AP-01 切贴边方向预览空白。 */
  useEffect(() => {
    const cv = previewRef.current;
    const img = imgRef.current;
    if (!cv) return;
    if (!img) {
      cv.width = 1;
      cv.height = 1;
      const ctx = cv.getContext('2d');
      ctx?.clearRect(0, 0, 1, 1);
      return;
    }
    processToCanvas(cv, img, overlay.proc, overlay.flip_h, 640);
  }, [overlay.proc, overlay.flip_h, overlay.side, imgVersion]);

  const drawWork = useCallback(() => {
    const cv = workRef.current;
    if (!cv) return;
    const ctx = cv.getContext('2d');
    if (!ctx) return;
    const W = cv.width;
    const H = cv.height;
    ctx.clearRect(0, 0, W, H);
    const img = imgRef.current;
    if (!img) {
      workViewRef.current = null;
      ctx.fillStyle = '#8d8980';
      ctx.font = '13px "Segoe UI", sans-serif';
      ctx.fillText('点「选择图片…」载入', 64, H / 2);
      return;
    }
    const iw = img.naturalWidth;
    const ih = img.naturalHeight;
    const k = Math.min((W - 16) / iw, (H - 16) / ih);
    const dw = iw * k;
    const dh = ih * k;
    const ox = (W - dw) / 2;
    const oy = (H - dh) / 2;
    workViewRef.current = { ox, oy, k, dw, dh };
    ctx.drawImage(img, ox, oy, dw, dh);
    const c = draftCropRef.current || overlay.proc?.crop;
    if (c) {
      const rx = ox + c[0] * k;
      const ry = oy + c[1] * k;
      const rw = c[2] * k;
      const rh = c[3] * k;
      ctx.save();
      ctx.fillStyle = 'rgba(0,0,0,0.55)';
      ctx.beginPath();
      ctx.rect(ox, oy, dw, dh);
      ctx.rect(rx, ry, rw, rh);
      ctx.fill('evenodd');
      ctx.strokeStyle = '#ffd75e';
      ctx.lineWidth = 1.5;
      ctx.strokeRect(rx + 0.5, ry + 0.5, rw, rh);
      ctx.restore();
    }
  }, [overlay.proc]);

  useEffect(() => {
    drawWork();
  }, [drawWork, draftCrop, imgVersion]);

  /* 挂件保存：队列串行 + 合并；同一对象不重复 POST。 */
  const persistOne = async (target: OverlayCfg) => {
    if (lastSavedRef.current === target) return;
    if (mode === 'ssf') {
      const r = await fetchWithTimeout(`/api/skin/ssf-settings?id=${encodeURIComponent(skinId)}`, {
        cache: 'no-store',
      });
      let cur: SsfPayload & { error?: string } = {};
      try {
        cur = (await r.json()) as SsfPayload & { error?: string };
      } catch {
        /* 非 JSON */
      }
      if (!r.ok) throw new Error(cur.error || `读取独立皮肤设置失败（${r.status}）`);
      const overrides = { ...(cur.overrides || {}), overlay: target };
      const p = await fetchWithTimeout('/api/skin/ssf-settings', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id: skinId, overrides }),
      });
      let body: { error?: string } = {};
      try {
        body = (await p.json()) as { error?: string };
      } catch {
        /* 非 JSON */
      }
      if (!p.ok) throw new Error(body.error || `保存失败（${p.status}）`);
    } else {
      const r = await fetchWithTimeout(`/api/skin?id=${encodeURIComponent(skinId)}`, { cache: 'no-store' });
      let skin: Record<string, unknown> = {};
      try {
        skin = (await r.json()) as Record<string, unknown>;
      } catch {
        /* 非 JSON */
      }
      if (!r.ok) throw new Error(`读取皮肤失败（${r.status}）`);
      /* A-2：校验返回 id 与请求一致——后端在皮肤损坏时返回默认皮，
         不校验会把默认皮整份覆盖目标皮肤文件。 */
      if ((skin as { id?: unknown }).id !== skinId) {
        throw new Error(`皮肤「${skinId}」读取异常：服务器返回了别的皮肤（可能文件已损坏）`);
      }
      const p = await fetchWithTimeout('/api/skin', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        /* select:false —— 挂件改动即存不再顺带切换当前皮肤。 */
        body: JSON.stringify({ ...skin, overlay: target, select: false }),
      });
      let body: { error?: string } = {};
      try {
        body = (await p.json()) as { error?: string };
      } catch {
        /* 非 JSON */
      }
      if (!p.ok) throw new Error(body.error || `保存失败（${p.status}）`);
    }
    lastSavedRef.current = target;
    if (overlayRef.current === target) {
      dirtyRef.current = false;
      setDirty(false);
    }
    setSaveError('');
  };

  const persist = (target: OverlayCfg): Promise<boolean> => {
    pendingRef.current = target;
    const promise = new Promise<boolean>((resolve) => {
      waitersRef.current.push({ resolve });
    });
    if (runningRef.current) return promise;
    runningRef.current = true;
    setSaving(true);
    const gen = loadGenRef.current;
    void (async () => {
      let ok = true;
      try {
        while (pendingRef.current) {
          const next = pendingRef.current;
          pendingRef.current = null;
          if (gen !== loadGenRef.current) {
            ok = false;
            break;
          }
          try {
            await persistOne(next);
          } catch (e) {
            ok = false;
            const msg = (e as Error).message;
            setSaveError(msg);
            notifyRef.current(`挂件保存失败：${msg}`);
            break;
          }
        }
      } finally {
        runningRef.current = false;
        setSaving(false);
        const waiters = waitersRef.current;
        waitersRef.current = [];
        waiters.forEach((w) => w.resolve(ok));
      }
    })();
    return promise;
  };

  /* 改动即存：700ms 防抖；图片/裁剪等大改走同一队列。 */
  useEffect(() => {
    if (!loaded) return;
    if (lastSavedRef.current === overlay) return;
    const timer = window.setTimeout(() => {
      void persist(overlay);
    }, 700);
    return () => window.clearTimeout(timer);
  }, [overlay, loaded]);

  const retrySave = () => {
    setSaveError('');
    void persist(overlayRef.current);
  };

  /* ── 图片选择 / 删除 ─────────────────────────────────────────── */

  const chooseFile = () => fileRef.current?.click();

  const onFileChange = async (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = '';
    if (!file) return;
    if (!file.type.startsWith('image/')) {
      notifyRef.current('请选择图片文件');
      return;
    }
    /* 魔数校验（与 DLL 同一白名单）：accept 已收窄，但「所有文件」入口、
       改过扩展名的文件仍能绕过 MIME 判断，必须按文件内容兜底。 */
    const kind = await sniffOverlayImageFile(file);
    if (!kind) {
      notifyRef.current(
        '仅支持 PNG / GIF / JPEG / WEBP 挂件图（此格式 DLL 挂件不显示），已忽略所选文件',
      );
      return;
    }
    if (file.size > 24 * 1024 * 1024) {
      notifyRef.current('图片文件过大（>24MB），请精简后再挂');
      return;
    }
    try {
      let dataUrl = await readFileAsDataUrl(file);
      /* 扩展名/MIME 可能与真实内容不一致（如 .bmp 实际是 PNG）：按魔数
         结果改写 data URL 前缀，保证下游解码与动图检测按真实格式走。 */
      const mime = OVERLAY_IMAGE_MIME[kind];
      if (!dataUrl.startsWith(`data:${mime};base64,`)) {
        const comma = dataUrl.indexOf(',');
        if (comma > 0) dataUrl = `data:${mime};base64,${dataUrl.slice(comma + 1)}`;
      }
      let img = await loadImage(dataUrl);
      const animated = await detectAnimated(dataUrl, mime);
      if (dataUrl.length > 11_000_000) {
        if (animated) {
          notifyRef.current('动图超过 8MB，无法自动压缩，请精简后再挂');
          return;
        }
        const shrunk = await shrinkImage(dataUrl, img);
        if (!shrunk) {
          notifyRef.current('图片过大（>8MB），请精简后再挂');
          return;
        }
        dataUrl = shrunk.url;
        img = shrunk.img;
      }
      setImageName(file.name);
      setDraft(null);
      setRatio('free');
      updateOverlay((o) => ({ ...o, image: dataUrl, proc: defaultProc(img) }));
      notifyRef.current(`已载入 ${file.name}（${img.naturalWidth}×${img.naturalHeight}）`);
    } catch (err) {
      notifyRef.current(`载入图片失败：${(err as Error).message}`);
    }
  };

  const deleteImage = async () => {
    const before = overlayRef.current;
    const old = before.image;
    if (!old) {
      notifyRef.current('当前没有挂件图片');
      return;
    }
    if (
      !window.confirm(
        '删除当前挂件图片？贴图数据会被清空；启用开关与贴法参数保留，重新选图即恢复。其他皮肤里同一张图的副本也会一并扫除。',
      )
    ) {
      return;
    }
    const presets = (before.presets || []).map((p) =>
      p.image === old ? { ...p, image: '', proc: null } : p,
    );
    const next: OverlayCfg = { ...before, image: '', proc: null, presets };
    updateOverlay(() => next);
    setImageName('');
    setDraft(null);
    setPicking(false);
    const ok = await persist(next);
    if (!ok) {
      replaceOverlay(before, true);
      return;
    }
    notifyRef.current('挂件图片已删除（重新「选择图片…」可再挂）');
    if (old.length >= 1024) {
      try {
        const r = await fetchWithTimeout('/api/skin/overlay-purge', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ image: old }),
        });
        let body: { vector?: Array<{ name?: string; skin?: string }>; ssf?: Array<{ name?: string; skin?: string }> } = {};
        try {
          body = (await r.json()) as typeof body;
        } catch {
          /* 非 JSON */
        }
        const names = [...(body.vector || []), ...(body.ssf || [])]
          .map((x) => x.name || x.skin)
          .filter((x): x is string => Boolean(x));
        if (names.length) {
          notifyRef.current(`已同步扫除其他皮肤中的同一张图（${names.length} 处）：${names.join('、')}`);
        }
      } catch {
        /* 清扫失败不拦删除主流程 */
      }
    }
  };

  /* ── 预设（多配置） ───────────────────────────────────────────── */

  const snapshotPreset = (name: string, ov: OverlayCfg): OverlayPreset => {
    const snap: OverlayPreset = { name };
    const bag = snap as Record<string, unknown>;
    for (const key of PRESET_KEYS) {
      bag[key] = ov[key];
    }
    if (ov.proc) {
      snap.proc = {
        ...ov.proc,
        crop: ov.proc.crop ? ([...ov.proc.crop] as Crop) : null,
        key: ov.proc.key ? ([...ov.proc.key] as Rgb) : null,
      };
    }
    return snap;
  };

  const savePreset = async () => {
    if (!overlayRef.current.image) {
      notifyRef.current('先选择图片再保存配置');
      return;
    }
    const def = activePreset || '';
    const answer = window.prompt('保存为配置（新名=新增，同名=覆盖）：', def);
    const name = answer?.trim();
    if (!name) return;
    const snap = snapshotPreset(name, overlayRef.current);
    const presets = [...(overlayRef.current.presets || [])];
    const idx = presets.findIndex((p) => p.name === name);
    const overwrote = idx >= 0;
    if (idx >= 0) presets[idx] = snap;
    else presets.push(snap);
    const next: OverlayCfg = { ...overlayRef.current, presets };
    updateOverlay(() => next);
    setActivePreset(name);
    const ok = await persist(next);
    notifyRef.current(
      ok ? (overwrote ? `配置「${name}」已更新` : `配置「${name}」已保存`) : '配置保存失败',
    );
  };

  const applyPreset = async (p: OverlayPreset) => {
    const pCfg = p as unknown as Partial<OverlayCfg>;
    const next = normalizeOverlay({
      ...overlayRef.current,
      ...pCfg,
      presets: overlayRef.current.presets,
    });
    updateOverlay(() => next);
    setActivePreset(p.name);
    setDraft(null);
    setRatio('free');
    setPicking(false);
    setImageName(next.image ? p.name : '');
    const ok = await persist(next);
    notifyRef.current(ok ? `已切换到配置「${p.name}」` : `切换配置「${p.name}」失败`);
  };

  const deletePreset = async (name: string) => {
    if (!window.confirm(`删除配置「${name}」？（只删这套快照，不影响当前挂件）`)) return;
    const presets = (overlayRef.current.presets || []).filter((p) => p.name !== name);
    const next: OverlayCfg = { ...overlayRef.current, presets };
    updateOverlay(() => next);
    if (activePreset === name) setActivePreset('');
    const ok = await persist(next);
    notifyRef.current(ok ? `配置「${name}」已删除` : `删除配置「${name}」失败`);
  };

  /* ── 布局拖拽 / 处理台交互 ────────────────────────────────────── */

  const onWidgetPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (!overlayRef.current.image) return;
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      /* synthetic */
    }
    dragLayoutRef.current = {
      x: e.clientX,
      y: e.clientY,
      ox: overlayRef.current.offset_x,
      oy: overlayRef.current.offset_y,
    };
  };

  const onWidgetPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    const d = dragLayoutRef.current;
    if (!d) return;
    const dx = e.clientX - d.x;
    const dy = e.clientY - d.y;
    updateOverlay((o) => ({
      ...o,
      offset_x: clamp(Math.round(d.ox + dx), -400, 400),
      offset_y: clamp(Math.round(d.oy + dy), -400, 400),
    }));
  };

  const endWidgetDrag = () => {
    dragLayoutRef.current = null;
  };

  /* 【挂件拖拽改大小 2026-11-01】右下角 grip 手柄：拖动改 base_height
     （真实挂件窗口高度，宽度由图片比例自动跟）。拖动走同一
     updateOverlay 乐观更新 + 700ms 队列保存链路。 */
  const onResizePointerDown = (e: ReactPointerEvent<HTMLSpanElement>) => {
    if (!overlayRef.current.image) return;
    e.stopPropagation();
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      /* synthetic */
    }
    dragResizeRef.current = { y: e.clientY, h: overlayRef.current.base_height };
  };

  const onResizePointerMove = (e: ReactPointerEvent<HTMLSpanElement>) => {
    const d = dragResizeRef.current;
    if (!d) return;
    const dy = e.clientY - d.y;
    updateOverlay((o) => ({ ...o, base_height: clamp(Math.round(d.h + dy), 24, 600) }));
  };

  const endResizeDrag = () => {
    dragResizeRef.current = null;
  };

  const canvasPoint = (e: ReactPointerEvent<HTMLCanvasElement>) => {
    const cv = e.currentTarget;
    const rect = cv.getBoundingClientRect();
    return {
      x: (e.clientX - rect.left) * (cv.width / rect.width),
      y: (e.clientY - rect.top) * (cv.height / rect.height),
    };
  };

  const pickAt = (view: { ox: number; oy: number; k: number }, x: number, y: number) => {
    const img = imgRef.current;
    if (!img) return;
    const sx = Math.floor((x - view.ox) / view.k);
    const sy = Math.floor((y - view.oy) / view.k);
    if (sx < 0 || sy < 0 || sx >= img.naturalWidth || sy >= img.naturalHeight) return;
    const c = samplePixel(img, sx, sy);
    if (!c) return;
    updateOverlay((o) => ({
      ...o,
      proc: {
        ...(o.proc || defaultProc(img)),
        key: c,
        src_w: img.naturalWidth,
        src_h: img.naturalHeight,
      },
    }));
    setPicking(false);
    notifyRef.current(`已取背景色 rgb(${c[0]}, ${c[1]}, ${c[2]})`);
  };

  const onWorkPointerDown = (e: ReactPointerEvent<HTMLCanvasElement>) => {
    const img = imgRef.current;
    const view = workViewRef.current;
    if (!img || !view) return;
    const { x, y } = canvasPoint(e);
    if (picking) {
      pickAt(view, x, y);
      return;
    }
    if (x < view.ox || x > view.ox + view.dw || y < view.oy || y > view.oy + view.dh) return;
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      /* synthetic */
    }
    dragCropRef.current = { x0: x, y0: y };
    setDraft(null);
  };

  const onWorkPointerMove = (e: ReactPointerEvent<HTMLCanvasElement>) => {
    const d = dragCropRef.current;
    const img = imgRef.current;
    const view = workViewRef.current;
    if (!d || !img || !view) return;
    const { x, y } = canvasPoint(e);
    const crop = cropFromPoints(
      view,
      d.x0,
      d.y0,
      x,
      y,
      ratio,
      img.naturalWidth,
      img.naturalHeight,
    );
    if (crop) setDraft(crop);
  };

  const endWorkDrag = () => {
    dragCropRef.current = null;
  };

  const updateProc = (patch: Partial<OverlayProc>) => {
    const img = imgRef.current;
    if (!img) return;
    updateOverlay((o) => ({
      ...o,
      proc: {
        ...(o.proc || defaultProc(img)),
        ...patch,
        src_w: img.naturalWidth,
        src_h: img.naturalHeight,
      },
    }));
  };

  const chooseRatio = (r: string) => {
    setRatio(r);
    const img = imgRef.current;
    if (!img || r === 'free') return;
    const rv = parseRatio(r);
    if (!rv) return;
    let w = img.naturalWidth;
    let h = w / rv;
    if (h > img.naturalHeight) {
      h = img.naturalHeight;
      w = h * rv;
    }
    setDraft([
      Math.round((img.naturalWidth - w) / 2),
      Math.round((img.naturalHeight - h) / 2),
      Math.round(w),
      Math.round(h),
    ]);
  };

  const applyCropDraft = () => {
    const img = imgRef.current;
    const crop = draftCropRef.current;
    if (!img) return;
    if (!crop) {
      notifyRef.current('先在处理台拖一个裁剪框');
      return;
    }
    updateProc({ crop });
    setDraft(null);
    setRatio('free');
    notifyRef.current(`裁剪已应用 ${crop[2]}×${crop[3]}`);
  };

  const resetCrop = () => {
    setDraft(null);
    updateProc({ crop: null });
    notifyRef.current('裁剪已还原');
  };

  const autoCrop = () => {
    const img = imgRef.current;
    if (!img) return;
    const proc = overlayRef.current.proc;
    const crop = autoCropBox(img, proc?.key ?? null, proc?.tol ?? 30);
    if (!crop) {
      notifyRef.current('没有找到可裁剪的内容');
      return;
    }
    updateProc({ crop });
    setDraft(null);
    notifyRef.current(`已自动裁到内容 ${crop[2]}×${crop[3]}`);
  };

  const hasImage = Boolean(overlay.image) && Boolean(imgInfo);
  const effectiveProc = overlay.proc || (imgRef.current ? defaultProc(imgRef.current) : EMPTY_PROC);
  const skinMeta = data.skins.skins.find((s) => s.id === skinId);
  const skinName = shortName(skinMeta?.name || skinId || '—');
  const modeLabel = mode === 'ssf' ? '独立皮肤' : '矢量皮肤';
  const saveLabel = saving ? '保存中…' : saveError ? '保存失败' : dirty ? '未保存' : '已保存';
  const saveTone = saveError ? 'gold' : dirty ? 'gold' : 'green';
  const hideOn = overlay.hide_delay_ms > 0;
  const placeholderMargin =
    overlay.side === 'right'
      ? { marginLeft: Math.max(0, overlay.gap) }
      : { marginRight: Math.max(0, overlay.gap) };
  const placeholder = !hasImage ? (
    overlay.image ? (
      <div className="buddy-stage-empty buddy-stage-loading" style={placeholderMargin}>
        <Icon name="refresh" />
        <b>图片载入中…</b>
        <span>正在解码挂件图</span>
      </div>
    ) : (
      <button
        type="button"
        className="buddy-stage-empty"
        style={placeholderMargin}
        onClick={chooseFile}
      >
        <Icon name="image" />
        <b>选择图片</b>
        <span>挂件会贴在候选框旁显示</span>
      </button>
    )
  ) : null;

  if (loadError && !loaded) {
    return (
      <>
        <PageHead
          title="候选挂件"
          sub={
            <>
              <span className="dot muted" />
              {modeLabel} · {skinName}
            </>
          }
        />
        <div className="buddy-page">
          <div className="card buddy-loaderror">
            <Icon name="info" />
            <div>
              <b>读取挂件配置失败</b>
              <p>{loadError}</p>
              <p className="buddy-loaderror-note">
                独立皮肤模式需要 `GET /api/skin/ssf-settings?id=…` 的缓存；矢量皮肤模式读取 `/api/skin`。
              </p>
            </div>
            <button
              type="button"
              className="btn outline"
              onClick={() => void load(data.skins.current, data.skins.current_kind === 'ssf' ? 'ssf' : 'json')}
            >
              <Icon name="refresh" />
              重试
            </button>
          </div>
        </div>
      </>
    );
  }

  return (
    <>
      <PageHead
        title="候选挂件"
        sub={
          <>
            <span className={overlay.enabled ? 'dot' : 'dot muted'} />
            {modeLabel} · {skinName} · {overlay.enabled ? '已启用' : '已关闭'}
            <span className="buddy-sep">·</span>
            {overlay.image ? '已选图' : '未选图'}
            <span className="buddy-sep">·</span>
            {saving ? '保存中' : saveError ? '保存失败' : dirty ? '有未保存改动' : '已同步'}
          </>
        }
        actions={
          <>
            <button type="button" className="btn ghost sm" onClick={() => void load(skinId, mode)}>
              <Icon name="refresh" />
              重新读取
            </button>
            <button type="button" className="btn outline sm" onClick={chooseFile}>
              <Icon name="upload" />
              选择图片…
            </button>
          </>
        }
      />

      <div className="buddy-page">
        <section className="card buddy-toolbar">
          <div className="buddy-toolbar-main">
            <div className="buddy-enable">
              <Switch
                on={overlay.enabled}
                onChange={(v) => {
                  updateOverlay((o) => ({ ...o, enabled: v }));
                  if (v && !overlayRef.current.image) notifyRef.current('已启用挂件，选择图片后即可显示');
                  else notifyRef.current(v ? '候选挂件已启用' : '候选挂件已关闭');
                }}
                title="启用挂件"
              />
              <div>
                <b>启用挂件</b>
                <span>兄弟窗跟随候选窗，全程鼠标穿透。</span>
              </div>
            </div>
            <input
              ref={fileRef}
              type="file"
              /* 与 DLL 魔数白名单一致：PNG/GIF/JPEG/WEBP 四种，其余格式
                 选进来也只会被 onFileChange 的魔数校验拒绝。 */
              accept="image/png,image/gif,image/jpeg,image/webp,.png,.gif,.jpg,.jpeg,.webp"
              className="buddy-file-input"
              onChange={(e) => void onFileChange(e)}
            />
            <button type="button" className="btn outline sm" onClick={chooseFile}>
              <Icon name="upload" />
              选择图片…
            </button>
            <button
              type="button"
              className="btn ghost sm"
              onClick={() => void deleteImage()}
              disabled={!overlay.image}
            >
              <Icon name="close" />
              删除图片
            </button>
            <span className="buddy-fileinfo">
              {overlay.image
                ? `${imageName || '当前挂件图'}${imgInfo ? ` · ${imgInfo.w}×${imgInfo.h}` : ''}`
                : '未选图'}
            </span>
            {imgInfo?.animated ? <Badge tone="gold">动图</Badge> : null}
            {dirty ? <Badge tone="gold">未保存</Badge> : null}
          </div>
          <div className="buddy-toolbar-side">
            <label className="buddy-control-label" htmlFor="buddy-preset-select">
              配置
            </label>
            <select
              id="buddy-preset-select"
              className="field-input buddy-select"
              value={activePreset}
              onChange={(e) => {
                const name = e.target.value;
                if (!name) {
                  setActivePreset('');
                  return;
                }
                const p = (overlay.presets || []).find((x) => x.name === name);
                if (p) void applyPreset(p);
              }}
            >
              <option value="">新配置</option>
              {(overlay.presets || []).map((p) => (
                <option key={p.name} value={p.name}>
                  {p.name}
                </option>
              ))}
            </select>
            <button type="button" className="btn outline sm" onClick={() => void savePreset()}>
              保存为配置…
            </button>
            <button
              type="button"
              className="btn ghost sm"
              onClick={() => void deletePreset(activePreset)}
              disabled={!activePreset}
            >
              删除
            </button>
          </div>
        </section>

        <div className="buddy-grid">
          <div className="buddy-main">
            <section className="card buddy-stage-card">
              <div className="buddy-stage-head">
                <div>
                  <b>候选窗预览</b>
                  <span>拖挂件改位置；拖右下角圆点改大小；改动自动保存，约 3 秒后同步到真实挂件窗口</span>
                </div>
                <div className="buddy-stage-actions">
                  {imgInfo?.animated ? <Badge tone="gold">动图 · 运行时逐帧</Badge> : null}
                  <Badge tone={saveTone}>{saveLabel}</Badge>
                  {saveError ? (
                    <button type="button" className="btn goldline sm" onClick={retrySave}>
                      <Icon name="refresh" />
                      重试保存
                    </button>
                  ) : null}
                </div>
              </div>
              <div className="buddy-stage">
                <div
                  className="buddy-row"
                  style={{
                    alignItems:
                      overlay.v_align === 'top'
                        ? 'flex-start'
                        : overlay.v_align === 'bottom'
                          ? 'flex-end'
                          : 'center',
                  }}
                >
                  {overlay.side === 'left' ? (
                    hasImage ? (
                      <BuddyWidget
                        hasImage={hasImage}
                        overlay={overlay}
                        previewRef={previewRef}
                        onPointerDown={onWidgetPointerDown}
                        onPointerMove={onWidgetPointerMove}
                        onPointerUp={endWidgetDrag}
                        onResizeDown={onResizePointerDown}
                        onResizeMove={onResizePointerMove}
                        onResizeUp={endResizeDrag}
                      />
                    ) : (
                      placeholder
                    )
                  ) : null}
                  <div className="buddy-cand" style={{ zIndex: 2 }}>
                    {/* width=200：贴近真实候选窗宽（预览位置与真实挂件
                        的 gap 基准一致；宽 310 会让水平距离感失真）。 */}
                    <Candidate skin={data.skin} size="sm" width={200} />
                  </div>
                  {overlay.side === 'right' ? (
                    hasImage ? (
                      <BuddyWidget
                        hasImage={hasImage}
                        overlay={overlay}
                        previewRef={previewRef}
                        onPointerDown={onWidgetPointerDown}
                        onPointerMove={onWidgetPointerMove}
                        onPointerUp={endWidgetDrag}
                        onResizeDown={onResizePointerDown}
                        onResizeMove={onResizePointerMove}
                        onResizeUp={endResizeDrag}
                      />
                    ) : (
                      placeholder
                    )
                  ) : null}
                </div>
              </div>
              <div className="buddy-stage-foot">
                <span>
                  {overlay.side === 'left' ? '贴左边' : '贴右边'} · {overlay.layer === 'above' ? '图在上' : '图在下'} ·
                  垂直{overlay.v_align === 'top' ? '顶对齐' : overlay.v_align === 'bottom' ? '底对齐' : '居中'} · 高度{' '}
                  {overlay.base_height}px · 间距 {overlay.gap}px · 透明度 {overlay.opacity}%
                </span>
                <span className="buddy-stage-note">
                  {mode === 'ssf'
                    ? '独立皮肤模式：预览用矢量候选窗示意，真实候选窗以运行时为准。'
                    : '预览按真实皮肤参数渲染。'}
                </span>
              </div>
            </section>

            <section className="card buddy-work-card">
              <div className="buddy-work-head">
                <div>
                  <b>处理台</b>
                  <span>
                    {picking
                      ? '取色模式：点图上背景处取色'
                      : '拖框裁剪；比例会约束拖框；取色后点图上背景'}
                  </span>
                </div>
                <div className="buddy-work-actions">
                  <button
                    type="button"
                    className={`btn sm ${picking ? 'goldline' : 'outline'}`}
                    onClick={() => setPicking((v) => !v)}
                    disabled={!hasImage}
                  >
                    <Icon name="eye" />
                    {picking ? '取消取色' : '取背景色'}
                  </button>
                  <button
                    type="button"
                    className="btn ghost sm"
                    onClick={() => {
                      updateProc({ key: null });
                      setPicking(false);
                    }}
                    disabled={!hasImage || !overlay.proc?.key}
                  >
                    取消抠图
                  </button>
                </div>
              </div>
              <canvas
                ref={workRef}
                width={360}
                height={240}
                className={`buddy-work${picking ? ' picking' : ''}`}
                onPointerDown={onWorkPointerDown}
                onPointerMove={onWorkPointerMove}
                onPointerUp={endWorkDrag}
                onPointerCancel={endWorkDrag}
              />
              <div className="buddy-work-foot">
                <span>
                  原图 {imgInfo ? `${imgInfo.w}×${imgInfo.h}` : '—'} · 裁剪{' '}
                  {effectiveProc.crop
                    ? `${effectiveProc.crop[2]}×${effectiveProc.crop[3]}`
                    : '全图'}
                  {effectiveProc.crop ? ` @ ${effectiveProc.crop[0]},${effectiveProc.crop[1]}` : ''}
                  {effectiveProc.key
                    ? ` · 抠图 rgb(${effectiveProc.key[0]},${effectiveProc.key[1]},${effectiveProc.key[2]}) 容差 ${effectiveProc.tol}`
                    : ''}
                </span>
                <span className="buddy-work-tip">处理结果是原图 + proc 参数；动图由运行时逐帧处理。</span>
              </div>
            </section>
          </div>

          <aside className="card buddy-inspector">
            <div className="buddy-insp-head">
              <div>
                <b>挂件参数</b>
                <span>{mode === 'ssf' ? '写入 ssf overrides.overlay' : '写入 skin.overlay'}</span>
              </div>
              <Badge tone={saveTone}>{saveLabel}</Badge>
            </div>

            <div className="buddy-insp-body">
              <section className="buddy-sec">
                <div className="buddy-sec-title">贴法</div>
                <div className="buddy-line">
                  <span>贴边</span>
                  <div className="buddy-seg">
                    {(['right', 'left'] as const).map((side) => (
                      <button
                        key={side}
                        type="button"
                        className={overlay.side === side ? 'on' : ''}
                        onClick={() => updateOverlay((o) => ({ ...o, side }))}
                      >
                        {side === 'right' ? '右侧' : '左侧'}
                      </button>
                    ))}
                  </div>
                </div>
                <div className="buddy-line">
                  <span>图层</span>
                  <select
                    className="field-input buddy-select"
                    value={overlay.layer}
                    onChange={(e) =>
                      updateOverlay((o) => ({
                        ...o,
                        layer: e.target.value === 'above' ? 'above' : 'below',
                      }))
                    }
                  >
                    <option value="below">图在下（被候选框压住）</option>
                    <option value="above">图在上（盖住候选框）</option>
                  </select>
                </div>
                <div className="buddy-line">
                  <span>垂直对齐</span>
                  <select
                    className="field-input buddy-select"
                    value={overlay.v_align}
                    onChange={(e) =>
                      updateOverlay((o) => ({
                        ...o,
                        v_align:
                          e.target.value === 'top'
                            ? 'top'
                            : e.target.value === 'bottom'
                              ? 'bottom'
                              : 'center',
                      }))
                    }
                  >
                    <option value="center">居中</option>
                    <option value="top">顶对齐</option>
                    <option value="bottom">底对齐</option>
                  </select>
                </div>
              </section>

              <section className="buddy-sec">
                <div className="buddy-sec-title">尺寸与透明度</div>
                <div className="buddy-slider-line">
                  <div>
                    <span>高度</span>
                    <b>{overlay.base_height}px</b>
                  </div>
                  <Slider
                    value={overlay.base_height}
                    min={24}
                    max={600}
                    step={5}
                    width={150}
                    label="挂件高度"
                    onChange={(v) => updateOverlay((o) => ({ ...o, base_height: v }))}
                  />
                </div>
                <div className="buddy-slider-line">
                  <div>
                    <span>间距</span>
                    <b>{overlay.gap}px</b>
                  </div>
                  <Slider
                    value={overlay.gap}
                    min={-400}
                    max={600}
                    step={2}
                    width={150}
                    label="与候选框间距"
                    onChange={(v) => updateOverlay((o) => ({ ...o, gap: v }))}
                  />
                </div>
                <div className="buddy-slider-line">
                  <div>
                    <span>不透明度</span>
                    <b>{overlay.opacity}%</b>
                  </div>
                  <Slider
                    value={overlay.opacity}
                    min={5}
                    max={100}
                    step={5}
                    width={150}
                    label="挂件不透明度"
                    onChange={(v) => updateOverlay((o) => ({ ...o, opacity: v }))}
                  />
                </div>
                <div className="buddy-line">
                  <span>偏移</span>
                  <span className="buddy-offset">
                    x {overlay.offset_x} / y {overlay.offset_y}
                  </span>
                  <button
                    type="button"
                    className="btn ghost sm"
                    onClick={() => updateOverlay((o) => ({ ...o, offset_x: 0, offset_y: 0 }))}
                    disabled={overlay.offset_x === 0 && overlay.offset_y === 0}
                  >
                    归零
                  </button>
                </div>
                <div className="buddy-line">
                  <span>延时消失</span>
                  <Switch
                    on={hideOn}
                    onChange={(v) =>
                      updateOverlay((o) => ({ ...o, hide_delay_ms: v ? 500 : 0 }))
                    }
                    title="候选窗收起后挂件停留时长"
                  />
                </div>
                {hideOn ? (
                  <div className="buddy-slider-line">
                    <div>
                      <span>延迟</span>
                      <b>{(overlay.hide_delay_ms / 1000).toFixed(1)}s</b>
                    </div>
                    <Slider
                      value={overlay.hide_delay_ms / 1000}
                      min={0.2}
                      max={2}
                      step={0.1}
                      width={150}
                      label="延时消失秒数"
                      onChange={(v) =>
                        updateOverlay((o) => ({
                          ...o,
                          hide_delay_ms: Math.round(v * 1000),
                        }))
                      }
                    />
                  </div>
                ) : null}
              </section>

              <section className="buddy-sec">
                <div className="buddy-sec-title">
                  图片处理
                  {!hasImage ? <span className="buddy-sec-hint">先选择图片</span> : null}
                </div>
                <div className={hasImage ? '' : 'buddy-disabled'}>
                  <div className="buddy-line">
                    <span>裁剪比例</span>
                    <select
                      className="field-input buddy-select"
                      value={ratio}
                      onChange={(e) => chooseRatio(e.target.value)}
                      disabled={!hasImage}
                    >
                      <option value="free">自由</option>
                      <option value="1">1:1</option>
                      <option value="2:3">2:3（竖）</option>
                      <option value="3:4">3:4（竖）</option>
                      <option value="9:16">9:16（竖屏）</option>
                    </select>
                  </div>
                  <div className="buddy-btn-row">
                    <button
                      type="button"
                      className="btn outline sm"
                      onClick={applyCropDraft}
                      disabled={!hasImage}
                    >
                      应用裁剪
                    </button>
                    <button
                      type="button"
                      className="btn ghost sm"
                      onClick={autoCrop}
                      disabled={!hasImage}
                    >
                      自动裁到内容
                    </button>
                    <button
                      type="button"
                      className="btn ghost sm"
                      onClick={resetCrop}
                      disabled={!hasImage || (!overlay.proc?.crop && !draftCrop)}
                    >
                      还原裁剪
                    </button>
                  </div>
                  <div className="buddy-line">
                    <span>抠图</span>
                    <span className="buddy-keyline">
                      {overlay.proc?.key ? (
                        <>
                          <i
                            className="buddy-keyswatch"
                            style={{
                              background: `rgb(${overlay.proc.key[0]}, ${overlay.proc.key[1]}, ${overlay.proc.key[2]})`,
                            }}
                          />
                          rgb({overlay.proc.key[0]},{overlay.proc.key[1]},{overlay.proc.key[2]})
                        </>
                      ) : (
                        '未取色'
                      )}
                    </span>
                    <button
                      type="button"
                      className="btn ghost sm"
                      onClick={() => setPicking((v) => !v)}
                      disabled={!hasImage}
                    >
                      {picking ? '取消取色' : '取背景色'}
                    </button>
                  </div>
                  <div className="buddy-slider-line">
                    <div>
                      <span>抠图容差</span>
                      <b>{effectiveProc.tol}</b>
                    </div>
                    <Slider
                      value={effectiveProc.tol}
                      min={4}
                      max={120}
                      step={2}
                      width={150}
                      label="抠图容差"
                      onChange={(v) => updateProc({ tol: v })}
                    />
                  </div>
                  <div className="buddy-slider-line">
                    <div>
                      <span>圆角</span>
                      <b>{effectiveProc.corner}</b>
                    </div>
                    <Slider
                      value={effectiveProc.corner}
                      min={0}
                      max={120}
                      step={2}
                      width={150}
                      label="圆角"
                      onChange={(v) => updateProc({ corner: v })}
                    />
                  </div>
                  <div className="buddy-line">
                    <span>圆角样式</span>
                    <div className="buddy-seg">
                      {(['round', 'star'] as const).map((shape) => (
                        <button
                          key={shape}
                          type="button"
                          className={effectiveProc.cshape === shape ? 'on' : ''}
                          onClick={() => updateProc({ cshape: shape })}
                          disabled={!hasImage}
                        >
                          {shape === 'round' ? '圆角' : '四角星'}
                        </button>
                      ))}
                    </div>
                  </div>
                  <div className="buddy-slider-line">
                    <div>
                      <span>边缘羽化</span>
                      <b>{effectiveProc.feather}</b>
                    </div>
                    <Slider
                      value={effectiveProc.feather}
                      min={0}
                      max={80}
                      step={2}
                      width={150}
                      label="边缘羽化"
                      onChange={(v) => updateProc({ feather: v })}
                    />
                  </div>
                  <div className="buddy-line">
                    <span>水平翻转</span>
                    <Switch
                      on={overlay.flip_h}
                      onChange={(v) => updateOverlay((o) => ({ ...o, flip_h: v }))}
                      title="运行时水平翻转"
                    />
                  </div>
                  {draftCrop ? (
                    <div className="buddy-draft">
                      草稿裁剪：{draftCrop[2]}×{draftCrop[3]} @ {draftCrop[0]},{draftCrop[1]} · 点「应用裁剪」写入
                    </div>
                  ) : null}
                </div>
              </section>

              <section className="buddy-sec">
                <div className="buddy-sec-title">
                  已保存配置
                  <Badge tone="gray">{overlay.presets?.length || 0}</Badge>
                </div>
                {(overlay.presets || []).length ? (
                  <div className="buddy-presets">
                    {(overlay.presets || []).map((p) => {
                      const thumb = typeof p.image === 'string' ? p.image : '';
                      const h = Number(p.base_height) || 240;
                      return (
                        <div
                          key={p.name}
                          className={`buddy-preset${activePreset === p.name ? ' on' : ''}`}
                        >
                          <button
                            type="button"
                            className="buddy-preset-main"
                            onClick={() => void applyPreset(p)}
                            title={`应用配置「${p.name}」`}
                          >
                            <span className="buddy-preset-thumb">
                              {thumb ? <img src={thumb} alt="" /> : <Icon name="image" />}
                            </span>
                            <span className="buddy-preset-meta">
                              <b>{p.name}</b>
                              <em>{h}px · {typeof p.enabled === 'boolean' && p.enabled ? '启用' : '关闭'}</em>
                            </span>
                          </button>
                          <button
                            type="button"
                            className="icon-btn buddy-preset-del"
                            aria-label={`删除配置 ${p.name}`}
                            onClick={() => void deletePreset(p.name)}
                          >
                            <Icon name="close" />
                          </button>
                        </div>
                      );
                    })}
                  </div>
                ) : (
                  <div className="buddy-empty">还没有保存的配置。调好图片与贴法后点「保存为配置…」。</div>
                )}
              </section>

              <div className="buddy-insp-note">
                <Icon name="info" />
                <span>
                  写入字段：
                  <code>
                    {mode === 'ssf'
                      ? 'overrides.overlay = { enabled,image,proc,side,base_height,offset_x,offset_y,gap,flip_h,layer,opacity,v_align,hide_delay_ms,presets }'
                      : 'skin.overlay = { enabled,image,proc,side,base_height,offset_x,offset_y,gap,flip_h,layer,opacity,v_align,hide_delay_ms,presets }'}
                  </code>
                </span>
              </div>
            </div>
          </aside>
        </div>
      </div>
    </>
  );
}

function BuddyWidget({
  hasImage,
  overlay,
  previewRef,
  onPointerDown,
  onPointerMove,
  onPointerUp,
  onResizeDown,
  onResizeMove,
  onResizeUp,
}: {
  hasImage: boolean;
  overlay: OverlayCfg;
  previewRef: RefObject<HTMLCanvasElement | null>;
  onPointerDown: (e: ReactPointerEvent<HTMLDivElement>) => void;
  onPointerMove: (e: ReactPointerEvent<HTMLDivElement>) => void;
  onPointerUp: () => void;
  onResizeDown: (e: ReactPointerEvent<HTMLSpanElement>) => void;
  onResizeMove: (e: ReactPointerEvent<HTMLSpanElement>) => void;
  onResizeUp: () => void;
}) {
  if (!hasImage) return null;
  const margin =
    overlay.side === 'left'
      ? { marginRight: overlay.gap - overlay.offset_x }
      : { marginLeft: overlay.gap + overlay.offset_x };
  return (
    <div
      className="buddy-widget"
      style={{
        ...margin,
        height: overlay.base_height,
        opacity: overlay.opacity / 100,
        zIndex: overlay.layer === 'above' ? 3 : 1,
        transform: `translateY(${overlay.offset_y}px)`,
      }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      title="拖动调整位置；拖右下角圆点改大小"
    >
      <canvas ref={previewRef} />
      <span
        className="buddy-widget-grip"
        aria-hidden="true"
        title="拖动改大小（base_height）"
        onPointerDown={onResizeDown}
        onPointerMove={onResizeMove}
        onPointerUp={onResizeUp}
        onPointerCancel={onResizeUp}
      />
      <span className="buddy-widget-size">{overlay.base_height}px</span>
    </div>
  );
}
