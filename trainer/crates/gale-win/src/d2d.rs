//! D2D 渲染：Direct3D11 + DXGI flip-model + DirectComposition + Direct2D。
//!
//! 管线（对照 _research/web-survey.md）：
//! D3D11 设备 → 合成交换链(FLIP_SEQUENTIAL, 预乘 alpha) → DComp 视图挂到 HWND
//! → D2D 设备上下文包裹交换链缓冲 → 每帧按 Frame 指令绘制 → Present(1,0)。
//!
//! 预乘 alpha 交换链是"磨砂/毛玻璃看见背后桌面"的关键：窗口像素 alpha=0 处
//! DWM 显示 backdrop（Acrylic/Blur 由 window.rs 的材质调用控制）。
//!
//! 文本（疾风1代 fonts.rs 视觉规格的 D2D 复刻）：
//! - 正文/UI = 系统字体探测（与虎符主程序同策略）：进程启动时用
//!   IDWriteFactory::GetSystemFontCollection 遍历家族本地化名，按优先级
//!   取第一个存在的霞鹜文楷家族；都没有则回退微软雅黑。探测一次进程内缓存；
//! - 测宽 = IDWriteTextLayout 簇宽度（逐字符推进量，折行与 1代逐字网格对齐）。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;

use windows::core::w;
use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Device, ID2D1DeviceContext, ID2D1Factory1, ID2D1SolidColorBrush,
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteFontFamily,
    IDWriteLocalizedStrings, IDWriteTextFormat, IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1, IDXGISwapChain4,
    DXGI_CREATE_FACTORY_FLAGS, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
    DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

// Move a break back rather than putting closing Chinese punctuation at a new
// line, or leaving an opening bracket stranded at the preceding line end.
fn line_break_end(chars: &[char], first: usize, mut end: usize) -> usize {
    while end > first + 1
        && end < chars.len()
        && ("，。！？；：、）》】〕〉」』”’…,.!?;:%".contains(chars[end])
            || "（《【〔〈「『“‘(".contains(chars[end - 1]))
    {
        end -= 1;
    }
    end
}

/// 词级重排（词不拆行）：给定 `wrap_text` 产出的行集合与词边界（全局字符下标
/// `(start, len)`），把跨行的多字词整体移到后一行；标点与单字仍可断行。
///
/// 背景：`wrap_text` 是纯像素宽贪心，会把词切断；且应用的布局缓存只在
/// 段/字号/宽度变化时重算，而词提列表是异步到达的——所以由 UI 层每帧调用
/// 本函数做修正。位置向量直接复用原行的 DirectWrite 实测推进量
/// （`pos[k+1] - pos[k]`），下划线、编码提示与光标因此保持像素级精度。
///
/// 返回 `None` 表示输入行集合原样可用，不需要提交新的布局。
/// 超过一行宽度的超长词无法整体放下，保持原断行（词长于行时必然被切）。
pub fn rewrap_words(
    lines: &[(usize, String, Vec<f32>)],
    words: &[(usize, usize)],
) -> Option<Vec<(usize, String, Vec<f32>)>> {
    let width = lines
        .iter()
        .filter_map(|line| line.2.last().copied())
        .fold(0., f32::max);
    rewrap_words_to_width(lines, words, width)
}

/// Reflow complete paragraphs using the actual available width. Existing
/// DirectWrite scalar advances remain the sole measurement source; fitting words
/// stay together, while a word wider than the viewport may break between scalars.
/// Explicit newline gaps in the source offsets remain paragraph boundaries.
pub fn rewrap_words_to_width(
    lines: &[(usize, String, Vec<f32>)],
    words: &[(usize, usize)],
    width: f32,
) -> Option<Vec<(usize, String, Vec<f32>)>> {
    if lines.is_empty() || !width.is_finite() || width <= 0. {
        return None;
    }
    let multi: Vec<_> = words.iter().copied().filter(|&(_, len)| len >= 2).collect();
    let overflows = lines
        .iter()
        .any(|line| line.2.last().is_some_and(|&x| x > width + 0.5));
    let straddles = lines.windows(2).any(|pair| {
        let boundary = pair[0].0 + pair[0].1.chars().count();
        pair[1].0 == boundary
            && multi.iter().any(|&(start, len)| {
                start < boundary && start.checked_add(len).is_some_and(|end| end > boundary)
            })
    });
    if !overflows && !straddles {
        return None;
    }

    // Rebuild paragraph advances from the measured source lines. A newline is
    // represented by an offset gap, so never merge across that gap or lose an
    // empty line. More output lines may be created than the original char wrap.
    let mut paragraphs: Vec<(usize, Vec<char>, Vec<f32>)> = Vec::new();
    for (base, text, positions) in lines {
        let chars: Vec<_> = text.chars().collect();
        if positions.len() != chars.len() + 1 || positions.iter().any(|x| !x.is_finite()) {
            return None;
        }
        let append = paragraphs
            .last()
            .is_some_and(|(start, chars, _)| *start + chars.len() == *base);
        if !append {
            paragraphs.push((*base, Vec::new(), vec![0.]));
        }
        let (_, paragraph, advances) = paragraphs.last_mut().unwrap();
        for (index, ch) in chars.into_iter().enumerate() {
            paragraph.push(ch);
            advances
                .push(advances.last().copied().unwrap() + positions[index + 1] - positions[index]);
        }
    }
    let mut out = Vec::new();
    for (base, chars, positions) in paragraphs {
        let n = chars.len();
        if n == 0 {
            out.push((base, String::new(), vec![0.]));
            continue;
        }
        // Protected boundaries only belong to words that actually fit. Oversize
        // phrases deliberately keep the ordinary scalar-break fallback.
        let mut protected = vec![None; n + 1];
        for &(start, len) in &multi {
            let Some(end) = start.checked_add(len) else {
                continue;
            };
            if start < base || end > base + n {
                continue;
            }
            let (start, end) = (start - base, end - base);
            if positions[end] - positions[start] <= width + 0.5 {
                protected[start + 1..end].fill(Some((start, end)));
            }
        }
        let mut first = 0;
        while first < n {
            let mut end = first + 1;
            while end < n && positions[end + 1] - positions[first] <= width + 0.5 {
                end += 1;
            }
            end = line_break_end(&chars, first, end);
            if let Some((start, word_end)) = protected[end] {
                // Punctuation adjustment may back into a fitting word at the
                // line start. Prefer keeping that word over the punctuation rule.
                end = if start > first { start } else { word_end };
            }
            let text: String = chars[first..end].iter().collect();
            let advances = positions[first..=end]
                .iter()
                .map(|x| x - positions[first])
                .collect();
            out.push((base + first, text, advances));
            first = end;
        }
    }
    if out.len() == lines.len() && out.iter().zip(lines).all(|(a, b)| a.0 == b.0 && a.1 == b.1) {
        None
    } else {
        Some(out)
    }
}
/// 渲染目标尺寸
#[derive(Debug, Clone, Copy)]
pub struct RenderSize {
    pub w: f32,
    pub h: f32,
}

/// 文本对齐（TextRun 专用；单字 Text 恒为居中）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// 一帧的渲染指令（UI 层生成，渲染层消费；与 D2D 解耦便于测试）
#[derive(Debug, Clone)]
pub enum DrawCmd {
    /// Blur all preceding commands, before the modal's dim and panel commands.
    /// Radius is the Gaussian standard deviation in logical pixels (CSS blur).
    /// Place outside any clip; unsupported effects leave the original backdrop.
    BlurBackdrop {
        radius: f32,
    },
    /// 背景填充（straight alpha，画刷直设）
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
    },
    /// 逐字文本（跟打正文：每个字单独一条便于分色；字号=fs）
    /// w = 字符格宽度（字形在格内按实际 advance 居中——ASCII 不偏左不挤）
    Text {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        ch: char,
        color: [f32; 4],
    },
    /// 圆角矩形（色块、卡片、chip、面板底、徽标）
    RoundRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        color: [f32; 4],
        stroke: Option<[f32; 4]>,
    },
    /// 行内连续文本（组字串/统计行等；align 决定 x 语义：Left=x 左缘 / Center=x 中心 / Right=x 右缘）
    TextRun {
        x: f32,
        y: f32,
        h: f32,
        text: String,
        color: [f32; 4],
        align: Align,
        font: FontFamily,
    },
    /// One DirectWrite layout per complete line preserves shaping and punctuation.
    StyledLine {
        x: f32,
        y: f32,
        w: f32,
        size: f32,
        text: String,
        spans: Vec<(u32, u32, [f32; 4])>,
        color: [f32; 4],
    },
    /// 1px 直线（顶栏分隔线、表格线；半透明）
    Line {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
    },
    /// 轴对齐裁剪域（卡片内容视窗；成对出现，Pop 恢复）
    PushClip {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    PopClip,
    /// 字形序列（DWrite glyph run 直绘；预留）
    Glyphs {
        x: f32,
        y: f32,
        glyphs: Vec<GlyphPos>,
        color: [f32; 4],
    },
}

#[derive(Debug, Clone, Copy)]
pub struct GlyphPos {
    pub x: f32,
    pub index: u16,
}

/// 帧上下文
pub struct Frame {
    pub size: RenderSize,
    pub cmds: Vec<DrawCmd>,
}

impl Frame {
    pub fn new(w: f32, h: f32) -> Self {
        Self {
            size: RenderSize { w, h },
            cmds: Vec::new(),
        }
    }
    pub fn push(&mut self, c: DrawCmd) {
        self.cmds.push(c);
    }
}

/// 字体族选择：Lxgw = 跟打正文，Ui = 界面（chip/统计/面板），Mono = 数字/编码。
/// Lxgw 保留系统探测的正文字体；Ui 使用微软雅黑，保证小字号标签清楚；
/// Mono 恒为系统等宽（Consolas）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontFamily {
    /// 跟打正文（系统探测：霞鹜文楷家族优先，回退微软雅黑）
    Lxgw,
    /// 界面标签（Microsoft YaHei UI）
    Ui,
    Mono,
}

// ── 系统字体探测（2026-11-08 用户拍板：去字体嵌入，改系统探测）──
//
// 与虎符主程序同策略：已安装霞鹜文楷就用，未安装回退微软雅黑。
// 遍历 GetSystemFontCollection 的家族本地化名（en-US 与 zh-CN 名都比对，
// 覆盖只装了中文名族或英文名族的安装形态），按下表优先级取第一个命中；
// 都没有 → "Microsoft YaHei UI"。探测一次，进程内缓存。

/// 正文家族优先级表（前 = 优先）。中英文名成对给出，
/// 对家族的任一本地化名做大小写不敏感比对。
const CJK_FAMILY_CANDIDATES: [&str; 10] = [
    "霞鹜文楷 GB Screen",
    "LXGW WenKai GB Screen",
    "霞鹜文楷 GB",
    "LXGW WenKai GB",
    "霞鹜文楷 Screen",
    "LXGW WenKai Screen",
    "霞鹜文楷",
    "LXGW WenKai",
    "霞鹜新晰黑",
    "LXGW WenKai Lite",
];

/// 家族全缺失时的回退（微软雅黑；系统必装）
const CJK_FAMILY_FALLBACK: &str = "Microsoft YaHei UI";

/// 探测结果缓存（进程内一次）
static DETECTED_CJK_FAMILY: OnceLock<String> = OnceLock::new();

/// 取正文字体家族（系统探测，进程内缓存）。
/// 返回值直接作 CreateTextFormat 家族名（命中候选表原样返回，未命中返回回退名）。
pub fn detected_cjk_family() -> &'static str {
    DETECTED_CJK_FAMILY.get_or_init(detect_cjk_family)
}

/// 已安装的自选字体优先；配置中的字体被卸载时按默认链回退。
pub fn resolved_cjk_family(requested: &str) -> String {
    static NAMES: OnceLock<HashSet<String>> = OnceLock::new();
    let requested = requested.trim();
    if requested.is_empty() {
        return detected_cjk_family().into();
    }
    let names = NAMES.get_or_init(|| unsafe {
        DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED)
            .ok()
            .and_then(|factory| system_family_names(&factory))
            .unwrap_or_default()
    });
    if names.contains(&requested.to_lowercase()) {
        requested.into()
    } else {
        detected_cjk_family().into()
    }
}

fn detect_cjk_family() -> String {
    let factory: IDWriteFactory = unsafe {
        match DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) {
            Ok(f) => f,
            Err(e) => {
                eprintln!(
                    "[d2d] DWriteCreateFactory 失败，正文字体回退 {CJK_FAMILY_FALLBACK}: {e:?}"
                );
                return CJK_FAMILY_FALLBACK.to_string();
            }
        }
    };
    let names = unsafe { system_family_names(&factory) };
    let Some(names) = names else {
        eprintln!("[d2d] 系统字体集枚举失败，正文字体回退 {CJK_FAMILY_FALLBACK}");
        return CJK_FAMILY_FALLBACK.to_string();
    };
    for candidate in CJK_FAMILY_CANDIDATES {
        if names.contains(&candidate.to_lowercase()) {
            return candidate.to_string();
        }
    }
    CJK_FAMILY_FALLBACK.to_string()
}

/// 收集系统字体集内全部家族名（小写；含每个家族的全部本地化名）
unsafe fn system_family_names(factory: &IDWriteFactory) -> Option<HashSet<String>> {
    let mut collection: Option<IDWriteFontCollection> = None;
    factory
        .GetSystemFontCollection(&mut collection, false)
        .ok()?;
    let collection = collection?;
    let count = collection.GetFontFamilyCount();
    let mut names = HashSet::new();
    for i in 0..count {
        let family: IDWriteFontFamily = collection.GetFontFamily(i).ok()?;
        let localized: IDWriteLocalizedStrings = family.GetFamilyNames().ok()?;
        for s in 0..localized.GetCount() {
            if let Some(name) = localized_string(&localized, s) {
                names.insert(name.to_lowercase());
            }
        }
    }
    Some(names)
}

/// 正文字体下拉与渲染使用同一个 DirectWrite 字体集。
/// 每个家族显示一个名称，优先中文，其次英文，避免列出重复字形/字号。
pub fn installed_font_families() -> Result<Vec<String>, String> {
    unsafe {
        let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
            .map_err(|e| format!("无法读取系统字体：{e}"))?;
        let mut collection = None;
        factory.GetSystemFontCollection(&mut collection, false)
            .map_err(|e| format!("无法读取系统字体：{e}"))?;
        let collection = collection.ok_or("系统字体集合为空")?;
        let mut names = Vec::new();
        for i in 0..collection.GetFontFamilyCount() {
            let Ok(family) = collection.GetFontFamily(i) else { continue };
            let Ok(localized) = family.GetFamilyNames() else { continue };
            let mut index = 0;
            for locale in [w!("zh-CN"), w!("en-US")] {
                let mut found = windows::Win32::Foundation::BOOL(0);
                if localized.FindLocaleName(locale, &mut index, &mut found).is_ok()
                    && found.as_bool()
                {
                    break;
                }
                index = 0;
            }
            if let Some(name) = localized_string(&localized, index) {
                if !name.is_empty() && !name.starts_with('@') {
                    names.push(name);
                }
            }
        }
        names.sort_by_key(|name| name.to_lowercase());
        names.dedup_by(|a, b| a.to_lowercase() == b.to_lowercase());
        if names.is_empty() {
            Err("系统字体集合为空".into())
        } else {
            Ok(names)
        }
    }
}

/// 取本地化串第 index 项
unsafe fn localized_string(ls: &IDWriteLocalizedStrings, index: u32) -> Option<String> {
    let len = ls.GetStringLength(index).ok()? as usize;
    let mut buf = vec![0u16; len + 1];
    ls.GetString(index, &mut buf).ok()?;
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// 字体 + 排版工具箱（一个 Renderer 一份）：
/// - 正文家族 = 用户所选系统字体（缺失时走霞鹜/雅黑回退），UI = Microsoft YaHei UI；
/// - (family, size) → IDWriteTextFormat 缓存（跟打器字号固定档位少，全缓存）；
/// - 逐簇测宽（折行/贪吃蛇/光标定位的统一真值）。
pub struct FontKit {
    dwrite: IDWriteFactory,
    body_family: String,
    requested_family: String,
    fmt_cache: HashMap<(FontFamily, u32), IDWriteTextFormat>,
    /// 测宽缓存：(family, size_q, ch) → 宽度（跟打器字符集小，命中率高）
    width_cache: HashMap<(FontFamily, u32, char), f32>,
}

impl FontKit {
    /// 创建字体工具箱。系统家族探测惰性完成（首次 CreateTextFormat 时一次）。
    pub fn new() -> Self {
        let dwrite: IDWriteFactory = unsafe {
            DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).expect("DWriteCreateFactory")
        };
        Self {
            dwrite,
            body_family: detected_cjk_family().into(),
            requested_family: String::new(),
            fmt_cache: HashMap::new(),
            width_cache: HashMap::new(),
        }
    }

    /// 改字体时同时失效绘制与测宽缓存，折行、字框与光标保持一致。
    pub fn set_body_family(&mut self, requested: &str) -> bool {
        if self.requested_family == requested {
            return false;
        }
        self.requested_family = requested.into();
        let family = resolved_cjk_family(requested);
        if self.body_family == family {
            return false;
        }
        self.body_family = family;
        self.fmt_cache.clear();
        self.width_cache.clear();
        true
    }

    /// 取（或建）文本格式。size 物理像素字号，量化到 0.25px 减少缓存键。
    pub fn format(
        &mut self,
        family: FontFamily,
        size: f32,
    ) -> windows::core::Result<IDWriteTextFormat> {
        let q = (size * 4.0).round() as u32;
        if let Some(fmt) = self.fmt_cache.get(&(family, q)) {
            return Ok(fmt.clone());
        }
        let fmt = unsafe { self.make_format(family, q as f32 / 4.0)? };
        self.fmt_cache.insert((family, q), fmt.clone());
        Ok(fmt)
    }

    unsafe fn make_format(
        &self,
        family: FontFamily,
        size: f32,
    ) -> windows::core::Result<IDWriteTextFormat> {
        // 正文保留用户习惯，界面标签使用清晰的无衬线字体。
        let family_name = match family {
            FontFamily::Lxgw => self.body_family.as_str(),
            FontFamily::Ui => "Microsoft YaHei UI",
            FontFamily::Mono => "Consolas",
        };
        // PCWSTR borrows this buffer; keep it alive until DirectWrite has
        // copied the family name in CreateTextFormat.
        let name: Vec<u16> = family_name.encode_utf16().chain(Some(0)).collect();
        let fmt = self.dwrite.CreateTextFormat(
            windows::core::PCWSTR(name.as_ptr()),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            windows::Win32::Graphics::DirectWrite::DWRITE_FONT_STYLE_NORMAL,
            windows::Win32::Graphics::DirectWrite::DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("zh-CN"),
        )?;
        let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        let _ = fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
        // 折行由 UI 层逐字测宽自控，禁掉 DWrite 自动折行（测宽/绘制都不换行）
        let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        Ok(fmt)
    }

    /// 逐字符宽度（DWrite 排版真值，跟打器折行/光标/贪吃蛇统一口径）。
    /// 结果进 width_cache；字号同样量化 0.25px。
    pub fn char_width(&mut self, family: FontFamily, size: f32, ch: char) -> f32 {
        let q = (size * 4.0).round() as u32;
        if let Some(w) = self.width_cache.get(&(family, q, ch)) {
            return *w;
        }
        let w = unsafe {
            let fmt = match self.format(family, q as f32 / 4.0) {
                Ok(f) => f,
                Err(_) => return size * if ch.is_ascii() { 0.5 } else { 1.0 },
            };
            let s: Vec<u16> = utf16_of(ch);
            let layout: IDWriteTextLayout =
                match self
                    .dwrite
                    .CreateTextLayout(&s, &fmt, size * 4.0, size * 4.0)
                {
                    Ok(l) => l,
                    Err(_) => return size * if ch.is_ascii() { 0.5 } else { 1.0 },
                };
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_err() {
                return size * if ch.is_ascii() { 0.5 } else { 1.0 };
            }
            m.widthIncludingTrailingWhitespace
        };
        let w = w.max(1.0);
        self.width_cache.insert((family, q, ch), w);
        w
    }

    /// 行内文本总宽（逐簇累加，与折行同源）
    pub fn text_width(&mut self, family: FontFamily, size: f32, text: &str) -> f32 {
        text.chars().map(|c| self.char_width(family, size, c)).sum()
    }

    /// ASCII 宽度表（0..=127，单位 = advance / 字号）：
    /// UI 层折行/光标/色块用它把每个 ASCII 字符的格子宽定为真实比例
    /// （×字号），替代固定 0.56 —— 宽字母（m/W）和数字（0.60）不再
    /// 溢出格子挤压重叠，窄字母（i/l）不再占过宽的格。
    /// 比例与字号无关（字号放大只是等比），一次测量全局复用。
    pub fn ascii_width_ratio_table(&mut self) -> [f32; 128] {
        let mut t = [0.56f32; 128];
        for b in 32u8..127 {
            let ch = b as char;
            let w = self.char_width(FontFamily::Lxgw, 100.0, ch);
            t[b as usize] = (w / 100.0).clamp(0.28, 1.0);
        }
        t
    }
}

/// 完整渲染器（D3D11 + DXGI + DComp + D2D + DWrite + FontKit）
pub struct Renderer {
    hwnd: isize,
    _d3d: ID3D11Device,
    dcomp_device: IDCompositionDevice,
    _dcomp_target: IDCompositionTarget,
    visual: IDCompositionVisual,
    d2d_ctx: ID2D1DeviceContext,
    brush: ID2D1SolidColorBrush,
    swapchain: IDXGISwapChain4,
    pub fonts: FontKit,
    pub width: f32,
    pub height: f32,
    /// 拖动尺寸期间的同步填充色（on_window_size_sync_fill）
    resize_fill: [f32; 4],
    /// 最近一帧（WM_SIZE 同步重放用；draw_frame 时记录）
    last_frame: Option<Frame>,
    last_frame_size: RenderSize,
    /// A failed optional effect must not prevent subsequent normal frames.
    blur_unavailable: bool,
}

/// 单字符 UTF-16（BMP 内 1 单元；罕见增平面字给代理对，量宽/绘制都兼容）
fn utf16_of(ch: char) -> Vec<u16> {
    let mut buf = [0u16; 2];
    let n = ch.encode_utf16(&mut buf).len();
    buf[..n].to_vec()
}

impl Renderer {
    /// 创建渲染器并挂到窗口。w/h 物理像素。
    pub fn new(hwnd: isize, w: u32, h: u32) -> windows::core::Result<Self> {
        unsafe {
            // 1) D3D11 设备（BGRA 支持是 D2D 互操作前提）
            let mut d3d: Option<ID3D11Device> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut d3d),
                None,
                None,
            )
            .map_err(|e| {
                eprintln!("[d2d] D3D11CreateDevice 失败: {e:?}");
                e
            })?;
            let d3d = d3d.unwrap();

            // 2) 合成交换链（预乘 alpha + flip + 低延迟）
            let dxgi_dev: IDXGIDevice = d3d.cast().map_err(|e| {
                eprintln!("[d2d] cast IDXGIDevice 失败: {e:?}");
                e
            })?;
            let factory: IDXGIFactory2 =
                CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).map_err(|e| {
                    eprintln!("[d2d] CreateDXGIFactory2 失败: {e:?}");
                    e
                })?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: w,
                Height: h,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                // 合成交换链必须显式 STRETCH（flip-model 不接受 UNSPECIFIED=0）
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
            };
            let sc: IDXGISwapChain1 = factory
                .CreateSwapChainForComposition(&d3d, &desc, None)
                .map_err(|e| {
                    eprintln!("[d2d] CreateSwapChainForComposition 失败: {e:?} (w={w} h={h})");
                    e
                })?;
            let swapchain: IDXGISwapChain4 = sc.cast().map_err(|e| {
                eprintln!("[d2d] cast IDXGISwapChain4 失败: {e:?}");
                e
            })?;

            // 3) DComp：视图挂 HWND
            let dcomp_device: IDCompositionDevice =
                DCompositionCreateDevice(&dxgi_dev).map_err(|e| {
                    eprintln!("[d2d] DCompositionCreateDevice 失败: {e:?}");
                    e
                })?;
            let dcomp_target = dcomp_device
                .CreateTargetForHwnd(windows::Win32::Foundation::HWND(hwnd as *mut _), false)
                .map_err(|e| {
                    eprintln!("[d2d] CreateTargetForHwnd 失败: {e:?} (hwnd={hwnd:x})");
                    e
                })?;
            let visual = dcomp_device.CreateVisual().map_err(|e| {
                eprintln!("[d2d] CreateVisual 失败: {e:?}");
                e
            })?;
            visual.SetContent(&sc).map_err(|e| {
                eprintln!("[d2d] SetContent 失败: {e:?}");
                e
            })?;
            dcomp_target.SetRoot(&visual).map_err(|e| {
                eprintln!("[d2d] SetRoot 失败: {e:?}");
                e
            })?;
            dcomp_device.Commit().map_err(|e| {
                eprintln!("[d2d] Commit 失败: {e:?}");
                e
            })?;

            // 4) D2D 设备上下文
            let d2d_factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).map_err(|e| {
                    eprintln!("[d2d] D2D1CreateFactory 失败: {e:?}");
                    e
                })?;
            let d2d_dev: ID2D1Device = d2d_factory.CreateDevice(&dxgi_dev).map_err(|e| {
                eprintln!("[d2d] D2D CreateDevice 失败: {e:?}");
                e
            })?;
            let d2d_ctx = d2d_dev
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
                .map_err(|e| {
                    eprintln!("[d2d] CreateDeviceContext 失败: {e:?}");
                    e
                })?;

            let brush = d2d_ctx
                .CreateSolidColorBrush(
                    &D2D1_COLOR_F {
                        r: 1.0,
                        g: 1.0,
                        b: 1.0,
                        a: 1.0,
                    },
                    None,
                )
                .map_err(|e| {
                    eprintln!("[d2d] CreateSolidColorBrush 失败: {e:?}");
                    e
                })?;

            // 字体工具箱（LXGW 内存加载；失败回退系统雅黑）
            let fonts = FontKit::new();

            let mut r = Self {
                hwnd,
                _d3d: d3d,
                dcomp_device,
                _dcomp_target: dcomp_target,
                visual,
                d2d_ctx,
                brush,
                swapchain,
                fonts,
                width: w as f32,
                height: h as f32,
                resize_fill: [0.149, 0.149, 0.165, 1.0], // #2626 2a 近似（主题加载后覆盖）
                last_frame: None,
                last_frame_size: RenderSize {
                    w: w as f32,
                    h: h as f32,
                },
                blur_unavailable: false,
            };
            r.rebind_target().map_err(|e| {
                eprintln!("[d2d] rebind_target 失败: {e:?}");
                e
            })?;
            Ok(r)
        }
    }

    unsafe fn rebind_target(&mut self) -> windows::core::Result<()> {
        let pf = D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        };
        let bmp_props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: pf,
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET
                | windows::Win32::Graphics::Direct2D::D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let buffer: IDXGISurface = self.swapchain.GetBuffer(0).map_err(|e| {
            eprintln!("[d2d] GetBuffer 失败: {e:?}");
            e
        })?;
        let bitmap = self
            .d2d_ctx
            .CreateBitmapFromDxgiSurface(&buffer, Some(&bmp_props))
            .map_err(|e| {
                eprintln!("[d2d] CreateBitmapFromDxgiSurface 失败: {e:?}");
                e
            })?;
        let target: windows::Win32::Graphics::Direct2D::ID2D1Image = bitmap.cast()?;
        self.d2d_ctx.SetTarget(&target);
        Ok(())
    }

    /// 不抛错的 resize：失败也吞掉（保住窗口和主循环），下帧重试。
    /// 主循环每次发现 client 尺寸与 renderer 记录不一致就会再调 resize，
    /// 所以瞬时失败自愈；持续失败（设备真没了）时按原尺寸继续画——
    /// 宁可画面比例旧一点也不闪退。
    pub fn resize_lossy(&mut self, w: u32, h: u32) {
        if self.resize(w, h).is_err() {
            eprintln!("[d2d] resize {w}x{h} 失败（已吞，保窗口）");
            // 尺寸记录不同步会导致每帧重试 resize；把记录对齐为"目标值"
            // 之外的事实尺寸无法读回——退而求其次：标记为新尺寸，
            // 若底层实际没变，Present 自动拉伸（STRETCH 缩放模式），不崩溃。
            self.width = w as f32;
            self.height = h as f32;
        }
    }

    /// WM_SIZE 同步补底色（拖边框防灰/黑）：模态尺寸循环期间主循环不跑帧，
    /// 新暴露区域没有内容 → 灰/黑。这里同步 resize 缓冲 + 重画上一帧内容
    /// （缩放适配新尺寸——"放大缩小"的连续感）。
    ///
    /// 实现：缓存最近一帧的 DrawCmd（draw_frame 时记录）。WM_SIZE 时把
    /// 旧帧以目标尺寸重画：内容按新 w/h 重排（Rect/Line 拉伸底/条带，
    /// 文本保持原位）——低频路径，指令重放足够快（<1ms 级）。
    pub fn on_window_size_sync_fill(&mut self, w: i32, h: i32) {
        if w <= 0 || h <= 0 {
            return;
        }
        if (w as f32 - self.width).abs() > 0.5 || (h as f32 - self.height).abs() > 0.5 {
            self.resize_lossy(w as u32, h as u32);
        }
        unsafe {
            self.d2d_ctx.BeginDraw();
            self.d2d_ctx.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            // 重放上一帧（scale_x/scale_y 把旧坐标系映射到新尺寸）；
            // 这是物理→物理的位图级拉伸（无 DPI 语义，保持原样）
            let old = (self.last_frame_size.w, self.last_frame_size.h);
            let (sx, sy) = if old.0 > 0.0 && old.1 > 0.0 {
                (self.width / old.0, self.height / old.1)
            } else {
                (1.0, 1.0)
            };
            self.replay_frame(sx, sy);
            if self.d2d_ctx.EndDraw(None, None).is_ok() {
                let _ = self.swapchain.Present(0, DXGI_PRESENT(0));
            }
            let _ = self.rebind_target();
        }
    }

    /// 重放缓存的上一帧指令（WM_SIZE 同步补帧；坐标按 sx/sy 缩放）
    unsafe fn replay_frame(&mut self, sx: f32, sy: f32) {
        let cmds = match &self.last_frame {
            Some(f) => f.cmds.clone(),
            None => {
                // 无缓存帧（极早期 resize）：退回纯底色
                let [r, g, b, a] = self.resize_fill;
                self.d2d_ctx.Clear(Some(&D2D1_COLOR_F { r, g, b, a }));
                return;
            }
        };
        let m = windows::Foundation::Numerics::Matrix3x2 {
            M11: sx,
            M12: 0.0,
            M21: 0.0,
            M22: sy,
            M31: 0.0,
            M32: 0.0,
        };
        self.d2d_ctx.SetTransform(&m);
        self.exec_cmds(&cmds);
        // 恢复单位变换（主循环下一帧自己再设）
        self.d2d_ctx
            .SetTransform(&windows::Foundation::Numerics::Matrix3x2 {
                M11: 1.0,
                M12: 0.0,
                M21: 0.0,
                M22: 1.0,
                M31: 0.0,
                M32: 0.0,
            });
    }

    /// 拖动尺寸期间的填充色（主题窗体背景色；main 在主题加载后设置）
    pub fn set_resize_fill(&mut self, c: [f32; 4]) {
        self.resize_fill = c;
    }

    /// WM_SIZE 同步真帧绘制（gale-app 重建的完整 UI 帧直绘）：
    /// resize 缓冲 → 按指令绘制 → Present。供 set_size_rebuild_callback
    /// 回调末端调用（比 on_window_size_sync_fill 的位图重放质量高：
    /// 文字原字号、布局按新尺寸重排）。
    pub fn draw_sync_frame(&mut self, w: i32, h: i32, frame: &Frame) {
        if w <= 0 || h <= 0 {
            return;
        }
        if (w as f32 - self.width).abs() > 0.5 || (h as f32 - self.height).abs() > 0.5 {
            self.resize_lossy(w as u32, h as u32);
        }
        unsafe {
            self.d2d_ctx.BeginDraw();
            self.d2d_ctx.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            // DPI 变换与 draw_frame 同款（逻辑帧 → 物理交换链）
            let sc = crate::window::dpi_scale();
            self.d2d_ctx
                .SetTransform(&windows::Foundation::Numerics::Matrix3x2 {
                    M11: sc,
                    M12: 0.0,
                    M21: 0.0,
                    M22: sc,
                    M31: 0.0,
                    M32: 0.0,
                });
            self.exec_cmds(&frame.cmds);
            if self.d2d_ctx.EndDraw(None, None).is_ok() {
                let _ = self.swapchain.Present(0, DXGI_PRESENT(0));
            }
            self.d2d_ctx
                .SetTransform(&windows::Foundation::Numerics::Matrix3x2 {
                    M11: 1.0,
                    M12: 0.0,
                    M21: 0.0,
                    M22: 1.0,
                    M31: 0.0,
                    M32: 0.0,
                });
            // 记录为最近帧（后续 WM_SIZE 若回调丢失还有位图重放兜底）
            self.last_frame = Some(Frame {
                size: frame.size,
                cmds: frame.cmds.clone(),
            });
            self.last_frame_size = frame.size;
            let _ = self.rebind_target();
        }
    }

    /// 窗口尺寸变化（重建缓冲）
    ///
    /// ResizeBuffers 失败（设备移除/超时等）不再向上抛——改走**整链重建**：
    /// 新建同参数交换链 + 重挂 DComp 视觉内容。仍失败才返回 Err（调用方
    /// 会退出，但此时窗口已不可救）。这修掉了"拉动边框闪退"：旧的
    /// `resize()` 抛错 → main 循环 `渲染帧失败，退出`。
    pub fn resize(&mut self, w: u32, h: u32) -> windows::core::Result<()> {
        unsafe {
            if w == 0 || h == 0 {
                // 最小化过程中会路过 0 尺寸；跳过（恢复时会再来正确的尺寸）
                return Ok(());
            }
            self.d2d_ctx.SetTarget(None);
            // ★ flags 必须与创建时一致（含 FRAME_LATENCY_WAITABLE_OBJECT），
            //   否则 DXGI_ERROR_INVALID_CALL——这就是"拉边框闪退"的根因：
            //   旧代码传 0 → 必败 → Err 上抛 → 渲染帧失败，退出。
            let flags =
                DXGI_SWAP_CHAIN_FLAG(DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as i32);
            if self
                .swapchain
                .ResizeBuffers(0, w, h, DXGI_FORMAT_B8G8R8A8_UNORM, flags)
                .is_err()
            {
                eprintln!("[d2d] ResizeBuffers 失败，尝试整链重建 ({w}x{h})");
                return self.recreate_swapchain(w, h);
            }
            self.width = w as f32;
            self.height = h as f32;
            self.rebind_target()
        }
    }

    /// 整链重建交换链（ResizeBuffers 失败的兜底）。
    /// 新交换链创建后必须重挂 DComp 视觉内容，否则窗口黑屏。
    unsafe fn recreate_swapchain(&mut self, w: u32, h: u32) -> windows::core::Result<()> {
        // 1) 从现有交换链拿 factory（保证同适配器）
        let factory: IDXGIFactory2 = self.swapchain.GetParent::<IDXGIFactory2>()?.cast()?;
        // 2) 从 D2D 上下文取设备 → IDXGIDevice（CreateSwapChainForComposition 用）
        let d2d_dev: ID2D1Device = self.d2d_ctx.GetDevice()?;
        let dxgi_dev: IDXGIDevice = d2d_dev.cast()?;
        // 3) 摘掉旧内容引用（旧交换链即将销毁；DComp 持有内容指针会阻碍释放）
        let _ = self.visual.SetContent(None);
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: w,
            Height: h,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            // 重建链不用 waitable 标志（避免旧链的 waitable 句柄语义迁移问题）
            Flags: 0,
        };
        let sc: IDXGISwapChain1 = factory.CreateSwapChainForComposition(&dxgi_dev, &desc, None)?;
        // 旧链在此丢弃（drop 释放）→ 新链接上
        self.swapchain = sc.cast()?;
        self.width = w as f32;
        self.height = h as f32;
        // 4) 重挂视觉内容（DComp 视图仍挂在 HWND 上，只换底层内容）；
        //    SetContent 接受 IUnknown（swapchain 直接满足）
        self.visual.SetContent(&self.swapchain)?;
        let _ = self.dcomp_device.Commit();
        self.rebind_target()
    }

    /// 渲染一帧（指令消费 + Present）
    pub fn draw_frame(&mut self, frame: &Frame) -> windows::core::Result<()> {
        unsafe {
            self.d2d_ctx.BeginDraw();
            // 整帧清成透明（alpha=0 处透出 DWM backdrop —— 磨砂/毛玻璃的关键）
            self.d2d_ctx.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));

            // ★ DPI 全局变换（150% 模糊根治）：帧坐标系 = 96-DPI 逻辑 px，
            //   交换链 = 物理 px。SetTransform(scale) 一步放大整个 UI——
            //   文字矢量按物理分辨率光栅化（清晰），ui.rs 布局常量零改动。
            //   Clear 必须在变换前（全屏清透明不受变换影响）。
            let sc = crate::window::dpi_scale();
            self.d2d_ctx
                .SetTransform(&windows::Foundation::Numerics::Matrix3x2 {
                    M11: sc,
                    M12: 0.0,
                    M21: 0.0,
                    M22: sc,
                    M31: 0.0,
                    M32: 0.0,
                });

            // 白条猎捕（GALE_BARHUNT=3）：Clear 后立刻画不透明黑全窗幕布——
            // 若白条仍在=它在交换链之下(DComp/GDI层)；消失=在 exec_cmds 路径内。
            if std::env::var("GALE_BARHUNT").as_deref() == Ok("3") {
                self.set_brush([0.0, 0.0, 0.0, 1.0]);
                let full = D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: self.width,
                    bottom: self.height,
                };
                self.d2d_ctx.FillRectangle(&full, &self.brush);
            }

            self.exec_cmds(&frame.cmds);

            // BARHUNT=4：exec_cmds 后【再 Clear 一次】——若白条是 cmds 之后/
            // Present 路径写入，二次清不掉它；若能清掉=白条其实在 cmds 内
            if std::env::var("GALE_BARHUNT").as_deref() == Ok("4") {
                self.d2d_ctx.Clear(Some(&D2D1_COLOR_F {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                }));
            }

            self.d2d_ctx.EndDraw(None, None)?;
            // EndDraw 后复位单位变换（防后续 rebind/别的绘制路径继承 scale）
            self.d2d_ctx
                .SetTransform(&windows::Foundation::Numerics::Matrix3x2 {
                    M11: 1.0,
                    M12: 0.0,
                    M21: 0.0,
                    M22: 1.0,
                    M31: 0.0,
                    M32: 0.0,
                });
            // 记录本帧（WM_SIZE 同步补帧重放用）
            self.last_frame = Some(Frame {
                size: frame.size,
                cmds: frame.cmds.clone(),
            });
            self.last_frame_size = frame.size;
            // SyncInterval=0：不等待垂直同步。E2E/后台桌面实测：窗口不在
            // 交互桌面（无 DWM 合成）时 Present(1) 永久阻塞且内部泵消息，
            // 造成"主循环假死但 WM_CHAR 仍被处理"的诡象（e2e-run9 诊断定论）。
            // 不等 VBlank 在前台桌面代价是多耗一点 GPU，跟打器 120fps 节流
            // 由上层 6e 的 8ms 门限负责。
            self.swapchain.Present(0, DXGI_PRESENT(0)).ok()?;
            self.rebind_target()
        }
    }

    /// Export the rendered frame directly from the D3D surface. No shell or screen capture.
    pub fn save_bmp(&mut self, path: &Path) -> Result<(), String> {
        self.render_export_frame()?;
        self.write_target_bmp(path)
    }

    /// Copy the rendered frame, or a pixel-space crop, as a native CF_DIB image.
    /// Replaying is necessary because Present has already flipped the back buffer.
    pub fn copy_bmp_to_clipboard(
        &mut self,
        crop: Option<(u32, u32, u32, u32)>,
    ) -> Result<(), String> {
        self.render_export_frame()?;
        let dib = self.read_target_dib(crop)?;
        crate::clipboard_bridge::try_write_dib(self.hwnd, &dib)
    }

    fn render_export_frame(&mut self) -> Result<(), String> {
        unsafe {
            let cmds = self
                .last_frame
                .as_ref()
                .ok_or("没有可导出的画面")?
                .cmds
                .clone();
            let mut previous_transform = windows::Foundation::Numerics::Matrix3x2::default();
            self.d2d_ctx.GetTransform(&mut previous_transform);
            self.d2d_ctx.BeginDraw();
            self.d2d_ctx.Clear(Some(&D2D1_COLOR_F {
                r: 1.,
                g: 1.,
                b: 1.,
                a: 1.,
            }));
            let sc = crate::window::dpi_scale();
            self.d2d_ctx
                .SetTransform(&windows::Foundation::Numerics::Matrix3x2 {
                    M11: sc,
                    M12: 0.,
                    M21: 0.,
                    M22: sc,
                    M31: 0.,
                    M32: 0.,
                });
            self.exec_cmds(&cmds);
            let result = self.d2d_ctx.EndDraw(None, None);
            self.d2d_ctx.SetTransform(&previous_transform);
            result.map_err(|e| e.to_string())
        }
    }

    fn write_target_bmp(&self, path: &Path) -> Result<(), String> {
        let dib = self.read_target_dib(None)?;
        let size = dib.len().checked_add(14).ok_or("图像尺寸过大")?;
        let file_size = u32::try_from(size).map_err(|_| "图像尺寸过大")?;
        let mut out = Vec::new();
        out.try_reserve_exact(size)
            .map_err(|_| "无法分配图像内存")?;
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&file_size.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&dib);
        std::fs::write(path, out).map_err(|e| e.to_string())
    }

    /// BITMAPINFOHEADER followed by tightly packed, top-down BGRx pixels.
    fn read_target_dib(&self, crop: Option<(u32, u32, u32, u32)>) -> Result<Vec<u8>, String> {
        use windows::Win32::Graphics::Direct3D11::*;
        unsafe {
            let source: ID3D11Texture2D = self.swapchain.GetBuffer(0).map_err(|e| e.to_string())?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            source.GetDesc(&mut desc);
            if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM || desc.SampleDesc.Count != 1 {
                return Err("渲染表面的像素格式不支持导出".into());
            }
            let (x, y, width, height) = crop.unwrap_or((0, 0, desc.Width, desc.Height));
            if width == 0
                || height == 0
                || x >= desc.Width
                || y >= desc.Height
                || width > desc.Width - x
                || height > desc.Height - y
                || width > i32::MAX as u32
                || height > i32::MAX as u32
            {
                return Err("导出范围为空或超出画面".into());
            }
            let row = (width as usize).checked_mul(4).ok_or("图像尺寸过大")?;
            let pixels = row.checked_mul(height as usize).ok_or("图像尺寸过大")?;
            let image_size = u32::try_from(pixels).map_err(|_| "图像尺寸过大")?;
            let size = 40usize.checked_add(pixels).ok_or("图像尺寸过大")?;
            let mut out = Vec::new();
            out.try_reserve_exact(size)
                .map_err(|_| "无法分配图像内存")?;
            out.extend_from_slice(&40u32.to_le_bytes());
            out.extend_from_slice(&(width as i32).to_le_bytes());
            out.extend_from_slice(&(-(height as i32)).to_le_bytes());
            out.extend_from_slice(&1u16.to_le_bytes());
            out.extend_from_slice(&32u16.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
            out.extend_from_slice(&image_size.to_le_bytes());
            out.extend_from_slice(&[0; 16]);
            out.resize(size, 0);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            let mut staging = None;
            self._d3d
                .CreateTexture2D(&desc, None, Some(&mut staging))
                .map_err(|e| e.to_string())?;
            let staging = staging.ok_or("无法创建图像缓冲")?;
            let ctx = self._d3d.GetImmediateContext().map_err(|e| e.to_string())?;
            ctx.CopyResource(&staging, &source);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| e.to_string())?;
            if mapped.pData.is_null() || (mapped.RowPitch as usize) < desc.Width as usize * 4 {
                ctx.Unmap(&staging, 0);
                return Err("无法读取渲染表面的像素".into());
            }
            // No fallible operation between Map and Unmap: every successful map
            // is released before returning, including invalid mapping metadata.
            for line in 0..height as usize {
                let src = std::slice::from_raw_parts(
                    (mapped.pData as *const u8)
                        .add((y as usize + line) * mapped.RowPitch as usize + x as usize * 4),
                    row,
                );
                let dst = &mut out[40 + line * row..40 + (line + 1) * row];
                dst.copy_from_slice(src);
                // CF_DIB/BI_RGB ignores alpha. Flatten premultiplied pixels on
                // white and mark them opaque, matching the export replay clear.
                for pixel in dst.chunks_exact_mut(4) {
                    let white = 255 - pixel[3];
                    for channel in &mut pixel[..3] {
                        *channel = channel.saturating_add(white);
                    }
                    pixel[3] = 255;
                }
            }
            ctx.Unmap(&staging, 0);
            Ok(out)
        }
    }

    /// Real DirectWrite advances, shared by the renderer, caret, wrapping and hints.
    pub fn wrap_text(
        &mut self,
        text: &str,
        size: f32,
        width: f32,
    ) -> Vec<(usize, String, Vec<f32>)> {
        let mut lines = Vec::new();
        let mut base = 0;
        for paragraph in text.split('\n') {
            let chars: Vec<char> = paragraph.chars().collect();
            let all = self.text_positions(paragraph, size);
            let mut first = 0;
            while first < chars.len() {
                let mut end = first + 1;
                while end < chars.len()
                    && all.get(end + 1).copied().unwrap_or(0.) - all[first] <= width
                {
                    end += 1;
                }
                end = line_break_end(&chars, first, end);
                let s: String = chars[first..end].iter().collect();
                let positions = self.text_positions(&s, size);
                lines.push((base + first, s, positions));
                first = end;
            }
            if chars.is_empty() {
                lines.push((base, String::new(), vec![0.]));
            }
            base += chars.len() + 1;
        }
        lines
    }
    fn text_positions(&mut self, text: &str, size: f32) -> Vec<f32> {
        unsafe {
            let mut out = vec![0.];
            let Ok(fmt) = self.fonts.format(FontFamily::Lxgw, size) else {
                return out;
            };
            let ws: Vec<u16> = text.encode_utf16().collect();
            let Ok(layout) = self
                .fonts
                .dwrite
                .CreateTextLayout(&ws, &fmt, 1_000_000., size * 1.5)
            else {
                return out;
            };
            let mut offset = 0u32;
            for c in text.chars() {
                offset += c.len_utf16() as u32;
                let (mut x, mut y) = (0., 0.);
                let mut metrics =
                    windows::Win32::Graphics::DirectWrite::DWRITE_HIT_TEST_METRICS::default();
                let _ = layout.HitTestTextPosition(offset, false, &mut x, &mut y, &mut metrics);
                out.push(x);
            }
            out
        }
    }

    unsafe fn set_brush(&mut self, c: [f32; 4]) {
        let [r, g, b, a] = c;
        self.brush.SetColor(&D2D1_COLOR_F { r, g, b, a });
    }

    /// 指令消费（draw_frame / draw_sync_frame / replay_frame 共用）
    unsafe fn exec_cmds(&mut self, cmds: &[DrawCmd]) {
        self.exec_cmds_inner(cmds, true);
    }

    unsafe fn exec_cmds_inner(&mut self, cmds: &[DrawCmd], allow_blur: bool) {
        let mut clip_depth = 0usize;
        for (index, cmd) in cmds.iter().enumerate() {
            match cmd {
                DrawCmd::BlurBackdrop { radius } => {
                    if allow_blur
                        && clip_depth == 0
                        && !self.blur_unavailable
                        && radius.is_finite()
                        && *radius > 0.0
                    {
                        if let Err(error) = self.blur_backdrop(&cmds[..index], radius.min(32.0)) {
                            self.blur_unavailable = true;
                            eprintln!("[d2d] 背景模糊不可用，保留遮罩: {error:?}");
                        }
                    }
                }
                DrawCmd::Rect { x, y, w, h, color } => {
                    self.set_brush(*color);
                    let rect = D2D_RECT_F {
                        left: *x,
                        top: *y,
                        right: x + w,
                        bottom: y + h,
                    };
                    self.d2d_ctx.FillRectangle(&rect, &self.brush);
                }
                DrawCmd::RoundRect {
                    x,
                    y,
                    w,
                    h,
                    r,
                    color,
                    stroke,
                } => {
                    self.set_brush(*color);
                    let rr = D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: *x,
                            top: *y,
                            right: x + w,
                            bottom: y + h,
                        },
                        radiusX: *r,
                        radiusY: *r,
                    };
                    self.d2d_ctx.FillRoundedRectangle(&rr, &self.brush);
                    if let Some(sc) = stroke {
                        self.set_brush(*sc);
                        self.d2d_ctx
                            .DrawRoundedRectangle(&rr, &self.brush, 1.0, None);
                    }
                }
                DrawCmd::Line { x, y, w, h, color } => {
                    self.set_brush(*color);
                    // 用 1px 细矩形代替 DrawLine（无抗锯齿毛边，半透明分隔线干净）
                    let rect = D2D_RECT_F {
                        left: *x,
                        top: *y,
                        right: x + w,
                        bottom: y + h,
                    };
                    self.d2d_ctx.FillRectangle(&rect, &self.brush);
                }
                DrawCmd::PushClip { x, y, w, h } => {
                    clip_depth += 1;
                    let rect = D2D_RECT_F {
                        left: *x,
                        top: *y,
                        right: x + w,
                        bottom: y + h,
                    };
                    let _ = self
                        .d2d_ctx
                        .PushAxisAlignedClip(&rect, D2D1_ANTIALIAS_MODE_ALIASED);
                }
                DrawCmd::PopClip => {
                    clip_depth = clip_depth.saturating_sub(1);
                    self.d2d_ctx.PopAxisAlignedClip();
                }
                DrawCmd::Text {
                    x,
                    y,
                    w,
                    h,
                    ch,
                    color,
                } => {
                    self.set_brush(*color);
                    self.draw_char(*x, *y, *w, *h, *ch);
                }
                DrawCmd::TextRun {
                    x,
                    y,
                    h,
                    text,
                    color,
                    align,
                    font,
                } => {
                    self.set_brush(*color);
                    self.draw_run(*x, *y, *h, text, *align, *font);
                }
                DrawCmd::StyledLine {
                    x,
                    y,
                    w,
                    size,
                    text,
                    spans,
                    color,
                } => {
                    self.set_brush(*color);
                    if let Ok(fmt) = self.fonts.format(FontFamily::Lxgw, *size) {
                        let ws: Vec<u16> = text.encode_utf16().collect();
                        if let Ok(layout) =
                            self.fonts
                                .dwrite
                                .CreateTextLayout(&ws, &fmt, *w + 4.0, *size * 1.5)
                        {
                            let mut effects = Vec::new();
                            for (start, len, c) in spans {
                                if let Ok(b) = self.d2d_ctx.CreateSolidColorBrush(
                                    &D2D1_COLOR_F {
                                        r: c[0],
                                        g: c[1],
                                        b: c[2],
                                        a: c[3],
                                    },
                                    None,
                                ) {
                                    let _ = layout.SetDrawingEffect(
                                        &b,
                                        windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_RANGE {
                                            startPosition: *start,
                                            length: *len,
                                        },
                                    );
                                    effects.push(b);
                                }
                            }
                            self.d2d_ctx.DrawTextLayout(
                                windows::Win32::Graphics::Direct2D::Common::D2D_POINT_2F {
                                    x: *x,
                                    y: *y,
                                },
                                &layout,
                                &self.brush,
                                D2D1_DRAW_TEXT_OPTIONS_NONE,
                            );
                        }
                    }
                }
                DrawCmd::Glyphs { .. } => {
                    // Glyph run 直绘路径（DWrite 排版层接入后启用）
                }
            }
        }
    }

    /// Build and validate the complete effect on a separate context. Main-frame
    /// BeginDraw/target/clip state is never redirected to a command list, and an
    /// effect failure is observed before the main context consumes any image.
    unsafe fn blur_backdrop(
        &mut self,
        prefix: &[DrawCmd],
        radius: f32,
    ) -> windows::core::Result<()> {
        use windows::Foundation::Numerics::Matrix3x2;
        use windows::Win32::Graphics::Direct2D::Common::{
            D2D1_BORDER_MODE_HARD, D2D1_COMPOSITE_MODE_SOURCE_COPY, D2D_SIZE_U,
        };
        use windows::Win32::Graphics::Direct2D::{
            CLSID_D2D1GaussianBlur, D2D1_GAUSSIANBLUR_PROP_BORDER_MODE,
            D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION, D2D1_INTERPOLATION_MODE_LINEAR,
            D2D1_PROPERTY_TYPE_ENUM, D2D1_PROPERTY_TYPE_FLOAT,
        };
        let identity = Matrix3x2 {
            M11: 1.0,
            M22: 1.0,
            ..Default::default()
        };
        let mut transform = identity;
        let (mut dpi_x, mut dpi_y) = (96.0, 96.0);
        self.d2d_ctx.GetTransform(&mut transform);
        self.d2d_ctx.GetDpi(&mut dpi_x, &mut dpi_y);
        let context = self
            .d2d_ctx
            .GetDevice()?
            .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
        context.SetDpi(dpi_x, dpi_y);
        context.SetUnitMode(self.d2d_ctx.GetUnitMode());
        context.SetAntialiasMode(self.d2d_ctx.GetAntialiasMode());
        context.SetTextAntialiasMode(self.d2d_ctx.GetTextAntialiasMode());
        let list = context.CreateCommandList()?;
        let brush = context.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            },
            None,
        )?;
        context.SetTarget(&list);
        context.BeginDraw();
        context.SetTransform(&identity);
        let original_context = std::mem::replace(&mut self.d2d_ctx, context.clone());
        let original_brush = std::mem::replace(&mut self.brush, brush);
        // A second backdrop marker in a prefix cannot recurse or create a cycle.
        self.exec_cmds_inner(prefix, false);
        let recorded = context.EndDraw(None, None);
        context.SetTarget(None);
        self.d2d_ctx = original_context;
        self.brush = original_brush;
        recorded?;
        list.Close()?;

        let effect = context.CreateEffect(&CLSID_D2D1GaussianBlur)?;
        effect.SetValue(
            D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
            D2D1_PROPERTY_TYPE_FLOAT,
            &radius.to_ne_bytes(),
        )?;
        effect.SetValue(
            D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
            D2D1_PROPERTY_TYPE_ENUM,
            &D2D1_BORDER_MODE_HARD.0.to_ne_bytes(),
        )?;
        effect.SetInput(0, &list, true);
        let output = effect.GetOutput()?;
        let bitmap = context.CreateBitmap(
            D2D_SIZE_U {
                width: self.width.max(1.0) as u32,
                height: self.height.max(1.0) as u32,
            },
            None,
            0,
            &D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: dpi_x,
                dpiY: dpi_y,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                colorContext: std::mem::ManuallyDrop::new(None),
            },
        )?;
        context.SetTarget(&bitmap);
        context.SetTransform(&transform);
        context.BeginDraw();
        context.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));
        context.DrawImage(
            &output,
            None,
            None,
            D2D1_INTERPOLATION_MODE_LINEAR,
            D2D1_COMPOSITE_MODE_SOURCE_COPY,
        );
        let rendered = context.EndDraw(None, None);
        context.SetTarget(None);
        rendered?;
        // The offscreen bitmap already includes the caller's DPI/resize transform.
        // Copy once with identity to avoid applying that transform a second time.
        self.d2d_ctx.SetTransform(&identity);
        self.d2d_ctx.DrawImage(
            &bitmap,
            None,
            None,
            D2D1_INTERPOLATION_MODE_LINEAR,
            D2D1_COMPOSITE_MODE_SOURCE_COPY,
        );
        self.d2d_ctx.SetTransform(&transform);
        Ok(())
    }

    /// 单字绘制（跟打正文）：字号=fs，字符盒居中，LXGW 族。
    /// x/y 是目标盒左上角；DWrite 基线制 → 以行高 h 推基线（居中视觉）。
    unsafe fn draw_char(&mut self, x: f32, y: f32, w: f32, h: f32, ch: char) {
        let fmt = match self.fonts.format(FontFamily::Lxgw, h) {
            Ok(f) => f,
            Err(_) => return,
        };
        let s: Vec<u16> = utf16_of(ch);
        // 字符格内按实际 advance 居中：格宽 w，字形宽 char_width(w)，
        // 左缘 = x + (w - adv)/2。CJK adv≈w（几乎顶格），ASCII adv≈0.5w
        // （居中后两侧对称，不再贴左缘挤成一团）。
        let adv = self.fonts.char_width(FontFamily::Lxgw, h, ch);
        let gx = x + ((w - adv) / 2.0).max(0.0);
        let rect = D2D_RECT_F {
            left: gx,
            top: y,
            right: gx + adv + 8.0,
            bottom: y + h * 1.4,
        };
        let _ = self.d2d_ctx.DrawText(
            &s,
            &fmt,
            &rect,
            &self.brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
    }

    /// 行内文本（UI 层）：UI 族 + 对齐语义。
    /// x 语义 = Left:左缘 / Center:中心 / Right:右缘；y 为行盒顶。
    unsafe fn draw_run(
        &mut self,
        x: f32,
        y: f32,
        h: f32,
        text: &str,
        align: Align,
        font: FontFamily,
    ) {
        let fmt = match self.fonts.format(font, h) {
            Ok(f) => f,
            Err(_) => return,
        };
        let _ = fmt.SetTextAlignment(match align {
            Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
            Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
        });
        let ws: Vec<u16> = text.encode_utf16().collect();
        // 锚点语义：Left=x 左缘 / Center=x 中心 / Right=x 右缘（NO_WRAP 不折行）
        let (l, r) = match align {
            Align::Left => (x, x + 4096.0),
            Align::Center => (x - 2048.0, x + 2048.0),
            Align::Right => (x - 4096.0, x),
        };
        let rect = D2D_RECT_F {
            left: l,
            top: y,
            right: r,
            bottom: y + h * 1.4,
        };
        let _ = self.d2d_ctx.DrawText(
            &ws,
            &fmt,
            &rect,
            &self.brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
        // format 是缓存共享对象，用完恢复默认（避免污染缓存键）
        let _ = fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// rewrap_words：跨行词整体下移、单字词快路径、位置向量按原推进量重建。
    /// CJK 全宽假设：每字推进 10.0（真实路径来自 DirectWrite，此处只用等差近似验证算法）。
    fn line(base: usize, text: &str) -> (usize, String, Vec<f32>) {
        let n = text.chars().count();
        (
            base,
            text.to_string(),
            (0..=n).map(|i| i as f32 * 10.).collect(),
        )
    }

    #[test]
    fn rewrap_moves_straddling_word_whole_and_rebuilds_positions() {
        // 行0 "XYAB" + 行1 "CD"；词 (2,4)="ABCD" 跨行 → ABCD 整词下移
        let lines = vec![line(0, "XYAB"), line(4, "CD")];
        let out = rewrap_words(&lines, &[(0, 2), (2, 4)]).expect("有跨行词，应返回重排结果");
        assert_eq!(out[0].1, "XY");
        assert_eq!(out[0].0, 0);
        assert_eq!(out[0].2, vec![0., 10., 20.]);
        assert_eq!(out[1].1, "ABCD");
        assert_eq!(out[1].0, 2, "行1 的 base 前移到词首");
        assert_eq!(out[1].2, vec![0., 10., 20., 30., 40.]);
    }

    #[test]
    fn rewrap_splits_a_phrase_wider_than_the_available_line() {
        let lines = vec![line(0, "ABCD"), line(4, "EFGHI"), line(9, "J")];
        let out = rewrap_words_to_width(&lines, &[(2, 6)], 50.).unwrap();
        assert_eq!(
            out.iter().map(|l| l.1.as_str()).collect::<Vec<_>>(),
            ["ABCDE", "FGHIJ"]
        );
        assert!(out.iter().all(|l| l.2.last().unwrap() <= &50.));
        assert_eq!(
            out.iter().map(|l| l.1.chars().count()).sum::<usize>(),
            10,
            "字符守恒"
        );
        for w in out.windows(2) {
            assert_eq!(
                w[1].0 - w[0].0,
                w[0].1.chars().count(),
                "相邻行 base 必须连续"
            );
        }
    }

    #[test]
    fn rewrap_appends_a_final_line_instead_of_clipping_its_overflow() {
        let lines = vec![line(0, "ABCDE"), line(5, "FGHIJ")];
        let words = [(4, 2)];
        let out = rewrap_words_to_width(&lines, &words, 50.).unwrap();
        assert_eq!(
            out.iter().map(|l| l.1.as_str()).collect::<Vec<_>>(),
            ["ABCD", "EFGHI", "J"]
        );
        assert_eq!(out.iter().map(|l| l.0).collect::<Vec<_>>(), [0, 4, 9]);
        assert!(out.iter().all(|l| l.2.last().unwrap() <= &50.));
        assert_eq!(
            out.iter().map(|l| l.1.as_str()).collect::<String>(),
            "ABCDEFGHIJ"
        );
        assert!(rewrap_words_to_width(&out, &words, 50.).is_none());
    }

    #[test]
    fn rewrap_preserves_unicode_advances_and_explicit_paragraph_gaps() {
        let lines = vec![
            (0, "甲𠮷".into(), vec![0., 12., 19.]),
            (2, "乙丙".into(), vec![0., 8., 19.]),
            (5, "丁".into(), vec![0., 13.]),
        ];
        let out = rewrap_words_to_width(&lines, &[(1, 2)], 20.).unwrap();
        assert_eq!(
            out.iter().map(|l| (l.0, l.1.as_str())).collect::<Vec<_>>(),
            [(0, "甲"), (1, "𠮷乙"), (3, "丙"), (5, "丁")]
        );
        assert_eq!(out[1].2, [0., 7., 15.]);
        assert!(out.iter().all(|l| l.2.last().unwrap() <= &20.));
        let blank_lines = vec![line(0, "甲乙"), line(3, ""), line(4, "丙丁")];
        let out = rewrap_words_to_width(&blank_lines, &[], 10.).unwrap();
        assert_eq!(
            out.iter().map(|l| (l.0, l.1.as_str())).collect::<Vec<_>>(),
            [(0, "甲"), (1, "乙"), (3, ""), (4, "丙"), (5, "丁")]
        );
    }

    #[test]
    fn rewrap_does_not_split_fitting_word_to_enforce_closing_punctuation_rule() {
        let lines = vec![line(0, "甲"), line(1, "乙，")];
        let out = rewrap_words_to_width(&lines, &[(0, 2)], 20.).unwrap();
        assert_eq!(
            out.iter().map(|l| l.1.as_str()).collect::<Vec<_>>(),
            ["甲乙", "，"]
        );
    }

    #[test]
    fn rewrap_single_char_words_take_fast_path() {
        let lines = vec![line(0, "AB"), line(2, "CD")];
        assert!(rewrap_words(&lines, &[(0, 1), (1, 1), (2, 1), (3, 1)]).is_none());
        // 没有跨行词时也走快路径
        let lines2 = vec![line(0, "ABCD"), line(4, "EF")];
        assert!(rewrap_words(&lines2, &[(0, 2), (2, 2), (4, 2)]).is_none());
    }

    #[test]
    #[ignore = "Native D3D/D2D readback fixture; requires a Windows desktop"]
    fn native_dib_crop_is_top_down_and_opaque() {
        let _com = crate::window::ComSta::new().unwrap();
        crate::window::enable_per_monitor_dpi_awareness();
        let window = crate::window::MainWindow::create("DIB crop fixture", 64, 48).unwrap();
        let mut renderer = Renderer::new(window.hwnd, 64, 48).unwrap();
        let scale = crate::window::dpi_scale();
        let mut frame = Frame::new(64.0 / scale, 48.0 / scale);
        frame.push(DrawCmd::Rect {
            x: 0.0,
            y: 0.0,
            w: frame.size.w,
            h: 24.0 / scale,
            color: [1.0, 0.0, 0.0, 1.0],
        });
        frame.push(DrawCmd::Rect {
            x: 0.0,
            y: 24.0 / scale,
            w: frame.size.w,
            h: 24.0 / scale,
            color: [0.0, 0.0, 1.0, 0.5],
        });
        renderer.draw_frame(&frame).unwrap();
        renderer.render_export_frame().unwrap();
        let dib = renderer.read_target_dib(Some((5, 20, 7, 8))).unwrap();
        assert_eq!(dib.len(), 40 + 7 * 8 * 4);
        assert_eq!(i32::from_le_bytes(dib[4..8].try_into().unwrap()), 7);
        assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), -8);
        assert_eq!(&dib[40..44], &[0, 0, 255, 255]);
        let bottom = &dib[dib.len() - 4..];
        assert_eq!(bottom[0], 255);
        assert!((126..=129).contains(&bottom[1]));
        assert!((126..=129).contains(&bottom[2]));
        assert_eq!(bottom[3], 255);
        assert!(renderer.read_target_dib(Some((63, 0, 2, 1))).is_err());
        assert!(renderer.read_target_dib(Some((0, 0, 0, 1))).is_err());
        assert!(renderer.read_target_dib(Some((u32::MAX, 0, 2, 1))).is_err());
    }

    #[test]
    #[ignore = "Native D3D/D2D fixture; writes images for visual inspection"]
    fn backdrop_blur_exports_native_fixture_and_survives_resize() {
        use windows::Foundation::Numerics::Matrix3x2;
        let _com = crate::window::ComSta::new().unwrap();
        crate::window::enable_per_monitor_dpi_awareness();
        let window = crate::window::MainWindow::create("Backdrop blur fixture", 960, 640).unwrap();
        let mut renderer = Renderer::new(window.hwnd, 960, 640).unwrap();
        let directory = std::env::var_os("HUFU_BLUR_FIXTURE_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("hufu-blur-fixture"));
        std::fs::create_dir_all(&directory).unwrap();
        let scale = crate::window::dpi_scale();
        let mut frame = Frame::new(960.0 / scale, 640.0 / scale);
        frame.push(DrawCmd::Rect {
            x: 0.0,
            y: 0.0,
            w: frame.size.w,
            h: frame.size.h,
            color: [0.93, 0.91, 0.85, 1.0],
        });
        for x in 0..80 {
            frame.push(DrawCmd::Rect {
                x: x as f32 * 12.0,
                y: 0.0,
                w: 6.0,
                h: frame.size.h,
                color: [0.13, 0.17, 0.2, 1.0],
            });
        }
        frame.push(DrawCmd::TextRun {
            x: 32.0,
            y: 70.0,
            h: 38.0,
            text: "岳阳楼记 · 背景正文".into(),
            color: [0.98, 0.76, 0.31, 1.0],
            align: Align::Left,
            font: FontFamily::Lxgw,
        });
        let backdrop_end = frame.cmds.len();
        frame.push(DrawCmd::Rect {
            x: 0.0,
            y: 0.0,
            w: frame.size.w,
            h: frame.size.h,
            color: [0.0, 0.0, 0.0, 0.24],
        });
        frame.push(DrawCmd::RoundRect {
            x: 200.0,
            y: 160.0,
            w: 280.0,
            h: 170.0,
            r: 16.0,
            color: [0.15, 0.18, 0.21, 1.0],
            stroke: None,
        });
        frame.push(DrawCmd::TextRun {
            x: 222.0,
            y: 182.0,
            h: 23.0,
            text: "清晰的模态面板".into(),
            color: [0.96, 0.94, 0.86, 1.0],
            align: Align::Left,
            font: FontFamily::Ui,
        });
        renderer.draw_frame(&frame).unwrap();
        let sharp_path = directory.join("01-modal-without-blur.bmp");
        renderer.save_bmp(&sharp_path).unwrap();
        let sharp = std::fs::read(&sharp_path).unwrap();
        frame
            .cmds
            .insert(backdrop_end, DrawCmd::BlurBackdrop { radius: 7.0 });
        renderer.draw_frame(&frame).unwrap();
        assert!(
            !renderer.blur_unavailable,
            "the native blur must succeed on this fixture device"
        );
        let blurred_path = directory.join("02-modal-blur-7px.bmp");
        renderer.save_bmp(&blurred_path).unwrap();
        let blurred = std::fs::read(&blurred_path).unwrap();
        // Compare stripe edge contrast away from the opaque panel. Dim is equal
        // in both frames, so only the actual blur can lower this energy.
        let energy = |pixels: &[u8]| -> u64 {
            (20..150)
                .flat_map(|y| (20..150).map(move |x| (x, y)))
                .map(|(x, y)| {
                    let offset = 54 + (y * 960 + x) * 4;
                    pixels[offset].abs_diff(pixels[offset - 4]) as u64
                })
                .sum()
        };
        assert!(
            energy(&blurred) * 3 < energy(&sharp),
            "background must lose sharp stripe edges"
        );
        let panel_pixel = 54 + ((260.0 * scale) as usize * 960 + (260.0 * scale) as usize) * 4;
        assert_eq!(
            &sharp[panel_pixel..panel_pixel + 4],
            &blurred[panel_pixel..panel_pixel + 4],
            "panel must stay sharp and opaque"
        );

        renderer.on_window_size_sync_fill(1200, 800);
        // Present flips the target; replay once into the current back buffer so
        // readback inspects the actual commands, rather than the next blank one.
        unsafe {
            renderer.d2d_ctx.BeginDraw();
            renderer.d2d_ctx.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            renderer.replay_frame(1200.0 / frame.size.w, 800.0 / frame.size.h);
            renderer.d2d_ctx.EndDraw(None, None).unwrap();
        }
        renderer
            .write_target_bmp(&directory.join("03-resize-replay.bmp"))
            .unwrap();
        assert!(!renderer.blur_unavailable);
        renderer.draw_sync_frame(960, 640, &frame);
        renderer
            .save_bmp(&directory.join("04-sync-redraw.bmp"))
            .unwrap();
        assert!(!renderer.blur_unavailable);
        // The same logical scene at 150% uses the caller's transform exactly
        // once; record/export without rerendering through the global DPI value.
        renderer.resize(1440, 960).unwrap();
        unsafe {
            let transform = Matrix3x2 {
                M11: 1.5,
                M22: 1.5,
                ..Default::default()
            };
            renderer.d2d_ctx.SetTransform(&transform);
            renderer.d2d_ctx.BeginDraw();
            renderer.d2d_ctx.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            renderer.exec_cmds(&frame.cmds);
            renderer.d2d_ctx.EndDraw(None, None).unwrap();
            let mut restored = Matrix3x2::default();
            renderer.d2d_ctx.GetTransform(&mut restored);
            assert_eq!(restored.M11, 1.5);
            assert_eq!(restored.M22, 1.5);
        }
        renderer
            .write_target_bmp(&directory.join("05-transform-150-percent.bmp"))
            .unwrap();
        assert!(!renderer.blur_unavailable);
        renderer.resize(960, 640).unwrap();
        renderer.blur_unavailable = true;
        renderer.draw_frame(&frame).unwrap();
        let fallback_path = directory.join("06-fallback-dim-only.bmp");
        renderer.save_bmp(&fallback_path).unwrap();
        assert_eq!(
            std::fs::read(fallback_path).unwrap(),
            sharp,
            "unavailable blur must preserve the ordinary dim-only scene"
        );
        println!("Native blur fixture exported to {}", directory.display());
    }

    #[test]
    fn chinese_wrap_keeps_closing_punctuation_with_its_character() {
        let chars: Vec<char> = "迁客骚人，多会于此。".chars().collect();
        assert_eq!(line_break_end(&chars, 0, 4), 3);
        let open: Vec<char> = "他问（你好）".chars().collect();
        assert_eq!(line_break_end(&open, 0, 3), 2);
        assert_eq!(line_break_end(&chars, 0, 1), 1);
    }

    #[test]
    fn cjk_family_resolves_without_bundled_assets() {
        // 字体已去嵌入：家族由系统探测解析（候选表或雅黑回退），无外部资产依赖
        let family = detected_cjk_family();
        assert!(!family.is_empty(), "探测结果不得为空");
    }

    #[test]
    fn frame_build() {
        let mut f = Frame::new(800.0, 600.0);
        f.push(DrawCmd::Rect {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
            color: [0.0, 0.0, 0.0, 1.0],
        });
        f.push(DrawCmd::RoundRect {
            x: 10.0,
            y: 10.0,
            w: 100.0,
            h: 40.0,
            r: 6.0,
            color: [1.0; 4],
            stroke: None,
        });
        assert_eq!(f.cmds.len(), 2);
        assert_eq!(f.size.w, 800.0);
    }

    #[test]
    fn frame_text_cmds() {
        let mut f = Frame::new(400.0, 300.0);
        f.push(DrawCmd::Text {
            x: 0.0,
            y: 0.0,
            w: 32.0,
            h: 32.0,
            ch: '疾',
            color: [1.0; 4],
        });
        f.push(DrawCmd::TextRun {
            x: 0.0,
            y: 40.0,
            h: 20.0,
            text: "组字串".into(),
            color: [0.9, 0.9, 0.9, 1.0],
            align: Align::Left,
            font: FontFamily::Ui,
        });
        assert_eq!(f.cmds.len(), 2);
    }

    #[test]
    fn clip_and_line_cmds() {
        let mut f = Frame::new(400.0, 300.0);
        f.push(DrawCmd::PushClip {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 100.0,
        });
        f.push(DrawCmd::Line {
            x: 0.0,
            y: 100.0,
            w: 400.0,
            h: 1.0,
            color: [1.0; 4],
        });
        f.push(DrawCmd::PopClip);
        assert!(matches!(f.cmds[0], DrawCmd::PushClip { .. }));
        assert!(matches!(f.cmds[2], DrawCmd::PopClip));
    }

    #[test]
    fn font_kit_measures_cjk_wider_than_ascii() {
        // 无窗口环境可跑：FontKit 只依赖 DWrite
        let mut kit = FontKit::new();
        let cjk = kit.char_width(FontFamily::Lxgw, 64.0, '跟');
        let ascii = kit.char_width(FontFamily::Lxgw, 64.0, 'a');
        assert!(cjk > ascii, "cjk={cjk} ascii={ascii}");
        assert!(cjk > 50.0 && cjk < 80.0, "64px 中文应≈64 宽，got {cjk}");
    }

    #[test]
    fn cjk_family_detection_lands_on_candidate_or_fallback() {
        // 系统字体探测（真实 DWrite 调用；无头环境可用）：结果必为
        // 候选表之一（本机装了霞鹜文楷）或微软雅黑回退，且进程内缓存一致。
        let family = detected_cjk_family();
        let known = CJK_FAMILY_CANDIDATES.contains(&family) || family == CJK_FAMILY_FALLBACK;
        assert!(known, "探测结果不在候选表/回退内: {family}");
        assert_eq!(detected_cjk_family(), family, "探测应进程内缓存");
        // 探测结果必须真的能建 TextFormat（族名有效）
        let mut kit = FontKit::new();
        assert!(kit.format(FontFamily::Lxgw, 24.0).is_ok());
    }

    #[test]
    fn font_kit_width_cache_stable() {
        let mut kit = FontKit::new();
        let a = kit.char_width(FontFamily::Ui, 13.0, '码');
        let b = kit.char_width(FontFamily::Ui, 13.0, '码');
        assert_eq!(a, b);
    }

    #[test]
    fn custom_font_changes_layout_metrics_and_missing_font_falls_back() {
        let format_family = |kit: &mut FontKit| {
            let fmt = kit.format(FontFamily::Lxgw, 48.).unwrap();
            let len = unsafe { fmt.GetFontFamilyNameLength() } as usize;
            let mut name = vec![0u16; len + 1];
            unsafe { fmt.GetFontFamilyName(&mut name).unwrap() };
            String::from_utf16(&name[..len]).unwrap()
        };
        let mut kit = FontKit::new();
        kit.set_body_family("Consolas");
        assert_eq!(kit.body_family, "Consolas");
        assert_eq!(format_family(&mut kit), "Consolas");
        let before = kit.char_width(FontFamily::Lxgw, 48., 'W');
        assert!(!kit.width_cache.is_empty());
        assert!(kit.set_body_family("Arial"));
        assert!(kit.width_cache.is_empty());
        assert!(kit.fmt_cache.is_empty());
        assert_eq!(format_family(&mut kit), "Arial");
        let after = kit.char_width(FontFamily::Lxgw, 48., 'W');
        assert!((before - after).abs() > 1.);
        kit.set_body_family("__hufu_uninstalled_font__");
        assert_eq!(kit.body_family, detected_cjk_family());
        assert!(kit.format(FontFamily::Lxgw, 100.).is_ok());
    }
}
