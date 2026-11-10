//! Windows 已安装字体选择器。字号由跟打器设置管理，避免点数与 DIP 混用。
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{DEFAULT_CHARSET, LOGFONTW};
use windows::Win32::UI::Controls::Dialogs::{
    ChooseFontW, CommDlgExtendedError, CF_FORCEFONTEXIST, CF_INITTOLOGFONTSTRUCT, CF_NOSCRIPTSEL,
    CF_NOSIZESEL, CF_NOSTYLESEL, CF_NOVERTFONTS, CF_SCALABLEONLY, CF_SCREENFONTS, CHOOSEFONTW,
};

/// UI 线程调用；取消返回 Ok(None)，失败返回明确错误。
pub fn choose_font(owner: isize, current: &str) -> Result<Option<String>, String> {
    let mut font = LOGFONTW {
        lfCharSet: DEFAULT_CHARSET,
        ..Default::default()
    };
    for (slot, unit) in font
        .lfFaceName
        .iter_mut()
        .take(31)
        .zip(current.encode_utf16())
    {
        *slot = unit;
    }
    let mut dialog = CHOOSEFONTW {
        lStructSize: std::mem::size_of::<CHOOSEFONTW>() as u32,
        hwndOwner: HWND(owner as *mut _),
        lpLogFont: &mut font,
        Flags: CF_SCREENFONTS
            | CF_FORCEFONTEXIST
            | CF_INITTOLOGFONTSTRUCT
            | CF_NOSCRIPTSEL
            | CF_NOSIZESEL
            | CF_NOSTYLESEL
            | CF_NOVERTFONTS
            | CF_SCALABLEONLY,
        ..Default::default()
    };
    unsafe {
        if ChooseFontW(&mut dialog).as_bool() {
            let end = font.lfFaceName.iter().position(|&c| c == 0).unwrap_or(32);
            let family = String::from_utf16_lossy(&font.lfFaceName[..end]);
            // GDI 与 DirectWrite 的系统字体集合并非完全相同；不把未能
            // 用于正文的族名保存成一次看似成功、实际立即回退的选择。
            if !family.is_empty() && crate::d2d::resolved_cjk_family(&family) == family {
                Ok(Some(family))
            } else {
                Err("所选字体暂不可用，请选择另一款系统字体。".into())
            }
        } else {
            let error = CommDlgExtendedError().0;
            if error == 0 {
                Ok(None)
            } else {
                Err(format!("无法打开系统字体选择器（{error:#x}）"))
            }
        }
    }
}
