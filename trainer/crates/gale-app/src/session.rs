//! Per-segment round state. All durations use the caller's monotonic milliseconds.
use gale_core::compare::{normalize_newlines, Comparer, InputEvent};
use gale_core::rules::{
    clean_body, fingerprint, CleaningPreset, RoundMetadata, SessionRules, TrainingMode,
};
use gale_core::score::{DifficultyTag, ScoreRecord, VALID_MIN_CHARS};
use gale_core::segment::{parse_group_text, split_smart, ParsedGroupText, Segment};
use gale_core::single::SingleSession;
use gale_core::stats::{SegmentStats, StatsEngine};
use gale_win::input_host::{BackspaceClass, BackspaceRouter, CommitCause};
use serde::{Deserialize, Serialize};
static SNAPSHOT_IO: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `BackspaceRouter` 的可 Clone 包装：Session 整体派生 Clone/Debug（快照
/// 序列化前克隆），gale-win 原类型只派生 Default/Debug。克隆语义 = 复位初始
/// 态——protect 窗是 20ms 级瞬时状态，克隆仅用于序列化快照，真实路由器由
/// 主实例驱动。
#[derive(Default, Debug)]
struct RouterCell(BackspaceRouter);
impl Clone for RouterCell {
    fn clone(&self) -> Self {
        Self(BackspaceRouter::default())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Idle,
    Typing,
    Done,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionSource {
    Group,
    Library(String),
    Single,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub title: String,
    pub source: SessionSource,
    pub segments: Vec<Segment>,
    pub index: usize,
    pub comparer: Comparer,
    pub stats: StatsEngine,
    pub phase: Phase,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
    pub preedit: String,
    pub composing: bool,
    pub recorded: bool,
    pub last_stats: Option<SegmentStats>,
    pub done_chars: u64,
    pub done_keys: u64,
    pub done_bs: u64,
    pub done_wrong: u64,
    pub done_ms: u64,
    pub rules: SessionRules,
    pub frozen_rules: Option<SessionRules>,
    pub raw_segments: Vec<Segment>,
    pub results: Vec<Option<ScoreRecord>>,
    /// Per-text starts survive retries, article reloads, and recovery snapshots.
    #[serde(default)]
    text_attempts: std::collections::BTreeMap<String, u32>,
    #[serde(default)]
    current_retype_count: u32,
    /// Old snapshots cannot reconstruct commit boundaries, hence None means unknown.
    #[serde(default)]
    word_attribution: Option<gale_core::scorecard::WordRateTracker>,
    pub cleaned: bool,
    pub pasted: bool,
    /// 载文难度（群文《难度》头 / 文来难度；结算时随成绩入 ScoreRecord.difficulty，
    /// 成绩单输出 `难度普(1.84)`）。旧快照缺省 None。
    #[serde(default)]
    pub difficulty: Option<gale_core::score::DifficultyTag>,
    default_seg_chars: usize,
    frozen_text_hash: String,
    #[serde(default)]
    frozen_raw_text_hash: String,
    last_clock_ms: u64,
    paused_at: Option<u64>,
    paused_ms: u64,
    /// 退格归属裁决器（P1-6 protect 窗）：物理退格先经它分类——ImeDelete 归
    /// “组字期退格”账、Correction 归“回改”账。纯运行态，不入快照。
    #[serde(skip)]
    bs_router: RouterCell,
    /// 整改12：本段 VK 击键直方图（热力图数据源）。运行态，不入快照；
    /// reset_current 清零，结算时快照进 last_vk_histogram。
    #[serde(skip)]
    pub vk_histogram: std::collections::HashMap<u8, u32>,
    /// 整改12：最近一次结算段的 VK 直方图快照（finalize 时冻结），
    /// UI 热力图按此字段消费；新文章载入时清空。
    #[serde(skip)]
    pub last_vk_histogram: std::collections::HashMap<u8, u32>,
    /// 整改13：当前文章全篇理论码长分析（server /api/trainer/analyze?include_full=true）。
    /// 结构定义在 crate 根（main.rs::FullAnalysis）；None = 旧版服务无此功能。
    /// 新文章载入时清空，后台取到后回填。
    #[serde(skip)]
    pub full_analysis: Option<crate::FullAnalysis>,
}
impl Session {
    pub fn new(seg_chars: usize) -> Self {
        let seg = Segment {
            mark: "1".into(),
            text: String::new(),
        };
        Self {
            title: String::new(),
            source: SessionSource::Custom,
            segments: vec![seg.clone()],
            index: 0,
            comparer: Comparer::new(""),
            stats: StatsEngine::new(),
            phase: Phase::Idle,
            started_ms: None,
            finished_ms: None,
            preedit: String::new(),
            composing: false,
            recorded: false,
            last_stats: None,
            done_chars: 0,
            done_keys: 0,
            done_bs: 0,
            done_wrong: 0,
            done_ms: 0,
            rules: SessionRules::default(),
            frozen_rules: None,
            raw_segments: vec![seg],
            results: vec![None],
            text_attempts: std::collections::BTreeMap::new(),
            current_retype_count: 0,
            word_attribution: Some(gale_core::scorecard::WordRateTracker::default()),
            cleaned: false,
            pasted: false,
            difficulty: None,
            default_seg_chars: seg_chars.clamp(10, 10000),
            frozen_text_hash: String::new(),
            frozen_raw_text_hash: String::new(),
            last_clock_ms: 0,
            paused_at: None,
            paused_ms: 0,
            bs_router: RouterCell::default(),
            vk_histogram: std::collections::HashMap::new(),
            last_vk_histogram: std::collections::HashMap::new(),
            full_analysis: None,
        }
    }
    fn loaded(&mut self) {
        self.index = 0;
        self.raw_segments = self.segments.clone();
        self.normalize_working_newlines();
        self.results = vec![None; self.segments.len()];
        self.cleaned = false;
        self.difficulty = None;
        // 新文章：热力图快照与全篇分析随旧文作废（整改12/13）
        self.last_vk_histogram.clear();
        self.full_analysis = None;
        self.recount();
        self.reset_current();
    }
    fn normalize_working_newlines(&mut self) {
        for seg in &mut self.segments {
            if seg.text.contains('\r') {
                seg.text = normalize_newlines(&seg.text).into_owned();
            }
        }
    }
    pub fn load_group(&mut self, parsed: ParsedGroupText) {
        if parsed.segments.is_empty() {
            return;
        }
        self.title = parsed.title;
        self.source = SessionSource::Group;
        self.segments = parsed.segments;
        self.loaded();
        // 群文《难度》头（"普 1.84"）/文来标签——载入时解析，结算时随成绩进成绩单
        self.difficulty = parsed.difficulty.as_deref().and_then(DifficultyTag::parse);
    }
    /// Clipboard and QQ imports share format recognition. The parser only
    /// removes metadata from a recognized article; plain text keeps smart splits.
    pub fn load_text(&mut self, text: &str) {
        let parsed = parse_group_text(text);
        if parsed.segments.iter().any(|seg| !seg.mark.is_empty()) {
            self.load_group(parsed);
        } else {
            self.load_book("自定义文字", text);
        }
    }
    pub fn load_book(&mut self, title: &str, text: &str) {
        self.segments = split_smart(text, self.default_seg_chars);
        if self.segments.is_empty() {
            self.segments.push(Segment {
                mark: "1".into(),
                text: text.into(),
            });
        }
        self.title = title.into();
        self.source = SessionSource::Library(title.into());
        self.loaded();
    }
    pub fn load_single(&mut self, s: SingleSession) {
        self.title = format!("练单字 · {}", s.title);
        self.source = SessionSource::Single;
        self.segments = (0..s.total_groups())
            .map(|i| {
                let mut group = s.clone();
                group.group_index = i;
                group.current_segment()
            })
            .collect();
        if self.segments.is_empty() {
            self.segments.push(s.current_segment());
        }
        self.loaded();
    }
    fn reset_current(&mut self) {
        self.comparer = Comparer::new(&self.current_segment().text);
        self.comparer.quote_rules = self.rules.quotes.clone();
        self.stats = StatsEngine::new();
        self.phase = Phase::Idle;
        self.started_ms = None;
        self.finished_ms = None;
        self.preedit.clear();
        self.composing = false;
        self.recorded = false;
        self.last_stats = None;
        self.current_retype_count = 0;
        self.word_attribution = Some(gale_core::scorecard::WordRateTracker::default());
        self.frozen_rules = None;
        self.frozen_text_hash.clear();
        self.frozen_raw_text_hash.clear();
        self.paused_at = None;
        self.paused_ms = 0;
        self.pasted = false;
        self.last_clock_ms = 0;
        // protect 窗随段清零：换段/重打后第一个退格按裸状态裁决
        self.bs_router = RouterCell::default();
        // 整改12：本段直方图随段清零（last_vk_histogram 保留至下一次结算覆盖）
        self.vk_histogram.clear();
    }
    fn recount(&mut self) {
        let a = self.aggregate();
        self.done_chars = a.chars;
        self.done_keys = a.keys;
        self.done_bs = a.backspaces;
        self.done_wrong = a.wrong;
        self.done_ms = a.time_ms;
    }
    pub fn aggregate(&self) -> gale_core::report::RaceSummary {
        gale_core::report::RaceSummary::from_records(self.results.iter().flatten())
    }
    /// Seed start counts from the durable journal after restoring the session.
    /// Old records have retype_count=0, so also count completions for compatibility.
    /// The max keeps aborted attempts already present in a recovery snapshot.
    pub fn seed_retype_history<'a>(&mut self, records: impl IntoIterator<Item = &'a ScoreRecord>) {
        let untracked_active =
            self.phase == Phase::Typing && !self.text_attempts.contains_key(&self.frozen_text_hash);
        let mut completed = std::collections::BTreeMap::<String, u32>::new();
        let mut attempts = std::collections::BTreeMap::<String, u32>::new();
        for record in records {
            let Some(meta) = record.round.as_ref().filter(|m| !m.text_hash.is_empty()) else {
                continue;
            };
            let count = completed.entry(meta.text_hash.clone()).or_default();
            *count = count.saturating_add(1);
            let observed = attempts.entry(meta.text_hash.clone()).or_default();
            *observed = (*observed).max(record.retype_count.saturating_add(1));
        }
        for (hash, count) in completed {
            let count = count.max(attempts.get(&hash).copied().unwrap_or(0));
            let saved = self.text_attempts.entry(hash).or_default();
            *saved = (*saved).max(count);
        }
        if untracked_active {
            let count = self
                .text_attempts
                .entry(self.frozen_text_hash.clone())
                .or_default();
            self.current_retype_count = *count;
            *count = count.saturating_add(1);
        }
    }
    pub fn current_segment(&self) -> &Segment {
        self.segments.get(self.index).unwrap_or(&self.segments[0])
    }
    pub fn current_index(&self) -> usize {
        self.index
    }
    pub fn total_segments(&self) -> usize {
        self.segments.len()
    }
    pub fn jump(&mut self, idx: usize) {
        if idx >= self.segments.len() {
            return;
        }
        self.index = idx;
        self.results[idx] = None;
        self.recount();
        self.reset_current();
    }
    pub fn next_segment(&mut self) {
        if self.index + 1 < self.segments.len() {
            self.jump(self.index + 1);
        }
    }
    pub fn is_segment_done(&self) -> bool {
        !self.current_segment().text.is_empty() && self.comparer.is_done()
    }
    pub fn segment_recorded(&self) -> bool {
        self.recorded
    }
    pub fn mark_sent(&mut self) {
        self.recorded = true;
    }
    pub fn set_rules(&mut self, rules: SessionRules) -> Result<(), String> {
        if self.frozen_rules.is_some() {
            return Err("本轮规则已冻结，请先重打再修改".into());
        }
        let rules = rules.normalized();
        if self.rules != rules {
            self.results.fill(None);
            self.recount();
        }
        self.rules = rules;
        self.comparer.quote_rules = self.rules.quotes.clone();
        Ok(())
    }
    pub fn apply_cleaning(&mut self, preset: &CleaningPreset) -> Result<(), String> {
        if self.frozen_rules.is_some() {
            return Err("开打后不能修改正文，请先重打".into());
        }
        // Transform all bodies transactionally; one failing rule leaves every original intact.
        let mut updated = self.raw_segments.clone();
        let mut changed = false;
        for seg in &mut updated {
            let out = clean_body(&seg.text, preset, self.rules.mode)?;
            changed |= out.cleaned_text != seg.text;
            seg.text = out.cleaned_text;
        }
        self.segments = updated;
        self.normalize_working_newlines();
        self.cleaned = changed;
        self.results.fill(None);
        self.recount();
        self.reset_current();
        Ok(())
    }
    pub fn restore_raw(&mut self) -> Result<(), String> {
        if self.frozen_rules.is_some() {
            return Err("开打后不能还原正文，请先重打".into());
        }
        self.segments = self.raw_segments.clone();
        self.normalize_working_newlines();
        self.cleaned = false;
        self.results.fill(None);
        self.recount();
        self.reset_current();
        Ok(())
    }
    fn clock(&mut self, now: u64) -> u64 {
        self.last_clock_ms = self.last_clock_ms.max(now);
        self.last_clock_ms
    }
    fn begin(&mut self, now: u64) {
        if self.phase != Phase::Idle || self.current_segment().text.is_empty() {
            return;
        }
        self.rules = self.rules.clone().normalized();
        self.comparer.quote_rules = self.rules.quotes.clone();
        self.frozen_rules = Some(self.rules.clone());
        self.frozen_text_hash = fingerprint(self.comparer.text.as_bytes());
        self.frozen_raw_text_hash = fingerprint(self.raw_segments[self.index].text.as_bytes());
        let attempts = self
            .text_attempts
            .entry(self.frozen_text_hash.clone())
            .or_default();
        self.current_retype_count = *attempts;
        *attempts = attempts.saturating_add(1);
        self.phase = Phase::Typing;
        self.started_ms = Some(now);
        self.comparer.started_at = Some(now);
    }
    pub fn feed_input(&mut self, ev: InputEvent, now_ms: u64) {
        if self.current_segment().text.is_empty()
            || self.phase == Phase::Done
            || self.paused_at.is_some()
        {
            return;
        }
        let now = self.clock(now_ms);
        let meaningful = matches!(&ev,InputEvent::Commit {chars} if !chars.is_empty())
            || matches!(ev, InputEvent::Backspace) && self.comparer.pos > 0;
        if !meaningful {
            return;
        }
        self.begin(now);
        // Event intervals use the same active-time axis as the headline duration.
        // This also preserves active time before blur, including the first commit.
        let active_now = now.saturating_sub(self.paused_ms);
        let previous_pos = self.comparer.pos;
        let out = self.comparer.feed(&ev, active_now);
        if let Some(attribution) = &mut self.word_attribution {
            if matches!(ev, InputEvent::Backspace) && self.comparer.pos < previous_pos {
                attribution.backspace();
            } else if !out.is_empty() {
                attribution.commit(&out);
            }
        }
        if matches!(ev, InputEvent::Commit { .. }) && !out.is_empty() {
            self.stats.on_commit(out.len(), active_now);
            self.stats.end_composition();
            // 提交清空组合：BackspaceRouter 不得置 protect（区分“空格提交瞬间
            // 组合清空”与“退格删空”，见 gale-win input_host BackspaceRouter 注释）
            self.bs_router.0.on_composition_change(false, true);
            self.composing = false;
            self.preedit.clear();
        }
        if self.comparer.is_done() {
            self.finished_ms = Some(now);
            self.phase = Phase::Done;
        }
    }
    pub fn on_key(&mut self, vk: u32, now_ms: u64) {
        if self.current_segment().text.is_empty()
            || self.phase == Phase::Done
            || self.paused_at.is_some()
            || !StatsEngine::is_stat_key(vk)
        {
            return;
        }
        let now = self.clock(now_ms);
        self.begin(now);
        // P1-6 protect 窗：物理退格先经 BackspaceRouter 裁决——ImeDelete 归
        // “组字期退格”账（composing=true 计账，含删空后 protect 窗内的那一次），
        // Correction 走 composing=false 的既有回改路径（Comparer 侧另记 corrections）。
        let composing = if vk == 0x08 {
            matches!(
                self.bs_router.0.on_backspace_key(self.composing),
                BackspaceClass::ImeDelete
            )
        } else {
            self.composing
        };
        self.stats
            .on_key_timed(vk, composing, now.saturating_sub(self.paused_ms));
    }
    /// A confirmed candidate choice is counted immediately, including sentence
    /// locks that do not commit text yet. Default Space confirmation does not
    /// produce this event; a choice without a physical key adds no guessed key.
    pub fn on_candidate_selection(&mut self, vk: Option<u32>, now_ms: u64) {
        if self.current_segment().text.is_empty()
            || self.phase == Phase::Done
            || self.recorded
            || self.paused_at.is_some()
        {
            return;
        }
        let now = self.clock(now_ms);
        self.begin(now);
        self.stats.note_select();
        if let Some(vk) = vk {
            self.stats
                .on_proven_selection_key_timed(vk, now.saturating_sub(self.paused_ms));
        }
    }

    /// Other-input-method fallback: Ding→标顶、Select→选重、Unknown 忽略。
    /// main.rs 在每帧 poll 之后 drain_commit_causes 并逐条转发到这里；
    /// The final Commit reaches Done before this queue drains. Accept it until
    /// finalize sets recorded, otherwise every final selection/top-code is lost.
    pub fn on_commit_cause(&mut self, cause: CommitCause) {
        if !self.recorded && matches!(self.phase, Phase::Typing | Phase::Done) {
            match cause {
                CommitCause::Ding => self.stats.note_ding(),
                CommitCause::Select => self.stats.note_select(),
                CommitCause::SelectExtra { vk, lag_ms } => {
                    if let Some(commit_ms) = self.stats.commit_attributions.last().map(|a| a.at_ms)
                    {
                        if self
                            .stats
                            .note_selection_key_timed(vk, commit_ms.saturating_sub(lag_ms))
                        {
                            self.stats.note_select();
                        }
                    }
                }
                CommitCause::Unknown => {}
            }
        }
    }
    /// 物理退格弹起（main.rs 帧循环对 VK_BACK 的按下态边沿观测）：清 protect。
    /// gale-win 事件泵不透传 keyup，这里是弹起信息的唯一来源。
    pub fn on_backspace_up(&mut self) {
        self.bs_router.0.on_backspace_up();
    }
    pub fn set_preedit(&mut self, p: String) {
        if !self.current_segment().text.is_empty() {
            self.preedit = p;
        }
    }
    pub fn set_composing(&mut self, c: bool) {
        self.set_composing_from_host(c, false);
    }
    /// host 批次感知通道（main.rs 事件循环）：`Composing(false)` 与 Commit 同批
    /// 出现时 batch_has_commit=true——提交清空不置 protect；只有 Composing(false)
    /// （退格删空/Esc 取消）才置 protect（下一次退格仍归 IME）。
    pub fn set_composing_from_host(&mut self, c: bool, batch_has_commit: bool) {
        if self.current_segment().text.is_empty() {
            return;
        }
        if c && !self.composing {
            self.stats.begin_composition();
        }
        if self.composing != c {
            let cleared_by_commit = !c && batch_has_commit;
            self.bs_router.0.on_composition_change(c, cleared_by_commit);
        }
        self.composing = c;
    }
    /// Hosts call this only for an explicitly cancelled composition, never for a normal
    /// END notification which may arrive before its commit notification.
    pub fn cancel_composition(&mut self) {
        if self.composing {
            // Esc/退格删空的显式取消 = 非提交清空 → 置 protect
            self.bs_router.0.on_composition_change(false, false);
        }
        self.stats.cancel_composition();
        self.composing = false;
        self.preedit.clear();
    }
    pub fn feed_paste(&mut self, text: &str, now_ms: u64) -> Result<(), String> {
        if self.current_segment().text.is_empty() {
            return Err("请先从剪贴板载入文章".into());
        }
        let rules = self.frozen_rules.as_ref().unwrap_or(&self.rules);
        if rules.mode == TrainingMode::Race || !rules.allow_paste {
            return Err("本轮禁止粘贴输入".into());
        }
        self.pasted = true;
        self.feed_input(InputEvent::Commit { chars: text.into() }, now_ms);
        Ok(())
    }
    pub fn set_paused(&mut self, paused: bool, now_ms: u64) {
        let rules = self.frozen_rules.as_ref().unwrap_or(&self.rules);
        if rules.mode == TrainingMode::Race || !rules.pause_on_blur || self.phase != Phase::Typing {
            return;
        }
        let now = self.clock(now_ms);
        if paused {
            if self.paused_at.is_none() {
                self.paused_at = Some(now);
            }
        } else if let Some(start) = self.paused_at.take() {
            self.paused_ms = self.paused_ms.saturating_add(now.saturating_sub(start));
        }
    }
    pub fn stop(&mut self) {
        self.stop_at(self.last_clock_ms);
    }
    pub fn stop_at(&mut self, now_ms: u64) {
        let now = self.clock(now_ms);
        if self.phase == Phase::Typing {
            self.finished_ms = Some(now);
            self.phase = Phase::Done;
        }
    }
    pub fn finalize_current(&mut self, now_ms: u64) -> Option<ScoreRecord> {
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.finalize_current_at(now_ms, unix)
    }
    pub fn finalize_current_at(&mut self, now_ms: u64, unix_secs: u64) -> Option<ScoreRecord> {
        if self.recorded {
            return None;
        }
        let now = self.clock(now_ms);
        let elapsed = self.elapsed_ms(now);
        if elapsed == 0 || self.comparer.pos == 0 || self.stats.keys == 0 {
            return None;
        }
        self.finished_ms.get_or_insert(now);
        self.phase = Phase::Done;
        let mut st = self.stats.finalize(&self.comparer, elapsed);
        st.para_index_hint = Some(self.index);
        self.last_stats = Some(st.clone());
        let label = match self.source {
            SessionSource::Group => "群文",
            SessionSource::Library(_) => "文库",
            SessionSource::Single => "练单字",
            SessionSource::Custom => "自定义",
        };
        let mut rec = ScoreRecord::from_stats(
            &st,
            &self.title,
            label,
            &self.current_segment().mark,
            self.index,
            unix_secs,
            self.difficulty.clone(),
        );
        let rules = self.frozen_rules.as_ref().unwrap_or(&self.rules);
        rec.retype_count = self.current_retype_count;
        rec.word_rate = self
            .word_attribution
            .as_ref()
            .filter(|a| a.is_valid_for(self.comparer.pos))
            .map(|a| a.percent());
        rec.round = Some(RoundMetadata {
            mode: rules.mode,
            text_hash: self.frozen_text_hash.clone(),
            raw_text_hash: self.frozen_raw_text_hash.clone(),
            rules_hash: rules.fingerprint(),
            quote_relaxed: self.comparer.quote_relaxed,
            cleaned: self.cleaned,
            pasted: self.pasted,
            complete: self.comparer.is_done(),
            // 开打时冻结的词提快照（frozen_rules 即 begin() 冻结）；成绩单 [词提开] 标记引用
            word_hints: rules.word_hints,
        });
        self.results[self.index] = Some(rec.clone());
        self.recorded = true;
        // 整改12：结算即快照——本段 VK 直方图冻结进 last_vk_histogram（UI 热力图消费）
        self.last_vk_histogram = self.vk_histogram.clone();
        self.recount();
        Some(rec)
    }
    pub fn idle_hint(&self) -> bool {
        matches!(self.phase, Phase::Idle | Phase::Done) || self.paused_at.is_some()
    }
    pub fn typed_len(&self) -> usize {
        self.comparer.pos
    }
    pub fn typed_any(&self) -> bool {
        self.comparer.pos != 0
    }
    pub fn elapsed_ms(&self, now_ms: u64) -> u64 {
        let end = self.finished_ms.unwrap_or(now_ms.max(self.last_clock_ms));
        let end = self.paused_at.map_or(end, |paused| end.min(paused));
        self.started_ms
            .map(|s| end.saturating_sub(s).saturating_sub(self.paused_ms))
            .unwrap_or(0)
    }
    pub fn live_stats(&self, now_ms: u64) -> LiveStats {
        let st = self.stats.finalize(&self.comparer, self.elapsed_ms(now_ms));
        let in_para = !self.recorded;
        // 整场口径 == aggregate()（<5 硬门后的 RaceSummary）+ 当前段实时 stats，
        // 与整场赛报同源、不重复实现：done_* 来自 recount()→aggregate()，
        // 回改同样只累计过硬门的已结算段，避免短段污染整场键准。
        let chars = self.done_chars + if in_para { st.chars as u64 } else { 0 };
        let keys = self.done_keys + if in_para { st.keys as u64 } else { 0 };
        let bs = self.done_bs + if in_para { st.backspaces as u64 } else { 0 };
        let corr: u64 = self
            .results
            .iter()
            .flatten()
            .filter(|r| r.chars >= VALID_MIN_CHARS)
            .map(|r| r.corrections as u64)
            .sum::<u64>()
            + if in_para { st.corrections as u64 } else { 0 };
        let ms = self.done_ms + if in_para { st.elapsed_ms } else { 0 };
        let seconds = ms as f64 / 1000.0;
        let k = keys as f64;
        let c = chars as f64;
        let cr = corr as f64;
        LiveStats {
            total_speed: if ms > 0 { c * 60000.0 / ms as f64 } else { 0.0 },
            para_speed: st.speed,
            para_accuracy: st.accuracy_pro,
            para_keys_per_sec: st.strokes_per_sec,
            para_code_len: st.code_len,
            accuracy: if k > 0.0 && c + cr > 0.0 {
                ((k - cr - bs as f64 * 2.0) / (c + cr) * c / k * 100.0).clamp(0.0, 100.0)
            } else {
                100.0
            },
            keys_per_sec: if seconds > 0.0 { k / seconds } else { 0.0 },
            code_len: if c > 0.0 { k / c } else { 0.0 },
            done_chars: chars,
        }
    }

    /// 面板通道（P1-4）：secondary_panels::draw 拿不到墙钟参数——用最近一次
    /// 事件时钟（last_clock_ms）冻结的整场实时口径（打开面板瞬间的读数）。
    pub fn live_stats_now(&self) -> LiveStats {
        self.live_stats(self.last_clock_ms)
    }
    /// Serialize through a bounded writer, before allocation can exceed the limit.
    pub fn snapshot_bytes_bounded(&self) -> std::io::Result<Vec<u8>> {
        use std::io::Write;
        struct Bounded(Vec<u8>);
        impl Write for Bounded {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.0.len().saturating_add(bytes.len()) > 8 * 1024 * 1024 {
                    return Err(std::io::Error::other("恢复快照超过 8 MiB"));
                }
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = Bounded(Vec::new());
        serde_json::to_writer(&mut writer, self).map_err(std::io::Error::other)?;
        writer.0.push(b'\n');
        Ok(writer.0)
    }
    /// Bounded journal, with a synced recoverable previous generation on rotation.
    pub fn save_snapshot(&self, path: &std::path::Path) -> std::io::Result<()> {
        let _guard = SNAPSHOT_IO
            .lock()
            .map_err(|_| std::io::Error::other("恢复记录写入锁异常"))?;
        let bytes = self.snapshot_bytes_bounded()?;
        let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if len.saturating_add(bytes.len() as u64) > 16 * 1024 * 1024 {
            gale_core::score::durable_replace(path, &bytes)
        } else {
            gale_core::score::append_json_line(path, self)
        }
    }
    fn valid_snapshot(&self) -> bool {
        let n = self.comparer.text.chars().count();
        if self.segments.is_empty()
            || self.segments.len() > 10000
            || self.index >= self.segments.len()
            || self.results.len() != self.segments.len()
            || self.raw_segments.len() != self.segments.len()
            || n > 100000
            || self.comparer.states.len() != n
            || self.comparer.pos > n
            || self.comparer.typed.len() != self.comparer.pos
            || self.comparer.text != self.segments[self.index].text
            || self.segments.iter().any(|s| s.text.contains('\r'))
            || self
                .word_attribution
                .as_ref()
                .is_some_and(|a| !a.is_valid_for(self.comparer.pos))
            || self.text_attempts.len() > 100000
            || self.text_attempts.keys().any(|hash| hash.len() > 128)
        {
            return false;
        }
        let expected: Vec<char> = self.comparer.text.chars().collect();
        let mut relaxed = 0;
        for (i, state) in self.comparer.states.iter().enumerate() {
            use gale_core::compare::CharState;
            if i >= self.comparer.pos {
                if *state != CharState::Pending {
                    return false;
                }
                continue;
            }
            let typed = self.comparer.typed[i];
            let correct = self.comparer.quote_rules.equivalent(typed, expected[i]);
            if *state
                != if correct {
                    CharState::Correct
                } else {
                    CharState::Wrong
                }
            {
                return false;
            }
            relaxed += usize::from(correct && typed != expected[i]);
        }
        if relaxed != self.comparer.quote_relaxed {
            return false;
        }
        if self.comparer.wrong
            != self
                .comparer
                .states
                .iter()
                .filter(|s| **s == gale_core::compare::CharState::Wrong)
                .count()
        {
            return false;
        }
        if self
            .segments
            .iter()
            .chain(self.raw_segments.iter())
            .any(|s| s.text.chars().count() > 100000)
        {
            return false;
        }
        if self
            .started_ms
            .zip(self.finished_ms)
            .is_some_and(|(a, b)| b < a)
        {
            return false;
        }
        if self.phase == Phase::Typing && (self.started_ms.is_none() || self.frozen_rules.is_none())
        {
            return false;
        }
        if self.rules.clone().normalized() != self.rules
            || self
                .frozen_rules
                .as_ref()
                .is_some_and(|r| r.clone().normalized() != *r)
        {
            return false;
        }
        for (idx, result) in self.results.iter().enumerate() {
            if let Some(r) = result {
                if r.para_index != idx
                    || [
                        r.speed,
                        r.accuracy,
                        r.code_len,
                        r.effective_speed,
                        r.strokes_per_sec,
                        r.accuracy_pro,
                        r.jj,
                    ]
                    .iter()
                    .any(|v| !v.is_finite())
                    || r.word_rate
                        .is_some_and(|v| !v.is_finite() || !(0. ..=100.).contains(&v))
                    || r.difficulty.as_ref().is_some_and(|d| {
                        d.label.chars().count() > 32 || d.score.is_some_and(|v| !v.is_finite())
                    })
                {
                    return false;
                }
            }
        }
        if let Some(st) = &self.last_stats {
            if [
                st.speed,
                st.accuracy_simple,
                st.accuracy_pro,
                st.code_len,
                st.effective_speed,
                st.strokes_per_sec,
                st.jj,
                st.zz_speed,
                st.zz_speed2,
                st.zz_mc,
            ]
            .iter()
            .any(|v| !v.is_finite())
            {
                return false;
            }
        }
        true
    }
    pub fn restore_snapshot(path: &std::path::Path) -> Result<Self, String> {
        use std::io::{BufRead, BufReader, Read};
        let mut last = None;
        for candidate in [path.to_path_buf(), gale_core::score::backup_path(path)] {
            let Ok(file) = std::fs::File::open(&candidate) else {
                continue;
            };
            if file.metadata().map_err(|e| e.to_string())?.len() > 16 * 1024 * 1024 {
                continue;
            }
            // Bound the reader before lines can allocate. Even a file changed during
            // the read cannot exceed this process's restore budget.
            let reader = BufReader::new(file).take(16 * 1024 * 1024 + 1);
            for line in reader.split(b'\n') {
                let line = line.map_err(|e| e.to_string())?;
                if line.len() > 8 * 1024 * 1024 {
                    continue;
                }
                if let Ok(s) = serde_json::from_slice::<Self>(&line) {
                    if s.valid_snapshot() {
                        last = Some(s);
                    }
                }
            }
            if last.is_some() {
                break;
            }
        }
        let mut s = last.ok_or("没有可恢复的完整记录")?;
        // A new process has a new monotonic epoch: resume the article, never manufacture
        // continuation time across shutdown. Finished result slots remain intact.
        if s.phase != Phase::Done {
            s.reset_current();
        }
        s.recount();
        Ok(s)
    }
}
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveStats {
    /// 整场速度（字/分）：已结算段（过 <5 硬门）+ 当前段实时，不重复计数。
    /// 口径与 aggregate()/整场赛报同源（总量比，非段均）。UI 接线待接（P1-4）。
    pub total_speed: f64,
    /// Current segment only, including its final values until the next reset.
    pub para_speed: f64,
    pub para_accuracy: f64,
    pub para_keys_per_sec: f64,
    pub para_code_len: f64,
    /// 整场键准 %（晴 Pro 精确式，总量比）。当前段用 para_accuracy（已接线：secondary_panels 结算/赛报面板）。
    pub accuracy: f64,
    /// 整场击键（键/秒）：总键数 / 总用时秒。UI 接线待接（P1-4）。
    pub keys_per_sec: f64,
    /// 整场码长（键/字）：总键数 / 总字数。UI 接线待接（P1-4）。
    pub code_len: f64,
    /// 整场累计字数：已结算段（≥5 字硬门口径）+ 当前段进行中实时计入。
    /// is_valid=false 的段也计入这里（练习总量）；"有效均速"类指标由 UI 按
    /// ScoreRecord.is_valid 另行拆分（已接线：secondary_panels）。
    pub done_chars: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_startup_ignores_input_and_can_restart_without_recording() {
        let mut s = Session::new(100);
        assert!(!s.is_segment_done());
        s.set_preedit("a".into());
        s.set_composing(true);
        s.on_key(0x41, 100);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            200,
        );
        s.feed_input(InputEvent::Backspace, 300);
        assert!(s.feed_paste("乙", 400).is_err());
        s.stop_at(500);
        s.jump(0);
        assert!(s.title.is_empty());
        assert!(s.current_segment().text.is_empty());
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(s.stats.keys, 0);
        assert!(s.started_ms.is_none());
        assert!(s.preedit.is_empty() && !s.composing);
        assert!(s.finalize_current_at(600, 1).is_none());
        assert!(s.snapshot_bytes_bounded().is_ok());
    }
    fn session() -> Session {
        let mut s = Session::new(100);
        // 5 字段保证结算记录过 <5 硬门，可进整场聚合（2 字用例见 validity_gate_*）
        s.load_group(ParsedGroupText {
            title: "test".into(),
            difficulty: None,
            segments: vec![
                Segment {
                    mark: "1".into(),
                    text: "甲乙丙丁戊".into(),
                },
                Segment {
                    mark: "2".into(),
                    text: "己庚辛壬癸".into(),
                },
            ],
        });
        s
    }
    #[test]
    fn clipboard_and_group_load_share_article_metadata_cleanup() {
        for newline in ["\n", "\r\n", "\r"] {
            for footer in [
                "-----第2-277101段-晴发文",
                "-----第2段-----",
                "—----第2段----—",
            ] {
                let raw = ["[普(1.84)]文章标题 [字数6]", " 正文。 ", footer].join(newline);
                let mut s = Session::new(1);
                s.load_text(&raw);
                assert_eq!(s.source, SessionSource::Group, "{raw:?}");
                assert_eq!(s.title, "文章标题");
                assert_eq!(s.current_segment().text, " 正文。 ");
                assert_eq!(s.total_segments(), 1, "group segments are not re-split");
                assert!(s.difficulty.is_some());
                assert_eq!(s.raw_segments[0].text, " 正文。 ");
            }
        }
    }

    #[test]
    fn clipboard_plain_body_keeps_content_and_smart_segmentation() {
        let raw = "《这是正文》\r\n  正文空格保留。\r\n\r\n下一行。";
        let mut s = Session::new(5);
        s.load_text(raw);
        assert_eq!(s.title, "自定义文字");
        assert_eq!(s.source, SessionSource::Library("自定义文字".into()));
        assert!(s.difficulty.is_none());
        assert!(s.total_segments() > 1);
        assert_eq!(
            s.segments
                .iter()
                .map(|seg| seg.text.as_str())
                .collect::<String>(),
            normalize_newlines(raw)
        );
    }
    /// 逐字一键 + 末尾整段 commit：首键开表（started=start），elapsed = end−start。
    fn round(s: &mut Session, start: u64, end: u64, text: &str) {
        for (i, _) in text.chars().enumerate() {
            s.on_key(0x41 + i as u32 % 26, start + i as u64 * 100);
        }
        s.feed_input(InputEvent::Commit { chars: text.into() }, end);
    }
    #[test]
    fn single_drill_keeps_all_groups_and_can_advance() {
        let mut sess = Session::new(100);
        let items: Vec<String> = "一二三四五六七八九十甲乙丙丁戊己庚辛壬癸子"
            .chars()
            .map(|c| c.to_string())
            .collect();
        sess.load_single(SingleSession {
            title: "test".into(),
            group_size: 10,
            items,
            group_index: 0,
            seed: 0,
        });
        assert_eq!(sess.total_segments(), 3);
        assert_eq!(sess.current_segment().text, "一二三四五六七八九十");
        sess.next_segment();
        assert_eq!(sess.current_segment().text, "甲乙丙丁戊己庚辛壬癸");
        sess.next_segment();
        assert_eq!(sess.current_segment().text, "子");
    }

    #[test]
    fn first_physical_key_and_single_sentence_commit() {
        let mut s = session();
        round(&mut s, 100, 1100, "甲乙");
        let r = s.finalize_current_at(1100, 1234567890).unwrap();
        assert_eq!(r.time_ms, 1000);
        assert_eq!(r.chars, 2);
        assert_eq!(r.keys, 2);
        assert_eq!(r.commit_groups, 1);
        assert_eq!(s.stats.commit_attributions[0].keys, 2);
        assert_eq!(r.ts, 1234567890);
        // 2 字段 <5 硬门：记录在（原始保留），但不进整场聚合
        assert_eq!(s.live_stats(1100).done_chars, 0);
        assert!(s.finalize_current(1200).is_none());
    }
    #[test]
    fn current_segment_metrics_do_not_inherit_previous_segments() {
        // 5 字段（≥5 硬门）保证已结算段进整场统计；2 字段见 validity_gate_* 测试
        let mut s = session();
        round(&mut s, 100, 1100, "甲乙丙丁戊");
        s.finalize_current_at(1100, 1).unwrap();
        s.next_segment();

        let idle = s.live_stats(1500);
        assert_eq!(idle.para_speed, 0.);
        assert_eq!(idle.para_accuracy, 100.);
        assert_eq!(idle.para_keys_per_sec, 0.);
        assert_eq!(idle.para_code_len, 0.);
        // 第 1 段：5 字 / 1s / 5 键
        assert_eq!(idle.total_speed, 300.);
        assert_eq!(idle.keys_per_sec, 5.);
        assert_eq!(idle.code_len, 1.);

        s.on_key(0x41, 2000);
        s.feed_input(
            InputEvent::Commit {
                chars: "己".into()
            },
            2500,
        );
        s.on_key(0x08, 3000);
        s.feed_input(InputEvent::Backspace, 3000);
        s.on_key(0x42, 4000);
        s.feed_input(
            InputEvent::Commit {
                chars: "己".into()
            },
            4500,
        );
        s.on_key(0x43, 5000);
        s.feed_input(
            InputEvent::Commit {
                chars: "庚".into()
            },
            6000,
        );

        // 当前段（2 字 / 4s / 4 键 / 回改1）；整场 = 第1段 + 当前段（7字 / 5s / 9键 / 回改1）
        let before = s.live_stats(6000);
        assert_eq!(before.para_speed, 30.);
        assert_eq!(before.para_accuracy, 50.);
        assert_eq!(before.para_keys_per_sec, 1.);
        assert_eq!(before.para_code_len, 2.);
        assert_eq!(before.total_speed, 84.);
        assert!((before.accuracy - 700. / 9.).abs() < 1e-9);
        assert_eq!(before.keys_per_sec, 1.8);
        assert!((before.code_len - 9. / 7.).abs() < 1e-9);

        let record = s.finalize_current_at(6000, 2).unwrap();
        let after = s.live_stats(10000);
        assert_eq!(after.para_accuracy, record.accuracy);
        assert_eq!(after.para_keys_per_sec, record.strokes_per_sec);
        assert_eq!(after.para_code_len, record.code_len);
        // 本段仅 2 字，<5 硬门把它从整场剔除：整场回到仅第 1 段（口径 == aggregate()）
        assert_eq!(after.total_speed, 300.);
        assert_eq!(after.accuracy, 100.);
        assert_eq!(after.keys_per_sec, 5.);
        assert_eq!(after.code_len, 1.);
        assert_eq!(s.aggregate().chars, 5);

        s.jump(1);
        let retry = s.live_stats(11000);
        assert_eq!(retry.para_speed, 0.);
        assert_eq!(retry.para_accuracy, 100.);
        assert_eq!(retry.para_keys_per_sec, 0.);
        assert_eq!(retry.para_code_len, 0.);
        assert_eq!(retry.total_speed, 300.);
        assert_eq!(retry.keys_per_sec, 5.);
        assert_eq!(retry.code_len, 1.);
    }
    #[test]
    fn current_keystroke_rate_advances_before_first_commit() {
        let mut s = session();
        round(&mut s, 100, 1100, "甲乙丙丁戊");
        s.finalize_current_at(1100, 1).unwrap();
        s.next_segment();
        s.on_key(0x41, 2000);
        let live = s.live_stats(4000);
        assert_eq!(s.comparer.pos, 0);
        assert_eq!(s.elapsed_ms(4000), 2000);
        assert_eq!(live.para_speed, 0.);
        assert_eq!(live.para_keys_per_sec, 0.5);
        assert_eq!(live.para_code_len, 0.);
        // 整场 = 第1段（5字/1s/5键）+ 当前段（1键/2s）：5字/3s、6键/3s、6/5
        assert_eq!(live.total_speed, 100.);
        assert_eq!(live.keys_per_sec, 2.);
        assert!((live.code_len - 1.2).abs() < 1e-9);
    }
    #[test]
    fn cross_segment_retries_replace_only_their_slot() {
        let mut s = session();
        round(&mut s, 100, 1100, "甲乙丙丁戊");
        s.finalize_current(1100).unwrap();
        s.next_segment();
        round(&mut s, 2000, 4000, "己庚辛壬癸");
        s.finalize_current(4000).unwrap();
        assert_eq!(s.aggregate().chars, 10);
        assert_eq!(s.aggregate().time_ms, 3000);
        s.jump(0);
        assert_eq!(s.aggregate().chars, 5);
        round(&mut s, 5000, 7000, "甲乙丙丁戊");
        s.finalize_current(7000).unwrap();
        assert_eq!(s.aggregate().chars, 10);
        assert_eq!(s.aggregate().time_ms, 4000);
        assert!((s.aggregate().speed - 150.0).abs() < 1e-9);
    }

    #[test]
    fn word_rate_tracks_accepted_commits_and_text_corrections() {
        let mut s = Session::new(100);
        s.load_book("t", "你好我呀末");
        s.on_key(0x41, 100);
        s.feed_input(
            InputEvent::Commit {
                chars: "你好，".into(),
            },
            200,
        );
        s.on_key(0x08, 300);
        s.feed_input(InputEvent::Backspace, 300); // remove the wrong punctuation
        s.on_key(0x42, 400);
        s.feed_input(
            InputEvent::Commit {
                chars: "我".into()
            },
            400,
        );
        s.on_key(0x43, 500);
        s.feed_input(
            InputEvent::Commit {
                chars: "呀".into()
            },
            500,
        );
        s.on_key(0x44, 600);
        s.feed_input(
            InputEvent::Commit {
                chars: "末多余".into(),
            },
            600,
        );
        let r = s.finalize_current_at(600, 1).unwrap();
        assert_eq!(r.chars, 5);
        assert_eq!(r.wrong, 0);
        assert_eq!(r.word_rate, Some(40.)); // extra text beyond the target is not counted
    }

    #[test]
    fn retype_counts_follow_text_across_segments_reload_and_recovery() {
        let mut s = session();
        round(&mut s, 100, 1100, "甲乙丙丁戊");
        let a = s.finalize_current_at(1100, 1).unwrap();
        assert_eq!(a.retype_count, 0);
        s.next_segment();
        round(&mut s, 2000, 3000, "己庚辛壬癸");
        let b = s.finalize_current_at(3000, 2).unwrap();
        assert_eq!(b.retype_count, 0);
        s.jump(0);
        s.on_key(0x41, 4000); // an abandoned attempt still counts as a start
        s.jump(0);
        round(&mut s, 5000, 6000, "甲乙丙丁戊");
        let c = s.finalize_current_at(6000, 3).unwrap();
        assert_eq!(c.retype_count, 2);
        s.load_book("new title, same text", "甲乙丙丁戊");
        round(&mut s, 7000, 8000, "甲乙丙丁戊");
        let d = s.finalize_current_at(8000, 4).unwrap();
        assert_eq!(d.retype_count, 3);
        let mut recovered: Session =
            serde_json::from_slice(&s.snapshot_bytes_bounded().unwrap()).unwrap();
        recovered.jump(0);
        recovered.seed_retype_history([&a, &b, &c, &d]);
        round(&mut recovered, 9000, 10000, "甲乙丙丁戊");
        assert_eq!(
            recovered
                .finalize_current_at(10000, 5)
                .unwrap()
                .retype_count,
            4
        );
        // A fresh launch without a recovery file also learns the latest start count.
        let mut fresh = Session::new(100);
        fresh.load_book("t", "甲乙丙丁戊");
        fresh.seed_retype_history([&a, &b, &c, &d]);
        round(&mut fresh, 100, 1100, "甲乙丙丁戊");
        assert_eq!(fresh.finalize_current_at(1100, 6).unwrap().retype_count, 4);
    }

    /// P1-4 有效性门槛边界（is_valid 双门槛 + <5 聚合硬门并存，语义分开）：
    /// 2字/4字 → 硬门挡出聚合且 is_valid=false；9字9s/9字10s/10字9s → 进聚合但
    /// is_valid=false（进练习字数、不进有效均速）；10字10s → is_valid=true。
    #[test]
    fn validity_gate_boundaries_mark_records_and_gate_aggregate() {
        let cases: &[(&str, u64, bool, u64)] = &[
            ("甲乙", 1_000, false, 0),                  // 2字：硬门 + 无效
            ("甲乙丙丁", 1_000, false, 0),              // 4字：硬门 + 无效
            ("甲乙丙丁戊己庚辛壬", 9_000, false, 9),    // 9字9s：进聚合、无效
            ("甲乙丙丁戊己庚辛壬", 10_000, false, 9),   // 9字10s：仅字数门不过
            ("甲乙丙丁戊己庚辛壬癸", 9_999, false, 10), // 10字9.999s：仅用时门不过
            ("甲乙丙丁戊己庚辛壬癸", 10_000, true, 10), // 10字10s：双过
        ];
        for &(text, ms, valid, agg_chars) in cases {
            let mut s = Session::new(100);
            s.load_book("t", text);
            let end = 100 + ms;
            round(&mut s, 100, end, text);
            let r = s.finalize_current_at(end, 1).unwrap();
            assert_eq!(r.chars, text.chars().count(), "case {text}");
            assert_eq!(r.is_valid, valid, "is_valid {text}/{ms}ms");
            assert_eq!(s.aggregate().chars, agg_chars, "aggregate {text}/{ms}ms");
            assert_eq!(s.live_stats(end).done_chars, agg_chars);
        }
    }

    /// P1-3 词提快照：RoundMetadata.word_hints 冻结开打时的规则；
    /// 赛制经 normalized() 强制关词提；成绩单 badge 走 gale_core::score::race_badge。
    #[test]
    fn round_metadata_freezes_word_hints_and_badge_uses_snapshot() {
        // 练习 + 词提开：开打后规则冻结，成绩引用快照
        let mut s = Session::new(100);
        s.load_book("t", "甲乙丙丁戊");
        let mut rules = SessionRules::default();
        rules.word_hints = true;
        s.set_rules(rules).unwrap();
        round(&mut s, 100, 1100, "甲乙丙丁戊");
        let meta = s.finalize_current_at(1100, 1).unwrap().round.unwrap();
        assert!(meta.word_hints);
        assert_eq!(
            gale_core::score::race_badge(meta.mode, meta.word_hints),
            "【非赛制】[词提开]"
        );
        // 赛制：normalized() 强制 word_hints=false，快照与 badge 一致
        let mut s2 = Session::new(100);
        s2.load_book("t", "甲乙丙丁戊");
        let mut race = SessionRules::default();
        race.mode = TrainingMode::Race;
        race.word_hints = true; // 会被 normalized 关掉
        s2.set_rules(race).unwrap();
        assert!(!s2.rules.word_hints);
        round(&mut s2, 100, 1100, "甲乙丙丁戊");
        let m2 = s2.finalize_current_at(1100, 2).unwrap().round.unwrap();
        assert!(!m2.word_hints);
        assert_eq!(
            gale_core::score::race_badge(m2.mode, m2.word_hints),
            "【赛制】"
        );
    }

    /// 难度链路：群文《难度》头随 load_group 进 Session，结算随成绩进
    /// ScoreRecord.difficulty（成绩单 `难度普(1.84)`）。
    #[test]
    fn group_difficulty_flows_into_record() {
        let mut s = Session::new(100);
        s.load_group(ParsedGroupText {
            title: "t".into(),
            difficulty: Some("普 1.84".into()),
            segments: vec![Segment {
                mark: "1".into(),
                text: "甲乙丙丁戊".into(),
            }],
        });
        assert_eq!(
            s.difficulty,
            Some(DifficultyTag {
                label: "普".into(),
                score: Some(1.84)
            })
        );
        round(&mut s, 100, 1100, "甲乙丙丁戊");
        let r = s.finalize_current_at(1100, 1).unwrap();
        assert_eq!(r.difficulty.as_ref().unwrap().label, "普");
        assert!((r.difficulty.as_ref().unwrap().score.unwrap() - 1.84).abs() < 1e-9);
        assert!(gale_core::score::format_score_report(&r).contains("字数5  难度普(1.84)  用时"));
        // 换载文（无难度头）清空
        s.load_book("plain", "甲乙丙丁戊甲乙丙丁戊");
        assert!(s.difficulty.is_none());
    }
    #[test]
    fn partial_round_does_not_claim_untyped_chars() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            600,
        );
        s.stop_at(1100);
        let r = s.finalize_current(1100).unwrap();
        assert_eq!(r.chars, 1);
        assert_eq!(r.time_ms, 1000);
        assert!(!r.round.unwrap().complete);
    }
    #[test]
    fn rules_and_body_frozen_until_restart() {
        let mut s = session();
        s.on_key(0x41, 10);
        assert!(s.set_rules(SessionRules::default()).is_err());
        assert!(s.apply_cleaning(&CleaningPreset::default()).is_err());
        s.jump(0);
        let mut r = SessionRules::default();
        r.mode = TrainingMode::Race;
        s.set_rules(r).unwrap();
        assert!(!s.rules.word_hints);
        s.on_key(0x41, 100);
        s.set_paused(true, 200);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲乙".into(),
            },
            1100,
        );
        assert_eq!(s.elapsed_ms(1100), 1000);
        assert!(s.feed_paste("甲", 1200).is_err());
    }
    #[test]
    fn backward_clock_never_reduces_elapsed_and_quotes_are_correct() {
        let mut s = session();
        s.segments[0].text = "“甲”".into();
        s.raw_segments = s.segments.clone();
        s.reset_current();
        s.on_key(0x41, 100);
        s.on_key(0x41, 400);
        s.feed_input(
            InputEvent::Commit {
                chars: "”甲“".into(),
            },
            300,
        );
        assert_eq!(s.elapsed_ms(1), 300);
        assert_eq!(s.comparer.wrong, 0);
        assert_eq!(s.comparer.quote_relaxed, 2);
    }
    #[test]
    fn damaged_snapshot_tail_and_invalid_positions_do_not_restore() {
        let path = std::env::temp_dir().join(format!("hufu-session-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut s = session();
        round(&mut s, 100, 1100, "甲乙丙丁戊");
        s.finalize_current_at(1100, 123456).unwrap();
        s.save_snapshot(&path).unwrap();
        let mut bad = s.clone();
        bad.comparer.pos = 999;
        bad.save_snapshot(&path).unwrap();
        bad = s.clone();
        bad.comparer.pos = 1;
        bad.comparer.typed.pop(); // In-range cursor with an impossible completed suffix.
        bad.save_snapshot(&path).unwrap();
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"{\"bad\":").unwrap();
        drop(file);
        let restored = Session::restore_snapshot(&path).unwrap();
        assert_eq!(restored.comparer.pos, 5);
        assert_eq!(restored.aggregate().chars, 5);
        std::fs::remove_file(path).ok();
    }
    #[test]
    fn practice_pause_not_counted() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.set_paused(true, 200);
        s.set_paused(false, 1200);
        s.on_key(0x42, 1300);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲乙".into(),
            },
            2100,
        );
        assert_eq!(s.elapsed_ms(2100), 1000);
        assert_eq!(s.comparer.timing_events[0].dt_ms, 1000);
        assert_eq!(s.stats.per_key[0].1, 200);

        let mut partial = session();
        partial.on_key(0x41, 100);
        partial.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            200,
        );
        partial.set_paused(true, 300);
        partial.stop_at(1300);
        assert_eq!(partial.elapsed_ms(1500), 200);
        assert_eq!(partial.finalize_current_at(1500, 123).unwrap().time_ms, 200);
    }

    #[test]
    fn raw_newlines_survive_while_working_coordinates_and_hashes_match() {
        let mut s = Session::new(100);
        let raw = "甲\r\n\r乙  ";
        s.load_book("raw", raw);
        assert_eq!(s.raw_segments[0].text, raw);
        assert_eq!(s.current_segment().text, "甲\n\n乙  ");
        assert!(!s.cleaned);
        round(&mut s, 100, 1100, "甲\r\n\r乙  ");
        let r = s.finalize_current_at(1100, 123).unwrap();
        assert_eq!(r.chars, 6);
        assert_eq!(r.wrong, 0);
        let meta = r.round.unwrap();
        assert_eq!(meta.text_hash, fingerprint("甲\n\n乙  ".as_bytes()));
        assert_eq!(meta.raw_text_hash, fingerprint(raw.as_bytes()));
        assert_ne!(meta.text_hash, meta.raw_text_hash);
        s.jump(0);
        s.restore_raw().unwrap();
        assert_eq!(s.current_segment().text, "甲\n\n乙  ");
        assert_eq!(s.raw_segments[0].text, raw);
        assert!(s.valid_snapshot());
    }

    #[test]
    fn host_late_start_cancel_then_new_key_keeps_batches_separate() {
        let mut s = session();
        s.on_key(0x41, 100); // The key that starts composition is initially idle.
        s.set_composing(true);
        s.set_preedit("ab".into());
        s.on_key(0x42, 200);
        s.set_composing(false); // END alone never means cancellation.
        assert_eq!(s.stats.waste_codes, 0);
        s.cancel_composition(); // Stable no-commit snapshot plus explicit physical cancel.
        assert_eq!(s.stats.waste_keys, 2);
        assert_eq!(s.stats.composing_keys, 2);
        s.on_key(0x43, 300); // Delivered after Cancelled by the host.
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            400,
        );
        assert_eq!(s.stats.commit_attributions[0].keys, 1);
        assert_eq!(s.stats.keys, 3);
        assert_eq!(s.stats.finalize(&s.comparer, 300).backspaces, 2);
    }

    #[test]
    fn cancelling_with_backspace_does_not_charge_the_same_key_twice() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.set_composing(true);
        s.on_key(0x08, 200);
        s.set_composing(false);
        s.cancel_composition();
        assert_eq!(s.stats.keys, 2);
        assert_eq!(s.stats.waste_keys, 2);
        assert_eq!(s.stats.finalize(&s.comparer, 100).backspaces, 2);
        s.on_key(0x42, 300);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            400,
        );
        assert_eq!(s.stats.commit_attributions[0].keys, 1);
    }

    #[test]
    fn committed_and_no_change_backspaces_are_excluded_from_new_commit() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            200,
        );
        s.on_key(0x08, 300);
        s.feed_input(InputEvent::Backspace, 300);
        s.on_key(0x08, 400);
        s.feed_input(InputEvent::Backspace, 400); // Already empty: no body diff.
        s.on_key(0x42, 500);
        s.set_composing(true);
        s.cancel_composition();
        assert_eq!(s.stats.waste_keys, 1);
        s.on_key(0x43, 600);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            700,
        );
        assert_eq!(s.stats.commit_attributions[1].keys, 1);
        assert_eq!(s.comparer.corrections, 1);
        assert_eq!(s.stats.keys, 5);
    }

    #[test]
    fn end_before_commit_and_top_code_continuation_keep_real_batches() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.set_composing(true);
        s.on_key(0x42, 200);
        s.set_composing(false); // A normal END may precede its commit.
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            300,
        );
        assert_eq!(s.stats.waste_codes, 0);
        s.set_composing(true); // Host restores the still-active next preedit.
        s.set_preedit("c".into());
        assert!(s.composing);
        assert_eq!(s.preedit, "c");
        s.on_key(0x43, 400);
        s.set_composing(false);
        s.feed_input(
            InputEvent::Commit {
                chars: "乙".into()
            },
            500,
        );
        assert_eq!(
            s.stats
                .commit_attributions
                .iter()
                .map(|c| c.keys)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(s.stats.keys, 3);
        assert_eq!(s.stats.composing_keys, 3);
        assert!(s.preedit.is_empty());
    }

    #[test]
    fn focus_ascii_commit_uses_only_physical_keys_without_composition() {
        let mut s = Session::new(100);
        s.load_book("ASCII", "ab");
        s.on_key(0x41, 100);
        s.feed_input(InputEvent::Commit { chars: "a".into() }, 200);
        s.set_paused(true, 200); // Focus-loss flush completes before pausing the round.
        s.set_paused(false, 1200);
        s.on_key(0x42, 1300);
        s.feed_input(InputEvent::Commit { chars: "b".into() }, 1400);
        assert_eq!(s.stats.composing_keys, 0);
        assert_eq!(s.stats.waste_codes, 0);
        assert_eq!(
            s.stats
                .commit_attributions
                .iter()
                .map(|c| c.keys)
                .collect::<Vec<_>>(),
            vec![1, 1]
        );
        assert_eq!(s.elapsed_ms(1400), 300);
    }

    /// P1-6 protect 窗端到端：组字中退格（ImeDelete→组字期退格账）→ 组合被
    /// 该退格删空（非提交清空，protect 置位）→ 下一个退格仍归 IME（protect
    /// 只护一次）→ 弹起后再退格才走 Correction（回改账，Comparer 记 corrections）。
    /// 语义与 gale-win input_host BackspaceRouter 13 用例回归一致。
    #[test]
    fn backspace_protect_window_routes_ime_delete_then_correction() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            150,
        );
        assert_eq!(s.comparer.pos, 1);
        // 第二字组字中
        s.set_composing(true);
        s.on_key(0x42, 200);
        // 组字中的退格：ImeDelete → 组字期退格账，不动跟打区
        s.on_key(0x08, 250);
        assert_eq!(s.stats.composition_backspaces, 1);
        assert_eq!(s.comparer.corrections, 0);
        // 该退格把组合删空：Composing(false) 同批无 Commit → protect 置位
        s.set_composing_from_host(false, false);
        // 下一个物理退格：protect 窗内仍 ImeDelete（清 IME 残迹）
        s.on_key(0x08, 300);
        assert_eq!(s.stats.composition_backspaces, 2);
        assert_eq!(s.comparer.corrections, 0);
        assert_eq!(s.comparer.pos, 1);
        // 弹起清 protect；再退格 → Correction：进回改账（删掉已上屏的“甲”）
        s.on_backspace_up();
        s.on_key(0x08, 350);
        assert_eq!(s.stats.composition_backspaces, 2);
        s.feed_input(InputEvent::Backspace, 360);
        assert_eq!(s.comparer.corrections, 1);
        assert_eq!(s.comparer.pos, 0);
    }

    /// P1-6 反例：提交清空组合（Composing(false) 与 Commit 同批）**不**置
    /// protect——紧接着的退格直接 Correction，与“退格删空”严格区分。
    #[test]
    fn commit_clear_does_not_arm_protect_window() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.set_composing(true);
        s.on_key(0x42, 150);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            200,
        );
        // feed_input 的 Commit 分支已通知 router(false, true)；主循环的
        // Composing(false) 同批补一次 (false, true) 幂等无害
        s.set_composing_from_host(false, true);
        s.on_key(0x08, 250);
        assert_eq!(s.stats.composition_backspaces, 0, "提交后退格不进组字账");
        s.feed_input(InputEvent::Backspace, 260);
        assert_eq!(s.comparer.corrections, 1);
    }

    /// P1-3 选重/标顶：Ding×2 + Select×1（Unknown 忽略）→ 结算记录计数
    /// ding=2/select=1，成绩单输出 `选重1  标顶2`。
    #[test]
    fn commit_causes_count_into_record_and_report() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.on_commit_cause(CommitCause::Ding);
        s.on_commit_cause(CommitCause::Ding);
        s.on_commit_cause(CommitCause::Select);
        s.on_commit_cause(CommitCause::Unknown);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲乙丙丁戊".into(),
            },
            200,
        );
        let r = s.finalize_current_at(300, 123).unwrap();
        assert_eq!(r.ding_count, 2);
        assert_eq!(r.select_count, 1);
        assert!(
            gale_core::score::format_score_report(&r).contains("选重1  标顶2"),
            "{}",
            gale_core::score::format_score_report(&r)
        );
    }

    #[test]
    fn final_selection_and_proven_modifier_key_are_recorded_once() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.on_key(0xA0, 150); // ordinary Shift is excluded until a commit proves selection
        assert_eq!(s.stats.keys, 1);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲乙丙丁戊".into(),
            },
            200,
        );
        assert_eq!(s.phase, Phase::Done);
        s.on_commit_cause(CommitCause::SelectExtra {
            vk: 0xA0,
            lag_ms: 5,
        });
        s.on_commit_cause(CommitCause::Ding);
        assert_eq!(s.stats.commit_attributions[0].keys, 2);
        let r = s.finalize_current_at(200, 123).unwrap();
        assert_eq!(r.keys, 2);
        assert_eq!(r.code_len, 0.4);
        assert_eq!(r.select_count, 1);
        assert_eq!(r.ding_count, 1);
        s.on_commit_cause(CommitCause::SelectExtra {
            vk: 0x14,
            lag_ms: 0,
        });
        assert_eq!(s.stats.keys, 2, "finalized records reject late attribution");
        assert_eq!(s.stats.select_count, 1);

        s.jump(0);
        s.on_key(0x41, 300);
        s.on_key(0xBA, 400);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲乙丙丁戊".into(),
            },
            400,
        );
        s.on_commit_cause(CommitCause::Select);
        let ordinary = s.finalize_current_at(400, 124).unwrap();
        assert_eq!(
            ordinary.keys, 2,
            "an ordinary selection key was already counted"
        );
        assert_eq!(ordinary.select_count, 1);
    }

    #[test]
    fn candidate_choices_count_before_delayed_sentence_commit_and_keep_each_choice() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.set_composing(true);
        s.on_key(0xBA, 200);
        s.on_candidate_selection(Some(0xBA), 200);
        s.on_key(0xDE, 500);
        s.on_candidate_selection(Some(0xDE), 500);
        s.on_candidate_selection(None, 900); // a proven mouse choice invents no key
        assert_eq!(s.stats.select_count, 3);
        assert_eq!(s.stats.keys, 3);
        assert!(s.stats.commit_attributions.is_empty());
        assert_eq!(s.comparer.pos, 0);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲乙丙丁戊".into(),
            },
            5_000,
        );
        let r = s.finalize_current_at(5_000, 123).unwrap();
        assert_eq!(r.select_count, 3);
        assert_eq!(r.keys, 3);
        assert_eq!(s.stats.commit_attributions[0].keys, 3);
    }

    #[test]
    fn candidate_modifier_selection_stays_pending_and_last_character_is_counted() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            200,
        );
        s.on_key(0x42, 300);
        s.set_composing(true);
        s.on_key(0xA0, 400); // ignored by ordinary physical-key counting
        s.on_candidate_selection(Some(0xA0), 400);
        assert_eq!(s.stats.select_count, 1);
        assert_eq!(s.stats.keys, 3);
        assert_eq!(s.stats.commit_attributions[0].keys, 1);
        assert_eq!(s.stats.attributed_keys, 1);
        s.feed_input(
            InputEvent::Commit {
                chars: "乙丙丁戊".into(),
            },
            2_000,
        );
        assert_eq!(s.phase, Phase::Done);
        assert_eq!(s.stats.commit_attributions[1].keys, 2);
        s.on_candidate_selection(Some(0x14), 2_010);
        assert_eq!(
            s.stats.select_count, 1,
            "finished text rejects later choices"
        );
        let r = s.finalize_current_at(2_000, 123).unwrap();
        assert_eq!(r.select_count, 1);
        assert_eq!(r.keys, 3);
        s.on_candidate_selection(Some(0x14), 2_100);
        assert_eq!(s.stats.select_count, 1);
        assert_eq!(s.stats.keys, 3);
    }

    #[test]
    fn cancelled_sentence_keeps_actual_choices_and_does_not_charge_previous_output() {
        let mut s = session();
        s.on_key(0x41, 100);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            200,
        );
        s.on_key(0x42, 300);
        s.set_composing(true);
        s.on_candidate_selection(Some(0x14), 400);
        s.cancel_composition();
        assert_eq!(
            s.stats.select_count, 1,
            "the lock was an actual choice even if later cancelled"
        );
        assert_eq!(s.stats.waste_keys, 2);
        assert_eq!(s.stats.commit_attributions[0].keys, 1);
        s.on_key(0x43, 600);
        s.feed_input(
            InputEvent::Commit {
                chars: "乙丙丁戊".into(),
            },
            700,
        );
        assert_eq!(s.stats.commit_attributions[1].keys, 1);
        assert_eq!(s.finalize_current_at(700, 123).unwrap().select_count, 1);
    }

    #[test]
    fn candidate_selection_ignores_empty_paused_and_recorded_rounds() {
        let mut empty = Session::new(100);
        empty.on_candidate_selection(Some(0x14), 100);
        assert_eq!(empty.phase, Phase::Idle);
        assert!(empty.started_ms.is_none());
        assert_eq!(empty.stats.keys, 0);
        assert_eq!(empty.stats.select_count, 0);

        let mut s = session();
        s.rules.pause_on_blur = true;
        s.on_key(0x41, 100);
        s.set_paused(true, 200);
        s.on_candidate_selection(Some(0x14), 500);
        assert_eq!(s.stats.keys, 1);
        assert_eq!(s.stats.select_count, 0);
        s.set_paused(false, 1_200);
        s.on_candidate_selection(Some(0x14), 1_300);
        assert_eq!(s.stats.keys, 2);
        assert_eq!(s.stats.select_count, 1);
        s.feed_input(
            InputEvent::Commit {
                chars: "甲".into()
            },
            1_400,
        );
        s.finalize_current_at(1_500, 123).unwrap();
        s.on_candidate_selection(Some(0x14), 1_600);
        assert_eq!(s.stats.keys, 2);
        assert_eq!(s.stats.select_count, 1);
    }

    /// 整改12：直方图随段清零、结算冻结进 last_vk_histogram、换段保留快照。
    #[test]
    fn vk_histogram_clears_on_reset_and_freezes_on_finalize() {
        let mut s = session();
        *s.vk_histogram.entry(0x41).or_insert(0) += 2;
        *s.vk_histogram.entry(0x08).or_insert(0) += 1;
        round(&mut s, 100, 1100, "甲乙丙丁戊");
        s.finalize_current_at(1100, 123).unwrap();
        assert_eq!(s.last_vk_histogram.get(&0x41), Some(&2));
        assert_eq!(s.last_vk_histogram.get(&0x08), Some(&1));
        s.next_segment(); // reset_current：清本段、留快照
        assert!(s.vk_histogram.is_empty());
        assert_eq!(s.last_vk_histogram.get(&0x41), Some(&2));
    }

    /// 整改12/13：新文章载入清空热力图快照与全篇分析；快照序列化跳过新字段。
    #[test]
    fn new_article_clears_heatmap_snapshot_and_full_analysis() {
        let mut s = session();
        round(&mut s, 100, 1100, "甲乙丙丁戊");
        // 先有按键才有热力图：模拟本段打过 A 键（无键则快照为空，非空断言无意义）
        *s.vk_histogram.entry(0x41).or_insert(0) += 1;
        s.finalize_current_at(1100, 123).unwrap();
        assert!(!s.last_vk_histogram.is_empty());
        s.full_analysis = Some(crate::FullAnalysis::default());
        s.load_book("新书", "另一篇正文的完整内容，足够五字以上。");
        assert!(s.last_vk_histogram.is_empty());
        assert!(s.full_analysis.is_none());
        // serde(skip)：快照字节不含直方图/分析字段（旧快照可恢复）
        let bytes = s.snapshot_bytes_bounded().unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("vk_histogram"));
        assert!(!text.contains("full_analysis"));
    }
}
