
"use strict";
const $ = s => document.querySelector(s);
const $$ = s => [...document.querySelectorAll(s)];
const api = async (m, p, b) => {
  const r = await fetch(p, { method: m, headers: { "Content-Type": "application/json" }, body: b ? JSON.stringify(b) : undefined });
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(j.error || r.status);
  return j;
};
let CFG = null, SKIN = null;
const msg = (t, cls) => { const e = $("#foot-msg"); e.textContent = t || ""; e.className = "msg " + (cls || ""); };

/* ── 路径工具 ── */
const getP = (o, p) => p.split(".").reduce((x, k) => (x == null ? x : x[k]), o);
const setP = (o, p, v) => { const ks = p.split("."); let t = o; for (let i = 0; i < ks.length - 1; i++) t = t[ks[i]]; t[ks[ks.length - 1]] = v; };

/* ── data-path 绑定 ── */
function bindInputs() {
  $$("[data-path]").forEach(el => {
    const v = getP(CFG, el.dataset.path);
    if (el.type === "number") el.value = v;
    else el.value = v ?? "";
  });
  // 【八十六修·全页实时生效】B 绑定的开关改动即存（防抖 400ms）；
  // 整句模型页控件由 autoSave 内部排除（该页保留「保存并应用」）。
  const B = (id, path, conv) => { const el = $(id); el.checked = !!getP(CFG, path); el.onchange = () => { setP(CFG, path, conv ? conv(el) : el.checked); autoSave(el); }; };
  B("#g-shift", "general.shift_switch"); B("#g-ctrl", "general.ctrl_space_switch");
  B("#g-recent", "general.switch_recent_schema");
  $("#g-caps").value = { Clear: "clear", Switch: "switch", None: "none" }[CFG.general.caps_action] || "clear";
  $("#g-caps").onchange = e => { CFG.general.caps_action = { clear: "Clear", switch: "Switch", none: "None" }[e.target.value]; autoSave(e.target); };
  B("#i-push", "input.auto_push"); B("#i-uniq", "input.auto_select_unique"); B("#i-clear", "input.auto_clear_empty");
  B("#i-enter", "input.enter_clear"); B("#i-mixed", "input.mixed_input");
  B("#i-ascii-p", "input.ascii_punct"); B("#i-dunhao", "input.slash_dunhao"); B("#i-bdunhao", "input.backslash_dunhao");
  // 【Tab 双模式 2026-09-08】下拉映射 tab_clear（清屏=true / 导航=false），
  // 切换即存。【八十六修】输入页其他项已全部改动即存（防抖），此处的
  // 独立直存保留（即时反馈文案）。
  {
    const tm = $("#i-tabmode");
    tm.value = CFG.input.tab_clear === false ? "nav" : "clear";
    tm.onchange = async () => {
      CFG.input.tab_clear = tm.value !== "nav";
      try { await saveConfig(); msg(tm.value === "nav" ? "Tab=选重导航 ✓（高亮下一候选，空格上屏）" : "Tab=清屏 ✓", "ok"); } catch (e) { msg("保存失败：" + e.message, "err"); }
    };
  }
  // #c-v（竖排候选）在 bindSkin 里接皮肤 layout.horizontal，不绑 config
  // 【2026-09-08】「显示序号」开关只在皮肤页（#sk-idx 绑 config.candidates.
  // show_index）——输入页不再重复（单一真源）
  B("#c-cmt", "candidates.show_pinyin_comment"); B("#c-ucmt", "candidates.show_unicode_comment");
  B("#c-split", "candidates.show_split");
  B("#p-full", "punct.full_shape"); B("#p-pair", "punct.pair_brackets");
  B("#s-en", "sentence.enabled"); B("#s-auto", "sentence.auto_enable"); B("#s-early", "sentence.early_commit");
  B("#s-empty", "sentence.empty_code_auto_commit"); B("#s-rr", "sentence.rerank.enabled");
  /* 【八十一修】「稳 N 键」文案跟随 early_need 数值；改观察窗即刷新 */
  {
    const earlyLbl = () => { const l = document.querySelector('label[for="s-early"]'); if (l) l.textContent = `提前上屏（稳 ${CFG.sentence.early_need} 键）`; };
    earlyLbl();
    const en = document.querySelector('input[data-path="sentence.early_need"]');
    if (en) en.addEventListener("input", earlyLbl);
  }
  B("#u-af", "user.auto_frequency"); B("#u-la", "user.log_adjust");
  B("#snd-en", "sound.enabled");
  $("#snd-vol").value = CFG.sound?.volume ?? 50;
  $("#snd-vol-v").textContent = (CFG.sound?.volume ?? 50) + "%";
  $("#snd-vol").oninput = e => { CFG.sound = CFG.sound || { enabled: false, volume: 50 }; CFG.sound.volume = +e.target.value; $("#snd-vol-v").textContent = e.target.value + "%"; autoSave(e.target); };
  $("#snd-test").onclick = () => new Audio("/api/sound?tag=key").play().catch(() => msg("试听失败", "err"));
  // OpenCC 繁简 / emoji
  B("#oc-en", "opencc.enabled"); B("#oc-emoji", "opencc.emoji");
  $("#oc-dir").value = CFG.opencc?.to_traditional === false ? "t2s" : "s2t";
  $("#oc-dir").onchange = e => { CFG.opencc = CFG.opencc || { enabled: false, to_traditional: true, emoji: false }; CFG.opencc.to_traditional = e.target.value === "s2t"; autoSave(e.target); };
  // 剪贴板上屏
  B("#cb-en", "clipboard.enabled");
  $("#cb-wl").value = (CFG.clipboard?.whitelist || []).join(", ");
  $("#cb-wl").onchange = e => {
    CFG.clipboard = CFG.clipboard || { enabled: false, whitelist: [] };
    CFG.clipboard.whitelist = e.target.value.split(/[,，;；\n]/).map(s => s.trim()).filter(Boolean);
    autoSave(e.target);
  };
  $("#exp-all").onclick = async () => {
    try {
      const r = await api("GET", "/api/export");
      const blob = new Blob([JSON.stringify(r, null, 2)], { type: "application/json" });
      const a = document.createElement("a");
      a.href = URL.createObjectURL(blob);
      a.download = `hufu-data-${r.stamp || Date.now()}.json`;
      a.click();
      msg("快照已导出", "ok");
    } catch (e) { msg(e.message, "err"); }
  };
  $("#c-2nd").value = CFG.candidates.second_select || "";
  $("#c-2nd").oninput = e => { if (e.target.value) CFG.candidates.second_select = e.target.value[0]; autoSave(e.target); };
  $("#c-3rd").value = CFG.candidates.third_select || "";
  $("#c-3rd").oninput = e => { if (e.target.value) CFG.candidates.third_select = e.target.value[0]; autoSave(e.target); };
  $("#r-prefix").value = CFG.reverse.prefix || "`";
  $("#r-prefix").oninput = e => { if (e.target.value) CFG.reverse.prefix = e.target.value[0]; autoSave(e.target); };
  // 【2026-09-06 大统一】反查/拆分方案下拉（/api/assets 实时列目录）
  const fillSel = (selId, cur, list, phOff) => {
    const el = $(selId); el.innerHTML = "";
    const mk = (v, t) => { const o = document.createElement("option"); o.value = v; o.textContent = t; return o; };
    el.appendChild(mk("", phOff));
    (list || []).forEach(n => el.appendChild(mk(n, n)));
    el.value = cur || "";
    if (el.selectedIndex < 0) el.selectedIndex = 0;
  };
  api("GET", "/api/assets").then(a => {
    fillSel("#rev-scheme", CFG.reverse.scheme, a.reverse, "（关闭反查）");
    fillSel("#split-scheme", CFG.candidates.split_scheme, a.split, "（关闭拆分）");
  }).catch(e => console.warn("assets", e));
  const pick = (selId, fn) => {
    $(selId).onchange = async e => {
      try {
        fn(e.target.value);
        await saveConfig();
        msg("已切换并生效 ✓", "ok");
      } catch (err) { msg(err.message, "err"); }
    };
  };
  pick("#rev-scheme", v => CFG.reverse.scheme = v);
  pick("#split-scheme", v => CFG.candidates.split_scheme = v);
  $$("[data-path]").forEach(el => {
    // 【八十六修】数字/文本改动即存（防抖；整句页控件 autoSave 内排除）
    if (el.type === "number") el.oninput = () => { const v = parseFloat(el.value); if (!isNaN(v)) setP(CFG, el.dataset.path, v); autoSave(el); };
    else el.oninput = () => { setP(CFG, el.dataset.path, el.value); autoSave(el); };
  });
}

/* ── 整句权重滑杆（拖动后点「保存并应用」生效；项数与 W_DEFAULTS 一一对应）── */
const WDEFS = [
  ["beam_width", "Beam 宽度", 10, 32000, 1],
  ["candidate_limit", "候选上限", 5, 50, 1],
  ["max_raw_length", "缓冲码长上限", 16, 256, 4],
  ["rank_penalty", "名次惩罚", 0, 1, 0.005],
  ["emitted_character_reward", "出字奖励", 0, 8, 0.05],
  ["isolation_lambda", "孤立生僻惩罚", 0, 20, 0.1],
  ["isolation_threshold", "生僻字频阈值", 100, 21000, 100],
  ["confidence", "提前上屏置信", 0.5, 1, 0.001],
  ["supplement_baseline", "语料奖励基准", 0, 20, 0.1],
  ["supplement_scale", "语料奖励缩放", 0, 8, 0.1],
  ["supplement_maximum", "语料奖励上限", 1, 40, 0.5],
  ["high_freq_limit", "整句高频字上限（0=不限）", 0, 4000, 1],
];
/* 出厂调校值：恢复初始值按钮用
   【2026-09-08 默认回归 1.4.8 模型值】W1 束宽 30000 实机每键解码
   数百 ms 致「越打越卡」（1.4.8 基线对照实锤）——出厂回 1.4.8 值。 */
const W_DEFAULTS = {
  beam_width: 200, candidate_limit: 20, max_raw_length: 128,
  rank_penalty: 0.03, emitted_character_reward: 2.0,
  isolation_lambda: 2.0, isolation_threshold: 3000, confidence: 0.99,
  dict_bias: 1.0, supplement_baseline: 9.0, supplement_scale: 2.0, supplement_maximum: 32.0,
  high_freq_limit: 0,
};
/* ── 【数值显示与手输 2026-09-10】滑杆数值显示统一（用户规格）：
   ① 纯数值最多两位小数（去尾零）；② 点击数字行内手动输入（回车/
   失焦提交按滑杆范围钳制，Esc 取消）。百分比类显示（NN%，材质浓
   度系列）带单位语义，跳过不改。权重滑杆动态生成，buildSliders
   尾部与皮肤滑杆绑定后各调用一次。 ── */
const fmtV = v => { let s = (+v).toFixed(2); if (s.includes(".")) s = s.replace(/0+$/, "").replace(/\.$/, ""); return s; };
function hookValFields() {
  $$(".field").forEach(f => {
    const rg = f.querySelector("input[type=range]");
    const sp = f.querySelector("span.val");
    if (!rg || !sp || sp.dataset.ed) return;
    if ((sp.textContent || "").includes("%")) return; // 百分比类跳过
    sp.dataset.ed = "1";
    sp.style.cursor = "text";
    sp.title = "点击输入数值";
    sp.textContent = fmtV(rg.value);
    rg.addEventListener("input", () => { sp.textContent = fmtV(rg.value); });
    sp.addEventListener("click", () => {
      if (sp.querySelector("input")) return;
      const inp = document.createElement("input");
      inp.type = "number"; inp.step = "any";
      inp.value = parseFloat(sp.textContent);
      inp.style.cssText = "width:58px;font-size:12px;padding:0 3px;border:1px solid var(--acc2);border-radius:4px;background:transparent;color:inherit";
      sp.textContent = ""; sp.appendChild(inp);
      inp.focus(); inp.select();
      let done = false;
      const finish = ok => {
        if (done) return; done = true;
        const v = parseFloat(inp.value);
        if (ok && isFinite(v)) {
          const cv = Math.min(parseFloat(rg.max), Math.max(parseFloat(rg.min), v));
          rg.value = cv;
          if (typeof rg.oninput === "function") rg.oninput();
        }
        sp.textContent = fmtV(rg.value);
      };
      inp.onblur = () => finish(true);
      inp.onkeydown = e => {
        if (e.key === "Enter") inp.blur();
        else if (e.key === "Escape") { done = true; sp.textContent = fmtV(rg.value); }
      };
    });
  });
}
function buildSliders() {
  const box = $("#w-sliders"); box.innerHTML = "";
  WDEFS.forEach(([k, name, min, max, step]) => {
    const f = document.createElement("div"); f.className = "field";
    f.innerHTML = `<label>${name} <span class="val" id="wv-${k}"></span></label>
      <input type="range" id="ws-${k}" min="${min}" max="${max}" step="${step}">`;
    box.appendChild(f);
    const s = f.querySelector("input"), v = f.querySelector(".val");
    const sync = () => { v.textContent = fmtV(CFG.sentence.weights[k]); s.value = CFG.sentence.weights[k]; };
    s.oninput = () => { CFG.sentence.weights[k] = parseFloat(s.value); v.textContent = fmtV(s.value); };
    sync();
  });
  hookValFields();
}
/* 恢复初始值：整句参数一键还原出厂调校值 + 自动保存应用 */
$("#w-reset").onclick = async () => {
  try {
    Object.assign(CFG.sentence.weights, W_DEFAULTS);
    /* 【八十一修 2026-09-14】出厂档=本机实测调校值（八十修定版）：
       稳 3 键 / 无水位线 / 空码上屏开 / 重排 5 选 350ms */
    CFG.sentence.early_need = 3;
    CFG.sentence.empty_code_auto_commit = true;
    CFG.sentence.rerank = Object.assign({}, CFG.sentence.rerank, { enabled: true, top_k: 5, debounce_ms: 350 });
    buildSliders();
    $$("[data-path]").forEach(el => { const v = getP(CFG, el.dataset.path); if (el.type === "number") el.value = v; else el.value = v ?? ""; });
    $("#s-empty").checked = true; $("#s-rr").checked = CFG.sentence.rerank.enabled;
    await saveConfig();
    msg("整句参数已恢复初始值并保存生效 ✓", "ok"); await refreshInfo();
  } catch (e) { msg(e.message, "err"); }
};

/* 【修复标签 2026-09-11】XSS：esc 补单引号转义（& < > " ' 全覆盖，属性插值双保险） */
const esc = s => String(s ?? "").replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

/* ── 【修复标签 2026-09-11】保存收口：数字 clamp + 导入校验 ──
   ① 所有数值配置保存前统一钳制（NaN/越界 → 界内），不只改显示。
     上限取任务规定值与滑杆/Rust scale_geometry 上限（同源，lib.rs:265）
     的较大者，保证不截断 UI 可达的合法值（如 margin 滑杆上限 60）。
   ② saveConfig/saveSkin 是 POST /api/config、/api/skin 的唯一出口：
     后端 apply_config（main.rs:575）整体替换 self.engine.config = cfg，
     不做字段合并——必须全量发送，不可改为只发变更字段；收集变更仍
     在内存 CFG/SKIN，动作触发时一次全量发送（与原行为一致，仅收口）。 */
const clampNum = (v, lo, hi) => { const n = parseFloat(v); return Number.isFinite(n) ? Math.min(hi, Math.max(lo, n)) : lo; };
const NUM_CLAMPS = {
  config: { "input.max_code_length": [1, 10], "candidates.page_size": [1, 10], "candidates.delay_show_ms": [0, 2000], "sound.volume": [0, 100], "appearance.anim_speed": [0, 2],
    /* 【八十一修】整句行为参数保存钳制（与 number 框 min/max 同源） */
    "sentence.early_need": [1, 8], "sentence.rerank.top_k": [2, 10], "sentence.rerank.debounce_ms": [50, 2000] },
  layout: {
    font_point: [8, 72], label_font_point: [0, 72], width: [0, 1200], min_width: [0, 1200],
    margin_x: [0, 60], margin_y: [0, 60], corner_radius: [0, 40], hilited_corner_radius: [0, 40],
    spacing: [0, 48], candidate_spacing: [0, 48], hilite_spacing: [0, 48], hilite_padding: [0, 40],
    line_spacing: [0, 40], border_width: [0, 12], shadow_radius: [0, 60],
    shadow_offset_x: [-5, 5], shadow_offset_y: [-5, 5],
  },
  material: {
    opacity: [0, 1], darken: [0, 1], noise: [0, 1],
    master_alpha: [0, 1], hilite_alpha: [0, 1], shadow_alpha: [0, 1], border_alpha: [0, 1],
  },
};
function sanitizeConfig(c) {
  if (!c) return;
  for (const p in NUM_CLAMPS.config) {
    if (typeof getP(c, p) === "number") setP(c, p, clampNum(getP(c, p), NUM_CLAMPS.config[p][0], NUM_CLAMPS.config[p][1]));
  }
}
function sanitizeSkin(s) {
  if (!s) return;
  for (const sec of ["layout", "material"]) {
    const o = s[sec]; if (!o) continue;
    for (const k in NUM_CLAMPS[sec]) {
      if (typeof o[k] === "number") o[k] = clampNum(o[k], NUM_CLAMPS[sec][k][0], NUM_CLAMPS[sec][k][1]);
    }
  }
}
const saveConfig = () => { sanitizeConfig(CFG); return api("POST", "/api/config", CFG); };
const saveSkin = () => { sanitizeSkin(SKIN); return api("POST", "/api/skin", SKIN); };

/* ── 【八十六修·全页实时生效】「保存并应用」按钮只保留在整句模型页
   （权重调参语义）；其余页与皮肤页同款「改动即存」——统一 400ms
   防抖 saveConfig（与皮肤滑杆同节奏）。整句页控件（#s-*、sentence.*
   data-path、权重滑杆）不自动存：该页仍有显式「保存并应用」。 ── */
let _cfgSaveTimer = null;
const saveConfigDebounced = () => {
  if (_cfgSaveTimer) clearTimeout(_cfgSaveTimer);
  _cfgSaveTimer = setTimeout(async () => {
    try { await saveConfig(); msg("已自动保存并应用 ✓", "ok"); } catch (e) { msg("自动保存失败：" + e.message, "err"); }
  }, 400);
};
const autoSave = el => {
  if (el && el.closest && el.closest("#tab-sentence")) return;
  saveConfigDebounced();
};

/* 【修复标签 2026-09-11】导入校验：hufu config 12 节必须齐全且类型对
   （后端 serde(default) 整体替换——缺节会静默回出厂值，前端先拦）；
   非法一律 alert 且不应用 */
const CFG_SECTIONS = ["general", "schema", "input", "candidates", "reverse", "sentence", "punct", "clipboard", "appearance", "sound", "opencc", "user"];
const HEX_COLOR_RE = /^#[0-9a-fA-F]{6}([0-9a-fA-F]{2})?$/;
function validateConfig(c) {
  if (typeof c !== "object" || c === null || Array.isArray(c)) return "顶层必须是 JSON 对象";
  for (const sec of CFG_SECTIONS)
    if (typeof c[sec] !== "object" || c[sec] === null || Array.isArray(c[sec]))
      return `缺少配置节 "${sec}"（后端整体替换，缺节将回出厂值）`;
  if (typeof c.schema.current !== "string") return "schema.current 必须是字符串";
  const numFields = [["input", "max_code_length"], ["candidates", "page_size"], ["candidates", "delay_show_ms"], ["sound", "volume"], ["appearance", "font_size"]];
  for (const [sec, key] of numFields)
    if (c[sec][key] !== undefined && typeof c[sec][key] !== "number") return `${sec}.${key} 必须是数字`;
  return null;
}
/* weasel 导入：后端 from_weasel_colors（lib.rs:388）只消费数字色值
   （0xAABBGGRR）。#RRGGBB 6 位串可无歧义换算（后端 ≤0xFFFFFF 自动补 FF）
   故接受并转换；8 位串端序有歧义（AA 前置还是 RR 前置）→ 拒绝并提示。 */
const WEASEL_COLOR_KEYS = ["back_color", "border_color", "text_color", "preedit_back_color", "candidate_text_color", "candidate_back_color", "comment_text_color", "label_color", "hilited_text_color", "hilited_back_color", "hilited_candidate_text_color", "hilited_candidate_back_color", "hilited_candidate_label_color", "hilited_comment_text_color", "hilited_label_color", "hilited_mark_color", "shadow_color"];
function validateWeaselColors(colors) {
  if (typeof colors !== "object" || colors === null || Array.isArray(colors)) return { err: "配色必须是 JSON 对象" };
  const out = Object.assign({}, colors); let n = 0;
  for (const k of WEASEL_COLOR_KEYS) {
    const v = colors[k]; if (v === undefined) continue;
    if (typeof v === "number" && Number.isFinite(v) && v >= 0) { n++; continue; }
    const m = typeof v === "string" ? /^#?([0-9a-fA-F]{6})$/.exec(v.trim()) : null;
    if (m) { out[k] = parseInt(m[1].slice(4, 6) + m[1].slice(2, 4) + m[1].slice(0, 2), 16); n++; }
    else return { err: `颜色 "${k}" 非法：须为数字（0xAABBGGRR）或 #RRGGBB 串（${HEX_COLOR_RE} 的 6 位形式）` };
  }
  if (!n) return { err: "未发现任何可识别的颜色字段（back_color/text_color 等 17 项）" };
  return { colors: out };
}

/* ── 方案 ── */
async function loadSchemas() {
  // 【2026-09-06 实时同步】列表改读 /api/schemas（服务端实时列码表目录），
  // current/整句态另从 /api/state 取——新建码表文件夹后刷新即出现。
  const [list, st] = await Promise.all([
    api("GET", "/api/schemas").catch(() => ({ schemas: [] })),
    api("GET", "/api/state").catch(() => ({})),
  ]);
  const names = list.schemas || [];
  const r = { schemas: names, current_schema: st.current_schema, sentence_active: st.sentence_active };
  const box = $("#schema-list"); box.innerHTML = "";
  names.sort((a, b) => a.localeCompare(b, "zh")).forEach(n => {
    const d = document.createElement("div");
    d.className = "schema-item" + (n === r.current_schema ? " on" : "");
    d.innerHTML = `<span>${esc(n)}</span>${n.includes("整句") ? '<span class="tag">整句</span>' : ""}` +
      `<button class="opendir" title="在资源管理器中打开此方案的码表文件夹">📂 打开文件夹</button>` +
      `<button class="opendir exp-dict" title="把用户调整（置顶/隐藏/加词/调频）合并进原始码表，导出为一张新码表（码表导出\\<方案>\\）">📤 导出码表</button>` +
      `${n === r.current_schema ? '<span class="tag">当前</span>' : ""}`;
    d.querySelector(".opendir").onclick = async ev => {
      ev.stopPropagation();
      try { await api("POST", "/api/open_schema_dir", { name: n }); msg(`已打开 ${n} 的文件夹`, "ok"); }
      catch (e) { msg(e.message, "err"); }
    };
    d.querySelector(".exp-dict").onclick = async ev => {
      ev.stopPropagation();
      try {
        const r2 = await api("POST", "/api/export_schema", { name: n });
        msg(`已导出 ${r2.lines} 行 → ${r2.path}`, "ok");
      } catch (e) { msg(e.message, "err"); }
    };
    d.onclick = async () => {
      try {
        await api("POST", "/api/schema", { name: n });
        /* 关键：重取服务端配置快照。否则内存 CFG.schema.current 仍是旧方案，
           之后任何 POST /api/config（保存/整句试验都会发）会把旧方案名回写、
           服务端视为 schema 变更→切回旧方案→整句「没启动」。 */
        try { CFG = await api("GET", "/api/config"); } catch (_) {}
        msg(`已切换到 ${n}`, "ok"); await loadSchemas(); await refreshInfo();
      }
      catch (e) { msg(e.message, "err"); }
    };
    box.appendChild(d);
  });
  $("#foot-schema").textContent = "当前方案：" + r.current_schema + (r.sentence_active ? "（整句激活）" : "");
  return r;
}

/* ── 整句模型状态 ── */
async function refreshInfo() {
  const r = await api("GET", "/api/state");
  $("#engine-info").textContent = r.current_schema;
  const pill = $("#s-model-state");
  const guide = $("#s-model-guide");
  if (r.sentence_active) {
    pill.textContent = "模型：已加载 ✓";
    if (guide) guide.style.display = "none";
  } else if (r.model_present === false) {
    /* 【无模型小包 2026-09-07】文件都不在：显示下载/放置指引 */
    pill.textContent = "模型：未安装";
    if (guide) guide.style.display = "block";
  } else {
    /* 文件在但未激活：非整句方案（如个人自用方案）的正常状态 */
    pill.textContent = "模型：未加载";
    if (guide) guide.style.display = "none";
  }
  return r;
}

/* ── 皮肤 ── */
const COLOR_ROLES = [
  ["back_color", "窗口背景"], ["border_color", "窗口边框"], ["text_color", "编码文字"],
  ["candidate_text_color", "候选文字"], ["comment_text_color", "注释文字"], ["label_color", "序号"],
  ["hilited_text_color", "编码文字（高亮）"], ["hilited_candidate_text_color", "候选文字（高亮）"],
  ["hilited_candidate_label_color", "序号（高亮）"],
  ["hilited_comment_text_color", "注释（高亮）"], ["hilited_mark_color", "标记点"],
  ["shadow_color", "窗口阴影"], ["preedit_back_color", "编码区背景"],
  ["hilited_back_color", "编码背景（高亮）"], ["hilited_label_color", "序号（编码高亮）"],
  ["candidate_back_color", "候选背景"], ["candidate_shadow_color", "候选阴影"],
  ["hilited_shadow_color", "阴影（编码高亮）"], ["hilited_candidate_shadow_color", "阴影（候选高亮）"],
  ["capsule_text_color", "胶囊文字"], ["capsule_back_color", "胶囊背景"],
];
function hexOf(c) { return "#" + c.slice(1, 7); }
function alphaOf(c) { return parseInt(c.slice(7, 9) || "FF", 16); }
function withA(hex, a) { return hex + Math.round(a).toString(16).padStart(2, "0").toUpperCase(); }
function buildColorGrid() {
  const g = $("#color-grid"); g.innerHTML = "";
  // 【用户定稿】颜色只管色相：无每色 alpha 数字框——透明度只由
  // 「整体透明度」与「高亮透明度」两个滑条决定（渲染端忽略 alpha 分量）。
  COLOR_ROLES.forEach(([k, name]) => {
    const d = document.createElement("div"); d.className = "color-item";
    d.innerHTML = `<label>${name}</label><input type="color" id="ck-${k}">`;
    g.appendChild(d);
  });
  COLOR_ROLES.forEach(([k]) => {
    const ci = g.querySelector(`#ck-${k}`);
    const sync = () => { ci.value = hexOf(SKIN.colors[k] || "#000000"); };
    ci.oninput = () => { SKIN.colors[k] = ci.value + "FF"; preview(); };
    ci.onchange = () => saveSkin().catch(() => {});
    sync();
  });
}
function bindSkin() {
  $$("[data-skin]").forEach(el => { el.value = SKIN[el.dataset.skin] ?? ""; el.oninput = () => SKIN[el.dataset.skin] = el.value; });
  const saveSkinQuiet = async () => { try { await saveSkin(); } catch (e) { msg("皮肤保存失败：" + e.message, "err"); } };
  // 【纯色模型·用户定稿】不再有材质（kind/tint/暗化/旧透明度全撤）：
  // 窗底=colors.back_color（alpha×整体透明度）、高亮底=自带alpha×高亮
  // 透明度、其余非文字元素=自带alpha×整体透明度；文字 alpha 恒满。
  // 窗口底色（写 colors.back_color；alpha 由整体透明度滑条决定）
  {
    const ci = $("#mat-bgc");
    const syncBg = () => { ci.value = hexOf(SKIN.colors.back_color || "#202022"); };
    ci.oninput = () => { SKIN.colors.back_color = ci.value + "FF"; preview(); };
    ci.onchange = saveSkinQuiet;
    syncBg();
  }
  // 边框宽度/颜色：真候选窗消费 layout.border_width + colors.border_color
  {
    const bw = $("#mat-bw"), bcv = $("#mat-bc");
    const syncB = () => {
      bw.value = SKIN.layout.border_width ?? 1;
      $("#mat-bw-v").textContent = fmtV(SKIN.layout.border_width ?? 1);
      bcv.value = hexOf(SKIN.colors.border_color || "#FFFFFF");
    };
    bw.oninput = () => { SKIN.layout.border_width = parseFloat(bw.value); $("#mat-bw-v").textContent = bw.value; preview(); };
    bw.onchange = saveSkinQuiet;
    bcv.oninput = () => { SKIN.colors.border_color = bcv.value + "FF"; preview(); };
    bcv.onchange = saveSkinQuiet;
    syncB();
  }
  // 【毛玻璃退役 2026-09-11】glass 材质整链删除——旧皮肤 material.kind
  // 残值一律按 solid 归一（server 渲染层同口径：不再分支）。
  {
    if (!SKIN.material) SKIN.material = {};
    if (SKIN.material.kind === "glass" || SKIN.material.kind === "frosted") {
      SKIN.material.kind = "solid";
    }
    // 【玻璃零偏移退役】偏移滑杆不再被玻璃模式禁用
    const sox = $("#l-sox"), soy = $("#l-soy");
    sox.disabled = false; soy.disabled = false;
    sox.title = soy.title = "";
  }
  // 【动效全局开关+速度 2026-09-11】皮肤页控件，真源=全局配置
  // appearance.anim / appearance.anim_speed（DLL 经 skin op 注入读取）
  {
    const an = $("#g-anim"), sp = $("#g-animsp");
    const syncAnim = () => {
      an.checked = CFG.appearance?.anim !== false;
      sp.value = Math.round((CFG.appearance?.anim_speed ?? 1) * 100);
      sp.disabled = !an.checked;
      $("#g-animsp-v").textContent = sp.value + "%";
    };
    an.oninput = () => {
      CFG.appearance = CFG.appearance || {};
      CFG.appearance.anim = an.checked;
      syncAnim();
    };
    an.onchange = saveConfig;
    sp.oninput = () => {
      CFG.appearance = CFG.appearance || {};
      CFG.appearance.anim_speed = sp.value / 100;
      $("#g-animsp-v").textContent = sp.value + "%";
    };
    sp.onchange = saveConfig;
    syncAnim();
  }
  // 【特效退役 2026-09-22】上屏特效下拉+落印字号控件整体退役
  // （commit_fx / stamp_font_scale 链路全删）——JS 块删除，HTML 行见下。
  // 【二十四修·动效大瘦身】上屏暂留控件整体退役（hide 即收）——JS 块删除。
  // 整体透明度（master_alpha 0-1）：窗底/边框/编码底/阴影的总乘法系数
  if (SKIN.material.master_alpha == null) SKIN.material.master_alpha = 1;
  {
    const ma = $("#mat-ma");
    ma.value = Math.round(SKIN.material.master_alpha * 100);
    $("#mat-ma-v").textContent = ma.value + "%";
    ma.oninput = () => {
      SKIN.material.master_alpha = ma.value / 100;
      $("#mat-ma-v").textContent = ma.value + "%";
      preview();
    };
    ma.onchange = saveSkinQuiet;
  }
  // 边框透明度（border_alpha 0-1）：边框色独立控制
  if (SKIN.material.border_alpha == null) SKIN.material.border_alpha = 1;
  {
    const ba = $("#mat-ba");
    ba.value = Math.round(SKIN.material.border_alpha * 100);
    $("#mat-ba-v").textContent = ba.value + "%";
    ba.oninput = () => {
      SKIN.material.border_alpha = ba.value / 100;
      $("#mat-ba-v").textContent = ba.value + "%";
      preview();
    };
    ba.onchange = saveSkinQuiet;
  }
  // 阴影透明度（shadow_alpha 0-1）：候选窗投影独立控制
  if (SKIN.material.shadow_alpha == null) SKIN.material.shadow_alpha = 1;
  {
    const sa = $("#mat-sa");
    sa.value = Math.round(SKIN.material.shadow_alpha * 100);
    $("#mat-sa-v").textContent = sa.value + "%";
    sa.oninput = () => {
      SKIN.material.shadow_alpha = sa.value / 100;
      $("#mat-sa-v").textContent = sa.value + "%";
      preview();
    };
    sa.onchange = saveSkinQuiet;
  }
  // 高亮色（写 colors.hilited_candidate_back_color——已从颜色区移到这里，
  // 与高亮透明度并排）
  {
    const ci = $("#mat-hi");
    const syncHi = () => { ci.value = hexOf(SKIN.colors.hilited_candidate_back_color || "#404046"); };
    ci.oninput = () => { SKIN.colors.hilited_candidate_back_color = ci.value + "FF"; preview(); };
    ci.onchange = saveSkinQuiet;
    syncHi();
  }
  // 高亮透明度（hilite_alpha 0-1）：高亮候选底独立控制
  if (SKIN.material.hilite_alpha == null) SKIN.material.hilite_alpha = 1;
  {
    const ha = $("#mat-ha");
    ha.value = Math.round(SKIN.material.hilite_alpha * 100);
    $("#mat-ha-v").textContent = ha.value + "%";
    ha.oninput = () => {
      SKIN.material.hilite_alpha = ha.value / 100;
      $("#mat-ha-v").textContent = ha.value + "%";
      preview();
    };
    ha.onchange = saveSkinQuiet;
  }
  // 【高亮开关 2026-10-30】hilite_on：关=候选窗完全不画高亮胶囊，
  // 高亮行文字/序号/注释也用普通色（与 DLL 渲染端同语义）。默认开。
  if (SKIN.material.hilite_on == null) SKIN.material.hilite_on = true;
  {
    const hon = $("#mat-hon");
    hon.checked = SKIN.material.hilite_on !== false;
    hon.onchange = () => { SKIN.material.hilite_on = hon.checked; preview(); saveSkinQuiet(); };
  }
  // rng("#mat-bw", ...) 已并入上方边框对齐块（真窗键 layout.border_width）
  $("#l-horiz").checked = SKIN.layout.horizontal; $("#l-horiz").onchange = e => { SKIN.layout.horizontal = e.target.checked; preview(); saveSkinQuiet(); msg(e.target.checked ? "已切换横排并保存皮肤 ✓（下次打字生效）" : "已切换竖排并保存皮肤 ✓", "ok"); };
  $("#l-inline").checked = SKIN.layout.inline_preedit; $("#l-inline").onchange = e => { SKIN.layout.inline_preedit = e.target.checked; saveSkinQuiet(); };
  // 【2026-09-06 实时生效】布局滑杆（阴影/圆角/间距/字级/固定宽等）
  // 此前只改内存+预览图、无任何保存路径（用户实测「阴影大小不生效」
  // 根因）——现拖动防抖 400ms 自动存皮肤；server 保存即 bump 皮肤
  // 版本号，DLL poll（40ms）发现版本变化强制重拉（绕 2.5s 缓存），
  // 连续调参即时生效（2026-09-08 用户实测「反应慢」的根修）。
  let _skinSaveTimer = null;
  const saveSkinDebounced = () => {
    clearTimeout(_skinSaveTimer);
    _skinSaveTimer = setTimeout(() => { saveSkinQuiet(); }, 400);
  };
  const lrng = (id, key) => { const el = $(id); el.value = SKIN.layout[key]; $(id + "-v").textContent = fmtV(SKIN.layout[key]); el.oninput = () => { SKIN.layout[key] = parseFloat(el.value); $(id + "-v").textContent = fmtV(el.value); preview(); saveSkinDebounced(); }; };
  lrng("#l-radius", "corner_radius"); lrng("#l-hr", "hilited_corner_radius");
  // 【皮肤元素自查 2026-09-08】序号格式（label_format，%s→数字）+
  // hilite_spacing（序号↔正文↔注释统一间距，与 DLL 同步激活）
  // 【序号开关 2026-09-08】皮肤页开关绑全局 config.candidates.show_index
  //（与「输入与候选」页同一真源；保存走 /api/config，DLL ≤2.5s 经
  // skin 响应附带生效）
  {
    const si = $("#sk-idx");
    si.checked = CFG.candidates.show_index !== false;
    si.onchange = async () => {
      CFG.candidates.show_index = si.checked;
      try {
        await saveConfig();
        preview();
        msg(si.checked ? "候选序号已开启 ✓" : "候选序号已隐藏 ✓", "ok");
      } catch (e) { msg("保存失败：" + e.message, "err"); }
    };
  }
  {
    const lf = $("#l-lfmt");
    lf.value = SKIN.layout.label_format ?? "%s.";
    lf.onchange = () => {
      const v = lf.value.trim();
      SKIN.layout.label_format = v.includes("%s") ? v : "%s.";
      lf.value = SKIN.layout.label_format;
      preview(); saveSkinQuiet();
    };
  }
  // 【序号样式 2026-09-08】digit/zh/roman——%s 的字形（数字/中文/罗马），
  // 与格式模板独立（样式管字形，模板管点/间距装饰）。DLL fmt_label
  // 同步映射（第 10 候选：digit=0 与选重键一致；zh=十；roman=Ⅹ）。
  {
    const ls = $("#l-lstyle");
    ls.value = ["digit", "zh", "roman"].includes(SKIN.layout.label_style) ? SKIN.layout.label_style : "digit";
    ls.onchange = () => { SKIN.layout.label_style = ls.value; preview(); saveSkinQuiet(); };
  }
  lrng("#l-hsp", "hilite_spacing");
  // 留白滑杆：毛玻璃退役后单一数据源 layout.margin_x/y
  {
    const mx = $("#l-mx"), my = $("#l-my"), mxv = $("#l-mx-v"), myv = $("#l-my-v");
    const syncMarginSliders = () => {
      mx.value = SKIN.layout.margin_x ?? 8; my.value = SKIN.layout.margin_y ?? 6;
      mxv.textContent = fmtV(mx.value); myv.textContent = fmtV(my.value);
    };
    mx.oninput = () => { SKIN.layout.margin_x = parseFloat(mx.value); mxv.textContent = fmtV(mx.value); preview(); saveSkinDebounced(); };
    my.oninput = () => { SKIN.layout.margin_y = parseFloat(my.value); myv.textContent = fmtV(my.value); preview(); saveSkinDebounced(); };
    syncMarginSliders();
  }
  lrng("#l-hp", "hilite_padding");
  lrng("#l-cs", "candidate_spacing"); lrng("#l-ls", "line_spacing");
  lrng("#l-lfp", "label_font_point"); lrng("#l-w", "width");
  lrng("#l-sr", "shadow_radius"); lrng("#l-sox", "shadow_offset_x"); lrng("#l-soy", "shadow_offset_y");
  hookValFields(); // 纯数值滑杆挂「两位小数+点击输入」
  // 【2026-09-06 用户规格】「竖排候选」开关从「输入与候选」页移除——
  // 只保留皮肤页「横排候选」（同一皮肤 layout.horizontal，取反即竖排）。
  // 候选字体/字号 → 皮肤 layout（真实候选窗读皮肤），改动即存
  const af = $("#ap-font"), asz = $("#ap-size");
  af.value = SKIN.layout.font_face || "";
  asz.value = SKIN.layout.font_point;
  const saveSkinNow = async () => {
    try { await saveSkin(); msg("外观已存入皮肤 ✓", "ok"); } catch (e) { msg("皮肤保存失败：" + e.message, "err"); }
  };
  af.onchange = () => { SKIN.layout.font_face = af.value.trim(); saveSkinNow(); };
  // 【序号字级联动 2026-09-08】字号变化时若联动开启，序号字级按原比例
  // 一起变（与滚轮缩放行为一致）；皮肤页只改字号不联动会导致序号
  // 字号失衡（用户实测「候选不好看」）。
  {
    const lk = $("#l-llock");
    lk.checked = SKIN.layout.label_size_lock !== false;
    lk.onchange = () => { SKIN.layout.label_size_lock = lk.checked; saveSkinQuiet(); };
  }
  asz.onchange = () => {
    const v = parseFloat(asz.value);
    if (v >= 8 && v <= 60) {
      const oldF = SKIN.layout.font_point || 14.5;
      if (SKIN.layout.label_size_lock !== false && oldF > 0) {
        const ratio = (SKIN.layout.label_font_point ?? oldF * 0.75) / oldF;
        const nl = Math.round(v * ratio * 2) / 2;   // 0.5 步进
        if (nl >= 6 && nl <= 40) {
          SKIN.layout.label_font_point = nl;
          const el = $("#l-lfp"); if (el) el.value = nl;
          const ev = $("#l-lfp-v"); if (ev) ev.textContent = fmtV(nl);
        }
      }
      // 【比例联动 2026-09-08】几何参数同比例放大（与滚轮缩放同一语义，
      // 见 hufu_skin::Layout::scale_geometry）——只放字不放垫会「字大
      // 垫小」放大不好看。联动后刷新对应滑杆显示。
      if (oldF > 0) {
        const r = v / oldF;
        if (r > 0.95 && r < 1.05) { /* 忽略微调 */ }
        else {
          const geo = [["corner_radius","#l-radius",40],["hilited_corner_radius","#l-hr",40],
                       ["margin_x","#l-mx",60],["margin_y","#l-my",60],
                       ["spacing",null,24],["candidate_spacing","#l-cs",48],
                       ["hilite_spacing","#l-hsp",16],["hilite_padding","#l-hp",40],
                       ["line_spacing","#l-ls",40],["border_width",null,12],
                       ["min_width",null,800]];
          for (const [k, sel, hi] of geo) {
            const nv = Math.round((SKIN.layout[k] || 0) * r * 2) / 2;
            SKIN.layout[k] = Math.min(Math.max(nv, 0), hi);
            if (sel) { const e = $(sel); if (e) e.value = SKIN.layout[k]; const t = $(sel + "-v"); if (t) t.textContent = fmtV(SKIN.layout[k]); }
          }
        }
      }
      SKIN.layout.font_point = v;
      preview(); saveSkinNow();
    }
  };

  /* 【2026-09-06 用户规格】三个皮肤页新能力：
     ① 预览背景深/浅切换（默认深）② 每皮肤「恢复默认值」③ 实机试打框 */
  // ① 深/浅底切换（浅色底内联覆盖，默认深=样式表原样）
  {
    const wrap = document.querySelector(".skin-preview-wrap");
    const bd = $("#pv-dark"), bl = $("#pv-light");
    const apply = light => {
      wrap.style.background = light ? "#E9E9EE" : "";
      bd.classList.toggle("on", !light);
      bl.classList.toggle("on", light);
    };
    bd.onclick = () => apply(false);
    bl.onclick = () => apply(true);
  }
  // ② 恢复默认值：颜色/布局/材质回出厂（保留皮肤 ID/名称/作者）
  {
    /* 【修复标签 2026-09-11】前端默认对齐 Rust 真值：Layout::default
       （lib.rs:302/305）margin_y=6.0、hilite_spacing=4.0（原 5/2 与渲染
       默认不一致）；MaterialConfig::default（lib.rs:140）补 glass_alpha=0.0 */
    const F = {
      layout: { horizontal: false, inline_preedit: true, font_face: "", font_point: 14.5, label_font_point: 11.5, label_format: "%s.", label_style: "digit", corner_radius: 8, hilited_corner_radius: 6, border_width: 1, margin_x: 8, margin_y: 6, spacing: 6, candidate_spacing: 4, hilite_spacing: 4, hilite_padding: 4, line_spacing: 3, min_width: 120, width: 250, shadow_radius: 12, shadow_offset_x: 0, shadow_offset_y: 0, blur_level: "low", mark_text: "·" },
      colors: { back_color: "#202022E6", border_color: "#FFFFFF26", text_color: "#E8E8EAFF", preedit_back_color: "#20202200", candidate_text_color: "#E8E8EAFF", candidate_back_color: "#20202200", candidate_shadow_color: "#00000000", comment_text_color: "#9A9AA0FF", label_color: "#C9C9C9FF", hilited_text_color: "#FFFFFFFF", hilited_back_color: "#20202200", hilited_candidate_text_color: "#FFFFFFFF", hilited_candidate_back_color: "#404046FF", hilited_candidate_label_color: "#FFD75EFF", hilited_comment_text_color: "#C9C9C9FF", hilited_label_color: "#FFD75EFF", hilited_mark_color: "#FFD75EFF", shadow_color: "#00000059", hilited_shadow_color: "#00000059", hilited_candidate_shadow_color: "#00000000", capsule_text_color: "#E8E8EAFF", capsule_back_color: "#202022CC" },
      material: { kind: "solid", tint: "#1C1C1E00", opacity: 1, darken: 0, noise: 0, border_width: 1, border_color: "#FFFFFF33", master_alpha: 1, hilite_alpha: 1, shadow_alpha: 1, border_alpha: 1, glass_alpha: 0, glass_shadow_alpha: 0.38, glass_shadow_size: 6, glass_margin_x: 4, glass_margin_y: 4 },
    };
    $("#skin-reset").onclick = async () => {
      try {
        // 【每皮肤恢复默认 2026-09-08】官方皮肤（hufu-*）恢复各自
        // 出厂配置（server 内嵌 official-skins 整文件写回——各皮肤
        // 保留自己的配色/布局个性）；用户自建皮肤（非官方 id）回退
        // 本地统一出厂常量（保留 id/名称）。
        const isOfficial = /^(hufu-[a-z]+)$/.test(SKIN.id || "");
        if (isOfficial) {
          const fac = await api("POST", "/api/skin/reset", { id: SKIN.id });
          if (fac && fac.id) {
            SKIN.layout = fac.layout; SKIN.colors = fac.colors; SKIN.material = fac.material;
          } else {
            SKIN.layout = JSON.parse(JSON.stringify(F.layout));
            SKIN.colors = JSON.parse(JSON.stringify(F.colors));
            SKIN.material = JSON.parse(JSON.stringify(F.material));
          }
        } else {
          SKIN.layout = JSON.parse(JSON.stringify(F.layout));
          SKIN.colors = JSON.parse(JSON.stringify(F.colors));
          SKIN.material = JSON.parse(JSON.stringify(F.material));
        }
        bindSkin(); buildColorGrid(); preview();
        await saveSkin();
        msg(isOfficial
          ? `已恢复「${SKIN.name || SKIN.id}」的出厂配置并保存生效 ✓`
          : "自建皮肤：已恢复为统一默认布局/配色（ID/名称保留）✓", "ok");
      } catch (e) { msg("恢复默认失败：" + e.message, "err"); }
    };
  }
}
function preview() {
  const c = SKIN.colors, L = SKIN.layout, M = SKIN.material;
  const w = $("#candwin-preview");
  w.className = "preview-wrap";
  // 【纯色模型 v2·用户定稿】颜色只管色相（alpha 分量忽略）：
  // · 窗底/边框/编码底/阴影 alpha = 整体透明度滑条
  // · 高亮底 alpha = 高亮透明度滑条（独立）
  // · 文字（编码/序号/候选/注释）一律 100% 不透明
  const ma = Math.max(0, Math.min(1, M.master_alpha ?? 1));
  const ha = Math.max(0, Math.min(1, M.hilite_alpha ?? 1));
  const sAlpha = Math.max(0, Math.min(1, M.shadow_alpha ?? 1));
  const bAlpha = Math.max(0, Math.min(1, M.border_alpha ?? 1));
  /* 【修复标签 2026-09-11】XSS：色值出自皮肤文件（用户可改 JSON），经
     innerHTML 的 style 属性插值——统一 esc()（合法 #RRGGBBAA 不受影响） */
  const elemA = hex => esc(withA(hex.slice(0, 7), Math.round(ma * 255)));
  const hiA = hex => esc(withA(hex.slice(0, 7), Math.round(ha * 255)));
  const shA = hex => esc(withA(hex.slice(0, 7), Math.round(sAlpha * 255))); // 阴影独立
  const bdA = hex => esc(withA(hex.slice(0, 7), Math.round(bAlpha * 255))); // 边框独立
  const solid = hex => esc(hex.slice(0, 7)); // 文字恒不透明
  // 边框预览 = 真窗消费键（elemA 只处理 hex 色值——先定义后使用，防 TDZ）
  const border = L.border_width > 0 ? `${L.border_width}px solid ${bdA(c.border_color)}` : "none";
  const bgc = elemA(c.back_color);
  // 棋盘格底透出透明度；四角真实透明（pre 视觉≈真窗圆角）
  // 默认底：外层光斑（衬托透明感）；开启真实屏幕背景后 video 铺底，
  // 预览窗透出的是真实屏幕内容——所见即所得
  const liveOn = !$("#livebg").style.display.includes("none");
  w.style.cssText = "position:relative;z-index:1;border:none;padding:0;background:transparent";
  const inner = document.createElement("div");
  w.innerHTML = "";
  inner.className = "candwin" + (L.horizontal ? " horizontal" : "");
  // 【毛玻璃退役 2026-09-11】预览不再有 glass 分支（backdrop-filter/
  // tint 层/玻璃留白全撤），底色/留白单一来源。
  const bgPart = `background:${bgc};`;
  const padY = L.margin_y;
  const padX = L.margin_x;
  inner.style.cssText = `${bgPart}border:${border};border-radius:${L.corner_radius}px;box-shadow:0 ${L.shadow_offset_y || 0}px ${SKIN.layout.shadow_radius * 2 || 8}px ${shA(c.shadow_color)};padding:${padY}px ${padX}px;gap:${L.candidate_spacing}px`;
  // 【皮肤元素自查 2026-09-08】预览消费 label_format（序号 printf 格式，
  // %s→序号；与 DLL 同步——出厂统一 "%s."，皮肤可改 "%s" 得纯数字）
  // 与 mark_text（高亮行左缘细竖条，weasel 语义）。
  // 【10 选序号】第 10 候选显示 0（1234567890，与引擎 0=10 选重一致）。
  // 【序号开关】皮肤页开关与「输入与候选」页同一真源（config.
  // candidates.show_index），切换即存全局配置（DLL 经 skin 响应附带
  // 读取，≤2.5s 生效），预览同步隐藏/显示序号列。
  // 【序号样式】与 DLL fmt_label 同映射：digit（第10=0）/zh（十）/roman（Ⅹ）
  const lblDigits = n => {
    const k = n === 10 ? 10 : n;
    if (L.label_style === "zh") return ["一","二","三","四","五","六","七","八","九","十"][k - 1];
    if (L.label_style === "roman") return ["Ⅰ","Ⅱ","Ⅲ","Ⅳ","Ⅴ","Ⅵ","Ⅶ","Ⅷ","Ⅸ","Ⅹ"][k - 1];
    return k === 10 ? 0 : k;
  };
  const fmtLbl = n => { const f = L.label_format || "%s."; const d = lblDigits(n); return f.includes("%s") ? f.replace("%s", d) : d + "."; };
  const cand = (i, t, cmt, hi) => `<div class="cand-line" style="${hi && M.hilite_on !== false ? `background:${hiA(c.hilited_candidate_back_color)};border-radius:${esc(L.hilited_corner_radius)}px;padding:${esc(L.hilite_padding)}px;` : `padding:${esc(L.hilite_padding)}px 1px;`}color:${solid(hi && M.hilite_on !== false ? c.hilited_candidate_text_color : c.candidate_text_color)}">${hi && M.hilite_on !== false && (L.mark_text || "").trim() ? `<span style="display:inline-block;width:2px;align-self:stretch;margin:-2px 4px 0 -3px;border-radius:1px;background:${solid(c.hilited_candidate_label_color)}"></span>` : ""}${CFG.candidates.show_index !== false ? `<span class="idx" style="color:${solid(hi && M.hilite_on !== false ? c.hilited_candidate_label_color : c.label_color)}">${esc(fmtLbl(i))}</span>` : ""}${esc(t)}${cmt ? ` <span class="cmt" style="color:${solid(hi && M.hilite_on !== false ? c.hilited_comment_text_color : c.comment_text_color)}">${esc(cmt)}</span>` : ""}</div>`;
  inner.innerHTML =
    `<div class="preedit-line" style="color:${solid(c.text_color)};background:${elemA(c.preedit_back_color)};border-radius:4px">tuja</div>` +
    cand(1, "我们", "wǒ men", true) + cand(2, "我", "wǒ") + cand(3, "War", "") + cand(4, "我军", "");
  w.appendChild(inner);
  $("#material-note").textContent = `纯色 · 整体透明度 ${Math.round(ma * 100)}% · 高亮${M.hilite_on === false ? "关" : `开 · 高亮透明度 ${Math.round(ha * 100)}%`} · 文字恒不透明`;
}
/* ── 真实屏幕背景：把实际屏幕作为预览底，透明度所见即所得 ── */
{
  const btn = () => $("#livebg-btn");
  let stream = null;
  document.addEventListener("DOMContentLoaded", () => {
    btn().onclick = async () => {
      const v = $("#livebg");
      if (stream) { // 再点一次关闭
        stream.getTracks().forEach(t => t.stop()); stream = null;
        v.style.display = "none"; v.srcObject = null;
        btn().textContent = "真实屏幕背景"; preview(); return;
      }
      try {
        stream = await navigator.mediaDevices.getDisplayMedia({ video: true, audio: false });
        v.srcObject = stream; v.style.display = "block";
        btn().textContent = "关闭真实背景";
        stream.getVideoTracks()[0].addEventListener("ended", () => {
          stream = null; v.style.display = "none"; v.srcObject = null;
          btn().textContent = "真实屏幕背景"; preview();
        });
        preview();
      } catch (e) { msg("未授权屏幕捕获，保持渐变底：" + e.message, "err"); }
    };
  });
}
async function loadSkinList(sel) {
  const r = await api("GET", "/api/skins");
  const s = $("#skin-select"); s.innerHTML = "";
  r.skins.forEach(x => { const o = document.createElement("option"); o.value = x.id; o.textContent = x.name; s.appendChild(o); });
  if (sel || r.current) s.value = sel || r.current;
  s.onchange = () => loadSkin(s.value);
}
async function loadSkin(id) {
  SKIN = await api("GET", "/api/skin?id=" + encodeURIComponent(id));
  bindSkin(); buildColorGrid(); ovBindSkin();
  // 选中即切换为当前皮肤（select 端点不写皮肤文件，避免覆盖）
  try { await api("POST", "/api/skin/select", { id }); } catch (e) { msg("切换当前皮肤失败：" + e.message, "err"); }
  // 双保险重绘：同步一次 + 下一帧再一次（个别浏览器 select 切换后
  // 首次重绘被合并掉——用户报「换皮肤预览不更新、动一下才变」）
  preview();
  requestAnimationFrame(() => preview());
}

/* ════ 挂件（候选窗贴图，兄弟窗）2026-10-02 ════
   数据流：原图 →（裁剪/抠图/圆角/羽化 全客户端 canvas）→ 预览实时；
   「烤成品」把处理结果烤成 PNG data-URL 写进 SKIN.overlay.image 走
   既有 saveSkin 通道（server bump skin_ver → DLL 秒级重拉）。
   贴法参数（side/height/gap/offset/layer/opacity/v_align/flip_h）不进
   像素管线，改动即存即生效（与皮肤滑杆同款防抖）。 */
const OV = {
  img: null, name: "", basedOnBaked: false,
  dataUrl: "", mime: "",   // 原文件 data-URL（动图烤成品走「原图+参数」路径）
  anim: null,              // {bitmaps:[ImageBitmap], delays:[ms]} 动图帧
  crop: null,              // {x,y,w,h} 源图像素；null=全图
  ratio: "free",           // free | 1 | 2:3 | 3:4 | 9:16
  key: null, tol: 30,      // 抠图键色 + 容差
  corner: 0, feather: 0,   // 圆角/羽化（240px 高基准，运行时按实际高折算）
  picking: false,          // 「取背景色」一次性取样模式
  drag: null, view: null,  // 处理台拖拽状态 / 源图→画布映射
  raf: 0, animIdx: 0, animLast: 0,
};
const ovEnsureCfg = () => {
  if (!SKIN.overlay) SKIN.overlay = { enabled: false, image: "", side: "right", base_height: 240, offset_x: 0, offset_y: 0, gap: 8, flip_h: false, layer: "below", opacity: 100, v_align: "center" };
  return SKIN.overlay;
};
let _ovSaveT = null;
const ovSaveDeb = () => { clearTimeout(_ovSaveT); _ovSaveT = setTimeout(() => { saveSkin().catch(e => msg("皮肤保存失败：" + e.message, "err")); }, 400); };
const ovSetR = (id, v) => { const el = $(id); if (el) el.value = v; const lab = $(id + "-v"); if (lab) lab.textContent = v; };

function ovBindSkin() {
  const c = ovEnsureCfg();
  $("#ov-en").checked = !!c.enabled;
  $("#ov-side").value = c.side === "left" ? "left" : "right";
  $("#ov-layer").value = c.layer === "above" ? "above" : "below";
  $("#ov-valign").value = ["center", "top", "bottom"].includes(c.v_align) ? c.v_align : "center";
  ovSetR("#ov-bh", c.base_height || 240); ovSetR("#ov-gap", c.gap ?? 8);
  ovSetR("#ov-op", c.opacity ?? 100);
  ovSetR("#ov-tol", OV.tol); ovSetR("#ov-corner", OV.corner); ovSetR("#ov-feather", OV.feather);
  $("#ov-flip").checked = !!c.flip_h;
  // 皮肤里已有挂件 → 载入：动图(proc 节)=原图+参数；静态=烤成品 PNG
  if (c.image && !OV.img) {
    if (c.proc) {
      const p = c.proc;
      if (p.crop && p.crop[2] >= 2) OV.crop = { x: p.crop[0], y: p.crop[1], w: p.crop[2], h: p.crop[3] };
      OV.key = p.key ? { r: p.key[0], g: p.key[1], b: p.key[2] } : null;
      OV.tol = p.tol ?? 30; OV.corner = p.corner ?? 0; OV.feather = p.feather ?? 0;
      ovSetR("#ov-tol", OV.tol); ovSetR("#ov-corner", OV.corner); ovSetR("#ov-feather", OV.feather);
      ovLoadFromDataUrl(c.image, "（当前动图+参数）", true);
    } else {
      ovLoadFromDataUrl(c.image, "（当前成品图）", true);
    }
  }
  ovMock(); ovPreview();
}
/* 载入 data-URL：统一走「解码→动图探测→首帧/帧序列」 */
function ovLoadFromDataUrl(url, label, baked) {
  const img = new Image();
  img.onload = async () => {
    OV.img = img; OV.name = label; OV.basedOnBaked = !!baked; OV.dataUrl = url;
    OV.anim = null; OV.animIdx = 0;
    // 动图探测（Chromium ImageDecoder；不支持则静态首帧回退）
    let anim = null;
    if (typeof ImageDecoder !== "undefined") {
      try {
        const buf = await (await fetch(url)).arrayBuffer();
        const mime = url.slice(5, url.indexOf(";")) || "image/gif";
        const dec = new ImageDecoder({ data: buf, type: mime });
        await dec.tracks.ready;
        const tr = dec.tracks.selectedTrack;
        if (tr && tr.frameCount > 1) {
          const n = Math.min(tr.frameCount, 120);
          const bitmaps = [], delays = [];
          let lastTs = 0;
          for (let i = 0; i < n; i++) {
            const r = await dec.decode({ frameIndex: i });
            bitmaps.push(await createImageBitmap(r.image));
            const ms = i === 0 ? 100 : Math.round((r.image.timestamp - lastTs) / 1000);
            delays.push(Math.min(1000, Math.max(20, ms || 100)));
            lastTs = r.image.timestamp;
            r.image.close();
          }
          anim = { bitmaps, delays };
        }
      } catch (e) { /* 解码失败→静态回退 */ }
    }
    OV.anim = anim;
    $("#ov-fileinfo").textContent = `${label} ${img.naturalWidth}×${img.naturalHeight}` + (anim ? `（动图 ${anim.bitmaps.length} 帧）` : "");
    const badge = $("#ov-anim-badge"); if (badge) badge.style.display = anim ? "inline" : "none";
    ovRenderWork(); ovPreview();
    if (anim) ovAnimTick(0);
  };
  img.src = url;
}
/* 预览动图节拍：按帧时长推进，重绘走 ovPreview（含像素管线） */
function ovAnimTick(ts) {
  if (!OV.anim) return;
  if (!ts || ts - OV.animLast >= OV.anim.delays[OV.animIdx]) {
    OV.animLast = ts || 0;
    OV.animIdx = OV.anim ? (OV.animIdx + 1) % OV.anim.bitmaps.length : 0;
    ovPreview();
  }
  requestAnimationFrame(ovAnimTick);
}
/* 处理管线：裁剪 → 翻转 → 抠图(软边) → 圆角/羽化(SDF) → 输出到 cv。
   srcFrame=动图当前帧（ImageBitmap，尺寸与源图一致）；缺省=静态源图 */
function ovProcess(cv, maxH, srcFrame) {
  const img = OV.img; if (!img) return false;
  const draw = srcFrame || img;
  const c0 = OV.crop;
  const sx = c0 ? c0.x : 0, sy = c0 ? c0.y : 0;
  const sw = c0 ? c0.w : img.naturalWidth, sh = c0 ? c0.h : img.naturalHeight;
  if (sw < 2 || sh < 2) return false;
  const k = Math.min(1, maxH / Math.max(sw, sh));
  const w = Math.max(1, Math.round(sw * k)), h = Math.max(1, Math.round(sh * k));
  cv.width = w; cv.height = h;
  const ctx = cv.getContext("2d", { willReadFrequently: true });
  ctx.clearRect(0, 0, w, h);
  if (ovEnsureCfg().flip_h) { ctx.translate(w, 0); ctx.scale(-1, 1); }
  ctx.drawImage(draw, sx, sy, sw, sh, 0, 0, w, h);
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  if (!(OV.key || OV.corner > 0 || OV.feather > 0)) return true;
  const id = ctx.getImageData(0, 0, w, h), d = id.data;
  const key = OV.key, tol = OV.tol, soft = Math.max(6, tol * 0.35);
  // 圆角/羽化按 240px 基准折算到当前输出高；SDF 半径取 max(圆角, 羽化)——
  // 羽化自带圆润弧（纯羽化=软圆角矩形），圆角>羽化时羽化沿弧带渐隐
  const rr = Math.max(OV.corner, OV.feather) * (h / 240);
  const hard = OV.corner * (h / 240);
  const f = OV.feather * (h / 240);
  const halfW = w / 2, halfH = h / 2;
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const i = (y * w + x) * 4;
      if (d[i + 3] === 0) continue;
      let a = d[i + 3];
      if (key) {
        const dist = Math.max(Math.abs(d[i] - key.r), Math.abs(d[i + 1] - key.g), Math.abs(d[i + 2] - key.b));
        if (dist <= tol) { d[i + 3] = 0; continue; }
        if (dist < tol + soft) a = a * (dist - tol) / soft;
      }
      if (rr > 0) {
        const qx = Math.abs(x - halfW + 0.5) - halfW + rr;
        const qy = Math.abs(y - halfH + 0.5) - halfH + rr;
        const qxo = Math.max(qx, 0), qyo = Math.max(qy, 0);
        const sd = Math.min(Math.max(qx, qy), 0) + Math.sqrt(qxo * qxo + qyo * qyo) - rr;
        if (hard > 0 && sd >= 0) { d[i + 3] = 0; continue; }
        if (f > 0) { let t = -sd / f; t = t < 0 ? 0 : t > 1 ? 1 : t; a *= t; }
      }
      d[i + 3] = a;
    }
  }
  ctx.putImageData(id, 0, 0);
  return true;
}
/* 处理台：源图适配绘制 + 裁剪框叠加 */
function ovRenderWork() {
  const cv = $("#ov-work"), ctx = cv.getContext("2d");
  const W = cv.width, H = cv.height;
  ctx.clearRect(0, 0, W, H);
  if (!OV.img) { ctx.fillStyle = "#9a9aa0"; ctx.font = "13px sans-serif"; ctx.fillText("点「选择图片…」载入", 60, H / 2); OV.view = null; return; }
  const iw = OV.img.naturalWidth, ih = OV.img.naturalHeight;
  const k = Math.min((W - 8) / iw, (H - 8) / ih);
  const dw = iw * k, dh = ih * k, ox = (W - dw) / 2, oy = (H - dh) / 2;
  OV.view = { ox, oy, k, dw, dh };
  ctx.drawImage(OV.img, ox, oy, dw, dh);
  // 裁剪框外压暗 + 框线
  const c = OV.crop;
  const rx = c ? ox + c.x * k : ox, ry = c ? oy + c.y * k : oy;
  const rw = c ? c.w * k : dw, rh = c ? c.h * k : dh;
  ctx.save();
  ctx.fillStyle = "rgba(0,0,0,0.55)";
  ctx.beginPath();
  ctx.rect(ox, oy, dw, dh);
  ctx.rect(rx, ry, rw, rh);
  ctx.fill("evenodd");
  ctx.strokeStyle = "#ffd75e"; ctx.lineWidth = 1.5;
  ctx.strokeRect(rx + 0.5, ry + 0.5, rw, rh);
  ctx.restore();
}
/* 布局应用（纯 CSS，拖拽热路径零像素重渲染）：
   高度=base_height（候选框 mock 即真实尺寸，所见即所得）、
   间距/水平微调→左右 margin、垂直微调→translateY、透明度→opacity */
function ovApplyLayout() {
  const wrapEl = $("#ov-imgwrap"), cfg = SKIN.overlay || {};
  const row = $("#ov-row"), mock = $("#ov-candmock");
  if (!OV.img) { wrapEl.style.display = "none"; return; }
  wrapEl.style.display = "";
  wrapEl.style.opacity = Math.max(0.05, (cfg.opacity ?? 100) / 100);
  // DOM 序：右贴=[候选框, 图]；左贴=[图, 候选框]
  if (cfg.side === "left") { row.appendChild(wrapEl); row.appendChild(mock); }
  else { row.appendChild(mock); row.appendChild(wrapEl); }
  const gx = cfg.offset_x ?? 0;
  wrapEl.style.marginLeft = (cfg.side === "left" ? gx : (cfg.gap ?? 8) + gx) + "px";
  wrapEl.style.marginRight = (cfg.side === "right" ? gx : (cfg.gap ?? 8) + gx) + "px";
  wrapEl.style.height = (cfg.base_height || 240) + "px";
  wrapEl.style.transform = "translateY(" + (cfg.offset_y ?? 0) + "px)";
  row.style.alignItems = cfg.v_align === "top" ? "flex-start" : cfg.v_align === "bottom" ? "flex-end" : "center";
}
/* 预览：重渲染像素 + 套布局；拖拽中只走 ovApplyLayout 不重渲。
   动图时渲染当前帧（ovAnimTick 节拍驱动） */
function ovPreview() {
  const cv = $("#ov-preview");
  const frame = OV.anim ? OV.anim.bitmaps[OV.animIdx % OV.anim.bitmaps.length] : null;
  const ok = OV.img && ovProcess(cv, 560, frame);
  if (!ok) $("#ov-imgwrap").style.display = "none";
  ovApplyLayout();
}
/* 模拟候选框：按当前皮肤颜色拼一个静态框（与皮肤页预览同源简化版） */
function ovMock() {
  if (!SKIN) return;
  const c = SKIN.colors, L = SKIN.layout;
  const cut = h => esc(String(h || "").slice(0, 7));
  const m = $("#ov-candmock");
  m.className = "candwin";
  m.style.cssText = `background:${cut(c.back_color)};border:${(L.border_width ?? 1) > 0 ? (L.border_width ?? 1) + "px solid " + cut(c.border_color) : "none"};border-radius:${L.corner_radius ?? 8}px;padding:${L.margin_y ?? 6}px ${L.margin_x ?? 8}px;display:flex;flex-direction:column;gap:${L.candidate_spacing ?? 4}px;box-shadow:0 4px 14px rgba(0,0,0,0.3)`;
  m.innerHTML =
    `<div class="preedit-line" style="color:${cut(c.text_color)};background:${cut(c.preedit_back_color)};border-radius:4px">tuja</div>` +
    `<div class="cand-line" style="background:${cut(c.hilited_candidate_back_color)};border-radius:${L.hilited_corner_radius ?? 6}px;padding:${L.hilite_padding ?? 4}px;color:${cut(c.hilited_candidate_text_color)}"><span class="idx" style="color:${cut(c.hilited_candidate_label_color)}">1.</span>我们 <span class="cmt" style="color:${cut(c.hilited_comment_text_color)}">wǒ men</span></div>` +
    `<div class="cand-line" style="padding:${L.hilite_padding ?? 4}px 1px;color:${cut(c.candidate_text_color)}"><span class="idx" style="color:${cut(c.label_color)}">2.</span>我 <span class="cmt" style="color:${cut(c.comment_text_color)}">wǒ</span></div>`;
}
/* ── 挂件事件绑定（一次） ── */
$("#ov-pick").onclick = () => $("#ov-file").click();
$("#ov-file").onchange = ev => {
  const f = ev.target.files[0]; if (!f) return;
  const rd = new FileReader();
  rd.onload = () => ovLoadFromDataUrl(rd.result, f.name, false);
  rd.onerror = () => msg("图片读取失败", "err");
  rd.readAsDataURL(f);
  ev.target.value = "";
};
$("#ov-en").onchange = () => {
  const c = ovEnsureCfg(); c.enabled = $("#ov-en").checked;
  if (c.enabled && !c.image) msg("启用成功——先「烤成品」写入图片才会显示", "");
  else msg(c.enabled ? "挂件已启用 ✓（打字即见，≤2.5s）" : "挂件已关闭", "ok");
  saveSkin().catch(e => msg("保存失败：" + e.message, "err"));
};
const ovLayR = (id, key) => {
  $(id).oninput = () => {
    const c = ovEnsureCfg(); c[key] = parseFloat($(id).value);
    $(id + "-v").textContent = $(id).value;
    ovApplyLayout(); ovSaveDeb();   // 布局参数纯 CSS，不重渲像素
  };
};
ovLayR("#ov-bh", "base_height"); ovLayR("#ov-gap", "gap"); ovLayR("#ov-op", "opacity");
const ovSel = (id, key) => {
  $(id).onchange = () => { const c = ovEnsureCfg(); c[key] = $(id).value; ovApplyLayout(); saveSkin().catch(e => msg("保存失败：" + e.message, "err")); };
};
ovSel("#ov-side", "side"); ovSel("#ov-layer", "layer"); ovSel("#ov-valign", "v_align");
$("#ov-flip").onchange = () => { const c = ovEnsureCfg(); c.flip_h = $("#ov-flip").checked; ovPreview(); saveSkin().catch(e => msg("保存失败：" + e.message, "err")); };
/* 预览直接 manipulation：拖图=定位（自动换贴边/改间距/垂直微调），
   右下手柄或滚轮=改高度；松手/滚轮防抖保存 → skin_ver → DLL 秒级生效 */
{
  const wrapEl = $("#ov-imgwrap"), mock = $("#ov-candmock"), row = $("#ov-row");
  let mode = null, grab = null, start = null;
  wrapEl.addEventListener("pointerdown", e => {
    if (!OV.img) return;
    const c = ovEnsureCfg();
    if (e.target.id === "ov-handle") {
      mode = "resize";
      grab = { y: e.clientY, h: c.base_height || 240 };
    } else {
      mode = "move";
      grab = { x: e.clientX, y: e.clientY };
      // position:relative 偏移：留在 flex 流里（mock 不跳位），视觉任意跟随
      start = null;
      wrapEl.classList.add("ov-dragging");
      wrapEl.style.position = "relative";
      wrapEl.style.left = "0px"; wrapEl.style.top = "0px";
      wrapEl.style.transform = "none";
    }
    wrapEl.setPointerCapture(e.pointerId);
    e.preventDefault();
  });
  wrapEl.addEventListener("pointermove", e => {
    if (!mode) return;
    const c = ovEnsureCfg();
    if (mode === "move") {
      wrapEl.style.left = (e.clientX - grab.x) + "px";
      wrapEl.style.top = (e.clientY - grab.y) + "px";
    } else {
      const nh = Math.round(grab.h + (e.clientY - grab.y));
      c.base_height = Math.min(600, Math.max(60, nh));
      ovSetR("#ov-bh", c.base_height);
      wrapEl.style.height = c.base_height + "px";
    }
    e.preventDefault();
  });
  wrapEl.addEventListener("pointerup", e => {
    if (!mode) return;
    const c = ovEnsureCfg();
    if (mode === "move") {
      // 松手：按最终几何反推 side/gap/offset_y（相对 row 坐标）
      const rr = row.getBoundingClientRect(), mr = mock.getBoundingClientRect(), wr = wrapEl.getBoundingClientRect();
      const left = wr.left - rr.left, top = wr.top - rr.top, w = wr.width, h = wr.height;
      const mockL = mr.left - rr.left, mockR = mr.right - rr.left, mockCx = mockL + mr.width / 2;
      const side = (left + w / 2) < mockCx ? "left" : "right";
      const visualGap = side === "right" ? left - mockR : mockL - (left + w);
      c.side = side; $("#ov-side").value = side;
      c.gap = Math.min(600, Math.max(-400, Math.round(visualGap - (c.offset_x ?? 0))));
      ovSetR("#ov-gap", c.gap);
      const alignedTop = c.v_align === "top" ? mr.top - rr.top
        : c.v_align === "bottom" ? (mr.bottom - rr.top) - h
        : (mr.top - rr.top) + mr.height / 2 - h / 2;
      c.offset_y = Math.min(1000, Math.max(-1000, Math.round(top - alignedTop)));
      ovSetR("#ov-oy", c.offset_y);
      // 回 flex 常规布局
      wrapEl.style.position = ""; wrapEl.style.left = ""; wrapEl.style.top = "";
      ovApplyLayout();
      ovSaveDeb();
    } else {
      ovSaveDeb();
    }
    wrapEl.classList.remove("ov-dragging");
    mode = null;
  });
  wrapEl.addEventListener("wheel", e => {
    if (!OV.img) return;
    e.preventDefault();
    const c = ovEnsureCfg();
    c.base_height = Math.min(600, Math.max(60, (c.base_height || 240) + (e.deltaY < 0 ? 10 : -10)));
    ovSetR("#ov-bh", c.base_height);
    wrapEl.style.height = c.base_height + "px";
    ovSaveDeb();
  }, { passive: false });
}
/* 处理台交互：拖框裁剪 / 取色 */
{
  const cv = $("#ov-work");
  const toSrc = e => {
    if (!OV.view) return null;
    const r = cv.getBoundingClientRect();
    const px = (e.clientX - r.left) * (cv.width / r.width) - OV.view.ox;
    const py = (e.clientY - r.top) * (cv.height / r.height) - OV.view.oy;
    return { x: px / OV.view.k, y: py / OV.view.k };
  };
  const clampRatio = (x0, y0, x1, y1) => {
    let w = x1 - x0, h = y1 - y0;
    if (OV.ratio !== "free") {
      const [a, b] = OV.ratio === "1" ? [1, 1] : OV.ratio.split(":").map(Number);
      const ar = a / b;
      if (Math.abs(w) >= Math.abs(h)) h = Math.sign(h || 1) * Math.abs(w) / ar;
      else w = Math.sign(w || 1) * Math.abs(h) * ar;
    }
    return { x: Math.min(x0, x0 + w), y: Math.min(y0, y0 + h), w: Math.abs(w), h: Math.abs(h) };
  };
  cv.addEventListener("pointerdown", e => {
    if (!OV.img) return;
    const p = toSrc(e); if (!p) return;
    if (OV.picking) { // 取背景色（映射回源图取样，考虑裁剪）
      const c0 = OV.crop;
      const sxp = Math.round((c0 ? c0.x : 0) + p.x), syp = Math.round((c0 ? c0.y : 0) + p.y);
      const tmp = document.createElement("canvas");
      tmp.width = OV.img.naturalWidth; tmp.height = OV.img.naturalHeight;
      const tc = tmp.getContext("2d", { willReadFrequently: true });
      tc.drawImage(OV.img, 0, 0);
      const d = tc.getImageData(sxp, syp, 1, 1).data;
      OV.key = { r: d[0], g: d[1], b: d[2] };
      OV.picking = false; cv.style.cursor = "crosshair";
      msg(`已取背景色 rgb(${d[0]},${d[1]},${d[2]})——调容差看效果`, "ok");
      ovPreview(); return;
    }
    OV.drag = { x0: p.x, y0: p.y };
    cv.setPointerCapture(e.pointerId);
  });
  cv.addEventListener("pointermove", e => {
    if (!OV.drag) return;
    const p = toSrc(e); if (!p) return;
    OV.crop = clampRatio(OV.drag.x0, OV.drag.y0, p.x, p.y);
    ovRenderWork();
    if (!OV.raf) OV.raf = requestAnimationFrame(() => { OV.raf = 0; ovPreview(); });
  });
  cv.addEventListener("pointerup", () => {
    if (!OV.drag) return;
    OV.drag = null;
    if (OV.crop && (OV.crop.w < 4 || OV.crop.h < 4)) OV.crop = null; // 点一下=无框
    ovRenderWork();
  });
}
$("#ov-ratio").onchange = () => { OV.ratio = $("#ov-ratio").value; };
$("#ov-crop-apply").onclick = () => {
  if (!OV.crop) return msg("先在处理台上拖出裁剪框", "err");
  msg(`裁剪已应用（${Math.round(OV.crop.w)}×${Math.round(OV.crop.h)}）——左侧预览同步`, "ok"); ovPreview();
};
$("#ov-crop-reset").onclick = () => { OV.crop = null; ovRenderWork(); ovPreview(); msg("裁剪已还原", "ok"); };
$("#ov-crop-auto").onclick = () => {
  if (!OV.img) return msg("先选择图片", "err");
  const c0 = OV.crop;
  const sx = c0 ? c0.x : 0, sy = c0 ? c0.y : 0, sw = c0 ? c0.w : OV.img.naturalWidth, sh = c0 ? c0.h : OV.img.naturalHeight;
  const tmp = document.createElement("canvas");
  const sc = Math.min(1, 800 / Math.max(sw, sh));
  tmp.width = Math.max(1, Math.round(sw * sc)); tmp.height = Math.max(1, Math.round(sh * sc));
  const tc = tmp.getContext("2d", { willReadFrequently: true });
  tc.drawImage(OV.img, sx, sy, sw, sh, 0, 0, tmp.width, tmp.height);
  const d = tc.getImageData(0, 0, tmp.width, tmp.height).data;
  const px = (x, y) => { const i = (y * tmp.width + x) * 4; return [d[i], d[i + 1], d[i + 2]]; };
  const cs = [px(0, 0), px(tmp.width - 1, 0), px(0, tmp.height - 1), px(tmp.width - 1, tmp.height - 1)];
  const bg = [0, 1, 2].map(k => cs.reduce((s, c) => s + c[k], 0) / 4);
  const T = Math.max(24, OV.tol);
  let x0 = tmp.width, y0 = tmp.height, x1 = -1, y1 = -1;
  for (let y = 0; y < tmp.height; y++) for (let x = 0; x < tmp.width; x++) {
    const p = px(x, y);
    if (Math.max(Math.abs(p[0] - bg[0]), Math.abs(p[1] - bg[1]), Math.abs(p[2] - bg[2])) > T) {
      if (x < x0) x0 = x; if (x > x1) x1 = x; if (y < y0) y0 = y; if (y > y1) y1 = y;
    }
  }
  if (x1 < 0) return msg("没找到内容（背景判断失败，试试调容差）", "err");
  OV.crop = { x: sx + x0 / sc, y: sy + y0 / sc, w: (x1 - x0 + 1) / sc, h: (y1 - y0 + 1) / sc };
  ovRenderWork(); ovPreview();
  msg(`已自动裁到内容（${Math.round(OV.crop.w)}×${Math.round(OV.crop.h)}）`, "ok");
};
$("#ov-key-pick").onclick = () => {
  if (!OV.img) return msg("先选择图片", "err");
  OV.picking = true; $("#ov-work").style.cursor = "cell";
  msg("取色模式：在处理台上点一下背景处", "");
};
$("#ov-key-clear").onclick = () => { OV.key = null; OV.picking = false; $("#ov-work").style.cursor = "crosshair"; ovPreview(); msg("抠图已取消", "ok"); };
const ovFxR = (id, key) => {
  $(id).oninput = () => { OV[key] = parseFloat($(id).value); $(id + "-v").textContent = $(id).value; if (!OV.raf) OV.raf = requestAnimationFrame(() => { OV.raf = 0; ovPreview(); }); };
};
ovFxR("#ov-tol", "tol"); ovFxR("#ov-corner", "corner"); ovFxR("#ov-feather", "feather");
$("#ov-bake").onclick = async () => {
  if (!OV.img) return msg("先选择图片", "err");
  const cfg = ovEnsureCfg();
  cfg.enabled = $("#ov-en").checked;
  if (OV.anim) {
    // 动图：存原图 data-URL + 处理参数（dll 逐帧套同一管线播放），
    // 烤成逐帧 PNG 体积不可行（无压缩编码器），见 overlaywin 路径②
    if ((OV.dataUrl || "").length > 11_000_000) return msg("动图文件过大（>8MB），请精简后再挂", "err");
    cfg.image = OV.dataUrl;
    cfg.proc = {
      crop: OV.crop ? [Math.round(OV.crop.x), Math.round(OV.crop.y), Math.round(OV.crop.w), Math.round(OV.crop.h)] : null,
      key: OV.key ? [Math.round(OV.key.r), Math.round(OV.key.g), Math.round(OV.key.b)] : null,
      tol: OV.tol, corner: OV.corner, feather: OV.feather,
      src_w: OV.img.naturalWidth, src_h: OV.img.naturalHeight,
    };
    try {
      await saveSkin();
      $("#ov-fileinfo").textContent = `${OV.name} → 动图 ${OV.anim.bitmaps.length} 帧（原图+参数）`;
      msg("动图已写入皮肤 ✓ 打字即见逐帧播放（≤2.5s）", "ok");
    } catch (e) { msg("保存失败：" + e.message, "err"); }
    return;
  }
  // 静态图：烤成品 PNG
  const maxH = Math.min(1200, Math.max(240, (cfg.base_height || 240) * 2));
  const cv = document.createElement("canvas");
  if (!ovProcess(cv, maxH)) return msg("处理失败（裁剪区过小？）", "err");
  cfg.image = cv.toDataURL("image/png");
  cfg.proc = null;
  try {
    await saveSkin();
    $("#ov-fileinfo").textContent = `${OV.name} → 成品 ${cv.width}×${cv.height}`;
    msg("成品已写入皮肤 ✓ 打字即见（≤2.5s）", "ok");
  } catch (e) { msg("保存失败：" + e.message, "err"); }
};
{ // 深/浅底切换（与皮肤页同款）
  const wrap = $("#ov-wrap"), bd = $("#ov-pv-dark"), bl = $("#ov-pv-light");
  const apply = light => {
    wrap.style.background = light ? "#E9E9EE" : "";
    bd.classList.toggle("on", !light); bl.classList.toggle("on", light);
  };
  bd.onclick = () => apply(false); bl.onclick = () => apply(true);
}

/* ── 用户词 ── */
$("#uw-list").addEventListener("click", async ev => {
  const b = ev.target.closest("button[data-act]"); if (!b) return;
  const { act, code, text } = b.dataset;
  try {
    if (act === "del") await api("POST", "/api/user_word/remove", { code, text });
    else await api("POST", `/api/candidate/${act}`, { code, text });
    await loadUserWords(); msg(act === "del" ? "已删除" : act === "pin" ? "已置顶" : "已隐藏", "ok");
  } catch (e) { msg(e.message, "err"); }
});

async function loadUserWords() {
  const r = await api("GET", "/api/user_words");
  $("#uw-list").innerHTML = (r.words || []).map(w => `<tr><td>${esc(w.code)}</td><td>${esc(w.text)}</td><td>
    <button data-act="pin" data-code="${esc(w.code)}" data-text="${esc(w.text)}">置顶</button>
    <button data-act="hide" data-code="${esc(w.code)}" data-text="${esc(w.text)}">隐藏</button>
    <button class="danger" data-act="del" data-code="${esc(w.code)}" data-text="${esc(w.text)}">删除</button>
  </td></tr>`).join("") || '<tr><td colspan="3" class="hint">暂无用户词</td></tr>';
}

/* 任意候选调整 */
async function adjCall(op) {
  const code = $("#adj-code").value.trim(), text = $("#adj-text").value.trim();
  if (!code || !text) { msg("编码与词不能为空", "err"); return; }
  try { await api("POST", `/api/candidate/${op}`, { code, text }); msg(op === "pin" ? "已置顶" : "已隐藏", "ok"); }
  catch (e) { msg(e.message, "err"); }
}
$("#adj-pin").onclick = () => adjCall("pin");
$("#adj-hide").onclick = () => adjCall("hide");

/* ── 初始化 ── */
document.querySelectorAll("nav a").forEach(a => a.onclick = () => {
  $$("nav a").forEach(x => x.classList.remove("on")); a.classList.add("on");
  $$("main section").forEach(s => s.classList.remove("on"));
  $("#tab-" + a.dataset.tab).classList.add("on");
  // 【八十六修·全页实时生效】「保存并应用」只在整句模型页显示（权重
  // 调参按用户拍板保留显式保存语义）；其余页全部改动即存（防抖 400ms
  // 自动 saveConfig），与皮肤页同款实时生效——全局按钮在这些页只会
  // 误导「要手动保存」。初始页=方案：按钮默认隐藏（下一行统一置）。
  $("#btn-save").style.display = a.dataset.tab === "sentence" ? "" : "none";
});
$("#btn-save").style.display = "none"; // 初始页（方案）非整句模型——八十六修
$("#btn-schema-refresh").onclick = () => loadSchemas().then(refreshInfo).catch(e => msg(e.message, "err"));
/* 窗口重新聚焦时同步配置快照（托盘切方案后本页不回写旧方案） */
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) api("GET", "/api/config").then(c => { CFG = c; }).catch(() => {});
});

$("#btn-save").onclick = async () => {
  try {
    await saveConfig();
    msg("已保存并应用 ✓", "ok"); await refreshInfo();
  } catch (e) { msg(e.message, "err"); }
};
$("#btn-reload").onclick = async () => { await init(); msg("已还原"); };

$("#s-test-go").onclick = async () => {
  const raw = $("#s-test-in").value.trim(); if (!raw) return;
  try {
    // 先应用当前权重
    await saveConfig();
    const r = await api("POST", "/api/sentence_test", { raw });
    $("#s-test-out").innerHTML = r.candidates.length
      ? r.candidates.slice(0, 8).map((t, i) => (i === 0 ? "<b>" : "") + esc(t) + (i === 0 ? "</b>" : "")).join("　·　")
      : '<span class="hint">无候选（检查模型加载与编码长度 >4）</span>';
    msg("整句试验完成", "ok");
  } catch (e) { $("#s-test-out").textContent = ""; msg(e.message, "err"); }
};

$("#skin-save").onclick = async () => {
  try { const r = await saveSkin(); msg("皮肤已保存 ✓", "ok"); await loadSkinList(SKIN.id); }
  catch (e) { msg(e.message, "err"); }
};
$("#skin-new").onclick = () => {
  SKIN = JSON.parse(JSON.stringify(SKIN));
  SKIN.id = "skin-" + Date.now().toString(36); SKIN.name = "新皮肤";
  bindSkin(); preview(); msg("已复制为新皮肤，保存后生效");
};
$("#weasel-import").onclick = async () => {
  /* 【修复标签 2026-09-11】导入校验：JSON.parse 后先结构校验（颜色须为
     数字/#RRGGBB），非法弹 alert 不应用，不再直接提交后端 */
  let colors;
  try { colors = JSON.parse($("#weasel-in").value); }
  catch (e) { alert("导入失败：JSON 解析错误 " + e.message); return; }
  const r = validateWeaselColors(colors);
  if (r.err) { alert("导入失败：" + r.err); return; }
  try {
    const id = "weasel-" + Date.now().toString(36);
    SKIN = await api("POST", "/api/weasel_import", { id, colors: r.colors });
    bindSkin(); buildColorGrid(); preview(); msg("weasel 配色已导入，保存后生效", "ok");
  } catch (e) { msg("导入失败：" + e.message, "err"); }
};
$("#weasel-export").onclick = () => {
  const patch = {};
  COLOR_ROLES.forEach(([k]) => {
    const c = SKIN.colors[k]; if (!c) return;
    const r = parseInt(c.slice(1, 3), 16), g = parseInt(c.slice(3, 5), 16), b = parseInt(c.slice(5, 7), 16), a = parseInt(c.slice(7, 9) || "FF", 16);
    /* 【三十六修】>>> 0 转无符号：a≥0x80 时带符号位运算产出负数，
       自己导出的配色粘回导入会被 v>=0 校验拒收（round-trip 断裂） */
    patch[k] = a === 255 ? ((b << 16) | (g << 8) | r) >>> 0 : ((a << 24) | (b << 16) | (g << 8) | r) >>> 0;
  });
  patch.name = SKIN.name; patch.author = SKIN.author || "HuFu";
  $("#weasel-in").value = JSON.stringify(patch, null, 2);
  msg("已生成 weasel patch（复制到 weasel.custom.yaml 使用）", "ok");
};

$("#uw-add").onclick = async () => {
  const code = $("#uw-code").value.trim(), text = $("#uw-text").value.trim();
  if (!code || !text) return msg("编码与词均不能为空", "err");
  try { await api("POST", "/api/user_word/add", { code, text }); $("#uw-code").value = ""; $("#uw-text").value = ""; await loadUserWords(); msg("用户词已添加 ✓", "ok"); }
  catch (e) { msg(e.message, "err"); }
};

$("#exp-dl").onclick = () => {
  const blob = new Blob([JSON.stringify(CFG, null, 2)], { type: "application/json" });
  const a = document.createElement("a"); a.href = URL.createObjectURL(blob); a.download = "hufu-config.json"; a.click();
};
$("#imp-btn").onclick = () => $("#imp-file").click();
$("#imp-file").onchange = async ev => {
  /* 【修复标签 2026-09-11】导入校验：文件导入先结构校验（12 节齐全、
     类型对），非法弹 alert 不应用（后端整体替换，缺节会回出厂值） */
  const f = ev.target.files[0]; if (!f) return;
  let c;
  try { c = JSON.parse(await f.text()); }
  catch (e) { alert("导入失败：JSON 解析错误 " + e.message); return; }
  const err = validateConfig(c);
  if (err) { alert("导入已取消：" + err); return; }
  CFG = c; await $("#btn-save").click();
};
$("#cfg-apply").onclick = async () => {
  /* 【修复标签 2026-09-11】导入校验：文本框应用同文件导入——先校验后应用 */
  let c;
  try { c = JSON.parse($("#cfg-raw").value); }
  catch (e) { alert("JSON 无效：" + e.message); return; }
  const err = validateConfig(c);
  if (err) { alert("未应用：" + err); return; }
  CFG = c; await $("#btn-save").click();
};

async function init() {
  try {
    CFG = await api("GET", "/api/config");
    $("#cfg-raw").value = JSON.stringify(CFG, null, 2);
    bindInputs(); buildSliders();
    applyPlatform();
    await loadSchemas(); await refreshInfo();
    await loadSkinList(); await loadSkin($("#skin-select").value);
    await loadUserWords();
    msg("就绪");
  } catch (e) { msg("初始化失败：" + e.message, "err"); }
}

/* 【Linux 2026-09-19】引擎自带中英切换仅 Windows 适用——Linux 的英文输入
   由 fcitx5 键盘布局提供（Ctrl+Space 切布局），这里隐藏对应开关（不影响
   Windows 行为）。 */
async function applyPlatform() {
  let os = "";
  try { os = (await api("GET", "/api/platform")).os || ""; } catch (_) {}
  if (os === "" || os === "windows") return;
  const hide = sel => {
    const el = document.querySelector(sel);
    const box = el && (el.closest(".checkline") || el.closest(".field"));
    if (box) box.style.display = "none";
  };
  ["#g-shift", "#g-ctrl", "#g-caps", "#i-mixed"].forEach(hide);
  const row = document.querySelector("#g-shift")?.closest(".row");
  if (row) {
    const hint = document.createElement("div");
    hint.className = "sub";
    hint.textContent = "中英切换由 fcitx5 键盘布局提供（Ctrl+Space 切换布局），引擎不自带英文输入。";
    row.appendChild(hint);
  }
}
init();
