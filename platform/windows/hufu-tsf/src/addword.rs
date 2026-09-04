//! {加词} 弹窗：`/jc {加词}` 选中后弹小窗。三个输入框（词 / 编码 /
//! 选重位）+ 候选顺序实时预览。预览做成候选窗样式：每项「序号+词」
//! 分色（label/text 色）流式排列，新词高亮底块；配色与字体套用当前
//! 皮肤（colors + layout.font_*）。每次键入（EN_UPDATE=0x400）立即刷
//! 新；预览变多窗口自适应加高，确定/取消恒在底部。窗口独立线程跑消
//! 息循环不阻塞 TSF。

use windows::core::*;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, CLEARTYPE_QUALITY, DEFAULT_CHARSET, FW_BOLD, FW_NORMAL,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;

const ID_WORD: i32 = 101;
const ID_CODE: i32 = 102;
const ID_POS: i32 = 103;
const ID_OK: i32 = 1001;
const ID_CANCEL: i32 = 1002;

// 预览动态项 id 段（CTLCOLOR 按段分色；GWLP_USERDATA 存文字 COLORREF）
const ID_CUR_BASE: i32 = 2000; // 现有项
const ID_AFT_BASE: i32 = 2200; // 加入后普通项
const ID_AFT_NEW: i32 = 2600; // 加入后新词高亮项
const ID_EN_UPDATE: u32 = 0x400; // EN_UPDATE（0x200 是 EN_KILLFOCUS！）

const CLASS: PCWSTR = w!("HuFuAddWord");
const WIN_W: i32 = 396;
const PV_X: i32 = 16;
const PV_W: i32 = 364; // 预览排版宽（候选流）
const EDIT_W: i32 = 284; // 输入框宽（收窄，不再通栏）

/// 皮肤数据（首次弹窗拉取；失败回退深色系）
struct Skin {
    bg: u32,
    text: u32,
    label: u32,
    hilite: u32,
    hilite_label: u32,
    hilite_bg: u32,
    font_face: Vec<u16>, // UTF-16 含 null
    font_pt: i32,
    label_pt: i32,
    cand_spacing: i32,
}
static SKIN: std::sync::Mutex<Option<Skin>> = std::sync::Mutex::new(None);
static BG_BRUSH: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
static HILITE_BRUSH: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
static FONT_MAIN: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
static FONT_LABEL: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
/// 新词专用：加粗+下划线（红色由 CTLCOLOR 分段着色）
static FONT_NEW: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
/// 新词红（COLORREF 0x00BBGGRR：R255 G80 B80）
const NEW_RED: u32 = 0x50_50_FF;
/// 预览动态控件（刷新时销毁重建）
static ITEMS: std::sync::Mutex<Vec<isize>> = std::sync::Mutex::new(Vec::new());

fn parse_hex(s: &str) -> Option<u32> {
    let t = s.trim().trim_start_matches('#');
    if t.len() < 6 {
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

/// 拉皮肤配色+字体（弹窗线程调用一次；失败回退深色+微软雅黑）。
fn load_skin() {
    let mut g = SKIN.lock().unwrap_or_else(|p| p.into_inner());
    if g.is_some() {
        return;
    }
    let mut sk = crate::ipc::call(&serde_json::json!({"op": "skin"}))
        .and_then(|v| v.get("skin").cloned())
        .unwrap_or(serde_json::Value::Null);
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
    let font_pt = sk
        .pointer("/layout/font_point")
        .and_then(|x| x.as_f64())
        .unwrap_or(16.0) as i32;
    let label_pt = sk
        .pointer("/layout/label_font_point")
        .and_then(|x| x.as_f64())
        .unwrap_or(12.0) as i32;
    let cand_spacing = sk
        .pointer("/layout/candidate_spacing")
        .and_then(|x| x.as_f64())
        .unwrap_or(6.0) as i32;
    let s = Skin {
        // 窗口底色提亮 20%：候选窗小面积用原底色可以，整窗大面积
        // 直接用会死黑（用户反馈），向白混一档。
        bg: lighten(
            get_color("back_color").unwrap_or(0x22_2E_16),
            0.20,
        ),
        text: get_color("text_color")
            .or_else(|| get_color("candidate_text_color"))
            .unwrap_or(0xEC_E2_D7),
        label: get_color("label_color")
            .or_else(|| get_color("comment_text_color"))
            .unwrap_or(0xB5_A6_8A),
        hilite: get_color("hilited_candidate_text_color").unwrap_or(0xF0_C7_8C),
        hilite_label: get_color("hilited_label_color").unwrap_or(0xF0_C7_8C),
        hilite_bg: get_color("hilited_candidate_back_color").unwrap_or(0x66_4A_2C),
        font_face: utf16z(&face),
        // 字号比皮肤候选窗大两档（弹窗阅读距离远；用户两轮要求加大）
        font_pt: (font_pt + 6).max(17),
        label_pt: (label_pt + 5).max(13),
        cand_spacing: cand_spacing.max(2),
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
        *FONT_MAIN.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(mk_font(s.font_pt, FW_NORMAL.0 as i32).0 as usize);
        *FONT_LABEL.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(mk_font(s.label_pt, FW_NORMAL.0 as i32).0 as usize);
        // 新词：加粗 + 下划线
        let bold_underline = CreateFontW(
            -s.font_pt,
            0,
            0,
            0,
            FW_BOLD.0 as i32,
            0,
            1, // underline
            0,
            DEFAULT_CHARSET.0 as u32,
            0,
            0,
            CLEARTYPE_QUALITY.0 as u32,
            0,
            PCWSTR(s.font_face.as_ptr()),
        );
        *FONT_NEW.lock().unwrap_or_else(|p| p.into_inner()) = Some(bold_underline.0 as usize);
        *BG_BRUSH.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(CreateSolidBrush(COLORREF(s.bg)).0 as usize);
        *HILITE_BRUSH.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(CreateSolidBrush(COLORREF(s.hilite_bg)).0 as usize);
    }
    *g = Some(s);
}

/// 弹出加词窗（非阻塞：新线程 + 消息循环）。
pub fn open() {
    std::thread::spawn(|| unsafe {
        crate::tsf::trace("addword open（线程已起）");
        load_skin();
        let hmod = GetModuleHandleW(None).unwrap_or_default();
        let hinst = HINSTANCE(hmod.0);
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
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
        let h = outer_h(300);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_DLGMODALFRAME,
            CLASS,
            w!("虎符 · 加词"),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
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
            return;
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if !IsDialogMessageW(hwnd, &mut msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
}

/// client 高 → 外框高。
unsafe fn outer_h(client_h: i32) -> i32 {
    let mut rc = RECT {
        left: 0,
        top: 0,
        right: WIN_W,
        bottom: client_h,
    };
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    let _ = AdjustWindowRect(&mut rc, style, false);
    rc.bottom - rc.top
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

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            let mk = |s: &str| -> Vec<u16> {
                let mut v: Vec<u16> = s.encode_utf16().collect();
                v.push(0);
                v
            };
            let mut first_edit = HWND::default();
            let rows: [(&str, i32, u32); 3] = [
                ("词（要打出的内容）", ID_WORD, WS_TABSTOP.0 | 0x80u32),
                ("编码（打什么出它）", ID_CODE, WS_TABSTOP.0 | 0x80u32),
                ("选重位（第几选，留空=首选）", ID_POS, WS_TABSTOP.0 | 0x80u32 | 0x2000u32),
            ];
            for (i, (label, id, extra)) in rows.iter().enumerate() {
                let y = 14 + i as i32 * 62;
                let lbl_txt = mk(label);
                let lbl = create_child(
                    hwnd,
                    w!("STATIC"),
                    PCWSTR(lbl_txt.as_ptr()),
                    WINDOW_EX_STYLE(0),
                    0,
                    PV_X,
                    y,
                    330,
                    24,
                    0,
                );
                set_item_font(lbl, true);
                let ed = create_child(
                    hwnd,
                    w!("EDIT"),
                    w!(""),
                    WS_EX_CLIENTEDGE,
                    *extra,
                    PV_X,
                    y + 26,
                    EDIT_W,
                    33,
                    *id,
                );
                set_item_font(ed, false);
                if i == 0 {
                    first_edit = ed;
                }
            }
            let t1 = mk("该编码候选（实时，第 N 选参考）：");
            let pt = create_child(
                hwnd,
                w!("STATIC"),
                PCWSTR(t1.as_ptr()),
                WINDOW_EX_STYLE(0),
                0,
                PV_X,
                202,
                340,
                22,
                0,
            );
            set_item_font(pt, true);
            for (label, id) in [("确定", ID_OK), ("取消", ID_CANCEL)] {
                let btxt = mk(label);
                let btn = create_child(
                    hwnd,
                    w!("BUTTON"),
                    PCWSTR(btxt.as_ptr()),
                    WINDOW_EX_STYLE(0),
                    WS_TABSTOP.0,
                    if id == ID_OK { 216 } else { 311 },
                    260,
                    82,
                    34,
                    id,
                );
                set_item_font(btn, false);
            }
            if !first_edit.0.is_null() {
                let _ = SetFocus(first_edit);
            }
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
        WM_CTLCOLORSTATIC => {
            let ctrl = HWND(lp.0 as _);
            let cid = GetWindowLongPtrW(ctrl, GWLP_ID) as i32;
            let sc = SKIN.lock().unwrap_or_else(|p| p.into_inner());
            let Some(c) = sc.as_ref() else {
                return DefWindowProcW(hwnd, msg, wp, lp);
            };
            let hdc = windows::Win32::Graphics::Gdi::HDC(wp.0 as _);
            // 新词项：红色（字体已加粗带下划线）；其余按段取皮肤色
            let fg: u32 = if cid >= ID_AFT_NEW {
                NEW_RED
            } else if cid >= ID_AFT_BASE || cid >= ID_CUR_BASE {
                c.text
            } else {
                c.label
            };
            let _ = windows::Win32::Graphics::Gdi::SetTextColor(hdc, COLORREF(fg));
            let _ = windows::Win32::Graphics::Gdi::SetBkColor(hdc, COLORREF(c.bg));
            if let Some(b) = *BG_BRUSH.lock().unwrap_or_else(|p| p.into_inner()) {
                return LRESULT(b as isize);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_CTLCOLOREDIT => {
            let sc = SKIN.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(c) = sc.as_ref() {
                let hdc = windows::Win32::Graphics::Gdi::HDC(wp.0 as _);
                let _ = windows::Win32::Graphics::Gdi::SetTextColor(hdc, COLORREF(c.text));
                let _ = windows::Win32::Graphics::Gdi::SetBkColor(hdc, COLORREF(c.bg));
                if let Some(b) = *BG_BRUSH.lock().unwrap_or_else(|p| p.into_inner()) {
                    return LRESULT(b as isize);
                }
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_COMMAND => {
            let id = (wp.0 as u32 & 0xFFFF) as i32;
            let notif = (wp.0 as u32 >> 16) as u32;
            // EN_UPDATE=0x400：文本每变一次立即刷（0x200 是 EN_KILLFOCUS，
            // 上版误用导致「光标移走才刷新」）
            if (id == ID_CODE || id == ID_POS || id == ID_WORD) && notif == ID_EN_UPDATE {
                unsafe { refresh_preview(hwnd) };
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

/// 新词项字体（加粗+下划线）。
unsafe fn set_new_font(h: HWND) {
    if let Some(f) = *FONT_NEW.lock().unwrap_or_else(|p| p.into_inner()) {
        let _ = SendMessageW(h, WM_SETFONT, WPARAM(f), LPARAM(1));
    }
}

/// 文本显示宽估算（正文字号：CJK=1em、ASCII≈0.56em）。
fn text_w(s: &str, em: i32) -> i32 {
    s.chars()
        .map(|c| if c.is_ascii() { (em as f32 * 0.56) as i32 } else { em })
        .sum()
}

/// 预览一行候选项：序号(label 色) + 词(text 色) 流式排列换行。
/// 返回排版后的下一行 y。new_idx=加入后行中新词下标（高亮块）。
unsafe fn draw_items(
    hwnd: HWND,
    y0: i32,
    head: &str,
    texts: &[String],
    new_idx: Option<usize>,
    id_base: i32,
    line_h: i32,
) -> i32 {
    let em = SKIN
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|s| s.font_pt)
        .unwrap_or(16);
    let lem = SKIN
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|s| s.label_pt)
        .unwrap_or(12);
    let spacing = SKIN
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|s| s.cand_spacing)
        .unwrap_or(6);
    // 行首标签（「现有：」/「加入后：」）
    let head_txt = utf16z(head);
    let head_w = text_w(head, lem) + 4;
    let h = create_child(
        hwnd,
        w!("STATIC"),
        PCWSTR(head_txt.as_ptr()),
        WINDOW_EX_STYLE(0),
        0,
        PV_X,
        y0,
        head_w + 2,
        line_h,
        0,
    );
    set_item_font(h, true);
    ITEMS.lock().unwrap_or_else(|p| p.into_inner()).push(h.0 as isize);

    let mut x = PV_X + head_w;
    let mut y = y0;
    let right = PV_X + PV_W;
    let mut cid = id_base;
    for (i, t) in texts.iter().enumerate() {
        let label = format!("{}.", i + 1);
        let lw = text_w(&label, lem) + 2;
        let tw = text_w(t, em) + 2;
        let need = lw + tw + spacing;
        if x + need > right {
            x = PV_X + 14; // 续行缩进
            y += line_h;
        }
        let is_new = new_idx == Some(i);
        let base = if is_new { ID_AFT_NEW } else { id_base };
        // 序号（新词行用高亮 id 段着色）
        let lbl_txt = utf16z(&label);
        let hl = create_child(
            hwnd,
            w!("STATIC"),
            PCWSTR(lbl_txt.as_ptr()),
            WINDOW_EX_STYLE(0),
            0,
            x,
            y + (line_h - lem - 4),
            lw,
            lem + 4,
            if is_new { base } else { base },
        );
        set_item_font(hl, true);
        ITEMS.lock().unwrap_or_else(|p| p.into_inner()).push(hl.0 as isize);
        // 词（新词用加粗+下划线字体）
        let w_txt = utf16z(t);
        let wd = create_child(
            hwnd,
            w!("STATIC"),
            PCWSTR(w_txt.as_ptr()),
            WINDOW_EX_STYLE(0),
            0,
            x + lw,
            y + (line_h - em - 6) / 2,
            tw,
            em + 6,
            base + 1,
        );
        if is_new {
            set_new_font(wd);
        } else {
            set_item_font(wd, false);
        }
        ITEMS.lock().unwrap_or_else(|p| p.into_inner()).push(wd.0 as isize);
        x += need;
        cid += 2;
        let _ = cid;
    }
    y + line_h
}

/// 清空旧预览项。
unsafe fn clear_items() {
    let mut g = ITEMS.lock().unwrap_or_else(|p| p.into_inner());
    for h in g.iter() {
        let _ = DestroyWindow(HWND(*h as *mut _));
    }
    g.clear();
}

/// 刷新预览 + 自适应布局。
unsafe fn refresh_preview(hwnd: HWND) {
    let read_box = |id: i32| -> String {
        let Ok(h) = GetDlgItem(hwnd, id) else { return String::new() };
        let len = GetWindowTextLengthW(h);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(h, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    };
    let code = read_box(ID_CODE).trim().to_string();
    let word = read_box(ID_WORD).trim().to_string();
    let pos: usize = read_box(ID_POS).trim().parse().unwrap_or(0);

    let line_h = SKIN
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|s| s.font_pt + 14)
        .unwrap_or(34);

    clear_items();
    let mut y = 228;
    if code.is_empty() {
        // 占位提示
        let t = utf16z("（输入编码后显示该码候选）");
        let h = create_child(
            hwnd,
            w!("STATIC"),
            PCWSTR(t.as_ptr()),
            WINDOW_EX_STYLE(0),
            0,
            PV_X,
            y,
            PV_W,
            line_h,
            0,
        );
        set_item_font(h, true);
        ITEMS.lock().unwrap_or_else(|p| p.into_inner()).push(h.0 as isize);
        y += line_h;
    } else {
        match code_preview(&code) {
            Some(texts) => {
                let cur: Vec<String> = if texts.is_empty() {
                    vec!["（无候选——新词将成为首选）".to_string()]
                } else {
                    texts.clone()
                };
                y = draw_items(hwnd, y, "现有：", &cur, None, ID_CUR_BASE, line_h);
                if !word.is_empty() {
                    // 与 server add / Schema 插入同规则模拟
                    let mut sim: Vec<String> =
                        texts.iter().filter(|t| **t != word).cloned().collect();
                    let idx = if pos >= 1 { (pos - 1).min(sim.len()) } else { 0 };
                    sim.insert(idx, word.clone());
                    // 「现有」与「加入后」隔开一行距，视觉分组
                    y = draw_items(
                        hwnd,
                        y + line_h / 2 + 4,
                        "加入后：",
                        &sim,
                        Some(idx),
                        ID_AFT_BASE,
                        line_h,
                    );
                }
            }
            None => {
                let t = utf16z("（查询失败）");
                let h = create_child(
                    hwnd,
                    w!("STATIC"),
                    PCWSTR(t.as_ptr()),
                    WINDOW_EX_STYLE(0),
                    0,
                    PV_X,
                    y,
                    PV_W,
                    line_h,
                    0,
                );
                set_item_font(h, true);
                ITEMS.lock().unwrap_or_else(|p| p.into_inner()).push(h.0 as isize);
                y += line_h;
            }
        }
    }

    // 自适应：按钮钉底、窗口随高
    let btn_y = y + 10;
    let client_h = btn_y + 30 + 14;
    for (id, dx) in [(ID_OK, 216), (ID_CANCEL, 311)] {
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
    let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, true);
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
    let mut s = std::net::TcpStream::connect(("127.0.0.1", 4390)).ok()?;
    let _ = s.set_write_timeout(Some(std::time::Duration::from_secs(2)));
    s.write_all(req.as_bytes()).ok()?;
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    let body = resp.split_once("\r\n\r\n").map(|(_, b)| b)?;
    let v: serde_json::Value = serde_json::from_str(body.trim_start()).ok()?;
    v.get("texts")
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
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
    let (Some(word), Some(code)) = (word, code) else {
        return;
    };
    let word = word.trim().to_string();
    let code = code.trim().to_string();
    if word.is_empty() || code.is_empty() {
        return;
    }
    if post_add(&code, &word, pos_num) {
        crate::tsf::trace(&format!("addword ok: {code} -> {word} @{pos_num}"));
    } else {
        crate::tsf::trace(&format!("addword POST 失败: {code} -> {word} @{pos_num}"));
    }
}

/// 裸 HTTP POST 127.0.0.1:4390 /api/user_word/add。
fn post_add(code: &str, word: &str, pos: i64) -> bool {
    use std::io::{Read, Write};
    let esc = |s: &str| -> String { s.replace('\\', "\\\\").replace('"', "\\\"") };
    let body = format!(
        "{{\"code\":\"{}\",\"text\":\"{}\",\"pos\":{}}}",
        esc(code),
        esc(word),
        pos
    );
    let req = format!(
        "POST /api/user_word/add HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let mut s = match std::net::TcpStream::connect(("127.0.0.1", 4390)) {
        Ok(s) => s,
        Err(e) => {
            crate::tsf::trace(&format!("addword tcp connect err: {e}"));
            return false;
        }
    };
    let _ = s.set_write_timeout(Some(std::time::Duration::from_secs(2)));
    if let Err(e) = s.write_all(req.as_bytes()) {
        crate::tsf::trace(&format!("addword tcp write err: {e}"));
        return false;
    }
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    resp.starts_with("HTTP/1.1 200") || resp.starts_with("HTTP/1.0 200")
}
