//! 报告系统 —— 日/周/月汇总、趋势、慢字/错字统计。
//!
//! 数据源：统计/records.jsonl（ScoreStore）。全部为纯计算，输出结构化数据供 UI 渲染。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::score::ScoreRecord;

/// One result slot per segment; speed is a ratio of totals, never an average of speeds.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RaceSummary {
    pub segments: usize,
    pub completed_segments: usize,
    pub chars: u64,
    pub keys: u64,
    pub wrong: u64,
    pub backspaces: u64,
    pub corrections: u64,
    pub time_ms: u64,
    pub speed: f64,
    pub effective_speed: f64,
    pub strokes_per_sec: f64,
    pub code_len: f64,
    pub accuracy: f64,
}
impl RaceSummary {
    pub fn from_records<'a>(records: impl IntoIterator<Item = &'a ScoreRecord>) -> Self {
        let mut out = Self::default();
        for r in records {
            out.segments += 1;
            out.completed_segments += usize::from(r.round.as_ref().is_none_or(|m| m.complete));
            out.chars += r.chars as u64;
            out.keys += r.keys as u64;
            out.wrong += r.wrong as u64;
            out.backspaces += r.backspaces as u64;
            out.corrections += r.corrections as u64;
            out.time_ms += r.time_ms;
        }
        let minutes = out.time_ms as f64 / 60000.0;
        let keys = out.keys as f64;
        let chars = out.chars as f64;
        let corr = out.corrections as f64;
        if minutes > 0.0 {
            out.speed = chars / minutes;
            out.effective_speed = (chars - out.wrong as f64 * 5.0).max(0.0) / minutes;
            out.strokes_per_sec = keys / (minutes * 60.0);
        }
        if chars > 0.0 {
            out.code_len = keys / chars;
        }
        out.accuracy = if keys > 0.0 && chars + corr > 0.0 {
            ((keys - corr - out.backspaces as f64 * 2.0) / (chars + corr) * chars / keys * 100.0)
                .clamp(0.0, 100.0)
        } else {
            100.0
        };
        out
    }
}

/// 一天的汇总
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DaySummary {
    pub date: String,  // YYYY-MM-DD
    pub chars: u64,    // 字数
    pub segments: u64, // 段数
    pub keys: u64,
    pub time_ms: u64,
    pub avg_speed: f64,    // 总字数 / 总用时
    pub avg_accuracy: f64, // 加权平均键准
    pub max_speed: f64,    // 当日最快一段
    pub wrong: u64,
    pub corrections: u64,
}

/// 错字频率表条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrongCharStat {
    pub ch: String,
    pub count: u64,
    /// 出现的原文上下文（前2后2）
    pub contexts: Vec<String>,
}

/// 汇总区间
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    Day,
    Week,
    Month,
    All,
}

/// 按日期聚合全部记录
pub fn daily_summaries(records: &[ScoreRecord]) -> Vec<DaySummary> {
    let mut by_date: BTreeMap<String, DaySummary> = BTreeMap::new();
    let mut accuracy_keys: BTreeMap<String, f64> = BTreeMap::new();
    for r in records {
        let e = by_date.entry(r.date.clone()).or_insert_with(|| DaySummary {
            date: r.date.clone(),
            ..Default::default()
        });
        e.chars += r.chars as u64;
        e.segments += 1;
        e.keys += r.keys as u64;
        e.time_ms += r.time_ms;
        e.wrong += r.wrong as u64;
        e.corrections += r.corrections as u64;
        e.max_speed = e.max_speed.max(r.speed);
        *accuracy_keys.entry(r.date.clone()).or_default() += r.accuracy * r.keys as f64;
    }
    by_date
        .into_values()
        .map(|mut d| {
            d.avg_speed = if d.time_ms > 0 {
                d.chars as f64 * 60000.0 / d.time_ms as f64
            } else {
                0.0
            };
            d.avg_accuracy = if d.keys > 0 {
                accuracy_keys.get(&d.date).copied().unwrap_or(0.0) / d.keys as f64
            } else {
                100.0
            };
            d
        })
        .collect()
}

/// 区间汇总（截至最新记录日期）
pub fn summarize(records: &[ScoreRecord], range: Range) -> DaySummary {
    let all = daily_summaries(records);
    let last_date = all.last().map(|d| d.date.clone()).unwrap_or_default();
    let cutoff = date_shift(&last_date, -(range_days(range).saturating_sub(1) as i64));
    let sel: Vec<&DaySummary> = all.iter().filter(|d| d.date >= cutoff).collect();
    let mut total = DaySummary {
        date: format!("近{}天", range_days(range)),
        ..Default::default()
    };
    for d in &sel {
        total.chars += d.chars;
        total.segments += d.segments;
        total.keys += d.keys;
        total.time_ms += d.time_ms;
        total.wrong += d.wrong;
        total.corrections += d.corrections;
        total.max_speed = total.max_speed.max(d.max_speed);
    }
    total.avg_speed = if total.time_ms > 0 {
        total.chars as f64 * 60000.0 / total.time_ms as f64
    } else {
        0.0
    };
    total.avg_accuracy = if total.keys > 0 {
        sel.iter()
            .map(|d| d.avg_accuracy * d.keys as f64)
            .sum::<f64>()
            / total.keys as f64
    } else {
        100.0
    };
    total
}

fn range_days(r: Range) -> u32 {
    match r {
        Range::Day => 1,
        Range::Week => 7,
        Range::Month => 30,
        Range::All => u32::MAX / 8,
    }
}

/// 日期平移（YYYY-MM-DD ± days）—— 纯逻辑 civil 换算
pub fn date_shift(date: &str, days: i64) -> String {
    let parts: Vec<i64> = date.split('-').filter_map(|p| p.parse().ok()).collect();
    if parts.len() != 3 {
        return date.to_string();
    }
    let (y, m, d) = (parts[0], parts[1], parts[2]);
    let days_epoch = days_from_civil(y, m, d) + days;
    let (ny, nm, nd) = civil_from_days(days_epoch);
    format!("{ny:04}-{nm:02}-{nd:02}")
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 错字频率（需要原文才能知道错在哪个字上——用记录里的 wrong + 原文快照；
/// 这里按记录聚合粒度统计"哪段错得多"，字符级由 UI 层在打字过程中收集）
pub fn worst_records(records: &[ScoreRecord], top: usize) -> Vec<&ScoreRecord> {
    let mut v: Vec<&ScoreRecord> = records.iter().collect();
    v.sort_by(|a, b| b.wrong.cmp(&a.wrong).then(b.time_ms.cmp(&a.time_ms)));
    v.truncate(top);
    v
}

// ─────────────────────────────────────────────────────────────
// 州州增值统计（`_research/stats-and-drill-models.md` §3.2 直译）
// ─────────────────────────────────────────────────────────────

/// 击键九级评定（历史全量 jj 序列 → JjRating）。
/// 展示文案对齐州州："基准 X 键/秒，达标占比 Y%"。
pub fn jj_rating_of_records(records: &[ScoreRecord]) -> crate::stats::JjRating {
    let jjs: Vec<f64> = records.iter().map(|r| r.jj).filter(|&j| j > 0.0).collect();
    crate::stats::jj_rating(&jjs)
}

/// 慢字 TopN（字符级；数据底座 = 逐字时长，由 UI 层在打字时收集
/// `Vec<(字, 该字本次上屏用时 ms)>` 后调用本聚合）。
/// 口径：同一字多次出现取平均，按平均时长倒序取 top。
pub struct SlowCharEntry {
    pub ch: String,
    pub avg_ms: f64,
    pub count: usize,
}

pub fn slow_chars_top(per_char: &[(String, u64)], top: usize) -> Vec<SlowCharEntry> {
    let mut acc: std::collections::HashMap<&str, (f64, usize)> = std::collections::HashMap::new();
    for (ch, ms) in per_char {
        let e = acc.entry(ch.as_str()).or_default();
        e.0 += *ms as f64;
        e.1 += 1;
    }
    let mut v: Vec<SlowCharEntry> = acc
        .into_iter()
        .map(|(ch, (sum, n))| SlowCharEntry {
            ch: ch.to_string(),
            avg_ms: sum / n as f64,
            count: n,
        })
        .collect();
    v.sort_by(|a, b| b.avg_ms.total_cmp(&a.avg_ms).then(a.ch.cmp(&b.ch)));
    v.truncate(top);
    v
}

/// 字数曲线（按日期累计字数，画进度用）
pub fn chars_curve(records: &[ScoreRecord]) -> Vec<(String, u64)> {
    let daily = daily_summaries(records);
    let mut out: Vec<(String, u64)> = Vec::with_capacity(daily.len());
    let mut acc = 0u64;
    for d in daily {
        acc += d.chars;
        out.push((d.date.clone(), acc));
    }
    out
}

/// 区间时间四分解合计（秒；把各段的 tb 相加——州州是单段视图，这里是跨段汇总）
pub fn total_time_breakdown(records: &[ScoreRecord]) -> crate::stats::TimeBreakdown {
    let mut t = crate::stats::TimeBreakdown::default();
    for r in records {
        t.avg_pos += r.time_breakdown.avg_pos;
        t.t_normal += r.time_breakdown.t_normal;
        t.t_stall += r.time_breakdown.t_stall;
        t.t_hg += r.time_breakdown.t_hg;
        t.t_err += r.time_breakdown.t_err;
        t.ideal_speed = t.ideal_speed.max(r.time_breakdown.ideal_speed);
    }
    t
}

/// 文本报告（人可直接读，也写入 报告/ 目录）
pub fn render_text_report(records: &[ScoreRecord], range: Range) -> String {
    let s = summarize(records, range);
    let daily = daily_summaries(records);
    let days = daily.len();
    let mut out = String::new();
    out.push_str(&format!("==== 打打你的 打字报告（{}）====\n", s.date));
    out.push_str(&format!(
        "字数 {} 字 | 段数 {} 段 | 用时 {} 分\n",
        s.chars,
        s.segments,
        s.time_ms / 60000
    ));
    out.push_str(&format!(
        "平均速度 {:.2} 字/分 | 最快 {:.2} 字/分 | 平均键准 {:.2}%\n",
        s.avg_speed, s.max_speed, s.avg_accuracy
    ));
    out.push_str(&format!("错字 {} | 回改 {}\n", s.wrong, s.corrections));
    out.push_str(&format!("打卡 {} 天\n", days));
    if !daily.is_empty() {
        out.push_str("\n最近 7 天：\n");
        for d in daily.iter().rev().take(7) {
            out.push_str(&format!(
                "  {} | {} 字 {} 段 | 均 {:.1} 字/分\n",
                d.date, d.chars, d.segments, d.avg_speed
            ));
        }
    }
    // 州州增值：时间四分解合计 + jjC 九级（旧记录无 tb/jj 数据时自动省略这两节）
    let tb = total_time_breakdown(records);
    if tb.t_normal + tb.t_stall + tb.t_hg > 0.0 {
        out.push_str("\n时间分解（州州口径）：\n");
        out.push_str(&format!(
            "  正常 {:.0}s | 卡顿 {:.0}s | 回改 {:.0}s | 错罚 {:.0}s\n",
            tb.t_normal, tb.t_stall, tb.t_hg, tb.t_err
        ));
        out.push_str(&format!(
            "  理想速度 {:.1} 字/分（按正常段速度打完有效字数的上界，封顶999）\n",
            tb.ideal_speed
        ));
    }
    let jr = jj_rating_of_records(records);
    if let Some(c) = jr.jj_c {
        out.push_str(&format!(
            "\n击键评定（州州口径）：基准 {} 键/秒，达标占比 {:.1}%\n",
            c + 4,
            jr.jj_c_up * 100.0
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::ScoreRecord;

    fn rec(date: &str, chars: usize, speed: f64, wrong: usize) -> ScoreRecord {
        ScoreRecord {
            round: None,
            ts: 0,
            date: date.into(),
            title: "t".into(),
            source: "群文".into(),
            mark: "1".into(),
            para_index: 0,
            chars,
            keys: chars as u32 + 10,
            backspaces: 3,
            wrong,
            time_ms: 60000,
            speed,
            accuracy: 95.0,
            code_len: 1.1,
            corrections: 1,
            effective_speed: speed * 0.95,
            strokes_per_sec: 5.0,
            composing_keys: 0,
            accuracy_pro: 95.0,
            waste_codes: 0,
            commit_groups: 0,
            input_method: String::new(),
            time_breakdown: crate::stats::TimeBreakdown::default(),
            jj: 5.0,
            version: "2".into(),
        }
    }

    #[test]
    fn daily_and_range() {
        let records = vec![
            rec("2026-09-27", 100, 150.0, 3),
            rec("2026-09-28", 200, 170.0, 1),
            rec("2026-09-28", 100, 190.0, 0),
        ];
        let daily = daily_summaries(&records);
        assert_eq!(daily.len(), 2);
        assert_eq!(daily[1].chars, 300);
        assert!((daily[1].avg_speed - 150.0).abs() < 0.001); // 300 chars / 2 minutes
        assert!((daily[1].max_speed - 190.0).abs() < 0.001);

        let week = summarize(&records, Range::Week);
        assert_eq!(week.chars, 400);
        assert_eq!(week.segments, 3);
        assert!(week.max_speed >= 190.0);
    }

    #[test]
    fn race_speed_uses_total_time_and_day_range_is_inclusive() {
        let mut a = rec("2026-11-08", 100, 100.0, 0);
        a.time_ms = 60000;
        let mut b = rec("2026-11-08", 100, 200.0, 1);
        b.time_ms = 30000;
        let summary = RaceSummary::from_records([&a, &b]);
        assert!((summary.speed - 133.333333).abs() < 0.0001);
        assert!((summary.effective_speed - 130.0).abs() < 0.0001);
        let old = rec("2026-11-07", 100, 100.0, 0);
        assert_eq!(summarize(&[a, b, old], Range::Day).chars, 200);
    }

    #[test]
    fn date_shift_math() {
        assert_eq!(date_shift("2026-09-28", -7), "2026-09-21");
        assert_eq!(date_shift("2026-03-01", -1), "2026-02-28");
        assert_eq!(date_shift("2024-03-01", -1), "2024-02-29");
        assert_eq!(date_shift("2026-01-01", -1), "2025-12-31");
    }

    #[test]
    fn text_report_renders() {
        let records = vec![rec("2026-09-28", 200, 170.0, 1)];
        let text = render_text_report(&records, Range::Month);
        assert!(text.contains("打打你的 打字报告"));
        assert!(text.contains("平均速度"));
        // 旧记录无 tb 数据：时间分解节自动省略（jj=5.0 由 rec 填充 → 击键评定节保留）
        assert!(!text.contains("时间分解"));
        let mut old_style = rec("2026-09-28", 200, 170.0, 1);
        old_style.jj = 0.0; // 旧记录无 jj（serde default 前）→ 评定节省略
        let text2 = render_text_report(&[old_style], Range::Month);
        assert!(!text2.contains("击键评定"));
    }

    #[test]
    fn text_report_with_zhouzhou_sections() {
        let mut r = rec("2026-09-28", 200, 170.0, 1);
        r.time_breakdown = crate::stats::TimeBreakdown {
            avg_pos: 0.3,
            t_normal: 45.0,
            t_stall: 8.0,
            t_hg: 5.0,
            t_err: 2.4,
            ideal_speed: 231.0,
        };
        r.jj = 7.2;
        let text = render_text_report(&[r], Range::Month);
        assert!(text.contains("时间分解（州州口径）"));
        assert!(text.contains("正常 45s"));
        assert!(text.contains("卡顿 8s"));
        assert!(text.contains("回改 5s"));
        assert!(text.contains("错罚 2s"));
        assert!(text.contains("理想速度 231.0 字/分"));
        assert!(text.contains("击键评定（州州口径）：基准 7 键/秒，达标占比"));
    }

    // ── 州州四分解/jjC/慢字/曲线（公式级测试）──

    #[test]
    fn breakdown_formula_zhouzhou() {
        use crate::compare::TimingEvent;
        // 手算样例：5 个正向事件 dt_ms = [300,300,300,2000,300]，1 个回退 [500]
        // avgPos = (0.3×3+2.0+0.3)/5 = 0.64；阈值 = 1.92
        // → 2000ms 卡顿（2.0 > 1.92）；tNormal = 0.3×4 = 1.2；tStall = 2.0；tHg = 0.5
        // 10 字 2 错：tErr = 2×0.64×4 = 5.12；ideal = 8×60/1.2 = 400
        // 注：州州阈值是"≤avgPos×3 算正常"（含等号）——1500ms 在 avg=0.54 时是 1.5≤1.62 归正常
        let ev = vec![
            TimingEvent {
                len_delta: 1,
                dt_ms: 300,
            },
            TimingEvent {
                len_delta: 1,
                dt_ms: 300,
            },
            TimingEvent {
                len_delta: 1,
                dt_ms: 300,
            },
            TimingEvent {
                len_delta: 1,
                dt_ms: 2000,
            },
            TimingEvent {
                len_delta: 1,
                dt_ms: 300,
            },
            TimingEvent {
                len_delta: -1,
                dt_ms: 500,
            },
        ];
        let tb = crate::stats::time_breakdown(&ev, 10, 2);
        assert!((tb.avg_pos - 0.64).abs() < 1e-9, "avg={}", tb.avg_pos);
        assert!((tb.t_normal - 1.2).abs() < 1e-9, "normal={}", tb.t_normal);
        assert!((tb.t_stall - 2.0).abs() < 1e-9, "stall={}", tb.t_stall);
        assert!((tb.t_hg - 0.5).abs() < 1e-9, "hg={}", tb.t_hg);
        assert!((tb.t_err - 5.12).abs() < 1e-9, "err={}", tb.t_err);
        assert!(
            (tb.ideal_speed - 400.0).abs() < 1e-9,
            "ideal={}",
            tb.ideal_speed
        );
        // 边界：dt 恰等于阈值 → 归正常（≤ 含等号）
        let ev_b = vec![
            TimingEvent {
                len_delta: 1,
                dt_ms: 1000,
            },
            TimingEvent {
                len_delta: 1,
                dt_ms: 2000,
            },
            TimingEvent {
                len_delta: 1,
                dt_ms: 1000,
            },
        ];
        // avg = 4/3 s；阈值 = 4 s；2000ms < 4s → 全正常
        let tb_b = crate::stats::time_breakdown(&ev_b, 3, 0);
        assert_eq!(tb_b.t_stall, 0.0);
        assert!((tb_b.t_normal - 4.0).abs() < 1e-9);
        // 空事件流 → 全零安全退化
        let z = crate::stats::time_breakdown(&[], 10, 2);
        assert_eq!(z.avg_pos, 0.0);
        assert_eq!(z.ideal_speed, 0.0);
    }

    #[test]
    fn jj_rating_nine_buckets() {
        // 州州口径：桶 = clamp(floor(jj)−4, 0, 8)；jjC = 首个占比≥10%；jjC_ = 累计
        // 样例：10 段历史 jj = [7.2, 7.5, 7.9, 6.1, 6.3, 8.4, 9.0, 3.2, 12.5, 5.0]
        // 桶：7.2/7.5/7.9→3；6.1/6.3→2；8.4/9.0→4/4；3.2→0；12.5→8；5.0→1
        let jjs = [7.2, 7.5, 7.9, 6.1, 6.3, 8.4, 9.0, 3.2, 12.5, 5.0];
        let r = crate::stats::jj_rating(&jjs);
        assert_eq!(r.pcts.len(), 9);
        assert!((r.pcts[0] - 0.1).abs() < 1e-9); // 3.2 → 桶0
        assert!((r.pcts[2] - 0.2).abs() < 1e-9); // 6.1,6.3 → 桶2
        assert!((r.pcts[3] - 0.3).abs() < 1e-9); // 7.2,7.5,7.9 → 桶3
        assert!((r.pcts[8] - 0.1).abs() < 1e-9); // 12.5 → 桶8
                                                 // 首个 ≥10% 是桶0（3.2 一段恰 10%）→ jjC=0（即"基准 4 键/秒"）
        assert_eq!(r.jj_c, Some(0));
        assert!((r.jj_c_up - 1.0).abs() < 1e-9); // 全部 ≥桶0
                                                 // 去掉 3.2 后：首个 ≥10% 变桶1 → jjC=1
        let jjs2 = [7.2, 7.5, 7.9, 6.1, 6.3, 8.4, 9.0, 12.5, 5.0];
        let r2 = crate::stats::jj_rating(&jjs2);
        assert_eq!(r2.jj_c, Some(1));
        // 空历史 → None
        assert_eq!(crate::stats::jj_rating(&[]).jj_c, None);
        // jj<4 全并入桶0；≥12 全并入桶8（clamp 边界）
        let r3 = crate::stats::jj_rating(&[2.0, 2.5, 3.9]);
        assert_eq!(r3.jj_c, Some(0));
        let r4 = crate::stats::jj_rating(&[15.0, 20.0]);
        assert_eq!(r4.jj_c, Some(8));
        assert!((r4.jj_c_up - 1.0).abs() < 1e-9);
    }

    #[test]
    fn zz_compute_speeds() {
        // 州州 ComputeStats：speed=(len−Cz)×60/sec；speed2=max(0,len−Cz×5)×60/sec；mc=Keys/(len−Cz)
        let (s, s2, mc) = crate::stats::zz_compute(100, 4, 250, 50.0);
        assert!((s - 96.0 * 60.0 / 50.0).abs() < 1e-9);
        assert!((s2 - 80.0 * 60.0 / 50.0).abs() < 1e-9);
        assert!((mc - 250.0 / 96.0).abs() < 1e-9);
        // 错太多 → speed2 钳 0，mc 分母为 0 → 0
        let (s2b, _mcb) = {
            let a = crate::stats::zz_compute(10, 9, 30, 10.0);
            (a.1, a.2)
        };
        assert_eq!(s2b, 0.0);
        // 10−9×5=−35 → 0
        let (_, _, mc_c) = crate::stats::zz_compute(10, 9, 30, 10.0);
        assert!((mc_c - 30.0 / 1.0).abs() < 1e-9); // 分母 len−Cz=1
    }

    #[test]
    fn slow_chars_and_curve() {
        // 慢字：同字多见取平均，按平均时长倒序
        let per_char = vec![
            ("的".to_string(), 300),
            ("的".to_string(), 500),
            ("疆".to_string(), 900),
            ("一".to_string(), 200),
        ];
        let top = slow_chars_top(&per_char, 2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].ch, "疆");
        assert!((top[0].avg_ms - 900.0).abs() < 1e-9);
        assert_eq!(top[1].ch, "的");
        assert!((top[1].avg_ms - 400.0).abs() < 1e-9);
        // 字数曲线：累计
        let records = vec![
            rec("2026-09-27", 100, 150.0, 0),
            rec("2026-09-28", 200, 170.0, 0),
            rec("2026-09-29", 50, 160.0, 0),
        ];
        let curve = chars_curve(&records);
        assert_eq!(curve.len(), 3);
        assert_eq!(curve[0], ("2026-09-27".to_string(), 100));
        assert_eq!(curve[2], ("2026-09-29".to_string(), 350));
    }

    #[test]
    fn timing_events_collected_by_comparer() {
        // Comparer.feed 自动维护时间事件流（州州 Report 同构）
        use crate::compare::{Comparer, InputEvent};
        let mut c = Comparer::new("甲乙丙丁");
        c.feed(
            &InputEvent::Commit {
                chars: "甲".into()
            },
            100,
        );
        c.feed(
            &InputEvent::Commit {
                chars: "乙".into()
            },
            400,
        );
        c.feed(&InputEvent::Backspace, 900);
        c.feed(
            &InputEvent::Commit {
                chars: "乙丙".into(),
            },
            1200,
        );
        c.feed(
            &InputEvent::Commit {
                chars: "丁".into()
            },
            1500,
        );
        let ev = &c.timing_events;
        assert_eq!(ev.len(), 5);
        // 首事件 dt 锚 started_at（=首 feed 的 100）→ 0
        assert_eq!(ev[0].len_delta, 1);
        assert_eq!(ev[0].dt_ms, 0);
        assert_eq!(ev[1].len_delta, 1);
        assert_eq!(ev[1].dt_ms, 300);
        assert_eq!(ev[2].len_delta, -1);
        assert_eq!(ev[2].dt_ms, 500);
        assert_eq!(ev[3].len_delta, 2); // 整句两字一次 commit
        assert_eq!(ev[3].dt_ms, 300);
        assert_eq!(ev[4].dt_ms, 300);
        // finalize 出的 tb 用这份事件流
        let st = crate::stats::StatsEngine::new().finalize(&c, 1500);
        assert!(st.time_breakdown.t_normal > 0.0 || st.time_breakdown.t_stall > 0.0);
        assert!(st.time_breakdown.t_hg > 0.0);
    }

    #[test]
    fn timed_key_channel_and_degradation() {
        use crate::stats::StatsEngine;
        // 新通道：per_key 记 (vk, dt)
        let mut st = StatsEngine::new();
        st.on_key_timed(0x41, false, 1000);
        st.on_key_timed(0x42, false, 1180);
        st.on_key_timed(0x43, true, 1450);
        assert_eq!(st.keys, 3);
        assert_eq!(st.composing_keys, 1);
        assert_eq!(st.per_key.len(), 2); // 首键无前驱不计
        assert_eq!(st.per_key[0], (0x42, 180));
        assert_eq!(st.per_key[1], (0x43, 270));
        // 旧通道退化：无时间戳 → per_key 空，统计不受影响
        let mut old = StatsEngine::new();
        old.on_key(&crate::stats::KeyEvent {
            vk: 0x41,
            composing: false,
        });
        old.on_key(&crate::stats::KeyEvent {
            vk: 0x42,
            composing: false,
        });
        assert_eq!(old.keys, 2);
        assert!(old.per_key.is_empty());
        // 非统计键不进任何通道
        let mut st2 = StatsEngine::new();
        st2.on_key_timed(0x70, false, 100); // F1
        assert_eq!(st2.keys, 0);
        assert!(st2.per_key.is_empty());
    }
}
