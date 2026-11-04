//! {加词}/{加权} 弹窗：`/jc {加词}`、`/jq {加权}` 选中后弹小窗。
//! 加词=三输入框（词 / 编码 / 选重位）+ 候选顺序实时预览；加权=两框
//!（词 / 权重，缺省 1000；编码由 server 反查最优码）。预览做成候选窗
//! 样式：每项「序号+词」分色（label/text 色）流式排列，新词高亮底块；
//! 配色与字体套用当前皮肤（colors + layout.font_*）。每次键入
//!（EN_UPDATE=0x400）立即刷新；预览变多窗口自适应加高，确定/取消恒
//! 在底部。窗口独立线程跑消息循环不阻塞 TSF。

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CreateFontW, CreatePen, CreateRoundRectRgn, CreateSolidBrush,
    DEFAULT_CHARSET, DeleteObject, EndPaint, FW_BOLD, FW_NORMAL, FillRect,
    GetStockObject, GetTextExtentPoint32W, HBRUSH, HDC, HGDIOBJ, NULL_BRUSH, NULL_PEN, PAINTSTRUCT,
    PS_SOLID, RoundRect, SelectObject, SetBkMode, SetTextColor, SetWindowRgn, TextOutW, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

const ID_WORD: i32 = 101;
const ID_CODE: i32 = 102;
const ID_POS: i32 = 103;
/// 【占位符 2026-10-08】加词框「占位符补位」勾选框（选重位超出
/// 现有候选数时用带圈数字 ③④… 补空位，词恰落第 N 位）。
const ID_PH: i32 = 104;
const ID_OK: i32 = 1001;
const ID_CANCEL: i32 = 1002;
/// 自绘预览区辅助 id（仅用于 WM_CTLCOLORSTATIC 选底色/字色）。
const ID_PV_TITLE: i32 = 105;
const ID_PV_MSG: i32 = 106;
const ID_PV_HEAD_CUR: i32 = 1990;
const ID_PV_HEAD_AFT: i32 = 2190;
// 预览项 id 段（CTLCOLOR 按段分色；子控件仍在用）
const ID_CUR_BASE: i32 = 2000; // 现有项
const ID_AFT_BASE: i32 = 2200; // 加入后普通项
const ID_AFT_PH: i32 = 2400; // 加入后占位项（③④… 用标签色压暗）
const ID_AFT_NEW: i32 = 2600; // 加入后新词高亮项

// 预览动态项 id 段已退役（2026-11-03 重绘：预览改为父窗口自绘，
// 不再创建 STATIC 子控件）。保留 ID_EN_UPDATE。
const ID_EN_UPDATE: u32 = 0x400; // EN_UPDATE（0x200 是 EN_KILLFOCUS！）

const CLASS: PCWSTR = w!("HuFuAddWord");

// 【2026-11-03 视觉重做】无边框卡片式弹窗：24 边距、
// 字段=标签+圆角输入卡、预览=真实候选皮肤卡片、底部主/次按钮。
// 【2026-11-04 字号放大】正文 15→17、小字 12→14、标题 23→26、预览 18→20；
// 窗口 520→560、加词高 680→720、加权高 604→640；全部固定像素、不做 DPI
// 缩放，几何常量按放大后的字体行高（GDI 实测 cell：26→34 / 17→23|24 /
// 14→20 / 20→27）同步重排。
const WIN_W: i32 = 560;
const M: i32 = 24;
const CONTENT_W: i32 = WIN_W - M * 2; // 512
const PV_X: i32 = M + 24;
const PV_W: i32 = CONTENT_W - 48;
/// 字段卡：首行 y、行距、卡高、编辑框内缩/高。
const ROW_Y0: i32 = 110;
const ROW_PITCH: i32 = 90;
const CARD_H: i32 = 50;
const EDIT_INSET: i32 = 12;
const EDIT_H: i32 = 36;
const EDIT_W: i32 = CONTENT_W - EDIT_INSET * 2;
const EDIT_DY: i32 = (CARD_H - EDIT_H) / 2;
const LBL_H: i32 = 26;
const LBL_W: i32 = 340;
/// 占位符开关（自绘 toggle）。
const TOGGLE_Y: i32 = 376;
const TOGGLE_W: i32 = 176;
const TOGGLE_H: i32 = 38;
/// 加词：预览卡 + 底栏。
const PREVIEW_Y: i32 = 426;
const PREVIEW_TITLE_Y: i32 = PREVIEW_Y + 12;
const PREVIEW_ITEMS_Y: i32 = PREVIEW_Y + 80;
const PV_START_Y: i32 = PREVIEW_ITEMS_Y;
const CLIENT_H: i32 = 720;
/// 加权：权重刻度 + 预览卡 + 底栏。
const SCALE_Y: i32 = 290;
const SCALE_H: i32 = 88;
const W_PREVIEW_Y: i32 = 392;
const W_CLIENT_H: i32 = 640;
/// 按钮几何（右对齐）。
const BTN_W: i32 = 116;
const BTN_H: i32 = 46;
const BTN_GAP: i32 = 12;
const BTN_OK_X: i32 = M + CONTENT_W - BTN_W;
const BTN_CANCEL_X: i32 = BTN_OK_X - BTN_GAP - BTN_W;

// ── 2026-11-03 视觉重做：固定 UI 调色板（GDI COLORREF=0x00BBGGRR）──
const DARK_BG: u32 = 0x17_13_12; // #121317
const DARK_CARD: u32 = 0x23_1D_1B; // #1B1D23
const DARK_BORDER: u32 = 0x37_2E_2B; // #2B2E37
const DARK_TEXT: u32 = 0xDE_E9_ED; // #EDE9DE
const DARK_MUTED: u32 = 0x8A_95_9A; // #9A958A
const DARK_ACCENT: u32 = 0x5C_B7_E7; // #E7B75C（无皮肤时的默认金）
const DARK_FG: u32 = 0x08_14_1A; // #1A1408
// 【2026-11-03 浅色模式】跟随 Windows“应用模式”的整套主题色。
// COLORREF = 0x00BBGGRR；浅色值同样按 BGR 书写。
#[derive(Clone, Copy)]
struct Theme {
    light: bool,
    bg: u32,
    card: u32,
    border: u32,
    text: u32,
    muted: u32,
    accent: u32,
    accent_fg: u32,
    chip_bg: u32,
    chip_fg: u32,
    preview: u32,
}
const THEME_DARK: Theme = Theme {
    light: false,
    bg: DARK_BG,
    card: DARK_CARD,
    border: DARK_BORDER,
    text: DARK_TEXT,
    muted: DARK_MUTED,
    accent: DARK_ACCENT,
    accent_fg: DARK_FG,
    chip_bg: 0x17_2F_3B, // #3B2F17 深金底
    chip_fg: 0x9A_D7_F3, // #F3D79A 浅金字
    preview: 0x27_20_1E,  // #1E2027
};
const THEME_LIGHT: Theme = Theme {
    light: true,
    bg: 0xE9_F1_F4,     // #F4F1E9
    card: 0xFF_FF_FF,
    border: 0xC6_D6_DE,  // #DED6C6
    text: 0x1D_26_2B,    // #2B261D
    muted: 0x68_76_7D,   // #7D7668
    accent: 0x22_7A_B0,  // #B07A22
    accent_fg: 0xED_F9_FF, // #FFF9ED
    chip_bg: 0xBE_E4_F5, // #F5E4BE
    chip_fg: 0x10_4A_6B, // #6B4A10
    preview: 0xED_F6_FA, // #FAF6ED
};

static THEME_IS_LIGHT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static THEME_LAST_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn theme() -> Theme {
    if THEME_IS_LIGHT.load(std::sync::atomic::Ordering::Relaxed) {
        THEME_LIGHT
    } else {
        THEME_DARK
    }
}

/// Windows 设置 → 个性化 → 颜色 → “应用模式”（浅色=1）。读不到默认深色。
fn read_system_light() -> bool {
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegGetValueW(
            key: isize,
            subkey: *const u16,
            value: *const u16,
            flags: u32,
            vtype: *mut u32,
            data: *mut core::ffi::c_void,
            cb: *mut u32,
        ) -> i32;
    }
    let sub: Vec<u16> = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize".encode_utf16().chain([0]).collect();
    let name: Vec<u16> = "AppsUseLightTheme".encode_utf16().chain([0]).collect();
    let mut v: u32 = 0;
    let mut ty: u32 = 0;
    let mut cb: u32 = 4;
    let ret = unsafe {
        RegGetValueW(
            -2147483647isize, /* HKEY_CURRENT_USER */
            sub.as_ptr(),
            name.as_ptr(),
            0x10, /* RRF_RT_REG_DWORD */
            &mut ty,
            &mut v as *mut u32 as *mut core::ffi::c_void,
            &mut cb,
        )
    };
    ret == 0 && v == 1
}

fn refresh_theme() {
    use std::sync::atomic::Ordering;
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let t = EPOCH.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64;
    let last = THEME_LAST_MS.load(Ordering::Relaxed);
    if last != 0 && t.saturating_sub(last) < 1500 {
        return;
    }
    THEME_LAST_MS.store(t, Ordering::Relaxed);
    THEME_IS_LIGHT.store(read_system_light(), Ordering::Relaxed);
}


/// 带圈数字（占位符）：1-20 → ①…⑳（U+2460+），21-35 → ㉑…㉟，
/// 36-50 → ㊱…㊿；越界 None（选重位超 50 不补位）。
fn circled_num(n: usize) -> Option<String> {
    let c = if (1..=20).contains(&n) {
        char::from_u32(0x2460 + n as u32 - 1)
    } else if (21..=35).contains(&n) {
        char::from_u32(0x3251 + n as u32 - 21)
    } else if (36..=50).contains(&n) {
        char::from_u32(0x32B1 + n as u32 - 36)
    } else {
        None
    };
    c.map(|ch| ch.to_string())
}

/// 皮肤数据（每次弹窗拉取，指纹没变走 same:true 微载荷；失败回退深色系）
#[allow(dead_code)]
struct Skin {
    bg: u32,
    text: u32,
    label: u32,
    // 【死字段删除】hilite/hilite_label/hilite_bg 三字段只写不读——
    // 预览着色已收敛为 NEW_RED（新词）/text（其余）/label（表头），
    // 高亮底块不再参与弹窗渲染。
    font_face: Vec<u16>, // UTF-16 含 null
    cand_spacing: i32,
    /// 【加词窗跟皮肤 2026-10-18·B】服务端皮肤指纹（op:"skin" 顶层
    /// fp）——下次弹窗 have_fp 命中即免重建；变了=换皮/改色/改字号，
    /// 字体刷子全套重建（弹窗线程每弹必拉，换皮即时生效）。
    /// 皮肤高亮色（新词胶囊/主按钮取它；缺失回退 DARK_ACCENT）。
    accent: u32,
    fp: String,
}
static SKIN: std::sync::Mutex<Option<Skin>> = std::sync::Mutex::new(None);
static BG_BRUSH: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
static FIELD_BRUSH: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
/// 候选预览内层（真实皮肤底）/新词胶囊（皮肤高亮色）。
static SKIN_BRUSH: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
static ACCENT_BRUSH: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
/// 新词胶囊底 / 预览内层 / 主题刷缓存。
static CHIP_BRUSH: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
static BRUSH_THEME: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2); // 2=未建

unsafe fn reset_brushes() {
    use std::sync::atomic::Ordering;
    for slot in [
        &BG_BRUSH,
        &FIELD_BRUSH,
        &SKIN_BRUSH,
        &ACCENT_BRUSH,
        &CHIP_BRUSH,
    ] {
        if let Some(b) = slot.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = DeleteObject(HGDIOBJ(b as *mut core::ffi::c_void));
        }
    }
    BRUSH_THEME.store(2, Ordering::Relaxed);
}

unsafe fn ensure_brushes(th: &Theme) {
    use std::sync::atomic::Ordering;
    let want: u8 = if th.light { 1 } else { 0 };
    if BRUSH_THEME.load(Ordering::Relaxed) == want {
        return;
    }
    // 【P2-2 2026-11-03】主题切换时旧刷子必须先释放，否则每换一次
    // 深浅色漏 5 个 GDI 刷子（加词窗关闭期间切主题必然走到这里）。
    reset_brushes();
    let mk = |c: u32| CreateSolidBrush(COLORREF(c)).0 as usize;
    *BG_BRUSH.lock().unwrap_or_else(|p| p.into_inner()) = Some(mk(th.bg));
    *FIELD_BRUSH.lock().unwrap_or_else(|p| p.into_inner()) = Some(mk(th.card));
    *SKIN_BRUSH.lock().unwrap_or_else(|p| p.into_inner()) = Some(mk(th.preview));
    *ACCENT_BRUSH.lock().unwrap_or_else(|p| p.into_inner()) = Some(mk(th.accent));
    *CHIP_BRUSH.lock().unwrap_or_else(|p| p.into_inner()) = Some(mk(th.chip_bg));
    BRUSH_THEME.store(want, Ordering::Relaxed);
}

fn refresh_theme_now() {
    THEME_LAST_MS.store(0, std::sync::atomic::Ordering::Relaxed);
    refresh_theme();
}

unsafe fn apply_theme_to_edits(hwnd: HWND, th: &Theme) {
    for id in [ID_WORD, ID_CODE, ID_POS] {
        if let Ok(h) = GetDlgItem(hwnd, id) {
            let _ = SendMessageW(
                h,
                0x0443, /* EM_SETBKGNDCOLOR */
                WPARAM(0),
                LPARAM(th.card as isize),
            );
            set_rich_text_color(h, th.text);
        }
    }
}

static FONT_MAIN: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
static FONT_LABEL: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
/// 新词专用：加粗+下划线（预览新词胶囊用）
static FONT_NEW: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
/// 固定 UI 字体（不跟皮肤字号，保证弹窗排版稳定）。
static FONT_TITLE: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
static FONT_UI: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
static FONT_UI_BOLD: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
static FONT_SMALL: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
static UI_FONTS_READY: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 占位符开关（自绘 toggle；替代旧 BS_AUTOCHECKBOX + BM_GETCHECK）。
static PH_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 当前聚焦字段（画焦点描边）。
static FOCUS_ID: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// 预览动态 STATIC 子控件（仍由 refresh_preview 创建/销毁）。
static ITEMS: std::sync::Mutex<Vec<isize>> = std::sync::Mutex::new(Vec::new());
/// 新词胶囊（父窗口自绘，STATIC 以 NULL_BRUSH 透明叠字）。
static NEW_CHIP_RECTS: std::sync::Mutex<Vec<(i32, i32, i32, i32)>> = std::sync::Mutex::new(Vec::new());

fn parse_hex(s: &str) -> Option<u32> {
    let t = s.trim().trim_start_matches('#');
    // 【P1 2026-11-03】非 ASCII（中文/emoji）按字节切 [0..2] 会 panic
    // 在 WndProc 跨 FFI 边界 = 宿主崩溃。颜色串只可能是 ASCII 十六进制。
    if t.len() < 6 || !t.is_ascii() {
        return None;
    }
    let r = u32::from_str_radix(&t[0..2], 16).ok()?;
    let g = u32::from_str_radix(&t[2..4], 16).ok()?;
    let b = u32::from_str_radix(&t[4..6], 16).ok()?;
    Some(b << 16 | g << 8 | r) // COLORREF 0x00BBGGRR
}

fn utf16z(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

/// 深色向白混合（皮肤底色太黑时提亮窗口底，避免大片死黑）。
fn lighten(c: u32, k: f32) -> u32 {
    let ch = |v: u32| -> u32 { v + ((255 - v) as f32 * k) as u32 };
    let r = ch(c & 0xFF);
    let g = ch((c >> 8) & 0xFF);
    let b = ch((c >> 16) & 0xFF);
    b << 16 | g << 8 | r
}

/// 相对亮度（0..1，REC.709 权重）。COLORREF 布局（低字节=R）。
fn lum(c: u32) -> f32 {
    let r = (c & 0xFF) as f32 / 255.0;
    let g = ((c >> 8) & 0xFF) as f32 / 255.0;
    let b = ((c >> 16) & 0xFF) as f32 / 255.0;
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// 【加词窗跟皮肤 2026-10-18·A】SSF 面板不透明区平均色（COLORREF）。
/// 帧=BGRA 预乘；只统计 alpha≥128 的像素（装饰皮大片透明边不算），
/// 不透明覆盖 <2% 视为无效（整图几乎全透明，回退默认色）。
/// 强调底上的文字色：深底白字、浅底深字。
fn contrast_fg(bg: u32) -> u32 {
    if lum(bg) > 0.55 {
        DARK_FG
    } else {
        0x00_F2_F2_F2
    }
}

fn avg_opaque_rgb(b: &crate::ssfwin::SsfBitmap) -> Option<u32> {
    let frame = b.frames.first()?;
    let px = (b.width as usize) * (b.height as usize);
    if px == 0 || frame.len() < px * 4 {
        return None;
    }
    let (mut sr, mut sg, mut sb, mut n) = (0u64, 0u64, 0u64, 0u64);
    for p in frame.chunks_exact(4) {
        if p[3] >= 128 {
            sr += p[2] as u64; // R
            sg += p[1] as u64; // G
            sb += p[0] as u64; // B
            n += 1;
        }
    }
    if n == 0 || n * 50 < px as u64 {
        return None;
    }
    let r = (sr / n) as u32;
    let g = (sg / n) as u32;
    let bch = (sb / n) as u32;
    Some(bch << 16 | g << 8 | r)
}

/// 【加词窗跟皮肤 2026-10-18·A】SSF 皮肤的加词窗配色：底=候选窗面板
/// 图平均色，文字/标签=display 正文色/注解色（#RRGGBBAA 前 6 位）。
/// 解不出（载荷缺节/图读不了/几乎全透明）返回 None 走默认回退。
fn ssf_panel_colors(sk: &serde_json::Value) -> (Option<u32>, Option<u32>, Option<u32>) {
    let cd = sk.pointer("/cache_dir").and_then(|x| x.as_str());
    let fp = sk.pointer("/fp").and_then(|x| x.as_str());
    // 加词窗是竖排观感：优先竖排候选面板，其次任一变体
    let img = [
        "vertical_candidates",
        "vertical",
        "vertical_code",
        "horizontal",
        "horizontal_candidates",
        "horizontal_code",
    ]
    .iter()
    .find_map(|k| {
        sk.pointer(&format!("/layouts/{k}/image"))
            .and_then(|x| x.as_str())
    });
    let back = match (cd, fp, img) {
        (Some(cd), Some(fp), Some(img)) => crate::ssfwin::load_bitmap(
            std::path::Path::new(cd),
            fp,
            img,
        )
        .ok()
        .and_then(|b| avg_opaque_rgb(&b)),
        _ => None,
    };
    let text = sk
        .pointer("/display/text_color")
        .and_then(|x| x.as_str())
        .and_then(parse_hex);
    let label = sk
        .pointer("/display/annotation_color")
        .and_then(|x| x.as_str())
        .and_then(parse_hex);
    (back, text, label)
}

// ── 【占位符记忆 2026-10-09】勾选态首选走 server 管道（server 落
// 数据\addword-ph.json），HKCU 只作管道不通时的兜底——NTQQ 渲染进程
// 跑在低完整性沙箱里写 HKCU 静默失败（用户实测「QQ 里勾了不记」的
// 根因），而管道在沙箱宿主里打字本就必通。回读同款双路。──
fn ph_remember(on: bool) {
    if crate::ipc::call(&serde_json::json!({"op": "aw_ph_set", "on": on})).is_some() {
        return;
    }
    ph_remember_hkcu(on);
}

fn ph_remember_hkcu(on: bool) {
    #[link(name = "advapi32")]
    extern "system" {
        fn RegCreateKeyExW(
            key: isize,
            subkey: *const u16,
            reserved: u32,
            class: *const u16,
            options: u32,
            desired: u32,
            sa: *const core::ffi::c_void,
            result: *mut isize,
            dispos: *mut u32,
        ) -> i32;
        fn RegSetValueExW(
            key: isize,
            name: *const u16,
            reserved: u32,
            vtype: u32,
            data: *const u8,
            cb: u32,
        ) -> i32;
        fn RegCloseKey(key: isize) -> i32;
    }
    unsafe {
    let sub: Vec<u16> = r"Software\HuFu".encode_utf16().chain([0]).collect();
        let name: Vec<u16> = "addword_ph".encode_utf16().chain([0]).collect();
        let mut hkey: isize = 0;
        if RegCreateKeyExW(
            -2147483647i64 as isize, /* HKEY_CURRENT_USER */
            sub.as_ptr(),
            0,
            std::ptr::null(),
            0,
            0x2002, /* KEY_SET_VALUE */
            std::ptr::null(),
            &mut hkey,
            std::ptr::null_mut(),
        ) == 0
        {
            let v: u32 = if on { 1 } else { 0 };
            RegSetValueExW(
                hkey,
                name.as_ptr(),
                0,
                4, /* REG_DWORD */
                &v as *const u32 as *const u8,
                4,
            );
            RegCloseKey(hkey);
        }
    }
}

fn ph_recall() -> bool {
    if let Some(r) = crate::ipc::call(&serde_json::json!({"op": "aw_ph_get"})) {
        return r.get("on").and_then(|v| v.as_bool()).unwrap_or(false);
    }
    ph_recall_hkcu()
}

fn ph_recall_hkcu() -> bool {
    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            key: isize,
            subkey: *const u16,
            reserved: u32,
            desired: u32,
            result: *mut isize,
        ) -> i32;
        fn RegQueryValueExW(
            key: isize,
            name: *const u16,
            reserved: *mut u32,
            vtype: *mut u32,
            data: *mut u8,
            cb: *mut u32,
        ) -> i32;
        fn RegCloseKey(key: isize) -> i32;
    }
    unsafe {
    let sub: Vec<u16> = r"Software\HuFu".encode_utf16().chain([0]).collect();
        let name: Vec<u16> = "addword_ph".encode_utf16().chain([0]).collect();
        let mut hkey: isize = 0;
        if RegOpenKeyExW(
            -2147483647i64 as isize, /* HKEY_CURRENT_USER */
            sub.as_ptr(),
            0,
            0x2001, /* KEY_QUERY_VALUE */
            &mut hkey,
        ) != 0
        {
            return false;
        }
        let mut v: u32 = 0;
        let mut cb: u32 = 4;
        let ok = RegQueryValueExW(
            hkey,
            name.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut v as *mut u32 as *mut u8,
            &mut cb,
        ) == 0
            && v == 1;
        RegCloseKey(hkey);
        ok
    }
}

/// 拉皮肤配色+字体（弹窗线程每次弹窗调用）。
/// 【加词窗跟皮肤 2026-10-18·B】原实现 `if g.is_some() { return }` 一次
/// 性缓存——换皮后老进程里加词窗永远是旧配色（用户实锤「背景色有时
/// 候不一样」的来源之二）。改为指纹制：带 have_fp 微载荷轮询，指纹没变
/// 零重建（一次管道往返，ms 级），变了=字体/刷子/Skin 全套重建。
/// IPC 失败：已有缓存沿用（比打回默认色好），首次失败才走默认深色系。
fn load_skin() {
    unsafe { ensure_ui_fonts() };
    refresh_theme_now();
    unsafe { ensure_brushes(&theme()) };
    let mut g = SKIN.lock().unwrap_or_else(|p| p.into_inner());
    let mut req = serde_json::json!({"op": "skin"});
    if let Some(fp) = g.as_ref().map(|s| s.fp.as_str()) {
        if !fp.is_empty() {
            req["have_fp"] = serde_json::json!(fp);
        }
    }
    let resp = crate::ipc::call(&req);
    let sk = match resp.as_ref() {
        Some(v) => {
            if v.get("same").and_then(|x| x.as_bool()) == Some(true) && g.is_some() {
                return; // 指纹未变：字体刷子全沿用
            }
            v.get("skin").cloned().unwrap_or(serde_json::Value::Null)
        }
        None => {
            if g.is_some() {
                return; // server 重启瞬间等：沿用缓存
            }
            serde_json::Value::Null
        }
    };
    let fp = resp
        .as_ref()
        .and_then(|v| v.get("fp").and_then(|x| x.as_str()).map(str::to_string))
        .unwrap_or_default();
    let get_color = |k: &str| -> Option<u32> {
        sk.pointer(&format!("/colors/{k}"))
            .and_then(|x| x.as_str())
            .and_then(parse_hex)
    };
    let face = sk
        .pointer("/layout/font_face")
        .and_then(|x| x.as_str())
        .unwrap_or("Microsoft YaHei UI")
        .to_string();
    let cand_spacing = sk
        .pointer("/layout/candidate_spacing")
        .and_then(|x| x.as_f64())
        .unwrap_or(6.0) as i32;
    // 【加词窗跟皮肤 2026-10-18·A】SSF 皮肤载荷没有 /colors 节（候选窗
    // 画在皮肤图上）——加词窗原实现一律回退固定灰绿（用户实锤「背景
    // 色有时候不一样」的来源之一）。改为：背景取皮肤候选窗面板图的
    // 不透明区平均色（与候选窗观感同源），文字/标签取皮肤 display
    // 四色里的正文色/注解色。
    let ssf_mode = sk.pointer("/kind").and_then(|x| x.as_str()) == Some("ssf");
    let (back, text, label) = if ssf_mode {
        ssf_panel_colors(&sk)
    } else {
        (
            get_color("back_color"),
            get_color("text_color").or_else(|| get_color("candidate_text_color")),
            get_color("label_color").or_else(|| get_color("comment_text_color")),
        )
    };
    let bg = lighten(back.unwrap_or(0x22_2E_16_u32), 0.20);
    let mut text_c = text.unwrap_or(0xEC_E2_D7_u32);
    let mut label_c = label.unwrap_or(0xB5_A6_8A);
    // ssf 面板平均色与皮肤文字色的对比度护栏：亮度差过小（花皮平均值
    // 发灰等）时按底色翻纯黑/纯白，保证弹窗永远可读。JSON 皮肤的
    // colors 是作者手定的完整配色，不越权改。
    if ssf_mode && back.is_some() {
        let lb = lum(bg);
        if (lb - lum(text_c)).abs() < 0.22 {
            text_c = if lb > 0.5 { 0x1A_1A_1A } else { 0xF2_F2_F2 };
        }
        if (lb - lum(label_c)).abs() < 0.15 {
            label_c = if lb > 0.5 { 0x3A_3A_3A } else { 0xC8_C8_C8 };
        }
    }
    // 强调色：优先皮肤高亮底，其次 SSF display.first_color，最后默认金。
    let accent = get_color("hilited_candidate_back_color")
        .or_else(|| get_color("hilited_back_color"))
        .or_else(|| get_color("hilited_text_color"))
        .or_else(|| {
            sk.pointer("/display/first_color")
                .and_then(|x| x.as_str())
                .and_then(parse_hex)
        })
        .unwrap_or(DARK_ACCENT);
    let s = Skin {
        bg,
        text: text_c,
        label: label_c,
        accent,
        font_face: utf16z(&face),
        cand_spacing: cand_spacing.max(2),
        fp,
    };
    unsafe {
        let face_ptr = PCWSTR(s.font_face.as_ptr());
        let mk_font = |pt: i32, weight: i32| {
            CreateFontW(
                -pt,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                0,
                0,
                CLEARTYPE_QUALITY.0 as u32,
                0,
                face_ptr,
            )
        };
        // 预览用固定 20/14px（皮肤字号只用在候选窗；弹窗预览要能一屏装下）
        // 【2026-11-04 字号放大】18/13 → 20/14（cell 27/20）。
        *FONT_MAIN.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(mk_font(20, FW_NORMAL.0 as i32).0 as usize);
        *FONT_LABEL.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(mk_font(14, FW_NORMAL.0 as i32).0 as usize);
        // 新词：加粗 + 下划线
        let bold_underline = CreateFontW(
            -20,
            0,
            0,
            0,
            FW_BOLD.0 as i32,
            0,
            0, // underline（新词用强调底色，不用下划线）
            0,
            DEFAULT_CHARSET.0 as u32,
            0,
            0,
            CLEARTYPE_QUALITY.0 as u32,
            0,
            PCWSTR(s.font_face.as_ptr()),
        );
        *FONT_NEW.lock().unwrap_or_else(|p| p.into_inner()) = Some(bold_underline.0 as usize);
        // 窗口底/输入卡底：固定深色 UI，不随皮肤（候选预览卡内才用皮肤色）。

    }
    *g = Some(s);
}

/// 固定 UI 字体（只建一次；与皮肤字号解耦，保证弹窗排版稳定）。
unsafe fn ensure_ui_fonts() {
    use std::sync::atomic::Ordering;
    if UI_FONTS_READY.load(Ordering::Relaxed) {
        return;
    }
    let face = utf16z("Microsoft YaHei UI");
    let mk = |px: i32, weight: i32| {
        CreateFontW(
            -px,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            0,
            0,
            CLEARTYPE_QUALITY.0 as u32,
            0,
            PCWSTR(face.as_ptr()),
        )
    };
    // 【2026-11-04 字号放大】标题 23→26、正文 15→17、小字 12→14。
    // GDI 实测 cell 高：26→34、17→23（粗体 24）、14→20——排版常量按此重排。
    *FONT_TITLE.lock().unwrap_or_else(|p| p.into_inner()) =
        Some(mk(26, FW_BOLD.0 as i32).0 as usize);
    *FONT_UI.lock().unwrap_or_else(|p| p.into_inner()) =
        Some(mk(17, FW_NORMAL.0 as i32).0 as usize);
    *FONT_UI_BOLD.lock().unwrap_or_else(|p| p.into_inner()) =
        Some(mk(17, FW_BOLD.0 as i32).0 as usize);
    *FONT_SMALL.lock().unwrap_or_else(|p| p.into_inner()) =
        Some(mk(14, FW_NORMAL.0 as i32).0 as usize);
    UI_FONTS_READY.store(true, Ordering::Relaxed);
}

/// 窗模式：false=加词（三框+预览），true=加权（词+权重两框）。
/// 【/jq 加权 2026-09-06】加权=提升该词权重（server 反查最优码，
/// 写 用户词.txt 词行 weight 列；缺省 1000）。
static MODE_WEIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn is_weight_mode() -> bool {
    MODE_WEIGHT.load(std::sync::atomic::Ordering::Relaxed)
}

/// 弹出加词窗（非阻塞：新线程 + 消息循环）。
pub fn open() {
    MODE_WEIGHT.store(false, std::sync::atomic::Ordering::Relaxed);
    open_common();
}

/// 【右键调频菜单 2026-10-06】带预填弹出加词窗：候选窗右键「加词」
/// 带出 (词, 编码, 选重位)——与 {加词}/、jc 同窗口同提交链路
///（/api/user_word/add），仅初始值预填。窗口已开时不强填（消费即弃，
/// 防陈旧预填滞留到下次开窗）。
pub fn open_prefilled(word: &str, code: &str, pos: &str) {
    MODE_WEIGHT.store(false, std::sync::atomic::Ordering::Relaxed);
    *PREFILL.lock().unwrap_or_else(|p| p.into_inner()) = Some((
        word.to_string(),
        code.to_string(),
        pos.to_string(),
    ));
    open_common();
}

/// 右键「加词」预填（词, 编码, 选重位）——开窗线程消费即清。
static PREFILL: std::sync::Mutex<Option<(String, String, String)>> =
    std::sync::Mutex::new(None);

/// 弹出加权窗（/jq {加权} 触发；词+权重两框）。
pub fn open_weight() {
    MODE_WEIGHT.store(true, std::sync::atomic::Ordering::Relaxed);
    open_common();
}

/// 【小窗打开判定 2026-09-12】加词/加权窗存活且可见——主文档 sink
/// 的按键直通用（激活间隙防组段写进主文档，见 tsf::dispatch 头部）。
/// 复用 ADDWORD_HWND 登记句柄；异常清理态由 IsWindow 兜底。
pub fn is_open() -> bool {
    let g = ADDWORD_HWND.lock().unwrap_or_else(|p| p.into_inner());
    *g != 0 && unsafe { IsWindow(HWND(*g as *mut _)).as_bool() }
}

// 【三十四修·死代码删除】current_hwnd()（十二修无条件直通门后零调用）已删。

/// 小窗线程 id 登记（0=无）——五修：dispatch 直通门只挡非小窗线程。
static ADDWORD_TID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// 当前线程是否小窗线程——直通门放行判定（词框键由小窗线程自己的
/// sink 处理，主线程的才需要直通防残留）。
pub fn in_window_thread() -> bool {
    // 【三十六修】单次 load（旧实现两次 load 之间存在 TOCTOU——首读
    // 非零、次读已被另一线程改写/清零，判定失真）。
    let tid = ADDWORD_TID.load(std::sync::atomic::Ordering::Relaxed);
    tid != 0
        && unsafe {
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn GetCurrentThreadId() -> u32;
            }
            GetCurrentThreadId()
        } == tid
}

/// 【T4 v5】加词窗消息：回灌链两拍——AW_FLUSH=无段提交转正（起 30ms
/// 定时器）；AW_TAIL=补建尾巴段（再起 30ms）。
pub const AW_FLUSH_MSG: u32 = 0x8000 + 0x550;
pub const AW_TAIL_MSG: u32 = 0x8000 + 0x552;
/// 【T4 v3b】冲刷定时器 id——CUAS 物化边界是宿主泵周期而非编辑会话。
const AW_FLUSH_TIMER: usize = 0x551;
const AW_TAIL_TIMER: usize = 0x553;
/// 【词框实时编码 2026-10-07】词行标签控件 id（编码尾巴显示位——
/// EDIT 回退模式下用；RichEdit 模式编码内联显示在框内）。
pub const AW_LIVE_LABEL: i32 = 110;
/// 【RichEdit 2026-10-07】词框是否为 RichEdit（TSF 原生）——true 时
/// tsf.rs 走标准组段路径（编码内联显示/上屏即时）；false（创建失败
/// 回退 EDIT）走 v3c CUAS 特判路径。
static AW_RICH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 词框是否 RichEdit 模式。
pub fn aw_rich() -> bool {
    AW_RICH.load(std::sync::atomic::Ordering::Relaxed)
}

/// 【词框实时编码】把打字中的编码尾巴写到词行标签（raw 空则还原）。
/// STATIC 不发 EN_UPDATE，无回流。
pub fn aw_live_code(raw: &str) {
    let h = aw_hwnd();
    if h == 0 {
        return;
    }
    let base = if is_weight_mode() {
        "词（要加权的字或词）"
    } else {
        "词（要打出的内容）"
    };
    let owned;
    let text: &str = if raw.is_empty() {
        base
    } else {
        // 编码尾巴只显 ASCII 原始码
        let ascii: String = raw.chars().filter(|c| c.is_ascii()).collect();
        if ascii.is_empty() {
            return;
        }
        owned = format!("{base} · 编码 {ascii}");
        &owned
    };
    let v: Vec<u16> = text.encode_utf16().chain([0]).collect();
    unsafe {
        if let Ok(lbl) =
            GetDlgItem(windows::Win32::Foundation::HWND(h as *mut _), AW_LIVE_LABEL)
        {
            let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                lbl,
                PCWSTR(v.as_ptr()),
            );
        }
    }
}

/// 【T4 v3】活着的加词窗句柄（0=无）——通用查询（RichEdit 化后回灌
/// PostMessage 已停用）。
pub fn aw_hwnd() -> isize {
    let guard = ADDWORD_HWND.lock().unwrap_or_else(|p| p.into_inner());
    *guard
}

/// 【词框候选内嵌·标题栏 2026-09-12 八修】小窗线程的候选显示在小窗
/// 加词窗单例登记（0=无）：open_common 临界区内读写，消息循环
/// 结束清零。短锁使用，绝不跨消息循环持有。
static ADDWORD_HWND: std::sync::Mutex<isize> = std::sync::Mutex::new(0);

fn open_common() {
    std::thread::spawn(|| unsafe {
        // 【2026-11-02 加词窗回归】先给本线程打"虎符自建 UI 线程"标记：下面
        // 自激活的 TIP 会走 Activate，必须跳过"无文档焦点 → 延迟挂载"的宿主
        // 准入制（自激活在建窗之前，GetFocus 恒 Err，词框会因此收不到任何键）。
        crate::tsf::mark_ime_ui_thread();
        // 【B1 修复 2026-09-13 三十四修】单例复查提前到线程起手（store
        // tid 与 TIP 自激活**之前**）：旧序 = store tid → 全套自激活 →
        // 临界区才发现窗口已在 → return——连按 /jc 每次 (a) B 线程
        // 覆盖 ADDWORD_TID 后悬空（真小窗线程 A 的键被直通门判为主
        // 线程，词框打不了中文）(b) 泄漏一套 TIP 激活+键 sink 不回收。
        // 新序：先查窗口在不在——在=前置复用直接退出（零登记零激活）。
        {
            let mut guard = ADDWORD_HWND.lock().unwrap_or_else(|p| p.into_inner());
            if *guard != 0 && IsWindow(HWND(*guard as *mut _)).as_bool() {
                let existing = HWND(*guard as *mut _);
                let _ = ShowWindow(existing, SW_SHOWNORMAL);
                let _ = SetForegroundWindow(existing);
                crate::tsf::trace("addword 窗口已在，前置复用（早退）");
                return;
            }
            if let Ok(existing) = FindWindowW(CLASS, None) {
                let mut pid: u32 = 0;
                let _ = GetWindowThreadProcessId(existing, Some(&mut pid));
                if pid == std::process::id() && !existing.0.is_null() {
                    *guard = existing.0 as isize;
                    let _ = ShowWindow(existing, SW_SHOWNORMAL);
                    let _ = SetForegroundWindow(existing);
                    crate::tsf::trace("addword 窗口已在（FindWindow 早退复用）");
                    return;
                }
            }
        }
        // 【三十六修·B1 残余竞态收口】tid 登记从这里（线程起手）移到
        crate::tsf::trace("addword: 打开 v3（自动编码跟踪+占位符提示+20%放大）");
        // 下方窗口登记临界区内：连按 /jc 两线程同过前置检查时，双线程
        // 都在此 store tid → 后 store 者覆盖真窗口线程的 tid（悬挂），
        // in_window_thread() 对真小窗线程失真 → 直通门误判（词框打
        // 不了中文）+ 输家 TIP 激活泄漏。tid 现与 *guard 登记同临界区
        // 同点写入——输家在任何早退路径都不再触碰 tid。
        // 【词框 TSF 化 2026-09-12 三修】线程 TSF 化的完整链：STA COM
        // → 显式 CoCreateInstance(ThreadMgr)（msctf 判定线程 TSF-
        // enabled 的标志=线程持有 ThreadMgr；只有 CoInitialize 不够，
        // 二修实锤键仍不进 sink）→ ActivateLanguageProfile（本线程
        // 当前输入法=HuFu）→ 而后创建的 EDIT 被 msctf 接管：获焦时
        // 关联 DocumentMgr + 激活 TIP（HuFuTs 实例化，Activate 在本
        // 线程跑，键 sink 装上）。ThreadMgr 保活到消息循环结束（变量
        // _tm 活着）；线程退出全部回收。
        // ThreadMgr/tid 写线程局部（tsf::set_thread_tm）——每线程各
        // 持自己的；进程级 Shared 只留首激活（主线程）锚不漂移。
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
        let _tm: windows::core::Result<windows::Win32::UI::TextServices::ITfThreadMgr> =
            windows::Win32::System::Com::CoCreateInstance(
                &windows::Win32::UI::TextServices::CLSID_TF_ThreadMgr,
                None,
                windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
            );
        if let Ok(tm) = &_tm {
            // tid=0 占位（下方四修会用 ThreadMgr::Activate 的真 tid 覆盖）
            crate::tsf::set_thread_tm(tm.clone(), 0);
        }
        let _ = (|| -> windows::core::Result<()> {
            // 【四修 2026-09-12】ActivateLanguageProfile 只是"选择"当前
            // 输入法——TIP 实例**不激活**（激活标记实锤：小窗线程 tid
            // 从未出现，键 sink 从未装，词框键入无 TIP 接收，字母直通）。
            // 焦点驱动的懒激活对该 EDIT 不发生（msctf 不接管非标准宿主
            // 线程的焦点），故显式自激活：
            //   ThreadMgr::Activate() → client id（本线程）
            //   CoCreateInstance(自家 CLSID) → ITfTextInputProcessor
            //   tip.Activate(tm, tid) → AdviseKeyEventSink 装上
            // 三修的线程局部化保证此激活不覆盖主线程锚（g.thread_mgr
            // 只认首激活；本线程走 THREAD_TM/THREAD_TID）。
            let profiles: windows::Win32::UI::TextServices::ITfInputProcessorProfiles =
                windows::Win32::System::Com::CoCreateInstance(
                    &windows::Win32::UI::TextServices::CLSID_TF_InputProcessorProfiles,
                    None,
                    windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
                )?;
            profiles.ActivateLanguageProfile(
                &crate::CLSID_HUFU_TSF,
                0x0804u16,
                &crate::com::PROFILE_GUID,
            )?;
            let tm = match _tm.as_ref() {
                Ok(t) => t.clone(),
                Err(e) => {
                    crate::tsf::trace(&format!("addword自激活: ThreadMgr缺失 {e:?}"));
                    return Err(e.clone());
                }
            };
            let tid = match tm.Activate() {
                Ok(t) => t,
                Err(e) => {
                    crate::tsf::trace(&format!("addword自激活: ThreadMgr.Activate失败 {e:?}"));
                    return Err(e);
                }
            };
            crate::tsf::set_thread_tm(tm.clone(), tid);
            let tip: windows::Win32::UI::TextServices::ITfTextInputProcessor =
                match windows::Win32::System::Com::CoCreateInstance(
                    &crate::CLSID_HUFU_TSF,
                    None,
                    windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
                ) {
                    Ok(t) => t,
                    Err(e) => {
                        crate::tsf::trace(&format!("addword自激活: CoCreate自家CLSID失败 {e:?}"));
                        return Err(e);
                    }
                };
            if let Err(e) = tip.Activate(&tm, tid) {
                crate::tsf::trace(&format!("addword自激活: tip.Activate失败 {e:?}"));
                return Err(e);
            }
            crate::tsf::trace(&format!(
                "addword: 小窗线程 TIP 已自激活 tid={tid}（词框键 sink 装上）"
            ));
            Ok(())
        })();
        crate::tsf::trace("addword open（线程已起）");
        // 【单例 2026-09-06】窗口已在（本进程）→ 前置复用，不再多开
        //（用户连按 /jq 会叠开多个同位窗口，只看得见第一个）
        // 【竞态闸门 2026-09-11】FindWindow(无)→CreateWindow 两步无锁：
        // 连按 /jq 两个线程同时查空 → 各建一窗（旧实现真实竞态）。
        // 临界区（ADDWORD_HWND 锁）内复查+建窗+登记；锁绝不跨消息
        // 循环持有。
        let mut guard = ADDWORD_HWND.lock().unwrap_or_else(|p| p.into_inner());
        if *guard != 0 && IsWindow(HWND(*guard as *mut _)).as_bool() {
            let existing = HWND(*guard as *mut _);
            let _ = ShowWindow(existing, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(existing);
            crate::tsf::trace("addword 窗口已在，前置复用");
            return;
        }
        if let Ok(existing) = FindWindowW(CLASS, None) {
            let mut pid: u32 = 0;
            let _ = GetWindowThreadProcessId(existing, Some(&mut pid));
            if pid == std::process::id() && !existing.0.is_null() {
                *guard = existing.0 as isize;
                let _ = ShowWindow(existing, SW_SHOWNORMAL);
                let _ = SetForegroundWindow(existing);
                crate::tsf::trace("addword 窗口已在（FindWindow 复用）");
                return;
            }
        }
        load_skin();
        let hmod = GetModuleHandleW(None).unwrap_or_default();
        let hinst = HINSTANCE(hmod.0);
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW | CS_DROPSHADOW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: windows::Win32::Graphics::Gdi::GetSysColorBrush(
                windows::Win32::Graphics::Gdi::COLOR_WINDOW,
            ),
            lpszClassName: CLASS,
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        // 【T4 修 2026-10-07】标题必须 NUL 终止——旧代码裸 collect() 无
        // 终止符，一直靠栈上恰逢 0 侥幸（插入探针挪了栈布局即现乱码尾）。
        let title: Vec<u16> = if is_weight_mode() {
            "虎符 · 加权".encode_utf16().chain([0]).collect()
        } else {
            "虎符 · 加词".encode_utf16().chain([0]).collect()
        };
        // 无边框卡片窗（2026-11-03 视觉重做）：自绘标题栏/底栏，
        // WS_POPUP 不占系统 caption；高度=客户区（outer_h 恒等）。
        let client_h = if is_weight_mode() { W_CLIENT_H } else { CLIENT_H };
        let h = outer_h(client_h);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST,
            CLASS,
            PCWSTR(title.as_ptr()),
            WS_POPUP | WS_SYSMENU,
            (sw - WIN_W) / 2,
            (sh - h) / 2,
            WIN_W,
            h,
            None,
            None,
            hinst,
            None,
        )
        .unwrap_or_default();
        if hwnd.0.is_null() {
            crate::tsf::trace("addword CreateWindow 失败（窗口未建）");
            return;
        }
        // 登记本窗口句柄（后续 open 前置复用）+ 本线程 id（五修：直通
        // 门判定用）——同一临界区同一写入点，B1 残余竞态收口（三十六
        // 修，见上方注释）。放锁再跑消息循环。
        *guard = hwnd.0 as isize;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThreadId() -> u32;
        }
        let my_tid = GetCurrentThreadId();
        ADDWORD_TID.store(my_tid, std::sync::atomic::Ordering::Relaxed);
        drop(guard);
        let _ = ShowWindow(hwnd, SW_SHOW);
        // 【2026-11-03 第三批】加词/加权窗 120ms 混合入场（系统关动画/RDP 瞬显）。
        if crate::candwin2::system_anim_allowed() {
            let _ = windows::Win32::UI::WindowsAndMessaging::AnimateWindow(
                hwnd,
                120,
                windows::Win32::UI::WindowsAndMessaging::AW_BLEND,
            );
        }
        // 【六修 2026-09-12】AttachThreadInput 抢前台——此前裸调
        // SetForegroundWindow 被系统前台锁定静默拒绝（跨线程输入队
        // 列：小窗线程≠前台线程），trace 实锤小窗从未成为前台：键全
        // 被 QQ 主线程 sink 吃掉、组段建在主文档 (1085,793)（「原光
        // 标残留+词框只有字母」的真正根因）。AttachThreadInput 共享
        // 前台线程的输入状态后 SetForegroundWindow/SetFocus 才有效；
        // 抢完立刻脱离 attach（短窗，不留输入耦合）。附带诊断：前台
        // 抢夺成败打 trace。
        #[link(name = "user32")]
        unsafe extern "system" {
            fn AttachThreadInput(idattach: u32, idattachto: u32, fattach: i32) -> i32;
        }
        let fg = GetForegroundWindow();
        let fg_tid = if fg.0.is_null() {
            0
        } else {
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(fg, Some(&mut pid))
        };
        let my_tid = GetCurrentThreadId();
        let mut fg_ok = SetForegroundWindow(hwnd).as_bool();
        if !fg_ok && fg_tid != 0 && fg_tid != my_tid {
            let at = AttachThreadInput(my_tid, fg_tid, 1);
            fg_ok = SetForegroundWindow(hwnd).as_bool();
            // 焦点直接给词框（attach 态下跨线程 SetFocus 有效）
            if let Some(fe) = FIRST_EDIT
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .map(|h| HWND(h as *mut _))
            {
                let _ = SetFocus(fe);
            }
            let _ = AttachThreadInput(my_tid, fg_tid, 0);
            crate::tsf::trace(&format!(
                "addword: attach={at} 前台抢夺={fg_ok}（fg_tid={fg_tid}）"
            ));
        } else {
            crate::tsf::trace(&format!("addword: 前台抢夺={fg_ok}（直接成功）"));
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // 【RichEdit Tab 2026-10-07】词框 RichEdit 的 DLGC 应答吞
            // Tab——IsDialogMessage 拿不到导航权。词框内 Tab/Shift+Tab
            // 手动导航三框环（词→编码→选重→词）。
            if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN
                && msg.wParam.0 as u16 == 0x09 /* VK_TAB */
            {
                let foc = 
                    windows::Win32::UI::Input::KeyboardAndMouse::GetFocus()
                ;
                // 【编码/选重位对换 2026-10-08】Tab 环与行序同步。
                // 【2026-11-03】加入占位符 toggle 与主/次按钮——旧环只
                // 在三框内循环，键盘用户永远到不了开关/确定（全键盘可达）。
                let mut order = vec![ID_WORD, ID_POS, ID_CODE];
                if !is_weight_mode() {
                    order.push(ID_PH);
                }
                order.push(ID_OK);
                order.push(ID_CANCEL);
                let cur = order.iter().position(|i| {
                    matches!(
                         GetDlgItem(hwnd, *i) ,
                        Ok(h) if h == foc
                    )
                });
                if let Some(c) = cur {
                    #[link(name = "user32")]
                    unsafe extern "system" {
                        fn GetKeyState(vkey: i32) -> i16;
                    }
                    let shift = ( GetKeyState(0x10)  as u16 & 0x8000) != 0;
                    let n = order.len();
                    let step = if shift { n - 1 } else { 1 };
                    // 跳过禁用控件（词/编码空时“确定”不可聚焦），
                    // 保证键盘用户 Tab 始终能走到下一个可用控件。
                    let mut moved = false;
                    for k in 1..=n {
                        let idx = (c + step * k) % n;
                        if let Ok(h) = GetDlgItem(hwnd, order[idx]) {
                            if windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(h).as_bool() {
                                let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(h);
                                moved = true;
                                break;
                            }
                        }
                    }
                    if moved {
                        continue;
                    }
                }
            }
            // 【2026-11-03】无系统对话框模板：显式接管 Enter/Esc，
            // 保证 owner-draw 按钮下键盘确定/取消仍然可用。
            if msg.message == WM_KEYDOWN {
                match msg.wParam.0 as u16 {
                    0x0D => {
                        // 确定不可用（词/编码为空）时不关闭，避免静默丢输入。
                        let ok = GetDlgItem(hwnd, ID_OK)
                            .map(|h| windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(h).as_bool())
                            .unwrap_or(false);
                        if ok {
                            let _ = SendMessageW(hwnd, WM_COMMAND, WPARAM(ID_OK as usize), LPARAM(0));
                        }
                        continue;
                    }
                    0x1B => {
                        let _ = DestroyWindow(hwnd);
                        continue;
                    }
                    _ => {}
                }
            }
            if !IsDialogMessageW(hwnd, &mut msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        // 消息循环退出（窗口已销毁）——清登记，下次 open 可再建
        let mut guard = ADDWORD_HWND.lock().unwrap_or_else(|p| p.into_inner());
        *guard = 0;
        // 清线程 id（防系统复用该 tid 时误判 in_window_thread）
        ADDWORD_TID.store(0, std::sync::atomic::Ordering::Relaxed);
    });
}

/// client 高 → 外框高（无边框 WS_POPUP：恒等）。
unsafe fn outer_h(client_h: i32) -> i32 {
    client_h
}

unsafe fn create_child(
    parent: HWND,
    cls: PCWSTR,
    text: PCWSTR,
    ex: WINDOW_EX_STYLE,
    style_extra: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: i32,
) -> HWND {
    let hmod = GetModuleHandleW(None).unwrap_or_default();
    CreateWindowExW(
        ex,
        cls,
        text,
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style_extra),
        x,
        y,
        w,
        h,
        parent,
        HMENU(id as *mut _),
        HINSTANCE(hmod.0),
        None,
    )
    .unwrap_or_default()
}

/// 【词框聚焦 2026-09-12】小窗首个 EDIT（词框）句柄——WM_ACTIVATE
/// 激活时聚焦用（isize 规避 HWND 跨静态的 Sync 问题）。
static FIRST_EDIT: std::sync::Mutex<Option<isize>> = std::sync::Mutex::new(None);
/// 【编码自动跟踪 2026-10-08】编码框当前内容是否系自动填写（true=词框
/// 每变都重算覆盖；用户手改过即 false，清空编码框回到 true）。
static CODE_AUTO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
/// 我们自己 SetWindowTextW(ID_CODE) 触发的 EN_UPDATE 回声标记（不算
/// 用户手改）。
static SET_CODE_ECHO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 【回声风暴修复 2026-10-08 v3】最后一条**程序化写入**编码框的值。
/// RichEdit 对邻近子窗扰动（预览销毁/重建 STATIC）会重发 EN_UPDATE——
/// 回声旗标只够挡第一条，风暴重发的文本==最后写入值：据此识别，不算
/// 用户手改（否则 CODE_AUTO 被误关 → 打第 2 个字起自动编码停摆，
/// 用户实测「中→dg 有了，再打 心 还是 DG」的根因）。
static CODE_LAST_SET: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
use std::sync::atomic::Ordering;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ACTIVATE => {
            // 【词框聚焦 2026-09-12 v1.5.3+】小窗激活时把键盘焦点落
            // 到词框 EDIT——此前只在 WM_CREATE 里 SetFocus（窗口未
            // 激活时设置无效），激活后焦点停在顶层窗自身：用户在词框
            // 打虎码，键全部被宿主主文档的 TSF sink 吃掉——组段建在
            // 主文档（原光标处 preedit 残留+候选框弹在主文档旁），
            // 词框打不进字。标准对话框模式：激活即聚焦首控件。
            // 低字节 WA_ACTIVE/WA_CLICKACTIVE 均聚焦；WA_INACTIVE 跳过。
            let lo = (wp.0 & 0xFFFF) as u16;
            if lo != 0 {
                let fe = FIRST_EDIT
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .unwrap_or(0);
                if fe != 0 {
                    let _ = SetFocus(HWND(fe as *mut _));
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_CREATE => {
            let mk = |s: &str| -> Vec<u16> {
                let mut v: Vec<u16> = s.encode_utf16().collect();
                v.push(0);
                v
            };
            let mut first_edit = HWND::default();
            // 【加权模式 2026-09-06】两框：词 + 权重（编码由 server
            // 反查；不建选重位）；加词模式=原有三框
            let rows: Vec<(&str, i32, u32)> = if is_weight_mode() {
                vec![
                    ("词", ID_WORD, WS_TABSTOP.0 | 0x80u32),
                    ("权重", ID_CODE, WS_TABSTOP.0 | 0x80u32 | 0x2000u32),
                ]
            } else {
                // 行序保持 词→选重位→编码（用户流程拍板），仅标签精简。
                vec![
                    ("词", ID_WORD, WS_TABSTOP.0 | 0x80u32),
                    ("选重位", ID_POS, WS_TABSTOP.0 | 0x80u32 | 0x2000u32),
                    ("编码", ID_CODE, WS_TABSTOP.0 | 0x80u32),
                ]
            };
            // 【T4 根治·词框 RichEdit v2 2026-10-07】词框用 RichEdit50W
            //（TSF 原生通道：编码内联显示、上屏即时、无 CUAS 延迟/重放
            // 全套怪癖）。msftedit.dll 先显式载入；创建失败回退 EDIT +
            // v3c 特判路径（AW_RICH=false），双保险。每个子件建窗结果
            // 全量留痕（上轮闪框排查零证据，这轮必须带证据迭代）。
            {
                #[link(name = "kernel32")]
                unsafe extern "system" {
                    fn LoadLibraryW(name: *const u16) -> isize;
                }
                let mn: Vec<u16> = "msftedit.dll\0".encode_utf16().collect();
                let ok = unsafe { LoadLibraryW(mn.as_ptr()) } != 0;
                crate::tsf::trace(&format!("awRich msftedit 载入={ok}"));
            }
            for (i, (label, id, extra)) in rows.iter().enumerate() {
                let y = ROW_Y0 + i as i32 * ROW_PITCH;
                let lbl_txt = mk(label);
                // 【词框实时编码 2026-10-07】词行标签给 id：EDIT 回退模式
                // 下把编码尾巴显示在标签上（RichEdit 模式框内内联显示）。
                // 【2026-11-04】标签上移量 26→30：17px 粗体 cell 高 24，
                // LBL_H=26，标签底与卡片顶留 4px。
                let lbl = create_child(
                    hwnd,
                    w!("STATIC"),
                    PCWSTR(lbl_txt.as_ptr()),
                    WINDOW_EX_STYLE(0),
                    0,
                    M,
                    y - 30,
                    LBL_W,
                    LBL_H,
                    if *id == ID_WORD { AW_LIVE_LABEL } else { 0 },
                );
                set_ui_font(lbl, true);
                // 【三框统一 RichEdit 2026-10-07】外观/行为一致；词框
                // TSF 原生通道。RichEdit 不认 ES_NUMBER——选重位/权重
                // 框数字过滤改在 EN_UPDATE 里做。创建失败回退 EDIT。
                let mut ed = create_child(
                    hwnd,
                    w!("RICHEDIT50W"),
                    w!(""),
                    WINDOW_EX_STYLE(0),
                    *extra,
                    M + EDIT_INSET,
                    y + EDIT_DY,
                    EDIT_W,
                    EDIT_H,
                    *id,
                );
                if *id == ID_WORD {
                    crate::tsf::trace(&format!(
                        "awRich 词框建 {2} hwnd=0x{0:x} err={1}",
                        ed.0 as usize,
                        std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
                        if ed.0.is_null() { "失败" } else { "成功" }
                    ));
                    if ed.0.is_null() {
                        ed = create_child(
                            hwnd,
                            w!("EDIT"),
                            w!(""),
                            WINDOW_EX_STYLE(0),
                            *extra,
                            M + EDIT_INSET,
                            y + EDIT_DY,
                            EDIT_W,
                            EDIT_H,
                            *id,
                        );
                        AW_RICH.store(false, std::sync::atomic::Ordering::Relaxed);
                    } else {
                        AW_RICH.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                } else if ed.0.is_null() {
                    // 选重位/编码框 RichEdit 建失败——回退 EDIT（保数字
                    // 框 ES_NUMBER 生效；词框才是 RichEdit 关键路径）
                    ed = create_child(
                        hwnd,
                        w!("EDIT"),
                        w!(""),
                        WINDOW_EX_STYLE(0),
                        *extra,
                        M + EDIT_INSET,
                        y + EDIT_DY,
                        EDIT_W,
                        EDIT_H,
                        *id,
                    );
                }
                set_ui_font(ed, false);
                // 无边框输入框：左右留白 10px；RichEdit 另设背景色。
                let _ = SendMessageW(
                    ed,
                    0x00D3, /* EM_SETMARGINS */
                    WPARAM(3),
                    LPARAM(0x000A_000A),
                );
                let _ = SendMessageW(
                    ed,
                    0x0443, /* EM_SETBKGNDCOLOR */
                    WPARAM(0),
                    LPARAM(theme().card as isize),
                );
                set_rich_text_color(ed, theme().text);
                // 【EDIT 免 IME·分字段 2026-09-10】只有编码框（打什么出
                // 它，纯字母）与选重位/权重框（纯数字）断开输入上下文——
                // 在这些框打 jav/123 若唤起输入法会先出候选不上字母。
                // 【词框恢复 IME 2026-09-10 用户反馈】词框恰恰要打中文
                //（HuFu 自己在词框组段正常，也可切其他输入法）——此前
                // 一刀切全断导致词框打不了中文。
                // 动态取 ImmAssociateContext：mingw 交叉工具链无 imm32
                // import lib（链接报 cannot find -limm32），imm32.dll
                // 运行期必然在（IMM 子系统）——GetProcAddress 最稳。
                if *id == ID_CODE || *id == ID_POS {
                    #[link(name = "kernel32")]
                    unsafe extern "system" {
                        fn GetModuleHandleW(name: *const u16) -> isize;
                        fn GetProcAddress(
                            module: isize,
                            name: *const u8,
                        ) -> *const core::ffi::c_void;
                    }
                    unsafe {
                        let mn: Vec<u16> = "imm32.dll\0".encode_utf16().collect();
                        let md = GetModuleHandleW(mn.as_ptr());
                        if md != 0 {
                            let p =
                                GetProcAddress(md, c"ImmAssociateContext".as_ptr() as *const u8);
                            if !p.is_null() {
                                type Iac = unsafe extern "system" fn(isize, isize) -> isize;
                                let f: Iac = std::mem::transmute(p);
                                let _ = f(ed.0 as isize, 0);
                            }
                        }
                    }
                }
                if i == 0 {
                    first_edit = ed;
                }
            }
            // 【右键调频菜单 2026-10-06】预填三框（词/编码/选重位）：
            // 消费即清——窗口复用/复开不残留旧值。
            // 【v4·开窗重置自动态 2026-10-09】配合去掉「空值重新武装」：
            // 每次开窗 CODE_AUTO 回 true（上次会话手改编码不再拖累本次
            // 自动反查）；预填写编码触发的 EN_UPDATE 随即置 false（调用
            // 方给的编码视为手改——次序保证此语义不变）。
            CODE_AUTO.store(true, std::sync::atomic::Ordering::Relaxed);
            SET_CODE_ECHO.store(false, std::sync::atomic::Ordering::Relaxed);
            *CODE_LAST_SET.lock().unwrap_or_else(|p| p.into_inner()) = None;
            {
                let prefill = PREFILL.lock().unwrap_or_else(|p| p.into_inner()).take();
                if let Some((w, c, p)) = prefill.filter(|_| !is_weight_mode()) {
                    // 【预填次序 2026-10-08】先编码/选重位后词：编码预填
                    // 触发的 EN_UPDATE 会把 CODE_AUTO 置 false（调用方给
                    // 的编码视为手改），词框后填即不会再触发自动反查覆盖。
                    for (id, t) in [(ID_CODE, c), (ID_POS, p), (ID_WORD, w)] {
                        if let Ok(h) = GetDlgItem(hwnd, id) {
                            let v: Vec<u16> =
                                t.encode_utf16().chain([0]).collect();
                            let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                                h,
                                PCWSTR(v.as_ptr()),
                            );
                        }
                    }
                    crate::tsf::trace("addword 预填（右键调频菜单·加词）");
                }
            }
            // ── 2026-11-03 视觉重做：自绘 toggle / 预览标题 / 主次按钮 ──
            PH_ON.store(
                if !is_weight_mode() { ph_recall() } else { false },
                std::sync::atomic::Ordering::Relaxed,
            );
            if !is_weight_mode() {
                // BS_OWNERDRAW=0x0B；空格/点击仍走 WM_COMMAND(BN_CLICKED)。
                let _ = create_child(
                    hwnd,
                    w!("BUTTON"),
                    w!(""),
                    WINDOW_EX_STYLE(0),
                    WS_TABSTOP.0 | 0x0B,
                    M,
                    TOGGLE_Y,
                    TOGGLE_W,
                    TOGGLE_H,
                    ID_PH,
                );
            }
            let pv_title_txt = mk(if is_weight_mode() {
                "效果预览"
            } else {
                "候选预览"
            });
            let pv_title = create_child(
                hwnd,
                w!("STATIC"),
                PCWSTR(pv_title_txt.as_ptr()),
                WINDOW_EX_STYLE(0),
                0,
                M + 16,
                if is_weight_mode() {
                    W_PREVIEW_Y + 12
                } else {
                    PREVIEW_TITLE_Y
                },
                260,
                26,
                ID_PV_TITLE,
            );
            set_ui_font(pv_title, true);
            // 主/次按钮（owner-draw，Enter/Esc 仍走 IDOK/IDCANCEL）。
            let btn_y = (if is_weight_mode() {
                W_CLIENT_H
            } else {
                CLIENT_H
            }) - 24
                - BTN_H;
            for (_label, id) in [("确定", ID_OK), ("取消", ID_CANCEL)] {
                let _ = create_child(
                    hwnd,
                    w!("BUTTON"),
                    w!(""),
                    WINDOW_EX_STYLE(0),
                    WS_TABSTOP.0 | 0x0B,
                    if id == ID_OK { BTN_OK_X } else { BTN_CANCEL_X },
                    btn_y,
                    BTN_W,
                    BTN_H,
                    id,
                );
            }
            if !first_edit.0.is_null() {
                let _ = SetFocus(first_edit);
                // 【词框聚焦 2026-09-12】存静态：WM_ACTIVATE 激活时聚焦
                //（WM_CREATE 时窗口未激活 SetFocus 无效）。
                *FIRST_EDIT.lock().unwrap_or_else(|p| p.into_inner()) = Some(first_edit.0 as isize);
            }
            FOCUS_ID.store(ID_WORD, std::sync::atomic::Ordering::Relaxed);
            apply_theme_to_edits(hwnd, &theme());
            refresh_preview(hwnd);
            LRESULT(0)
        }
        WM_ERASEBKGND => {
            let brush = BG_BRUSH
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .map(|b| windows::Win32::Graphics::Gdi::HBRUSH(b as *mut _));
            if let Some(br) = brush {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let hdc = windows::Win32::Graphics::Gdi::HDC(wp.0 as *mut _);
                let _ = windows::Win32::Graphics::Gdi::FillRect(hdc, &rc, br);
                LRESULT(1)
            } else {
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            paint_scene(hwnd, hdc);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_DRAWITEM => {
            let dis = &*(lp.0 as *const DrawItem);
            paint_item(dis);
            LRESULT(1)
        }
        WM_SIZE => {
            let mut rc = RECT::default();
            if GetClientRect(hwnd, &mut rc).is_ok() {
                let rgn = CreateRoundRectRgn(0, 0, rc.right + 1, rc.bottom + 1, 18, 18);
                let _ = SetWindowRgn(hwnd, rgn, true);
            }
            LRESULT(0)
        }
        WM_NCHITTEST => {
            // 顶部 68px 可拖拽；关闭 × 区域留给 WM_LBUTTONUP。
            let sx = (lp.0 & 0xffff) as i16 as i32;
            let sy = ((lp.0 >> 16) & 0xffff) as i16 as i32;
            let mut wr = RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            let cx = sx - wr.left;
            let cy = sy - wr.top;
            if cy < 68 {
                if (WIN_W - 46..=WIN_W - 14).contains(&cx) && (10..=48).contains(&cy) {
                    return LRESULT(HTCLIENT as isize);
                }
                return LRESULT(HTCAPTION as isize);
            }
            LRESULT(HTCLIENT as isize)
        }
        WM_LBUTTONUP => {
            let x = (lp.0 & 0xffff) as i16 as i32;
            let y = ((lp.0 >> 16) & 0xffff) as i16 as i32;
            if (WIN_W - 46..=WIN_W - 14).contains(&x) && (10..=48).contains(&y) {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        m if m == windows::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE
            || m == windows::Win32::UI::WindowsAndMessaging::WM_THEMECHANGED =>
        {
            // 系统浅色/深色切换：立刻重读并刷新窗口（无需重开）。
            refresh_theme_now();
            reset_brushes();
            let th = theme();
            ensure_brushes(&th);
            apply_theme_to_edits(hwnd, &th);
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, true);
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC => {
            let ctrl = HWND(lp.0 as _);
            let cid = GetWindowLongPtrW(ctrl, GWLP_ID) as i32;
            unsafe { ensure_brushes(&theme()) };
            let th = theme();
            let hdc = windows::Win32::Graphics::Gdi::HDC(wp.0 as _);
            let (fg, bg, brush) = if cid >= ID_AFT_NEW {
                // 胶囊由父窗口自绘；文字控件返回空刷子保持透明，露出圆角底。
                let _ = SetBkMode(hdc, TRANSPARENT);
                (th.chip_fg, th.preview, Some(GetStockObject(NULL_BRUSH).0 as usize))
            } else if cid == ID_PV_HEAD_CUR || cid == ID_PV_HEAD_AFT {
                // 【P2-4 2026-11-03】头部判定必须放在 `>= ID_CUR_BASE`
                // 之前：ID_PV_HEAD_AFT=2190 也是 >=2000，旧顺序会把它
                // 当普通候选（"加入后"标题恒亮一档）。
                (th.muted, th.preview, *SKIN_BRUSH.lock().unwrap_or_else(|p| p.into_inner()))
            } else if cid >= ID_AFT_PH {
                // 占位项 ③④…：标签色压暗（与真候选区分）
                (th.muted, th.preview, *SKIN_BRUSH.lock().unwrap_or_else(|p| p.into_inner()))
            } else if cid >= ID_CUR_BASE {
                (th.text, th.preview, *SKIN_BRUSH.lock().unwrap_or_else(|p| p.into_inner()))
            } else if cid == ID_PV_TITLE {
                (th.text, th.card, *FIELD_BRUSH.lock().unwrap_or_else(|p| p.into_inner()))
            } else if cid == ID_PV_MSG {
                (th.muted, th.preview, *SKIN_BRUSH.lock().unwrap_or_else(|p| p.into_inner()))
            } else {
                (th.muted, th.bg, *BG_BRUSH.lock().unwrap_or_else(|p| p.into_inner()))
            };
            let _ = windows::Win32::Graphics::Gdi::SetTextColor(hdc, COLORREF(fg));
            let _ = windows::Win32::Graphics::Gdi::SetBkColor(hdc, COLORREF(bg));
            if let Some(b) = brush {
                return LRESULT(b as isize);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_CTLCOLORBTN => {
            // 【2026-11-04 深色四角修复】owner-draw BUTTON 的背景擦除仍要
            // 问父窗口取画刷；不处理时走 DefWindowProc 回系统面色（系统
            // 浅色主题下=白），圆角外四个角就露白。这里统一回窗口底色
            // BG_BRUSH（缓存刷子，禁止删除），与 paint_item 的整块铺底
            // 互为兜底。
            unsafe { ensure_brushes(&theme()) };
            let th = theme();
            let hdc = windows::Win32::Graphics::Gdi::HDC(wp.0 as _);
            let _ = SetBkMode(hdc, TRANSPARENT);
            let _ = windows::Win32::Graphics::Gdi::SetBkColor(hdc, COLORREF(th.bg));
            if let Some(b) = *BG_BRUSH.lock().unwrap_or_else(|p| p.into_inner()) {
                return LRESULT(b as isize);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_CTLCOLOREDIT => {
            unsafe { ensure_brushes(&theme()) };
            let th = theme();
            let hdc = windows::Win32::Graphics::Gdi::HDC(wp.0 as _);
            let _ = windows::Win32::Graphics::Gdi::SetTextColor(hdc, COLORREF(th.text));
            let _ = windows::Win32::Graphics::Gdi::SetBkColor(hdc, COLORREF(th.card));
            if let Some(b) = *FIELD_BRUSH.lock().unwrap_or_else(|p| p.into_inner()) {
                return LRESULT(b as isize);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        // 【T4 v5】回灌链：第一拍（AW_FLUSH→30ms→pending 转正）、
        // 第二拍（AW_TAIL→30ms→补建尾巴段）。
        m if m == AW_FLUSH_MSG || m == AW_TAIL_MSG => {
            let tail = m == AW_TAIL_MSG;
            let _ = unsafe {
                windows::Win32::UI::WindowsAndMessaging::SetTimer(
                    hwnd,
                    if tail { AW_TAIL_TIMER } else { AW_FLUSH_TIMER },
                    30,
                    None,
                )
            };
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_TIMER => {
            let id = wp.0 as usize;
            if id == AW_FLUSH_TIMER {
                let _ = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::KillTimer(hwnd, AW_FLUSH_TIMER)
                };
                crate::tsf::aw_flush_pending();
                LRESULT(0)
            } else if id == AW_TAIL_TIMER {
                let _ = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::KillTimer(hwnd, AW_TAIL_TIMER)
                };
                crate::tsf::aw_flush_tail();
                LRESULT(0)
            } else {
                // 【RichEdit 2026-10-07】其余定时器必须还給 DefWindowProc
                // ——RichEdit 内部靠定时器（光标闪烁/布局），全吞=闪框。
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
        WM_COMMAND => {
            let id = (wp.0 as u32 & 0xFFFF) as i32;
            let notif = (wp.0 as u32 >> 16) as u32;
            // 输入框焦点 → 卡片描边高亮（EN_SETFOCUS=0x100 / EN_KILLFOCUS=0x200）
            if notif == 0x0100 && (id == ID_WORD || id == ID_CODE || id == ID_POS) {
                FOCUS_ID.store(id, std::sync::atomic::Ordering::Relaxed);
                let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, false);
            } else if notif == 0x0200
                && FOCUS_ID.load(std::sync::atomic::Ordering::Relaxed) == id
            {
                FOCUS_ID.store(0, std::sync::atomic::Ordering::Relaxed);
                let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, false);
            }
            // 【Tab 到编码框光标置尾 2026-10-09】EN_SETFOCUS(0x0100) 时
            // EM_SETSEL(尾,尾)：Tab 进编码框光标落在自动编码之后——
            // 不满意直接退格改，不用先按 End/点框尾（用户规格）。
            // 鼠标点击入框时本消息先行、点击定位随后覆盖，互不干扰。
            if id == ID_CODE && notif == 0x0100 {
                if let Ok(h) = GetDlgItem(hwnd, ID_CODE) {
                    let n = unsafe { GetWindowTextLengthW(h) };
                    let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                        h,
                        0x00B1, /* EM_SETSEL */
                        WPARAM(n as usize),
                        LPARAM(n as isize),
                    );
                }
                return LRESULT(0);
            }
            // EN_UPDATE=0x400：文本每变一次立即刷（0x200 是 EN_KILLFOCUS，
            // 上版误用导致「光标移走才刷新」）
            if (id == ID_CODE || id == ID_POS || id == ID_WORD) && notif == ID_EN_UPDATE {
                let rd = |i: i32| -> String {
                    match GetDlgItem(hwnd, i) {
                        Ok(h) => {
                            let n = GetWindowTextLengthW(h);
                            if n <= 0 {
                                return String::new();
                            }
                            let mut b = vec![0u16; n as usize + 1];
                            let g = GetWindowTextW(h, &mut b);
                            String::from_utf16_lossy(&b[..g.max(0) as usize])
                        }
                        Err(_) => String::new(),
                    }
                };
                let (wv, cv, pv) = (rd(ID_WORD), rd(ID_CODE), rd(ID_POS));
                // 【程序化写编码框的回声 2026-10-08】SET_CODE_ECHO 事件
                // （我们自己 SetWindowTextW 触发）不算用户手改，编码保
                // 持自动态，直接刷预览。
                if id == ID_CODE && SET_CODE_ECHO.swap(false, Ordering::Relaxed) {
                    unsafe { refresh_preview(hwnd) };
                    return LRESULT(0);
                }
                if id == ID_CODE {
                    // 【空格泄漏防御 2026-10-09】真机取证：/jc 上屏空格会经
                    // 词框 TIP sink 双投——开窗 1 秒后编码框出现「 」，
                    // ≠CODE_LAST_SET 被误判手改 → CODE_AUTO=false → 自动
                    // 编码死（旧版靠「空值重新武装」硬撑，连带删空回填
                    // bug）。编码框合法内容无空格：纯空白=泄漏，清空且
                    // 不算手改。
                    if !cv.is_empty() && cv.trim().is_empty() {
                        if let Ok(h) = GetDlgItem(hwnd, ID_CODE) {
                            SET_CODE_ECHO.store(true, Ordering::Relaxed);
                            *CODE_LAST_SET
                                .lock()
                                .unwrap_or_else(|p| p.into_inner()) = Some(String::new());
                            let _ =
                                windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                                    h,
                                    PCWSTR([0u16].as_ptr()),
                                );
                            return LRESULT(0);
                        }
                    }
                    // 用户手改编码 → 停止自动跟踪（清空编码框即恢复）。
                    // 【v3】文本==最后程序化写入值 = 预览销毁扰动重发的
                    // 回声（旗标已被第一条消费），不算手改。
                    // 【v5·空值不算手改 2026-10-09】真机取证：开窗首个
                    // EN_UPDATE 常为空文本（控件创建/回声），对照
                    // CODE_LAST_SET=None 必不等 → 误判手改 → 自动编码
                    // 开窗即死（旧版靠「空值重新武装」续命，连带删空
                    // 回填 bug）。空编码框不构成任何「改」——永不置
                    // false；非空且≠最后程序化值才是手改。
                    let storm_echo = cv.trim().is_empty() || {
                        let last = CODE_LAST_SET.lock().unwrap_or_else(|p| p.into_inner());
                        last.as_deref() == Some(cv.trim())
                    };
                    if !storm_echo {
                        CODE_AUTO.store(false, Ordering::Relaxed);
                    }
                }
                // 【自动填写编码·实时跟踪 2026-10-08 v2】词框每变一次
                // （加字/减字/清空）都按当前词重算编码：自动态（编码框
                // 空、或内容系自动填的）持续覆盖更新；用户手改过编码则
                // 不再覆盖，清空编码框即恢复自动。首版「只填一次」不符
                // 合打字节奏（用户反馈：打第 2 个字、回删 1 个字编码都
                // 不更新）。
                // 【v4·空值不再重新武装 2026-10-09】用户实锤：手删编码
                // 删到空的一瞬间自动编码又回来了（RichEdit 风暴重发的
                // 词框 EN_UPDATE 带「编码已空」命中 `|| cv.is_empty()`
                // 重新武装 → 立即回填，只能 Ctrl+A 重打）。去掉空值
                // 重新武装：手改（含删空）后自动永久停（重开窗重置，
                // 见创建处）。
                if !is_weight_mode() && id == ID_WORD {
                    let auto = CODE_AUTO.load(Ordering::Relaxed);
                    let wtrim = wv.trim().to_string();
                    if auto {
                        if wtrim.is_empty() {
                            // 词清空 → 自动态下编码框同步清空（回声事件刷预览）
                            if !cv.is_empty() {
                                if let Ok(h) = GetDlgItem(hwnd, ID_CODE) {
                                    SET_CODE_ECHO.store(true, Ordering::Relaxed);
                                    *CODE_LAST_SET
                                        .lock()
                                        .unwrap_or_else(|p| p.into_inner()) = Some(String::new());
                                    let _ =
                                        windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                                            h,
                                            PCWSTR([0u16].as_ptr()),
                                        );
                                    return LRESULT(0);
                                }
                            }
                        } else if let Some(hint) = word_code(&wtrim) {
                            if hint != cv.trim() {
                                if let Ok(h) = GetDlgItem(hwnd, ID_CODE) {
                                    let v: Vec<u16> =
                                        hint.encode_utf16().chain([0]).collect();
                                    SET_CODE_ECHO.store(true, Ordering::Relaxed);
                                    CODE_AUTO.store(true, Ordering::Relaxed);
                                    *CODE_LAST_SET
                                        .lock()
                                        .unwrap_or_else(|p| p.into_inner()) = Some(hint.clone());
                                    crate::tsf::trace(&format!(
                                        "addword: 自动编码 「{wtrim}」→ {hint}"
                                    ));
                                    let _ =
                                        windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                                            h,
                                            PCWSTR(v.as_ptr()),
                                        );
                                    // 触发的 ID_CODE 回声事件会带新编码刷预览。
                                    return LRESULT(0);
                                }
                            }
                        }
                        // hint 查不到（如生僻内容）→ 保留现值，落到下面
                        // 统一预览刷新。
                    }
                }
                // 【RichEdit 数字过滤 2026-10-07】RichEdit 不认
                // ES_NUMBER——选重位/加权权重框在 EN_UPDATE 剔除非
                // 数字（写回后光标置尾）。
                if id == ID_POS || (id == ID_CODE && is_weight_mode()) {
                    let src = if id == ID_POS { &pv } else { &cv };
                    let filtered: String =
                        src.chars().filter(|c| c.is_ascii_digit()).collect();
                    if filtered != *src {
                        if let Ok(h) = GetDlgItem(hwnd, id) {
                            let v: Vec<u16> =
                                filtered.encode_utf16().chain([0]).collect();
                            let _ =
                                windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(
                                    h,
                                    PCWSTR(v.as_ptr()),
                                );
                            let n = filtered.encode_utf16().count() as i32;
                            let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                                h,
                                0x00B1, /* EM_SETSEL */
                                WPARAM(n as usize),
                                LPARAM(n as isize),
                            );
                            return LRESULT(0);
                        }
                    }
                }
                // 【RichEdit 死循环掐断 2026-10-07】RichEdit 对邻近子窗
                // 扰动（预览区销毁/重建 STATIC）会重发 EN_UPDATE——每
                // 2-3ms 一环。三框文本没变就不重刷预览。
                static LAST_TXT: std::sync::Mutex<(String, String, String)> =
                    std::sync::Mutex::new((
                        String::new(),
                        String::new(),
                        String::new(),
                    ));
                {
                    let mut last = LAST_TXT.lock().unwrap_or_else(|p| p.into_inner());
                    if *last == (wv.clone(), cv.clone(), pv.clone()) {
                        return LRESULT(0);
                    }
                    *last = (wv, cv, pv);
                }
                unsafe { refresh_preview(hwnd) };
            }
            // 【占位符勾选 2026-10-08】勾/去勾即时重排预览（BN_CLICKED=0）
            if id == ID_PH && notif == 0 {
                let checked = !PH_ON.load(std::sync::atomic::Ordering::Relaxed);
                PH_ON.store(checked, std::sync::atomic::Ordering::Relaxed);
                crate::tsf::trace(&format!("addword: 占位符 toggle={checked}"));
                ph_remember(checked);
                unsafe { refresh_preview(hwnd) };
                return LRESULT(0);
            }
            let want = (id == ID_OK || id == ID_CANCEL || id == 1 || id == 2) && notif == 0;
            if want {
                if id == ID_OK || id == 1 {
                    unsafe { submit(hwnd) };
                }
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // 【T4 v3】窗毁清 pending/tail（防跨窗残留）
            let _ = crate::tsf::aw_clear_pending();
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn set_item_font(h: HWND, is_label: bool) {
    let store = if is_label {
        FONT_LABEL.lock().unwrap_or_else(|p| p.into_inner())
    } else {
        FONT_MAIN.lock().unwrap_or_else(|p| p.into_inner())
    };
    if let Some(f) = *store {
        let _ = SendMessageW(h, WM_SETFONT, WPARAM(f), LPARAM(1));
    }
}

/// 固定 UI 字体（标签/输入框/按钮）。
unsafe fn set_ui_font(h: HWND, bold: bool) {
    let store = if bold {
        FONT_UI_BOLD.lock().unwrap_or_else(|p| p.into_inner())
    } else {
        FONT_UI.lock().unwrap_or_else(|p| p.into_inner())
    };
    if let Some(f) = *store {
        let _ = SendMessageW(h, WM_SETFONT, WPARAM(f), LPARAM(1));
    }
}

/// ── 2026-11-03 视觉重做：GDI 自绘基础 ──
unsafe fn ui_font(bold: bool) -> Option<usize> {
    let g = if bold {
        FONT_UI_BOLD.lock().unwrap_or_else(|p| p.into_inner())
    } else {
        FONT_UI.lock().unwrap_or_else(|p| p.into_inner())
    };
    *g
}

unsafe fn title_font() -> Option<usize> {
    *FONT_TITLE.lock().unwrap_or_else(|p| p.into_inner())
}
unsafe fn small_font() -> Option<usize> {
    *FONT_SMALL.lock().unwrap_or_else(|p| p.into_inner())
}

/// 用指定字体画一行文字，返回显示宽（px）。
unsafe fn draw_text(
    hdc: HDC,
    s: &str,
    x: i32,
    y: i32,
    font: Option<usize>,
    color: u32,
) -> i32 {
    if s.is_empty() {
        return 0;
    }
    let old = match font {
        Some(f) => SelectObject(hdc, HGDIOBJ(f as *mut core::ffi::c_void)),
        None => HGDIOBJ::default(),
    };
    let _ = SetTextColor(hdc, COLORREF(color));
    let _ = SetBkMode(hdc, TRANSPARENT);
    let t = utf16z(s);
    let _ = TextOutW(hdc, x, y, &t[..t.len() - 1]);
    let mut sz = windows::Win32::Foundation::SIZE::default();
    let w = if GetTextExtentPoint32W(hdc, &t[..t.len() - 1], &mut sz).as_bool() {
        sz.cx
    } else {
        0
    };
    if !old.0.is_null() {
        SelectObject(hdc, old);
    }
    w
}

unsafe fn measure_ui_w(hdc: HDC, s: &str, font: Option<usize>) -> i32 {
    if s.is_empty() {
        return 0;
    }
    let old = match font {
        Some(f) => SelectObject(hdc, HGDIOBJ(f as *mut core::ffi::c_void)),
        None => HGDIOBJ::default(),
    };
    let t = utf16z(s);
    let mut sz = windows::Win32::Foundation::SIZE::default();
    let w = if GetTextExtentPoint32W(hdc, &t[..t.len() - 1], &mut sz).as_bool() {
        sz.cx
    } else {
        0
    };
    if !old.0.is_null() {
        SelectObject(hdc, old);
    }
    w
}

unsafe fn draw_text_center(
    hdc: HDC,
    s: &str,
    left: i32,
    top: i32,
    w: i32,
    h: i32,
    font: Option<usize>,
    px: i32,
    color: u32,
) {
    let tw = measure_ui_w(hdc, s, font);
    let _ = draw_text(hdc, s, left + (w - tw).max(0) / 2, top + (h - px).max(0) / 2 - 1, font, color);
}

unsafe fn draw_text_right(
    hdc: HDC,
    s: &str,
    right: i32,
    y: i32,
    font: Option<usize>,
    color: u32,
) {
    let tw = measure_ui_w(hdc, s, font);
    let _ = draw_text(hdc, s, right - tw, y, font, color);
}

/// 圆角矩形填充+1px 描边（fill=0 表示不填；border=0 表示不描边）。
unsafe fn round_box(
    hdc: HDC,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    r: i32,
    fill: u32,
    border: u32,
) {
    let br = if fill != 0 {
        CreateSolidBrush(COLORREF(fill))
    } else {
        HBRUSH(GetStockObject(NULL_BRUSH).0)
    };
    let pen = if border != 0 {
        CreatePen(PS_SOLID, 1, COLORREF(border))
    } else {
        windows::Win32::Graphics::Gdi::HPEN(GetStockObject(NULL_PEN).0)
    };
    let ob = SelectObject(hdc, HGDIOBJ(br.0));
    let op = SelectObject(hdc, HGDIOBJ(pen.0));
    let _ = RoundRect(hdc, x, y, x + w, y + h, r, r);
    SelectObject(hdc, op);
    SelectObject(hdc, ob);
    if fill != 0 {
        let _ = DeleteObject(HGDIOBJ(br.0));
    }
    if border != 0 {
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }
}

unsafe fn hline(hdc: HDC, x1: i32, x2: i32, y: i32, color: u32) {
    let br = CreateSolidBrush(COLORREF(color));
    let rc = RECT { left: x1, top: y, right: x2, bottom: y + 1 };
    let _ = FillRect(hdc, &rc, br);
    let _ = DeleteObject(HGDIOBJ(br.0));
}

unsafe fn weight_now(hwnd: HWND) -> i64 {
    let mut n = 0i64;
    if let Ok(h) = GetDlgItem(hwnd, if is_weight_mode() { ID_CODE } else { ID_POS }) {
        let len = GetWindowTextLengthW(h);
        if len > 0 {
            let mut b = vec![0u16; len as usize + 1];
            let g = GetWindowTextW(h, &mut b);
            n = String::from_utf16_lossy(&b[..g.max(0) as usize])
                .trim()
                .parse()
                .unwrap_or(0);
        }
    }
    n
}

unsafe fn paint_scene(hwnd: HWND, hdc: HDC) {
    let mut rc = RECT::default();
    if GetClientRect(hwnd, &mut rc).is_err() {
        return;
    }
    let (cw, ch) = (rc.right, rc.bottom);
    let th = theme();
    unsafe { ensure_brushes(&th) };
    let (accent, pv_bg, pv_label) = (th.accent, th.preview, th.muted);
    let weight = is_weight_mode();
    // 底
    let bg = CreateSolidBrush(COLORREF(th.bg));
    let _ = FillRect(hdc, &rc, bg);
    let _ = DeleteObject(HGDIOBJ(bg.0));
    // 抬头强调条 + 标题/副标题
    // 【2026-11-04】标题 26px（cell 34）y=10；副标题 14px（cell 20）y=46；
    // 分隔线 74——三段各留 2~8px 不重叠。
    let bar = CreateSolidBrush(COLORREF(accent));
    let _ = FillRect(hdc, &RECT { left: M, top: 20, right: M + 3, bottom: 46 }, bar);
    let _ = DeleteObject(HGDIOBJ(bar.0));
    let _ = draw_text(hdc, if weight { "加权" } else { "加词" }, M + 12, 10, title_font(), th.text);
    let _ = draw_text(
        hdc,
        if weight { "调整已有词的候选优先级" } else { "把新词放进候选顺序" },
        M + 12,
        46,
        small_font(),
        th.muted,
    );
    // 关闭 ×（点击热区 WIN_W-46..WIN_W-14, y 10..48）
    let _ = draw_text(hdc, "×", WIN_W - 42, 10, title_font(), th.muted);
    hline(hdc, M, WIN_W - M, 74, th.border);

    // 字段卡
    let focus = FOCUS_ID.load(std::sync::atomic::Ordering::Relaxed);
    let n_rows = if weight { 2 } else { 3 };
    for i in 0..n_rows {
        let y = ROW_Y0 + i * ROW_PITCH;
        let id = if i == 0 {
            ID_WORD
        } else if weight {
            ID_CODE
        } else if i == 1 {
            ID_POS
        } else {
            ID_CODE
        };
        let border = if focus == id { accent } else { th.border };
        round_box(hdc, M, y, CONTENT_W, CARD_H, 10, th.card, border);
    }
    // 字段右侧提示（14px，cell 20；与粗体标签基线对齐）
    let hint_y = |i: i32| ROW_Y0 + i * ROW_PITCH - 28;
    let hint_x_right = M + CONTENT_W;
    let hints: [&str; 3] = if weight {
        ["中文 / 英文均可", "缺省 1000，越大越靠前", ""]
    } else {
        ["中文 / 英文均可", "留空 = 首选", "自动跟随词，可手改"]
    };
    for (i, h) in hints.iter().enumerate() {
        if h.is_empty() {
            continue;
        }
        draw_text_right(hdc, h, hint_x_right, hint_y(i as i32), small_font(), th.muted);
    }
    if !weight {
        // 占位符开关（按钮本身自绘）+ 说明（14px；右侧说明宽 307px，
        // TOGGLE_W 放大会顶到右边缘，故 toggle 宽 176 时说明左缘 212）
        let _ = draw_text(
            hdc,
            "选重位超过现有候选时，用 ③ ④ … 补齐前序空位",
            M + TOGGLE_W + 12,
            TOGGLE_Y + 9,
            small_font(),
            th.muted,
        );
    }

    // 预览卡
    let footer_top = ch - 80;
    let pv_y = if weight { W_PREVIEW_Y } else { PREVIEW_Y };
    let pv_bottom = footer_top - 12;
    let pv_h = (pv_bottom - pv_y).max(80);
    round_box(hdc, M, pv_y, CONTENT_W, pv_h, 12, th.card, th.border);
    // 预览内层（真实皮肤底）——标题行 17px 粗体 cell 24 + 12 上边距，
    // 内层顶从 pv_y+52 起（原 +44）。
    let body_y = pv_y + 52;
    let body_h = (pv_bottom - body_y - 12).max(40);
    round_box(hdc, M + 12, body_y, CONTENT_W - 24, body_h, 8, pv_bg, th.border);
    if weight {
        // 右对齐图例
        let _ = draw_text_right(hdc, "权重越高，越靠前", M + CONTENT_W - 16, pv_y + 15, small_font(), th.muted);
        // 简单效果示意：词 → 前部
        // 【2026-11-04】字号放大后长词会横穿预览卡右缘：先按像素宽裁成 …
        //（右侧给「→ 候选前部」留位），再画。
        let wtext = weight_word(hwnd);
        let raw = if wtext.is_empty() { "（输入要加权的词）" } else { wtext.as_str() };
        let mw = |s: &str, f: Option<usize>, em: i32| -> i32 {
            match f {
                Some(_) => measure_ui_w(hdc, s, f).max(text_w(s, em)),
                None => text_w(s, em),
            }
        };
        let (wd, _) = fit_text(&mw, raw, ui_font(true), 17, (CONTENT_W - 168).max(80));
        let w = wd.as_str();
        let ww = measure_ui_w(hdc, w, ui_font(true));
        let _ = draw_text(hdc, w, M + 28, body_y + 16, ui_font(true), th.text);
        let _ = draw_text(hdc, "→", M + 28 + ww + 10, body_y + 16, ui_font(false), accent);
        let _ = draw_text(hdc, "候选前部", M + 28 + ww + 42, body_y + 16, ui_font(false), th.muted);
        let _ = draw_text(hdc, "确定后自动反查编码，并按权重排到候选前部。", M + 28, body_y + 52, small_font(), pv_label);
    } else {
        let _ = draw_text_right(hdc, "现有 → 加入后", M + CONTENT_W - 16, pv_y + 15, small_font(), th.muted);
        // 新词胶囊：父窗口先画圆角底，STATIC 文字以 NULL_BRUSH 透明叠上。
        let rects = NEW_CHIP_RECTS.lock().unwrap_or_else(|p| p.into_inner());
        for (cx, cy, cw, chh) in rects.iter() {
            round_box(hdc, *cx, *cy, *cw, *chh, (*chh / 2).max(4), th.chip_bg, 0);
        }
    }

    // 加权刻度
    if weight {
        round_box(hdc, M, SCALE_Y, CONTENT_W, SCALE_H, 12, th.card, th.border);
        let wv = weight_now(hwnd);
        let _ = draw_text(hdc, "权重", M + 16, SCALE_Y + 10, small_font(), th.muted);
        let val = if wv == 0 { "1000（默认）".to_string() } else { format!("{wv}") };
        let _ = draw_text_right(hdc, &val, M + CONTENT_W - 16, SCALE_Y + 8, ui_font(true), accent);
        let tx1 = M + 16;
        let tx2 = M + CONTENT_W - 16;
        let ty = SCALE_Y + 48;
        let track = CreateSolidBrush(COLORREF(th.border));
        let _ = FillRect(hdc, &RECT { left: tx1, top: ty, right: tx2, bottom: ty + 6 }, track);
        let _ = DeleteObject(HGDIOBJ(track.0));
        let v = if wv <= 0 { 1000 } else { wv.clamp(0, 3000) };
        let mx = tx1 + ((tx2 - tx1) as f32 * (v as f32 / 3000.0)) as i32;
        let fill = CreateSolidBrush(COLORREF(accent));
        let _ = FillRect(hdc, &RECT { left: tx1, top: ty, right: mx.max(tx1 + 2), bottom: ty + 6 }, fill);
        let _ = DeleteObject(HGDIOBJ(fill.0));
        round_box(hdc, mx - 6, ty - 5, 12, 16, 5, accent, accent);
        let _ = draw_text(hdc, "低", tx1, SCALE_Y + 62, small_font(), th.muted);
        let _ = draw_text_center(hdc, "1000 · 默认", tx1, SCALE_Y + 62, tx2 - tx1, 20, small_font(), 20, th.muted);
        let _ = draw_text_right(hdc, "高", tx2, SCALE_Y + 62, small_font(), th.muted);
    }

    // 底栏
    hline(hdc, M, WIN_W - M, footer_top, th.border);
    // 两行键盘提示（单行会与右侧按钮重叠）：开关/确定/取消都可全键盘。
    if weight {
        let _ = draw_text(hdc, "Tab 切换", M, ch - 44, small_font(), th.muted);
        let _ = draw_text(hdc, "Enter 确定 · Esc 取消", M, ch - 24, small_font(), th.muted);
    } else {
        let _ = draw_text(hdc, "Tab 切换 · 空格 勾选占位符", M, ch - 44, small_font(), th.muted);
        let _ = draw_text(hdc, "Enter 确定 · Esc 取消", M, ch - 24, small_font(), th.muted);
    }
    // 外框
    round_box(hdc, 0, 0, cw, ch, 14, 0, th.border);
}

unsafe fn weight_word(hwnd: HWND) -> String {
    if let Ok(h) = GetDlgItem(hwnd, ID_WORD) {
        let n = GetWindowTextLengthW(h);
        if n > 0 {
            let mut b = vec![0u16; n as usize + 1];
            let g = GetWindowTextW(h, &mut b);
            return String::from_utf16_lossy(&b[..g.max(0) as usize]);
        }
    }
    String::new()
}

/// WM_DRAWITEM 的 DRAWITEMSTRUCT 本地镜像（避免额外 windows feature）。
#[repr(C)]
struct DrawItem {
    ctl_type: u32,
    ctl_id: u32,
    item_id: u32,
    item_action: u32,
    item_state: u32,
    hwnd_item: isize,
    hdc: HDC,
    rc_item: RECT,
    item_data: usize,
}

/// WM_DRAWITEM：主/次按钮 + 占位符 toggle。
unsafe fn paint_item(dis: &DrawItem) {
    let hdc = dis.hdc;
    let rc = dis.rc_item;
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;
    let selected = dis.item_state & 1 != 0; // ODS_SELECTED
    let disabled = dis.item_state & 4 != 0; // ODS_DISABLED
    let focused = dis.item_state & 16 != 0; // ODS_FOCUS
    let th = theme();
    unsafe { ensure_brushes(&th) };
    let accent = th.accent;
    // 【2026-11-04 深色四角修复】owner-draw 按钮只画圆角盒时，圆角以外
    // 的四个角没有像素：BUTTON 子控件的默认擦除用系统面色（系统浅色
    // 主题下=白），深色底上就露出白角。先整块铺父窗口底色再画圆角盒。
    // 刷子用 ensure_brushes 的缓存 BG_BRUSH（与窗口底同色，禁止删除）。
    match *BG_BRUSH.lock().unwrap_or_else(|p| p.into_inner()) {
        Some(b) => {
            let _ = FillRect(hdc, &rc, HBRUSH(b as *mut core::ffi::c_void));
        }
        None => {
            let b = CreateSolidBrush(COLORREF(th.bg));
            let _ = FillRect(hdc, &rc, b);
            let _ = DeleteObject(HGDIOBJ(b.0));
        }
    }
    match dis.ctl_id as i32 {
        ID_OK => {
            let fill = if disabled {
                th.border
            } else {
                accent
            };
            round_box(hdc, rc.left, rc.top, w, h, 10, fill, fill);
            let fg = if disabled { th.muted } else { contrast_fg(accent) };
            // 17px 粗体 cell 高 24（居中用实测行高，别用旧的 15）
            draw_text_center(hdc, "确定", rc.left, rc.top, w, h, ui_font(true), 24, fg);
            if focused && !disabled {
                round_box(hdc, rc.left + 3, rc.top + 3, w - 6, h - 6, 8, 0, th.accent_fg);
            }
        }
        ID_CANCEL => {
            let fill = if selected { th.border } else { th.card };
            round_box(hdc, rc.left, rc.top, w, h, 10, fill, th.border);
            draw_text_center(hdc, "取消", rc.left, rc.top, w, h, ui_font(false), 23, th.text);
            if focused {
                round_box(hdc, rc.left + 3, rc.top + 3, w - 6, h - 6, 8, 0, accent);
            }
        }
        ID_PH => {
            let on = PH_ON.load(std::sync::atomic::Ordering::Relaxed);
            if on {
                round_box(hdc, rc.left, rc.top, w, h, 19, th.card, accent);
                let bx = rc.left + 10;
                let by = rc.top + (h - 20) / 2;
                round_box(hdc, bx, by, 20, 20, 5, accent, accent);
                let _ = draw_text_center(hdc, "✓", bx, by, 20, 20, small_font(), 20, th.accent_fg);
            } else {
                round_box(hdc, rc.left, rc.top, w, h, 19, th.card, th.border);
                let bx = rc.left + 10;
                let by = rc.top + (h - 20) / 2;
                round_box(hdc, bx, by, 20, 20, 5, th.bg, th.border);
            }
            let fg = if on { th.text } else { th.muted };
            let _ = draw_text(hdc, "占位符补位", rc.left + 40, rc.top + (h - 23) / 2, ui_font(false), fg);
            if focused {
                round_box(hdc, rc.left + 3, rc.top + 3, w - 6, h - 6, 17, 0, accent);
            }
        }
        _ => {}
    }
}

/// RichEdit 文字颜色（WM_CTLCOLOREDIT 只管背景，字色需 CHARFORMAT）。
unsafe fn set_rich_text_color(h: HWND, color: u32) {
    #[repr(C)]
    struct CharFormatW {
        cb_size: u32,
        mask: u32,
        effects: u32,
        height: i32,
        offset: i32,
        text_color: u32,
        charset: u8,
        pitch: u8,
        face: [u16; 32],
    }
    let mut cf = CharFormatW {
        cb_size: std::mem::size_of::<CharFormatW>() as u32,
        mask: 0x4000_0000, // CFM_COLOR
        effects: 0,
        height: 0,
        offset: 0,
        text_color: color,
        charset: 0,
        pitch: 0,
        face: [0; 32],
    };
    let _ = SendMessageW(
        h,
        0x0444, /* EM_SETCHARFORMAT */
        WPARAM(4), /* SCF_ALL */
        LPARAM(&mut cf as *mut _ as isize),
    );
}

/// 新词项字体（加粗+下划线）。
unsafe fn set_new_font(h: HWND) {
    if let Some(f) = *FONT_NEW.lock().unwrap_or_else(|p| p.into_inner()) {
        let _ = SendMessageW(h, WM_SETFONT, WPARAM(f), LPARAM(1));
    }
}

/// 文本显示宽估算（正文字号：CJK=1em、ASCII≈0.56em）。
fn text_w(s: &str, em: i32) -> i32 {
    s.chars()
        .map(|c| {
            if c.is_ascii() {
                (em as f32 * 0.56) as i32
            } else {
                em
            }
        })
        .sum()
}

/// 【词宽实测 2026-11】GetTextExtentPoint32W 按实际字体测显示宽。
/// text_w 估算（CJK=1em/ASCII≈0.56em）对加粗+下划线的新词字体普遍
/// 偏窄（宽 ASCII W/M≈0.7-0.8em、无独立粗体字重时 GDI 合成粗体还有
/// 右侧 overhang），STATIC 按估算宽硬裁=「预览新词吞最后一个字」
///（用户实锤；纯显示层——预览与提交同读框、落库数据完整）。主候选
/// 窗是 DirectWrite 实测宽，本窗是全仓唯一用估算宽的候选式渲染，
/// 改 GDI 实测对齐。测量失败（DC/字体异常）兜底回估算。
unsafe fn measure_w(s: &str, font_usize: usize) -> Option<i32> {
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        GetDC, GetTextExtentPoint32W, HGDIOBJ, ReleaseDC, SelectObject,
    };
    if s.is_empty() {
        return Some(0);
    }
    let hdc = GetDC(None);
    if hdc.is_invalid() {
        return None;
    }
    let old = SelectObject(hdc, HGDIOBJ(font_usize as *mut core::ffi::c_void));
    let txt = utf16z(s);
    let mut sz = SIZE::default();
    let ok = GetTextExtentPoint32W(hdc, &txt[..txt.len() - 1], &mut sz);
    SelectObject(hdc, old);
    ReleaseDC(None, hdc);
    if ok.as_bool() {
        Some(sz.cx)
    } else {
        None
    }
}

/// 按宽度裁剪长候选（末尾加 …）；避免单个超长词撑出预览卡。
fn fit_text(
    mw: &dyn Fn(&str, Option<usize>, i32) -> i32,
    s: &str,
    font: Option<usize>,
    em: i32,
    max_w: i32,
) -> (String, i32) {
    let full = mw(s, font, em);
    if full <= max_w {
        return (s.to_string(), full);
    }
    let mut out = String::new();
    for ch in s.chars() {
        let test = format!("{out}{ch}…");
        if mw(&test, font, em) > max_w {
            break;
        }
        out.push(ch);
    }
    if out.is_empty() {
        out.push('…');
    }
    let w = mw(&out, font, em);
    (out, w)
}

/// 预览一行候选项：序号(label 色) + 词(text 色) 流式排列换行。
/// 返回排版后的下一行 y。new_idx=加入后行中新词下标（高亮块）。
/// 【占位符 2026-10-08】ph_start..ph_end 段为占位项（③④…），用
/// ID_AFT_PH 段（标签色）压暗显示。
unsafe fn draw_items(
    hwnd: HWND,
    y0: i32,
    head: &str,
    texts: &[String],
    new_idx: Option<usize>,
    id_base: i32,
    line_h: i32,
    ph_start: usize,
    ph_end: usize,
) -> i32 {
    // 预览固定字号（与 FONT_MAIN/FONT_LABEL 一致）：20/14px（cell 27/20）。
    let em = 20;
    let lem = 14;
    let spacing = SKIN
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|s| s.cand_spacing)
        .unwrap_or(6);
    // 【词宽实测 2026-11】三字体取锁一次；测量用与渲染同一字体
    //（新词=加粗+下划线），失败回估算。
    let (label_font, main_font, new_font) = {
        let l = FONT_LABEL.lock().unwrap_or_else(|p| p.into_inner());
        let m = FONT_MAIN.lock().unwrap_or_else(|p| p.into_inner());
        let n = FONT_NEW.lock().unwrap_or_else(|p| p.into_inner());
        (*l, *m, *n)
    };
    let mw = |s: &str, f: Option<usize>, em: i32| -> i32 {
        match f {
            Some(h) => unsafe { measure_w(s, h) }.unwrap_or_else(|| text_w(s, em)),
            None => text_w(s, em),
        }
    };
    // 行首标签（「现有：」/「加入后：」）
    let head_txt = utf16z(head);
    let head_w = mw(head, label_font, lem) + 4;
    // 【SS_NOPREFIX=0x80】词含 & 时 STATIC 默认把它当加速键前缀吞掉
    //（"R&B"→"RB"、尾随 & 直接消失）——预览 STATIC 一律加。
    let h = create_child(
        hwnd,
        w!("STATIC"),
        PCWSTR(head_txt.as_ptr()),
        WINDOW_EX_STYLE(0),
        0x80,
        PV_X,
        y0,
        head_w + 2,
        lem + 6,
        id_base - 10,
    );
    set_item_font(h, true);
    ITEMS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(h.0 as isize);

    // 分组标题单独占一行，候选从下一行起排（视觉分层更清楚）。
    // 14px cell 20 → 标题行占 24px。
    let mut x = PV_X;
    let mut y = y0 + lem + 10;
    let right = PV_X + PV_W;
    for (i, t) in texts.iter().enumerate() {
        let label = format!("{}.", i + 1);
        let is_new = new_idx == Some(i);
        let lw = mw(&label, label_font, lem) + 2;
        let tf = if is_new { new_font } else { main_font };
        // 单个候选超过整行可用宽 → 先裁成 …，防横向出血。
        // 先按字符数硬上限（防 GDI 测量/裁剪在极长串上的边界问题），
        // 再按像素宽精裁；两层都保留末尾 …。
        let t_short: String = if t.chars().count() > 16 {
            let mut q: String = t.chars().take(16).collect();
            q.push('…');
            q
        } else {
            t.clone()
        };
        let max_line = (right - PV_X).max(80);
        let (mut disp, mut tw) = fit_text(&mw, &t_short, tf, em, max_line - lw - spacing - 18);
        if tw < 0 { tw = 0; }
        let mut sw = tw + 12; // STATIC 宽度留 12px，防 … 被裁掉
        // 新词胶囊左右各伸 4px，预留额外 gap，避免被相邻 STATIC 底色盖住。
        let extra = if is_new { 12 } else { 0 };
        let mut need = lw + sw + spacing + extra;
        if x + need > right {
            x = PV_X; // 续行/换行从内容左缘起
            y += line_h;
        }
        // 换行后仍超宽（label+长词）→ 按当前行剩余宽再裁一次。
        let avail = right - x - lw - 2;
        if sw > avail {
            let fit = fit_text(&mw, &disp, tf, em, (avail - 12).max(20));
            disp = fit.0;
            tw = fit.1;
            sw = tw + 12;
            need = lw + sw + spacing + extra;
        }
        let base = if is_new {
            ID_AFT_NEW
        } else if i >= ph_start && i < ph_end {
            ID_AFT_PH
        } else {
            id_base
        };
        // 序号（新词行用高亮 id 段着色）
        let lbl_txt = utf16z(&label);
        // 20px cell 27 → 词盒高 em+8（28）；14px cell 20 → 序号盒高 lem+6（20）
        let label_h = if is_new { em + 8 } else { lem + 6 };
        let label_y = if is_new {
            y + (line_h - em - 8) / 2
        } else {
            y + (line_h - lem - 6) / 2
        };
        let hl = create_child(
            hwnd,
            w!("STATIC"),
            PCWSTR(lbl_txt.as_ptr()),
            WINDOW_EX_STYLE(0),
            0x80,
            x,
            label_y,
            lw,
            label_h,
            base, // 【三十四修】旧 `if is_new { base } else { base }` 恒等式简化
        );
        set_item_font(hl, true);
        ITEMS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(hl.0 as isize);
        // 词（新词用加粗字体；超长已按宽度裁成 …）
        let w_txt = utf16z(&disp);
        let wd = create_child(
            hwnd,
            w!("STATIC"),
            PCWSTR(w_txt.as_ptr()),
            WINDOW_EX_STYLE(0),
            0x80,
            x + lw,
            y + (line_h - em - 8) / 2,
            sw,
            em + 8,
            base + 1,
        );
        if is_new {
            set_new_font(wd);
        } else {
            set_item_font(wd, false);
        }
        ITEMS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(wd.0 as isize);
        if is_new {
            let chip_x = (x - 4).max(PV_X);
            let chip_y = y + (line_h - em - 8) / 2 - 3;
            let chip_w = (lw + sw + 8).min(right - chip_x);
            NEW_CHIP_RECTS
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push((chip_x, chip_y, chip_w, em + 14));
        }
        x += need;
        // 【三十四修·死代码删除】cid 计数器（`cid += 2; let _ = cid;`
        // 只写不读）已删。
    }
    y + line_h
}

/// 清空旧预览项。
unsafe fn clear_items() {
    NEW_CHIP_RECTS.lock().unwrap_or_else(|p| p.into_inner()).clear();
    let mut g = ITEMS.lock().unwrap_or_else(|p| p.into_inner());
    for h in g.iter() {
        let _ = DestroyWindow(HWND(*h as *mut _));
    }
    g.clear();
}

/// 刷新预览 + 自适应布局。
unsafe fn refresh_preview(hwnd: HWND) {
    let read_box = |id: i32| -> String {
        let Ok(h) = GetDlgItem(hwnd, id) else {
            return String::new();
        };
        let len = GetWindowTextLengthW(h);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(h, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    };
    let word = read_box(ID_WORD).trim().to_string();
    let code = read_box(ID_CODE).trim().to_string();
    let pos: usize = read_box(ID_POS).trim().parse().unwrap_or(0);
    let ph_on = PH_ON.load(std::sync::atomic::Ordering::Relaxed);
    // 【2026-11-04】预览行高 30→34：20px cell 27 + 7px 行距（旧 18px cell 24 + 6）。
    let line_h = 34;

    clear_items();
    let mut y = PV_START_Y;
    let mut old_len_for_hint = 0usize;

    let mk_msg = |text: &str, yy: i32| {
        let t = utf16z(text);
        let h = create_child(
            hwnd,
            w!("STATIC"),
            PCWSTR(t.as_ptr()),
            WINDOW_EX_STYLE(0),
            0x80 | 0x4000, /* SS_ENDELLIPSIS */
            PV_X,
            yy,
            PV_W,
            line_h,
            ID_PV_MSG,
        );
        if !h.0.is_null() {
            set_ui_font(h, false);
            ITEMS
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(h.0 as isize);
        }
    };

    if is_weight_mode() {
        // 加权模式：效果示意全部由 paint_scene 自绘，不建预览子项。
    } else if code.is_empty() {
        mk_msg("输入编码后显示该码候选", y);
        y += line_h;
    } else {
        match code_preview(&code) {
            Some(texts) => {
                let cur: Vec<String> = if texts.is_empty() {
                    vec!["（无候选——新词将成为首选）".to_string()]
                } else {
                    texts.clone()
                };
                y = draw_items(hwnd, y, "现有", &cur, None, ID_CUR_BASE, line_h, 0, 0);
                if !word.is_empty() {
                    let mut sim: Vec<String> =
                        texts.iter().filter(|t| **t != word).cloned().collect();
                    let old_len = sim.len();
                    old_len_for_hint = old_len;
                    let mut ph_end = old_len;
                    if ph_on && pos >= 1 && pos - 1 > old_len && pos <= 50 {
                        for p in old_len..pos - 1 {
                            if let Some(c) = circled_num(p + 1) {
                                sim.push(c);
                            }
                        }
                        ph_end = sim.len();
                    }
                    let idx = if pos >= 1 {
                        (pos as usize - 1).min(sim.len())
                    } else {
                        0
                    };
                    let is_ph = sim
                        .get(idx)
                        .map(|t| {
                            t.chars().count() == 1
                                && t.chars()
                                    .next()
                                    .map(|c| {
                                        let u = c as u32;
                                        (0x2460..=0x2473).contains(&u)
                                            || (0x3251..=0x325F).contains(&u)
                                    })
                                    .unwrap_or(false)
                        })
                        .unwrap_or(false);
                    if is_ph {
                        sim[idx] = word.clone();
                    } else {
                        sim.insert(idx, word.clone());
                    }
                    crate::tsf::trace(&format!(
                        "addword 预演: ph={ph_on} pos={pos} old_len={old_len} idx={idx} → {}",
                        sim.join("·")
                    ));
                    y = draw_items(
                        hwnd,
                        y + line_h / 2,
                        "加入后",
                        &sim,
                        Some(idx),
                        ID_AFT_BASE,
                        line_h,
                        old_len,
                        ph_end,
                    );
                }
            }
            None => {
                mk_msg("（查询失败）", y);
                y += line_h;
            }
        }
    }

    if ph_on {
        let hint = if pos == 0 {
            "占位符已开：填选重位后自动补 ③ ④ …"
        } else if pos >= 1 && pos - 1 <= old_len_for_hint {
            "占位符已开：现有候选已够该位"
        } else {
            "占位符已开：已按 ③ ④ … 补齐前序空位"
        };
        mk_msg(hint, y + 2);
        y += line_h + 4;
    }

    // 窗口高度：固定布局为下限；预览内容多时向下长（底栏随之下移）。
    let min_h = if is_weight_mode() { W_CLIENT_H } else { CLIENT_H };
    let client_h = min_h.max(y + 124);
    let btn_y = client_h - 24 - BTN_H;
    for (id, dx) in [(ID_OK, BTN_OK_X), (ID_CANCEL, BTN_CANCEL_X)] {
        if let Ok(ch) = GetDlgItem(hwnd, id) {
            let _ = SetWindowPos(
                ch,
                None,
                dx,
                btn_y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
    let _ = SetWindowPos(
        hwnd,
        None,
        0,
        0,
        WIN_W,
        outer_h(client_h),
        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
    // 确定可用性：词必填；加词模式编码必填。
    let ok = !word.is_empty() && (is_weight_mode() || !code.is_empty());
    if let Ok(h) = GetDlgItem(hwnd, ID_OK) {
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(h, ok);
    }
    let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, true);
}

/// 【2026-11-02 P2】server 端口探测/缓存：
///  - 只缓存**命中 /api/about 虎符特征**的端口（纯 TCP 可达会把别家服务当虎符）；
///  - 缓存命中后 HTTP 失败（server 换端口/重启/被别家占）→ reset_http_port()
///    清缓存，下次调用重新探测；
///  - 无服务时 1s 负缓存，避免词框每键都扫 7 个端口。
const ADDWORD_HTTP_CANDS: [u16; 7] = [4390, 17890, 17891, 17892, 17893, 17894, 27890];
static ADDWORD_HTTP_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);
static ADDWORD_HTTP_LAST_PROBE_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn probe_now_ms() -> u64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    EPOCH
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
}

/// 端口的 /api/about 是否确实是虎符 server（github + exe_dir 形状）。
/// 只做 TCP connect 会把别家本地服务当虎符，把加词请求发错地方。
fn probe_hufu_about(port: u16) -> bool {
    use std::io::{Read, Write};
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = std::net::TcpStream::connect_timeout(
        &addr,
        std::time::Duration::from_millis(150),
    ) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(std::time::Duration::from_millis(300)));
    let _ = s.set_write_timeout(Some(std::time::Duration::from_millis(300)));
    let req = "GET /api/about HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n";
    if s.write_all(req.as_bytes()).is_err() {
        return false;
    }
    let mut resp = String::new();
    let mut s = s.take(16 * 1024);
    let _ = s.read_to_string(&mut resp);
    resp.contains("\"github\"") && resp.contains("hufu-ime-rust") && resp.contains("\"exe_dir\"")
}

fn http_port() -> u16 {
    use std::sync::atomic::Ordering;
    let cached = ADDWORD_HTTP_PORT.load(Ordering::Relaxed);
    if cached != 0 {
        return cached;
    }
    let now = probe_now_ms();
    let last = ADDWORD_HTTP_LAST_PROBE_MS.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < 1000 {
        return 4390;
    }
    ADDWORD_HTTP_LAST_PROBE_MS.store(now, Ordering::Relaxed);
    for p in ADDWORD_HTTP_CANDS {
        if probe_hufu_about(p) {
            ADDWORD_HTTP_PORT.store(p, Ordering::Relaxed);
            return p;
        }
    }
    4390
}

/// 清缓存端口。之前有缓存（server 换端口）→ 同时清负缓存，立刻重探；
/// 无缓存时保留 1s 负缓存，避免没有 server 时每键扫端口。
fn reset_http_port() {
    use std::sync::atomic::Ordering;
    let had = ADDWORD_HTTP_PORT.swap(0, Ordering::Relaxed);
    if had != 0 {
        ADDWORD_HTTP_LAST_PROBE_MS.store(0, Ordering::Relaxed);
    }
}

/// GET /api/code_preview?code=xxx → 候选文本列表。
fn code_preview(code: &str) -> Option<Vec<String>> {
    use std::io::{Read, Write};
    let enc: String = code
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    let req = format!(
        "GET /api/code_preview?code={enc} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    );
    let port = http_port();
    let mut s = match std::net::TcpStream::connect(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(_) => {
            reset_http_port();
            return None;
        }
    };
    // 【读超时 2026-09-11】与 http_post_json 同款：实时预览路径在
    // EN_UPDATE 每键触发，server 卡住会冻结加词窗 UI。
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(std::time::Duration::from_secs(2)));
    if s.write_all(req.as_bytes()).is_err() {
        reset_http_port();
        return None;
    }
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    let Some((_, body)) = resp.split_once("\r\n\r\n") else {
        reset_http_port();
        return None;
    };
    let v: serde_json::Value = match serde_json::from_str(body.trim_start()) {
        Ok(v) => v,
        Err(_) => {
            reset_http_port();
            return None;
        }
    };
    v.get("texts").and_then(|t| t.as_array()).map(|a| {
        a.iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect()
    })
}

/// GET /api/word_code?text=词 → 编码提示（词典反查最优码，新词按通用
/// 组词规则生成）。【加词自动填码 2026-10-08】词框变动且编码框空时预填。
fn word_code(text: &str) -> Option<String> {
    use std::io::{Read, Write};
    let enc: String = text
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    let req = format!(
        "GET /api/word_code?text={enc} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    );
    let port = http_port();
    let mut s = match std::net::TcpStream::connect(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(_) => {
            reset_http_port();
            return None;
        }
    };
    // 读超时同 code_preview：预填路径在词框 EN_UPDATE 每键触发。
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(std::time::Duration::from_secs(2)));
    if s.write_all(req.as_bytes()).is_err() {
        reset_http_port();
        return None;
    }
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    let Some((_, body)) = resp.split_once("\r\n\r\n") else {
        reset_http_port();
        return None;
    };
    let v: serde_json::Value = match serde_json::from_str(body.trim_start()) {
        Ok(v) => v,
        Err(_) => {
            reset_http_port();
            return None;
        }
    };
    v.get("code")
        .and_then(|c| c.as_str())
        .filter(|c| !c.is_empty())
        .map(|c| c.to_string())
}

/// 读输入框 → POST server 加词。
unsafe fn submit(hwnd: HWND) {
    let read_edit = |id: i32| -> Option<String> {
        let h = GetDlgItem(hwnd, id).ok()?;
        let len = GetWindowTextLengthW(h);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(h, &mut buf);
        if n <= 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..n as usize]))
    };
    let (word, code, pos) = (read_edit(ID_WORD), read_edit(ID_CODE), read_edit(ID_POS));
    let pos_num = pos
        .as_deref()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(0);
    // 【加权模式 2026-09-06】词+权重（缺省 1000）→ server 反查最优码
    if is_weight_mode() {
        let Some(word) = word else { return };
        let word = word.trim().to_string();
        if word.is_empty() {
            return;
        }
        let wv: i64 = code
            .as_deref()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(1000);
        if post_weight(&word, wv) {
            crate::tsf::trace(&format!("addweight ok: {word} *{wv}"));
        } else {
            crate::tsf::trace(&format!("addweight POST 失败: {word} *{wv}"));
        }
        return;
    }
    let (Some(word), Some(code)) = (word, code) else {
        return;
    };
    let word = word.trim().to_string();
    let code = code.trim().to_string();
    if word.is_empty() || code.is_empty() {
        return;
    }
    // 【占位符补位提交 2026-10-08】勾选且 pos-1 超出现有候选数时，
    // 先按序提交带圈数字占位词条（③@p3、④@p4…——server {添加}pN
    // 时序回放逐条落位），词最后落在恰好的第 N 位。与预览同规则。
    let ph_on = PH_ON.load(std::sync::atomic::Ordering::Relaxed);
    if ph_on && pos_num >= 2 {
        let existing = code_preview(&code)
            .map(|texts| texts.iter().filter(|t| **t != word).count())
            .unwrap_or(0);
        if pos_num as usize - 1 > existing && pos_num <= 50 {
            for p in existing..pos_num as usize - 1 {
                if let Some(c) = circled_num(p + 1) {
                    if !post_add(&code, &c, p as i64 + 1) {
                        crate::tsf::trace(&format!(
                            "addword 占位符 POST 失败: {code} -> {c} @{}",
                            p + 1
                        ));
                    }
                }
            }
        }
    }
    if post_add(&code, &word, pos_num) {
        crate::tsf::trace(&format!("addword ok: {code} -> {word} @{pos_num}"));
    } else {
        crate::tsf::trace(&format!("addword POST 失败: {code} -> {word} @{pos_num}"));
    }
}

/// 裸 HTTP POST 127.0.0.1:<server 端口> /api/user_word/weight。
/// 【JSON 安全 2026-09-11】旧手拼 esc 只转义 \ 和 "——词里带换行/
/// 制表符等控制字符会拼出非法 JSON（server 端解析 400，加词静默
/// 失败）。改 serde_json 正规序列化。
fn post_weight(word: &str, weight: i64) -> bool {
    let body = serde_json::json!({"text": word, "weight": weight}).to_string();
    http_post_json("/api/user_word/weight", &body)
}

/// 裸 HTTP POST 127.0.0.1:<server 端口> /api/user_word/add。（同上 JSON 安全）
fn post_add(code: &str, word: &str, pos: i64) -> bool {
    let body = serde_json::json!({"code": code, "text": word, "pos": pos}).to_string();
    http_post_json("/api/user_word/add", &body)
}

/// 公共 POST（path + JSON body → 200 即成功）。
fn http_post_json(path: &str, body: &str) -> bool {
    use std::io::{Read, Write};
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let mut s = match std::net::TcpStream::connect(("127.0.0.1", http_port())) {
        Ok(s) => s,
        Err(e) => {
            crate::tsf::trace(&format!("addword tcp connect err: {e}"));
            reset_http_port();
            return false;
        }
    };
    // 【读超时 2026-09-11】旧实现只有写超时，读无超时——server 卡住
    // 时加词窗 UI 线程（submit 是按钮点击路径）无限冻结。
    let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(std::time::Duration::from_secs(2)));
    if let Err(e) = s.write_all(req.as_bytes()) {
        crate::tsf::trace(&format!("addword tcp write err: {e}"));
        reset_http_port();
        return false;
    }
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    let ok = resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200");
    if !ok {
        reset_http_port();
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bmp(w: u32, h: u32, frames: Vec<Vec<u8>>) -> crate::ssfwin::SsfBitmap {
        crate::ssfwin::SsfBitmap {
            width: w,
            height: h,
            plays: 0,
            delays_ms: vec![0; frames.len()],
            frames,
        }
    }

    /// 全不透明均色 → 平均色即原色（COLORREF 布局：b<<16|g<<8|r）。
    #[test]
    fn avg_opaque_uniform() {
        let b = bmp(4, 4, vec![[10u8, 20, 30, 255].repeat(16)]); // B,G,R,A
        assert_eq!(avg_opaque_rgb(&b), Some(10 << 16 | 20 << 8 | 30));
    }

    /// 半透明像素不计入；覆盖 <2%（50 分之一）→ None 回退默认。
    #[test]
    fn avg_opaque_coverage_gate() {
        // 100 像素，仅 1 个不透明（1%）→ None
        let mut px = [0u8, 0, 0, 0].repeat(100);
        px[0..4].copy_from_slice(&[255, 255, 255, 255]);
        let b = bmp(100, 1, vec![px]);
        assert_eq!(avg_opaque_rgb(&b), None);
        // alpha 127/128 门槛：127 不算，128 算
        let mut px2 = [0u8, 0, 0, 127].repeat(8);
        px2[0..4].copy_from_slice(&[200, 100, 50, 128]); // B=200,G=100,R=50
        let b2 = bmp(8, 1, vec![px2]);
        assert_eq!(avg_opaque_rgb(&b2), Some(200 << 16 | 100 << 8 | 50));
    }

    /// 亮度：白=1、黑=0、中灰≈0.5 量级正确。
    #[test]
    fn luminance_bounds() {
        assert!((lum(0x00_FF_FF_FF) - 1.0).abs() < 1e-6);
        assert!(lum(0) < 1e-6);
        assert!((lum(0x00_80_80_80) - 0.5).abs() < 0.01);
    }

    /// ssf 载荷配色：display 四色解析 + 面板图缺失（无 cache_dir 节）
    /// 时 back=None 走默认，text/label 仍解出。
    #[test]
    fn ssf_panel_colors_payload_paths() {
        let sk = serde_json::json!({
            "kind": "ssf",
            "display": {"text_color": "#F0E6D2FF", "annotation_color": "#A0A0A0FF"},
            "layouts": {"vertical_candidates": {"image": "panel.png"}}
            // 无 cache_dir/fp → 面板路径跳过
        });
        let (back, text, label) = ssf_panel_colors(&sk);
        assert!(back.is_none());
        assert_eq!(text, Some(0xD2 << 16 | 0xE6 << 8 | 0xF0)); // #F0E6D2
        assert_eq!(label, Some(0xA0 << 16 | 0xA0 << 8 | 0xA0));
    }
}
