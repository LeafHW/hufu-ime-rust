import { useEffect, useMemo, useRef, useState } from 'react';
import type { Bootstrap, Skin, SkinMeta } from '../types';
import { fallbackSkin } from '../types';
import { deleteSsfSkin, loadSkin, loadFonts, loadSsfSettings, saveSsfSettings, resetSkin, saveSkin, selectSkin as selectSkinApi } from '../api';
import type { SsfOverrides } from '../api';
import { useDirtyGuard } from '../dirtyGuard';
import { Candidate } from '../components/Candidate';
import { Icon } from '../components/Icon';
import { Badge, PageHead, Slider, Switch } from '../components/ui';

const shortName = (n: string) => n.replace(/（.*?）|\(.*?\)/g, '').trim() || n;

const COLOR_LABELS: Record<string, string> = {
  back_color: '窗底色',
  border_color: '边框',
  candidate_back_color: '候选底',
  candidate_text_color: '候选文字',
  comment_text_color: '注释',
  label_color: '序号',
  text_color: '编码',
  hilited_text_color: '编码高亮',
  hilited_candidate_back_color: '高亮底',
  hilited_candidate_text_color: '高亮文字',
  hilited_candidate_label_color: '高亮序号',
  hilited_label_color: '高亮序号',
  hilited_comment_text_color: '高亮注释',
  shadow_color: '阴影',
  preedit_back_color: '编码底',
  hilited_back_color: '高亮背景',
  capsule_back_color: '状态胶囊底',
  capsule_text_color: '状态胶囊字',
  mark_text: '标记',
  dark: '深色',
  light: '浅色',
};

function ColorSwatch({
  value,
  label,
  onChange,
}: {
  value: string;
  label: string;
  onChange?: (v: string) => void;
}) {
  if (!onChange) {
    return (
      <span className="swatch" title={`${label} ${value}`} style={{ background: value }}>
        <span className="sr-only">{label}</span>
      </span>
    );
  }
  return (
    <span className="swatch edit" title={`${label} ${value}（点击修改）`} style={{ background: value }}>
      {/* server 一律存 8 位 #RRGGBBAA（hufu-skin lib.rs:25）；取色器只吃 6 位：
          显示取前 6 位、保存拼回原 alpha（丢 alpha 会让半透明底色变全不透明）。 */}
      <input
        type="color"
        aria-label={label}
        value={/^#[0-9a-fA-F]{8}$/.test(value) ? value.slice(0, 7) : /^#[0-9a-fA-F]{6}$/.test(value) ? value : '#000000'}
        onChange={(e) => onChange(/^#[0-9a-fA-F]{8}$/.test(value) ? e.target.value + value.slice(7) : e.target.value)}
      />
      <span className="sr-only">{label}</span>
    </span>
  );
}

export function SkinStudio({
  data,
  notify,
  refresh,
}: {
  data: Bootstrap;
  notify: (msg: string) => void;
  refresh: () => Promise<Bootstrap>;
}) {
  const [selectedId, setSelectedId] = useState(data.skins.current);
  const [skin, setSkin] = useState<Skin | null>(data.skin);
  const [original, setOriginal] = useState<Skin | null>(data.skin);
  const [ab, setAb] = useState(false);
  const [bg, setBg] = useState('wall');
  const [search, setSearch] = useState('');
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const [anim, setAnim] = useState(data.config.appearance?.anim !== false);
  const [flash, setFlash] = useState(data.config.appearance?.anim_flash !== false);
  /* 逐候选字号/动效速度：乐观值 + 防抖写（/api/config 读-改-写）。 */
  const [pcf, setPcf] = useState(data.config.appearance?.per_cand_font === true);
  const [animPct, setAnimPct] = useState<number | null>(null);
  const [firstPct, setFirstPct] = useState<number | null>(null);
  const [otherPct, setOtherPct] = useState<number | null>(null);
  const cfgDebounce = useRef<ReturnType<typeof setTimeout> | null>(null);
  /* 字体下拉（/api/fonts 本机简中字体表，矢量皮与 SSF 参数共用）。 */
  const [fonts, setFonts] = useState<string[]>([]);
  /* SSF 皮肤调参（overrides=用户覆盖；base=皮肤原值，还原基准）。 */
  const [ssfOv, setSsfOv] = useState<Partial<SsfOverrides>>({});
  const [ssfBase, setSsfBase] = useState<Record<string, unknown>>({});
  const ssfDebounce = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [dirty, setDirty] = useState(false);
  const [importing, setImporting] = useState(false);
  const [creating, setCreating] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [selecting, setSelecting] = useState(false);
  const [cfgBusy, setCfgBusy] = useState(false);
  const cfgBusyRef = useRef(false);
  const cfgPendingRef = useRef<Record<string, unknown> | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const weaselRef = useRef<HTMLInputElement>(null);
  const [weaselBusy, setWeaselBusy] = useState(false);
  const [realPreview] = useState(false);
  const [previewVer, setPreviewVer] = useState(0);
  const [realFailed, setRealFailed] = useState(false);
  const [horizontal, setHorizontal] = useState(false);
  const [trialRaw, setTrialRaw] = useState('tuja');
  const [trialUrl, setTrialUrl] = useState('');
  const [trialBusy, setTrialBusy] = useState(false);
  const trialSeq = useRef(0);
  const dirtyRef = useRef(false);

  useEffect(() => {
    dirtyRef.current = dirty;
  }, [dirty]);

  /* 字体表：挂载拉一次（/api/fonts）。失败静默（下拉退化为手动输入）。 */
  useEffect(() => {
    let alive = true;
    void loadFonts()
      .then((r) => {
        if (alive) setFonts(r.fonts);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  /* OV-B1：脏状态上报全局守卫（切路由/关窗前 App 统一拦截确认）。 */
  useDirtyGuard('skin', '皮肤工作室', dirty);

  useEffect(() => {
    /* 未保存的本地改动不被轮询/当前皮肤切换冲掉。 */
    if (dirtyRef.current) return;
    setSelectedId(data.skins.current);
    setSkin(data.skin);
    setOriginal(data.skin);
    setAb(false);
    setDirty(false);
    setHorizontal(data.skin?.layout?.horizontal === true);
  }, [data.skins.current, data.skin?.id, data.skin?.name]);

  const jsonSkins = useMemo(() => {
    const list = data.skins.skins.filter((s) => s.kind === 'json' && s.name.toLowerCase().includes(search.toLowerCase()));
    return [...list].sort((a, b) => {
      if (a.id === data.skins.current) return -1;
      if (b.id === data.skins.current) return 1;
      return a.name.localeCompare(b.name, 'zh-CN');
    });
  }, [data.skins.skins, data.skins.current, search]);
  const ssfSkins = useMemo(
    () => data.skins.skins.filter((s) => s.kind === 'ssf' && s.name.toLowerCase().includes(search.toLowerCase())),
    [data.skins.skins, search],
  );

  const currentMeta = data.skins.skins.find((s) => s.id === selectedId);
  const realCurrentMeta = data.skins.skins.find((s) => s.id === data.skins.current);
  const isSsf = currentMeta?.kind === 'ssf';
  const shownSkin = ab ? original : skin;
  /* 逐候选字号（预览）：config 真值 + 乐观值合成。 */
  const pcfScales = {
    enabled: pcf,
    first: (firstPct ?? Math.round((data.config.appearance?.first_font_scale ?? 1) * 100)) / 100,
    other: (otherPct ?? Math.round((data.config.appearance?.other_font_scale ?? 0.7) * 100)) / 100,
  };

  /* SSF 皮肤调参：选中 SSF 皮时读取 overrides + 皮肤原值。 */
  useEffect(() => {
    if (!isSsf || !selectedId) return;
    let alive = true;
    void loadSsfSettings(selectedId)
      .then((r) => {
        if (!alive) return;
        setSsfOv(r.overrides || {});
        setSsfBase(r.base || {});
      })
      .catch(() => {
        if (alive) {
          setSsfOv({});
          setSsfBase({});
        }
      });
    return () => {
      alive = false;
    };
  }, [isSsf, selectedId]);

  /* SSF 参数写：本地乐观 + 防抖 500ms POST（server 写盘即 bump skin_ver，
     DLL 立即重拉生效）。 */
  const patchSsf = (patch: Partial<SsfOverrides>, immediate = false) => {
    setSsfOv((prev) => ({ ...prev, ...patch }));
    if (ssfDebounce.current) clearTimeout(ssfDebounce.current);
    const id = selectedId;
    if (!id) return;
    const fire = () => {
      ssfDebounce.current = null;
      setSsfOv((cur) => {
        void saveSsfSettings(id, cur as SsfOverrides)
          .then(() => setPreviewVer((v) => v + 1))
          .catch((e) => notify(`皮肤参数保存失败：${(e as Error).message}`));
        return cur;
      });
    };
    if (immediate) fire();
    else ssfDebounce.current = setTimeout(fire, 500);
  };

  useEffect(() => {
    if (!realPreview || isSsf || !selectedId) {
      setTrialBusy(false);
      return;
    }
    const seq = ++trialSeq.current;
    const timer = window.setTimeout(() => {
      const run = async () => {
        setTrialBusy(true);
        try {
          await fetch('/api/reset', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
          let state: Record<string, unknown> = {};
          for (const ch of trialRaw) {
            const r = await fetch('/api/key', {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ op: 'key', key: ch }),
            });
            const data = (await r.json().catch(() => ({}))) as Record<string, unknown>;
            state = (data.state as Record<string, unknown>) || {};
          }
          if (seq !== trialSeq.current) return;
          const candidates = Array.isArray(state.candidates) ? (state.candidates as Array<Record<string, unknown>>) : [];
          const items = candidates
            .slice(0, 5)
            .map((c) => `${String(c.text ?? '')}|${String(c.comment ?? '')}`)
            .join('\n');
          const q = new URLSearchParams({
            id: selectedId,
            scale: '1',
            raw: String(state.preedit ?? state.raw ?? trialRaw),
            items,
            horizontal: horizontal ? '1' : '0',
            opaque: '1',
            v: String(previewVer),
          });
          setTrialUrl(`/api/skin/preview_frame?${q.toString()}`);
          await fetch('/api/reset', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
        } catch {
          if (seq === trialSeq.current) setTrialUrl('');
        } finally {
          if (seq === trialSeq.current) setTrialBusy(false);
        }
      };
      void run();
    }, 180);
    return () => window.clearTimeout(timer);
  }, [trialRaw, realPreview, isSsf, selectedId, previewVer, horizontal]);


  /* 卡片点选=仅加载进编辑器/预览（P0-1：切换当前皮肤走 makeCurrent → api.selectSkin） */
  const applySkin = async (meta: SkinMeta) => {
    setSelectedId(meta.id);
    setRealFailed(false);
    setPreviewVer((v) => v + 1);
    setHorizontal(meta.kind === 'json' ? data.skin?.layout?.horizontal === true : false);
    setAb(false);
    setDirty(false);
    if (meta.kind === 'ssf') {
      setSkin(null);
      setOriginal(null);
      return;
    }
    setLoading(true);
    try {
      const next = await loadSkin(meta.id);
      setSkin(next);
      setOriginal(next);
    } catch (e) {
      notify(`读取皮肤失败：${(e as Error).message}`);
    } finally {
      setLoading(false);
    }
  };

  /* P0-1：显式「设为当前」——POST /api/skin/select，成功后立即刷新
     当前皮肤状态与列表（不再依赖“保存顺带切皮”的副作用）。 */
  const makeCurrent = async (meta: SkinMeta) => {
    if (selecting) return;
    if (data.skins.current === meta.id && data.skins.current_kind === meta.kind) {
      notify(`「${shortName(meta.name)}」已是当前皮肤`);
      return;
    }
    setSelecting(true);
    try {
      await selectSkinApi(meta.id, meta.kind);
      notify(`已将「${shortName(meta.name)}」设为当前皮肤`);
      await refresh();
    } catch (e) {
      notify(`切换失败：${(e as Error).message}`);
    } finally {
      setSelecting(false);
    }
  };

  /* 假状态清理③：候选窗动效/选重暂留接真实配置（读-改-写 /api/config）。
     写请求在途时的新写不丢弃，合并进 pending 队列，完成后重放
     （快速连切开关曾出现第二个静默丢失、UI 与服务器脱钩）。 */
  const writeAppearance = async (patch: Record<string, unknown>): Promise<void> => {
    if (cfgBusyRef.current) {
      cfgPendingRef.current = { ...(cfgPendingRef.current || {}), ...patch };
      return;
    }
    cfgBusyRef.current = true;
    setCfgBusy(true);
    try {
      const r = await fetch('/api/config', { cache: 'no-store' });
      if (!r.ok) throw new Error(`读取配置失败（${r.status}）`);
      const fresh = (await r.json()) as Record<string, unknown>;
      const appearance = { ...(fresh.appearance as Record<string, unknown> | undefined), ...patch };
      const p = await fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ ...fresh, appearance }),
      });
      const body = (await p.json().catch(() => ({}))) as { error?: string };
      if (!p.ok) throw new Error(body.error || `保存失败（${p.status}）`);
      await refresh();
    } finally {
      cfgBusyRef.current = false;
      setCfgBusy(false);
      const pending = cfgPendingRef.current;
      cfgPendingRef.current = null;
      if (pending) {
        void writeAppearance(pending).catch((e) => notify(`保存失败：${(e as Error).message}`));
      }
    }
  };

  /* 滑杆防抖写：拖动过程中只在停手 400ms 后写一次（writeAppearance
     本身是读-改-写，避免拖动逐帧打 /api/config）。 */
  const scheduleCfg = (patch: Record<string, unknown>) => {
    if (cfgDebounce.current) clearTimeout(cfgDebounce.current);
    cfgDebounce.current = setTimeout(() => {
      void writeAppearance(patch).catch((e) => {
        setAnimPct(null);
        setFirstPct(null);
        setOtherPct(null);
        setPcf(data.config.appearance?.per_cand_font === true);
        notify(`保存失败：${(e as Error).message}`);
      });
    }, 400);
  };

  const patchLayout = (key: string, value: number | string) => {
    setSkin((s) => (s ? { ...s, layout: { ...s.layout, [key]: value } } : s));
    setDirty(true);
  };
  const patchMaterial = (key: string, value: number | boolean) => {
    setSkin((s) => (s ? { ...s, material: { ...s.material, [key]: value } } : s));
    setDirty(true);
  };
  const patchColors = (key: string, value: string) => {
    setSkin((s) => (s ? { ...s, colors: { ...s.colors, [key]: value } } : s));
    setDirty(true);
  };

  /* AP-04：保存前先 GET 服务器最新，只合并本页管理的字段再回写，
     防止用页面加载时的旧快照清空挂件 overlay / 打回滚轮调的字号。
     保存不再切皮（select:false）。 */
  const onSave = async () => {
    if (!skin || saving) return;
    const id = skin.id || selectedId;
    setSaving(true);
    try {
      const fresh = await loadSkin(id); // A-2：校验返回 id 与请求一致
      const merged: Skin = {
        ...fresh,
        colors: {
          ...fresh.colors,
          back_color: skin.colors?.back_color ?? fresh.colors?.back_color,
          hilited_candidate_back_color:
            skin.colors?.hilited_candidate_back_color ?? fresh.colors?.hilited_candidate_back_color,
        },
        material: {
          ...fresh.material,
          master_alpha: skin.material.master_alpha ?? fresh.material.master_alpha,
          hilite_alpha: skin.material.hilite_alpha ?? fresh.material.hilite_alpha,
          shadow_alpha: skin.material.shadow_alpha ?? fresh.material.shadow_alpha,
          hilite_on: skin.material.hilite_on ?? fresh.material.hilite_on,
        },
        layout: {
          ...fresh.layout,
          font_point: skin.layout.font_point ?? fresh.layout.font_point,
          font_face: skin.layout.font_face ?? fresh.layout.font_face,
          corner_radius: skin.layout.corner_radius ?? fresh.layout.corner_radius,
          candidate_spacing: skin.layout.candidate_spacing ?? fresh.layout.candidate_spacing,
          horizontal: skin.layout.horizontal ?? fresh.layout.horizontal,
        },
      };
      await saveSkin(merged, { select: false });
      setSkin(merged);
      setOriginal(merged);
      setAb(false);
      setDirty(false);
      setRealFailed(false);
      setPreviewVer((v) => v + 1);
      notify(`已保存皮肤「${shortName(merged.name)}」`);
    } catch (e) {
      notify(`保存失败：${(e as Error).message}`);
    } finally {
      setSaving(false);
    }
  };

  /* 自动保存（对齐旧版"改动即存"习惯）：dirty 且非 A/B 对比时
     防抖 600ms 调 onSave；拖动中 skin 连续变化不断重置计时，停手即存。 */
  const onSaveRef = useRef(onSave);
  onSaveRef.current = onSave;
  useEffect(() => {
    if (!dirty || !skin || saving || ab) return;
    const t = setTimeout(() => void onSaveRef.current(), 600);
    return () => clearTimeout(t);
  }, [dirty, skin, saving, ab]);

  /* 假状态清理⑤：恢复出厂（官方矢量皮，POST /api/skin/reset）。 */
  const onReset = async () => {
    if (!skin || resetting) return;
    if (!window.confirm(`将皮肤「${shortName(skin.name)}」恢复出厂？该皮肤的全部改动（含挂件副本）会被覆盖。`)) return;
    setResetting(true);
    try {
      const s = await resetSkin(skin.id || selectedId);
      setSkin(s);
      setOriginal(s);
      setAb(false);
      setDirty(false);
      setHorizontal(s.layout?.horizontal === true);
      setPreviewVer((v) => v + 1);
      notify(`「${shortName(s.name)}」已恢复出厂`);
      await refresh();
    } catch (e) {
      notify(`恢复失败：${(e as Error).message}`);
    } finally {
      setResetting(false);
    }
  };

  /* 假状态清理⑤：删除 .ssf（DELETE /api/skin?kind=ssf），会清理缓存与挂件副本。 */
  const onDeleteSsf = async () => {
    const meta = data.skins.skins.find((s) => s.id === selectedId);
    if (!meta || meta.kind !== 'ssf' || deleting) return;
    if (!window.confirm(`删除位图皮肤「${shortName(meta.name)}」？会同时清理该皮肤的缓存与挂件副本；若它是当前皮肤将回退默认皮肤。`)) return;
    setDeleting(true);
    try {
      await deleteSsfSkin(meta.id);
      notify(`已删除「${shortName(meta.name)}」`);
      const next = await refresh();
      if (selectedId === meta.id) {
        setSelectedId(next.skins.current);
        setSkin(next.skin);
        setOriginal(next.skin);
        setDirty(false);
        setHorizontal(next.skin?.layout?.horizontal === true);
      }
    } catch (e) {
      notify(`删除失败：${(e as Error).message}`);
    } finally {
      setDeleting(false);
    }
  };

  const importSsf = async (file: File | undefined) => {
    if (!file || importing) return;
    setImporting(true);
    try {
      const buf = await file.arrayBuffer();
      const r = await fetch(`/api/skin/import_ssf?name=${encodeURIComponent(file.name)}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/octet-stream' },
        body: buf,
      });
      const j = (await r.json().catch(() => ({}))) as { error?: string; id?: string; warnings?: string[] };
      if (!r.ok) throw new Error(j.error || `HTTP ${r.status}`);
      const warn = j.warnings?.length ? `（${j.warnings.join('；')}）` : '';
      await refresh();
      notify(`已导入 .ssf 皮肤：${j.id || file.name}${warn}`);
    } catch (e) {
      notify(`导入失败：${(e as Error).message}`);
    } finally {
      setImporting(false);
      if (fileRef.current) fileRef.current.value = '';
    }
  };

  const importWeasel = async (file: File | undefined) => {
    if (!file || weaselBusy) return;
    setWeaselBusy(true);
    try {
      const raw = JSON.parse(await file.text()) as Record<string, unknown>;
      const schemes = raw.preset_color_schemes;
      let colors: Record<string, unknown> = raw;
      let id = file.name.replace(/\.json$/i, '');
      if (schemes && typeof schemes === 'object') {
        const entries = Object.entries(schemes as Record<string, unknown>);
        if (!entries.length) throw new Error('文件里没有配色方案');
        const [key, value] = entries[0];
        colors = value as Record<string, unknown>;
        id = key;
      }
      const r = await fetch('/api/weasel_import', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ id, colors }),
      });
      const skin = (await r.json().catch(() => ({}))) as Skin & { error?: string };
      if (!r.ok) throw new Error(skin.error || `HTTP ${r.status}`);
      await saveSkin(skin, { select: false }); // 导入≠设为当前
      await refresh();
      notify(`已导入 weasel 配色「${skin.name || id}」`);
    } catch (e) {
      notify(`weasel 导入失败：${(e as Error).message}`);
    } finally {
      setWeaselBusy(false);
      if (weaselRef.current) weaselRef.current.value = '';
    }
  };

  const createSkin = async () => {
    if (!skin || creating) {
      notify('请先选择一个矢量皮肤作为模板');
      return;
    }
    setCreating(true);
    try {
      const clone = JSON.parse(JSON.stringify(skin)) as Skin;
      const now = new Date();
      const hh = String(now.getHours()).padStart(2, '0');
      const mm = String(now.getMinutes()).padStart(2, '0');
      clone.id = `custom-${Date.now()}`;
      clone.name = `新皮肤 ${hh}:${mm}`;
      await saveSkin(clone, { select: false }); // 新建≠设为当前
      setSelectedId(clone.id);
      setSkin(clone);
      setOriginal(clone);
      setDirty(false);
      setHorizontal(clone.layout?.horizontal === true);
      await refresh();
      notify(`已创建「${clone.name}」`);
    } catch (e) {
      notify(`新建皮肤失败：${(e as Error).message}`);
    } finally {
      setCreating(false);
    }
  };

  const highlight = shownSkin?.colors?.hilited_candidate_back_color || '#E7B75C';

  return (
    <>
      <PageHead
        title="皮肤工作室"
        sub={
          <>
            矢量皮肤 · {jsonSkins.length} 款<span style={{ color: 'var(--text3)' }}>·</span>
            当前 {shortName(realCurrentMeta?.name || '—')}
          </>
        }
        actions={
          <>
            <button type="button" className="btn outline" disabled={importing} onClick={() => fileRef.current?.click()}>
              <Icon name="upload" />
              {importing ? '导入中…' : '导入 .ssf'}
            </button>
            <button type="button" className="btn primary" disabled={creating || !skin} onClick={() => void createSkin()}>
              <Icon name="plus" />
              {creating ? '创建中…' : '新建皮肤'}
            </button>
          </>
        }
      />

      <input
        ref={fileRef}
        type="file"
        accept=".ssf,application/zip"
        style={{ display: 'none' }}
        onChange={(e) => void importSsf(e.target.files?.[0])}
      />
      <input
        ref={weaselRef}
        type="file"
        accept=".json,application/json"
        style={{ display: 'none' }}
        onChange={(e) => void importWeasel(e.target.files?.[0])}
      />

      <div className={`studio${inspectorOpen ? ' inspector-open' : ''}`}>
        <div className="card skin-list">
          <div className="search">
            <Icon name="search" />
            <input
              aria-label="搜索皮肤"
              placeholder="搜索皮肤…"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </div>
          <div className="list-label">
            <span>矢量皮肤</span>
            <span>{jsonSkins.length}</span>
          </div>
          {jsonSkins.map((meta, i) => {
            const active = selectedId === meta.id;
            const current = data.skins.current === meta.id;
            return (
              <div
                key={meta.id}
                className={`skin-item${active ? ' on' : ''}`}
                onClick={() => applySkin(meta)}
              >
                <div className={`skin-thumb${i % 3 === 1 ? ' t2' : i % 3 === 2 ? ' t3' : ''}`} />
                <div className="skin-meta">
                  <b>{shortName(meta.name)}</b>
                  <span>{current ? '当前 · ' : ''}矢量皮肤</span>
                </div>
                {current ? (
                  <Badge tone="green">当前</Badge>
                ) : (
                  <button
                    type="button"
                    className="btn outline sm"
                    style={{ padding: '2px 8px', fontSize: 11, whiteSpace: 'nowrap' }}
                    disabled={selecting}
                    title={`将「${shortName(meta.name)}」设为当前皮肤`}
                    onClick={(e) => {
                      e.stopPropagation();
                      void makeCurrent(meta);
                    }}
                  >
                    设为当前
                  </button>
                )}
                {active ? <Icon name="check" className="icon check" /> : null}
              </div>
            );
          })}
          <div className="list-label" style={{ marginTop: 6 }}>
            <span>位图皮肤 (.ssf)</span>
            <span>{ssfSkins.length}</span>
          </div>
          {ssfSkins.map((meta) => {
            const active = selectedId === meta.id;
            const current = data.skins.current === meta.id && data.skins.current_kind === 'ssf';
            return (
              <div
                key={meta.id}
                className={`skin-item${active ? ' on' : ''}`}
                onClick={() => applySkin(meta)}
              >
                <div className="skin-thumb ssf" />
                <div className="skin-meta">
                  <b>{shortName(meta.name)}</b>
                  <span>{current ? '当前 · ' : ''}位图 · {meta.author || '已导入'}</span>
                </div>
                {current ? (
                  <Badge tone="green">当前</Badge>
                ) : (
                  <button
                    type="button"
                    className="btn outline sm"
                    style={{ padding: '2px 8px', fontSize: 11, whiteSpace: 'nowrap' }}
                    disabled={selecting}
                    title={`将「${shortName(meta.name)}」设为当前皮肤`}
                    onClick={(e) => {
                      e.stopPropagation();
                      void makeCurrent(meta);
                    }}
                  >
                    设为当前
                  </button>
                )}
                {active ? <Icon name="check" className="icon check" /> : null}
              </div>
            );
          })}
          <div className="import-tile" role="button" tabIndex={0} onClick={() => fileRef.current?.click()} onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); fileRef.current?.click(); } }}>
            <Icon name="plus" />
            <span>
              <b>导入 .ssf 皮肤</b>
              <i>点击选择 .ssf 文件</i>
            </span>
          </div>
          <div className="list-note">皮肤文件由你自备，只在本机导入；删除 .ssf 会清理其缓存与挂件副本，矢量皮肤可恢复出厂。</div>
        </div>

        <div className="card stage-card">
          <div className="stage-toolbar">
            <div className="seg">
              {(
                [
                  ['board', '棋盘'],
                  ['dark', '深色'],
                  ['light', '浅色'],
                  ['wall', '壁纸'],
                ] as const
              ).map(([key, label]) => (
                <button
                  key={key}
                  type="button"
                  className={`sg${bg === key ? ' on' : ''}`}
                  onClick={() => setBg(key)}
                >
                  {label}
                </button>
              ))}
            </div>
            <div className="spacer" />
            <div className="tool">100%</div>
            <div className="tool">
              横排
              <Switch
                on={horizontal}
                title="横排候选（立即保存该皮肤的排列方向）"
                onChange={(v) => {
                  if (saving) return;
                  /* SSF 皮：横排走 overrides（/api/skin/ssf-settings），不是 json 皮文件。 */
                  if (isSsf) {
                    setHorizontal(v);
                    patchSsf({ horizontal: v }, true);
                    return;
                  }
                  if (!skin) return;
                  const id = skin.id || selectedId;
                  setHorizontal(v);
                  setSaving(true);
                  /* AP-02：不再全量回写页面旧快照——先 GET 服务器最新，
                     只改横排字段再 POST（select:false），未保存的本地改动
                     不被顺带提交；成功后同步更新 A/B 基准。 */
                  void (async () => {
                    try {
                      const fresh = await loadSkin(id);
                      const next: Skin = { ...fresh, layout: { ...fresh.layout, horizontal: v } };
                      await saveSkin(next, { select: false });
                      setOriginal((prev) =>
                        prev ? { ...prev, layout: { ...prev.layout, horizontal: v } } : next,
                      );
                      if (!dirtyRef.current) {
                        setSkin(next);
                        setDirty(false);
                      } else {
                        setSkin((s) => (s ? { ...s, layout: { ...s.layout, horizontal: v } } : next));
                      }
                      setPreviewVer((n) => n + 1);
                    } catch (err) {
                      setHorizontal(!v);
                      notify(`横排保存失败：${(err as Error).message}`);
                    } finally {
                      setSaving(false);
                    }
                  })();
                }}
              />
            </div>
            <button
              type="button"
              className={`tool${ab ? ' ab-on' : ''}`}
              onClick={() => setAb((v) => !v)}
              disabled={!skin}
            >
              {ab ? '回到编辑' : 'A/B'}
            </button>
            <button
              type="button"
              className="btn outline sm params-toggle"
              onClick={() => setInspectorOpen((v) => !v)}
            >
              参数
            </button>
          </div>

          <div className={`stage bg-${bg}`}>
            <div className="band" />
            {horizontal ? (
              <div className="cand-wrap">
                <Candidate skin={shownSkin} size="lg" ab={ab} horizontal fontScales={pcfScales} />
              </div>
            ) : isSsf ? (
              <div className="ssf-wrap">
                <img
                  className="skin-ssf-preview"
                  src={`/api/skin/preview?id=${encodeURIComponent(selectedId)}&kind=ssf&v=${previewVer}`}
                  alt="位图皮肤预览"
                  onError={(e) => {
                    (e.currentTarget as HTMLImageElement).style.display = 'none';
                  }}
                />
              </div>
            ) : loading ? (
              <div className="skin-empty">读取皮肤中…</div>
            ) : (
              <div className="cand-wrap">
                <Candidate skin={shownSkin} size="lg" ab={ab} fontScales={pcfScales} />
              </div>
            )}
          </div>

          <div className="stage-cap">
            <span>
              <b>{shortName(shownSkin?.name || currentMeta?.name || '—')}</b> · {isSsf ? '位图皮肤' : '矢量皮肤'} · {horizontal ? '横排预览' : '预览'}
            </span>
            <span className="sync-state">
              <span className={saving ? 'dot gold' : 'dot'} />
              {saving ? '保存中…' : ab ? 'A/B：已保存版本' : skin ? (dirty ? '本地预览 · 未保存改动' : '已保存') : '已保存'}
            </span>
          </div>
        </div>

        <div className="card inspector">
          <div className="insp-head">
            <div className="insp-name">
              <b>{shortName(shownSkin?.name || currentMeta?.name || '皮肤')}</b>
              <Badge tone={dirty ? 'gold' : 'green'}>{saving ? '保存中' : dirty ? '未保存' : '已保存'}</Badge>
            </div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
              <button
                type="button"
                className="icon-btn params-close"
                aria-label="关闭参数面板"
                onClick={() => setInspectorOpen(false)}
              >
                <Icon name="close" />
              </button>
              <button type="button" className="btn goldline sm" disabled={!skin || saving} onClick={onSave}>
                保存
              </button>
              {!isSsf && skin ? (
                <button
                  type="button"
                  className="btn ghost sm"
                  disabled={saving || resetting}
                  title="按官方出厂配置覆盖该皮肤"
                  onClick={() => void onReset()}
                >
                  {resetting ? '恢复中…' : '恢复出厂'}
                </button>
              ) : null}
              {isSsf ? (
                <button
                  type="button"
                  className="btn danger sm"
                  disabled={saving || deleting}
                  title="删除该 .ssf 文件与缓存、挂件副本"
                  onClick={() => void onDeleteSsf()}
                >
                  {deleting ? '删除中…' : '删除'}
                </button>
              ) : null}
            </div>
          </div>

          <div className="insp-body">
            <div className="insp-sec">
              <div className="insp-sec-h">
                <Icon name="chev-down" className="icon chev" />
                全局 · 候选窗
              </div>
              <div className="row-line"><span className="lab">候选窗动效</span><Switch on={anim} title="appearance.anim" onChange={(v) => { setAnim(v); void writeAppearance({ anim: v }).catch((e) => { setAnim(!v); notify(`保存失败：${(e as Error).message}`); }); }} /></div>
              <div className="row-line"><span className="lab">选重暂留</span><Switch on={flash} title="appearance.anim_flash" onChange={(v) => { setFlash(v); void writeAppearance({ anim_flash: v }).catch((e) => { setFlash(!v); notify(`保存失败：${(e as Error).message}`); }); }} /></div>
              <div className="row-line">
                <span className="lab">动效速度</span>
                <Slider
                  value={animPct ?? Math.round((data.config.appearance?.anim_speed ?? 1) * 100)}
                  min={0}
                  max={200}
                  step={5}
                  width={104}
                  label="动效速度"
                  onChange={(v) => {
                    setAnimPct(v);
                    scheduleCfg({ anim_speed: v / 100 });
                  }}
                />
                <span className="val">{(animPct ?? Math.round((data.config.appearance?.anim_speed ?? 1) * 100))}%</span>
              </div>
              <div className="row-line">
                <span className="lab" title="开：第 1 候选按其比例放大、其余候选按其比例缩小；关：全页同一字号（默认）">首选放大 / 其余缩小</span>
                <Switch
                  on={pcf}
                  title="appearance.per_cand_font"
                  onChange={(v) => {
                    setPcf(v);
                    void writeAppearance({ per_cand_font: v }).catch((e) => {
                      setPcf(!v);
                      notify(`保存失败：${(e as Error).message}`);
                    });
                  }}
                />
              </div>
              <div className="row-line">
                <span className="lab">　首选比例</span>
                <Slider
                  disabled={!pcf}
                  value={firstPct ?? Math.round((data.config.appearance?.first_font_scale ?? 1) * 100)}
                  min={50}
                  max={200}
                  step={5}
                  width={104}
                  label="首选字号比例"
                  onChange={(v) => {
                    setFirstPct(v);
                    scheduleCfg({ first_font_scale: v / 100 });
                  }}
                />
                <span className="val">{(firstPct ?? Math.round((data.config.appearance?.first_font_scale ?? 1) * 100))}%</span>
              </div>
              <div className="row-line">
                <span className="lab">　其余比例</span>
                <Slider
                  disabled={!pcf}
                  value={otherPct ?? Math.round((data.config.appearance?.other_font_scale ?? 0.7) * 100)}
                  min={50}
                  max={100}
                  step={5}
                  width={104}
                  label="其余字号比例"
                  onChange={(v) => {
                    setOtherPct(v);
                    scheduleCfg({ other_font_scale: v / 100 });
                  }}
                />
                <span className="val">{(otherPct ?? Math.round((data.config.appearance?.other_font_scale ?? 0.7) * 100))}%</span>
              </div>
            </div>

            {!isSsf && (
            <>
            <div className="insp-sec">
              <div className="insp-sec-h">
                <Icon name="chev-down" className="icon chev" />
                材质
              </div>
              <div className="row-line">
                <span className="lab">窗底色</span>
                <div style={{ display: 'flex', alignItems: 'center', gap: 7 }}>
                  <ColorSwatch
                    value={skin?.colors?.back_color || '#1B1D22'}
                    label="窗底色"
                    onChange={(v) => patchColors('back_color', v)}
                  />
                  <span className="val">{(skin?.colors?.back_color || '#1B1D22').slice(0, 7)}</span>
                </div>
              </div>
              <div className="row-line">
                <span className="lab">整体透明度</span>
                <Slider
                  value={Math.round((skin?.material?.master_alpha ?? 0.94) * 100)}
                  onChange={(v) => patchMaterial('master_alpha', v / 100)}
                  label="整体透明度"
                />
              </div>
              <div className="row-line">
                <span className="lab" title="开：首选候选有高亮底+高亮文字色；关：完全不画高亮，所有候选统一普通样式（默认开）">高亮开关</span>
                <Switch
                  on={skin?.material?.hilite_on !== false}
                  title="skin.material.hilite_on"
                  onChange={(v) => patchMaterial('hilite_on', v)}
                />
              </div>
              <div className="row-line">
                <span className="lab">高亮色</span>
                <div style={{ display: 'flex', alignItems: 'center', gap: 7 }}>
                  <ColorSwatch
                    value={highlight}
                    label="高亮色"
                    onChange={(v) => patchColors('hilited_candidate_back_color', v)}
                  />
                  <span className="val">{highlight.slice(0, 7)}</span>
                </div>
              </div>
              <div className="row-line">
                <span className="lab">高亮透明度</span>
                <Slider
                  value={Math.round((skin?.material?.hilite_alpha ?? 1) * 100)}
                  onChange={(v) => patchMaterial('hilite_alpha', v / 100)}
                  label="高亮透明度"
                />
              </div>
              <div className="row-line">
                <span className="lab">阴影</span>
                <Slider
                  value={Math.round((skin?.material?.shadow_alpha ?? 0.55) * 100)}
                  onChange={(v) => patchMaterial('shadow_alpha', v / 100)}
                  label="阴影"
                />
              </div>
            </div>

            {/* 【颜色补全 2026-11-01】自有皮肤此前只有窗底色/高亮色两个
                取色器，其余颜色字段（边框/候选文字/注释/序号/编码色等）
                无 UI 可调。此处按 COLOR_LABELS 全量补齐；窗底色与高亮底
                已在上方「材质」区（带透明度联动），不重复列出。 */}
            <div className="insp-sec">
              <div className="insp-sec-h">
                <Icon name="chev-down" className="icon chev" />
                颜色
                <Badge>全部可调</Badge>
              </div>
              {([
                'border_color',
                'candidate_back_color',
                'preedit_back_color',
                'text_color',
                'hilited_text_color',
                'candidate_text_color',
                'hilited_candidate_text_color',
                'label_color',
                'hilited_label_color',
                'hilited_candidate_label_color',
                'comment_text_color',
                'hilited_comment_text_color',
                'hilited_back_color',
                'shadow_color',
                'capsule_back_color',
                'capsule_text_color',
              ] as const).map((key) => {
                const cur =
                  (skin?.colors?.[key] as string | undefined) ||
                  (fallbackSkin.colors as unknown as Record<string, string>)[key] ||
                  '#00000000';
                return (
                  <div className="row-line" key={key}>
                    <span className="lab" title={`skin.colors.${key}`}>{COLOR_LABELS[key] || key}</span>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 7 }}>
                      <ColorSwatch value={cur} label={COLOR_LABELS[key] || key} onChange={(v) => patchColors(key, v)} />
                      <span className="val">{cur.slice(0, 9)}</span>
                    </div>
                  </div>
                );
              })}
            </div>

            <div className="insp-sec">
              <div className="insp-sec-h">
                <Icon name="chev-down" className="icon chev" />
                布局
              </div>
              <div className="row-line">
                <span className="lab">候选字号</span>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                  <Slider
                    value={Number(skin?.layout?.font_point) || 16}
                    min={10}
                    max={36}
                    step={0.5}
                    width={70}
                    onChange={(v) => patchLayout('font_point', v)}
                    label="候选字号"
                  />
                  <span className="val">{Number(skin?.layout?.font_point) || 16} pt</span>
                </div>
              </div>
              <div className="row-line">
                <span className="lab">候选字体</span>
                <select
                  className="field-input"
                  style={{ width: 172, height: 26, flex: 'none' }}
                  value={skin?.layout?.font_face || ''}
                  onChange={(e) => patchLayout('font_face', e.target.value)}
                  title="本机已安装的简体中文字体；（自动）=名字带「霞鹜」优先，回退微软雅黑"
                >
                  <option value="">（自动）</option>
                  {fonts.map((f) => (
                    <option key={f} value={f}>
                      {f}
                    </option>
                  ))}
                  {skin?.layout?.font_face && !fonts.includes(skin.layout.font_face) ? (
                    <option value={skin.layout.font_face}>{skin.layout.font_face}</option>
                  ) : null}
                </select>
              </div>
              <div className="row-line">
                <span className="lab">圆角</span>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                  <Slider
                    value={Number(skin?.layout?.corner_radius) || 10}
                    min={0}
                    max={24}
                    step={1}
                    width={70}
                    onChange={(v) => patchLayout('corner_radius', v)}
                    label="圆角"
                  />
                  <span className="val">{Number(skin?.layout?.corner_radius) || 10} px</span>
                </div>
              </div>
              <div className="row-line">
                <span className="lab">候选间距</span>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                  <Slider
                    value={Number(skin?.layout?.candidate_spacing) || 6}
                    min={0}
                    max={24}
                    step={1}
                    width={70}
                    onChange={(v) => patchLayout('candidate_spacing', v)}
                    label="候选间距"
                  />
                  <span className="val">{Number(skin?.layout?.candidate_spacing) || 6} px</span>
                </div>
              </div>
            </div>
            </>
            )}

            {isSsf && (
            <>
            <div className="insp-sec">
              <div className="insp-sec-h">
                <Icon name="chev-down" className="icon chev" />
                显示设置
                <Badge>SSF</Badge>
              </div>
              <div className="row-line">
                <span className="lab">字号</span>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                  <Slider
                    value={Number(ssfOv.font_pt ?? (ssfBase.font_pt as number | undefined)) || 16}
                    min={10}
                    max={36}
                    step={0.5}
                    width={70}
                    onChange={(v) => patchSsf({ font_pt: v })}
                    label="字号"
                  />
                  <span className="val">{Number(ssfOv.font_pt ?? (ssfBase.font_pt as number | undefined)) || 16} pt</span>
                </div>
              </div>
              <div className="row-line">
                <span className="lab">字体</span>
                <select
                  className="field-input"
                  style={{ width: 172, height: 26, flex: 'none' }}
                  value={(ssfOv.font_face as string | undefined) ?? ((ssfBase.font_face as string | undefined) || '')}
                  onChange={(e) => patchSsf({ font_face: e.target.value || null }, true)}
                  title="本机已安装的简体中文字体；（自动）=霞鹜优先，回退微软雅黑"
                >
                  <option value="">（自动）</option>
                  {fonts.map((f) => (
                    <option key={f} value={f}>
                      {f}
                    </option>
                  ))}
                </select>
              </div>
              <div className="row-line">
                <span className="lab" title="候选词前显示序号；关=跟随全局设置">显示序号</span>
                <Switch
                  on={ssfOv.show_index === true}
                  title="独立皮肤专属：开=强制显示序号，关=跟随全局候选设置"
                  onChange={(v) => patchSsf({ show_index: v ? true : null }, true)}
                />
              </div>
              <div className="row-line">
                <span className="lab" title="候选窗与光标行的垂直距离：正=离光标更远（窗下移），负=贴近光标（可压进光标行）；0=默认间距">候选高度</span>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                  <Slider
                    value={Math.round(Number(ssfOv.cand_y ?? 0))}
                    min={-100}
                    max={100}
                    step={1}
                    width={104}
                    onChange={(v) => patchSsf({ cand_y: v === 0 ? null : v })}
                    label="候选高度"
                  />
                  <span className="val">{Math.round(Number(ssfOv.cand_y ?? 0)) || 0} px</span>
                </div>
              </div>
            </div>
            <div className="insp-sec">
              <div className="insp-sec-h">
                <Icon name="chev-down" className="icon chev" />
                文字颜色
                <Badge>覆盖皮肤原色</Badge>
              </div>
              {([
                ['code_color', '编码', '编码行文字色'],
                ['first_color', '首选', '第一候选文字色（皮肤的高亮表达）'],
                ['text_color', '候选', '候选文字色'],
                ['annotation_color', '注释', '注释/拼音色'],
              ] as const).map(([key, lab, tip]) => {
                const cur =
                  (ssfOv[key] as string | undefined) || (ssfBase[key] as string | undefined) || '#CCCCCC';
                return (
                  <div className="row-line" key={key}>
                    <span className="lab" title={tip}>{lab}</span>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 7 }}>
                      <ColorSwatch value={cur} label={lab} onChange={(v) => patchSsf({ [key]: v } as Partial<SsfOverrides>)} />
                      <span className="val">{cur.slice(0, 7)}</span>
                    </div>
                  </div>
                );
              })}
              <button
                type="button"
                className="btn outline sm"
                onClick={() =>
                  patchSsf(
                    {
                      font_pt: null,
                      font_face: null,
                      show_index: null,
                      cand_y: null,
                      code_color: null,
                      first_color: null,
                      text_color: null,
                      annotation_color: null,
                    },
                    true,
                  )
                }
                title="字号/字体/序号/候选高度/颜色全部还原为皮肤原值"
              >
                还原皮肤原值
              </button>
            </div>
            </>
            )}

            <div className="insp-sec">
              <div className="insp-sec-h">
                <Icon name="chev-down" className="icon chev" />
                颜色角色
                <Badge>{Object.keys(shownSkin?.colors || {}).length || 21}</Badge>
              </div>
              <div className="color-grid">
                {Object.entries(shownSkin?.colors || {}).map(([key, value]) => (
                  <ColorSwatch key={key} value={value} label={COLOR_LABELS[key] || key} />
                ))}
              </div>
              <button type="button" className="link-more" disabled={weaselBusy} onClick={() => weaselRef.current?.click()}>
                {weaselBusy ? '导入中…' : 'weasel 配色互导 →'}
              </button>
            </div>
          </div>
        </div>
      </div>
    </>
  );
}
