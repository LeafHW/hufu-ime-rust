import { useEffect, useMemo, useRef, useState } from 'react';
import type { Bootstrap, Skin, SkinMeta } from '../types';
import { fallbackSkin } from '../types';
import { deleteSsfSkin, fetchWithTimeout, loadSkin, loadFonts, loadSsfSettings, saveSsfSettings, resetSkin, saveSkin, selectSkin as selectSkinApi, updateConfig } from '../api';
import type { SsfOverrides } from '../api';
import { useDirtyGuard } from '../dirtyGuard';
import { Candidate } from '../components/Candidate';
import { useBodyLight } from '../hooks';
import { skinSwatch } from '../skinSwatch';
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
      <span className="swatch" title={`${label} ${value}`}>
        {/* 【棋盘底纹 2026-11-01】半透明/全透明色（alpha<FF）直接铺色会
            视觉消失，与实色块并排像「大小不一」——棋盘底+覆盖色层透出。 */}
        <span className="sw-fill" style={{ background: value }}>
          <span className="sr-only">{label}</span>
        </span>
      </span>
    );
  }
  return (
    <span className="swatch edit" title={`${label} ${value}（点击修改）`}>
      <span className="sw-fill" style={{ background: value }} />
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
  navigate,
}: {
  data: Bootstrap;
  notify: (msg: string) => void;
  refresh: () => Promise<Bootstrap>;
  /** 跳到「动效」页（动效开关已统一收在那里）。 */
  navigate: (r: import('../types').Route) => void;
}) {
  const [selectedId, setSelectedId] = useState(data.skins.current);
  const [skin, setSkin] = useState<Skin | null>(data.skin);
  const [original, setOriginal] = useState<Skin | null>(data.skin);
  const [ab, setAb] = useState(false);
  /* 【2026-11-03】预览背景去掉「棋盘」，默认跟主题；用户手动选过记住本次会话。 */
  const [bgChoice, setBgChoice] = useState<'dark' | 'light' | 'wall' | null>(null);
  const lightTheme = useBodyLight();
  const bg = bgChoice ?? (lightTheme ? 'light' : 'dark');
  const [search, setSearch] = useState('');
  /* 【皮肤列表分页签 2026-11-01】矢量/位图分组切换；初始跟当前皮肤类型。 */
  const [listTab, setListTab] = useState<'json' | 'ssf'>(data.skins.current_kind === 'ssf' ? 'ssf' : 'json');
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [inspectorOpen, setInspectorOpen] = useState(false);
  /* 逐候选字号：乐观值 + 防抖写（/api/config 读-改-写）。
     动效开关/速度/动效预览已迁到「动效」页，这里只留皮肤相关的字号强调。 */
  const [pcf, setPcf] = useState(data.config.appearance?.per_cand_font === true);
  /* 【2026-11-03】序号开关是全局 candidates.show_index（SSF 另有 per-skin 覆盖）。 */
  const [showIndex, setShowIndex] = useState(data.config.candidates?.show_index !== false);
  const [firstPct, setFirstPct] = useState<number | null>(null);
  const [otherPct, setOtherPct] = useState<number | null>(null);
  const cfgDebounce = useRef<ReturnType<typeof setTimeout> | null>(null);
  /* 字体下拉（/api/fonts 本机简中字体表，矢量皮与 SSF 参数共用）。 */
  const [fonts, setFonts] = useState<string[]>([]);
  /* SSF 皮肤调参（overrides=用户覆盖；base=皮肤原值，还原基准）。 */
  const [ssfOv, setSsfOv] = useState<Partial<SsfOverrides>>({});
  const [ssfBase, setSsfBase] = useState<Record<string, unknown>>({});
  const ssfDebounce = useRef<ReturnType<typeof setTimeout> | null>(null);
  /* 最新选中皮肤 id（ref）：500ms 防抖窗口内可能已换皮，而 fire 闭包里捕到的
     selectedId 是发起时的旧值——写盘前必须和它比对，防止旧皮参数写到新皮上。 */
  const selectedIdRef = useRef(selectedId);
  /* 自动保存失败标记：失败后 dirty 仍为 true，saving 由 true→false 又会触发
     effect，于是每 0.6s 重试一次（无限狂打 server + 狂弹 toast）。置位后暂停
     自动重试，等用户再次改动（编辑操作里清除）才继续。 */
  const saveFailedRef = useRef(false);
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

  useEffect(() => {
    selectedIdRef.current = selectedId;
  }, [selectedId]);

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
    /* 只在有 JSON 皮肤快照时同步方向：ssf 皮 data.skin 恒为 null，方向挂在
       overrides 上（见下方 ssf-settings effect），这里若照写会把横排冲成竖排。 */
    if (data.skin) setHorizontal(data.skin.layout?.horizontal === true);
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
  useEffect(() => {
    setShowIndex(data.config.candidates?.show_index !== false);
  }, [data.config.candidates?.show_index]);

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
        /* 横排是 overrides 的成员（server ssf.rs:782-784 会注入 layout.horizontal）：
           只回填颜色/字号会让「已存横排」的皮肤一直显示成竖排，开关点一下又只是
           再写一次 true——永远关不掉。ssf 皮没有 data.skin 可参考，方向只能从这拿。 */
        setHorizontal(r.overrides?.horizontal === true);
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

  /* 换皮即丢弃待发的 SSF 参数写（防「换皮串写」）：在 500ms 防抖窗口里切皮肤，
     旧皮的最后一笔 overrides 会被 POST 到刚选中的新皮上（新皮凭空多出上一张
     皮的颜色/字号）。selectedId 一变就 clearTimeout 并置空，定时器不再有意义。 */
  useEffect(
    () => () => {
      if (ssfDebounce.current) {
        clearTimeout(ssfDebounce.current);
        ssfDebounce.current = null;
      }
    },
    [selectedId],
  );

  /* SSF 参数写：本地乐观 + 防抖 500ms POST（server 写盘即 bump skin_ver，
     DLL 立即重拉生效）。 */
  const patchSsf = (patch: Partial<SsfOverrides>, immediate = false) => {
    setSsfOv((prev) => ({ ...prev, ...patch }));
    if (ssfDebounce.current) clearTimeout(ssfDebounce.current);
    const id = selectedId;
    if (!id) return;
    const fire = () => {
      ssfDebounce.current = null;
      /* 换皮串写第二道防线：真正发请求前再核对一次「当前选中仍是发起时那张皮」。
         clearTimeout 只挡得住 effect 之后才切皮的情况，immediate 路径与竞态
         仍可能带着旧 id 出发——不等就整体丢弃，宁可不写也不写错皮。 */
      if (selectedIdRef.current !== id) return;
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
          await fetchWithTimeout('/api/reset', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
          let state: Record<string, unknown> = {};
          for (const ch of trialRaw) {
            const r = await fetchWithTimeout('/api/key', {
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
          await fetchWithTimeout('/api/reset', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
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
    /* 选中皮肤时列表页签联动到对应组，避免选中项落在隐藏页签里。 */
    setListTab(meta.kind === 'ssf' ? 'ssf' : 'json');
    setRealFailed(false);
    setPreviewVer((v) => v + 1);
    setAb(false);
    setDirty(false);
    /* 换皮=换一份要保存的内容：清掉上一张皮的保存失败标记，
       否则新皮一进来自动保存就处于暂停态，怎么改都存不下去。 */
    saveFailedRef.current = false;
    if (meta.kind === 'ssf') {
      /* 方向先复位，真值等 ssf-settings 的 overrides 回来再定。 */
      setHorizontal(false);
      setSkin(null);
      setOriginal(null);
      return;
    }
    setLoading(true);
    try {
      const next = await loadSkin(meta.id);
      setSkin(next);
      setOriginal(next);
      /* 方向要取「刚载入的这张皮」：data.skin 是当前已生效的皮，
         选中别款时两者不是同一份，照 data.skin 取会把别款的方向抄过来。 */
      setHorizontal(next.layout?.horizontal === true);
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

  /* 假状态清理③：候选窗动效/选重暂留接真实配置（P1-15 起统一走
     updateConfig：GET→深度合并→带 rev POST；409 由它提示其他页面已改）。
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
      await updateConfig({ appearance: patch });
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

  /* 旧开关真源是 JSON 皮肤 layout.inline_preedit，并非全局 config 字段。
     GET 最新皮肤后只改这一项，避免顺带提交编辑器脏值或清掉挂件；
     select:false 不因编辑别款而切换当前皮肤。SSF 后端没有此覆盖契约。 */
  const setInlinePreedit = async (value: boolean) => {
    if (!skin || isSsf || loading || saving) return;
    const id = skin.id;
    setSaving(true);
    try {
      const fresh = await loadSkin(id);
      const next: Skin = { ...fresh, layout: { ...fresh.layout, inline_preedit: value } };
      await saveSkin(next, { select: false });
      if (selectedIdRef.current !== id) return;
      setOriginal((prev) => prev?.id === id ? { ...prev, layout: { ...prev.layout, inline_preedit: value } } : next);
      setSkin((prev) => prev?.id === id ? { ...prev, layout: { ...prev.layout, inline_preedit: value } } : prev);
      setPreviewVer((v) => v + 1);
      notify(`编码内联已${value ? '开启' : '关闭'}并保存${id === data.skins.current ? '（下次打字生效）' : '（设为当前皮肤后生效）'}`);
    } catch (e) {
      notify(`编码内联保存失败：${(e as Error).message}`);
    } finally {
      setSaving(false);
    }
  };

  /* 全局候选设置（candidates.*）：走 updateConfig（同上，带 rev / 409 提示）。 */
  const writeCandidates = async (patch: Record<string, unknown>): Promise<void> => {
    await updateConfig({ candidates: patch });
    await refresh();
  };

  /* 滑杆防抖写：拖动过程中只在停手 400ms 后写一次（writeAppearance
     本身是读-改-写，避免拖动逐帧打 /api/config）。 */
  const scheduleCfg = (patch: Record<string, unknown>) => {
    if (cfgDebounce.current) clearTimeout(cfgDebounce.current);
    cfgDebounce.current = setTimeout(() => {
      void writeAppearance(patch).catch((e) => {
        setFirstPct(null);
        setOtherPct(null);
        setPcf(data.config.appearance?.per_cand_font === true);
        notify(`保存失败：${(e as Error).message}`);
      });
    }, 400);
  };

  /* 三个 patch* 都是「用户主动编辑」：顺手清掉保存失败标记，恢复自动保存
     （失败退避只该拦住无人操作的空转重试，不该让用户改完还存不下去）。 */
  const patchLayout = (key: string, value: number | string) => {
    setSkin((s) => (s ? { ...s, layout: { ...s.layout, [key]: value } } : s));
    saveFailedRef.current = false;
    setDirty(true);
  };
  const patchMaterial = (key: string, value: number | boolean) => {
    setSkin((s) => (s ? { ...s, material: { ...s.material, [key]: value } } : s));
    saveFailedRef.current = false;
    setDirty(true);
  };
  const patchColors = (key: string, value: string) => {
    setSkin((s) => (s ? { ...s, colors: { ...s.colors, [key]: value } } : s));
    saveFailedRef.current = false;
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
      /* 【颜色白名单修复 2026-11-01】旧白名单只合并 back_color 与
         hilited_candidate_back_color 两键（材质区两个取色器时代）；
         色板全量上 UI 后其余颜色项改动在保存时被静默丢弃（POST 落盘
         旧值、界面却转已保存）——假设置。现按色板 14 项全列合并。 */
      const COLOR_KEYS = [
        'back_color',
        'border_color',
        'shadow_color',
        'candidate_back_color',
        'preedit_back_color',
        'text_color',
        'candidate_text_color',
        'label_color',
        'comment_text_color',
        'hilited_text_color',
        'hilited_comment_text_color',
        'hilited_candidate_back_color',
        'hilited_candidate_text_color',
        'hilited_candidate_label_color',
      ] as const;
      const colors: Record<string, string> = { ...(fresh.colors || {}) };
      for (const k of COLOR_KEYS) {
        const v = (skin.colors as Record<string, string> | undefined)?.[k];
        if (v != null) colors[k] = v;
      }
      const merged: Skin = {
        ...fresh,
        colors: colors as typeof fresh.colors,
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
          /* 【2026-11-03】新增两个布局项必须同步进白名单，否则又是「UI 能改、
             保存被丢弃」的假设置：候选窗总宽（0=自适应）与序号显示种类。 */
          width: skin.layout.width ?? fresh.layout.width,
          label_style: skin.layout.label_style ?? fresh.layout.label_style,
          /* 【2026-11-04】边框厚度必须有 UI+白名单成对，否则又是「能拖不落盘」的假设置。 */
          border_width: skin.layout.border_width ?? fresh.layout.border_width,
        },
      };
      await saveSkin(merged, { select: false });
      setSkin(merged);
      setOriginal(merged);
      setAb(false);
      setDirty(false);
      setRealFailed(false);
      saveFailedRef.current = false;
      setPreviewVer((v) => v + 1);
      notify(`已保存皮肤「${shortName(merged.name)}」`);
    } catch (e) {
      /* 置失败标记：不拦住的话 saving 由 true→false 又会触发自动保存 effect，
         dirty 仍是 true → 每 0.6s 重试一次，无限刷 server 也无限弹 toast。
         置位后自动重试暂停，toast 只出现这一次；用户下次改动时清除（见 patch*）。 */
      saveFailedRef.current = true;
      notify(`保存失败：${(e as Error).message}（已暂停自动重试，改动后会自动再试）`);
    } finally {
      setSaving(false);
    }
  };

  /* 自动保存（对齐旧版"改动即存"习惯）：dirty 且非 A/B 对比时
     防抖 600ms 调 onSave；拖动中 skin 连续变化不断重置计时，停手即存。
     保存失败后 saveFailedRef 被置位，这里直接不再排期——否则每次 saving
     回落都会重新排一次 600ms 定时器，形成 0.6s 一次的死循环重试。 */
  const onSaveRef = useRef(onSave);
  onSaveRef.current = onSave;
  useEffect(() => {
    if (!dirty || !skin || saving || ab) return;
    if (saveFailedRef.current) return;
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
      saveFailedRef.current = false;
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
        saveFailedRef.current = false;
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
      const r = await fetchWithTimeout(`/api/skin/import_ssf?name=${encodeURIComponent(file.name)}`, {
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
      const r = await fetchWithTimeout('/api/weasel_import', {
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
      saveFailedRef.current = false;
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
          {/* 【皮肤列表分页签 2026-11-01】矢量与 .ssf 原本同栏竖排，位图
              在底部要滚动才能点到；改为顶部两页签一键切换组。 */}
          <div className="seg sk-kind-seg" role="tablist">
            <button
              type="button"
              role="tab"
              aria-selected={listTab === 'json'}
              className={`sg${listTab === 'json' ? ' on' : ''}`}
              onClick={() => setListTab('json')}
            >
              矢量 {jsonSkins.length}
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={listTab === 'ssf'}
              className={`sg${listTab === 'ssf' ? ' on' : ''}`}
              onClick={() => setListTab('ssf')}
            >
              位图 .ssf {ssfSkins.length}
            </button>
          </div>
          {listTab === 'json' && (
            <>
          <div className="list-label">
            <span>矢量皮肤</span>
            <span>{jsonSkins.length}</span>
          </div>
          {jsonSkins.map((meta) => {
            const active = selectedId === meta.id;
            const current = data.skins.current === meta.id;
            const pal = skinSwatch(meta.swatch, meta.accent, lightTheme);
            return (
              <div
                key={meta.id}
                className={`skin-item${active ? ' on' : ''}`}
                onClick={() => applySkin(meta)}
              >
                <div
                  className="skin-thumb"
                  style={
                    {
                      background: pal.background,
                      borderColor: pal.border,
                      '--sw-bar': pal.bar,
                    } as React.CSSProperties
                  }
                />
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
            </>
          )}
          {listTab === 'ssf' && (
            <>
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
            </>
          )}
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
                  ['dark', '深色'],
                  ['light', '浅色'],
                  ['wall', '壁纸'],
                ] as const
              ).map(([key, label]) => (
                <button
                  key={key}
                  type="button"
                  className={`sg${bg === key ? ' on' : ''}`}
                  onClick={() => setBgChoice(key)}
                >
                  {label}
                </button>
              ))}
            </div>
            <div className="spacer" />
            <div className="tool">100%</div>
            <div className="skin-layout-tools">
            <div className="tool skin-layout-tool">
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
            <div className="tool skin-layout-tool inline-preedit-tool">
              <span id="inline-preedit-label">编码内联</span>
              <button
                type="button"
                className={`switch${(isSsf ? ssfOv.inline_preedit !== false : skin?.layout.inline_preedit !== false) ? ' on' : ''}`}
                aria-labelledby="inline-preedit-label"
                aria-pressed={isSsf ? ssfOv.inline_preedit !== false : skin?.layout.inline_preedit !== false}
                disabled={(isSsf ? !selectedId : !skin) || loading || saving}
                title="编码内联到应用：立即保存所选皮肤；开启在应用中显示编码，关闭仅在候选窗中显示"
                onClick={() => {
                  if (isSsf) {
                    /* 【编码内联 2026-11-05】SSF 同款覆盖：写 overrides 的
                       inline_preedit（server 投影 layout 同键下发，DLL 零
                       改动）。开关类参数 immediate 立即落盘，不等防抖。 */
                    const next = ssfOv.inline_preedit === false;
                    patchSsf({ inline_preedit: next }, true);
                    notify(`编码内联已${next ? '开启' : '关闭'}并保存${selectedId === data.skins.current ? '（下次打字生效）' : '（设为当前皮肤后生效）'}`);
                  } else {
                    void setInlinePreedit(skin?.layout.inline_preedit === false);
                  }
                }}
              />
            </div>
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
                <Candidate
                  skin={shownSkin}
                  size="lg"
                  ab={ab}
                  fontScales={pcfScales}
                  width={
                    Number(skin?.layout?.width) > 0
                      ? Math.min(300, Math.max(150, Math.round(Number(skin?.layout?.width))))
                      : undefined
                  }
                />
              </div>
            )}
          </div>

          {/* 【颜色区下置 2026-11-01】色板网格放预览正下方（预览已砍到
              44% 高），按功能分三区：高亮 / 候选框 / 文字。项集合与 insp
              旧「颜色」区一致（窗底色/高亮底原在「材质」区，现统一收编）。 */}
          {!isSsf ? (
            <div className="stage-colors">
              {([
                {
                  title: '高亮区 · 选中项底色与文字',
                  keys: [
                    ['hilited_candidate_back_color', '高亮底'],
                    ['hilited_candidate_text_color', '高亮文字'],
                    ['hilited_candidate_label_color', '高亮序号'],
                    ['hilited_text_color', '编码高亮'],
                    ['hilited_comment_text_color', '高亮注释'],
                    /* hilited_back_color 全链零消费，撤出（假设置清除 2026-11-01） */
                  ] as const,
                },
                {
                  title: '文字区 · 普通候选文字',
                  keys: [
                    ['text_color', '编码'],
                    ['candidate_text_color', '候选文字'],
                    ['label_color', '序号'],
                    ['comment_text_color', '注释'],
                  ] as const,
                },
                {
                  title: '候选框区 · 窗体与底色',
                  keys: [
                    /* 【假设置清除 2026-11-01】hilited_back_color 与
                       capsule_back/text_color 在 DLL+server 全链零消费
                       （「状态胶囊」概念引擎不存在，胶囊=高亮候选底块，
                       由高亮底负责），设置后永不生效——撤出 UI。 */
                    ['back_color', '窗底色'],
                    ['border_color', '边框'],
                    ['shadow_color', '阴影'],
                    ['candidate_back_color', '候选底'],
                    ['preedit_back_color', '编码底'],
                  ] as const,
                },
              ]).map(({ title, keys }) => (
                <div className="sc-group" key={title}>
                  <div className="sc-h">{title}</div>
                  <div className="sc-grid">
                    {keys.map(([key, label]) => {
                      const cur =
                        (skin?.colors?.[key] as string | undefined) ||
                        (fallbackSkin.colors as unknown as Record<string, string>)[key] ||
                        '#00000000';
                      return (
                        <div className="sc-item" key={key} title={`skin.colors.${key}`}>
                          <ColorSwatch value={cur} label={label} onChange={(v) => patchColors(key, v)} />
                          <span className="lab">{label}</span>
                        </div>
                      );
                    })}
                  </div>
                </div>
              ))}
            </div>
          ) : (
            /* 【SSF 文字颜色下置 2026-11-01】SSF 皮肤的覆盖色（编码/首选/
               候选/注释）从 insp 面板移到预览下方，与自有皮肤色板同位。 */
            <div className="stage-colors">
              <div className="sc-group">
                <div className="sc-h">文字颜色 · 覆盖皮肤原色</div>
                <div className="sc-grid">
                  {([
                    ['code_color', '编码', '编码行文字色'],
                    ['first_color', '首选', '第一候选文字色（皮肤的高亮表达）'],
                    ['text_color', '候选', '候选文字色'],
                    ['annotation_color', '注释', '注释/拼音色'],
                  ] as const).map(([key, label, tip]) => {
                    const cur =
                      (ssfOv[key] as string | undefined) || (ssfBase[key] as string | undefined) || '#CCCCCC';
                    return (
                      <div className="sc-item" key={key} title={tip}>
                        <ColorSwatch value={cur} label={label} onChange={(v) => patchSsf({ [key]: v } as Partial<SsfOverrides>)} />
                        <span className="lab">{label}</span>
                      </div>
                    );
                  })}
                </div>
                <button
                  type="button"
                  className="btn outline sm"
                  style={{ marginTop: 8 }}
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
            </div>
          )}

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
              {/* 【2026-11-03】候选窗动效总开关/速度/选重暂留/动效预览统一迁到「动效」页；
                  皮肤页只保留与皮肤本身相关的字号强调。 */}
              <div className="row-line">
                <span className="lab" title="候选窗动效、动效速度、选重暂留、高亮跟随放大都在「动效」页">动效设置</span>
                <button type="button" className="link-more" onClick={() => navigate('motion')}>
                  去「动效」页调整 →
                </button>
              </div>
              <div className="row-line">
                <span className="lab" title="开：高亮候选按其比例放大、其余候选缩小（想让它跟着光标移动，去「动效」页打开「高亮跟随放大」）；关：全页同一字号（默认）">高亮区放大 / 其余缩小</span>
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
                <span className="lab">　高亮区比例</span>
                <Slider
                  disabled={!pcf}
                  value={firstPct ?? Math.round((data.config.appearance?.first_font_scale ?? 1) * 100)}
                  min={50}
                  max={200}
                  step={5}
                  width={104}
                  label="高亮区字号比例"
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
            {/* 【颜色区下置 2026-11-01】原竖排「颜色」区整体移到预览下方的
                网格（stage-colors）；此处不再重复。 */}
            <div className="insp-sec">
              <div className="insp-sec-h">
                <Icon name="chev-down" className="icon chev" />
                材质
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

            {/* 【颜色区下置 2026-11-01】原竖排「颜色」区整体移到预览下方的
                4 列网格（stage-colors，18 项含窗底色/高亮底），此处删除。 */}

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
                <span className="lab" title="候选窗边框厚度（px）；0=不画边框。改完自动保存，候选窗即时生效">边框厚度</span>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                  <Slider
                    value={Number(skin?.layout?.border_width ?? 1)}
                    min={0}
                    max={6}
                    step={0.5}
                    width={70}
                    onChange={(v) => patchLayout('border_width', v)}
                    label="边框厚度"
                  />
                  <span className="val">{Number(skin?.layout?.border_width ?? 1)} px</span>
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
              <div className="row-line">
                <span className="lab" title="候选窗总宽（px）；0=按候选内容自适应">候选宽度</span>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                  <Slider
                    value={Math.round(Number(skin?.layout?.width) || 0)}
                    min={0}
                    max={600}
                    step={10}
                    width={70}
                    onChange={(v) => patchLayout('width', v)}
                    label="候选宽度"
                    suffix="px"
                  />
                  <span className="val">
                    {Number(skin?.layout?.width) > 0 ? `${Math.round(Number(skin?.layout?.width))} px` : '自适应'}
                  </span>
                </div>
              </div>
              <div className="row-line">
                <span className="lab" title="候选词前是否显示序号（全局设置，矢量皮肤共用；SSF 可在其专属设置里覆盖）">序号开关</span>
                <Switch
                  on={showIndex}
                  title="candidates.show_index"
                  onChange={(v) => {
                    setShowIndex(v);
                    void writeCandidates({ show_index: v }).catch((e) => {
                      setShowIndex(!v);
                      notify(`保存失败：${(e as Error).message}`);
                    });
                  }}
                />
              </div>
              <div className="row-line">
                <span className="lab" title="序号的字形样式：阿拉伯数字 / 中文数字 / 罗马数字；需要先打开上面的「序号开关」">序号显示种类</span>
                <div className="seg" style={{ opacity: showIndex ? 1 : 0.45 }}>
                  {(
                    [
                      ['digit', '数字'],
                      ['zh', '中文'],
                      ['roman', '罗马'],
                    ] as const
                  ).map(([key, label]) => (
                    <button
                      key={key}
                      type="button"
                      className={`sg${(skin?.layout?.label_style || 'digit') === key ? ' on' : ''}`}
                      disabled={!showIndex}
                      title={showIndex ? `序号显示为${label}` : '先打开「序号开关」'}
                      onClick={() => patchLayout('label_style', key)}
                    >
                      {label}
                    </button>
                  ))}
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
            {/* 【SSF 文字颜色下置 2026-11-01】原「文字颜色」insp 区整体
                移到预览下方（与自有皮肤色板同位），此处删除。 */}
            </>
            )}

            <div className="insp-sec">
              <div className="insp-sec-h">
                <Icon name="chev-down" className="icon chev" />
                颜色角色
                <Badge>{Object.keys(shownSkin?.colors || {}).length || 21}</Badge>
              </div>
              {/* 【颜色区下置 2026-11-01】自有皮肤的可调色板已上移到预览
                  下方网格，此处只读色块墙对其冗余——仅 SSF（无编辑色板）
                  保留总览；weasel 配色互导两种皮肤都保留。 */}
              {isSsf ? (
                <div className="color-grid">
                  {Object.entries(shownSkin?.colors || {}).map(([key, value]) => (
                    <ColorSwatch key={key} value={value} label={COLOR_LABELS[key] || key} />
                  ))}
                </div>
              ) : null}
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
