//! TSF 语言栏按钮：任务栏输入指示区常驻「中」字图标（Rime/虎爪风格）。
//!
//! 与 Shell_NotifyIcon 托盘图标不同：语言栏项显示在任务栏右下
//! 「输入指示区」（微软拼音 中/英、极点/虎爪 的字牌就在这里），
//! **永不进入折叠隐藏区**。生命周期跟随输入法：Activate 挂载、
//! Deactivate 移除——切到虎符才出现，与用户需求一致。
//!
//! 点击 → 管道 op "settings" 打开设置页（与托盘双击/Ctrl+Alt+H 同通道）。
use windows::core::{implement, Interface, Result, GUID};
use windows::Win32::Foundation::{BOOL, E_INVALIDARG, HANDLE, RECT};
use windows::Win32::UI::TextServices::{
    ITfLangBarItem, ITfLangBarItemButton, ITfLangBarItemButton_Impl, ITfLangBarItem_Impl,
    ITfLangBarItemMgr, ITfLangBarItemSink, ITfSource, ITfSource_Impl, TF_LANGBARITEMINFO,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

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

#[implement(ITfLangBarItem, ITfLangBarItemButton, ITfSource)]
pub struct HuFuLangBar {
    icon: isize, // HICON（isize 便于 ICONINFO 交互）
    /// msctf 挂的更新 sink（静态图标不主动通知，但 Advise 必须接受）
    sinks: std::cell::RefCell<Vec<(u32, ITfLangBarItemSink)>>,
    next_cookie: std::cell::Cell<u32>,
}

impl HuFuLangBar {
    pub fn new() -> HuFuLangBar {
        HuFuLangBar {
            icon: make_zh_icon(),
            sinks: std::cell::RefCell::new(Vec::new()),
            next_cookie: std::cell::Cell::new(0x4846_0001),
        }
    }
}

/// ITfSource：msctf（语言栏宿主）会对项 AdviseSink(ITfLangBarItemSink)。
/// 不实现该接口时 AddItem 在真实宿主里可能 E_FAIL；静态图标无需主动
/// OnUpdate，但挂/摘必须登记成功。
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
        let cookie = self.next_cookie.get();
        self.next_cookie.set(cookie + 1);
        self.sinks.borrow_mut().push((cookie, sink));
        Ok(cookie)
    }

    fn UnadviseSink(&self, dwcookie: u32) -> Result<()> {
        let mut v = self.sinks.borrow_mut();
        let before = v.len();
        v.retain(|(c, _)| *c != dwcookie);
        if v.len() == before {
            return Err(windows::core::Error::from(windows::Win32::Foundation::E_INVALIDARG));
        }
        Ok(())
    }
}

/// 挂载到线程的语言栏（Activate 时调用）。项实例存 thread_local：
/// msctf 的 AddItem/RemoveItem 按对象引用操作（同一 GUID 认领）。
pub fn install(mgr: &ITfLangBarItemMgr) -> Result<()> {
    let item: ITfLangBarItem = unsafe { HuFuLangBar::new().into() };
    let r = unsafe { mgr.AddItem(&item) };
    // 诊断标记：Activate 后 %TEMP%\hufu-langbar.txt 可查挂载结果
    let msg = match &r {
        Ok(()) => format!("ok pid={} t={:?}\n", std::process::id(), std::time::SystemTime::now()),
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
        let t: Vec<u16> = "中".encode_utf16().collect();
        text[..t.len()].copy_from_slice(&t);
        unsafe {
            (*pclbid).clsidService = CLSID_HUFU;
            (*pclbid).guidItem = LANGBAR_ITEM_GUID;
            (*pclbid).dwStyle = TF_LBI_STYLE_BTN_BUTTON | TF_LBI_STYLE_SHOWNINTRAY;
            (*pclbid).ulSort = 0;
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
        Ok(windows::core::BSTR::from("HuFu 虎符输入法（点击打开设置）"))
    }
}

impl ITfLangBarItemButton_Impl for HuFuLangBar_Impl {
    fn OnClick(
        &self,
        _click: windows::Win32::UI::TextServices::TfLBIClick,
        _pt: &windows::Win32::Foundation::POINT,
        _prcarea: *const RECT,
    ) -> Result<()> {
        // 与托盘双击同通道：管道 op "settings" → server 开设置页
        let _ = crate::ipc::call(&serde_json::json!({"op": "settings"}));
        Ok(())
    }

    fn InitMenu(&self, _pmenu: Option<&windows::Win32::UI::TextServices::ITfMenu>) -> Result<()> {
        Ok(())
    }

    fn OnMenuSelect(&self, _id: u32) -> Result<()> {
        Ok(())
    }

    fn GetIcon(&self) -> Result<HICON> {
        Ok(HICON(self.icon as *mut _))
    }

    fn GetText(&self) -> Result<windows::core::BSTR> {
        Ok(windows::core::BSTR::from("中"))
    }
}

// ───────────────────────── 「中」图标软件光栅化 ─────────────────────────
// 32×32 BGRA 超采样 4×；圆角方块底 + 「中」（口 字环 + 竖贯通）。
// 与 server 托盘爪印图标同一套画法（tray.rs make_hu_icon 的移植）。
fn make_zh_icon() -> isize {
    const S: usize = 32;
    const SS: usize = 4;
    let (bx0, by0, bx1, by1) = (8.8f32, 10.6f32, 23.2f32, 25.4f32); // 口 外沿
    let stroke = 2.5f32;
    let (vx, vy0, vy1) = (16.0f32, 5.4f32, 27.6f32); // 竖：中心 x 与上下端
    let r = 7.5f32; // 底圆角
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
                        // 口：在环上 = 在外沿内 且 不在缩进 stroke 的内沿内
                        let in_outer = px >= bx0 && px <= bx1 && py >= by0 && py <= by1;
                        let in_inner = px >= bx0 + stroke
                            && px <= bx1 - stroke
                            && py >= by0 + stroke
                            && py <= by1 - stroke;
                        // 竖：中心线 ± stroke/2，贯穿 y 范围
                        let in_bar = (px - vx).abs() <= stroke / 2.0 && py >= vy0 && py <= vy1;
                        if (in_outer && !in_inner) || in_bar {
                            cov_fg += 1;
                        }
                    }
                }
            }
            let a_bg = cov_bg as f32 / (SS * SS) as f32;
            let a_fg = cov_fg as f32 / (SS * SS) as f32;
            let a = a_bg;
            if a > 0.0 {
                let blend = |i: usize| -> u8 {
                    let v = fg[i] * a_fg + bg[i] * (a_bg - a_fg) / a_bg.max(1e-6);
                    (v * a * 255.0) as u8
                };
                let i = (y * S + x) * 4;
                buf[i] = blend(2);
                buf[i + 1] = blend(1);
                buf[i + 2] = blend(0);
                buf[i + 3] = (a * 255.0) as u8;
            }
        }
    }
    unsafe { bgra_to_hicon(&buf, S as i32, S as i32) }
}

/// BGRA 缓冲 → HICON（CreateBitmap 直接带位 + CreateIconIndirect；
/// mask 全零单色位图，透明由 32bpp alpha 决定）
unsafe fn bgra_to_hicon(buf: &[u8], w: i32, h: i32) -> isize {
    use windows::Win32::Graphics::Gdi::{
        CreateBitmap, DeleteObject, HBITMAP,
    };
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

