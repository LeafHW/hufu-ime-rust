//! 统计引擎 —— 每一段跟打数据的准确性都在这里。
//!
//! 口径（对齐晴跟打 Pro / 疾风1代，蓝本：`_research/typesunny-stats.md`、jifeng records 字段，
//! 完整对照表见 `docs/stats-formula.md`）：
//! - **字数**  = 当前已比较的 Unicode 标量数（中途停止不计尚未输入的正文）
//! - **总键数（串行口径）** = WM_KEYDOWN 统计键（is_stat_key 区段表：字母/数字/符号/空格/回车/退格全计；
//!   组字中的键同样计入——它们就是用户真实按下的键）
//! - **上屏键组（并击口径）** = 每次 IME commit 记一组（state 2.x，州州跟打器双口径对照），
//!   需要输入宿主在 commit 时调 on_commit_group
//! - **码长**  = 键数 / 字数（保留 3 位小数，如 1.061）
//! - **击键**  = 键数 / 用时秒（键/秒）
//! - **速度**  = 字数 / 用时分钟（字/分）
//! - **键准**（晴 Pro 精确口径，**产线**）= (键 − 回改 − 退格×2) / (字数 + 回改) × 字数 / 键 × 100%
//!   （ScoreRecord.accuracy 直接取该值；`accuracy_simple` 为疾风1代简化对照式，非产线口径）
//! - **回改**  = 物理退格删除已上屏字符的次数，不论此前对错（见 compare.rs）
//! - **错字**  = 比对不匹配的上屏字符数
//! - **有效速度** = 速度 − 错字×5/分钟（晴 Pro "错一罚五" GetValidSpeed 口径，下限 0）
//! - **废码**  = 组字编码被取消的次数；取消时该轮组字全部键数计入退格（晴 Pro 空上屏分支）
//!
//! 疾风1代 records.jsonl 实样（已验证）：
//! `{"chars":196,"keys":208,"backspaces":6,"wrong":2,"time_ms":58285,"speed":201.77,"accuracy":94.71,"code_len":1.0612}`
//! —— 本模块公式与该实测数据完全对齐（见 tests::jifeng_record_reproduced）。

use crate::compare::Comparer;
use serde::{Deserialize, Serialize};

/// 一次击键的原始数据（由输入宿主逐键上报）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyEvent {
    pub vk: u32,
    pub composing: bool, // 是否处于 IME 组字中
}

/// 统计引擎（一段的实时统计器）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsEngine {
    #[serde(default)]
    pub composition_backspaces: u32,
    /// Physical composition Backspaces already included in a cancelled key batch.
    #[serde(default)]
    pub cancelled_backspaces: u32,
    #[serde(default)]
    pub commit_attributions: Vec<CommitAttribution>,
    #[serde(default)]
    pub attributed_keys: u32,
    /// Counted composing keys in the current unsettled physical batch.
    #[serde(skip)]
    pending_composing_keys: u32,
    #[serde(skip)]
    pending_composition_backspaces: u32,
    /// 统计键总数（is_stat_key 通过的）
    pub keys: u32,
    /// 退格键数（= Comparer.backspaces，双通道互验）
    pub backspaces: u32,
    /// 组字中的键数（键数计入总键数，但单独记账）
    pub composing_keys: u32,
    /// 废码次数（组字编码被取消）
    #[serde(default)]
    pub waste_codes: u32,
    /// 被取消组字浪费的键数
    #[serde(default)]
    pub waste_keys: u32,
    /// 上屏键组数（并击口径：每次 IME commit 记一组）
    #[serde(default)]
    pub commit_groups: u32,
    /// 标顶次数（20ms 键-提交配对：提交由顶屏键完成，research-digest §B.5；
    /// gale-app 输入宿主 drain_commit_causes → session 转发注入）。旧 JSON 缺省 0。
    #[serde(default)]
    pub ding_count: u32,
    /// 实际候选选择次数；虎符按选择事件计数，其他输入法保留键-提交配对。
    /// 锁词后延迟上屏不延迟此计数。旧 JSON 缺省 0。
    #[serde(default)]
    pub select_count: u32,
    /// 组字会话起点（累计键数；None=未在组字）——运行态，不入 JSON
    #[serde(skip)]
    pub composition_start_keys: Option<u32>,
    /// 逐键时间戳通道（vk, 距上一统计键毫秒）——键位热力/键间隔分析的数据底座
    /// （`_research/stats-and-drill-models.md` §3.3 record 增值字段 per_key）。
    /// 仅 on_key_timed 通道填充；旧 on_key（无时间戳）退化为空——分析照常，只是没有键间隔。
    #[serde(default)]
    pub per_key: Vec<(u32, u32)>,
    /// 上一统计键的绝对毫秒（per_key dt 锚点；运行态，不入 JSON）
    #[serde(skip)]
    pub last_key_ms: Option<u64>,
}

impl Default for StatsEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl StatsEngine {
    pub fn new() -> Self {
        Self {
            composition_backspaces: 0,
            cancelled_backspaces: 0,
            commit_attributions: Vec::new(),
            attributed_keys: 0,
            pending_composing_keys: 0,
            pending_composition_backspaces: 0,
            keys: 0,
            backspaces: 0,
            composing_keys: 0,
            waste_codes: 0,
            waste_keys: 0,
            commit_groups: 0,
            ding_count: 0,
            select_count: 0,
            composition_start_keys: None,
            per_key: Vec::new(),
            last_key_ms: None,
        }
    }

    /// 一次 IME 上屏（commit）记一组（并击口径）。输入宿主在每次 commit 时调用。
    pub fn on_commit_group(&mut self) {
        self.commit_groups = self.commit_groups.saturating_add(1);
        self.settle_pending_keys();
    }

    /// 20ms 配对账（P1-3 选重/标顶）：标顶一次（顶屏键完成提交）。
    /// 由 main.rs 消费 drain_commit_causes 后经 Session::on_commit_cause 注入。
    pub fn note_ding(&mut self) {
        self.ding_count = self.ding_count.saturating_add(1);
    }
    /// 选重一次（真实候选选择事件，或其他输入法的键-提交配对）。
    pub fn note_select(&mut self) {
        self.select_count = self.select_count.saturating_add(1);
    }

    fn settle_pending_keys(&mut self) {
        self.attributed_keys = self.keys;
        self.pending_composing_keys = 0;
        self.pending_composition_backspaces = 0;
        self.composition_start_keys = None;
    }

    /// One real commit can contain many characters. Attribute the key *batch*, never
    /// invent a key/character mapping for sentence IMEs or concurrent chords.
    pub fn on_commit(&mut self, chars: usize, now_ms: u64) {
        if chars == 0 {
            return;
        }
        let keys = self.keys.saturating_sub(self.attributed_keys);
        self.on_commit_group();
        self.commit_attributions.push(CommitAttribution {
            chars,
            keys,
            at_ms: now_ms,
        });
    }

    /// 组字会话追踪（晴 Pro CompositionStartHit 口径）：
    /// 组字开始时的累计键数，用于编码取消时把整轮键数计入退格。
    /// 由输入宿主在组字开始/上屏/取消时调用。
    pub fn begin_composition(&mut self) {
        if self.composition_start_keys.is_none() {
            // TSF may announce composition only after its first physical key.
            // Recover that observed batch, never guess keys per committed character.
            self.composition_start_keys = Some(self.attributed_keys);
        }
        let pending = self.keys.saturating_sub(self.attributed_keys);
        self.composing_keys = self
            .composing_keys
            .saturating_add(pending.saturating_sub(self.pending_composing_keys));
        self.pending_composing_keys = pending;
    }

    /// 组字正常上屏结束（晴 ClearCompositionState）。
    pub fn end_composition(&mut self) {
        self.composition_start_keys = None;
    }

    /// 组字被取消（空上屏，晴 Pro on_text_input 空串分支）：
    /// 该轮组字的全部键数计入退格 + waste_keys，waste_codes +1。
    pub fn cancel_composition(&mut self) {
        if let Some(start) = self.composition_start_keys.take() {
            let wasted = self.keys.saturating_sub(start.max(self.attributed_keys));
            let overlap = self.pending_composition_backspaces.min(wasted);
            // The cancelling Backspace is both a real composing Backspace and
            // part of the wasted batch. Charge that intersection only once.
            self.backspaces = self
                .backspaces
                .saturating_add(wasted.saturating_sub(overlap));
            self.cancelled_backspaces = self.cancelled_backspaces.saturating_add(overlap);
            self.waste_keys = self.waste_keys.saturating_add(wasted);
            self.waste_codes = self.waste_codes.saturating_add(1);
            self.settle_pending_keys();
        }
    }

    /// 是否为"统计键"：区段表口径（蓝本 jifeng win.rs is_stat_key）。
    /// 字母(0x41-0x5A,0xE5 虎符重映射也算)、数字(0x30-0x39)、符号区段、空格(0x20)、
    /// 回车(0x0D)、退格(0x08)、Tab(0x09)、OEM 键(0xBA-0xBF,0xDB-0xDE,0xC0)、
    /// 小键盘(0x60-0x69,0x6A-0x6F)。非统计键：F1-F12、方向键、PgUp 等控制键、Shift/Ctrl/Win/Alt 修饰键。
    pub fn is_stat_key(vk: u32) -> bool {
        matches!(vk,
            0x20 | 0x08 | 0x09 | 0x0D
            | 0x30..=0x39          // 数字排
            | 0x41..=0x5A          // 字母
            | 0x60..=0x6F          // 小键盘
            | 0xBA..=0xBF          // OEM ;=,-./
            | 0xDB..=0xDE          // OEM [\]'
            | 0xC0                 // OEM `
            | 0xE5                 // 虎符布局层重映射后的字母
        )
    }

    pub fn on_key(&mut self, ev: &KeyEvent) {
        if !Self::is_stat_key(ev.vk) {
            return;
        }
        self.keys = self.keys.saturating_add(1);
        if ev.composing {
            self.composing_keys = self.composing_keys.saturating_add(1);
            self.pending_composing_keys = self.pending_composing_keys.saturating_add(1);
        }
        if ev.vk == 0x08 {
            self.backspaces = self.backspaces.saturating_add(1);
            if ev.composing {
                self.composition_backspaces = self.composition_backspaces.saturating_add(1);
                self.pending_composition_backspaces =
                    self.pending_composition_backspaces.saturating_add(1);
            } else {
                // A physical deletion has no output key batch, even when the
                // caret is already at zero and the committed body stays unchanged.
                self.settle_pending_keys();
            }
        }
    }

    /// 带时间戳的击键通道（输入宿主有键级时间戳时优先用这个）：
    /// 与 on_key 统计口径完全一致，额外记录 (vk, 距上一统计键毫秒) 到 per_key。
    /// 旧通道 on_key（无时间戳）退化为不采集键间隔——统计不受影响，只是 per_key 为空。
    pub fn on_key_timed(&mut self, vk: u32, composing: bool, ts_ms: u64) {
        if !Self::is_stat_key(vk) {
            return;
        }
        if let Some(last) = self.last_key_ms {
            let dt = ts_ms.saturating_sub(last);
            self.per_key.push((vk, dt.min(u32::MAX as u64) as u32));
        }
        self.last_key_ms = Some(ts_ms);
        self.on_key(&KeyEvent { vk, composing });
    }

    /// Count a normally excluded physical key when a real candidate-selection
    /// event proves its use. The choice may only lock part of a sentence, so
    /// this key belongs to the current unsettled batch until commit or cancel.
    /// Ordinary statistical keys already arrived through on_key_timed.
    pub fn on_proven_selection_key_timed(&mut self, vk: u32, ts_ms: u64) -> bool {
        if vk == 0 || vk > 255 || vk == 0x0E || Self::is_stat_key(vk) {
            return false;
        }
        self.begin_composition();
        self.keys = self.keys.saturating_add(1);
        self.composing_keys = self.composing_keys.saturating_add(1);
        self.pending_composing_keys = self.pending_composing_keys.saturating_add(1);
        if let Some(last) = self.last_key_ms {
            self.per_key
                .push((vk, ts_ms.saturating_sub(last).min(u32::MAX as u64) as u32));
            self.last_key_ms = Some(ts_ms.max(last));
        } else {
            self.last_key_ms = Some(ts_ms);
        }
        true
    }

    /// Add a normally excluded key only after the input host proves that it
    /// selected the last committed candidate. The poller emits at most one
    /// commit per document diff, and attribution is drained directly after it.
    /// This repairs that completed batch, so the key cannot leak into the next
    /// composition's wasted-key accounting or be counted twice by on_key.
    pub fn note_selection_key_timed(&mut self, vk: u32, ts_ms: u64) -> bool {
        if vk == 0x0E || Self::is_stat_key(vk) || self.commit_attributions.is_empty() {
            return false;
        }
        self.keys = self.keys.saturating_add(1);
        self.composing_keys = self.composing_keys.saturating_add(1);
        self.attributed_keys = self.attributed_keys.saturating_add(1);
        if let Some(start) = &mut self.composition_start_keys {
            *start = start.saturating_add(1);
        }
        let batch = self.commit_attributions.last_mut().unwrap();
        batch.keys = batch.keys.saturating_add(1);
        if let Some(last) = self.last_key_ms {
            self.per_key
                .push((vk, ts_ms.saturating_sub(last).min(u32::MAX as u64) as u32));
            self.last_key_ms = Some(ts_ms.max(last));
        } else {
            self.last_key_ms = Some(ts_ms);
        }
        true
    }

    /// 结算一段。`elapsed_ms` 为真实用时（首键→完成），由应用层计时器给出。
    pub fn finalize(&self, cmp: &Comparer, elapsed_ms: u64) -> SegmentStats {
        let chars = cmp.pos.min(cmp.states.len()) as f64;
        let keys = self.keys as f64;
        let wrong = cmp.wrong as f64;
        let backspaces = self
            .composition_backspaces
            .saturating_add(self.waste_keys)
            .saturating_sub(self.cancelled_backspaces) as f64;
        let corrections = cmp.corrections as f64;
        let minutes = elapsed_ms as f64 / 60_000.0;
        let seconds = elapsed_ms as f64 / 1000.0;
        let speed = if minutes > 0.0 { chars / minutes } else { 0.0 };
        // 键准（疾风1代简化对照式，非产线）= (键数 − 退格 − 回改 − 错字) / 键数
        // （疾风1代 records.jsonl 实测回归：196字/208键/6退/2错/回改3 → 94.7115，见 jifeng_record_reproduced）
        let accuracy = if keys > 0.0 {
            ((keys - backspaces - corrections - wrong) / keys * 100.0).clamp(0.0, 100.0)
        } else {
            100.0
        };
        // 键准（晴 Pro 精确口径，**产线**；对照公式，见 docs/stats-formula.md §2）：
        // (Hit − Correction − Backs×2) / (字数 + Correction) × 字数 / Hit
        let accuracy_pro = if keys > 0.0 && chars + corrections > 0.0 {
            ((keys - corrections - backspaces * 2.0) / (chars + corrections) * chars / keys * 100.0)
                .clamp(0.0, 100.0)
        } else {
            100.0
        };
        let code_len = if chars > 0.0 { keys / chars } else { 0.0 };
        let strokes_per_sec = if seconds > 0.0 { keys / seconds } else { 0.0 };
        // 有效速度（晴 GetValidSpeed）：Speed − Wrong×5/分钟，下限 0
        let effective_speed = (speed - wrong * 5.0 / minutes.max(f64::EPSILON)).max(0.0);
        // 州州 ComputeStats 直译（zz_ 前缀单列，避免与晴口径混用）
        let (zz_speed, zz_speed2, zz_mc) =
            zz_compute(cmp.pos.min(cmp.states.len()), cmp.wrong, self.keys, seconds);
        let tb = time_breakdown(&cmp.timing_events, chars as usize, cmp.wrong);
        // 有效性双门槛（score.rs 常量，与 <5 聚合硬门语义分开）：字数≥10 且 用时≥10s。
        let is_valid = chars as usize >= crate::score::VALID_EFFECTIVE_CHARS
            && elapsed_ms >= crate::score::VALID_EFFECTIVE_MS;
        SegmentStats {
            chars: chars as usize,
            keys: self.keys,
            backspaces: backspaces as u32,
            wrong: cmp.wrong,
            corrections: cmp.corrections,
            composing_keys: self.composing_keys,
            elapsed_ms,
            speed,
            accuracy_simple: accuracy,
            accuracy_pro,
            effective_speed,
            code_len,
            strokes_per_sec,
            waste_codes: self.waste_codes,
            waste_keys: self.waste_keys,
            commit_groups: self.commit_groups,
            ding_count: self.ding_count,
            select_count: self.select_count,
            zz_speed,
            zz_speed2,
            zz_mc,
            time_breakdown: tb,
            jj: strokes_per_sec,
            para_index_hint: None,
            is_valid,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitAttribution {
    pub chars: usize,
    pub keys: u32,
    pub at_ms: u64,
}

/// 一段的结算成绩
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentStats {
    pub chars: usize,
    pub keys: u32,
    pub backspaces: u32,
    pub wrong: usize,
    pub corrections: usize,
    pub composing_keys: u32,
    pub elapsed_ms: u64,
    /// 速度 字/分
    pub speed: f64,
    /// 键准 %（疾风1代简化对照式：(键-退-改-错)/键）。**非产线口径**——
    /// 仅用于疾风1代 records.jsonl 历史对照回归；成绩/聚合一律用 accuracy_pro。
    pub accuracy_simple: f64,
    /// 键准 %（晴跟打 Pro 精确口径，**产线**，见 docs/stats-formula.md §2）
    #[serde(default)]
    pub accuracy_pro: f64,
    /// 有效速度 = 速度 − 错字×5/分钟（晴 GetValidSpeed 口径，下限 0）
    pub effective_speed: f64,
    /// 码长 = 键/字
    pub code_len: f64,
    /// 击键 键/秒
    pub strokes_per_sec: f64,
    /// 废码次数（组字被取消）
    #[serde(default)]
    pub waste_codes: u32,
    /// 废码浪费的键数
    #[serde(default)]
    pub waste_keys: u32,
    /// 上屏键组数（并击口径：每次 commit 一组）
    #[serde(default)]
    pub commit_groups: u32,
    /// 标顶次数（顶屏键完成提交；晴成绩单 `标顶N`）。旧 JSON 缺省 0。
    #[serde(default)]
    pub ding_count: u32,
    /// 实际候选选择次数（含整句锁词；晴成绩单 `选重N`）。旧 JSON 缺省 0。
    #[serde(default)]
    pub select_count: u32,
    // ── 州州增值口径（`_research/stats-and-drill-models.md` §3.2 直译）──
    /// 州州 speed：(字数−错字)×60/秒 —— "已打部分中错的字数"扣除后的裸速度
    #[serde(default)]
    pub zz_speed: f64,
    /// 州州 speed2（错一罚五）：max(0, 字数−错字×5)×60/秒
    #[serde(default)]
    pub zz_speed2: f64,
    /// 州州 mc（码长）：Keys / (字数−错字)
    #[serde(default)]
    pub zz_mc: f64,
    /// 四分解（州州 SpeedAnalysisWindow 公式原样）：
    /// (正常, 卡顿, 回改, 错字罚时) 秒
    #[serde(default)]
    pub time_breakdown: TimeBreakdown,
    /// 击键速度 jj（键/秒）——jjC 九级评定的输入（strokes_per_sec 同值，单列避免口径混用）
    #[serde(default)]
    pub jj: f64,
    /// 有效性（晴 E4 双门槛）：字数≥10 且 用时≥10s（score.rs 常量）。
    /// is_valid=false 的段仍可计入"练习总字数"，但不进"有效均速/均键准"类指标；
    /// 与字数<5 的聚合硬门（VALID_MIN_CHARS）语义分开。旧 JSON 缺省 false。
    #[serde(default)]
    pub is_valid: bool,
    /// 应用层回填：本段在会话中的段号（F3 重打回滚会话累计时判归属用）
    #[serde(skip)]
    pub para_index_hint: Option<usize>,
}

/// 时间四分解（州州 SpeedAnalysisWindow.xaml.cs 逐行核实，Apache-2.0）：
/// avgPos = 正向事件均时；正常 ≤ avgPos×3；卡顿 > avgPos×3；
/// 回改 = Length<0 事件累计；错字罚时 = 错字 × avgPos × 4（"错一罚五"多付的 4 倍）。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TimeBreakdown {
    /// 正向事件平均用时（秒）
    pub avg_pos: f64,
    /// 正常打字用时（秒）
    pub t_normal: f64,
    /// 卡顿/停留用时（秒）
    pub t_stall: f64,
    /// 回改用时（秒）
    pub t_hg: f64,
    /// 错字罚时（秒，推算值）
    pub t_err: f64,
    /// 理想速度 = (字数−错字)×60/t_normal，封顶 999
    pub ideal_speed: f64,
}

/// 击键九级评定（州州 JjCheckWindow.xaml.cs 公式原样）：
/// 桶 = clamp(floor(jj)−4, 0, 8)（4..12+ 共 9 级）；
/// jjC = 首个占比 ≥10% 的等级；jjC_ = ≥jjC 的累计占比；
/// 评定值 = jjC + jjC_（如 8.235 = "8 键/秒以上稳定占 23.5%"）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JjRating {
    /// 9 桶占比（0..=8 对应 4..12+ 键/秒）
    pub pcts: Vec<f64>,
    /// 基准等级（首个占比 ≥10% 的桶；无历史 = None）
    pub jj_c: Option<u32>,
    /// 达标及以上累计占比（0..=1）
    pub jj_c_up: f64,
}

/// 州州 ComputeStats 直译（`TypingSession.cs`；Apache-2.0）：
/// speed = (len−Cz)×60/sec；speed2 = max(0, len−Cz×5)×60/sec；mc = Keys/(len−Cz)。
/// `input_len` = 上屏总字数，`cz` = 已打部分中错的字数（结算口径同晴）。
pub fn zz_compute(input_len: usize, cz: usize, keys: u32, sec: f64) -> (f64, f64, f64) {
    let len = input_len as f64;
    let cz = cz as f64;
    let speed = if sec > 0.0 {
        (len - cz) * 60.0 / sec
    } else {
        0.0
    };
    let speed2 = if sec > 0.0 {
        (len - cz * 5.0).max(0.0) * 60.0 / sec
    } else {
        0.0
    };
    let mc = if len > cz {
        keys as f64 / (len - cz)
    } else {
        0.0
    };
    (speed, speed2, mc)
}

/// 四分解（州州 SpeedAnalysisWindow 公式原样）。
/// `events` = Comparer.timing_events；`chars`/`cz` = 总字数/错字数。
pub fn time_breakdown(
    events: &[crate::compare::TimingEvent],
    chars: usize,
    cz: usize,
) -> TimeBreakdown {
    let mut tb = TimeBreakdown::default();
    let pos: Vec<&crate::compare::TimingEvent> =
        events.iter().filter(|e| e.len_delta > 0).collect();
    if pos.is_empty() {
        return tb;
    }
    // avgPos：正向事件 TotalTime（dt）平均值——州州秒口径，这里 dt_ms/1000
    let total_pos: f64 = pos.iter().map(|e| e.dt_ms as f64 / 1000.0).sum();
    tb.avg_pos = total_pos / pos.len() as f64;
    let threshold = tb.avg_pos * 3.0;
    for e in &pos {
        let t = e.dt_ms as f64 / 1000.0;
        if t <= threshold {
            tb.t_normal += t;
        } else {
            tb.t_stall += t;
        }
    }
    // 回改：Length<0 事件累计
    tb.t_hg = events
        .iter()
        .filter(|e| e.len_delta < 0)
        .map(|e| e.dt_ms as f64 / 1000.0)
        .sum();
    // 错字罚时 = Cz × avgPos × 4
    tb.t_err = cz as f64 * tb.avg_pos * 4.0;
    // 理想速度 = (useLen−Cz)×60/t_normal，封顶 999
    if tb.t_normal > 0.0 {
        tb.ideal_speed = ((chars as f64 - cz as f64) * 60.0 / tb.t_normal).min(999.0);
    }
    tb
}

/// jjC 九级评定（州州 JjCheckWindow 公式原样）。
/// 输入 = 全部历史段的击键速度 jj 序列（键/秒）。
pub fn jj_rating(jjs: &[f64]) -> JjRating {
    let mut cnt = [0u32; 9];
    for &jj in jjs {
        let b = (jj.floor() as i64 - 4).clamp(0, 8) as usize;
        cnt[b] += 1;
    }
    let total = jjs.len() as f64;
    let pcts: Vec<f64> = cnt
        .iter()
        .map(|&c| if total > 0.0 { c as f64 / total } else { 0.0 })
        .collect();
    let mut r = JjRating {
        pcts,
        jj_c: None,
        jj_c_up: 0.0,
    };
    // jjC = 首个占比 ≥10% 的等级
    for (i, &p) in r.pcts.iter().enumerate() {
        if p >= 0.10 {
            r.jj_c = Some(i as u32);
            // jjC_ = ≥jjC 的累计占比
            r.jj_c_up = r.pcts[i..].iter().sum();
            break;
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::{Comparer, InputEvent};

    /// 疾风1代真实记录复现：196 字 / 208 键 / 6 退格 / 2 错 / 58.285s
    /// → speed=201.767, accuracy=94.712, code_len=1.0612（records.jsonl 第1行实测值）
    #[test]
    fn jifeng_record_reproduced() {
        let mut st = StatsEngine::new();
        let mut cmp = Comparer::new(&"字".repeat(196));
        // 208 个统计键、其中 6 个退格、2 个错字、3 次回改
        for _ in 0..(208 - 6) {
            st.on_key(&KeyEvent {
                vk: 0x41,
                composing: false,
            });
        }
        for _ in 0..6 {
            st.on_key(&KeyEvent {
                vk: 0x08,
                composing: false,
            });
        }
        cmp.pos = cmp.states.len();
        st.composition_backspaces = cmp.backspaces as u32;
        cmp.wrong = 2;
        cmp.backspaces = 6;
        cmp.corrections = 3;
        cmp.pos = cmp.states.len();
        st.composition_backspaces = cmp.backspaces as u32;
        let s = st.finalize(&cmp, 58_285);
        assert!((s.speed - 201.767).abs() < 0.01, "speed={}", s.speed);
        assert!(
            (s.accuracy_simple - 94.7115).abs() < 0.01,
            "accuracy_simple={}",
            s.accuracy_simple
        );
        assert!(
            (s.code_len - 1.06122).abs() < 0.0001,
            "code_len={}",
            s.code_len
        );
        assert_eq!(s.keys, 208);
        assert_eq!(s.chars, 196);
    }

    /// 第2条实测：369字/399键/15退/2错 → accuracy=93.985（回改7 反推）
    #[test]
    fn jifeng_record2_reproduced() {
        let mut st = StatsEngine::new();
        let mut cmp = Comparer::new(&"字".repeat(369));
        for _ in 0..(399 - 15) {
            st.on_key(&KeyEvent {
                vk: 0x41,
                composing: false,
            });
        }
        for _ in 0..15 {
            st.on_key(&KeyEvent {
                vk: 0x08,
                composing: false,
            });
        }
        cmp.pos = cmp.states.len();
        st.composition_backspaces = cmp.backspaces as u32;
        cmp.wrong = 2;
        cmp.backspaces = 15;
        cmp.corrections = 7;
        cmp.pos = cmp.states.len();
        st.composition_backspaces = cmp.backspaces as u32;
        let s = st.finalize(&cmp, 132_811);
        assert!((s.speed - 166.703).abs() < 0.01, "speed={}", s.speed);
        assert!(
            (s.accuracy_simple - 93.985).abs() < 0.01,
            "accuracy_simple={}",
            s.accuracy_simple
        );
    }

    /// 20ms 配对账（P1-3）：note_ding/note_select 计数原样进 SegmentStats。
    #[test]
    fn ding_select_counters_flow_into_finalize() {
        let mut st = StatsEngine::new();
        st.note_ding();
        st.note_ding();
        st.note_select();
        let cmp = Comparer::new("");
        let s = st.finalize(&cmp, 10_000);
        assert_eq!(s.ding_count, 2);
        assert_eq!(s.select_count, 1);
    }

    #[test]
    fn stat_key_table() {
        // 统计键
        for vk in [
            0x20u32, 0x41, 0x5A, 0x30, 0x39, 0x08, 0x0D, 0xBA, 0xC0, 0xE5, 0x60, 0x6F,
        ] {
            assert!(StatsEngine::is_stat_key(vk), "vk={vk:#x} 应统计");
        }
        // 非统计键：F1-F12、方向键、修饰键、PgUp/PgDn 等
        for vk in [
            0x70u32, 0x7B, 0x25, 0x26, 0x27, 0x28, 0x10, 0x11, 0x12, 0x5B, 0xA0, 0xA4, 0x21, 0x22,
        ] {
            assert!(!StatsEngine::is_stat_key(vk), "vk={vk:#x} 不应统计");
        }
    }

    #[test]
    fn end_to_end_sentence() {
        let mut st = StatsEngine::new();
        let mut cmp = Comparer::new("你好世界");
        // 模拟：组字 ni hao（5键） 上屏"你好"；组字 shi jie（7键）上屏"世界"
        for _ in 0..5 {
            st.on_key(&KeyEvent {
                vk: 0x41 + 2,
                composing: true,
            });
        }
        cmp.feed(
            &InputEvent::Commit {
                chars: "你好".into(),
            },
            0,
        );
        for _ in 0..7 {
            st.on_key(&KeyEvent {
                vk: 0x41,
                composing: true,
            });
        }
        cmp.feed(
            &InputEvent::Commit {
                chars: "世界".into(),
            },
            0,
        );
        let s = st.finalize(&cmp, 3_000);
        assert_eq!(s.chars, 4);
        assert_eq!(s.keys, 12);
        assert_eq!(s.composing_keys, 12);
        assert!((s.speed - 80.0).abs() < 0.001); // 4字/3秒 = 80字/分
        assert!((s.code_len - 3.0).abs() < 0.001);
    }

    #[test]
    fn zero_division_safe() {
        let st = StatsEngine::new();
        let cmp = Comparer::new("");
        let s = st.finalize(&cmp, 0);
        assert_eq!(s.speed, 0.0);
        assert_eq!(s.accuracy_simple, 100.0);
        assert_eq!(s.code_len, 0.0);
        assert!(!s.is_valid); // 0字0s：无效
    }

    #[test]
    fn backspace_double_channel_crosscheck() {
        // 键流与比对层两个通道的退格数必须一致
        let mut st = StatsEngine::new();
        let mut cmp = Comparer::new("abcd");
        cmp.feed(&InputEvent::Commit { chars: "ab".into() }, 0);
        st.on_key(&KeyEvent {
            vk: 0x41,
            composing: false,
        });
        st.on_key(&KeyEvent {
            vk: 0x42,
            composing: false,
        });
        for _ in 0..2 {
            st.on_key(&KeyEvent {
                vk: 0x08,
                composing: false,
            });
            cmp.feed(&InputEvent::Backspace, 0);
        }
        assert_eq!(st.backspaces, cmp.backspaces as u32);
        assert_eq!(cmp.corrections, 2);
    }

    // ── 晴跟打 Pro 口径锚点（蓝本 _research/typesunny-stats.md §2）──

    /// 晴 Pro 精确键准：(Hit − Correction − Backs×2) / (字数 + Correction) × 字数 / Hit
    /// 手算：500字/1280键/回改3/退格5 →
    /// (1280−3−10)/(503)×500/1280 = 1267/503×0.390625 = 0.98397… ×100 = 98.40%
    #[test]
    fn typesunny_accuracy_pro_formula() {
        let mut st = StatsEngine::new();
        let mut cmp = Comparer::new(&"字".repeat(500));
        for _ in 0..1275 {
            st.on_key(&KeyEvent {
                vk: 0x41,
                composing: false,
            });
        }
        for _ in 0..5 {
            st.on_key(&KeyEvent {
                vk: 0x08,
                composing: false,
            });
        }
        cmp.backspaces = 5;
        cmp.corrections = 3;
        cmp.pos = cmp.states.len();
        st.composition_backspaces = cmp.backspaces as u32;
        cmp.wrong = 2;
        cmp.pos = cmp.states.len();
        st.composition_backspaces = cmp.backspaces as u32;
        let s = st.finalize(&cmp, 60_000);
        // 手算期望值
        let expected = (1280.0 - 3.0 - 10.0) / (500.0 + 3.0) * 500.0 / 1280.0 * 100.0;
        assert!(
            (s.accuracy_pro - expected).abs() < 1e-9,
            "pro={}",
            s.accuracy_pro
        );
        assert!(
            (s.accuracy_pro - 98.397).abs() < 0.01,
            "pro={}",
            s.accuracy_pro
        );
    }

    /// 错一罚五（晴 GetValidSpeed）：Speed − Wrong×5/TotalMinutes，下限 0。
    /// 疾风1代记录：196字/2错/58.285s → 201.767 − 10/0.9714 = 201.767 − 10.294 = 191.47
    #[test]
    fn wrong_penalty_five_effective_speed() {
        let mut st = StatsEngine::new();
        let mut cmp = Comparer::new(&"字".repeat(196));
        for _ in 0..202 {
            st.on_key(&KeyEvent {
                vk: 0x41,
                composing: false,
            });
        }
        for _ in 0..6 {
            st.on_key(&KeyEvent {
                vk: 0x08,
                composing: false,
            });
        }
        cmp.pos = cmp.states.len();
        st.composition_backspaces = cmp.backspaces as u32;
        cmp.wrong = 2;
        cmp.backspaces = 6;
        cmp.pos = cmp.states.len();
        st.composition_backspaces = cmp.backspaces as u32;
        let s = st.finalize(&cmp, 58_285);
        assert!(
            (s.effective_speed - 191.47).abs() < 0.05,
            "eff={}",
            s.effective_speed
        );
        // 无错字：有效速度 = 速度
        let s0 = {
            let mut c2 = Comparer::new(&"字".repeat(196));
            c2.pos = c2.states.len();
            c2.wrong = 0;
            st.finalize(&c2, 58_285)
        };
        assert!((s0.effective_speed - s0.speed).abs() < 1e-9);
    }

    /// 罚到下限：错字极多时有效速度不为负
    #[test]
    fn effective_speed_floor_zero() {
        let st = StatsEngine::new();
        let mut cmp = Comparer::new("十个字十个字十个");
        cmp.wrong = 50;
        let s = st.finalize(&cmp, 1_000);
        assert_eq!(s.effective_speed, 0.0);
    }

    /// 有效性双门槛边界（E4/P1-4）：字数≥10 且 用时≥10s 才 is_valid。
    /// 覆盖 2字/4字/9字9s/9字10s/10字9s/10字10s。
    #[test]
    fn is_valid_double_threshold_boundaries() {
        let cases: &[(&str, u64, bool)] = &[
            ("甲乙", 1_000, false),                 // 2字：双不过
            ("甲乙丙丁", 1_000, false),             // 4字：<字数门（也<5 硬门）
            ("甲乙丙丁戊己庚辛壬", 9_000, false),   // 9字9s：双不过
            ("甲乙丙丁戊己庚辛壬", 10_000, false),  // 9字10s：仅字数不过
            ("甲乙丙丁戊己庚辛壬癸", 9_999, false), // 10字9.999s：仅用时不过
            ("甲乙丙丁戊己庚辛壬癸", 10_000, true), // 10字10s：双过
        ];
        for (text, ms, valid) in cases {
            let st = StatsEngine::new();
            let mut cmp = Comparer::new(text);
            cmp.pos = cmp.states.len();
            let s = st.finalize(&cmp, *ms);
            assert_eq!(s.is_valid, *valid, "text={text} ms={ms}");
        }
    }

    /// 废码：组字编码被取消 → 整轮组字键数计入退格 + waste（晴 Pro 空上屏分支）
    #[test]
    fn composition_cancel_wastes_keys_into_backspaces() {
        let mut st = StatsEngine::new();
        // 组字 4 键后取消
        st.begin_composition();
        for _ in 0..4 {
            st.on_key(&KeyEvent {
                vk: 0x41,
                composing: true,
            });
        }
        st.cancel_composition();
        assert_eq!(st.keys, 4); // 键照计（真实按过）
        assert_eq!(st.backspaces, 4); // 整轮计入退格
        assert_eq!(st.waste_codes, 1);
        assert_eq!(st.waste_keys, 4);
        // 重新组字上屏正常
        st.begin_composition();
        st.on_key(&KeyEvent {
            vk: 0x41,
            composing: true,
        });
        st.on_key(&KeyEvent {
            vk: 0x42,
            composing: true,
        });
        st.end_composition();
        st.on_commit_group();
        assert_eq!(st.waste_codes, 1); // 不再累加
        assert_eq!(st.backspaces, 4); // 正常上屏不加退格
        assert_eq!(st.commit_groups, 1); // 并击口径一组
                                         // 未组字时取消是 no-op
        st.cancel_composition();
        assert_eq!(st.waste_codes, 1);
    }

    #[test]
    fn late_composition_start_recovers_first_keys_and_cancel_settles_the_batch() {
        let mut st = StatsEngine::new();
        st.on_key(&KeyEvent {
            vk: 0x41,
            composing: false,
        });
        st.on_key(&KeyEvent {
            vk: 0x42,
            composing: false,
        });
        st.begin_composition(); // TSF reports the new composition after dispatching keys.
        assert_eq!(st.composing_keys, 2);
        st.on_key(&KeyEvent {
            vk: 0x08,
            composing: true,
        });
        st.cancel_composition();
        assert_eq!(st.waste_keys, 3);
        assert_eq!(st.backspaces, 3);
        assert_eq!(st.attributed_keys, 3);
        assert_eq!(st.finalize(&Comparer::new("甲"), 1000).backspaces, 3);
        st.cancel_composition(); // Repeated terminal notification is harmless.
        assert_eq!(st.waste_codes, 1);
        st.on_key(&KeyEvent {
            vk: 0x43,
            composing: false,
        });
        st.on_commit(1, 1000);
        assert_eq!(st.commit_attributions[0].keys, 1);
        assert_eq!(st.keys, 4);
    }

    #[test]
    fn split_commits_and_still_active_composition_never_invent_keys() {
        let mut st = StatsEngine::new();
        st.begin_composition();
        for _ in 0..4 {
            st.on_key(&KeyEvent {
                vk: 0x41,
                composing: true,
            });
        }
        st.on_commit(1, 100);
        st.begin_composition(); // The same edit session still has active preedit.
        st.on_commit(2, 100); // Another output diff, without any new physical key.
        assert_eq!(
            st.commit_attributions
                .iter()
                .map(|c| c.keys)
                .collect::<Vec<_>>(),
            vec![4, 0]
        );
        st.begin_composition();
        st.cancel_composition();
        assert_eq!(
            st.waste_keys, 0,
            "already attributed keys cannot be wasted again"
        );
        st.on_key(&KeyEvent {
            vk: 0x42,
            composing: false,
        });
        st.on_commit(1, 200);
        assert_eq!(
            st.commit_attributions.iter().map(|c| c.keys).sum::<u32>(),
            st.keys
        );
    }

    #[test]
    fn noncomposing_physical_deletions_do_not_feed_the_next_output_batch() {
        let mut st = StatsEngine::new();
        st.on_key(&KeyEvent {
            vk: 0x08,
            composing: false,
        });
        st.on_key(&KeyEvent {
            vk: 0x08,
            composing: false,
        });
        st.on_key(&KeyEvent {
            vk: 0x41,
            composing: false,
        });
        st.begin_composition();
        assert_eq!(st.composing_keys, 1);
        st.on_commit(1, 100);
        assert_eq!(st.keys, 3);
        assert_eq!(st.commit_attributions[0].keys, 1);
        assert_eq!(
            st.finalize(&Comparer::new("甲"), 100).backspaces,
            0,
            "committed deletions are corrections, not composing Backspaces"
        );
    }

    #[test]
    fn proven_selection_modifier_repairs_only_completed_key_batch() {
        let mut st = StatsEngine::new();
        assert!(
            !st.note_selection_key_timed(0xA0, 90),
            "no commit to attribute"
        );
        st.on_key_timed(0x41, true, 100);
        st.on_key_timed(0xA0, true, 190); // an ordinary modifier stays excluded
        st.on_commit(2, 200);
        st.end_composition();
        st.begin_composition(); // host may already report the next composition
        assert!(st.note_selection_key_timed(0xA0, 195));
        assert_eq!(st.keys, 2);
        assert_eq!(st.composing_keys, 2);
        assert_eq!(st.commit_attributions[0].keys, 2);
        assert_eq!(st.attributed_keys, 2);
        assert_eq!(st.per_key.last(), Some(&(0xA0, 95)));
        assert!(
            !st.note_selection_key_timed(0xBA, 200),
            "normal OEM key was counted already"
        );
        st.on_key_timed(0x42, true, 250);
        st.cancel_composition();
        assert_eq!(
            st.waste_keys, 1,
            "the completed selection never becomes next-round waste"
        );
        assert_eq!(st.commit_attributions[0].keys, 2);
    }

    #[test]
    fn proven_selection_keys_wait_in_current_batch_without_rewriting_previous_commit() {
        let mut st = StatsEngine::new();
        st.on_key_timed(0x41, false, 100);
        st.on_commit(1, 110);
        st.on_key_timed(0x42, false, 200);
        st.on_key_timed(0xA0, true, 300); // excluded until its selection is confirmed
        assert!(st.on_proven_selection_key_timed(0xA0, 300));
        assert!(st.on_proven_selection_key_timed(0x14, 500));
        assert_eq!(st.keys, 4);
        assert_eq!(st.attributed_keys, 1);
        assert_eq!(st.commit_attributions[0].keys, 1);
        assert_eq!(st.composition_start_keys, Some(1));
        st.begin_composition(); // repeated composition observations cannot recount the keys
        assert_eq!(st.composing_keys, 3);
        assert_eq!(st.per_key, vec![(0x42, 100), (0xA0, 100), (0x14, 200)]);
        st.on_commit(2, 3_000); // sentence locking can precede actual output by seconds
        assert_eq!(st.commit_attributions[1].keys, 3);
        assert_eq!(st.commit_attributions[0].keys, 1);
        assert_eq!(st.attributed_keys, st.keys);
    }

    #[test]
    fn proven_selection_keys_are_cancelled_with_the_pending_sentence_only() {
        let mut st = StatsEngine::new();
        st.on_key_timed(0x41, false, 100);
        st.on_commit(1, 110);
        st.on_key_timed(0x42, true, 200);
        assert!(st.on_proven_selection_key_timed(0x70, 300));
        st.cancel_composition();
        assert_eq!(st.waste_codes, 1);
        assert_eq!(st.waste_keys, 2);
        assert_eq!(st.commit_attributions[0].keys, 1);
        assert_eq!(st.keys, 3);
        st.on_key_timed(0x43, true, 500);
        st.on_commit(1, 600);
        assert_eq!(st.commit_attributions[1].keys, 1);
        assert_eq!(st.attributed_keys, st.keys);
    }

    #[test]
    fn proven_selection_does_not_duplicate_statistical_keys_or_numpad_enter() {
        let mut st = StatsEngine::new();
        for vk in [0xBA, 0xDE, 0x32, 0x62, 0x0D] {
            st.on_key_timed(vk, true, 100);
            assert!(!st.on_proven_selection_key_timed(vk, 100));
        }
        assert!(!st.on_proven_selection_key_timed(0x0E, 100));
        assert!(!st.on_proven_selection_key_timed(0, 100));
        assert!(!st.on_proven_selection_key_timed(256, 100));
        st.on_commit(1, 200);
        assert_eq!(st.keys, 5);
        assert_eq!(st.commit_attributions[0].keys, 5);
    }

    /// 并击口径：3 次 commit → commit_groups = 3，与串行键数并存
    #[test]
    fn commit_groups_parallel_channel() {
        let mut st = StatsEngine::new();
        st.begin_composition();
        for _ in 0..3 {
            st.on_key(&KeyEvent {
                vk: 0x41,
                composing: true,
            });
        }
        st.end_composition();
        st.on_commit_group();
        st.begin_composition();
        for _ in 0..2 {
            st.on_key(&KeyEvent {
                vk: 0x41,
                composing: true,
            });
        }
        st.end_composition();
        st.on_commit_group();
        let cmp = Comparer::new("五字测试组");
        let s = st.finalize(&cmp, 2_000);
        assert_eq!(s.keys, 5); // 串行口径：物理键全计
        assert_eq!(s.commit_groups, 2); // 并击口径：2 次上屏
    }

    /// 晴 Pro 击键/码长/速度三角公式互验（typesunny-stats.md §2.6 手算样例）
    /// 500字/1280键/8:48(528s)：击键=1280/528=2.4242，码长=2.56，速度=56.818
    #[test]
    fn typesunny_triple_formula() {
        let mut st = StatsEngine::new();
        let mut cmp = Comparer::new(&"字".repeat(500));
        for _ in 0..1280 {
            st.on_key(&KeyEvent {
                vk: 0x41,
                composing: false,
            });
        }
        cmp.pos = cmp.states.len();
        let s = st.finalize(&cmp, 528_000);
        assert!(
            (s.strokes_per_sec - 2.4242).abs() < 0.001,
            "hit={}",
            s.strokes_per_sec
        );
        assert!((s.code_len - 2.56).abs() < 0.001, "kpw={}", s.code_len);
        assert!((s.speed - 56.818).abs() < 0.001, "speed={}", s.speed);
    }
}
