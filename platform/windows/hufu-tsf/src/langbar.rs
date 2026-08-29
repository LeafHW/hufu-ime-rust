//! TSF 语言栏按钮：任务栏输入指示区「中/A」状态牌（微软拼音风格）。
//!
//! - 图标/文字随中英模式切换（进程全局态，tsf.rs 每帧从引擎 state 同步）
//! - 左键 → 管道 op "toggle_lang" 切换中英（与 Shift 单击同语义）
//! - 右键 → 管道 op "settings" 打开设置页（与托盘双击同通道）
//! - 生命周期跟随输入法：Activate 挂载、Deactivate 移除
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use windows::core::{implement, Interface, Result, GUID};
use windows::Win32::Foundation::{BOOL, E_INVALIDARG, RECT};
use windows::Win32::UI::TextServices::{
    ITfLangBarItem, ITfLangBarItemButton, ITfLangBarItemButton_Impl, ITfLangBarItem_Impl,
    ITfLangBarItemMgr, ITfLangBarItemSink, ITfSource, ITfSource_Impl, TF_LANGBARITEMINFO,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

/// COM 指针跨线程搬运套（sink 由 msctf 线程 Advise、模式切换线程通知：
/// 实际调用仍是 COM 默认自由线程封送语义，这里只解除 Rust 的静态限制）
struct SendSink(ITfLangBarItemSink);
unsafe impl Send for SendSink {}

/// 语言栏项固定 GUID（身份稳定，msctf 按此识别）
pub const LANGBAR_ITEM_GUID: GUID = GUID::from_values(
    0x9C3B7D82,
    0x1A44,
    0x4E6F,
    [0xB5, 0xC8, 0x2D, 0x7E, 0x8F, 0x1A, 0x0B, 0x93],
);

/// 服务 CLSID（与 com.rs 一致；GetInfo 需要）
const CLSID_HUFU: GUID = GUID::from_values(
    0x8F5C2A10,
    0x3E77,
    0x4B9C,
    [0xA1, 0xD4, 0x9E, 0x0B, 0x7C, 0x2F, 0x5A, 0x88],
);

const TF_LBI_STYLE_SHOWNINTRAY: u32 = 0x2; // 在任务栏角落（输入指示区）显示
const TF_LBI_STYLE_BTN_BUTTON: u32 = 0x10000;

// ── 进程全局模式态（tsf.rs 每帧同步；点击切换也走这里）──
static CHINESE: AtomicBool = AtomicBool::new(true);
/// msctf 挂的更新 sink：进程全局（多线程各挂各的项，共用一份名单）
static SINKS: Mutex<Vec<(u32, SendSink)>> = Mutex::new(Vec::new());
static NEXT_COOKIE: AtomicU32 = AtomicU32::new(0x4846_0001);

/// 读取当前中英态（msctf 拉 GetText/GetIcon 时用）
fn is_chinese() -> bool {
    CHINESE.load(Ordering::Relaxed)
}

/// 更新模式并广播所有 sink（msctf 重拉图标/文字）。返回是否有变化。
pub fn set_mode(zh: bool) -> bool {
    if CHINESE.swap(zh, Ordering::Relaxed) == zh {
        return false;
    }
    notify_sinks();
    true
}

fn notify_sinks() {
    // TF_LBI_ICON|TF_LBI_TEXT|TF_LBI_TOOLTIP
    const UPD: u32 = 0x1 | 0x2 | 0x4;
    if let Ok(v) = SINKS.lock() {
        for (_, sink) in v.iter() {
            unsafe {
                let _ = sink.0.OnUpdate(UPD);
            }
        }
    }
}

#[implement(ITfLangBarItem, ITfLangBarItemButton, ITfSource)]
pub struct HuFuLangBar {
    icon_zh: isize, // HICON
    icon_en: isize,
}

impl HuFuLangBar {
    pub fn new() -> HuFuLangBar {
        HuFuLangBar {
            icon_zh: make_zh_icon(),
            icon_en: make_a_icon(),
        }
    }
}

/// ITfSource：msctf（语言栏宿主）会对项 AdviseSink(ITfLangBarItemSink)。
/// 不实现该接口时 AddItem 在真实宿主里可能 E_FAIL。名单进程全局共享
///（多线程的项一起收广播，cookie 全局唯一防误删）。
impl ITfSource_Impl for HuFuLangBar_Impl {
    fn AdviseSink(
        &self,
        riid: *const GUID,
        punk: Option<&windows::core::IUnknown>,
    ) -> Result<u32> {
        if unsafe { riid.as_ref() } != Some(&ITfLangBarItemSink::IID) {
            return Err(windows::core::Error::from(E_INVALIDARG));
        }
        let sink: ITfLangBarItemSink = punk
            .and_then(|u| u.cast().ok())
            .ok_or_else(|| windows::core::Error::from(E_INVALIDARG))?;
        let cookie = NEXT_COOKIE.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut v) = SINKS.lock() {
            v.push((cookie, SendSink(sink)));
        }
        Ok(cookie)
    }

    fn UnadviseSink(&self, dwcookie: u32) -> Result<()> {
        if let Ok(mut v) = SINKS.lock() {
            let before = v.len();
            v.retain(|(c, _)| *c != dwcookie);
            if v.len() != before {
                return Ok(());
            }
        }
        Err(windows::core::Error::from(E_INVALIDARG))
    }
}

/// 挂载到线程的语言栏（Activate 时调用）。项实例存 thread_local：
/// msctf 的 AddItem/RemoveItem 按对象引用操作（同一 GUID 认领）。
pub fn install(mgr: &ITfLangBarItemMgr) -> Result<()> {
    let item: ITfLangBarItem = unsafe { HuFuLangBar::new().into() };
    let r = unsafe { mgr.AddItem(&item) };
    // 诊断标记：Activate 后 %TEMP%\hufu-langbar.txt 可查挂载结果
    let msg = match &r {
        Ok(()) => format!(
            "ok pid={} zh={} t={:?}\n",
            std::process::id(),
            is_chinese(),
            std::time::SystemTime::now()
        ),
        Err(e) => format!("FAIL {:#010x} pid={}\n", e.code().0, std::process::id()),
    };
    let _ = std::fs::write(std::env::temp_dir().join("hufu-langbar.txt"), msg);
    r?;
    LANGBAR_ITEM.with(|c| *c.borrow_mut() = Some(item));
    Ok(())
}

/// 供 tsf.rs Deactivate 使用：从线程语言栏摘除本项
pub fn uninstall(mgr: &ITfLangBarItemMgr) {
    LANGBAR_ITEM.with(|c| {
        if let Some(item) = c.borrow_mut().take() {
            unsafe {
                let _ = mgr.RemoveItem(&item);
            }
        }
    });
}

thread_local! {
    static LANGBAR_ITEM: std::cell::RefCell<Option<ITfLangBarItem>> =
        const { std::cell::RefCell::new(None) };
}

impl ITfLangBarItem_Impl for HuFuLangBar_Impl {
    fn GetInfo(&self, pclbid: *mut TF_LANGBARITEMINFO) -> Result<()> {
        if pclbid.is_null() {
            return Ok(());
        }
        let mut text = [0u16; 32];
        let t: Vec<u16> = "虎符输入法".encode_utf16().collect();
        text[..t.len()].copy_from_slice(&t);
        unsafe {
            (*pclbid).clsidService = CLSID_HUFU;
            (*pclbid).guidItem = LANGBAR_ITEM_GUID;
            (*pclbid).dwStyle = TF_LBI_STYLE_BTN_BUTTON | TF_LBI_STYLE_SHOWNINTRAY;
            (*pclbid).ulSort = 1;
            (*pclbid).szDescription = text;
        }
        Ok(())
    }

    fn GetStatus(&self) -> Result<u32> {
        Ok(0) // 无特殊状态（无 HIDDEN/Disabled）
    }

    fn Show(&self, _fshow: BOOL) -> Result<()> {
        Ok(())
    }

    fn GetTooltipString(&self) -> Result<windows::core::BSTR> {
        Ok(windows::core::BSTR::from(if is_chinese() {
            "虎符输入法 · 中文模式（左键切英文，右键打开设置）"
        } else {
            "虎符输入法 · 英文模式（左键切中文，右键打开设置）"
        }))
    }
}

impl ITfLangBarItemButton_Impl for HuFuLangBar_Impl {
    fn OnClick(
        &self,
        click: windows::Win32::UI::TextServices::TfLBIClick,
        _pt: &windows::Win32::Foundation::POINT,
        _prcarea: *const RECT,
    ) -> Result<()> {
        // 右键 → 设置页；左键 → 切换中英（引擎返回最新态，回填图标）
        if click == windows::Win32::UI::TextServices::TfLBIClick(1) {
            let _ = crate::ipc::call(&serde_json::json!({"op": "settings"}));
        } else {
            if let Some(resp) = crate::ipc::call(&serde_json::json!({"op": "toggle_lang"})) {
                let zh = resp
                    .pointer("/state/chinese")
                    .and_then(|v| v.as_bool())
                    .unwrap_or_else(is_chinese);
                set_mode(zh);
            }
        }
        Ok(())
    }

    fn InitMenu(&self, _pmenu: Option<&windows::Win32::UI::TextServices::ITfMenu>) -> Result<()> {
        Ok(())
    }

    fn OnMenuSelect(&self, _id: u32) -> Result<()> {
        Ok(())
    }

    fn GetIcon(&self) -> Result<HICON> {
        Ok(HICON((if is_chinese() { self.icon_zh } else { self.icon_en }) as *mut _))
    }

    fn GetText(&self) -> Result<windows::core::BSTR> {
        Ok(windows::core::BSTR::from(if is_chinese() { "中" } else { "A" }))
    }
}

// ───────────────────────── 「中」「A」图标软件光栅化 ─────────────────────────
// 32×32 BGRA 超采样 4×；圆角方块底 + 字形几何。「中」（口环+竖贯通）
// 与 server 托盘爪印图标同一套画法；「A」两斜腿 + 横杠。

/// 通用：圆角方块底 + 前景覆盖判定函数 → HICON
fn make_plate_icon(fg_hit: impl Fn(f32, f32) -> bool) -> isize {
    const S: usize = 32;
    const SS: usize = 4;
    let r = 7.5f32;
    let half = 14.0f32;
    let bg = [0.105, 0.105, 0.118f32]; // #1B1B1E
    let fg = [0.96, 0.96, 0.97f32]; // #F5F5F7
    let mut buf = vec![0u8; S * S * 4];
    for y in 0..S {
        for x in 0..S {
            let mut cov_bg = 0u32;
            let mut cov_fg = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f32 + (sx as f32 + 0.5) / SS as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / SS as f32;
                    let qx = (px - 16.0).abs() - (half - r);
                    let qy = (py - 16.0).abs() - (half - r);
                    let d = (qx.max(0.0) * qx.max(0.0) + qy.max(0.0) * qy.max(0.0)).sqrt();
                    if d <= r {
                        cov_bg += 1;
                        if fg_hit(px, py) {
                            cov_fg += 1;
                        }
                    }
                }
            }
            let a_bg = cov_bg as f32 / (SS * SS) as f32;
            let a_fg = cov_fg as f32 / (SS * SS) as f32;
            if a_bg > 0.0 {
                let blend = |i: usize| -> u8 {
                    let v = fg[i] * a_fg + bg[i] * (a_bg - a_fg) / a_bg.max(1e-6);
                    (v * a_bg * 255.0) as u8
                };
                let i = (y * S + x) * 4;
                buf[i] = blend(2);
                buf[i + 1] = blend(1);
                buf[i + 2] = blend(0);
                buf[i + 3] = (a_bg * 255.0) as u8;
            }
        }
    }
    unsafe { bgra_to_hicon(&buf, S as i32, S as i32) }
}

fn make_zh_icon() -> isize {
    // 「中」：口 字环 + 竖贯通（与初版逐像素同参）
    let (bx0, by0, bx1, by1) = (8.8f32, 10.6f32, 23.2f32, 25.4f32);
    let stroke = 2.5f32;
    let (vx, vy0, vy1) = (16.0f32, 5.4f32, 27.6f32);
    make_plate_icon(move |px, py| {
        let in_outer = px >= bx0 && px <= bx1 && py >= by0 && py <= by1;
        let in_inner = px >= bx0 + stroke
            && px <= bx1 - stroke
            && py >= by0 + stroke
            && py <= by1 - stroke;
        let in_bar = (px - vx).abs() <= stroke / 2.0 && py >= vy0 && py <= vy1;
        (in_outer && !in_inner) || in_bar
    })
}

fn make_a_icon() -> isize {
    // 「A」：顶点 (16, 5.8)，底脚 (9.6, 26.4)/(22.4, 26.4)，两斜腿
    // 各宽 2.7；横杠 y∈[19.6, 22.2] 夹在两腿内侧。
    let apex = (16.0f32, 5.8f32);
    let lfoot = (9.6f32, 26.4f32);
    let rfoot = (22.4f32, 26.4f32);
    let stroke = 2.7f32;
    // 点到直线距离（有向）：腿 = |d| ≤ stroke/2
    let dist = |p: (f32, f32), a: (f32, f32), b: (f32, f32)| -> f32 {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        ((dy * p.0 - dx * p.1 + b.0 * a.1 - b.1 * a.0) / (dx * dx + dy * dy).sqrt()).abs()
    };
    let leg_l = |px: f32, py: f32| dist((px, py), apex, lfoot) <= stroke / 2.0;
    let leg_r = |px: f32, py: f32| dist((px, py), apex, rfoot) <= stroke / 2.0;
    make_plate_icon(move |px, py| {
        if py >= 19.6 && py <= 22.2 {
            // 横杠：x 夹在该高度两腿中心线之间（略收 0.4 防外凸）
            let t = (py - apex.1) / (lfoot.1 - apex.1);
            let xl = apex.0 + (lfoot.0 - apex.0) * t + 0.4;
            let xr = apex.0 + (rfoot.0 - apex.0) * t - 0.4;
            if px >= xl && px <= xr {
                return true;
            }
        }
        leg_l(px, py) || leg_r(px, py)
    })
}

/// BGRA 缓冲 → HICON（CreateBitmap 直接带位 + CreateIconIndirect；
/// mask 全零单色位图，透明由 32bpp alpha 决定）
unsafe fn bgra_to_hicon(buf: &[u8], w: i32, h: i32) -> isize {
    use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject, HBITMAP};
    let color: HBITMAP = CreateBitmap(w, h, 1, 32, Some(buf.as_ptr() as *const _));
    let mask: HBITMAP = CreateBitmap(w, h, 1, 1, None);
    let info = ICONINFO {
        fIcon: windows::Win32::Foundation::BOOL(1),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: color,
    };
    let hicon = CreateIconIndirect(&info).map(|h| h.0 as isize).unwrap_or(0);
    let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(color.0));
    let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(mask.0));
    hicon
}
