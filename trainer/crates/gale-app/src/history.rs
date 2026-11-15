//! 底栏成绩账本：点「成绩记录」后为一张向下展开的分段成绩表，不再有独立成绩页。
//!
//! 结构：表头（点它收起）+ 9 列表格（用时/重打/段号/速度/击键/码长/键准/打词/字数）。
//! 表格占底部区域，正文区向上让位；数据直接取 ScoreRecord，不做额外加工。
use crate::{
    modals::{Action, Hit, Modal, Model},
    session::{Phase, Session},
    ui::*,
};
use gale_core::{config::Config, score::ScoreRecord, theme::Theme};
use gale_win::d2d::{Align, DrawCmd, FontFamily, Frame};

/// 收起时的摘要条高度。
pub const BAR_H: f32 = 36.;
/// 表格行高。
const ROW_H: f32 = 32.;
/// 表头高度（列名）。
const HEAD_H: f32 = 26.;
/// 面板底部留白。
const PAD_BOTTOM: f32 = 8.;

/// 面板底边距离窗口底的距离：常规=状态行上方；极简模式没有状态行，贴底 12px。
pub fn bottom_inset(cfg: &Config) -> f32 {
    if cfg.minimal {
        12.
    } else {
        STATUS_H + 8.
    }
}

fn visible_rows(cfg: &Config, m: &Model, h: f32) -> usize {
    let ph = if cfg.ledger_panel_h > 0. {
        cfg.ledger_panel_h
    } else {
        (h * 0.55).max(180.)
    };
    let body = (ph - BAR_H - HEAD_H - PAD_BOTTOM).max(ROW_H);
    (body / ROW_H).floor().clamp(1., 14.) as usize
}

/// 展开后的面板高度：用户拖过就用记忆高度，否则按成绩条数自动收短。
pub fn panel_height(cfg: &Config, m: &Model, h: f32) -> f32 {
    if cfg.ledger_panel_h > 0. {
        return cfg.ledger_panel_h.clamp(120., (h - 140.).max(160.));
    }
    let rows = m.history.len().min(visible_rows(cfg, m, h)).max(1) as f32;
    (BAR_H + HEAD_H + rows * ROW_H + PAD_BOTTOM).clamp(120., (h - 140.).max(200.))
}

/// 【2026-11-13】成绩面板/摘要条的左右边界：对照模式下与两张卡对齐
/// （非极简卡边 36px、极简 16px），字贴模式保持原 24px 内距。
fn horizontal_bounds(cfg: &Config, w: f32) -> (f32, f32) {
    if cfg.display_mode.is_split() {
        let bw = if cfg.minimal {
            (w - 56.).max(360.).min(1600.)
        } else {
            (w - 96.).max(360.).min(1600.)
        };
        (((w - bw) / 2.) - 12., bw + 24.)
    } else {
        (24., (w - 48.).max(320.))
    }
}

pub fn panel_rect_for(cfg: &Config, m: &Model, w: f32, h: f32) -> (f32, f32, f32, f32) {
    let ph = panel_height(cfg, m, h);
    let bottom = h - bottom_inset(cfg);
    let (x, pw) = horizontal_bounds(cfg, w);
    (x, bottom - ph, pw, ph)
}

/// 面板上沿 8px 作为拖动把手（拖高拖低），主循环据此进入拖动。
pub fn resize_grip_rect(cfg: &Config, m: &Model, w: f32, h: f32) -> Option<(f32, f32, f32, f32)> {
    if !m.ledger_open || m.modal != Modal::None {
        return None;
    }
    let (x, y, pw, _) = panel_rect_for(cfg, m, w, h);
    Some((x, y - 2., pw, 8.))
}

/// 收起状态下的摘要条（仅常规模式；极简模式不画底栏），横向与面板同界。
pub fn bar_rect(cfg: &Config, w: f32, h: f32) -> (f32, f32, f32, f32) {
    let bottom = h - STATUS_H - 8.;
    let (x, bw) = horizontal_bounds(cfg, w);
    (x, bottom - BAR_H, bw.max(200.), BAR_H)
}

pub fn scroll_max(cfg: &Config, m: &Model, h: f32) -> f32 {
    let rows = m.history.len() as f32;
    let visible = visible_rows(cfg, m, h) as f32;
    ((rows - visible).max(0.) * ROW_H).max(0.)
}

pub fn record_offset(m: &Model, idx: usize) -> f32 {
    // 列表按最新在上：第 idx 条（m.history 顺序）的距离 = (len-1-idx) 行。
    (m.history.len().saturating_sub(1 + idx) as f32) * ROW_H
}

pub fn draw(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    sess: &Session,
    cfg: &Config,
    m: &Model,
    t: &Theme,
    w: f32,
    h: f32,
) {
    if m.modal != Modal::None {
        return;
    }
    if m.ledger_open {
        draw_table(f, hs, sess, cfg, m, t, w, h);
    } else if !cfg.minimal {
        draw_bar(f, hs, sess, cfg, m, t, w, h);
    }
}

fn draw_bar(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    sess: &Session,
    cfg: &Config,
    m: &Model,
    t: &Theme,
    w: f32,
    h: f32,
) {
    let p = tokens(t);
    let (x, y, bw, bh) = bar_rect(cfg, w, h);
    if sess.phase == Phase::Done {
        // 完成态与空闲态同一张底栏：同背景、同 14px 内距、同 1px 垂直基准。
        round(f, x, y, bw, bh, 8., p.surface, Some(p.line));
        let record = sess
            .results
            .get(sess.current_index())
            .and_then(Option::as_ref);
        let has_next = sess.current_index() + 1 < sess.total_segments();
        let summary = record
            .map(|r| format!("本段完成  {:.1} 字/分 · 键准 {:.1}%", r.speed, r.accuracy))
            .unwrap_or_else(|| "本段已完成".into());
        let controls_w = if has_next { 292. } else { 196. };
        text(
            f,
            x + 14.,
            y + 9.,
            14.,
            elide(&summary, ((bw - controls_w - 46.) / 14.).max(6.) as usize),
            p.text2,
        );
        let mut bx = x + bw - 14. - controls_w;
        if has_next {
            button(f, hs, bx, y + 2., 90., "下一段", Action::Next, t, true);
            bx += 96.;
        }
        button(
            f,
            hs,
            bx,
            y + 2.,
            82.,
            "重打",
            Action::Restart,
            t,
            !has_next,
        );
        if let Some(idx) = m.history.len().checked_sub(1) {
            button(
                f,
                hs,
                bx + 88.,
                y + 2.,
                98.,
                "查看成绩",
                Action::SelectHistory(idx),
                t,
                false,
            );
        }
        return;
    }
    round(f, x, y, bw, bh, 8., p.surface, Some(p.line));
    // 练字记录：底栏顺带显示今日实际上屏字数，完整 7/30/总在设置「数据与关于」。
    let practice_today = {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let today = gale_core::score::unix_to_date(now);
        cfg.practice_days.get(&today).copied().unwrap_or(0)
    };
    let bar_label = if bw >= 700. && practice_today > 0 {
        format!(
            "成绩记录 · {} 条 · 今日练字 {} 字",
            m.history.len(),
            practice_today
        )
    } else {
        format!("成绩记录 · {} 条", m.history.len())
    };
    text(f, x + 14., y + 9., 14., bar_label, p.text2);
    if let Some(r) = m.history.last() {
        if bw >= 560. {
            // 【审计 V20】摘要条也按「数值 + 单位」成组：数值 13px 次文字色、
            // 「字/分」10px 次级色紧跟其后（3px），整组右对齐到箭头左侧。
            // 旧实现是一整串 13px muted，数值与单位没有层级、读起来像一串数字。
            let vu = ValueUnit {
                value: 13.,
                unit: 10.,
            };
            let value = fmt_speed(r);
            let w = ui_text_width("最近", 11.) + STAT_LABEL_GAP + vu.group_width(&value, "字/分");
            draw_stat_group(
                f,
                x + bw - 34. - w,
                y + 22.,
                "最近",
                11.,
                &value,
                p.text2,
                "字/分",
                vu,
                p.muted,
                p.muted,
            );
        }
    }
    right(f, x + bw - 14., y + 7., 16., "⌄", p.muted);
    hs.push(Hit {
        x,
        y,
        w: bw,
        h: bh,
        action: Action::ToggleLedger,
    });
}

/// 【审计 V16】列的视觉权重分档：成绩数字不再和标题、元信息同一权重。
/// 主数值（用时/速度）用主字号＋主文字色；次级指标降一号、用次级文字色；
/// 元信息（重打/段号）用最次一级文字色。行高 ROW_H 不变，字号只影响字面，
/// 所以任何一档都不会把一行撑高。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CellRole {
    Primary,
    Secondary,
    Meta,
}

/// 一列的静态描述：列名、分宽权重、视觉角色、表头单位。
struct ColumnSpec {
    label: &'static str,
    /// 只在「保底宽度之外的余量」上参与分配，见 column_widths。
    weight: f32,
    role: CellRole,
    /// 【审计 V16】单位放列头而不是行内：640 宽下速度列仅 65px，行内
    /// 「57.3 字/分」要 60.5px、「1234.5 字/分」要 88.7px，后者必被裁切；
    /// 审计 V16 的方法本身也要求「速度/准确率等单位在列头明确」。
    unit: &'static str,
}

const COLUMNS: [ColumnSpec; 9] = [
    ColumnSpec {
        label: "用时",
        weight: 0.10,
        role: CellRole::Primary,
        unit: "分:秒",
    },
    ColumnSpec {
        label: "重打",
        weight: 0.07,
        role: CellRole::Meta,
        unit: "",
    },
    ColumnSpec {
        label: "段号",
        weight: 0.11,
        role: CellRole::Meta,
        unit: "",
    },
    ColumnSpec {
        label: "速度",
        weight: 0.10,
        role: CellRole::Primary,
        unit: "字/分",
    },
    ColumnSpec {
        label: "击键",
        weight: 0.09,
        role: CellRole::Secondary,
        unit: "/秒",
    },
    ColumnSpec {
        label: "码长",
        weight: 0.09,
        role: CellRole::Secondary,
        unit: "",
    },
    ColumnSpec {
        label: "键准",
        weight: 0.10,
        role: CellRole::Secondary,
        unit: "%",
    },
    ColumnSpec {
        label: "打词",
        weight: 0.09,
        role: CellRole::Secondary,
        unit: "%",
    },
    ColumnSpec {
        label: "字数",
        weight: 0.08,
        role: CellRole::Secondary,
        unit: "",
    },
];

/// 【审计 V16】各列最宽内容：列宽按它反推保底值，保证最小窗也不裁数字。
/// 用时取 99:59.9（单段超 100 分钟才会更长）、速度取四位 9999.9、段号取
/// 「第999段」这种常见标记；键准/打词不再带 %，单位已移到列头。
const WORST_CELLS: [&str; 9] = [
    "99:59.9", "999", "第999段", "9999.9", "99.99", "99.99", "100.0", "100", "99999",
];

/// TextRun 的 y 是行盒顶，DirectWrite 基线约在 y+0.8em；
/// 用它把不同字号对齐到同一条基线，行内不会出现基线错位。
const BASELINE_RATIO: f32 = 0.8;
/// 行内基线相对行顶的固定偏移：所有列共用，字号变了基线不动。
const ROW_BASELINE: f32 = 21.;
/// 表头基线相对 BAR_H 下沿的固定偏移。
const HEAD_BASELINE: f32 = 15.5;
/// 表头「列名」与「单位」的间隙（4px：同一组，紧贴但不粘连）。
const HEAD_UNIT_GAP: f32 = 4.;
/// 单元格右内距。
const CELL_PAD: f32 = 6.;

fn baseline_text_y(baseline: f32, size: f32) -> f32 {
    baseline - size * BASELINE_RATIO
}

/// Mono 字族宽度估算（ASCII 0.63em、非 ASCII 约 1em）：只用于列宽保底与
/// 「数值 + 单位」成组的右对齐，不参与命中测试。
fn mono_width(s: &str, size: f32) -> f32 {
    s.chars()
        .map(|c| if c.is_ascii() { size * 0.63 } else { size })
        .sum()
}

/// 【审计 V16】表格字号随可用宽度收缩。640 宽最小窗落在 13/11/11/8 档：
/// 13px 下「99:59.9」= 57.3px，比用时列保底 69.3px 窄；11px 下「第999段」
/// = 53.8px，比段号列保底 65.8px 窄；行内文本盒底 ≤ 29.4 < ROW_H 32，
/// 所以行高不因字号调整而溢出。
#[derive(Clone, Copy, Debug, PartialEq)]
struct TableMetrics {
    primary: f32,
    cell: f32,
    head: f32,
    unit: f32,
}

impl TableMetrics {
    fn for_width(inner: f32) -> Self {
        if inner >= 700. {
            Self {
                primary: 14.,
                cell: 12.,
                head: 12.,
                unit: 9.,
            }
        } else if inner >= 520. {
            Self {
                primary: 13.,
                cell: 11.,
                head: 11.,
                unit: 8.,
            }
        } else {
            Self {
                primary: 12.,
                cell: 10.,
                head: 10.,
                unit: 8.,
            }
        }
    }

    fn size_of(&self, role: CellRole) -> f32 {
        if role == CellRole::Primary {
            self.primary
        } else {
            self.cell
        }
    }
}

/// 【审计 V16】列宽 = 保底宽度 + 按权重分的余量。旧实现直接 inner*weight/0.83，
/// 640 宽下重打列只剩 45px，只能靠列裁剪硬切；现在先按最宽内容保底，
/// 余量再按权重分，最小窗每列都容得下自己的数字。
fn column_widths(inner: f32, m: TableMetrics) -> [f32; 9] {
    let inner = inner.max(1.);
    let mut min = [0f32; 9];
    let mut total_min = 0.;
    for (i, spec) in COLUMNS.iter().enumerate() {
        let cell_w = mono_width(WORST_CELLS[i], m.size_of(spec.role));
        let head_w = ui_text_width(spec.label, m.head)
            + if spec.unit.is_empty() {
                0.
            } else {
                HEAD_UNIT_GAP + ui_text_width(spec.unit, m.unit)
            };
        min[i] = cell_w.max(head_w) + CELL_PAD * 2.;
        total_min += min[i];
    }
    let weight_sum: f32 = COLUMNS.iter().map(|c| c.weight).sum();
    let extra = inner - total_min;
    let mut out = [0f32; 9];
    if extra >= 0. {
        for (i, spec) in COLUMNS.iter().enumerate() {
            out[i] = min[i] + extra * spec.weight / weight_sum;
        }
    } else {
        // 低于 640 最小窗的兜底：按比例压缩，宁可窄也不超出面板。
        let scale = inner / total_min;
        for (i, w) in out.iter_mut().enumerate() {
            *w = min[i] * scale;
        }
    }
    out
}

/// 表头一列的排布结果：列名右缘、（可选）单位右缘。
#[derive(Clone, Copy, Debug, PartialEq)]
struct HeaderSlot {
    label_right: f32,
    unit_right: Option<f32>,
}

/// 【审计 V16】表头「列名 + 单位」的右对齐排布：单位贴列名右侧、字号更小；
/// 列宽放不下时只丢单位（列名仍在），绝不靠裁剪区把字切一半。
/// 640 宽两档（inner 536/560）9 列单位都放得下：最紧的用时/速度需 45.7px，
/// 而列宽有 69.3px 保底。
fn header_slots(col_right: f32, col_w: f32, spec: &ColumnSpec, m: TableMetrics) -> HeaderSlot {
    let pad = CELL_PAD.min((col_w * 0.25).max(2.));
    let avail = (col_w - pad * 2.).max(1.);
    let label_w = ui_text_width(spec.label, m.head);
    let unit_w = if spec.unit.is_empty() {
        0.
    } else {
        ui_text_width(spec.unit, m.unit)
    };
    let show_unit = unit_w > 0. && label_w + HEAD_UNIT_GAP + unit_w <= avail + 0.01;
    let content_w = label_w + if show_unit { HEAD_UNIT_GAP + unit_w } else { 0. };
    let right = col_right - pad;
    HeaderSlot {
        label_right: right - content_w + label_w,
        unit_right: show_unit.then_some(right),
    }
}

/// 【审计 V20】一个「数值 + 单位」组的字号组合：数值主字号、单位小字。
#[derive(Clone, Copy, Debug, PartialEq)]
struct ValueUnit {
    value: f32,
    unit: f32,
}

impl ValueUnit {
    /// 单位紧贴数值，间隙 3px（明显小于组间 12px：组内不散、组间可分）。
    const UNIT_GAP: f32 = 3.;

    fn group_width(&self, value: &str, unit: &str) -> f32 {
        let mut w = mono_width(value, self.value);
        if !unit.is_empty() {
            w += Self::UNIT_GAP + ui_text_width(unit, self.unit);
        }
        w
    }

    fn value_y(&self, baseline: f32) -> f32 {
        baseline_text_y(baseline, self.value)
    }

    fn unit_y(&self, baseline: f32) -> f32 {
        baseline_text_y(baseline, self.unit)
    }
}

/// 【审计 V20】统计组之间的固定间隙：所有组用同一个值，组与组间距一致。
const STAT_GROUP_GAP: f32 = 12.;
/// 组内「标签 → 数值」的间隙。
const STAT_LABEL_GAP: f32 = 4.;

/// 【审计 V20】画一组「[标签] 数值 单位」，返回整组宽度。
/// 数值与单位共用一条基线（字号不同但基线对齐），单位小字 + 次级文字色；
/// 组与组之间由调用方用 STAT_GROUP_GAP 分隔。
#[allow(clippy::too_many_arguments)]
fn draw_stat_group(
    f: &mut Frame,
    x: f32,
    baseline: f32,
    label: &str,
    label_size: f32,
    value: &str,
    value_color: [f32; 4],
    unit: &str,
    vu: ValueUnit,
    unit_color: [f32; 4],
    label_color: [f32; 4],
) -> f32 {
    let mut w = 0.;
    if !label.is_empty() {
        text(
            f,
            x,
            baseline_text_y(baseline, label_size),
            label_size,
            label,
            label_color,
        );
        w += ui_text_width(label, label_size) + STAT_LABEL_GAP;
    }
    mono(f, x + w, vu.value_y(baseline), vu.value, value, value_color);
    w += mono_width(value, vu.value);
    if !unit.is_empty() {
        text(
            f,
            x + w + ValueUnit::UNIT_GAP,
            vu.unit_y(baseline),
            vu.unit,
            unit,
            unit_color,
        );
        w += ValueUnit::UNIT_GAP + ui_text_width(unit, vu.unit);
    }
    w
}

/// 【审计 V20】摘要组的取舍：先保「平均速度」（主结果），再保「条数」，
/// 「本轮 N 段」最先让路；三者都放得下时全显示。返回 (条数, 本轮段数, 平均)。
fn summary_group_plan(available: f32, w_count: f32, w_done: f32, w_avg: f32) -> (bool, bool, bool) {
    let avg = w_avg <= available + 0.01;
    let rest = if avg {
        available - w_avg - STAT_GROUP_GAP
    } else {
        available
    };
    let count = w_count <= rest + 0.01;
    let rest = if count {
        rest - w_count - STAT_GROUP_GAP
    } else {
        rest
    };
    let done = count && avg && w_done <= rest + 0.01;
    (count, done, avg)
}

/// 行内只放数字/标记，单位统一在表头（见 ColumnSpec::unit）。
/// 键准/打词不再带 %：单位移到列头后数字右缘对齐更整齐。
fn cell_text(r: &ScoreRecord) -> [String; 9] {
    let seg = r.mark.trim().to_string();
    [
        duration(r.time_ms),
        r.retype_count.to_string(),
        seg,
        fmt_speed(r),
        format!("{:.2}", r.strokes_per_sec),
        format!("{:.2}", r.code_len),
        format!("{:.1}", r.accuracy.clamp(0., 100.)),
        r.word_rate
            .map(|v| format!("{v:.0}"))
            .unwrap_or_else(|| "—".into()),
        r.chars.to_string(),
    ]
}

/// 【审计 V20】表头摘要：「条数 / 本轮段数 / 平均速度」三组，组内数值与单位
/// 同基线、单位小字次级色，组间固定 STAT_GROUP_GAP；宽度不足时按
/// summary_group_plan 让出「本轮段数」→「条数」，平均速度是主结果优先保。
fn draw_ledger_summary(
    f: &mut Frame,
    x: f32,
    y: f32,
    available: f32,
    m: &Model,
    done: usize,
    avg: f64,
    p: &Palette,
) {
    let baseline = y + 21.;
    if done == 0 {
        // 没有完成段就没有「平均」，只留一句说明（仍是次级文字色，不抢标题）。
        text(f, x + 96., y + 12., 12., "本轮还没有完成的段落", p.muted);
        return;
    }
    let meta = ValueUnit {
        value: 12.,
        unit: 10.,
    };
    // 平均速度是主结果：16px 主文字色，单位 10px 次级色，与表头其余组同一基线。
    let main = ValueUnit {
        value: 16.,
        unit: 10.,
    };
    let count = m.history.len().to_string();
    let done_s = done.to_string();
    let avg_s = if avg.is_finite() {
        format!("{avg:.1}")
    } else {
        "—".into()
    };
    let w_count = meta.group_width(&count, "条");
    let w_done = ui_text_width("本轮", 11.) + STAT_LABEL_GAP + meta.group_width(&done_s, "段");
    let w_avg = ui_text_width("平均", 11.) + STAT_LABEL_GAP + main.group_width(&avg_s, "字/分");
    let (show_count, show_done, show_avg) = summary_group_plan(available, w_count, w_done, w_avg);
    let mut groups: Vec<(&str, &str, &str, ValueUnit, [f32; 4])> = Vec::new();
    if show_count {
        groups.push(("", count.as_str(), "条", meta, p.text2));
    }
    if show_done {
        groups.push(("本轮", done_s.as_str(), "段", meta, p.text2));
    }
    if show_avg {
        groups.push(("平均", avg_s.as_str(), "字/分", main, p.text));
    }
    let mut gx = x + 96.;
    for (i, (label, value, unit, vu, color)) in groups.iter().enumerate() {
        if i > 0 {
            gx += STAT_GROUP_GAP;
            // 组间用一颗 60% 的 muted 圆点分隔，间隙仍是同一个 STAT_GROUP_GAP。
            text(
                f,
                gx - STAT_GROUP_GAP / 2. - 2.,
                baseline_text_y(baseline, 11.),
                11.,
                "·",
                alpha(p.muted, 0.6),
            );
        }
        gx += draw_stat_group(
            f, gx, baseline, label, 11., value, *color, unit, *vu, p.muted, p.muted,
        );
    }
}

fn draw_table(
    f: &mut Frame,
    hs: &mut Vec<Hit>,
    sess: &Session,
    cfg: &Config,
    m: &Model,
    t: &Theme,
    w: f32,
    h: f32,
) {
    let p = tokens(t);
    let (x, y, pw, ph) = panel_rect_for(cfg, m, w, h);
    round(f, x, y, pw, ph, 10., p.surface, Some(p.line2));
    // 上沿拖动把手：hover 时主程序把光标换成上下箭头，拖动改面板高度。
    round(
        f,
        x + pw / 2. - 18.,
        y + 3.,
        36.,
        4.,
        2.,
        alpha(p.line2, 0.9),
        None,
    );
    // 表头（可点收起）
    let done = sess.results.iter().flatten().count();
    let avg = if done == 0 {
        0.
    } else {
        sess.results.iter().flatten().map(|r| r.speed).sum::<f64>() / done as f64
    };
    let has_next = sess.current_index() + 1 < sess.total_segments();
    // 【审计 V20】右侧按钮/「收起」占位：摘要不得压上去，组数与列宽都按它收。
    let reserved_right = if sess.phase == Phase::Done {
        if has_next {
            272.
        } else {
            168.
        }
    } else {
        72.
    };
    text(f, x + 16., y + 10., 15., "成绩记录", p.text);
    // 【审计 V20】摘要从一整串 12px muted 改成三组「数值 + 单位」：
    // 组内同基线、单位小字次级色，组间固定 STAT_GROUP_GAP（详见 draw_ledger_summary）。
    draw_ledger_summary(f, x, y, pw - 112. - reserved_right, m, done, avg, &p);
    right(f, x + pw - 16., y + 9., 15., "收起 ⌃", p.muted);
    hs.push(Hit {
        x,
        y,
        w: pw,
        h: BAR_H,
        action: Action::ToggleLedger,
    });
    if sess.phase == Phase::Done {
        let mut rx = x + pw - 76.;
        if has_next {
            button(
                f,
                hs,
                rx - 90.,
                y + 3.,
                84.,
                "下一段",
                Action::Next,
                t,
                true,
            );
            rx -= 98.;
        }
        button(
            f,
            hs,
            rx - 76.,
            y + 3.,
            70.,
            "重打",
            Action::Restart,
            t,
            false,
        );
    }
    line(f, x + 12., y + BAR_H, pw - 24., p.line);

    let left = x + 16.;
    let inner = pw - 32.;
    // 【审计 V16】字号与列宽都按可用宽度自适应：640 宽最小窗落 13/11/11/8 档，
    // 9 列先按最宽内容保底再分余量，不再靠列裁剪把数字切一半。
    let metrics = TableMetrics::for_width(inner);
    let col_w = column_widths(inner, metrics);
    let mut col_x = [0f32; 9];
    let mut cx = left;
    for i in 0..9 {
        col_x[i] = cx;
        cx += col_w[i];
    }
    let head_base = y + BAR_H + HEAD_BASELINE;
    for (i, spec) in COLUMNS.iter().enumerate() {
        let slot = header_slots(col_x[i] + col_w[i], col_w[i], spec, metrics);
        f.push(DrawCmd::PushClip {
            x: col_x[i] + 1.,
            y: y + BAR_H + 4.,
            w: (col_w[i] - 2.).max(1.),
            h: HEAD_H - 6.,
        });
        // 列名次级文字色、单位再小一号用更次一级文字色：列头本身就有层级。
        right(
            f,
            slot.label_right,
            baseline_text_y(head_base, metrics.head),
            metrics.head,
            spec.label,
            p.text2,
        );
        if let Some(ur) = slot.unit_right {
            right(
                f,
                ur,
                baseline_text_y(head_base, metrics.unit),
                metrics.unit,
                spec.unit,
                p.muted,
            );
        }
        f.push(DrawCmd::PopClip);
    }
    let list_top = y + BAR_H + HEAD_H;
    let list_h = ph - BAR_H - HEAD_H - PAD_BOTTOM;
    let max = scroll_max(cfg, m, h);
    let scroll = m.ledger_scroll.clamp(0., max);
    f.push(DrawCmd::PushClip {
        x,
        y: list_top,
        w: pw,
        h: list_h.max(1.),
    });
    let mut row_y = list_top - scroll;
    for idx in (0..m.history.len()).rev() {
        if row_y + ROW_H > list_top && row_y < list_top + list_h {
            let r = &m.history[idx];
            let latest = idx + 1 == m.history.len();
            if latest {
                // 【审计 V16】当前记录同时用行底、左缘标记、文字三处表示：
                // 只靠 7% 的行底在深色皮肤上几乎看不出来。
                rect(f, x + 16., row_y, pw - 32., ROW_H, alpha(p.accent, 0.07));
                rect(f, x + 10., row_y + 6., 2., ROW_H - 12., alpha(p.accent, 0.85));
            }
            let cells = cell_text(r);
            for (i, spec) in COLUMNS.iter().enumerate() {
                // 【审计 V16】主数值（用时/速度）主字号＋主色；次级指标降一号、
                // 次级文字色；元信息（重打/段号）最次一级、仍右对齐。
                // 基线固定 ROW_BASELINE，字号不同也共线，行高仍为 ROW_H。
                let size = metrics.size_of(spec.role);
                let color = match spec.role {
                    CellRole::Primary => p.text,
                    CellRole::Secondary => p.text2,
                    CellRole::Meta if latest => p.accent,
                    CellRole::Meta => p.muted,
                };
                f.push(DrawCmd::PushClip {
                    x: col_x[i] + 2.,
                    y: row_y + 2.,
                    w: (col_w[i] - 4.).max(1.),
                    h: ROW_H - 4.,
                });
                f.push(DrawCmd::TextRun {
                    x: col_x[i] + col_w[i] - CELL_PAD,
                    y: baseline_text_y(row_y + ROW_BASELINE, size),
                    h: size,
                    text: cells[i].clone(),
                    color,
                    font: FontFamily::Mono,
                    align: Align::Right,
                });
                f.push(DrawCmd::PopClip);
            }
            line(f, x + 16., row_y + ROW_H - 1., pw - 32., alpha(p.line, 0.4));
        }
        row_y += ROW_H;
    }
    f.push(DrawCmd::PopClip);
    if max > 0. {
        let track = list_h.max(1.);
        let content = m.history.len() as f32 * ROW_H;
        let thumb = (track * track / content.max(1.)).clamp(18., track);
        rect(f, x + pw - 8., list_top, 4., track, alpha(p.text, 0.06));
        rect(
            f,
            x + pw - 8.,
            list_top + scroll / max * (track - thumb),
            4.,
            thumb,
            alpha(p.accent, 0.65),
        );
    }
}

fn fmt_speed(r: &ScoreRecord) -> String {
    if r.speed.is_finite() && r.speed >= 0. && r.speed <= 1000. {
        format!("{:.1}", r.speed)
    } else {
        "—".into()
    }
}

fn duration(ms: u64) -> String {
    let secs = ms as f64 / 1000.;
    format!("{}:{:04.1}", (secs / 60.) as u64, secs % 60.)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gale_core::config::DisplayMode;

    /// 【2026-11-13】成绩面板/摘要条的左右边界必须与对照模式两张卡对齐：
    /// 非极简 36px 包边、极简 16px；字贴模式保持 24px 内距。
    #[test]
    fn ledger_panel_aligns_with_split_cards_in_both_chrome_modes() {
        let m = Model {
            ledger_open: true,
            ..Default::default()
        };
        let w = 1300.;
        for minimal in [false, true] {
            let mut cfg = Config::default();
            cfg.display_mode = DisplayMode::Split;
            cfg.minimal = minimal;
            let (x, _, pw, _) = panel_rect_for(&cfg, &m, w, 800.);
            let bw = if minimal {
                (w - 56.).max(360.).min(1600.)
            } else {
                (w - 96.).max(360.).min(1600.)
            };
            let card_x = (w - bw) / 2. - 12.;
            assert!((x - card_x).abs() < 0.01, "panel x {x} vs card {card_x}");
            assert!((pw - (bw + 24.)).abs() < 0.01, "panel w {pw}");
        }
        let cfg = Config::default();
        let (x, _, pw, _) = panel_rect_for(&cfg, &m, w, 800.);
        assert_eq!(x, 24.);
        assert_eq!(pw, w - 48.);
        // 摘要条与面板同界
        let (bx, _, bwidth, _) = bar_rect(&cfg, w, 800.);
        assert_eq!(bx, 24.);
        assert_eq!(bwidth, w - 48.);
        let mut split = Config::default();
        split.display_mode = DisplayMode::Split;
        let (bx, _, bwidth, _) = bar_rect(&split, w, 800.);
        assert_eq!(bx, 36.);
        assert_eq!(bwidth, w - 72.);
    }

    /// 【审计 V16】640 宽最小窗（inner 536/560）下 9 列都容得下最宽内容：
    /// 字号落 13/11/11/8 档，列宽合计等于可用宽度，表头单位不被丢弃。
    #[test]
    fn table_metrics_and_columns_fit_at_six_forty() {
        // 640 宽窗的三种横向布局都落在同一个字号档：
        // 字贴 pw = 640-48 = 592 → inner 560；对照非极简 pw = (640-96)+24 = 568 →
        // inner 536；对照极简 pw = (640-56)+24 = 608 → inner 576。
        for inner in [536f32, 560., 576.] {
            let m = TableMetrics::for_width(inner);
            assert_eq!(
                (m.primary, m.cell, m.head, m.unit),
                (13., 11., 11., 8.),
                "inner {inner} 应落 13/11/11/8 档"
            );
            let widths = column_widths(inner, m);
            let sum: f32 = widths.iter().sum();
            assert!((sum - inner).abs() < 0.01, "列宽合计 {sum} != inner {inner}");
            let mut cx = 16f32;
            for (i, spec) in COLUMNS.iter().enumerate() {
                // 最宽内容 + 左右内距必须装得下（右侧对到 CELL_PAD，左侧留 2px）。
                let need = mono_width(WORST_CELLS[i], m.size_of(spec.role));
                assert!(
                    need + 8. <= widths[i] + 0.01,
                    "列 {i} 最宽内容 {need} 超列宽 {}",
                    widths[i]
                );
                let slot = header_slots(cx + widths[i], widths[i], spec, m);
                assert!(
                    slot.label_right - ui_text_width(spec.label, m.head) >= cx + 2.,
                    "列 {i} 列名左侧溢出"
                );
                if !spec.unit.is_empty() {
                    let ur = slot.unit_right.expect("640 宽下表头单位不应被丢弃");
                    // 单位紧贴列名：列名右缘 + HEAD_UNIT_GAP 就是单位左缘。
                    assert!(
                        (ur - ui_text_width(spec.unit, m.unit)
                            - (slot.label_right + HEAD_UNIT_GAP))
                            .abs()
                            < 0.01
                    );
                }
                cx += widths[i];
            }
        }
        // 更宽的窗口只放大字号，不会反而缩小。
        assert!(TableMetrics::for_width(1220.).primary >= TableMetrics::for_width(560.).primary);
        assert_eq!(TableMetrics::for_width(480.).primary, 12.);
    }

    /// 【审计 V16】三档字号共用同一条行内基线：字号越大盒顶越靠上、基线不动，
    /// 且都在 ROW_H 之内——行高不因字号调整而溢出、同排不出现基线错位。
    #[test]
    fn cell_fonts_share_one_baseline_inside_row_height() {
        let m = TableMetrics::for_width(560.);
        let baseline = ROW_BASELINE;
        for size in [m.primary, m.cell, m.head, m.unit] {
            let y = baseline_text_y(baseline, size);
            assert!((y + size * BASELINE_RATIO - baseline).abs() < 0.01);
            assert!(y > 0.);
            assert!(
                y + size * 1.4 <= ROW_H,
                "size {size} 文本盒底 {} 超出 ROW_H",
                y + size * 1.4
            );
        }
        assert!(baseline_text_y(baseline, m.primary) < baseline_text_y(baseline, m.cell));
    }

    /// 【审计 V20】「数值 + 单位」成组：单位与数值同基线、间隙 3px，
    /// 组间固定 12px——组内不散、组间可分，组宽可用于右对齐。
    #[test]
    fn value_unit_groups_share_baseline_and_constant_gap() {
        let vu = ValueUnit {
            value: 16.,
            unit: 10.,
        };
        let w = vu.group_width("57.3", "字/分");
        let expect = mono_width("57.3", 16.) + ValueUnit::UNIT_GAP + ui_text_width("字/分", 10.);
        assert!((w - expect).abs() < 0.01);
        assert!(ValueUnit::UNIT_GAP < STAT_GROUP_GAP / 3.);
        assert!((vu.value_y(30.) + 16. * BASELINE_RATIO - 30.).abs() < 0.01);
        assert!((vu.unit_y(30.) + 10. * BASELINE_RATIO - 30.).abs() < 0.01);
        // 空单位不占单位间隙。
        assert!((vu.group_width("3", "") - mono_width("3", 16.)).abs() < 0.01);
    }

    /// 【审计 V20】640 宽表头摘要（含完成态「下一段 + 重打」按钮占位 272px）
    /// 三组仍放得下；四位速度这种极端值挤不下时先让出「本轮段数」，
    /// 平均速度组与条数组保留，组间仍是同一个 STAT_GROUP_GAP。
    #[test]
    fn ledger_summary_group_plan_keeps_average_at_six_forty() {
        let meta = ValueUnit {
            value: 12.,
            unit: 10.,
        };
        let main = ValueUnit {
            value: 16.,
            unit: 10.,
        };
        let available = 592. - 112. - 272.;
        let w_count = meta.group_width("12", "条");
        let w_done = ui_text_width("本轮", 11.) + STAT_LABEL_GAP + meta.group_width("3", "段");
        let w_avg = ui_text_width("平均", 11.) + STAT_LABEL_GAP + main.group_width("57.3", "字/分");
        assert_eq!(
            summary_group_plan(available, w_count, w_done, w_avg),
            (true, true, true)
        );
        assert!(w_count + w_done + w_avg + 2. * STAT_GROUP_GAP <= available + 0.01);
        // 四位条数 + 四位速度：三组超宽，丢「本轮段数」，主结果仍在。
        let w_count4 = meta.group_width("1234", "条");
        let w_avg4 = ui_text_width("平均", 11.) + STAT_LABEL_GAP + main.group_width("1234.5", "字/分");
        let plan = summary_group_plan(available, w_count4, w_done, w_avg4);
        assert!(plan.2, "平均速度组必须保留");
        assert!(!plan.1, "宽度不够时「本轮段数」先让路");
        assert!(w_count4 + w_avg4 + STAT_GROUP_GAP <= available + 0.01);
        // 标题「成绩记录」4 字 15px 结束于 x+75，摘要从 x+96 起，不重叠。
        assert!(96. > ui_text_width("成绩记录", 15.) + 16.);
    }
}
