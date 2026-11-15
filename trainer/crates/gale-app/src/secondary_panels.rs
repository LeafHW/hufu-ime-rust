//! Result, race report, online library and cleaning workbench from the Quiet Room design.
//!
//! These views render actual session/account data. Historical comparisons and character
//! timings are shown only when their provenance is available; design sample values are
//! deliberately never used as application data.
use crate::{
    modals::{Action, FieldSpec, Hit, Modal, Model},
    session::Session,
    ui::*,
};
use gale_core::{config::Config, theme::Theme};
use gale_win::d2d::{DrawCmd, Frame};

type Ink = [f32; 4];

// Secondary windows use a 15 DIP floor: 14 DIP CJK read too small on the
// 960/1040 wide panels, and every view here is supporting text or controls.
fn text(f: &mut Frame, x: f32, y: f32, size: f32, s: impl Into<String>, c: Ink) {
    crate::ui::text(f, x, y, size.max(15.), s, c);
}
fn mono(f: &mut Frame, x: f32, y: f32, size: f32, s: impl Into<String>, c: Ink) {
    crate::ui::mono(f, x, y, size.max(15.), s, c);
}
fn body_text(f: &mut Frame, x: f32, y: f32, size: f32, s: impl Into<String>, c: Ink) {
    crate::ui::body_text(f, x, y, size.max(15.), s, c);
}
fn right(f: &mut Frame, x: f32, y: f32, size: f32, s: impl Into<String>, c: Ink) {
    crate::ui::right(f, x, y, size.max(15.), s, c);
}

fn card(f: &mut Frame, x: f32, y: f32, w: f32, h: f32, t: &Theme) {
    let p = tokens(t);
    round(f, x, y, w, h, 9., p.surface2, Some(p.line));
}

fn clip(f: &mut Frame, x: f32, y: f32, w: f32, h: f32) {
    f.push(DrawCmd::PushClip {
        x,
        y,
        w: w.max(0.),
        h: h.max(0.),
    });
}

fn end_clip(f: &mut Frame) {
    f.push(DrawCmd::PopClip);
}

fn clipped_text(f: &mut Frame, x: f32, y: f32, w: f32, size: f32, s: &str, c: Ink) {
    clip(f, x, y, w, size * 1.7);
    text(f, x, y, size, s, c);
    end_clip(f);
}

fn keycap(f: &mut Frame, x: f32, y: f32, s: &str, t: &Theme) {
    let p = tokens(t);
    round(f, x, y, 36., 28., 4., p.surface, Some(p.line2));
    mono(f, x + 7., y + 4., 14., s, p.muted);
}

fn action(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    s: &str,
    a: Action,
    t: &Theme,
    primary: bool,
) {
    let p = tokens(t);
    round(
        f,
        x,
        y,
        w,
        36.,
        8.,
        if primary { p.accent } else { p.surface },
        Some(if primary {
            alpha(p.accent, 0.8)
        } else {
            p.line2
        }),
    );
    clipped_text(
        f,
        x + 12.,
        y + 8.,
        w - 20.,
        14.,
        s,
        if primary { p.bg } else { p.text2 },
    );
    if a != Action::None {
        hs.push(Hit {
            x,
            y,
            w,
            h: 36.,
            action: a,
        });
    }
}

fn link(f: &mut Frame, hs: &mut Vec<Hit>, x: f32, y: f32, w: f32, s: &str, a: Action, t: &Theme) {
    let p = tokens(t);
    clipped_text(f, x, y + 8., w, 14., s, p.accent);
    if a != Action::None {
        hs.push(Hit {
            x,
            y,
            w,
            h: 36.,
            action: a,
        });
    }
}

fn chip(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    x: f32,
    y: f32,
    w: f32,
    s: &str,
    selected: bool,
    a: Action,
    t: &Theme,
) {
    let p = tokens(t);
    round(
        f,
        x,
        y,
        w,
        36.,
        5.,
        if selected {
            alpha(p.accent, 0.12)
        } else {
            p.surface
        },
        Some(if selected {
            alpha(p.accent, 0.36)
        } else {
            p.line
        }),
    );
    clipped_text(
        f,
        x + 7.,
        y + 8.,
        w - 12.,
        14.,
        s,
        if selected { p.accent } else { p.muted },
    );
    if a != Action::None {
        hs.push(Hit {
            x,
            y,
            w,
            h: 36.,
            action: a,
        });
    }
}

fn toggle(f: &mut Frame, hs: &mut Vec<Hit>, x: f32, y: f32, on: bool, a: Action, t: &Theme) {
    let p = tokens(t);
    round(
        f,
        x,
        y,
        26.,
        15.,
        7.5,
        if on { p.accent } else { p.line2 },
        None,
    );
    round(
        f,
        x + if on { 14. } else { 3. },
        y + 3.,
        9.,
        9.,
        4.5,
        if on { p.bg } else { p.muted },
        None,
    );
    if a != Action::None {
        hs.push(Hit {
            x: x - 3.,
            y: y - 9.,
            w: 32.,
            h: 36.,
            action: a,
        });
    }
}

/// Draw a short static segment using small filled pixels. DrawCmd::Line is an
/// axis-aligned divider, so it must not be used for the sparkline's sloping edges.
fn stroke(f: &mut Frame, from: (f32, f32), to: (f32, f32), c: Ink) {
    let n = ((to.0 - from.0).abs().max((to.1 - from.1).abs()) / 1.25)
        .ceil()
        .max(1.)
        .min(800.) as usize;
    for i in 0..=n {
        let z = i as f32 / n as f32;
        rect(
            f,
            from.0 + (to.0 - from.0) * z,
            from.1 + (to.1 - from.1) * z,
            1.4,
            1.4,
            c,
        );
    }
}

fn wrap_lines(s: &str, width: f32, size: f32) -> Vec<String> {
    let limit = (width / size).max(1.);
    let mut out = Vec::new();
    for paragraph in s.split('\n') {
        let mut line = String::new();
        let mut used = 0.;
        for ch in paragraph.chars() {
            let cw = if ch.is_ascii() { 0.57 } else { 1. };
            if used + cw > limit && !line.is_empty() {
                out.push(std::mem::take(&mut line));
                used = 0.;
            }
            line.push(ch);
            used += cw;
        }
        out.push(line);
    }
    out
}

fn paragraph(f: &mut Frame, x: f32, y: f32, w: f32, h: f32, size: f32, lh: f32, s: &str, c: Ink) {
    clip(f, x, y, w, h);
    let rows = (h / lh).ceil().max(0.) as usize;
    for (i, line) in wrap_lines(s, w, size).into_iter().take(rows).enumerate() {
        body_text(f, x, y + i as f32 * lh, size, line, c);
    }
    end_clip(f);
}

fn overlay_header(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    t: &Theme,
    x: f32,
    y: f32,
    pw: f32,
    icon: &str,
    title: &str,
    sub: &str,
    key: &str,
) {
    let p = tokens(t);
    line(f, x + 16., y + 62., pw - 32., p.line);
    text(f, x + 22., y + 17., 23., icon, p.accent);
    text(f, x + 62., y + 10., 20., title, p.text);
    clipped_text(f, x + 62., y + 36., pw - 198., 14., sub, p.muted);
    keycap(f, x + pw - 102., y + 17., key, t);
    text(f, x + pw - 49., y + 13., 25., "×", p.muted);
    hs.push(Hit {
        x: x + pw - 58.,
        y: y + 10.,
        w: 42.,
        h: 42.,
        action: Action::Close,
    });
}

fn login_visible(m: &Model) -> bool {
    (!m.online_authenticated || m.online_expired) && m.online_login_expanded
}

const ONLINE_FORM_TOP: f32 = 124.;
const ONLINE_FORM_ROW: f32 = 44.;
const ONLINE_FORM_LABEL: f32 = 88.;
const ONLINE_CHARS_Y: f32 = 230.;

fn online_form(pw: f32) -> (f32, f32) {
    let w = (pw - 40.).min(620.).max(220.);
    ((pw - w) / 2., w)
}

fn field_shell(f: &mut Frame, x: f32, y: f32, w: f32, label: &str, t: &Theme) {
    let p = tokens(t);
    text(f, x, y - 21., 15., label, p.muted);
    round(f, x - 1., y - 1., w + 2., 40., 7., p.bg2, Some(p.line2));
}

pub(crate) fn content_height_online(m: &Model) -> f32 {
    if login_visible(m) {
        if m.online_register_mode {
            480.
        } else {
            420.
        }
    } else if m.online_authenticated && !m.online_expired {
        440.
    } else {
        280.
    }
}

fn online_status_strip(f: &mut Frame, x: f32, y: f32, w: f32, value: &str, t: &Theme, bad: bool) {
    let p = tokens(t);
    let color = if bad { p.red } else { p.muted };
    round(
        f,
        x,
        y,
        w,
        34.,
        6.,
        alpha(color, 0.07),
        Some(alpha(color, 0.28)),
    );
    rect(f, x, y + 7., 3., 20., color);
    clipped_text(f, x + 12., y + 8., w - 24., 15., value, color);
}

fn online_field(f: &mut Frame, x: f32, y: f32, w: f32, label: &str, t: &Theme) {
    let p = tokens(t);
    text(f, x, y + 10., 15., label, p.muted);
    round(
        f,
        x + ONLINE_FORM_LABEL - 1.,
        y - 1.,
        w - ONLINE_FORM_LABEL + 2.,
        40.,
        7.,
        p.bg2,
        Some(p.line2),
    );
}

fn draw_online(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    cfg: &Config,
    t: &Theme,
    x: f32,
    y: f32,
    pw: f32,
    ph: f32,
) {
    let p = tokens(t);
    overlay_header(
        f,
        hs,
        t,
        x,
        y,
        pw,
        "◎",
        "在线发文",
        "按分类、难度与字数取文",
        // 【审计 I04】弹层快捷键跟用户配置，不再写死 F8。
        &crate::modals::hint_key(cfg, Some("online"), "F8"),
    );
    let (fx, fw) = online_form(pw);
    let ix = x + fx;
    let active = m.online_authenticated && !m.online_expired;
    if login_visible(m) {
        for (i, (label, register)) in [("登录", false), ("注册账号", true)]
            .into_iter()
            .enumerate()
        {
            chip(
                f,
                hs,
                ix + i as f32 * 124.,
                y + 76.,
                116.,
                label,
                m.online_register_mode == register,
                if m.online_register_mode == register {
                    Action::None
                } else {
                    crate::aux_marker("online_register")
                },
                t,
            );
        }
        link(
            f,
            hs,
            ix + fw - 92.,
            y + 76.,
            92.,
            "收起账号",
            Action::OnlineAccount,
            t,
        );
        online_field(f, ix, y + ONLINE_FORM_TOP, fw, "用户名", t);
        online_field(f, ix, y + ONLINE_FORM_TOP + ONLINE_FORM_ROW, fw, "密码", t);
        if m.online_register_mode {
            online_field(
                f,
                ix,
                y + ONLINE_FORM_TOP + 2. * ONLINE_FORM_ROW,
                fw,
                "邮箱",
                t,
            );
            online_field(
                f,
                ix,
                y + ONLINE_FORM_TOP + 3. * ONLINE_FORM_ROW,
                fw - 140.,
                "验证码",
                t,
            );
            let code_y = y + ONLINE_FORM_TOP + 3. * ONLINE_FORM_ROW;
            let label = if m.online_code_wait > 0 {
                format!("重发 {} 秒", m.online_code_wait)
            } else {
                "发送验证码".into()
            };
            action(
                f,
                hs,
                ix + fw - 130.,
                code_y,
                130.,
                &label,
                if m.online_code_wait > 0 {
                    Action::None
                } else {
                    crate::aux_marker("online_reg_code")
                },
                t,
                false,
            );
            clipped_text(
                f,
                ix,
                y + 304.,
                fw,
                15.,
                "密码不保存；验证码每 60 秒可获取一次。",
                p.muted,
            );
            action(
                f,
                hs,
                ix,
                y + 330.,
                160.,
                "注册并登录",
                crate::aux_marker("online_reg_submit"),
                t,
                true,
            );
            link(
                f,
                hs,
                ix + 176.,
                y + 330.,
                140.,
                "‹ 返回登录",
                crate::aux_marker("online_register"),
                t,
            );
        } else {
            online_field(
                f,
                ix,
                y + ONLINE_FORM_TOP + 2. * ONLINE_FORM_ROW,
                fw,
                "服务地址",
                t,
            );
            action(
                f,
                hs,
                ix,
                y + 266.,
                148.,
                "登录",
                Action::OnlineLogin,
                t,
                true,
            );
            clipped_text(
                f,
                ix + 164.,
                y + 276.,
                fw - 164.,
                15.,
                "HTTPS · 密码不保存",
                p.muted,
            );
        }
        if !m.online_status.is_empty() {
            let bad = ["失败", "错误", "频繁", "过期", "无效", "不"]
                .iter()
                .any(|word| m.online_status.contains(word));
            online_status_strip(
                f,
                ix,
                y + if m.online_register_mode { 378. } else { 314. },
                fw,
                &m.online_status,
                t,
                bad,
            );
        }
    } else if active {
        // One compact account row; all article controls remain visible together.
        card(f, ix, y + 74., fw, 46., t);
        let name = if m.online_account.is_empty() {
            "已登录"
        } else {
            &m.online_account
        };
        clipped_text(f, ix + 12., y + 81., fw - 144., 15., name, p.text);
        clipped_text(
            f,
            ix + 12.,
            y + 101.,
            fw - 144.,
            15.,
            &cfg.wenlai_domain,
            p.muted,
        );
        action(
            f,
            hs,
            ix + fw - 116.,
            y + 79.,
            104.,
            "退出登录",
            Action::OnlineLogout,
            t,
            false,
        );
        text(f, ix, y + 146., 15., "分类", p.muted);
        let cx = ix + 48.;
        let cw = fw - 48.;
        let more_w = if m.online_categories.len() > 4 {
            70.
        } else {
            0.
        };
        let cap = ((cw - more_w + 8.) / 88.).floor().max(1.) as usize;
        let selected = m
            .online_category
            .min(m.online_categories.len().saturating_sub(1));
        let start = selected.saturating_add(1).saturating_sub(cap);
        let visible = m.online_categories.len().min(cap);
        let bw = ((cw - more_w - 8. * visible.saturating_sub(1) as f32) / visible.max(1) as f32)
            .min(140.);
        for (j, (idx, (_, name))) in m
            .online_categories
            .iter()
            .enumerate()
            .skip(start)
            .take(visible)
            .enumerate()
        {
            chip(
                f,
                hs,
                cx + j as f32 * (bw + 8.),
                y + 136.,
                bw,
                &elide_width(name, bw - 14., 15.),
                idx == m.online_category,
                Action::OnlineCategory(idx),
                t,
            );
        }
        if m.online_categories.is_empty() {
            text(f, cx, y + 146., 15., "分类加载中…", p.muted);
        } else if m.online_categories.len() > cap {
            link(
                f,
                hs,
                ix + fw - 66.,
                y + 136.,
                66.,
                "更多 ›",
                Action::OnlineCategory((selected + 1) % m.online_categories.len()),
                t,
            );
        }
        text(f, ix, y + 190., 15., "难度", p.muted);
        let dw = (cw - 6. * 6.) / 7.;
        for (i, name) in ["淼", "水", "易", "普", "难", "虐", "随机"]
            .into_iter()
            .enumerate()
        {
            chip(
                f,
                hs,
                cx + i as f32 * (dw + 6.),
                y + 180.,
                dw,
                name,
                m.online_difficulty == i,
                Action::OnlineDifficultyTo(i),
                t,
            );
        }
        text(f, ix, y + ONLINE_CHARS_Y + 10., 15., "字数", p.muted);
        round(
            f,
            cx - 1.,
            y + ONLINE_CHARS_Y - 1.,
            94.,
            40.,
            7.,
            p.bg2,
            Some(p.line2),
        );
        toggle(
            f,
            hs,
            cx + 108.,
            y + ONLINE_CHARS_Y + 11.,
            m.online_strict,
            Action::OnlineStrict,
            t,
        );
        text(
            f,
            cx + 144.,
            y + ONLINE_CHARS_Y + 10.,
            15.,
            "精确字数",
            p.muted,
        );
        toggle(
            f,
            hs,
            cx + 250.,
            y + ONLINE_CHARS_Y + 11.,
            m.field("online_auto_next") != "false",
            crate::aux_marker("online_auto_next"),
            t,
        );
        text(
            f,
            cx + 286.,
            y + ONLINE_CHARS_Y + 10.,
            15.,
            "自动换文",
            p.muted,
        );
        let aw = (fw - 16.) / 3.;
        for (i, (name, dir)) in [("上一段", -1), ("随机取文", 0), ("下一段", 1)]
            .into_iter()
            .enumerate()
        {
            action(
                f,
                hs,
                ix + i as f32 * (aw + 8.),
                y + 284.,
                aw,
                name,
                Action::OnlineFetch(dir),
                t,
                dir == 0,
            );
        }
        let status = if m.online_busy {
            "正在取文，当前文章已保留…"
        } else if !m.online_status.is_empty() {
            &m.online_status
        } else {
            "取文后直接载入；上 / 下一段也随机换文。"
        };
        let bad = ["失败", "过期", "错误", "频繁"]
            .iter()
            .any(|word| status.contains(word));
        online_status_strip(f, ix, y + 332., fw, status, t, bad);
    } else {
        card(f, ix, y + 78., fw, 126., t);
        text(
            f,
            ix + 16.,
            y + 92.,
            18.,
            if m.online_expired {
                "登录已过期"
            } else {
                "登录后在线取文"
            },
            p.text,
        );
        clipped_text(
            f,
            ix + 16.,
            y + 122.,
            fw - 32.,
            15.,
            if m.online_expired {
                "当前文章已保留，重新登录即可继续。"
            } else {
                "选择分类、难度和字数，取文后直接跟打。"
            },
            p.muted,
        );
        action(
            f,
            hs,
            ix + 16.,
            y + 154.,
            168.,
            "登录 / 注册",
            Action::OnlineAccount,
            t,
            true,
        );
    }
    line(f, x + 16., y + ph - 62., pw - 32., p.line);
    clipped_text(
        f,
        ix,
        y + ph - 39.,
        fw - 132.,
        15.,
        "取文后直接跟打",
        p.muted,
    );
    action(
        f,
        hs,
        x + pw - 132.,
        y + ph - 50.,
        112.,
        "返回跟打",
        Action::Close,
        t,
        true,
    );
}

fn rules_side(pw: f32) -> f32 {
    (pw * 0.24).clamp(176., 220.)
}

fn rules_options_extra(pw: f32) -> f32 {
    if pw - rules_side(pw) - 32. >= 540. {
        0.
    } else {
        44.
    }
}

pub(crate) fn content_height_rules(pw: f32) -> f32 {
    620. + rules_options_extra(pw)
}

fn rules_geometry(pw: f32, _ph: f32) -> (f32, f32, f32, f32, f32, f32) {
    let side = rules_side(pw);
    let rw = pw - side - 32.;
    (
        side,
        rw,
        16.,
        326. + rules_options_extra(pw),
        166.,
        (rw - 44.) / 2.,
    )
}
fn result_paragraph(
    f: &mut Frame,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    source: &str,
    output: &str,
    t: &Theme,
) {
    let p = tokens(t);
    let a: Vec<char> = source.chars().collect();
    let b: Vec<char> = output.chars().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let changed_end = b.len().saturating_sub(suffix);
    let lines = wrap_lines(output, w, 14.);
    let mut at = 0usize;
    clip(f, x, y, w, h);
    for (i, line) in lines
        .iter()
        .take((h / 21.).ceil().max(0.) as usize)
        .enumerate()
    {
        let mut spans = Vec::new();
        let mut u16_at = 0u32;
        for ch in line.chars() {
            if at >= prefix && at < changed_end {
                spans.push((u16_at, ch.len_utf16() as u32, p.accent));
            }
            at += 1;
            u16_at += ch.len_utf16() as u32;
        }
        f.push(DrawCmd::StyledLine {
            x,
            y: y + i as f32 * 21.,
            w,
            size: 14.,
            text: line.clone(),
            spans,
            color: p.text2,
        });
        // A wrapped paragraph consumes LF as well; account for source line breaks
        // when locating the changed range without ever slicing UTF-8 by byte index.
        if b.get(at) == Some(&'\n') {
            at += 1;
        }
    }
    end_clip(f);
}

fn draw_rules(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    cfg: &Config,
    t: &Theme,
    x: f32,
    y: f32,
    pw: f32,
    ph: f32,
) {
    let p = tokens(t);
    overlay_header(
        f,
        hs,
        t,
        x,
        y,
        pw,
        "≡",
        "文本清洗",
        "保留原文；预览确认后再应用",
        "F9",
    );
    // Narrow layouts add an option row beyond the fixed panel's body. Keep
    // every body command in one section so fit_panel scrolls the quote controls
    // with the content instead of mistaking them for the fixed footer.
    clip(f, x + 1., y + 62., pw - 2., content_height_rules(pw) - 124.);
    let (side, rw, _, test_y, test_h, cw) = rules_geometry(pw, ph);
    text(
        f,
        x + 16.,
        y + 84.,
        18.,
        format!("规则 · {} 条", m.rules.len()),
        p.text2,
    );
    link(
        f,
        hs,
        x + side - 58.,
        y + 76.,
        44.,
        "重置",
        Action::RuleReset,
        t,
    );
    let offset = (m.scroll as usize).min(m.rules.len().saturating_sub(1));
    let list_rows = 8;
    // 【紧凑 2026-11-14】行高 62→48：8 行列表从 496 收到 384，
    // 仍保留 toggle + 名称 + 类型两行文字的可读密度。
    for (j, (idx, (name, on, content, _, _))) in m
        .rules
        .iter()
        .enumerate()
        .skip(offset)
        .take(list_rows)
        .enumerate()
    {
        let yy = y + 106. + j as f32 * 44.;
        if idx == m.rule_selected {
            round(
                f,
                x + 10.,
                yy - 4.,
                side - 20.,
                44.,
                6.,
                p.surface2,
                Some(p.line),
            );
        }
        toggle(f, hs, x + 18., yy + 10., *on, Action::RuleToggle(idx), t);
        clipped_text(f, x + 58., yy + 2., side - 72., 14., name, p.text2);
        text(
            f,
            x + 58.,
            yy + 24.,
            12.,
            if *content {
                "内容级"
            } else if cfg
                .cleaning_preset
                .rules
                .get(idx)
                .is_some_and(|r| r.builtin)
            {
                "内置安全规则"
            } else {
                "自定义规则"
            },
            p.muted,
        );
        hs.push(Hit {
            x: x + 54.,
            y: yy - 4.,
            w: side - 66.,
            h: 44.,
            action: Action::RuleSelect(idx),
        });
    }
    let add_y = y + 106. + list_rows.min(m.rules.len().saturating_sub(offset)) as f32 * 44.;
    action(
        f,
        hs,
        x + 12.,
        add_y + 8.,
        side - 24.,
        "+ 新建规则",
        Action::RuleAdd,
        t,
        false,
    );
    let bw = (side - 40.) / 3.;
    for (i, (label, a)) in [
        ("上移", Action::RuleUp),
        ("下移", Action::RuleDown),
        ("删除", Action::RuleDelete),
    ]
    .into_iter()
    .enumerate()
    {
        chip(
            f,
            hs,
            x + 12. + i as f32 * (bw + 8.),
            y + 510.,
            bw,
            label,
            false,
            a,
            t,
        );
    }
    let rx = x + side + 16.;
    let extra = rules_options_extra(pw);
    card(f, rx, y + 74., rw, 244. + extra, t);
    text(f, rx + 16., y + 88., 18., "规则编辑", p.text2);
    for (yy, label) in [(124., "名称"), (168., "正则表达式"), (212., "替换文本")] {
        text(f, rx + 16., y + yy + 10., 15., label, p.muted);
        round(
            f,
            rx + 104. - 1.,
            y + yy - 1.,
            rw - 120. + 2.,
            38.,
            7.,
            p.bg2,
            Some(p.line2),
        );
    }
    clipped_text(
        f,
        rx + 16.,
        y + 256.,
        rw - 32.,
        15.,
        "捕获组用 $1；不支持反向引用与环视。",
        p.muted,
    );
    let scope_y = y + 278.;
    let scope_chip = ((rw - 80.) / 2.).min(102.);
    text(f, rx + 16., scope_y + 10., 15., "范围", p.muted);
    chip(
        f,
        hs,
        rx + 58.,
        scope_y,
        scope_chip,
        "仅本次",
        !m.rule_save_preset,
        if m.rule_save_preset {
            Action::RuleScopeToggle
        } else {
            Action::None
        },
        t,
    );
    chip(
        f,
        hs,
        rx + 66. + scope_chip,
        scope_y,
        scope_chip,
        "保存预设",
        m.rule_save_preset,
        if m.rule_save_preset {
            Action::None
        } else {
            Action::RuleScopeToggle
        },
        t,
    );
    let safe = cfg
        .cleaning_preset
        .rules
        .get(m.rule_selected)
        .is_some_and(|r| r.safe);
    let hx = if extra > 0. {
        rx + 16.
    } else {
        rx + 16. + (rw - 32.) / 2. + 8.
    };
    let level_y = scope_y + extra;
    text(f, hx, level_y + 10., 15., "级别", p.muted);
    chip(
        f,
        hs,
        hx + 42.,
        level_y,
        70.,
        "安全",
        safe,
        if safe {
            Action::None
        } else {
            Action::RuleContentToggle
        },
        t,
    );
    chip(
        f,
        hs,
        hx + 120.,
        level_y,
        82.,
        "内容级",
        !safe,
        if safe {
            Action::RuleContentToggle
        } else {
            Action::None
        },
        t,
    );
    // 【审计 I06】规则保存的结果在这里留成状态行。
    crate::modals::save_status_line(f, rx + rw - 288., y + 90., 150., &m.save, t);
    action(
        f,
        hs,
        rx + rw - 122.,
        y + 80.,
        106.,
        "保存规则",
        Action::RuleSave,
        t,
        true,
    );
    let ty = y + test_y;
    card(f, rx, ty, rw, test_h, t);
    text(f, rx + 16., ty + 15., 18., "测试", p.text2);
    clipped_text(
        f,
        rx + 76.,
        ty + 18.,
        rw - 238.,
        14.,
        &format!("命中 {} 处", m.rule_hits),
        if m.rule_error.is_empty() {
            p.accent
        } else {
            p.red
        },
    );
    link(
        f,
        hs,
        rx + rw - 142.,
        ty + 5.,
        126.,
        "预览整组 →",
        Action::RuleTest,
        t,
    );
    let col_y = ty + 50.;
    let col_h = test_h - 78.;
    round(f, rx + 12., col_y, cw, col_h, 6., p.bg2, Some(p.line));
    round(f, rx + 32. + cw, col_y, cw, col_h, 6., p.bg2, Some(p.line));
    text(f, rx + 20., col_y + 10., 14., "原文 · 可编辑", p.muted);
    text(f, rx + 40. + cw, col_y + 10., 14., "替换后", p.muted);
    let available = !m.rule_test_source.is_empty() || !m.rule_preview.is_empty() || m.rule_hits > 0;
    if !m.rule_error.is_empty() {
        paragraph(
            f,
            rx + 40. + cw,
            col_y + 30.,
            cw - 16.,
            col_h - 38.,
            14.,
            24.,
            &m.rule_error,
            p.red,
        );
    } else if !available {
        paragraph(
            f,
            rx + 40. + cw,
            col_y + 30.,
            cw - 16.,
            col_h - 38.,
            14.,
            24.,
            "点击「预览整组」查看替换结果。",
            p.muted,
        );
    } else if m.rule_preview.is_empty() {
        paragraph(
            f,
            rx + 40. + cw,
            col_y + 30.,
            cw - 16.,
            col_h - 38.,
            14.,
            24.,
            "替换后为空文本。",
            p.accent,
        );
    } else {
        result_paragraph(
            f,
            rx + 40. + cw,
            col_y + 30.,
            cw - 16.,
            col_h - 38.,
            &m.rule_test_source,
            &m.rule_preview,
            t,
        );
    }
    clipped_text(
        f,
        rx + 16.,
        ty + test_h - 23.,
        rw - 32.,
        14.,
        if available && m.rule_test_source != m.field("rule_test") {
            "原文已改，请重新预览；预览不写回正文。"
        } else {
            "预览不改正文，明确应用后才生效。"
        },
        p.muted,
    );
    // 【紧凑 2026-11-14】引号卡 86→54：标题行与三组开关同行。
    let qy = y + 500. + extra;
    card(f, rx, qy, rw, 54., t);
    text(f, rx + 16., qy + 7., 18., "引号宽松比对", p.text2);
    toggle(
        f,
        hs,
        rx + rw - 48.,
        qy + 12.,
        m.quote_relaxed,
        Action::QuoteToggle,
        t,
    );
    let q = &cfg.session_rules.quotes;
    for (i, (label, on)) in [
        ("双引号", q.double),
        ("单引号", q.single),
        ("角括号", q.corner),
    ]
    .into_iter()
    .enumerate()
    {
        let xx = rx + 16. + i as f32 * ((rw - 32.) / 3.);
        toggle(
            f,
            hs,
            xx,
            qy + 30.,
            on && m.quote_relaxed,
            Action::QuoteClass(i as u8),
            t,
        );
        text(f, xx + 34., qy + 30., 15., label, p.muted);
    }
    end_clip(f);
    line(f, x + 16., y + ph - 62., pw - 32., p.line);
    let buttons = [
        ("还原原文", Action::RuleRestore, false),
        ("导入", Action::RuleImport, false),
        ("导出", Action::RuleExport, false),
        ("应用到当前文", Action::RuleApply, true),
    ];
    let bw = ((pw - 72.) / 4.).min(180.);
    for (i, (label, a, primary)) in buttons.into_iter().enumerate() {
        action(
            f,
            hs,
            x + 24. + i as f32 * (bw + 8.),
            y + ph - 49.,
            bw,
            label,
            a,
            t,
            primary,
        );
    }
}

/// The native editors share the exact geometry of their drawn field shells.
/// The outer panel transforms and clips these fields with the same body scroll.
pub fn fields(m: &Model, x: f32, y: f32, pw: f32, ph: f32) -> Vec<FieldSpec> {
    let mut out = Vec::new();
    let mut add = |id, x, y, w, h, multi, password| {
        out.push(FieldSpec {
            id,
            x,
            y,
            w,
            h,
            multi,
            password,
        })
    };
    match m.modal {
        Modal::Online if login_visible(m) => {
            let (fx, fw) = online_form(pw);
            let fx = x + fx + ONLINE_FORM_LABEL;
            let input_w = fw - ONLINE_FORM_LABEL;
            add(
                "username",
                fx,
                y + ONLINE_FORM_TOP,
                input_w,
                38.,
                false,
                false,
            );
            add(
                "password",
                fx,
                y + ONLINE_FORM_TOP + ONLINE_FORM_ROW,
                input_w,
                38.,
                false,
                true,
            );
            if m.online_register_mode {
                add(
                    "reg_email",
                    fx,
                    y + ONLINE_FORM_TOP + 2. * ONLINE_FORM_ROW,
                    input_w,
                    38.,
                    false,
                    false,
                );
                add(
                    "reg_code",
                    fx,
                    y + ONLINE_FORM_TOP + 3. * ONLINE_FORM_ROW,
                    input_w - 140.,
                    38.,
                    false,
                    false,
                );
            } else {
                add(
                    "server",
                    fx,
                    y + ONLINE_FORM_TOP + 2. * ONLINE_FORM_ROW,
                    input_w,
                    38.,
                    false,
                    false,
                );
            }
        }
        Modal::Online if m.online_authenticated && !m.online_expired => {
            let (fx, _) = online_form(pw);
            add(
                "online_chars",
                x + fx + 48.,
                y + ONLINE_CHARS_Y,
                92.,
                36.,
                false,
                false,
            );
        }
        Modal::Rules => {
            let (side, rw, _, test_y, test_h, cw) = rules_geometry(pw, ph);
            let rx = x + side + 16.;
            for (id, yy) in [
                ("rule_name", 124.),
                ("rule_pattern", 168.),
                ("rule_replacement", 212.),
            ] {
                add(id, rx + 104., y + yy, rw - 120., 36., false, false);
            }
            add(
                "rule_test",
                rx + 20.,
                y + test_y + 94.,
                cw - 16.,
                test_h - 128.,
                true,
                false,
            );
        }
        _ => {}
    }
    out
}
pub fn draw(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    m: &Model,
    _sess: &Session,
    cfg: &Config,
    t: &Theme,
    x: f32,
    y: f32,
    pw: f32,
    ph: f32,
) {
    match m.modal {
        Modal::Online => draw_online(f, hs, m, cfg, t, x, y, pw, ph),
        Modal::Rules => draw_rules(f, hs, m, cfg, t, x, y, pw, ph),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn overlaps(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
        a.0 < b.0 + b.2 && a.0 + a.2 > b.0 && a.1 < b.1 + b.3 && a.1 + a.3 > b.1
    }

    #[test]
    fn compact_online_editors_and_buttons_never_cover_each_other() {
        let cfg = Config::default();
        let theme = crate::ui::theme("墨玉");
        for pw in [528., 560., 608., 880.] {
            for state in 0..4 {
                let mut m = Model::default();
                m.modal = Modal::Online;
                m.online_login_expanded = state == 1 || state == 2;
                m.online_register_mode = state == 2;
                m.online_authenticated = state == 3;
                m.online_categories = ["古文", "诗词", "现代", "全部", "长分类名称"]
                    .into_iter()
                    .map(|s| (s.into(), s.into()))
                    .collect();
                let ph = content_height_online(&m);
                let mut f = Frame::new(pw, ph);
                let mut hs = Vec::new();
                draw_online(&mut f, &mut hs, &m, &cfg, &theme, 0., 0., pw, ph);
                for field in fields(&m, 0., 0., pw, ph) {
                    assert!(field.x >= 20. && field.x + field.w <= pw - 20.);
                    assert!(field.y >= 62. && field.y + field.h <= ph - 62.);
                    for hit in &hs {
                        assert!(
                            !overlaps(
                                (field.x, field.y, field.w, field.h),
                                (hit.x, hit.y, hit.w, hit.h)
                            ),
                            "state={state} pw={pw}: field {} covers {:?}",
                            field.id,
                            hit.action
                        );
                    }
                }
                for hit in hs.iter().filter(|h| h.action != Action::Close) {
                    assert!(
                        hit.x >= 0. && hit.x + hit.w <= pw && hit.y + hit.h <= ph - 62.,
                        "state={state} pw={pw}: {:?} outside body",
                        hit.action
                    );
                }
            }
        }
    }

    #[test]
    fn compact_cleaning_editors_and_actions_remain_inside_the_content() {
        let cfg = Config::default();
        let theme = crate::ui::theme("墨玉");
        let mut m = Model::default();
        m.modal = Modal::Rules;
        for pw in [608., 720., 840., 1040.] {
            let ph = content_height_rules(pw);
            let mut f = Frame::new(pw, ph);
            let mut hs = Vec::new();
            draw_rules(&mut f, &mut hs, &m, &cfg, &theme, 0., 0., pw, ph);
            for field in fields(&m, 0., 0., pw, ph) {
                assert!(field.x >= 12. && field.x + field.w <= pw - 12.);
                assert!(field.y >= 62. && field.y + field.h <= ph - 62.);
                for hit in &hs {
                    assert!(
                        !overlaps(
                            (field.x, field.y, field.w, field.h),
                            (hit.x, hit.y, hit.w, hit.h)
                        ),
                        "pw={pw}: field {} covers {:?}",
                        field.id,
                        hit.action
                    );
                }
            }
            for hit in &hs {
                assert!(
                    hit.x >= 0. && hit.x + hit.w <= pw && hit.y + hit.h <= ph,
                    "pw={pw}: {:?} outside panel",
                    hit.action
                );
            }
        }
    }

    #[test]
    fn cleaning_fields_fit_both_comparison_columns() {
        for pw in [640., 880., 1040.] {
            let mut m = Model::default();
            m.modal = Modal::Rules;
            let fs = fields(&m, 0., 0., pw, 900.);
            let raw = fs.iter().find(|z| z.id == "rule_test").unwrap();
            let (side, rw, _, _, _, cw) = rules_geometry(pw, 900.);
            assert!(raw.x >= side && raw.x + raw.w <= side + 16. + 12. + cw);
            assert!(raw.w > 100. && rw > raw.w * 2.);
            // 【紧凑 2026-11-14】整页收到 680 高：测试输入框必须止步于引号卡上方。
            assert!(raw.y + raw.h < 554.);
            assert!(fs.iter().all(|field| field.w > 100. && field.h >= 34.));
        }
    }

    #[test]
    fn register_fields_leave_room_for_send_code_and_cooldown_disables_it() {
        let cfg = Config::default();
        let t = crate::ui::theme("墨玉");
        for pw in [640., 880.] {
            let mut m = Model::default();
            m.modal = Modal::Online;
            m.online_login_expanded = true;
            m.online_register_mode = true;
            let fs = fields(&m, 0., 0., pw, 760.);
            let ids: Vec<_> = fs.iter().map(|z| z.id).collect();
            assert_eq!(ids, vec!["username", "password", "reg_email", "reg_code"]);
            assert!(fs
                .iter()
                .all(|field| field.h >= 34. && field.w >= 150. && !field.multi));
            let code = fs.iter().find(|z| z.id == "reg_code").unwrap();
            let mut f = Frame::new(pw, 760.);
            let mut hs = Vec::new();
            draw_online(&mut f, &mut hs, &m, &cfg, &t, 0., 0., pw, 760.);
            let send = hs
                .iter()
                .find(|h| h.action == crate::aux_marker("online_reg_code"))
                .unwrap();
            assert!(code.x + code.w + 8. <= send.x);
            assert_eq!(code.y, send.y);
            assert!(hs
                .iter()
                .any(|h| h.action == crate::aux_marker("online_reg_submit")));
            assert!(hs
                .iter()
                .any(|h| h.action == crate::aux_marker("online_register")));
            m.online_code_wait = 59;
            hs.clear();
            draw_online(
                &mut Frame::new(pw, 760.),
                &mut hs,
                &m,
                &cfg,
                &t,
                0.,
                0.,
                pw,
                760.,
            );
            assert!(!hs
                .iter()
                .any(|h| h.action == crate::aux_marker("online_reg_code")));
        }
    }

    #[test]
    fn authenticated_online_page_hides_credentials_and_never_draws_article_body() {
        let cfg = Config::default();
        let t = crate::ui::theme("墨玉");
        let mut m = Model::default();
        m.modal = Modal::Online;
        m.online_login_expanded = true;
        let ids: Vec<_> = fields(&m, 0., 0., 880., 900.)
            .iter()
            .map(|z| z.id)
            .collect();
        assert_eq!(ids, vec!["username", "password", "server"]);
        let mut f = Frame::new(880., 900.);
        let mut hs = Vec::new();
        draw_online(&mut f, &mut hs, &m, &cfg, &t, 0., 0., 880., 900.);
        assert!(hs
            .iter()
            .any(|h| h.action == crate::aux_marker("online_register")));
        assert!(hs.iter().any(|h| h.action == Action::OnlineLogin));

        m.online_authenticated = true;
        m.online_preview = "正文哨兵：不能出现在在线面板中".into();
        let fs = fields(&m, 0., 0., 880., 900.);
        assert_eq!(fs.len(), 1);
        assert_eq!(fs[0].id, "online_chars");
        f.cmds.clear();
        hs.clear();
        draw_online(&mut f, &mut hs, &m, &cfg, &t, 0., 0., 880., 900.);
        assert!(!hs.iter().any(|h| h.action == Action::OnlineLoad));
        assert!(hs.iter().any(|h| h.action == Action::OnlineLogout));
        assert!(!f.cmds.iter().any(|c| matches!(c,
            DrawCmd::TextRun { text, .. } if text.contains("正文哨兵")
        )));
    }

    #[test]
    fn selecting_active_scope_does_not_toggle_to_persistent_storage() {
        let cfg = Config::default();
        let t = crate::ui::theme("墨玉");
        for persistent in [false, true] {
            let mut m = Model::default();
            m.modal = Modal::Rules;
            m.rule_save_preset = persistent;
            let mut f = Frame::new(1040., 900.);
            let mut hs = Vec::new();
            draw_rules(&mut f, &mut hs, &m, &cfg, &t, 0., 0., 1040., 900.);
            let rx = rules_side(1040.) + 16.;
            let local = crate::modals::hit(&hs, rx + 80., 296.);
            let saved = crate::modals::hit(&hs, rx + 200., 296.);
            assert_eq!(
                local,
                if persistent {
                    Some(Action::RuleScopeToggle)
                } else {
                    None
                }
            );
            assert_eq!(
                saved,
                if persistent {
                    None
                } else {
                    Some(Action::RuleScopeToggle)
                }
            );
        }
    }

    #[test]
    fn secondary_controls_are_readable_in_both_themes() {
        let cfg = Config::default();
        for name in ["墨玉", "宣纸"] {
            let t = crate::ui::theme(name);
            for modal in [Modal::Online, Modal::Rules] {
                let mut m = Model::default();
                m.modal = modal;
                let mut f = Frame::new(880., 900.);
                let mut hs = Vec::new();
                draw(
                    &mut f,
                    &mut hs,
                    &m,
                    &Session::new(100),
                    &cfg,
                    &t,
                    0.,
                    0.,
                    880.,
                    900.,
                );
                for cmd in &f.cmds {
                    match cmd {
                        DrawCmd::TextRun { h, text, .. } => {
                            assert!(*h >= 14., "{modal:?}: {text}");
                            assert!(!text.contains("\\n"), "literal newline escape: {text}");
                        }
                        DrawCmd::StyledLine { size, .. } => assert!(*size >= 14.),
                        _ => {}
                    }
                }
                assert!(hs.iter().all(|hit| hit.h >= 34.));
                let primary_label = if modal == Modal::Online {
                    "登录 / 注册"
                } else {
                    "应用到当前文"
                };
                assert!(f.cmds.iter().any(|cmd| matches!(cmd,
                    DrawCmd::TextRun { text, color, .. }
                        if text == primary_label && *color == tokens(&t).bg
                )));
            }
        }
    }
}
